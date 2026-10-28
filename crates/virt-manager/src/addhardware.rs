//! `addhardware.py` (`vmmAddHardware`): the Add New Virtual Hardware
//! dialog. A list of device types on the left, the picked type's page
//! under the blue header on the right; Finish builds the device,
//! hotplugs it into a running VM and adds it to the persistent config.
//!
//! **Not ported yet**: the XML editor tab (`xmleditor.py`), the
//! storage browser behind "Manage..."
//! and `check_path_search`'s permission fixups.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use virtinst_core::devices::build::{self, GuestInfo, NewChar, NewDisk, NewInterface};
use virtinst_core::devices::DeviceId;
use virtinst_core::nodedev::NodeDevice;
use virtinst_xml::{Document, Element};

use crate::addstorage::{plan_disk, AddStorage, DiskRequest};
use crate::devpages::{input_pretty_name, sound_pretty_model, watchdog_pretty_action};
use crate::devpages::{EntryCombo, Grid, GridRow, SimpleCombo};
use crate::domain::VmmDomain;
use crate::error;
use crate::fsdetails::FsDetails;
use crate::gfxdetails::GraphicsDetails;
use crate::netlist::NetList;
use crate::storagebrowse::{self, BrowseReason, StorageBrowser};
use crate::tpmdetails::TpmDetails;
use crate::vsockdetails::VsockDetails;

/// `PAGE_*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Disk,
    Controller,
    Network,
    Input,
    Graphics,
    Sound,
    Hostdev,
    Char,
    Video,
    Watchdog,
    Filesystem,
    Smartcard,
    Usbredir,
    Tpm,
    Rng,
    Panic,
    Vsock,
}

impl Page {
    fn name(self) -> &'static str {
        match self {
            Page::Disk => "disk",
            Page::Controller => "controller",
            Page::Network => "network",
            Page::Input => "input",
            Page::Graphics => "graphics",
            Page::Sound => "sound",
            Page::Hostdev => "hostdev",
            Page::Char => "char",
            Page::Video => "video",
            Page::Watchdog => "watchdog",
            Page::Filesystem => "filesystem",
            Page::Smartcard => "smartcard",
            Page::Usbredir => "usbredir",
            Page::Tpm => "tpm",
            Page::Rng => "rng",
            Page::Panic => "panic",
            Page::Vsock => "vsock",
        }
    }
}

/// One `hw-list` row: name, icon name, page, is sensitive, error text,
/// device type (serial/parallel/...).
struct HwOption {
    name: &'static str,
    icon: &'static str,
    page: Page,
    sensitive: bool,
    errortxt: Option<String>,
    devtype: Option<&'static str>,
}

/// Makes a device's element in a document.
type BuildFn = Rc<dyn Fn(&mut Document) -> Result<Element, String>>;

/// What `_validate_device` checks beyond the XML.
enum Validate {
    None,
    /// `self._netlist.validate_device(dev)`.
    Network(NewInterface),
    /// `_validate_hostdev_collision`: the node device passed through.
    Hostdev(Box<NodeDevice>),
    /// `vmmAddStorage.validate_device`: the disk's path, device type,
    /// shareable and readonly.
    Disk(Option<String>, String, bool, bool),
}

/// A device ready to add: `build` makes its element. `usb_replace` is
/// the USB controller whose model changes instead
/// (`_remove_usb_controller`).
struct NewDev {
    build: BuildFn,
    usb_replace: Option<(DeviceId, Option<String>)>,
    validate: Validate,
    /// A disk's volume to create first (`build_storage`).
    vol_install: Option<virtinst_core::storage::VolInstall>,
    /// `vmm_controller`: the virtio-scsi controller a disk brings.
    controller: Option<BuildFn>,
}

impl NewDev {
    fn new(build: impl Fn(&mut Document) -> Result<Element, String> + 'static) -> NewDev {
        NewDev {
            build: Rc::new(build),
            usb_replace: None,
            validate: Validate::None,
            vol_install: None,
            controller: None,
        }
    }

    /// `get_xml()`.
    fn xml(&self) -> Result<String, String> {
        let mut doc = Document::new();
        let el = (self.build)(&mut doc)?;
        virtinst_xml::element_to_string(doc, el).map_err(|e| e.to_string())
    }
}

/// `char_recommended_types`.
fn char_recommended_types(tag: &str) -> Vec<&'static str> {
    if tag == "console" {
        return vec!["pty"];
    }
    let mut ret = vec!["pty", "file", "unix"];
    if tag == "channel" {
        ret.splice(0..0, ["spicevmc", "spiceport", "qemu-vdagent"]);
    }
    ret
}

/// `controller_recommended_types` with `controller_pretty_type`.
const CONTROLLER_TYPES: [(&str, &str); 4] = [
    ("scsi", "SCSI"),
    ("usb", "USB"),
    ("virtio-serial", "VirtIO Serial"),
    ("ccid", "CCID"),
];

/// `_build_input_combo`'s devices, keyed `type:bus`.
const INPUT_DEVICES: [(&str, &str); 5] = [
    ("tablet", "usb"),
    ("mouse", "usb"),
    ("keyboard", "usb"),
    ("keyboard", "virtio"),
    ("tablet", "virtio"),
];

/// A `host-device` row: the node device (`None` for "No Devices
/// Available"), label, sensitive, tooltip.
type HostdevRow = (Option<NodeDevice>, String, bool, Option<String>);

/// `_populate_hostdev_model`'s rows for `devtype`, sorted by label as
/// the model is. `devs` is every node device with its `is_active()`.
fn hostdev_rows(devtype: &str, devs: &[(NodeDevice, bool)]) -> Vec<HostdevRow> {
    let all: Vec<NodeDevice> = devs.iter().map(|(d, _)| d.clone()).collect();
    let pretty = |d: &NodeDevice| virtinst_core::nodedev::pretty_name(d, &all);
    let netdevs: Vec<&NodeDevice> = all
        .iter()
        .filter(|d| d.device_type.as_deref() == Some("net"))
        .collect();
    let mut rows: Vec<HostdevRow> = devs
        .iter()
        .filter(|(d, _)| d.device_type.as_deref() == Some(devtype))
        .filter(|(d, _)| !d.is_usb_linux_root_hub() && !d.is_pci_bridge())
        .map(|(dev, active)| {
            let mut prettyname = pretty(dev);
            if devtype == "pci" {
                for subdev in &netdevs {
                    if subdev.parent.as_ref() == Some(&dev.name) {
                        prettyname += &format!(" ({})", pretty(subdev));
                    }
                }
            }
            // parent device names are appended with mdev names in
            // libvirt 7.8.0
            if devtype == "mdev" && prettyname.chars().count() <= 41 {
                for parentdev in &all {
                    if dev.parent.as_ref() == Some(&parentdev.name) {
                        prettyname = format!("{} {prettyname}", pretty(parentdev));
                    }
                }
            }
            let tooltip = (!active).then(|| {
                format!(
                    "{prettyname} is not active in the host system.\n\
                     Please start the mdev in the host system before adding it to the guest."
                )
            });
            (Some(dev.clone()), prettyname, *active, tooltip)
        })
        .collect();
    rows.sort_by(|a, b| a.1.cmp(&b.1));
    if rows.is_empty() {
        rows.push((None, "No Devices Available".into(), false, None));
    }
    rows
}

/// Python's `str()` of a list of names: `['a', 'b']`.
pub(crate) fn py_list(names: &[String]) -> String {
    let quoted: Vec<String> = names.iter().map(|n| format!("'{n}'")).collect();
    format!("[{}]", quoted.join(", "))
}

/// `_build_disk_device_combo`'s rows: device, icon, label.
const DISK_DEVICES: [(&str, &str, &str); 4] = [
    ("disk", "drive-harddisk", "Disk device"),
    ("cdrom", "media-optical", "CDROM device"),
    ("floppy", "media-floppy", "Floppy device"),
    ("lun", "drive-harddisk", "LUN Passthrough"),
];

/// `disk_old_recommended_buses`.
fn disk_old_recommended_buses(guest: &GuestInfo, is_test: bool, is_xen: bool) -> Vec<&'static str> {
    let mut ret = vec![];
    if guest.is_hvm() || is_test {
        if !guest.is_q35() {
            ret.push("ide");
        }
        ret.extend(["sata", "fdc", "scsi", "usb"]);
        if matches!(guest.domain_type.as_deref(), Some("qemu" | "kvm" | "test")) {
            ret.extend(["sd", "virtio"]);
        }
    }
    if is_xen || is_test {
        ret.push("xen");
    }
    ret
}

/// `disk_recommended_buses`: domcaps' buses when it lists them, that
/// fit the device type.
fn disk_recommended_buses(buses: &[String], devtype: &str) -> Vec<String> {
    let fits: &[&str] = match devtype {
        "disk" => &["ide", "sata", "scsi", "sd", "usb", "virtio", "xen"],
        "floppy" => &["fdc"],
        "cdrom" => &["ide", "sata", "scsi", "usb"],
        "lun" => &["scsi"],
        _ => &[],
    };
    buses
        .iter()
        .filter(|b| fits.contains(&b.as_str()))
        .cloned()
        .collect()
}

