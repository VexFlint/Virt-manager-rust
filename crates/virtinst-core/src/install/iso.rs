//! Builds a fresh domain `Document` for `InstallMethod::Iso { live: true, .. }`
//! — upstream's `no_install = True` path for CD media
//! (`_get_postinstall_bootdev`: `if self.cdrom and self._no_install:
//! return BOOT_DEVICE_CDROM`), reached only when `_validate_storage_page`'s
//! `livecd = not storage_enabled` held: no disk was configured at all, so
//! the CD is both the install media and the only thing the guest ever
//! boots from.
//!
//! `live: false` (a persistent disk *and* an install CD, the ordinary
//! "install from ISO onto a new disk" case) is real upstream behavior too,
//! but needs the two-phase `requires_postboot_xml_changes()` dance —
//! `createXML` with `boot dev="cdrom"`, then a *different* `defineXML`
//! with `boot dev="hd"` once install finishes — which nothing in this
//! crate does yet (`create_new`/`define_new` are both single-shot). Not
//! built here; flagged rather than faked. See the ticket 08 amendment for
//! why direct `edit_xml` construction is used instead of
//! `Guest::to_xml_string()`.
//!
//! **Real simplifications, flagged rather than built** (same posture as
//! `import.rs`):
//! - The cdrom device is given upstream's actual x86/hvm defaults
//!   (`_default_bus`: `bus="ide"`, `generate_target`: `target="hda"` — the
//!   first free `hd*` slot when nothing else claims it — `read_only`
//!   always true for cdrom, per `set_defaults`), not derived generally —
//!   a non-x86 `arch` or non-hvm guest would get a different bus upstream
//!   (`fdc`/`xen`/`scsi`/`sd`/`usb` depending on machine type), unmodeled
//!   here same as `import.rs`'s disk-bus gap.
//! - No media-existence validation, matching every other path field in
//!   this module.

use uuid::Uuid;
use virtinst_xml::{Document, Element};

fn push_text_child(doc: &mut Document, parent: Element, tag: &str, text: &str) {
    let el = Element::new_with_text(doc, tag, text);
    parent
        .push_child(doc, el)
        .expect("a freshly-created detached element always attaches cleanly");
}

