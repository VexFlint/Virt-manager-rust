//! `<clock>` — upstream's `domain/clock.py`. Started as the simplest
//! possible exercise of the derive macro (a single flat attribute,
//! no nested elements) — but that first pass missed `timers`, upstream's
//! `_ClockTimer` list (`<clock><timer name="..." .../></clock>`),
//! found while surveying `guest.py`'s `XMLChildProperty` calls for
//! wiring `Guest` together. Closed now, same "added when needed, found
//! by reading the real declarations" pattern as everywhere else.
//!
//! One of ticket 17's 9 files that branches on driver family (Xen's
//! default `offset` differs from QEMU's) and `has_hyperv_timer()`
//! (checked by `DomainFeatures::_set_hyperv_defaults` upstream) — both
//! default-resolution/business logic, not modeled here.

use virtinst_xml::XmlBound;

/// `<clock><timer .../></clock>` — upstream's `_ClockTimer`.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "timer")]
pub struct ClockTimer {
    #[xml(attribute = "name")]
    pub name: Option<String>,

    #[xml(attribute = "present")]
    pub present: Option<bool>,

    #[xml(attribute = "tickpolicy")]
    pub tickpolicy: Option<String>,

    #[xml(attribute = "track")]
    pub track: Option<String>,

    #[xml(attribute = "mode")]
    pub mode: Option<String>,

    #[xml(attribute = "frequency")]
    pub frequency: Option<u32>,

    #[xml(path = "catchup", attribute = "threshold")]
    pub threshold: Option<u32>,

    #[xml(path = "catchup", attribute = "slew")]
    pub slew: Option<u32>,

    #[xml(path = "catchup", attribute = "limit")]
    pub limit: Option<u32>,
}

/// ```xml
/// <clock offset="utc">
///   <timer name="rtc" tickpolicy="catchup"/>
///   <timer name="pit" tickpolicy="delay"/>
/// </clock>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "clock")]
pub struct Clock {
    #[xml(attribute = "offset")]
    pub offset: Option<String>,

    #[xml(list)]
    pub timers: Vec<ClockTimer>,
}

/// `_ClockTimer`, for the CLI parsers.
pub static CLOCK_TIMER: crate::xmlbuilder::XmlClass = {
    use crate::xmlbuilder::{prop, Kind, XmlClass};
    XmlClass {
        xml_name: "timer",
        props: &[
            prop("name", "./@name", Kind::Str),
            prop("present", "./@present", Kind::YesNo),
            prop("tickpolicy", "./@tickpolicy", Kind::Str),
            prop("track", "./@track", Kind::Str),
            prop("mode", "./@mode", Kind::Str),
            prop("frequency", "./@frequency", Kind::Int),
            prop("threshold", "./catchup/@threshold", Kind::Int),
            prop("slew", "./catchup/@slew", Kind::Int),
            prop("limit", "./catchup/@limit", Kind::Int),
        ],
        children: &[],
        prop_order: &[],
    }
};

/// `DomainClock`, for the CLI parsers.
pub static DOMAIN_CLOCK: crate::xmlbuilder::XmlClass = {
    use crate::xmlbuilder::{child, prop, Kind, XmlClass};
    XmlClass {
        xml_name: "clock",
        props: &[prop("offset", "./@offset", Kind::Str)],
        children: &[child("timers", &CLOCK_TIMER, ".", false)],
        prop_order: &[],
    }
};
