//! `<idmap>` — upstream's `domain/idmap.py`, user-namespace UID/GID
//! mapping for containers. Six flat `is_int=True` attributes across two
//! nested elements (`./uid/@*`, `./gid/@*`) — already-exercised shapes,
//! no new macro capability needed.

use virtinst_xml::XmlBound;

/// ```xml
/// <idmap>
///   <uid start="0" target="1000" count="10"/>
///   <gid start="0" target="1000" count="10"/>
/// </idmap>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "idmap")]
pub struct DomainIdmap {
    #[xml(path = "uid", attribute = "start")]
    pub uid_start: Option<u32>,

    #[xml(path = "uid", attribute = "target")]
    pub uid_target: Option<u32>,

    #[xml(path = "uid", attribute = "count")]
    pub uid_count: Option<u32>,

    #[xml(path = "gid", attribute = "start")]
    pub gid_start: Option<u32>,

    #[xml(path = "gid", attribute = "target")]
    pub gid_target: Option<u32>,

    #[xml(path = "gid", attribute = "count")]
    pub gid_count: Option<u32>,
}

/// `DomainIdmap`.
pub static DOMAIN_IDMAP: crate::xmlbuilder::XmlClass = {
    use crate::xmlbuilder::{prop, Kind, XmlClass};
    XmlClass {
        xml_name: "idmap",
        props: &[
            prop("uid_start", "./uid/@start", Kind::Int),
            prop("uid_target", "./uid/@target", Kind::Int),
            prop("uid_count", "./uid/@count", Kind::Int),
            prop("gid_start", "./gid/@start", Kind::Int),
            prop("gid_target", "./gid/@target", Kind::Int),
            prop("gid_count", "./gid/@count", Kind::Int),
        ],
        children: &[],
        prop_order: &[
            "uid_start",
            "uid_target",
            "uid_count",
            "gid_start",
            "gid_target",
            "gid_count",
        ],
    }
};
