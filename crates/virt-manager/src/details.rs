//! Upstream's `virtManager/details/details.py` (`vmmDetails`): the VM
//! window's Details page. Left, the `hw-list` sidebar (fixed rows for
//! Overview/OS/Performance/CPUs/Memory/Boot plus one row per device);
//! right, the page for the selected row; below, Remove / Cancel / Apply.
//!
//! Per ticket 10 the sidebar is a plain `GtkListBox` with directly built
//! rows. Ported so far: the sidebar (labels and icons exactly as
//! upstream computes them), the edit/Apply machinery and the Overview
//! page; the other pages arrive in the next slices.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use virtinst_xml::{Document, Element};

use crate::domain::{DomainXml, VmmDomain};
use virtinst_core::domain::cpu;

/// `HW_LIST_TYPE_*`; the order is the order rows are listed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HwType {
    General,
    Os,
    Stats,
    Cpu,
    Memory,
    Boot,
    Disk,
    Nic,
    Input,
    Graphics,
    Sound,
    Char,
    Hostdev,
    Video,
    Watchdog,
    Controller,
    Filesystem,
    Smartcard,
    Redirdev,
    Tpm,
    Rng,
    Panic,
    Vsock,
}

/// `EDIT_*`: what the pending changes touch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edit {
    Name,
    Title,
    Desc,
    Vcpus,
    Cpu,
    Topology,
    Mem,
    MemShared,
    Autostart,
    BootOrder,
    BootMenu,
    Kernel,
    Init,
    OsName,
    DiskPath,
    Disk,
    NetModel,
    NetSource,
    NetLinkstate,
    SoundModel,
    SmartcardMode,
    VideoModel,
    Video3d,
    WatchdogModel,
    WatchdogAction,
    Gfx,
    ControllerModel,
    HostdevRombar,
    HostdevStartupPolicy,
    Tpm,
    VsockAuto,
    VsockCid,
    Fs,
}

impl HwType {
    /// `remove_pages`: rows that get a Remove button.
    fn is_removable(self) -> bool {
        self >= HwType::Disk
    }
}

/// One `hw-list` row: `[label, icon name, hw type, device, key]`. A
/// device is identified like upstream's `get_xml_id()`: its tag plus
/// its position among same-tag siblings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HwRow {
    pub label: String,
    pub icon: String,
    pub hwtype: HwType,
    pub device: Option<(String, usize)>,
}

impl HwRow {
    fn key(&self) -> String {
        match &self.device {
            Some((tag, idx)) => format!("./devices/{tag}[{}]", idx + 1),
            None => self.label.clone(),
        }
    }
}

/// `_init_hw_list`'s static rows.
fn static_rows(is_customize_dialog: bool) -> Vec<HwRow> {
    let mut rows = vec![
        ("Overview", HwType::General, "computer"),
        ("OS information", HwType::Os, "computer"),
    ];
    if !is_customize_dialog {
        rows.push(("Performance", HwType::Stats, performance_icon_name()));
    }
    rows.extend([
        ("CPUs", HwType::Cpu, "device_cpu"),
        ("Memory", HwType::Memory, "device_mem"),
        ("Boot Options", HwType::Boot, "system-run"),
    ]);
    rows.into_iter()
        .map(|(label, hwtype, icon)| HwRow {
            label: label.into(),
            icon: icon.into(),
            hwtype,
            device: None,
        })
        .collect()
}

/// `_get_performance_icon_name`: not in every icon theme.
fn performance_icon_name() -> &'static str {
    let icon = "utilities-system-monitor";
    let has = gtk::gdk::Display::default()
        .map(|d| gtk::IconTheme::for_display(&d).has_icon(icon))
        .unwrap_or(false);
    if has {
        icon
    } else {
        "system-run"
    }
}

// Small readers over the raw device elements.
fn attr(doc: &Document, el: Element, name: &str) -> Option<String> {
    el.attribute(doc, name).map(str::to_string)
}

fn child_attr(doc: &Document, el: Element, path: &[&str], name: &str) -> Option<String> {
    virtinst_xml::resolve_path(doc, el, path).and_then(|c| attr(doc, c, name))
}

/// storagepool.py's `_pretty_bytes`.
fn pretty_bytes(val: u64) -> String {
    let val = val as f64;
    if val > 1024.0 * 1024.0 * 1024.0 {
        format!("{:.2} GiB", val / (1024.0 * 1024.0 * 1024.0))
    } else {
        format!("{:.2} MiB", val / (1024.0 * 1024.0))
    }
}

/// `vmmAddHardware.disk_pretty_bus`.
pub fn disk_pretty_bus(bus: &str) -> String {
    match bus {
        "ide" => "IDE",
        "sata" => "SATA",
        "scsi" => "SCSI",
        "sd" => "SD",
        "usb" => "USB",
        "virtio" => "VirtIO",
        "xen" => "Xen",
        other => other,
    }
    .to_string()
}

/// `vmmAddHardware.char_pretty_type`.
pub fn char_pretty_type(val: &str) -> String {
    match val {
        "pty" => "Pseudo TTY",
        "file" => "Output to a file",
        "tcp" => "TCP net console",
        "udp" => "UDP net console",
        "unix" => "UNIX socket",
        "spicevmc" => "Spice agent",
        "spiceport" => "Spice port",
        "qemu-vdagent" => "QEMU vdagent",
        other => other,
    }
    .to_string()
}

/// `vmmAddHardware.char_pretty_channel_name`.
pub fn char_pretty_channel_name(val: &str) -> Option<&'static str> {
    match val {
        "com.redhat.spice.0" => Some("spice"),
        "org.qemu.guest_agent.0" => Some("qemu-ga"),
        "org.libguestfs.channel.0" => Some("libguestfs"),
        "org.spice-space.webdav.0" => Some("spice-webdav"),
        _ => None,
    }
}

/// `vmmGraphicsDetails.graphics_pretty_type_simple`.
pub fn graphics_pretty_type_simple(gtype: &str) -> String {
    if matches!(gtype, "vnc" | "sdl" | "rdp") {
        gtype.to_uppercase()
    } else {
        capitalize(gtype)
    }
}

/// `vmmAddHardware.video_pretty_model`.
pub fn video_pretty_model(model: &str) -> String {
    if matches!(model, "qxl" | "vmvga" | "vga") {
        model.to_uppercase()
    } else {
        capitalize(model)
    }
}

/// `vmmAddHardware.controller_pretty_type`.
fn controller_pretty_type(val: &str) -> String {
    match val {
        "ide" => "IDE",
        "fdc" => "Floppy",
        "scsi" => "SCSI",
        "sata" => "SATA",
        "virtio-serial" => "VirtIO Serial",
        "usb" => "USB",
        "pci" => "PCI",
        "ccid" => "CCID",
        "xenbus" => "xenbus",
        other => other,
    }
    .to_string()
}

/// `vmmAddHardware.controller_pretty_desc`.
fn controller_pretty_desc(ctype: &str, model: Option<&str>) -> String {
    match (ctype, model) {
        ("scsi", Some("virtio-scsi")) => "VirtIO SCSI".into(),
        ("pci", Some("pcie-root")) => "PCIe".into(),
        _ => controller_pretty_type(ctype),
    }
}

/// Python's `str.capitalize()`.
fn capitalize(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut chars = lower.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// `vmmAddHardware.hostdev_pretty_name`.
fn hostdev_pretty_name(doc: &Document, el: Element) -> String {
    let dehex = |v: String| v.strip_prefix("0x").map(str::to_string).unwrap_or(v);
    let safeint = |v: String| match v.parse::<i64>() {
        Ok(n) => format!("{n:03}"),
        Err(_) => v,
    };
    let src = |path: &[&str], name| child_attr(doc, el, path, name);
    let mut label = attr(doc, el, "type").unwrap_or_default().to_uppercase();
    let vendor = src(&["source", "vendor"], "id");
    let product = src(&["source", "product"], "id");
    let bus = src(&["source", "address"], "bus");
    let device = src(&["source", "address"], "device");
    let slot = src(&["source", "address"], "slot");
    let function = src(&["source", "address"], "function");
    let domain = src(&["source", "address"], "domain");
    let uuid = src(&["source", "address"], "uuid");
    if let (Some(v), Some(p)) = (vendor, product) {
        label += &format!(" {}:{}", dehex(v), dehex(p));
    } else if let (Some(b), Some(d)) = (bus.clone(), device) {
        label += &format!(" {}:{}", safeint(b), safeint(d));
    } else if let (Some(b), Some(s), Some(f), Some(d)) = (bus, slot, function, domain) {
        label += &format!(" {}:{}:{}.{}", dehex(d), dehex(b), dehex(s), dehex(f));
    } else if let Some(u) = uuid {
        label += &format!(" {u}");
    }
    label
}

/// `_label_for_device`.
fn label_for_device(doc: &Document, tag: &str, idx: usize, el: Element, disk_idx: usize) -> String {
    let a = |name| attr(doc, el, name);
    let port = || {
        child_attr(doc, el, &["target"], "port")
            .and_then(|p| p.parse::<u32>().ok())
            .unwrap_or(0)
            + 1
    };
    match tag {
        "disk" => {
            let device = a("device").unwrap_or_else(|| "disk".into());
            if device == "floppy" {
                return format!("Floppy {disk_idx}");
            }
            let bus = child_attr(doc, el, &["target"], "bus")
                .map(|b| disk_pretty_bus(&b))
                .unwrap_or_default();
            match device.as_str() {
                "cdrom" => format!("{bus} CDROM {disk_idx}"),
                "disk" => format!("{bus} Disk {disk_idx}"),
                other => format!("{bus} {} {disk_idx}", capitalize(other)),
            }
        }
        "interface" => {
            let mac = child_attr(doc, el, &["mac"], "address").unwrap_or_default();
            let start = mac.len().saturating_sub(9);
            format!("NIC {}", &mac[start..])
        }
        "input" => match a("type").as_deref() {
            Some("tablet") => "Tablet".into(),
            Some("mouse") => "Mouse".into(),
            Some("keyboard") => "Keyboard".into(),
            _ => "Input".into(),
        },
        "serial" => format!("Serial {}", port()),
        "parallel" => format!("Parallel {}", port()),
        "console" => format!("Console {}", port()),
        "channel" => {
            let ctype = a("type").unwrap_or_default();
            let pretty_type = char_pretty_type(&ctype);
            let name =
                child_attr(doc, el, &["target"], "name").and_then(|n| char_pretty_channel_name(&n));
            // Don't print channel name with qemu-vdagent, to avoid
            // ambiguity with the typical spice agent channel
            match name {
                Some(name) if ctype != "qemu-vdagent" => format!("Channel ({name})"),
                _ => format!("Channel {pretty_type}"),
            }
        }
        "graphics" => format!(
            "Display {}",
            graphics_pretty_type_simple(&a("type").unwrap_or_default())
        ),
        "redirdev" => format!(
            "{} Redirector {}",
            disk_pretty_bus(&a("bus").unwrap_or_default()),
            idx + 1
        ),
        "hostdev" => hostdev_pretty_name(doc, el),
        "sound" => format!("Sound {}", a("model").unwrap_or_default()),
        "video" => format!(
            "Video {}",
            video_pretty_model(&child_attr(doc, el, &["model"], "type").unwrap_or_default())
        ),
        "filesystem" => {
            let target = child_attr(doc, el, &["target"], "dir").unwrap_or_default();
            format!("Filesystem {}", target.chars().take(8).collect::<String>())
        }
        "controller" => {
            let desc =
                controller_pretty_desc(&a("type").unwrap_or_default(), a("model").as_deref());
            match a("index") {
                Some(i) => format!("Controller {desc} {i}"),
                None => format!("Controller {desc}"),
            }
        }
        "rng" => {
            let backend = virtinst_xml::resolve_path(doc, el, &["backend"])
                .filter(|b| b.attribute(doc, "model") == Some("random"))
                .map(|b| b.text_content(doc))
                .filter(|t| !t.is_empty());
            match backend {
                Some(dev) => format!("RNG {dev}"),
                None => "RNG".into(),
            }
        }
        "tpm" => match child_attr(doc, el, &["backend", "device"], "path") {
            Some(path) => format!("TPM {path}"),
            None => format!(
                "TPM v{}",
                child_attr(doc, el, &["backend"], "version").unwrap_or_default()
            ),
        },
        "panic" => "Panic Notifier".into(),
        "smartcard" => "Smartcard".into(),
        "vsock" => "VirtIO VSOCK".into(),
        _ => "Watchdog".into(),
    }
}

/// `_icon_for_device`.
fn icon_for_device(doc: &Document, tag: &str, el: Element) -> &'static str {
    let a = |name| attr(doc, el, name);
    match tag {
        "disk" => match a("device").as_deref() {
            Some("cdrom") => "media-optical",
            Some("floppy") => "media-floppy",
            _ => "drive-harddisk",
        },
        "input" => match a("type").as_deref() {
            Some("keyboard") => "input-keyboard",
            Some("tablet") => "input-tablet",
            _ => "input-mouse",
        },
        "redirdev" => "device_usb",
        "hostdev" if a("type").as_deref() == Some("usb") => "device_usb",
        "hostdev" => "device_pci",
        "interface" | "vsock" => "network-idle",
        "graphics" | "video" => "video-display",
        "serial" | "parallel" | "console" | "channel" | "smartcard" => "device_serial",
        "watchdog" | "controller" => "device_pci",
        "sound" => "audio-card",
        "rng" | "panic" => "system-run",
        "tpm" => "device_cpu",
        "filesystem" => "folder",
        _ => "device_pci",
    }
}

/// `get_serialcon_devices`'s two lists, `(xml index, element)`:
/// every `<serial>`, then every `<console>` except a first one that
/// mirrors the first serial (`serial_is_console_dup`).
pub type IndexedElements = Vec<(usize, Element)>;
pub fn serial_console_lists(
    doc: &Document,
    devices: Element,
) -> (IndexedElements, IndexedElements) {
    let all = |tag: &str| -> IndexedElements {
        devices.find_all(doc, tag).into_iter().enumerate().collect()
    };
    let serials = all("serial");
    let mut consoles = all("console");
    if let (Some(&(_, serial)), Some(&(_, console))) = (serials.first(), consoles.first()) {
        let ctarget = child_attr(doc, console, &["target"], "type");
        if attr(doc, console, "type") == attr(doc, serial, "type")
            && ctarget.as_deref().map_or(true, |t| t == "serial")
        {
            consoles.remove(0);
        }
    }
    (serials, consoles)
}

/// `_repopulate_hw_list`'s device half: every device row, in the
/// order upstream lists them (by type, then document order).
pub fn device_rows(x: &DomainXml) -> Vec<HwRow> {
    let doc = &x.doc;
    let Some(devices) = virtinst_xml::resolve_path(doc, x.root(), &["devices"]) else {
        return vec![];
    };
    let all = |tag: &str| -> Vec<(usize, Element)> {
        devices.find_all(doc, tag).into_iter().enumerate().collect()
    };

    let mut rows = vec![];
    let mut push = |hwtype, tag: &str, idx, el, disk_idx| {
        rows.push(HwRow {
            label: label_for_device(doc, tag, idx, el, disk_idx),
            icon: icon_for_device(doc, tag, el).into(),
            hwtype,
            device: Some((tag.to_string(), idx)),
        });
    };

    // _calculate_disk_bus_index
    let mut counts: Vec<(String, usize)> = vec![];
    for (idx, el) in all("disk") {
        let key = attr(doc, el, "device").unwrap_or_else(|| "disk".into())
            + &child_attr(doc, el, &["target"], "bus").unwrap_or_default();
        let n = match counts.iter_mut().find(|(k, _)| *k == key) {
            Some((_, n)) => {
                *n += 1;
                *n
            }
            None => {
                counts.push((key, 1));
                1
            }
        };
        push(HwType::Disk, "disk", idx, el, n);
    }

    let (serials, consoles) = serial_console_lists(doc, devices);

    for (tag, hwtype) in [
        ("interface", HwType::Nic),
        ("input", HwType::Input),
        ("graphics", HwType::Graphics),
        ("sound", HwType::Sound),
    ] {
        for (idx, el) in all(tag) {
            push(hwtype, tag, idx, el, 0);
        }
    }
    for (idx, el) in serials {
        push(HwType::Char, "serial", idx, el, 0);
    }
    for (idx, el) in all("parallel") {
        push(HwType::Char, "parallel", idx, el, 0);
    }
    for (idx, el) in consoles {
        push(HwType::Char, "console", idx, el, 0);
    }
    for (tag, hwtype) in [
        ("channel", HwType::Char),
        ("hostdev", HwType::Hostdev),
        ("redirdev", HwType::Redirdev),
        ("video", HwType::Video),
        ("watchdog", HwType::Watchdog),
    ] {
        for (idx, el) in all(tag) {
            push(hwtype, tag, idx, el, 0);
        }
    }
    for (idx, el) in all("controller") {
        let model = attr(doc, el, "model").unwrap_or_default();
        // skip USB2 ICH9 companion controllers, and the parts of a
        // default PCIe setup that upstream condenses to one listing
        if matches!(
            model.as_str(),
            "ich9-uhci1"
                | "ich9-uhci2"
                | "ich9-uhci3"
                | "pcie-root-port"
                | "dmi-to-pci-bridge"
                | "pci-bridge"
        ) {
            continue;
        }
        push(HwType::Controller, "controller", idx, el, 0);
    }
    for (tag, hwtype) in [
        ("filesystem", HwType::Filesystem),
        ("smartcard", HwType::Smartcard),
        ("tpm", HwType::Tpm),
        ("rng", HwType::Rng),
        ("panic", HwType::Panic),
        ("vsock", HwType::Vsock),
    ] {
        for (idx, el) in all(tag) {
            push(hwtype, tag, idx, el, 0);
        }
    }
    // upstream inserts each new row after every row of a lower or equal
    // type: a stable sort by type gives the same order.
    rows.sort_by_key(|r| r.hwtype);
    rows
}

