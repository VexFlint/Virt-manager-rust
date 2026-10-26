//! New devices for Add Hardware: each builder makes a detached element
//! from what the wizard page picked, with the device's `set_defaults`
//! applied, ready to append to `<devices>` or serialize for
//! `attachDevice`. [`GuestInfo`] carries the guest facts those defaults
//! and the pages' default selections depend on.

use virtinst_xml::{Document, Element};

use super::edit::set_attr;
use crate::capabilities::domcaps::DomainCapabilities;
use crate::nodedev::NodeDevice;

/// `DeviceChannel.CHANNEL_NAMES`.
pub const CHANNEL_NAME_SPICE: &str = "com.redhat.spice.0";
pub const CHANNEL_NAME_QEMUGA: &str = "org.qemu.guest_agent.0";
pub const CHANNEL_NAME_LIBGUESTFS: &str = "org.libguestfs.channel.0";
pub const CHANNEL_NAME_SPICE_WEBDAV: &str = "org.spice-space.webdav.0";
pub const CHANNEL_NAMES: [&str; 4] = [
    CHANNEL_NAME_SPICE,
    CHANNEL_NAME_QEMUGA,
    CHANNEL_NAME_LIBGUESTFS,
    CHANNEL_NAME_SPICE_WEBDAV,
];

/// The guest facts device defaults read: `guest.os`, its graphics and
/// the osinfo answers, taken from the domain XML by the caller.
#[derive(Clone, Debug, Default)]
pub struct GuestInfo {
    pub os_type: Option<String>,
    pub arch: Option<String>,
    pub machine: Option<String>,
    pub domain_type: Option<String>,
    pub has_spice: bool,
    pub has_gl: bool,
    pub is_uefi: bool,
    /// `guest.osinfo.supports_virtiogpu()`.
    pub os_virtiogpu: bool,
    /// `guest.osinfo.supports_virtionet()`.
    pub os_virtionet: bool,
    /// `guest.osinfo.supported_netmodels()`.
    pub os_netmodels: Vec<String>,
    /// `guest.osinfo.supports_virtioscsi()`.
    pub os_virtioscsi: bool,
}

impl GuestInfo {
    pub fn is_hvm(&self) -> bool {
        self.os_type.as_deref() == Some("hvm")
    }
    pub fn is_container(&self) -> bool {
        self.os_type.as_deref() == Some("exe")
    }
    fn arch(&self) -> &str {
        self.arch.as_deref().unwrap_or_default()
    }
    fn machine(&self) -> &str {
        self.machine.as_deref().unwrap_or_default()
    }
    pub fn is_x86(&self) -> bool {
        matches!(self.arch(), "x86_64" | "i686")
    }
    pub fn is_q35(&self) -> bool {
        self.is_x86() && self.machine().contains("q35")
    }
    fn is_arm(&self) -> bool {
        matches!(self.arch(), "armv7l" | "aarch64")
    }
    fn is_arm_machvirt(&self) -> bool {
        self.is_arm() && self.machine().starts_with("virt")
    }
    fn is_pseries(&self) -> bool {
        matches!(self.arch(), "ppc64" | "ppc64le") && self.machine().starts_with("pseries")
    }
    fn is_s390x(&self) -> bool {
        self.arch() == "s390x"
    }
    fn is_riscv_virt(&self) -> bool {
        matches!(self.arch(), "riscv64" | "riscv32") && self.machine().starts_with("virt")
    }
    fn is_loongarch64(&self) -> bool {
        self.arch() == "loongarch64"
    }

    /// `defaults_to_pcie`.
    pub fn defaults_to_pcie(&self) -> bool {
        self.is_q35() || self.is_arm_machvirt() || self.is_riscv_virt() || self.is_loongarch64()
    }

