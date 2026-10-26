//! `createvm.py` (`vmmCreateVM`): the New VM wizard. Step 1 picks the
//! connection, the install method and the architecture options; the
//! later steps are filled in by the next pieces of the port.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use virtinst_core::capabilities::{Capabilities, CapsInfo};
use virtinst_core::install::{prefers_uefi, GuestCapsDefaults};

use crate::connection::VmmConnection;
use crate::devpages::{Grid, GridRow, SimpleCombo};
use crate::domain::pretty_hv;
use crate::engine;

const PAGE_NAME: usize = 0;
const PAGE_INSTALL: usize = 1;
const PAGE_STORAGE: usize = 3;
const PAGE_FINISH: usize = 4;

/// `INSTALL_PAGE_*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallPage {
    Iso,
    Url,
    Manual,
    Import,
    ContainerApp,
    ContainerOs,
    VzTemplate,
}

/// `_pretty_arch`.
fn pretty_arch(arch: &str) -> String {
    if arch == "armv7l" {
        "arm".into()
    } else {
        arch.into()
    }
}

fn radio(label: &str, group: Option<&gtk::CheckButton>) -> gtk::CheckButton {
    let r = gtk::CheckButton::with_mnemonic(label);
    r.set_group(group);
    r
}

/// The `dialog-warning` icon beside a wrapping label
/// (`startup-error-box`, `arch-warning-box`).
fn warning_box() -> (gtk::Box, gtk::Label) {
    let image = gtk::Image::from_icon_name("dialog-warning");
    image.set_valign(gtk::Align::Start);
    let label = gtk::Label::new(None);
    label.set_halign(gtk::Align::Start);
    label.set_valign(gtk::Align::Start);
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_max_width_chars(45);
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    b.set_halign(gtk::Align::Start);
    b.append(&image);
    b.append(&label);
    (b, label)
}

/// A title label over an indented radio group (`virt-install-box` and
/// friends).
fn method_box(title: &str, radios: &[&gtk::CheckButton]) -> gtk::Box {
    let label = gtk::Label::new(Some(title));
    label.set_halign(gtk::Align::Start);
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 3);
    vbox.set_margin_start(12);
    for r in radios {
        vbox.append(*r);
    }
    let b = gtk::Box::new(gtk::Orientation::Vertical, 8);
    b.append(&label);
    b.append(&vbox);
    b
}

pub struct Inner {
    window: gtk::Window,
    header_pagenum: gtk::Label,
    pages: gtk::Stack,
    cur_page: Cell<usize>,
    back: gtk::Button,
    forward: gtk::Button,
    finish: gtk::Button,

    // Step 1
    conn_combo: gtk::DropDown,
    conn_label: gtk::Label,
    conn_uris: RefCell<Vec<String>>,
    startup_error_box: gtk::Box,
    startup_error: gtk::Label,
    install_box: gtk::Box,
    vz_install_box: gtk::Box,
    vz_virt_type_hvm: gtk::CheckButton,
    vz_virt_type_exe: gtk::CheckButton,
    virt_install_box: gtk::Box,
    method_local: gtk::CheckButton,
    method_tree: gtk::CheckButton,
    method_import: gtk::CheckButton,
    method_manual: gtk::CheckButton,
    container_install_box: gtk::Box,
    method_container_app: gtk::CheckButton,
    method_container_os: gtk::CheckButton,
    arch_warning_box: gtk::Box,
    arch_warning: gtk::Label,
    arch_expander: gtk::Expander,
    xen_type: SimpleCombo,
    xen_type_row: GridRow,
    virt_type: SimpleCombo,
    virt_type_row: GridRow,
    arch: SimpleCombo,
    arch_row: GridRow,
    machine: SimpleCombo,
    machine_row: GridRow,

    conn: RefCell<Option<VmmConnection>>,
    conn_handler: RefCell<Option<glib::SignalHandlerId>>,
    caps: RefCell<Option<Rc<Capabilities>>>,
    capsinfo: RefCell<Option<CapsInfo>>,
    /// `_gdata.uefi_requested`.
    uefi_requested: Cell<bool>,
    /// `installable_arch` from the last `_set_caps_state`, which the
    /// install page's `kernel-info-box` follows.
    installable_arch: Cell<bool>,
    /// Set while the combos are refilled, so their `changed` handlers
    /// don't fire (upstream's `disconnect_by_func`).
    populating: Cell<bool>,
    /// Bumped on every connection or caps change, so a stale async
    /// answer is dropped.
    generation: Cell<u64>,
}

#[derive(Clone)]
pub struct VmmCreateVM(Rc<Inner>);

thread_local! {
    static INSTANCE: RefCell<Option<VmmCreateVM>> = const { RefCell::new(None) };
}

impl VmmCreateVM {
    /// `vmmCreateVM.show_instance(parentobj, uri)`.
    pub fn show_instance(parent: &gtk::Window, uri: Option<String>) {
        let this = INSTANCE.with(|i| i.borrow_mut().get_or_insert_with(Self::new).clone());
        this.show(parent, uri.as_deref());
    }

