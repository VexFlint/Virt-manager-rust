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
//! - **`edit_inactive_xml`**: the general fetch/parse/mutate/define
//!   shape `add_device` above is one instance of — pulled out once
//!   `virt-xml --edit` (ticket 16) needed the same three steps
//!   (fetch inactive XML, mutate the parsed DOM in place, define) but
//!   with a caller-supplied mutation instead of always "append this
//!   device". `add_device` is now built on top of it.
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
use virtinst_xml::{
    list_add, parse_libvirt_xml, to_xml_string, Document, EditXMLError, Element, XmlBound,
};

/// Either half of this module's work can fail for a different reason —
/// a real libvirt call, or parsing/serializing the XML in between — plus
/// a third for `edit_inactive_xml`'s caller-supplied mutation rejecting
/// its own input (e.g. `--edit 5` when there are only 2 watchdogs). One
/// small enum rather than reaching for an error-handling crate for three
/// variants.
#[derive(Debug)]
pub enum DomainError {
    Libvirt(LibvirtError),
    Xml(EditXMLError),
    Edit(String),
}

impl std::fmt::Display for DomainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DomainError::Libvirt(e) => write!(f, "{e}"),
            DomainError::Xml(e) => write!(f, "{e}"),
            DomainError::Edit(msg) => write!(f, "{msg}"),
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

    /// Defines a **brand-new** domain from XML — plain
    /// `virConnectDefineXML`, no existing domain involved. An
    /// associated function taking `&Connection` rather than a
    /// `Domain` method, same shape as `lookup_by_name`: there's
    /// nothing to call this *on* yet, it's what makes the `Domain`.
    /// Backs `virt-clone` (ticket 16's `Cloner`, `crate::cloner`) —
    /// upstream's clone path ultimately reaches the same
    /// `conn.defineXML(xml)` once there's no OS-install step in the
    /// way (`--preserve-data` has none).
    pub fn define_new(conn: &Connection, xml: &str) -> Result<Domain, LibvirtError> {
        let inner = LibvirtDomain::define_xml(conn.as_connect(), xml)?;
        Ok(Domain { inner })
    }

    /// Starts a **brand-new, transient** domain from XML —
    /// `virDomainCreateXML`, no flags. Same "makes the `Domain`,
    /// nothing to call it on yet" shape as `define_new`. Transient
    /// means exactly what upstream's own `conn.createXML` doc says:
    /// gone if destroyed, gone if the host restarts — `define_new`
    /// alongside it is what makes a domain persist. Backs
    /// `virt-install`'s default (non-`--transient`, `doboot`) case,
    /// which upstream's `_create_guest` does as *two* separate calls
    /// (`createXML` then `defineXML`), not one — same shape as
    /// `Domain::attach_device` + `Domain::add_device`.
    pub fn create_new(conn: &Connection, xml: &str) -> Result<Domain, LibvirtError> {
        let inner = LibvirtDomain::create_xml(conn.as_connect(), xml, 0)?;
        Ok(Domain { inner })
    }

    /// The domain's persistent name — used to report success after a
    /// clone/edit without the caller needing to hang onto the name
    /// string it passed in separately.
    pub fn name(&self) -> Result<String, LibvirtError> {
        self.inner.get_name()
    }

    /// Removes the domain's persistent config — `virDomainUndefine`.
    /// Nothing in `virt-clone` calls this yet (it never undefines what
    /// it creates); exists for tests to clean up a domain they
    /// defined, and generally useful enough not to gate behind
    /// `#[cfg(test)]`.
    pub fn undefine(&self) -> Result<(), LibvirtError> {
        self.inner.undefine()
    }

    pub fn is_active(&self) -> Result<bool, LibvirtError> {
        self.inner.is_active()
    }

    /// Starts a defined-but-inactive domain — `virDomainCreate`.
    pub fn start(&self) -> Result<(), LibvirtError> {
        self.inner.create().map(|_| ())
    }

    /// Forcibly stops a running domain — `virDomainDestroy`. Not
    /// `virt-clone`'s job (upstream's `Cloner` itself requires the
    /// source domain already be shut off, never stops one); exists so
    /// tests can satisfy that same requirement against `test:///`'s
    /// always-running default domain without a second, differently-
    /// shaped fixture domain.
    pub fn destroy(&self) -> Result<(), LibvirtError> {
        self.inner.destroy()
    }

    /// The domain's persistent (`VIR_DOMAIN_XML_INACTIVE`) config as
    /// raw XML — what `edit_inactive_xml` fetches internally, exposed
    /// for callers that just need to read it (`--print-xml`, tests
    /// confirming a define actually landed).
    pub fn inactive_xml(&self) -> Result<String, LibvirtError> {
        self.inner.get_xml_desc(sys::VIR_DOMAIN_XML_INACTIVE)
    }

    /// The domain's currently-running config as raw XML —
    /// `virDomainGetXMLDesc` with no flags, which for an active domain
    /// reflects the live/in-memory state rather than the persistent one
    /// (`inactive_xml`'s job). Exists for tests that need to confirm
    /// `Domain::create_new` was actually handed the XML it looks like it
    /// was — `iso.rs`'s two-phase test is the first consumer, checking
    /// the transient instance really booted from cdrom rather than
    /// `defineXML`'s hd-booting config.
    pub fn live_xml(&self) -> Result<String, LibvirtError> {
        self.inner.get_xml_desc(0)
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
    /// device, redefines. Now a thin call onto [`Self::edit_inactive_xml`].
    pub fn add_device<T: XmlBound>(
        &self,
        conn: &Connection,
        device: &T,
    ) -> Result<(), DomainError> {
        self.edit_inactive_xml(conn, |doc, root| {
            list_add(doc, root, &["devices"], device);
            Ok(())
        })
    }

    /// Fetch the domain's *inactive* XML, parse it into a DOM, hand the
    /// document and its root `<domain>` element to `edit` to mutate in
    /// place, then `virDomainDefineXML` the result. The general shape
    /// `add_device` is one instance of, and `virt-xml --edit` (ticket
    /// 16) needs directly: locate an existing element (by index/`all`/
    /// selector), mutate only the attributes a `--watchdog`-style
    /// optstring actually named, leave everything else in the document
    /// untouched.
    ///
    /// If `edit` returns `Err`, nothing is defined — the caller's own
    /// rejection of its input (a bad `--edit` index, an unmatched
    /// selector) must not still write a half-applied document back to
    /// libvirt.
    pub fn edit_inactive_xml(
        &self,
        conn: &Connection,
        edit: impl FnOnce(&mut Document, Element) -> Result<(), String>,
    ) -> Result<(), DomainError> {
        let inactive_xml = self.inner.get_xml_desc(sys::VIR_DOMAIN_XML_INACTIVE)?;
        let mut doc = parse_libvirt_xml(&inactive_xml)?;
        let root = doc
            .root_element()
            .expect("libvirt's own XMLDesc output always has a root element");

        edit(&mut doc, root).map_err(DomainError::Edit)?;

        let new_xml = doc.write_str()?;
        LibvirtDomain::define_xml(conn.as_connect(), &new_xml)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::{DeviceHostdev, DeviceRng};
    use crate::test_support::TEST_DOMAIN_LOCK;

    /// Real libvirt calls against the `test` domain libvirt's builtin
    /// `test:///default` driver always defines and starts running -
    /// same no-daemon guarantee `connection.rs`'s own tests rely on.
    ///
    /// Uses a PCI `<hostdev>`, not a `<disk>`: the test driver's live
    /// attach only implements hostdev (libvirt 10.0 rejects disk,
    /// interface, rng and watchdog with "live attach of device '...'
    /// is not supported"), so hostdev is the device that proves the
    /// hotplug path on every libvirt version CI may run.
    #[test]
    fn attach_device_hotplugs_a_hostdev_onto_the_running_test_domain() {
        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        let domain = Domain::lookup_by_name(&conn, "test").expect("the default test domain");
        assert!(
            domain.is_active().expect("query succeeds"),
            "test domain starts running"
        );

        let hostdev = DeviceHostdev {
            mode: Some("subsystem".into()),
            hostdev_type: Some("pci".into()),
            managed: Some(true),
            pci_domain: Some("0x0000".into()),
            pci_bus: Some("0x00".into()),
            pci_slot: Some("0x1f".into()),
            pci_function: Some("0x0".into()),
            ..Default::default()
        };
        domain
            .attach_device(&hostdev)
            .expect("the test driver accepts hostdev hotplug");
    }

    #[test]
    fn add_device_persists_to_the_inactive_config() {
        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let inactive_xml = domain.inactive_xml().expect("fetch inactive XML");
        // libvirt's own XMLDesc serializer single-quotes attributes
        // (unlike this crate's writer) - assert on tag/content, not
        // quote style.
        assert!(inactive_xml.contains("<rng"));
        assert!(inactive_xml.contains("virtio"));
        assert!(inactive_xml.contains("/dev/urandom"));
    }

    /// `virt-xml --edit`'s core promise, proven against a real domain:
    /// editing one attribute leaves every other attribute on that same
    /// element — including ones this crate doesn't even model —
    /// completely untouched.
    #[test]
    fn edit_inactive_xml_mutates_one_attribute_and_leaves_the_rest_alone() {
        use crate::devices::DeviceWatchdog;

        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        let domain = Domain::lookup_by_name(&conn, "test").expect("the default test domain");

        let watchdog = DeviceWatchdog {
            model: Some("i6300esb".into()),
            action: Some("reset".into()),
        };
        domain
            .add_device(&conn, &watchdog)
            .expect("adding the watchdog succeeds");

        domain
            .edit_inactive_xml(&conn, |doc, root| {
                let devices = virtinst_xml::resolve_path(doc, root, &["devices"])
                    .ok_or_else(|| "no <devices> element".to_string())?;
                let el = devices
                    .find_all(doc, "watchdog")
                    .into_iter()
                    .next()
                    .ok_or_else(|| "no watchdog element".to_string())?;
                el.set_attribute(doc, "action", "poweroff");
                Ok(())
            })
            .expect("editing the existing watchdog succeeds");

        // Re-parse and check the watchdog element's own attributes
        // directly, not a whole-document substring scan — `test:///`'s
        // default domain XML has plenty of unrelated text ("restart"
        // in `<on_reboot>`, etc.) that a bare `!contains("reset")`
        // could pass against for the wrong reason.
        let inactive_xml = domain.inactive_xml().expect("fetch inactive XML");
        let doc = virtinst_xml::parse_libvirt_xml(&inactive_xml).expect("re-parses");
        let root = doc.root_element().unwrap();
        let devices = virtinst_xml::resolve_path(&doc, root, &["devices"]).unwrap();
        let watchdog = devices.find_all(&doc, "watchdog")[0];
        // The edited attribute changed...
        assert_eq!(watchdog.attribute(&doc, "action"), Some("poweroff"));
        // ...but the untouched one is exactly as it was.
        assert_eq!(watchdog.attribute(&doc, "model"), Some("i6300esb"));

        // `test:///default`'s "test" domain persists across tests within
        // one test binary run — clean up what this test added so a
        // later test (or a later run of this one) doesn't inherit a
        // leftover watchdog. Same trap `virtinst-cli`'s equivalent test
        // hit for real (confirmed empirically: without this, it saw 2
        // watchdogs after adding 1).
        use virtinst_xml::list_remove;
        domain
            .edit_inactive_xml(&conn, |doc, root| {
                let devices = virtinst_xml::resolve_path(doc, root, &["devices"])
                    .ok_or_else(|| "no <devices> element".to_string())?;
                for el in devices.find_all(doc, "watchdog") {
                    list_remove(doc, el).map_err(|e| e.to_string())?;
                }
                Ok(())
            })
            .expect("test cleanup succeeds");
    }
}