    /// `DeviceSound.default_model`.
    pub fn default_sound_model(&self) -> &'static str {
        if self.defaults_to_pcie() {
            "ich9"
        } else {
            "ich6"
        }
    }

    /// `Guest.supports_virtioscsi`. `is_qemu` is the connection's.
    pub fn supports_virtioscsi(&self, is_qemu: bool) -> bool {
        crate::install::domain_skeleton::supports_virtio(
            self.arch(),
            self.machine.as_deref(),
            is_qemu,
            self.os_virtioscsi,
        )
    }

    /// `DeviceInterface.default_model`. `is_qemu` is the connection's.
    pub fn default_nic_model(&self, is_qemu: bool) -> Option<&'static str> {
        crate::install::default_nic_model(
            crate::install::GuestCapsDefaults {
                domain_type: self.domain_type.as_deref().unwrap_or_default(),
                os_type: self.os_type.as_deref().unwrap_or_default(),
                arch: self.arch(),
                machine: self.machine.as_deref(),
                emulator: None,
                is_qemu,
            },
            self.os_virtionet,
            &self.os_netmodels,
        )
    }

    /// `DeviceVideo.default_model`. `is_qemu` is the connection's.
    pub fn default_video_model(
        &self,
        domcaps: Option<&DomainCapabilities>,
        is_qemu: bool,
    ) -> Option<&'static str> {
        if !self.is_hvm() {
            return None;
        }
        if self.is_pseries() {
            return Some("vga");
        }
        if self.is_loongarch64() || self.is_riscv_virt() {
            return Some("virtio");
        }
        if self.is_s390x() && is_qemu {
            return Some("virtio");
        }
        if self.has_spice && self.has_gl {
            return Some("virtio");
        }
        let video = domcaps.map(|d| &d.devices.video);
        let model_type = video.and_then(|v| v.get_enum("modelType"));
        let has_model = |m: &str| model_type.is_some_and(|e| e.has_value(m));
        if has_model("virtio") && self.os_virtiogpu {
            return Some("virtio");
        }
        // `supports_video_qxl`: spice support stands in when domcaps
        // has no model list.
        let qxl = match model_type {
            Some(e) => e.has_value("qxl"),
            None => domcaps.is_some_and(|d| {
                d.devices
                    .graphics
                    .get_enum("type")
                    .is_some_and(|e| e.has_value("spice"))
            }),
        };
        if self.is_x86() && self.has_spice && qxl {
            return Some("qxl");
        }
        if self.is_uefi {
            if self.is_arm() {
                return Some("ramfb");
            }
            if has_model("bochs") {
                return Some("bochs");
            }
        }
        Some("vga")
    }
}

/// `DeviceTpm.default_model`.
pub fn default_tpm_model(
    domcaps: Option<&DomainCapabilities>,
    guest: &GuestInfo,
) -> Option<&'static str> {
    let tpm = domcaps.map(|d| &d.devices.tpm);
    let present = tpm.is_some_and(|t| t.supported == Some(true));
    if !present && !guest.is_pseries() {
        return Some(super::tpm::MODEL_CRB);
    }
    if tpm
        .and_then(|t| t.get_enum("model"))
        .is_some_and(|e| e.has_value(super::tpm::MODEL_CRB))
    {
        return Some(super::tpm::MODEL_CRB);
    }
    None
}

/// `domcaps.supported_panic_models()`.
pub fn supported_panic_models(domcaps: Option<&DomainCapabilities>) -> Vec<String> {
    domcaps
        .and_then(|d| d.devices.panic.get_enum("model"))
        .map(|e| e.values.clone())
        .unwrap_or_default()
}

fn new(doc: &mut Document, tag: &str) -> Element {
    Element::new(doc, tag)
}

fn set(doc: &mut Document, el: Element, path: &str, name: &str, val: Option<&str>) {
    if val.is_some() {
        set_attr(doc, el, path, name, val);
    }
}

/// `_build_input`.
pub fn input(doc: &mut Document, typ: &str, bus: &str) -> Element {
    let el = new(doc, "input");
    el.set_attribute(doc, "type", typ);
    el.set_attribute(doc, "bus", bus);
    el
}

/// `_build_sound`; `set_defaults` fills in the guest's default model.
pub fn sound(doc: &mut Document, model: Option<&str>, guest: &GuestInfo) -> Element {
    let el = new(doc, "sound");
    el.set_attribute(doc, "model", model.unwrap_or(guest.default_sound_model()));
    el
}

/// `_build_video`. `model` is the page's pick; `default_model` is
/// [`GuestInfo::default_video_model`], for an unset pick.
pub fn video(
    doc: &mut Document,
    model: Option<&str>,
    default_model: Option<&str>,
    guest: &GuestInfo,
) -> Element {
    let el = new(doc, "video");
    let model = model.or(default_model);
    set(doc, el, "model", "type", model);
    if model == Some("virtio") && guest.has_gl {
        set(doc, el, "model/acceleration", "accel3d", Some("yes"));
    }
    el
}

/// `_build_watchdog`.
pub fn watchdog(doc: &mut Document, model: Option<&str>, action: Option<&str>) -> Element {
    let el = new(doc, "watchdog");
    el.set_attribute(doc, "model", model.unwrap_or("i6300esb"));
    el.set_attribute(doc, "action", action.unwrap_or("reset"));
    el
}

/// `_build_smartcard`: passthrough cards go through a spicevmc channel.
pub fn smartcard(doc: &mut Document, mode: Option<&str>) -> Element {
    let el = new(doc, "smartcard");
    let mode = mode.unwrap_or("passthrough");
    el.set_attribute(doc, "mode", mode);
    if mode == "passthrough" {
        el.set_attribute(doc, "type", "spicevmc");
    } else {
        el.set_attribute(doc, "type", "tcp");
    }
    el
}

/// `_build_usbredir`.
pub fn redirdev(doc: &mut Document, typ: Option<&str>) -> Element {
    let el = new(doc, "redirdev");
    el.set_attribute(doc, "bus", "usb");
    el.set_attribute(doc, "type", typ.unwrap_or("spicevmc"));
    el
}

