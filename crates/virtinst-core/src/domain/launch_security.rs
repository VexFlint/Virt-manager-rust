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
