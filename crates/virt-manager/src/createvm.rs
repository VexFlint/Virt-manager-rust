//! `createvm.py` (`vmmCreateVM`): the New VM wizard. Step 1 picks the
//! connection, the install method and the architecture options; the
//! later steps are filled in by the next pieces of the port.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use gtk::prelude::*;
use gtk::{gio, glib};
use virtinst_core::capabilities::{Capabilities, CapsInfo};
use virtinst_core::install::{prefers_uefi, GuestCapsDefaults};

use crate::addstorage::{get_default_path, plan_disk, AddStorage, DiskPlan, DiskRequest};
use crate::asyncjob;
use crate::connection::VmmConnection;
use crate::devpages::{EntryCombo, Grid, GridRow, SimpleCombo};
use crate::domain::pretty_hv;
use crate::engine;
use crate::error;
use crate::mediacombo::MediaCombo;
use crate::netlist::{NetList, NetSelection};
use crate::oslist::{self, OsList};
use crate::storagebrowse::{self, BrowseReason, StorageBrowser};

const PAGE_NAME: usize = 0;
const PAGE_INSTALL: usize = 1;
const PAGE_MEM: usize = 2;
const PAGE_STORAGE: usize = 3;
const PAGE_FINISH: usize = 4;

/// Seconds before OS detection gives up.
const DETECT_TIMEOUT: u32 = 20;
/// MiB, when the OS has no recommendation.
const DEFAULT_MEM: f64 = 1024.0;

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

impl InstallPage {
    fn name(self) -> &'static str {
        match self {
            InstallPage::Iso => "iso",
            InstallPage::Url => "url",
            InstallPage::Manual => "manual",
            InstallPage::Import => "import",
            InstallPage::ContainerApp => "app",
            InstallPage::ContainerOs => "oscontainer",
            InstallPage::VzTemplate => "vz",
        }
    }
}

/// `_pretty_memory`, from KiB.
fn pretty_memory(mem: u64) -> String {
    format!("{} MiB", mem / 1024)
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

fn start_label(text: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Label {
    let l = gtk::Label::with_mnemonic(text);
    l.set_halign(gtk::Align::Start);
    l.set_mnemonic_widget(Some(widget));
    l
}

/// An info or warning icon beside small text (`kernel-info-box`,
/// `install-oscontainer-notsupport`).
fn small_note(icon: &str, markup: &str) -> gtk::Box {
    let image = gtk::Image::from_icon_name(icon);
    image.set_valign(gtk::Align::Start);
    let label = gtk::Label::new(None);
    label.set_markup(markup);
    label.set_halign(gtk::Align::Start);
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_max_width_chars(60);
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 3);
    b.append(&image);
    b.append(&label);
    b
}

/// An entry with a Browse button beside it.
fn entry_browse(label: &str) -> (gtk::Box, gtk::Entry, gtk::Button) {
    let entry = gtk::Entry::new();
    entry.set_hexpand(true);
    let browse = gtk::Button::with_mnemonic(label);
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    b.append(&entry);
    b.append(&browse);
    (b, entry, browse)
}

/// `os.geteuid() == 0`.
fn is_root() -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").is_ok_and(|m| m.uid() == 0)
}

/// `is_virt_bootstrap_installed`.
fn is_virt_bootstrap_installed() -> bool {
    glib::find_program_in_path("virt-bootstrap").is_some()
}

/// Step 2: the install media, and the OS.
struct InstallWidgets {
    top: gtk::Box,
    method_pages: gtk::Stack,
    mediacombo: Rc<MediaCombo>,
    iso_browse: gtk::Button,
    url_combo: Rc<EntryCombo>,
    url_options: gtk::Expander,
    urlopts_entry: gtk::Entry,
    import_entry: gtk::Entry,
    import_browse: gtk::Button,
    kernel_info_box: gtk::Box,
    app_entry: gtk::Entry,
    app_browse: gtk::Button,
    oscontainer_fs: gtk::Entry,
    oscontainer_browse: gtk::Button,
    oscontainer_notsupport: gtk::Box,
    oscontainer_notsupport_conn: gtk::Box,
    oscontainer_bootstrap: gtk::CheckButton,
    oscontainer_source: gtk::Box,
    oscontainer_source_url: Rc<EntryCombo>,
    oscontainer_source_insecure: gtk::CheckButton,
    oscontainer_auth_options: gtk::Expander,
    oscontainer_source_user: gtk::Entry,
    oscontainer_source_passwd: gtk::PasswordEntry,
    oscontainer_rootpw_box: gtk::Box,
    oscontainer_rootpw: gtk::PasswordEntry,
    container_template: gtk::Entry,
    os_distro_box: gtk::Box,
    os_list: Rc<OsList>,
    detect_os_box: gtk::Box,
    detect_os: gtk::CheckButton,
    detect_os_spinner: gtk::Spinner,
}

impl InstallWidgets {
    fn new() -> InstallWidgets {
        // ISO
        let mediacombo = MediaCombo::new(None);
        let iso_browse = gtk::Button::with_mnemonic("Bro_wse...");
        iso_browse.update_property(&[gtk::accessible::Property::Label("install-iso-browse")]);
        let iso_label = gtk::Label::with_mnemonic("Choose _ISO or CDROM install media:");
        iso_label.set_halign(gtk::Align::Start);
        mediacombo.set_mnemonic_label(&iso_label);
        let hbox4 = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        hbox4.append(&mediacombo.top_box);
        hbox4.append(&iso_browse);
        let page_iso = gtk::Box::new(gtk::Orientation::Vertical, 6);
        page_iso.append(&iso_label);
        page_iso.append(&hbox4);

        // URL
        let url_combo = EntryCombo::new(vec![]);
        url_combo.top.set_halign(gtk::Align::Fill);
        url_combo.top.set_hexpand(true);
        url_combo.entry.set_hexpand(true);
        url_combo
            .entry
            .update_property(&[gtk::accessible::Property::Label("install-url-entry")]);
        let urlopts_entry = gtk::Entry::new();
        urlopts_entry.set_hexpand(true);
        urlopts_entry.update_property(&[gtk::accessible::Property::Label("install-urlopts-entry")]);
        let table7 = gtk::Grid::builder()
            .row_spacing(4)
            .column_spacing(6)
            .margin_start(12)
            .build();
        let label26 = gtk::Label::with_mnemonic("Kerne_l options:");
        label26.set_halign(gtk::Align::End);
        label26.set_mnemonic_widget(Some(&urlopts_entry));
        table7.attach(&label26, 0, 0, 1, 1);
        table7.attach(&urlopts_entry, 1, 0, 1, 1);
        let url_options = gtk::Expander::with_mnemonic("URL _Options");
        url_options.set_child(Some(&table7));
        let table2 = gtk::Grid::builder()
            .row_spacing(4)
            .column_spacing(6)
            .build();
        table2.attach(&url_combo.top, 0, 0, 1, 1);
        table2.attach(&url_options, 0, 1, 1, 1);
        let page_url = gtk::Box::new(gtk::Orientation::Vertical, 6);
        page_url.append(&start_label(
            "Provide the operating system install U_RL:",
            &url_combo.entry,
        ));
        page_url.append(&table2);

        // Import
        let (hbox1, import_entry, import_browse) = entry_browse("B_rowse...");
        import_entry.update_property(&[gtk::accessible::Property::Label("install-import-entry")]);
        let kernel_info_box = small_note(
            "dialog-information",
            "<small>Kernel/initrd settings can be configured with 'Customize before \
             install' on the final page.</small>",
        );
        let page_import = gtk::Box::new(gtk::Orientation::Vertical, 6);
        page_import.append(&start_label(
            "Provide the existing stora_ge path:",
            &import_entry,
        ));
        page_import.append(&hbox1);
        page_import.append(&kernel_info_box);

        // Application container
        let (hbox15, app_entry, app_browse) = entry_browse("B_rowse...");
        let page_app = gtk::Box::new(gtk::Orientation::Vertical, 6);
        page_app.append(&start_label("Provide the _application path:", &app_entry));
        page_app.append(&hbox15);

        // OS container
        let (hbox17, oscontainer_fs, oscontainer_browse) = entry_browse("B_rowse...");
        let oscontainer_notsupport = small_note(
            "dialog-warning",
            "<small>The OS directory tree must already exist. To enable OS directory \
             tree creation,\nplease install <a href=\"https://github.com/virt-manager/\
             virt-bootstrap\">virt-bootstrap</a></small>",
        );
        let oscontainer_notsupport_conn = small_note(
            "dialog-warning",
            "<small>The OS directory tree must already exist. Creating an OS directory \
             tree for remote\nconnections is not yet supported.</small>",
        );
        let oscontainer_bootstrap =
            gtk::CheckButton::with_label("Create OS directory tree from container image");
        let oscontainer_source_url = EntryCombo::new(vec![]);
        oscontainer_source_url.top.set_halign(gtk::Align::Fill);
        oscontainer_source_url.top.set_hexpand(true);
        oscontainer_source_url.entry.set_hexpand(true);
        let src_grid = gtk::Grid::builder()
            .row_spacing(4)
            .column_spacing(6)
            .build();
        src_grid.attach(&gtk::Label::new(Some("Source URI:")), 0, 0, 1, 1);
        src_grid.attach(&oscontainer_source_url.top, 1, 0, 1, 1);
        let oscontainer_source_insecure =
            gtk::CheckButton::with_label("Do not verify TLS certificates of registry");
        let oscontainer_source_user = gtk::Entry::new();
        let oscontainer_source_passwd = gtk::PasswordEntry::new();
        let auth_grid = gtk::Grid::builder()
            .row_spacing(4)
            .column_spacing(6)
            .margin_start(12)
            .build();
        let user_label = gtk::Label::new(Some("Username:"));
        user_label.set_halign(gtk::Align::End);
        let passwd_label = gtk::Label::new(Some("Password:"));
        passwd_label.set_halign(gtk::Align::End);
        auth_grid.attach(&user_label, 0, 0, 1, 1);
        auth_grid.attach(&passwd_label, 0, 1, 1, 1);
        auth_grid.attach(&oscontainer_source_user, 1, 0, 1, 1);
        auth_grid.attach(&oscontainer_source_passwd, 1, 1, 1, 1);
        let oscontainer_auth_options =
            gtk::Expander::new(Some("Credentials for accessing the source registry"));
        oscontainer_auth_options.set_child(Some(&auth_grid));
        let oscontainer_source = gtk::Box::new(gtk::Orientation::Vertical, 10);
        oscontainer_source.set_sensitive(false);
        oscontainer_source.set_margin_start(10);
        oscontainer_source.set_margin_end(10);
        oscontainer_source.set_margin_top(10);
        oscontainer_source.set_margin_bottom(10);
        oscontainer_source.append(&src_grid);
        oscontainer_source.append(&oscontainer_source_insecure);
        oscontainer_source.append(&oscontainer_auth_options);
        let oscontainer_rootpw = gtk::PasswordEntry::new();
        oscontainer_rootpw.set_hexpand(true);
        let oscontainer_rootpw_box = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        oscontainer_rootpw_box.set_sensitive(false);
        oscontainer_rootpw_box.append(&gtk::Label::new(Some("Root password:")));
        oscontainer_rootpw_box.append(&oscontainer_rootpw);
        let vbox21 = gtk::Box::new(gtk::Orientation::Vertical, 12);
        vbox21.append(&hbox17);
        vbox21.append(&oscontainer_notsupport);
        vbox21.append(&oscontainer_notsupport_conn);
        vbox21.append(&oscontainer_bootstrap);
        vbox21.append(&oscontainer_source);
        vbox21.append(&oscontainer_rootpw_box);
        let page_oscontainer = gtk::Box::new(gtk::Orientation::Vertical, 6);
        page_oscontainer.append(&start_label(
            "Provide the existing OS root _directory:",
            &oscontainer_fs,
        ));
        page_oscontainer.append(&vbox21);

        // VZ template
        let container_template = gtk::Entry::new();
        container_template.set_hexpand(true);
        let page_vz = gtk::Box::new(gtk::Orientation::Vertical, 6);
        page_vz.append(&start_label(
            "Select _container template:",
            &container_template,
        ));
        page_vz.append(&container_template);

        let method_pages = gtk::Stack::new();
        method_pages.set_vhomogeneous(false);
        for (page, widget) in [
            (InstallPage::Iso, &page_iso),
            (InstallPage::Url, &page_url),
            (
                InstallPage::Manual,
                &gtk::Box::new(gtk::Orientation::Vertical, 0),
            ),
            (InstallPage::Import, &page_import),
            (InstallPage::ContainerApp, &page_app),
            (InstallPage::ContainerOs, &page_oscontainer),
            (InstallPage::VzTemplate, &page_vz),
        ] {
            method_pages.add_named(widget, Some(page.name()));
        }

        // The OS
        let os_list = OsList::new();
        let detect_os = gtk::CheckButton::with_mnemonic(
            "A_utomatically detect from the installation media / source",
        );
        detect_os.set_active(true);
        let detect_os_spinner = gtk::Spinner::new();
        let detect_os_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        detect_os_box.append(&detect_os);
        detect_os_box.append(&detect_os_spinner);
        let os_distro_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        os_distro_box.append(&start_label(
            "C_hoose the operating system you are installing:",
            &os_list.search_entry,
        ));
        os_distro_box.append(&os_list.search_entry);
        os_distro_box.append(&detect_os_box);

        let top = gtk::Box::new(gtk::Orientation::Vertical, 24);
        top.append(&method_pages);
        top.append(&os_distro_box);

        InstallWidgets {
            top,
            method_pages,
            mediacombo,
            iso_browse,
            url_combo,
            url_options,
            urlopts_entry,
            import_entry,
            import_browse,
            kernel_info_box,
            app_entry,
            app_browse,
            oscontainer_fs,
            oscontainer_browse,
            oscontainer_notsupport,
            oscontainer_notsupport_conn,
            oscontainer_bootstrap,
            oscontainer_source,
            oscontainer_source_url,
            oscontainer_source_insecure,
            oscontainer_auth_options,
            oscontainer_source_user,
            oscontainer_source_passwd,
            oscontainer_rootpw_box,
            oscontainer_rootpw,
            container_template,
            os_distro_box,
            os_list,
            detect_os_box,
            detect_os,
            detect_os_spinner,
        }
    }
}