/// `_build_rng`: a `/dev/random`-style backend.
pub fn rng(doc: &mut Document, device: &str) -> Element {
    let el = new(doc, "rng");
    el.set_attribute(doc, "model", "virtio");
    Element::build("backend")
        .attribute("model", "random")
        .add_text(device)
        .push_to(doc, el);
    el
}

/// `_build_panic`: no model asks libvirt for its default (`set_stub`).
pub fn panic(doc: &mut Document, model: Option<&str>) -> Element {
    let el = new(doc, "panic");
    set(doc, el, "", "model", model);
    el
}

/// `_build_vsock`.
pub fn vsock(doc: &mut Document, auto_cid: bool, cid: u32) -> Element {
    let el = new(doc, "vsock");
    el.set_attribute(doc, "model", "virtio");
    let c = Element::build("cid")
        .attribute("auto", if auto_cid { "yes" } else { "no" })
        .push_to(doc, el);
    if !auto_cid {
        c.set_attribute(doc, "address", cid.to_string());
    }
    el
}

/// What the char page picked; `None` fields were hidden.
#[derive(Clone, Debug, Default)]
pub struct NewChar {
    /// `serial`, `parallel`, `console` or `channel`.
    pub tag: String,
    pub char_type: String,
    pub source_path: Option<String>,
    pub source_channel: Option<String>,
    pub clipboard: Option<bool>,
    pub target_name: Option<String>,
    pub target_type: Option<String>,
}

/// `_build_char` plus `_DeviceChar.set_defaults`.
pub fn char_device(doc: &mut Document, c: &NewChar) -> Element {
    let el = new(doc, &c.tag);
    el.set_attribute(doc, "type", &c.char_type);
    let mut target_type = c.target_type.clone();
    let mut target_name = c.target_name.clone().filter(|n| !n.is_empty());
    let mode = matches!(c.char_type.as_str(), "unix" | "tcp").then_some("bind");
    let path = c.source_path.as_deref().filter(|p| !p.is_empty());
    let channel = c.source_channel.as_deref().filter(|p| !p.is_empty());
    if mode.is_some() || path.is_some() || channel.is_some() || c.clipboard.is_some() {
        let source = Element::build("source").push_to(doc, el);
        set(doc, source, "", "mode", mode);
        set(doc, source, "", "path", path);
        set(doc, source, "", "channel", channel);
        if let Some(clipboard) = c.clipboard {
            Element::build("clipboard")
                .attribute("copypaste", if clipboard { "yes" } else { "no" })
                .push_to(doc, source);
        }
    }
    if target_type.is_none() && c.tag == "channel" {
        target_type = Some("virtio".into());
    }
    if target_name.is_none() && matches!(c.char_type.as_str(), "spicevmc" | "qemu-vdagent") {
        target_name = Some(CHANNEL_NAME_SPICE.into());
    }
    if target_type.is_some() || target_name.is_some() {
        let target = Element::build("target").push_to(doc, el);
        set(doc, target, "", "type", target_type.as_deref());
        set(doc, target, "", "name", target_name.as_deref());
    }
    el
}

/// `_build_controller`. `index` is one past the highest index of the
/// guest's controllers of this type, if it has any; `usb3` is
/// `get_usb3_controller`.
pub fn controller(
    doc: &mut Document,
    ctype: &str,
    model: Option<&str>,
    index: Option<u32>,
) -> Element {
    let el = new(doc, "controller");
    el.set_attribute(doc, "type", ctype);
    if let Some(index) = index {
        el.set_attribute(doc, "index", index.to_string());
    }
    match model {
        Some("usb3") => {
            el.set_attribute(doc, "model", "qemu-xhci");
            el.set_attribute(doc, "ports", "15");
        }
        Some("none") | None => {}
        Some(m) => el.set_attribute(doc, "model", m),
    }
    el
}

/// `DeviceGraphics.set_defaults` on a built `<graphics>`: auto ports
/// where a listen needs them, and no image compression for spice on a
/// local connection.
pub fn graphics_defaults(doc: &mut Document, gfx: Element, is_remote: bool) {
    let gtype = gfx.attribute(doc, "type").map(str::to_string);
    let listen = super::graphics::first_listen_type(doc, gfx);
    let need_port = listen
        .as_deref()
        .map_or(true, |l| matches!(l, "address" | "network"));
    let is_spice = gtype.as_deref() == Some("spice");
    if matches!(gtype.as_deref(), Some("vnc" | "spice"))
        && need_port
        && gfx.attribute(doc, "port").is_none()
    {
        gfx.set_attribute(doc, "port", "-1");
    }
    if is_spice && need_port && gfx.attribute(doc, "tlsPort").is_none() {
        gfx.set_attribute(doc, "tlsPort", "-1");
    }
    if is_spice
        && gfx.attribute(doc, "autoport").is_none()
        && gfx.attribute(doc, "port") == Some("-1")
        && gfx.attribute(doc, "tlsPort") == Some("-1")
    {
        gfx.set_attribute(doc, "autoport", "yes");
    }
    if is_spice && !is_remote && gfx.find(doc, "image").is_none() {
        Element::build("image")
            .attribute("compression", "off")
            .push_to(doc, gfx);
    }
}

