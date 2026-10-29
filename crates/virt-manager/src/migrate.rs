//! Upstream's `virtManager/migrate.py` + `ui/migrate.ui`
//! (`vmmMigrateDialog`): the Migrate the virtual machine dialog. The
//! libvirt side, `vmmDomain.migrate()`, is in `domain/migration.rs`.
//!
//! The `GtkComboBox` with insensitive rows for unusable connections
//! becomes a `GtkDropDown` whose rows can't be picked (not activatable,
//! greyed). `libvirt_gethostname()` runs on a worker, so the source and
//! address fields fill in a moment after the rest.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use virt::domain::Domain;
use virtinst_core::connection::Uri;

use crate::asyncjob::{self, CancelCb};
use crate::connection::{ConnHandle, VmmConnection};
use crate::domain::migration::{self, MigrateOptions};
use crate::domain::VmmDomain;
use crate::engine;
use crate::error;
use crate::xmleditor::XmlEditor;

const MODE_TOOLTIP: &str = "Tunnel migration through the libvirtd connection channel, \
rather than having the hypervisor open a separate network connection to the destination. \
The source libvirt instance connects directly to the destination libvirt instance.\n\n\
This can simplify setup since no additional firewall ports need to be open, and will \
encrypt migration traffic if your libvirt connection is encrypted. But it can be \
difficult to make this work with SSH transport.";

const UNSAFE_TOOLTIP: &str = "By default libvirt will refuse to migrate a VM for certain \
configurations that could lead to malfunctioning guests, like if a disk's cache mode is \
not 'none'.\n\n\
Enabling this option tells libvirt to skip those checks.";

const TEMPORARY_TOOLTIP: &str = "By default, the migrated VM config is removed from the \
source host, and saved persistently on the destination host. The destination host is \
considered the new home of the VM.\n\n\
If 'temporary' is selected, the migration is considered only a temporary move: the source \
host maintains a copy of the VM config, and the running copy moved to the destination is \
only transient, and will disappear when it is shutdown.";

const TUNNEL_WARNING: &str = "A remotely accessible libvirt URI is required for tunneled \
migration, but the selected connection is a local URI. Libvirt will reject this unless \
you add a transport.";

/// One `migrate-dest` row: `COL_LABEL`, `COL_URI`, `COL_CAN_MIGRATE`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DestRow {
    pub label: String,
    pub uri: Option<String>,
    pub can_migrate: bool,
}

/// What `_build_dest_row` reads from a connection.
#[derive(Clone, Debug)]
pub struct DestConn {
    pub uri: String,
    pub pretty_desc: String,
    pub disconnected: bool,
    pub active: bool,
}

impl DestConn {
    fn of(conn: &VmmConnection) -> DestConn {
        DestConn {
            uri: conn.get_uri(),
            pretty_desc: conn.get_pretty_desc(),
            disconnected: conn.is_disconnected(),
            active: conn.is_active(),
        }
    }
}

/// `get_driver`: the URI scheme without its transport.
fn driver(uri: &str) -> String {
    Uri::parse(uri).scheme
}

/// `_build_dest_row`: why a connection can't be the destination, or
/// that it can.
pub fn build_dest_row(origuri: &str, dest: &DestConn) -> DestRow {
    let pretty_uri = &dest.pretty_desc;
    let mut can_migrate = false;
    let desc = if driver(&dest.uri) != driver(origuri) {
        format!("{pretty_uri} (Hypervisors do not match)")
    } else if dest.disconnected {
        format!("{pretty_uri} (Disconnected)")
    } else if dest.uri == origuri {
        format!("{pretty_uri} (Same connection)")
    } else {
        can_migrate = dest.active;
        pretty_uri.clone()
    };
    DestRow {
        label: desc,
        uri: Some(dest.uri.clone()),
        can_migrate,
    }
}

/// `_populate_destconn`: every known connection, a placeholder first
/// when none is usable, in the model's sort order (the placeholder,
/// then by label); and the row to select, the first usable one.
pub fn dest_rows(origuri: &str, conns: &[DestConn]) -> (Vec<DestRow>, usize) {
    let mut rows: Vec<DestRow> = conns.iter().map(|c| build_dest_row(origuri, c)).collect();
    if !rows.iter().any(|r| r.can_migrate) {
        rows.insert(
            0,
            DestRow {
                label: "No usable connections available.".into(),
                uri: None,
                can_migrate: false,
            },
        );
    }
    rows.sort_by(|a, b| (a.uri.is_some(), &a.label).cmp(&(b.uri.is_some(), &b.label)));
    let active = rows.iter().position(|r| r.can_migrate).unwrap_or(0);
    (rows, active)
}

