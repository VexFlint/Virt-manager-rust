//! `<domainsnapshot>` — upstream's `virtinst/snapshot.py`
//! (`DomainSnapshot`, `_SnapshotDisk`), plus the XML-only helpers of
//! `virtManager/object/domain.py`'s `vmmDomainSnapshot` (`is_external`)
//! that the snapshots page reads.
//!
//! Two uses, matching upstream's: a fresh `<domainsnapshot>` built for
//! `virDomainSnapshotCreateXML` ([`DomainSnapshot::get_xml`]), and an
//! existing snapshot's XML with only its description changed, for the
//! `VIR_DOMAIN_SNAPSHOT_CREATE_REDEFINE` the page's Apply does
//! ([`set_description`]) — the rest of libvirt's document (the embedded
//! `<domain>`, `<disks>`, `<cookie>`, ...) is kept as is.

use virtinst_xml::XmlBound;

/// `_SnapshotDisk`: one `<disks><disk>` entry.
///
/// `source` is upstream's `_DiskSource` child; only its file path is
/// bound here, the one part anything reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "disk")]
pub struct SnapshotDisk {
    #[xml(attribute = "name")]
    pub name: Option<String>,

    #[xml(attribute = "snapshot")]
    pub snapshot: Option<String>,

    #[xml(path = "source", attribute = "file")]
    pub source_file: Option<String>,
}

/// ```xml
/// <domainsnapshot>
///   <name>snapshot1</name>
///   <description>before the upgrade</description>
///   <state>running</state>
///   <creationTime>1700000000</creationTime>
///   <parent><name>snapshot0</name></parent>
///   <memory snapshot="internal"/>
///   <disks><disk name="vda" snapshot="internal"/></disks>
///   <domain>...</domain>
/// </domainsnapshot>
/// ```
///
/// Field order is the write order, upstream's `_XML_PROP_ORDER`
/// (`name`, `description`, `creationTime`) first.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "domainsnapshot")]
pub struct DomainSnapshot {
    #[xml(path = "name", text)]
    pub name: Option<String>,

    #[xml(path = "description", text)]
    pub description: Option<String>,

    /// `creationTime`, `is_int=True`: seconds since the epoch.
    #[xml(path = "creationTime", text)]
    pub creation_time: Option<u32>,

    #[xml(path = "state", text)]
    pub state: Option<String>,

    #[xml(path = "parent/name", text)]
    pub parent: Option<String>,

    #[xml(path = "memory", attribute = "snapshot")]
    pub memory_type: Option<String>,

    #[xml(path = "memory", attribute = "file")]
    pub memory_file: Option<String>,

    #[xml(path = "disks", list)]
    pub disks: Vec<SnapshotDisk>,
}

impl DomainSnapshot {
    /// `DomainSnapshot(conn, parsexml=xml)`.
    pub fn parse(xml: &str) -> Result<DomainSnapshot, String> {
        let doc = virtinst_xml::parse_libvirt_xml(xml).map_err(|e| e.to_string())?;
        let root = doc.root_element().ok_or("Empty snapshot XML")?;
        Ok(DomainSnapshot::from_element(&doc, root))
    }

    /// `get_xml()` of a freshly built snapshot: its set fields, in
    /// `_XML_PROP_ORDER`, as a standalone document.
    pub fn get_xml(&self) -> Result<String, String> {
        virtinst_xml::to_xml_string(self).map_err(|e| e.to_string())
    }

    /// `vmmDomainSnapshot.is_external`: an external memory state, or
    /// any disk snapshotted externally.
    pub fn is_external(&self) -> bool {
        if self.memory_type.as_deref() == Some("external") {
            return true;
        }
        self.disks
            .iter()
            .any(|d| d.snapshot.as_deref() == Some("external"))
    }
}

