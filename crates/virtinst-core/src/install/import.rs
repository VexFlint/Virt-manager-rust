//! Builds a fresh domain `Document` for `InstallMethod::Import` —
//! upstream's `no_install = True` path (`virtinstall.py`'s
//! `build_installer`, `Installer._create_guest`): no OS-install phase,
//! the given disk becomes the VM's disk as-is.
//!
//! Per the amendment on ticket 08 (see `.scratch/rust-conversion/
//! issues/08-create-vm-wizard.md`): this builds the domain XML through
//! direct `edit_xml::Document`/`Element` construction, the same
//! primitives `cloner.rs` uses for DOM surgery, applied to a *fresh*
//! document instead of an edited one. `Guest::to_xml_string()` cannot
//! do this — it only serializes `Guest`'s flat fields, silently
//! dropping `<os>`/`<vcpu>`/`<devices>` (pinned by `xml_binding_tests`'
//! `to_xml_string_on_guest_omits_nested_and_list_fields`). The shared
//! `<name>`/`<uuid>`/`<memory>`/`<currentMemory>`/`<vcpu>`/`<os><type
//! arch>` skeleton common to every install method lives in
//! `domain_skeleton.rs`, extracted once `iso.rs` and this became two
//! identical copies.
//!
//! **Real simplifications, flagged rather than built:**
//! - The disk is given fixed defaults (`type="file"`, `device="disk"`,
//!   `target dev="vda" bus="virtio"`) instead of upstream's
//!   `guest.set_defaults()` — real driver/OS-aware bus selection
//!   (e.g. IDE for very old guest OS profiles, SCSI/virtio otherwise)
//!   isn't ported. Same gap already flagged for `virt-xml
//!   --add-device`.
//! - No storage validation (`DeviceDisk.path_definitely_exists`) —
//!   upstream requires the import path already exist; this crate
//!   doesn't check, matching every other field in this module (a
//!   caller concern, not baked into construction).
//! - `memory_kib`/`vcpus` are whatever the caller passes, no default —
//!   upstream's own `--memory`/`--vcpus` are optional too, but fall
//!   back to `osinfo`'s recommended-resources lookup when absent
//!   (`get_recommended_ram`/`get_recommended_ncpus`); `virt-install`
//!   here just requires `--memory` and defaults `--vcpus` to a flat
//!   `1`, since `OsInfo` isn't built yet either (still blocked on the
//!   `gir` codegen tool, same as libvirt-glib events).
//!
//! Always gets one default NIC (`domain_skeleton::push_default_nic`) —
//! upstream's own CLI default whenever `--network`/`--nonetworks` are
//! both absent, not something specific to this install method.

use virtinst_xml::{Document, Element};

use super::domain_skeleton::{build_domain_skeleton, push_boot_dev, push_default_nic};

