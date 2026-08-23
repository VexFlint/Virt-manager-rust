//! Builds fresh domain `Document`s for `InstallMethod::Manual { boot_dev:
//! Some(_) }` — both the CLI's bare `--boot <dev>` single-phase case
//! (`build_manual_document`) and `--pxe`'s two-phase case
//! (`build_manual_two_phase_documents`).
//!
//! `--boot <dev>` alone is upstream's single-phase path: `virtinstall.py`'s
//! `build_installer` sets `no_install = True` when `--boot` is the only
//! install-method flag given (`options.boot_was_set`), and `install/
//! installer.py`'s `requires_postboot_xml_changes`'s `if self._no_install:
//! return False` short-circuits before the `install_bootdev` check ever
//! runs.
//!
//! `--pxe` (`install_bootdev = "network"`, `no_install` left unset) always
//! goes through the two-phase path, grounded directly in
//! `_alter_bootconfig`/`_get_postinstall_bootdev`:
//! - **initial** (`createXML`): `_alter_bootconfig` sets bootorder to
//!   `_build_boot_order(guest, "network")` unconditionally — `[network]`,
//!   or `[network, hd]` if a disk is present (same "always keep the disk
//!   bootable" rule `build_manual_document` already implements).
//! - **final** (`defineXML`): `_get_postinstall_bootdev` — with
//!   `install_bootdev` set **and a disk present**, resolves to `"hd"`
//!   (`_build_boot_order(guest, "hd")` = `[hd]`, the disk-loop finds `hd`
//!   already there so nothing else gets appended); with no disk, resolves
//!   back to `install_bootdev` unchanged (`"network"`), so initial and
//!   final agree — two-phase still runs (`requires_postboot_xml_changes()`
//!   only checks `_install_bootdev` truthiness, not disk presence), it's
//!   just vacuous: both documents describe the same boot order, only the
//!   UUID-sharing machinery is doing real work.
//! - The initial document also gets `<on_reboot>destroy</on_reboot>`
//!   (`_alter_bootconfig`'s first line) — the actual mechanism, not the
//!   boot order alone, that lets the persisted config take over once
//!   install finishes. See `push_on_reboot_destroy`'s doc comment.
//!
//! `InstallMethod::Manual { boot_dev: None }` (the wizard's bare "Manual
//! install" radio button) isn't built here either — it has no CLI
//! trigger at all (nothing sets `install_bootdev` to `None` while also
//! reaching this code path; the wizard doesn't run through
//! `virtinstall.py`'s CLI-flag branching), so there'd be nothing to call
//! it from. Shipping an unreachable function is worse than not building
//! it — deferred until the real Create VM wizard exists.
//!
//! Every document built here also gets one default NIC
//! (`domain_skeleton::push_default_nic`) — this was flagged as a real
//! gap in an earlier version of this module (`--pxe` producing a domain
//! that can't actually network-boot, sharper than `import.rs`'s bus-
//! selection gap since it made a shipped feature inert rather than
//! merely non-optimal) and fixed once grounded: it's upstream's own CLI
//! default whenever `--network`/`--nonetworks` are absent, not specific
//! to `--pxe`. Upstream also gates `--pxe` on `supports_pxe(guest)`
//! (`virtinstall.py:379`) — unvalidated here, consistent with every
//! other field in this module.

use virtinst_xml::Document;

use super::domain_skeleton::{
    build_domain_skeleton, push_boot_dev, push_default_clock, push_default_graphics,
    push_default_nic, push_on_reboot_destroy, push_virtio_disk, GuestCapsDefaults,
};

/// `name`/`memory_kib`/`vcpus`/`caps`/`graphics_type` — see
/// `build_import_document`'s doc comment, identical rationale.
/// `boot_dev` is libvirt's own `<os><boot dev=...>` string (`"network"`,
/// `"hd"`, …), passed straight through unvalidated, matching
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
    caps: GuestCapsDefaults,
    graphics_type: &str,
) -> Document {
    build_manual_phase_document(
        name,
        memory_kib,
        vcpus,
        &crate::cloner::generate_uuid(),
        caps,
        boot_dev,
        disk_path,
        &crate::cloner::generate_mac(caps.is_qemu),
        graphics_type,
        false,
    )
}

