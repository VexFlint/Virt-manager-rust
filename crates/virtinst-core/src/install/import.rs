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
/// (plus `loader`/`emulator`/`machine`, not modeled yet) from `guest.
/// set_capabilities_defaults()`, and hardcoding any of them would be a
/// real, connection-specific-shaped divergence (`test:///` defaults to
/// `i686`, a real `qemu:///system` to `x86_64`).
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
pub fn build_import_document(
    name: &str,
    memory_kib: u64,
    vcpus: u32,
    disk_path: &str,
    domain_type: &str,
    os_type: &str,
    arch: &str,
) -> Document {
    let (mut doc, root, os_el) = build_domain_skeleton(
        name,
        memory_kib,
        vcpus,
        &crate::cloner::generate_uuid(),
        domain_type,
        os_type,
        arch,
    );
    push_boot_dev(&mut doc, os_el, "hd");

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
    push_default_nic(
        &mut doc,
        devices_el,
        &crate::cloner::generate_mac(domain_type == "qemu"),
    );
    root.push_child(&mut doc, devices_el)
        .expect("a freshly-created detached element always attaches cleanly");

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
        assert_eq!(
            os.find(&doc, "boot").unwrap().attribute(&doc, "dev"),
            Some("hd")
        );

        let devices = root.find(&doc, "devices").unwrap();
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
    fn nic_oui_follows_domain_type_not_just_qemu() {
        // `builds_a_complete_bootable_domain` only exercises
        // `domain_type == "qemu"` (the `52:54:00` OUI branch) — this
        // pins the other branch (`push_default_nic`'s `domain_type ==
        // "qemu"` proxy for `cloner::generate_mac`'s `is_qemu`), so a
        // broken proxy wouldn't go unnoticed.
        let doc = build_import_document("x", 1048576, 1, "/tmp/x.img", "test", "hvm", "i686");
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
            "non-qemu domain_type should get the other OUI, got {mac}"
        );
    }

    #[test]
    fn serializes_and_reparses_cleanly() {
        let doc = build_import_document("x", 1048576, 1, "/tmp/x.img", "test", "hvm", "i686");
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
    /// connection's own default `domain`/`arch` from
    /// `DomainCapabilities` (not hardcoded — `test:///`'s is `i686`,
    /// confirmed by probing it directly, not assumed), build an import
    /// document, then do upstream's actual two-call default sequence
    /// (`create_new` — starts it running, transient — then
    /// `define_new` — persists it), matching `Installer._create_guest`
    /// exactly. Confirms both effects landed, then cleans up: destroy
    /// the transient/running instance, undefine the persistent config.
    #[test]
    fn import_creates_a_running_and_persisted_domain() {
        use crate::capabilities::DomainCapabilities;
        use crate::connection::{Connection, Domain};
        use crate::test_support::TEST_DOMAIN_LOCK;

        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        let caps = DomainCapabilities::fetch(&conn, None, None, None, None)
            .expect("domcaps fetch succeeds");
        let domain_type = caps.domain.expect("test driver reports a domain type");
        let arch = caps.arch.expect("test driver reports a default arch");

        let doc = build_import_document(
            "import-end-to-end",
            1048576,
            1,
            "/guest/import-end-to-end.img",
            &domain_type,
            "hvm",
            &arch,
        );
        let xml = doc.write_str().expect("serializes");

        let running = Domain::create_new(&conn, &xml).expect("transient create succeeds");
        assert!(running.is_active().expect("query succeeds"));

        let persisted = Domain::define_new(&conn, &xml).expect("define succeeds");
        let refetched = Domain::lookup_by_name(&conn, "import-end-to-end")
            .expect("the domain is findable by the name we gave it");
        assert_eq!(refetched.name().unwrap(), "import-end-to-end");
        assert!(refetched
            .inactive_xml()
            .expect("fetch inactive XML")
            .contains("import-end-to-end.img"));

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
        );
        let xml = doc.write_str().expect("serializes");
        assert!(
            xml.contains("<type arch=\"i686\">xen</type>"),
            "the injected os_type must reach <os><type>, not a stray hardcoded \"hvm\": {xml}"
        );
    }
}
