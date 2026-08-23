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
//! - The disk's `type="file"`/`device="disk"` shape is fixed, but its
//!   `bus`/target `dev` prefix are no longer a hardcoded `"virtio"`/
//!   `"vda"` — `disk_bus` is the caller's already-resolved
//!   `domain_skeleton::default_disk_bus` answer (upstream's
//!   `guest.set_defaults()`/`disk.py`'s `_default_bus`, ported), see
//!   that function's own doc comment for the decision tree and why
//!   `virt-install.rs` resolves it rather than this function computing
//!   it internally.
//! - No storage validation (`DeviceDisk.path_definitely_exists`) —
//!   upstream requires the import path already exist; this crate
//!   doesn't check, matching every other field in this module (a
//!   caller concern, not baked into construction).
//! - `memory_kib`/`vcpus` are whatever the caller passes, no default —
//!   upstream's own `--memory`/`--vcpus` are optional too, but fall
//!   back to `osinfo`'s recommended-resources lookup when absent
//!   (`get_recommended_ram`/`get_recommended_ncpus`); `virt-install`
//!   here just requires `--memory` and defaults `--vcpus` to a flat
//!   `1` — `osinfo::OsInfo` exists now (ticket 04) but doesn't have a
//!   `get_recommended_resources()`-equivalent query yet, a real gap
//!   distinct from `OsInfo` not existing at all.
//!
//! Always gets one default NIC (`domain_skeleton::push_default_nic`) —
//! upstream's own CLI default whenever `--network`/`--nonetworks` are
//! both absent, not something specific to this install method.

use virtinst_xml::Document;

use crate::capabilities::DefaultCpuMode;

use super::domain_skeleton::{
    build_domain_skeleton, push_boot_dev, push_default_clock, push_default_cpu,
    push_default_graphics, push_default_nic, push_disk, GuestCapsDefaults,
};