/// `_build_regular_migrate_uri`: `None` lets libvirt pick the address.
pub fn build_regular_migrate_uri(
    is_xen: bool,
    address: Option<&str>,
    port: Option<u32>,
) -> Option<String> {
    let address = address.filter(|a| !a.is_empty())?;
    let mut uri = if is_xen {
        address.to_string()
    } else {
        format!("tcp:{address}")
    };
    if let Some(port) = port.filter(|p| *p != 0) {
        uri.push_str(&format!(":{port}"));
    }
    Some(uri)
}

/// `vmmConnection.libvirt_gethostname`, on a worker.
async fn libvirt_gethostname(conn: &VmmConnection) -> String {
    let Some(handle) = conn.handle() else {
        return String::new();
    };
    gio::spawn_blocking(move || handle.connect().get_hostname().unwrap_or_default())
        .await
        .unwrap_or_default()
}

/// `_cancel_migration`, on the asyncjob's cancel worker.
fn cancel_migration(src: &ConnHandle, uuid: &str) -> Result<(), String> {
    glib::g_debug!("virt-manager", "Cancelling migrate job");
    Domain::lookup_by_uuid_string(src.connect(), uuid)
        .map_err(|e| e.message().to_string())
        .and_then(|dom| migration::abort_job(&dom))
        .map_err(|e| {
            eprintln!("Error cancelling migrate job: {e}");
            format!("Error cancelling migrate job: {e}")
        })
}

struct Inner {
    window: gtk::Window,
    header_label: gtk::Label,
    xmleditor: XmlEditor,
    config_box: gtk::Box,
    label_name: gtk::Label,
    label_src: gtk::Label,
    dest: gtk::DropDown,
    dest_store: gio::ListStore,
    mode: gtk::DropDown,
    direct_box: gtk::Grid,
    tunnel_box: gtk::Grid,
    set_address: gtk::CheckButton,
    address: gtk::Entry,
    address_label: gtk::Label,
    set_port: gtk::CheckButton,
    port: gtk::SpinButton,
    port_label: gtk::Label,
    tunnel_uri: gtk::Entry,
    tunnel_warning_box: gtk::Box,
    tunnel_warning_label: gtk::Label,
    advanced_expander: gtk::Expander,
    unsafe_migrate: gtk::CheckButton,
    temporary: gtk::CheckButton,
    cancel: gtk::Button,
    finish: gtk::Button,
    vm: RefCell<Option<VmmDomain>>,
    removed_handler: RefCell<Option<(VmmConnection, glib::SignalHandlerId)>>,
    /// Set while the destination rows are rebuilt, so the selection
    /// changes on the way don't each run `_destconn_changed`.
    populating: Cell<bool>,
    /// Bumped per lookup, so a hostname that arrives late is dropped.
    src_generation: Cell<u64>,
    dest_generation: Cell<u64>,
    /// Set while the address is filled in by code, not typed.
    setting_address: Cell<bool>,
    /// The user typed in the address since the destination changed, so
    /// the looked-up hostname must not replace it.
    address_edited: Cell<bool>,
}

/// `vmmMigrateDialog`.
#[derive(Clone)]
pub struct VmmMigrateDialog(Rc<Inner>);

thread_local! {
    static INSTANCE: RefCell<Option<VmmMigrateDialog>> = const { RefCell::new(None) };
}

/// A `table2` label: right-aligned, lighter.
fn summary_label(text: &str) -> gtk::Label {
    let label = gtk::Label::with_mnemonic(text);
    label.set_halign(gtk::Align::End);
    label.add_css_class("vmm-lighter");
    label
}

fn mnemonic_label(text: &str, target: &impl IsA<gtk::Widget>, halign: gtk::Align) -> gtk::Label {
    let label = gtk::Label::with_mnemonic(text);
    label.set_halign(halign);
    label.set_mnemonic_widget(Some(target));
    label
}

fn accessible_name(widget: &impl IsA<gtk::Accessible>, name: &str) {
    widget.update_property(&[gtk::accessible::Property::Label(name)]);
}