/// `name`/`memory_kib`/`vcpus` are the wizard's Name/Memory pages'
/// worth of data — not `InstallMethod`'s job to carry (ticket 08 keeps
/// them off the enum entirely, see its own doc comment). `domain_type`
/// (`<domain type=...>`, e.g. `"qemu"`/`"test"`), `os_type` (`<os><type>`'s
/// text content, e.g. `"hvm"`), and `arch` (`<os><type arch=...>`) come
/// from the connection's own host `capabilities::Capabilities::
/// guest_lookup` rather than being guessed — upstream gets all three
/// (plus `loader`, not modeled yet) from `guest.
/// set_capabilities_defaults()`, and hardcoding any of them would be a
/// real, connection-specific-shaped divergence (`test:///` defaults to
/// `i686`, a real `qemu:///system` to `x86_64`).
///
/// `machine`/`emulator` are ticket 21 slice (a) — see `build_domain_
/// skeleton`'s own doc comment for exactly what's threaded (the
/// fallback-only `capsinfo.machines.first()`/plain `capsinfo.emulator`)
/// versus what stays unbuilt (the q35-preferred machine branch, real
/// UEFI/loader selection). `is_qemu` is the connection's URI scheme
/// (`Uri::is_qemu()`) for the default NIC's OUI — deliberately *not*
/// derived from `domain_type` (see `push_default_nic`'s doc comment
/// for the real bug that distinction fixes: `domain_type` is
/// `hypervisor_type`, e.g. `"kvm"` on a real accelerated
/// `qemu:///system`, not the literal string `"qemu"`).
///
/// Always stamps an explicit `<uuid>` (via the same `generate_uuid()`
/// approach `cloner.rs` uses, matching upstream's own hand-rolled
/// UUIDv4 bit-twiddling) rather than leaving it for libvirt to invent.
/// Required, not cosmetic: `virt-install`'s default sequence calls
/// `Domain::create_new` then `Domain::define_new` with this *same*
/// document's XML (matching upstream's `createXML` then `defineXML`,
/// upstream's `_gdata`/`Guest` object having a stable `.uuid` from the
/// start) — without a fixed UUID, each call would get its own
/// libvirt-generated one, and the second call fails outright
/// ("domain already exists with uuid ..." — hit this for real while
/// writing this function's own test, not a theoretical concern).
#[allow(clippy::too_many_arguments)]
pub fn build_import_document(
    name: &str,
    memory_kib: u64,
    vcpus: u32,
    disk_path: &str,
    domain_type: &str,
    os_type: &str,
    arch: &str,
    machine: Option<&str>,
    emulator: Option<&str>,
    is_qemu: bool,
) -> Document {
    let (mut doc, _root, os_el, devices_el) = build_domain_skeleton(
        name,
        memory_kib,
        vcpus,
        &crate::cloner::generate_uuid(),
        domain_type,
        os_type,
        arch,
        machine,
        emulator,
    );
    push_boot_dev(&mut doc, os_el, "hd");

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
    push_default_nic(&mut doc, devices_el, &crate::cloner::generate_mac(is_qemu));

    doc
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn builds_a_complete_bootable_domain() {
        // Asserts a real (non-predictable) OUI prefix — see
        // `crate::cloner::PREDICTABLE_TEST_LOCK`'s doc comment.
        let _guard = crate::cloner::PREDICTABLE_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let doc = build_import_document(
            "imported-guest",
            2097152,
            2,
            "/var/lib/libvirt/images/imported.qcow2",
            "qemu",
            "hvm",
            "x86_64",
            Some("q35"),
            Some("/usr/bin/qemu-system-x86_64"),
            true,
        );
        let root = doc.root_element().unwrap();

        assert_eq!(root.attribute(&doc, "type"), Some("qemu"));
        assert!(Uuid::parse_str(&root.find(&doc, "uuid").unwrap().text_content(&doc)).is_ok());
        assert_eq!(
            root.find(&doc, "name").unwrap().text_content(&doc),
            "imported-guest"
        );
        let mem = root.find(&doc, "memory").unwrap();
        assert_eq!(mem.text_content(&doc), "2097152");
        assert_eq!(mem.attribute(&doc, "unit"), Some("KiB"));
        assert_eq!(
            root.find(&doc, "currentMemory").unwrap().text_content(&doc),
            "2097152"
        );
        assert_eq!(root.find(&doc, "vcpu").unwrap().text_content(&doc), "2");

        let os = root.find(&doc, "os").unwrap();
        let os_type = os.find(&doc, "type").unwrap();
        assert_eq!(os_type.text_content(&doc), "hvm");
        assert_eq!(os_type.attribute(&doc, "arch"), Some("x86_64"));
        assert_eq!(os_type.attribute(&doc, "machine"), Some("q35"));
        assert_eq!(
            os.find(&doc, "boot").unwrap().attribute(&doc, "dev"),
            Some("hd")
        );

        let devices = root.find(&doc, "devices").unwrap();
        assert_eq!(
            devices.find(&doc, "emulator").unwrap().text_content(&doc),
            "/usr/bin/qemu-system-x86_64"
        );
        let disk = devices.find(&doc, "disk").unwrap();
        assert_eq!(disk.attribute(&doc, "type"), Some("file"));
        assert_eq!(disk.attribute(&doc, "device"), Some("disk"));
        assert_eq!(
            disk.find(&doc, "source").unwrap().attribute(&doc, "file"),
            Some("/var/lib/libvirt/images/imported.qcow2")
        );
        let target = disk.find(&doc, "target").unwrap();
        assert_eq!(target.attribute(&doc, "dev"), Some("vda"));
        assert_eq!(target.attribute(&doc, "bus"), Some("virtio"));

        let iface = devices.find(&doc, "interface").unwrap();
        assert_eq!(iface.attribute(&doc, "type"), Some("network"));
        assert_eq!(
            iface
                .find(&doc, "source")
                .unwrap()
                .attribute(&doc, "network"),
            Some("default")
        );
        assert!(iface
            .find(&doc, "mac")
            .unwrap()
            .attribute(&doc, "address")
            .unwrap()
            .starts_with("52:54:00:"));
        assert_eq!(
            iface.find(&doc, "model").unwrap().attribute(&doc, "type"),
            Some("virtio")
        );
    }

    #[test]
    fn nic_oui_follows_is_qemu_not_domain_type() {
        // `builds_a_complete_bootable_domain` only exercises
        // `is_qemu == true` (the `52:54:00` OUI branch) — this pins the
        // other branch with `is_qemu == false`.
        let doc = build_import_document(
            "x",
            1048576,
            1,
            "/tmp/x.img",
            "test",
            "hvm",
            "i686",
            None,
            None,
            false,
        );
        let root = doc.root_element().unwrap();
        let mac = root
            .find(&doc, "devices")
            .unwrap()
            .find(&doc, "interface")
            .unwrap()
            .find(&doc, "mac")
            .unwrap()
            .attribute(&doc, "address")
            .unwrap()
            .to_string();
        assert!(
            mac.starts_with("00:16:3e:"),
            "is_qemu = false should get the other OUI, got {mac}"
        );
    }

    /// Regression for a real bug, found and fixed while continuing
    /// ticket 21: `domain_type` (`hypervisor_type`, e.g. `"kvm"` on a
    /// real accelerated `qemu:///system` — confirmed against this
    /// machine's own real capture, `domcaps.rs`'s `QEMU_DOMCAPS_
    /// EXCERPT`: `<domain>kvm</domain>`) is *not* the same thing as
    /// `is_qemu` (the connection's URI scheme, `Uri::is_qemu()`). Every
    /// builder used to derive `is_qemu` from `domain_type == "qemu"`
    /// (a literal string match) instead of taking it as its own
    /// parameter — meaning every real, non-`test:///` domain this
    /// crate ever built (`domain_type = "kvm"`, the common
    /// KVM-accelerated case) got the Xen OUI (`00:16:3e`) instead of
    /// qemu's (`52:54:00`), silently, since `test:///default`'s own
    /// domain type genuinely is the literal string `"test"` and never
    /// triggered it. `domain_type = "kvm"` with `is_qemu = true` must
    /// produce the qemu OUI.
    #[test]
    fn nic_oui_is_correct_for_a_real_kvm_accelerated_install() {
        let doc = build_import_document(
            "x",
            1048576,
            1,
            "/tmp/x.img",
            "kvm",
            "hvm",
            "x86_64",
            None,
            None,
            true,
        );
        let root = doc.root_element().unwrap();
        let mac = root
            .find(&doc, "devices")
            .unwrap()
            .find(&doc, "interface")
            .unwrap()
            .find(&doc, "mac")
            .unwrap()
            .attribute(&doc, "address")
            .unwrap()
            .to_string();
        assert!(
            mac.starts_with("52:54:00:"),
            "domain_type = \"kvm\" on a real qemu:// connection must still get qemu's OUI, got {mac}"
        );
    }

    /// `machine`/`emulator` are both optional (`test:///default`'s own
    /// real capabilities report no `<machine>` elements at all, per the
    /// pinned capture in `caps.rs` — `None` is the real, common case,
    /// not just a theoretical one) — `None` must omit the attribute/
    /// element entirely, not emit an empty one.
    #[test]
    fn machine_and_emulator_are_omitted_entirely_when_absent() {
        let doc = build_import_document(
            "x",
            1048576,
            1,
            "/tmp/x.img",
            "test",
            "hvm",
            "i686",
            None,
            None,
            false,
        );
        let root = doc.root_element().unwrap();
        let type_el = root.find(&doc, "os").unwrap().find(&doc, "type").unwrap();
        assert_eq!(type_el.attribute(&doc, "machine"), None);
        assert!(root
            .find(&doc, "devices")
            .unwrap()
            .find(&doc, "emulator")
            .is_none());
    }

    #[test]
    fn serializes_and_reparses_cleanly() {
        let doc = build_import_document(
            "x",
            1048576,
            1,
            "/tmp/x.img",
            "test",
            "hvm",
            "i686",
            None,
            None,
            false,
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

    /// Real end-to-end proof against `test:///default`: fetch the
    /// connection's own real `hypervisor_type`/`arch`/`machine`/
    /// `emulator` via `Capabilities::guest_lookup` (not hardcoded —
    /// `test:///`'s arch is `i686`, confirmed by probing it directly,
    /// not assumed; its `<guest>` reports no `<machine>` elements at
    /// all, so `machine` is a real `None` here, not a placeholder —
    /// see `machine_and_emulator_are_omitted_entirely_when_absent`),
    /// build an import document, then do upstream's actual two-call
    /// default sequence (`create_new` — starts it running, transient —
    /// then `define_new` — persists it), matching `Installer._create_
    /// guest` exactly. Also proves the real driver accepts a domain
    /// defined with the new `<devices><emulator>` element (ticket 21
    /// slice (a)) — not just that this crate's own XML construction
    /// looks right. Confirms both effects landed, then cleans up:
    /// destroy the transient/running instance, undefine the persistent
    /// config.
    #[test]
    fn import_creates_a_running_and_persisted_domain() {
        use crate::capabilities::Capabilities;
        use crate::connection::{Connection, Domain};
        use crate::test_support::TEST_DOMAIN_LOCK;

        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        let host_caps = Capabilities::fetch(&conn).expect("host caps fetch succeeds");
        let capsinfo = host_caps
            .guest_lookup(None, None, None, None)
            .expect("test driver always resolves a default guest");
        let domain_type = capsinfo
            .hypervisor_type
            .expect("test driver reports a domain type");
        let arch = capsinfo.arch.expect("test driver reports a default arch");
        let machine = capsinfo.machines.first().map(|s| s.as_str());
        assert_eq!(
            machine, None,
            "test:///default's own real capabilities report no machines at all"
        );

        let doc = build_import_document(
            "import-end-to-end",
            1048576,
            1,
            "/guest/import-end-to-end.img",
            &domain_type,
            "hvm",
            &arch,
            machine,
            capsinfo.emulator.as_deref(),
            conn.uri().is_qemu(),
        );
        let xml = doc.write_str().expect("serializes");

        let running = Domain::create_new(&conn, &xml).expect("transient create succeeds");
        assert!(running.is_active().expect("query succeeds"));

        let persisted = Domain::define_new(&conn, &xml).expect("define succeeds");
        let refetched = Domain::lookup_by_name(&conn, "import-end-to-end")
            .expect("the domain is findable by the name we gave it");
        assert_eq!(refetched.name().unwrap(), "import-end-to-end");
        let inactive_xml = refetched.inactive_xml().expect("fetch inactive XML");
        assert!(inactive_xml.contains("import-end-to-end.img"));
        assert!(
            inactive_xml.contains("<emulator>"),
            "the real driver kept the <emulator> element we defined it with"
        );

        // The actual point of the two-call sequence: destroy the
        // running instance and the *config* must still be there.
        // Without `define_new`, this domain would vanish entirely on
        // destroy (transient means exactly that) — this is the
        // assertion that fails if a future refactor drops the second
        // call.
        running
            .destroy()
            .expect("cleanup: destroy the transient instance");
        let still_defined = Domain::lookup_by_name(&conn, "import-end-to-end")
            .expect("persistent config survives destroying the running instance");
        assert!(!still_defined.is_active().expect("query succeeds"));

        persisted
            .undefine()
            .expect("cleanup: undefine the persistent config");
    }

    /// Closes the gap an advisor review flagged on ticket 20 piece (b):
    /// every builder test above pins a literal `"hvm"` for `os_type`, so
    /// none of them exercise the actual composition `virt-install.rs`
    /// now runs — `Capabilities::fetch` → `guest_lookup` →
    /// `capsinfo.os_type` → `build_import_document`. This test runs
    /// that exact sequence with `VIRTINST_TEST_CAPS_FILE` pointed at a
    /// fixture whose only guest reports `os_type=xen` (`test:///
    /// default`'s real capabilities never do — every real capture in
    /// this crate is `hvm`), so a `"hvm"` showing up in the output could
    /// only mean the injected value was never actually threaded through
    /// (e.g. a stray hardcoded literal reintroduced upstream of this
    /// call, the same class of bug the advisor was checking for).
    #[test]
    fn capsinfo_os_type_actually_flows_into_the_built_document() {
        use crate::capabilities::Capabilities;
        use crate::connection::Connection;
        use crate::test_support::TEST_DOMAIN_LOCK;

        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let fixture = r#"<capabilities>
  <host><cpu><arch>i686</arch></cpu></host>
  <guest>
    <os_type>xen</os_type>
    <arch name='i686'>
      <wordsize>32</wordsize>
      <emulator>/usr/bin/test-hv</emulator>
      <domain type='test'/>
    </arch>
  </guest>
</capabilities>"#;
        let fixture_path = std::env::temp_dir().join(format!(
            "virtinst-core-test-caps-{}.xml",
            std::process::id()
        ));
        std::fs::write(&fixture_path, fixture).expect("writes the fixture");
        std::env::set_var("VIRTINST_TEST_CAPS_FILE", &fixture_path);

        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        let host_caps =
            Capabilities::fetch(&conn).expect("reads the injected fixture, not libvirt");
        let capsinfo = host_caps
            .guest_lookup(None, None, None, None)
            .expect("the fixture's one guest matches");

        std::env::remove_var("VIRTINST_TEST_CAPS_FILE");
        std::fs::remove_file(&fixture_path).ok();

        assert_eq!(
            capsinfo.os_type.as_deref(),
            Some("xen"),
            "guest_lookup must read the injected fixture, not the real test:/// driver's hvm guest"
        );

        let doc = build_import_document(
            "capsinfo-os-type-flow-check",
            1048576,
            1,
            "/guest/whatever.img",
            &capsinfo.hypervisor_type.unwrap(),
            &capsinfo.os_type.unwrap(),
            &capsinfo.arch.unwrap(),
            capsinfo.machines.first().map(|s| s.as_str()),
            capsinfo.emulator.as_deref(),
            false,
        );
        // Through the DOM, not a raw string match on the serialized
        // XML — a string match couples this test to `virtinst_xml`'s
        // writer's attribute ordering/quote style, neither of which
        // this test is about.
        let root = doc.root_element().unwrap();
        let type_el = root.find(&doc, "os").unwrap().find(&doc, "type").unwrap();
        assert_eq!(
            type_el.text_content(&doc),
            "xen",
            "the injected os_type must reach <os><type>, not a stray hardcoded \"hvm\""
        );
        assert_eq!(type_el.attribute(&doc, "arch"), Some("i686"));
    }

    /// Same gap, closed for `machine`/`emulator` (ticket 21 slice (a)):
    /// `builds_a_complete_bootable_domain` pins a literal `Some("q35")`
    /// and the real end-to-end test's driver reports no machines at
    /// all, so neither exercises the actual composition `virt-
    /// install.rs` runs — `capsinfo.machines.first()` → `<os><type
    /// machine=...>`. This fixture has a real, *populated* multi-
    /// machine list (trimmed from the same real `virsh -c qemu:///
    /// system capabilities` capture as `caps.rs`'s own `REAL_QEMU_
    /// SYSTEM_CAPS_EXCERPT`), so `.first()` returns a real, non-trivial
    /// value to check, not `None`.
    ///
    /// Confirmed directly against upstream's own `capabilities.py`
    /// (`all_machine_names`: `ret.append(m.name)` per `<machine>` in
    /// document order, canonical alias appended only if not already
    /// present) before writing this: for this exact machine-list shape,
    /// **upstream's own fallback would also land on `pc-i440fx-11.1`**,
    /// the first `<machine>`'s own name, not the friendlier `pc`
    /// canonical alias two entries later — this isn't a Rust-side
    /// divergence to fix, it's upstream's real, faithfully-ported
    /// behavior on real capabilities data.
    #[test]
    fn capsinfo_machine_fallback_actually_flows_into_the_built_document() {
        use crate::capabilities::Capabilities;
        use crate::connection::Connection;
        use crate::test_support::TEST_DOMAIN_LOCK;

        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let fixture = r#"<capabilities>
  <host><cpu><arch>x86_64</arch></cpu></host>
  <guest>
    <os_type>hvm</os_type>
    <arch name='x86_64'>
      <wordsize>64</wordsize>
      <emulator>/usr/bin/qemu-system-x86_64</emulator>
      <machine maxCpus='255'>pc-i440fx-11.1</machine>
      <machine canonical='pc-i440fx-11.1' maxCpus='255'>pc</machine>
      <machine maxCpus='4096'>pc-q35-11.1</machine>
      <machine canonical='pc-q35-11.1' maxCpus='4096'>q35</machine>
      <domain type='qemu'/>
      <domain type='kvm'/>
    </arch>
  </guest>
</capabilities>"#;
        let fixture_path = std::env::temp_dir().join(format!(
            "virtinst-core-test-machine-caps-{}.xml",
            std::process::id()
        ));
        std::fs::write(&fixture_path, fixture).expect("writes the fixture");
        std::env::set_var("VIRTINST_TEST_CAPS_FILE", &fixture_path);

        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        let host_caps =
            Capabilities::fetch(&conn).expect("reads the injected fixture, not libvirt");
        let capsinfo = host_caps
            .guest_lookup(None, None, None, None)
            .expect("the fixture's one guest matches");

        std::env::remove_var("VIRTINST_TEST_CAPS_FILE");
        std::fs::remove_file(&fixture_path).ok();

        assert_eq!(
            capsinfo.machines.first().map(|s| s.as_str()),
            Some("pc-i440fx-11.1"),
            "guest_lookup must read the injected fixture's real machine list, not test:/// \
             default's empty one"
        );

        let doc = build_import_document(
            "capsinfo-machine-flow-check",
            1048576,
            1,
            "/guest/whatever.img",
            &capsinfo.hypervisor_type.unwrap(),
            "hvm",
            &capsinfo.arch.unwrap(),
            capsinfo.machines.first().map(|s| s.as_str()),
            capsinfo.emulator.as_deref(),
            true,
        );
        let root = doc.root_element().unwrap();
        let type_el = root.find(&doc, "os").unwrap().find(&doc, "type").unwrap();
        assert_eq!(
            type_el.attribute(&doc, "machine"),
            Some("pc-i440fx-11.1"),
            "the injected machine fallback must reach <os><type machine=...>"
        );
        assert_eq!(
            root.find(&doc, "devices")
                .unwrap()
                .find(&doc, "emulator")
                .unwrap()
                .text_content(&doc),
            "/usr/bin/qemu-system-x86_64"
        );
    }

    /// The actual regression test for the NIC-OUI bug: every prior test
    /// in this file that exercises `is_qemu` passes it as a literal
    /// (`true`/`false`/a real `conn.uri().is_qemu()` on `test:///`,
    /// which is always `false`), so none of them can catch a wrong
    /// *derivation* of `is_qemu` — only a wrong *consumption* of it.
    /// The bug was in the derivation: `is_qemu` used to come from
    /// `hypervisor_type == "qemu"`, a string match that's `false` for a
    /// real KVM-accelerated connection (`hypervisor_type` there is the
    /// literal string `"kvm"`, confirmed against `domcaps.rs`'s own
    /// `QEMU_DOMCAPS_EXCERPT`). Upstream's real check
    /// (`interface.py:30`, `conn.is_qemu()`) keys on the connection's
    /// URI scheme instead, which is what `Uri::is_qemu()` gives.
    ///
    /// This test reproduces that exact precondition by combining both
    /// env-var injection points at once: `VIRTINST_TEST_FAKE_URI` makes
    /// `conn.uri()` report a real `qemu:///system` URI while still
    /// physically opening `test:///default`, and
    /// `VIRTINST_TEST_CAPS_FILE` makes the fetched capabilities report
    /// `hypervisor_type = "kvm"` — the same "URI says qemu, driver name
    /// says kvm" shape a real accelerated install hits. It asserts both
    /// facts hold simultaneously (the bug's actual precondition, which
    /// nothing else in this suite states), then checks the built
    /// document's MAC to prove the fix threads `conn.uri().is_qemu()`,
    /// not `hypervisor_type`, into `generate_mac`. Before the fix this
    /// test fails (MAC gets the Xen OUI); after, it passes.
    #[test]
    fn nic_oui_is_correct_when_domain_type_is_kvm_on_a_real_qemu_connection() {
        use crate::capabilities::Capabilities;
        use crate::connection::Connection;
        use crate::test_support::TEST_DOMAIN_LOCK;

        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let fixture = r#"<capabilities>
  <host><cpu><arch>x86_64</arch></cpu></host>
  <guest>
    <os_type>hvm</os_type>
    <arch name='x86_64'>
      <wordsize>64</wordsize>
      <emulator>/usr/bin/qemu-system-x86_64</emulator>
      <domain type='kvm'/>
    </arch>
  </guest>
</capabilities>"#;
        let fixture_path = std::env::temp_dir().join(format!(
            "virtinst-core-test-is-qemu-caps-{}.xml",
            std::process::id()
        ));
        std::fs::write(&fixture_path, fixture).expect("writes the fixture");
        std::env::set_var("VIRTINST_TEST_CAPS_FILE", &fixture_path);
        std::env::set_var("VIRTINST_TEST_FAKE_URI", "qemu:///system");

        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        let host_caps =
            Capabilities::fetch(&conn).expect("reads the injected fixture, not libvirt");
        let capsinfo = host_caps
            .guest_lookup(None, None, None, None)
            .expect("the fixture's one guest matches");

        std::env::remove_var("VIRTINST_TEST_CAPS_FILE");
        std::env::remove_var("VIRTINST_TEST_FAKE_URI");
        std::fs::remove_file(&fixture_path).ok();

        // The bug's actual precondition: a driver-reported hypervisor
        // type of "kvm" on a connection whose URI scheme is qemu.
        assert_eq!(capsinfo.hypervisor_type.as_deref(), Some("kvm"));
        assert!(
            conn.uri().is_qemu(),
            "the fake URI must make the connection report itself as qemu"
        );

        let doc = build_import_document(
            "is-qemu-flow-check",
            1048576,
            1,
            "/guest/whatever.img",
            &capsinfo.hypervisor_type.unwrap(),
            "hvm",
            &capsinfo.arch.unwrap(),
            None,
            capsinfo.emulator.as_deref(),
            conn.uri().is_qemu(),
        );
        let root = doc.root_element().unwrap();
        let mac = root
            .find(&doc, "devices")
            .unwrap()
            .find(&doc, "interface")
            .unwrap()
            .find(&doc, "mac")
            .unwrap()
            .attribute(&doc, "address")
            .unwrap()
            .to_string();
        assert!(
            mac.starts_with("52:54:00:"),
            "hypervisor_type = \"kvm\" on a qemu:// connection must still get qemu's OUI \
             (derived from the URI scheme, not hypervisor_type), got {mac}"
        );
    }
}
