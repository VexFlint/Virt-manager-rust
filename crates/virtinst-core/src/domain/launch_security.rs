//! `<launchSecurity>` — upstream's `domain/launch_security.py` (SEV/
//! SEV-ES/SEV-SNP/TDX confidential-computing attestation data). Mostly
//! flat text children plus three `is_yesno=True` attributes and one
//! `is_bool=True` presence marker with its own nested attribute
//! (`quoteGenerationService`/`quoteGenerationSocket` — the same
//! present-plus-sibling-attribute shape `DeviceDisk`'s `readonly` +
//! neighboring attributes already established, just both landing on
//! the *same* element here instead of siblings).
//!
//! `_set_defaults_sev*`/`_set_defaults_tdx` (Q35 UEFI validation,
//! policy defaulting) aren't modeled — default-resolution logic,
//! ticket 17, same as everywhere else.

use virtinst_xml::XmlBound;

/// ```xml
/// <launchSecurity type="sev" kernelHashes="yes">
///   <cbitpos>47</cbitpos>
///   <reducedPhysBits>1</reducedPhysBits>
///   <policy>0x0003</policy>
///   <session>BASE64</session>
///   <dhCert>BASE64</dhCert>
/// </launchSecurity>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "launchSecurity")]
pub struct DomainLaunchSecurity {
    #[xml(attribute = "type")]
    pub security_type: Option<String>,

    #[xml(path = "cbitpos", text)]
    pub cbitpos: Option<u32>,

    #[xml(path = "reducedPhysBits", text)]
    pub reduced_phys_bits: Option<u32>,

    #[xml(path = "policy", text)]
    pub policy: Option<String>,

    #[xml(path = "session", text)]
    pub session: Option<String>,

    #[xml(path = "dhCert", text)]
    pub dh_cert: Option<String>,

    #[xml(path = "guestVisibleWorkarounds", text)]
    pub guest_visible_workarounds: Option<String>,

    #[xml(path = "idBlock", text)]
    pub id_block: Option<String>,

    #[xml(path = "idAuth", text)]
    pub id_auth: Option<String>,

    #[xml(path = "hostData", text)]
    pub host_data: Option<String>,

    #[xml(attribute = "kernelHashes")]
    pub kernel_hashes: Option<bool>,

    #[xml(attribute = "authorKey")]
    pub author_key: Option<bool>,

    #[xml(attribute = "vcek")]
    pub vcek: Option<bool>,

    #[xml(path = "mrConfigId", text)]
    pub mr_config_id: Option<String>,

    #[xml(path = "mrOwner", text)]
    pub mr_owner: Option<String>,

    #[xml(path = "mrOwnerConfig", text)]
    pub mr_owner_config: Option<String>,

    #[xml(path = "quoteGenerationService", present)]
    pub quote_generation_service: bool,

    #[xml(path = "quoteGenerationService", attribute = "path")]
    pub quote_generation_socket: Option<String>,
}

/// `DomainLaunchSecurity`.
pub static DOMAIN_LAUNCH_SECURITY: crate::xmlbuilder::XmlClass = {
    use crate::xmlbuilder::{prop, Kind, XmlClass};
    XmlClass {
        xml_name: "launchSecurity",
        props: &[
            prop("type", "./@type", Kind::Str),
            prop("cbitpos", "./cbitpos", Kind::Int),
            prop("reducedPhysBits", "./reducedPhysBits", Kind::Int),
            prop("policy", "./policy", Kind::Str),
            prop("session", "./session", Kind::Str),
            prop("dhCert", "./dhCert", Kind::Str),
            prop(
                "guestVisibleWorkarounds",
                "./guestVisibleWorkarounds",
                Kind::Str,
            ),
            prop("idBlock", "./idBlock", Kind::Str),
            prop("idAuth", "./idAuth", Kind::Str),
            prop("hostData", "./hostData", Kind::Str),
            prop("kernelHashes", "./@kernelHashes", Kind::YesNo),
            prop("authorKey", "./@authorKey", Kind::YesNo),
            prop("vcek", "./@vcek", Kind::YesNo),
            prop("mrConfigId", "./mrConfigId", Kind::Str),
            prop("mrOwner", "./mrOwner", Kind::Str),
            prop("mrOwnerConfig", "./mrOwnerConfig", Kind::Str),
            prop(
                "quoteGenerationService",
                "./quoteGenerationService",
                Kind::Bool,
            ),
            prop(
                "quoteGenerationSocket",
                "./quoteGenerationService/@path",
                Kind::Str,
            ),
        ],
        children: &[],
        prop_order: &[
            "type",
            "cbitpos",
            "reducedPhysBits",
            "policy",
            "session",
            "dhCert",
        ],
    }
};

/// `DomainLaunchSecurity.set_defaults`. `q35_uefi` is the guest's
/// `os.is_q35() and is_uefi()`; `supports_sev_es` its domcaps'
/// `supports_sev_launch_security(check_es=True)`.
pub fn set_defaults(
    obj: &mut crate::xmlbuilder::XmlObject,
    q35_uefi: bool,
    supports_sev_es: impl FnOnce() -> bool,
) -> Result<(), String> {
    let name = match obj.get_str("type").as_deref() {
        Some("sev") => "SEV",
        Some("sev-snp") => "SEV-SNP",
        Some("tdx") => "TDX",
        _ => return Ok(()),
    };
    if !q35_uefi {
        return Err(format!(
            "{name} launch security requires a Q35 UEFI machine"
        ));
    }
    // The 'policy' is a mandatory 4-byte argument for the SEV firmware.
    // If missing, we use 0x03 for the original SEV implementation and
    // 0x07 for SEV-ES.
    // Reference: https://libvirt.org/formatdomain.html#launch-security
    if name == "SEV" && obj.get("policy").is_none() {
        let policy = if supports_sev_es() { "0x07" } else { "0x03" };
        obj.set("policy", Some(crate::xmlbuilder::Value::Str(policy.into())))?;
    }
    Ok(())
}
