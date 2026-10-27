//! `<redirdev>` — upstream's `devices/redirdev.py`. Grounded in the
//! real `XMLProperty`/`XMLChildProperty` declarations: two self
//! attributes plus the same `CharSource` `<source>` subset already
//! modeled for `DeviceSerial` and `DeviceSmartcard`. No gaps.

use virtinst_xml::XmlBound;

/// ```xml
/// <redirdev bus="usb" type="spicevmc"/>
/// ```
/// or
/// ```xml
/// <redirdev bus="usb" type="tcp">
///   <source mode="connect" host="localhost" service="4000"/>
/// </redirdev>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "redirdev")]
pub struct DeviceRedirdev {
    #[xml(attribute = "bus")]
    pub bus: Option<String>,

    #[xml(attribute = "type")]
    pub redirdev_type: Option<String>,

    #[xml(path = "source", attribute = "mode")]
    pub source_mode: Option<String>,

    #[xml(path = "source", attribute = "host")]
    pub source_host: Option<String>,

    #[xml(path = "source", attribute = "service")]
    pub source_service: Option<u32>,
}

// `DeviceRedirdev` as an `XMLBuilder` class, for the CLI parsers.

use crate::xmlbuilder::{child, prop, Kind, XmlClass, XmlObject};

/// `DeviceRedirdev`.
pub static DEVICE_REDIRDEV: XmlClass = XmlClass {
    xml_name: "redirdev",
    props: &[
        prop("bus", "./@bus", Kind::Str),
        prop("type", "./@type", Kind::Str),
    ],
    children: crate::device_children!(child("source", &super::char::CHAR_SOURCE, ".", true)),
    prop_order: crate::device_prop_order!("bus", "type", "source"),
};

/// `DeviceRedirdev.set_defaults`.
pub fn set_defaults(dev: &mut XmlObject) -> Result<(), String> {
    if dev.get_str("bus").map_or(true, |b| b.is_empty()) {
        dev.set("bus", Some("usb".into()))?;
    }
    if dev.get_str("type").map_or(true, |t| t.is_empty()) {
        dev.set("type", Some("spicevmc".into()))?;
    }
    Ok(())
}