/// `DeviceFilesystem.set_defaults` and `validate`. `driver_mode` is
/// whether the connection is qemu, lxc or test, the drivers the
/// defaults apply to.
pub fn filesystem_defaults(
    doc: &mut Document,
    fs: Element,
    driver_mode: bool,
) -> Result<(), String> {
    if driver_mode {
        if fs.attribute(doc, "type").is_none() {
            fs.set_attribute(doc, "type", super::filesystem::TYPE_MOUNT);
        }
        if fs.attribute(doc, "accessmode").is_none() {
            let driver = super::edit::attr(doc, fs, &["driver"], "type");
            if let Some(mode) = super::filesystem::default_accessmode(driver.as_deref()) {
                fs.set_attribute(doc, "accessmode", mode);
            }
        }
    }
    // `validate_target`: for qemu's default type the target is only a
    // mount tag.
    let target = super::edit::attr(doc, fs, &["target"], "dir").unwrap_or_default();
    let is_mount = fs
        .attribute(doc, "type")
        .map_or(true, |t| t == super::filesystem::TYPE_MOUNT);
    if !target.is_empty() && !(driver_mode && is_mount) && !target.starts_with('/') {
        return Err(format!(
            "Filesystem target '{target}' must be an absolute path"
        ));
    }
    Ok(())
}

/// `vmmNetworkList.build_device`: what the network page picked. `mac`
/// is always set; the page generates one when the box is unchecked, as
/// `set_defaults` would.
#[derive(Clone, Debug, Default)]
pub struct NewInterface {
    pub nettype: String,
    pub source: Option<String>,
    pub source_mode: Option<String>,
    pub portgroup: Option<String>,
    pub mac: String,
    pub model: Option<String>,
}

/// `_build_network`, with `set_defaults`' model: `default_model` is
/// [`GuestInfo::default_nic_model`], for "Hypervisor default".
pub fn interface(doc: &mut Document, n: &NewInterface, default_model: Option<&str>) -> Element {
    let el = new(doc, "interface");
    super::network::set_source(
        doc,
        el,
        &n.nettype,
        n.source.as_deref(),
        n.source_mode.as_deref(),
        n.portgroup.as_deref(),
    );
    set_attr(doc, el, "mac", "address", Some(&n.mac));
    set(
        doc,
        el,
        "model",
        "type",
        n.model.as_deref().or(default_model),
    );
    el
}

/// `re.match("^nvidia-[0-9]{2}", ..) or re.match("^i915-GVTg_V[0-9]_[0-9]", ..)`.
fn is_vfio_pci_mdev(type_id: &str) -> bool {
    let digit = |c: Option<char>| c.is_some_and(|c| c.is_ascii_digit());
    if let Some(rest) = type_id.strip_prefix("nvidia-") {
        let mut c = rest.chars();
        return digit(c.next()) && digit(c.next());
    }
    if let Some(rest) = type_id.strip_prefix("i915-GVTg_V") {
        let mut c = rest.chars();
        return digit(c.next()) && c.next() == Some('_') && digit(c.next());
    }
    false
}

/// `_build_hostdev`: `DeviceHostdev.set_from_nodedev` plus
/// `set_defaults`. `all` is the host's node devices: a USB device that
/// shares its vendor and product with another is picked by its bus
/// address too, and a `net` device passes its parent through.
pub fn hostdev(
    doc: &mut Document,
    dev: &NodeDevice,
    all: &[NodeDevice],
    is_xen: bool,
    startup_policy: Option<&str>,
) -> Result<Element, String> {
    let el = new(doc, "hostdev");
    el.set_attribute(doc, "mode", "subsystem");
    fill_hostdev(doc, el, dev, all)?;
    if el.attribute(doc, "managed").is_none() {
        el.set_attribute(doc, "managed", if is_xen { "no" } else { "yes" });
    }
    if el.attribute(doc, "type") == Some("usb") {
        set(doc, el, "source", "startupPolicy", startup_policy);
    }
    Ok(el)
}

