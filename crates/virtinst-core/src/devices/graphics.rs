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
