//! `--graphics`: `ParserGraphics`, and the guest's spice device
//! helpers it triggers (`change_graphics`, `_remove_spice_devices`).

use std::cell::RefCell;
use std::sync::OnceLock;

use virtinst_core::capabilities::domcaps::DomainCapabilities;
use virtinst_core::devices::build::GuestInfo;
use virtinst_core::devices::{char, graphics, redirdev, sound};
use virtinst_core::xmlbuilder::{Value, XmlObject};

use crate::parser::{
    add_common_device_args, conn_support, ArgDef, ArgInst, Error, ParseCtx, ParserDef,
};

thread_local! {
    static DOMCAPS: RefCell<Option<DomainCapabilities>> = const { RefCell::new(None) };
}

/// The guest's `lookup_domcaps()`, for the spice devices an edit adds.
pub fn set_domcaps(domcaps: Option<DomainCapabilities>) {
    DOMCAPS.with(|d| *d.borrow_mut() = domcaps);
}

/// `ParserGraphics._parse`.
fn graphics_parse(ctx: &mut ParseCtx) -> Result<Option<Vec<XmlObject>>, Error> {
    if ctx.optstr == "none" {
        crate::parser::update_guest_flags(|f| f.skip_default_graphics = true);
        return Ok(Some(vec![]));
    }
    Ok(None)
}

/// `ParserGraphics.set_keymap_cb`.
fn set_keymap_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    let s = val.as_ref().map(|v| v.to_xml_string()).unwrap_or_default();
    let val = if s.is_empty() {
        None
    } else if s.to_lowercase() == "local" {
        log::debug!("keymap=local is no longer implemented. Using None.");
        None
    } else if s.to_lowercase() == "none" {
        None
    } else {
        val
    };
    inst.set("keymap", val)
}

/// `ParserGraphics.set_type_cb`.
fn set_type_cb(
    ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    if ctx.editing {
        // `self.guest.change_graphics(val, inst)`: the type now, the
        // guest's spice devices once the object is back in the guest.
        graphics::set_type_prop(inst, val)?;
        ctx.post_edit = Some(change_graphics);
        return Ok(());
    }
    if val.as_ref().and_then(|v| v.as_str()) == Some("default") {
        return Ok(());
    }
    graphics::set_type_prop(inst, val)
}

fn set_port_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    graphics::set_port_prop(inst, val)
}

fn set_tlsport_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    graphics::set_tlsport_prop(inst, val)
}

fn set_listen_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    graphics::set_listen_prop(inst, val, conn_support().graphics_listen_none)
}

/// `ParserGraphics`.
pub fn graphics() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new(
            "graphics",
            Some("devices.graphics"),
            Some(&graphics::DEVICE_GRAPHICS),
        );
        def.remove_first = &["type"];
        def.stub_none = false;
        def.custom_parse = Some(graphics_parse);
        add_common_device_args(&mut def, false, false);

        def.arg(ArgDef::new("type", "_type").cb(set_type_cb));
        def.arg(ArgDef::new("port", "_port").cb(set_port_cb));
        def.arg(ArgDef::new("tlsPort", "_tlsPort").cb(set_tlsport_cb));
        def.arg(ArgDef::new("websocket", "websocket"));
        def.arg(ArgDef::new("listen", "_listen").cb(set_listen_cb));
        def.arg(ArgDef::new("keymap", "keymap").cb(set_keymap_cb));
        def.arg(ArgDef::new("password", "passwd"));
        def.arg(ArgDef::new("passwordValidTo", "passwdValidTo"));
        def.arg(ArgDef::new("connected", "connected"));
        def.arg(ArgDef::new("defaultMode", "defaultMode"));

        let l = ("listens", "listens");
        def.arg(ArgDef::new("listens[0-9]*.type", "type").find_inst(l.0, l.1));
        def.arg(ArgDef::new("listens[0-9]*.address", "address").find_inst(l.0, l.1));
        def.arg(ArgDef::new("listens[0-9]*.network", "network").find_inst(l.0, l.1));
        def.arg(ArgDef::new("listens[0-9]*.socket", "socket").find_inst(l.0, l.1));

        def.arg(ArgDef::new("image.compression", "image_compression"));
        def.arg(ArgDef::new("streaming.mode", "streaming_mode"));
        def.arg(ArgDef::new("clipboard.copypaste", "clipboard_copypaste").onoff());
        def.arg(ArgDef::new("mouse.mode", "mouse_mode"));
        def.arg(ArgDef::new("filetransfer.enable", "filetransfer_enable").onoff());
        def.arg(ArgDef::new("zlib.compression", "zlib_compression"));

        def.arg(ArgDef::new("gl.enable", "gl").onoff());
        def.arg(ArgDef::new("gl.rendernode", "rendernode"));

        for (cli, alias) in [
            ("tlsPort", "tlsport"),
            ("password", "passwd"),
            ("passwordValidTo", "passwdValidTo"),
            ("image.compression", "image_compression"),
            ("streaming.mode", "streaming_mode"),
            ("clipboard.copypaste", "clipboard_copypaste"),
            ("filetransfer.enable", "filetransfer_enable"),
            ("mouse.mode", "mouse_mode"),
            ("gl.enable", "gl"),
            ("gl.rendernode", "rendernode"),
        ] {
            def.alias(cli, &[alias]);
        }
        def
    })
}

