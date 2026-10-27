//! `<graphics>` — upstream's `devices/graphics.py` (`DeviceGraphics`).
//! Grounded in the real `XMLProperty` declarations before writing
//! anything: unlike disk/network, most fields here are flat attributes
//! on `<graphics>` itself (`type`, `port`, `autoport`, `listen`,
//! `passwd` are all `./@...`) — the SPICE-specific channel config is
//! the nested exception (`image_compression = XMLProperty("./image/@compression")`),
//! included as one representative case rather than modeling every SPICE
//! sub-channel now. Console-rendering itself is gir-bound per tickets
//! 05/14 — this struct is XML binding only, same as every other device
//! struct so far.

use virtinst_xml::XmlBound;

/// ```xml
/// <graphics type="vnc" port="-1" autoport="yes" listen="127.0.0.1"/>
/// ```
/// or
/// ```xml
/// <graphics type="spice" autoport="yes">
///   <image compression="auto_glz"/>
/// </graphics>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "graphics")]
pub struct DeviceGraphics {
    #[xml(attribute = "type")]
    pub graphics_type: Option<String>,

    #[xml(attribute = "port")]
    pub port: Option<String>,

    #[xml(attribute = "tlsPort")]
    pub tls_port: Option<String>,

    #[xml(attribute = "autoport")]
    pub autoport: Option<bool>,

    #[xml(attribute = "listen")]
    pub listen: Option<String>,

    #[xml(attribute = "passwd")]
    pub passwd: Option<String>,

    /// SPICE-only. The other SPICE sub-channels (`streaming`,
    /// `clipboard`, `mouse`, `filetransfer`, `gl`, `zlib`) follow the
    /// exact same `path = "...", attribute = "..."` shape — added as
    /// each is actually needed rather than modeled speculatively now.
    #[xml(path = "image", attribute = "compression")]
    pub image_compression: Option<String>,
}

// Editing an existing `<graphics>` in place, for the details page. The
// setters follow `DeviceGraphics`' Python properties, side effects
// included; `define_graphics` applies them in the order listen, port,
// passwd, type, gl, rendernode.

use virtinst_xml::{Document, Element};

use super::edit::set_attr;

/// `_validate_port`.
fn validate_port(name: &str, val: Option<i64>) -> Result<Option<i64>, String> {
    match val {
        Some(v) if v < 5900 && v != -1 => Err(format!(
            "{name} must be above 5900, or -1 for auto allocation"
        )),
        _ => Ok(val),
    }
}

fn int_attr(doc: &Document, gfx: Element, name: &str) -> Option<i64> {
    gfx.attribute(doc, name).and_then(|v| v.parse().ok())
}

/// `_get_default_autoport`, read before the new port is written, as
/// the Python setter does.
fn default_autoport(doc: &Document, gfx: Element) -> Option<&'static str> {
    if gfx.attribute(doc, "type") != Some("spice") {
        return None;
    }
    (int_attr(doc, gfx, "port") == Some(-1) && int_attr(doc, gfx, "tlsPort") == Some(-1))
        .then_some("yes")
}

fn set_port_attr(doc: &mut Document, gfx: Element, name: &str, val: Option<i64>) {
    let autoport = default_autoport(doc, gfx);
    set_attr(doc, gfx, "", "autoport", autoport);
    set_attr(doc, gfx, "", name, val.map(|v| v.to_string()).as_deref());
}

/// `port`: checks the value, then resets `autoport`.
pub fn set_port(doc: &mut Document, gfx: Element, val: Option<i64>) -> Result<(), String> {
    let val = validate_port("Port", val)?;
    set_port_attr(doc, gfx, "port", val);
    Ok(())
}

fn remove_all_listens(doc: &mut Document, gfx: Element) {
    for listen in gfx.find_all(doc, "listen") {
        let _ = listen.detach(doc);
    }
}

fn add_listen(doc: &mut Document, gfx: Element, ltype: &str) {
    Element::build("listen")
        .attribute("type", ltype)
        .push_to(doc, gfx);
}