/// `_init_state`'s connection combo: a middle-ellipsized, 30 character
/// label, insensitive (and not selectable) for `COL_CAN_MIGRATE` false.
/// `button` is the closed combo's own row, where the theme doesn't dim
/// an insensitive label, so it is dimmed explicitly as GTK3's cell
/// view does.
fn dest_factory(button: bool) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        label.set_width_chars(30);
        label.set_max_width_chars(30);
        item.set_child(Some(&label));
    });
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
        let (Some(label), Some(obj)) = (
            item.child().and_downcast::<gtk::Label>(),
            item.item().and_downcast::<glib::BoxedAnyObject>(),
        ) else {
            return;
        };
        let row = obj.borrow::<DestRow>();
        label.set_text(&row.label);
        label.set_sensitive(row.can_migrate);
        if button && !row.can_migrate {
            label.add_css_class("dim-label");
        } else {
            label.remove_css_class("dim-label");
        }
        item.set_activatable(row.can_migrate);
        item.set_selectable(row.can_migrate);
    });
    factory
}

impl VmmMigrateDialog {
    /// `vmmMigrateDialog.show_instance(parentobj, vm)`.
    pub fn show_instance(parent: &gtk::Window, vm: &VmmDomain) {
        let this = INSTANCE.with(|i| i.borrow_mut().get_or_insert_with(Self::new).clone());
        this.show(parent, vm);
    }