/// `Guest.has_spice`.
pub(crate) fn has_spice(devices: &XmlObject) -> bool {
    devices
        .children("graphics")
        .iter()
        .any(|g| g.get_str("_type").as_deref() == Some("spice"))
}

/// `Guest._usb_disabled`.
fn usb_disabled(devices: &XmlObject) -> bool {
    let usb: Vec<_> = devices
        .children("controller")
        .iter()
        .filter(|c| c.get_str("type").as_deref() == Some("usb"))
        .collect();
    !usb.is_empty()
        && usb
            .iter()
            .all(|c| c.get_str("model").as_deref() == Some("none"))
}

/// `Guest._add_spice_devices`.
fn add_spice_devices(guest: &mut XmlObject, info: &GuestInfo) -> Result<(), Error> {
    if !has_spice(guest.child("devices")) {
        return Ok(());
    }
    let is_x86 = matches!(info.arch.as_deref(), Some("x86_64" | "i686"));
    if guest.child("features").get("vmport").is_none() && is_x86 && conn_support().vmport {
        guest
            .child_mut("features")
            .set("vmport", Some(false.into()))?;
    }
    let (spicevmc, redirdev_usb) = DOMCAPS.with(|d| match &*d.borrow() {
        Some(dc) => (dc.supports_channel_spicevmc(), dc.supports_redirdev_usb()),
        None => (true, true),
    });
    let devices = guest.child_mut("devices");

    // `_add_spice_channels`
    let has_spicevmc = devices
        .children("channel")
        .iter()
        .any(|c| c.get_str("type").as_deref() == Some("spicevmc"));
    if spicevmc && !has_spicevmc {
        // We explicitly don't check for virtioserial support here.
        // We did that for a while, which excluded windows, and
        // we received some complaints.
        // https://bugzilla.redhat.com/show_bug.cgi?id=1660123
        let mut dev = XmlObject::new(&char::DEVICE_CHANNEL);
        dev.set("type", Some("spicevmc".into()))?;
        char::set_defaults(&mut dev)?;
        devices.add_child(dev);
    }

    // `_add_spice_sound`
    if devices.children("sound").is_empty() && info.os_type.as_deref() == Some("hvm") {
        let mut dev = XmlObject::new(&sound::DEVICE_SOUND);
        sound::set_defaults(&mut dev, info.defaults_to_pcie())?;
        devices.add_child(dev);
    }

    // `_add_spice_usbredir`
    if redirdev_usb && devices.children("redirdev").is_empty() && !usb_disabled(devices) && is_x86 {
        // If we use 4 devices here, we fill up all the emulated USB2 slots,
        // and directly assigned devices are forced to fall back to USB1
        // https://bugzilla.redhat.com/show_bug.cgi?id=1135488
        for _ in 0..2 {
            let mut dev = XmlObject::new(&redirdev::DEVICE_REDIRDEV);
            dev.set("bus", Some("usb".into()))?;
            dev.set("type", Some("spicevmc".into()))?;
            redirdev::set_defaults(&mut dev)?;
            devices.add_child(dev);
        }
    }
    Ok(())
}