/// A drop-down of icon and label rows, like the GtkComboBox with a
/// pixbuf renderer upstream uses for `storage-devtype`.
fn icon_dropdown(rows: &[(&str, &str)]) -> gtk::DropDown {
    let labels: Vec<&str> = rows.iter().map(|(_, l)| *l).collect();
    let icons: Vec<String> = rows.iter().map(|(i, _)| i.to_string()).collect();
    let dropdown = gtk::DropDown::from_strings(&labels);
    dropdown.set_halign(gtk::Align::Start);
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        hbox.append(&gtk::Image::new());
        hbox.append(&gtk::Label::new(None));
        item.set_child(Some(&hbox));
    });
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let Some(hbox) = item.child().and_downcast::<gtk::Box>() else {
            return;
        };
        let text = item
            .item()
            .and_downcast::<gtk::StringObject>()
            .map(|o| o.string().to_string())
            .unwrap_or_default();
        if let Some(image) = hbox.first_child().and_downcast::<gtk::Image>() {
            image.set_icon_name(icons.get(item.position() as usize).map(String::as_str));
        }
        if let Some(label) = hbox.last_child().and_downcast::<gtk::Label>() {
            label.set_text(&text);
        }
    });
    dropdown.set_factory(Some(&factory));
    dropdown
}

fn page_box(child: &impl IsA<gtk::Widget>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
    b.append(child);
    b
}

#[derive(Clone)]
pub struct AddHardware(Rc<Inner>);

struct Inner {
    vm: VmmDomain,
    window: gtk::Window,
    storagebrowser: RefCell<Option<StorageBrowser>>,
    hw_list: gtk::ListBox,
    options: Vec<HwOption>,
    top_pages: gtk::Stack,
    page_title: gtk::Label,
    create_pages: gtk::Stack,
    error_label: gtk::Label,
    finish: gtk::Button,

    controller_type: SimpleCombo,
    controller_model: SimpleCombo,
    controller_model_row: GridRow,
    controller_tooltip: gtk::Image,
    addstorage: Rc<AddStorage>,
    storage_devtype: gtk::DropDown,
    storage_devtypes: Vec<&'static str>,
    storage_bustype: SimpleCombo,
    netlist: Rc<NetList>,
    mac_address: gtk::CheckButton,
    create_mac_address: gtk::Entry,
    net_model: SimpleCombo,
    input_type: SimpleCombo,
    gfx: Rc<GraphicsDetails>,
    sound_model: SimpleCombo,
    host_device: gtk::ListBox,
    hostdev_rows: RefCell<Vec<HostdevRow>>,
    /// Every node device, for `set_from_nodedev`.
    nodedevs: RefCell<Vec<NodeDevice>>,
    hostdev_usb_startup_policy: SimpleCombo,
    hostdev_usb_startup_policy_row: gtk::Box,
    /// Bumped per `_populate_hostdev_model`, so a slow fetch can't win
    /// over a newer pick.
    hostdev_generation: Cell<u32>,
    char_target_name: Rc<EntryCombo>,
    char_target_name_row: GridRow,
    char_device_type: SimpleCombo,
    char_target_type: SimpleCombo,
    char_target_type_row: GridRow,
    char_auto_socket: gtk::CheckButton,
    char_auto_socket_row: GridRow,
    char_path: gtk::Entry,
    char_path_row: GridRow,
    char_channel: gtk::Entry,
    char_channel_row: GridRow,
    char_clipboard: gtk::CheckButton,
    char_clipboard_row: GridRow,
    video_model: SimpleCombo,
    watchdog_model: SimpleCombo,
    watchdog_action: SimpleCombo,
    fs: Rc<FsDetails>,
    smartcard_mode: SimpleCombo,
    usbredir_type: SimpleCombo,
    tpm: Rc<TpmDetails>,
    rng_device: gtk::Entry,
    panic_model: SimpleCombo,
    vsock: Rc<VsockDetails>,

    /// `_remove_usb_controller`: the guest's `model='none'` USB
    /// controller, which a new USB controller replaces.
    remove_usb_controller: RefCell<Option<DeviceId>>,
    /// Set while a programmatic change runs, so the char page's
    /// listeners don't fire on it.
    quiet: Cell<bool>,
}