    fn new() -> VmmMigrateDialog {
        let window = gtk::Window::builder()
            .title("Migrate the virtual machine")
            .width_request(300)
            .height_request(400)
            .default_width(400)
            .default_height(300)
            .resizable(false)
            .build();

        // header
        let image = gtk::Image::from_icon_name("vm_clone_wizard");
        image.set_pixel_size(48);
        let header_label = gtk::Label::new(None);
        header_label.set_halign(gtk::Align::Start);
        header_label.add_css_class("vmm-header-text");
        let hbox77 = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        hbox77.set_margin_start(6);
        hbox77.set_margin_end(6);
        hbox77.set_margin_top(6);
        hbox77.set_margin_bottom(6);
        hbox77.append(&image);
        hbox77.append(&header_label);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        header.add_css_class("vmm-header");
        header.append(&hbox77);

        // table2: what moves where
        let label_name = gtk::Label::new(Some("label"));
        label_name.set_halign(gtk::Align::Start);
        label_name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let label_src = gtk::Label::new(Some("label"));
        label_src.set_halign(gtk::Align::Start);
        label_src.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let dest_store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let dest = gtk::DropDown::new(Some(dest_store.clone()), None::<gtk::Expression>);
        dest.set_factory(Some(&dest_factory(true)));
        dest.set_list_factory(Some(&dest_factory(false)));
        accessible_name(&dest, "conn-combo");
        let new_host = summary_label("New _host:");
        new_host.set_mnemonic_widget(Some(&dest));
        let table2 = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(6)
            .build();
        table2.attach(&summary_label("Migrating VM:"), 0, 0, 1, 1);
        table2.attach(&summary_label("Original host:"), 0, 1, 1, 1);
        table2.attach(&new_host, 0, 2, 1, 1);
        table2.attach(&label_name, 1, 0, 1, 1);
        table2.attach(&label_src, 1, 1, 1, 1);
        table2.attach(&dest, 1, 2, 1, 1);

        // Connectivity: mode
        let mode = gtk::DropDown::from_strings(&["Direct", "Tunnelled"]);
        mode.set_halign(gtk::Align::Start);
        mode.set_hexpand(false);
        mode.set_tooltip_text(Some(MODE_TOOLTIP));
        let mode_label = mnemonic_label("M_ode:", &mode, gtk::Align::End);
        mode_label.set_tooltip_text(Some(MODE_TOOLTIP));
        let box1 = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        box1.append(&mode_label);
        box1.append(&mode);

        // Connectivity: direct address and port
        let set_address = gtk::CheckButton::new();
        set_address.set_halign(gtk::Align::Start);
        accessible_name(&set_address, "address-check");
        let set_port = gtk::CheckButton::new();
        set_port.set_halign(gtk::Align::Start);
        accessible_name(&set_port, "port-check");
        let address = gtk::Entry::new();
        accessible_name(&address, "address-text");
        let address_label = gtk::Label::new(Some("Let libvirt decide"));
        address_label.set_halign(gtk::Align::Start);
        let box2 = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        box2.append(&address);
        box2.append(&address_label);
        let port = gtk::SpinButton::new(
            Some(&gtk::Adjustment::new(0.0, 0.0, 70000.0, 1.0, 10.0, 0.0)),
            1.0,
            0,
        );
        let port_label = gtk::Label::new(Some("Let libvirt decide"));
        port_label.set_halign(gtk::Align::Start);
        port_label.set_hexpand(true);
        let hbox4 = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        hbox4.append(&port);
        hbox4.append(&port_label);
        let direct_box = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(6)
            .build();
        direct_box.attach(
            &mnemonic_label("_Address:", &set_address, gtk::Align::End),
            0,
            0,
            1,
            1,
        );
        direct_box.attach(
            &mnemonic_label("_Port:", &set_port, gtk::Align::Start),
            0,
            1,
            1,
            1,
        );
        direct_box.attach(&set_address, 1, 0, 1, 1);
        direct_box.attach(&set_port, 1, 1, 1, 1);
        direct_box.attach(&box2, 2, 0, 1, 1);
        direct_box.attach(&hbox4, 2, 1, 1, 1);

        // Connectivity: tunnelled URI
        let tunnel_uri = gtk::Entry::new();
        tunnel_uri.set_hexpand(true);
        let warn_icon = gtk::Image::from_icon_name("dialog-warning");
        warn_icon.set_valign(gtk::Align::Start);
        let tunnel_warning_label = gtk::Label::new(None);
        tunnel_warning_label.set_halign(gtk::Align::Start);
        tunnel_warning_label.set_xalign(0.0);
        tunnel_warning_label.set_wrap(true);
        // The .ui's width-chars is 0, but GtkNotebook (the XML editor's
        // tabs) measures pages without a width, which sizes a wrapping
        // label at its minimum width: the dialog grew several lines
        // taller than the text. 40, its max, lays it out the same.
        tunnel_warning_label.set_width_chars(40);
        tunnel_warning_label.set_max_width_chars(40);
        let tunnel_warning_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        tunnel_warning_box.set_hexpand(true);
        tunnel_warning_box.append(&warn_icon);
        tunnel_warning_box.append(&tunnel_warning_label);
        let tunnel_box = gtk::Grid::builder().column_spacing(6).build();
        let uri_label = mnemonic_label("_URI:", &tunnel_uri, gtk::Align::Start);
        uri_label.set_hexpand(false);
        tunnel_box.attach(&uri_label, 0, 0, 1, 1);
        tunnel_box.attach(&tunnel_uri, 1, 0, 1, 1);
        tunnel_box.attach(&tunnel_warning_box, 1, 1, 1, 1);

        let grid2 = gtk::Grid::builder().row_spacing(12).build();
        grid2.set_margin_start(12);
        grid2.attach(&box1, 0, 0, 1, 1);
        grid2.attach(&direct_box, 0, 1, 1, 1);
        grid2.attach(&tunnel_box, 0, 2, 1, 1);
        // frame1: a shadowless frame, bold title
        let frame_label = gtk::Label::new(None);
        frame_label.set_markup("<b>Connectivity</b>");
        frame_label.set_xalign(0.0);
        let frame1 = gtk::Box::new(gtk::Orientation::Vertical, 6);
        frame1.append(&frame_label);
        frame1.append(&grid2);

        // Advanced options
        let unsafe_migrate = gtk::CheckButton::new();
        unsafe_migrate.set_halign(gtk::Align::Start);
        unsafe_migrate.set_tooltip_text(Some(UNSAFE_TOOLTIP));
        let unsafe_label = mnemonic_label("A_llow unsafe:", &unsafe_migrate, gtk::Align::Start);
        unsafe_label.set_tooltip_text(Some(UNSAFE_TOOLTIP));
        let temporary = gtk::CheckButton::new();
        temporary.set_halign(gtk::Align::Start);
        temporary.set_tooltip_text(Some(TEMPORARY_TOOLTIP));
        let temporary_label = mnemonic_label("_Temporary move:", &temporary, gtk::Align::Start);
        temporary_label.set_tooltip_text(Some(TEMPORARY_TOOLTIP));
        let grid1 = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(6)
            .build();
        grid1.set_margin_start(12);
        grid1.set_margin_top(6);
        grid1.attach(&unsafe_label, 0, 0, 1, 1);
        grid1.attach(&unsafe_migrate, 1, 0, 1, 1);
        grid1.attach(&temporary_label, 0, 1, 1, 1);
        grid1.attach(&temporary, 1, 1, 1, 1);
        let advanced_expander = gtk::Expander::new(Some("Advanced options"));
        advanced_expander.set_child(Some(&grid1));

        let config_box = gtk::Box::new(gtk::Orientation::Vertical, 18);
        config_box.append(&frame1);
        config_box.append(&advanced_expander);

        let details_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        details_box.append(&table2);
        details_box.append(&config_box);
        let xmleditor = XmlEditor::new(&details_box);
        let details_box_align = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        details_box_align.set_vexpand(true);
        details_box_align.append(xmleditor.widget());

        let cancel = gtk::Button::with_mnemonic("_Cancel");
        let finish = gtk::Button::with_mnemonic("_Migrate");
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.set_halign(gtk::Align::End);
        buttons.append(&cancel);
        buttons.append(&finish);

        let vbox2 = gtk::Box::new(gtk::Orientation::Vertical, 18);
        vbox2.set_margin_start(12);
        vbox2.set_margin_end(12);
        vbox2.set_margin_top(12);
        vbox2.set_margin_bottom(12);
        vbox2.set_vexpand(true);
        vbox2.append(&details_box_align);
        vbox2.append(&buttons);
        let vbox1 = gtk::Box::new(gtk::Orientation::Vertical, 6);
        vbox1.append(&header);
        vbox1.append(&vbox2);
        window.set_child(Some(&vbox1));

        let this = VmmMigrateDialog(Rc::new(Inner {
            window,
            header_label,
            xmleditor,
            config_box,
            label_name,
            label_src,
            dest,
            dest_store,
            mode,
            direct_box,
            tunnel_box,
            set_address,
            address,
            address_label,
            set_port,
            port,
            port_label,
            tunnel_uri,
            tunnel_warning_box,
            tunnel_warning_label,
            advanced_expander,
            unsafe_migrate,
            temporary,
            cancel,
            finish,
            vm: RefCell::default(),
            removed_handler: RefCell::default(),
            populating: Cell::new(false),
            src_generation: Cell::new(0),
            dest_generation: Cell::new(0),
            setting_address: Cell::new(false),
            address_edited: Cell::new(false),
        }));
        this.connect_signals();
        this
    }

