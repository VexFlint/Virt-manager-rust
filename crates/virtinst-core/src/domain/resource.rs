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