impl AddHardware {
    pub fn new(vm: &VmmDomain) -> AddHardware {
        let conn = vm.conn();
        let is_qemu = conn.as_ref().is_some_and(|c| c.is_qemu());
        let is_xen = conn.as_ref().is_some_and(|c| c.is_xen());
        let is_test = conn.as_ref().is_some_and(|c| c.is_test());
        let guest = guest_info(vm);

        let window = gtk::Window::builder()
            .title("Add New Virtual Hardware")
            .hide_on_close(true)
            .build();
        window.set_size_request(500, 400);

        // hw-list
        let hw_list = gtk::ListBox::new();
        hw_list.set_selection_mode(gtk::SelectionMode::Browse);
        hw_list.update_property(&[gtk::accessible::Property::Label("Hardware")]);
        let hw_scroll = gtk::ScrolledWindow::builder()
            .child(&hw_list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .has_frame(true)
            .min_content_width(165)
            .build();

        // Controller
        let controller_type = SimpleCombo::new(
            CONTROLLER_TYPES
                .iter()
                .map(|(v, l)| (Some(v.to_string()), l.to_string()))
                .collect(),
            str::to_string,
        );
        controller_type.set(Some("scsi"));
        let controller_tooltip = gtk::Image::from_icon_name("dialog-information");
        controller_tooltip.set_visible(false);
        let controller_model = SimpleCombo::unsorted(vec![], str::to_string);
        let ctype_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        ctype_box.append(&controller_type.widget);
        ctype_box.append(&controller_tooltip);
        let mut g = Grid::new();
        let row = g.row("_Type:", &ctype_box);
        row.set_mnemonic(&controller_type.widget);
        let controller_model_row = g.row("_Model:", &controller_model.widget);
        let controller_page = page_box(&g.into_grid());

        // Storage
        let addstorage = AddStorage::new(conn.clone());
        // LUN passthrough is a qemu thing.
        let disk_devices: Vec<_> = DISK_DEVICES
            .iter()
            .filter(|(d, _, _)| *d != "lun" || is_qemu || is_test)
            .collect();
        let storage_devtype = icon_dropdown(
            &disk_devices
                .iter()
                .map(|(_, icon, label)| (*icon, *label))
                .collect::<Vec<_>>(),
        );
        let storage_devtypes: Vec<&'static str> = disk_devices.iter().map(|(d, _, _)| *d).collect();
        let storage_bustype = SimpleCombo::unsorted(vec![], crate::details::disk_pretty_bus);
        let mut g = Grid::new();
        g.row("_Device type:", &storage_devtype);
        g.row("_Bus type:", &storage_bustype.widget);
        let disk_page = gtk::Box::new(gtk::Orientation::Vertical, 12);
        disk_page.append(&addstorage.top_box);
        disk_page.append(&g.into_grid());
        disk_page.append(&addstorage.advanced_top_box);

        // Network
        let netlist = NetList::new(conn.clone());
        let mac_address = gtk::CheckButton::new();
        mac_address.update_property(&[gtk::accessible::Property::Label("mac-address-enable")]);
        let create_mac_address = gtk::Entry::new();
        create_mac_address.set_max_length(17);
        create_mac_address.set_text("aa:bb:cc:dd:ee:ff");
        create_mac_address
            .update_property(&[gtk::accessible::Property::Label("MAC Address Field")]);
        let net_model = SimpleCombo::unsorted(vec![], str::to_string);
        let net_grid = gtk::Grid::builder()
            .row_spacing(9)
            .column_spacing(6)
            .build();
        net_grid.attach(&netlist.top_label, 0, 0, 1, 1);
        net_grid.attach(&netlist.top_box, 1, 0, 1, 1);
        let mac_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        mac_box.append(&mac_address);
        mac_box.append(&create_mac_address);
        let mac_label = gtk::Label::with_mnemonic("_MAC address:");
        mac_label.set_halign(gtk::Align::End);
        mac_label.set_mnemonic_widget(Some(&create_mac_address));
        net_grid.attach(&mac_label, 0, 1, 1, 1);
        net_grid.attach(&mac_box, 1, 1, 1, 1);
        let model_label = gtk::Label::with_mnemonic("Device mode_l:");
        model_label.set_halign(gtk::Align::End);
        model_label.set_mnemonic_widget(Some(&net_model.widget));
        net_grid.attach(&model_label, 0, 2, 1, 1);
        net_grid.attach(&net_model.widget, 1, 2, 1, 1);
        let network_page = page_box(&net_grid);

        // Host device
        let host_device = gtk::ListBox::new();
        host_device.set_selection_mode(gtk::SelectionMode::Browse);
        host_device.update_property(&[gtk::accessible::Property::Label("host-device")]);
        let host_scroll = gtk::ScrolledWindow::builder()
            .child(&host_device)
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .has_frame(true)
            .width_request(500)
            .vexpand(true)
            .hexpand(true)
            .build();
        let mut values = vec![(None, "Hypervisor default".to_string())];
        values.extend(
            virtinst_core::devices::hostdev::STARTUP_POLICIES
                .iter()
                .map(|m| (Some(m.to_string()), m.to_string())),
        );
        let hostdev_usb_startup_policy = SimpleCombo::unsorted(values, str::to_string);
        hostdev_usb_startup_policy.set(None);
        let hostdev_usb_startup_policy_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let policy_label = gtk::Label::new(Some("Startup Policy:"));
        hostdev_usb_startup_policy_row.append(&policy_label);
        hostdev_usb_startup_policy_row.append(&hostdev_usb_startup_policy.widget);
        let host_grid = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(6)
            .build();
        let host_label = gtk::Label::with_mnemonic("Host _Device:");
        host_label.set_halign(gtk::Align::Start);
        host_label.set_mnemonic_widget(Some(&host_device));
        host_grid.attach(&host_label, 0, 0, 1, 1);
        host_grid.attach(&host_scroll, 0, 1, 1, 1);
        host_grid.attach(&hostdev_usb_startup_policy_row, 0, 2, 1, 1);
        let hostdev_page = page_box(&host_grid);
        hostdev_page.set_vexpand(true);

        // Input
        let input_type = SimpleCombo::new(
            INPUT_DEVICES
                .iter()
                .map(|(t, b)| (Some(format!("{t}:{b}")), input_pretty_name(t, b)))
                .collect(),
            str::to_string,
        );
        input_type.widget.set_selected(0);
        let mut g = Grid::new();
        g.row("_Type:", &input_type.widget);
        let input_page = page_box(&g.into_grid());

        // Graphics
        let gfx = GraphicsDetails::new(conn.clone());
        let graphics_page = page_box(&gfx.top_box);

        // Sound
        let sound_model = SimpleCombo::new(
            virtinst_core::devices::sound::RECOMMENDED_MODELS
                .iter()
                .map(|m| (Some(m.to_string()), sound_pretty_model(m)))
                .collect(),
            sound_pretty_model,
        );
        sound_model.set(Some(guest.default_sound_model()));
        let mut g = Grid::new();
        g.row("_Model:", &sound_model.widget);
        let sound_page = page_box(&g.into_grid());

        // Char
        let char_target_name = EntryCombo::new(
            build::CHANNEL_NAMES
                .iter()
                .map(|n| (Some(n.to_string()), n.to_string()))
                .collect(),
        );
        char_target_name.set_active(0);
        let char_device_type = SimpleCombo::new(vec![], str::to_string);
        let char_target_type = SimpleCombo::new(
            if is_qemu {
                vec![(Some("virtio".into()), "VirtIO".into())]
            } else {
                vec![(None, "Hypervisor default".into())]
            },
            str::to_string,
        );
        char_target_type.widget.set_selected(0);
        let char_auto_socket = gtk::CheckButton::new();
        let char_path = gtk::Entry::new();
        char_path.set_hexpand(true);
        let char_channel = gtk::Entry::new();
        let char_clipboard = gtk::CheckButton::new();
        let mut g = Grid::new();
        let char_target_name_row = g.row("_Name:", &char_target_name.top);
        char_target_name_row.set_mnemonic(&char_target_name.entry);
        g.row("Device _Type:", &char_device_type.widget);
        let char_target_type_row = g.row("T_ype:", &char_target_type.widget);
        let char_auto_socket_row = g.row("_Auto socket:", &char_auto_socket);
        let char_path_row = g.row("_Path:", &char_path);
        let char_channel_row = g.row("_Channel:", &char_channel);
        let char_clipboard_row = g.row("_Share Clipboard:", &char_clipboard);
        let char_page = page_box(&g.into_grid());

        // Video
        let mut values: Vec<_> =
            virtinst_core::devices::video::recommended_models(is_xen, is_qemu || is_test)
                .iter()
                .map(|m| (Some(m.to_string()), crate::details::video_pretty_model(m)))
                .collect();
        if values.is_empty() {
            values.push((None, "Hypervisor default".into()));
        }
        let video_model = SimpleCombo::new(values, crate::details::video_pretty_model);
        video_model.set(guest.default_video_model(None, is_qemu));
        let mut g = Grid::new();
        g.row("_Model:", &video_model.widget);
        let video_page = page_box(&g.into_grid());

        // Watchdog
        use virtinst_core::devices::watchdog::{ACTIONS, MODELS};
        let watchdog_model = SimpleCombo::new(
            MODELS
                .iter()
                .map(|m| (Some(m.to_string()), m.to_uppercase()))
                .collect(),
            |m| m.to_uppercase(),
        );
        watchdog_model.set(Some("i6300esb"));
        let watchdog_action = SimpleCombo::new(
            ACTIONS
                .iter()
                .map(|a| (Some(a.to_string()), watchdog_pretty_action(a)))
                .collect(),
            watchdog_pretty_action,
        );
        watchdog_action.set(Some("reset"));
        let mut g = Grid::new();
        g.row("_Model:", &watchdog_model.widget);
        g.row("Ac_tion:", &watchdog_action.widget);
        let watchdog_page = page_box(&g.into_grid());

        // Filesystem
        let fs = FsDetails::new(conn.as_ref());
        let fs_page = page_box(&fs.top_box);

        // Smartcard
        let smartcard_mode = SimpleCombo::new(
            vec![
                (Some("passthrough".into()), "Passthrough".into()),
                (Some("host".into()), "Host".into()),
            ],
            str::to_string,
        );
        smartcard_mode.widget.set_selected(0);
        let mut g = Grid::new();
        g.row("_Mode:", &smartcard_mode.widget);
        let smartcard_page = page_box(&g.into_grid());

        // USB redirection
        let usbredir_type = SimpleCombo::new(
            vec![(Some("spicevmc".into()), "Spice channel".into())],
            str::to_string,
        );
        usbredir_type.widget.set_selected(0);
        let mut g = Grid::new();
        g.row("_Type:", &usbredir_type.widget);
        let usbredir_page = page_box(&g.into_grid());

        // TPM
        let tpm = TpmDetails::new();
        let tpm_page = page_box(&tpm.top_box);

        // RNG
        let rng_device = gtk::Entry::new();
        rng_device.set_hexpand(true);
        let mut g = Grid::new();
        g.row("Host _Device:", &rng_device);
        let rng_page = page_box(&g.into_grid());

        // Panic
        let panic_model =
            SimpleCombo::new(vec![(None, "Hypervisor default".into())], str::to_string);
        panic_model.set(None);
        let mut g = Grid::new();
        g.row("_Model:", &panic_model.widget);
        let panic_page = page_box(&g.into_grid());

        // VSOCK
        let vsock = VsockDetails::new();
        let vsock_page = page_box(&vsock.top_box);

        let create_pages = gtk::Stack::new();
        create_pages.set_vhomogeneous(false);
        create_pages.set_hhomogeneous(false);
        for (page, widget) in [
            (Page::Disk, &disk_page),
            (Page::Controller, &controller_page),
            (Page::Network, &network_page),
            (Page::Input, &input_page),
            (Page::Graphics, &graphics_page),
            (Page::Sound, &sound_page),
            (Page::Hostdev, &hostdev_page),
            (Page::Char, &char_page),
            (Page::Video, &video_page),
            (Page::Watchdog, &watchdog_page),
            (Page::Filesystem, &fs_page),
            (Page::Smartcard, &smartcard_page),
            (Page::Usbredir, &usbredir_page),
            (Page::Tpm, &tpm_page),
            (Page::Rng, &rng_page),
            (Page::Panic, &panic_page),
            (Page::Vsock, &vsock_page),
        ] {
            create_pages.add_named(widget, Some(page.name()));
        }

        // The blue header over the page.
        let page_title = gtk::Label::new(Some("Page title"));
        page_title.set_halign(gtk::Align::Start);
        page_title.set_valign(gtk::Align::Start);
        page_title.set_margin_start(6);
        page_title.set_margin_end(6);
        page_title.set_margin_top(6);
        page_title.set_margin_bottom(6);
        page_title.add_css_class("vmm-header-text");
        let page_title_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        page_title_box.add_css_class("vmm-header");
        page_title_box.append(&page_title);
        let create_vbox = gtk::Box::new(gtk::Orientation::Vertical, 12);
        create_vbox.append(&page_title_box);
        create_vbox.append(&create_pages);

        let error_label = gtk::Label::new(Some("label"));
        error_label.set_wrap(true);
        let top_pages = gtk::Stack::new();
        top_pages.set_hexpand(true);
        top_pages.add_named(&create_vbox, Some("info"));
        top_pages.add_named(&error_label, Some("error"));

        let mainhbox = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        mainhbox.set_vexpand(true);
        mainhbox.append(&hw_scroll);
        mainhbox.append(&top_pages);

        let cancel = gtk::Button::with_mnemonic("_Cancel");
        let finish = gtk::Button::with_mnemonic("_Finish");
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.set_halign(gtk::Align::End);
        buttons.append(&cancel);
        buttons.append(&finish);

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 6);
        vbox.set_margin_start(12);
        vbox.set_margin_end(12);
        vbox.set_margin_top(12);
        vbox.set_margin_bottom(12);
        vbox.append(&mainhbox);
        vbox.append(&buttons);
        window.set_child(Some(&vbox));

        let options = hw_options(&guest);
        for opt in &options {
            hw_list.append(&hw_row(opt));
        }

        let this = AddHardware(Rc::new(Inner {
            vm: vm.clone(),
            window,
            storagebrowser: RefCell::default(),
            hw_list,
            options,
            top_pages,
            page_title,
            create_pages,
            error_label,
            finish: finish.clone(),
            controller_type,
            controller_model,
            controller_model_row,
            controller_tooltip,
            addstorage,
            storage_devtype,
            storage_devtypes,
            storage_bustype,
            netlist,
            mac_address,
            create_mac_address,
            net_model,
            input_type,
            gfx,
            sound_model,
            host_device,
            hostdev_rows: RefCell::default(),
            nodedevs: RefCell::default(),
            hostdev_usb_startup_policy,
            hostdev_usb_startup_policy_row,
            hostdev_generation: Cell::new(0),
            char_target_name,
            char_target_name_row,
            char_device_type,
            char_target_type,
            char_target_type_row,
            char_auto_socket,
            char_auto_socket_row,
            char_path,
            char_path_row,
            char_channel,
            char_channel_row,
            char_clipboard,
            char_clipboard_row,
            video_model,
            watchdog_model,
            watchdog_action,
            fs,
            smartcard_mode,
            usbredir_type,
            tpm,
            rng_device,
            panic_model,
            vsock,
            remove_usb_controller: RefCell::default(),
            quiet: Cell::new(false),
        }));

        let weak = Rc::downgrade(&this.0);
        let with = move |f: fn(&AddHardware)| {
            let weak = weak.clone();
            move || {
                if let Some(inner) = weak.upgrade() {
                    f(&AddHardware(inner));
                }
            }
        };
        let i = &this.0;
        let cb = with(AddHardware::close);
        cancel.connect_clicked(move |_| cb());
        let cb = with(|this| {
            let this = this.clone();
            glib::spawn_future_local(async move { this.finish().await });
        });
        finish.connect_clicked(move |_| cb());
        let cb = with(AddHardware::hw_selected);
        i.hw_list.connect_row_selected(move |_, _| cb());
        let cb = with(AddHardware::change_char_device_type);
        i.char_device_type
            .widget
            .connect_selected_notify(move |_| cb());
        let cb = with(AddHardware::change_char_target_name);
        i.char_target_name.entry.connect_changed(move |_| cb());
        let cb = with(AddHardware::change_char_auto_socket);
        i.char_auto_socket.connect_toggled(move |_| cb());
        let cb = with(AddHardware::change_storage_devtype);
        i.storage_devtype.connect_selected_notify(move |_| cb());
        let cb = with(AddHardware::storage_bus_changed);
        i.storage_bustype
            .widget
            .connect_selected_notify(move |_| cb());
        let weak = Rc::downgrade(&this.0);
        i.addstorage.connect_browse_clicked(move |entry| {
            if let Some(inner) = weak.upgrade() {
                AddHardware(inner).browse_storage(entry);
            }
        });
        let cb = with(AddHardware::change_macaddr_use);
        i.mac_address.connect_toggled(move |_| cb());
        let cb = with(AddHardware::hostdev_row_selected);
        i.host_device.connect_row_selected(move |_, _| cb());
        let cb = with(AddHardware::change_controller_type);
        i.controller_type
            .widget
            .connect_selected_notify(move |_| cb());

        // bind_escape_key_close
        let keys = gtk::EventControllerKey::new();
        let cb = with(AddHardware::close);
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                cb();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        i.window.add_controller(keys);

        this.load_domcaps();
        this
    }

