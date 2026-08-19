//! Wraps `virt::domain::Domain` — upstream's `vmmDomain.attach_device()`/
//! `.add_device()` (`virtManager/object/domain.py`), the concrete
//! live+persistent shape ticket 12 traced and ticket 10 deferred: two
//! explicit calls, not one `AFFECT_LIVE|AFFECT_CONFIG` flag.
//!
//! - **`attach_device`**: live hotplug only. A no-op (not an error) if
//!   the domain isn't currently running — matches upstream's
//!   `if not self.is_active(): return` exactly. Plain
//!   `virDomainAttachDevice`, no flags, same as upstream's
//!   `self._backend.attachDevice(devxml)`.
//! - **`add_device`**: always persists. Fetches the domain's *inactive*
//!   XML (`VIR_DOMAIN_XML_INACTIVE`), appends the device to `<devices>`
//!   via [`virtinst_xml::list_add`] against the parsed DOM, then
//!   `virDomainDefineXML`s the result. Matches upstream's
//!   `Guest.add_device()` (`self.devices.add_child(dev)`) +
//!   `_redefine_xmlobj()`'s `_define()` call exactly — this is the
//!   DOM-level append ticket 06 built for precisely this, not a
//!   reparse-every-field round trip.
//!
//! **Not this module's job**: `addhardware.py`'s actual `_add_device`
//! sequencing — attach-then-add-persistent ordering, the confirm-and-
//! degrade-to-persistent-only dialog on hotplug failure, controller-
//! first ordering for a device's companion controller
//! (`NewDevice::Disk`'s `companion_controller`, `devices::new_device`).
//! That's GUI-shell flow control over these two primitives, and lives
//! in `virt-manager` (not yet built), not here.

use crate::connection::Connection;
use virt::domain::Domain as LibvirtDomain;
use virt::error::Error as LibvirtError;
use virt::sys;
use virtinst_xml::{list_add, parse_libvirt_xml, to_xml_string, EditXMLError, XmlBound};

/// Either half of this module's work can fail for a different reason —
/// a real libvirt call, or parsing/serializing the XML in between. One
/// small enum rather than reaching for an error-handling crate for two
/// variants.
#[derive(Debug)]
pub enum DomainError {
    Libvirt(LibvirtError),
    Xml(EditXMLError),
}

impl std::fmt::Display for DomainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DomainError::Libvirt(e) => write!(f, "{e}"),
            DomainError::Xml(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DomainError {}

impl From<LibvirtError> for DomainError {
    fn from(e: LibvirtError) -> Self {
        DomainError::Libvirt(e)
    }
}

impl From<EditXMLError> for DomainError {
    fn from(e: EditXMLError) -> Self {
        DomainError::Xml(e)
    }
}

/// A libvirt domain (VM), looked up through a [`Connection`].
pub struct Domain {
    inner: LibvirtDomain,
}

impl Domain {
    pub fn lookup_by_name(conn: &Connection, name: &str) -> Result<Domain, LibvirtError> {
        let inner = LibvirtDomain::lookup_by_name(conn.as_connect(), name)?;
        Ok(Domain { inner })
    }

    pub fn is_active(&self) -> Result<bool, LibvirtError> {
        self.inner.is_active()
    }

    /// Live hotplug — upstream's `attach_device()`. A no-op, not an
    /// error, when the domain isn't running.
    pub fn attach_device<T: XmlBound>(&self, device: &T) -> Result<(), DomainError> {
        if !self.is_active()? {
            return Ok(());
        }
        let xml = to_xml_string(device)?;
        self.inner.attach_device(&xml)?;
        Ok(())
    }

    /// Persistent config update — upstream's `add_device()`. Always
    /// runs, live or not: fetches the inactive XML, appends the
    /// device, redefines.
    pub fn add_device<T: XmlBound>(
        &self,
        conn: &Connection,
        device: &T,
    ) -> Result<(), DomainError> {
        let inactive_xml = self.inner.get_xml_desc(sys::VIR_DOMAIN_XML_INACTIVE)?;
        let mut doc = parse_libvirt_xml(&inactive_xml)?;
        let root = doc
            .root_element()
            .expect("libvirt's own XMLDesc output always has a root element");

        list_add(&mut doc, root, &["devices"], device);

        let new_xml = doc.write_str()?;
        LibvirtDomain::define_xml(conn.as_connect(), &new_xml)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::{DeviceDisk, DeviceRng};

    /// Real libvirt calls against the `test` domain libvirt's builtin
    /// `test:///default` driver always defines and starts running -
    /// same no-daemon guarantee `connection.rs`'s own tests rely on.
    #[test]
    fn attach_device_hotplugs_a_disk_onto_the_running_test_domain() {
        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        let domain = Domain::lookup_by_name(&conn, "test").expect("the default test domain");
        assert!(
            domain.is_active().expect("query succeeds"),
            "test domain starts running"
        );

        let disk = DeviceDisk {
            disk_type: Some("file".into()),
            device: Some("disk".into()),
            source_file: Some("/guest/attach-device-test.img".into()),
            target_dev: Some("vdz".into()),
            target_bus: Some("virtio".into()),
            ..Default::default()
        };
        domain
            .attach_device(&disk)
            .expect("the test driver accepts disk hotplug");
    }

    #[test]
    fn add_device_persists_to_the_inactive_config() {
        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        let domain = Domain::lookup_by_name(&conn, "test").expect("the default test domain");

        let rng = DeviceRng {
            model: Some("virtio".into()),
            backend_model: Some("random".into()),
            backend_device: Some("/dev/urandom".into()),
            ..Default::default()
        };
        domain
            .add_device(&conn, &rng)
            .expect("defining the modified inactive XML succeeds");

        // Re-fetch and confirm it actually landed in the persistent
        // config, not just that the call returned Ok.
        let inactive_xml = domain
            .inner
            .get_xml_desc(sys::VIR_DOMAIN_XML_INACTIVE)
            .expect("fetch inactive XML");
        // libvirt's own XMLDesc serializer single-quotes attributes
        // (unlike this crate's writer) - assert on tag/content, not
        // quote style.
        assert!(inactive_xml.contains("<rng"));
        assert!(inactive_xml.contains("virtio"));
        assert!(inactive_xml.contains("/dev/urandom"));
    }
}
