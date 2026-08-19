//! VM cloning — upstream's `cloner.py` (622 LOC), wrapped by `virt-clone`
//! (ticket 16). This slice is `--preserve-data` only: rewrite the
//! source domain's XML into a new, distinct domain (new name, new UUID,
//! fresh MACs, disk sources pointed at caller-given paths) and define
//! it — no actual disk-content duplication (`cloner.py`'s
//! `start_duplicate`, real byte-level copying, real progress reporting).
//! `--preserve-data` is upstream's own supported mode for exactly this:
//! "don't touch storage, the paths I'm giving you already have the
//! right content" — not a crippled clone, a real one of upstream's own
//! shapes.
//!
//! Operates directly on the `edit_xml` `Document`/`Element` DOM, the
//! same surgical-mutation pattern `virtinst-cli`'s `--edit --watchdog`
//! uses — deliberately *not* "parse into a `Guest` struct, mutate
//! fields, call `write_to`". `Guest::devices` and everything under it
//! (`disks`/`interfaces`/`graphics`) are `#[xml(list)]`/`#[xml(nested)]`
//! fields, which `write_to` never serializes at all (ticket 06's
//! preservation design: no "absent" state distinguishable from
//! "present but empty", so list/nested fields are read-only). A typed
//! round trip literally cannot touch a `<mac>` or `<source file=...>`
//! — only direct DOM element mutation can.
//!
//! Ports `Cloner.__init__`'s `_init_new_guest` — the *unconditional*
//! changes upstream always makes — plus `_process_disks`' path
//! rewriting in `--preserve-data` mode. **Not ported, flagged rather
//! than built:**
//! - NVRAM handling (`_prepare_nvram`) — no `<os><nvram>` rewriting.
//! - The guest-agent channel source-path rewrite (`channel.source.path`
//!   cleared when it embeds the old domain name) — needs
//!   `DeviceChannel::TYPE_UNIX`-style matching this module doesn't have
//!   a reason to duplicate for a first slice.
//! - UUID/MAC **collision checking** against the connection's existing
//!   domains. Upstream retries up to 256 times against
//!   `conn.lookupByUUID`/`fetch_all_domains`; this generates once,
//!   unchecked. `Connection` doesn't have a domain-listing method yet
//!   either (see its own doc comment).
//! - `--auto-clone` (name/path auto-generation) — this slice requires
//!   an explicit `--name` and one `--file` per non-shareable disk, same
//!   as upstream requires when `--auto-clone` isn't passed.

use uuid::Uuid;
use virtinst_xml::{parse_libvirt_xml, Document, EditXMLError, Element};

/// libvirt's own MAC-address OUI convention (`devices/interface.py`'s
/// `_random_mac`): QEMU/KVM domains get `52:54:00`, Xen gets `00:16:3E`.
/// The remaining three bytes are random — upstream's docstring claims
/// the first bit of the first random byte is masked to 0, but the
/// actual code doesn't do that; checked directly, not assumed.
const OUI_QEMU: [u8; 3] = [0x52, 0x54, 0x00];
const OUI_OTHER: [u8; 3] = [0x00, 0x16, 0x3E];

#[derive(Debug)]
pub enum ClonerError {
    Xml(EditXMLError),
    /// The source domain has no `<devices>` element at all.
    NoDevices,
    /// The number of `--file` paths given didn't match the number of
    /// non-shareable disks on the source domain — upstream's
    /// `diskinfo.raise_error()` catches the same mismatch, just
    /// per-disk instead of as one count check.
    DiskCountMismatch {
        non_shareable_disks: usize,
        paths_given: usize,
    },
}

impl std::fmt::Display for ClonerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClonerError::Xml(e) => write!(f, "{e}"),
            ClonerError::NoDevices => write!(f, "source domain XML has no <devices> element"),
            ClonerError::DiskCountMismatch { non_shareable_disks, paths_given } => write!(
                f,
                "{non_shareable_disks} disk(s) need a new path but {paths_given} --file path(s) were given"
            ),
        }
    }
}

impl std::error::Error for ClonerError {}

impl From<EditXMLError> for ClonerError {
    fn from(e: EditXMLError) -> Self {
        ClonerError::Xml(e)
    }
}

/// A random MAC in libvirt's own format — upstream's
/// `DeviceInterface.generate_mac`, minus the connection-collision retry
/// loop (see this module's doc comment for why).
pub fn generate_mac(is_qemu: bool) -> String {
    let oui = if is_qemu { OUI_QEMU } else { OUI_OTHER };
    // Harvest 3 random bytes from a UUIDv4's own randomness rather than
    // adding a `getrandom` dependency for one 3-byte draw — `uuid`'s
    // `v4` feature already pulls it in internally. Bytes 0-2 of a v4
    // UUID are untouched by the version/variant bit-twiddling (that
    // lands on bytes 6 and 8), so they're as random as any other byte
    // in it.
    let r = Uuid::new_v4();
    let b = r.as_bytes();
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        oui[0], oui[1], oui[2], b[0], b[1], b[2]
    )
}