    fn connect_signals(&self) {
        let i = &self.0;
        let t = self.clone();
        i.window.connect_close_request(move |_| {
            t.close();
            glib::Propagation::Stop
        });
        // `bind_escape_key_close`
        let esc = gtk::EventControllerKey::new();
        let t = self.clone();
        esc.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                t.close();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        i.window.add_controller(esc);
        let t = self.clone();
        i.cancel.connect_clicked(move |_| t.close());
        let t = self.clone();
        i.finish.connect_clicked(move |_| t.finish());
        let t = self.clone();
        i.dest
            .connect_selected_notify(move |_| t.destconn_changed());
        let t = self.clone();
        i.address.connect_changed(move |_| {
            if !t.0.setting_address.get() {
                t.0.address_edited.set(true);
            }
        });
        let t = self.clone();
        i.set_address
            .connect_toggled(move |_| t.set_address_toggled());
        let t = self.clone();
        i.set_port.connect_toggled(move |_| t.set_port_toggled());
        let t = self.clone();
        i.mode.connect_selected_notify(move |_| t.mode_changed());
        let t = self.clone();
        i.xmleditor.connect_xml_requested(move || {
            let t = t.clone();
            glib::spawn_future_local(async move { t.xmleditor_xml_requested().await });
        });
    }

    fn vm(&self) -> Option<VmmDomain> {
        self.0.vm.borrow().clone()
    }

    // Public API

    /// `show`.
    fn show(&self, parent: &gtk::Window, vm: &VmmDomain) {
        glib::g_debug!("virt-manager", "Showing migrate wizard");
        self.set_vm(Some(vm));
        self.reset_state();
        self.0.window.set_transient_for(Some(parent));
        self.0.window.present();
    }

    /// `close`.
    fn close(&self) {
        glib::g_debug!("virt-manager", "Closing migrate wizard");
        self.0.window.set_visible(false);
        self.set_vm(None);
    }

    /// `_set_vm`, with `_vm_removed_cb`.
    fn set_vm(&self, newvm: Option<&VmmDomain>) {
        if let Some((conn, id)) = self.0.removed_handler.take() {
            conn.disconnect(id);
        }
        if let Some(conn) = newvm.and_then(|vm| vm.conn()) {
            let weak = Rc::downgrade(&self.0);
            let id = conn.connect_local("vm-removed", false, move |args| {
                let removed = args[1].get::<VmmDomain>().ok();
                if let Some(inner) = weak.upgrade() {
                    let this = VmmMigrateDialog(inner);
                    if removed.is_some() && removed == this.vm() {
                        this.close();
                    }
                }
                None
            });
            self.0.removed_handler.replace(Some((conn, id)));
        }
        self.0.vm.replace(newvm.cloned());
    }

