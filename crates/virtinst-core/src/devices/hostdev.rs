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
