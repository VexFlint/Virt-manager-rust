//! Builds fresh domain `Document`s for `InstallMethod::Iso` — both the
//! `live: true` single-phase case (`build_iso_document`) and the
//! `live: false` two-phase case (`build_iso_two_phase_documents`).
//!
//! `live: true` is upstream's `no_install = True` path for CD media
//! (`_get_postinstall_bootdev`: `if self.cdrom and self._no_install:
//! return BOOT_DEVICE_CDROM`), reached only when `_validate_storage_page`'s
//! `livecd = not storage_enabled` held: no disk was configured at all, so
//! the CD is both the install media and the only thing the guest ever
//! boots from.
//!
//! `live: false` (a persistent disk *and* an install CD, the ordinary
//! "install from ISO onto a new disk" case) needs upstream's
//! `requires_postboot_xml_changes()` two-phase dance: `createXML` boots
//! from the CD, then a *different* `defineXML` persists the
//! post-install config. Grounded directly in `_build_postboot_xml`/
//! `_get_postinstall_bootdev`/`_remove_install_cdrom_media`:
//! - **initial** (`createXML`): `<boot dev="cdrom"/>`, cdrom device with
//!   its media attached, persistent disk also present.
//! - **final** (`defineXML`): `<boot dev="hd"/>` (`_get_postinstall_
//!   bootdev` falls to the disk[0]-derived branch since `_no_install` is
//!   false and the `self.cdrom and self._no_install` guard fails; the
//!   persistent disk being present resolves to `BOOT_DEVICE_HARDDISK`),
//!   cdrom device kept but with its `<source>` stripped
//!   (`_remove_install_cdrom_media`: `disk.set_source_path(None)` — an
//!   empty drive, not a removed device), persistent disk unchanged.
//!
//! **The initial document also gets `<on_reboot>destroy</on_reboot>`**
//! (`_alter_bootconfig`'s first line, `guest.on_reboot = "destroy"`) —
//! this is the actual mechanism that makes two-phase installs work.
//! libvirt's own default is `restart`; without this override the
//! transient instance would reboot straight back into the cdrom-booting
//! initial config forever once install finishes, instead of yielding to
//! the already-persisted final config. Missing from this module's first
//! version of `build_iso_two_phase_documents` — no test caught it
//! because `test:///` never actually boots anything, only the XML's
//! presence/absence is checkable here.
//!
//! **One deliberate divergence from upstream, not a bug**: upstream's
//! *initial* boot order for a plain `--cdrom` (no `--pxe`) actually
//! comes out empty (`_alter_bootconfig`'s `bootdev = self._install_
//! bootdev`, which is `None` here — only `--pxe` sets it — so `guest.os.
//! bootorder = []`), relying on implicit BIOS-default device-class
//! priority (cdrom before disk) to boot the CD first. That's real but
//! untestable the way this crate proves things (`inactive_xml()`
//! assertions can't observe what a BIOS would have picked), and
//! environment-dependent in a way an explicit `<boot dev="cdrom"/>`
//! isn't. This module emits the explicit tag instead — same outcome,
//! deterministic, and it's exactly what `live: true`'s `build_iso_
//! document` already does.
//!
//! **Also not modeled**: `_remove_install_cdrom_media`'s "keep media
//! attached for Windows, which has a multi-stage install" exception —
//! this crate has no `OsInfo`/guest-OS detection at all yet (still
//! blocked on the `gir` codegen tool), so the final document always
//! strips the cdrom media, matching every non-Windows case.
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
//!
//! Shares the `<name>`/`<uuid>`/`<memory>`/`<currentMemory>`/`<vcpu>`/
//! `<os><type arch>` skeleton with `import.rs`/`manual.rs` via
//! `domain_skeleton.rs`. `build_domain_skeleton` takes the UUID as a
//! parameter specifically so the two-phase case can stamp the *same*
//! UUID into both documents — see its own doc comment.

use virtinst_xml::{Document, Element};

use super::domain_skeleton::{
    build_domain_skeleton, push_boot_dev, push_default_graphics, push_default_nic,
    push_on_reboot_destroy, push_virtio_disk, GuestCapsDefaults,
};

