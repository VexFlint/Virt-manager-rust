//! `<pm>` — upstream's `domain/pm.py`. Two `is_yesno=True` attributes,
//! each on its own nested element (`./suspend-to-mem/@enabled`,
//! `./suspend-to-disk/@enabled`) — already-supported `Option<bool>`
//! binding, no new macro capability needed.
//!
//! `set_defaults` (disabling both when the connection supports it, to
//! avoid ACPI-shutdown-triggers-suspend guest confusion) isn't modeled
//! here — per-driver/connection default resolution is ticket 17's
//! not-yet-built pipeline stage, same as every other struct so far.

use virtinst_xml::XmlBound;

/// ```xml
/// <pm>
///   <suspend-to-mem enabled="no"/>
///   <suspend-to-disk enabled="yes"/>
/// </pm>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "pm")]
pub struct DomainPm {
    #[xml(path = "suspend-to-mem", attribute = "enabled")]
    pub suspend_to_mem: Option<bool>,

    #[xml(path = "suspend-to-disk", attribute = "enabled")]
    pub suspend_to_disk: Option<bool>,
}
