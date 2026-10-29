//! `hoststorage.py` (`vmmHostStorage`): the storage pool list, the
//! selected pool's details, its XML tab and its volumes. The storage
//! browser and the Connection Details window (`host.rs`) embed it.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gio, glib};
use virtinst_core::storage::{self, PoolXml, VolXml};

use crate::connection::VmmConnection;
use crate::createpool::CreatePool;
use crate::createvol::CreateVolume;
use crate::error;
use crate::xmleditor::XmlEditor;

/// `EDIT_POOL_IDS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PoolEdit {
    Name,
    Autostart,
    Xml,
}

/// A pool row: `[pool object, label, pool.is_active(), percent string]`.
#[derive(Clone, Debug)]
pub struct PoolRow {
    pub xml: PoolXml,
    pub active: bool,
    pub autostart: bool,
    /// `XMLDesc(0)`, for the XML tab.
    pub xml_desc: String,
}

impl PoolRow {
    pub fn name(&self) -> &str {
        &self.xml.name
    }
}

/// A volume row: `[obj, name, sizestr, capacity, format, in use by
/// string, sensitive]`.
#[derive(Clone, Debug)]
pub struct VolRow {
    pub xml: VolXml,
    pub name: String,
    pub sizestr: String,
    pub format: String,
    pub in_use_by: String,
    pub sensitive: bool,
}

/// `_get_pool_size_percent`.
fn pool_size_percent(pool: &PoolXml) -> String {
    let per = if pool.capacity > 0 {
        (pool.allocation as f64 / pool.capacity as f64 * 100.0) as u64
    } else {
        0
    };
    format!("<span size='small'>{per}%</span>")
}