/// `name`/`memory_kib`/`vcpus`/`caps`/`graphics_type` — see
/// `build_import_document`'s doc comment, identical rationale. `media`
/// is `InstallMethod::Iso::media`, the path to the ISO/CD image.
///
/// Builds and boots from the CD alone (`<boot dev="cdrom">`) with no
/// persistent disk device at all — matches `live: true`'s own doc comment
/// on `InstallMethod::Iso` ("no persistent disk attached").
pub fn build_iso_document(
    name: &str,
    memory_kib: u64,
    vcpus: u32,
    media: &str,
    caps: GuestCapsDefaults,
    graphics_type: &str,
) -> Document {
    let (mut doc, _root, os_el, devices_el) = build_domain_skeleton(
        name,
        memory_kib,
        vcpus,
        &crate::cloner::generate_uuid(),
        &caps,
    );
    push_boot_dev(&mut doc, os_el, "cdrom");

    push_cdrom_disk(&mut doc, devices_el, Some(media));
    push_default_nic(
        &mut doc,
        devices_el,
        &crate::cloner::generate_mac(caps.is_qemu),
    );
    push_default_graphics(&mut doc, devices_el, graphics_type);

    doc
}

/// `InstallMethod::Iso { live: false, .. }` — install from CD onto a new
/// persistent disk, upstream's two-phase case. See this module's own doc
/// comment for the exact `createXML`/`defineXML` shapes and the one
/// deliberate divergence (explicit `<boot dev="cdrom"/>` on the initial
/// document rather than an empty bootorder relying on BIOS default
/// ordering).
///
/// Returns `(initial, final)` — pass `initial`'s XML to `Domain::
/// create_new` and `final`'s to `Domain::define_new`, in that order,
/// matching upstream's `_create_guest`. Both documents share one
/// `generate_uuid()`, generated once here and threaded through both
/// `build_domain_skeleton` calls — required for the same reason a single
/// document always needed one: two libvirt-generated UUIDs would collide
/// on the second call.
pub fn build_iso_two_phase_documents(
    name: &str,
    memory_kib: u64,
    vcpus: u32,
    media: &str,
    disk_path: &str,
    caps: GuestCapsDefaults,
    graphics_type: &str,
) -> (Document, Document) {
    let uuid = crate::cloner::generate_uuid();
    // Same device, same MAC in both documents — same reason the UUID is
    // shared, see `push_default_nic`'s doc comment.
    let mac = crate::cloner::generate_mac(caps.is_qemu);

    let initial = build_iso_phase_document(
        name,
        memory_kib,
        vcpus,
        &uuid,
        caps,
        "cdrom",
        Some(media),
        disk_path,
        &mac,
        graphics_type,
        true,
    );
    let final_doc = build_iso_phase_document(
        name,
        memory_kib,
        vcpus,
        &uuid,
        caps,
        "hd",
        None,
        disk_path,
        &mac,
        graphics_type,
        false,
    );
    (initial, final_doc)
}

/// Shared shape for one phase of the two-phase document pair: same
/// skeleton, same three devices (cdrom + persistent disk + NIC),
/// differing in `boot_dev`, whether the cdrom has media inserted
/// (`cdrom_source: Option<&str>`), and `is_initial` (`<on_reboot>
/// destroy</on_reboot>` — see this module's own doc comment for why
/// only the initial document gets it). `caps` is `Copy`, so both calls in
/// `build_iso_two_phase_documents` pass their own copy rather than a
/// shared reference.
#[allow(clippy::too_many_arguments)]
fn build_iso_phase_document(
    name: &str,
    memory_kib: u64,
    vcpus: u32,
    uuid: &str,
    caps: GuestCapsDefaults,
    boot_dev: &str,
    cdrom_source: Option<&str>,
    disk_path: &str,
    mac: &str,
    graphics_type: &str,
    is_initial: bool,
) -> Document {
    let (mut doc, root, os_el, devices_el) =
        build_domain_skeleton(name, memory_kib, vcpus, uuid, &caps);
    if is_initial {
        push_on_reboot_destroy(&mut doc, root);
    }
    push_boot_dev(&mut doc, os_el, boot_dev);

    push_cdrom_disk(&mut doc, devices_el, cdrom_source);
    push_virtio_disk(&mut doc, devices_el, disk_path);
    push_default_nic(&mut doc, devices_el, mac);
    push_default_graphics(&mut doc, devices_el, graphics_type);

    doc
}

