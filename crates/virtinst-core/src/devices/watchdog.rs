//! `<watchdog>` — upstream's `devices/watchdog.py`. Grounded in the
//! real `XMLProperty` declarations: the simplest device yet, two flat
//! self-attributes and nothing else.

use virtinst_xml::XmlBound;

/// ```xml
/// <watchdog model="i6300esb" action="reset"/>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "watchdog")]
pub struct DeviceWatchdog {
    #[xml(attribute = "model")]
    pub model: Option<String>,

    #[xml(attribute = "action")]
    pub action: Option<String>,
}

// Editing an existing `<watchdog>` in place, for the details page.

use virtinst_xml::{Document, Element};

/// `DeviceWatchdog.MODELS`.
pub const MODELS: [&str; 3] = ["i6300esb", "ib700", "diag288"];
/// `DeviceWatchdog.ACTIONS`.
pub const ACTIONS: [&str; 6] = ["reset", "shutdown", "poweroff", "pause", "dump", "none"];

/// `define_watchdog`'s `model`: a new model drops the guest address.
pub fn set_model(doc: &mut Document, watchdog: Element, model: Option<&str>) {
    if watchdog.attribute(doc, "model") != model {
        super::edit::clear_address(doc, watchdog);
    }
    super::edit::set_attr(doc, watchdog, "", "model", model);
}

/// `define_watchdog`'s `action`.
pub fn set_action(doc: &mut Document, watchdog: Element, action: Option<&str>) {
    super::edit::set_attr(doc, watchdog, "", "action", action);
}
