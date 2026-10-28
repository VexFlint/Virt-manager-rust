//! `<vcpus>` — upstream's `domain/vcpus.py`. First domain-subsystem
//! struct with a `#[xml(list)]` field (`DeviceList` already exercised
//! this shape for devices; `DomainVcpu` is the same pattern one level
//! up, a list of per-vcpu pinning/hotplug entries rather than a list
//! of devices).

use virtinst_xml::XmlBound;

/// A single `<vcpu>` entry inside `<vcpus>` — upstream's `_DomainVCPU`.
///
/// ```xml
/// <vcpu id="0" enabled="yes" hotpluggable="no" order="1"/>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "vcpu")]
pub struct DomainVcpu {
    #[xml(attribute = "id")]
    pub id: Option<u32>,

    #[xml(attribute = "enabled")]
    pub enabled: Option<bool>,

    #[xml(attribute = "hotpluggable")]
    pub hotpluggable: Option<bool>,

    #[xml(attribute = "order")]
    pub order: Option<u32>,
}

/// ```xml
/// <vcpus>
///   <vcpu id="0" enabled="yes" hotpluggable="no" order="1"/>
///   <vcpu id="1" enabled="yes" hotpluggable="yes" order="2"/>
/// </vcpus>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "vcpus")]
pub struct DomainVcpus {
    #[xml(list)]
    pub vcpu: Vec<DomainVcpu>,
}

// ---------------------------------------------------------------------
// `DomainVCPUs` as an XML backed object, for the CLI parsers.
// ---------------------------------------------------------------------

use crate::xmlbuilder::{child, prop, Kind, XmlClass};

/// `_DomainVCPU`.
pub static DOMAIN_VCPU: XmlClass = XmlClass {
    xml_name: "vcpu",
    props: &[
        prop("id", "./@id", Kind::Int),
        prop("enabled", "./@enabled", Kind::YesNo),
        prop("hotpluggable", "./@hotpluggable", Kind::YesNo),
        prop("order", "./@order", Kind::Int),
    ],
    children: &[],
    prop_order: &["id", "enabled", "hotpluggable", "order"],
};

/// `DomainVCPUs`.
pub static DOMAIN_VCPUS: XmlClass = XmlClass {
    xml_name: "vcpus",
    props: &[],
    children: &[child("vcpu", &DOMAIN_VCPU, ".", false)],
    prop_order: &[],
};
