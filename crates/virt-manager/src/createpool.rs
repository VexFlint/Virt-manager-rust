//! `createpool.py` (`vmmCreatePool`): the "Add a New Storage Pool"
//! wizard, with the fields each pool type takes.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use virtinst_core::storage::{self, NewPool};

use crate::asyncjob;
use crate::connection::VmmConnection;
use crate::devpages::{EntryCombo, SimpleCombo};
use crate::error;

/// A grid row's label and widgets, hidden together
/// (`uiutil.set_grid_row_visible`).
struct Row {
    label: gtk::Label,
    widgets: Vec<gtk::Widget>,
}

impl Row {
    fn set_visible(&self, visible: bool) {
        self.label.set_visible(visible);
        for w in &self.widgets {
            w.set_visible(visible);
        }
    }
}

fn row_label(text: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Label {
    let l = gtk::Label::with_mnemonic(text);
    l.set_halign(gtk::Align::End);
    l.set_mnemonic_widget(Some(widget));
    l
}

pub struct Inner {
    conn: VmmConnection,
    window: gtk::Window,
    pool_name: gtk::Entry,
    pool_type: SimpleCombo,
    pool_target_path: gtk::Entry,
    pool_target_button: gtk::Button,
    target_row: Row,
    pool_format: Rc<EntryCombo>,
    format_row: Row,
    pool_hostname: gtk::Entry,
    hostname_row: Row,
    pool_source_name: Rc<EntryCombo>,
    source_name_row: Row,
    pool_source_path: Rc<EntryCombo>,
    pool_source_button: gtk::Button,
    source_row: Row,
    pool_iqn_chk: gtk::CheckButton,
    pool_iqn: gtk::Entry,
    iqn_row: Row,
    pool_created: RefCell<Option<Box<dyn Fn()>>>,
}

#[derive(Clone)]
pub struct CreatePool(Rc<Inner>);

impl CreatePool {
    pub fn new(conn: &VmmConnection) -> CreatePool {
        let window = gtk::Window::builder()
            .title("Add a New Storage Pool")
            .hide_on_close(true)
            .build();

        // header
        let icon = gtk::Image::from_icon_name("document-new");
        icon.set_pixel_size(48);
        let header_label = gtk::Label::new(None);
        header_label.set_markup("<span size='large'>Create storage pool</span>");
        header_label.set_xalign(0.0);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        header.set_margin_start(6);
        header.set_margin_end(6);
        header.set_margin_top(6);
        header.set_margin_bottom(6);
        header.append(&icon);
        header.append(&header_label);

        // `_build_pool_type_list`
        let pool_type = SimpleCombo::unsorted(
            storage::POOL_TYPES
                .iter()
                .map(|t| {
                    (
                        Some(t.to_string()),
                        format!("{t}: {}", storage::pretty_pool_type(t)),
                    )
                })
                .collect(),
            str::to_string,
        );
        pool_type.widget.set_halign(gtk::Align::Start);

        let pool_name = gtk::Entry::new();
        pool_name.update_property(&[gtk::accessible::Property::Label("pool-name")]);
        let pool_target_path = gtk::Entry::new();
        pool_target_path.set_hexpand(true);
        let pool_target_button = gtk::Button::with_mnemonic("B_rowse");
        let pool_format = EntryCombo::new(vec![(Some("auto".into()), "auto".into())]);
        pool_format.entry.set_width_chars(10);
        let pool_hostname = gtk::Entry::new();
        pool_hostname.set_width_chars(25);
        let pool_source_name = EntryCombo::new(vec![]);
        let pool_source_path = EntryCombo::new(vec![]);
        pool_source_path.entry.set_width_chars(25);
        let pool_source_button = gtk::Button::with_mnemonic("Bro_wse");
        let pool_iqn_chk = gtk::CheckButton::new();
        let pool_iqn = gtk::Entry::new();
        pool_iqn.set_width_chars(25);
        let iqn_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        iqn_box.append(&pool_iqn_chk);
        iqn_box.append(&pool_iqn);

        let grid = gtk::Grid::new();
        grid.set_row_spacing(4);
        grid.set_column_spacing(6);
        grid.attach(&row_label("_Name:", &pool_name), 0, 0, 1, 1);
        grid.attach(&pool_name, 1, 0, 1, 1);
        grid.attach(&row_label("_Type:", &pool_type.widget), 0, 1, 1, 1);
        grid.attach(&pool_type.widget, 1, 1, 1, 1);
        grid.attach(&gtk::Label::new(Some(" ")), 0, 2, 1, 1);
        let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
        separator.set_valign(gtk::Align::Center);
        grid.attach(&separator, 1, 2, 2, 1);

        let target_label = row_label("Tar_get Path:", &pool_target_path);
        grid.attach(&target_label, 0, 3, 1, 1);
        grid.attach(&pool_target_path, 1, 3, 1, 1);
        grid.attach(&pool_target_button, 2, 3, 1, 1);
        let format_label = row_label("F_ormat:", &pool_format.entry);
        grid.attach(&format_label, 0, 4, 1, 1);
        grid.attach(&pool_format.top, 1, 4, 1, 1);
        let hostname_label = row_label("Host Na_me:", &pool_hostname);
        grid.attach(&hostname_label, 0, 5, 1, 1);
        grid.attach(&pool_hostname, 1, 5, 1, 1);
        let source_name_label = row_label("Sou_rce Name:", &pool_source_name.entry);
        grid.attach(&source_name_label, 0, 6, 1, 1);
        grid.attach(&pool_source_name.top, 1, 6, 1, 1);
        let source_label = row_label("_Source Path:", &pool_source_path.entry);
        grid.attach(&source_label, 0, 7, 1, 1);
        grid.attach(&pool_source_path.top, 1, 7, 1, 1);
        grid.attach(&pool_source_button, 2, 7, 1, 1);
        let iqn_label = row_label("Initiator _IQN:", &pool_iqn);
        grid.attach(&iqn_label, 0, 8, 1, 1);
        grid.attach(&iqn_box, 1, 8, 1, 1);

        let pool_cancel = gtk::Button::with_mnemonic("_Cancel");
        let pool_finish = gtk::Button::with_mnemonic("_Finish");
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        buttons.set_halign(gtk::Align::End);
        buttons.append(&pool_cancel);
        buttons.append(&pool_finish);

        let vbox2 = gtk::Box::new(gtk::Orientation::Vertical, 18);
        vbox2.set_margin_start(12);
        vbox2.set_margin_end(12);
        vbox2.set_margin_top(12);
        vbox2.set_margin_bottom(12);
        vbox2.append(&grid);
        vbox2.append(&buttons);
        let vbox1 = gtk::Box::new(gtk::Orientation::Vertical, 0);
        vbox1.append(&header);
        vbox1.append(&vbox2);
        window.set_child(Some(&vbox1));

        let row = |label: &gtk::Label, widgets: &[&gtk::Widget]| Row {
            label: label.clone(),
            widgets: widgets.iter().map(|w| (*w).clone()).collect(),
        };
        let this = CreatePool(Rc::new(Inner {
            conn: conn.clone(),
            window,
            target_row: row(
                &target_label,
                &[
                    pool_target_path.upcast_ref(),
                    pool_target_button.upcast_ref(),
                ],
            ),
            format_row: row(&format_label, &[pool_format.top.upcast_ref()]),
            hostname_row: row(&hostname_label, &[pool_hostname.upcast_ref()]),
            source_name_row: row(&source_name_label, &[pool_source_name.top.upcast_ref()]),
            source_row: row(
                &source_label,
                &[
                    pool_source_path.top.upcast_ref(),
                    pool_source_button.upcast_ref(),
                ],
            ),
            iqn_row: row(&iqn_label, &[iqn_box.upcast_ref()]),
            pool_name,
            pool_type,
            pool_target_path,
            pool_target_button,
            pool_format,
            pool_hostname,
            pool_source_name,
            pool_source_path,
            pool_source_button,
            pool_iqn_chk,
            pool_iqn,
            pool_created: RefCell::default(),
        }));

        let with = |f: fn(&CreatePool)| {
            let weak = Rc::downgrade(&this.0);
            move || {
                if let Some(inner) = weak.upgrade() {
                    f(&CreatePool(inner));
                }
            }
        };
        let cb = with(CreatePool::close);
        pool_cancel.connect_clicked(move |_| cb());
        let cb = with(|t| {
            let t = t.clone();
            glib::spawn_future_local(async move { t.finish().await });
        });
        pool_finish.connect_clicked(move |_| cb());
        let cb = with(CreatePool::show_options_by_pool);
        this.0
            .pool_type
            .widget
            .connect_selected_notify(move |_| cb());
        let cb = with(|t| {
            let t = t.clone();
            glib::spawn_future_local(async move { t.browse_source().await });
        });
        this.0.pool_source_button.connect_clicked(move |_| cb());
        let cb = with(|t| {
            let t = t.clone();
            glib::spawn_future_local(async move { t.browse_target().await });
        });
        this.0.pool_target_button.connect_clicked(move |_| cb());
        let cb = with(|t| {
            t.0.pool_iqn.set_sensitive(t.0.pool_iqn_chk.is_active());
        });
        this.0.pool_iqn_chk.connect_toggled(move |_| cb());

        let esc = gtk::EventControllerKey::new();
        let cb = with(CreatePool::close);
        esc.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                cb();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        this.0.window.add_controller(esc);
        this
    }

    /// `show`.
    pub fn show(&self, parent: &gtk::Window) {
        eprintln!("Showing new pool wizard");
        let this = self.clone();
        glib::spawn_future_local(async move { this.reset_state().await });
        self.0.window.set_transient_for(Some(parent));
        self.0.window.present();
    }

    /// `close`.
    pub fn close(&self) {
        eprintln!("Closing new pool wizard");
        self.0.window.set_visible(false);
    }

    /// Called once a new pool is defined and started.
    pub fn connect_pool_created(&self, cb: impl Fn() + 'static) {
        *self.0.pool_created.borrow_mut() = Some(Box::new(cb));
    }

    /// `_reset_state`.
    async fn reset_state(&self) {
        let i = &self.0;
        i.pool_name.set_text("");
        i.pool_name.grab_focus();
        i.pool_target_path.set_text("");
        i.pool_source_path.entry.set_text("");
        i.pool_hostname.set_text("");
        i.pool_iqn_chk.set_active(false);
        i.pool_iqn.set_sensitive(false);
        i.pool_iqn.set_text("");
        i.pool_format.set_active(0);
        i.pool_type.widget.set_selected(0);

        if let Some(handle) = i.conn.handle() {
            let name =
                gio::spawn_blocking(move || storage::find_free_pool_name(handle.connect(), "pool"))
                    .await
                    .unwrap_or_default();
            i.pool_name.set_text(&name);
        }
        self.show_options_by_pool();
    }

    /// `_populate_pool_sources`: SCSI adapters, or LVM volume groups.
    async fn populate_pool_sources(&self) {
        let i = &self.0;
        let pooltype = i.pool_type.get().unwrap_or_default();
        i.pool_source_path.set_rows(vec![]);
        i.pool_source_name.set_rows(vec![]);

        let (entries, use_list) = match pooltype.as_str() {
            "scsi" => {
                // `_list_scsi_adapters`
                let mut hosts: Vec<String> = i
                    .conn
                    .list_nodedevs()
                    .await
                    .into_iter()
                    .filter(|d| d.device_type.as_deref() == Some("scsi_host"))
                    .filter_map(|d| d.host.map(|h| format!("host{h}")))
                    .collect();
                hosts.sort();
                (hosts, &i.pool_source_path)
            }
            "logical" => {
                let Some(handle) = i.conn.handle() else {
                    return;
                };
                let mut vgs = gio::spawn_blocking(move || {
                    storage::pool_list_from_sources(handle.connect(), "logical")
                })
                .await
                .unwrap_or_default();
                vgs.sort();
                (vgs, &i.pool_source_name)
            }
            _ => return,
        };
        // A type change since this started owns the lists now
        if i.pool_type.get().as_deref() != Some(pooltype.as_str()) {
            return;
        }
        let rows: Vec<(Option<String>, String)> = entries
            .iter()
            .map(|e| (Some(e.clone()), e.clone()))
            .collect();
        let any = !rows.is_empty();
        use_list.set_rows(rows);
        if any {
            use_list.set_active(0);
        }
    }

    /// `_make_stub_pool`.
    fn make_stub_pool(&self) -> NewPool {
        NewPool {
            name: self.0.pool_name.text().to_string(),
            pool_type: self.0.pool_type.get().unwrap_or_default(),
            ..Default::default()
        }
    }

    /// `_show_options_by_pool`.
    fn show_options_by_pool(&self) {
        let i = &self.0;
        let pool = self.make_stub_pool();
        let is_remote = i.conn.is_remote();
        let src = pool.supports_source_path();
        let mut src_b = src && !is_remote;
        let tgt = pool.supports_target_path();
        let tgt_b = tgt && !is_remote;
        let iqn = pool.supports_iqn();
        let src_name = pool.supports_source_name();
        let is_lvm = pool.pool_type == "logical";
        let is_scsi = pool.pool_type == "scsi";

        // Source path browsing is meaningless for net pools
        if matches!(
            pool.pool_type.as_str(),
            "netfs" | "iscsi" | "scsi" | "gluster"
        ) {
            src_b = false;
        }

        i.target_row.set_visible(tgt);
        i.source_row.set_visible(src);
        i.hostname_row.set_visible(pool.supports_hosts());
        i.format_row.set_visible(pool.supports_format());
        i.iqn_row.set_visible(iqn);
        i.source_name_row.set_visible(src_name);

        i.source_name_row.label.set_text_with_mnemonic(if is_lvm {
            "Volg_roup Name:"
        } else {
            "Sou_rce Name:"
        });
        let src_label = if iqn {
            "_Source IQN:"
        } else if is_scsi {
            "_Source Adapter:"
        } else {
            "_Source Path:"
        };
        i.source_row.label.set_text_with_mnemonic(src_label);

        if tgt {
            let session = i.conn.is_qemu_unprivileged();
            i.pool_target_path
                .set_text(&pool.default_target_path(session).unwrap_or_default());
        }
        i.pool_target_button.set_sensitive(tgt_b);
        i.pool_source_button.set_sensitive(src_b);

        if src_name {
            i.pool_source_name
                .entry
                .set_text(&pool.default_source_name().unwrap_or_default());
        }

        let this = self.clone();
        glib::spawn_future_local(async move { this.populate_pool_sources().await });
    }

    /// `_get_visible_text`: the text, unless the widget is hidden or
    /// insensitive.
    fn visible_text(widget: &impl IsA<gtk::Widget>, text: Option<String>) -> Option<String> {
        if !widget.is_sensitive() || !widget.is_visible() {
            return None;
        }
        text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
    }

    /// `_build_xmlobj_from_ui`.
    fn build_xmlobj(&self) -> NewPool {
        let i = &self.0;
        let mut pool = self.make_stub_pool();
        pool.target_path = Self::visible_text(
            &i.pool_target_path,
            Some(i.pool_target_path.text().to_string()),
        );
        pool.host = Self::visible_text(&i.pool_hostname, Some(i.pool_hostname.text().to_string()));
        pool.source_path = Self::visible_text(&i.pool_source_path.top, i.pool_source_path.get());
        let fmt = i.pool_format.get();
        if fmt.is_some() && pool.supports_format() {
            pool.format = fmt;
        }
        pool.iqn = Self::visible_text(&i.pool_iqn, Some(i.pool_iqn.text().to_string()));
        pool.source_name = Self::visible_text(&i.pool_source_name.top, i.pool_source_name.get());
        pool
    }

    /// `_finish`.
    async fn finish(&self) {
        let i = &self.0;
        let pool = self.build_xmlobj();
        let Some(handle) = i.conn.handle() else {
            return;
        };
        let session = i.conn.is_qemu_unprivileged();
        let validated = gio::spawn_blocking(move || {
            let mut pool = pool;
            pool.validate(handle.connect(), session).map(|_| pool)
        })
        .await
        .unwrap_or_else(|_| Err("Worker thread panicked".into()));
        let pool = match validated {
            Ok(p) => p,
            Err(e) => {
                error::show_err(
                    Some(&i.window),
                    &format!("Error validating pool: {e}"),
                    None,
                    "",
                )
                .await;
                return;
            }
        };
        // `_get_build_default`: building these simply entails creating
        // a directory
        let build = matches!(pool.pool_type.as_str(), "dir" | "fs" | "netfs");

        let Some(handle) = i.conn.handle() else {
            return;
        };
        eprintln!(
            "Creating storage pool '{}' with xml:\n{}",
            pool.name,
            pool.to_xml()
        );
        let err = asyncjob::run(
            Some(&i.window),
            "Creating storage pool...",
            "Creating the storage pool may take a while...",
            true,
            move |_job| {
                eprintln!("Starting background pool creation.");
                pool.install(handle.connect(), build)
                    .map_err(|e| (e.clone(), e))?;
                eprintln!("Pool creation succeeded");
                Ok(())
            },
        )
        .await;
        if let Some((error, details)) = err {
            error::show_err(
                Some(&i.window),
                &format!("Error creating pool: {error}"),
                Some(&details),
                "",
            )
            .await;
            return;
        }
        if let Some(cb) = i.pool_created.borrow().as_ref() {
            cb();
        }
        self.close();
    }

    /// `_browse_source_cb`.
    async fn browse_source(&self) {
        let dialog = gtk::FileDialog::builder()
            .title("Choose source path")
            .modal(true)
            .initial_folder(&gio::File::for_path("/dev"))
            .build();
        if let Some(path) = dialog
            .open_future(Some(&self.0.window))
            .await
            .ok()
            .and_then(|f| f.path())
        {
            self.0
                .pool_source_path
                .entry
                .set_text(&path.to_string_lossy());
        }
    }

    /// `_browse_target_cb`.
    async fn browse_target(&self) {
        let dialog = gtk::FileDialog::builder()
            .title("Choose target directory")
            .modal(true)
            .build();
        let current = self.0.pool_target_path.text();
        if let Some(dir) = std::path::Path::new(current.as_str()).parent() {
            if !current.is_empty() {
                dialog.set_initial_folder(Some(&gio::File::for_path(dir)));
            }
        }
        if let Some(path) = dialog
            .select_folder_future(Some(&self.0.window))
            .await
            .ok()
            .and_then(|f| f.path())
        {
            self.0.pool_target_path.set_text(&path.to_string_lossy());
        }
    }
}