/// `name`/`memory_kib`/`vcpus`/`domain_type`/`arch` — see
/// `build_import_document`'s doc comment, identical rationale. `media` is
/// `InstallMethod::Iso::media`, the path to the ISO/CD image.
///
/// Builds and boots from the CD alone (`<boot dev="cdrom">`) with no
/// persistent disk device at all — matches `live: true`'s own doc comment
/// on `InstallMethod::Iso` ("no persistent disk attached"). Stamps an
/// explicit `<uuid>` for the same reason `build_import_document` does:
/// `virt-install`'s default sequence calls `Domain::create_new` then
/// `Domain::define_new` against this *same* XML, and two separate
/// libvirt-generated UUIDs collide on the second call.
pub fn build_iso_document(
    name: &str,
    memory_kib: u64,
    vcpus: u32,
    media: &str,
    domain_type: &str,
    arch: &str,
) -> Document {
    let mut doc = Document::new();
    let root = Element::new(&mut doc, "domain");
    root.set_attribute(&mut doc, "type", domain_type);
    doc.push_root_node(root)
        .expect("a freshly-created detached element always attaches cleanly");

    push_text_child(&mut doc, root, "name", name);
    push_text_child(&mut doc, root, "uuid", &Uuid::new_v4().to_string());

    let mem_el = Element::new_with_text(&mut doc, "memory", &memory_kib.to_string());
    mem_el.set_attribute(&mut doc, "unit", "KiB");
    root.push_child(&mut doc, mem_el)
        .expect("a freshly-created detached element always attaches cleanly");

    let curmem_el = Element::new_with_text(&mut doc, "currentMemory", &memory_kib.to_string());
    curmem_el.set_attribute(&mut doc, "unit", "KiB");
    root.push_child(&mut doc, curmem_el)
        .expect("a freshly-created detached element always attaches cleanly");

    push_text_child(&mut doc, root, "vcpu", &vcpus.to_string());

    let os_el = Element::new(&mut doc, "os");
    let type_el = Element::new_with_text(&mut doc, "type", "hvm");
    type_el.set_attribute(&mut doc, "arch", arch);
    os_el
        .push_child(&mut doc, type_el)
        .expect("a freshly-created detached element always attaches cleanly");
    let boot_el = Element::new(&mut doc, "boot");
    boot_el.set_attribute(&mut doc, "dev", "cdrom");
    os_el
        .push_child(&mut doc, boot_el)
        .expect("a freshly-created detached element always attaches cleanly");
    root.push_child(&mut doc, os_el)
        .expect("a freshly-created detached element always attaches cleanly");

    let devices_el = Element::new(&mut doc, "devices");
    let disk_el = Element::new(&mut doc, "disk");
    disk_el.set_attribute(&mut doc, "type", "file");
    disk_el.set_attribute(&mut doc, "device", "cdrom");
    let source_el = Element::new(&mut doc, "source");
    source_el.set_attribute(&mut doc, "file", media);
    disk_el
        .push_child(&mut doc, source_el)
        .expect("a freshly-created detached element always attaches cleanly");
    let target_el = Element::new(&mut doc, "target");
    target_el.set_attribute(&mut doc, "dev", "hda");
    target_el.set_attribute(&mut doc, "bus", "ide");
    disk_el
        .push_child(&mut doc, target_el)
        .expect("a freshly-created detached element always attaches cleanly");
    let readonly_el = Element::new(&mut doc, "readonly");
    disk_el
        .push_child(&mut doc, readonly_el)
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
    fn builds_a_livecd_domain_with_no_persistent_disk() {
        let doc = build_iso_document(
            "livecd-guest",
            2097152,
            2,
            "/var/lib/libvirt/images/rescue.iso",
            "qemu",
            "x86_64",
        );
        let root = doc.root_element().unwrap();

        assert_eq!(root.attribute(&doc, "type"), Some("qemu"));
        assert!(Uuid::parse_str(&root.find(&doc, "uuid").unwrap().text_content(&doc)).is_ok());
        assert_eq!(
            root.find(&doc, "name").unwrap().text_content(&doc),
            "livecd-guest"
        );

        let os = root.find(&doc, "os").unwrap();
        assert_eq!(
            os.find(&doc, "boot").unwrap().attribute(&doc, "dev"),
            Some("cdrom")
        );

        let devices = root.find(&doc, "devices").unwrap();
        let disks = devices.find_all(&doc, "disk");
        assert_eq!(disks.len(), 1, "livecd has no persistent disk, cdrom only");

        let disk = disks[0];
        assert_eq!(disk.attribute(&doc, "type"), Some("file"));
        assert_eq!(disk.attribute(&doc, "device"), Some("cdrom"));
        assert_eq!(
            disk.find(&doc, "source").unwrap().attribute(&doc, "file"),
            Some("/var/lib/libvirt/images/rescue.iso")
        );
        let target = disk.find(&doc, "target").unwrap();
        assert_eq!(target.attribute(&doc, "dev"), Some("hda"));
        assert_eq!(target.attribute(&doc, "bus"), Some("ide"));
        assert!(disk.find(&doc, "readonly").is_some());
    }

    #[test]
    fn serializes_and_reparses_cleanly() {
        let doc = build_iso_document("x", 1048576, 1, "/tmp/x.iso", "test", "i686");
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

    /// Real end-to-end proof against `test:///default`, same shape and
    /// same rationale as `import.rs`'s `import_creates_a_running_and_
    /// persisted_domain` — see that test's doc comment for why the
    /// destroy-then-relookup assertion is the one that actually matters.
    #[test]
    fn livecd_creates_a_running_and_persisted_domain() {
        use crate::capabilities::DomainCapabilities;
        use crate::connection::{Connection, Domain};
        use crate::test_support::TEST_DOMAIN_LOCK;

        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        let caps = DomainCapabilities::fetch(&conn, None, None, None, None)
            .expect("domcaps fetch succeeds");
        let domain_type = caps.domain.expect("test driver reports a domain type");
        let arch = caps.arch.expect("test driver reports a default arch");

        let doc = build_iso_document(
            "livecd-end-to-end",
            1048576,
            1,
            "/guest/livecd-end-to-end.iso",
            &domain_type,
            &arch,
        );
        let xml = doc.write_str().expect("serializes");

        let running = Domain::create_new(&conn, &xml).expect("transient create succeeds");
        assert!(running.is_active().expect("query succeeds"));

        let persisted = Domain::define_new(&conn, &xml).expect("define succeeds");
        let refetched = Domain::lookup_by_name(&conn, "livecd-end-to-end")
            .expect("the domain is findable by the name we gave it");
        assert_eq!(refetched.name().unwrap(), "livecd-end-to-end");
        assert!(refetched
            .inactive_xml()
            .expect("fetch inactive XML")
            .contains("livecd-end-to-end.iso"));

        running
            .destroy()
            .expect("cleanup: destroy the transient instance");
        let still_defined = Domain::lookup_by_name(&conn, "livecd-end-to-end")
            .expect("persistent config survives destroying the running instance");
        assert!(!still_defined.is_active().expect("query succeeds"));

        persisted
            .undefine()
            .expect("cleanup: undefine the persistent config");
    }
}
