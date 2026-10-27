//! `<memoryBacking>` — upstream's `domain/memorybacking.py`. First
//! `#[xml(list)]` field combined with a `path` — upstream's
//! `relative_xpath="./hugepages"` puts `<page>` entries a level below
//! `<memoryBacking>` itself (`<memoryBacking><hugepages><page/></hugepages></memoryBacking>`),
//! which `list_read`'s own `path` parameter already supports (it
//! resolves the path to a container first, same as every other field
//! kind), just not yet exercised by an existing struct.

use virtinst_xml::XmlBound;

/// A single `<page>` entry inside `<memoryBacking><hugepages>` —
/// upstream's `_HugepagesPage`.
///
/// ```xml
/// <page size="1" unit="GiB" nodeset="0-1"/>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "page")]
pub struct HugepagesPage {
    #[xml(attribute = "size")]
    pub size: Option<String>,

    #[xml(attribute = "unit")]
    pub unit: Option<String>,

    #[xml(attribute = "nodeset")]
    pub nodeset: Option<String>,
}

/// ```xml
/// <memoryBacking>
///   <hugepages>
///     <page size="1" unit="GiB" nodeset="0-1"/>
///   </hugepages>
///   <locked/>
///   <access mode="shared"/>
///   <allocation mode="immediate" threads="8"/>
/// </memoryBacking>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "memoryBacking")]
pub struct DomainMemoryBacking {
    #[xml(path = "hugepages", present)]
    pub hugepages: bool,

    #[xml(path = "nosharepages", present)]
    pub nosharepages: bool,

    #[xml(path = "locked", present)]
    pub locked: bool,

    #[xml(path = "discard", present)]
    pub discard: bool,

    #[xml(path = "access", attribute = "mode")]
    pub access_mode: Option<String>,

    #[xml(path = "source", attribute = "type")]
    pub source_type: Option<String>,

    #[xml(path = "allocation", attribute = "mode")]
    pub allocation_mode: Option<String>,

    #[xml(path = "allocation", attribute = "threads")]
    pub allocation_threads: Option<u32>,

    #[xml(path = "hugepages", list)]
    pub pages: Vec<HugepagesPage>,
}

/// `_HugepagesPage`, for the CLI parsers.
pub static HUGEPAGES_PAGE: crate::xmlbuilder::XmlClass = {
    use crate::xmlbuilder::{prop, Kind, XmlClass};
    XmlClass {
        xml_name: "page",
        props: &[
            prop("size", "./@size", Kind::Str),
            prop("unit", "./@unit", Kind::Str),
            prop("nodeset", "./@nodeset", Kind::Str),
        ],
        children: &[],
        prop_order: &[],
    }
};

/// `DomainMemoryBacking`, for the CLI parsers.
pub static DOMAIN_MEMORY_BACKING: crate::xmlbuilder::XmlClass = {
    use crate::xmlbuilder::{child, prop, Kind, XmlClass};
    XmlClass {
        xml_name: "memoryBacking",
        props: &[
            prop("hugepages", "./hugepages", Kind::Bool),
            prop("nosharepages", "./nosharepages", Kind::Bool),
            prop("locked", "./locked", Kind::Bool),
            prop("discard", "./discard", Kind::Bool),
            prop("access_mode", "./access/@mode", Kind::Str),
            prop("source_type", "./source/@type", Kind::Str),
            prop("allocation_mode", "./allocation/@mode", Kind::Str),
            prop("allocation_threads", "./allocation/@threads", Kind::Int),
        ],
        children: &[child("pages", &HUGEPAGES_PAGE, "./hugepages", false)],
        prop_order: &["hugepages", "nosharepages", "locked", "pages"],
    }
};
