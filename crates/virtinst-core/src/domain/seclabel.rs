//! `<seclabel>` — upstream's `domain/seclabel.py`. Flat self-attributes
//! plus two plain text children. `_guess_secmodel`/`set_defaults`
//! (connection security-model detection/defaulting) not modeled —
//! default-resolution logic, same as everywhere else.

use virtinst_xml::XmlBound;

/// ```xml
/// <seclabel type="dynamic" model="selinux" relabel="yes">
///   <label>system_u:system_r:svirt_t:s0:c1,c2</label>
///   <baselabel>system_u:system_r:svirt_t:s0</baselabel>
/// </seclabel>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "seclabel")]
pub struct DomainSeclabel {
    #[xml(attribute = "model")]
    pub model: Option<String>,

    #[xml(attribute = "type")]
    pub seclabel_type: Option<String>,

    #[xml(path = "label", text)]
    pub label: Option<String>,

    #[xml(path = "baselabel", text)]
    pub baselabel: Option<String>,

    #[xml(attribute = "relabel")]
    pub relabel: Option<bool>,
}

/// `DomainSeclabel`.
pub static DOMAIN_SECLABEL: crate::xmlbuilder::XmlClass = {
    use crate::xmlbuilder::{prop, Kind, XmlClass};
    XmlClass {
        xml_name: "seclabel",
        props: &[
            prop("model", "./@model", Kind::Str),
            prop("type", "./@type", Kind::Str),
            prop("label", "./label", Kind::Str),
            prop("baselabel", "./baselabel", Kind::Str),
            prop("relabel", "./@relabel", Kind::YesNo),
        ],
        children: &[],
        prop_order: &["type", "model", "relabel", "label"],
    }
};

/// `DomainSeclabel._guess_secmodel`; `caps_models` are the host
/// capabilities' `secmodels` models.
fn guess_secmodel(label: Option<&str>, caps_models: &[Option<String>]) -> Option<String> {
    // We always want the testSecurity model when running tests
    if caps_models
        .iter()
        .any(|m| m.as_deref() == Some("testSecurity"))
    {
        return Some("testSecurity".into());
    }
    let label = label.filter(|l| !l.is_empty());
    let Some(label) = label else {
        return caps_models.first().cloned().flatten();
    };
    match label.split(':').count().min(3) {
        3 => Some("selinux".into()),
        2 => Some("dac".into()),
        _ => None,
    }
}

/// `DomainSeclabel.set_defaults`; `caps_models` is only called when
/// the model needs guessing.
pub fn set_defaults(
    obj: &mut crate::xmlbuilder::XmlObject,
    caps_models: impl FnOnce() -> Vec<Option<String>>,
) -> Result<(), String> {
    use crate::xmlbuilder::Value;
    let typ = obj.get_str("type");
    let model = obj.get_str("model");
    if typ.is_none() && model.is_none() {
        // Let libvirt fill it in
        return Ok(());
    }
    if typ.is_none() {
        obj.set("type", Some(Value::Str("dynamic".into())))?;
    }
    if model.is_none() {
        let guess = guess_secmodel(obj.get_str("label").as_deref(), &caps_models());
        obj.set("model", guess.map(Value::Str))?;
    }
    Ok(())
}