/// `_chipset_label_from_machine`.
fn chipset_label_from_machine(machine: Option<&str>) -> &'static str {
    if machine.is_some_and(|m| m.contains("q35")) {
        "Q35"
    } else {
        "i440FX"
    }
}

/// `DomainCapabilities._uefi_arch_patterns`, as substring/suffix checks
/// equivalent to upstream's anchored regexes.
fn uefi_arch_for_path(path: &str) -> Option<&'static str> {
    let file = |pre: &str, post: &str| {
        path.find(pre)
            .is_some_and(|i| path[i + pre.len()..].ends_with(post) || post.is_empty())
    };
    let i686 = file("edk2-i386-", ".fd") || path.contains("ovmf-ia32");
    let x86_64 = file("edk2-x86_64-", ".fd")
        || path.ends_with("OVMF_CODE.fd")
        || (path.contains("ovmf-x64/OVMF") && path.ends_with(".fd"))
        || path.contains("ovmf-x86_64-")
        || path.contains("ovmf")
        || path.contains("OVMF");
    let aarch64 = path.ends_with("AAVMF_CODE.fd")
        || path.contains("aarch64/QEMU_EFI")
        || path.contains("aarch64")
        || path.ends_with("edk2-aarch64-code.fd");
    let armv7l = path.ends_with("AAVMF32_CODE.fd")
        || path.contains("arm/QEMU_EFI")
        || path.ends_with("edk2-arm-code.fd");
    let riscv64 = path.contains("RISCV_VIRT_CODE.") || path.contains("riscv64");
    let loongarch64 = path.contains("loongarch64/QEMU_CODE.") || path.contains("loongarch64");
    [
        ("i686", i686),
        ("x86_64", x86_64),
        ("aarch64", aarch64),
        ("armv7l", armv7l),
        ("riscv64", riscv64),
        ("loongarch64", loongarch64),
    ]
    .into_iter()
    .find(|(_, hit)| *hit)
    .map(|(arch, _)| arch)
}

/// `DomainCapabilities.arch_can_uefi`.
fn arch_can_uefi(arch: Option<&str>) -> bool {
    matches!(
        arch,
        Some("i686" | "x86_64" | "aarch64" | "armv7l" | "riscv64" | "loongarch64")
    )
}

/// `DomainCapabilities.label_for_firmware_path`.
pub fn label_for_firmware_path(arch: Option<&str>, path: Option<&str>) -> String {
    let Some(path) = path.filter(|p| !p.is_empty()) else {
        return if matches!(arch, Some("i686" | "x86_64")) {
            "BIOS".into()
        } else {
            "Default".into()
        };
    };
    match uefi_arch_for_path(path) {
        Some(a) => format!("UEFI {a}: {path}"),
        None => format!("Custom: {path}"),
    }
}

fn grid_label(text: &str) -> gtk::Label {
    let label = gtk::Label::with_mnemonic(text);
    label.set_xalign(1.0);
    label
}

fn value_label() -> gtk::Label {
    let label = gtk::Label::new(None);
    label.set_xalign(0.0);
    label.set_selectable(true);
    label
}

/// A glade frame with shadow none: bold title, indented child.
fn frame(title: &str, child: &impl IsA<gtk::Widget>) -> gtk::Box {
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let label = gtk::Label::new(None);
    label.set_markup_with_mnemonic(&format!("<b>{title}</b>"));
    label.set_xalign(0.0);
    vbox.append(&label);
    child.set_margin_start(12);
    vbox.append(child);
    vbox
}

/// The Overview page's widgets (`overview-*`, `machine-type*`).
struct Overview {
    page: gtk::Box,
    name: gtk::Entry,
    uuid: gtk::Label,
    status_icon: gtk::Image,
    status_text: gtk::Label,
    title: gtk::Entry,
    description: gtk::TextView,
    hv: gtk::Label,
    arch: gtk::Label,
    emulator: gtk::Label,
    machine_title: gtk::Label,
    machine_label: gtk::Label,
    chipset_title: gtk::Label,
    chipset_label: gtk::Label,
    firmware_title: gtk::Label,
    firmware_label: gtk::Label,
}

impl Overview {
    fn new() -> Overview {
        let name = gtk::Entry::new();
        name.set_hexpand(true);
        let uuid = value_label();
        let status_icon = gtk::Image::new();
        let status_text = gtk::Label::new(Some("Shut down"));
        let status_box = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        status_box.append(&status_icon);
        status_box.append(&status_text);
        let title = gtk::Entry::new();
        let description = gtk::TextView::new();
        description.set_wrap_mode(gtk::WrapMode::Word);
        let desc_scroll = gtk::ScrolledWindow::builder()
            .child(&description)
            .min_content_height(80)
            .has_frame(true)
            .build();

        let basic = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(12)
            .build();
        let name_label = grid_label("_Name:");
        name_label.set_mnemonic_widget(Some(&name));
        let title_label = grid_label("T_itle:");
        title_label.set_mnemonic_widget(Some(&title));
        let desc_label = grid_label("D_escription:");
        desc_label.set_mnemonic_widget(Some(&description));
        desc_label.set_valign(gtk::Align::Start);
        basic.attach(&name_label, 0, 0, 1, 1);
        basic.attach(&name, 1, 0, 1, 1);
        basic.attach(&grid_label("UUID:"), 0, 1, 1, 1);
        basic.attach(&uuid, 1, 1, 1, 1);
        basic.attach(&grid_label("Status:"), 0, 2, 1, 1);
        basic.attach(&status_box, 1, 2, 1, 1);
        basic.attach(&title_label, 0, 3, 1, 1);
        basic.attach(&title, 1, 3, 1, 1);
        basic.attach(&desc_label, 0, 4, 1, 1);
        basic.attach(&desc_scroll, 1, 4, 1, 1);

        let hv = value_label();
        let arch = value_label();
        let emulator = value_label();
        let machine_title = grid_label("Machine _Type: ");
        let machine_label = value_label();
        let chipset_title = grid_label("Chipse_t:");
        let chipset_label = value_label();
        let firmware_title = grid_label("Firm_ware:");
        let firmware_label = value_label();
        let hvgrid = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(12)
            .build();
        hvgrid.attach(&grid_label("Hypervisor:"), 0, 0, 1, 1);
        hvgrid.attach(&hv, 1, 0, 1, 1);
        hvgrid.attach(&grid_label("Architecture:"), 0, 1, 1, 1);
        hvgrid.attach(&arch, 1, 1, 1, 1);
        hvgrid.attach(&grid_label("Emulator:"), 0, 2, 1, 1);
        hvgrid.attach(&emulator, 1, 2, 1, 1);
        hvgrid.attach(&machine_title, 0, 3, 1, 1);
        hvgrid.attach(&machine_label, 1, 3, 1, 1);
        hvgrid.attach(&chipset_title, 0, 4, 1, 1);
        hvgrid.attach(&chipset_label, 1, 4, 1, 1);
        hvgrid.attach(&firmware_title, 0, 5, 1, 1);
        hvgrid.attach(&firmware_label, 1, 5, 1, 1);

        let page = gtk::Box::new(gtk::Orientation::Vertical, 18);
        page.append(&frame("Basic Details", &basic));
        page.append(&frame("Hypervisor Details", &hvgrid));

        Overview {
            page,
            name,
            uuid,
            status_icon,
            status_text,
            title,
            description,
            hv,
            arch,
            emulator,
            machine_title,
            machine_label,
            chipset_title,
            chipset_label,
            firmware_title,
            firmware_label,
        }
    }

    /// `uiutil.set_grid_row_visible` for one title/value pair.
    fn set_row_visible(title: &gtk::Label, value: &gtk::Label, visible: bool) {
        title.set_visible(visible);
        value.set_visible(visible);
    }
}

/// A `vm.hotplug(...)` call for [`VmmDetails::change_config`].
type Hotplug = std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>>>>;

/// `uiutil.spin_get_helper`: the typed text if it parses, else the
/// adjustment's value.
fn spin_get(spin: &gtk::SpinButton) -> f64 {
    spin.text()
        .trim()
        .parse::<f64>()
        .unwrap_or_else(|_| spin.value())
}

fn spin(lower: f64, upper: f64, page: f64) -> gtk::SpinButton {
    let adj = gtk::Adjustment::new(lower, lower, upper, 1.0, page, 0.0);
    let spin = gtk::SpinButton::new(Some(&adj), 1.0, 0);
    spin.set_numeric(true);
    spin.set_halign(gtk::Align::Start);
    spin
}

fn left_label(text: &str) -> gtk::Label {
    let label = gtk::Label::with_mnemonic(text);
    label.set_xalign(0.0);
    label.set_halign(gtk::Align::Start);
    label
}

/// One row of the `cpu-model` combo: `[label, sortkey, idstring]`;
/// `None` is the separator.
type CpuModelRow = Option<(String, String, String)>;

/// The `cpu-model` combo with its entry: GTK4 has no non-deprecated
/// editable combo, so this is an entry plus a drop-down button whose
/// popover lists the rows.
struct CpuModelCombo {
    widget: gtk::Box,
    entry: gtk::Entry,
    list: gtk::ListBox,
    popover: gtk::Popover,
    rows: RefCell<Vec<CpuModelRow>>,
    /// Whether the domcaps models are in.
    loaded: Cell<bool>,
}

impl CpuModelCombo {
    fn new() -> Rc<CpuModelCombo> {
        let entry = gtk::Entry::new();
        entry.set_width_chars(25);
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        let scroll = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(400)
            .build();
        let popover = gtk::Popover::new();
        popover.set_child(Some(&scroll));
        let button = gtk::MenuButton::new();
        button.set_popover(Some(&popover));
        let widget = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        widget.add_css_class("linked");
        widget.append(&entry);
        widget.append(&button);
        widget.set_halign(gtk::Align::Start);
        entry.update_property(&[gtk::accessible::Property::Label("CPU model")]);
        let this = Rc::new(CpuModelCombo {
            widget,
            entry,
            list,
            popover,
            rows: RefCell::default(),
            loaded: Cell::new(false),
        });
        let weak = Rc::downgrade(&this);
        this.list.connect_row_activated(move |_, row| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            let label = this
                .rows
                .borrow()
                .get(row.index() as usize)
                .and_then(|r| r.as_ref().map(|(label, _, _)| label.clone()));
            if let Some(label) = label {
                this.entry.set_text(&label);
            }
            this.popover.popdown();
        });
        this.set_rows(vec![]);
        this
    }

    /// `_init_details`' model: the special modes, a separator, then
    /// the domcaps models, sorted by sort key.
    fn set_rows(&self, models: Vec<String>) {
        let special = [
            ("Application Default", "01", cpu::SPECIAL_MODE_APP_DEFAULT),
            ("Hypervisor Default", "02", cpu::SPECIAL_MODE_HV_DEFAULT),
            ("Clear CPU configuration", "03", cpu::SPECIAL_MODE_CLEAR),
            ("host-model", "04", cpu::SPECIAL_MODE_HOST_MODEL),
            ("host-passthrough", "05", cpu::SPECIAL_MODE_HOST_PASSTHROUGH),
            ("maximum", "06", cpu::SPECIAL_MODE_MAXIMUM),
        ];
        let mut rows: Vec<CpuModelRow> = special
            .iter()
            .map(|(l, k, id)| Some((l.to_string(), k.to_string(), id.to_string())))
            .collect();
        let mut models: Vec<_> = models
            .into_iter()
            .map(|m| Some((m.clone(), m.clone(), m)))
            .collect();
        models.sort_by_key(|r| r.as_ref().map(|(_, k, _)| k.to_lowercase()));
        rows.push(None);
        rows.extend(models);

        self.list.remove_all();
        for row in &rows {
            let child: gtk::Widget = match row {
                Some((label, _, _)) => {
                    let l = gtk::Label::new(Some(label));
                    l.set_xalign(0.0);
                    l.upcast()
                }
                None => gtk::Separator::new(gtk::Orientation::Horizontal).upcast(),
            };
            let lbrow = gtk::ListBoxRow::builder().child(&child).build();
            lbrow.set_activatable(row.is_some());
            lbrow.set_selectable(false);
            self.list.append(&lbrow);
        }
        self.rows.replace(rows);
    }

    fn text(&self) -> String {
        self.entry.text().to_string()
    }

    /// The row id for the entry's text, if it is a row's label.
    fn key_for_text(&self) -> Option<String> {
        let text = self.text();
        self.rows
            .borrow()
            .iter()
            .flatten()
            .find(|(label, _, _)| *label == text)
            .map(|(_, _, id)| id.clone())
    }

    /// `uiutil.set_list_selection(..., column=2)`.
    fn select_id(&self, id: &str) {
        let label = self
            .rows
            .borrow()
            .iter()
            .flatten()
            .find(|(_, _, i)| i == id)
            .map(|(label, _, _)| label.clone());
        if let Some(label) = label {
            self.entry.set_text(&label);
        }
    }
}

/// The CPUs page (`cpu-*`, `state-host-cpus`).
struct CpuPage {
    page: gtk::Box,
    host_cpus: gtk::Label,
    vcpus: gtk::SpinButton,
    vcpus_warn_box: gtk::Box,
    copy_host: gtk::CheckButton,
    model_label: gtk::Label,
    model: Rc<CpuModelCombo>,
    secure: gtk::CheckButton,
    topology_expander: gtk::Expander,
    topology_enable: gtk::CheckButton,
    topology_table: gtk::Grid,
    sockets: gtk::SpinButton,
    cores: gtk::SpinButton,
    threads: gtk::SpinButton,
}

impl CpuPage {
    fn new() -> CpuPage {
        let host_cpus = left_label("8");
        let vcpus = spin(1.0, 1024.0, 2.0);
        vcpus.set_hexpand(true);
        let vcpus_label = left_label("vCPU a_llocation:");
        vcpus_label.set_mnemonic_widget(Some(&vcpus));
        let warn_icon = gtk::Image::from_icon_name("dialog-warning");
        let warn_text = gtk::Label::new(None);
        warn_text.set_markup("<small>Overcommitting vCPUs can hurt performance</small>");
        let vcpus_warn_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        vcpus_warn_box.set_halign(gtk::Align::Start);
        vcpus_warn_box.append(&warn_icon);
        vcpus_warn_box.append(&warn_text);
        let cpus_grid = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(12)
            .build();
        cpus_grid.attach(&left_label("Logical host CPUs:"), 0, 0, 1, 1);
        cpus_grid.attach(&host_cpus, 1, 0, 1, 1);
        cpus_grid.attach(&vcpus_label, 0, 1, 1, 1);
        cpus_grid.attach(&vcpus, 1, 1, 1, 1);
        cpus_grid.attach(&vcpus_warn_box, 0, 2, 2, 1);

        let copy_host = gtk::CheckButton::with_mnemonic("Copy host CP_U configuration");
        let model = CpuModelCombo::new();
        model.widget.set_hexpand(true);
        let model_label = left_label("M_odel:");
        model_label.set_mnemonic_widget(Some(&model.entry));
        let secure = gtk::CheckButton::with_label("Enable available CPU security flaw mitigations");
        let config_grid = gtk::Grid::builder()
            .row_spacing(3)
            .column_spacing(12)
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Start)
            .build();
        config_grid.attach(&copy_host, 0, 0, 2, 1);
        config_grid.attach(&model_label, 0, 1, 1, 1);
        config_grid.attach(&model.widget, 1, 1, 1, 1);
        config_grid.attach(&secure, 0, 2, 2, 1);

        let topology_enable = gtk::CheckButton::with_mnemonic("Manuall_y set CPU topology");
        let sockets = spin(1.0, 256.0, 10.0);
        let cores = spin(1.0, 256.0, 10.0);
        let threads = spin(1.0, 256.0, 10.0);
        let topology_table = gtk::Grid::builder()
            .row_spacing(4)
            .column_spacing(12)
            .build();
        for (i, (text, spin)) in [
            ("Socke_ts:", &sockets),
            ("Cor_es:", &cores),
            ("Thread_s:", &threads),
        ]
        .into_iter()
        .enumerate()
        {
            let label = left_label(text);
            label.set_mnemonic_widget(Some(spin));
            topology_table.attach(&label, 0, i as i32, 1, 1);
            topology_table.attach(spin, 1, i as i32, 1, 1);
        }
        let topology_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        topology_box.set_margin_start(12);
        topology_box.append(&topology_enable);
        topology_box.append(&topology_table);
        let topology_expander = gtk::Expander::new(None);
        let exp_label = gtk::Label::new(None);
        exp_label.set_markup_with_mnemonic("<b>To_pology</b>");
        topology_expander.set_label_widget(Some(&exp_label));
        topology_expander.set_child(Some(&topology_box));

        let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
        page.append(&frame("CPUs", &cpus_grid));
        page.append(&frame("Configu_ration", &config_grid));
        page.append(&topology_expander);

        CpuPage {
            page,
            host_cpus,
            vcpus,
            vcpus_warn_box,
            copy_host,
            model_label,
            model,
            secure,
            topology_expander,
            topology_enable,
            topology_table,
            sockets,
            cores,
            threads,
        }
    }
}

/// The Memory page (`mem-*`, `state-host-memory`, `shared-memory`).
struct MemoryPage {
    page: gtk::Box,
    host_memory: gtk::Label,
    memory: gtk::SpinButton,
    maxmem: gtk::SpinButton,
    shared: gtk::CheckButton,
}

