//! `createvol.py` (`vmmCreateVolume`): the "Add a Storage Volume"
//! wizard, a new volume in a pool with its format, capacity, sparse
//! allocation and backing store, and the XML tab.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use virtinst_core::storage::{self, NewVolume, PoolXml, VolInstall};

use crate::asyncjob;
use crate::connection::VmmConnection;
use crate::devpages::EntryCombo;
use crate::error;
use crate::storagebrowse::{self, BrowseReason, StorageBrowser};
use crate::xmleditor::XmlEditor;

/// The new volume's pool and name, for `vol-created`.
type CreatedCallback = Box<dyn Fn(&str, &str)>;

fn start_label(text: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Label {
    let l = gtk::Label::with_mnemonic(text);
    l.set_halign(gtk::Align::Start);
    l.set_mnemonic_widget(Some(widget));
    l
}

pub struct Inner {
    conn: VmmConnection,
    window: gtk::Window,
    xmleditor: XmlEditor,
    parent_pool: RefCell<PoolXml>,
    name_hint: RefCell<Option<String>>,
    storage_browser: RefCell<Option<StorageBrowser>>,
    vol_name: gtk::Entry,
    vol_name_suffix: gtk::Label,
    vol_format: Rc<EntryCombo>,
    vol_format_label: gtk::Label,
    vol_parent_info: gtk::Label,
    vol_capacity: gtk::SpinButton,
    vol_nonsparse: gtk::CheckButton,
    backing_expander: gtk::Expander,
    backing_store: gtk::Entry,
    vol_create: gtk::Button,
    vol_created: RefCell<Option<CreatedCallback>>,
}

#[derive(Clone)]
pub struct CreateVolume(Rc<Inner>);

impl CreateVolume {
    pub fn new(conn: &VmmConnection, parent_pool: PoolXml) -> CreateVolume {
        let window = gtk::Window::builder()
            .title("Add a Storage Volume")
            .hide_on_close(true)
            .build();

        // header
        let icon = gtk::Image::from_icon_name("document-new");
        icon.set_pixel_size(48);
        let header_label = gtk::Label::new(None);
        header_label.set_markup("<span size='large'>Create storage volume</span>");
        header_label.set_xalign(0.0);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        header.set_margin_start(6);
        header.set_margin_end(6);
        header.set_margin_top(6);
        header.set_margin_bottom(6);
        header.append(&icon);
        header.append(&header_label);

        let intro = gtk::Label::new(Some(
            "Create a storage unit to be used directly by a virtual machine.",
        ));
        intro.set_halign(gtk::Align::Start);

        // table1: name and format
        let vol_name = gtk::Entry::new();
        vol_name.set_width_chars(30);
        vol_name.update_property(&[gtk::accessible::Property::Label("vol-name")]);
        let vol_name_suffix = gtk::Label::new(Some("suffix"));
        let name_box = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        name_box.append(&vol_name);
        name_box.append(&vol_name_suffix);
        let vol_format = EntryCombo::new(
            ["raw", "qcow2"]
                .iter()
                .map(|f| (Some(f.to_string()), f.to_string()))
                .collect(),
        );
        vol_format.entry.set_width_chars(10);
        let vol_format_label = start_label("F_ormat:", &vol_format.entry);
        let table1 = gtk::Grid::new();
        table1.set_row_spacing(6);
        table1.set_column_spacing(6);
        table1.attach(&start_label("_Name:", &vol_name), 0, 0, 1, 1);
        table1.attach(&name_box, 1, 0, 1, 1);
        table1.attach(&vol_format_label, 0, 1, 1, 1);
        table1.attach(&vol_format.top, 1, 1, 1, 1);

        // backing-expander
        let backing_store = gtk::Entry::new();
        backing_store.set_hexpand(true);
        backing_store.update_property(&[gtk::accessible::Property::Label("backing-store")]);
        let backing_browse = gtk::Button::with_mnemonic("_Browse...");
        let grid1 = gtk::Grid::new();
        grid1.set_margin_top(4);
        grid1.set_column_spacing(6);
        grid1.attach(&start_label("Pa_th:", &backing_store), 0, 0, 1, 1);
        grid1.attach(&backing_store, 1, 0, 1, 1);
        grid1.attach(&backing_browse, 2, 0, 1, 1);
        let backing_expander = gtk::Expander::with_mnemonic("_Backing store");
        backing_expander.set_child(Some(&grid1));

        // size-box
        let quota = gtk::Label::new(None);
        quota.set_markup("<b>Storage Volume Quota</b>");
        quota.set_halign(gtk::Align::Start);
        let vol_parent_info = gtk::Label::new(None);
        vol_parent_info.set_halign(gtk::Align::Start);
        let adjustment = gtk::Adjustment::new(1.0, 0.0, 100_000.0, 1.0, 1000.0, 0.0);
        let vol_capacity = gtk::SpinButton::new(Some(&adjustment), 10.0, 1);
        vol_capacity.update_property(&[gtk::accessible::Property::Label("vol-capacity")]);
        let vol_nonsparse = gtk::CheckButton::with_mnemonic("_Allocate entire volume now");
        let table2 = gtk::Grid::new();
        table2.set_halign(gtk::Align::Start);
        table2.set_row_spacing(6);
        table2.set_column_spacing(6);
        table2.attach(&start_label("Ca_pacity:", &vol_capacity), 0, 0, 1, 1);
        table2.attach(&vol_capacity, 1, 0, 1, 1);
        table2.attach(&gtk::Label::new(Some("GiB")), 2, 0, 1, 1);
        table2.attach(&vol_nonsparse, 1, 1, 2, 1);
        let size_box = gtk::Box::new(gtk::Orientation::Vertical, 5);
        let vbox6 = gtk::Box::new(gtk::Orientation::Vertical, 0);
        vbox6.append(&quota);
        vbox6.append(&vol_parent_info);
        size_box.append(&vbox6);
        size_box.append(&table2);

        let vbox4 = gtk::Grid::new();
        vbox4.set_row_spacing(18);
        vbox4.attach(&table1, 0, 0, 1, 1);
        vbox4.attach(&backing_expander, 0, 1, 1, 1);
        vbox4.attach(&size_box, 0, 2, 1, 1);

        let details_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        details_box.append(&intro);
        details_box.append(&vbox4);
        // `vmmXMLEditor(..., details-box-align, details-box)`
        let xmleditor = XmlEditor::new(&details_box);
        xmleditor.widget().set_size_request(400, 400);
        xmleditor.widget().set_vexpand(true);

        let vol_cancel = gtk::Button::with_mnemonic("_Cancel");
        let vol_create = gtk::Button::with_mnemonic("_Finish");
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.set_halign(gtk::Align::End);
        buttons.append(&vol_cancel);
        buttons.append(&vol_create);

        let vbox1 = gtk::Box::new(gtk::Orientation::Vertical, 18);
        vbox1.set_margin_start(12);
        vbox1.set_margin_end(12);
        vbox1.set_margin_top(12);
        vbox1.set_margin_bottom(12);
        vbox1.append(xmleditor.widget());
        vbox1.append(&buttons);
        let box1 = gtk::Box::new(gtk::Orientation::Vertical, 6);
        box1.append(&header);
        box1.append(&vbox1);
        window.set_child(Some(&box1));

        let this = CreateVolume(Rc::new(Inner {
            conn: conn.clone(),
            window,
            xmleditor,
            parent_pool: RefCell::new(parent_pool),
            name_hint: RefCell::default(),
            storage_browser: RefCell::default(),
            vol_name,
            vol_name_suffix,
            vol_format,
            vol_format_label,
            vol_parent_info,
            vol_capacity,
            vol_nonsparse,
            backing_expander,
            backing_store,
            vol_create,
            vol_created: RefCell::default(),
        }));

        let with = |f: fn(&CreateVolume)| {
            let weak = Rc::downgrade(&this.0);
            move || {
                if let Some(inner) = weak.upgrade() {
                    f(&CreateVolume(inner));
                }
            }
        };
        let cb = with(CreateVolume::close);
        vol_cancel.connect_clicked(move |_| cb());
        let cb = with(|t| {
            let t = t.clone();
            glib::spawn_future_local(async move { t.finish().await });
        });
        this.0.vol_create.connect_clicked(move |_| cb());
        let cb = with(CreateVolume::vol_name_changed);
        this.0.vol_name.connect_changed(move |_| cb());
        let cb = with(CreateVolume::vol_format_changed);
        this.0.vol_format.entry.connect_changed(move |_| cb());
        let cb = with(CreateVolume::browse_file);
        backing_browse.connect_clicked(move |_| cb());
        let cb = with(CreateVolume::xmleditor_xml_requested);
        this.0.xmleditor.connect_xml_requested(cb);

        let esc = gtk::EventControllerKey::new();
        let cb = with(CreateVolume::close);
        esc.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                cb();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        this.0.window.add_controller(esc);
        let cb = with(CreateVolume::close);
        this.0.window.connect_close_request(move |_| {
            cb();
            glib::Propagation::Stop
        });
        this
    }

    /// `show`.
    pub fn show(&self, parent: &gtk::Window) {
        eprintln!(
            "Showing new volume wizard for parent_pool={}",
            self.0.parent_pool.borrow().name
        );
        let this = self.clone();
        glib::spawn_future_local(async move { this.reset_state().await });
        self.0.window.set_transient_for(Some(parent));
        self.0.window.present();
    }

    /// `close`.
    pub fn close(&self) {
        eprintln!("Closing new volume wizard");
        self.0.window.set_visible(false);
        if let Some(b) = self.0.storage_browser.borrow().as_ref() {
            b.close();
        }
        self.set_modal(false);
    }

    pub fn set_name_hint(&self, hint: Option<&str>) {
        *self.0.name_hint.borrow_mut() = hint.map(str::to_string);
    }

    pub fn set_modal(&self, modal: bool) {
        self.0.window.set_modal(modal);
    }

    pub fn set_parent_pool(&self, pool: PoolXml) {
        *self.0.parent_pool.borrow_mut() = pool;
    }

    /// `vol-created`: called with the pool and volume names once the
    /// volume exists.
    pub fn connect_vol_created(&self, cb: impl Fn(&str, &str) + 'static) {
        *self.0.vol_created.borrow_mut() = Some(Box::new(cb));
    }

    /// `_reset_state`.
    async fn reset_state(&self) {
        let i = &self.0;
        i.xmleditor.reset_state();

        let pool = i.parent_pool.borrow().clone();
        let hasformat = self.stub_vol().supports_format();
        i.vol_format_label.set_visible(hasformat);
        i.vol_format.top.set_visible(hasformat);
        i.vol_format
            .set(i.conn.get_default_storage_format().as_deref());

        i.vol_name.set_text("");
        i.vol_name.grab_focus();
        i.backing_store.set_text("");
        i.vol_nonsparse.set_active(!self.should_default_sparse());
        self.show_sparse();
        self.show_backing();
        i.backing_expander.set_expanded(false);

        let pool_avail = pool.available / 1024 / 1024 / 1024;
        let default_cap = 20;
        i.vol_capacity.set_range(0.1, 1_000_000.0);
        i.vol_capacity.set_value(default_cap.min(pool_avail) as f64);

        i.vol_parent_info.set_markup(&format!(
            "<b>{}'s</b> available space: {}",
            glib::markup_escape_text(&pool.name),
            storage::pretty_bytes(pool.available)
        ));

        let name = self.default_vol_name().await;
        i.vol_name.set_text(&name);
        self.vol_name_changed();
    }

    /// `_get_config_format`.
    fn get_config_format(&self) -> Option<String> {
        if !self.0.vol_format.top.is_visible() {
            return None;
        }
        self.0.vol_format.get()
    }

    /// `_default_vol_name`: a free name for the hint, without the
    /// suffix the name label shows.
    async fn default_vol_name(&self) -> String {
        let hint = self
            .0
            .name_hint
            .borrow()
            .clone()
            .unwrap_or_else(|| "vol".into());
        let suffix = self.default_suffix();
        let Some(handle) = self.0.conn.handle() else {
            return String::new();
        };
        let pool_name = self.0.parent_pool.borrow().name.clone();
        let suffix2 = suffix.clone();
        let res = gio::spawn_blocking(move || {
            let conn = handle.connect();
            let pool = virt::storage_pool::StoragePool::lookup_by_name(conn, &pool_name)
                .map_err(|e| e.to_string())?;
            let xml = PoolXml::parse(&pool.get_xml_desc(0).unwrap_or_default())
                .ok_or("Unparseable pool XML")?;
            storage::find_free_name(&storage::Pool { pool, xml }, &hint, &suffix2, &[])
        })
        .await
        .unwrap_or_else(|_| Err("Worker thread panicked".into()));
        match res {
            Ok(ret) if !suffix.is_empty() => ret
                .rsplit_once('.')
                .map(|(base, _)| base.to_string())
                .unwrap_or(ret),
            Ok(ret) => ret,
            Err(e) => {
                eprintln!("Error finding a default vol name: {e}");
                String::new()
            }
        }
    }

    /// `_make_stub_vol`: a volume in the parent pool, for its type.
    fn stub_vol(&self) -> VolInstall {
        let pool = self.0.parent_pool.borrow();
        let file_type = pool.get_disk_type().to_string();
        VolInstall {
            pool_name: pool.name.clone(),
            name: String::new(),
            capacity: 0,
            allocation: 0,
            format: None,
            file_type,
            backing_store: None,
            backing_format: None,
        }
    }

    /// `_default_suffix`.
    fn default_suffix(&self) -> String {
        if self.stub_vol().file_type != "file" {
            return String::new();
        }
        storage::file_extension_for_format(self.get_config_format().as_deref())
    }

    /// `_should_default_sparse`.
    fn should_default_sparse(&self) -> bool {
        self.get_config_format().as_deref() == Some("qcow2")
    }

    /// `_can_sparse`.
    fn can_sparse(&self) -> bool {
        let pool = self.0.parent_pool.borrow();
        pool.get_disk_type() == "file" || pool.pool_type == "zfs"
    }

    fn show_sparse(&self) {
        self.0.vol_nonsparse.set_visible(self.can_sparse());
    }

    /// `_can_backing`.
    fn can_backing(&self) -> bool {
        self.0.parent_pool.borrow().pool_type == "logical"
            || self.get_config_format().as_deref() == Some("qcow2")
    }

    fn show_backing(&self) {
        self.0.backing_expander.set_visible(self.can_backing());
    }

    /// `_browse_file`: the backing store, from the storage browser.
    fn browse_file(&self) {
        let entry = self.0.backing_store.clone();
        let created = self.0.storage_browser.borrow().is_none();
        storagebrowse::browse(
            &self.0.storage_browser,
            &self.0.conn,
            &self.0.window,
            BrowseReason::Image,
            move |text| entry.set_text(text),
        );
        if created {
            if let Some(b) = self.0.storage_browser.borrow().as_ref() {
                b.set_modal(self.0.window.is_modal());
            }
        }
    }

    /// `_build_xmlobj_from_ui`'s fields.
    fn build_xmlobj_from_ui(&self) -> NewVolume {
        let i = &self.0;
        let volname = format!("{}{}", i.vol_name.text(), i.vol_name_suffix.text());
        let fmt = self.get_config_format();
        let cap = i.vol_capacity.value();
        let alloc = if i.vol_nonsparse.is_active() {
            cap
        } else {
            0.0
        };
        let backing = i.backing_store.text().to_string();
        NewVolume {
            name: volname,
            capacity: cap,
            allocation: alloc,
            backing_store: Some(backing).filter(|b| !b.is_empty()),
            format: fmt,
        }
    }

    /// `_build_xmlobj`: the volume's XML, from the fields, or the XML
    /// tab when it's showing and asked for. `None` once the error is
    /// shown.
    async fn build_xmlobj(&self, check_xmleditor: bool) -> Option<String> {
        let res = self.build_xmlobj_from_ui().build_xmlobj().and_then(|obj| {
            if check_xmleditor && self.0.xmleditor.is_xml_selected() {
                // `_build_xmlobj_from_xmleditor`
                let xml = self.0.xmleditor.get_xml();
                eprintln!("Using XML from xmleditor:\n{xml}");
                return storage::parse_volume(&xml);
            }
            Ok(obj)
        });
        match res {
            Ok(obj) => Some(obj.get_xml()),
            Err(e) => {
                self.show_err(&format!("Error building XML: {e}"), None)
                    .await;
                None
            }
        }
    }

    /// `_finish`: validate, then create the volume under a progress
    /// dialog, refresh the pool and report `vol-created`.
    async fn finish(&self) {
        let i = &self.0;
        let Some(xml) = self.build_xmlobj(true).await else {
            return;
        };
        let Some(handle) = i.conn.handle() else {
            return;
        };
        let pool_name = i.parent_pool.borrow().name.clone();
        // `vol.validate()`: the volume's name and XML with the defaults
        let lookup = pool_name.clone();
        let res = gio::spawn_blocking(move || {
            let conn = handle.connect();
            let pool = virt::storage_pool::StoragePool::lookup_by_name(conn, &lookup)
                .map_err(|e| e.to_string())?;
            let mut vol = storage::parse_volume(&xml)?;
            storage::validate_volume(conn, &pool, &mut vol)?;
            Ok::<_, String>((vol.get_str("name").unwrap_or_default(), vol.get_xml()))
        })
        .await
        .unwrap_or_else(|_| Err("Worker thread panicked".into()));
        let (volname, xml) = match res {
            Ok(v) => v,
            Err(e) => {
                self.show_err(&format!("Error validating volume: {e}"), None)
                    .await;
                return;
            }
        };

        i.window.set_cursor_from_name(Some("progress"));
        let Some(handle) = i.conn.handle() else {
            return;
        };
        let job_pool = pool_name.clone();
        let err = asyncjob::run(
            Some(&i.window),
            "Creating storage volume...",
            "Creating the storage volume may take a while...",
            true,
            move |_job| {
                // `_async_vol_create`: lookup different pool obj
                let conn = handle.connect();
                let pool = virt::storage_pool::StoragePool::lookup_by_name(conn, &job_pool)
                    .map_err(|e| (e.to_string(), e.to_string()))?;
                eprintln!("Starting background vol creation.");
                storage::install_volume(conn, &pool, &xml).map_err(|e| (e.clone(), e))?;
                eprintln!("vol creation complete.");
                let _ = pool.refresh(0);
                Ok(())
            },
        )
        .await;
        i.window.set_cursor(None);

        if let Some((error, details)) = err {
            self.show_err(&format!("Error creating vol: {error}"), Some(&details))
                .await;
            return;
        }
        if let Some(cb) = i.vol_created.borrow().as_ref() {
            cb(&pool_name, &volname);
        }
        self.close();
    }

    /// `_xmleditor_xml_requested_cb`.
    fn xmleditor_xml_requested(&self) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            let xml = this.build_xmlobj(false).await;
            this.0.xmleditor.set_xml(xml.as_deref().unwrap_or(""));
        });
    }

    async fn show_err(&self, info: &str, details: Option<&str>) {
        error::show_err(Some(&self.0.window), info, details, "").await;
    }

    /// `_vol_format_changed_cb`.
    fn vol_format_changed(&self) {
        self.show_sparse();
        self.0
            .vol_nonsparse
            .set_active(!self.should_default_sparse());
        self.show_backing();
        self.vol_name_changed();
    }

    /// `_vol_name_changed_cb`.
    fn vol_name_changed(&self) {
        let text = self.0.vol_name.text();
        let suffix = if text.contains('.') {
            String::new()
        } else {
            self.default_suffix()
        };
        self.0.vol_name_suffix.set_text(&suffix);
    }
}