    /// The parts of `_set_initial_state` and `_reset_state` that read
    /// domcaps: the panic models, the video default, the TPM models
    /// and virtiofs.
    fn load_domcaps(&self) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            let i = &this.0;
            let caps = i.vm.get_domain_capabilities().await;
            let caps = caps.as_deref();
            let guest = guest_info(&i.vm);
            let is_qemu = i.vm.conn().is_some_and(|c| c.is_qemu());

            let mut values = vec![(None, "Hypervisor default".to_string())];
            values.extend(
                build::supported_panic_models(caps)
                    .into_iter()
                    .map(|m| (Some(m.clone()), m)),
            );
            i.panic_model.set_values(values);
            i.panic_model.set(None);

            i.video_model.set(guest.default_video_model(caps, is_qemu));

            if let Some(caps) = caps {
                let tpm = &caps.devices.tpm;
                i.tpm.set_models(tpm.supported.is_some().then(|| {
                    tpm.get_enum("model")
                        .map(|e| e.values.clone())
                        .unwrap_or_default()
                }));
                let container = i.vm.conn().is_some_and(|c| c.is_container_only());
                if !container {
                    i.fs.set_virtiofs_supported(caps.supports_filesystem_virtiofs());
                }
            }
            i.tpm
                .reset_state_with_model(build::default_tpm_model(caps, &guest));
        });
    }

    /// `show`.
    pub fn show(&self, parent: &gtk::Window) {
        glib::g_debug!("virt-manager", "Showing addhw");
        self.reset_state();
        self.0.window.set_transient_for(Some(parent));
        self.0.window.present();
    }

    /// `close`.
    pub fn close(&self) {
        if self.0.window.is_visible() {
            glib::g_debug!("virt-manager", "Closing addhw");
            self.0.window.set_visible(false);
        }
        if let Some(b) = self.0.storagebrowser.borrow().as_ref() {
            b.close();
        }
    }

    /// `_cleanup`.
    pub fn cleanup(&self) {
        self.0.window.destroy();
    }

    /// `_reset_state`.
    fn reset_state(&self) {
        let i = &self.0;
        self.set_hw_selection(0);

        // Storage params
        i.storage_devtype.set_selected(0);
        self.change_storage_devtype();
        i.addstorage.reset_state();

        // Network init
        let newmac = i.vm.conn().and_then(|c| generate_mac(&c));
        i.mac_address.set_active(newmac.is_some());
        i.create_mac_address
            .set_text(newmac.as_deref().unwrap_or_default());
        self.change_macaddr_use();

        i.netlist.reset_state();
        self.populate_network_model_combo();

        // Char parameters
        i.char_path.set_text("");
        i.char_channel.set_text("");
        i.char_auto_socket.set_active(true);
        i.char_clipboard.set_active(true);

        // RNG params; every libvirt we support has `conn_rng_urandom`.
        i.rng_device.set_text("/dev/urandom");

        // Remaining devices
        i.fs.reset_state();
        i.gfx.reset_state();
        i.vsock.reset_state();
        let guest = guest_info(&i.vm);
        let this = self.clone();
        glib::spawn_future_local(async move {
            let i = &this.0;
            let caps = i.vm.get_domain_capabilities().await;
            i.tpm
                .reset_state_with_model(build::default_tpm_model(caps.as_deref(), &guest));
            let (have_shared_mem, _) = i.vm.has_shared_mem().await;
            i.fs.set_have_shared_mem(have_shared_mem);
        });
    }

    /// `storage-devtype`'s pick.
    fn storage_devtype(&self) -> &'static str {
        let i = &self.0;
        i.storage_devtypes
            .get(i.storage_devtype.selected() as usize)
            .copied()
            .unwrap_or("disk")
    }

    /// `_refresh_disk_bus`: the buses for the device type, the first
    /// disk of that type's bus picked. The domcaps fetch is async.
    fn refresh_disk_bus(&self, devtype: &'static str) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            let i = &this.0;
            let caps = i.vm.get_domain_capabilities().await;
            let guest = guest_info(&i.vm);
            let conn = i.vm.conn();
            let is_test = conn.as_ref().is_some_and(|c| c.is_test());
            let is_xen = conn.as_ref().is_some_and(|c| c.is_xen());
            let buses: Vec<String> =
                match caps.as_deref().and_then(|c| c.devices.disk.get_enum("bus")) {
                    Some(e) => e.values.clone(),
                    None => disk_old_recommended_buses(&guest, is_test, is_xen)
                        .into_iter()
                        .map(str::to_string)
                        .collect(),
                };
            if this.storage_devtype() != devtype {
                return;
            }
            let buses = disk_recommended_buses(&buses, devtype);
            i.storage_bustype.set_values(
                buses
                    .iter()
                    .map(|b| (Some(b.clone()), crate::details::disk_pretty_bus(b)))
                    .collect(),
            );
            // By default, select bus of the first disk assigned to the VM
            let default_bus = i.vm.xmlobj().and_then(|x| {
                virtinst_xml::resolve_path(&x.doc, x.root(), &["devices"])?
                    .find_all(&x.doc, "disk")
                    .into_iter()
                    .find(|d| d.attribute(&x.doc, "device").unwrap_or("disk") == devtype)
                    .and_then(|d| crate::devpages::xml_attr(&x.doc, d, &["target"], "bus"))
            });
            match default_bus {
                Some(bus) => i.storage_bustype.set(Some(&bus)),
                None if !buses.is_empty() => i.storage_bustype.widget.set_selected(0),
                None => (),
            }
            this.storage_bus_changed();
        });
    }

    /// `_change_storage_devtype`.
    fn change_storage_devtype(&self) {
        let devtype = self.storage_devtype();
        self.refresh_disk_bus(devtype);
        let allow_create = !matches!(devtype, "cdrom" | "floppy");
        self.0.addstorage.set_allow_create(allow_create);
    }

    /// `_storage_bus_changed_cb`.
    fn storage_bus_changed(&self) {
        let bus = self.0.storage_bustype.get();
        self.0.addstorage.set_disk_bus(bus.as_deref());
    }

    /// `_browse_file`: the storage browser fills `entry`.
    fn browse_storage(&self, entry: &gtk::Entry) {
        let Some(conn) = self.0.vm.conn() else {
            return;
        };
        let entry = entry.clone();
        storagebrowse::browse(
            &self.0.storagebrowser,
            &conn,
            &self.0.window,
            BrowseReason::Image,
            move |path| {
                if !path.is_empty() {
                    entry.set_text(path);
                }
            },
        );
    }

    /// `_build_storage`, with `vmmAddStorage.build_device`: the pool
    /// and path lookups block, so this runs them on a worker.
    async fn build_storage(&self) -> Result<NewDev, String> {
        let i = &self.0;
        let conn = i.vm.conn().ok_or("Not connected")?;
        let handle = conn.handle().ok_or("Not connected")?;
        let device = self.storage_devtype().to_string();
        let bus = i.storage_bustype.get().ok_or("No bus selected")?;
        let guest = guest_info(&i.vm);
        let live = i.vm.xmlobj();
        let inactive = i.vm.make_xmlobj_to_define().await?;
        let disk_paths_targets =
            |doc: &Document, root: Element| -> Vec<(Option<String>, Option<String>)> {
                virtinst_xml::resolve_path(doc, root, &["devices"])
                    .map(|d| d.find_all(doc, "disk"))
                    .unwrap_or_default()
                    .into_iter()
                    .map(|d| {
                        (
                            virtinst_core::devices::disk::source_path(doc, d),
                            crate::devpages::xml_attr(doc, d, &["target"], "dev"),
                        )
                    })
                    .collect()
            };
        let live_disks = live
            .as_ref()
            .map(|x| disk_paths_targets(&x.doc, x.root()))
            .unwrap_or_default();
        let inactive_disks = disk_paths_targets(&inactive.doc, inactive.root());

        let req = DiskRequest {
            vmname: i.vm.get_name(),
            is_default_storage: i.addstorage.is_default_storage(),
            entry_path: i.addstorage.entry_path(),
            size: i.addstorage.size(),
            fmt: conn.get_default_storage_format(),
            collide_paths: live_disks.iter().filter_map(|(p, _)| p.clone()).collect(),
            is_session: conn.is_qemu_unprivileged(),
            is_local: !conn.is_remote(),
        };
        let plan = gtk::gio::spawn_blocking(move || plan_disk(handle.connect(), &req))
            .await
            .map_err(|_| "Worker thread panicked".to_string())??;

        // Generate target
        let mut used: Vec<String> = vec![];
        for t in live_disks
            .iter()
            .chain(&inactive_disks)
            .filter_map(|(_, t)| t.clone())
        {
            if !used.contains(&t) {
                used.push(t);
            }
        }
        let target = build::generate_target(&bus, &device, &used)?;

        let values = i.addstorage.get_values();
        let d = NewDisk {
            device: device.clone(),
            bus,
            target,
            path: plan.path.clone(),
            props: plan.props,
            sparse_install: plan.vol_install.as_ref().is_some_and(|v| v.allocation == 0),
            cache: values.cache,
            discard: values.discard,
            readonly: values.readonly,
            shareable: values.shareable,
            removable: values.removable,
            serial: values.serial,
        };
        let is_qemu = conn.is_qemu();
        let is_xen = conn.is_xen();
        let shareable = d.shareable.unwrap_or(false);
        let readonly = d.readonly.unwrap_or(device == "cdrom");
        let mut dev = NewDev::new(move |doc| Ok(build::disk(doc, &d, is_qemu, is_xen)));
        dev.validate = Validate::Disk(plan.path, device, shareable, readonly);
        dev.vol_install = plan.vol_install;

        // `_set_disk_controller`: add a SCSI controller with model
        // virtio-scsi if needed
        let has_any_scsi = live.as_ref().is_some_and(|x| {
            virtinst_xml::resolve_path(&x.doc, x.root(), &["devices"])
                .map(|d| d.find_all(&x.doc, "controller"))
                .unwrap_or_default()
                .iter()
                .any(|c| c.attribute(&x.doc, "type") == Some("scsi"))
        });
        if !has_any_scsi && guest.supports_virtioscsi(is_qemu) {
            dev.controller = Some(Rc::new(|doc: &mut Document| {
                Ok(build::controller(doc, "scsi", Some("virtio-scsi"), Some(0)))
            }));
        }
        Ok(dev)
    }

    /// `populate_network_model_combo`.
    fn populate_network_model_combo(&self) {
        let i = &self.0;
        let guest = guest_info(&i.vm);
        let is_qemu = i.vm.conn().is_some_and(|c| c.is_qemu());
        let mut values = vec![(None, "Hypervisor default".to_string())];
        values.extend(
            virtinst_core::devices::network::recommended_models(
                guest.domain_type.as_deref().unwrap_or_default(),
                guest.os_type.as_deref().unwrap_or_default(),
                guest.arch.as_deref().unwrap_or_default(),
                guest.machine.as_deref().unwrap_or_default(),
            )
            .into_iter()
            .map(|m| (Some(m.to_string()), m.to_string())),
        );
        i.net_model.set_values(values);
        i.net_model.set(guest.default_nic_model(is_qemu));
    }

    /// `_change_macaddr_use`.
    fn change_macaddr_use(&self) {
        let i = &self.0;
        i.create_mac_address
            .set_sensitive(i.mac_address.is_active());
    }

    /// `_populate_hostdev_model`: fetched on a worker, so Finish waits
    /// for the rows.
    fn populate_hostdev_model(&self, devtype: &'static str) {
        let i = &self.0;
        let gen = i.hostdev_generation.get() + 1;
        i.hostdev_generation.set(gen);
        i.hostdev_rows.replace(vec![]);
        i.host_device.remove_all();
        i.finish.set_sensitive(false);
        let Some(conn) = i.vm.conn() else {
            return;
        };
        let this = self.clone();
        glib::spawn_future_local(async move {
            let devs = conn.list_nodedevs_with_state().await;
            let i = &this.0;
            if i.hostdev_generation.get() != gen {
                return;
            }
            // `filter_nodedevs` takes the libvirt name of the type.
            let nodedev_type = if devtype == "usb" {
                "usb_device"
            } else {
                devtype
            };
            let rows = hostdev_rows(nodedev_type, &devs);
            for (_, label, sensitive, tooltip) in &rows {
                let l = gtk::Label::new(Some(label));
                l.set_xalign(0.0);
                l.set_margin_start(3);
                l.set_margin_end(3);
                l.set_margin_top(2);
                l.set_margin_bottom(2);
                l.set_sensitive(*sensitive);
                l.set_tooltip_text(tooltip.as_deref());
                i.host_device.append(&l);
            }
            i.hostdev_rows.replace(rows);
            i.nodedevs
                .replace(devs.into_iter().map(|(d, _)| d).collect());
            i.host_device
                .select_row(i.host_device.row_at_index(0).as_ref());
            this.hostdev_row_selected();
        });
    }

    /// `_hostdev_row_selected_cb`.
    fn hostdev_row_selected(&self) {
        let i = &self.0;
        if self.get_hw_selection().map(|o| o.page) != Some(Page::Hostdev) {
            return;
        }
        let sensitive = i
            .host_device
            .selected_row()
            .and_then(|r| {
                i.hostdev_rows
                    .borrow()
                    .get(r.index() as usize)
                    .map(|row| row.2)
            })
            .unwrap_or(false);
        i.finish.set_sensitive(sensitive);
    }

    fn set_hw_selection(&self, idx: i32) {
        let list = &self.0.hw_list;
        list.select_row(list.row_at_index(idx).as_ref());
    }

    fn get_hw_selection(&self) -> Option<&HwOption> {
        let row = self.0.hw_list.selected_row()?;
        self.0.options.get(row.index() as usize)
    }

    /// `_get_char_class`, as the row's tag.
    fn get_char_class(&self) -> &'static str {
        self.get_hw_selection()
            .and_then(|o| o.devtype)
            .unwrap_or("serial")
    }

    /// `_set_error_page`.
    fn set_error_page(&self, msg: Option<&str>) {
        let i = &self.0;
        i.top_pages.set_visible_child_name("error");
        i.error_label
            .set_text(msg.unwrap_or("Hardware selection error."));
        i.finish.set_sensitive(false);
    }

    /// `_hw_selected_cb`.
    fn hw_selected(&self) {
        let i = &self.0;
        i.finish.set_sensitive(true);
        let Some(opt) = self.get_hw_selection() else {
            self.set_error_page(None);
            return;
        };
        if !opt.sensitive {
            self.set_error_page(opt.errortxt.as_deref());
            return;
        }
        let page = opt.page;

        if page == Page::Char {
            // Need to do this here, since we share the char page between
            // multiple different HW options
            self.populate_char_device_type_combo();
            self.change_char_device_type();
            self.change_char_target_name();
        }
        if page == Page::Hostdev {
            // Need to do this here, since we share the hostdev page
            // between two separate HW options
            let devtype = opt.devtype.unwrap_or("usb");
            self.populate_hostdev_model(devtype);
            i.hostdev_usb_startup_policy_row
                .set_visible(devtype == "usb");
        }
        if page == Page::Controller {
            // We need to trigger this as it can desensitive 'finish'
            self.change_controller_type();
        }

        self.set_page_title(page);
        i.create_pages.set_visible_child_name(page.name());
        i.top_pages.set_visible_child_name("info");
    }

    /// `_dev_to_title`.
    fn dev_to_title(&self, page: Page) -> String {
        match page {
            Page::Disk => "Storage".into(),
            Page::Controller => "Controller".into(),
            Page::Network => "Network".into(),
            Page::Input => "Input".into(),
            Page::Graphics => "Graphics".into(),
            Page::Sound => "Sound".into(),
            Page::Video => "Video Device".into(),
            Page::Watchdog => "Watchdog Device".into(),
            Page::Filesystem => "Filesystem Passthrough".into(),
            Page::Smartcard => "Smartcard".into(),
            Page::Usbredir => "USB Redirection".into(),
            Page::Tpm => "TPM".into(),
            Page::Rng => "Random Number Generator".into(),
            Page::Panic => "Panic Notifier".into(),
            Page::Vsock => "VM Sockets".into(),
            Page::Char => {
                let tag = self.get_char_class();
                let mut c = tag.chars();
                let cap: String = c
                    .next()
                    .map(|f| f.to_uppercase().chain(c).collect())
                    .unwrap_or_default();
                format!("{cap} Device")
            }
            Page::Hostdev => match self.get_hw_selection().and_then(|o| o.devtype) {
                Some("pci") => "PCI Device".into(),
                Some("mdev") => "MDEV Device".into(),
                _ => "USB Device".into(),
            },
        }
    }

    /// `_set_page_title`.
    fn set_page_title(&self, page: Page) {
        self.0.page_title.set_markup(&self.dev_to_title(page));
    }

    /// `_populate_char_device_type_combo`.
    fn populate_char_device_type_combo(&self) {
        let i = &self.0;
        let tag = self.get_char_class();
        i.quiet.set(true);
        i.char_device_type.set_values(
            char_recommended_types(tag)
                .into_iter()
                .map(|t| {
                    let label = format!("{} ({t})", crate::details::char_pretty_type(t));
                    (Some(t.to_string()), label)
                })
                .collect(),
        );
        i.char_device_type.set(Some("pty"));
        i.quiet.set(false);
    }

    /// `_change_char_auto_socket`.
    fn change_char_auto_socket(&self) {
        let i = &self.0;
        if !i.char_auto_socket.is_visible() {
            return;
        }
        let doshow = !i.char_auto_socket.is_active();
        i.char_path_row.set_visible(doshow);
    }

    /// `_change_char_target_name`.
    fn change_char_target_name(&self) {
        let i = &self.0;
        if i.quiet.get() || !WidgetExt::is_visible(&i.char_target_name.entry) {
            return;
        }
        let text = i.char_target_name.entry.text();
        let settype = match text.as_str() {
            build::CHANNEL_NAME_SPICE => Some("spicevmc"),
            build::CHANNEL_NAME_SPICE_WEBDAV => {
                i.char_channel.set_text(&text);
                Some("spiceport")
            }
            build::CHANNEL_NAME_QEMUGA | build::CHANNEL_NAME_LIBGUESTFS => Some("unix"),
            _ => None,
        };
        if let Some(settype) = settype {
            i.char_device_type.set(Some(settype));
        }
    }

    /// `_change_char_device_type`.
    fn change_char_device_type(&self) {
        let i = &self.0;
        if i.quiet.get() {
            return;
        }
        let Some(devtype) = i.char_device_type.get() else {
            return;
        };
        let tag = self.get_char_class();
        let ischan = tag == "channel";
        let iscon = tag == "console";
        let show_auto = devtype == "unix" && ischan;

        let supports_path = ["file", "unix", "dev", "pipe"];
        i.char_path_row
            .set_visible(supports_path.contains(&devtype.as_str()));
        i.char_channel_row.set_visible(devtype == "spiceport");
        i.char_clipboard_row.set_visible(devtype == "qemu-vdagent");

        i.char_target_name_row.set_visible(ischan);
        i.char_target_type_row.set_visible(iscon);
        i.char_auto_socket_row.set_visible(show_auto);
        self.change_char_auto_socket();
    }

    /// `_change_controller_type`.
    fn change_controller_type(&self) {
        let i = &self.0;
        let combo = &i.controller_model;
        let vmname = i.vm.get_name();
        i.controller_tooltip.set_visible(false);
        i.controller_tooltip.set_tooltip_text(Some(&format!(
            "{vmname} already has a USB controller attached.\n\
             Adding more than one USB controller is not supported.\n\
             You can change the USB controller type in the VM details screen."
        )));
        combo.widget.set_sensitive(true);
        // Unlike upstream, a USB pick that's changed back doesn't leave
        // the controller to be replaced behind.
        i.remove_usb_controller.replace(None);

        let controller_type = i.controller_type.get().unwrap_or_default();
        if controller_type == "usb" {
            let usb: Vec<(usize, Element)> = i
                .vm
                .xmlobj()
                .map(|x| {
                    let devices = virtinst_xml::resolve_path(&x.doc, x.root(), &["devices"]);
                    let ids: Vec<_> = devices
                        .map(|d| d.find_all(&x.doc, "controller"))
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|c| c.attribute(&x.doc, "type") == Some("usb"))
                        .enumerate()
                        .collect();
                    if let [(idx, el)] = ids.as_slice() {
                        if el.attribute(&x.doc, "model") == Some("none") {
                            i.remove_usb_controller.replace(Some(
                                virtinst_core::devices::device_id(&x.doc, *el, "controller", *idx),
                            ));
                        }
                    }
                    ids
                })
                .unwrap_or_default();
            if usb.is_empty() || i.remove_usb_controller.borrow().is_some() {
                i.finish.set_sensitive(true);
            } else {
                i.controller_tooltip.set_visible(true);
                i.finish.set_sensitive(false);
            }
        } else {
            i.finish.set_sensitive(true);
        }

        // `populate_controller_model_combo`
        let rows = virtinst_core::devices::controller::model_rows(&controller_type);
        let n_rows = rows.len();
        let first = rows.first().and_then(|(v, _)| *v);
        combo.set_values(
            rows.into_iter()
                .map(|(v, l)| (v.map(str::to_string), l.to_string()))
                .collect(),
        );
        combo.set(first);
        i.controller_model_row.set_visible(n_rows > 1);
    }

    /// `_build_device_page`, with `set_defaults` folded into the
    /// builders.
    fn build_device_page(&self, page: Page) -> Result<NewDev, String> {
        let i = &self.0;
        let guest = guest_info(&i.vm);
        let conn = i.vm.conn();
        Ok(match page {
            Page::Controller => return self.build_controller(),
            Page::Input => {
                let val = i.input_type.get().unwrap_or_default();
                let (typ, bus) = val.split_once(':').unwrap_or(("mouse", "usb"));
                let (typ, bus) = (typ.to_string(), bus.to_string());
                NewDev::new(move |doc| Ok(build::input(doc, &typ, &bus)))
            }
            Page::Graphics => {
                let values = i.gfx.build_values();
                let is_remote = conn.as_ref().is_some_and(|c| c.is_remote());
                NewDev::new(move |doc| {
                    let el = Element::new(doc, "graphics");
                    values.apply(doc, el)?;
                    build::graphics_defaults(doc, el, is_remote);
                    Ok(el)
                })
            }
            Page::Sound => {
                let model = i.sound_model.get();
                NewDev::new(move |doc| Ok(build::sound(doc, model.as_deref(), &guest)))
            }
            Page::Char => {
                let tag = self.get_char_class();
                let path = i.char_path.text().to_string();
                let channel = i.char_channel.text().to_string();
                let c = NewChar {
                    tag: tag.to_string(),
                    char_type: i.char_device_type.get().unwrap_or_else(|| "pty".into()),
                    source_path: i.char_path_row.is_visible().then_some(path),
                    source_channel: i.char_channel_row.is_visible().then_some(channel),
                    clipboard: i
                        .char_clipboard_row
                        .is_visible()
                        .then(|| i.char_clipboard.is_active()),
                    target_name: i
                        .char_target_name_row
                        .is_visible()
                        .then(|| i.char_target_name.entry.text().to_string()),
                    target_type: i
                        .char_target_type_row
                        .is_visible()
                        .then(|| i.char_target_type.get())
                        .flatten(),
                };
                NewDev::new(move |doc| Ok(build::char_device(doc, &c)))
            }
            Page::Video => {
                let model = i.video_model.get();
                let is_qemu = conn.as_ref().is_some_and(|c| c.is_qemu());
                let default = guest.default_video_model(None, is_qemu);
                NewDev::new(move |doc| Ok(build::video(doc, model.as_deref(), default, &guest)))
            }
            Page::Watchdog => {
                let model = i.watchdog_model.get();
                let action = i.watchdog_action.get();
                NewDev::new(move |doc| {
                    Ok(build::watchdog(doc, model.as_deref(), action.as_deref()))
                })
            }
            Page::Filesystem => {
                let values = i.fs.build_values();
                let driver_mode = conn
                    .as_ref()
                    .is_some_and(|c| c.is_qemu() || c.is_lxc() || c.is_test());
                let dev = NewDev::new(move |doc| {
                    let el = Element::new(doc, "filesystem");
                    values.apply(doc, el);
                    build::filesystem_defaults(doc, el, driver_mode)?;
                    Ok(el)
                });
                // `build_device` validates as it builds.
                dev.xml()?;
                dev
            }
            Page::Smartcard => {
                let mode = i.smartcard_mode.get();
                NewDev::new(move |doc| Ok(build::smartcard(doc, mode.as_deref())))
            }
            Page::Usbredir => {
                let typ = i.usbredir_type.get();
                NewDev::new(move |doc| Ok(build::redirdev(doc, typ.as_deref())))
            }
            Page::Tpm => {
                let values = i.tpm.build_values();
                NewDev::new(move |doc| {
                    let el = Element::new(doc, "tpm");
                    values.apply(doc, el);
                    tpm_defaults(doc, el);
                    Ok(el)
                })
            }
            Page::Rng => {
                let device = i.rng_device.text().to_string();
                NewDev::new(move |doc| Ok(build::rng(doc, &device)))
            }
            Page::Panic => {
                let model = i.panic_model.get();
                NewDev::new(move |doc| Ok(build::panic(doc, model.as_deref())))
            }
            Page::Vsock => {
                let (auto_cid, cid) = i.vsock.get_values();
                NewDev::new(move |doc| Ok(build::vsock(doc, auto_cid, cid)))
            }
            Page::Network => {
                let (nettype, source, source_mode, portgroup) =
                    i.netlist
                        .get_network_selection()
                        .ok_or("No network source selected")?;
                let mac = if i.mac_address.is_active() {
                    i.create_mac_address.text().to_string()
                } else {
                    // `set_defaults`, here so the hotplugged and the
                    // persistent device get the same MAC.
                    conn.as_ref().and_then(generate_mac).unwrap_or_default()
                };
                let is_qemu = conn.as_ref().is_some_and(|c| c.is_qemu());
                let default_model = guest.default_nic_model(is_qemu);
                let n = NewInterface {
                    nettype,
                    source,
                    source_mode,
                    portgroup,
                    mac,
                    model: i.net_model.get(),
                };
                let mut dev = NewDev::new({
                    let n = n.clone();
                    move |doc| Ok(build::interface(doc, &n, default_model))
                });
                dev.validate = Validate::Network(n);
                dev
            }
            Page::Hostdev => {
                let row = i
                    .host_device
                    .selected_row()
                    .and_then(|r| i.hostdev_rows.borrow().get(r.index() as usize).cloned());
                let nodedev = row.and_then(|r| r.0).ok_or("No host device selected")?;
                let all = i.nodedevs.borrow().clone();
                let is_xen = conn.as_ref().is_some_and(|c| c.is_xen());
                let policy = i.hostdev_usb_startup_policy.get();
                let dev = {
                    let nodedev = nodedev.clone();
                    NewDev::new(move |doc| {
                        build::hostdev(doc, &nodedev, &all, is_xen, policy.as_deref())
                    })
                };
                // `set_from_nodedev` raises for a type it can't pass.
                dev.xml()?;
                NewDev {
                    validate: Validate::Hostdev(Box::new(nodedev)),
                    ..dev
                }
            }
            Page::Disk => return Err("The storage page builds asynchronously".into()),
        })
    }

    /// `_build_controller`.
    fn build_controller(&self) -> Result<NewDev, String> {
        let i = &self.0;
        let controller_type = i.controller_type.get().unwrap_or_default();
        let model = i.controller_model.get();

        if let Some(id) = i.remove_usb_controller.borrow().clone() {
            let mut dev = NewDev::new(|_| Err("unused".into()));
            dev.usb_replace = Some((id, model));
            return Ok(dev);
        }

        let index = i.vm.xmlobj().and_then(|x| {
            let devices = virtinst_xml::resolve_path(&x.doc, x.root(), &["devices"])?;
            devices
                .find_all(&x.doc, "controller")
                .into_iter()
                .filter(|c| c.attribute(&x.doc, "type") == Some(controller_type.as_str()))
                .map(|c| {
                    c.attribute(&x.doc, "index")
                        .and_then(|v| v.parse::<u32>().ok())
                        .unwrap_or(0)
                })
                .max()
                .map(|m| m + 1)
        });
        Ok(NewDev::new(move |doc| {
            Ok(build::controller(
                doc,
                &controller_type,
                model.as_deref(),
                index,
            ))
        }))
    }

    /// `_build_device`.
    async fn build_device(&self) -> Option<NewDev> {
        let page = self.get_hw_selection()?.page;
        let dev = if page == Page::Disk {
            self.build_storage().await
        } else {
            self.build_device_page(page)
        };
        match dev {
            Ok(dev) => Some(dev),
            Err(e) => {
                self.show_err(&format!("Error building device XML: {e}"), None)
                    .await;
                None
            }
        }
    }

    async fn show_err(&self, msg: &str, details: Option<&str>) {
        error::show_err(Some(&self.0.window), msg, details, "").await;
    }

    /// `_finish`.
    async fn finish(&self) {
        let Some(dev) = self.build_device().await else {
            return;
        };
        match self.validate_device(&dev).await {
            Ok(true) => (),
            Ok(false) => return,
            Err(e) => {
                self.show_err(&format!("Error validating device parameters: {e}"), None)
                    .await;
                return;
            }
        }

        let i = &self.0;
        i.window.set_cursor_from_name(Some("progress"));
        let vol_install = dev.vol_install.clone();
        let handle = i.vm.conn().and_then(|c| c.handle());
        let err = crate::asyncjob::run(
            Some(&i.window),
            "Creating device",
            "Depending on the device, this may take a few minutes to complete.",
            true,
            // `_setup_device`: only disks have work, `build_storage`.
            move |_| {
                let (Some(vol), Some(handle)) = (vol_install, handle) else {
                    return Ok(());
                };
                glib::g_debug!(
                    "virt-manager",
                    "Running build_storage() for vol={}",
                    vol.name
                );
                vol.install(handle.connect()).map_err(|e| (e.clone(), e))?;
                glib::g_debug!("virt-manager", "build_storage() complete");
                if let Ok(pool) = virt::storage_pool::StoragePool::lookup_by_name(
                    handle.connect(),
                    &vol.pool_name,
                ) {
                    let _ = pool.refresh(0);
                }
                Ok(())
            },
        )
        .await;
        // `_finish_cb`
        let mut failure = true;
        let mut error = err.map(|(e, d)| (e, Some(d)));
        if error.is_none() {
            match self.add_device(&dev).await {
                Ok(f) => failure = f,
                Err(e) => error = Some((format!("Unable to add device: {e}"), None)),
            }
        }
        if let Some((e, details)) = error {
            self.show_err(&e, details.as_deref()).await;
        }
        i.window.set_cursor_from_name(None);
        if !failure {
            self.close();
        }
    }

    /// `_validate_device`: whether to go on.
    async fn validate_device(&self, dev: &NewDev) -> Result<bool, String> {
        let i = &self.0;
        match &dev.validate {
            Validate::None | Validate::Disk(..) => (),
            Validate::Network(n) => {
                i.netlist
                    .validate_device(&i.window, &n.nettype, n.source.as_deref(), &n.mac)
                    .await?
            }
            Validate::Hostdev(nodedev) => {
                // `_validate_hostdev_collision`
                let names: Vec<String> =
                    i.vm.conn()
                        .map(|c| c.list_vms())
                        .unwrap_or_default()
                        .iter()
                        .filter(|vm| {
                            vm.xmlobj().is_some_and(|x| {
                                virtinst_xml::resolve_path(&x.doc, x.root(), &["devices"])
                                    .map(|d| d.find_all(&x.doc, "hostdev"))
                                    .unwrap_or_default()
                                    .into_iter()
                                    .any(|h| nodedev.compare_to_hostdev(&x.doc, h))
                            })
                        })
                        .map(|vm| vm.get_name())
                        .collect();
                if !names.is_empty() {
                    let res = error::yes_no(
                        Some(&i.window),
                        &format!(
                            "The device is already in use by other guests {}",
                            py_list(&names)
                        ),
                        Some("Do you really want to use the device?"),
                    )
                    .await;
                    if !res {
                        return Ok(false);
                    }
                }
            }
        }
        if let Validate::Disk(path, device, shareable, readonly) = &dev.validate {
            // `vmmAddStorage.validate_device`
            if path.is_none() && matches!(device.as_str(), "disk" | "lun") {
                error::val_err(Some(&i.window), "A storage path must be specified.").await;
                return Ok(false);
            }
            if let Some(path) = path {
                // Disk collision
                let names =
                    i.vm.path_in_use_by(path.clone(), *shareable, *readonly)
                        .await?;
                if !names.is_empty() {
                    let msg = format!(
                        "Disk '{path}' is already in use by other guests {}",
                        py_list(&names)
                    );
                    if !error::yes_no(
                        Some(&i.window),
                        &msg,
                        Some("Do you really want to use the disk?"),
                    )
                    .await
                    {
                        return Ok(false);
                    }
                }
            }
        }
        Ok(true)
    }

    /// `_add_device`: hotplug, asking whether to go on when that fails,
    /// then the persistent config. Resolves to whether it failed.
    async fn add_device(&self, dev: &NewDev) -> Result<bool, String> {
        let i = &self.0;
        let vm = &i.vm;
        let parent = Some(&i.window);

        if let Some((id, model)) = &dev.usb_replace {
            // `change_config_helper(define_controller, ...)`
            let define_model = model.clone();
            let res = vm
                .define_device(id, move |doc, el| {
                    let devices = el.parent(doc).ok_or("Controller has no parent")?;
                    virtinst_core::devices::controller::set_usb_model(
                        doc,
                        devices,
                        define_model.as_deref(),
                    );
                    Ok(())
                })
                .await;
            if let Err(e) = res {
                error::show_err(
                    parent.map(|w| w.upcast_ref()),
                    &format!("Error changing VM configuration: {e}"),
                    None,
                    "",
                )
                .await;
            } else if vm.is_active() {
                error::show_notice(
                    parent.map(|w| w.upcast_ref()),
                    "These changes will take effect after the next guest shutdown.",
                    None,
                )
                .await;
            }
            i.remove_usb_controller.replace(None);
            return Ok(false);
        }

        let xml = dev.xml()?;
        glib::g_debug!("virt-manager", "Adding device:\n{xml}");
        let controller_xml = match &dev.controller {
            Some(build) => {
                let mut doc = Document::new();
                let el = build(&mut doc)?;
                let xml = virtinst_xml::element_to_string(doc, el).map_err(|e| e.to_string())?;
                glib::g_debug!("virt-manager", "Adding controller:\n{xml}");
                Some(xml)
            }
            None => None,
        };

        // Hotplug device
        let attach = async {
            if let Some(cxml) = controller_xml {
                vm.attach_device(cxml).await?;
            }
            vm.attach_device(xml).await
        };
        if let Err(e) = attach.await {
            glib::g_debug!("virt-manager", "Device could not be hotplugged: {e}");
            let (res, _) = error::run_dialog(
                parent.map(|w| w.upcast_ref()),
                error::DialogSpec {
                    kind: error::MessageType::Warning,
                    buttons: error::Buttons::YesNo,
                    text1: "Are you sure you want to add this device?",
                    text2: Some(
                        "This device could not be attached to the running machine. \
                         Would you like to make the device available after the next \
                         guest shutdown?",
                    ),
                    title: "",
                    details: Some(&e),
                    chktext: None,
                },
            )
            .await;
            if !res {
                return Ok(false);
            }
        }

        // Alter persistent config
        if let Some(controller) = dev.controller.clone() {
            vm.add_device(move |doc| controller(doc)).await?;
        }
        let build = dev.build.clone();
        vm.add_device(move |doc| build(doc)).await?;
        Ok(false)
    }
}