/// `listen`: `none` and `socket` become `<listen type=...>`, anything
/// else is the address (`None` for the hypervisor default).
pub fn set_listen(doc: &mut Document, gfx: Element, val: Option<&str>) {
    match val {
        Some("none") => {
            // `_set_listen_none`; every libvirt we support has
            // `conn_graphics_listen_none`.
            remove_all_listens(doc, gfx);
            set_attr(doc, gfx, "", "listen", None);
            set_port_attr(doc, gfx, "port", None);
            set_port_attr(doc, gfx, "tlsPort", None);
            set_attr(doc, gfx, "", "autoport", None);
            set_attr(doc, gfx, "", "socket", None);
            add_listen(doc, gfx, "none");
        }
        Some("socket") => {
            remove_all_listens(doc, gfx);
            add_listen(doc, gfx, "socket");
        }
        addr => {
            remove_all_listens(doc, gfx);
            set_attr(doc, gfx, "", "listen", addr);
        }
    }
}

pub fn set_passwd(doc: &mut Document, gfx: Element, val: Option<&str>) {
    set_attr(doc, gfx, "", "passwd", val);
}

/// `type`: VNC only takes `connected='keep'`, so other values go.
pub fn set_type(doc: &mut Document, gfx: Element, val: &str) {
    gfx.set_attribute(doc, "type", val);
    if val == "vnc" && gfx.attribute(doc, "connected") != Some("keep") {
        set_attr(doc, gfx, "", "connected", None);
    }
}

/// `gl`: `./gl/@enable`, yes/no.
pub fn set_gl(doc: &mut Document, gfx: Element, val: Option<bool>) {
    let val = val.map(|v| if v { "yes" } else { "no" });
    set_attr(doc, gfx, "gl", "enable", val);
}

pub fn set_rendernode(doc: &mut Document, gfx: Element, val: Option<&str>) {
    set_attr(doc, gfx, "gl", "rendernode", val);
}

/// `get_first_listen_type`.
pub fn first_listen_type(doc: &Document, gfx: Element) -> Option<String> {
    gfx.find(doc, "listen")
        .and_then(|l| l.attribute(doc, "type"))
        .map(str::to_string)
}

// The `XMLBuilder` port of `DeviceGraphics`, for the CLI parsers. The
// properties with Python setters (`type`, `port`, `tlsPort`, `listen`)
// are set through the functions below, side effects included.

use crate::xmlbuilder::{child, prop, Kind, Value, XmlClass, XmlObject};

/// `_GraphicsListen`.
pub static GRAPHICS_LISTEN: XmlClass = XmlClass {
    xml_name: "listen",
    props: &[
        prop("type", "./@type", Kind::Str),
        prop("address", "./@address", Kind::Str),
        prop("network", "./@network", Kind::Str),
        prop("socket", "./@socket", Kind::Str),
    ],
    children: &[],
    prop_order: &[],
};

/// `DeviceGraphics`.
pub static DEVICE_GRAPHICS: XmlClass = XmlClass {
    xml_name: "graphics",
    props: &[
        prop("keymap", "./@keymap", Kind::Str),
        prop("_type", "./@type", Kind::Str),
        prop("_port", "./@port", Kind::Int),
        prop("_tlsPort", "./@tlsPort", Kind::Int),
        prop("autoport", "./@autoport", Kind::YesNo),
        prop("websocket", "./@websocket", Kind::Int),
        prop("xauth", "./@xauth", Kind::Str),
        prop("display", "./@display", Kind::Str),
        prop("_listen", "./@listen", Kind::Str),
        prop("passwd", "./@passwd", Kind::Str),
        prop("passwdValidTo", "./@passwdValidTo", Kind::Str),
        prop("socket", "./@socket", Kind::Str),
        prop("connected", "./@connected", Kind::Str),
        prop("defaultMode", "./@defaultMode", Kind::Str),
        prop("image_compression", "./image/@compression", Kind::Str),
        prop("streaming_mode", "./streaming/@mode", Kind::Str),
        prop("clipboard_copypaste", "./clipboard/@copypaste", Kind::YesNo),
        prop("mouse_mode", "./mouse/@mode", Kind::Str),
        prop("filetransfer_enable", "./filetransfer/@enable", Kind::YesNo),
        prop("gl", "./gl/@enable", Kind::YesNo),
        prop("rendernode", "./gl/@rendernode", Kind::Str),
        prop("zlib_compression", "./zlib/@compression", Kind::Str),
    ],
    children: crate::device_children!(child("listens", &GRAPHICS_LISTEN, ".", false)),
    prop_order: crate::device_prop_order!(
        "_type",
        "gl",
        "_port",
        "_tlsPort",
        "autoport",
        "websocket",
        "keymap",
        "_listen",
        "passwd",
        "display",
        "xauth",
    ),
};

fn get_int(dev: &XmlObject, name: &str) -> Option<i64> {
    match dev.get(name) {
        Some(Value::Int(n)) => Some(n),
        _ => None,
    }
}

