//! `<blkiotune>` — upstream's `domain/blkiotune.py`. Per-device I/O
//! weight/throttle entries under a top-level weight. Note upstream's
//! own asymmetry, kept exactly: the top-level `weight` is
//! `is_int=True`, but `_BlkiotuneDevice.weight` isn't (plain text) —
//! not a typo here, `./path` and `./weight` on a `<device>` really are
//! both bound as strings upstream, only the `*_sec` throttles are ints.

use virtinst_xml::XmlBound;

/// A single `<device>` entry inside `<blkiotune>` — upstream's
/// `_BlkiotuneDevice`.
///
/// ```xml
/// <device>
///   <path>/dev/sda</path>
///   <weight>200</weight>
///   <read_bytes_sec>1000</read_bytes_sec>
///   <write_bytes_sec>1000</write_bytes_sec>
///   <read_iops_sec>10</read_iops_sec>
///   <write_iops_sec>10</write_iops_sec>
/// </device>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "device")]
pub struct BlkiotuneDevice {
    #[xml(path = "path", text)]
    pub path: Option<String>,

    #[xml(path = "weight", text)]
    pub weight: Option<String>,

    #[xml(path = "read_bytes_sec", text)]
    pub read_bytes_sec: Option<u32>,

    #[xml(path = "write_bytes_sec", text)]
    pub write_bytes_sec: Option<u32>,

    #[xml(path = "read_iops_sec", text)]
    pub read_iops_sec: Option<u32>,

    #[xml(path = "write_iops_sec", text)]
    pub write_iops_sec: Option<u32>,
}

/// ```xml
/// <blkiotune>
///   <weight>500</weight>
///   <device>...</device>
/// </blkiotune>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "blkiotune")]
pub struct DomainBlkiotune {
    #[xml(path = "weight", text)]
    pub weight: Option<u32>,

    #[xml(list)]
    pub devices: Vec<BlkiotuneDevice>,
}

/// `_BlkiotuneDevice`, for the CLI parsers.
pub static BLKIOTUNE_DEVICE: crate::xmlbuilder::XmlClass = {
    use crate::xmlbuilder::{prop, Kind, XmlClass};
    XmlClass {
        xml_name: "device",
        props: &[
            prop("path", "./path", Kind::Str),
            prop("weight", "./weight", Kind::Str),
            prop("read_bytes_sec", "./read_bytes_sec", Kind::Int),
            prop("write_bytes_sec", "./write_bytes_sec", Kind::Int),
            prop("read_iops_sec", "./read_iops_sec", Kind::Int),
            prop("write_iops_sec", "./write_iops_sec", Kind::Int),
        ],
        children: &[],
        prop_order: &["path", "weight"],
    }
};

/// `DomainBlkiotune`, for the CLI parsers.
pub static DOMAIN_BLKIOTUNE: crate::xmlbuilder::XmlClass = {
    use crate::xmlbuilder::{child, prop, Kind, XmlClass};
    XmlClass {
        xml_name: "blkiotune",
        props: &[prop("weight", "./weight", Kind::Int)],
        children: &[child("devices", &BLKIOTUNE_DEVICE, ".", false)],
        prop_order: &["weight"],
    }
};