    /// `_reset_state`.
    fn reset_state(&self) {
        let i = &self.0;
        let Some(vm) = self.vm() else {
            return;
        };
        let Some(conn) = vm.conn() else {
            return;
        };
        // `_xmleditor.reset_state()`: clear the XML, back to Details
        i.xmleditor.set_xml("");
        i.xmleditor.widget().set_current_page(Some(0));

        i.header_label.set_markup(&format!(
            "<span size='large'>Migrate '{}'</span>",
            glib::markup_escape_text(&vm.get_name())
        ));
        i.advanced_expander.set_expanded(false);
        i.cancel.grab_focus();
        i.config_box.set_visible(true);

        i.label_name.set_text(&vm.get_name_or_title());
        i.label_src.set_text("");
        i.label_src.set_tooltip_text(Some(&conn.get_uri()));
        let gen = i.src_generation.get() + 1;
        i.src_generation.set(gen);
        let this = self.clone();
        let srcconn = conn.clone();
        glib::spawn_future_local(async move {
            let hostname = libvirt_gethostname(&srcconn).await;
            if this.0.src_generation.get() == gen {
                let srctext = format!("{hostname} ({})", srcconn.get_pretty_desc());
                this.0.label_src.set_text(&srctext);
            }
        });

        i.set_address.set_active(true);
        self.set_address_toggled();
        i.set_port.set_active(true);
        self.set_port_toggled();

        i.mode.set_selected(0);
        self.mode_changed();
        i.unsafe_migrate.set_active(false);
        i.temporary.set_active(false);

        if conn.is_xen() {
            // Default xen port is 8002
            i.port.set_value(8002.0);
        } else {
            // QEMU migrate port range is 49152+64
            i.port.set_value(49152.0);
        }

        self.populate_destconn(&conn);
    }

    // Listeners

    fn selected_dest_row(&self) -> Option<DestRow> {
        let obj = self
            .0
            .dest
            .selected_item()
            .and_downcast::<glib::BoxedAnyObject>()?;
        let row = obj.borrow::<DestRow>().clone();
        Some(row)
    }

    /// `_destconn_changed`.
    fn destconn_changed(&self) {
        let i = &self.0;
        if i.populating.get() {
            return;
        }
        let Some(row) = self.selected_dest_row() else {
            return;
        };
        let can_migrate = row.can_migrate;
        let tooltip = (!can_migrate).then_some("A valid destination connection must be selected.");
        i.config_box.set_visible(can_migrate);
        i.finish.set_sensitive(can_migrate);
        i.finish.set_tooltip_text(tooltip);

        let destconn = row
            .uri
            .as_deref()
            .filter(|_| can_migrate)
            .and_then(|uri| engine::get().connmanager().get_conn(uri));
        let tunnel_uri = destconn.as_ref().map(|c| c.get_uri()).unwrap_or_default();
        let tunnel_warning = destconn
            .as_ref()
            .filter(|c| !c.is_remote())
            .map(|_| format!("<span size='small'>{TUNNEL_WARNING}</span>"));

        // `address = destconn.libvirt_gethostname()`. Upstream fills it
        // before the dialog takes input again, so Migrate waits for the
        // lookup, and text the user typed meanwhile is kept.
        let gen = i.dest_generation.get() + 1;
        i.dest_generation.set(gen);
        self.set_address_text("");
        i.address_edited.set(false);
        if let Some(destconn) = destconn {
            i.finish.set_sensitive(false);
            let this = self.clone();
            glib::spawn_future_local(async move {
                let address = libvirt_gethostname(&destconn).await;
                let i = &this.0;
                if i.dest_generation.get() != gen {
                    return;
                }
                if !i.address_edited.get() {
                    this.set_address_text(&address);
                }
                i.finish.set_sensitive(true);
            });
        }

        i.tunnel_uri.set_text(&tunnel_uri);
        i.tunnel_warning_box.set_visible(tunnel_warning.is_some());
        i.tunnel_warning_label
            .set_markup(tunnel_warning.as_deref().unwrap_or_default());
    }

    /// Fills the address without counting it as the user's edit.
    fn set_address_text(&self, text: &str) {
        self.0.setting_address.set(true);
        self.0.address.set_text(text);
        self.0.setting_address.set(false);
    }

    /// `_set_address_toggled`.
    fn set_address_toggled(&self) {
        let i = &self.0;
        let enable = i.set_address.is_active();
        i.address.set_visible(enable);
        i.address_label.set_visible(!enable);

        let port_enable = i.set_port.is_active();
        i.set_port.set_active(enable && port_enable);
        self.set_port_toggled();
    }

    /// `_set_port_toggled`.
    fn set_port_toggled(&self) {
        let i = &self.0;
        let enable = i.set_port.is_active();
        i.port.set_visible(enable);
        i.port_label.set_visible(!enable);
    }

    /// `_is_tunnel_selected`.
    fn is_tunnel_selected(&self) -> bool {
        self.0.mode.selected() == 1
    }

