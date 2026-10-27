//! `<iommu>` — upstream's `devices/iommu.py`, as an `XMLBuilder` class
//! for the CLI parsers.

use crate::xmlbuilder::{prop, Kind, XmlClass};

/// `DeviceIommu`.
pub static DEVICE_IOMMU: XmlClass = XmlClass {
    xml_name: "iommu",
    props: &[
        prop("model", "./@model", Kind::Str),
        prop("aw_bits", "./driver/@aw_bits", Kind::Int),
        prop("intremap", "./driver/@intremap", Kind::OnOff),
        prop("caching_mode", "./driver/@caching_mode", Kind::OnOff),
        prop("eim", "./driver/@eim", Kind::OnOff),
        prop("iotlb", "./driver/@iotlb", Kind::OnOff),
    ],
    children: crate::device_children!(),
    prop_order: crate::device_prop_order!(),
};