impl MemoryPage {
    fn new() -> MemoryPage {
        let host_memory = left_label("2 GiB");
        let memory = spin(1.0, 10240000.0, 25.0);
        let maxmem = spin(1.0, 10240000.0, 25.0);
        let shared = gtk::CheckButton::with_mnemonic("Enable shared _memory");
        shared.set_halign(gtk::Align::Start);
        let grid = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(6)
            .valign(gtk::Align::Start)
            .build();
        let right = |text: &str| {
            let l = gtk::Label::with_mnemonic(text);
            l.set_halign(gtk::Align::End);
            l
        };
        let with_unit = |spin: &gtk::SpinButton| {
            let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 3);
            hbox.append(spin);
            hbox.append(&left_label("MiB"));
            hbox
        };
        let cur_label = right("Current a_llocation:");
        cur_label.set_mnemonic_widget(Some(&memory));
        let max_label = right("Ma_ximum allocation:");
        max_label.set_mnemonic_widget(Some(&maxmem));
        grid.attach(&right("Total host memory:"), 0, 0, 1, 1);
        grid.attach(&host_memory, 1, 0, 1, 1);
        grid.attach(&cur_label, 0, 1, 1, 1);
        let cur_box = with_unit(&memory);
        cur_box.set_hexpand(true);
        grid.attach(&cur_box, 1, 1, 1, 1);
        grid.attach(&max_label, 0, 2, 1, 1);
        grid.attach(&with_unit(&maxmem), 1, 2, 1, 1);
        grid.attach(&shared, 1, 3, 1, 1);

        let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        page.append(&frame("Memory", &grid));
        MemoryPage {
            page,
            host_memory,
            memory,
            maxmem,
            shared,
        }
    }
}

/// One `boot-list` row: `[XML boot type, display name, icon name,
/// enabled, can select]`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct BootRow {
    key: Option<String>,
    label: String,
    icon: Option<String>,
    active: bool,
    can_select: bool,
}

/// An entry with its Browse button (`boot-kernel` + `boot-kernel-browse`).
fn path_entry() -> (gtk::Box, gtk::Entry, gtk::Button) {
    let entry = gtk::Entry::new();
    entry.set_hexpand(true);
    let browse = gtk::Button::with_label("Browse");
    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    hbox.set_hexpand(true);
    hbox.append(&entry);
    hbox.append(&browse);
    (hbox, entry, browse)
}

/// `vmmAddStorage`'s `_EDIT_*`: which advanced disk options changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StorageEdit {
    Cache,
    Discard,
    Readonly,
    Shareable,
    Removable,
    Serial,
}

/// A "Hypervisor default" plus values drop-down (`build_simple_combo`
/// with a `None` first row).
struct ModeCombo {
    widget: gtk::DropDown,
    values: RefCell<Vec<Option<String>>>,
}

impl ModeCombo {
    fn new(modes: &[&str]) -> ModeCombo {
        let mut values = vec![None];
        values.extend(modes.iter().map(|m| Some(m.to_string())));
        let labels: Vec<&str> = std::iter::once("Hypervisor default")
            .chain(modes.iter().copied())
            .collect();
        let widget = gtk::DropDown::from_strings(&labels);
        widget.set_halign(gtk::Align::Start);
        ModeCombo {
            widget,
            values: RefCell::new(values),
        }
    }

    /// Replaces the listed values, keeping "Hypervisor default" first.
    fn set_modes(&self, modes: &[&str]) {
        let mut values = vec![None];
        values.extend(modes.iter().map(|m| Some(m.to_string())));
        let labels: Vec<&str> = std::iter::once("Hypervisor default")
            .chain(modes.iter().copied())
            .collect();
        self.widget.set_model(Some(&gtk::StringList::new(&labels)));
        self.values.replace(values);
    }

    fn get(&self) -> Option<String> {
        self.values
            .borrow()
            .get(self.widget.selected() as usize)
            .cloned()
            .flatten()
    }

    /// `set_list_selection`, adding a row for a value not listed.
    fn set(&self, val: Option<&str>) {
        let val = val.map(str::to_string);
        let idx = self.values.borrow().iter().position(|v| *v == val);
        let idx = idx.unwrap_or_else(|| {
            let label = val.clone().unwrap_or_default();
            if let Some(model) = self.widget.model().and_downcast::<gtk::StringList>() {
                model.append(&label);
            }
            self.values.borrow_mut().push(val);
            self.values.borrow().len() - 1
        });
        self.widget.set_selected(idx as u32);
    }
}

/// The disk page (`disk-*`) with `vmmAddStorage`'s advanced options.
struct DiskPage {
    page: gtk::Box,
    source_label: gtk::Label,
    source_box: gtk::Box,
    browse: gtk::Button,
    mediacombo: Rc<crate::mediacombo::MediaCombo>,
    target_type: gtk::Label,
    bus_label: gtk::Label,
    size: gtk::Label,
    readonly: gtk::CheckButton,
    shareable: gtk::CheckButton,
    removable: gtk::CheckButton,
    removable_label: gtk::Label,
    serial: gtk::Entry,
    cache: ModeCombo,
    discard: ModeCombo,
    storage_edits: RefCell<Vec<StorageEdit>>,
}

impl DiskPage {
    fn new(conn: Option<crate::connection::VmmConnection>) -> DiskPage {
        let right = |text: &str| {
            let l = gtk::Label::with_mnemonic(text);
            l.set_halign(gtk::Align::End);
            l
        };
        let value = || {
            let l = left_label("");
            l.set_selectable(true);
            l
        };
        let source_mnemonic = right("Source _path:");
        let source_label = value();
        source_label.set_ellipsize(gtk::pango::EllipsizeMode::Start);
        source_label.update_property(&[gtk::accessible::Property::Label("disk-source-label")]);
        let mediacombo = crate::mediacombo::MediaCombo::new(conn);
        mediacombo.set_mnemonic_label(&source_mnemonic);
        mediacombo.show_clear_icon();
        let browse = gtk::Button::with_mnemonic("_Browse");
        let source_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        source_box.set_hexpand(true);
        source_box.append(&mediacombo.top_box);
        source_box.append(&browse);
        let source = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        source.set_hexpand(true);
        source.append(&source_label);
        source.append(&source_box);
        let target_type = value();
        let bus_label = left_label("");
        let size = left_label("");

        let grid = gtk::Grid::builder()
            .row_spacing(4)
            .column_spacing(8)
            .build();
        grid.attach(&source_mnemonic, 0, 0, 1, 1);
        grid.attach(&source, 1, 0, 1, 1);
        grid.attach(&right("Device type:"), 0, 1, 1, 1);
        grid.attach(&target_type, 1, 1, 1, 1);
        // The bus is only editable in the customize dialog.
        grid.attach(&right("Disk bus:"), 0, 2, 1, 1);
        grid.attach(&bus_label, 1, 2, 1, 1);
        grid.attach(&right("Storage size:"), 0, 3, 1, 1);
        grid.attach(&size, 1, 3, 1, 1);

        let readonly = gtk::CheckButton::new();
        let shareable = gtk::CheckButton::new();
        let removable = gtk::CheckButton::new();
        let serial = gtk::Entry::new();
        let cache = ModeCombo::new(&virtinst_core::devices::disk::CACHE_MODES);
        let discard = ModeCombo::new(&virtinst_core::devices::disk::DISCARD_MODES);
        let adv = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(6)
            .margin_start(12)
            .margin_top(6)
            .build();
        let mut row = 0;
        let mut add = |text: &str, w: &gtk::Widget| {
            let l = right(text);
            l.set_mnemonic_widget(Some(w));
            adv.attach(&l, 0, row, 1, 1);
            adv.attach(w, 1, row, 1, 1);
            row += 1;
            l
        };
        add("R_eadonly:", readonly.upcast_ref());
        add("Sharea_ble:", shareable.upcast_ref());
        let removable_label = add("Removab_le:", removable.upcast_ref());
        add("Seria_l:", serial.upcast_ref());
        add("Cac_he mode:", cache.widget.upcast_ref());
        add("Discard mod_e:", discard.widget.upcast_ref());
        let advanced = gtk::Expander::with_mnemonic("Advanced _options");
        advanced.set_child(Some(&adv));

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 12);
        vbox.append(&grid);
        vbox.append(&advanced);
        let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
        page.append(&frame("Virtual Disk", &vbox));
        DiskPage {
            page,
            source_label,
            source_box,
            browse,
            mediacombo,
            target_type,
            bus_label,
            size,
            readonly,
            shareable,
            removable,
            removable_label,
            serial,
            cache,
            discard,
            storage_edits: RefCell::default(),
        }
    }

    /// `vmmAddStorage.set_disk_bus`: removable only applies to USB.
    fn set_disk_bus(&self, bus: Option<&str>) {
        let show = bus == Some("usb");
        self.removable.set_visible(show);
        self.removable_label.set_visible(show);
    }

    /// `vmmAddStorage.get_values`.
    fn get_values(&self, edits: &mut crate::domain::DiskEdits) {
        for e in self.storage_edits.borrow().iter() {
            match e {
                StorageEdit::Cache => edits.cache = Some(self.cache.get()),
                StorageEdit::Discard => edits.discard = Some(self.discard.get()),
                StorageEdit::Readonly => edits.readonly = Some(self.readonly.is_active()),
                StorageEdit::Shareable => edits.shareable = Some(self.shareable.is_active()),
                StorageEdit::Removable => edits.removable = Some(self.removable.is_active()),
                StorageEdit::Serial => edits.serial = Some(Some(self.serial.text().to_string())),
            }
        }
    }
}

/// The network page (`network-*`).
struct NetworkPage {
    page: gtk::Box,
    netlist: Rc<crate::netlist::NetList>,
    model: ModeCombo,
    mac: gtk::Label,
    ip: gtk::Label,
    refresh_ip: gtk::Button,
    link_state: gtk::CheckButton,
}

impl NetworkPage {
    fn new(conn: Option<crate::connection::VmmConnection>) -> NetworkPage {
        let right = |text: &str| {
            let l = gtk::Label::with_mnemonic(text);
            l.set_halign(gtk::Align::End);
            l
        };
        let netlist = crate::netlist::NetList::new(conn);
        let model = ModeCombo::new(&[]);
        let mac = left_label("");
        mac.set_selectable(true);
        let ip = left_label("");
        ip.set_selectable(true);
        let refresh_ip = gtk::Button::from_icon_name("view-refresh");
        refresh_ip.set_halign(gtk::Align::Start);
        refresh_ip.set_valign(gtk::Align::Center);
        refresh_ip.update_property(&[gtk::accessible::Property::Label("refresh-ip")]);
        let ip_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        ip_box.append(&ip);
        ip_box.append(&refresh_ip);
        let link_state = gtk::CheckButton::with_label("active");
        link_state.set_halign(gtk::Align::Start);

        let grid = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(12)
            .build();
        grid.attach(&netlist.top_label, 0, 0, 1, 1);
        grid.attach(&netlist.top_box, 1, 0, 1, 1);
        let model_label = right("Device mode_l:");
        model_label.set_mnemonic_widget(Some(&model.widget));
        grid.attach(&model_label, 0, 1, 1, 1);
        grid.attach(&model.widget, 1, 1, 1, 1);
        // The MAC is only editable in the customize dialog.
        grid.attach(&right("MAC address:"), 0, 2, 1, 1);
        grid.attach(&mac, 1, 2, 1, 1);
        let ip_label = right("I_P address:");
        ip_label.set_mnemonic_widget(Some(&refresh_ip));
        grid.attach(&ip_label, 0, 3, 1, 1);
        grid.attach(&ip_box, 1, 3, 1, 1);
        let link_label = right("Link _state:");
        link_label.set_mnemonic_widget(Some(&link_state));
        grid.attach(&link_label, 0, 4, 1, 1);
        grid.attach(&link_state, 1, 4, 1, 1);

        let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
        page.append(&frame("Virtual Network Interface", &grid));
        NetworkPage {
            page,
            netlist,
            model,
            mac,
            ip,
            refresh_ip,
            link_state,
        }
    }
}

/// The OS information page (`details-os-*`).
struct OsPage {
    page: gtk::Box,
    os_list: Rc<crate::oslist::OsList>,
}

impl OsPage {
    fn new() -> OsPage {
        let os_list = crate::oslist::OsList::new();
        let align = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        align.set_hexpand(true);
        align.set_margin_top(3);
        align.set_margin_bottom(3);
        align.append(&os_list.search_entry);
        let os_frame = frame("Operating Sys_tem", &align);
        if let Some(label) = os_frame.first_child().and_downcast::<gtk::Label>() {
            label.set_mnemonic_widget(Some(&os_list.search_entry));
        }
        let page = gtk::Box::new(gtk::Orientation::Vertical, 18);
        page.append(&os_frame);
        OsPage { page, os_list }
    }
}

/// The Boot Options page (`boot-*`).
struct BootPage {
    page: gtk::Grid,
    autostart: gtk::CheckButton,
    order_frame: gtk::Box,
    menu: gtk::CheckButton,
    list: gtk::ListBox,
    rows: RefCell<Vec<BootRow>>,
    moveup: gtk::Button,
    movedown: gtk::Button,
    kernel_expander: gtk::Expander,
    kernel_enable: gtk::CheckButton,
    kernel_box: gtk::Box,
    kernel: gtk::Entry,
    kernel_browse: gtk::Button,
    initrd: gtk::Entry,
    initrd_browse: gtk::Button,
    dtb_label: gtk::Label,
    dtb_box: gtk::Box,
    dtb: gtk::Entry,
    dtb_browse: gtk::Button,
    kernel_args: gtk::Entry,
    init_frame: gtk::Box,
    init_path: gtk::Entry,
    init_args: gtk::Entry,
}

impl BootPage {
    fn new() -> BootPage {
        let autostart = gtk::CheckButton::with_mnemonic("Start virt_ual machine on host boot up");
        autostart.set_halign(gtk::Align::Start);

        let menu = gtk::CheckButton::with_mnemonic("Enable boot me_nu");
        menu.set_halign(gtk::Align::Start);
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        list.update_property(&[gtk::accessible::Property::Label("Boot device order")]);
        let scroll = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .has_frame(true)
            .width_request(125)
            .height_request(125)
            .propagate_natural_width(true)
            .build();
        let moveup = gtk::Button::from_icon_name("go-up");
        moveup.update_property(&[gtk::accessible::Property::Label("Move up")]);
        moveup.set_sensitive(false);
        let movedown = gtk::Button::from_icon_name("go-down");
        movedown.update_property(&[gtk::accessible::Property::Label("Move down")]);
        movedown.set_sensitive(false);
        let buttons = gtk::Box::new(gtk::Orientation::Vertical, 6);
        buttons.append(&moveup);
        buttons.append(&movedown);
        let table = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(6)
            .halign(gtk::Align::Start)
            .build();
        table.attach(&scroll, 0, 0, 1, 1);
        table.attach(&buttons, 1, 0, 1, 1);
        let order_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        order_box.append(&menu);
        order_box.append(&table);

        let kernel_enable = gtk::CheckButton::with_mnemonic("Ena_ble direct kernel boot");
        kernel_enable.set_halign(gtk::Align::Start);
        let (kernel_hbox, kernel, kernel_browse) = path_entry();
        let (initrd_hbox, initrd, initrd_browse) = path_entry();
        let (dtb_box, dtb, dtb_browse) = path_entry();
        let kernel_args = gtk::Entry::new();
        kernel_args.set_hexpand(true);
        let ktable = gtk::Grid::builder()
            .column_spacing(6)
            .row_spacing(6)
            .build();
        ktable.set_hexpand(true);
        let dtb_label = left_label("D_TB path:");
        dtb_label.set_mnemonic_widget(Some(&dtb));
        for (row, (text, widget, mnemonic)) in [
            (
                "Ke_rnel path:",
                kernel_hbox.upcast_ref::<gtk::Widget>(),
                &kernel,
            ),
            ("_Initrd path:", initrd_hbox.upcast_ref(), &initrd),
        ]
        .into_iter()
        .enumerate()
        {
            let label = left_label(text);
            label.set_mnemonic_widget(Some(mnemonic));
            ktable.attach(&label, 0, row as i32, 1, 1);
            ktable.attach(widget, 1, row as i32, 1, 1);
        }
        ktable.attach(&dtb_label, 0, 2, 1, 1);
        ktable.attach(&dtb_box, 1, 2, 1, 1);
        let args_label = left_label("Kernel ar_gs:");
        args_label.set_mnemonic_widget(Some(&kernel_args));
        ktable.attach(&args_label, 0, 3, 1, 1);
        ktable.attach(&kernel_args, 1, 3, 1, 1);
        let kernel_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        kernel_box.append(&ktable);
        let kernel_vbox = gtk::Box::new(gtk::Orientation::Vertical, 6);
        kernel_vbox.set_margin_start(12);
        kernel_vbox.append(&kernel_enable);
        kernel_vbox.append(&kernel_box);
        let kernel_expander = gtk::Expander::new(None);
        let exp_label = gtk::Label::new(None);
        exp_label.set_markup_with_mnemonic("<b>Dir_ect kernel boot</b>");
        kernel_expander.set_label_widget(Some(&exp_label));
        kernel_expander.set_child(Some(&kernel_vbox));

        let init_path = gtk::Entry::new();
        let init_args = gtk::Entry::new();
        let igrid = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(6)
            .build();
        for (row, (text, entry)) in [("Init _path:", &init_path), ("Init ar_gs:", &init_args)]
            .into_iter()
            .enumerate()
        {
            let label = left_label(text);
            label.set_mnemonic_widget(Some(entry));
            igrid.attach(&label, 0, row as i32, 1, 1);
            igrid.attach(entry, 1, row as i32, 1, 1);
        }

        let page = gtk::Grid::builder().row_spacing(24).build();
        let order_frame = frame("Boot device order", &order_box);
        let init_frame = frame("Container init", &igrid);
        page.attach(&frame("Autostart", &autostart), 0, 0, 1, 1);
        page.attach(&order_frame, 0, 1, 1, 1);
        page.attach(&kernel_expander, 0, 2, 1, 1);
        page.attach(&init_frame, 0, 3, 1, 1);

        BootPage {
            page,
            autostart,
            order_frame,
            menu,
            list,
            rows: RefCell::default(),
            moveup,
            movedown,
            kernel_expander,
            kernel_enable,
            kernel_box,
            kernel,
            kernel_browse,
            initrd,
            initrd_browse,
            dtb_label,
            dtb_box,
            dtb,
            dtb_browse,
            kernel_args,
            init_frame,
            init_path,
            init_args,
        }
    }
}

