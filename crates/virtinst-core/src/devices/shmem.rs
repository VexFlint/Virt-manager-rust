//! `<shmem>` — upstream's `devices/shmem.py`, as an `XMLBuilder` class
//! for the CLI parsers.

use crate::xmlbuilder::{prop, Kind, XmlClass};

/// `DeviceShMem`.
pub static DEVICE_SHMEM: XmlClass = XmlClass {
    xml_name: "shmem",
    props: &[
        prop("name", "./@name", Kind::Str),
        prop("role", "./@role", Kind::Str),
        prop("type", "./model/@type", Kind::Str),
        prop("size", "./size", Kind::Int),
        prop("size_unit", "./size/@unit", Kind::Str),
        prop("server_path", "./server/@path", Kind::Str),
        prop("msi_vectors", "./msi/@vectors", Kind::Int),
        prop("msi_ioeventfd", "./msi/@ioeventfd", Kind::OnOff),
    ],
    children: crate::device_children!(),
    prop_order: crate::device_prop_order!(
        "name",
        "role",
        "type",
        "size",
        "size_unit",
        "server_path",
        "msi_vectors",
        "msi_ioeventfd",
    ),
};
