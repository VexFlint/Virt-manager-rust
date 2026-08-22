//! `<memballoon>` — upstream's `devices/memballoon.py`. Grounded in the
//! real `XMLProperty` declarations: `model` and `stats_period` are
//! plain/int self- and nested-attributes; `autodeflate` and
//! `freePageReporting` are both `is_onoff=True` — `Option<OnOff>`
//! (`virtinst_xml`), the same convention `DeviceController`'s
//! `target_hotplug` already established, not a new one.
//!
//! Upstream's `set_defaults` (`if not self.model: self.model =
//! "virtio"`) is CLI/build-time policy, not modeled here — same
//! boundary every other device struct in this crate draws (ticket 17
//! owns per-driver default resolution).

use virtinst_xml::{OnOff, XmlBound};

/// ```xml
/// <memballoon model="virtio" autodeflate="on">
///   <stats period="10"/>
/// </memballoon>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "memballoon")]
pub struct DeviceMemballoon {
    #[xml(attribute = "model")]
    pub model: Option<String>,

    #[xml(attribute = "autodeflate")]
    pub autodeflate: Option<OnOff>,

    #[xml(path = "stats", attribute = "period")]
    pub stats_period: Option<u32>,

    #[xml(attribute = "freePageReporting")]
    pub free_page_reporting: Option<OnOff>,
}