/// `_validate_port`.
fn validate_port_value(name: &str, val: Option<Value>) -> Result<Option<Value>, String> {
    let Some(val) = val else {
        return Ok(None);
    };
    let n = match val {
        Value::Int(n) => n,
        Value::Bool(b) => i64::from(b),
        Value::Str(s) => s
            .trim()
            .parse::<i64>()
            .map_err(|_| format!("invalid literal for int() with base 10: '{s}'"))?,
    };
    if n < 5900 && n != -1 {
        return Err(format!(
            "{name} must be above 5900, or -1 for auto allocation"
        ));
    }
    Ok(Some(Value::Int(n)))
}

/// `_get_default_autoport`.
fn default_autoport_value(dev: &XmlObject) -> Option<Value> {
    // By default, don't do this for VNC to maintain back compat with
    // old libvirt that didn't support 'autoport'
    if dev.get_str("_type").as_deref() != Some("spice") {
        return None;
    }
    if get_int(dev, "_port") == Some(-1) && get_int(dev, "_tlsPort") == Some(-1) {
        return Some(Value::Bool(true));
    }
    None
}

/// `DeviceGraphics.type`'s setter.
pub fn set_type_prop(dev: &mut XmlObject, val: Option<Value>) -> Result<(), String> {
    let is_vnc = val.as_ref().and_then(|v| v.as_str()) == Some("vnc");
    dev.set("_type", val)?;
    // connected is valid for Spice and VNC, but VNC accepts only 'keep' and
    // libvirt errors out if there is any other value
    if is_vnc && dev.get_str("connected").as_deref() != Some("keep") {
        dev.set("connected", None)?;
    }
    Ok(())
}

/// `DeviceGraphics.port`'s setter.
pub fn set_port_prop(dev: &mut XmlObject, val: Option<Value>) -> Result<(), String> {
    let val = validate_port_value("Port", val)?;
    dev.set("autoport", default_autoport_value(dev))?;
    dev.set("_port", val)
}

/// `DeviceGraphics.tlsPort`'s setter.
pub fn set_tlsport_prop(dev: &mut XmlObject, val: Option<Value>) -> Result<(), String> {
    let val = validate_port_value("TLS Port", val)?;
    dev.set("autoport", default_autoport_value(dev))?;
    dev.set("_tlsPort", val)
}

fn obj_remove_all_listens(dev: &mut XmlObject) {
    while !dev.children("listens").is_empty() {
        dev.remove_child("listens", 0);
    }
}

fn obj_add_listen(dev: &mut XmlObject, ltype: &str) -> Result<(), String> {
    dev.add_child(XmlObject::new(&GRAPHICS_LISTEN));
    let listens = dev.children_mut("listens");
    let last = listens.len() - 1;
    listens[last].set("type", Some(ltype.into()))
}

/// `DeviceGraphics.listen`'s setter. `listen_none` is
/// `conn.support.conn_graphics_listen_none()`.
pub fn set_listen_prop(
    dev: &mut XmlObject,
    val: Option<Value>,
    listen_none: bool,
) -> Result<(), String> {
    match val.as_ref().and_then(|v| v.as_str()) {
        Some("none") => {
            // `_set_listen_none`
            obj_remove_all_listens(dev);
            set_listen_prop(dev, None, listen_none)?;
            set_port_prop(dev, None)?;
            set_tlsport_prop(dev, None)?;
            dev.set("autoport", None)?;
            dev.set("socket", None)?;
            if listen_none {
                obj_add_listen(dev, "none")?;
            }
            Ok(())
        }
        Some("socket") => {
            obj_remove_all_listens(dev);
            obj_add_listen(dev, "socket")
        }
        _ => {
            obj_remove_all_listens(dev);
            dev.set("_listen", val)
        }
    }
}

/// `get_first_listen_type`.
fn first_listen(dev: &XmlObject) -> Option<String> {
    dev.children("listens")
        .first()
        .and_then(|l| l.get_str("type"))
}

/// What `DeviceGraphics.set_defaults` asks of the guest and connection.
pub struct GraphicsDefaults<'a> {
    /// `_default_type(guest)`.
    pub default_type: &'a dyn Fn() -> Option<String>,
    /// `conn.is_remote()`.
    pub is_remote: bool,
    /// The first DRM render node's devnode path, for spice GL.
    pub rendernode: &'a dyn Fn() -> Option<String>,
    /// `~/.Xauthority` and `$DISPLAY`, for sdl.
    pub xauth: Option<String>,
    pub display: Option<String>,
}

