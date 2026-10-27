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

// `DeviceInterface` as an `XMLBuilder` class, for the CLI parsers.

use crate::xmlbuilder::{child, prop, Kind, XmlClass, XmlObject, DEVICE_ADDRESS};

/// `_Backend`.
pub static INTERFACE_BACKEND: XmlClass = XmlClass {
    xml_name: "backend",
    props: &[
        prop("type", "./@type", Kind::Str),
        prop("logFile", "./@logFile", Kind::AbsPath),
    ],
    children: &[],
    prop_order: &[],
};

/// `_VirtualPort`.
pub static INTERFACE_VIRTUALPORT: XmlClass = XmlClass {
    xml_name: "virtualport",
    props: &[
        prop("type", "./@type", Kind::Str),
        prop("managerid", "./parameters/@managerid", Kind::Int),
        prop("typeid", "./parameters/@typeid", Kind::Int),
        prop("typeidversion", "./parameters/@typeidversion", Kind::Int),
        prop("instanceid", "./parameters/@instanceid", Kind::Str),
        prop("profileid", "./parameters/@profileid", Kind::Str),
        prop("interfaceid", "./parameters/@interfaceid", Kind::Str),
    ],
    children: &[],
    prop_order: &[],
};

/// `_PortForwardRange`.
pub static PORT_FORWARD_RANGE: XmlClass = XmlClass {
    xml_name: "range",
    props: &[
        prop("start", "./@start", Kind::Int),
        prop("end", "./@end", Kind::Int),
        prop("to", "./@to", Kind::Int),
        prop("exclude", "./@exclude", Kind::YesNo),
    ],
    children: &[],
    prop_order: &[],
};

/// `_PortForward`.
pub static PORT_FORWARD: XmlClass = XmlClass {
    xml_name: "portForward",
    props: &[
        prop("proto", "./@proto", Kind::Str),
        prop("address", "./@address", Kind::Str),
        prop("dev", "./@dev", Kind::Str),
    ],
    children: &[child("range", &PORT_FORWARD_RANGE, ".", false)],
    prop_order: &[],
};

/// `DeviceInterface`.
pub static DEVICE_INTERFACE: XmlClass = XmlClass {
    xml_name: "interface",
    props: &[
        prop("bridge", "./source/@bridge", Kind::Str),
        prop("network", "./source/@network", Kind::Str),
        prop("source_dev", "./source/@dev", Kind::Str),
        prop("type", "./@type", Kind::Str),
        prop("trustGuestRxFilters", "./@trustGuestRxFilters", Kind::YesNo),
        prop("macaddr", "./mac/@address", Kind::Str),
        prop("source_type", "./source/@type", Kind::Str),
        prop("source_path", "./source/@path", Kind::Str),
        prop("source_mode", "./source/@mode", Kind::Str),
        prop("managed", "./@managed", Kind::YesNo),
        prop("portgroup", "./source/@portgroup", Kind::Str),
        prop("model", "./model/@type", Kind::Str),
        prop("target_dev", "./target/@dev", Kind::Str),
        prop("filterref", "./filterref/@filter", Kind::Str),
        prop("link_state", "./link/@state", Kind::Str),
        prop("driver_name", "./driver/@name", Kind::Str),
        prop("driver_queues", "./driver/@queues", Kind::Int),
        prop("rom_bar", "./rom/@bar", Kind::OnOff),
        prop("rom_file", "./rom/@file", Kind::Str),
        prop("mtu_size", "./mtu/@size", Kind::Int),
    ],
    children: crate::device_children!(
        child("backend", &INTERFACE_BACKEND, ".", true),
        child("virtualport", &INTERFACE_VIRTUALPORT, ".", true),
        child("source_address", &DEVICE_ADDRESS, "./source", true),
        child("portForward", &PORT_FORWARD, ".", false),
    ),
    prop_order: crate::device_prop_order!(
        "backend",
        "bridge",
        "network",
        "source_dev",
        "source_type",
        "source_path",
        "source_mode",
        "portgroup",
        "macaddr",
        "target_dev",
        "model",
        "virtualport",
        "filterref",
        "rom_bar",
        "rom_file",
        "mtu_size",
        "portForward",
    ),
};