/// `<disk type="file" device="cdrom"><source file=.../><target dev="hda"
/// bus="ide"/><readonly/></disk>` — `source` omitted entirely when
/// `media` is `None` (an empty drive, matching upstream's `_remove_
/// install_cdrom_media`: the device stays, the media doesn't).
fn push_cdrom_disk(doc: &mut Document, devices_el: Element, media: Option<&str>) {
    let disk_el = Element::new(doc, "disk");
    disk_el.set_attribute(doc, "type", "file");
    disk_el.set_attribute(doc, "device", "cdrom");
    if let Some(media) = media {
        let source_el = Element::new(doc, "source");
        source_el.set_attribute(doc, "file", media);
        disk_el
            .push_child(doc, source_el)
            .expect("a freshly-created detached element always attaches cleanly");
    }
    let target_el = Element::new(doc, "target");
    target_el.set_attribute(doc, "dev", "hda");
    target_el.set_attribute(doc, "bus", "ide");
    disk_el
        .push_child(doc, target_el)
        .expect("a freshly-created detached element always attaches cleanly");
    let readonly_el = Element::new(doc, "readonly");
    disk_el
        .push_child(doc, readonly_el)
        .expect("a freshly-created detached element always attaches cleanly");
    devices_el
        .push_child(doc, disk_el)
        .expect("a freshly-created detached element always attaches cleanly");
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn builds_a_livecd_domain_with_no_persistent_disk() {
        let doc = build_iso_document(
            "livecd-guest",
            2097152,
            2,
            "/var/lib/libvirt/images/rescue.iso",
            GuestCapsDefaults {
                domain_type: "qemu",
                os_type: "hvm",
                arch: "x86_64",
                machine: None,
                emulator: None,
                is_qemu: true,
            },
            "spice",
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

        let iface = devices.find(&doc, "interface").unwrap();
        assert_eq!(iface.attribute(&doc, "type"), Some("network"));
        assert_eq!(
            iface
                .find(&doc, "source")
                .unwrap()
                .attribute(&doc, "network"),
            Some("default")
        );

        let graphics = devices.find(&doc, "graphics").unwrap();
        assert_eq!(graphics.attribute(&doc, "type"), Some("spice"));
    }

    #[test]
    fn serializes_and_reparses_cleanly() {
        let doc = build_iso_document(
            "x",
            1048576,
            1,
            "/tmp/x.iso",
            GuestCapsDefaults {
                domain_type: "test",
                os_type: "hvm",
                arch: "i686",
                machine: None,
                emulator: None,
                is_qemu: false,
            },
            "vnc",
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
        let domcaps = DomainCapabilities::fetch(&conn, None, None, None, None)
            .expect("domcaps fetch succeeds");
        let domain_type = domcaps.domain.expect("test driver reports a domain type");
        let arch = domcaps.arch.expect("test driver reports a default arch");

        let doc = build_iso_document(
            "livecd-end-to-end",
            1048576,
            1,
            "/guest/livecd-end-to-end.iso",
            GuestCapsDefaults {
                domain_type: &domain_type,
                os_type: "hvm",
                arch: &arch,
                machine: None,
                emulator: None,
                is_qemu: conn.uri().is_qemu(),
            },
            "spice",
        );
        let xml = doc.write_str().expect("serializes");

        let running = Domain::create_new(&conn, &xml).expect("transient create succeeds");
        assert!(running.is_active().expect("query succeeds"));

        let persisted = Domain::define_new(&conn, &xml).expect("define succeeds");
        let refetched = Domain::lookup_by_name(&conn, "livecd-end-to-end")
            .expect("the domain is findable by the name we gave it");
        assert_eq!(refetched.name().unwrap(), "livecd-end-to-end");
        let inactive_xml = refetched.inactive_xml().expect("fetch inactive XML");
        assert!(inactive_xml.contains("livecd-end-to-end.iso"));
        assert!(
            inactive_xml.contains("type='spice'"),
            "the real driver kept the <graphics type='spice'> element we defined it with"
        );

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

    #[test]
    fn two_phase_documents_share_a_uuid_and_differ_in_boot_and_cdrom_source() {
        let (initial, final_doc) = build_iso_two_phase_documents(
            "two-phase-guest",
            2097152,
            2,
            "/var/lib/libvirt/images/install.iso",
            "/var/lib/libvirt/images/disk.qcow2",
            GuestCapsDefaults {
                domain_type: "qemu",
                os_type: "hvm",
                arch: "x86_64",
                machine: None,
                emulator: None,
                is_qemu: true,
            },
            "spice",
        );

        let initial_root = initial.root_element().unwrap();
        let final_root = final_doc.root_element().unwrap();

        let initial_uuid = initial_root
            .find(&initial, "uuid")
            .unwrap()
            .text_content(&initial);
        let final_uuid = final_root
            .find(&final_doc, "uuid")
            .unwrap()
            .text_content(&final_doc);
        assert_eq!(
            initial_uuid, final_uuid,
            "both phases must describe the same domain"
        );
        assert!(Uuid::parse_str(&initial_uuid).is_ok());

        // The actual mechanism that makes two-phase work: only the
        // initial document tells libvirt not to reboot back into itself
        // once the install's reboot happens.
        assert_eq!(
            initial_root
                .find(&initial, "on_reboot")
                .map(|el| el.text_content(&initial)),
            Some("destroy".to_string())
        );
        assert!(
            final_root.find(&final_doc, "on_reboot").is_none(),
            "the final document doesn't touch on_reboot, libvirt's default (restart) stands"
        );

        let initial_os = initial_root.find(&initial, "os").unwrap();
        assert_eq!(
            initial_os
                .find(&initial, "boot")
                .unwrap()
                .attribute(&initial, "dev"),
            Some("cdrom")
        );
        let final_os = final_root.find(&final_doc, "os").unwrap();
        assert_eq!(
            final_os
                .find(&final_doc, "boot")
                .unwrap()
                .attribute(&final_doc, "dev"),
            Some("hd")
        );

        let initial_devices = initial_root.find(&initial, "devices").unwrap();
        let initial_disks = initial_devices.find_all(&initial, "disk");
        assert_eq!(initial_disks.len(), 2, "cdrom + persistent disk");
        let initial_cdrom = initial_disks
            .iter()
            .find(|d| d.attribute(&initial, "device") == Some("cdrom"))
            .unwrap();
        assert_eq!(
            initial_cdrom
                .find(&initial, "source")
                .unwrap()
                .attribute(&initial, "file"),
            Some("/var/lib/libvirt/images/install.iso"),
            "install phase has media inserted"
        );

        let final_devices = final_root.find(&final_doc, "devices").unwrap();
        let final_disks = final_devices.find_all(&final_doc, "disk");
        assert_eq!(final_disks.len(), 2, "cdrom device stays, media doesn't");
        let final_cdrom = final_disks
            .iter()
            .find(|d| d.attribute(&final_doc, "device") == Some("cdrom"))
            .unwrap();
        assert!(
            final_cdrom.find(&final_doc, "source").is_none(),
            "postinstall cdrom has no media inserted"
        );
        let final_persistent_disk = final_disks
            .iter()
            .find(|d| d.attribute(&final_doc, "device") == Some("disk"))
            .unwrap();
        assert_eq!(
            final_persistent_disk
                .find(&final_doc, "source")
                .unwrap()
                .attribute(&final_doc, "file"),
            Some("/var/lib/libvirt/images/disk.qcow2")
        );

        // Same NIC, same MAC in both documents — same reason the disk
        // and UUID are shared.
        let initial_mac = initial_devices
            .find(&initial, "interface")
            .unwrap()
            .find(&initial, "mac")
            .unwrap()
            .attribute(&initial, "address")
            .unwrap()
            .to_string();
        let final_mac = final_devices
            .find(&final_doc, "interface")
            .unwrap()
            .find(&final_doc, "mac")
            .unwrap()
            .attribute(&final_doc, "address")
            .unwrap()
            .to_string();
        assert_eq!(initial_mac, final_mac);

        // Both documents get the same default graphics device too.
        assert_eq!(
            initial_devices
                .find(&initial, "graphics")
                .unwrap()
                .attribute(&initial, "type"),
            Some("spice")
        );
        assert_eq!(
            final_devices
                .find(&final_doc, "graphics")
                .unwrap()
                .attribute(&final_doc, "type"),
            Some("spice")
        );
    }

    /// Real end-to-end proof that two-phase create/define actually
    /// happens: `create_new` gets the initial (cdrom-booting) XML,
    /// `define_new` gets a *different* final (hd-booting) XML, and the
    /// persisted config that survives is the final one — this is the
    /// assertion that would fail if a future refactor collapsed the two
    /// calls back onto the same document.
    #[test]
    fn two_phase_install_creates_transient_cdrom_boot_then_persists_hd_boot() {
        use crate::capabilities::DomainCapabilities;
        use crate::connection::{Connection, Domain};
        use crate::test_support::TEST_DOMAIN_LOCK;

        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        let domcaps = DomainCapabilities::fetch(&conn, None, None, None, None)
            .expect("domcaps fetch succeeds");
        let domain_type = domcaps.domain.expect("test driver reports a domain type");
        let arch = domcaps.arch.expect("test driver reports a default arch");

        let (initial, final_doc) = build_iso_two_phase_documents(
            "two-phase-end-to-end",
            1048576,
            1,
            "/guest/two-phase-end-to-end.iso",
            "/guest/two-phase-end-to-end.img",
            GuestCapsDefaults {
                domain_type: &domain_type,
                os_type: "hvm",
                arch: &arch,
                machine: None,
                emulator: None,
                is_qemu: conn.uri().is_qemu(),
            },
            "spice",
        );
        let initial_xml = initial.write_str().expect("serializes");
        let final_xml = final_doc.write_str().expect("serializes");
        assert_ne!(
            initial_xml, final_xml,
            "the two phases must actually be different XML"
        );

        let running = Domain::create_new(&conn, &initial_xml).expect("transient create succeeds");
        assert!(running.is_active().expect("query succeeds"));
        // The assertion that actually proves create_new got the initial
        // (cdrom-booting) document rather than the final one — checked
        // before define_new runs, so this reflects create_new alone.
        let live_xml = running.live_xml().expect("fetch live XML");
        assert!(
            live_xml.contains("boot dev='cdrom'"),
            "the running instance boots from cdrom, matching the initial document"
        );
        assert!(
            !live_xml.contains("boot dev='hd'"),
            "the running instance's boot order has no hd entry yet"
        );
        assert!(
            live_xml.contains("<on_reboot>destroy</on_reboot>"),
            "the running instance won't reboot back into the install media — the actual \
             mechanism that makes two-phase work, not just a different boot order"
        );
        // Holds because this document's initial boot order is a single
        // `[cdrom]` entry, not `[cdrom, hd]` — if `--pxe --disk`'s
        // two-phase case is ever built reusing `_build_boot_order`'s
        // real "always append hd when a disk is present" rule (see
        // `manual.rs`), an initial document built that way would fail
        // this specific assertion without being a regression.

        let persisted = Domain::define_new(&conn, &final_xml).expect("define succeeds");
        let refetched = Domain::lookup_by_name(&conn, "two-phase-end-to-end")
            .expect("the domain is findable by the name we gave it");
        let inactive_xml = refetched.inactive_xml().expect("fetch inactive XML");
        assert!(
            inactive_xml.contains("boot dev='hd'"),
            "the persisted config boots from hd, not cdrom"
        );
        assert!(
            !inactive_xml.contains("boot dev='cdrom'"),
            "the persisted config's boot order has no cdrom entry"
        );
        assert!(
            !inactive_xml.contains("<on_reboot>destroy</on_reboot>"),
            "the persisted config doesn't carry the initial-only on_reboot override"
        );

        running
            .destroy()
            .expect("cleanup: destroy the transient instance");
        let still_defined = Domain::lookup_by_name(&conn, "two-phase-end-to-end")
            .expect("persistent config survives destroying the running instance");
        assert!(!still_defined.is_active().expect("query succeeds"));

        persisted
            .undefine()
            .expect("cleanup: undefine the persistent config");
    }
}