    /// `_mode_changed`.
    fn mode_changed(&self) {
        let is_tunnel = self.is_tunnel_selected();
        self.0.direct_box.set_visible(!is_tunnel);
        self.0.tunnel_box.set_visible(is_tunnel);
    }

    // destconn combo handling

    /// `_populate_destconn`.
    fn populate_destconn(&self, conn: &VmmConnection) {
        let i = &self.0;
        let conns: Vec<DestConn> = engine::get()
            .connmanager()
            .conns()
            .iter()
            .map(DestConn::of)
            .collect();
        let (rows, active) = dest_rows(&conn.get_uri(), &conns);
        i.populating.set(true);
        i.dest_store.remove_all();
        for row in rows {
            i.dest_store.append(&glib::BoxedAnyObject::new(row));
        }
        i.dest.set_selected(active as u32);
        i.populating.set(false);
        self.destconn_changed();
    }

    // migrate handling

    /// `_build_regular_migrate_uri`, from the visible fields.
    fn regular_migrate_uri(&self, conn: &VmmConnection) -> Option<String> {
        let i = &self.0;
        // the widget's, not the entry's password `visibility`
        let address = WidgetExt::is_visible(&i.address).then(|| i.address.text().to_string());
        let port = i.port.is_visible().then(|| i.port.value() as u32);
        build_regular_migrate_uri(conn.is_xen(), address.as_deref(), port)
    }

    /// `_finish`.
    fn finish(&self) {
        let i = &self.0;
        let (Some(vm), Some(row)) = (self.vm(), self.selected_dest_row()) else {
            return;
        };
        let Some(conn) = vm.conn() else {
            return;
        };
        let xml = i.xmleditor.is_xml_selected().then(|| {
            let xml = i.xmleditor.get_xml();
            glib::g_debug!("virt-manager", "Using XML from xmleditor:\n{xml}");
            xml
        });
        let destconn = row
            .uri
            .as_deref()
            .and_then(|uri| engine::get().connmanager().get_conn(uri));
        let tunnel = self.is_tunnel_selected();
        let dest_uri = if tunnel {
            Some(i.tunnel_uri.text().to_string())
        } else {
            self.regular_migrate_uri(&conn)
        };
        let opts = MigrateOptions {
            dest_uri,
            tunnel,
            unsafe_migrate: i.unsafe_migrate.is_active(),
            temporary: i.temporary.is_active(),
            xml,
        };

        // `set_finish_cursor`
        i.window.set_cursor_from_name(Some("progress"));
        let this = self.clone();
        glib::spawn_future_local(async move {
            this.run_migrate(vm, conn, destconn, row.label, opts).await
        });
    }

    /// The `vmmAsyncJob` half of `_finish`: `_async_migrate`, then
    /// `_finish_cb`.
    async fn run_migrate(
        &self,
        vm: VmmDomain,
        conn: VmmConnection,
        destconn: Option<VmmConnection>,
        mut destlabel: String,
        opts: MigrateOptions,
    ) {
        let i = &self.0;
        let src_handle = conn.handle();
        let mut cancel_cb: Option<CancelCb> = None;
        if vm.supports_domain_job_info().await {
            if let Some(handle) = src_handle.clone() {
                let uuid = vm.get_uuid();
                cancel_cb = Some(Arc::new(move || cancel_migration(&handle, &uuid)));
            }
        }
        if let Some(uri) = opts.dest_uri.as_deref().filter(|u| !u.is_empty()) {
            destlabel.push(' ');
            destlabel.push_str(uri);
        }
        let name = vm.get_name();
        let title = format!("Migrating VM '{name}'");
        let text = format!("Migrating VM '{name}' to {destlabel}. This may take a while.");

        // `migrate()`'s first step, before any libvirt call
        vm.set_install_abort();
        let is_test = conn.is_test();
        let srcuri = conn.get_uri();
        let desturi = destconn.as_ref().map(|c| c.get_uri()).unwrap_or_default();
        let dest_handle = destconn.as_ref().and_then(|c| c.handle());
        let err = asyncjob::run_with_cancel(
            Some(&i.window),
            &title,
            &text,
            true,
            cancel_cb,
            move |job| {
                let meter = job.meter();
                let fail = |e: String| (e.clone(), e);
                let src = src_handle.ok_or_else(|| fail("Not connected".into()))?;
                let dest = dest_handle.ok_or_else(|| fail("Not connected".into()))?;
                let dom = Domain::lookup_by_name(src.connect(), &name)
                    .map_err(|e| fail(e.message().to_string()))?;
                glib::g_debug!(
                    "virt-manager",
                    "Migrating vm={name} from {srcuri} to {desturi}"
                );
                migration::migrate(is_test, &dom, dest.connect(), &opts, Some(meter)).map_err(fail)
            },
        )
        .await;

        // `_finish_cb`
        i.window.set_cursor(None);
        match err {
            Some((error, details)) => {
                error::show_err(
                    Some(&i.window),
                    &format!("Unable to migrate guest: {error}"),
                    Some(&details),
                    "",
                )
                .await;
            }
            None => {
                if let Some(destconn) = destconn {
                    destconn.tick();
                }
                conn.tick();
                self.close();
            }
        }
    }