/// `DeviceTpm.set_defaults` for what the TPM page leaves: a device
/// path implies passthrough, else emulator.
fn tpm_defaults(doc: &mut Document, tpm: Element) {
    let backend = virtinst_xml::resolve_or_create_path(doc, tpm, &["backend"]);
    if backend.attribute(doc, "type").is_none() {
        let has_path = virtinst_xml::resolve_path(doc, backend, &["device"])
            .and_then(|d| d.attribute(doc, "path"))
            .is_some();
        let typ = if has_path { "passthrough" } else { "emulator" };
        backend.set_attribute(doc, "type", typ);
    }
}

/// The guest facts the device defaults read, from the VM's XML.
fn guest_info(vm: &VmmDomain) -> GuestInfo {
    let Some(x) = vm.xmlobj() else {
        return GuestInfo::default();
    };
    let doc = &x.doc;
    let os = &x.guest.os;
    let graphics = virtinst_xml::resolve_path(doc, x.root(), &["devices"])
        .map(|d| d.find_all(doc, "graphics"))
        .unwrap_or_default();
    let has_spice = graphics
        .iter()
        .any(|g| g.attribute(doc, "type") == Some("spice"));
    let has_gl = graphics
        .iter()
        .any(|g| g.find(doc, "gl").and_then(|gl| gl.attribute(doc, "enable")) == Some("yes"));
    let is_uefi = (os.loader.is_some() && os.loader_type.as_deref() == Some("pflash"))
        || os.firmware.as_deref() == Some("efi");
    let osinfo = x
        .guest
        .metadata
        .libosinfo
        .os_id
        .as_deref()
        .and_then(crate::oslist::lookup_osinfo);
    GuestInfo {
        os_type: os.os_type.clone(),
        arch: os.arch.clone(),
        machine: os.machine.clone(),
        domain_type: x.guest.domain_type.clone(),
        has_spice,
        has_gl,
        is_uefi,
        os_virtiogpu: osinfo.as_ref().is_some_and(|o| o.supports_virtiogpu()),
        os_virtionet: osinfo.as_ref().is_some_and(|o| o.supports_virtionet()),
        os_virtioscsi: osinfo.as_ref().is_some_and(|o| o.supports_virtioscsi()),
        os_virtiodisk: osinfo.as_ref().is_some_and(|o| o.supports_virtiodisk()),
        os_netmodels: osinfo
            .as_ref()
            .map(|o| o.supported_netmodels())
            .unwrap_or_default(),
        spice_fallback: false,
    }
}