/// The property `DeviceInterface.source` maps to for `type`.
pub fn source_propname(iface_type: Option<&str>) -> Option<&'static str> {
    match iface_type {
        Some("network") => Some("network"),
        Some("bridge") => Some("bridge"),
        Some("direct" | "vdpa") => Some("source_dev"),
        _ => None,
    }
}

/// `DeviceInterface._set_source`.
pub fn set_source_prop(dev: &mut XmlObject, newsource: Option<String>) -> Result<(), String> {
    dev.set("bridge", None)?;
    dev.set("network", None)?;
    dev.set("source_dev", None)?;
    let typ = dev.get_str("type");
    match source_propname(typ.as_deref()) {
        Some(p) => dev.set(p, newsource.map(Into::into)),
        None => Ok(()),
    }
}

/// Upstream's `DeviceInterface._set_default_type`.
fn set_default_type(dev: &mut XmlObject) -> Result<(), String> {
    if dev.get_str("type").is_some_and(|t| !t.is_empty()) {
        return Ok(());
    }
    if dev.child("backend").get_str("type").as_deref() == Some("passt") {
        return dev.set("type", Some("user".into()));
    }
    dev.set("type", Some("bridge".into()))
}

/// What `DeviceInterface.set_defaults` needs from the connection and
/// the guest.
pub struct InterfaceDefaults<'a> {
    /// `generate_mac(conn)`.
    pub generate_mac: &'a dyn Fn() -> Option<String>,
    /// `_default_bridge(conn)`.
    pub default_bridge: &'a dyn Fn() -> Option<String>,
    /// `default_model(guest)`.
    pub default_model: Option<&'static str>,
}

/// `DeviceInterface.set_defaults`.
pub fn set_defaults(dev: &mut XmlObject, d: &InterfaceDefaults) -> Result<(), String> {
    set_default_type(dev)?;
    if dev.get_str("macaddr").map_or(true, |m| m.is_empty()) {
        dev.set("macaddr", (d.generate_mac)().map(Into::into))?;
    }
    if dev.get_str("type").as_deref() == Some("bridge")
        && dev.get_str("bridge").map_or(true, |b| b.is_empty())
    {
        dev.set("bridge", (d.default_bridge)().map(Into::into))?;
    }
    if dev.get_str("model").map_or(true, |m| m.is_empty()) {
        dev.set("model", d.default_model.map(Into::into))?;
    }
    Ok(())
}

/// `_default_route`: the interface of `/proc/net/route`'s default route.
fn default_route() -> Option<String> {
    let routes = std::fs::read_to_string("/proc/net/route").ok()?;
    for line in routes.lines() {
        let info: Vec<&str> = line.split_whitespace().collect();
        if info.len() != 11 {
            log::debug!(
                "Unexpected field count={} when parsing /proc/net/route",
                info.len()
            );
            break;
        }
        if u64::from_str_radix(info[1], 16) == Ok(0) {
            return Some(info[0].to_string());
        }
    }
    None
}

/// `_host_default_bridge`.
pub fn host_default_bridge() -> Option<String> {
    let dev = default_route()?;
    // New style peth0 == phys dev, eth0 == bridge, eth0 == default route
    if std::path::Path::new(&format!("/sys/class/net/{dev}/bridge")).exists() {
        return Some(dev);
    }
    // Old style, peth0 == phys dev, eth0 == netloop, xenbr0 == bridge,
    // vif0.0 == netloop attached, eth0 == default route
    let defn = dev
        .chars()
        .last()
        .and_then(|c| c.to_digit(10))
        .map_or(-1, |d| d as i64);
    if defn >= 0
        && std::path::Path::new(&format!("/sys/class/net/peth{defn}/brport")).exists()
        && std::path::Path::new(&format!("/sys/class/net/xenbr{defn}/bridge")).exists()
    {
        // Upstream returns the unformatted "xenbr%d" here.
        return Some("xenbr%d".into());
    }
    None
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