/// Removes every `name` device matching `pred`.
fn remove_matching(devices: &mut XmlObject, name: &str, pred: impl Fn(&XmlObject) -> bool) {
    let mut i = 0;
    while i < devices.children(name).len() {
        if pred(&devices.children(name)[i]) {
            devices.remove_child(name, i);
        } else {
            i += 1;
        }
    }
}

/// `Guest._force_remove_spice_devices`.
pub(crate) fn force_remove_spice_devices(devices: &mut XmlObject) {
    let is_type = |t: &'static str| move |d: &XmlObject| d.get_str("type").as_deref() == Some(t);
    remove_matching(devices, "audio", is_type("spice"));
    remove_matching(devices, "channel", is_type("spicevmc"));
    remove_matching(devices, "redirdev", is_type("spicevmc"));
    remove_matching(devices, "serial", is_type("spiceport"));
    remove_matching(devices, "console", is_type("spiceport"));
}

/// `Guest._remove_spice_devices(rmdev)`.
pub fn remove_spice_devices(devices: &mut XmlObject, rmdev: &XmlObject) {
    if rmdev.class.xml_name != "graphics" || has_spice(devices) {
        return;
    }
    force_remove_spice_devices(devices);
}

/// `_remove_spice_devices` for the guest's own `idx`th graphics.
fn remove_spice_devices_for(devices: &mut XmlObject) {
    if has_spice(devices) {
        return;
    }
    force_remove_spice_devices(devices);
}

/// `Guest.change_graphics`, past setting the type.
fn change_graphics(guest: &mut XmlObject, idx: usize) -> Result<(), Error> {
    let is_spice = guest.child("devices").children("graphics")[idx]
        .get_str("_type")
        .as_deref()
        == Some("spice");
    if is_spice {
        let info = super::guest_info(guest);
        add_spice_devices(guest, &info)
    } else {
        remove_spice_devices_for(guest.child_mut("devices"));
        Ok(())
    }
}

/// The first DRM render node's devnode, `_default_spice_gl`'s pick.
pub fn default_rendernode() -> Option<String> {
    crate::parser::nodedevs()
        .into_iter()
        .filter(|n| {
            n.device_type.as_deref() == Some("drm") && n.drm_type.as_deref() == Some("render")
        })
        .find_map(|n| {
            // `get_devnode()`
            n.devnodes
                .iter()
                .find(|p| {
                    let parts: Vec<&str> = p.split('/').collect();
                    parts.len() > 2 && parts[parts.len() - 2] == "by-path"
                })
                .or(n.devnodes.first())
                .cloned()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{find_objects_to_edit, parse_edit, parse_new};
    use virtinst_core::guest::GUEST;

    #[test]
    fn ports_are_validated() {
        let mut guest = XmlObject::new(&GUEST);
        let err = parse_new(graphics(), &mut guest, "vnc,port=-50").unwrap_err();
        assert!(err.contains("Port must be above 5900"), "{err}");
        let err = parse_new(graphics(), &mut guest, "spice,tlsport=5").unwrap_err();
        assert!(err.contains("TLS Port must be above 5900"), "{err}");
    }

    #[test]
    fn spice_to_vnc_drops_spice_devices() {
        let xml = "<domain><name>x</name><devices>\
                   <channel type='spicevmc'><target type='virtio'/></channel>\
                   <graphics type='spice' autoport='yes'/>\
                   <redirdev bus='usb' type='spicevmc'/></devices></domain>";
        let mut guest = XmlObject::parse(&GUEST, xml).unwrap();
        let idx = find_objects_to_edit(graphics(), &mut guest, "edit", None).unwrap();
        parse_edit(graphics(), &mut guest, idx[0], "type=vnc").unwrap();
        let out = guest.get_document_xml();
        assert!(out.contains(r#"<graphics type="vnc""#), "{out}");
        assert!(!out.contains("spicevmc"), "{out}");
    }
}