    fn new() -> VmmCreateVM {
        let window = gtk::Window::builder()
            .title("New VM")
            .resizable(false)
            .hide_on_close(true)
            .build();
        window.set_size_request(500, 500);
        window.update_property(&[gtk::accessible::Property::Label("vmm-create")]);

        // The blue header.
        let image = gtk::Image::from_icon_name("vm_new_wizard");
        image.set_pixel_size(48);
        let title = gtk::Label::new(None);
        title.set_markup("<span size='large'>Create a new virtual machine</span>");
        title.set_halign(gtk::Align::Start);
        title.add_css_class("vmm-header-text");
        let header_pagenum = gtk::Label::new(Some("Step foo of bar"));
        header_pagenum.set_halign(gtk::Align::Start);
        header_pagenum.add_css_class("vmm-header-subtext");
        header_pagenum.update_property(&[gtk::accessible::Property::Label("pagenum-label")]);
        let vbox3 = gtk::Box::new(gtk::Orientation::Vertical, 0);
        vbox3.set_hexpand(true);
        vbox3.append(&title);
        vbox3.append(&header_pagenum);
        let hbox77 = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        hbox77.set_margin_start(6);
        hbox77.set_margin_end(6);
        hbox77.set_margin_top(6);
        hbox77.set_margin_bottom(6);
        hbox77.append(&image);
        hbox77.append(&vbox3);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        header.add_css_class("vmm-header");
        header.append(&hbox77);

        // Step 1: connection
        let conn_combo = gtk::DropDown::from_strings(&[]);
        conn_combo.set_size_request(400, -1);
        conn_combo.update_property(&[gtk::accessible::Property::Label("create-conn")]);
        let conn_label = gtk::Label::new(Some("conn label"));
        conn_label.set_halign(gtk::Align::Start);
        conn_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        conn_label.set_max_width_chars(25);
        let label77 = gtk::Label::with_mnemonic("C_onnection:");
        label77.set_halign(gtk::Align::End);
        label77.set_mnemonic_widget(Some(&conn_combo));
        let hbox3 = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        hbox3.append(&conn_combo);
        hbox3.append(&conn_label);
        let table5 = gtk::Grid::builder()
            .row_spacing(4)
            .column_spacing(6)
            .build();
        table5.attach(&label77, 0, 0, 1, 1);
        table5.attach(&hbox3, 1, 0, 2, 1);

        let (startup_error_box, startup_error) = warning_box();

        // Install method
        let vz_virt_type_hvm = radio("_Virtual machine", None);
        let vz_virt_type_exe = radio("_Container", Some(&vz_virt_type_hvm));
        let vz_install_box = method_box(
            "Choose virtualization type",
            &[&vz_virt_type_hvm, &vz_virt_type_exe],
        );
        let method_local = radio("_Local install media (ISO image or CDROM)", None);
        let method_tree = radio(
            "Network _Install (HTTP, HTTPS, or FTP)",
            Some(&method_local),
        );
        let method_import = radio("Import _existing disk image", Some(&method_local));
        let method_manual = radio("Ma_nual install", Some(&method_local));
        let virt_install_box = method_box(
            "Choose how you would like to install the operating system",
            &[&method_local, &method_tree, &method_import, &method_manual],
        );
        let method_container_app = radio("_Application container", None);
        let method_container_os = radio("O_perating system container", Some(&method_container_app));
        let container_install_box = method_box(
            "Choose the container type",
            &[&method_container_app, &method_container_os],
        );
        let install_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        install_box.set_valign(gtk::Align::Start);
        install_box.append(&vz_install_box);
        install_box.append(&virt_install_box);
        install_box.append(&container_install_box);

        // Architecture options
        let (arch_warning_box, arch_warning) = warning_box();
        let xen_type = SimpleCombo::unsorted(vec![], |v| v.to_string());
        let virt_type = SimpleCombo::unsorted(vec![], |v| v.to_string());
        let arch = SimpleCombo::unsorted(vec![], pretty_arch);
        let machine = SimpleCombo::unsorted(vec![], |v| v.to_string());
        let mut grid = Grid::new();
        let xen_type_row = grid.row("_Xen Type:", &xen_type.widget);
        let virt_type_row = grid.row("_Virt Type:", &virt_type.widget);
        let arch_row = grid.row("_Architecture:", &arch.widget);
        let machine_row = grid.row("_Machine Type:", &machine.widget);
        let table6 = grid.into_grid();
        table6.set_column_spacing(6);
        table6.set_margin_start(12);
        table6.set_margin_top(6);
        for row in [&xen_type_row, &virt_type_row, &arch_row, &machine_row] {
            row.label.set_halign(gtk::Align::Start);
        }
        let arch_expander = gtk::Expander::new(Some("Architecture options"));
        arch_expander.set_valign(gtk::Align::Start);
        arch_expander.set_vexpand(true);
        arch_expander.set_child(Some(&table6));
        let box4 = gtk::Box::new(gtk::Orientation::Vertical, 6);
        box4.append(&arch_warning_box);
        box4.append(&arch_expander);

        let page_name = gtk::Box::new(gtk::Orientation::Vertical, 24);
        page_name.append(&table5);
        page_name.append(&startup_error_box);
        page_name.append(&install_box);
        page_name.append(&box4);

        let pages = gtk::Stack::new();
        pages.set_vexpand(true);
        pages.add_named(&page_name, Some("0"));
        for nr in PAGE_INSTALL..=PAGE_FINISH {
            let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
            pages.add_named(&page, Some(&nr.to_string()));
        }

        let cancel = gtk::Button::with_mnemonic("_Cancel");
        let back = gtk::Button::with_mnemonic("_Back");
        back.set_sensitive(false);
        let forward = gtk::Button::with_mnemonic("_Forward");
        let finish = gtk::Button::with_mnemonic("_Finish");
        finish.set_visible(false);
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        buttons.set_halign(gtk::Align::End);
        for b in [&cancel, &back, &forward, &finish] {
            buttons.append(b);
        }

        let vbox2 = gtk::Box::new(gtk::Orientation::Vertical, 15);
        vbox2.set_margin_start(12);
        vbox2.set_margin_end(12);
        vbox2.set_margin_top(12);
        vbox2.set_margin_bottom(12);
        vbox2.set_vexpand(true);
        vbox2.append(&pages);
        vbox2.append(&buttons);
        let vbox1 = gtk::Box::new(gtk::Orientation::Vertical, 0);
        vbox1.append(&header);
        vbox1.append(&vbox2);
        window.set_child(Some(&vbox1));

        let this = VmmCreateVM(Rc::new(Inner {
            window,
            header_pagenum,
            pages,
            cur_page: Cell::new(PAGE_NAME),
            back,
            forward,
            finish,
            conn_combo,
            conn_label,
            conn_uris: RefCell::default(),
            startup_error_box,
            startup_error,
            install_box,
            vz_install_box,
            vz_virt_type_hvm,
            vz_virt_type_exe,
            virt_install_box,
            method_local,
            method_tree,
            method_import,
            method_manual,
            container_install_box,
            method_container_app,
            method_container_os,
            arch_warning_box,
            arch_warning,
            arch_expander,
            xen_type,
            xen_type_row,
            virt_type,
            virt_type_row,
            arch,
            arch_row,
            machine,
            machine_row,
            conn: RefCell::default(),
            conn_handler: RefCell::default(),
            caps: RefCell::default(),
            capsinfo: RefCell::default(),
            uefi_requested: Cell::new(false),
            installable_arch: Cell::new(true),
            populating: Cell::new(false),
            generation: Cell::new(0),
        }));
        this.connect_signals(&cancel);
        this
    }

