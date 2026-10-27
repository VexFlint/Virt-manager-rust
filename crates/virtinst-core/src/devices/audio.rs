//! `<audio>` — upstream's `devices/audio.py`, as an `XMLBuilder` class
//! for the CLI parsers.

use crate::xmlbuilder::{prop, Kind, XmlClass};

/// `DeviceAudio`.
pub static DEVICE_AUDIO: XmlClass = XmlClass {
    xml_name: "audio",
    props: &[
        prop("type", "./@type", Kind::Str),
        prop("id", "./@id", Kind::Str),
    ],
    children: crate::device_children!(),
    prop_order: crate::device_prop_order!(),
};