/// Step 3: memory and CPUs.
struct MemWidgets {
    top: gtk::Box,
    mem: gtk::SpinButton,
    phys_mem_label: gtk::Label,
    cpus: gtk::SpinButton,
    phys_cpu_label: gtk::Label,
}

impl MemWidgets {
    fn new() -> MemWidgets {
        let mem = gtk::SpinButton::with_range(0.0, 8096.0, 1.0);
        mem.set_halign(gtk::Align::Start);
        mem.set_width_chars(7);
        mem.update_property(&[gtk::accessible::Property::Label("mem")]);
        let phys_mem_label = gtk::Label::new(Some("(Insert host mem)"));
        phys_mem_label.set_halign(gtk::Align::Start);
        phys_mem_label.set_hexpand(true);
        let cpus = gtk::SpinButton::with_range(0.0, 128.0, 1.0);
        cpus.set_halign(gtk::Align::Start);
        cpus.set_width_chars(7);
        cpus.update_property(&[gtk::accessible::Property::Label("cpus")]);
        let phys_cpu_label = gtk::Label::new(Some("Insert Phys cpu count"));
        phys_cpu_label.set_halign(gtk::Align::Start);
        phys_cpu_label.set_hexpand(true);

        let table3 = gtk::Grid::builder()
            .row_spacing(4)
            .column_spacing(6)
            .build();
        for (row, (text, spin, label)) in [
            ("_Memory:", &mem, &phys_mem_label),
            ("C_PUs:", &cpus, &phys_cpu_label),
        ]
        .into_iter()
        .enumerate()
        {
            let l = gtk::Label::with_mnemonic(text);
            l.set_halign(gtk::Align::End);
            l.set_valign(gtk::Align::Start);
            l.set_margin_top(6);
            l.set_mnemonic_widget(Some(spin));
            let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
            b.append(spin);
            b.append(label);
            table3.attach(&l, 0, row as i32, 1, 1);
            table3.attach(&b, 1, row as i32, 1, 1);
        }
        let label15 = gtk::Label::new(Some("Choose Memory and CPU settings:"));
        label15.set_halign(gtk::Align::Start);
        let vbox12 = gtk::Box::new(gtk::Orientation::Vertical, 12);
        vbox12.append(&label15);
        vbox12.append(&table3);
        let top = gtk::Box::new(gtk::Orientation::Vertical, 40);
        top.append(&vbox12);
        MemWidgets {
            top,
            mem,
            phys_mem_label,
            cpus,
            phys_cpu_label,
        }
    }
}

/// Step 5: the summary, the name and the network.
struct FinishWidgets {
    top: gtk::Box,
    name: gtk::Entry,
    summary_os: gtk::Label,
    summary_install: gtk::Label,
    summary_mem: gtk::Label,
    summary_cpu: gtk::Label,
    summary_storage: gtk::Label,
    summary_storage_path: gtk::Label,
    customize: gtk::CheckButton,
    advanced_expander: gtk::Expander,
    netdev_ui_align: gtk::Box,
}

impl FinishWidgets {
    fn new() -> FinishWidgets {
        let summary_title = gtk::Label::new(None);
        summary_title.set_markup("Ready to begin the installation");
        summary_title.set_halign(gtk::Align::Start);

        let name = gtk::Entry::new();
        name.set_halign(gtk::Align::Start);
        name.set_width_chars(40);
        name.update_property(&[gtk::accessible::Property::Label("create-vm-name")]);
        let value = || {
            let l = gtk::Label::new(Some("label"));
            l.set_halign(gtk::Align::Start);
            l
        };
        let summary_os = value();
        let summary_install = value();
        let summary_mem = value();
        let summary_cpu = value();
        let summary_storage = value();
        summary_storage.set_wrap(true);
        let summary_storage_path = value();
        summary_storage_path.set_ellipsize(gtk::pango::EllipsizeMode::Start);
        summary_storage_path.set_max_width_chars(27);
        let box3 = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        box3.append(&summary_storage);
        box3.append(&summary_storage_path);
        let customize = gtk::CheckButton::with_mnemonic("C_ustomize configuration before install");
        customize.set_valign(gtk::Align::Start);

        let table4 = gtk::Grid::builder()
            .halign(gtk::Align::Start)
            .row_spacing(6)
            .column_spacing(6)
            .build();
        let rows: [(&str, &gtk::Widget); 6] = [
            ("_Name:", name.upcast_ref()),
            ("OS:", summary_os.upcast_ref()),
            ("Install:", summary_install.upcast_ref()),
            ("Memory:", summary_mem.upcast_ref()),
            ("CPUs:", summary_cpu.upcast_ref()),
            ("Storage:", box3.upcast_ref()),
        ];
        for (row, (text, widget)) in rows.into_iter().enumerate() {
            let l = gtk::Label::with_mnemonic(text);
            l.set_halign(gtk::Align::End);
            l.add_css_class("vmm-lighter");
            if row == 0 {
                l.set_mnemonic_widget(Some(&name));
            }
            table4.attach(&l, 0, row as i32, 1, 1);
            table4.attach(widget, 1, row as i32, 1, 1);
        }
        table4.attach(&customize, 1, 6, 1, 1);

        let vbox16 = gtk::Box::new(gtk::Orientation::Vertical, 12);
        vbox16.append(&summary_title);
        vbox16.append(&table4);

        let netdev_ui_align = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let grid17 = gtk::Grid::builder()
            .margin_start(12)
            .margin_top(6)
            .row_spacing(6)
            .build();
        grid17.attach(&netdev_ui_align, 0, 0, 1, 1);
        let advanced_expander = gtk::Expander::with_mnemonic("N_etwork selection");
        advanced_expander.set_child(Some(&grid17));

        let top = gtk::Box::new(gtk::Orientation::Vertical, 15);
        top.append(&vbox16);
        top.append(&advanced_expander);
        FinishWidgets {
            top,
            name,
            summary_os,
            summary_install,
            summary_mem,
            summary_cpu,
            summary_storage,
            summary_storage_path,
            customize,
            advanced_expander,
            netdev_ui_align,
        }
    }
}

/// The NIC the finish page picked.
#[derive(Clone)]
struct NewInterface {
    selection: NetSelection,
    mac: String,
}

/// What a failed install left behind: `failed_guest`'s disk, and
/// whether the install created it.
#[derive(Clone)]
struct FailedGuest {
    disk_path: Option<String>,
    storage_was_created: bool,
}

/// What `_start_install` needs: the guest's XML and its storage.
#[derive(Clone)]
struct InstallPlan {
    name: String,
    uuid: String,
    is_vz: bool,
    needs_postboot: bool,
    /// The guest with its install defaults, and the installer that
    /// builds its XML (after a tree install fetched its kernel).
    guest: virtinst_core::install::newguest::NewGuest,
    installer: virtinst_core::install::newguest::Installer,
    conn_facts: virtinst_core::install::newguest::ConnFacts,
    tree: Option<TreeInstall>,
    vol_install: Option<virtinst_core::storage::VolInstall>,
    disk_path: Option<String>,
    bootstrap: Option<BootstrapArgs>,
    /// The customize dialog's edits: the guest's XML, which replaces
    /// `guest`'s, and the installer's autostart.
    customized: Option<(String, bool)>,
}

