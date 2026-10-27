//! `<pstore>` — upstream's `devices/pstore.py`, as an `XMLBuilder` class
//! for the CLI parsers.

use crate::xmlbuilder::{prop, Kind, XmlClass};

/// `DevicePstore`.
pub static DEVICE_PSTORE: XmlClass = XmlClass {
    xml_name: "pstore",
    props: &[
        prop("backend", "./@backend", Kind::Str),
        prop("path", "./path", Kind::Str),
        prop("size", "./size", Kind::Int),
    ],
    children: crate::device_children!(),
    prop_order: crate::device_prop_order!(),
};
