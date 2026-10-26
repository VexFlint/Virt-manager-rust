//! New devices for Add Hardware: each builder makes a detached element
//! from what the wizard page picked, with the device's `set_defaults`
//! applied, ready to append to `<devices>` or serialize for
//! `attachDevice`. [`GuestInfo`] carries the guest facts those defaults
//! and the pages' default selections depend on.

use virtinst_xml::{Document, Element};

use super::edit::set_attr;
use crate::capabilities::domcaps::DomainCapabilities;

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