fn fill_hostdev(
    doc: &mut Document,
    el: Element,
    dev: &NodeDevice,
    all: &[NodeDevice],
) -> Result<(), String> {
    let addr = |doc: &mut Document, name: &str, val: &Option<String>| {
        set(doc, el, "source/address", name, val.as_deref());
    };
    match dev.device_type.as_deref() {
        Some("pci") => {
            el.set_attribute(doc, "type", "pci");
            addr(doc, "domain", &dev.domain);
            addr(doc, "bus", &dev.bus);
            addr(doc, "slot", &dev.slot);
            addr(doc, "function", &dev.function);
        }
        Some("usb_device") => {
            el.set_attribute(doc, "type", "usb");
            set(doc, el, "source/vendor", "id", dev.vendor_id.as_deref());
            set(doc, el, "source/product", "id", dev.product_id.as_deref());
            let count = all
                .iter()
                .filter(|d| {
                    d.device_type.as_deref() == Some("usb_device")
                        && d.vendor_id == dev.vendor_id
                        && d.product_id == dev.product_id
                })
                .count();
            if count > 1 {
                addr(doc, "bus", &dev.bus);
                addr(doc, "device", &dev.device);
            }
        }
        Some("net") => {
            let parent = all
                .iter()
                .find(|d| Some(&d.name) == dev.parent.as_ref())
                .ok_or("Unsupported node device type 'None'")?;
            return fill_hostdev(doc, el, parent, all);
        }
        Some("mdev") => {
            el.set_attribute(doc, "type", "mdev");
            let type_id = dev.type_id.as_deref().unwrap_or_default();
            let (model, managed) = match type_id {
                "vfio_ccw-io" => ("vfio-ccw", "no"),
                "vfio_ap-passthrough" => ("vfio-ap", "no"),
                t if is_vfio_pci_mdev(t) => {
                    el.set_attribute(doc, "display", "off");
                    el.set_attribute(doc, "ramfb", "off");
                    ("vfio-pci", "yes")
                }
                _ => {
                    return Err(format!(
                        "Don't know how to generate nodedev for mdev type id '{type_id}'"
                    ))
                }
            };
            el.set_attribute(doc, "model", model);
            el.set_attribute(doc, "managed", managed);
            set_attr(
                doc,
                el,
                "source/address",
                "uuid",
                Some(&dev.get_mdev_uuid()),
            );
        }
        other => {
            return Err(format!(
                "Unsupported node device type '{}'",
                other.unwrap_or("None")
            ))
        }
    }
    Ok(())
}

/// `DeviceDisk.num_to_target`: 1 -> `a`, 26 -> `z`, 27 -> `aa`.
pub fn num_to_target(num: usize) -> String {
    let mut num = num;
    let mut digits = vec![];
    for factor in 0..3u32 {
        let base = 26usize.pow(factor + 1);
        let mut amt = (num % base) / 26usize.pow(factor);
        if amt == 0 && num >= base {
            amt = 26;
        }
        num -= amt;
        digits.insert(0, amt);
    }
    digits
        .into_iter()
        .filter(|d| *d != 0)
        .map(|d| (b'a' + d as u8 - 1) as char)
        .collect()
}

/// `DeviceDisk.get_target_prefix`: the prefix and how many disks it
/// numbers.
fn target_prefix(bus: &str, device: &str) -> (&'static str, usize) {
    match bus {
        "virtio" => ("vd", 1024),
        "xen" => ("xvd", 1024),
        _ if bus == "fdc" || device == "floppy" => ("fd", 2),
        "ide" => ("hd", 4),
        // sata, scsi, usb, sd
        _ => ("sd", 1024),
    }
}

/// `DeviceDisk.generate_target`: the first free target for the bus,
/// or the first one past the used ones.
pub fn generate_target(bus: &str, device: &str, used: &[String]) -> Result<String, String> {
    let (prefix, maxnode) = target_prefix(bus, device);
    let mut skip: Vec<&str> = used
        .iter()
        .map(String::as_str)
        .filter(|t| t.starts_with(prefix))
        .collect();
    skip.sort();
    let mut first_found = None;
    for i in 0..maxnode {
        let gen_t = format!("{prefix}{}", num_to_target(i + 1));
        if let Some(pos) = skip.iter().position(|t| *t == gen_t) {
            skip.remove(pos);
            continue;
        }
        if skip.is_empty() {
            return Ok(gen_t);
        }
        first_found.get_or_insert(gen_t);
    }
    first_found.ok_or_else(|| {
        if maxnode == 1 {
            format!("Only {maxnode} disk for bus '{bus}' are supported")
        } else {
            format!("Only {maxnode} disks for bus '{bus}' are supported")
        }
    })
}

/// The storage page's disk: `vmmAddStorage.build_device` with the
/// advanced options the user changed, and `set_defaults`.
#[derive(Clone, Debug, Default)]
pub struct NewDisk {
    pub device: String,
    pub bus: String,
    pub target: String,
    pub path: Option<String>,
    /// The storage's type and format; `None` keeps `type='file'`.
    pub props: Option<super::disk::PathProps>,
    /// A volume is made for it with `allocation=0`.
    pub sparse_install: bool,
    pub cache: Option<Option<String>>,
    pub discard: Option<Option<String>>,
    pub readonly: Option<bool>,
    pub shareable: Option<bool>,
    pub removable: Option<bool>,
    pub serial: Option<String>,
}

