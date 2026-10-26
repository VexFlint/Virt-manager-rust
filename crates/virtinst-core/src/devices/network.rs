//! `<interface>` — upstream's `devices/interface.py` (`DeviceInterface`).
//! Field subset chosen to match `DeviceDisk`'s scope: the common
//! `type='network'` shape, checked against upstream's actual
//! `XMLProperty` XPath declarations rather than assumed from memory —
//! `network = XMLProperty("./source/@network")`,
//! `macaddr = XMLProperty("./mac/@address")`, etc.

use virtinst_xml::XmlBound;

/// ```xml
/// <interface type="network">
///   <mac address="52:54:00:12:34:56"/>
///   <source network="default"/>
///   <model type="virtio"/>
///   <target dev="vnet0"/>
///   <link state="up"/>
/// </interface>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "interface")]
pub struct DeviceNetwork {
    #[xml(attribute = "type")]
    pub interface_type: Option<String>,

    #[xml(path = "mac", attribute = "address")]
    pub macaddr: Option<String>,

    /// For `type="network"`.
    #[xml(path = "source", attribute = "network")]
    pub source_network: Option<String>,

    /// For `type="bridge"`.
    #[xml(path = "source", attribute = "bridge")]
    pub source_bridge: Option<String>,

    #[xml(path = "model", attribute = "type")]
    pub model: Option<String>,

    #[xml(path = "target", attribute = "dev")]
    pub target_dev: Option<String>,

    #[xml(path = "link", attribute = "state")]
    pub link_state: Option<String>,
}

// Editing an existing `<interface>` in place, for the details page.

use super::edit::{attr, set_attr};
use virtinst_xml::{Document, Element};

/// `DeviceInterface.source`'s attribute for a type.
fn source_attr(ntype: &str) -> Option<&'static str> {
    match ntype {
        "network" => Some("network"),
        "bridge" => Some("bridge"),
        "direct" | "vdpa" => Some("dev"),
        _ => None,
    }
}

/// `(type, source, network)`: the interface type, `DeviceInterface.source`
/// for it, and `source@network`, which a running VM's XML keeps next to
/// `type='bridge'` for forward-mode bridge networks.
pub fn interface_source(
    doc: &Document,
    iface: Element,
) -> (String, Option<String>, Option<String>) {
    let ntype = iface.attribute(doc, "type").unwrap_or_default().to_string();
    let source = source_attr(&ntype).and_then(|a| attr(doc, iface, &["source"], a));
    let network = attr(doc, iface, &["source"], "network");
    (ntype, source, network)
}

/// `define_network`'s `ntype` branch: clear the old source, then set
/// the type, its source, `mode` and `portgroup`.
pub fn set_source(
    doc: &mut Document,
    iface: Element,
    ntype: &str,
    source: Option<&str>,
    mode: Option<&str>,
    portgroup: Option<&str>,
) {
    for a in ["bridge", "network", "dev"] {
        set_attr(doc, iface, "source", a, None);
    }
    iface.set_attribute(doc, "type", ntype);
    if let Some(a) = source_attr(ntype) {
        set_attr(doc, iface, "source", a, source);
    }
    set_attr(doc, iface, "source", "mode", mode);
    set_attr(doc, iface, "source", "portgroup", portgroup);
}

/// `define_network`'s `model`: a new model drops the PCI address.
pub fn set_model(doc: &mut Document, iface: Element, model: Option<&str>) {
    if attr(doc, iface, &["model"], "type").as_deref() != model {
        if let Some(addr) = iface.find(doc, "address") {
            let _ = addr.detach(doc);
        }
    }
    set_attr(doc, iface, "model", "type", model);
}

/// `link_state = "up" if linkstate else "down"`.
pub fn set_link_state(doc: &mut Document, iface: Element, up: bool) {
    set_attr(
        doc,
        iface,
        "link",
        "state",
        Some(if up { "up" } else { "down" }),
    );
}

/// `compare_device` for interfaces: the same MAC at the same index.
pub fn find_interface(
    doc: &Document,
    root: Element,
    mac: Option<&str>,
    idx: usize,
) -> Option<Element> {
    let devices = root.find(doc, "devices")?;
    let iface = *devices.find_all(doc, "interface").get(idx)?;
    (attr(doc, iface, &["mac"], "address").as_deref() == mac).then_some(iface)
}

/// `vmmAddHardware.interface_recommended_models`.
pub fn recommended_models(
    domain_type: &str,
    os_type: &str,
    arch: &str,
    machine: &str,
) -> Vec<&'static str> {
    if os_type != "hvm" {
        return vec![];
    }
    let mut ret = vec![];
    if matches!(domain_type, "kvm" | "qemu" | "vz" | "test") {
        ret.push("virtio");
    }
    if matches!(arch, "x86_64" | "i686") {
        if machine.contains("q35") {
            ret.push("e1000e");
        } else {
            ret.push("rtl8139");
            ret.push("e1000");
        }
    }
    if matches!(domain_type, "xen" | "test") {
        ret.push("netfront");
    }
    ret.sort();
    ret
}

#[cfg(test)]
mod edit_tests {
    use super::*;

    #[test]
    fn source_model_and_link_edits() {
        let xml = "<domain><devices><interface type='network'>\
            <mac address='52:54:00:12:34:56'/><source network='default' portgroup='a'/>\
            <model type='e1000'/><address type='pci' slot='0x03'/></interface></devices></domain>";
        let mut doc = virtinst_xml::parse_libvirt_xml(xml).unwrap();
        let root = doc.root_element().unwrap();
        assert!(find_interface(&doc, root, Some("52:54:00:00:00:00"), 0).is_none());
        let nic = find_interface(&doc, root, Some("52:54:00:12:34:56"), 0).unwrap();
        assert_eq!(
            interface_source(&doc, nic),
            (
                "network".into(),
                Some("default".into()),
                Some("default".into())
            )
        );

        set_source(&mut doc, nic, "direct", Some("eth0"), Some("bridge"), None);
        assert_eq!(nic.attribute(&doc, "type"), Some("direct"));
        assert_eq!(attr(&doc, nic, &["source"], "dev").as_deref(), Some("eth0"));
        assert_eq!(
            attr(&doc, nic, &["source"], "mode").as_deref(),
            Some("bridge")
        );
        assert_eq!(attr(&doc, nic, &["source"], "network"), None);
        assert_eq!(attr(&doc, nic, &["source"], "portgroup"), None);

        set_source(&mut doc, nic, "user", None, None, None);
        assert!(nic.find(&doc, "source").is_none());

        set_model(&mut doc, nic, Some("e1000"));
        assert!(nic.find(&doc, "address").is_some());
        set_model(&mut doc, nic, Some("virtio"));
        assert!(nic.find(&doc, "address").is_none());
        set_model(&mut doc, nic, None);
        assert!(nic.find(&doc, "model").is_none());

        set_link_state(&mut doc, nic, false);
        assert_eq!(attr(&doc, nic, &["link"], "state").as_deref(), Some("down"));
    }

    #[test]
    fn recommended_models_follow_machine() {
        assert_eq!(
            recommended_models("kvm", "hvm", "x86_64", "pc-q35-9.0"),
            ["e1000e", "virtio"]
        );
        assert_eq!(
            recommended_models("qemu", "hvm", "x86_64", "pc-i440fx-9.0"),
            ["e1000", "rtl8139", "virtio"]
        );
        assert!(recommended_models("kvm", "xen", "x86_64", "").is_empty());
        assert_eq!(
            recommended_models("kvm", "hvm", "aarch64", "virt"),
            ["virtio"]
        );
    }
}
