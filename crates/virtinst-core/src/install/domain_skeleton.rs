//! Shared scaffolding for the direct-DOM domain builders
//! (`import::build_import_document`, `iso::build_iso_document`,
//! `manual::build_manual_document`) — extracted once a third caller
//! needed the identical `<name>`/`<uuid>`/`<memory>`/`<currentMemory>`/
//! `<vcpu>`/`<os><type arch>` skeleton, per this project's "wait for a
//! second/real consumer" rule (same rule `#[xml(nested)]` and
//! `connect_and_lookup`/`report` in `virtinst-cli` were built under —
//! two identical copies stayed duplicated, a third got extracted).
//!
//! Per the ticket 08 amendment: direct `edit_xml::Document`/`Element`
//! construction, not `Guest::to_xml_string()` — see any of the three
//! callers' doc comments for the full rationale.
//!
//! Also home to the fixed-default device pushers shared across builders
//! once a third caller needed them (`push_virtio_disk`,
//! `push_default_nic`) and the two-phase-only elements
//! (`push_on_reboot_destroy`).

use virtinst_xml::{Document, Element};

/// Builds `<domain type=domain_type><name/><uuid/><memory/>
/// <currentMemory/><vcpu/><os><type arch=arch>os_type</type></os></domain>`
/// and returns the open document plus its `<os>` element — callers push
/// their own `<boot>` entries onto `os` (bootdev handling differs enough
/// per install method, see `Iso::live`/`Manual`, that it isn't shared
/// here — container guests skip `<boot>` entirely, see `container.rs`)
/// and their own `<devices>` onto `root`.
///
/// `os_type` is `<os><type>`'s text content — `"hvm"` for every install
/// method except containers (`"exe"`, `DomainOs.is_container()`'s check).
/// `arch` is still always passed through from `DomainCapabilities` even
/// for containers — confirmed directly in `guest.py`'s `set_capabilities_
/// defaults`: `self.os.arch = capsinfo.arch` runs unconditionally, no
/// container exception, so this isn't guessed.
///
/// `uuid` is caller-supplied rather than generated here — required
/// because `virt-install`'s default sequence calls `Domain::create_new`
/// then `Domain::define_new`, and for a two-phase install (`Iso { live:
/// false }`) those two calls need two *different* XML documents that
/// still describe the same domain, i.e. the same UUID stamped into both.
/// A single `generate_uuid()` inside this function couldn't do that — the
/// caller generates one and passes it to both `build_domain_skeleton`
/// calls (see `iso::build_iso_two_phase_documents`). Callers that only
/// ever build one document (`import`, `manual`, single-phase `iso`) just
/// generate one `generate_uuid()` and pass it through same as before.
/// Without a fixed UUID at all, each libvirt call invents its own and
/// the second call fails outright ("domain already exists with uuid
/// ..." — hit this for real, see `import.rs`'s doc comment).
pub(super) fn build_domain_skeleton(
    name: &str,
    memory_kib: u64,
    vcpus: u32,
    uuid: &str,
    domain_type: &str,
    os_type: &str,
    arch: &str,
) -> (Document, Element, Element) {
    let mut doc = Document::new();
    let root = Element::new(&mut doc, "domain");
    root.set_attribute(&mut doc, "type", domain_type);
    doc.push_root_node(root)
        .expect("a freshly-created detached element always attaches cleanly");

    push_text_child(&mut doc, root, "name", name);
    push_text_child(&mut doc, root, "uuid", uuid);

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
    let type_el = Element::new_with_text(&mut doc, "type", os_type);
    type_el.set_attribute(&mut doc, "arch", arch);
    os_el
        .push_child(&mut doc, type_el)
        .expect("a freshly-created detached element always attaches cleanly");
    root.push_child(&mut doc, os_el)
        .expect("a freshly-created detached element always attaches cleanly");

    (doc, root, os_el)
}

pub(super) fn push_text_child(doc: &mut Document, parent: Element, tag: &str, text: &str) {
    let el = Element::new_with_text(doc, tag, text);
    parent
        .push_child(doc, el)
        .expect("a freshly-created detached element always attaches cleanly");
}

/// Pushes `<boot dev=dev/>` onto `os` — shared by every caller that
/// needs to add one boot entry (`Manual` may add a second, see
/// `manual::build_manual_document`).
pub(super) fn push_boot_dev(doc: &mut Document, os: Element, dev: &str) {
    let boot_el = Element::new(doc, "boot");
    boot_el.set_attribute(doc, "dev", dev);
    os.push_child(doc, boot_el)
        .expect("a freshly-created detached element always attaches cleanly");
}

