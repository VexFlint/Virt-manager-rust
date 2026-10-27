//! `<tpm>` — upstream's `devices/tpm.py`. Grounded in the real
//! `XMLProperty`/`XMLChildProperty` declarations: `active_pcr_banks` is
//! `XMLChildProperty(_ActivePCRBanks, is_single=True, relative_xpath="./backend")`,
//! so `sha1`/`sha256`/etc. (each `is_bool=True`, upstream's own nested
//! helper class) sit three levels deep —
//! `backend/active_pcr_banks/sha1` — the first `#[xml(present)]` field
//! with a multi-segment path.

use virtinst_xml::XmlBound;

/// ```xml
/// <tpm model="tpm-crb">
///   <backend type="emulator" version="2.0" persistent_state="yes">
///     <active_pcr_banks>
///       <sha256/>
///     </active_pcr_banks>
///   </backend>
/// </tpm>
/// ```
/// or
/// ```xml
/// <tpm model="tpm-tis">
///   <backend type="passthrough">
///     <device path="/dev/tpm0"/>
///   </backend>
/// </tpm>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "tpm")]
pub struct DeviceTpm {
    #[xml(attribute = "model")]
    pub model: Option<String>,

    #[xml(path = "backend", attribute = "type")]
    pub backend_type: Option<String>,

    #[xml(path = "backend", attribute = "version")]
    pub version: Option<String>,

    #[xml(path = "backend", attribute = "persistent_state")]
    pub persistent_state: Option<bool>,

    #[xml(path = "backend/device", attribute = "path")]
    pub device_path: Option<String>,

    #[xml(path = "backend/active_pcr_banks/sha1", present)]
    pub sha1: bool,

    #[xml(path = "backend/active_pcr_banks/sha256", present)]
    pub sha256: bool,
}

// Editing an existing `<tpm>` in place, for the details page.

use virtinst_xml::{Document, Element};

pub const TYPE_PASSTHROUGH: &str = "passthrough";
pub const TYPE_EMULATOR: &str = "emulator";
pub const MODEL_TIS: &str = "tpm-tis";
pub const MODEL_CRB: &str = "tpm-crb";
pub const MODEL_SPAPR: &str = "tpm-spapr";
pub const VERSION_1_2: &str = "1.2";
pub const VERSION_2_0: &str = "2.0";

/// `define_tpm`'s `type`.
pub fn set_type(doc: &mut Document, tpm: Element, val: Option<&str>) {
    super::edit::set_attr(doc, tpm, "backend", "type", val);
}

/// `define_tpm`'s `model`.
pub fn set_model(doc: &mut Document, tpm: Element, val: Option<&str>) {
    super::edit::set_attr(doc, tpm, "", "model", val);
}

/// `define_tpm`'s `version`.
pub fn set_version(doc: &mut Document, tpm: Element, val: Option<&str>) {
    super::edit::set_attr(doc, tpm, "backend", "version", val);
}

/// `define_tpm`'s `device_path`.
pub fn set_device_path(doc: &mut Document, tpm: Element, val: Option<&str>) {
    super::edit::set_attr(doc, tpm, "backend/device", "path", val);
}

// `DeviceTpm` as an `XMLBuilder` class, for the CLI parsers.

use crate::xmlbuilder::{child, prop, Kind, XmlClass, XmlObject};

/// `_ActivePCRBanks`.
pub static TPM_ACTIVE_PCR_BANKS: XmlClass = XmlClass {
    xml_name: "active_pcr_banks",
    props: &[
        prop("sha1", "./sha1", Kind::Bool),
        prop("sha256", "./sha256", Kind::Bool),
        prop("sha384", "./sha384", Kind::Bool),
        prop("sha512", "./sha512", Kind::Bool),
    ],
    children: &[],
    prop_order: &[],
};

/// `DeviceTpm`.
pub static DEVICE_TPM: XmlClass = XmlClass {
    xml_name: "tpm",
    props: &[
        prop("type", "./backend/@type", Kind::Str),
        prop("version", "./backend/@version", Kind::Str),
        prop("model", "./@model", Kind::Str),
        prop("device_path", "./backend/device/@path", Kind::Str),
        prop(
            "encryption_secret",
            "./backend/encryption/@secret",
            Kind::Str,
        ),
        prop(
            "persistent_state",
            "./backend/@persistent_state",
            Kind::YesNo,
        ),
        prop("debug", "./backend/@debug", Kind::Str),
        prop("source_type", "./backend/source/@type", Kind::Str),
        prop("source_path", "./backend/source/@path", Kind::Str),
        prop("profile_source", "./backend/profile/@source", Kind::Str),
        prop(
            "profile_removeDisabled",
            "./backend/profile/@removeDisabled",
            Kind::Str,
        ),
    ],
    children: crate::device_children!(child(
        "active_pcr_banks",
        &TPM_ACTIVE_PCR_BANKS,
        "./backend",
        true
    )),
    prop_order: crate::device_prop_order!(),
};

/// `DeviceTpm.set_defaults`. `default_model` is
/// `DeviceTpm.default_model(guest)`, asked for only when needed.
pub fn set_defaults(
    dev: &mut XmlObject,
    default_model: impl FnOnce() -> Option<&'static str>,
) -> Result<(), String> {
    let empty = |v: Option<String>| v.map_or(true, |s| s.is_empty());
    if !empty(dev.get_str("device_path")) && empty(dev.get_str("type")) {
        dev.set("type", Some(TYPE_PASSTHROUGH.into()))?;
    }
    if empty(dev.get_str("type")) {
        // Libvirt requires a backend type to be specified. 'emulator'
        // may not be available if swtpm is not installed, but trying to
        // fallback to 'passthrough' in that case isn't really workable.
        // Instead we specify it unconditionally and let libvirt error.
        dev.set("type", Some(TYPE_EMULATOR.into()))?;
    }
    // passthrough and model and version are all interconnected, so
    // don't try to set a default model if other bits are set
    if dev.get_str("type").as_deref() == Some(TYPE_EMULATOR)
        && empty(dev.get_str("model"))
        && empty(dev.get_str("version"))
    {
        dev.set("model", default_model().map(Into::into))?;
    }
    Ok(())
}

#[cfg(test)]
mod edit_tests {
    use super::*;

    #[test]
    fn emulated_to_passthrough() {
        let xml = "<tpm model='tpm-crb'><backend type='emulator' version='2.0'/></tpm>";
        let mut doc = virtinst_xml::parse_libvirt_xml(xml).unwrap();
        let tpm = doc.root_element().unwrap();
        set_type(&mut doc, tpm, Some(TYPE_PASSTHROUGH));
        set_version(&mut doc, tpm, None);
        set_device_path(&mut doc, tpm, Some("/dev/tpm0"));
        set_model(&mut doc, tpm, Some(MODEL_TIS));
        let out = doc.write_str().unwrap();
        assert!(out.contains("model=\"tpm-tis\""), "{out}");
        assert!(out.contains("type=\"passthrough\""), "{out}");
        assert!(out.contains("<device path=\"/dev/tpm0\""), "{out}");
        assert!(!out.contains("version=\"2.0\""), "{out}");
    }
}