    /// Runs `f` on the wizard from a signal handler, unless the combos
    /// are being refilled.
    fn handler<W>(&self, f: fn(&VmmCreateVM)) -> impl Fn(&W) + 'static {
        let weak = Rc::downgrade(&self.0);
        move |_| {
            if let Some(inner) = weak.upgrade() {
                if !inner.populating.get() {
                    f(&VmmCreateVM(inner));
                }
            }
        }
    }

    fn connect_signals(&self, cancel: &gtk::Button) {
        let i = &self.0;
        cancel.connect_clicked(self.handler(VmmCreateVM::close_requested));
        i.back
            .connect_clicked(self.handler(VmmCreateVM::back_clicked));
        i.forward
            .connect_clicked(self.handler(VmmCreateVM::forward_clicked));
        i.conn_combo
            .connect_selected_notify(self.handler(VmmCreateVM::conn_changed));
        for r in [
            &i.method_local,
            &i.method_tree,
            &i.method_import,
            &i.method_manual,
            &i.method_container_app,
            &i.method_container_os,
        ] {
            r.connect_toggled(self.handler(VmmCreateVM::method_changed));
        }
        i.vz_virt_type_hvm
            .connect_toggled(self.handler(VmmCreateVM::vz_virt_type_changed));
        i.xen_type
            .widget
            .connect_selected_notify(self.handler(VmmCreateVM::xen_type_changed));
        i.arch
            .widget
            .connect_selected_notify(self.handler(VmmCreateVM::arch_changed));
        i.virt_type
            .widget
            .connect_selected_notify(self.handler(VmmCreateVM::virt_type_changed));
        i.machine
            .widget
            .connect_selected_notify(self.handler(VmmCreateVM::machine_changed));

        let close = self.handler::<()>(VmmCreateVM::close_requested);
        i.window.connect_close_request(move |_| {
            close(&());
            glib::Propagation::Stop
        });
        // `bind_escape_key_close`
        let esc = gtk::EventControllerKey::new();
        let close = self.handler::<()>(VmmCreateVM::close_requested);
        esc.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                close(&());
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        i.window.add_controller(esc);
    }

    // Standard window methods

    fn show(&self, parent: &gtk::Window, uri: Option<&str>) {
        eprintln!("Showing new vm wizard");
        if !self.0.window.is_visible() {
            self.reset_state(uri);
            self.0.window.set_transient_for(Some(parent));
            engine::get().increment_window_counter();
        }
        self.0.window.present();
    }

    fn close(&self) {
        if self.0.window.is_visible() {
            eprintln!("Closing new vm wizard");
            engine::get().decrement_window_counter();
        }
        self.0.window.set_visible(false);
        self.set_conn(None);
    }

    /// `_close_requested`: nothing was created yet, so nothing to clean
    /// up before closing.
    fn close_requested(&self) {
        self.close();
    }

    // Initial state handling

    fn show_startup_error(&self, error: &str, hideinstall: bool) {
        let i = &self.0;
        i.startup_error_box.set_visible(true);
        i.forward.set_sensitive(false);
        if hideinstall {
            i.install_box.set_visible(false);
            i.arch_expander.set_visible(false);
        }
        i.startup_error.set_text(&format!("Error: {error}"));
    }

    fn show_startup_warning(&self, error: &str) {
        self.0.startup_error_box.set_visible(true);
        self.0.startup_error.set_markup(&format!(
            "<span size='small'>Warning: {}</span>",
            glib::markup_escape_text(error)
        ));
    }

    fn show_arch_warning(&self, error: &str) {
        self.0.arch_warning_box.set_visible(true);
        self.0.arch_warning.set_markup(&format!(
            "<span size='small'>Warning: {}</span>",
            glib::markup_escape_text(error)
        ));
    }

    /// `_reset_state`: back to step 1 with the defaults.
    fn reset_state(&self, urihint: Option<&str>) {
        let i = &self.0;
        self.set_page(PAGE_NAME);

        i.populating.set(true);
        i.method_local.set_active(true);
        let activeconn = self.populate_conn_list(urihint);
        i.arch_expander.set_expanded(false);
        i.vz_virt_type_hvm.set_active(true);
        i.populating.set(false);

        self.set_conn(activeconn);
    }

    /// `_set_caps_state`: the install methods this guest type allows.
    fn set_caps_state(&self) {
        let i = &self.0;
        i.arch_warning_box.set_visible(false);
        let (Some(conn), Some(capsinfo)) = (i.conn.borrow().clone(), i.capsinfo.borrow().clone())
        else {
            return;
        };
        i.uefi_requested.set(false);

        let arch = capsinfo.arch.clone().unwrap_or_default();
        let os_type = capsinfo.os_type.clone().unwrap_or_default();
        let is_x86 = matches!(arch.as_str(), "i686" | "x86_64");
        let installable_arch = is_x86 || matches!(arch.as_str(), "ppc64" | "ppc64le" | "s390x");

        let default_efi = engine::get().config().get_default_firmware_setting() == "uefi"
            && is_x86
            && os_type == "hvm";
        if default_efi {
            eprintln!("UEFI default requested via app preferences");
        }
        let machine = self.get_config_machine().or_else(|| {
            capsinfo
                .recommended_machine(conn.is_qemu() || conn.is_test())
                .map(str::to_string)
                .or_else(|| capsinfo.machines.first().cloned())
        });
        let domain_type = capsinfo.hypervisor_type.clone().unwrap_or_default();
        let caps = GuestCapsDefaults {
            domain_type: &domain_type,
            os_type: &os_type,
            arch: &arch,
            machine: machine.as_deref(),
            emulator: capsinfo.emulator.as_deref(),
            is_qemu: conn.is_qemu(),
        };
        let uri = virtinst_core::connection::Uri::parse(&conn.get_uri());
        let wants_uefi = prefers_uefi(caps, conn.is_test(), uri.is_bhyve(), false) || default_efi;

        if !wants_uefi {
            self.apply_caps_state(&conn, &capsinfo, installable_arch);
            return;
        }
        // `guest.enable_uefi()`, called for validation: libvirt has to
        // pick the firmware itself (the loader path lookup isn't
        // ported).
        let generation = i.generation.get();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let domcaps = conn
                .fetch_domcaps(
                    capsinfo.emulator.clone(),
                    capsinfo.arch.clone(),
                    machine,
                    capsinfo.hypervisor_type.clone(),
                )
                .await;
            if this.0.generation.get() != generation {
                return;
            }
            let installable_arch = if domcaps.is_some_and(|d| d.supports_firmware_efi()) {
                this.0.uefi_requested.set(true);
                eprintln!("UEFI found, setting it as default.");
                true
            } else {
                let e = "Libvirt version does not support UEFI.";
                eprintln!("Error checking for UEFI default: {e}");
                this.show_arch_warning(&format!(
                    "Failed to setup UEFI: {e}\nInstall options are limited."
                ));
                false
            };
            this.apply_caps_state(&conn, &capsinfo, installable_arch);
        });
    }

    /// The rest of `_set_caps_state`, once UEFI is settled.
    fn apply_caps_state(&self, conn: &VmmConnection, capsinfo: &CapsInfo, installable_arch: bool) {
        let i = &self.0;
        i.installable_arch.set(installable_arch);
        let os_type = capsinfo.os_type.as_deref().unwrap_or_default();
        let is_local = !conn.is_remote();
        // Every libvirt we support has storage APIs and streams.
        let can_storage = true;
        let is_pv = os_type == "xen";
        let is_container_only = conn.is_container_only();
        let is_vz = conn.is_vz();
        let is_vz_container = is_vz && os_type == "exe";
        let can_remote_url = !conn.is_test();

        i.method_tree
            .set_sensitive((is_local || can_remote_url) && installable_arch);
        i.method_local
            .set_sensitive(!is_pv && can_storage && installable_arch);
        i.method_manual.set_sensitive(!is_container_only);
        i.method_import.set_sensitive(can_storage);
        let virt_methods = [
            &i.method_local,
            &i.method_tree,
            &i.method_manual,
            &i.method_import,
        ];

        let mut local_tt = None;
        let mut tree_tt = None;
        if !is_local && !can_remote_url {
            tree_tt = Some("Libvirt version does not support remote URL installs.".to_string());
        }
        if is_pv {
            local_tt = Some("CDROM/ISO installs not available for paravirt guests.".to_string());
        }
        if !installable_arch {
            let msg = format!(
                "Architecture '{}' is not installable",
                capsinfo.arch.as_deref().unwrap_or_default()
            );
            tree_tt = Some(msg.clone());
            local_tt = Some(msg);
        }

        if !virt_methods
            .iter()
            .any(|w| w.is_active() && w.is_sensitive())
        {
            if let Some(w) = virt_methods.iter().find(|w| w.is_sensitive()) {
                w.set_active(true);
            }
        }

        if !(is_container_only || virt_methods.iter().any(|w| w.is_sensitive())) {
            self.show_startup_error("No install methods available for this connection.", false);
            return;
        }

        i.method_tree.set_tooltip_text(tree_tt.as_deref());
        i.method_local.set_tooltip_text(local_tt.as_deref());
        i.method_import.set_tooltip_text(None);

        // Container install options
        i.method_container_app.set_active(true);
        i.container_install_box.set_visible(is_container_only);
        i.vz_install_box.set_visible(is_vz);
        i.virt_install_box
            .set_visible(!is_container_only && !is_vz_container);
        self.set_page_num_text(i.cur_page.get());
    }

    /// `_populate_conn_state`: everything that hangs off the connection.
    fn populate_conn_state(&self) {
        let i = &self.0;
        let Some(conn) = i.conn.borrow().clone() else {
            return;
        };
        i.install_box.set_visible(true);
        i.arch_expander.set_visible(true);
        // Upstream fetches the caps inline; here Forward waits for them.
        i.forward.set_sensitive(false);
        i.capsinfo.replace(None);
        i.caps.replace(None);

        let generation = i.generation.get();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let caps = conn.fetch_caps().await;
            if this.0.generation.get() != generation {
                return;
            }
            match caps {
                Ok(caps) => this.populate_caps(&conn, Rc::new(caps)),
                Err(e) => {
                    eprintln!("Error setting create wizard conn state: {e}");
                    this.show_startup_error(&e, true);
                }
            }
        });
    }

    fn populate_caps(&self, conn: &VmmConnection, caps: Rc<Capabilities>) {
        let i = &self.0;
        i.caps.replace(Some(caps.clone()));
        i.forward.set_sensitive(true);

        if !caps.has_install_options() {
            let mut error = "No hypervisor options were found for this connection.".to_string();
            if conn.is_qemu() {
                error += "\n\nThis usually means that QEMU or KVM is not installed on your \
                          machine, or the KVM kernel modules are not loaded.";
            }
            self.show_startup_error(&error, true);
            return;
        }

        if !self.change_caps(None, None, None) {
            return;
        }

        // A bit out of order, but populate the xen/virt/arch/machine
        // lists so we can work with a default.
        self.populate_xen_type();
        self.populate_arch();
        self.populate_virt_type();

        let show_arch = i.xen_type_row.is_visible()
            || i.virt_type_row.is_visible()
            || i.arch_row.is_visible()
            || i.machine_row.is_visible();
        i.arch_expander.set_visible(show_arch);

        if conn.is_qemu() {
            let kvm = i
                .capsinfo
                .borrow()
                .as_ref()
                .and_then(|info| caps.guest_for_info(info))
                .is_some_and(|g| g.is_kvm_available());
            if !kvm {
                self.show_startup_warning(
                    "KVM is not available. This may mean the KVM package is not installed, \
                     or the KVM kernel modules are not loaded. Your virtual machines may \
                     perform poorly.",
                );
            }
        } else if conn.is_vz() {
            let has = |t: &str| caps.guests.iter().any(|g| g.os_type.as_deref() == Some(t));
            let (has_hvm, has_exe) = (has("hvm"), has("exe"));
            i.populating.set(true);
            i.vz_virt_type_hvm.set_sensitive(has_hvm);
            i.vz_virt_type_exe.set_sensitive(has_exe);
            i.vz_virt_type_hvm.set_active(has_hvm);
            i.vz_virt_type_exe.set_active(!has_hvm && has_exe);
            i.populating.set(false);
        }
    }

    fn conn_state_changed(&self) {
        let disconnected = self
            .0
            .conn
            .borrow()
            .as_ref()
            .is_some_and(|c| c.is_disconnected());
        if disconnected {
            self.close();
        }
    }

    fn set_conn(&self, newconn: Option<VmmConnection>) {
        let i = &self.0;
        i.startup_error_box.set_visible(false);
        i.arch_warning_box.set_visible(false);
        i.generation.set(i.generation.get() + 1);

        let oldconn = i.conn.replace(newconn.clone());
        if let (Some(old), Some(id)) = (oldconn, i.conn_handler.take()) {
            old.disconnect(id);
        }
        i.capsinfo.replace(None);
        i.caps.replace(None);

        let Some(conn) = newconn else {
            self.show_startup_error("No active connection to install on.", true);
            return;
        };
        let weak = Rc::downgrade(&self.0);
        let id = conn.connect_local("state-changed", false, move |_| {
            if let Some(inner) = weak.upgrade() {
                VmmCreateVM(inner).conn_state_changed();
            }
            None
        });
        i.conn_handler.replace(Some(id));
        self.populate_conn_state();
    }

    /// `_change_caps`: look up the guest for these values, and refresh
    /// what depends on it. `false` when the lookup failed.
    fn change_caps(&self, gtype: Option<&str>, arch: Option<&str>, domtype: Option<&str>) -> bool {
        let i = &self.0;
        let Some(caps) = i.caps.borrow().clone() else {
            return false;
        };
        // If none specified, prefer HVM so install options aren't
        // limited with a default PV choice.
        let gtype = gtype.or_else(|| {
            caps.guests
                .iter()
                .any(|g| g.os_type.as_deref() == Some("hvm"))
                .then_some("hvm")
        });
        let capsinfo = match caps.guest_lookup(gtype, arch, domtype, None) {
            Ok(c) => c,
            Err(e) => {
                self.show_startup_error(&e, true);
                return false;
            }
        };
        if let Some(old) = i.capsinfo.borrow().as_ref() {
            if old.os_type == capsinfo.os_type
                && old.arch == capsinfo.arch
                && old.hypervisor_type == capsinfo.hypervisor_type
            {
                return true;
            }
        }
        eprintln!(
            "Guest type set to os_type={}, arch={}, dom_type={}",
            capsinfo.os_type.as_deref().unwrap_or_default(),
            capsinfo.arch.as_deref().unwrap_or_default(),
            capsinfo.hypervisor_type.as_deref().unwrap_or_default(),
        );
        i.capsinfo.replace(Some(capsinfo));
        i.generation.set(i.generation.get() + 1);
        self.populate_machine();
        self.set_caps_state();
        true
    }

    // Helpers for populating hv/arch/machine/conn UI

    /// Refills `combo` with `rows` and selects `default`, without
    /// firing its handler.
    fn fill_combo(&self, combo: &SimpleCombo, rows: Vec<(Option<String>, String)>, default: usize) {
        self.0.populating.set(true);
        combo.set_values(rows);
        combo.widget.set_selected(default as u32);
        self.0.populating.set(false);
    }

    fn populate_xen_type(&self) {
        let i = &self.0;
        let (Some(conn), Some(caps), Some(capsinfo)) = (
            i.conn.borrow().clone(),
            i.caps.borrow().clone(),
            i.capsinfo.borrow().clone(),
        ) else {
            return;
        };
        let mut rows: Vec<(Option<String>, String)> = vec![];
        let mut default = 0;
        if conn.is_xen() || conn.is_test() {
            for guest in &caps.guests {
                let Some(dom) = guest.domains.first() else {
                    continue;
                };
                let gtype = guest.os_type.clone().unwrap_or_default();
                let domtype = dom.hypervisor_type.clone().unwrap_or_default();
                let label = pretty_hv(&gtype, &domtype);
                // Don't add multiple rows for each arch
                if rows.iter().any(|(_, l)| *l == label) {
                    continue;
                }
                // Determine if this is the default given by guest_lookup
                if capsinfo.os_type.as_deref() == Some(&gtype)
                    && capsinfo.hypervisor_type.as_deref() == Some(&domtype)
                {
                    default = rows.len();
                }
                rows.push((Some(gtype), label));
            }
        }
        let show = !rows.is_empty();
        self.fill_combo(&i.xen_type, rows, default);
        i.xen_type_row.set_visible(show);
    }

    fn populate_arch(&self) {
        let i = &self.0;
        let (Some(caps), Some(capsinfo)) = (i.caps.borrow().clone(), i.capsinfo.borrow().clone())
        else {
            return;
        };
        let mut archs: Vec<String> = caps
            .guests
            .iter()
            .filter(|g| g.os_type == capsinfo.os_type)
            .filter_map(|g| g.arch.clone())
            .collect();
        let host_arch = caps.host.cpu.arch.clone().unwrap_or_default();

        // Combine x86/i686 to avoid confusion
        let has = |a: &[String], x: &str| a.iter().any(|v| v == x);
        if host_arch == "x86_64" && has(&archs, "x86_64") && has(&archs, "i686") {
            archs.retain(|a| a != "i686");
        }
        archs.sort();

        let mut prios: Vec<&str> = vec![
            "x86_64", "i686", "aarch64", "armv7l", "ppc64", "ppc64le", "riscv64", "s390x",
        ];
        if !prios.contains(&host_arch.as_str()) {
            prios.clear();
        }
        prios.retain(|p| has(&archs, p));
        archs.retain(|a| !prios.contains(&a.as_str()));
        // Upstream puts a separator row between the two groups; a
        // GTK4 drop-down has none.
        let archs: Vec<String> = prios.iter().map(|p| p.to_string()).chain(archs).collect();

        let default = capsinfo
            .arch
            .as_ref()
            .and_then(|a| archs.iter().position(|x| x == a))
            .unwrap_or(0);
        let show = archs.len() >= 2;
        let rows = archs
            .into_iter()
            .map(|a| {
                let label = pretty_arch(&a);
                (Some(a), label)
            })
            .collect();
        self.fill_combo(&i.arch, rows, default);
        i.arch_row.set_visible(show);
    }

    fn populate_virt_type(&self) {
        let i = &self.0;
        let (Some(conn), Some(caps), Some(capsinfo)) = (
            i.conn.borrow().clone(),
            i.caps.borrow().clone(),
            i.capsinfo.borrow().clone(),
        ) else {
            return;
        };
        // Allow choosing between qemu and kvm for archs that
        // traditionally have a decent amount of TCG usage, like armv7l.
        // Also include aarch64 which can be used for arm32 VMs as well
        let mut domains: Vec<String> = caps
            .guest_for_info(&capsinfo)
            .map(|g| {
                g.domains
                    .iter()
                    .filter_map(|d| d.hypervisor_type.clone())
                    .collect()
            })
            .unwrap_or_default();
        let arch = capsinfo.arch.as_deref().unwrap_or_default();
        if !conn.is_qemu() || matches!(arch, "i686" | "x86_64" | "ppc64" | "ppc64le") {
            domains.clear();
        }

        let mut default = capsinfo
            .hypervisor_type
            .as_ref()
            .and_then(|h| domains.iter().position(|d| d == h))
            .unwrap_or(0);
        if let Some(pos) = domains.iter().position(|d| d == "kvm") {
            let kvm = domains.remove(pos);
            domains.insert(0, kvm);
            // Upstream picks the index before moving kvm up, so its
            // default can land on the wrong row; follow the value.
            default = capsinfo
                .hypervisor_type
                .as_ref()
                .and_then(|h| domains.iter().position(|d| d == h))
                .unwrap_or(0);
        }

        let os_type = capsinfo.os_type.clone().unwrap_or_default();
        let rows: Vec<(Option<String>, String)> = domains
            .into_iter()
            .map(|d| {
                let label = pretty_hv(&os_type, &d);
                (Some(d), label)
            })
            .collect();
        let show = rows.len() > 1;
        self.fill_combo(&i.virt_type, rows, default);
        i.virt_type_row.set_visible(show);
    }

    fn populate_machine(&self) {
        let i = &self.0;
        let (Some(conn), Some(capsinfo)) = (i.conn.borrow().clone(), i.capsinfo.borrow().clone())
        else {
            return;
        };
        let mut machines = capsinfo.machines.clone();
        if matches!(capsinfo.arch.as_deref(), Some("i686" | "x86_64")) {
            machines.clear();
        }
        machines.sort();

        let defmachine = capsinfo.recommended_machine(conn.is_qemu() || conn.is_test());
        // Upstream lists the recommended machine first, then a
        // separator, then the rest.
        if let Some(def) = defmachine {
            if let Some(pos) = machines.iter().position(|m| m == def) {
                let m = machines.remove(pos);
                machines.insert(0, m);
            }
        }
        let default = defmachine
            .and_then(|d| machines.iter().position(|m| m == d))
            .unwrap_or(0);

        let show = machines.len() > 1;
        let rows = machines.into_iter().map(|m| (Some(m.clone()), m)).collect();
        self.fill_combo(&i.machine, rows, default);
        i.machine_row.set_visible(show);
    }

    fn populate_conn_list(&self, urihint: Option<&str>) -> Option<VmmConnection> {
        let i = &self.0;
        let connmanager = engine::get().connmanager();
        let mut conns: Vec<VmmConnection> = vec![];
        let mut default: Option<usize> = None;
        for connobj in connmanager.conns() {
            if !connobj.is_active() {
                continue;
            }
            if Some(connobj.get_uri().as_str()) == urihint {
                default = Some(conns.len());
            } else if default.is_none() && !connobj.is_remote() {
                // Favor local connections over remote connections
                default = Some(conns.len());
            }
            conns.push(connobj);
        }
        let default = default.unwrap_or(0);

        let descs: Vec<String> = conns.iter().map(|c| c.get_pretty_desc()).collect();
        let labels: Vec<&str> = descs.iter().map(String::as_str).collect();
        i.conn_combo.set_model(Some(&gtk::StringList::new(&labels)));
        i.conn_uris
            .replace(conns.iter().map(|c| c.get_uri()).collect());

        let activeconn = conns.get(default).cloned();
        i.conn_combo.set_selected(default as u32);
        i.conn_label
            .set_text(descs.get(default).map(String::as_str).unwrap_or_default());
        i.conn_combo.set_visible(conns.len() > 1);
        i.conn_label.set_visible(conns.len() <= 1);
        activeconn
    }

    // Config getters

    fn get_config_machine(&self) -> Option<String> {
        let i = &self.0;
        if !i.machine_row.is_visible() {
            return None;
        }
        i.machine.get()
    }

    /// `_get_config_install_page`.
    pub fn get_config_install_page(&self) -> InstallPage {
        let i = &self.0;
        if i.vz_install_box.get_visible() && i.vz_virt_type_exe.is_active() {
            return InstallPage::VzTemplate;
        }
        if i.virt_install_box.get_visible() {
            if i.method_tree.is_active() {
                InstallPage::Url
            } else if i.method_import.is_active() {
                InstallPage::Import
            } else if i.method_manual.is_active() {
                InstallPage::Manual
            } else {
                InstallPage::Iso
            }
        } else if i.method_container_os.is_active() {
            InstallPage::ContainerOs
        } else {
            InstallPage::ContainerApp
        }
    }

    fn should_skip_disk_page(&self) -> bool {
        matches!(
            self.get_config_install_page(),
            InstallPage::Import
                | InstallPage::ContainerApp
                | InstallPage::ContainerOs
                | InstallPage::VzTemplate
        )
    }

    // Intro page listeners

    fn conn_changed(&self) {
        let i = &self.0;
        let uri = i
            .conn_uris
            .borrow()
            .get(i.conn_combo.selected() as usize)
            .cloned();
        let newconn = uri.and_then(|u| engine::get().connmanager().get_conn(&u));
        if let Some(c) = &newconn {
            i.conn_label.set_text(&c.get_pretty_desc());
        }
        // If we aren't visible, let reset_state handle this for us
        if !i.window.is_visible() {
            return;
        }
        if *i.conn.borrow() != newconn {
            self.set_conn(newconn);
        }
    }

    fn method_changed(&self) {
        // Reset the page number, since the total page numbers depend
        // on the chosen install method
        self.set_page_num_text(0);
    }

    fn machine_changed(&self) {
        self.0.generation.set(self.0.generation.get() + 1);
        self.set_caps_state();
    }

    fn xen_type_changed(&self) {
        let Some(os_type) = self.0.xen_type.get() else {
            return;
        };
        if self.change_caps(Some(&os_type), None, None) {
            self.populate_arch();
        }
    }

    fn arch_changed(&self) {
        let Some(arch) = self.0.arch.get() else {
            return;
        };
        let os_type = self
            .0
            .capsinfo
            .borrow()
            .as_ref()
            .and_then(|c| c.os_type.clone());
        if self.change_caps(os_type.as_deref(), Some(&arch), None) {
            self.populate_virt_type();
        }
    }

    fn virt_type_changed(&self) {
        let Some(domtype) = self.0.virt_type.get() else {
            return;
        };
        let (os_type, arch) = self
            .0
            .capsinfo
            .borrow()
            .as_ref()
            .map(|c| (c.os_type.clone(), c.arch.clone()))
            .unwrap_or_default();
        self.change_caps(os_type.as_deref(), arch.as_deref(), Some(&domtype));
    }

    fn vz_virt_type_changed(&self) {
        if self.0.vz_virt_type_hvm.is_active() {
            self.change_caps(Some("hvm"), None, None);
        } else {
            self.change_caps(Some("exe"), None, None);
        }
    }

    // Navigation methods

    /// `_set_page_num_text`: "Step 1 of 5" in the header.
    fn set_page_num_text(&self, cur: usize) {
        let mut cur = cur + 1;
        let mut last = PAGE_FINISH + 1;
        if self.should_skip_disk_page() {
            last -= 1;
            cur = cur.min(last);
        }
        self.0
            .header_pagenum
            .set_markup(&format!("Step {cur} of {last}"));
    }

    fn back_clicked(&self) {
        let curpage = self.0.cur_page.get();
        let mut next_page = curpage - 1;
        if curpage == PAGE_FINISH && self.should_skip_disk_page() {
            // Skip over storage page
            next_page -= 1;
        }
        self.set_page(next_page);
    }

    fn get_next_pagenum(&self, curpage: usize) -> usize {
        let next_page = curpage + 1;
        if next_page == PAGE_STORAGE && self.should_skip_disk_page() {
            // Skip storage page for import installs
            return next_page + 1;
        }
        next_page
    }

    fn forward_clicked(&self) {
        let curpage = self.0.cur_page.get();
        if !self.validate(curpage) {
            return;
        }
        self.0.forward.grab_focus();
        self.set_page(self.get_next_pagenum(curpage));
    }

    /// `set_current_page` plus `_page_changed`.
    fn set_page(&self, pagenum: usize) {
        let i = &self.0;
        i.cur_page.set(pagenum);
        i.pages.set_visible_child_name(&pagenum.to_string());
        if pagenum == PAGE_FINISH {
            i.finish.grab_focus();
        }
        i.back.set_sensitive(pagenum != PAGE_NAME);
        i.forward.set_visible(pagenum != PAGE_FINISH);
        i.finish.set_visible(pagenum == PAGE_FINISH);
        self.set_page_num_text(pagenum);
    }

    // Page validation routines

    fn validate(&self, pagenum: usize) -> bool {
        match pagenum {
            PAGE_NAME => self.validate_intro_page(),
            _ => true,
        }
    }

    /// `_validate_intro_page`: the guest has to resolve.
    fn validate_intro_page(&self) -> bool {
        self.0.capsinfo.borrow().is_some()
    }
}
