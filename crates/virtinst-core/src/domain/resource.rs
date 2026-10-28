//! `<resource>` — upstream's `domain/resource.py`. Two plain string
//! fields, one a text child (`./partition`), one a nested attribute
//! (`./fibrechannel/@appid`) — both already-exercised binding shapes,
//! no new macro capability needed.

use virtinst_xml::XmlBound;

/// ```xml
/// <resource>
///   <partition>/machine/production</partition>
///   <fibrechannel appid="example.com:app1"/>
/// </resource>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "resource")]
pub struct DomainResource {
    #[xml(path = "partition", text)]
    pub partition: Option<String>,

    #[xml(path = "fibrechannel", attribute = "appid")]
    pub fibrechannel_appid: Option<String>,
}

/// `DomainResource`, for the CLI parsers.
pub static DOMAIN_RESOURCE: crate::xmlbuilder::XmlClass = {
    use crate::xmlbuilder::{prop, Kind, XmlClass};
    XmlClass {
        xml_name: "resource",
        props: &[
            prop("partition", "./partition", Kind::Str),
            prop("fibrechannel_appid", "./fibrechannel/@appid", Kind::Str),
        ],
        children: &[],
        prop_order: &["partition", "fibrechannel_appid"],
    }
};
