//! `<metadata>` — upstream's `domain/metadata.py`. The file this crate
//! missed on the first full sweep of `domain/*.py` (not in the earlier
//! batch commit's list) — found while surveying `guest.py`'s
//! `XMLChildProperty` calls for wiring `Guest` together.
//!
//! Same stripped-local-name caveat as `xmlnsqemu.rs`: `libosinfo:os`'s
//! path segment is written `"os"` (not `"libosinfo:os"`), and
//! `libosinfo:libosinfo`'s tag is `"libosinfo"` — `edit_xml::find`
//! matches on the name *after* the namespace prefix, confirmed
//! empirically there, not re-derived here.

use virtinst_xml::XmlBound;

/// `<metadata><libosinfo:libosinfo><libosinfo:os id="..."/></libosinfo:libosinfo></metadata>`
/// — upstream's `_XMLNSLibosinfo`.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "libosinfo")]
pub struct MetadataLibosinfo {
    #[xml(path = "os", attribute = "id")]
    pub os_id: Option<String>,
}

/// ```xml
/// <metadata>
///   <libosinfo:libosinfo xmlns:libosinfo="http://libosinfo.org/xmlns/libvirt/domain/1.0">
///     <libosinfo:os id="http://fedoraproject.org/fedora/40"/>
///   </libosinfo:libosinfo>
/// </metadata>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "metadata")]
pub struct DomainMetadata {
    #[xml(nested)]
    pub libosinfo: MetadataLibosinfo,
}

/// `_XMLNSLibosinfo`, for the CLI parsers.
pub static METADATA_LIBOSINFO: crate::xmlbuilder::XmlClass = {
    use crate::xmlbuilder::{prop, Kind, XmlClass};
    XmlClass {
        xml_name: "libosinfo:libosinfo",
        props: &[prop("os_id", "./libosinfo:os/@id", Kind::Str)],
        children: &[],
        prop_order: &[],
    }
};

/// `DomainMetadata`, for the CLI parsers.
pub static DOMAIN_METADATA: crate::xmlbuilder::XmlClass = {
    use crate::xmlbuilder::{child, XmlClass};
    XmlClass {
        xml_name: "metadata",
        props: &[],
        children: &[child("libosinfo", &METADATA_LIBOSINFO, ".", true)],
        prop_order: &[],
    }
};
