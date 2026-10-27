//! `<video>` — upstream's `devices/video.py`. Grounded in the real
//! `XMLProperty` declarations. Upstream wraps `model` in a Python
//! getter/setter property (clearing dependent fields when the model
//! changes away from `qxl`) — that's Python-level convenience logic on
//! top of the underlying binding, not part of the XPath shape itself;
//! the real XPath is plain `./model/@type` like every sibling field
//! here. `blob = XMLProperty("./model/@blob", is_onoff=True)` is the
//! same flagged on/off-boolean gap as other devices — not modeled.
//!
//! Last of ticket 12's 17 `NewDevice` variants to get a real XML
//! binding.

use virtinst_xml::XmlBound;

/// ```xml
/// <video>
///   <model type="qxl" ram="65536" vram="65536" vgamem="16384" heads="1" primary="yes"/>
/// </video>
/// ```
/// or
/// ```xml
/// <video>
///   <model type="virtio" heads="1">
///     <acceleration accel3d="yes"/>
///   </model>
/// </video>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "video")]
pub struct DeviceVideo {
    #[xml(path = "model", attribute = "type")]
    pub model: Option<String>,

    #[xml(path = "model", attribute = "vram")]
    pub vram: Option<u32>,

    #[xml(path = "model", attribute = "vram64")]
    pub vram64: Option<u32>,

    #[xml(path = "model", attribute = "ram")]
    pub ram: Option<u32>,

    #[xml(path = "model", attribute = "heads")]
    pub heads: Option<u32>,

    #[xml(path = "model", attribute = "vgamem")]
    pub vgamem: Option<u32>,

    #[xml(path = "model", attribute = "primary")]
    pub primary: Option<bool>,

    #[xml(path = "model/acceleration", attribute = "accel3d")]
    pub accel3d: Option<bool>,
}

// Editing an existing `<video>` in place, for the details page.

use virtinst_xml::{Document, Element};

use super::edit::{attr, set_attr};

/// `define_video`'s `model`. A new model drops the guest address and
/// the sizes and 3D setting, so they go back to the new model's
/// defaults; models other than `qxl` never keep `ram`/`vgamem`/`vram64`.
pub fn set_model(doc: &mut Document, video: Element, model: &str) {
    if attr(doc, video, &["model"], "type").as_deref() == Some(model) {
        return;
    }
    set_attr(doc, video, "model", "type", Some(model));
    super::edit::clear_address(doc, video);
    for name in ["vram", "heads", "ram", "vgamem", "vram64"] {
        set_attr(doc, video, "model", name, None);
    }
    set_attr(doc, video, "model/acceleration", "accel3d", None);
}

/// `define_video`'s `accel3d`: `./model/acceleration/@accel3d`, yes/no.
pub fn set_accel3d(doc: &mut Document, video: Element, val: bool) {
    let val = if val { "yes" } else { "no" };
    set_attr(doc, video, "model/acceleration", "accel3d", Some(val));
}

/// `video_recommended_models`, by `conn.is_xen()` / `is_qemu()`.
pub fn recommended_models(is_xen: bool, is_qemu: bool) -> &'static [&'static str] {
    if is_xen {
        &["xen", "vga"]
    } else if is_qemu {
        &["vga", "bochs", "qxl", "virtio", "ramfb", "none"]
    } else {
        &[]
    }
}

// `DeviceVideo` as an `XMLBuilder` class, for the CLI parsers.

use crate::xmlbuilder::{prop, Kind, Value, XmlClass, XmlObject};

/// `DeviceVideo`.
pub static DEVICE_VIDEO: XmlClass = XmlClass {
    xml_name: "video",
    props: &[
        prop("vram", "./model/@vram", Kind::Int),
        prop("vram64", "./model/@vram64", Kind::Int),
        prop("ram", "./model/@ram", Kind::Int),
        prop("heads", "./model/@heads", Kind::Int),
        prop("vgamem", "./model/@vgamem", Kind::Int),
        prop("accel3d", "./model/acceleration/@accel3d", Kind::YesNo),
        prop("blob", "./model/@blob", Kind::OnOff),
        prop("primary", "./model/@primary", Kind::YesNo),
        prop("_model", "./model/@type", Kind::Str),
    ],
    children: crate::device_children!(),
    prop_order: crate::device_prop_order!("_model", "vram", "heads", "vgamem"),
};

/// `DeviceVideo.model`'s setter: anything but qxl drops the qxl only
/// sizes.
pub fn set_model_prop(dev: &mut XmlObject, val: Option<Value>) -> Result<(), String> {
    let is_qxl = val.as_ref().and_then(|v| v.as_str()) == Some("qxl");
    dev.set("_model", val)?;
    if !is_qxl {
        dev.set("ram", None)?;
        dev.set("vgamem", None)?;
        dev.set("vram64", None)?;
    }
    Ok(())
}

/// `DeviceVideo.set_defaults`. `default_model` is
/// `DeviceVideo.default_model(guest)`, `has_gl` the guest's.
pub fn set_defaults(
    dev: &mut XmlObject,
    default_model: Option<&str>,
    has_gl: bool,
) -> Result<(), String> {
    if dev.get_str("_model").map_or(true, |m| m.is_empty()) {
        set_model_prop(dev, default_model.map(Value::from))?;
    }
    if dev.get_str("_model").as_deref() == Some("virtio") && has_gl {
        dev.set("accel3d", Some(Value::Bool(true)))?;
    }
    Ok(())
}

#[cfg(test)]
mod edit_tests {
    use super::*;

    #[test]
    fn model_change_resets_sizes() {
        let xml = "<video><model type='qxl' ram='65536' vram='65536' vgamem='16384' \
            heads='1' primary='yes'><acceleration accel3d='no'/></model>\
            <address type='pci' slot='0x01'/></video>";
        let mut doc = virtinst_xml::parse_libvirt_xml(xml).unwrap();
        let video = doc.root_element().unwrap();
        set_model(&mut doc, video, "virtio");
        let out = doc.write_str().unwrap();
        assert!(out.contains("type=\"virtio\""), "{out}");
        assert!(out.contains("primary=\"yes\""), "{out}");
        for gone in ["ram=", "vgamem=", "heads=", "<acceleration", "<address"] {
            assert!(!out.contains(gone), "{out}");
        }
        set_accel3d(&mut doc, video, true);
        let out = doc.write_str().unwrap();
        assert!(out.contains("<acceleration accel3d=\"yes\"/>"), "{out}");
    }
}
