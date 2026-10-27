//! `<hostdev>` — upstream's `devices/hostdev.py`. A union-of-shapes
//! device (USB vendor/product-ID-based, PCI domain/bus/slot/function-
//! based, SCSI adapter-based, …) — grounded in the real `XMLProperty`
//! declarations, first real exercise of a genuine multi-segment `path`
//! (`vendor = XMLProperty("./source/vendor/@id")` → `path = "source/vendor"`)
//! and of `text` under a multi-segment path
//! (`net_interface = XMLProperty("./source/interface")`, no `@attr` —
//! it's the element's own text content).
//!
//! Modeled: mode/type/managed (self), USB vendor/product IDs, PCI
//! address fields, and the network-interface text case. Not modeled
//! yet: SCSI adapter fields, `rom_bar` (`is_onoff`, same flagged gap as
//! other devices), `uuid`, `misc_char`/`storage_block` (same
//! text-under-path shape as `net_interface`, omitted for scope not for
//! a design reason).

use virtinst_xml::XmlBound;

/// USB, by vendor/product ID:
/// ```xml
/// <hostdev mode="subsystem" type="usb" managed="yes">
///   <source>
///     <vendor id="0x1234"/>
///     <product id="0x5678"/>
///   </source>
/// </hostdev>
/// ```
/// PCI, by address:
/// ```xml
/// <hostdev mode="subsystem" type="pci" managed="yes">
///   <source>
///     <address domain="0x0000" bus="0x00" slot="0x02" function="0x0"/>
///   </source>
/// </hostdev>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "hostdev")]
pub struct DeviceHostdev {
    #[xml(attribute = "mode")]
    pub mode: Option<String>,

    #[xml(attribute = "type")]
    pub hostdev_type: Option<String>,

    #[xml(attribute = "managed")]
    pub managed: Option<bool>,

    #[xml(path = "source/vendor", attribute = "id")]
    pub vendor: Option<String>,

    #[xml(path = "source/product", attribute = "id")]
    pub product: Option<String>,

    #[xml(path = "source/address", attribute = "domain")]
    pub pci_domain: Option<String>,

    #[xml(path = "source/address", attribute = "bus")]
    pub pci_bus: Option<String>,

    #[xml(path = "source/address", attribute = "slot")]
    pub pci_slot: Option<String>,

    #[xml(path = "source/address", attribute = "function")]
    pub pci_function: Option<String>,

    #[xml(path = "source/interface", text)]
    pub net_interface: Option<String>,
}