/// `xmlobj.description = desc; xmlobj.get_xml()` on a parsed
/// snapshot: the document re-serialized with only the description
/// changed. Returns `(origxml, newxml)` — both serialized the same way,
/// so comparing them says whether anything changed, like upstream's
/// `origxml = xmlobj.get_xml()` before the edit.
pub fn set_description(xml: &str, desc: &str) -> Result<(String, String), String> {
    let mut doc = virtinst_xml::parse_libvirt_xml(xml).map_err(|e| e.to_string())?;
    let root = doc.root_element().ok_or("Empty snapshot XML")?;
    let origxml = doc.write_str().map_err(|e| e.to_string())?;
    let el = virtinst_xml::resolve_or_create_path(&mut doc, root, &["description"]);
    el.set_text_content(&mut doc, desc);
    let newxml = doc.write_str().map_err(|e| e.to_string())?;
    Ok((origxml, newxml))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What libvirt's qemu driver returns for an internal snapshot of
    /// a running VM (`virsh snapshot-dumpxml`), trimmed.
    const INTERNAL: &str = "<domainsnapshot>
  <name>snapshot1</name>
  <description>before &lt;upgrade&gt;</description>
  <state>running</state>
  <parent>
    <name>snapshot0</name>
  </parent>
  <creationTime>1700000000</creationTime>
  <memory snapshot='internal'/>
  <disks>
    <disk name='vda' snapshot='internal'/>
    <disk name='sda' snapshot='no'/>
  </disks>
  <domain type='kvm'>
    <name>fedora</name>
    <description>the VM's own description</description>
    <memory unit='KiB'>2097152</memory>
    <devices>
      <disk type='file' device='disk'>
        <source file='/var/lib/libvirt/images/fedora.qcow2'/>
        <target dev='vda' bus='virtio'/>
      </disk>
    </devices>
  </domain>
  <active>1</active>
</domainsnapshot>";

    const EXTERNAL_DISK: &str = "<domainsnapshot>
  <name>snapshot2</name>
  <state>shutoff</state>
  <creationTime>1700000100</creationTime>
  <memory snapshot='no'/>
  <disks>
    <disk name='vda' snapshot='external' type='file'>
      <driver type='qcow2'/>
      <source file='/var/lib/libvirt/images/fedora.snapshot2'/>
    </disk>
  </disks>
  <domain type='kvm'><name>fedora</name></domain>
</domainsnapshot>";

    #[test]
    fn parses_libvirts_snapshot_xml() {
        let s = DomainSnapshot::parse(INTERNAL).unwrap();
        assert_eq!(s.name.as_deref(), Some("snapshot1"));
        // The snapshot's own description, not the embedded domain's.
        assert_eq!(s.description.as_deref(), Some("before <upgrade>"));
        assert_eq!(s.state.as_deref(), Some("running"));
        assert_eq!(s.creation_time, Some(1_700_000_000));
        assert_eq!(s.parent.as_deref(), Some("snapshot0"));
        assert_eq!(s.memory_type.as_deref(), Some("internal"));
        assert_eq!(s.memory_file, None);
        assert_eq!(s.disks.len(), 2);
        assert_eq!(s.disks[0].name.as_deref(), Some("vda"));
        assert_eq!(s.disks[1].snapshot.as_deref(), Some("no"));
        assert!(!s.is_external());

        let e = DomainSnapshot::parse(EXTERNAL_DISK).unwrap();
        assert_eq!(e.description, None);
        assert_eq!(e.parent, None);
        assert_eq!(
            e.disks[0].source_file.as_deref(),
            Some("/var/lib/libvirt/images/fedora.snapshot2")
        );
        assert!(e.is_external());
    }

    #[test]
    fn external_memory_alone_is_external() {
        let s = DomainSnapshot {
            memory_type: Some("external".into()),
            ..Default::default()
        };
        assert!(s.is_external());
        assert!(!DomainSnapshot::default().is_external());
    }

    /// `_validate_new_snapshot`'s XML: name, description, and the
    /// memory file for an external snapshot of a running VM, in
    /// `_XML_PROP_ORDER`.
    #[test]
    fn new_snapshot_xml_matches_upstream() {
        let internal = DomainSnapshot {
            name: Some("snapshot1".into()),
            description: Some("a & b".into()),
            ..Default::default()
        };
        let xml = internal.get_xml().unwrap();
        assert_eq!(
            xml,
            "<domainsnapshot>\n  <name>snapshot1</name>\n  \
             <description>a &amp; b</description>\n</domainsnapshot>"
        );

        let external = DomainSnapshot {
            name: Some("snapshot2".into()),
            memory_type: Some("external".into()),
            memory_file: Some("/var/lib/libvirt/images/vm-mem.snapshot2".into()),
            ..Default::default()
        };
        let xml = external.get_xml().unwrap();
        assert!(!xml.contains("<description"), "{xml}");
        assert!(
            xml.contains(
                r#"<memory snapshot="external" file="/var/lib/libvirt/images/vm-mem.snapshot2"/>"#
            ),
            "{xml}"
        );
        assert!(xml.find("<name>").unwrap() < xml.find("<memory").unwrap());
        // And it reads back the same.
        assert_eq!(DomainSnapshot::parse(&xml).unwrap(), external);
    }

    /// `_apply`: only the description changes; libvirt's embedded
    /// `<domain>` (with its own `<description>`) and the rest stay.
    #[test]
    fn set_description_keeps_the_rest_of_the_document() {
        let (orig, new) = set_description(INTERNAL, "after").unwrap();
        assert_ne!(orig, new);
        let s = DomainSnapshot::parse(&new).unwrap();
        assert_eq!(s.description.as_deref(), Some("after"));
        assert_eq!(s.parent.as_deref(), Some("snapshot0"));
        assert_eq!(s.disks.len(), 2);
        assert!(new.contains("<description>the VM's own description</description>"));
        assert!(new.contains("<active>1</active>"));
        assert_eq!(orig.replace("before &lt;upgrade&gt;", "after"), new);

        // No description yet: one is added; the same text is no change.
        let (orig, new) = set_description(EXTERNAL_DISK, "first").unwrap();
        assert!(
            !orig.contains("<description>") && new.contains("<description>first</description>")
        );
        let (orig, new) = set_description(&new, "first").unwrap();
        assert_eq!(orig, new);
    }
}
