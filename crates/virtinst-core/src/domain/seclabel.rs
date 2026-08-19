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
