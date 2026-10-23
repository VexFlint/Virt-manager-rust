//! Upstream's `virtManager/details/details.py` (`vmmDetails`): the VM
//! window's Details page. Left, the `hw-list` sidebar (fixed rows for
//! Overview/OS/Performance/CPUs/Memory/Boot plus one row per device);
//! right, the page for the selected row; below, Remove / Cancel / Apply.
//!
//! Per ticket 10 the sidebar is a plain `GtkListBox` with directly built
//! rows. This first slice ports the sidebar (labels and icons exactly as
//! upstream computes them) and the Overview page's read-only view; the
//! other pages and the edit/Apply machinery arrive in the next slices.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use virtinst_xml::{Document, Element};

use crate::domain::{DomainXml, VmmDomain};

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

    let serials = all("serial");
    let mut consoles = all("console");
    // serial_is_console_dup: the first <console> mirroring the first
    // <serial> isn't listed twice.
    if let (Some(&(_, serial)), Some(&(_, console))) = (serials.first(), consoles.first()) {
        let ctarget = child_attr(doc, console, &["target"], "type");
        if attr(doc, console, "type") == attr(doc, serial, "type")
            && ctarget.as_deref().map_or(true, |t| t == "serial")
        {
            consoles.remove(0);
        }
    }

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
    label.set_markup(&format!("<b>{title}</b>"));
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

        // Edits are wired up with the edit/Apply port; until then the
        // fields are display-only.
        name.set_editable(false);
        title.set_editable(false);
        description.set_editable(false);

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

struct Inner {
    vm: VmmDomain,
    is_customize_dialog: bool,
    top_box: gtk::Box,
    hw_list: gtk::ListBox,
    rows: RefCell<Vec<HwRow>>,
    hw_panel: gtk::Stack,
    overview: Overview,
    unported: gtk::Label,
    config_remove: gtk::Button,
    config_cancel: gtk::Button,
    config_apply: gtk::Button,
    oldhwkey: RefCell<Option<String>>,
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
        let unported = gtk::Label::new(Some("This page is not available yet in this version."));
        unported.set_valign(gtk::Align::Start);
        unported.set_xalign(0.0);
        let hw_panel = gtk::Stack::new();
        hw_panel.add_named(&overview.page, Some("overview"));
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
            unported,
            config_remove,
            config_cancel,
            config_apply,
            oldhwkey: RefCell::default(),
            repopulating: Cell::new(false),
        }));
        this.disable_apply();
        // Device removal arrives with the edit port.
        this.0.config_remove.set_sensitive(false);
        let weak = Rc::downgrade(&this.0);
        this.0.hw_list.connect_row_selected(move |_, _| {
            if let Some(inner) = weak.upgrade() {
                VmmDetails(inner).hw_changed();
            }
        });
        this.repopulate_hw_list();
        this
    }

    pub fn top_box(&self) -> &gtk::Box {
        &self.0.top_box
    }

    fn disable_apply(&self) {
        self.0.config_apply.set_sensitive(false);
        self.0.config_cancel.set_sensitive(false);
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

    /// `_hw_changed_cb`.
    fn hw_changed(&self) {
        if self.0.repopulating.get() {
            return;
        }
        let Some(row) = self.get_hw_row() else {
            return;
        };
        let key = Some(row.key());
        if *self.0.oldhwkey.borrow() == key && self.0.hw_panel.visible_child_name().is_some() {
            self.refresh_page();
            return;
        }
        self.0.oldhwkey.replace(key);
        self.refresh_page();
    }

    /// `_refresh_page`.
    fn refresh_page(&self) {
        let Some(row) = self.get_hw_row() else {
            return;
        };
        let i = &self.0;
        match row.hwtype {
            HwType::General => {
                self.refresh_overview_page();
                i.hw_panel.set_visible_child_name("overview");
            }
            _ => i.hw_panel.set_visible_child(&i.unported),
        }
        self.disable_apply();
        i.config_remove.set_visible(row.hwtype.is_removable());
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

    /// `_refresh_vm_state`: the status line.
    fn refresh_vm_state(&self) {
        let vm = &self.0.vm;
        let o = &self.0.overview;
        o.name.set_sensitive(true);
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

    /// `vmwindow_has_unapplied_changes`: nothing is editable yet.
    pub fn vmwindow_has_unapplied_changes(&self) -> bool {
        false
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
