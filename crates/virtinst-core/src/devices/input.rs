//! `<input>` — upstream's `devices/input.py`. Small device, grounded in
//! the real `XMLProperty` declarations same as every other struct.
//! `source/@repeat` uses the `on`/`off` boolean spelling (`is_onoff`) —
//! left out for now, same flagged gap as `DeviceController`'s
//! `target/@hotplug`.

use virtinst_xml::XmlBound;

/// ```xml
/// <input type="tablet" bus="usb"/>
/// ```
/// or
/// ```xml
/// <input type="evdev" bus="virtio">
///   <source dev="/dev/input/event1" grab="all"/>
/// </input>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "input")]
pub struct DeviceInput {
    #[xml(attribute = "type")]
    pub input_type: Option<String>,

    #[xml(attribute = "bus")]
    pub bus: Option<String>,

    #[xml(attribute = "model")]
    pub model: Option<String>,

    #[xml(path = "source", attribute = "evdev")]
    pub source_evdev: Option<String>,

    #[xml(path = "source", attribute = "dev")]
    pub source_dev: Option<String>,

    #[xml(path = "source", attribute = "grab")]
    pub source_grab: Option<String>,
}

// `DeviceInput` as an `XMLBuilder` class, for the CLI parsers.

use crate::xmlbuilder::{prop, Kind, XmlClass, XmlObject};

/// `DeviceInput`.
pub static DEVICE_INPUT: XmlClass = XmlClass {
    xml_name: "input",
    props: &[
        prop("type", "./@type", Kind::Str),
        prop("bus", "./@bus", Kind::Str),
        prop("model", "./@model", Kind::Str),
        prop("source_evdev", "./source/@evdev", Kind::Str),
        prop("source_dev", "./source/@dev", Kind::Str),
        prop("source_repeat", "./source/@repeat", Kind::OnOff),
        prop("source_grab", "./source/@grab", Kind::Str),
        prop("source_grabToggle", "./source/@grabToggle", Kind::Str),
    ],
    children: crate::device_children!(),
    prop_order: crate::device_prop_order!(),
};

/// `DeviceInput.set_defaults`.
pub fn set_defaults(dev: &mut XmlObject) -> Result<(), String> {
    if dev.get_str("type").map_or(true, |t| t.is_empty()) {
        dev.set("type", Some("mouse".into()))?;
    }
    Ok(())
}