/// `DeviceInterface.generate_mac`: a random MAC no VM on `conn` uses.
fn generate_mac(conn: &crate::connection::VmmConnection) -> Option<String> {
    for _ in 0..256 {
        let mac = virtinst_core::cloner::generate_mac(conn.is_qemu());
        if crate::netlist::check_mac_in_use(conn, &mac).is_ok() {
            return Some(mac);
        }
    }
    glib::g_debug!("virt-manager", "Failed to generate non-conflicting MAC");
    None
}

/// `_set_initial_state`'s `add_hw_option` rows.
fn hw_options(guest: &GuestInfo) -> Vec<HwOption> {
    let is_hvm = guest.is_hvm();
    let not_hvm = Some("Not supported for this guest type.");
    // Every libvirt we support has `conn_nodedev`.
    let nodedev_err = Some("Connection does not support host device enumeration");
    let pci_err = if guest.is_container() {
        Some("Not supported for containers")
    } else {
        nodedev_err
    };
    let vsock_err = Some("Not supported for this hypervisor/libvirt/arch combination.");
    let opt = |name, icon, page, sensitive: bool, errortxt: Option<&str>, devtype| HwOption {
        name,
        icon,
        page,
        sensitive,
        errortxt: errortxt.map(str::to_string),
        devtype,
    };
    vec![
        opt("Storage", "drive-harddisk", Page::Disk, true, None, None),
        opt(
            "Controller",
            "device_pci",
            Page::Controller,
            true,
            None,
            None,
        ),
        opt("Network", "network-idle", Page::Network, true, None, None),
        opt("Input", "input-mouse", Page::Input, is_hvm, not_hvm, None),
        opt(
            "Graphics",
            "video-display",
            Page::Graphics,
            true,
            None,
            None,
        ),
        opt("Sound", "audio-card", Page::Sound, is_hvm, not_hvm, None),
        opt(
            "Serial",
            "device_serial",
            Page::Char,
            is_hvm,
            not_hvm,
            Some("serial"),
        ),
        opt(
            "Parallel",
            "device_serial",
            Page::Char,
            is_hvm,
            not_hvm,
            Some("parallel"),
        ),
        opt(
            "Console",
            "device_serial",
            Page::Char,
            true,
            None,
            Some("console"),
        ),
        opt(
            "Channel",
            "device_serial",
            Page::Char,
            is_hvm,
            not_hvm,
            Some("channel"),
        ),
        opt(
            "USB Host Device",
            "device_usb",
            Page::Hostdev,
            true,
            nodedev_err,
            Some("usb"),
        ),
        opt(
            "PCI Host Device",
            "device_pci",
            Page::Hostdev,
            !guest.is_container(),
            pci_err,
            Some("pci"),
        ),
        opt(
            "MDEV Host Device",
            "device_pci",
            Page::Hostdev,
            true,
            nodedev_err,
            Some("mdev"),
        ),
        opt("Video", "video-display", Page::Video, true, None, None),
        opt(
            "Watchdog",
            "device_pci",
            Page::Watchdog,
            is_hvm,
            not_hvm,
            None,
        ),
        opt("Filesystem", "folder", Page::Filesystem, true, None, None),
        opt(
            "Smartcard",
            "device_serial",
            Page::Smartcard,
            true,
            None,
            None,
        ),
        opt(
            "USB Redirection",
            "device_usb",
            Page::Usbredir,
            true,
            None,
            None,
        ),
        opt("TPM", "device_cpu", Page::Tpm, true, None, None),
        opt("RNG", "system-run", Page::Rng, true, None, None),
        opt(
            "Panic Notifier",
            "system-run",
            Page::Panic,
            true,
            None,
            None,
        ),
        opt(
            "VirtIO VSOCK",
            "network-idle",
            Page::Vsock,
            is_hvm,
            vsock_err,
            None,
        ),
    ]
}