/// `InstallMethod::Manual { boot_dev: Some(_) }` via `--pxe` — the
/// two-phase case. See this module's own header for the exact
/// `createXML`/`defineXML` bootorder derivation and why it still runs
/// (with the same UUID-sharing machinery `iso::build_iso_two_phase_
/// documents` uses) even when a disk-less invocation makes the two
/// phases' content identical.
///
/// Returns `(initial, final)` — pass `initial`'s XML to `Domain::
/// create_new` and `final`'s to `Domain::define_new`, in that order.
pub fn build_manual_two_phase_documents(
    name: &str,
    memory_kib: u64,
    vcpus: u32,
    boot_dev: &str,
    disk_path: Option<&str>,
    caps: GuestCapsDefaults,
    graphics_type: &str,
) -> (Document, Document) {
    let uuid = crate::cloner::generate_uuid();
    // Same NIC, same MAC in both documents — same reason the UUID is
    // shared, see `push_default_nic`'s doc comment.
    let mac = crate::cloner::generate_mac(caps.is_qemu);

    let initial = build_manual_phase_document(
        name,
        memory_kib,
        vcpus,
        &uuid,
        caps,
        boot_dev,
        disk_path,
        &mac,
        graphics_type,
        true,
    );

    // _get_postinstall_bootdev: a present disk always wins back to "hd"
    // for the postinstall boot; with no disk, the install_bootdev the
    // caller asked for stands.
    let final_boot_dev = if disk_path.is_some() { "hd" } else { boot_dev };
    let final_doc = build_manual_phase_document(
        name,
        memory_kib,
        vcpus,
        &uuid,
        caps,
        final_boot_dev,
        disk_path,
        &mac,
        graphics_type,
        false,
    );

    (initial, final_doc)
}