/// `conn.list_pools()`, fetched on a worker.
async fn fetch_pools(conn: &VmmConnection) -> Vec<PoolRow> {
    let Some(handle) = conn.handle() else {
        return vec![];
    };
    gio::spawn_blocking(move || {
        let pools = handle
            .connect()
            .list_all_storage_pools(0)
            .unwrap_or_default();
        pools
            .iter()
            .filter_map(|p| {
                let xml_desc = p.get_xml_desc(0).ok()?;
                Some(PoolRow {
                    xml: PoolXml::parse(&xml_desc)?,
                    active: p.is_active().unwrap_or(false),
                    autostart: p.get_autostart().unwrap_or(false),
                    xml_desc,
                })
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// `pool.get_volumes()` with `DeviceDisk.paths_in_use_by`, fetched on
/// a worker. The rows are unsorted and their sensitivity unset.
async fn fetch_vols(conn: &VmmConnection, pool: &str) -> Vec<VolRow> {
    let Some(handle) = conn.handle() else {
        return vec![];
    };
    let pool = pool.to_string();
    gio::spawn_blocking(move || {
        let conn = handle.connect();
        let Ok(p) = virt::storage_pool::StoragePool::lookup_by_name(conn, &pool) else {
            return vec![];
        };
        let pool_type = PoolXml::parse(&p.get_xml_desc(0).unwrap_or_default())
            .map(|x| x.pool_type)
            .unwrap_or_default();
        let vols: Vec<VolXml> = p
            .list_all_volumes(0)
            .unwrap_or_default()
            .iter()
            .filter_map(|v| VolXml::parse(&v.get_xml_desc(0).ok()?))
            .collect();
        let doms = conn.list_all_domains(0).unwrap_or_default();
        let vms: Vec<(String, String)> = doms
            .iter()
            .filter_map(|d| Some((d.get_name().ok()?, d.get_xml_desc(0).ok()?)))
            .collect();
        let volmap = storage::get_volmap(conn);
        vols.into_iter()
            .map(|xml| {
                let names = storage::path_in_use_by(&xml.target_path, &volmap, &vms, false, false);
                VolRow {
                    name: xml.pretty_name(&pool_type),
                    sizestr: storage::pretty_bytes(xml.capacity),
                    format: xml.format.clone().unwrap_or_default(),
                    in_use_by: names.join(", "),
                    sensitive: true,
                    xml,
                }
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// Runs `f` on the named pool on a worker, as libvirt's message on
/// failure.
async fn with_pool(
    conn: &VmmConnection,
    name: &str,
    f: impl FnOnce(&virt::connect::Connect, &virt::storage_pool::StoragePool) -> Result<(), String>
        + Send
        + 'static,
) -> Result<(), String> {
    let handle = conn.handle().ok_or("Not connected")?;
    let name = name.to_string();
    gio::spawn_blocking(move || {
        let conn = handle.connect();
        let pool = virt::storage_pool::StoragePool::lookup_by_name(conn, &name)
            .map_err(|e| e.message().to_string())?;
        f(conn, &pool)
    })
    .await
    .map_err(|_| "Worker thread panicked".to_string())?
}

fn libvirt_err(e: virt::error::Error) -> String {
    e.message().to_string()
}

type Callback = Box<dyn Fn()>;
type VolCallback = Box<dyn Fn(&VolRow)>;
type VolSensitiveCallback = Box<dyn Fn(&str) -> bool>;
type VolPopupCallback = Rc<RefCell<Option<Box<dyn Fn(u32, &gtk::Widget, f64, f64)>>>>;

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.set_tooltip_text(Some(tooltip));
    b
}

fn field_label(text: &str) -> gtk::Label {
    let l = gtk::Label::with_mnemonic(text);
    l.set_xalign(0.0);
    l.set_halign(gtk::Align::Start);
    l
}

pub struct Inner {
    conn: VmmConnection,
    /// `storage-grid`.
    pub top_box: gtk::Box,
    pool_list: gtk::ListBox,
    pools: RefCell<Vec<PoolRow>>,
    /// The pool the details show, for applying its edits.
    shown_pool: RefCell<Option<PoolRow>>,
    /// The pool to select once the list is filled.
    pending_select: RefCell<Option<String>>,
    pages: gtk::Stack,
    error_label: gtk::Label,
    xmleditor: XmlEditor,
    pool_details: gtk::Box,
    pub pool_name_label: gtk::Label,
    pub pool_name_entry: gtk::Entry,
    pool_sizes: gtk::Label,
    pool_location: gtk::Label,
    pub pool_state_label: gtk::Label,
    pub pool_state_box: gtk::Box,
    pool_state_icon: gtk::Image,
    pool_state: gtk::Label,
    pub pool_autostart_label: gtk::Label,
    pub pool_autostart: gtk::CheckButton,
    vol_store: gio::ListStore,
    vol_selection: gtk::SingleSelection,
    vol_list: gtk::ColumnView,
    vol_menu: gtk::Popover,
    pub pool_add: gtk::Button,
    pool_start: gtk::Button,
    pool_stop: gtk::Button,
    pool_delete: gtk::Button,
    pool_refresh: gtk::Button,
    pub vol_add: gtk::Button,
    vol_delete: gtk::Button,
    pub browse_local: gtk::Button,
    pub browse_cancel: gtk::Button,
    pub choose_volume: gtk::Button,
    pub pool_apply: gtk::Button,
    active_edits: RefCell<Vec<PoolEdit>>,
    /// Set while the widgets are filled in, so they aren't edits.
    quiet: Cell<bool>,
    /// Bumped per volume fetch, so a stale one is dropped.
    vol_generation: Cell<u64>,
    vol_sensitive_cb: Option<VolSensitiveCallback>,
    /// Name hint passed to the volume wizard: the storage browser
    /// sets it to the VM name the create VM wizard is building.
    name_hint: RefCell<Option<String>>,
    addvol: RefCell<Option<CreateVolume>>,
    addpool: RefCell<Option<CreatePool>>,
    /// The volume to select once the list is filled.
    pending_vol: RefCell<Option<String>>,
    browse_clicked: RefCell<Option<Callback>>,
    volume_chosen: RefCell<Option<VolCallback>>,
    cancel_clicked: RefCell<Option<Callback>>,
    /// The connection's `state-changed` and `pool-event` handlers.
    conn_handlers: RefCell<Vec<glib::SignalHandlerId>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        for id in self.conn_handlers.take() {
            self.conn.disconnect(id);
        }
    }
}

#[derive(Clone)]
pub struct HostStorage(Rc<Inner>);

impl std::ops::Deref for HostStorage {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

impl HostStorage {
    pub fn new(
        conn: &VmmConnection,
        vol_sensitive_cb: Option<VolSensitiveCallback>,
    ) -> HostStorage {
        // Bottom bar: pool buttons on the left, dialog buttons on the
        // right (`hbox9`).
        let pool_add = icon_button("list-add", "Add Pool");
        let pool_start = icon_button("media-playback-start", "Start Pool");
        let pool_stop = icon_button("process-stop", "Stop Pool");
        let pool_delete = icon_button("edit-delete", "Delete Pool");
        let pool_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        pool_buttons.set_hexpand(true);
        for b in [&pool_add, &pool_start, &pool_stop, &pool_delete] {
            pool_buttons.append(b);
        }
        let browse_local = gtk::Button::with_mnemonic("_Browse Local");
        browse_local.set_tooltip_text(Some("Browse local filesystem"));
        let browse_cancel = gtk::Button::with_mnemonic("_Cancel");
        browse_cancel.set_tooltip_text(Some("Cancel and close dialog"));
        let choose_volume = gtk::Button::with_mnemonic("Ch_oose Volume");
        choose_volume.set_tooltip_text(Some("Choose the selected volume"));
        let pool_apply = gtk::Button::with_mnemonic("_Apply");
        pool_apply.set_tooltip_text(Some("Apply pool changes"));
        let dialog_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        for b in [&browse_local, &browse_cancel, &choose_volume, &pool_apply] {
            dialog_buttons.append(b);
        }
        let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        bottom.set_valign(gtk::Align::End);
        bottom.set_margin_start(3);
        bottom.set_margin_end(3);
        bottom.set_margin_top(3);
        bottom.set_margin_bottom(3);
        bottom.append(&pool_buttons);
        bottom.append(&dialog_buttons);

        // pool-list
        let pool_list = gtk::ListBox::new();
        pool_list.set_selection_mode(gtk::SelectionMode::Single);
        pool_list.set_size_request(134, -1);
        pool_list.update_property(&[gtk::accessible::Property::Label("pool-list")]);
        let pool_scroll = gtk::ScrolledWindow::builder()
            .child(&pool_list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .has_frame(true)
            .build();

        // pool-details: table5
        let pool_name_entry = gtk::Entry::new();
        pool_name_entry.update_property(&[gtk::accessible::Property::Label("pool-name-entry")]);
        let pool_sizes = gtk::Label::new(None);
        pool_sizes.set_halign(gtk::Align::Start);
        let pool_location = gtk::Label::new(None);
        pool_location.set_halign(gtk::Align::Start);
        pool_location.set_selectable(true);
        let pool_state_icon = gtk::Image::from_icon_name("image-missing");
        let pool_state = gtk::Label::new(Some("Active"));
        let pool_state_box = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        pool_state_box.set_halign(gtk::Align::Start);
        pool_state_box.append(&pool_state_icon);
        pool_state_box.append(&pool_state);
        let pool_autostart = gtk::CheckButton::with_mnemonic("On Boot");
        pool_autostart.set_halign(gtk::Align::Start);
        pool_autostart.update_property(&[gtk::accessible::Property::Label("pool-autostart")]);
        let pool_name_label = field_label("Name:");
        let pool_state_label = field_label("State:");
        let pool_autostart_label = field_label("A_utostart:");
        pool_autostart_label.set_mnemonic_widget(Some(&pool_autostart));
        let table = gtk::Grid::new();
        table.set_row_spacing(5);
        table.set_column_spacing(8);
        table.attach(&pool_name_label, 0, 0, 1, 1);
        table.attach(&pool_name_entry, 1, 0, 1, 1);
        table.attach(&field_label("Size:"), 0, 1, 1, 1);
        table.attach(&pool_sizes, 1, 1, 1, 1);
        table.attach(&field_label("Location:"), 0, 2, 1, 1);
        table.attach(&pool_location, 1, 2, 1, 1);
        table.attach(&pool_state_label, 0, 3, 1, 1);
        table.attach(&pool_state_box, 1, 3, 1, 1);
        table.attach(&pool_autostart_label, 0, 4, 1, 1);
        table.attach(&pool_autostart, 1, 4, 1, 1);

        // Volumes header and vol-list
        let vol_title = gtk::Label::new(None);
        vol_title.set_markup("<b>Volumes</b>");
        vol_title.set_halign(gtk::Align::Start);
        let vol_add = icon_button("list-add", "Create new volume");
        let pool_refresh = icon_button("view-refresh", "Refresh volume list");
        let vol_delete = icon_button("edit-delete", "Delete volume");
        let vol_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        vol_buttons.set_hexpand(true);
        for b in [&vol_add, &pool_refresh, &vol_delete] {
            vol_buttons.append(b);
        }
        let vol_header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        vol_header.append(&vol_title);
        vol_header.append(&vol_buttons);

        let vol_store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let vol_list = gtk::ColumnView::new(None::<gtk::SingleSelection>);
        vol_list.update_property(&[gtk::accessible::Property::Label("vol-list")]);
        let sorted = gtk::SortListModel::new(Some(vol_store.clone()), vol_list.sorter());
        let vol_selection = gtk::SingleSelection::new(Some(sorted));
        vol_selection.set_autoselect(false);
        vol_selection.set_can_unselect(true);
        vol_list.set_model(Some(&vol_selection));
        let popup_cb: VolPopupCallback = Rc::new(RefCell::new(None));
        build_vol_columns(&vol_list, &popup_cb);
        let vol_scroll = gtk::ScrolledWindow::builder()
            .child(&vol_list)
            .has_frame(true)
            .vexpand(true)
            .build();
        let vol_box = gtk::Box::new(gtk::Orientation::Vertical, 3);
        vol_box.set_vexpand(true);
        vol_box.append(&vol_header);
        vol_box.append(&vol_scroll);

        let pool_details = gtk::Box::new(gtk::Orientation::Vertical, 6);
        pool_details.append(&table);
        pool_details.append(&vol_box);

        // `_volmenu`
        let copy_path = gtk::Button::with_mnemonic("Copy Volume Path");
        copy_path.set_has_frame(false);
        let vol_menu = gtk::Popover::new();
        vol_menu.set_child(Some(&copy_path));
        vol_menu.set_has_arrow(false);
        vol_menu.set_halign(gtk::Align::Start);
        vol_menu.set_parent(&vol_list);

        // `vmmXMLEditor(..., pool-details-align, pool-details)`
        let xmleditor = XmlEditor::new(&pool_details);
        let pool_details_align = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        xmleditor.widget().set_hexpand(true);
        pool_details_align.append(xmleditor.widget());

        // storage-pages: the details, or an error
        let error_label = gtk::Label::new(Some("some error here"));
        error_label.update_property(&[gtk::accessible::Property::Label("pool-error-label")]);
        let pages = gtk::Stack::new();
        pages.add_named(&pool_details_align, Some("details"));
        pages.add_named(&error_label, Some("error"));

        let pane = gtk::Paned::new(gtk::Orientation::Horizontal);
        pane.set_start_child(Some(&pool_scroll));
        pane.set_end_child(Some(&pages));
        pane.set_resize_start_child(false);
        pane.set_shrink_start_child(true);
        pane.set_position(200);
        pane.set_hexpand(true);
        pane.set_vexpand(true);
        pane.set_margin_start(3);
        pane.set_margin_end(3);
        pane.set_margin_top(3);
        pane.set_margin_bottom(3);

        let top_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        top_box.set_margin_start(3);
        top_box.set_margin_end(3);
        top_box.set_margin_top(3);
        top_box.set_margin_bottom(3);
        top_box.append(&pane);
        top_box.append(&bottom);

        // These are enabled in storagebrowse.rs
        browse_local.set_visible(false);
        browse_cancel.set_visible(false);
        choose_volume.set_visible(false);

        let this = HostStorage(Rc::new(Inner {
            conn: conn.clone(),
            top_box,
            pool_list,
            pools: RefCell::default(),
            shown_pool: RefCell::default(),
            pending_select: RefCell::default(),
            pages,
            error_label,
            xmleditor,
            pool_details,
            pool_name_label,
            pool_name_entry,
            pool_sizes,
            pool_location,
            pool_state_label,
            pool_state_box,
            pool_state_icon,
            pool_state,
            pool_autostart_label,
            pool_autostart,
            vol_store,
            vol_selection,
            vol_list,
            vol_menu,
            pool_add,
            pool_start,
            pool_stop,
            pool_delete,
            pool_refresh,
            vol_add,
            vol_delete,
            browse_local,
            browse_cancel,
            choose_volume,
            pool_apply,
            active_edits: RefCell::default(),
            quiet: Cell::new(false),
            vol_generation: Cell::new(0),
            vol_sensitive_cb,
            name_hint: RefCell::default(),
            addvol: RefCell::default(),
            addpool: RefCell::default(),
            pending_vol: RefCell::default(),
            browse_clicked: RefCell::default(),
            volume_chosen: RefCell::default(),
            cancel_clicked: RefCell::default(),
            conn_handlers: RefCell::default(),
        }));
        let weak = this.weak();
        *popup_cb.borrow_mut() = Some(Box::new(move |pos, cell, x, y| {
            if let Some(inner) = weak.upgrade() {
                HostStorage(inner).vol_popup_menu(pos, cell, x, y);
            }
        }));
        this.connect_signals(&copy_path);
        // `_conn_state_changed_cb`
        let cb = this.with(HostStorage::refresh_conn_state);
        let id = conn.connect_local("state-changed", false, move |_| {
            cb();
            None
        });
        this.conn_handlers.borrow_mut().push(id);
        // `_conn_pools_changed_cb` / `_pool_changed_cb`
        let weak = this.weak();
        let id = conn.connect_local("pool-event", false, move |args| {
            let name = args[1].get::<String>().ok()?;
            let this = HostStorage(weak.upgrade()?);
            this.spawn(|t| async move { t.pool_event(&name).await });
            None
        });
        this.conn_handlers.borrow_mut().push(id);
        this.refresh_conn_state();
        this
    }

    fn weak(&self) -> Weak<Inner> {
        Rc::downgrade(&self.0)
    }

    fn with(&self, f: impl Fn(&HostStorage) + 'static) -> impl Fn() + 'static {
        let weak = self.weak();
        move || {
            if let Some(inner) = weak.upgrade() {
                f(&HostStorage(inner));
            }
        }
    }

    fn spawn<F: std::future::Future<Output = ()> + 'static>(
        &self,
        f: impl FnOnce(HostStorage) -> F + 'static,
    ) {
        let this = self.clone();
        glib::spawn_future_local(async move { f(this).await });
    }

    fn connect_signals(&self, copy_path: &gtk::Button) {
        let cb = self.with(|t| t.spawn(|t| async move { t.pool_start_clicked().await }));
        self.pool_start.connect_clicked(move |_| cb());
        let cb = self.with(|t| t.spawn(|t| async move { t.pool_stop_clicked().await }));
        self.pool_stop.connect_clicked(move |_| cb());
        let cb = self.with(|t| t.spawn(|t| async move { t.pool_delete_clicked().await }));
        self.pool_delete.connect_clicked(move |_| cb());
        let cb = self.with(|t| t.spawn(|t| async move { t.pool_refresh_clicked().await }));
        self.pool_refresh.connect_clicked(move |_| cb());
        let cb = self.with(|t| t.spawn(|t| async move { t.pool_apply().await }));
        self.pool_apply.connect_clicked(move |_| cb());
        let cb = self.with(HostStorage::pool_add_clicked);
        self.pool_add.connect_clicked(move |_| cb());
        let cb = self.with(HostStorage::vol_add_clicked);
        self.vol_add.connect_clicked(move |_| cb());
        let cb = self.with(|t| t.spawn(|t| async move { t.vol_delete_clicked().await }));
        self.vol_delete.connect_clicked(move |_| cb());

        let cb = self.with(|t| {
            if let Some(cb) = t.browse_clicked.borrow().as_ref() {
                cb();
            }
        });
        self.browse_local.connect_clicked(move |_| cb());
        let cb = self.with(|t| {
            if let Some(cb) = t.cancel_clicked.borrow().as_ref() {
                cb();
            }
        });
        self.browse_cancel.connect_clicked(move |_| cb());
        let cb = self.with(HostStorage::emit_volume_chosen);
        self.choose_volume.connect_clicked(move |_| cb());
        let cb = self.with(HostStorage::emit_volume_chosen);
        self.vol_list.connect_activate(move |_, _| cb());

        let cb = self.with(|t| t.enable_pool_apply(PoolEdit::Name));
        self.pool_name_entry.connect_changed(move |_| cb());
        let cb = self.with(|t| t.enable_pool_apply(PoolEdit::Autostart));
        self.pool_autostart.connect_toggled(move |_| cb());

        let cb = self.with(|t| t.enable_pool_apply(PoolEdit::Xml));
        self.xmleditor.connect_changed(cb);
        // `_xmleditor_xml_requested_cb`/`_xmleditor_xml_reset_cb`:
        // refetch, so the XML tab gets the pool's current XML.
        let cb = self.with(HostStorage::refresh_page);
        self.xmleditor.connect_xml_requested(cb);
        let cb = self.with(HostStorage::refresh_page);
        self.xmleditor.connect_xml_reset(cb);

        let cb = self.with(HostStorage::vol_selected);
        self.vol_selection
            .connect_selection_changed(move |_, _, _| cb());
        let cb = self.with(HostStorage::vol_selected);
        self.vol_selection
            .connect_items_changed(move |_, _, _, _| cb());

        let cb = self.with(HostStorage::pool_selected);
        self.pool_list.connect_row_selected(move |_, _| cb());

        let weak = self.weak();
        copy_path.connect_clicked(move |_| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let this = HostStorage(inner);
            this.vol_menu.popdown();
            // `_vol_copy_path_cb`
            if let Some(vol) = this.current_vol() {
                if !vol.xml.target_path.is_empty() {
                    this.top_box.clipboard().set_text(&vol.xml.target_path);
                }
            }
        });
    }

    /// `_vol_popup_menu_cb`: a right click on a cell selects its
    /// row and pops up the menu there.
    fn vol_popup_menu(&self, pos: u32, cell: &gtk::Widget, x: f64, y: f64) {
        self.vol_selection.set_selected(pos);
        let (x, y) = cell
            .compute_point(
                &self.vol_list,
                &gtk::graphene::Point::new(x as f32, y as f32),
            )
            .map(|p| (p.x() as f64, p.y() as f64))
            .unwrap_or((x, y));
        self.vol_menu
            .set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        self.vol_menu.popup();
    }

    ///////////////
    // Public API //
    ///////////////

    pub fn connect_browse_clicked(&self, cb: impl Fn() + 'static) {
        *self.browse_clicked.borrow_mut() = Some(Box::new(cb));
    }

    /// `set_name_hint`.
    pub fn set_name_hint(&self, val: Option<&str>) {
        *self.name_hint.borrow_mut() = val.map(str::to_string);
    }

    pub fn connect_volume_chosen(&self, cb: impl Fn(&VolRow) + 'static) {
        *self.volume_chosen.borrow_mut() = Some(Box::new(cb));
    }

    pub fn connect_cancel_clicked(&self, cb: impl Fn() + 'static) {
        *self.cancel_clicked.borrow_mut() = Some(Box::new(cb));
    }

    /// `refresh_page`.
    pub fn refresh_page(&self) {
        // A priority tick does nothing for an inactive connection
        if !self.conn.is_active() {
            return;
        }
        self.spawn(|t| async move { t.populate_pools().await });
    }

    /// `uiutil.set_list_selection(pool-list, pool)`, by name, now or
    /// once the list is filled.
    pub fn select_pool(&self, name: &str) {
        *self.pending_select.borrow_mut() = Some(name.to_string());
        self.apply_pending_select();
    }

    pub fn close(&self) {
        if let Some(a) = self.addvol.borrow().as_ref() {
            a.close();
        }
        if let Some(a) = self.addpool.borrow().as_ref() {
            a.close();
        }
        self.vol_menu.popdown();
    }

    fn emit_volume_chosen(&self) {
        let Some(vol) = self.current_vol() else {
            return;
        };
        // A row-activate on an insensitive row can't choose it
        if !vol.sensitive {
            return;
        }
        if let Some(cb) = self.volume_chosen.borrow().as_ref() {
            cb(&vol);
        }
    }

    fn parent(&self) -> Option<gtk::Window> {
        self.top_box.root().and_downcast::<gtk::Window>()
    }

    //////////////////
    // UI populating //
    //////////////////

    /// `_refresh_conn_state`.
    fn refresh_conn_state(&self) {
        let conn_active = self.conn.is_active();
        self.pool_add.set_sensitive(conn_active);
        self.spawn(move |t| async move {
            t.populate_pools().await;
            // After the (now empty) list's own "No storage pool selected."
            if !conn_active {
                t.set_error_page("Connection not active.");
            }
        });
    }

    fn current_pool(&self) -> Option<PoolRow> {
        let idx = self.pool_list.selected_row()?.index();
        self.pools.borrow().get(idx as usize).cloned()
    }

    fn current_vol(&self) -> Option<VolRow> {
        self.current_pool()?;
        let obj = self
            .vol_selection
            .selected_item()
            .and_downcast::<glib::BoxedAnyObject>()?;
        let row = obj.borrow::<VolRow>().clone();
        Some(row)
    }

    /// `_populate_pool_state`.
    fn populate_pool_state(&self, pool: &PoolRow) {
        let active = pool.active;
        self.quiet.set(true);
        self.pool_details.set_sensitive(true);
        self.pool_name_entry.set_text(pool.name());
        self.pool_name_entry.set_editable(!active);
        self.pool_sizes.set_markup(&format!(
            "{} Free / <i>{} In Use</i>",
            storage::pretty_bytes(pool.xml.available),
            storage::pretty_bytes(pool.xml.allocation)
        ));
        self.pool_location
            .set_text(pool.xml.target_path.as_deref().unwrap_or(""));
        self.pool_state_icon.set_icon_name(Some(if active {
            "state_running"
        } else {
            "state_shutoff"
        }));
        self.pool_state
            .set_text(if active { "Active" } else { "Inactive" });
        self.pool_autostart.set_label(Some("On Boot"));
        self.pool_autostart.set_active(pool.autostart);
        self.quiet.set(false);

        self.vol_list.set_sensitive(active);
        self.spawn(|t| async move { t.populate_vols().await });

        self.pool_delete.set_sensitive(!active);
        self.pool_stop.set_sensitive(active);
        self.pool_start.set_sensitive(!active);
        self.pool_refresh.set_sensitive(active);
        self.vol_add.set_sensitive(active);
        self.vol_add.set_tooltip_text(Some("Create new volume"));
        self.vol_delete.set_sensitive(false);

        if active && !storage::supports_volume_creation(&pool.xml.pool_type, false) {
            self.vol_add.set_sensitive(false);
            self.vol_add
                .set_tooltip_text(Some("Pool does not support volume creation"));
        }

        // `set_xml_from_libvirtobject`: only while the XML tab shows
        if self.xmleditor.is_xml_selected() {
            match storage::pool_xml_to_define(&pool.xml_desc) {
                Ok(xml) => self.xmleditor.set_xml(&xml),
                Err(e) => {
                    eprintln!("{e}");
                    self.set_error_page(&format!("Error selecting pool: {e}"));
                }
            }
        }
    }

    /// `_set_error_page`.
    fn set_error_page(&self, msg: &str) {
        self.pages.set_visible_child_name("error");
        self.error_label.set_text(msg);
        self.pool_delete.set_sensitive(false);
        self.pool_stop.set_sensitive(false);
        self.pool_start.set_sensitive(false);
        self.disable_pool_apply();
    }

    /// `_refresh_current_pool`.
    fn refresh_current_pool(&self) {
        let pool = self.current_pool();
        *self.shown_pool.borrow_mut() = pool.clone();
        let Some(pool) = pool else {
            self.vol_store.remove_all();
            self.set_error_page("No storage pool selected.");
            return;
        };
        self.pages.set_visible_child_name("details");
        self.populate_pool_state(&pool);
        self.disable_pool_apply();
    }

    /// `_populate_pools`: refetches the pools, keeping the selection.
    async fn populate_pools(&self) {
        let pools = fetch_pools(&self.conn).await;
        let curpool = self
            .pending_select
            .borrow()
            .clone()
            .or_else(|| self.current_pool().map(|p| p.name().to_string()));
        let mut pools = pools;
        pools.sort_by(|a, b| a.name().cmp(b.name()));

        // Prevent events while the list is rebuilt
        self.quiet.set(true);
        self.pool_list.remove_all();
        for pool in &pools {
            self.pool_list.append(&pool_row_widget(pool));
        }
        *self.pools.borrow_mut() = pools;
        self.quiet.set(false);

        let pos = curpool
            .and_then(|name| self.pools.borrow().iter().position(|p| p.name() == name))
            .or_else(|| (!self.pools.borrow().is_empty()).then_some(0));
        self.pending_select.borrow_mut().take();
        match pos.and_then(|p| self.pool_list.row_at_index(p as i32)) {
            Some(row) => {
                self.quiet.set(true);
                self.pool_list.select_row(Some(&row));
                self.quiet.set(false);
                self.refresh_current_pool();
            }
            None => self.refresh_current_pool(),
        }
    }

    /// A `pool-event` for `name`. Upstream recaches that pool, whose
    /// `state-changed`/`refreshed` update its row and, when it's the
    /// one shown, the details (`_update_pool_row`); or it polls, and
    /// the `pool-added`/`pool-removed` that follow repopulate the list
    /// (`_conn_pools_changed_cb`).
    async fn pool_event(&self, name: &str) {
        let mut pools = fetch_pools(&self.conn).await;
        pools.sort_by(|a, b| a.name().cmp(b.name()));
        let same_list = {
            let old = self.pools.borrow();
            old.len() == pools.len() && old.iter().zip(&pools).all(|(a, b)| a.name() == b.name())
        };
        if !same_list {
            self.populate_pools().await;
            return;
        }

        // Update active sensitivity and percent available inline
        for (i, pool) in pools.iter().enumerate() {
            if let Some(row) = self.pool_list.row_at_index(i as i32) {
                row.set_child(Some(&pool_row_widget(pool)));
            }
        }
        *self.pools.borrow_mut() = pools;
        if self.current_pool().is_some_and(|p| p.name() == name) {
            self.refresh_current_pool();
        }
    }

    fn apply_pending_select(&self) {
        let Some(name) = self.pending_select.borrow().clone() else {
            return;
        };
        let pos = self.pools.borrow().iter().position(|p| p.name() == name);
        if let Some(row) = pos.and_then(|p| self.pool_list.row_at_index(p as i32)) {
            self.pending_select.borrow_mut().take();
            self.pool_list.select_row(Some(&row));
        }
    }

    /// `_populate_vols`.
    async fn populate_vols(&self) {
        let gen = self.vol_generation.get() + 1;
        self.vol_generation.set(gen);
        let Some(pool) = self.current_pool() else {
            self.vol_store.remove_all();
            return;
        };
        let vols = if pool.active {
            fetch_vols(&self.conn, pool.name()).await
        } else {
            vec![]
        };
        if self.vol_generation.get() != gen {
            return;
        }
        self.vol_selection.set_selected(gtk::INVALID_LIST_POSITION);
        let objs: Vec<glib::BoxedAnyObject> = vols
            .into_iter()
            .map(|mut vol| {
                if let Some(cb) = &self.vol_sensitive_cb {
                    vol.sensitive = cb(&vol.format);
                }
                glib::BoxedAnyObject::new(vol)
            })
            .collect();
        self.vol_store.splice(0, self.vol_store.n_items(), &objs);
        if let Some(name) = self.pending_vol.borrow_mut().take() {
            let model = self.vol_selection.model();
            let pos = model.and_then(|m| {
                (0..m.n_items()).find(|&p| {
                    m.item(p)
                        .and_downcast::<glib::BoxedAnyObject>()
                        .is_some_and(|o| o.borrow::<VolRow>().xml.name == name)
                })
            });
            if let Some(pos) = pos {
                self.vol_selection.set_selected(pos);
            }
        }
        self.vol_selected();
    }

    ///////////////////////////
    // Pool lifecycle actions //
    ///////////////////////////

    async fn pool_action(
        &self,
        what: &str,
        f: impl FnOnce(&virt::connect::Connect, &virt::storage_pool::StoragePool) -> Result<(), String>
            + Send
            + 'static,
    ) {
        let Some(pool) = self.current_pool() else {
            return;
        };
        let name = pool.name().to_string();
        eprintln!("{what} pool '{name}'");
        if let Err(e) = with_pool(&self.conn, &name, f).await {
            let verb = what.to_lowercase();
            error::show_err(
                self.parent().as_ref(),
                &format!("Error {verb} pool '{name}': {e}"),
                Some(&e),
                "",
            )
            .await;
        }
        self.populate_pools().await;
    }

    /// `_pool_stop_cb`.
    async fn pool_stop_clicked(&self) {
        self.pool_action("Stopping", |_, p| p.destroy().map_err(libvirt_err))
            .await;
    }

    /// `_pool_start_cb`.
    async fn pool_start_clicked(&self) {
        self.pool_action("Starting", |_, p| {
            p.create(0).map(|_| ()).map_err(libvirt_err)
        })
        .await;
    }

    /// `_pool_delete_cb`.
    async fn pool_delete_clicked(&self) {
        let Some(pool) = self.current_pool() else {
            return;
        };
        let msg = format!(
            "Are you sure you want to permanently delete the pool {}?",
            pool.name()
        );
        if !error::yes_no(self.parent().as_ref(), &msg, None).await {
            return;
        }
        self.pool_action("Deleting", |_, p| p.undefine().map_err(libvirt_err))
            .await;
    }

    /// `_pool_refresh_cb`.
    async fn pool_refresh_clicked(&self) {
        self.confirm_changes().await;
        self.pool_action("Refresh", |_, p| {
            p.refresh(0).map(|_| ()).map_err(libvirt_err)
        })
        .await;
    }

    //////////////////////////////
    // Volume action listeners //
    //////////////////////////////

    /// `_pool_add_cb`.
    fn pool_add_clicked(&self) {
        eprintln!("Launching 'Add Pool' wizard");
        let addpool = self.addpool.borrow().clone();
        let addpool = addpool.unwrap_or_else(|| {
            let a = CreatePool::new(&self.conn);
            let weak = self.weak();
            a.connect_pool_created(move || {
                if let Some(inner) = weak.upgrade() {
                    HostStorage(inner).refresh_page();
                }
            });
            *self.addpool.borrow_mut() = Some(a.clone());
            a
        });
        if let Some(parent) = self.parent() {
            addpool.show(&parent);
        }
    }

    /// `_vol_add_cb`.
    fn vol_add_clicked(&self) {
        let Some(pool) = self.current_pool() else {
            return;
        };
        eprintln!("Launching 'Add Volume' wizard for pool '{}'", pool.name());
        let addvol = self.addvol.borrow().clone();
        let addvol = match addvol {
            Some(a) => {
                a.set_parent_pool(pool.xml.clone());
                a
            }
            None => {
                let a = CreateVolume::new(&self.conn, pool.xml.clone());
                let weak = self.weak();
                a.connect_vol_created(move |pool, vol| {
                    if let Some(inner) = weak.upgrade() {
                        HostStorage(inner).vol_created(pool, vol);
                    }
                });
                *self.addvol.borrow_mut() = Some(a.clone());
                a
            }
        };
        let Some(parent) = self.parent() else {
            return;
        };
        addvol.set_modal(parent.is_modal());
        addvol.set_name_hint(self.name_hint.borrow().as_deref());
        addvol.show(&parent);
    }

    /// `_vol_created_cb`: refetch and select the new volume.
    fn vol_created(&self, pool: &str, vol: &str) {
        if self.current_pool().map(|p| p.xml.name).as_deref() != Some(pool) {
            return;
        }
        *self.pending_vol.borrow_mut() = Some(vol.to_string());
        self.spawn(|t| async move { t.populate_pools().await });
    }

    /// `_vol_delete_cb`.
    async fn vol_delete_clicked(&self) {
        let (Some(pool), Some(vol)) = (self.current_pool(), self.current_vol()) else {
            return;
        };
        let msg = format!(
            "Are you sure you want to permanently delete the volume {}?",
            vol.xml.name
        );
        if !error::yes_no(self.parent().as_ref(), &msg, None).await {
            return;
        }
        eprintln!("Deleting volume '{}'", vol.xml.name);
        let volname = vol.xml.name.clone();
        let res = with_pool(&self.conn, pool.name(), move |_, p| {
            virt::storage_vol::StorageVol::lookup_by_name(p, &volname)
                .and_then(|v| v.delete(0))
                .map_err(libvirt_err)?;
            p.refresh(0).map(|_| ()).map_err(libvirt_err)
        })
        .await;
        if let Err(e) = res {
            error::show_err(
                self.parent().as_ref(),
                &format!("Error deleting volume '{}': {e}", vol.xml.name),
                Some(&e),
                "",
            )
            .await;
        }
        self.populate_pools().await;
    }

    ////////////////////////////////
    // pool apply/config actions //
    ////////////////////////////////

    /// `_pool_apply`, for the pool the details show.
    async fn pool_apply(&self) {
        let Some(pool) = self.shown_pool.borrow().clone() else {
            return;
        };
        let edits = self.active_edits.borrow().clone();
        let autostart = self.pool_autostart.is_active();
        let newname = self.pool_name_entry.text().to_string();
        let newxml = self.xmleditor.get_xml();
        let edits_renamed = edits.contains(&PoolEdit::Name);
        let renamed_to = newname.clone();
        eprintln!("Applying changes for pool '{}'", pool.name());
        let res = with_pool(&self.conn, pool.name(), move |conn, p| {
            if edits.contains(&PoolEdit::Autostart) {
                p.set_autostart(autostart).map_err(libvirt_err)?;
            }
            if edits.contains(&PoolEdit::Name) {
                storage::rename_pool(conn, p, &newname)?;
            }
            if edits.contains(&PoolEdit::Xml) {
                // `define_xml`
                eprintln!("Redefining pool with XML:\n{newxml}");
                virt::storage_pool::StoragePool::define_xml(conn, &newxml, 0)
                    .map_err(libvirt_err)?;
            }
            Ok(())
        })
        .await;
        if let Err(e) = res {
            error::show_err(
                self.parent().as_ref(),
                &format!("Error changing pool settings: {e}"),
                Some(&e),
                "",
            )
            .await;
            return;
        }
        self.disable_pool_apply();
        // A rename keeps the renamed pool selected
        if edits_renamed && self.current_pool().map(|p| p.xml.name) == Some(pool.xml.name.clone()) {
            *self.pending_select.borrow_mut() = Some(renamed_to);
        }
        self.populate_pools().await;
    }

    fn enable_pool_apply(&self, edit: PoolEdit) {
        if self.quiet.get() {
            return;
        }
        let mut edits = self.active_edits.borrow_mut();
        if !edits.contains(&edit) {
            edits.push(edit);
        }
        self.pool_apply.set_sensitive(true);
        self.xmleditor.set_details_changed(true);
    }

    fn disable_pool_apply(&self) {
        self.active_edits.borrow_mut().clear();
        self.pool_apply.set_sensitive(false);
        self.xmleditor.set_details_changed(false);
    }

    /// `_confirm_changes`.
    async fn confirm_changes(&self) {
        let visible = self.top_box.is_mapped();
        if visible
            && !self.active_edits.borrow().is_empty()
            && error::confirm_unapplied_changes(self.parent().as_ref()).await
        {
            self.pool_apply().await;
        }
        self.disable_pool_apply();
    }

    //////////////
    // Listeners //
    //////////////

    /// `_vol_selected_cb`.
    fn vol_selected(&self) {
        let vol = self.current_vol();
        self.vol_delete.set_sensitive(vol.is_some());
        let can_choose = vol.is_some_and(|v| v.sensitive);
        self.choose_volume.set_sensitive(can_choose);
    }

    /// `_pool_selected_cb`, after the select function's
    /// `_confirm_changes` for the pool being left.
    fn pool_selected(&self) {
        if self.quiet.get() {
            return;
        }
        if self.active_edits.borrow().is_empty() {
            self.refresh_current_pool();
            return;
        }
        self.spawn(|t| async move {
            t.confirm_changes().await;
            t.refresh_current_pool();
        });
    }
}

/// A `pool-list` row: the percent in use, then the name over the
/// pretty type, greyed out when inactive.
fn pool_row_widget(pool: &PoolRow) -> gtk::Widget {
    let per = gtk::Label::new(None);
    per.set_markup(&pool_size_percent(&pool.xml));
    let label = gtk::Label::new(None);
    label.set_markup(&format!(
        "{}\n<span size='small'>{}</span>",
        glib::markup_escape_text(pool.name()),
        glib::markup_escape_text(&storage::pretty_pool_type(&pool.xml.pool_type))
    ));
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_hexpand(true);
    label.set_sensitive(pool.active);
    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 3);
    hbox.set_margin_start(2);
    hbox.set_margin_end(2);
    hbox.set_margin_top(2);
    hbox.set_margin_bottom(2);
    hbox.append(&per);
    hbox.append(&label);
    hbox.upcast()
}

/// The `vol-list` columns: Volumes, Size, Format and Used By, each
/// sortable, with name ascending first.
fn build_vol_columns(list: &gtk::ColumnView, popup_cb: &VolPopupCallback) {
    let column = |list: &gtk::ColumnView,
                  title: &str,
                  expand: bool,
                  text: fn(&VolRow) -> String,
                  cmp: fn(&VolRow, &VolRow) -> std::cmp::Ordering|
     -> gtk::ColumnViewColumn {
        column_impl(list, title, expand, text, cmp, popup_cb.clone())
    };
    fn column_impl(
        list: &gtk::ColumnView,
        title: &str,
        expand: bool,
        text: fn(&VolRow) -> String,
        cmp: fn(&VolRow, &VolRow) -> std::cmp::Ordering,
        popup_cb: VolPopupCallback,
    ) -> gtk::ColumnViewColumn {
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
            let l = gtk::Label::new(None);
            l.set_xalign(0.0);
            l.set_hexpand(true);
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            let click = gtk::GestureClick::new();
            click.set_button(3);
            let weak_item = item.downgrade();
            let popup_cb = popup_cb.clone();
            click.connect_pressed(move |g, _, x, y| {
                let (Some(item), Some(cell)) = (weak_item.upgrade(), g.widget()) else {
                    return;
                };
                if let Some(cb) = popup_cb.borrow().as_ref() {
                    cb(item.position(), &cell, x, y);
                }
            });
            l.add_controller(click);
            item.set_child(Some(&l));
        });
        factory.connect_bind(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
            let (Some(child), Some(obj)) = (
                item.child().and_downcast::<gtk::Label>(),
                item.item().and_downcast::<glib::BoxedAnyObject>(),
            ) else {
                return;
            };
            let row = obj.borrow::<VolRow>();
            child.set_text(&text(&row));
            child.set_sensitive(row.sensitive);
        });
        let col = gtk::ColumnViewColumn::new(Some(title), Some(factory));
        col.set_expand(expand);
        col.set_resizable(true);
        let sorter = gtk::CustomSorter::new(move |a, b| {
            let (Some(a), Some(b)) = (
                a.downcast_ref::<glib::BoxedAnyObject>(),
                b.downcast_ref::<glib::BoxedAnyObject>(),
            ) else {
                return gtk::Ordering::Equal;
            };
            cmp(&a.borrow::<VolRow>(), &b.borrow::<VolRow>()).into()
        });
        col.set_sorter(Some(&sorter));
        list.append_column(&col);
        col
    }

    let name = column(
        list,
        "Volumes",
        true,
        |r| r.name.clone(),
        |a, b| a.name.cmp(&b.name),
    );
    column(
        list,
        "Size",
        false,
        |r| r.sizestr.clone(),
        |a, b| a.xml.capacity.cmp(&b.xml.capacity),
    );
    column(
        list,
        "Format",
        false,
        |r| r.format.clone(),
        |a, b| a.format.cmp(&b.format),
    );
    column(
        list,
        "Used By",
        false,
        |r| r.in_use_by.clone(),
        |a, b| a.in_use_by.cmp(&b.in_use_by),
    );
    list.sort_by_column(Some(&name), gtk::SortType::Ascending);
}