struct Inner {
    vm: VmmDomain,
    is_customize_dialog: bool,
    top_box: gtk::Box,
    hw_list: gtk::ListBox,
    rows: RefCell<Vec<HwRow>>,
    hw_panel: gtk::Stack,
    overview: Overview,
    cpu: CpuPage,
    memory: MemoryPage,
    boot: BootPage,
    os: OsPage,
    disk: DiskPage,
    network: NetworkPage,
    input: crate::devpages::InputPage,
    sound: crate::devpages::SoundPage,
    video: crate::devpages::VideoPage,
    watchdog: crate::devpages::WatchdogPage,
    smartcard: crate::devpages::SmartcardPage,
    redir: crate::devpages::RedirPage,
    rng: crate::devpages::RngPage,
    panic: crate::devpages::PanicPage,
    char_page: crate::devpages::CharPage,
    controller: crate::devpages::ControllerPage,
    hostdev: crate::devpages::HostdevPage,
    tpm: Rc<crate::tpmdetails::TpmDetails>,
    tpm_models_loaded: Cell<bool>,
    vsock: Rc<crate::vsockdetails::VsockDetails>,
    fs: Rc<crate::fsdetails::FsDetails>,
    fs_drivers_loaded: Cell<bool>,
    graphics_title: gtk::Label,
    gfxdetails: Rc<crate::gfxdetails::GraphicsDetails>,
    unported: gtk::Label,
    config_remove: gtk::Button,
    config_cancel: gtk::Button,
    config_apply: gtk::Button,
    oldhwkey: RefCell<Option<String>>,
    active_edits: RefCell<Vec<Edit>>,
    /// Set while a page is filled in, so the widgets' change signals
    /// don't count as edits.
    refreshing: Cell<bool>,
    /// Set while the list is being rebuilt, so the selection churn
    /// doesn't refresh pages.
    repopulating: Cell<bool>,
}

#[derive(Clone)]
pub struct VmmDetails(Rc<Inner>);