/// `is_initial` gates `<on_reboot>destroy</on_reboot>` — see
/// `push_on_reboot_destroy`'s doc comment for why only the initial
/// document of a two-phase pair gets it; `build_manual_document`'s
/// single-phase call always passes `false` since upstream's
/// `_alter_bootconfig` (the only place that sets it) never runs at all
/// outside `requires_postboot_xml_changes()`'s two-phase path. `devices_
/// el` (and the default NIC/graphics on it) is always built now, even
/// with no disk — a disk-less `Manual` document used to return before
/// `<devices>` existed at all; that's no longer true since the NIC
/// needs somewhere to attach. `caps` is `Copy`, same reasoning as
/// `iso::build_iso_phase_document`.
#[allow(clippy::too_many_arguments)]
fn build_manual_phase_document(
    name: &str,
    memory_kib: u64,
    vcpus: u32,
    uuid: &str,
    caps: GuestCapsDefaults,
    boot_dev: &str,
    disk_path: Option<&str>,
    mac: &str,
    graphics_type: &str,
    is_initial: bool,
) -> Document {
    let (mut doc, root, os_el, devices_el) =
        build_domain_skeleton(name, memory_kib, vcpus, uuid, &caps);
    push_default_clock(&mut doc, root, caps.os_type, caps.arch, caps.is_qemu);
    if is_initial {
        push_on_reboot_destroy(&mut doc, root);
    }
    push_boot_dev(&mut doc, os_el, boot_dev);
    if disk_path.is_some() && boot_dev != "hd" {
        push_boot_dev(&mut doc, os_el, "hd");
    }

    if let Some(disk_path) = disk_path {
        push_virtio_disk(&mut doc, devices_el, disk_path);
    }
    push_default_nic(&mut doc, devices_el, mac);
    push_default_graphics(&mut doc, devices_el, graphics_type);

    doc
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn no_disk_means_a_single_boot_entry_and_no_disk_device() {
        let doc = build_manual_document(
            "net-only",
            1048576,
            1,
            "network",
            None,
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

        assert!(Uuid::parse_str(&root.find(&doc, "uuid").unwrap().text_content(&doc)).is_ok());

        let os = root.find(&doc, "os").unwrap();
        let boots = os.find_all(&doc, "boot");
        assert_eq!(boots.len(), 1);
        assert_eq!(boots[0].attribute(&doc, "dev"), Some("network"));

        let devices = root.find(&doc, "devices").unwrap();
        assert!(devices.find(&doc, "disk").is_none(), "no disk was given");
        let iface = devices.find(&doc, "interface").unwrap();
        assert_eq!(iface.attribute(&doc, "type"), Some("network"));
        assert_eq!(
            devices
                .find(&doc, "graphics")
                .unwrap()
                .attribute(&doc, "type"),
            Some("spice")
        );

        // Ticket 21 slice (d).
        assert_eq!(
            root.find(&doc, "clock")
                .unwrap()
                .find_all(&doc, "timer")
                .len(),
            3
        );
    }

    #[test]
    fn a_disk_appends_hd_to_the_boot_order_and_a_disk_device() {
        let doc = build_manual_document(
            "net-with-disk",
            2097152,
            2,
            "network",
            Some("/var/lib/libvirt/images/x.qcow2"),
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
        assert!(devices.find(&doc, "interface").is_some());
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
            GuestCapsDefaults {
                domain_type: "test",
                os_type: "hvm",
                arch: "i686",
                machine: None,
                emulator: None,
                is_qemu: false,
            },
            "spice",
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
            GuestCapsDefaults {
                domain_type: "test",
                os_type: "hvm",
                arch: "i686",
                machine: None,
                emulator: None,
                is_qemu: false,
            },
            "spice",
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
        let domcaps = DomainCapabilities::fetch(&conn, None, None, None, None)
            .expect("domcaps fetch succeeds");
        let domain_type = domcaps.domain.expect("test driver reports a domain type");
        let arch = domcaps.arch.expect("test driver reports a default arch");

        let doc = build_manual_document(
            "manual-end-to-end",
            1048576,
            1,
            "network",
            Some("/guest/manual-end-to-end.img"),
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
        let refetched = Domain::lookup_by_name(&conn, "manual-end-to-end")
            .expect("the domain is findable by the name we gave it");
        assert_eq!(refetched.name().unwrap(), "manual-end-to-end");
        let inactive_xml = refetched.inactive_xml().expect("fetch inactive XML");
        assert!(inactive_xml.contains("manual-end-to-end.img"));
        assert!(
            inactive_xml.contains("type='spice'"),
            "the real driver kept the <graphics type='spice'> element we defined it with"
        );
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

    #[test]
    fn two_phase_with_a_disk_narrows_from_network_hd_down_to_hd_alone() {
        let (initial, final_doc) = build_manual_two_phase_documents(
            "pxe-two-phase",
            2097152,
            2,
            "network",
            Some("/var/lib/libvirt/images/disk.qcow2"),
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
        assert_eq!(initial_uuid, final_uuid);
        assert!(Uuid::parse_str(&initial_uuid).is_ok());

        assert_eq!(
            initial_root
                .find(&initial, "on_reboot")
                .map(|el| el.text_content(&initial)),
            Some("destroy".to_string()),
            "only the initial document overrides on_reboot"
        );
        assert!(final_root.find(&final_doc, "on_reboot").is_none());

        let initial_boots = initial_root
            .find(&initial, "os")
            .unwrap()
            .find_all(&initial, "boot");
        assert_eq!(initial_boots.len(), 2, "network first, hd as fallback");
        assert_eq!(initial_boots[0].attribute(&initial, "dev"), Some("network"));
        assert_eq!(initial_boots[1].attribute(&initial, "dev"), Some("hd"));

        let final_boots = final_root
            .find(&final_doc, "os")
            .unwrap()
            .find_all(&final_doc, "boot");
        assert_eq!(
            final_boots.len(),
            1,
            "postinstall boot favors the now-installed disk"
        );
        assert_eq!(final_boots[0].attribute(&final_doc, "dev"), Some("hd"));

        let initial_mac = initial_root
            .find(&initial, "devices")
            .unwrap()
            .find(&initial, "interface")
            .unwrap()
            .find(&initial, "mac")
            .unwrap()
            .attribute(&initial, "address")
            .unwrap()
            .to_string();
        let final_mac = final_root
            .find(&final_doc, "devices")
            .unwrap()
            .find(&final_doc, "interface")
            .unwrap()
            .find(&final_doc, "mac")
            .unwrap()
            .attribute(&final_doc, "address")
            .unwrap()
            .to_string();
        assert_eq!(initial_mac, final_mac, "same NIC in both documents");

        // Ticket 21 slice (d): both phases get the clock trio too.
        assert!(initial_root.find(&initial, "clock").is_some());
        assert!(final_root.find(&final_doc, "clock").is_some());
    }

    #[test]
    fn two_phase_with_no_disk_has_identical_boot_orders_but_still_runs() {
        // "Vacuous" only describes the boot order (_get_postinstall_
        // bootdev falls back to install_bootdev unchanged with no disk
        // to override it to "hd") — the two documents are never fully
        // identical, since on_reboot is still initial-only.
        let (initial, final_doc) = build_manual_two_phase_documents(
            "pxe-no-disk",
            1048576,
            1,
            "network",
            None,
            GuestCapsDefaults {
                domain_type: "test",
                os_type: "hvm",
                arch: "i686",
                machine: None,
                emulator: None,
                is_qemu: false,
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
        assert_eq!(initial_uuid, final_uuid);

        assert_eq!(
            initial_root
                .find(&initial, "on_reboot")
                .map(|el| el.text_content(&initial)),
            Some("destroy".to_string())
        );
        assert!(final_root.find(&final_doc, "on_reboot").is_none());

        for (doc, root) in [(&initial, &initial_root), (&final_doc, &final_root)] {
            let boots = root.find(doc, "os").unwrap().find_all(doc, "boot");
            assert_eq!(boots.len(), 1);
            assert_eq!(boots[0].attribute(doc, "dev"), Some("network"));
        }
    }

    /// Real end-to-end proof that `--pxe --disk`'s two-phase sequence
    /// actually happens: the running instance boots `[network, hd]`,
    /// the persisted config narrows to `[hd]` alone — same shape and
    /// same rationale as `iso.rs`'s two-phase test (`live_xml()` before
    /// `define_new` runs, `inactive_xml()` after).
    #[test]
    fn two_phase_install_creates_transient_network_boot_then_persists_hd_boot() {
        use crate::capabilities::DomainCapabilities;
        use crate::connection::{Connection, Domain};
        use crate::test_support::TEST_DOMAIN_LOCK;

        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        let domcaps = DomainCapabilities::fetch(&conn, None, None, None, None)
            .expect("domcaps fetch succeeds");
        let domain_type = domcaps.domain.expect("test driver reports a domain type");
        let arch = domcaps.arch.expect("test driver reports a default arch");

        let (initial, final_doc) = build_manual_two_phase_documents(
            "pxe-two-phase-end-to-end",
            1048576,
            1,
            "network",
            Some("/guest/pxe-two-phase-end-to-end.img"),
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
        assert_ne!(initial_xml, final_xml);

        let running = Domain::create_new(&conn, &initial_xml).expect("transient create succeeds");
        assert!(running.is_active().expect("query succeeds"));
        let live_xml = running.live_xml().expect("fetch live XML");
        assert!(
            live_xml.contains("boot dev='network'"),
            "the running instance's boot order starts with network"
        );
        assert!(
            live_xml.contains("boot dev='hd'"),
            "the running instance still keeps hd as a fallback"
        );
        assert!(
            live_xml.contains("<on_reboot>destroy</on_reboot>"),
            "the running instance won't reboot back into the install boot order"
        );

        let persisted = Domain::define_new(&conn, &final_xml).expect("define succeeds");
        let refetched = Domain::lookup_by_name(&conn, "pxe-two-phase-end-to-end")
            .expect("the domain is findable by the name we gave it");
        let inactive_xml = refetched.inactive_xml().expect("fetch inactive XML");
        assert!(
            inactive_xml.contains("boot dev='hd'"),
            "the persisted config boots from hd"
        );
        assert!(
            !inactive_xml.contains("boot dev='network'"),
            "the persisted config's boot order has no network entry"
        );
        assert!(
            !inactive_xml.contains("<on_reboot>destroy</on_reboot>"),
            "the persisted config doesn't carry the initial-only on_reboot override"
        );

        running
            .destroy()
            .expect("cleanup: destroy the transient instance");
        let still_defined = Domain::lookup_by_name(&conn, "pxe-two-phase-end-to-end")
            .expect("persistent config survives destroying the running instance");
        assert!(!still_defined.is_active().expect("query succeeds"));

        persisted
            .undefine()
            .expect("cleanup: undefine the persistent config");
    }
}