/// A `location` install: the tree media to fetch the kernel from.
#[derive(Clone)]
struct TreeInstall {
    location: String,
    extra_args: Option<String>,
    uri: String,
    os_name: String,
}

/// `Installer._prepare` for a tree install: fetch (and, on a remote
/// connection, upload) the kernel and initrd and set the install XML to
/// boot them, raising its memory for a network install
/// (`_alter_install_resources`). Returns the media's temporary files
/// and uploaded volumes, for `_cleanup`.
fn prepare_tree_install(
    tree: &TreeInstall,
    conn: &virt::connect::Connect,
    guest: &mut virtinst_core::install::newguest::NewGuest,
    installer: &mut virtinst_core::install::newguest::Installer,
    meter: &mut asyncjob::Meter,
) -> Result<
    (
        virtinst_core::install::installertreemedia::TreeMedia,
        Vec<virt::storage_vol::StorageVol>,
    ),
    String,
> {
    use virtinst_core::install::installertreemedia::{self as treemedia, GuestInfo, TreeMedia};
    let uri = virtinst_core::connection::Uri::parse(&tree.uri);
    let info = GuestInfo {
        arch: guest.arch.clone(),
        os_type: guest.os_type.clone(),
        os_name: tree.os_name.clone(),
    };
    let mut tm = TreeMedia::new(
        &uri,
        Some(tree.location.clone()),
        None,
        None,
        None,
        None,
        None,
    )?;
    tm.set_extra_args(tree.extra_args.iter().cloned().collect());
    let (kernel, initrd, kernel_args) = match tm.prepare(&uri, &info, meter, None) {
        Ok(r) => r,
        Err(e) => {
            tm.cleanup();
            return Err(e);
        }
    };
    let (paths, tmpvols) = match treemedia::upload_media(conn, &uri, meter, vec![kernel, initrd]) {
        Ok(r) => r,
        Err(e) => {
            tm.cleanup();
            return Err(e);
        }
    };
    installer.treemedia_bootconfig = Some((paths[0].clone(), paths[1].clone(), kernel_args));

    // `_alter_install_resources`
    if tm.requires_internet(&uri, &info).unwrap_or(false) {
        let os = virtinst_core::osinfo::OsDb::load_default()
            .ok()
            .and_then(|db| db.lookup_os(&tree.os_name));
        if let Some(os) = os {
            let ram = os
                .get_network_install_required_ram(&guest.arch)
                .unwrap_or(0)
                / 1024;
            let ram = u64::try_from(ram).unwrap_or(0);
            if ram > guest.current_memory {
                eprintln!(
                    "Overriding memory to {} MiB needed for {} network install.",
                    ram / 1024,
                    os.short_id()
                );
                installer.install_memory = Some(ram);
            }
        }
    }
    Ok((tm, tmpvols))
}

/// `Installer._build_xml`, the install storage and `_create_guest`.
#[allow(clippy::too_many_arguments)]
fn create_domain(
    conn: &virt::connect::Connect,
    guest: &virtinst_core::install::newguest::NewGuest,
    installer: &virtinst_core::install::newguest::Installer,
    cf: &virtinst_core::install::newguest::ConnFacts,
    customized: Option<&(String, bool)>,
    is_vz: bool,
    vol_install: Option<&virtinst_core::storage::VolInstall>,
    created: &AtomicBool,
    meter: &mut asyncjob::Meter,
) -> Result<(), (String, String)> {
    let xml = match customized {
        Some((xml, _)) => installer
            .build_xml_from_xml(xml, guest.os.is_windows)
            .map_err(|e| (e.clone(), e))?,
        None => installer.build_xml(guest, cf),
    };
    eprintln!(
        "Generated initial_xml: {}",
        xml.initial
            .as_ref()
            .map_or("None required".to_string(), |x| format!("\n{x}"))
    );
    eprintln!("Generated final_xml: \n{}", xml.final_xml);
    if let Some(vol) = &vol_install {
        meter.start(&format!("Allocating '{}'", vol.name), None);
        vol.install(conn).map_err(|e| (e.clone(), e))?;
        created.store(true, Ordering::SeqCst);
        meter.end();
    }
    // `_create_guest`
    meter.start("Creating domain...", None);
    let verr = |e: virt::error::Error| {
        let msg = e.message().to_string();
        (msg.clone(), msg)
    };
    let dom = if is_vz {
        // `_manual_transient_create`
        let dom = virt::domain::Domain::define_xml(
            conn,
            xml.initial.as_deref().unwrap_or(&xml.final_xml),
        )
        .map_err(verr)?;
        if let Err(e) = dom.create() {
            let _ = dom.undefine();
            return Err(verr(e));
        }
        if xml.initial.as_ref().is_some_and(|x| *x != xml.final_xml) {
            virt::domain::Domain::define_xml(conn, &xml.final_xml).map_err(verr)?
        } else {
            dom
        }
    } else {
        virt::domain::Domain::create_xml(conn, xml.initial.as_deref().unwrap_or(&xml.final_xml), 0)
            .map_err(verr)?;
        virt::domain::Domain::define_xml(conn, &xml.final_xml).map_err(verr)?
    };
    if customized.is_some_and(|(_, autostart)| *autostart) {
        eprintln!("Setting autostart");
        dom.set_autostart(true).map_err(verr)?;
    }
    meter.end();
    eprintln!("Install completed");
    // Kick off pool updates
    if let Some(vol) = &vol_install {
        match virt::storage_pool::StoragePool::lookup_by_name(conn, &vol.pool_name) {
            Ok(pool) => {
                let _ = pool.refresh(0);
            }
            Err(_) => eprintln!(
                "Error looking up pool={} for refresh after VM creation.",
                vol.pool_name
            ),
        }
    }
    Ok(())
}

/// Whether a disk of the guest `xml` has its source at `path`.
fn guest_xml_has_disk_path(xml: &str, path: &str) -> bool {
    let Ok(doc) = virtinst_xml::parse_libvirt_xml(xml) else {
        return false;
    };
    let Some(devices) = doc.root_element().and_then(|r| r.find(&doc, "devices")) else {
        return false;
    };
    devices.find_all(&doc, "disk").iter().any(|disk| {
        disk.find(&doc, "source").is_some_and(|src| {
            ["file", "dev", "dir"]
                .iter()
                .any(|a| src.attribute(&doc, a) == Some(path))
        })
    })
}

/// The `virtBootstrap.bootstrap` arguments for an OS container.
#[derive(Clone)]
struct BootstrapArgs {
    src: String,
    dest: String,
    user: String,
    passwd: String,
    insecure: bool,
    root_password: String,
}