impl VmmDetails {
    pub fn new(vm: &VmmDomain, is_customize_dialog: bool) -> VmmDetails {
        let hw_list = gtk::ListBox::new();
        hw_list.set_selection_mode(gtk::SelectionMode::Browse);
        hw_list.update_property(&[gtk::accessible::Property::Label("Hardware")]);
        let hw_scroll = gtk::ScrolledWindow::builder()
            .child(&hw_list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .has_frame(true)
            .min_content_width(165)
            .build();
        let add_hardware = gtk::Button::with_mnemonic("A_dd Hardware");
        // The Add Hardware wizard (ticket 12) isn't ported yet.
        add_hardware.set_sensitive(false);
        let left = gtk::Box::new(gtk::Orientation::Vertical, 6);
        left.append(&hw_scroll);
        left.append(&add_hardware);

        let overview = Overview::new();
        let cpu = CpuPage::new();
        let memory = MemoryPage::new();
        let boot = BootPage::new();
        let os = OsPage::new();
        let disk = DiskPage::new(vm.conn());
        let network = NetworkPage::new(vm.conn());
        use crate::devpages as dp;
        let input = dp::InputPage::new();
        let sound = dp::SoundPage::new();
        let video = dp::VideoPage::new();
        video.build_models(vm.conn().as_ref());
        let watchdog = dp::WatchdogPage::new();
        let smartcard = dp::SmartcardPage::new();
        let redir = dp::RedirPage::new();
        let rng = dp::RngPage::new();
        let panic = dp::PanicPage::new();
        let char_page = dp::CharPage::new();
        let controller = dp::ControllerPage::new();
        let hostdev = dp::HostdevPage::new();
        let tpm = crate::tpmdetails::TpmDetails::new();
        let vsock = crate::vsockdetails::VsockDetails::new();
        let fs = crate::fsdetails::FsDetails::new(vm.conn().as_ref());
        let gfxdetails = crate::gfxdetails::GraphicsDetails::new(vm.conn());
        let graphics_title = dp::title("");
        let graphics_page = gtk::Box::new(gtk::Orientation::Vertical, 6);
        graphics_title.set_xalign(0.0);
        graphics_page.append(&graphics_title);
        gfxdetails.top_box.set_margin_start(12);
        graphics_page.append(&gfxdetails.top_box);
        let unported = gtk::Label::new(Some("This page is not available yet in this version."));
        unported.set_valign(gtk::Align::Start);
        unported.set_xalign(0.0);
        let hw_panel = gtk::Stack::new();
        hw_panel.add_named(&overview.page, Some("overview"));
        hw_panel.add_named(&cpu.page, Some("cpu"));
        hw_panel.add_named(&memory.page, Some("memory"));
        hw_panel.add_named(&boot.page, Some("boot"));
        hw_panel.add_named(&os.page, Some("os"));
        hw_panel.add_named(&disk.page, Some("disk"));
        hw_panel.add_named(&network.page, Some("network"));
        hw_panel.add_named(&input.page, Some("input"));
        hw_panel.add_named(&sound.page, Some("sound"));
        hw_panel.add_named(&video.page, Some("video"));
        hw_panel.add_named(&watchdog.page, Some("watchdog"));
        hw_panel.add_named(&smartcard.page, Some("smartcard"));
        hw_panel.add_named(&redir.page, Some("redir"));
        hw_panel.add_named(&rng.page, Some("rng"));
        hw_panel.add_named(&panic.page, Some("panic"));
        hw_panel.add_named(&graphics_page, Some("graphics"));
        hw_panel.add_named(&char_page.page, Some("char"));
        hw_panel.add_named(&controller.page, Some("controller"));
        hw_panel.add_named(&hostdev.page, Some("hostdev"));
        hw_panel.add_named(
            &dp::frame_page(&dp::title("TPM Device"), &tpm.top_box),
            Some("tpm"),
        );
        hw_panel.add_named(
            &dp::frame_page(&dp::title("Virtio VSOCK"), &vsock.top_box),
            Some("vsock"),
        );
        hw_panel.add_named(
            &dp::frame_page(&dp::title("Filesystem"), &fs.top_box),
            Some("filesystem"),
        );
        hw_panel.add_named(&unported, Some("unported"));
        let panel_scroll = gtk::ScrolledWindow::builder()
            .child(&hw_panel)
            .hexpand(true)
            .vexpand(true)
            .build();

        let config_remove = gtk::Button::with_mnemonic("_Remove");
        let config_cancel = gtk::Button::with_mnemonic("_Cancel");
        let config_apply = gtk::Button::with_mnemonic("_Apply");
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        buttons.append(&config_remove);
        buttons.append(&spacer);
        buttons.append(&config_cancel);
        buttons.append(&config_apply);
        let right = gtk::Box::new(gtk::Orientation::Vertical, 6);
        right.append(&panel_scroll);
        right.append(&buttons);

        let top_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        top_box.set_margin_top(6);
        top_box.set_margin_bottom(6);
        top_box.set_margin_start(6);
        top_box.set_margin_end(6);
        top_box.append(&left);
        top_box.append(&right);

        let this = VmmDetails(Rc::new(Inner {
            vm: vm.clone(),
            is_customize_dialog,
            top_box,
            hw_list,
            rows: RefCell::default(),
            hw_panel,
            overview,
            cpu,
            memory,
            boot,
            os,
            disk,
            network,
            input,
            sound,
            video,
            watchdog,
            smartcard,
            redir,
            rng,
            panic,
            char_page,
            controller,
            hostdev,
            tpm,
            tpm_models_loaded: Cell::new(false),
            vsock,
            fs,
            fs_drivers_loaded: Cell::new(false),
            graphics_title,
            gfxdetails,
            unported,
            config_remove,
            config_cancel,
            config_apply,
            oldhwkey: RefCell::default(),
            active_edits: RefCell::default(),
            refreshing: Cell::new(false),
            repopulating: Cell::new(false),
        }));
        this.disable_apply();
        let weak = Rc::downgrade(&this.0);
        this.0.hw_list.connect_row_selected(move |_, _| {
            if let Some(inner) = weak.upgrade() {
                VmmDetails(inner).hw_changed();
            }
        });
        this.connect_edits();
        this.init_cpu_models();
        this.repopulate_hw_list();
        this
    }

    /// `_init_details`' edit signals, and the Cancel/Apply buttons.
    fn connect_edits(&self) {
        let i = &self.0;
        let weak = Rc::downgrade(&self.0);
        let enable = move |edit| {
            let weak = weak.clone();
            move || {
                if let Some(inner) = weak.upgrade() {
                    VmmDetails(inner).enable_apply(edit);
                }
            }
        };
        let cb = enable(Edit::Name);
        i.overview.name.connect_changed(move |_| cb());
        let cb = enable(Edit::Title);
        i.overview.title.connect_changed(move |_| cb());
        let cb = enable(Edit::Desc);
        i.overview
            .description
            .buffer()
            .connect_changed(move |_| cb());

        let cb = enable(Edit::Cpu);
        i.cpu.model.entry.connect_changed(move |_| cb());
        let cb = enable(Edit::Cpu);
        i.cpu.secure.connect_toggled(move |_| cb());
        let cb = enable(Edit::Mem);
        i.memory.maxmem.connect_value_changed(move |_| cb());
        let cb = enable(Edit::MemShared);
        i.memory.shared.connect_toggled(move |_| cb());

        let weak = Rc::downgrade(&self.0);
        let with = move |f: fn(&VmmDetails)| {
            let weak = weak.clone();
            move || {
                if let Some(inner) = weak.upgrade() {
                    f(&VmmDetails(inner));
                }
            }
        };
        let cb = with(Self::config_vcpus_changed);
        i.cpu.vcpus.connect_value_changed(move |_| cb());
        let cb = with(Self::cpu_copy_host_clicked);
        i.cpu.copy_host.connect_toggled(move |_| cb());
        let cb = with(Self::cpu_topology_enable);
        i.cpu.topology_enable.connect_toggled(move |_| cb());
        for spin in [&i.cpu.sockets, &i.cpu.cores, &i.cpu.threads] {
            let cb = with(Self::sync_cpu_topology_ui);
            spin.connect_value_changed(move |_| cb());
        }
        let cb = with(Self::curmem_changed);
        i.memory.memory.connect_value_changed(move |_| cb());

        let b = &i.boot;
        let cb = enable(Edit::Autostart);
        b.autostart.connect_toggled(move |_| cb());
        let cb = enable(Edit::BootMenu);
        b.menu.connect_toggled(move |_| cb());
        for entry in [&b.kernel, &b.initrd, &b.dtb, &b.kernel_args] {
            let cb = enable(Edit::Kernel);
            entry.connect_changed(move |_| cb());
        }
        for entry in [&b.init_path, &b.init_args] {
            let cb = enable(Edit::Init);
            entry.connect_changed(move |_| cb());
        }
        let cb = enable(Edit::OsName);
        i.os.os_list.connect_os_selected(move |_| cb());
        let cb = enable(Edit::NetSource);
        i.network.netlist.connect_changed(cb);
        let cb = enable(Edit::NetModel);
        i.network
            .model
            .widget
            .connect_selected_notify(move |_| cb());
        let cb = enable(Edit::NetLinkstate);
        i.network.link_state.connect_toggled(move |_| cb());
        let cb = enable(Edit::SoundModel);
        i.sound.model.widget.connect_selected_notify(move |_| cb());
        let cb = enable(Edit::SmartcardMode);
        i.smartcard
            .mode
            .widget
            .connect_selected_notify(move |_| cb());
        let cb = enable(Edit::WatchdogModel);
        i.watchdog
            .model
            .widget
            .connect_selected_notify(move |_| cb());
        let cb = enable(Edit::WatchdogAction);
        i.watchdog
            .action
            .widget
            .connect_selected_notify(move |_| cb());
        let cb = enable(Edit::ControllerModel);
        i.controller
            .model
            .widget
            .connect_selected_notify(move |_| cb());
        let cb = enable(Edit::Gfx);
        i.gfxdetails.connect_changed(cb);
        let cb = enable(Edit::HostdevRombar);
        i.hostdev.rombar.connect_toggled(move |_| cb());
        let cb = enable(Edit::HostdevStartupPolicy);
        i.hostdev
            .startup_policy
            .widget
            .connect_selected_notify(move |_| cb());
        i.tpm.connect_changed(enable(Edit::Tpm));
        i.vsock.connect_changed_auto_cid(enable(Edit::VsockAuto));
        let cb = enable(Edit::VsockCid);
        i.vsock.cid.connect_value_changed(move |_| cb());
        i.fs.connect_changed(enable(Edit::Fs));
        let cb = with(Self::video_model_changed);
        i.video.model.widget.connect_selected_notify(move |_| cb());
        let cb = with(Self::video_3d_toggled);
        i.video.accel3d.connect_toggled(move |_| cb());
        let rweak = Rc::downgrade(&self.0);
        i.network.refresh_ip.connect_clicked(move |_| {
            if let Some(inner) = rweak.upgrade() {
                let this = VmmDetails(inner);
                if let Some(row) = this.get_hw_row() {
                    this.set_network_ip_details(&row);
                }
            }
        });
        let cb = enable(Edit::DiskPath);
        i.disk.mediacombo.connect_changed(cb);
        let cb = with(Self::disk_source_browse_clicked);
        i.disk.browse.connect_clicked(move |_| cb());
        let sweak = Rc::downgrade(&self.0);
        let storage = |edit: StorageEdit| {
            let weak = sweak.clone();
            move || {
                if let Some(inner) = weak.upgrade() {
                    VmmDetails(inner).storage_changed(edit);
                }
            }
        };
        let d = &i.disk;
        let cb = storage(StorageEdit::Cache);
        d.cache.widget.connect_selected_notify(move |_| cb());
        let cb = storage(StorageEdit::Discard);
        d.discard.widget.connect_selected_notify(move |_| cb());
        let cb = storage(StorageEdit::Readonly);
        d.readonly.connect_toggled(move |_| cb());
        let cb = storage(StorageEdit::Shareable);
        d.shareable.connect_toggled(move |_| cb());
        let cb = storage(StorageEdit::Removable);
        d.removable.connect_toggled(move |_| cb());
        let cb = storage(StorageEdit::Serial);
        d.serial.connect_changed(move |_| cb());
        let cb = with(Self::boot_kernel_toggled);
        b.kernel_enable.connect_toggled(move |_| cb());
        let cb = with(Self::config_bootdev_selected);
        b.list.connect_row_selected(move |_, _| cb());
        let cb = with(|this| this.config_boot_move(true));
        b.moveup.connect_clicked(move |_| cb());
        let cb = with(|this| this.config_boot_move(false));
        b.movedown.connect_clicked(move |_| cb());
        for (button, entry) in [
            (&b.kernel_browse, &b.kernel),
            (&b.initrd_browse, &b.initrd),
            (&b.dtb_browse, &b.dtb),
        ] {
            let weak = Rc::downgrade(&self.0);
            let entry = entry.clone();
            button.connect_clicked(move |_| {
                if let Some(inner) = weak.upgrade() {
                    VmmDetails(inner).browse_file(&entry);
                }
            });
        }

        let weak = Rc::downgrade(&self.0);
        i.config_remove.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                let this = VmmDetails(inner);
                glib::spawn_future_local(async move { this.config_remove().await });
            }
        });
        self.init_menus();
        let weak = Rc::downgrade(&self.0);
        i.config_cancel.connect_clicked(move |_| {
            // Remove current changes and deactivate 'apply' button
            if let Some(inner) = weak.upgrade() {
                VmmDetails(inner).refresh_page();
            }
        });
        let weak = Rc::downgrade(&self.0);
        i.config_apply.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                let this = VmmDetails(inner);
                glib::spawn_future_local(async move {
                    this.config_apply(None).await;
                });
            }
        });
    }

    /// `_browse_file` into `entry`. The storage browser isn't ported
    /// yet, so this offers the local file chooser.
    fn browse_file(&self, entry: &gtk::Entry) {
        let dialog = gtk::FileDialog::builder()
            .title("Locate or create storage volume")
            .modal(true)
            .build();
        let entry = entry.clone();
        dialog.open(
            self.parent().as_ref(),
            gtk::gio::Cancellable::NONE,
            move |res| {
                if let Some(path) = res.ok().and_then(|f| f.path()) {
                    entry.set_text(&path.to_string_lossy());
                }
            },
        );
    }

    /// `_boot_kernel_toggled_cb`.
    fn boot_kernel_toggled(&self) {
        let b = &self.0.boot;
        b.kernel_box.set_sensitive(b.kernel_enable.is_active());
        self.enable_apply(Edit::Kernel);
    }

    /// `_get_config_boot_order`.
    fn get_config_boot_order(&self) -> Vec<String> {
        self.0
            .boot
            .rows
            .borrow()
            .iter()
            .filter(|r| r.active)
            .filter_map(|r| r.key.clone())
            .collect()
    }

    fn get_config_boot_selection(&self) -> Option<BootRow> {
        let idx = self.0.boot.list.selected_row()?.index();
        self.0.boot.rows.borrow().get(idx as usize).cloned()
    }

    /// `_config_bootdev_selected`.
    fn config_bootdev_selected(&self) {
        let b = &self.0.boot;
        let selection = self.get_config_boot_selection().and_then(|r| r.key);
        let devs = self.get_config_boot_order();
        let pos = selection
            .as_ref()
            .and_then(|s| devs.iter().position(|d| d == s));
        b.movedown
            .set_sensitive(pos.is_some_and(|p| p + 1 != devs.len()));
        b.moveup.set_sensitive(pos.is_some_and(|p| p != 0));
    }

    /// Rebuilds `boot-list` from its rows, keeping the selected key.
    fn redraw_boot_list(&self, select: Option<String>) {
        let b = &self.0.boot;
        b.list.remove_all();
        let rows = b.rows.borrow().clone();
        for (idx, row) in rows.iter().enumerate() {
            let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            hbox.set_margin_top(3);
            hbox.set_margin_bottom(3);
            hbox.set_margin_start(3);
            hbox.set_margin_end(6);
            let check = gtk::CheckButton::new();
            check.set_active(row.active);
            check.set_visible(row.can_select);
            check.update_property(&[gtk::accessible::Property::Label(&row.label)]);
            let weak = Rc::downgrade(&self.0);
            check.connect_toggled(move |_| {
                if let Some(inner) = weak.upgrade() {
                    VmmDetails(inner).config_boot_toggled(idx);
                }
            });
            hbox.append(&check);
            if let Some(icon) = &row.icon {
                hbox.append(&gtk::Image::from_icon_name(icon));
            }
            let label = gtk::Label::new(Some(&row.label));
            label.set_xalign(0.0);
            label.set_sensitive(row.active);
            hbox.append(&label);
            b.list
                .append(&gtk::ListBoxRow::builder().child(&hbox).build());
        }
        let idx = select.and_then(|k| rows.iter().position(|r| r.key.as_ref() == Some(&k)));
        b.list
            .select_row(idx.and_then(|i| b.list.row_at_index(i as i32)).as_ref());
        self.config_bootdev_selected();
    }

    /// `_config_boot_toggled_cb`.
    fn config_boot_toggled(&self, idx: usize) {
        let active = {
            let mut rows = self.0.boot.rows.borrow_mut();
            let Some(row) = rows.get_mut(idx) else {
                return;
            };
            row.active = !row.active;
            row.active
        };
        if let Some(lbrow) = self.0.boot.list.row_at_index(idx as i32) {
            if let Some(label) = lbrow
                .child()
                .and_then(|c| c.last_child())
                .and_downcast::<gtk::Label>()
            {
                label.set_sensitive(active);
            }
        }
        self.config_bootdev_selected();
        self.enable_apply(Edit::BootOrder);
    }

    /// `_config_boot_move`.
    fn config_boot_move(&self, move_up: bool) {
        let Some(row) = self.get_config_boot_selection() else {
            return;
        };
        let Some(key) = row.key.clone() else {
            return;
        };
        {
            let mut rows = self.0.boot.rows.borrow_mut();
            let Some(idx) = rows.iter().position(|r| r.key.as_ref() == Some(&key)) else {
                return;
            };
            // Swap with the neighbouring active row
            let other = if move_up {
                idx.checked_sub(1)
            } else {
                Some(idx + 1).filter(|i| *i < rows.len())
            };
            let Some(other) = other.filter(|o| rows[*o].active) else {
                return;
            };
            rows.swap(idx, other);
        }
        self.redraw_boot_list(Some(key));
        self.enable_apply(Edit::BootOrder);
    }

    /// `_make_boot_rows`.
    fn make_boot_rows(&self) -> Vec<BootRow> {
        let row = |key: &str, label: &str, icon: &str| BootRow {
            key: Some(key.into()),
            label: label.into(),
            icon: Some(icon.into()),
            active: false,
            can_select: true,
        };
        if !self.0.vm.can_use_device_boot_order() {
            return vec![
                row("hd", "Hard Disk", "drive-harddisk"),
                row("cdrom", "CDROM", "media-optical"),
                row("network", "Network (PXE)", "network-idle"),
                row("fd", "Floppy", "media-floppy"),
            ];
        }
        let mut ret = vec![];
        if let Some(x) = self.0.vm.xmlobj() {
            let hw_rows = self.0.rows.borrow();
            for id in virtinst_core::guest::bootable_device_ids(&x.doc, x.root()) {
                if let Some(hw) = hw_rows.iter().find(|r| r.key() == id) {
                    ret.push(row(&id, &hw.label, &hw.icon));
                }
            }
        }
        if ret.is_empty() {
            ret.push(BootRow {
                key: None,
                label: "No bootable devices".into(),
                icon: None,
                active: false,
                can_select: false,
            });
        }
        ret
    }

    /// `_refresh_boot_order`.
    fn refresh_boot_order(&self) {
        let mut boot_rows = self.make_boot_rows();
        let mut ordered = vec![];
        for key in self.0.vm.get_boot_order() {
            if let Some(pos) = boot_rows.iter().position(|r| r.key.as_ref() == Some(&key)) {
                let mut row = boot_rows.remove(pos);
                row.active = true;
                ordered.push(row);
            }
        }
        ordered.extend(boot_rows);
        self.0.boot.rows.replace(ordered);
        self.redraw_boot_list(None);
    }

    /// `_refresh_boot_page`.
    fn refresh_boot_page(&self) {
        let i = &self.0;
        let b = &i.boot;
        let Some(x) = i.vm.xmlobj() else {
            return;
        };
        let os = &x.guest.os;
        let os_type = os.os_type.as_deref();
        let is_container = os_type == Some("exe");
        let is_xenpv = matches!(os_type, Some("xen" | "linux" | "xenpvh"));
        b.order_frame.set_visible(!is_container && !is_xenpv);
        b.kernel_expander.set_visible(!is_container);
        b.init_frame.set_visible(is_container);

        // Kernel/initrd boot
        let expand = [&os.kernel, &os.dtb, &os.initrd, &os.kernel_args]
            .iter()
            .any(|v| v.is_some());
        // If the user unsets kernel/initrd by unchecking the 'enable
        // kernel boot' box, we keep the previous values cached in the
        // text fields to allow easy switching back and forth.
        let keep_text = |entry: &gtk::Entry, guestval: &Option<String>| {
            let guestval = guestval.as_deref().unwrap_or("");
            if !entry.text().trim().is_empty() && guestval.is_empty() {
                return;
            }
            entry.set_text(guestval);
        };
        keep_text(&b.kernel, &os.kernel);
        keep_text(&b.initrd, &os.initrd);
        keep_text(&b.dtb, &os.dtb);
        keep_text(&b.kernel_args, &os.kernel_args);
        if expand {
            // Only 'expand' if requested, so a refresh doesn't
            // magically unexpand the UI the user just touched
            b.kernel_expander.set_expanded(true);
        }
        b.kernel_enable.set_active(expand);
        b.kernel_box.set_sensitive(expand);

        // Only show dtb if it's supported
        let arch = os.arch.as_deref().unwrap_or("");
        let show_dtb = !b.dtb.text().trim().is_empty()
            || x.guest.domain_type.as_deref() == Some("test")
            || arch.contains("arm")
            || arch.contains("microblaze")
            || arch.contains("ppc");
        b.dtb_label.set_visible(show_dtb);
        b.dtb_box.set_visible(show_dtb);

        // <init> populate
        b.init_path.set_text(os.init.as_deref().unwrap_or(""));
        b.init_args
            .set_text(&virtinst_core::guest::initargs_string(os));

        // Boot menu populate
        b.menu.set_active(os.bootmenu_enable.unwrap_or(false));
        self.refresh_boot_order();

        // Refresh autostart (a libvirt call)
        let this = self.clone();
        glib::spawn_future_local(async move {
            let autoval = this.0.vm.get_autostart().await;
            let chk = &this.0.boot.autostart;
            chk.set_sensitive(autoval.is_some());
            this.quietly(|| chk.set_active(autoval.unwrap_or(false)));
        });
    }

    /// `_curmem_changed_cb`.
    fn curmem_changed(&self) {
        self.enable_apply(Edit::Mem);
        let maxmem = &self.0.memory.maxmem;
        let mem = spin_get(&self.0.memory.memory);
        if maxmem.value() < mem {
            maxmem.set_value(mem);
        }
        let (_, upper) = maxmem.range();
        maxmem.set_range(mem, upper);
    }

    /// `_config_vcpus_changed_cb`.
    fn config_vcpus_changed(&self) {
        self.enable_apply(Edit::Vcpus);
        self.update_vcpus_warning();
    }

    /// Warn about overcommit.
    fn update_vcpus_warning(&self) {
        let host_active_count = self
            .0
            .vm
            .conn()
            .map(|c| c.host_active_processor_count())
            .unwrap_or(0);
        let cur = spin_get(&self.0.cpu.vcpus);
        self.0
            .cpu
            .vcpus_warn_box
            .set_visible(cur > f64::from(host_active_count));
    }

    /// `_cpu_copy_host_clicked_cb`.
    fn cpu_copy_host_clicked(&self) {
        let c = &self.0.cpu;
        let show = !c.copy_host.is_active();
        c.model_label.set_visible(show);
        c.model.widget.set_visible(show);
        c.secure.set_visible(show);
        self.enable_apply(Edit::Cpu);
    }

    /// `_sync_cpu_topology_ui`.
    fn sync_cpu_topology_ui(&self) {
        let c = &self.0.cpu;
        let manual_top = c.topology_table.is_sensitive();
        c.vcpus.set_sensitive(!manual_top);
        if manual_top {
            // `spin_get_helper(...) or 1`
            let get = |s: &gtk::SpinButton| Some(spin_get(s)).filter(|v| *v != 0.0).unwrap_or(1.0);
            let total = get(&c.cores) * get(&c.sockets) * get(&c.threads);
            c.vcpus.set_value(total);
        } else {
            let vcpus = spin_get(&c.vcpus);
            c.sockets.set_value(if vcpus == 0.0 { 1.0 } else { vcpus });
            c.cores.set_value(1.0);
            c.threads.set_value(1.0);
        }
        self.enable_apply(Edit::Topology);
    }

    /// `_cpu_topology_enable_cb`.
    fn cpu_topology_enable(&self) {
        let c = &self.0.cpu;
        c.topology_table
            .set_sensitive(c.topology_enable.is_active());
        self.sync_cpu_topology_ui();
    }

    /// `_get_config_cpu_model`.
    fn get_config_cpu_model(&self) -> String {
        let c = &self.0.cpu;
        if c.copy_host.is_active() {
            return cpu::SPECIAL_MODE_HOST_PASSTHROUGH.into();
        }
        match c.model.key_for_text() {
            None => c.model.text(),
            Some(key) if key == cpu::SPECIAL_MODE_APP_DEFAULT => {
                crate::engine::get().config().get_default_cpu_setting()
            }
            Some(key) => key,
        }
    }

    fn parent(&self) -> Option<gtk::Window> {
        self.0.top_box.root().and_downcast::<gtk::Window>()
    }

    pub fn top_box(&self) -> &gtk::Box {
        &self.0.top_box
    }

    fn disable_apply(&self) {
        self.0.active_edits.borrow_mut().clear();
        self.0.config_apply.set_sensitive(false);
        self.0.config_cancel.set_sensitive(false);
    }

    fn enable_apply(&self, edit: Edit) {
        if self.0.refreshing.get() {
            return;
        }
        self.0.config_apply.set_sensitive(true);
        self.0.config_cancel.set_sensitive(true);
        let mut edits = self.0.active_edits.borrow_mut();
        if !edits.contains(&edit) {
            edits.push(edit);
        }
    }

    fn edited(&self, edit: Edit) -> bool {
        self.0.active_edits.borrow().contains(&edit)
    }

    /// `_config_apply`: `true` when the changes went in (or there were
    /// none to make).
    async fn config_apply(&self, row: Option<HwRow>) -> bool {
        let Some(row) = row.or_else(|| self.get_hw_row()) else {
            return true;
        };
        let res = match row.hwtype {
            HwType::General => self.apply_overview().await,
            HwType::Cpu => Ok(self.apply_vcpus().await),
            HwType::Memory => Ok(self.apply_memory().await),
            HwType::Boot => Ok(self.apply_boot_options().await),
            HwType::Os => Ok(self.apply_os().await),
            HwType::Disk => Ok(self.apply_disk(&row).await),
            HwType::Nic => Ok(self.apply_network(&row).await),
            HwType::Sound => Ok(self.apply_sound(&row).await),
            HwType::Smartcard => Ok(self.apply_smartcard(&row).await),
            HwType::Video => Ok(self.apply_video(&row).await),
            HwType::Watchdog => Ok(self.apply_watchdog(&row).await),
            HwType::Graphics => Ok(self.apply_graphics(&row).await),
            HwType::Controller => Ok(self.apply_controller(&row).await),
            HwType::Hostdev => Ok(self.apply_hostdev(&row).await),
            HwType::Tpm => Ok(self.apply_tpm(&row).await),
            HwType::Vsock => Ok(self.apply_vsock(&row).await),
            HwType::Filesystem => Ok(self.apply_filesystem(&row).await),
            _ => Ok(true),
        };
        let success = match res {
            Ok(success) => success,
            Err(e) => {
                crate::error::show_err(
                    self.parent().as_ref(),
                    &format!("Error applying changes: {e}"),
                    None,
                    "",
                )
                .await;
                false
            }
        };
        if success {
            self.disable_apply();
        }
        success
    }

    /// `_apply_overview`. The machine type and firmware edits belong to
    /// the customize dialog.
    async fn apply_overview(&self) -> Result<bool, String> {
        let o = &self.0.overview;
        let mut title = None;
        let mut description = None;
        if self.edited(Edit::Title) {
            title = Some(o.title.text().to_string());
        }
        if self.edited(Edit::Desc) {
            let buffer = o.description.buffer();
            let (start, end) = buffer.bounds();
            description = Some(buffer.text(&start, &end, false).to_string());
        }

        // This needs to be last
        if self.edited(Edit::Name) {
            // Renaming is pretty convoluted, so do it here synchronously
            self.0.vm.rename_domain(&o.name.text()).await?;
            if title.is_none() && description.is_none() {
                // Saves some useless redefine attempts
                return Ok(true);
            }
        }

        let vm = self.0.vm.clone();
        let vm2 = vm.clone();
        let (t, d) = (title.clone(), description.clone());
        let hotplug = (title.is_some() || description.is_some()).then(|| {
            Box::pin(async move { vm2.hotplug_metadata(description, title).await }) as Hotplug
        });
        Ok(self
            .change_config(
                async move { vm.define_overview(d.map(Some), t.map(Some), None).await },
                hotplug,
            )
            .await)
    }

    /// `_apply_boot_options`.
    async fn apply_boot_options(&self) -> bool {
        let b = &self.0.boot;
        let parent = self.parent();
        if self.edited(Edit::Autostart) {
            if let Err(e) = self.0.vm.set_autostart(b.autostart.is_active()).await {
                crate::error::show_err(
                    parent.as_ref(),
                    &format!("Error changing autostart value: {e}"),
                    None,
                    "",
                )
                .await;
                return false;
            }
        }

        let mut edits = crate::domain::BootEdits::default();
        if self.edited(Edit::BootOrder) {
            edits.boot_order = Some(self.get_config_boot_order());
        }
        if self.edited(Edit::BootMenu) {
            edits.boot_menu = Some(b.menu.is_active());
        }
        if self.edited(Edit::Kernel) {
            // `_get_text(..., checksens=True)`
            let text = |e: &gtk::Entry| {
                if !e.is_sensitive() || !WidgetExt::is_visible(e) || !b.kernel_box.is_sensitive() {
                    return String::new();
                }
                e.text().trim().to_string()
            };
            let k = crate::domain::KernelBoot {
                kernel: text(&b.kernel),
                initrd: text(&b.initrd),
                dtb: if b.dtb_box.is_visible() {
                    text(&b.dtb)
                } else {
                    String::new()
                },
                kernel_args: text(&b.kernel_args),
            };
            if !k.initrd.is_empty() && k.kernel.is_empty() {
                crate::error::val_err(
                    parent.as_ref(),
                    "Cannot set initrd without specifying a kernel path",
                )
                .await;
                return false;
            }
            if !k.kernel_args.is_empty() && k.kernel.is_empty() {
                crate::error::val_err(
                    parent.as_ref(),
                    "Cannot set kernel arguments without specifying a kernel path",
                )
                .await;
                return false;
            }
            edits.kernel = Some(k);
        }
        if self.edited(Edit::Init) {
            let init = b.init_path.text().trim().to_string();
            let initargs = b.init_args.text().trim().to_string();
            if init.is_empty() {
                crate::error::val_err(parent.as_ref(), "An init path must be specified").await;
                return false;
            }
            edits.init = Some((init, initargs));
        }

        let vm = self.0.vm.clone();
        self.change_config(async move { vm.define_boot(edits).await }, None)
            .await
    }

    /// `_apply_os`.
    async fn apply_os(&self) -> bool {
        if !self.edited(Edit::OsName) {
            return true;
        }
        let os_id = self.0.os.os_list.get_selected_os().and_then(|o| o.full_id);
        let vm = self.0.vm.clone();
        self.change_config(async move { vm.define_os(os_id).await }, None)
            .await
    }

    /// The row's device element in the live XML.
    fn device_element(&self, row: &HwRow) -> Option<(Rc<DomainXml>, Element)> {
        let (tag, idx) = row.device.clone()?;
        let x = self.0.vm.xmlobj()?;
        let devices = virtinst_xml::resolve_path(&x.doc, x.root(), &["devices"])?;
        let el = *devices.find_all(&x.doc, &tag).get(idx)?;
        Some((x, el))
    }

    fn nic_id(&self, row: &HwRow) -> Option<crate::domain::NicId> {
        let (x, el) = self.device_element(row)?;
        Some(crate::domain::NicId {
            mac: child_attr(&x.doc, el, &["mac"], "address"),
            idx: row.device.as_ref()?.1,
        })
    }

    /// `_refresh_{input,sound,video,...}_page` for the pages in
    /// `devpages.rs`.
    fn refresh_simple_device_page(&self, row: &HwRow) {
        let i = &self.0;
        let Some((x, el)) = self.device_element(row) else {
            return;
        };
        let doc = &x.doc;
        let name = match row.hwtype {
            HwType::Input => {
                i.input.refresh(doc, el);
                "input"
            }
            HwType::Sound => {
                i.sound.refresh(doc, el);
                "sound"
            }
            HwType::Video => {
                i.video.refresh(doc, el);
                "video"
            }
            HwType::Watchdog => {
                i.watchdog.refresh(doc, el);
                "watchdog"
            }
            HwType::Smartcard => {
                i.smartcard.refresh(doc, el);
                "smartcard"
            }
            HwType::Redirdev => {
                i.redir.refresh(doc, el, &row.label);
                "redir"
            }
            HwType::Rng => {
                i.rng.refresh(doc, el);
                "rng"
            }
            HwType::Panic => {
                i.panic.refresh(doc, el);
                "panic"
            }
            _ => return,
        };
        i.hw_panel.set_visible_child_name(name);
    }

    /// `_video_model_changed_cb`.
    fn video_model_changed(&self) {
        self.0.video.sync_3d_row();
        self.enable_apply(Edit::VideoModel);
    }

    /// `_video_3d_toggled_cb`.
    fn video_3d_toggled(&self) {
        self.0.video.accel3d.set_inconsistent(false);
        self.enable_apply(Edit::Video3d);
    }

    fn device_id(&self, row: &HwRow) -> Option<virtinst_core::devices::DeviceId> {
        let (x, el) = self.device_element(row)?;
        let (tag, idx) = row.device.as_ref()?;
        Some(virtinst_core::devices::device_id(&x.doc, el, tag, *idx))
    }

    /// `_change_config(define_func, kwargs, devobj=devobj)` for the
    /// simple devices: the same edit goes to the inactive config, then
    /// to the running VM.
    async fn change_device_config(
        &self,
        row: &HwRow,
        edit: impl Fn(&mut Document, Element) -> Result<(), String> + Clone + 'static,
    ) -> bool {
        self.change_device_config_split(row, edit.clone(), edit)
            .await
    }

    /// `change_device_config` where the running VM gets a different
    /// edit than the inactive config.
    async fn change_device_config_split(
        &self,
        row: &HwRow,
        edit: impl Fn(&mut Document, Element) -> Result<(), String> + 'static,
        hedit: impl Fn(&mut Document, Element) -> Result<(), String> + 'static,
    ) -> bool {
        let Some(id) = self.device_id(row) else {
            return false;
        };
        let vm = self.0.vm.clone();
        let hotplug_vm = vm.clone();
        let hid = id.clone();
        self.change_config(
            async move { vm.define_device(&id, edit).await },
            Some(Box::pin(async move {
                hotplug_vm.hotplug_device(&hid, hedit).await
            })),
        )
        .await
    }

    /// `_refresh_char_page`.
    fn refresh_char_page(&self, row: &HwRow) {
        let Some((x, el)) = self.device_element(row) else {
            return;
        };
        let Some((tag, _)) = &row.device else {
            return;
        };
        let doc = &x.doc;
        // `serial_is_console_dup` (`DeviceConsole.get_console_duplicate`)
        let primary = tag == "serial"
            && virtinst_xml::resolve_path(doc, x.root(), &["devices", "console"]).is_some_and(
                |console| {
                    let ctarget = child_attr(doc, console, &["target"], "type");
                    attr(doc, console, "type") == attr(doc, el, "type")
                        && ctarget.as_deref().map_or(true, |t| t == "serial")
                },
            );
        self.0.char_page.refresh(doc, el, tag, primary);
    }

    /// `_refresh_hostdev_page`. The node device lookup is async, so
    /// the page first shows `hostdev_pretty_name`.
    fn refresh_hostdev_page(&self, row: &HwRow) {
        let Some((x, el)) = self.device_element(row) else {
            return;
        };
        self.0
            .hostdev
            .refresh(&x.doc, el, &hostdev_pretty_name(&x.doc, el));
        let Some(conn) = self.0.vm.conn() else {
            return;
        };
        let devtype = match attr(&x.doc, el, "type").as_deref() {
            Some("usb") => "usb_device".to_string(),
            other => other.unwrap_or_default().to_string(),
        };
        let this = self.clone();
        let key = row.key();
        glib::spawn_future_local(async move {
            let all = conn.list_nodedevs().await;
            if this.get_hw_row().map(|r| r.key()) != Some(key) {
                return;
            }
            // the last match wins, as upstream's loop
            let nodedev = all.iter().rfind(|d| {
                d.device_type.as_deref() == Some(devtype.as_str())
                    && d.compare_to_hostdev(&x.doc, el)
            });
            let pretty = nodedev
                .map(|d| virtinst_core::nodedev::pretty_name(d, &all))
                .filter(|p| !p.is_empty());
            if let Some(pretty) = pretty {
                this.0.hostdev.set_source(&pretty);
            }
        });
    }

    /// `_refresh_tpm_page`. The model list comes from domcaps, which is
    /// fetched once.
    fn refresh_tpm_page(&self, row: &HwRow) {
        let i = &self.0;
        let Some((x, el)) = self.device_element(row) else {
            return;
        };
        i.tpm.set_dev(&x.doc, el);
        if i.tpm_models_loaded.replace(true) {
            return;
        }
        let this = self.clone();
        glib::spawn_future_local(async move {
            let Some(caps) = this.0.vm.get_domain_capabilities().await else {
                this.0.tpm_models_loaded.set(false);
                return;
            };
            let tpm = &caps.devices.tpm;
            let models = tpm.supported.is_some().then(|| {
                tpm.get_enum("model")
                    .map(|e| e.values.clone())
                    .unwrap_or_default()
            });
            this.0.tpm.set_models(models);
        });
    }

    /// `_refresh_filesystem_page`. The virtiofs driver row and the
    /// shared memory warning wait on domcaps.
    fn refresh_filesystem_page(&self, row: &HwRow) {
        let i = &self.0;
        let Some((x, el)) = self.device_element(row) else {
            return;
        };
        i.fs.set_dev(&x.doc, el);
        let container = i.vm.conn().is_some_and(|c| c.is_container_only());
        let load_drivers = !container && !i.fs_drivers_loaded.replace(true);
        let this = self.clone();
        glib::spawn_future_local(async move {
            if load_drivers {
                match this.0.vm.get_domain_capabilities().await {
                    Some(caps) => this
                        .0
                        .fs
                        .set_virtiofs_supported(caps.supports_filesystem_virtiofs()),
                    None => this.0.fs_drivers_loaded.set(false),
                }
            }
            let (have_shared_mem, _) = this.0.vm.has_shared_mem().await;
            this.0.fs.set_have_shared_mem(have_shared_mem);
        });
    }

    /// `_refresh_controller_page`.
    fn refresh_controller_page(&self, row: &HwRow) {
        let Some((x, el)) = self.device_element(row) else {
            return;
        };
        let doc = &x.doc;
        let ctype = attr(doc, el, "type").unwrap_or_default();
        let model = attr(doc, el, "model");
        let attached = matches!(ctype.as_str(), "scsi" | "sata" | "ide" | "fdc").then(|| {
            // `get_attached_devices`: disks on this bus whose drive
            // address names this controller's index.
            let index: Option<i64> = attr(doc, el, "index").and_then(|i| i.parse().ok());
            let rows = self.0.rows.borrow();
            let disks = virtinst_xml::resolve_path(doc, x.root(), &["devices"])
                .map(|d| d.find_all(doc, "disk"))
                .unwrap_or_default();
            disks
                .into_iter()
                .enumerate()
                .filter(|(_, d)| {
                    let controller: Option<i64> = child_attr(doc, *d, &["address"], "controller")
                        .and_then(|c| c.parse().ok());
                    child_attr(doc, *d, &["target"], "bus").as_deref() == Some(ctype.as_str())
                        && controller == index
                })
                .map(|(idx, d)| {
                    let name = rows
                        .iter()
                        .find(|r| r.device == Some(("disk".to_string(), idx)))
                        .map(|r| r.label.clone())
                        .unwrap_or_default();
                    // `DeviceAddress.pretty_desc`
                    let a = |n| child_attr(doc, d, &["address"], n).unwrap_or("None".into());
                    let address =
                        if child_attr(doc, d, &["address"], "type").as_deref() == Some("drive") {
                            format!(
                                "{}:{}:{}:{}",
                                a("controller"),
                                a("bus"),
                                a("target"),
                                a("unit")
                            )
                        } else {
                            "None".into()
                        };
                    format!("{name} on {address}")
                })
                .collect()
        });
        let type_label = controller_pretty_desc(&ctype, model.as_deref());
        self.0.controller.refresh(doc, el, &type_label, attached);
    }

    /// `_apply_controller`.
    async fn apply_controller(&self, row: &HwRow) -> bool {
        let model = self
            .edited(Edit::ControllerModel)
            .then(|| self.0.controller.model.get());
        let Some((x, el)) = self.device_element(row) else {
            return false;
        };
        let is_usb = attr(&x.doc, el, "type").as_deref() == Some("usb");
        let hmodel = model.clone();
        self.change_device_config_split(
            row,
            move |doc, el| {
                use virtinst_core::devices::controller as c;
                let Some(model) = &model else {
                    return Ok(());
                };
                if is_usb {
                    let devices = el.parent(doc).ok_or("Controller has no parent")?;
                    c::set_usb_model(doc, devices, model.as_deref());
                } else {
                    c::set_model(doc, el, model.as_deref());
                }
                Ok(())
            },
            // The running VM's USB controllers stay as they are; the
            // device goes back to libvirt unchanged, like upstream's
            // hotplug of the untouched live device.
            move |doc, el| {
                if let (Some(model), false) = (&hmodel, is_usb) {
                    virtinst_core::devices::controller::set_model(doc, el, model.as_deref());
                }
                Ok(())
            },
        )
        .await
    }

    /// `_refresh_graphics_page`.
    fn refresh_graphics_page(&self, row: &HwRow) {
        let Some((x, el)) = self.device_element(row) else {
            return;
        };
        let doc = &x.doc;
        let gtype = attr(doc, el, "type").unwrap_or_default();
        let title = format!("{} Server", graphics_pretty_type_simple(&gtype));
        self.0
            .graphics_title
            .set_markup(&format!("<b>{}</b>", glib::markup_escape_text(&title)));
        let has_virtio_3d = virtinst_xml::resolve_path(doc, x.root(), &["devices"])
            .map(|d| d.find_all(doc, "video"))
            .unwrap_or_default()
            .into_iter()
            .any(|v| {
                child_attr(doc, v, &["model"], "type").as_deref() == Some("virtio")
                    && child_attr(doc, v, &["model", "acceleration"], "accel3d").as_deref()
                        == Some("yes")
            });
        self.0.gfxdetails.set_dev(doc, el, has_virtio_3d);
    }

    /// `_apply_graphics`.
    async fn apply_graphics(&self, row: &HwRow) -> bool {
        let values = if self.edited(Edit::Gfx) {
            self.0.gfxdetails.get_values()
        } else {
            Default::default()
        };
        self.change_device_config(row, move |doc, el| values.apply(doc, el))
            .await
    }

    /// `_apply_sound`.
    async fn apply_sound(&self, row: &HwRow) -> bool {
        let model = self
            .edited(Edit::SoundModel)
            .then(|| self.0.sound.model.get())
            .flatten();
        self.change_device_config(row, move |doc, el| {
            if let Some(model) = &model {
                virtinst_core::devices::sound::set_model(doc, el, model);
            }
            Ok(())
        })
        .await
    }

    /// `_apply_smartcard`.
    async fn apply_smartcard(&self, row: &HwRow) -> bool {
        let mode = self
            .edited(Edit::SmartcardMode)
            .then(|| self.0.smartcard.mode.get())
            .flatten();
        self.change_device_config(row, move |doc, el| {
            if let Some(mode) = &mode {
                virtinst_core::devices::smartcard::set_mode(doc, el, mode);
            }
            Ok(())
        })
        .await
    }

    /// `_apply_video`.
    async fn apply_video(&self, row: &HwRow) -> bool {
        let v = &self.0.video;
        let model = self
            .edited(Edit::VideoModel)
            .then(|| v.model.get())
            .flatten();
        let accel3d = self.edited(Edit::Video3d).then(|| v.accel3d.is_active());
        self.change_device_config(row, move |doc, el| {
            use virtinst_core::devices::video;
            if let Some(model) = &model {
                video::set_model(doc, el, model);
            }
            if let Some(accel3d) = accel3d {
                video::set_accel3d(doc, el, accel3d);
            }
            Ok(())
        })
        .await
    }

    /// `_apply_hostdev`.
    async fn apply_hostdev(&self, row: &HwRow) -> bool {
        let h = &self.0.hostdev;
        let rom_bar = self
            .edited(Edit::HostdevRombar)
            .then(|| h.rombar.is_active());
        let policy = self
            .edited(Edit::HostdevStartupPolicy)
            .then(|| h.startup_policy.get());
        self.change_device_config(row, move |doc, el| {
            use virtinst_core::devices::hostdev;
            if let Some(rom_bar) = rom_bar {
                hostdev::set_rom_bar(doc, el, rom_bar);
            }
            if let Some(policy) = &policy {
                hostdev::set_startup_policy(doc, el, policy.as_deref());
            }
            Ok(())
        })
        .await
    }

    /// `_apply_tpm`.
    async fn apply_tpm(&self, row: &HwRow) -> bool {
        let values = Rc::new(if self.edited(Edit::Tpm) {
            self.0.tpm.get_values()
        } else {
            Default::default()
        });
        self.change_device_config(row, move |doc, el| {
            values.apply(doc, el);
            Ok(())
        })
        .await
    }

    /// `_apply_vsock`.
    async fn apply_vsock(&self, row: &HwRow) -> bool {
        let (auto_cid, cid) = self.0.vsock.get_values();
        let auto_cid = self.edited(Edit::VsockAuto).then_some(auto_cid);
        let cid = self.edited(Edit::VsockCid).then_some(cid);
        self.change_device_config(row, move |doc, el| {
            use virtinst_core::devices::vsock;
            if let Some(auto_cid) = auto_cid {
                vsock::set_auto_cid(doc, el, auto_cid);
            }
            if let Some(cid) = cid {
                vsock::set_cid(doc, el, cid);
            }
            Ok(())
        })
        .await
    }

    /// `_apply_filesystem`.
    async fn apply_filesystem(&self, row: &HwRow) -> bool {
        let edits = Rc::new(if self.edited(Edit::Fs) {
            self.0.fs.get_values()
        } else {
            Default::default()
        });
        self.change_device_config(row, move |doc, el| {
            edits.apply(doc, el);
            Ok(())
        })
        .await
    }

    /// `_apply_watchdog`.
    async fn apply_watchdog(&self, row: &HwRow) -> bool {
        let w = &self.0.watchdog;
        let model = self.edited(Edit::WatchdogModel).then(|| w.model.get());
        let action = self.edited(Edit::WatchdogAction).then(|| w.action.get());
        self.change_device_config(row, move |doc, el| {
            use virtinst_core::devices::watchdog;
            if let Some(model) = &model {
                watchdog::set_model(doc, el, model.as_deref());
            }
            if let Some(action) = &action {
                watchdog::set_action(doc, el, action.as_deref());
            }
            Ok(())
        })
        .await
    }

    /// `_apply_network`.
    async fn apply_network(&self, row: &HwRow) -> bool {
        let Some(id) = self.nic_id(row) else {
            return false;
        };
        let n = &self.0.network;
        let mut edits = crate::domain::NetworkEdits::default();
        if self.edited(Edit::NetModel) {
            edits.model = Some(n.model.get());
        }
        if self.edited(Edit::NetSource) {
            edits.source = n.netlist.get_network_selection();
        }
        if self.edited(Edit::NetLinkstate) {
            edits.linkstate = Some(n.link_state.is_active());
        }
        let vm = self.0.vm.clone();
        let hotplug_vm = vm.clone();
        let (hid, hedits) = (id.clone(), edits.clone());
        self.change_config(
            async move { vm.define_network(id, edits).await },
            Some(Box::pin(async move {
                hotplug_vm.hotplug_network(hid, hedits).await
            })),
        )
        .await
    }

    /// `_apply_disk`.
    async fn apply_disk(&self, row: &HwRow) -> bool {
        let Some((x, el)) = self.device_element(row) else {
            return false;
        };
        let id = crate::domain::DiskId {
            target: child_attr(&x.doc, el, &["target"], "dev"),
            bus: child_attr(&x.doc, el, &["target"], "bus"),
        };
        let d = &self.0.disk;
        let parent = self.parent();
        let mut edits = crate::domain::DiskEdits::default();
        if self.edited(Edit::DiskPath) {
            let path = d.mediacombo.get_path(true);
            let names = if path.is_empty() {
                vec![]
            } else {
                self.0
                    .vm
                    .path_in_use_by(path.clone())
                    .await
                    .unwrap_or_default()
            };
            if !names.is_empty() {
                let names = names
                    .iter()
                    .map(|n| format!("'{n}'"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let msg = format!("Disk '{path}' is already in use by other guests [{names}]");
                if !crate::error::yes_no(
                    parent.as_ref(),
                    &msg,
                    Some("Do you really want to use the disk?"),
                )
                .await
                {
                    return false;
                }
            }
            edits.path = Some(Some(path).filter(|p| !p.is_empty()));
        }
        if self.edited(Edit::Disk) {
            d.get_values(&mut edits);
        }

        let vm = self.0.vm.clone();
        let hotplug_vm = vm.clone();
        let (hid, hedits) = (id.clone(), edits.clone());
        self.change_config(
            async move { vm.define_disk(id, edits).await },
            Some(Box::pin(async move {
                hotplug_vm.hotplug_disk(hid, hedits).await
            })),
        )
        .await
    }

    /// `_apply_vcpus`.
    async fn apply_vcpus(&self) -> bool {
        let c = &self.0.cpu;
        let vcpus = self.edited(Edit::Vcpus).then(|| spin_get(&c.vcpus) as u32);
        let model = self
            .edited(Edit::Cpu)
            .then(|| (self.get_config_cpu_model(), c.secure.is_active()));
        let topology = self.edited(Edit::Topology).then(|| {
            c.topology_enable.is_active().then(|| {
                (
                    c.sockets.value() as u32,
                    c.cores.value() as u32,
                    c.threads.value() as u32,
                )
            })
        });
        let vm = self.0.vm.clone();
        self.change_config(
            async move { vm.define_cpu(vcpus, model, topology).await },
            None,
        )
        .await
    }

    /// `_apply_memory`.
    async fn apply_memory(&self) -> bool {
        let m = &self.0.memory;
        let memory = self.edited(Edit::Mem).then(|| {
            let maxmem = spin_get(&m.maxmem) as u64 * 1024;
            let curmem = spin_get(&m.memory) as u64 * 1024;
            (curmem, maxmem)
        });
        let mem_shared = self.edited(Edit::MemShared).then(|| m.shared.is_active());
        let vm = self.0.vm.clone();
        let vm2 = vm.clone();
        let hotplug = memory.map(|(cur, max)| {
            Box::pin(async move { vm2.hotplug_memory(cur, max).await }) as Hotplug
        });
        self.change_config(
            async move { vm.define_memory(memory, mem_shared).await },
            hotplug,
        )
        .await
    }

    /// `vmmAddHardware.change_config_helper` for guest XML edits: define,
    /// then hotplug the metadata into a running VM, telling the user
    /// when the change has to wait for the next shutdown.
    async fn change_config(
        &self,
        define: impl std::future::Future<Output = Result<(), String>>,
        hotplug: Option<Hotplug>,
    ) -> bool {
        let parent = self.parent();
        // Persistent config change
        if let Err(e) = define.await {
            crate::error::show_err(
                parent.as_ref(),
                &format!("Error changing VM configuration: {e}"),
                None,
                "",
            )
            .await;
            return false;
        }
        let vm = &self.0.vm;
        if !vm.is_active() {
            return true;
        }

        // Hotplug change
        let mut hotplug_err = None;
        let did_hotplug = hotplug.is_some();
        if let Some(hotplug) = hotplug {
            if let Err(e) = hotplug.await {
                glib::g_debug!("virt-manager", "Hotplug failed: {e}");
                hotplug_err = Some(e);
            }
        }
        if did_hotplug && hotplug_err.is_none() {
            return true;
        }

        let msg = "These changes will take effect after the next guest shutdown.";
        crate::error::show_notice(parent.as_ref(), msg, hotplug_err.as_deref()).await;
        true
    }

    fn make_row_widget(row: &HwRow) -> gtk::ListBoxRow {
        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        hbox.set_margin_top(3);
        hbox.set_margin_bottom(3);
        hbox.set_margin_start(3);
        hbox.set_margin_end(3);
        let icon = gtk::Image::from_icon_name(&row.icon);
        icon.set_icon_size(gtk::IconSize::Large);
        let label = gtk::Label::new(Some(&row.label));
        label.set_xalign(0.0);
        hbox.append(&icon);
        hbox.append(&label);
        gtk::ListBoxRow::builder().child(&hbox).build()
    }

    /// `_init_hw_list` + `_repopulate_hw_list`: rebuilds the rows from
    /// the latest XML, keeping the selection on the same key.
    fn repopulate_hw_list(&self) {
        let i = &self.0;
        let mut rows = static_rows(i.is_customize_dialog);
        if let Some(x) = i.vm.xmlobj() {
            rows.extend(device_rows(&x));
        }
        if *i.rows.borrow() == rows {
            return;
        }
        let selected_key = i.oldhwkey.borrow().clone();
        i.repopulating.set(true);
        i.hw_list.remove_all();
        for row in &rows {
            i.hw_list.append(&Self::make_row_widget(row));
        }
        let idx = selected_key
            .and_then(|k| rows.iter().position(|r| r.key() == k))
            .unwrap_or(0);
        i.rows.replace(rows);
        i.hw_list
            .select_row(i.hw_list.row_at_index(idx as i32).as_ref());
        i.repopulating.set(false);
        self.hw_changed();
    }

    fn get_hw_row(&self) -> Option<HwRow> {
        let idx = self.0.hw_list.selected_row()?.index();
        self.0.rows.borrow().get(idx as usize).cloned()
    }

    /// `_hw_changed_cb`. Pending edits are offered for applying first
    /// (asynchronously here, the old page staying up meanwhile); if
    /// that fails, the old row is selected again.
    fn hw_changed(&self) {
        if self.0.repopulating.get() {
            return;
        }
        let Some(row) = self.get_hw_row() else {
            return;
        };
        let key = Some(row.key());
        if *self.0.oldhwkey.borrow() == key {
            if self.0.hw_panel.visible_child_name().is_none() {
                self.refresh_page();
            }
            return;
        }
        let oldrow = {
            let old = self.0.oldhwkey.borrow();
            self.0
                .rows
                .borrow()
                .iter()
                .find(|r| Some(r.key()) == *old)
                .cloned()
        };
        if !self.0.config_apply.is_sensitive() || oldrow.is_none() {
            self.0.oldhwkey.replace(key);
            self.refresh_page();
            return;
        }
        let this = self.clone();
        glib::spawn_future_local(async move {
            if this.has_unapplied_changes(oldrow).await {
                // Unapplied changes, and syncing them failed
                this.select_key(this.0.oldhwkey.borrow().clone());
            } else {
                this.0.oldhwkey.replace(key);
                this.refresh_page();
            }
        });
    }

    /// `_set_hw_selection(..., _disable_apply=False)`, by key and
    /// without re-entering [`Self::hw_changed`].
    fn select_key(&self, key: Option<String>) {
        let i = &self.0;
        let idx = key
            .and_then(|k| i.rows.borrow().iter().position(|r| r.key() == k))
            .unwrap_or(0);
        i.repopulating.set(true);
        i.hw_list
            .select_row(i.hw_list.row_at_index(idx as i32).as_ref());
        i.repopulating.set(false);
    }

    /// `_has_unapplied_changes`: `false` when nothing is pending, the
    /// user declines applying, or applying works; `true` when applying
    /// failed, and the caller should abort what it was about to do.
    async fn has_unapplied_changes(&self, row: Option<HwRow>) -> bool {
        let Some(row) = row else {
            return false;
        };
        if !self.0.config_apply.is_sensitive() {
            return false;
        }
        if !crate::error::confirm_unapplied_changes(self.parent().as_ref()).await {
            return false;
        }
        !self.config_apply(Some(row)).await
    }

    /// `_refresh_page`.
    fn refresh_page(&self) {
        let Some(row) = self.get_hw_row() else {
            return;
        };
        let i = &self.0;
        i.refreshing.set(true);
        i.config_remove.set_sensitive(true);
        i.config_remove
            .set_tooltip_text(Some("Remove this device from the virtual machine"));
        match row.hwtype {
            HwType::General => {
                self.refresh_overview_page();
                i.hw_panel.set_visible_child_name("overview");
            }
            HwType::Cpu => {
                self.refresh_config_cpu();
                i.hw_panel.set_visible_child_name("cpu");
            }
            HwType::Memory => {
                self.refresh_config_memory();
                i.hw_panel.set_visible_child_name("memory");
            }
            HwType::Boot => {
                self.refresh_boot_page();
                i.hw_panel.set_visible_child_name("boot");
            }
            HwType::Os => {
                self.refresh_os_page();
                i.hw_panel.set_visible_child_name("os");
            }
            HwType::Disk => {
                self.refresh_disk_page(&row);
                i.hw_panel.set_visible_child_name("disk");
            }
            HwType::Nic => {
                self.refresh_network_page(&row);
                i.hw_panel.set_visible_child_name("network");
            }
            HwType::Input
            | HwType::Sound
            | HwType::Video
            | HwType::Watchdog
            | HwType::Smartcard
            | HwType::Redirdev
            | HwType::Rng
            | HwType::Panic => self.refresh_simple_device_page(&row),
            HwType::Graphics => {
                self.refresh_graphics_page(&row);
                i.hw_panel.set_visible_child_name("graphics");
            }
            HwType::Char => {
                self.refresh_char_page(&row);
                i.hw_panel.set_visible_child_name("char");
            }
            HwType::Controller => {
                self.refresh_controller_page(&row);
                i.hw_panel.set_visible_child_name("controller");
            }
            HwType::Hostdev => {
                self.refresh_hostdev_page(&row);
                i.hw_panel.set_visible_child_name("hostdev");
            }
            HwType::Tpm => {
                self.refresh_tpm_page(&row);
                i.hw_panel.set_visible_child_name("tpm");
            }
            HwType::Vsock => {
                if let Some((x, el)) = self.device_element(&row) {
                    i.vsock.set_dev(&x.doc, el, i.vm.is_active());
                }
                i.hw_panel.set_visible_child_name("vsock");
            }
            HwType::Filesystem => {
                self.refresh_filesystem_page(&row);
                i.hw_panel.set_visible_child_name("filesystem");
            }
            _ => i.hw_panel.set_visible_child(&i.unported),
        }
        self.check_device_remove(&row);
        i.refreshing.set(false);
        self.disable_apply();
        i.config_remove.set_visible(row.hwtype.is_removable());
    }

    /// `_init_menus` and `_popup_addhw_menu_cb`: the hardware list's
    /// right-click menu.
    fn init_menus(&self) {
        let i = &self.0;
        let actions = gio::SimpleActionGroup::new();
        // The Add Hardware wizard isn't ported yet.
        let addhw = gio::SimpleAction::new("add-hardware", None);
        addhw.set_enabled(false);
        actions.add_action(&addhw);
        let rmhw = gio::SimpleAction::new("remove-hardware", None);
        let weak = Rc::downgrade(&self.0);
        rmhw.connect_activate(move |_, _| {
            if let Some(inner) = weak.upgrade() {
                let this = VmmDetails(inner);
                glib::spawn_future_local(async move { this.config_remove().await });
            }
        });
        actions.add_action(&rmhw);
        i.top_box.insert_action_group("hw", Some(&actions));

        // Not parented to the list itself, which treats every child as
        // a row when it is repopulated.
        let popover = gtk::PopoverMenu::from_model(None::<&gio::MenuModel>);
        popover.set_parent(&i.top_box);
        popover.set_has_arrow(false);
        popover.set_halign(gtk::Align::Start);

        let click = gtk::GestureClick::new();
        click.set_button(gtk::gdk::BUTTON_SECONDARY);
        let weak = Rc::downgrade(&self.0);
        click.connect_pressed(move |_, _, x, y| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            // force select the list entry before showing popup_menu
            let Some(listrow) = inner.hw_list.row_at_y(y as i32) else {
                return;
            };
            inner.hw_list.select_row(Some(&listrow));
            let menu = gio::Menu::new();
            menu.append(Some("_Add Hardware"), Some("hw.add-hardware"));
            if inner.config_remove.is_visible() {
                menu.append(Some("_Remove Hardware"), Some("hw.remove-hardware"));
            }
            rmhw.set_enabled(inner.config_remove.is_sensitive());
            popover.set_menu_model(Some(&menu));
            let point = inner
                .hw_list
                .compute_point(
                    &inner.top_box,
                    &gtk::graphene::Point::new(x as f32, y as f32),
                )
                .unwrap_or_else(|| gtk::graphene::Point::new(x as f32, y as f32));
            popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
                point.x() as i32,
                point.y() as i32,
                1,
                1,
            )));
            popover.popup();
        });
        i.hw_list.add_controller(click);
    }

    /// `_disable_device_remove`.
    fn disable_device_remove(&self, tooltip: &str) {
        self.0.config_remove.set_sensitive(false);
        self.0.config_remove.set_tooltip_text(Some(tooltip));
    }

    /// The `_disable_device_remove` calls of the `_refresh_*_page`s.
    fn check_device_remove(&self, row: &HwRow) {
        const UNSUPPORTED: &str = "Hypervisor does not support removing this device";
        const ATTACHED: &str = "Cannot remove controller while devices are attached.";
        let Some((x, el)) = self.device_element(row) else {
            return;
        };
        let doc = &x.doc;
        let devices = virtinst_xml::resolve_path(doc, x.root(), &["devices"]);
        let count = |tag| devices.map_or(0, |d| d.find_all(doc, tag).len());
        let a = |name| attr(doc, el, name);
        match row.hwtype {
            HwType::Disk => {
                // vmmDeleteStorage isn't ported yet
                self.disable_device_remove("Removing disks is not available yet in this version.");
            }
            HwType::Input => {
                let typ = a("type");
                if matches!(typ.as_deref(), Some("mouse" | "keyboard"))
                    && matches!(a("bus").as_deref(), Some("xen" | "ps2"))
                {
                    self.disable_device_remove(UNSUPPORTED);
                }
            }
            HwType::Video => {
                if count("graphics") > 0 && count("video") <= 1 {
                    self.disable_device_remove(
                        "Cannot remove last video device while Graphics/Display is attached.",
                    );
                }
            }
            HwType::Controller => {
                let ctype = a("type").unwrap_or_default();
                let index: Option<i64> = a("index").and_then(|i| i.parse().ok());
                let is_x86 = matches!(x.guest.os.arch.as_deref(), Some("x86_64" | "i686"));
                if is_x86 && ctype == "usb" {
                    self.disable_device_remove(UNSUPPORTED);
                }
                // `get_attached_devices`
                let attached = match ctype.as_str() {
                    "pci" => {
                        self.disable_device_remove(UNSUPPORTED);
                        false
                    }
                    "scsi" | "sata" | "ide" | "fdc" => devices.is_some_and(|d| {
                        d.find_all(doc, "disk").iter().any(|disk| {
                            let controller: Option<i64> =
                                child_attr(doc, *disk, &["address"], "controller")
                                    .and_then(|c| c.parse().ok());
                            child_attr(doc, *disk, &["target"], "bus").as_deref()
                                == Some(ctype.as_str())
                                && controller == index
                        })
                    }),
                    "virtio-serial" => devices.is_some_and(|d| {
                        let channel = d.find_all(doc, "channel").iter().any(|c| {
                            let controller: Option<i64> =
                                child_attr(doc, *c, &["address"], "controller")
                                    .and_then(|c| c.parse().ok());
                            child_attr(doc, *c, &["address"], "type").as_deref()
                                == Some("virtio-serial")
                                && controller == index
                        });
                        // virtio console is implied to be on virtio-serial index=0
                        let console = index == Some(0)
                            && d.find_all(doc, "console").iter().any(|c| {
                                child_attr(doc, *c, &["target"], "type").as_deref()
                                    == Some("virtio")
                            });
                        channel || console
                    }),
                    _ => false,
                };
                if attached {
                    self.disable_device_remove(ATTACHED);
                }
            }
            _ => {}
        }
    }

    /// `_config_remove`.
    async fn config_remove(&self) {
        let Some(row) = self.get_hw_row() else {
            return;
        };
        if !row.hwtype.is_removable() || !self.0.config_remove.is_sensitive() {
            return;
        }
        self.remove_non_disk(&row).await;
    }

    /// `_remove_non_disk`, with `remove_devobj_internal`.
    async fn remove_non_disk(&self, row: &HwRow) {
        let parent = self.parent();
        let config = crate::engine::get().config();
        let ok = crate::error::chkbox_helper(
            parent.as_ref(),
            || config.get_confirm_removedev(),
            |v| config.set_confirm_removedev(v),
            "Are you sure you want to remove this device?",
            None,
        )
        .await;
        if !ok {
            return;
        }
        let (Some(id), Some(x)) = (self.device_id(row), self.0.vm.xmlobj()) else {
            return;
        };
        let devxml = virtinst_xml::parse_libvirt_xml(&x.xml)
            .ok()
            .and_then(|doc| {
                let root = doc.root_element()?;
                let dev = virtinst_core::devices::find_device(&doc, root, &id)?;
                virtinst_xml::element_to_string(doc, dev).ok()
            })
            .unwrap_or_default();
        glib::g_debug!("virt-manager", "Removing device: {devxml}");

        // Define the change
        if let Err(e) = self.0.vm.remove_device(&id).await {
            crate::error::show_err(
                parent.as_ref(),
                &format!("Error Removing Device: {e}"),
                None,
                "",
            )
            .await;
            return;
        }

        // Try to hot remove
        if let Err(e) = self.0.vm.detach_device(devxml).await {
            glib::g_debug!("virt-manager", "Device could not be hotUNplugged: {e}");
            crate::error::show_info(
                parent.as_ref(),
                "Device could not be removed from the running machine",
                Some(&format!(
                    "This change will take effect after the next guest shutdown.\n\n{e}"
                )),
                None,
                crate::error::Buttons::Ok,
            )
            .await;
            return;
        }

        // the config change refreshes the page, rather than leaving it
        // untouched because it thinks changes are in progress
        self.disable_apply();
    }

    /// `_refresh_os_page`. Inspection (libguestfs) isn't supported, so
    /// the applications list and its refresh button stay hidden, as
    /// upstream hides them when `inspection_supported()` is false.
    fn refresh_os_page(&self) {
        let os_id = self
            .0
            .vm
            .xmlobj()
            .and_then(|x| x.guest.metadata.libosinfo.os_id.clone());
        self.0
            .os
            .os_list
            .select_os(&crate::oslist::lookup_os_by_full_id(os_id.as_deref()));
    }

    /// `vmmAddStorage`'s `_change_cb`.
    fn storage_changed(&self, edit: StorageEdit) {
        if self.0.refreshing.get() {
            return;
        }
        let mut edits = self.0.disk.storage_edits.borrow_mut();
        if !edits.contains(&edit) {
            edits.push(edit);
        }
        drop(edits);
        self.enable_apply(Edit::Disk);
    }

    /// `_disk_source_browse_clicked_cb`.
    fn disk_source_browse_clicked(&self) {
        let Some(row) = self.get_hw_row() else {
            return;
        };
        let is_floppy = self
            .device_element(&row)
            .is_some_and(|(x, el)| attr(&x.doc, el, "device").as_deref() == Some("floppy"));
        let title = if is_floppy {
            "Locate floppy media"
        } else {
            "Locate ISO media"
        };
        let dialog = gtk::FileDialog::builder().title(title).modal(true).build();
        let combo = self.0.disk.mediacombo.clone();
        dialog.open(
            self.parent().as_ref(),
            gtk::gio::Cancellable::NONE,
            move |res| {
                if let Some(path) = res.ok().and_then(|f| f.path()) {
                    combo.set_path(&path.to_string_lossy());
                }
            },
        );
    }

    /// `_set_network_ip_details`, fetched off the main thread.
    fn set_network_ip_details(&self, row: &HwRow) {
        let Some((x, el)) = self.device_element(row) else {
            return;
        };
        let mac = child_attr(&x.doc, el, &["mac"], "address").unwrap_or_default();
        let (ntype, source, network) =
            virtinst_core::devices::network::interface_source(&x.doc, el);
        let network = if ntype == "network" { source } else { network };
        let this = self.clone();
        let key = row.key();
        glib::spawn_future_local(async move {
            let (ipv4, ipv6) = this.0.vm.get_ips(mac, network).await;
            if this.get_hw_row().map(|r| r.key()) != Some(key) {
                return;
            }
            let label = [ipv4, ipv6]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("\n");
            this.0
                .network
                .ip
                .set_text(if label.is_empty() { "Unknown" } else { &label });
        });
    }

    /// `_refresh_network_page`.
    fn refresh_network_page(&self, row: &HwRow) {
        let Some((x, el)) = self.device_element(row) else {
            return;
        };
        let doc = &x.doc;
        let n = &self.0.network;
        let os = &x.guest.os;
        n.model
            .set_modes(&virtinst_core::devices::network::recommended_models(
                x.guest.domain_type.as_deref().unwrap_or(""),
                os.os_type.as_deref().unwrap_or(""),
                os.arch.as_deref().unwrap_or(""),
                os.machine.as_deref().unwrap_or(""),
            ));
        n.model
            .set(child_attr(doc, el, &["model"], "type").as_deref());
        n.mac
            .set_text(&child_attr(doc, el, &["mac"], "address").unwrap_or_default());
        let state = child_attr(doc, el, &["link"], "state");
        n.link_state
            .set_active(state.is_none() || state.as_deref() == Some("up"));
        n.ip.set_text("Unknown");
        self.set_network_ip_details(row);

        let (nettype, source, network) = virtinst_core::devices::network::interface_source(doc, el);
        n.netlist.set_dev(crate::netlist::NetDev {
            nettype,
            source,
            network,
            portgroup: child_attr(doc, el, &["source"], "portgroup"),
        });
    }

    /// `_refresh_disk_page`.
    fn refresh_disk_page(&self, row: &HwRow) {
        let Some((x, el)) = self.device_element(row) else {
            return;
        };
        let doc = &x.doc;
        let d = &self.0.disk;
        let path = virtinst_core::devices::disk::source_path(doc, el);
        let volume = virtinst_core::devices::disk::source_volume(doc, el);
        let devtype = attr(doc, el, "device").unwrap_or_else(|| "disk".into());
        let bus = child_attr(doc, el, &["target"], "bus");

        let has_storage = path.is_some() || volume.is_some();
        d.size.set_text(if has_storage { "Unknown" } else { "-" });
        d.target_type.set_text(&row.label);
        d.bus_label.set_text(
            &bus.as_deref()
                .map(disk_pretty_bus)
                .filter(|b| !b.is_empty())
                .unwrap_or_else(|| "-".into()),
        );

        let is_cdrom = devtype == "cdrom";
        let is_removable = is_cdrom || devtype == "floppy";
        d.source_box.set_visible(is_removable);
        d.source_label.set_visible(!is_removable);
        d.source_label.set_text(path.as_deref().unwrap_or("-"));
        if is_removable {
            d.mediacombo.reset_state(devtype == "floppy");
            d.mediacombo.set_path(path.as_deref().unwrap_or(""));
        }

        // vmmAddStorage.set_dev
        d.set_disk_bus(bus.as_deref());
        d.cache
            .set(child_attr(doc, el, &["driver"], "cache").as_deref());
        d.discard
            .set(child_attr(doc, el, &["driver"], "discard").as_deref());
        d.serial.set_text(
            &virtinst_xml::resolve_path(doc, el, &["serial"])
                .map(|e| e.text_content(doc))
                .unwrap_or_default(),
        );
        d.readonly.set_active(el.find(doc, "readonly").is_some());
        d.readonly.set_sensitive(!is_cdrom);
        d.shareable.set_active(el.find(doc, "shareable").is_some());
        d.removable
            .set_active(child_attr(doc, el, &["target"], "removable").as_deref() == Some("on"));
        d.storage_edits.borrow_mut().clear();

        // conn.get_vol_by_path, off the main thread
        if !has_storage {
            return;
        }
        let this = self.clone();
        let key = row.key();
        glib::spawn_future_local(async move {
            let info = this.0.vm.disk_volume_info(path.clone(), volume).await;
            if this.get_hw_row().map(|r| r.key()) != Some(key) {
                return;
            }
            let d = &this.0.disk;
            if let Some((volpath, capacity)) = info {
                d.size.set_text(&pretty_bytes(capacity));
                if path.is_none() {
                    d.source_label.set_text(&volpath);
                }
            }
        });
    }

    /// `_refresh_overview_page`.
    fn refresh_overview_page(&self) {
        let i = &self.0;
        let o = &i.overview;
        let vm = &i.vm;
        o.name.set_text(&vm.get_name());
        o.uuid.set_text(&vm.get_uuid());
        o.description.buffer().set_text(&vm.get_description());
        o.title.set_text(&vm.get_title().unwrap_or_default());

        let Some(x) = vm.xmlobj() else {
            return;
        };
        let os = &x.guest.os;
        o.hv.set_text(&vm.get_pretty_hv_type());
        o.arch.set_text(os.arch.as_deref().unwrap_or("Unknown"));
        o.emulator
            .set_text(x.emulator().as_deref().unwrap_or("None"));

        let firmware = if os.firmware.as_deref() == Some("efi") {
            "UEFI".to_string()
        } else {
            label_for_firmware_path(os.arch.as_deref(), os.loader.as_deref())
        };
        o.firmware_label.set_text(&firmware);

        let machtype = os.machine.clone().unwrap_or_else(|| "Unknown".into());
        o.machine_label.set_text(&machtype);
        o.chipset_label
            .set_text(chipset_label_from_machine(Some(&machtype)));

        // `_init_details`' row visibility.
        let arch = os.arch.as_deref();
        let is_x86 = matches!(arch, Some("i686" | "x86_64"));
        Overview::set_row_visible(&o.machine_title, &o.machine_label, !is_x86);
        let uri = vm.conn().map(|c| c.get_uri()).unwrap_or_default();
        let qemu_or_test = uri.starts_with("qemu") || uri.starts_with("test");
        Overview::set_row_visible(&o.chipset_title, &o.chipset_label, qemu_or_test && is_x86);
        let is_hvm = os.os_type.as_deref() == Some("hvm");
        Overview::set_row_visible(&o.firmware_title, &o.firmware_label, false);
        let this = self.clone();
        let arch = arch.map(str::to_string);
        glib::spawn_future_local(async move {
            let efi = this
                .0
                .vm
                .get_domain_capabilities()
                .await
                .is_some_and(|c| c.supports_firmware_efi());
            let o = &this.0.overview;
            Overview::set_row_visible(
                &o.firmware_title,
                &o.firmware_label,
                is_hvm && (efi || arch_can_uefi(arch.as_deref())),
            );
        });
    }

    /// Fills in widgets after an async lookup without counting it as
    /// an edit.
    fn quietly(&self, f: impl FnOnce()) {
        let was = self.0.refreshing.replace(true);
        f();
        self.0.refreshing.set(was);
    }

    /// `_refresh_config_cpu`.
    fn refresh_config_cpu(&self) {
        let i = &self.0;
        let c = &i.cpu;
        let Some(x) = i.vm.xmlobj() else {
            return;
        };
        self.init_cpu_models();
        // Set topology first, because it impacts vcpus values
        let cpu = &x.guest.cpu;
        let show_top = cpu.has_topology();
        c.topology_enable.set_active(show_top);
        c.topology_table.set_sensitive(show_top);
        c.sockets
            .set_value(f64::from(cpu.topology_sockets.unwrap_or(1)));
        c.cores
            .set_value(f64::from(cpu.topology_cores.unwrap_or(1)));
        c.threads
            .set_value(f64::from(cpu.topology_threads.unwrap_or(1)));
        if show_top {
            c.topology_expander.set_expanded(true);
        }

        let host_active_count =
            i.vm.conn()
                .map(|c| c.host_active_processor_count())
                .unwrap_or(0);
        c.vcpus.set_value(f64::from(x.guest.vcpus.unwrap_or(1)));
        c.host_cpus.set_text(&host_active_count.to_string());

        // Trigger this again to make sure vcpus is correct
        self.sync_cpu_topology_ui();
        self.update_vcpus_warning();

        // CPU model config
        let mode = cpu.mode.as_deref();
        let is_host = matches!(mode, Some("host-model" | "host-passthrough"));
        let special_mode = mode.filter(|m| cpu::SPECIAL_MODES.contains(m));
        let model = cpu
            .model
            .clone()
            .filter(|m| !m.is_empty())
            .or_else(|| special_mode.map(str::to_string));
        match model {
            Some(model) => c.model.entry.set_text(&model),
            None => c.model.select_id(cpu::SPECIAL_MODE_HV_DEFAULT),
        }

        c.copy_host.set_active(is_host);
        let mut text = "Copy host CP_U configuration".to_string();
        if is_host {
            text += &format!(" ({})", mode.unwrap_or_default());
        }
        c.copy_host.set_label(Some(&text));
        self.cpu_copy_host_clicked();

        // `_cpu_secure_is_available` and `check_security_features`
        // need the expanded host CPU, which takes a libvirt call.
        let is_x86 = matches!(x.guest.os.arch.as_deref(), Some("x86_64" | "i686"));
        let guest_cpu = cpu.clone();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let features = this.0.vm.get_cpu_security_features().await;
            let secure = &this.0.cpu.secure;
            let available = is_x86 && !features.is_empty();
            secure.set_sensitive(available);
            secure.set_tooltip_text((!available).then_some(
                "No security features to copy, the host is missing \
                 security patches or the host CPU is not vulnerable.",
            ));
            let active = guest_cpu.check_security_features(&features);
            this.quietly(|| secure.set_active(active));
        });
    }

    /// `_refresh_config_memory`.
    fn refresh_config_memory(&self) {
        let i = &self.0;
        let m = &i.memory;
        let Some(x) = i.vm.xmlobj() else {
            return;
        };
        let host_mem = i.vm.conn().map(|c| c.host_memory_size()).unwrap_or(0) / 1024;
        let vm_cur_mem = f64::from(x.guest.current_memory.unwrap_or(0)) / 1024.0;
        let vm_max_mem = f64::from(x.guest.memory.unwrap_or(0)) / 1024.0;
        m.host_memory.set_text(&format!("{host_mem} MiB"));
        m.memory.set_value(vm_cur_mem.round());
        m.maxmem.set_value(vm_max_mem.round());

        let this = self.clone();
        glib::spawn_future_local(async move {
            let (shared_mem, shared_mem_err) = this.0.vm.has_shared_mem().await;
            let shared = &this.0.memory.shared;
            this.quietly(|| shared.set_active(shared_mem));
            shared.set_sensitive(shared_mem_err.is_none());
            shared.set_tooltip_text(shared_mem_err.as_deref());
        });
    }

    /// `_init_details`' CPU model list, from domcaps (which needs the
    /// XML, so it is retried until that has arrived).
    fn init_cpu_models(&self) {
        if self.0.cpu.model.loaded.replace(true) {
            return;
        }
        let this = self.clone();
        glib::spawn_future_local(async move {
            match this.0.vm.get_domain_capabilities().await {
                Some(caps) => this.0.cpu.model.set_rows(caps.get_cpu_models()),
                None => this.0.cpu.model.loaded.set(false),
            }
        });
    }

    /// `_refresh_vm_state`: the status line.
    fn refresh_vm_state(&self) {
        let vm = &self.0.vm;
        let o = &self.0.overview;
        o.name.set_editable(!vm.is_active());
        let status = match vm.run_status_reason() {
            Some(reason) => format!("{} ({reason})", vm.run_status()),
            None => vm.run_status().to_string(),
        };
        o.status_text.set_text(&status);
        o.status_icon.set_icon_name(Some(vm.run_status_icon_name()));
    }

    /// `vmwindow_refresh_vm_state`.
    pub fn vmwindow_refresh_vm_state(&self, is_current_page: bool) {
        if !is_current_page {
            self.disable_apply();
            return;
        }
        self.refresh_vm_state();
        self.repopulate_hw_list();
        if self.0.config_apply.is_sensitive() {
            // Apply button sensitive means user is making changes,
            // don't erase them
            return;
        }
        self.refresh_page();
    }

    /// Whether [`Self::vmwindow_has_unapplied_changes`] would prompt:
    /// lets the caller hold its action without waiting.
    pub fn has_pending_edits(&self) -> bool {
        self.0.config_apply.is_sensitive()
    }

    /// `vmwindow_has_unapplied_changes`.
    pub async fn vmwindow_has_unapplied_changes(&self) -> bool {
        self.has_unapplied_changes(self.get_hw_row()).await
    }

    pub fn vmwindow_close(&self) {
        self.disable_apply();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<domain type="kvm">
  <name>t</name>
  <os><type arch="x86_64" machine="pc-q35-8.2">hvm</type></os>
  <devices>
    <emulator>/usr/bin/qemu-system-x86_64</emulator>
    <disk type="file" device="disk"><target dev="vda" bus="virtio"/></disk>
    <disk type="file" device="cdrom"><target dev="sda" bus="sata"/></disk>
    <disk type="file" device="disk"><target dev="vdb" bus="virtio"/></disk>
    <controller type="usb" index="0" model="qemu-xhci"/>
    <controller type="pci" index="0" model="pcie-root"/>
    <controller type="pci" index="1" model="pcie-root-port"/>
    <interface type="network"><mac address="52:54:00:6b:3c:58"/></interface>
    <serial type="pty"><target port="0"/></serial>
    <console type="pty"><target type="serial" port="0"/></console>
    <channel type="unix"><target type="virtio" name="org.qemu.guest_agent.0"/></channel>
    <channel type="spicevmc"><target type="virtio" name="com.redhat.spice.0"/></channel>
    <input type="tablet" bus="usb"/>
    <input type="mouse" bus="ps2"/>
    <graphics type="spice" autoport="yes"/>
    <sound model="ich9"/>
    <video><model type="qxl"/></video>
    <hostdev mode="subsystem" type="usb"><source><vendor id="0x046d"/><product id="0xc52b"/></source></hostdev>
    <redirdev bus="usb" type="spicevmc"/>
    <redirdev bus="usb" type="spicevmc"/>
    <rng model="virtio"><backend model="random">/dev/urandom</backend></rng>
    <tpm model="tpm-crb"><backend type="emulator" version="2.0"/></tpm>
  </devices>
</domain>"#;

    #[test]
    fn device_rows_match_upstream_labels_and_order() {
        let x = DomainXml::parse(XML.to_string()).unwrap();
        let rows = device_rows(&x);
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "VirtIO Disk 1",
                "SATA CDROM 1",
                "VirtIO Disk 2",
                "NIC :6b:3c:58",
                "Tablet",
                "Mouse",
                "Display Spice",
                "Sound ich9",
                "Serial 1",
                "Channel (qemu-ga)",
                "Channel (spice)",
                "USB 046d:c52b",
                "Video QXL",
                "Controller USB 0",
                "Controller PCIe 0",
                "USB Redirector 1",
                "USB Redirector 2",
                "TPM v2.0",
                "RNG /dev/urandom",
            ]
        );
        assert_eq!(rows[1].icon, "media-optical");
        assert_eq!(rows[11].icon, "device_usb");
        assert_eq!(rows[16].key(), "./devices/redirdev[2]");
    }

    #[test]
    fn firmware_labels() {
        assert_eq!(label_for_firmware_path(Some("x86_64"), None), "BIOS");
        assert_eq!(label_for_firmware_path(Some("aarch64"), None), "Default");
        assert_eq!(
            label_for_firmware_path(Some("x86_64"), Some("/usr/share/OVMF/OVMF_CODE.fd")),
            "UEFI x86_64: /usr/share/OVMF/OVMF_CODE.fd"
        );
        assert_eq!(
            label_for_firmware_path(Some("x86_64"), Some("/opt/fw.bin")),
            "Custom: /opt/fw.bin"
        );
    }

    #[test]
    fn pretty_helpers() {
        assert_eq!(graphics_pretty_type_simple("vnc"), "VNC");
        assert_eq!(graphics_pretty_type_simple("spice"), "Spice");
        assert_eq!(video_pretty_model("virtio"), "Virtio");
        assert_eq!(chipset_label_from_machine(Some("pc-i440fx-8.2")), "i440FX");
        assert_eq!(chipset_label_from_machine(Some("pc-q35-8.2")), "Q35");
    }
}
