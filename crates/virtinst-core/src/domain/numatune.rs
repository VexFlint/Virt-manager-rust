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