impl BootstrapArgs {
    /// Runs `virt-bootstrap`; the error is its output.
    fn run(&self) -> Result<(), String> {
        let mut cmd = std::process::Command::new("virt-bootstrap");
        if self.insecure {
            cmd.arg("--not-secure");
        }
        if !self.user.is_empty() {
            cmd.args(["--username", &self.user]);
        }
        if !self.passwd.is_empty() {
            cmd.args(["--password", &self.passwd]);
        }
        if !self.root_password.is_empty() {
            cmd.args(["--root-password", &self.root_password]);
        }
        cmd.args([&self.src, &self.dest]);
        let out = cmd.output().map_err(|e| e.to_string())?;
        if out.status.success() {
            return Ok(());
        }
        Err(format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

/// `_check_install_status`: watches the installing domain and starts
/// it again once the install shut it down.
fn watch_install_status(vm: &crate::domain::VmmDomain) {
    let id: Rc<RefCell<Option<glib::SignalHandlerId>>> = Rc::default();
    let id2 = id.clone();
    let handler = vm.connect_local("state-changed", false, move |args| {
        let vm = args[0].get::<crate::domain::VmmDomain>().ok()?;
        let done = if vm.is_crashed() {
            eprintln!("VM crashed, cancelling install plans.");
            true
        } else if !vm.is_shutoff() {
            false
        } else if vm.get_install_abort() {
            eprintln!("User manually shutdown VM, not restarting guest after install.");
            true
        } else {
            eprintln!("Install should be completed, starting VM.");
            let vm2 = vm.clone();
            glib::spawn_future_local(async move {
                if let Err(e) = vm2.run_action(crate::domain::DomainAction::Startup).await {
                    error::show_err(None, &format!("Error continuing install: {e}"), None, "")
                        .await;
                }
            });
            true
        };
        if done {
            if let Some(id) = id2.borrow_mut().take() {
                vm.disconnect(id);
            }
        }
        None
    });
    id.replace(Some(handler));
}

/// `_GuestData`: what the pages so far chose for the guest.
#[derive(Default, Clone)]
struct GuestData {
    /// The OS short id (`guest.osinfo.name`).
    osinfo: Option<String>,
    /// Its libosinfo full id.
    osinfo_id: Option<String>,
    failed_guest: Option<FailedGuest>,
    name: Option<String>,
    location: Option<String>,
    cdrom: Option<String>,
    extra_args: Option<String>,
    livecd: bool,
    /// `guest.os.init`, for an application container.
    init: Option<String>,
    /// The `/` filesystem of a container: `(type, source)`.
    filesystem: Option<(Option<String>, String)>,
    vcpus: u32,
    /// KiB.
    memory: u64,
    disk: Option<Rc<DiskPlan>>,
    interface: Option<NewInterface>,
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

    // Steps 2 to 4
    inst: InstallWidgets,
    memw: MemWidgets,
    enable_storage: gtk::CheckButton,
    storage_align: gtk::Box,
    addstorage: Rc<AddStorage>,
    fin: FinishWidgets,
    netlist: RefCell<Option<Rc<NetList>>>,

    gdata: RefCell<GuestData>,
    storage_browser: RefCell<Option<StorageBrowser>>,
    /// `_customize_window`.
    customize_window: RefCell<Option<crate::vmwindow::VmmVMWindow>>,
    detect_os_in_progress: Cell<bool>,
    os_already_detected_for_media: Cell<bool>,
    /// Bumped per detection run, so a timed out one is ignored.
    detect_seq: Cell<u64>,

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
        let inst = InstallWidgets::new();
        let memw = MemWidgets::new();
        pages.add_named(&page_name, Some(&PAGE_NAME.to_string()));
        pages.add_named(&inst.top, Some(&PAGE_INSTALL.to_string()));
        pages.add_named(&memw.top, Some(&PAGE_MEM.to_string()));
        // Step 4: storage
        let enable_storage =
            gtk::CheckButton::with_mnemonic("_Enable storage for this virtual machine");
        enable_storage.set_active(true);
        let addstorage = AddStorage::new(None);
        let storage_align = gtk::Box::new(gtk::Orientation::Vertical, 0);
        storage_align.append(&addstorage.top_box);
        let storage_area = gtk::Box::new(gtk::Orientation::Vertical, 12);
        storage_area.append(&enable_storage);
        storage_area.append(&storage_align);
        pages.add_named(&storage_area, Some(&PAGE_STORAGE.to_string()));
        let fin = FinishWidgets::new();
        pages.add_named(&fin.top, Some(&PAGE_FINISH.to_string()));
        // Size the window to the shown page only.
        pages.set_vhomogeneous(false);
        pages.set_hhomogeneous(false);

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
            inst,
            memw,
            enable_storage,
            storage_align,
            addstorage,
            fin,
            netlist: RefCell::default(),
            gdata: RefCell::default(),
            storage_browser: RefCell::default(),
            customize_window: RefCell::default(),
            detect_os_in_progress: Cell::new(false),
            os_already_detected_for_media: Cell::new(false),
            detect_seq: Cell::new(0),
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

        // Install page listeners
        let inst = &i.inst;
        inst.iso_browse
            .connect_clicked(self.handler(VmmCreateVM::browse_iso));
        inst.import_browse
            .connect_clicked(self.handler(VmmCreateVM::browse_import));
        inst.app_browse
            .connect_clicked(self.handler(VmmCreateVM::browse_app));
        inst.oscontainer_browse
            .connect_clicked(self.handler(VmmCreateVM::browse_oscontainer));
        inst.oscontainer_bootstrap
            .connect_toggled(self.handler(VmmCreateVM::container_source_toggle));
        inst.detect_os
            .connect_toggled(self.handler(VmmCreateVM::detect_os_toggled));
        let changed = self.handler::<()>(|t| {
            let entry = t.0.inst.mediacombo.entry.clone();
            t.detectable_media_widget_changed(Some(&entry));
        });
        inst.mediacombo.connect_changed(move || changed(&()));
        inst.mediacombo
            .entry
            .connect_activate(self.handler(|t| t.detectable_media_widget_changed(None)));
        inst.url_combo.entry.connect_changed(self.handler(|t| {
            let entry = t.0.inst.url_combo.entry.clone();
            t.detectable_media_widget_changed(Some(&entry));
        }));
        inst.url_combo
            .entry
            .connect_activate(self.handler(|t| t.detectable_media_widget_changed(None)));

        // Storage page listeners
        let weak = Rc::downgrade(&self.0);
        i.addstorage.connect_browse_clicked(move |entry| {
            if let Some(inner) = weak.upgrade() {
                VmmCreateVM(inner).browse_entry(entry, BrowseReason::Image);
            }
        });
        // Summary page listeners
        i.fin
            .name
            .connect_changed(self.handler(VmmCreateVM::name_changed));
        i.finish
            .connect_clicked(self.handler(VmmCreateVM::finish_clicked));

        i.enable_storage.connect_toggled(self.handler(|t| {
            let active = t.0.enable_storage.is_active();
            t.0.storage_align.set_sensitive(active);
        }));

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
        if let Some(b) = self.0.storage_browser.borrow().as_ref() {
            b.close();
        }
        self.cleanup_customize_window();
        self.set_conn(None);
        self.0.gdata.replace(GuestData::default());
    }

    /// `_cleanup_customize_window`. Taken out first: cleaning it up can
    /// re-enter here through its `closed` handler.
    fn cleanup_customize_window(&self) {
        let window = self.0.customize_window.take();
        if let Some(window) = window {
            window.cleanup();
        }
    }

    /// `_show_customize_dialog`.
    fn show_customize_dialog(&self, plan: InstallPlan) -> Result<(), String> {
        let conn = self.0.conn.borrow().clone().ok_or("Not connected")?;
        let xml = plan.guest.to_xml(&plan.conn_facts);
        let vdomain = crate::domain::VmmDomain::new_virtinst(&conn, xml)?;

        let weak = Rc::downgrade(&self.0);
        let finished = move |vdomain: &crate::domain::VmmDomain| {
            let Some(inner) = weak.upgrade() else { return };
            let this = VmmCreateVM(inner);
            if !this.0.window.is_visible() {
                return;
            }
            eprintln!("User finished customize dialog, starting install");
            this.0.gdata.borrow_mut().failed_guest = None;
            let mut plan = plan.clone();
            plan.name = vdomain.get_name();
            plan.customized = vdomain.virtinst_backend();
            glib::spawn_future_local(async move { this.start_install(plan).await });
        };
        let weak = Rc::downgrade(&self.0);
        let closed = move || {
            let Some(inner) = weak.upgrade() else { return };
            eprintln!("User closed customize window, closing wizard");
            VmmCreateVM(inner).close_requested();
        };

        // We specifically don't use vmmVMWindow.get_instance here since
        // it's not a top level VM window
        self.cleanup_customize_window();
        let window = crate::vmwindow::VmmVMWindow::new_customize(
            &vdomain,
            self.0.window.upcast_ref(),
            crate::vmwindow::CustomizeHandlers {
                finished: Rc::new(finished),
                closed: Rc::new(closed),
            },
        );
        window.show();
        self.0.customize_window.replace(Some(window));
        Ok(())
    }

    /// `_close_requested`: removes the disk a failed install created
    /// before closing.
    fn close_requested(&self) {
        let failed = self.0.gdata.borrow().failed_guest.clone();
        let handle = self.0.conn.borrow().as_ref().and_then(|c| c.handle());
        let (Some(failed), Some(handle)) = (failed, handle) else {
            self.close();
            return;
        };
        let this = self.clone();
        glib::spawn_future_local(async move {
            let err = asyncjob::run(
                Some(&this.0.window),
                "Removing disk images",
                "Removing disk images we created for this virtual machine.",
                true,
                move |job| {
                    // `Installer.cleanup_created_disks`
                    let Some(path) = failed.disk_path.filter(|_| failed.storage_was_created) else {
                        return Ok(());
                    };
                    let name = path.rsplit('/').next().unwrap_or_default().to_string();
                    let mut meter = job.meter();
                    meter.start(&format!("Removing disk '{name}'"), None);
                    match virt::storage_vol::StorageVol::lookup_by_path(handle.connect(), &path)
                        .and_then(|vol| vol.delete(0))
                    {
                        Ok(()) => meter.end(),
                        Err(e) => eprintln!("Failed to remove disk '{name}': {e}"),
                    }
                    Ok(())
                },
            )
            .await;
            if let Some((error, details)) = err {
                eprintln!("Error cleaning up disk images:\nerror={error}\ndetails={details}");
            }
            this.close();
        });
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

        // Everything from this point forward should be connection
        // independent
        let inst = &i.inst;
        let config = engine::get().config();

        // Distro/Variant
        inst.os_list.reset_state();
        i.os_already_detected_for_media.set(false);

        let media_rows =
            |urls: Vec<String>| urls.into_iter().map(|u| (Some(u.clone()), u)).collect();

        // Install URL
        inst.urlopts_entry.set_text("");
        inst.url_combo.entry.set_text("");
        inst.url_options.set_expanded(false);
        inst.url_combo.set_rows(media_rows(config.get_media_urls()));

        // Install import
        inst.import_entry.set_text("");

        // Install container app
        inst.app_entry.set_text("/bin/sh");

        // Install container OS
        inst.oscontainer_fs.set_text("");
        inst.oscontainer_source_url.entry.set_text("");
        inst.oscontainer_source_user.set_text("");
        inst.oscontainer_source_passwd.set_text("");
        inst.oscontainer_source_insecure.set_active(false);
        inst.oscontainer_bootstrap.set_active(false);
        inst.oscontainer_auth_options.set_expanded(false);
        inst.oscontainer_rootpw.set_text("");
        inst.oscontainer_source_url
            .set_rows(media_rows(config.get_container_urls()));

        // Install VZ container from template
        inst.container_template.set_text("centos-7-x86_64");

        // Storage
        i.enable_storage.set_active(true);
        i.addstorage.reset_state();

        // Final page
        i.fin.name.set_text("");
        i.fin.customize.set_active(false);
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

        i.inst.kernel_info_box.set_visible(!installable_arch);
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

        // ISO media
        // Dependent on connection so we need to do this here
        i.inst.mediacombo.set_conn(Some(conn.clone()));
        i.inst.mediacombo.reset_state(false);

        // Allow container bootstrap only for local connection and
        // only if virt-bootstrap is installed. Otherwise, show message.
        let is_local = !conn.is_remote();
        let vb_installed = is_virt_bootstrap_installed();
        let vb_enabled = is_local && vb_installed;
        let inst = &i.inst;
        inst.oscontainer_notsupport_conn.set_visible(!is_local);
        inst.oscontainer_notsupport.set_visible(!vb_installed);
        inst.oscontainer_bootstrap.set_visible(vb_enabled);
        inst.oscontainer_source.set_visible(vb_enabled);
        inst.oscontainer_rootpw_box.set_visible(vb_enabled);

        // Memory
        let memory = conn.host_memory_size();
        i.memw.phys_mem_label.set_markup(&format!(
            "<span size='small'>Up to {} available on the host</span>",
            pretty_memory(memory)
        ));
        i.memw.mem.set_range(50.0, (memory / 1024) as f64);

        // CPU
        let phys_cpus = conn.host_active_processor_count();
        i.memw.phys_cpu_label.set_markup(&format!(
            "<span size='small'>Up to {phys_cpus} available</span>"
        ));
        i.memw.cpus.set_range(1.0, phys_cpus.max(1) as f64);

        // Storage
        i.addstorage.set_conn(Some(conn.clone()));
        i.addstorage.reset_state();

        // Networking
        i.fin.advanced_expander.set_expanded(false);
        let netlist = NetList::new(Some(conn.clone()));
        let grid = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(6)
            .build();
        grid.attach(&netlist.top_label, 0, 0, 1, 1);
        grid.attach(&netlist.top_box, 1, 0, 1, 1);
        i.fin.netdev_ui_align.append(&grid);
        netlist.reset_state();
        i.netlist.replace(Some(netlist));
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
        if i.netlist.take().is_some() {
            while let Some(child) = i.fin.netdev_ui_align.first_child() {
                i.fin.netdev_ui_align.remove(&child);
            }
        }

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
        if curpage == PAGE_INSTALL {
            // Make sure we have detected the OS before validating the
            // page
            if self.start_detect_os_if_needed(true) {
                return;
            }
        }
        let this = self.clone();
        glib::spawn_future_local(async move {
            if !this.validate(curpage).await {
                return;
            }
            this.0.forward.grab_focus();
            if curpage == PAGE_NAME {
                this.set_install_page();
            }
            this.set_page(this.get_next_pagenum(curpage));
        });
    }

    /// `set_current_page` plus `_page_changed`.
    fn set_page(&self, pagenum: usize) {
        let i = &self.0;
        i.cur_page.set(pagenum);
        i.pages.set_visible_child_name(&pagenum.to_string());
        if pagenum == PAGE_FINISH {
            self.populate_summary();
            i.finish.grab_focus();
        }
        i.back.set_sensitive(pagenum != PAGE_NAME);
        i.forward.set_visible(pagenum != PAGE_FINISH);
        i.finish.set_visible(pagenum == PAGE_FINISH);
        self.set_page_num_text(pagenum);
    }

    // Page validation routines

    async fn validate(&self, pagenum: usize) -> bool {
        match pagenum {
            PAGE_NAME => self.validate_intro_page(),
            PAGE_INSTALL => self.validate_install_page().await,
            PAGE_MEM => self.validate_mem_page(),
            PAGE_STORAGE => self.validate_storage_page().await,
            PAGE_FINISH => self.validate_final_page().await,
            _ => true,
        }
    }

    /// `_validate_intro_page`: the guest has to resolve.
    fn validate_intro_page(&self) -> bool {
        self.0.capsinfo.borrow().is_some()
    }

    /// `_name_changed`: the default disk path follows the name.
    fn name_changed(&self) {
        let i = &self.0;
        let newname = i.fin.name.text().to_string();
        if i.cur_page.get() != PAGE_FINISH || newname.is_empty() {
            return;
        }
        if !self.is_default_storage() {
            return;
        }
        let Some(conn) = i.conn.borrow().clone() else {
            return;
        };
        let Some(handle) = conn.handle() else {
            return;
        };
        let fmt = conn.get_default_storage_format();
        let is_session = conn.is_qemu_unprivileged();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let name = newname.clone();
            let path = gio::spawn_blocking(move || {
                get_default_path(handle.connect(), &name, fmt.as_deref(), &[], is_session)
            })
            .await
            .ok()
            .and_then(|r| r.ok())
            .flatten();
            // Only if the name didn't change again meanwhile.
            if this.0.fin.name.text() == newname {
                this.populate_summary_storage(path.as_deref());
            }
        });
    }

