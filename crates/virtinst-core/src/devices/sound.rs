//! `<sound>` — upstream's `devices/sound.py`. Grounded in the real
//! `XMLProperty` declarations: note `multichannel` uses `is_yesno=True`
//! (unlike the `is_onoff` fields skipped in a couple of earlier
//! devices) — the first real exercise of `Option<bool>` since
//! `DeviceGraphics`'s `autoport`. `streams` has no `is_int` flag
//! upstream despite the name, so it stays `Option<String>` rather than
//! guessed into `Option<u32>`.

use virtinst_xml::XmlBound;

/// ```xml
/// <sound model="ich9" multichannel="yes">
///   <audio id="1"/>
/// </sound>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "sound")]
pub struct DeviceSound {
    #[xml(attribute = "model")]
    pub model: Option<String>,

    #[xml(attribute = "multichannel")]
    pub multichannel: Option<bool>,

    #[xml(attribute = "streams")]
    pub streams: Option<String>,

    #[xml(path = "audio", attribute = "id")]
    pub audio_id: Option<String>,
}

// Editing an existing `<sound>` in place, for the details page.

use virtinst_xml::{Document, Element};

/// `sound_recommended_models`.
pub const RECOMMENDED_MODELS: [&str; 3] = ["ich6", "ich9", "ac97"];

/// `define_sound`'s `model`: a new model drops the guest address.
pub fn set_model(doc: &mut Document, sound: Element, model: &str) {
    if sound.attribute(doc, "model") != Some(model) {
        super::edit::clear_address(doc, sound);
    }
    sound.set_attribute(doc, "model", model);
}

// `DeviceSound` as an `XMLBuilder` class, for the CLI parsers.

use crate::xmlbuilder::{child, prop, Kind, XmlClass, XmlObject};

/// `_Codec`.
pub static SOUND_CODEC: XmlClass = XmlClass {
    xml_name: "codec",
    props: &[prop("type", "./@type", Kind::Str)],
    children: &[],
    prop_order: &[],
};

/// `DeviceSound`.
pub static DEVICE_SOUND: XmlClass = XmlClass {
    xml_name: "sound",
    props: &[
        prop("model", "./@model", Kind::Str),
        prop("audio_id", "./audio/@id", Kind::Str),
        prop("multichannel", "./@multichannel", Kind::YesNo),
        prop("streams", "./@streams", Kind::Str),
    ],
    children: crate::device_children!(child("codecs", &SOUND_CODEC, ".", false)),
    prop_order: crate::device_prop_order!(),
};

/// `DeviceSound.set_defaults`. `defaults_to_pcie` is the guest's.
pub fn set_defaults(dev: &mut XmlObject, defaults_to_pcie: bool) -> Result<(), String> {
    if dev.get_str("model").map_or(true, |m| m.is_empty()) {
        let model = if defaults_to_pcie { "ich9" } else { "ich6" };
        dev.set("model", Some(model.into()))?;
    }
    Ok(())
}