/// Builds a [`NewDisk`]. `is_qemu`/`is_xen` are the connection's.
pub fn disk(doc: &mut Document, d: &NewDisk, is_qemu: bool, is_xen: bool) -> Element {
    use super::disk as dd;
    let el = new(doc, "disk");
    el.set_attribute(doc, "type", "file");
    el.set_attribute(doc, "device", &d.device);
    let props = d.props.clone().unwrap_or(dd::PathProps {
        dev_type: "file".into(),
        format: None,
    });
    // `conn_disk_driver_name_qemu`
    dd::set_source_path(doc, el, d.path.as_deref(), Some(&props), is_qemu);
    if is_xen && props.dev_type == "block" && d.path.is_some() {
        set_attr(doc, el, "driver", "name", Some("phy"));
    }
    set_attr(doc, el, "target", "dev", Some(&d.target));
    set_attr(doc, el, "target", "bus", Some(&d.bus));

    if let Some(v) = &d.cache {
        dd::set_driver_attr(doc, el, "cache", v.as_deref());
    }
    if let Some(v) = &d.discard {
        dd::set_driver_attr(doc, el, "discard", v.as_deref());
    }
    let is_cdrom = d.device == "cdrom";
    if let Some(v) = d.readonly {
        dd::set_marker(doc, el, "readonly", v);
    }
    if let Some(v) = d.shareable {
        dd::set_marker(doc, el, "shareable", v);
    }
    if let Some(v) = d.removable {
        dd::set_removable(doc, el, v);
    }
    if let Some(v) = &d.serial {
        dd::set_serial(doc, el, Some(v));
    }

    // `set_defaults`
    if is_cdrom {
        dd::set_marker(doc, el, "readonly", true);
    }
    let is_disk = d.device == "disk";
    let mut discard_unmap = is_qemu && is_disk && d.sparse_install;
    if is_qemu && is_disk && props.dev_type == "block" {
        discard_unmap = true;
        if super::edit::attr(doc, el, &["driver"], "cache").is_none() {
            set_attr(doc, el, "driver", "cache", Some("none"));
        }
        if super::edit::attr(doc, el, &["driver"], "io").is_none() {
            set_attr(doc, el, "driver", "io", Some("native"));
        }
    }
    if discard_unmap && super::edit::attr(doc, el, &["driver"], "discard").is_none() {
        set_attr(doc, el, "driver", "discard", Some("unmap"));
    }
    el
}

