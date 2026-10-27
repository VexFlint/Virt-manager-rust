//! `<memory>` (memory devices: dimm, nvdimm, virtio-mem...) — upstream's
//! `devices/memory.py`, as an `XMLBuilder` class for the CLI parsers.

use crate::xmlbuilder::{child, prop, Kind, XmlClass};

/// `_DeviceMemoryTarget`.
pub static DEVICE_MEMORY_TARGET: XmlClass = XmlClass {
    xml_name: "target",
    props: &[
        prop("size", "./size", Kind::Int),
        prop("node", "./node", Kind::Int),
        prop("label_size", "./label/size", Kind::Int),
        prop("readonly", "./readonly", Kind::Bool),
        prop("block", "./block", Kind::Int),
        prop("requested", "./requested", Kind::Int),
        prop("current", "./current", Kind::Int),
        prop("address_base", "./address/@base", Kind::Str),
        prop("dynamicMemslots", "./@dynamicMemslots", Kind::YesNo),
    ],
    children: &[],
    prop_order: &[],
};

/// `_DeviceMemorySource`.
pub static DEVICE_MEMORY_SOURCE: XmlClass = XmlClass {
    xml_name: "source",
    props: &[
        prop("pagesize", "./pagesize", Kind::Int),
        prop("nodemask", "./nodemask", Kind::Str),
        prop("path", "./path", Kind::Str),
        prop("alignsize", "./alignsize", Kind::Int),
        prop("pmem", "./pmem", Kind::Bool),
    ],
    children: &[],
    prop_order: &[],
};

/// `DeviceMemory`.
pub static DEVICE_MEMORY: XmlClass = XmlClass {
    xml_name: "memory",
    props: &[
        prop("model", "./@model", Kind::Str),
        prop("access", "./@access", Kind::Str),
        prop("discard", "./@discard", Kind::YesNo),
        prop("uuid", "./uuid", Kind::Str),
    ],
    children: crate::device_children!(
        child("source", &DEVICE_MEMORY_SOURCE, ".", true),
        child("target", &DEVICE_MEMORY_TARGET, ".", true),
    ),
    prop_order: crate::device_prop_order!(),
};