    /// `_is_default_storage`.
    fn is_default_storage(&self) -> bool {
        self.0.addstorage.is_default_storage() && !self.should_skip_disk_page()
    }

    /// `_populate_summary_storage`.
    fn populate_summary_storage(&self, path: Option<&str>) {
        let i = &self.0;
        let gdata = i.gdata.borrow();
        let mut storagesize = String::new();
        let storagepath = if let Some(disk) = &gdata.disk {
            if let Some(vol) = &disk.vol_install {
                storagesize = format!(
                    "{:.1} GiB",
                    vol.capacity as f64 / (1024.0 * 1024.0 * 1024.0)
                );
            }
            let path = path.or(disk.path.as_deref()).unwrap_or_default();
            format!(
                "<span size='small'>{}</span>",
                glib::markup_escape_text(path)
            )
        } else if let Some((_, source)) = &gdata.filesystem {
            format!(
                "<span size='small'>{}</span>",
                glib::markup_escape_text(source)
            )
        } else {
            "None".to_string()
        };
        i.fin.summary_storage.set_markup(&storagesize);
        i.fin.summary_storage.set_visible(!storagesize.is_empty());
        i.fin.summary_storage_path.set_markup(&storagepath);
    }

    /// `_populate_summary`.
    fn populate_summary(&self) {
        let i = &self.0;
        let install = match self.get_config_install_page() {
            InstallPage::Iso => "Local CDROM/ISO",
            InstallPage::Url => "URL Install Tree",
            InstallPage::Import => "Import existing OS image",
            InstallPage::Manual => "Manual install",
            InstallPage::ContainerApp => "Application container",
            InstallPage::ContainerOs => "Operating system container",
            InstallPage::VzTemplate => "Virtuozzo container",
        };
        let (os_label, mem, cpu) = {
            let gdata = i.gdata.borrow();
            let os = gdata
                .osinfo
                .as_deref()
                .and_then(|n| oslist::all_os().iter().find(|o| o.name == n).cloned())
                .unwrap_or_else(virtinst_core::osinfo::OsVariant::generic);
            (
                os.label,
                pretty_memory(gdata.memory),
                gdata.vcpus.to_string(),
            )
        };
        i.fin.summary_os.set_text(&os_label);
        i.fin.summary_install.set_text(install);
        i.fin.summary_mem.set_text(&mem);
        i.fin.summary_cpu.set_text(&cpu);
        self.populate_summary_storage(None);

        let nsource = i
            .netlist
            .borrow()
            .as_ref()
            .and_then(|n| n.get_network_selection())
            .and_then(|sel| sel.1);
        if nsource.is_none() {
            i.fin.advanced_expander.set_expanded(true);
        }
    }

    /// `_validate_final_page`.
    async fn validate_final_page(&self) -> bool {
        let i = &self.0;
        let parent = Some(&i.window);
        let Some(conn) = i.conn.borrow().clone() else {
            return false;
        };
        // HV + Arch selection
        let name = i.fin.name.text().to_string();
        if Some(&name) != i.gdata.borrow().name.as_ref() {
            // `Guest.validate_name`
            let res =
                virtinst_core::storage::validate_generic_name("Guest", &name).and_then(|_| {
                    if conn.list_vms().iter().any(|v| v.get_name() == name) {
                        Err(format!("Guest name '{name}' is already in use."))
                    } else {
                        Ok(())
                    }
                });
            if let Err(e) = res {
                error::val_err_text2(parent, "Invalid guest name", Some(&e)).await;
                return false;
            }
            i.gdata.borrow_mut().name = Some(name);
            if self.is_default_storage() {
                eprintln!(
                    "User changed VM name and using default storage, re-validating with new \
                     default storage path."
                );
                if !self.validate_storage_page().await {
                    return false;
                }
            }
        }

        let mac = virtinst_core::cloner::generate_mac(conn.is_qemu());
        let Some(netlist) = i.netlist.borrow().clone() else {
            return false;
        };
        let Some(selection) = netlist.get_network_selection() else {
            return false;
        };
        if let Err(e) = netlist
            .validate_device(&i.window, &selection.0, selection.1.as_deref(), &mac)
            .await
        {
            error::show_err(
                parent,
                &format!("Uncaught error validating install parameters: {e}"),
                None,
                "",
            )
            .await;
            return false;
        }
        i.gdata.borrow_mut().interface = Some(NewInterface { selection, mac });
        true
    }

    /// `_finish_clicked`.
    fn finish_clicked(&self) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            // Validate the final page
            if !this.validate(this.0.cur_page.get()).await {
                return;
            }
            eprintln!("Starting create finish() sequence");
            this.0.gdata.borrow_mut().failed_guest = None;

