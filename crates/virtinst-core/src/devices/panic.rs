//! `<panic>` — upstream's `devices/panic.py`. Genuinely a one-field
//! device once traced: `model = XMLProperty("./@model")` is the only
//! real data.
//!
//! `set_stub = XMLProperty(".", is_bool=True)` is **not modeled, on
//! purpose** — it isn't a binding gap the way the `..`/`[@mode=...]`
//! XPath cases were. Its own XPath is `.` (the element itself), and
//! upstream only ever *sets* it (`self.set_stub = True` in
//! `set_defaults`, when no `model` was picked) to stop its dynamic
//! engine from pruning an otherwise-attribute-less `<panic/>` out of the
//! serialized document entirely. This crate's derive macro never prunes
//! elements it didn't create in the first place — ticket 06 already
//! decided not to port that engine — so there's nothing for a
//! `set_stub`-equivalent to signal here. A bare `<panic/>` (no `model`)
//! reads back as `model: None` and round-trips as `<panic/>` either way.

use virtinst_xml::XmlBound;

/// ```xml
/// <panic model="isa"/>
/// ```
/// or, the stub case telling libvirt to pick a default:
/// ```xml
/// <panic/>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "panic")]
pub struct DevicePanic {
    #[xml(attribute = "model")]
    pub model: Option<String>,
}

// `DevicePanic` as an `XMLBuilder` class, for the CLI parsers.

use crate::xmlbuilder::{prop, Kind, Value, XmlClass, XmlObject};

/// `DevicePanic`.
pub static DEVICE_PANIC: XmlClass = XmlClass {
    xml_name: "panic",
    props: &[
        prop("model", "./@model", Kind::Str),
        prop("set_stub", ".", Kind::Bool),
    ],
    children: crate::device_children!(),
    prop_order: crate::device_prop_order!(),
};

/// `DevicePanic.set_defaults`.
pub fn set_defaults(dev: &mut XmlObject) -> Result<(), String> {
    let empty = |v: Option<String>| v.map_or(true, |s| s.is_empty());
    if empty(dev.get_path("address.type").map(|v| v.to_xml_string()))
        && !empty(dev.get_path("address.iobase").map(|v| v.to_xml_string()))
    {
        dev.set_path("address.type", Some("isa".into()))?;
    }
    if empty(dev.get_str("model")) {
        // This asks libvirt to fill in a default
        dev.set("set_stub", Some(Value::Bool(true)))?;
    }
    Ok(())
}