/// A `hw-list` row: the icon and name, greyed when insensitive. The row
/// itself stays selectable, to show the error page.
fn hw_row(opt: &HwOption) -> gtk::ListBoxRow {
    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    hbox.set_margin_top(3);
    hbox.set_margin_bottom(3);
    hbox.set_margin_start(3);
    hbox.set_margin_end(3);
    let image = gtk::Image::from_icon_name(opt.icon);
    let label = gtk::Label::new(Some(opt.name));
    label.set_xalign(0.0);
    label.set_margin_start(6);
    hbox.append(&image);
    hbox.append(&label);
    hbox.set_sensitive(opt.sensitive);
    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&hbox));
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostdev_list() {
        let parse = |xml: &str| NodeDevice::parse(xml).unwrap();
        let pci = parse(
            "<device><name>pci_0000_00_03_0</name><capability type='pci'>\
             <domain>0</domain><bus>0</bus><slot>3</slot><function>0</function>\
             <product id='0x100e'>82540EM</product><vendor id='0x8086'>Intel</vendor>\
             </capability></device>",
        );
        let net = parse(
            "<device><name>net_eth0</name><parent>pci_0000_00_03_0</parent>\
             <capability type='net'><interface>eth0</interface></capability></device>",
        );
        let bridge = parse(
            "<device><name>pci_0000_00_1c_0</name><capability type='pci'>\
             <capability type='pci-bridge'/></capability></device>",
        );
        let hub = parse(
            "<device><name>usb_usb1</name><capability type='usb_device'><bus>1</bus>\
             <device>1</device><product id='0x0002'/><vendor id='0x1d6b'/>\
             </capability></device>",
        );
        let devs = vec![(pci, true), (net, true), (bridge, true), (hub, false)];
        let rows = hostdev_rows("pci", &devs);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, "0000:00:03:0 Intel 82540EM (Interface eth0)");
        assert!(rows[0].2 && rows[0].3.is_none());
        let rows = hostdev_rows("usb_device", &devs);
        assert_eq!(
            (rows[0].0.is_none(), rows[0].1.as_str(), rows[0].2),
            (true, "No Devices Available", false)
        );
        assert_eq!(py_list(&["a".into(), "b".into()]), "['a', 'b']");
    }

    #[test]
    fn char_types() {
        assert_eq!(char_recommended_types("console"), ["pty"]);
        assert_eq!(char_recommended_types("serial"), ["pty", "file", "unix"]);
        assert_eq!(
            char_recommended_types("channel"),
            [
                "spicevmc",
                "spiceport",
                "qemu-vdagent",
                "pty",
                "file",
                "unix"
            ]
        );
    }
}