            this.0.window.set_cursor_from_name(Some("progress"));
            // `build_guest` + `build_installer` + `set_install_defaults`
            let res = match this.build_install().await {
                Ok(plan) if this.0.fin.customize.is_active() => {
                    eprintln!("User requested 'customize', launching dialog");
                    this.show_customize_dialog(plan)
                }
                Ok(plan) => {
                    this.start_install(plan).await;
                    Ok(())
                }
                Err(e) => Err(e),
            };
            match res {
                Ok(()) => {}
                Err(e) => {
                    this.0.window.set_cursor_from_name(None);
                    error::show_err(
                        Some(&this.0.window),
                        &format!("Error starting installation: {e}"),
                        None,
                        "",
                    )
                    .await;
                }
            }
        });
    }

    /// `_GuestData.build_guest` and `build_installer`, then
    /// `installer.set_install_defaults(guest)` and the XML it makes.
    async fn build_install(&self) -> Result<InstallPlan, String> {
        use virtinst_core::install::newguest as ng;
        let i = &self.0;
        let conn = i.conn.borrow().clone().ok_or("No connection")?;
        let handle = conn.handle().ok_or("Not connected")?;
        let caps = i.caps.borrow().clone().ok_or("No capabilities")?;
        let capsinfo = i.capsinfo.borrow().clone().ok_or("No capabilities")?;
        let gdata = i.gdata.borrow().clone();

        let cf = ng::ConnFacts {
            uri: virtinst_core::connection::Uri::parse(&conn.get_uri()),
            host_arch: caps.host.cpu.arch.clone().unwrap_or_default(),
            host_cpu_vendor: caps.host.cpu.vendor.clone(),
        };
        let features = caps
            .guest_for_info(&capsinfo)
            .map(|g| g.features.clone())
            .unwrap_or_default();
        let capsf = ng::CapsFacts {
            machines: capsinfo.machines.clone(),
            acpi: features.acpi == Some(true),
            apic: features.apic == Some(true),
            pae: features.pae,
        };

        // `set_capabilities_defaults`
        let arch = capsinfo.arch.clone().unwrap_or_default();
        let os_type = capsinfo.os_type.clone().unwrap_or_default();
        let domain_type = capsinfo.hypervisor_type.clone().unwrap_or_default();
        let is_xenpv = matches!(os_type.as_str(), "xen" | "linux" | "xenpvh");
        let machine = self.get_config_machine().or_else(|| {
            capsinfo
                .recommended_machine(conn.is_qemu() || conn.is_test())
                .map(String::from)
        });
        let os = match gdata.osinfo_id.as_deref().and_then(oslist::lookup_osinfo) {
            Some(os) => ng::OsFacts::from_osinfo(&os, &arch),
            None => ng::OsFacts::default(),
        };
        let mut guest = ng::NewGuest {
            emulator: capsinfo
                .emulator
                .clone()
                .filter(|_| !is_xenpv && domain_type != "vz"),
            loader: capsinfo.loader.clone(),
            domain_type,
            os_type,
            arch: arch.clone(),
            machine,
            name: gdata.name.clone().unwrap_or_default(),
            memory: gdata.memory,
            current_memory: gdata.memory,
            vcpus: gdata.vcpus,
            os,
            uefi_requested: i.uefi_requested.get().then_some(true),
            init: gdata.init.clone(),
            ..Default::default()
        };

        if let Some((fstype, source)) = &gdata.filesystem {
            guest.devices.push(ng::Device::Filesystem {
                fstype: fstype.clone(),
                accessmode: None,
                source: source.clone(),
                target: "/".into(),
            });
        }
        let mut vol_install = None;
        let mut disk_path = None;
        if let Some(plan) = &gdata.disk {
            vol_install = plan.vol_install.clone();
            disk_path = plan.path.clone();
            guest
                .devices
                .push(ng::Device::Disk(virtinst_core::devices::build::NewDisk {
                    device: "disk".into(),
                    path: plan.path.clone(),
                    props: plan.props.clone(),
                    sparse_install: plan.vol_install.as_ref().is_some_and(|v| v.allocation == 0),
                    ..Default::default()
                }));
        }
        if let Some(nic) = &gdata.interface {
            let (nettype, source, source_mode, portgroup) = nic.selection.clone();
            guest.devices.push(ng::Device::Interface(
                virtinst_core::devices::build::NewInterface {
                    nettype,
                    source,
                    source_mode,
                    portgroup,
                    mac: nic.mac.clone(),
                    model: None,
                },
            ));
        }

        // A tree install: an ISO location is attached like an install
        // CD-ROM, and the kernel and initrd are fetched at install time.
        let uri = virtinst_core::connection::Uri::parse(&conn.get_uri());
        let tree_iso = match &gdata.location {
            Some(loc) => {
                use virtinst_core::install::installertreemedia::TreeMedia;
                TreeMedia::new(&uri, Some(loc.clone()), None, None, None, None, None)?
                    .cdrom_path()
                    .map(str::to_string)
            }
            None => None,
        };

        // The installer's CD-ROM storage, `manage_path` style.
        let installer = match gdata.cdrom.clone().or(tree_iso) {
            Some(path) => {
                let is_local = !conn.is_remote();
                let h = handle.clone();
                let (path, props) = gio::spawn_blocking(move || {
                    let path = virtinst_core::storage::manage_path(h.connect(), &path)
                        .map(|m| m.path)
                        .unwrap_or(path);
                    let props = crate::domain::path_props(h.connect(), &path, is_local);
                    (path, props)
                })
                .await
                .map_err(|_| "Worker thread panicked".to_string())?;
                match gdata.location {
                    Some(_) => ng::Installer::for_location(Some(
                        ng::Installer::for_cdrom(&path, Some(props))
                            .cdrom
                            .expect("a cdrom"),
                    )),
                    None => ng::Installer::for_cdrom(&path, Some(props)),
                }
            }
            None if gdata.location.is_some() => ng::Installer::for_location(None),
            None => ng::Installer::default(),
        };

        guest.set_default_machine(&cf, &capsf);
        let domcaps = conn
            .fetch_domcaps(
                guest.emulator.clone(),
                Some(guest.arch.clone()),
                guest.machine.clone(),
                Some(guest.domain_type.clone()),
            )
            .await
            .unwrap_or_default();
        guest.cpu_ctx = self.cpu_context(&conn, &guest, &domcaps).await;
        installer.set_install_defaults(&mut guest, &cf, &capsf, &domcaps)?;

        let bootstrap = (gdata.filesystem.as_ref().is_some_and(|f| f.0.is_none())
            && i.inst.oscontainer_bootstrap.is_active())
        .then(|| BootstrapArgs {
            src: self.get_config_oscontainer_source_url(false),
            dest: i.inst.oscontainer_fs.text().to_string(),
            user: i.inst.oscontainer_source_user.text().trim().to_string(),
            passwd: i.inst.oscontainer_source_passwd.text().to_string(),
            insecure: i.inst.oscontainer_source_insecure.is_active(),
            root_password: i.inst.oscontainer_rootpw.text().to_string(),
        });

        let tree = gdata.location.clone().map(|location| TreeInstall {
            location,
            extra_args: gdata.extra_args.clone(),
            uri: conn.get_uri(),
            os_name: gdata.osinfo.clone().unwrap_or_else(|| "generic".into()),
        });
        Ok(InstallPlan {
            name: guest.name.clone(),
            uuid: guest.uuid.clone(),
            is_vz: guest.domain_type == "vz",
            needs_postboot: installer.requires_postboot_xml_changes(),
            guest,
            installer,
            conn_facts: cf,
            tree,
            vol_install,
            disk_path,
            bootstrap,
            customized: None,
        })
    }

    /// What `DomainCpu.set_defaults` reads: the app default CPU mode
    /// and, for `host-model-only`, the host model's security features.
    async fn cpu_context(
        &self,
        conn: &VmmConnection,
        guest: &virtinst_core::install::newguest::NewGuest,
        domcaps: &virtinst_core::capabilities::DomainCapabilities,
    ) -> virtinst_core::domain::cpu::CpuEditContext {
        let mode = domcaps.app_default_cpu_mode(
            Some(&guest.domain_type),
            Some(&guest.arch),
            guest.machine.as_deref(),
        );
        let mut security_features = vec![];
        if mode == "host-model-only" {
            if let (Some(cpu_xml), Some(handle)) =
                (domcaps.host_model_cpu_xml.clone(), conn.handle())
            {
                let params = (
                    domcaps.path.clone(),
                    domcaps.arch.clone(),
                    domcaps.machine.clone(),
                    domcaps.domain.clone(),
                );
                let expanded = gio::spawn_blocking(move || {
                    virtinst_core::capabilities::domcaps::baseline_hypervisor_cpu(
                        handle.connect(),
                        params,
                        &cpu_xml,
                    )
                })
                .await
                .ok()
                .flatten();
                if let Some(xml) = expanded {
                    security_features =
                        virtinst_core::capabilities::domcaps::cpu_security_features(&xml);
                }
            }
        }
        virtinst_core::domain::cpu::CpuEditContext {
            app_default_mode: mode.to_string(),
            host_cpu_model: conn.host_cpu_model(),
            security_features,
        }
    }

    /// `_start_install` + `_do_async_install` + `_install_finished_cb`.
    async fn start_install(&self, plan: InstallPlan) {
        let i = &self.0;
        let Some(conn) = i.conn.borrow().clone() else {
            return;
        };
        let Some(handle) = conn.handle() else {
            return;
        };
        let storage_created = Arc::new(AtomicBool::new(false));
        let created = storage_created.clone();
        let InstallPlan {
            name,
            uuid,
            is_vz,
            needs_postboot,
            mut guest,
            mut installer,
            conn_facts,
            tree,
            vol_install,
            disk_path,
            bootstrap,
            customized,
        } = plan;
        // `_sync_disk_storage_params`: the new storage goes with the
        // customized guest only while a disk still uses its path.
        let vol_install = vol_install.filter(|_| match &customized {
            Some((xml, _)) => disk_path
                .as_deref()
                .is_some_and(|path| guest_xml_has_disk_path(xml, path)),
            None => true,
        });
        // parentobj = self._customize_window or self
        let parent: gtk::Window = match i.customize_window.borrow().as_ref() {
            Some(w) => w.window(),
            None => i.window.clone().upcast(),
        };
        let err = asyncjob::run(
            Some(&parent),
            "Creating Virtual Machine",
            "The virtual machine is now being created. Allocation of disk storage and \
             retrieval of the installation images may take a few minutes to complete.",
            true,
            move |job| {
                let mut meter = job.meter();
                if let Some(args) = bootstrap {
                    // `_create_directory_tree`
                    meter.start("Bootstrapping container", None);
                    args.run().map_err(|e| {
                        (
                            "virt-bootstrap did not complete successfully".to_string(),
                            e,
                        )
                    })?;
                }
                let conn = handle.connect();
                eprintln!("Starting background install process");
                // `installer.start_install`: `_prepare`
                let mut tree_media = None;
                if let Some(tree) = &tree {
                    let (tm, tmpvols) =
                        prepare_tree_install(tree, conn, &mut guest, &mut installer, &mut meter)
                            .map_err(|e| (e.clone(), e))?;
                    tree_media = Some((tm, tmpvols));
                }
                // `_cleanup`, however the install goes.
                let cleanup = |tree_media: Option<(
                    virtinst_core::install::installertreemedia::TreeMedia,
                    Vec<virt::storage_vol::StorageVol>,
                )>| {
                    if let Some((mut tm, tmpvols)) = tree_media {
                        tm.cleanup();
                        for vol in tmpvols {
                            eprintln!("Removing volume '{}'", vol.get_name().unwrap_or_default());
                            let _ = vol.delete(0);
                        }
                    }
                };
                let res = create_domain(
                    conn,
                    &guest,
                    &installer,
                    &conn_facts,
                    customized.as_ref(),
                    is_vz,
                    vol_install.as_ref(),
                    &created,
                    &mut meter,
                );
                cleanup(tree_media);
                res
            },
        )
        .await;

        // Wait for VM to show up
        let mut error = err;
        let mut foundvm = None;
        if error.is_none() {
            for _ in 0..200 {
                conn.tick();
                foundvm = conn.list_vms().into_iter().find(|v| v.get_uuid() == uuid);
                if foundvm.is_some() {
                    break;
                }
                glib::timeout_future(std::time::Duration::from_millis(100)).await;
            }
            if foundvm.is_none() {
                let e = format!("VM '{name}' didn't show up after expected time.");
                error = Some((e.clone(), e));
            }
        }

        i.window.set_cursor_from_name(None);
        let vm = match (error, foundvm) {
            (None, Some(vm)) => vm,
            (error, _) => {
                let (e, details) = error.unwrap_or_default();
                error::show_err(
                    Some(&parent),
                    &format!("Unable to complete install: '{e}'"),
                    Some(&details),
                    "",
                )
                .await;
                i.gdata.borrow_mut().failed_guest = Some(FailedGuest {
                    disk_path,
                    storage_was_created: storage_created.load(Ordering::SeqCst),
                });
                return;
            }
        };

        if vm.is_shutoff() {
            // Domain is already shutdown, but no error was raised.
            // Probably means guest had no 'install' phase, as in
            // for live cds. Try to restart the domain.
            let _ = vm.run_action(crate::domain::DomainAction::Startup).await;
        } else if needs_postboot {
            // Register a status listener, which will restart the
            // guest after the install has finished
            watch_install_status(&vm);
        }

        self.close();
        // Launch details dialog for new VM
        crate::vmwindow::VmmVMWindow::show_instance(&vm);
    }

    /// `_validate_storage_page`.
    async fn validate_storage_page(&self) -> bool {
        let i = &self.0;
        let parent = Some(&i.window);
        let Some(conn) = i.conn.borrow().clone() else {
            return false;
        };
        let Some(handle) = conn.handle() else {
            return false;
        };
        let instpage = self.get_config_install_page();
        let is_import = instpage == InstallPage::Import;
        let storage_enabled = is_import || i.enable_storage.is_active();

        let mut disk = None;
        if storage_enabled {
            // `_get_storage_path` + `build_device`. Don't generate a new
            // path if the install failed.
            let is_default = !is_import && i.addstorage.is_default_storage();
            let failed_path = i
                .gdata
                .borrow()
                .failed_guest
                .as_ref()
                .and_then(|f| f.disk_path.clone())
                .filter(|_| is_default);
            if let Some(p) = &failed_path {
                eprintln!("Reusing failed disk path={p}");
            }
            let req = DiskRequest {
                vmname: i.gdata.borrow().name.clone().unwrap_or_default(),
                is_default_storage: is_default && failed_path.is_none(),
                entry_path: if is_import {
                    i.inst.import_entry.text().to_string()
                } else {
                    failed_path.unwrap_or_else(|| i.addstorage.entry_path())
                },
                size: i.addstorage.size(),
                fmt: conn.get_default_storage_format(),
                collide_paths: vec![],
                is_session: conn.is_qemu_unprivileged(),
                is_local: !conn.is_remote(),
            };
            let plan = gio::spawn_blocking(move || plan_disk(handle.connect(), &req))
                .await
                .unwrap_or_else(|_| Err("Worker thread panicked".to_string()));
            let plan = match plan {
                Ok(p) => p,
                Err(e) => {
                    error::val_err_text2(parent, "Storage parameter error.", Some(&e)).await;
                    return false;
                }
            };

            // `validate_device`
            let Some(path) = plan.path.clone() else {
                error::val_err(parent, "A storage path must be specified.").await;
                return false;
            };
            let names = match conn.handle() {
                Some(handle) => {
                    let path = path.clone();
                    gio::spawn_blocking(move || {
                        crate::domain::path_in_use_by(handle.connect(), &path, false, false)
                    })
                    .await
                    .unwrap_or_else(|_| Err("Worker thread panicked".to_string()))
                }
                None => Ok(vec![]),
            };
            let names = match names {
                Ok(n) => n,
                Err(e) => {
                    error::val_err_text2(parent, "Storage parameter error.", Some(&e)).await;
                    return false;
                }
            };
            if !names.is_empty() {
                let msg = format!(
                    "Disk '{path}' is already in use by other guests {}",
                    crate::addhardware::py_list(&names)
                );
                if !error::yes_no(parent, &msg, Some("Do you really want to use the disk?")).await {
                    return false;
                }
            }
            disk = Some(Rc::new(plan));
        }

        let mut gdata = i.gdata.borrow_mut();
        if instpage == InstallPage::Iso {
            // CD/ISO install and no disks implies LiveCD
            gdata.livecd = !storage_enabled;
        }
        gdata.disk = disk;
        true
    }

    fn validate_mem_page(&self) -> bool {
        let mut gdata = self.0.gdata.borrow_mut();
        gdata.vcpus = self.0.memw.cpus.value() as u32;
        gdata.memory = self.0.memw.mem.value() as u64 * 1024;
        true
    }

    fn is_container_install(&self) -> bool {
        matches!(
            self.get_config_install_page(),
            InstallPage::ContainerApp | InstallPage::ContainerOs | InstallPage::VzTemplate
        )
    }

    /// `_get_config_detectable_media`: `(cdrom, location)`.
    fn get_config_detectable_media(&self) -> (Option<String>, Option<String>) {
        match self.get_config_install_page() {
            InstallPage::Iso => (Some(self.0.inst.mediacombo.get_path(false)), None),
            InstallPage::Url => (None, Some(self.0.inst.url_combo.entry.text().to_string())),
            _ => (None, None),
        }
        .map_empty()
    }

    /// `_get_config_url_info`.
    fn get_config_url_info(&self, store_media: bool) -> (String, String) {
        let media = self.0.inst.url_combo.entry.text().trim().to_string();
        let extra = self.0.inst.urlopts_entry.text().trim().to_string();
        if !media.is_empty() && store_media {
            engine::get().config().add_media_url(&media);
        }
        (media, extra)
    }

    /// `_get_config_oscontainer_source_url`.
    fn get_config_oscontainer_source_url(&self, store_media: bool) -> String {
        let src_url = self
            .0
            .inst
            .oscontainer_source_url
            .entry
            .text()
            .trim()
            .to_string();
        if !src_url.is_empty() && store_media {
            engine::get().config().add_container_url(&src_url);
        }
        src_url
    }

    /// `_change_os_detect`.
    fn change_os_detect(&self, sensitive: bool) {
        let os_list = &self.0.inst.os_list;
        os_list.set_sensitive(sensitive);
        if !sensitive && os_list.get_selected_os().is_none() {
            os_list
                .search_entry
                .set_text("Waiting for install media / source");
        }
    }

    /// `_set_install_page`: step 2's widgets for the chosen method.
    fn set_install_page(&self) {
        let i = &self.0;
        let instpage = self.get_config_install_page();
        let is_remote = i.conn.borrow().as_ref().is_some_and(|c| c.is_remote());

        // Setting OS value for container doesn't matter presently
        i.inst
            .os_distro_box
            .set_visible(!self.is_container_install());

        let enabledetect =
            instpage == InstallPage::Url || (instpage == InstallPage::Iso && !is_remote);
        i.inst.detect_os_box.set_visible(enabledetect);
        let dodetect = enabledetect && i.inst.detect_os.is_active();
        self.change_os_detect(!dodetect);

        // Manual installs have nothing to ask for
        let has_install = instpage != InstallPage::Manual;
        i.inst.method_pages.set_visible(has_install);
        if !has_install {
            i.inst.os_list.search_entry.grab_focus();
        }
        i.inst.method_pages.set_visible_child_name(instpage.name());
    }

    // Install page listeners

    /// `_detectable_media_widget_changed`.
    fn detectable_media_widget_changed(&self, entry: Option<&gtk::Entry>) {
        self.0.os_already_detected_for_media.set(false);
        // If the text entry widget has focus, don't fire
        // detect_media_os, it means the user is probably typing. It
        // will be detected when the user activates the widget, or we
        // try to switch pages
        if entry.is_some_and(|e| e.has_focus() || e.focus_child().is_some()) {
            return;
        }
        self.start_detect_os_if_needed(false);
    }

    fn detect_os_toggled(&self) {
        let detect_os = &self.0.inst.detect_os;
        if !detect_os.get_visible() {
            return;
        }
        // We are only here if the user explicitly changed detection UI
        let dodetect = detect_os.is_active();
        self.change_os_detect(!dodetect);
        if dodetect {
            self.0.os_already_detected_for_media.set(false);
            self.start_detect_os_if_needed(false);
        }
    }

    /// `_browse_file`: the storage browser, filling `entry` (or
    /// calling `cb`) with the pick.
    fn browse_file(&self, reason: BrowseReason, cb: impl Fn(&str) + 'static) {
        let i = &self.0;
        let Some(conn) = i.conn.borrow().clone() else {
            return;
        };
        // A browser for an older connection is thrown away.
        let stale = i
            .storage_browser
            .borrow()
            .as_ref()
            .is_some_and(|b| b.conn() != conn);
        if stale {
            i.storage_browser.replace(None);
        }
        storagebrowse::browse(&i.storage_browser, &conn, &i.window, reason, cb);
    }

    fn browse_entry(&self, entry: &gtk::Entry, reason: BrowseReason) {
        let entry = entry.clone();
        self.browse_file(reason, move |path| entry.set_text(path));
    }

    fn browse_iso(&self) {
        let media = self.0.inst.mediacombo.clone();
        self.browse_file(BrowseReason::IsoMedia, move |path| media.set_path(path));
    }

    fn browse_import(&self) {
        self.browse_entry(&self.0.inst.import_entry, BrowseReason::Image);
    }

    fn browse_app(&self) {
        self.browse_entry(&self.0.inst.app_entry, BrowseReason::Image);
    }

    fn browse_oscontainer(&self) {
        self.browse_entry(&self.0.inst.oscontainer_fs, BrowseReason::Fs);
    }

    /// `_container_source_toggle`: enable the container source URL
    /// entry on checkbox click.
    fn container_source_toggle(&self) {
        let inst = &self.0.inst;
        let enable_src = inst.oscontainer_bootstrap.is_active();
        inst.oscontainer_source.set_sensitive(enable_src);
        inst.oscontainer_rootpw_box.set_sensitive(enable_src);

        // Auto-generate a path if not specified
        if enable_src && inst.oscontainer_fs.text().is_empty() {
            let fs_dir = if is_root() {
                "/var/lib/libvirt/filesystems".to_string()
            } else {
                glib::home_dir()
                    .join(".local/share/libvirt/filesystems")
                    .to_string_lossy()
                    .into_owned()
            };
            let default_name = self.generate_name("generic", true);
            inst.oscontainer_fs
                .set_text(&format!("{fs_dir}/{default_name}"));
        }
    }

    /// `Guest.generate_name` for this connection's guests.
    fn generate_name(&self, os_name: &str, is_container: bool) -> String {
        let i = &self.0;
        let names: Vec<String> = i
            .conn
            .borrow()
            .as_ref()
            .map(|c| c.list_vms().iter().map(|v| v.get_name()).collect())
            .unwrap_or_default();
        let arch = i
            .capsinfo
            .borrow()
            .as_ref()
            .and_then(|c| c.arch.clone())
            .unwrap_or_default();
        let host_arch = i
            .caps
            .borrow()
            .as_ref()
            .and_then(|c| c.host.cpu.arch.clone());
        virtinst_core::guest::generate_name(
            os_name,
            is_container,
            &arch,
            host_arch.as_deref(),
            |n| names.iter().any(|x| x == n),
        )
    }

    // OS detection

    /// `_start_detect_os_if_needed`: kicks off OS detection when there's
    /// media to detect and it hasn't been detected yet. `true` when it
    /// started.
    fn start_detect_os_if_needed(&self, forward_after_finish: bool) -> bool {
        let i = &self.0;
        let (cdrom, location) = self.get_config_detectable_media();
        if i.detect_os_in_progress.get()
            || i.cur_page.get() != PAGE_INSTALL
            || (cdrom.is_none() && location.is_none())
            || !i.inst.detect_os.is_active()
            || i.os_already_detected_for_media.get()
        {
            return false;
        }
        self.do_start_detect_os(cdrom, location, forward_after_finish);
        true
    }

    fn do_start_detect_os(
        &self,
        cdrom: Option<String>,
        location: Option<String>,
        forward_after_finish: bool,
    ) {
        let i = &self.0;
        eprintln!(
            "Starting OS detection thread for cdrom={} location={}",
            cdrom.as_deref().unwrap_or("None"),
            location.as_deref().unwrap_or("None")
        );
        i.detect_os_in_progress.set(true);
        i.forward.set_sensitive(false);
        i.inst.os_list.search_entry.set_text("Detecting...");
        i.inst.detect_os_spinner.start();
        let seq = i.detect_seq.get() + 1;
        i.detect_seq.set(seq);

        let is_remote = i.conn.borrow().as_ref().is_some_and(|c| c.is_remote());
        let uri = i
            .conn
            .borrow()
            .as_ref()
            .map(|c| c.get_uri())
            .unwrap_or_default();
        // `self._gdata.build_guest()`: the guest facts the tree lookup reads.
        let capsinfo = i.capsinfo.borrow().clone();
        let guest = virtinst_core::install::installertreemedia::GuestInfo {
            arch: capsinfo
                .as_ref()
                .and_then(|c| c.arch.clone())
                .unwrap_or_default(),
            os_type: capsinfo
                .as_ref()
                .and_then(|c| c.os_type.clone())
                .unwrap_or_default(),
            os_name: "generic".into(),
        };
        let this = self.clone();
        glib::spawn_future_local(async move {
            let detect = gio::spawn_blocking(move || {
                // `installer.detect_distro`
                let db = virtinst_core::osinfo::OsDb::load_default().ok()?;
                let os = if let Some(location) = location {
                    use virtinst_core::install::installertreemedia::TreeMedia;
                    let uri = virtinst_core::connection::Uri::parse(&uri);
                    let detected =
                        TreeMedia::new(&uri, Some(location), None, None, None, None, None)
                            .and_then(|mut tm| tm.detect_distro(&uri, &guest));
                    match detected {
                        Ok(distro) => return distro,
                        Err(e) => {
                            eprintln!("Error detecting distro: {e}");
                            None
                        }
                    }
                } else if let Some(cdrom) = cdrom {
                    if is_remote {
                        eprintln!("Can't detect distro for cdrom remote connection.");
                        None
                    } else {
                        db.guess_os_by_iso(&cdrom)
                    }
                } else {
                    None
                };
                os.map(|o| o.short_id())
            });
            // A hard time limit, in case detection hangs (like a slow
            // URL lookup)
            let distro = glib::future_with_timeout(
                std::time::Duration::from_secs(DETECT_TIMEOUT.into()),
                detect,
            )
            .await
            .ok()
            .and_then(|r| r.ok())
            .flatten();
            if this.0.detect_seq.get() == seq {
                this.report_detect_os(distro, forward_after_finish);
            }
        });
    }

    /// `_report_detect_os_progress`, once detection finished or timed
    /// out.
    fn report_detect_os(&self, distro: Option<String>, forward_after_finish: bool) {
        let i = &self.0;
        i.inst.detect_os_spinner.stop();
        eprintln!(
            "Finished UI OS detection. distro={}",
            distro.as_deref().unwrap_or("None")
        );
        i.forward.set_sensitive(true);
        i.os_already_detected_for_media.set(true);
        i.detect_os_in_progress.set(false);

        if !i.inst.detect_os.is_active() {
            // If the user changed the OS detect checkbox in the
            // meantime, don't update the UI
            return;
        }
        let os = distro.and_then(|d| oslist::all_os().iter().find(|o| o.name == d).cloned());
        match os {
            Some(os) => i.inst.os_list.select_os(&os),
            None => {
                i.inst.os_list.reset_state();
                i.inst.os_list.search_entry.set_text("None detected");
            }
        }
        if forward_after_finish {
            let this = self.clone();
            glib::idle_add_local_once(move || this.forward_clicked());
        }
    }

    // Page validation

    /// `_validate_oscontainer_bootstrap`.
    async fn validate_oscontainer_bootstrap(
        &self,
        fs: &str,
        src_url: &str,
        user: &str,
        passwd: &str,
    ) -> bool {
        let parent = Some(&self.0.window);
        // Check if the source path was provided
        if src_url.is_empty() {
            error::val_err(parent, "Source URL is required").await;
            return false;
        }
        // Require username and password when authenticate to source
        // registry.
        if !user.is_empty() && passwd.is_empty() {
            error::val_err(
                parent,
                "Please specify password for accessing source registry",
            )
            .await;
            return false;
        }
        // Validate destination path
        let path = std::path::Path::new(fs);
        if !path.exists() {
            return true;
        }
        if !path.is_dir() {
            error::val_err(parent, &format!("Destination path is not directory: {fs}")).await;
            return false;
        }
        // `os.access(fs, os.W_OK)`
        let writable = gio::File::for_path(path)
            .query_info(
                gio::FILE_ATTRIBUTE_ACCESS_CAN_WRITE,
                gio::FileQueryInfoFlags::NONE,
                gio::Cancellable::NONE,
            )
            .is_ok_and(|info| info.boolean(gio::FILE_ATTRIBUTE_ACCESS_CAN_WRITE));
        if !writable {
            error::val_err(
                parent,
                &format!("No write permissions for directory path: {fs}"),
            )
            .await;
            return false;
        }
        if std::fs::read_dir(path).is_ok_and(|mut d| d.next().is_none()) {
            return true;
        }
        // Show Yes/No dialog if the destination is not empty
        error::yes_no(
            parent,
            "OS root directory is not empty",
            Some(
                "Creating root file system in a non-empty directory might fail due to file \
                 conflicts.\nWould you like to continue?",
            ),
        )
        .await
    }

    /// `_validate_install_page`.
    async fn validate_install_page(&self) -> bool {
        let i = &self.0;
        let parent = Some(&i.window);
        let Some(conn) = i.conn.borrow().clone() else {
            return false;
        };
        let instmethod = self.get_config_install_page();
        let mut location = None;
        let mut extra = None;
        let mut cdrom = None;
        let mut init = None;
        let mut fs = None;
        let mut template = None;
        let osobj = i.inst.os_list.get_selected_os();

        match instmethod {
            InstallPage::Iso => {
                let media = i.inst.mediacombo.get_path(false);
                if media.is_empty() {
                    error::val_err(parent, "An install media selection is required.").await;
                    return false;
                }
                cdrom = Some(media);
            }
            InstallPage::Url => {
                let (media, opts) = self.get_config_url_info(false);
                if media.is_empty() {
                    error::val_err(parent, "An install tree is required.").await;
                    return false;
                }
                location = Some(media);
                extra = Some(opts).filter(|e| !e.is_empty());
            }
            InstallPage::Import => {
                let import_path = i.inst.import_entry.text().to_string();
                if import_path.is_empty() {
                    error::val_err(parent, "A storage path to import is required.").await;
                    return false;
                }
                let exists = match conn.handle() {
                    Some(handle) => {
                        let is_remote = conn.is_remote();
                        let path = import_path.clone();
                        gio::spawn_blocking(move || {
                            virtinst_core::storage::path_definitely_exists(
                                handle.connect(),
                                &path,
                                is_remote,
                            )
                        })
                        .await
                        .unwrap_or(false)
                    }
                    None => false,
                };
                if !exists {
                    error::val_err(parent, "The import path must point to an existing storage.")
                        .await;
                    return false;
                }
            }
            InstallPage::ContainerApp => {
                let text = i.inst.app_entry.text().to_string();
                if text.is_empty() {
                    error::val_err(parent, "An application path is required.").await;
                    return false;
                }
                init = Some(text);
            }
            InstallPage::ContainerOs => {
                let text = i.inst.oscontainer_fs.text().to_string();
                if text.is_empty() {
                    error::val_err(parent, "An OS directory path is required.").await;
                    return false;
                }
                if i.inst.oscontainer_bootstrap.is_active() {
                    let src_url = self.get_config_oscontainer_source_url(false);
                    let user = i.inst.oscontainer_source_user.text().trim().to_string();
                    let passwd = i.inst.oscontainer_source_passwd.text().to_string();
                    if !self
                        .validate_oscontainer_bootstrap(&text, &src_url, &user, &passwd)
                        .await
                    {
                        return false;
                    }
                }
                fs = Some(text);
            }
            InstallPage::VzTemplate => {
                let text = i.inst.container_template.text().to_string();
                if text.is_empty() {
                    error::val_err(parent, "A template name is required.").await;
                    return false;
                }
                template = Some(text);
            }
            InstallPage::Manual => {}
        }

        if !self.is_container_install() && osobj.is_none() {
            let msg = format!("You must select an OS.\n\n{}", OsList::EOL_TEXT);
            error::val_err(parent, &msg).await;
            return false;
        }

        let os_name = osobj
            .as_ref()
            .map_or_else(|| "generic".to_string(), |o| o.name.clone());
        let is_container = i
            .capsinfo
            .borrow()
            .as_ref()
            .is_some_and(|c| c.os_type.as_deref() == Some("exe"));
        let name = self.generate_name(&os_name, is_container);
        let is_import = instmethod == InstallPage::Import;
        if let Err(e) = virtinst_core::storage::validate_generic_name("Guest", &name) {
            error::val_err(parent, &format!("Error setting default name.\n\n{e}")).await;
            return false;
        }
        // `build_installer`: the tree media checks its location.
        if let Some(loc) = &location {
            let uri = virtinst_core::connection::Uri::parse(&conn.get_uri());
            use virtinst_core::install::installertreemedia::TreeMedia;
            if let Err(e) = TreeMedia::new(&uri, Some(loc.clone()), None, None, None, None, None) {
                error::val_err_text2(parent, "Error setting installer parameters.", Some(&e)).await;
                return false;
            }
        }

        {
            let mut gdata = i.gdata.borrow_mut();
            gdata.init = init;
            gdata.filesystem = fs
                .map(|f| (None, f))
                .or(template.map(|t| (Some("template".to_string()), t)));
            gdata.location = location;
            gdata.cdrom = cdrom;
            gdata.extra_args = extra;
            gdata.livecd = false;
            gdata.osinfo = osobj.as_ref().map(|o| o.name.clone());
            gdata.osinfo_id = osobj.as_ref().and_then(|o| o.full_id.clone());
            gdata.name = Some(name);
        }

        // Kind of wonky, run storage validation now, which will assign
        // the import path. Import installer skips the storage page.
        if is_import && !self.validate_storage_page().await {
            return false;
        }

        // Change the default values suggested to the user.
        let arch = i
            .capsinfo
            .borrow()
            .as_ref()
            .and_then(|c| c.arch.clone())
            .unwrap_or_default();
        let res = osobj
            .as_ref()
            .and_then(|o| o.full_id.as_deref())
            .and_then(oslist::lookup_osinfo)
            .map(|o| o.get_recommended_resources())
            .unwrap_or_default();
        let ram = res.get_recommended_ram(&arch);
        let n_cpus = res.get_recommended_ncpus(&arch);
        eprintln!(
            "Recommended resources for os={os_name}: ram={ram:?} ncpus={n_cpus:?} storage={:?}",
            res.get_recommended_storage(&arch)
        );
        let ram_size = ram.map_or(DEFAULT_MEM, |r| (r / (1024 * 1024)) as f64);
        i.memw.mem.set_value(ram_size);
        i.memw.cpus.set_value(n_cpus.unwrap_or(1) as f64);
        i.fin
            .name
            .set_text(i.gdata.borrow().name.as_deref().unwrap_or_default());
        if let Some(storage) = res.get_recommended_storage(&arch) {
            i.addstorage
                .set_size((storage / (1024 * 1024 * 1024)) as f64);
        }

        // Validation passed, store the install path (if there is one)
        // in gsettings
        self.get_config_oscontainer_source_url(true);
        if instmethod == InstallPage::Iso {
            i.inst.mediacombo.get_path(true);
        }
        self.get_config_url_info(true);
        true
    }
}

/// `(None, None)` for empty strings, as Python's falsy checks do.
trait MapEmpty {
    fn map_empty(self) -> Self;
}

impl MapEmpty for (Option<String>, Option<String>) {
    fn map_empty(self) -> Self {
        (
            self.0.filter(|s| !s.is_empty()),
            self.1.filter(|s| !s.is_empty()),
        )
    }
}