/// Pushes `<on_reboot>destroy</on_reboot>` onto `root` — upstream's
/// `_alter_bootconfig`'s very first line (`guest.on_reboot = "destroy"`),
/// applied *only* to the initial (`createXML`) document in a two-phase
/// install. This is the actual mechanism that makes two-phase installs
/// work: libvirt's own default is `restart`, so without this override
/// the transient instance would reboot straight back into the install
/// media/bootdev forever instead of yielding to the already-`defineXML`-
/// ed final config once the install's reboot happens. Missing entirely
/// from the first two-phase commit (`iso.rs`'s `Iso { live: false }`) —
/// no test caught it because `test:///` never actually boots anything,
/// so nothing here exercises the behavior this element controls, only
/// its presence in the XML.
pub(super) fn push_on_reboot_destroy(doc: &mut Document, root: Element) {
    push_text_child(doc, root, "on_reboot", "destroy");
}

/// `<disk type="file" device="disk"><source file=.../><target dev="vda"
/// bus="virtio"/></disk>`, pushed onto `devices_el` — the fixed-defaults
/// persistent disk shape shared by `import.rs`, `iso.rs`'s two-phase
/// case, and `manual.rs` (real driver/OS-aware bus selection isn't
/// ported, see `import.rs`'s doc comment for the gap). Moved here once
/// `iso.rs` and `manual.rs` became the second and third copies.
pub(super) fn push_virtio_disk(doc: &mut Document, devices_el: Element, path: &str) {
    let disk_el = Element::new(doc, "disk");
    disk_el.set_attribute(doc, "type", "file");
    disk_el.set_attribute(doc, "device", "disk");
    let source_el = Element::new(doc, "source");
    source_el.set_attribute(doc, "file", path);
    disk_el
        .push_child(doc, source_el)
        .expect("a freshly-created detached element always attaches cleanly");
    let target_el = Element::new(doc, "target");
    target_el.set_attribute(doc, "dev", "vda");
    target_el.set_attribute(doc, "bus", "virtio");
    disk_el
        .push_child(doc, target_el)
        .expect("a freshly-created detached element always attaches cleanly");
    devices_el
        .push_child(doc, disk_el)
        .expect("a freshly-created detached element always attaches cleanly");
}

/// `<interface type="network"><source network="default"/><mac
/// address=mac/><model type="virtio"/></interface>`, pushed onto
/// `devices_el` — upstream's own CLI default, not a `--pxe`-specific
/// fix: `virtinstall.py`'s `convert_old_networks` synthesizes a single
/// `"default"`-network entry whenever both `--network` and
/// `--nonetworks` are absent, applied unconditionally before any
/// install-method-specific logic runs. Every document this crate builds
/// gets one, matching that. `--pxe` is just the one install method
/// where omitting it makes the resulting domain outright inert (can't
/// network-boot at all) rather than merely missing connectivity
/// afterward — that's what prompted grounding this, but the fix is
/// general.
///
/// `mac` is caller-supplied for the same reason `uuid` is: a two-phase
/// pair's initial and final documents must describe the *same* device,
/// not two independently-randomized MACs. Reuses `crate::cloner::
/// generate_mac` (`52:54:00`/`00:16:3E` OUI by driver) rather than
/// duplicating it — the exact same function this crate's `virt-clone`
/// support already uses for the same reason. Every caller here derives
/// `generate_mac`'s `is_qemu` from `domain_type == "qemu"` (the string
/// already threaded through from `DomainCapabilities`); `cloner.rs`
/// derives it from the connection itself. The two can disagree in
/// principle — a `qemu+ssh://`-style connection whose domcaps happens to
/// report something other than the literal string `"qemu"` would get
/// the wrong OUI — not reachable through this crate's current CLI
/// surface, but worth knowing if `push_default_nic` ever gets a second
/// caller with a real `Connection` in hand. Model is a fixed `virtio`
/// default (`DeviceInterface.default_model`: `hvm` + `guest.
/// supports_virtionet()`, the common qemu/kvm x86 case) — the non-virtio
/// fallback chain (`e1000`/`rtl8139`/`ne2k_pci`/`pcnet`, chosen via
/// `guest.osinfo.supported_netmodels()`) needs `OsInfo`, unmodeled, same
/// posture as this crate's disk-bus default.
pub(super) fn push_default_nic(doc: &mut Document, devices_el: Element, mac: &str) {
    let iface_el = Element::new(doc, "interface");
    iface_el.set_attribute(doc, "type", "network");
    let source_el = Element::new(doc, "source");
    source_el.set_attribute(doc, "network", "default");
    iface_el
        .push_child(doc, source_el)
        .expect("a freshly-created detached element always attaches cleanly");
    let mac_el = Element::new(doc, "mac");
    mac_el.set_attribute(doc, "address", mac);
    iface_el
        .push_child(doc, mac_el)
        .expect("a freshly-created detached element always attaches cleanly");
    let model_el = Element::new(doc, "model");
    model_el.set_attribute(doc, "type", "virtio");
    iface_el
        .push_child(doc, model_el)
        .expect("a freshly-created detached element always attaches cleanly");
    devices_el
        .push_child(doc, iface_el)
        .expect("a freshly-created detached element always attaches cleanly");
}