    // UI listeners

    /// `_xmleditor_xml_requested_cb`.
    async fn xmleditor_xml_requested(&self) {
        let Some(vm) = self.vm() else {
            return;
        };
        let _ = vm.refresh_xml().await;
        if let Some(x) = vm.xmlobj() {
            self.0.xmleditor.set_xml(&x.xml);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(uri: &str, state: &str) -> DestConn {
        DestConn {
            uri: uri.into(),
            pretty_desc: crate::connection::pretty_desc(uri),
            disconnected: state == "disconnected",
            active: state == "active",
        }
    }

    #[test]
    fn dest_rows_say_why_a_connection_is_unusable() {
        let src = "qemu:///system";
        let row = |c| build_dest_row(src, &c);
        assert_eq!(
            row(conn("xen:///", "active")),
            DestRow {
                label: "Xen (Hypervisors do not match)".into(),
                uri: Some("xen:///".into()),
                can_migrate: false,
            }
        );
        // The hypervisor check comes first, then the state.
        assert_eq!(
            row(conn("lxc:///", "disconnected")).label,
            "LXC (Hypervisors do not match)"
        );
        assert_eq!(
            row(conn("qemu+ssh://h/system", "disconnected")).label,
            "QEMU/KVM: h (Disconnected)"
        );
        assert_eq!(
            row(conn("qemu:///system", "active")).label,
            "QEMU/KVM (Same connection)"
        );
        // Still connecting: plain label, but not usable yet.
        let connecting = row(conn("qemu+ssh://h/system", "connecting"));
        assert_eq!(connecting.label, "QEMU/KVM: h");
        assert!(!connecting.can_migrate);
        // The transport doesn't count as a different hypervisor.
        let remote = row(conn("qemu+tls://h2/system", "active"));
        assert_eq!(remote.label, "QEMU/KVM: h2");
        assert!(remote.can_migrate);
    }

    #[test]
    fn dest_rows_sort_and_select_like_upstream() {
        let src = "test:///default";
        let conns = [
            conn("test:///default", "active"),
            conn("test:///tmp/zz.xml", "active"),
            conn("test:///tmp/aa.xml", "disconnected"),
            conn("test:///tmp/mm.xml", "active"),
        ];
        let (rows, active) = dest_rows(src, &conns);
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "test aa.xml (Disconnected)",
                "test default (Same connection)",
                "test mm.xml",
                "test zz.xml",
            ]
        );
        assert_eq!(active, 2);
        assert_eq!(rows[active].uri.as_deref(), Some("test:///tmp/mm.xml"));

        // Nothing usable: the placeholder leads and is selected.
        let (rows, active) = dest_rows(src, &conns[..1]);
        assert_eq!(active, 0);
        assert_eq!(
            rows[0],
            DestRow {
                label: "No usable connections available.".into(),
                uri: None,
                can_migrate: false,
            }
        );
        assert_eq!(rows[1].label, "test default (Same connection)");
        let (rows, _) = dest_rows(src, &[]);
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn regular_migrate_uri_like_upstream() {
        assert_eq!(
            build_regular_migrate_uri(false, Some("dest"), Some(49152)).as_deref(),
            Some("tcp:dest:49152")
        );
        // Port unchecked ('Let libvirt decide'), or 0
        assert_eq!(
            build_regular_migrate_uri(false, Some("dest"), None).as_deref(),
            Some("tcp:dest")
        );
        assert_eq!(
            build_regular_migrate_uri(false, Some("dest"), Some(0)).as_deref(),
            Some("tcp:dest")
        );
        // Xen takes the bare address
        assert_eq!(
            build_regular_migrate_uri(true, Some("dest"), Some(8002)).as_deref(),
            Some("dest:8002")
        );
        // No address: libvirt decides, port or not
        assert_eq!(build_regular_migrate_uri(false, None, Some(49152)), None);
        assert_eq!(
            build_regular_migrate_uri(false, Some(""), Some(49152)),
            None
        );
    }
}
