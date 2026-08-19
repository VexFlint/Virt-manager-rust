//! Builds a fresh domain `Document` for `InstallMethod::Manual { boot_dev:
//! Some(_) }` — specifically the CLI's bare `--boot <dev>` invocation
//! (`virtinstall.py`'s `build_installer`: when `--boot` is the *only*
//! install-method flag given, `options.boot_was_set` sets `no_install =
//! True`), the single-phase case.
//!
//! **Not built: the two-phase case**, reached by `--pxe`
//! (`install_bootdev = "network"`, `no_install` left unset). Grounded
//! directly in `install/installer.py`'s `requires_postboot_xml_changes`:
//! `if self._no_install: return False` short-circuits `--boot` to
//! single-phase, but falls through to
//! `bool(self._cdrom or self._install_bootdev or self._treemedia)` for
//! `--pxe`, which is `True`. Two-phase means a *different* `<boot>`
//! entry in the `createXML` call (`_alter_bootconfig`, sets bootorder to
//! `[install_bootdev, maybe "hd"]` unconditionally) than the
//! `defineXML` call (`_get_postinstall_bootdev` — with `install_bootdev`
//! set **and a disk present**, this resolves back to `"hd"`, overriding
//! the "network" the CLI asked for, since the postinstall boot should
//! favor the now-installed disk; with no disk, both phases agree on
//! `"network"` and the distinction is vacuous for that specific case
//! even though `requires_postboot_xml_changes()` still reports `True`).
//! This crate's `create_new`/
//! `define_new` are both single-shot against one XML string, same gap
//! already flagged on `iso::build_iso_document`'s `live: false`. Two
//! features now blocked on the same missing primitive — the signal that
//! two-phase create/define is the next slice, not a coincidence.
//!
//! `InstallMethod::Manual { boot_dev: None }` (the wizard's bare "Manual
//! install" radio button) isn't built here either — it has no CLI
//! trigger at all (nothing sets `install_bootdev` to `None` while also
//! reaching this code path; the wizard doesn't run through
//! `virtinstall.py`'s CLI-flag branching), so there'd be nothing to call
//! it from. Shipping an unreachable function is worse than not building
//! it — deferred until the real Create VM wizard exists.

use uuid::Uuid;
use virtinst_xml::{Document, Element};

use super::domain_skeleton::{build_domain_skeleton, push_boot_dev};