/// `DeviceGraphics.set_defaults`.
pub fn set_defaults(dev: &mut XmlObject, d: &GraphicsDefaults) -> Result<(), String> {
    let empty = |v: Option<String>| v.map_or(true, |s| s.is_empty());
    if empty(dev.get_str("_type")) {
        set_type_prop(dev, (d.default_type)().map(Into::into))?;
    }
    let typ = dev.get_str("_type");

    if typ.as_deref() == Some("sdl") {
        if empty(dev.get_str("xauth")) {
            dev.set("xauth", d.xauth.clone().map(Into::into))?;
        }
        if empty(dev.get_str("display")) {
            dev.set("display", d.display.clone().map(Into::into))?;
        }
    }

    let need_port = matches!(
        first_listen(dev).as_deref(),
        None | Some("address" | "network")
    );
    if dev.get("_port").is_none() {
        // `_get_default_port`
        let port = (matches!(typ.as_deref(), Some("vnc" | "spice")) && need_port)
            .then_some(Value::Int(-1));
        set_port_prop(dev, port)?;
    }
    if dev.get("_tlsPort").is_none() {
        // `_get_default_tlsport`
        let port = (typ.as_deref() == Some("spice") && need_port).then_some(Value::Int(-1));
        set_tlsport_prop(dev, port)?;
    }
    if dev.get("autoport").is_none() {
        dev.set("autoport", default_autoport_value(dev))?;
    }

    if empty(dev.get_str("image_compression")) && typ.as_deref() == Some("spice") {
        // `_default_image_compression`
        if !d.is_remote {
            log::debug!("Local connection, disabling spice image compression.");
            dev.set("image_compression", Some("off".into()))?;
        }
    }
    if typ.as_deref() == Some("spice") && dev.get("gl") == Some(Value::Bool(true)) {
        // `_default_spice_gl`: if spice GL but rendernode wasn't
        // specified, hardcode the first one
        if empty(dev.get_str("rendernode")) {
            if let Some(path) = (d.rendernode)() {
                dev.set("rendernode", Some(path.into()))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod edit_tests {
    use super::*;

    fn parse(xml: &str) -> (Document, Element) {
        let doc = virtinst_xml::parse_libvirt_xml(xml).unwrap();
        let root = doc.root_element().unwrap();
        (doc, root)
    }

    #[test]
    fn listen_none_and_back() {
        let (mut doc, gfx) = parse(
            "<graphics type='spice' port='-1' tlsPort='-1' autoport='yes' listen='127.0.0.1'>\
             <listen type='address' address='127.0.0.1'/></graphics>",
        );
        set_listen(&mut doc, gfx, Some("none"));
        let out = doc.write_str().unwrap();
        for gone in ["port=", "autoport=", "listen=", "address="] {
            assert!(!out.contains(gone), "{out}");
        }
        assert!(out.contains("<listen type=\"none\"/>"), "{out}");
        assert_eq!(first_listen_type(&doc, gfx).as_deref(), Some("none"));

        set_listen(&mut doc, gfx, Some("0.0.0.0"));
        set_port(&mut doc, gfx, Some(-1)).unwrap();
        assert_eq!(gfx.attribute(&doc, "listen"), Some("0.0.0.0"));
        assert_eq!(gfx.attribute(&doc, "port"), Some("-1"));
        assert_eq!(first_listen_type(&doc, gfx), None);
    }

    #[test]
    fn port_checks_and_type() {
        let (mut doc, gfx) = parse("<graphics type='spice' connected='disconnect' port='5901'/>");
        assert!(set_port(&mut doc, gfx, Some(80)).is_err());
        set_port(&mut doc, gfx, Some(5905)).unwrap();
        assert_eq!(gfx.attribute(&doc, "port"), Some("5905"));
        set_type(&mut doc, gfx, "vnc");
        assert_eq!(gfx.attribute(&doc, "connected"), None);
        set_passwd(&mut doc, gfx, Some("s3cret"));
        set_gl(&mut doc, gfx, Some(true));
        set_rendernode(&mut doc, gfx, Some("/dev/dri/renderD128"));
        let out = doc.write_str().unwrap();
        assert!(out.contains("passwd=\"s3cret\""), "{out}");
        assert!(out.contains("enable=\"yes\""), "{out}");
        set_gl(&mut doc, gfx, None);
        set_rendernode(&mut doc, gfx, None);
        let out = doc.write_str().unwrap();
        assert!(!out.contains("<gl"), "{out}");
    }
}
