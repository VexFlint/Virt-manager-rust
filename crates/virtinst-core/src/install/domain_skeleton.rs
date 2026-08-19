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

use uuid::Uuid;
use virtinst_xml::{Document, Element};

/// Builds `<domain type=domain_type><name/><uuid/><memory/>
/// <currentMemory/><vcpu/><os><type arch=arch>hvm</type></os></domain>`
/// and returns the open document plus its `<os>` element — callers push
/// their own `<boot>` entries onto `os` (bootdev handling differs enough
/// per install method, see `Iso::live`/`Manual`, that it isn't shared
/// here) and their own `<devices>` onto `root`.
///
/// Always stamps an explicit `<uuid>` via `Uuid::new_v4()` rather than
/// leaving it for libvirt to invent — required because `virt-install`'s
/// default sequence calls `Domain::create_new` then `Domain::define_new`
/// against the *same* XML, and two separate libvirt-generated UUIDs
/// collide on the second call (hit for real, see `import.rs`'s doc
/// comment for the exact error).
pub(super) fn build_domain_skeleton(
    name: &str,
    memory_kib: u64,
    vcpus: u32,
    domain_type: &str,
    arch: &str,
) -> (Document, Element, Element) {
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