/// Builds the clone-target `Document` from a shut-off source domain's
/// XML. `new_disk_paths` must have exactly one entry per non-shareable
/// `<disk>`, applied in document order (upstream's `get_nonshare_
/// diskinfos()` iterates in the same order the disks appear in the
/// source XML).
pub fn build_clone_document(
    src_xml: &str,
    new_name: &str,
    is_qemu: bool,
    new_disk_paths: &[String],
) -> Result<Document, ClonerError> {
    let mut doc = parse_libvirt_xml(src_xml)?;
    let root = doc
        .root_element()
        .expect("a parsed libvirt domain document always has a root element");

    set_text(&mut doc, root, "name", new_name);

    // title = None — upstream clears it outright, not "set to empty".
    if let Some(title_el) = root.find(&doc, "title") {
        title_el.detach(&mut doc)?;
    }

    set_text(&mut doc, root, "uuid", &Uuid::new_v4().to_string());

    let devices = root.find(&doc, "devices").ok_or(ClonerError::NoDevices)?;

    // Graphics: avoid port conflicts with the still-running source by
    // forcing autoport, same as upstream's warning-and-reset.
    for g in devices.find_all(&doc, "graphics") {
        if let Some(port) = g.attribute(&doc, "port") {
            if port != "-1" {
                g.set_attribute(&mut doc, "port", "-1");
            }
        }
    }

    // Interfaces: drop the old <target dev=...> (let libvirt assign a
    // fresh one) and generate a fresh MAC.
    for iface in devices.find_all(&doc, "interface") {
        if let Some(target_el) = iface.find(&doc, "target") {
            target_el.detach(&mut doc)?;
        }
        let mac = generate_mac(is_qemu);
        match iface.find(&doc, "mac") {
            Some(mac_el) => mac_el.set_attribute(&mut doc, "address", mac),
            None => {
                let mac_el = Element::new(&mut doc, "mac");
                mac_el.set_attribute(&mut doc, "address", mac);
                iface
                    .push_child(&mut doc, mac_el)
                    .expect("a freshly-created detached element always attaches cleanly");
            }
        }
    }

    // Disks: non-shareable ones, in document order, get the next
    // caller-given path. `--preserve-data` semantics — the path's
    // content isn't touched, just pointed at.
    let disks: Vec<Element> = devices
        .find_all(&doc, "disk")
        .into_iter()
        .filter(|d| d.find(&doc, "shareable").is_none())
        .collect();
    if disks.len() != new_disk_paths.len() {
        return Err(ClonerError::DiskCountMismatch {
            non_shareable_disks: disks.len(),
            paths_given: new_disk_paths.len(),
        });
    }
    for (disk, path) in disks.iter().zip(new_disk_paths) {
        let source_el = match disk.find(&doc, "source") {
            Some(s) => s,
            None => {
                let s = Element::new(&mut doc, "source");
                disk.push_child(&mut doc, s)
                    .expect("a freshly-created detached element always attaches cleanly");
                s
            }
        };
        source_el.set_attribute(&mut doc, "file", path.as_str());
    }

    Ok(doc)
}