impl DeviceHostdev {
    /// Ticket 21 slice (e): upstream's `DeviceHostdev.set_defaults`
    /// (`hostdev.py:154`) — `managed`/`mode`, two unconditional/
    /// URI-scheme-only lines, zero `OsInfo` dependency at all (the
    /// easiest item on ticket 21's whole list, by the scoping pass's
    /// own account).
    ///
    /// **No real caller yet**: this crate's `virt-install`/`virt-xml`
    /// have no `--hostdev` flag, so nothing builds a `DeviceHostdev` to
    /// call this on. Built ahead of that consumer anyway — the same
    /// precedent `domcaps.rs`'s `supports_graphics_spice`/`default_
    /// graphics_type` set under ticket 21 slice (c) (real logic, real
    /// tests, landed before their `virt-install.rs` wiring in the same
    /// slice) — rather than left unbuilt and unverifiable until a CLI
    /// flag happens to show up.
    ///
    /// `uri: &crate::connection::Uri`, not a bare `is_xen: bool` — same
    /// reasoning as `domcaps.rs`'s query methods: a single same-typed
    /// `bool` here is a smaller version of the "wrong argument, no test
    /// catches it" shape that review flagged there, and `Uri` costs
    /// nothing to construct (`Uri::parse` is a pure, connection-free
    /// parser, no libvirt call).
    ///
    /// `self.mode.is_none()`, not upstream's `if not self.mode` (which
    /// also treats `Some("")` as falsy) — every other `Option` gate in
    /// this crate reads absence the same way, and an empty-string
    /// `mode=""` attribute doesn't arise from real libvirt XML, so this
    /// is a deliberate match to this crate's existing convention, not
    /// an unnoticed narrowing.
    pub fn set_defaults(&mut self, uri: &crate::connection::Uri) {
        if self.managed.is_none() {
            self.managed = Some(!uri.is_xen());
        }
        if self.mode.is_none() {
            self.mode = Some("subsystem".to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Uri;

    #[test]
    fn defaults_managed_to_yes_for_a_non_xen_connection() {
        let mut dev = DeviceHostdev::default();
        dev.set_defaults(&Uri::parse("qemu:///system"));
        assert_eq!(dev.managed, Some(true));
    }

    #[test]
    fn defaults_managed_to_no_for_a_real_xen_connection() {
        let mut dev = DeviceHostdev::default();
        dev.set_defaults(&Uri::parse("xen+ssh://root@example.com/"));
        assert_eq!(dev.managed, Some(false));
    }

    #[test]
    fn does_not_override_an_explicit_managed_value() {
        let mut dev = DeviceHostdev {
            managed: Some(false),
            ..Default::default()
        };
        // A non-xen URI would otherwise default `managed` to `true` —
        // proves the explicit `false` survives, not just that some
        // value ends up set.
        dev.set_defaults(&Uri::parse("qemu:///system"));
        assert_eq!(dev.managed, Some(false));
    }

    #[test]
    fn defaults_mode_to_subsystem() {
        let mut dev = DeviceHostdev::default();
        dev.set_defaults(&Uri::parse("qemu:///system"));
        assert_eq!(dev.mode.as_deref(), Some("subsystem"));
    }

    #[test]
    fn does_not_override_an_explicit_mode() {
        let mut dev = DeviceHostdev {
            mode: Some("capabilities".to_string()),
            ..Default::default()
        };
        dev.set_defaults(&Uri::parse("qemu:///system"));
        assert_eq!(dev.mode.as_deref(), Some("capabilities"));
    }
}

// Editing an existing `<hostdev>` in place, for the details page.

use virtinst_xml::{Document, Element};

/// `DeviceHostdev.STARTUP_POLICIES`.
pub const STARTUP_POLICIES: [&str; 3] = ["mandatory", "optional", "requisite"];

/// `define_hostdev`'s `rom_bar` (`is_onoff`).
pub fn set_rom_bar(doc: &mut Document, hostdev: Element, rom_bar: bool) {
    super::edit::set_attr(
        doc,
        hostdev,
        "rom",
        "bar",
        Some(if rom_bar { "on" } else { "off" }),
    );
}

/// `define_hostdev`'s `startup_policy`.
pub fn set_startup_policy(doc: &mut Document, hostdev: Element, policy: Option<&str>) {
    super::edit::set_attr(doc, hostdev, "source", "startupPolicy", policy);
}

// `DeviceHostdev` as an `XMLBuilder` class, for the CLI parsers.

use crate::nodedev::NodeDevice;
use crate::xmlbuilder::{prop, Kind, Value, XmlClass, XmlObject};

/// `DeviceHostdev`.
pub static DEVICE_HOSTDEV: XmlClass = XmlClass {
    xml_name: "hostdev",
    props: &[
        prop("mode", "./@mode", Kind::Str),
        prop("type", "./@type", Kind::Str),
        prop("managed", "./@managed", Kind::YesNo),
        prop("model", "./@model", Kind::Str),
        prop("display", "./@display", Kind::Str),
        prop("ramfb", "./@ramfb", Kind::Str),
        prop("vendor", "./source/vendor/@id", Kind::Str),
        prop("product", "./source/product/@id", Kind::Str),
        prop("device", "./source/address/@device", Kind::Str),
        prop("bus", "./source/address/@bus", Kind::Str),
        prop("startup_policy", "./source/@startupPolicy", Kind::Str),
        prop("domain", "./source/address/@domain", Kind::Str),
        prop("function", "./source/address/@function", Kind::Str),
        prop("slot", "./source/address/@slot", Kind::Str),
        prop("driver_name", "./driver/@name", Kind::Str),
        prop("rom_bar", "./rom/@bar", Kind::OnOff),
        prop("scsi_adapter", "./source/adapter/@name", Kind::Str),
        prop("scsi_bus", "./source/address/@bus", Kind::Int),
        prop("scsi_target", "./source/address/@target", Kind::Int),
        prop("scsi_unit", "./source/address/@unit", Kind::Int),
        prop("net_interface", "./source/interface", Kind::Str),
        prop("misc_char", "./source/char", Kind::Str),
        prop("storage_block", "./source/block", Kind::Str),
        prop("uuid", "./source/address/@uuid", Kind::Str),
    ],
    children: crate::device_children!(),
    prop_order: crate::device_prop_order!(
        "mode", "type", "managed", "vendor", "product", "domain", "bus", "slot", "function",
        "model", "display", "ramfb",
    ),
};

/// `DeviceHostdev.set_from_nodedev`. `all` is
/// `conn.fetch_all_nodedevs()`.
pub fn set_from_nodedev(
    dev: &mut XmlObject,
    nodedev: Option<&NodeDevice>,
    all: &[NodeDevice],
) -> Result<(), String> {
    let s = |v: &Option<String>| v.as_deref().map(Value::from);
    let Some(nodedev) = nodedev else {
        return Err("'NoneType' object has no attribute 'device_type'".into());
    };
    match nodedev.device_type.as_deref() {
        Some("pci") => {
            dev.set("type", Some("pci".into()))?;
            dev.set("domain", s(&nodedev.domain))?;
            dev.set("bus", s(&nodedev.bus))?;
            dev.set("slot", s(&nodedev.slot))?;
            dev.set("function", s(&nodedev.function))?;
        }
        Some("usb_device") => {
            dev.set("type", Some("usb".into()))?;
            dev.set("vendor", s(&nodedev.vendor_id))?;
            dev.set("product", s(&nodedev.product_id))?;
            let count = all
                .iter()
                .filter(|d| {
                    d.device_type.as_deref() == Some("usb_device")
                        && d.vendor_id == nodedev.vendor_id
                        && d.product_id == nodedev.product_id
                })
                .count();
            if count > 1 {
                dev.set("bus", s(&nodedev.bus))?;
                dev.set("device", s(&nodedev.device))?;
            }
        }
        Some("net") => {
            let founddev = all
                .iter()
                .find(|d| Some(&d.name) == nodedev.parent.as_ref());
            return set_from_nodedev(dev, founddev, all);
        }
        Some("scsi") => {
            dev.set("type", Some("scsi".into()))?;
            let host = nodedev.host.as_deref().unwrap_or("None");
            dev.set("scsi_adapter", Some(format!("scsi_host{host}").into()))?;
            dev.set("scsi_bus", s(&nodedev.bus))?;
            dev.set("scsi_target", s(&nodedev.target))?;
            dev.set("scsi_unit", s(&nodedev.lun))?;
            dev.set("managed", Some(Value::Bool(false)))?;
        }
        Some("mdev") => {
            dev.set("type", Some("mdev".into()))?;
            let type_id = nodedev.type_id.as_deref().unwrap_or_default();
            if type_id == "vfio_ccw-io" {
                dev.set("model", Some("vfio-ccw".into()))?;
                dev.set("managed", Some("no".into()))?;
            } else if type_id == "vfio_ap-passthrough" {
                dev.set("model", Some("vfio-ap".into()))?;
                dev.set("managed", Some("no".into()))?;
            } else if super::build::is_vfio_pci_mdev(type_id) {
                dev.set("model", Some("vfio-pci".into()))?;
                dev.set("managed", Some("yes".into()))?;
                dev.set("display", Some("off".into()))?;
                dev.set("ramfb", Some("off".into()))?;
            } else {
                return Err(format!(
                    "Don't know how to generate nodedev for mdev type id '{type_id}'"
                ));
            }
            dev.set("uuid", Some(nodedev.get_mdev_uuid().into()))?;
        }
        other => {
            return Err(format!(
                "Unsupported node device type '{}'",
                other.unwrap_or("None")
            ))
        }
    }
    Ok(())
}

/// `DeviceHostdev.set_defaults`.
pub fn set_defaults(dev: &mut XmlObject, is_xen: bool) -> Result<(), String> {
    if dev.get("managed").is_none() {
        dev.set("managed", Some(if is_xen { "no" } else { "yes" }.into()))?;
    }
    if dev.get_str("mode").map_or(true, |m| m.is_empty()) {
        dev.set("mode", Some("subsystem".into()))?;
    }
    Ok(())
}

#[cfg(test)]
mod edit_tests {
    use super::*;

    #[test]
    fn rom_bar_and_policy() {
        let xml = "<hostdev mode='subsystem' type='usb'><source>\
            <vendor id='0x1234'/></source></hostdev>";
        let mut doc = virtinst_xml::parse_libvirt_xml(xml).unwrap();
        let dev = doc.root_element().unwrap();
        set_rom_bar(&mut doc, dev, false);
        set_startup_policy(&mut doc, dev, Some("optional"));
        let out = doc.write_str().unwrap();
        assert!(out.contains("<rom bar=\"off\""), "{out}");
        assert!(out.contains("startupPolicy=\"optional\""), "{out}");
        set_startup_policy(&mut doc, dev, None);
        let out = doc.write_str().unwrap();
        assert!(
            !out.contains("startupPolicy") && out.contains("<vendor"),
            "{out}"
        );
    }
}
