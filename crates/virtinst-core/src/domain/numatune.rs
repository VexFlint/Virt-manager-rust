//! `<numatune>` — upstream's `domain/numatune.py`. A flat `<memory>`
//! attribute group plus a list of per-cell `<memnode>` entries.

use virtinst_xml::XmlBound;

/// A single `<memnode>` entry inside `<numatune>` — upstream's
/// `_Numatune`.
///
/// ```xml
/// <memnode cellid="0" mode="strict" nodeset="1"/>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "memnode")]
pub struct NumatuneMemnode {
    #[xml(attribute = "cellid")]
    pub cellid: Option<u32>,

    #[xml(attribute = "mode")]
    pub mode: Option<String>,

    #[xml(attribute = "nodeset")]
    pub nodeset: Option<String>,
}

/// ```xml
/// <numatune>
///   <memory mode="strict" nodeset="0-1" placement="static"/>
///   <memnode cellid="0" mode="strict" nodeset="1"/>
/// </numatune>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "numatune")]
pub struct DomainNumatune {
    #[xml(path = "memory", attribute = "nodeset")]
    pub memory_nodeset: Option<String>,

    #[xml(path = "memory", attribute = "mode")]
    pub memory_mode: Option<String>,

    #[xml(path = "memory", attribute = "placement")]
    pub memory_placement: Option<String>,

    #[xml(list)]
    pub memnode: Vec<NumatuneMemnode>,
}

/// `_Numatune`, for the CLI parsers.
pub static NUMATUNE_MEMNODE: crate::xmlbuilder::XmlClass = {
    use crate::xmlbuilder::{prop, Kind, XmlClass};
    XmlClass {
        xml_name: "memnode",
        props: &[
            prop("cellid", "./@cellid", Kind::Int),
            prop("mode", "./@mode", Kind::Str),
            prop("nodeset", "./@nodeset", Kind::Str),
        ],
        children: &[],
        prop_order: &["cellid", "mode", "nodeset"],
    }
};

/// `DomainNumatune`, for the CLI parsers.
pub static DOMAIN_NUMATUNE: crate::xmlbuilder::XmlClass = {
    use crate::xmlbuilder::{child, prop, Kind, XmlClass};
    XmlClass {
        xml_name: "numatune",
        props: &[
            prop("memory_nodeset", "./memory/@nodeset", Kind::Str),
            prop("memory_mode", "./memory/@mode", Kind::Str),
            prop("memory_placement", "./memory/@placement", Kind::Str),
        ],
        children: &[child("memnode", &NUMATUNE_MEMNODE, ".", false)],
        prop_order: &[
            "memory_mode",
            "memory_nodeset",
            "memory_placement",
            "memnode",
        ],
    }
};