/// Sets `<tag>value</tag>` directly under `root`, creating the element
/// if it wasn't already there — `<name>`/`<uuid>` always exist on a
/// real domain document, but this stays honest about not assuming it.
fn set_text(doc: &mut Document, root: Element, tag: &str, value: &str) {
    match root.find(doc, tag) {
        Some(el) => el.set_text_content(doc, value),
        None => {
            let el = Element::new_with_text(doc, tag, value);
            root.push_child(doc, el)
                .expect("a freshly-created detached element always attaches cleanly");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One disk, one interface (with an existing target/mac to
    /// overwrite), a graphics device with a fixed port, a title, and a
    /// foreign `<qemu:commandline>` block this crate doesn't model at
    /// all — the same "prove preservation, not just the parts we
    /// touch" shape `--edit`'s tests use.
    const SOURCE_FIXTURE: &str = r#"<domain type="test" xmlns:qemu="http://libvirt.org/schemas/domain/qemu/1.0">
  <name>original</name>
  <uuid>11111111-1111-1111-1111-111111111111</uuid>
  <title>Original VM</title>
  <memory unit="KiB">1048576</memory>
  <devices>
    <disk type="file" device="disk">
      <source file="/var/lib/libvirt/images/original.qcow2"/>
      <target dev="vda" bus="virtio"/>
    </disk>
    <interface type="network">
      <mac address="52:54:00:aa:bb:cc"/>
      <source network="default"/>
      <target dev="vnet3"/>
    </interface>
    <graphics type="vnc" port="5900"/>
  </devices>
  <qemu:commandline>
    <qemu:arg value="-foo"/>
  </qemu:commandline>
</domain>"#;

    #[test]
    fn rewrites_name_uuid_and_drops_title() {
        let doc = build_clone_document(
            SOURCE_FIXTURE,
            "the-clone",
            true,
            &["/var/lib/libvirt/images/clone.qcow2".to_string()],
        )
        .unwrap();
        let root = doc.root_element().unwrap();

        assert_eq!(
            root.find(&doc, "name").unwrap().text_content(&doc),
            "the-clone"
        );
        assert!(
            root.find(&doc, "title").is_none(),
            "title is dropped, not emptied"
        );

        let new_uuid = root.find(&doc, "uuid").unwrap().text_content(&doc);
        assert_ne!(new_uuid, "11111111-1111-1111-1111-111111111111");
        assert!(Uuid::parse_str(&new_uuid).is_ok(), "still a valid UUID");
    }

    #[test]
    fn rewrites_interface_mac_and_drops_target() {
        let doc = build_clone_document(
            SOURCE_FIXTURE,
            "the-clone",
            true,
            &["/var/lib/libvirt/images/clone.qcow2".to_string()],
        )
        .unwrap();
        let root = doc.root_element().unwrap();
        let devices = root.find(&doc, "devices").unwrap();
        let iface = devices.find(&doc, "interface").unwrap();

        assert!(iface.find(&doc, "target").is_none());
        let mac = iface
            .find(&doc, "mac")
            .unwrap()
            .attribute(&doc, "address")
            .unwrap();
        assert_ne!(mac, "52:54:00:aa:bb:cc");
        assert!(mac.starts_with("52:54:00:"), "qemu OUI prefix, got {mac}");
        // Untouched sibling field on the same element.
        assert_eq!(
            iface
                .find(&doc, "source")
                .unwrap()
                .attribute(&doc, "network"),
            Some("default")
        );
    }

    #[test]
    fn forces_graphics_to_autoport() {
        let doc = build_clone_document(
            SOURCE_FIXTURE,
            "the-clone",
            true,
            &["/var/lib/libvirt/images/clone.qcow2".to_string()],
        )
        .unwrap();
        let root = doc.root_element().unwrap();
        let devices = root.find(&doc, "devices").unwrap();
        let graphics = devices.find(&doc, "graphics").unwrap();
        assert_eq!(graphics.attribute(&doc, "port"), Some("-1"));
        // Untouched attribute on the same element.
        assert_eq!(graphics.attribute(&doc, "type"), Some("vnc"));
    }

    #[test]
    fn rewrites_disk_source_path() {
        let doc = build_clone_document(
            SOURCE_FIXTURE,
            "the-clone",
            true,
            &["/var/lib/libvirt/images/clone.qcow2".to_string()],
        )
        .unwrap();
        let root = doc.root_element().unwrap();
        let devices = root.find(&doc, "devices").unwrap();
        let disk = devices.find(&doc, "disk").unwrap();
        assert_eq!(
            disk.find(&doc, "source").unwrap().attribute(&doc, "file"),
            Some("/var/lib/libvirt/images/clone.qcow2")
        );
        // Untouched sibling element on the same disk.
        assert_eq!(
            disk.find(&doc, "target").unwrap().attribute(&doc, "dev"),
            Some("vda")
        );
    }

    #[test]
    fn leaves_unrelated_content_completely_untouched() {
        let doc = build_clone_document(
            SOURCE_FIXTURE,
            "the-clone",
            true,
            &["/var/lib/libvirt/images/clone.qcow2".to_string()],
        )
        .unwrap();
        let root = doc.root_element().unwrap();

        assert_eq!(
            root.find(&doc, "memory").unwrap().text_content(&doc),
            "1048576"
        );
        let commandline = root.find_all(&doc, "commandline");
        assert_eq!(commandline.len(), 1);
        let args = commandline[0].find_all(&doc, "arg");
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].attribute(&doc, "value"), Some("-foo"));
    }

    #[test]
    fn disk_count_mismatch_is_an_error() {
        let result = build_clone_document(SOURCE_FIXTURE, "the-clone", true, &[]);
        assert!(matches!(
            result,
            Err(ClonerError::DiskCountMismatch {
                non_shareable_disks: 1,
                paths_given: 0
            })
        ));
    }

    #[test]
    fn shareable_disks_are_excluded_from_the_path_count() {
        const WITH_SHARED_DISK: &str = r#"<domain type="test">
  <name>original</name>
  <uuid>11111111-1111-1111-1111-111111111111</uuid>
  <devices>
    <disk type="file" device="disk">
      <source file="/data/shared.img"/>
      <target dev="vdb" bus="virtio"/>
      <shareable/>
    </disk>
    <disk type="file" device="disk">
      <source file="/var/lib/libvirt/images/original.qcow2"/>
      <target dev="vda" bus="virtio"/>
    </disk>
  </devices>
</domain>"#;
        // One path given for the one non-shareable disk — must not
        // error, and must not touch the shared disk's source at all.
        let doc = build_clone_document(
            WITH_SHARED_DISK,
            "the-clone",
            true,
            &["/var/lib/libvirt/images/clone.qcow2".to_string()],
        )
        .unwrap();
        let root = doc.root_element().unwrap();
        let devices = root.find(&doc, "devices").unwrap();
        let disks = devices.find_all(&doc, "disk");
        assert_eq!(
            disks[0]
                .find(&doc, "source")
                .unwrap()
                .attribute(&doc, "file"),
            Some("/data/shared.img"),
            "shareable disk's path is untouched"
        );
        assert_eq!(
            disks[1]
                .find(&doc, "source")
                .unwrap()
                .attribute(&doc, "file"),
            Some("/var/lib/libvirt/images/clone.qcow2")
        );
    }

    #[test]
    fn generate_mac_uses_the_right_oui_per_driver() {
        let qemu_mac = generate_mac(true);
        assert!(qemu_mac.starts_with("52:54:00:"), "got {qemu_mac}");
        let other_mac = generate_mac(false);
        assert!(other_mac.starts_with("00:16:3e:"), "got {other_mac}");
        // Sanity: full six-group, lowercase-hex, colon-separated shape.
        assert_eq!(qemu_mac.split(':').count(), 6);
        assert!(qemu_mac.chars().all(|c| c.is_ascii_hexdigit() || c == ':'));
    }

    /// Real end-to-end proof against `test:///default`'s `"test"`
    /// domain: destroy it (satisfying upstream's own "source must be
    /// shut off" requirement — checked directly in `cloner.py`'s
    /// `_init_src`, not skipped for convenience), clone it via the
    /// exact pipeline `virt-clone` uses (`inactive_xml` →
    /// `build_clone_document` → `write_str` → `Domain::define_new`),
    /// confirm the clone actually exists as its own domain with a
    /// different UUID, then restore everything: undefine the clone,
    /// restart `"test"`. Shares `TEST_DOMAIN_LOCK` with
    /// `connection::domain`'s tests — same shared, process-wide
    /// `"test"` domain, same race if two tests' fetch/mutate/define (or
    /// destroy/create) cycles interleave.
    #[test]
    fn clones_a_real_shut_off_domain_end_to_end() {
        use crate::connection::{Connection, Domain};
        use crate::test_support::TEST_DOMAIN_LOCK;

        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        let source = Domain::lookup_by_name(&conn, "test").expect("the default test domain");

        let was_active = source.is_active().expect("query succeeds");
        if was_active {
            source
                .destroy()
                .expect("stopping the source domain succeeds");
        }

        let src_xml = source.inactive_xml().expect("fetch inactive XML");
        let src_doc = virtinst_xml::parse_libvirt_xml(&src_xml).expect("re-parses");
        let src_root = src_doc.root_element().unwrap();
        let src_uuid = src_root
            .find(&src_doc, "uuid")
            .unwrap()
            .text_content(&src_doc);

        let clone_doc = build_clone_document(
            &src_xml,
            "test-clone-end-to-end",
            true,
            &["/guest/clone-diskimage1".to_string()],
        )
        .expect("the default test domain's one plain disk clones cleanly");
        let clone_xml = clone_doc.write_str().expect("serializes");

        let clone = Domain::define_new(&conn, &clone_xml).expect("defining the clone succeeds");

        // It's really a distinct, separately-defined domain, not just
        // an in-memory object.
        let refetched =
            Domain::lookup_by_name(&conn, "test-clone-end-to-end").expect("clone is findable");
        assert_eq!(refetched.name().unwrap(), "test-clone-end-to-end");

        let clone_inactive_xml = refetched.inactive_xml().expect("fetch clone's XML");
        let doc2 = virtinst_xml::parse_libvirt_xml(&clone_inactive_xml).expect("re-parses");
        let root2 = doc2.root_element().unwrap();
        assert_ne!(
            root2.find(&doc2, "uuid").unwrap().text_content(&doc2),
            src_uuid,
            "clone got its own UUID, not the source's"
        );
        let devices2 = root2.find(&doc2, "devices").unwrap();
        assert_eq!(
            devices2
                .find(&doc2, "disk")
                .unwrap()
                .find(&doc2, "source")
                .unwrap()
                .attribute(&doc2, "file"),
            Some("/guest/clone-diskimage1")
        );

        // Cleanup — leave the shared fixture exactly as every other
        // test found it.
        clone.undefine().expect("cleanup: undefine the clone");
        if was_active {
            source.start().expect("cleanup: restart the source domain");
        }
    }
}