/// `name`/`memory_kib`/`vcpus` are the wizard's Name/Memory pages'
/// worth of data — not `InstallMethod`'s job to carry (ticket 08 keeps
/// them off the enum entirely, see its own doc comment). `caps` bundles
/// the rest — see `GuestCapsDefaults`'s own doc comment for what each
/// field is and why they travel together; `domain_type`/`os_type`/
/// `arch` come from the connection's own host `capabilities::
/// Capabilities::guest_lookup` rather than being guessed (upstream gets
/// all three, plus `loader`, not modeled yet, from `guest.
/// set_capabilities_defaults()`), and hardcoding any of them would be a
/// real, connection-specific-shaped divergence (`test:///` defaults to
/// `i686`, a real `qemu:///system` to `x86_64`). `graphics_type`
/// (`"spice"`/`"vnc"`) is ticket 21 slice (c) — caller-resolved via
/// `DomainCapabilities::default_graphics_type`, see `push_default_
/// graphics`'s doc comment for the XML it produces. The `<clock>`
/// timer trio (ticket 21 slice (d)) needs no separate parameter at
/// all — `push_default_clock` self-gates on `caps.os_type`/`caps.arch`/
/// `caps.is_qemu`, already in hand. `cpu_mode` (ticket 21 slice (f)) is
/// the opposite shape from `<clock>`: it's the caller's already-
/// resolved [`DefaultCpuMode`] (or `None`), the same "resolve once,
/// pass the answer in" pattern `graphics_type` uses — see `push_
/// default_cpu`'s doc comment for why this one can't self-gate on
/// primitives already in hand the way `<clock>` does. `clock_offset`
/// is `osinfo::OsInfo::clock_offset()`'s result (or `"utc"` when no
/// `--os-variant` was given — see `push_default_clock`'s own doc
/// comment for why that's the right no-OS-selected default, not a
/// placeholder): unlike `os_type`/`arch`/`is_qemu`, `<clock>` itself
/// can't derive this from anything already in `caps`, so it travels
/// as its own parameter even though the rest of `<clock>` self-gates.
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
    caps: GuestCapsDefaults,
    graphics_type: &str,
    cpu_mode: Option<&DefaultCpuMode>,
    clock_offset: &str,
    disk_bus: &str,
) -> Document {
    let (mut doc, root, os_el, devices_el) = build_domain_skeleton(
        name,
        memory_kib,
        vcpus,
        &crate::cloner::generate_uuid(),
        &caps,
    );
    push_default_clock(
        &mut doc,
        root,
        caps.os_type,
        caps.arch,
        caps.is_qemu,
        clock_offset,
    );
    push_default_cpu(&mut doc, root, cpu_mode);
    push_boot_dev(&mut doc, os_el, "hd");

    push_disk(&mut doc, devices_el, disk_path, disk_bus);
    push_default_nic(
        &mut doc,
        devices_el,
        &crate::cloner::generate_mac(caps.is_qemu),
    );
    push_default_graphics(&mut doc, devices_el, graphics_type);

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
            GuestCapsDefaults {
                domain_type: "qemu",
                os_type: "hvm",
                arch: "x86_64",
                machine: Some("q35"),
                emulator: Some("/usr/bin/qemu-system-x86_64"),
                is_qemu: true,
            },
            "spice",
            None,
            "utc",
            "virtio",
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

        // Ticket 21 slice (c): a default `<graphics>` element every
        // builder gets now, spice's full port/tlsPort/autoport trio.
        let graphics = devices.find(&doc, "graphics").unwrap();
        assert_eq!(graphics.attribute(&doc, "type"), Some("spice"));
        assert_eq!(graphics.attribute(&doc, "port"), Some("-1"));
        assert_eq!(graphics.attribute(&doc, "tlsPort"), Some("-1"));
        assert_eq!(graphics.attribute(&doc, "autoport"), Some("yes"));

        // Ticket 21 slice (d)+(f): the x86+qemu clock timer trio, plus
        // the offset attribute ticket 04's real OsInfo bindings
        // unblocked -- "utc" here since this test passes it as a
        // literal, matching "no --os-variant selected".
        let clock = root.find(&doc, "clock").unwrap();
        assert_eq!(clock.attribute(&doc, "offset"), Some("utc"));
        let timers = clock.find_all(&doc, "timer");
        assert_eq!(timers.len(), 3);
        let rtc = timers
            .iter()
            .find(|t| t.attribute(&doc, "name") == Some("rtc"))
            .unwrap();
        assert_eq!(rtc.attribute(&doc, "tickpolicy"), Some("catchup"));
        let pit = timers
            .iter()
            .find(|t| t.attribute(&doc, "name") == Some("pit"))
            .unwrap();
        assert_eq!(pit.attribute(&doc, "tickpolicy"), Some("delay"));
        let hpet = timers
            .iter()
            .find(|t| t.attribute(&doc, "name") == Some("hpet"))
            .unwrap();
        assert_eq!(hpet.attribute(&doc, "present"), Some("no"));
    }

    /// `push_default_clock`'s `is_qemu` gate: reuses the same fixture
    /// `nic_oui_follows_is_qemu_not_domain_type` pins (`os_type = "hvm"`,
    /// `arch = "i686"` — still x86, per upstream's own `is_x86()` — but
    /// `is_qemu = false`), so a `<clock>` showing up here could only mean
    /// the `is_qemu` check was dropped or inverted.
    #[test]
    fn clock_is_omitted_when_the_connection_is_not_qemu() {
        let doc = build_import_document(
            "x",
            1048576,
            1,
            "/tmp/x.img",
            GuestCapsDefaults {
                domain_type: "test",
                os_type: "hvm",
                arch: "i686",
                machine: None,
                emulator: None,
                is_qemu: false,
            },
            "spice",
            None,
            "utc",
            "virtio",
        );
        let root = doc.root_element().unwrap();
        assert!(root.find(&doc, "clock").is_none());
    }

    /// `push_default_clock`'s arch gate: `is_qemu = true` and
    /// `os_type = "hvm"` alone aren't enough — upstream's `is_x86()`
    /// (`x86_64`/`i686` only) still has to hold.
    #[test]
    fn clock_is_omitted_for_a_non_x86_arch() {
        let doc = build_import_document(
            "x",
            1048576,
            1,
            "/tmp/x.img",
            GuestCapsDefaults {
                domain_type: "qemu",
                os_type: "hvm",
                arch: "aarch64",
                machine: None,
                emulator: None,
                is_qemu: true,
            },
            "spice",
            None,
            "utc",
            "virtio",
        );
        let root = doc.root_element().unwrap();
        assert!(root.find(&doc, "clock").is_none());
    }

    /// Ticket 21 slice (f) follow-up: `clock_offset` threads through
    /// to the emitted `<clock offset=...>` attribute -- a literal
    /// `"localtime"` here proves the plumbing, not `OsInfo::
    /// clock_offset()` itself (already proven directly against real
    /// osinfo-db entries in `osinfo::mod::tests`).
    #[test]
    fn clock_offset_threads_through_to_the_emitted_attribute() {
        let doc = build_import_document(
            "x",
            1048576,
            1,
            "/tmp/x.img",
            GuestCapsDefaults {
                domain_type: "qemu",
                os_type: "hvm",
                arch: "x86_64",
                machine: None,
                emulator: None,
                is_qemu: true,
            },
            "spice",
            None,
            "localtime",
            "virtio",
        );
        let root = doc.root_element().unwrap();
        let clock = root.find(&doc, "clock").unwrap();
        assert_eq!(clock.attribute(&doc, "offset"), Some("localtime"));
    }

    /// `push_default_graphics`'s other branch: `vnc` gets `port="-1"`
    /// alone — no `tlsPort`, no `autoport` (upstream's `_get_default_
    /// autoport` deliberately withholds it for vnc, "to maintain back
    /// compat with old libvirt that didn't support 'autoport'").
    #[test]
    fn vnc_graphics_gets_only_a_port_no_tls_port_or_autoport() {
        let doc = build_import_document(
            "x",
            1048576,
            1,
            "/tmp/x.img",
            GuestCapsDefaults {
                domain_type: "test",
                os_type: "hvm",
                arch: "i686",
                machine: None,
                emulator: None,
                is_qemu: false,
            },
            "vnc",
            None,
            "utc",
            "virtio",
        );
        let root = doc.root_element().unwrap();
        let graphics = root
            .find(&doc, "devices")
            .unwrap()
            .find(&doc, "graphics")
            .unwrap();
        assert_eq!(graphics.attribute(&doc, "type"), Some("vnc"));
        assert_eq!(graphics.attribute(&doc, "port"), Some("-1"));
        assert_eq!(graphics.attribute(&doc, "tlsPort"), None);
        assert_eq!(graphics.attribute(&doc, "autoport"), None);
    }

    /// Ticket 21 slice (f): `cpu_mode = None` (the default for every
    /// other test in this module) emits no `<cpu>` element at all —
    /// the resolution itself is `domcaps.rs`'s own job, this only
    /// proves `push_default_cpu` doesn't invent one out of nothing.
    #[test]
    fn cpu_is_omitted_when_no_default_mode_was_resolved() {
        let doc = build_import_document(
            "x",
            1048576,
            1,
            "/tmp/x.img",
            GuestCapsDefaults {
                domain_type: "test",
                os_type: "hvm",
                arch: "x86_64",
                machine: None,
                emulator: None,
                is_qemu: false,
            },
            "spice",
            None,
            "utc",
            "virtio",
        );
        let root = doc.root_element().unwrap();
        assert!(root.find(&doc, "cpu").is_none());
    }

    /// `DefaultCpuMode::Mode` — a bare `<cpu mode="...">`, no other
    /// attributes or children.
    #[test]
    fn cpu_mode_variant_emits_a_bare_mode_attribute() {
        let resolved = DefaultCpuMode::Mode("host-passthrough");
        let doc = build_import_document(
            "x",
            1048576,
            1,
            "/tmp/x.img",
            GuestCapsDefaults {
                domain_type: "kvm",
                os_type: "hvm",
                arch: "x86_64",
                machine: None,
                emulator: None,
                is_qemu: true,
            },
            "spice",
            Some(&resolved),
            "utc",
            "virtio",
        );
        let root = doc.root_element().unwrap();
        let cpu = root.find(&doc, "cpu").unwrap();
        assert_eq!(cpu.attribute(&doc, "mode"), Some("host-passthrough"));
        assert_eq!(cpu.attribute(&doc, "match"), None);
        assert!(cpu.find(&doc, "model").is_none());
    }

    /// `DefaultCpuMode::Custom { model: Some(_) }` — `mode="custom"`,
    /// `match="exact"`, and a `<model>` text child.
    #[test]
    fn cpu_custom_variant_with_a_model_emits_match_exact_and_the_model_element() {
        let resolved = DefaultCpuMode::Custom {
            model: Some("Skylake-Client-IBRS".to_string()),
        };
        let doc = build_import_document(
            "x",
            1048576,
            1,
            "/tmp/x.img",
            GuestCapsDefaults {
                domain_type: "kvm",
                os_type: "hvm",
                arch: "x86_64",
                machine: None,
                emulator: None,
                is_qemu: true,
            },
            "spice",
            Some(&resolved),
            "utc",
            "virtio",
        );
        let root = doc.root_element().unwrap();
        let cpu = root.find(&doc, "cpu").unwrap();
        assert_eq!(cpu.attribute(&doc, "mode"), Some("custom"));
        assert_eq!(cpu.attribute(&doc, "match"), Some("exact"));
        assert_eq!(
            cpu.find(&doc, "model").unwrap().text_content(&doc),
            "Skylake-Client-IBRS"
        );
    }

    /// `DefaultCpuMode::Custom { model: None }` — upstream's `_validate_
    /// default_host_model_only` logic stripping an unusable model still
    /// leaves `mode="custom" match="exact"` standing with no `<model>`
    /// child (applied unconditionally here rather than gated on a
    /// GUI-only setting this crate has no equivalent for — see
    /// `default_cpu_mode`'s own doc comment).
    #[test]
    fn cpu_custom_variant_with_no_model_emits_custom_mode_alone() {
        let resolved = DefaultCpuMode::Custom { model: None };
        let doc = build_import_document(
            "x",
            1048576,
            1,
            "/tmp/x.img",
            GuestCapsDefaults {
                domain_type: "kvm",
                os_type: "hvm",
                arch: "x86_64",
                machine: None,
                emulator: None,
                is_qemu: true,
            },
            "spice",
            Some(&resolved),
            "utc",
            "virtio",
        );
        let root = doc.root_element().unwrap();
        let cpu = root.find(&doc, "cpu").unwrap();
        assert_eq!(cpu.attribute(&doc, "mode"), Some("custom"));
        assert_eq!(cpu.attribute(&doc, "match"), Some("exact"));
        assert!(cpu.find(&doc, "model").is_none());
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
            GuestCapsDefaults {
                domain_type: "test",
                os_type: "hvm",
                arch: "i686",
                machine: None,
                emulator: None,
                is_qemu: false,
            },
            "spice",
            None,
            "utc",
            "virtio",
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
            GuestCapsDefaults {
                domain_type: "kvm",
                os_type: "hvm",
                arch: "x86_64",
                machine: None,
                emulator: None,
                is_qemu: true,
            },
            "spice",
            None,
            "utc",
            "virtio",
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
            GuestCapsDefaults {
                domain_type: "test",
                os_type: "hvm",
                arch: "i686",
                machine: None,
                emulator: None,
                is_qemu: false,
            },
            "spice",
            None,
            "utc",
            "virtio",
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
            GuestCapsDefaults {
                domain_type: "test",
                os_type: "hvm",
                arch: "i686",
                machine: None,
                emulator: None,
                is_qemu: false,
            },
            "spice",
            None,
            "utc",
            "virtio",
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
            GuestCapsDefaults {
                domain_type: &domain_type,
                os_type: "hvm",
                arch: &arch,
                machine,
                emulator: capsinfo.emulator.as_deref(),
                is_qemu: conn.uri().is_qemu(),
            },
            "spice",
            None,
            "utc",
            "virtio",
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
        assert!(
            inactive_xml.contains("type='spice'"),
            "the real driver kept the <graphics type='spice'> element we defined it with"
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
            GuestCapsDefaults {
                domain_type: &capsinfo.hypervisor_type.unwrap(),
                os_type: &capsinfo.os_type.unwrap(),
                arch: &capsinfo.arch.unwrap(),
                machine: capsinfo.machines.first().map(|s| s.as_str()),
                emulator: capsinfo.emulator.as_deref(),
                is_qemu: false,
            },
            "spice",
            None,
            "utc",
            "virtio",
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
            GuestCapsDefaults {
                domain_type: &capsinfo.hypervisor_type.unwrap(),
                os_type: "hvm",
                arch: &capsinfo.arch.unwrap(),
                machine: capsinfo.machines.first().map(|s| s.as_str()),
                emulator: capsinfo.emulator.as_deref(),
                is_qemu: true,
            },
            "spice",
            None,
            "utc",
            "virtio",
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
            GuestCapsDefaults {
                domain_type: &capsinfo.hypervisor_type.unwrap(),
                os_type: "hvm",
                arch: &capsinfo.arch.unwrap(),
                machine: None,
                emulator: capsinfo.emulator.as_deref(),
                is_qemu: conn.uri().is_qemu(),
            },
            "spice",
            None,
            "utc",
            "virtio",
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

    /// A real gap `push_default_clock`'s own gate creates: `is_qemu`
    /// requires a real `qemu://`-scheme connection, but every one of
    /// this module's other real end-to-end tests runs against
    /// `test:///default`, where `conn.uri().is_qemu()` is always
    /// `false` — so none of them ever actually ask the real libvirt
    /// driver to accept a `<clock>` with this trio at all, only this
    /// crate's own serialization/DOM-construction path. Closes that gap
    /// with the same `VIRTINST_TEST_FAKE_URI` technique the NIC-OUI
    /// composition test above uses: physically open `test:///default`,
    /// but make the connection report itself as `qemu:///system` so
    /// `is_qemu` really is `true`, then run the real `create_new`/
    /// `define_new` sequence and check the refetched `inactive_xml`
    /// kept the trio — not just that this crate's own writer produced
    /// it.
    #[test]
    fn clock_trio_survives_a_real_libvirt_define_when_is_qemu_is_true() {
        use crate::connection::{Connection, Domain};
        use crate::test_support::TEST_DOMAIN_LOCK;

        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        // `Connection::open` reads `VIRTINST_TEST_FAKE_URI` once and
        // stores the parsed `Uri` (see `connection.rs`'s doc comment on
        // that env var) — safe to remove it right after `open` returns,
        // both `conn.uri()` calls below read the already-stored value,
        // not the environment. The `assert!` right after `open` is the
        // actual proof of that, not just an aside: if `uri()` re-derived
        // from the environment instead, this assert would already be
        // `false` here (right after removal), and the test would fail
        // loudly rather than silently building a clock-less document.
        std::env::set_var("VIRTINST_TEST_FAKE_URI", "qemu:///system");
        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        std::env::remove_var("VIRTINST_TEST_FAKE_URI");
        assert!(
            conn.uri().is_qemu(),
            "the fake URI must make the connection report itself as qemu"
        );

        // `domain_type: "test"` and `is_qemu: true` deliberately
        // disagree here — the same `hypervisor_type`-vs-URI-scheme split
        // the NIC-OUI bug was about, just both halves pinned on purpose
        // this time: `domain_type` has to stay `"test"` for the real
        // `test:///` driver to accept `create_new`/`define_new` at all
        // (it's the connection actually reached), while `is_qemu` comes
        // from the faked URI so `push_default_clock`'s real gate
        // actually fires. Not a typo.
        let doc = build_import_document(
            "clock-end-to-end",
            1048576,
            1,
            "/guest/clock-end-to-end.img",
            GuestCapsDefaults {
                domain_type: "test",
                os_type: "hvm",
                arch: "x86_64",
                machine: None,
                emulator: None,
                is_qemu: conn.uri().is_qemu(),
            },
            "spice",
            None,
            "utc",
            "virtio",
        );
        let xml = doc.write_str().expect("serializes");

        let running = Domain::create_new(&conn, &xml).expect("transient create succeeds");
        let persisted = Domain::define_new(&conn, &xml).expect("define succeeds");
        let refetched = Domain::lookup_by_name(&conn, "clock-end-to-end")
            .expect("the domain is findable by the name we gave it");
        let inactive_xml = refetched.inactive_xml().expect("fetch inactive XML");
        assert!(
            inactive_xml.contains("name='rtc'")
                && inactive_xml.contains("name='pit'")
                && inactive_xml.contains("name='hpet'"),
            "the real driver kept all three timers we defined it with, got: {inactive_xml}"
        );

        running
            .destroy()
            .expect("cleanup: destroy the transient instance");
        persisted
            .undefine()
            .expect("cleanup: undefine the persistent config");
    }

    /// Ticket 21 slice (f)'s real discriminating check: does a
    /// `<cpu mode="host-passthrough"/>` this crate resolved actually
    /// survive a real `Domain::create_new`/`define_new` round trip
    /// against `test:///default`, not just this crate's own
    /// serialization? Unlike `clock_trio_survives_a_real_libvirt_
    /// define_when_is_qemu_is_true`, no `VIRTINST_TEST_FAKE_URI` trick
    /// is needed here: `default_cpu_mode` is a pure function over an
    /// already-parsed `DomainCapabilities` plus plain strings, resolved
    /// entirely before `push_default_cpu` ever runs — proven directly
    /// against a real fixture in `domcaps.rs`'s own test module, so
    /// this test only needs to prove the *other* half: that the
    /// already-resolved value round-trips through a live libvirt
    /// driver unchanged.
    #[test]
    fn cpu_mode_survives_a_real_libvirt_define() {
        use crate::capabilities::DomainCapabilities;
        use crate::connection::{Connection, Domain, Uri};
        use crate::test_support::TEST_DOMAIN_LOCK;

        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let conn = Connection::open("test:///default").expect("test:/// driver always opens");

        // A real captured <cpu> shape (domcaps.rs's own REAL_QEMU_CPU_
        // EXCERPT, inlined here rather than shared across modules —
        // this crate's convention keeps test fixtures local to the
        // module that needs them) that advertises host-passthrough as
        // safe; `domain_type: "kvm"`/arches deliberately don't match
        // this connection's real `"test"`/whatever-arch-it-reports —
        // `default_cpu_mode` doesn't touch the connection at all beyond
        // the URI-scheme gate, which `qemu:///system`'s bare `Uri::
        // parse` (no real connection opened to it) satisfies fine.
        let domcaps_xml = r#"<domainCapabilities>
  <cpu>
    <mode name='host-passthrough' supported='yes'>
      <enum name='hostPassthroughMigratable'>
        <value>on</value>
      </enum>
    </mode>
  </cpu>
</domainCapabilities>"#;
        let domcaps = DomainCapabilities::parse(domcaps_xml).expect("parses");
        let resolved = domcaps
            .default_cpu_mode(
                &Uri::parse("qemu:///system"),
                "kvm",
                "x86_64",
                "x86_64",
                Some("Skylake-Client-IBRS"),
            )
            .expect("host-passthrough resolves for this fixture");
        assert_eq!(resolved, DefaultCpuMode::Mode("host-passthrough"));

        let doc = build_import_document(
            "cpu-end-to-end",
            1048576,
            1,
            "/guest/cpu-end-to-end.img",
            GuestCapsDefaults {
                domain_type: "test",
                os_type: "hvm",
                arch: "x86_64",
                machine: None,
                emulator: None,
                is_qemu: conn.uri().is_qemu(),
            },
            "spice",
            Some(&resolved),
            "utc",
            "virtio",
        );
        let xml = doc.write_str().expect("serializes");

        let running = Domain::create_new(&conn, &xml).expect("transient create succeeds");
        let persisted = Domain::define_new(&conn, &xml).expect("define succeeds");
        let refetched = Domain::lookup_by_name(&conn, "cpu-end-to-end")
            .expect("the domain is findable by the name we gave it");
        let inactive_xml = refetched.inactive_xml().expect("fetch inactive XML");
        assert!(
            inactive_xml.contains("mode='host-passthrough'"),
            "the real driver kept the <cpu mode='host-passthrough'> element we defined it \
             with, got: {inactive_xml}"
        );

        running
            .destroy()
            .expect("cleanup: destroy the transient instance");
        persisted
            .undefine()
            .expect("cleanup: undefine the persistent config");
    }

    /// Ticket 04's real `libosinfo` bindings, first real end-to-end
    /// proof: an `OsInfo` resolved from the real, installed `osinfo-db`
    /// (Windows 10, `family = "winnt"`) produces `clock_offset() ==
    /// "localtime"`, and that value survives a real `Domain::
    /// create_new`/`define_new` round trip against `test:///default`
    /// unchanged -- same "resolve for real, prove the resolved value
    /// round-trips" split `cpu_mode_survives_a_real_libvirt_define`
    /// established: `OsInfo::clock_offset()` is already proven directly
    /// against real osinfo-db entries in `osinfo::mod::tests`, so this
    /// test only needs the other half.
    #[test]
    fn os_variant_clock_offset_survives_a_real_libvirt_define() {
        use crate::connection::{Connection, Domain};
        use crate::osinfo::OsDb;
        use crate::test_support::TEST_DOMAIN_LOCK;

        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let db = OsDb::load_default().expect("osinfo-db is installed on this machine");
        let windows = db
            .os("http://microsoft.com/win/10")
            .expect("Windows 10 is a real osinfo-db entry");
        assert_eq!(windows.clock_offset(), "localtime");

        // `push_default_clock`'s `is_qemu` gate strikes again — same
        // gap `clock_trio_survives_a_real_libvirt_define_when_is_qemu_
        // is_true` closed for the timer trio itself, hit for real here
        // too on the first attempt at this test (`test:///default`'s
        // own `is_qemu()` is structurally always `false`, so the whole
        // `<clock>` element -- offset included -- would never even be
        // pushed without this). Same fake-URI technique, same
        // deliberate `domain_type: "test"` vs. faked `is_qemu: true`
        // disagreement — see that test's own comment for why.
        std::env::set_var("VIRTINST_TEST_FAKE_URI", "qemu:///system");
        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        std::env::remove_var("VIRTINST_TEST_FAKE_URI");
        assert!(
            conn.uri().is_qemu(),
            "the fake URI must make is_qemu() true"
        );

        let doc = build_import_document(
            "os-variant-end-to-end",
            1048576,
            1,
            "/guest/os-variant-end-to-end.img",
            GuestCapsDefaults {
                domain_type: "test",
                os_type: "hvm",
                arch: "x86_64",
                machine: None,
                emulator: None,
                is_qemu: conn.uri().is_qemu(),
            },
            "spice",
            None,
            windows.clock_offset(),
            "virtio",
        );
        let xml = doc.write_str().expect("serializes");

        let running = Domain::create_new(&conn, &xml).expect("transient create succeeds");
        let persisted = Domain::define_new(&conn, &xml).expect("define succeeds");
        let refetched = Domain::lookup_by_name(&conn, "os-variant-end-to-end")
            .expect("the domain is findable by the name we gave it");
        let inactive_xml = refetched.inactive_xml().expect("fetch inactive XML");
        assert!(
            inactive_xml.contains("offset='localtime'"),
            "the real driver kept the <clock offset='localtime'> we defined it with, got: \
             {inactive_xml}"
        );

        running
            .destroy()
            .expect("cleanup: destroy the transient instance");
        persisted
            .undefine()
            .expect("cleanup: undefine the persistent config");
    }
}