/// `get_xml()` of a built device: the element on its own.
pub fn to_xml(build: impl FnOnce(&mut Document) -> Element) -> String {
    let mut doc = Document::new();
    let el = build(&mut doc);
    virtinst_xml::element_to_string(doc, el).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The element's tags and attributes, in a stable order: libvirt
    /// doesn't care about attribute order, and the writer doesn't keep
    /// it.
    fn shape(build: impl FnOnce(&mut Document) -> Element) -> Vec<String> {
        let xml = to_xml(build);
        let doc = virtinst_xml::parse_libvirt_xml(&xml).unwrap();
        let mut out = vec![];
        fn walk(doc: &Document, el: Element, prefix: &str, out: &mut Vec<String>) {
            let path = format!("{prefix}{}", el.name(doc));
            let mut attrs: Vec<String> = el
                .attributes(doc)
                .iter()
                .map(|(k, v)| format!("{path}@{k}={v}"))
                .collect();
            attrs.sort();
            out.push(path.clone());
            out.extend(attrs);
            let text = el.text_content(doc);
            if el.child_elements(doc).is_empty() && !text.trim().is_empty() {
                out.push(format!("{path}={}", text.trim()));
            }
            for child in el.child_elements(doc) {
                walk(doc, child, &format!("{path}/"), out);
            }
        }
        walk(&doc, doc.root_element().unwrap(), "", &mut out);
        out
    }

    fn q35() -> GuestInfo {
        GuestInfo {
            os_type: Some("hvm".into()),
            arch: Some("x86_64".into()),
            machine: Some("pc-q35-8.2".into()),
            ..Default::default()
        }
    }

    #[test]
    fn simple_devices() {
        let g = q35();
        assert_eq!(shape(|d| sound(d, None, &g)), ["sound", "sound@model=ich9"]);
        assert_eq!(
            shape(|d| watchdog(d, None, None)),
            [
                "watchdog",
                "watchdog@action=reset",
                "watchdog@model=i6300esb"
            ]
        );
        assert_eq!(
            shape(|d| smartcard(d, Some("host"))),
            ["smartcard", "smartcard@mode=host", "smartcard@type=tcp"]
        );
        assert_eq!(
            shape(|d| rng(d, "/dev/urandom")),
            [
                "rng",
                "rng@model=virtio",
                "rng/backend",
                "rng/backend@model=random",
                "rng/backend=/dev/urandom"
            ]
        );
        assert_eq!(shape(|d| panic(d, None)), ["panic"]);
        assert_eq!(
            shape(|d| vsock(d, false, 42)),
            [
                "vsock",
                "vsock@model=virtio",
                "vsock/cid",
                "vsock/cid@address=42",
                "vsock/cid@auto=no"
            ]
        );
        assert_eq!(
            shape(|d| controller(d, "usb", Some("usb3"), Some(1))),
            [
                "controller",
                "controller@index=1",
                "controller@model=qemu-xhci",
                "controller@ports=15",
                "controller@type=usb"
            ]
        );
    }

    #[test]
    fn disk_targets() {
        assert_eq!(num_to_target(1), "a");
        assert_eq!(num_to_target(26), "z");
        assert_eq!(num_to_target(27), "aa");
        assert_eq!(num_to_target(703), "aaa");
        let used = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            generate_target("virtio", "disk", &used(&["vda", "sda"])).unwrap(),
            "vdb"
        );
        // Past the used ones, like upstream, rather than filling a hole.
        assert_eq!(
            generate_target("virtio", "disk", &used(&["vdb"])).unwrap(),
            "vdc"
        );
        assert_eq!(generate_target("sata", "cdrom", &[]).unwrap(), "sda");
        assert_eq!(
            generate_target("fdc", "floppy", &used(&["fda"])).unwrap(),
            "fdb"
        );
        assert!(generate_target("ide", "disk", &used(&["hda", "hdb", "hdc", "hdd"])).is_err());
    }

    #[test]
    fn disks() {
        let d = NewDisk {
            device: "disk".into(),
            bus: "virtio".into(),
            target: "vdb".into(),
            path: Some("/var/lib/libvirt/images/vm.qcow2".into()),
            props: Some(super::super::disk::PathProps {
                dev_type: "file".into(),
                format: Some("qcow2".into()),
            }),
            sparse_install: true,
            serial: Some("abc".into()),
            ..Default::default()
        };
        assert_eq!(
            shape(|doc| disk(doc, &d, true, false)),
            [
                "disk",
                "disk@device=disk",
                "disk@type=file",
                "disk/source",
                "disk/source@file=/var/lib/libvirt/images/vm.qcow2",
                "disk/driver",
                "disk/driver@discard=unmap",
                "disk/driver@name=qemu",
                "disk/driver@type=qcow2",
                "disk/target",
                "disk/target@bus=virtio",
                "disk/target@dev=vdb",
                "disk/serial",
                "disk/serial=abc",
            ]
        );
        let cd = NewDisk {
            device: "cdrom".into(),
            bus: "sata".into(),
            target: "sda".into(),
            ..Default::default()
        };
        assert_eq!(
            shape(|doc| disk(doc, &cd, true, false)),
            [
                "disk",
                "disk@device=cdrom",
                "disk@type=file",
                "disk/target",
                "disk/target@bus=sata",
                "disk/target@dev=sda",
                "disk/readonly",
            ]
        );
        let blk = NewDisk {
            device: "disk".into(),
            bus: "scsi".into(),
            target: "sda".into(),
            path: Some("/dev/sdb".into()),
            props: Some(super::super::disk::PathProps {
                dev_type: "block".into(),
                format: None,
            }),
            ..Default::default()
        };
        let s = shape(|doc| disk(doc, &blk, true, false));
        for want in [
            "disk@type=block",
            "disk/source@dev=/dev/sdb",
            "disk/driver@type=raw",
            "disk/driver@cache=none",
            "disk/driver@io=native",
            "disk/driver@discard=unmap",
        ] {
            assert!(s.contains(&want.to_string()), "{want} in {s:?}");
        }
    }

    #[test]
    fn interfaces() {
        let mut n = NewInterface {
            nettype: "network".into(),
            source: Some("default".into()),
            portgroup: Some("eng".into()),
            mac: "52:54:00:aa:bb:cc".into(),
            ..Default::default()
        };
        assert_eq!(
            shape(|d| interface(d, &n, Some("virtio"))),
            [
                "interface",
                "interface@type=network",
                "interface/source",
                "interface/source@network=default",
                "interface/source@portgroup=eng",
                "interface/mac",
                "interface/mac@address=52:54:00:aa:bb:cc",
                "interface/model",
                "interface/model@type=virtio"
            ]
        );
        n.nettype = "direct".into();
        n.source = Some("eth0".into());
        n.source_mode = Some("bridge".into());
        n.portgroup = None;
        n.model = Some("e1000e".into());
        assert_eq!(
            shape(|d| interface(d, &n, None)),
            [
                "interface",
                "interface@type=direct",
                "interface/source",
                "interface/source@dev=eth0",
                "interface/source@mode=bridge",
                "interface/mac",
                "interface/mac@address=52:54:00:aa:bb:cc",
                "interface/model",
                "interface/model@type=e1000e"
            ]
        );
        let mut g = q35();
        g.domain_type = Some("kvm".into());
        assert_eq!(g.default_nic_model(true), Some("e1000e"));
        g.os_virtionet = true;
        assert_eq!(g.default_nic_model(true), Some("virtio"));
    }

    #[test]
    fn hostdevs() {
        let parse = |xml: &str| NodeDevice::parse(xml).unwrap();
        let pci = parse(
            "<device><name>pci_0000_00_02_0</name><capability type='pci'>\
             <domain>0</domain><bus>0</bus><slot>2</slot><function>0</function>\
             </capability></device>",
        );
        let net = parse(
            "<device><name>net_eth0</name><parent>pci_0000_00_02_0</parent>\
             <capability type='net'><interface>eth0</interface></capability></device>",
        );
        let usb = |dev: &str| {
            parse(&format!(
                "<device><name>usb_1_{dev}</name><capability type='usb_device'>\
                 <bus>1</bus><device>{dev}</device><product id='0xc52b'/>\
                 <vendor id='0x046d'/></capability></device>"
            ))
        };
        let all = vec![pci.clone(), net.clone(), usb("2")];
        let pci_shape = [
            "hostdev",
            "hostdev@managed=yes",
            "hostdev@mode=subsystem",
            "hostdev@type=pci",
            "hostdev/source",
            "hostdev/source/address",
            "hostdev/source/address@bus=0",
            "hostdev/source/address@domain=0",
            "hostdev/source/address@function=0",
            "hostdev/source/address@slot=2",
        ];
        assert_eq!(
            shape(|d| hostdev(d, &pci, &all, false, Some("optional")).unwrap()),
            pci_shape
        );
        assert_eq!(
            shape(|d| hostdev(d, &net, &all, false, None).unwrap()),
            pci_shape
        );
        assert_eq!(
            shape(|d| hostdev(d, &usb("2"), &all, true, Some("optional")).unwrap()),
            [
                "hostdev",
                "hostdev@managed=no",
                "hostdev@mode=subsystem",
                "hostdev@type=usb",
                "hostdev/source",
                "hostdev/source@startupPolicy=optional",
                "hostdev/source/vendor",
                "hostdev/source/vendor@id=0x046d",
                "hostdev/source/product",
                "hostdev/source/product@id=0xc52b",
            ]
        );
        // Two of the same model: the bus address tells them apart.
        let all = vec![usb("2"), usb("3")];
        let s = shape(|d| hostdev(d, &usb("3"), &all, false, None).unwrap());
        assert!(
            s.contains(&"hostdev/source/address@device=3".to_string()),
            "{s:?}"
        );

        let mdev = parse(
            "<device><name>mdev_4b20d080_1b54_4048_85b3_a6a62d165c01</name>\
             <capability type='mdev'><type id='nvidia-63'/></capability></device>",
        );
        assert_eq!(
            shape(|d| hostdev(d, &mdev, &[], false, None).unwrap()),
            [
                "hostdev",
                "hostdev@display=off",
                "hostdev@managed=yes",
                "hostdev@mode=subsystem",
                "hostdev@model=vfio-pci",
                "hostdev@ramfb=off",
                "hostdev@type=mdev",
                "hostdev/source",
                "hostdev/source/address",
                "hostdev/source/address@uuid=4b20d080-1b54-4048-85b3-a6a62d165c01",
            ]
        );
        assert!(is_vfio_pci_mdev("i915-GVTg_V5_4"));
        assert!(!is_vfio_pci_mdev("nvidia-6"));
        let mut d = Document::new();
        assert!(hostdev(
            &mut d,
            &parse("<device><name>x</name><capability type='scsi'/></device>"),
            &[],
            false,
            None
        )
        .is_err());
    }

    #[test]
    fn char_defaults() {
        let unix = NewChar {
            tag: "channel".into(),
            char_type: "unix".into(),
            target_name: Some(CHANNEL_NAME_QEMUGA.into()),
            ..Default::default()
        };
        assert_eq!(
            shape(|d| char_device(d, &unix)),
            [
                "channel",
                "channel@type=unix",
                "channel/source",
                "channel/source@mode=bind",
                "channel/target",
                "channel/target@name=org.qemu.guest_agent.0",
                "channel/target@type=virtio"
            ]
        );
        let vdagent = NewChar {
            tag: "channel".into(),
            char_type: "qemu-vdagent".into(),
            clipboard: Some(true),
            ..Default::default()
        };
        assert_eq!(
            shape(|d| char_device(d, &vdagent)),
            [
                "channel",
                "channel@type=qemu-vdagent",
                "channel/source",
                "channel/source/clipboard",
                "channel/source/clipboard@copypaste=yes",
                "channel/target",
                "channel/target@name=com.redhat.spice.0",
                "channel/target@type=virtio"
            ]
        );
        let pty = NewChar {
            tag: "serial".into(),
            char_type: "pty".into(),
            ..Default::default()
        };
        assert_eq!(
            shape(|d| char_device(d, &pty)),
            ["serial", "serial@type=pty"]
        );
    }

    #[test]
    fn video_defaults() {
        let mut g = q35();
        assert_eq!(g.default_video_model(None, true), Some("vga"));
        g.has_spice = true;
        g.has_gl = true;
        assert_eq!(g.default_video_model(None, true), Some("virtio"));
        assert_eq!(
            shape(|d| video(d, Some("virtio"), None, &g)),
            [
                "video",
                "video/model",
                "video/model@type=virtio",
                "video/model/acceleration",
                "video/model/acceleration@accel3d=yes"
            ]
        );
        assert_eq!(default_tpm_model(None, &g), Some("tpm-crb"));
    }
}