/// `name`/`memory_kib`/`vcpus`/`domain_type`/`arch` — see
/// `build_import_document`'s doc comment, identical rationale. `boot_dev`
/// is libvirt's own `<os><boot dev=...>` string (`"network"`, `"hd"`,
/// …), passed straight through unvalidated, matching
/// `InstallMethod::Manual::boot_dev`'s own doc comment. `disk_path` is
/// optional: `Manual` doesn't require a disk (a bare `--boot network`
/// with no `--disk` is a real, valid invocation — e.g. a pure network
/// boot appliance).
///
/// Builds the boot order the same way upstream's `_build_boot_order`
/// does: `[boot_dev]`, then `"hd"` appended if a disk was actually given
/// and isn't already the boot device — so
/// `build_manual_document(.., "network", Some(path), ..)` emits
/// `<boot dev="network"/><boot dev="hd"/>`, matching upstream's "always
/// keep the disk bootable, even when something else has boot priority"
/// comment (needed for Windows virtio installs and PXE-then-local-disk
/// fallback). `_can_set_guest_bootorder` also gates this upstream
/// (`not any(d.boot.order for d in guest.devices.get_all())`) — always
/// true here since nothing on a freshly-built document has a per-device
/// boot order to conflict with, so it isn't modeled as a check.
///
/// `boot_dev = "hd"` with a disk present emits byte-identical XML to
/// `build_import_document`'s output (single `<boot dev="hd"/>`, one
/// `type="file" device="disk"` disk) — `--boot hd --disk X` and
/// `--import --disk X` are genuinely the same domain, not a bug worth
/// deduplicating away (the two CLI flags mean different things to a
/// user even when they land on the same XML).
pub fn build_manual_document(
    name: &str,
    memory_kib: u64,
    vcpus: u32,
    boot_dev: &str,
    disk_path: Option<&str>,
    domain_type: &str,
    arch: &str,
) -> Document {
    let (mut doc, root, os_el) = build_domain_skeleton(
        name,
        memory_kib,
        vcpus,
        &Uuid::new_v4().to_string(),
        domain_type,
        arch,
    );
    push_boot_dev(&mut doc, os_el, boot_dev);

    let Some(disk_path) = disk_path else {
        return doc;
    };

    if boot_dev != "hd" {
        push_boot_dev(&mut doc, os_el, "hd");
    }

    let devices_el = Element::new(&mut doc, "devices");
    let disk_el = Element::new(&mut doc, "disk");
    disk_el.set_attribute(&mut doc, "type", "file");
    disk_el.set_attribute(&mut doc, "device", "disk");
    let source_el = Element::new(&mut doc, "source");
    source_el.set_attribute(&mut doc, "file", disk_path);
    disk_el
        .push_child(&mut doc, source_el)
        .expect("a freshly-created detached element always attaches cleanly");
    let target_el = Element::new(&mut doc, "target");
    target_el.set_attribute(&mut doc, "dev", "vda");
    target_el.set_attribute(&mut doc, "bus", "virtio");
    disk_el
        .push_child(&mut doc, target_el)
        .expect("a freshly-created detached element always attaches cleanly");
    devices_el
        .push_child(&mut doc, disk_el)
        .expect("a freshly-created detached element always attaches cleanly");
    root.push_child(&mut doc, devices_el)
        .expect("a freshly-created detached element always attaches cleanly");

    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_disk_means_a_single_boot_entry_and_no_devices() {
        let doc = build_manual_document("net-only", 1048576, 1, "network", None, "qemu", "x86_64");
        let root = doc.root_element().unwrap();

        assert!(Uuid::parse_str(&root.find(&doc, "uuid").unwrap().text_content(&doc)).is_ok());

        let os = root.find(&doc, "os").unwrap();
        let boots = os.find_all(&doc, "boot");
        assert_eq!(boots.len(), 1);
        assert_eq!(boots[0].attribute(&doc, "dev"), Some("network"));

        assert!(root.find(&doc, "devices").is_none());
    }

    #[test]
    fn a_disk_appends_hd_to_the_boot_order_and_a_disk_device() {
        let doc = build_manual_document(
            "net-with-disk",
            2097152,
            2,
            "network",
            Some("/var/lib/libvirt/images/x.qcow2"),
            "qemu",
            "x86_64",
        );
        let root = doc.root_element().unwrap();

        let os = root.find(&doc, "os").unwrap();
        let boots = os.find_all(&doc, "boot");
        assert_eq!(boots.len(), 2, "network first, then hd as a fallback");
        assert_eq!(boots[0].attribute(&doc, "dev"), Some("network"));
        assert_eq!(boots[1].attribute(&doc, "dev"), Some("hd"));

        let devices = root.find(&doc, "devices").unwrap();
        let disk = devices.find(&doc, "disk").unwrap();
        assert_eq!(disk.attribute(&doc, "device"), Some("disk"));
        assert_eq!(
            disk.find(&doc, "source").unwrap().attribute(&doc, "file"),
            Some("/var/lib/libvirt/images/x.qcow2")
        );
    }

    #[test]
    fn boot_dev_hd_with_a_disk_stays_a_single_entry() {
        // boot_dev is already "hd" — _build_boot_order doesn't append a
        // second "hd" when it's already there.
        let doc = build_manual_document(
            "hd-boot",
            1048576,
            1,
            "hd",
            Some("/tmp/x.img"),
            "test",
            "i686",
        );
        let root = doc.root_element().unwrap();
        let os = root.find(&doc, "os").unwrap();
        assert_eq!(os.find_all(&doc, "boot").len(), 1);
    }

    #[test]
    fn serializes_and_reparses_cleanly() {
        let doc = build_manual_document(
            "x",
            1048576,
            1,
            "network",
            Some("/tmp/x.img"),
            "test",
            "i686",
        );
        let xml = doc.write_str().expect("serializes");
        let reparsed = virtinst_xml::parse_libvirt_xml(&xml).expect("re-parses");
        let root = reparsed.root_element().unwrap();
        assert_eq!(
            root.find(&reparsed, "name")
                .unwrap()
                .text_content(&reparsed),
            "x"
        );
    }

    /// Real end-to-end proof against `test:///default`, matching
    /// `import.rs`/`iso.rs`'s shape but exercising the one input
    /// combination that's structurally new to this module: `boot_dev =
    /// "network"` with a disk present, which is the only case that
    /// produces the two-entry `<boot dev="network"/><boot dev="hd"/>`
    /// order (`boot_dev = "hd"` with a disk emits the same single-entry
    /// XML `import.rs`'s own end-to-end test already covers, so it
    /// wouldn't prove anything new here). `test:///default` accepts
    /// `<boot dev="network"/>` with no `<interface>` device fine —
    /// confirmed directly (this was flagged as an open question before
    /// writing this test, not assumed).
    #[test]
    fn manual_creates_a_running_and_persisted_domain_with_two_boot_entries() {
        use crate::capabilities::DomainCapabilities;
        use crate::connection::{Connection, Domain};
        use crate::test_support::TEST_DOMAIN_LOCK;

        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        let caps = DomainCapabilities::fetch(&conn, None, None, None, None)
            .expect("domcaps fetch succeeds");
        let domain_type = caps.domain.expect("test driver reports a domain type");
        let arch = caps.arch.expect("test driver reports a default arch");

        let doc = build_manual_document(
            "manual-end-to-end",
            1048576,
            1,
            "network",
            Some("/guest/manual-end-to-end.img"),
            &domain_type,
            &arch,
        );
        let xml = doc.write_str().expect("serializes");

        let running = Domain::create_new(&conn, &xml).expect("transient create succeeds");
        assert!(running.is_active().expect("query succeeds"));

        let persisted = Domain::define_new(&conn, &xml).expect("define succeeds");
        let refetched = Domain::lookup_by_name(&conn, "manual-end-to-end")
            .expect("the domain is findable by the name we gave it");
        assert_eq!(refetched.name().unwrap(), "manual-end-to-end");
        let inactive_xml = refetched.inactive_xml().expect("fetch inactive XML");
        assert!(inactive_xml.contains("manual-end-to-end.img"));
        // The point of this test over import.rs's: the two-entry boot
        // order actually round-tripped through libvirt, in order.
        // libvirt's own XMLDesc output uses single-quoted attributes
        // (confirmed directly — this crate's own writer uses double
        // quotes, but this string is what came back from libvirt, not
        // what we sent it).
        let network_pos = inactive_xml
            .find("boot dev='network'")
            .expect("network boot entry present");
        let hd_pos = inactive_xml
            .find("boot dev='hd'")
            .expect("hd boot entry present");
        assert!(
            network_pos < hd_pos,
            "network must come before hd in the boot order"
        );

        running
            .destroy()
            .expect("cleanup: destroy the transient instance");
        let still_defined = Domain::lookup_by_name(&conn, "manual-end-to-end")
            .expect("persistent config survives destroying the running instance");
        assert!(!still_defined.is_active().expect("query succeeds"));

        persisted
            .undefine()
            .expect("cleanup: undefine the persistent config");
    }
}
