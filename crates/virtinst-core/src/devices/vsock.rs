//! `<vsock>` — upstream's `devices/vsock.py`. Grounded in the real
//! `XMLProperty` declarations. Clean, no gaps this time: three fields,
//! all straightforward forward paths.

use virtinst_xml::XmlBound;

/// ```xml
/// <vsock model="virtio">
///   <cid auto="no" address="3"/>
/// </vsock>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "vsock")]
pub struct DeviceVsock {
    #[xml(attribute = "model")]
    pub model: Option<String>,

    #[xml(path = "cid", attribute = "auto")]
    pub auto_cid: Option<bool>,

    #[xml(path = "cid", attribute = "address")]
    pub cid: Option<u32>,
}

// Editing an existing `<vsock>` in place, for the details page.

use virtinst_xml::{Document, Element};

/// `vmmVsockDetails.MIN_GUEST_CID`.
pub const MIN_GUEST_CID: u32 = 3;

/// `define_vsock`'s `auto_cid` (`is_yesno`).
pub fn set_auto_cid(doc: &mut Document, vsock: Element, auto: bool) {
    super::edit::set_attr(
        doc,
        vsock,
        "cid",
        "auto",
        Some(if auto { "yes" } else { "no" }),
    );
}

/// `define_vsock`'s `cid`.
pub fn set_cid(doc: &mut Document, vsock: Element, cid: u32) {
    super::edit::set_attr(doc, vsock, "cid", "address", Some(&cid.to_string()));
}

#[cfg(test)]
mod edit_tests {
    use super::*;

    #[test]
    fn manual_cid() {
        let xml = "<vsock model='virtio'><cid auto='yes'/></vsock>";
        let mut doc = virtinst_xml::parse_libvirt_xml(xml).unwrap();
        let vsock = doc.root_element().unwrap();
        set_auto_cid(&mut doc, vsock, false);
        set_cid(&mut doc, vsock, 42);
        let out = doc.write_str().unwrap();
        assert!(
            out.contains("auto=\"no\"") && out.contains("address=\"42\""),
            "{out}"
        );
    }
}
