//! `<disk>` — upstream's `devices/disk.py`. First real exercise of
//! ticket 06's derive macro against a device with several distinct
//! nested-element attribute groups (`<driver>`, `<source>`, `<target>`),
//! not just flat attributes-on-self.
//!
//! `read_only`/`shareable`/`transient` were left out when this struct
//! was first written — `<disk><readonly/></disk>` binds a child
//! element's *presence*, not an attribute or text value, and that field
//! kind (`#[xml(present)]`) didn't exist in the macro yet. Added now
//! that `DeviceFilesystem` needed the same pattern and it got built
//! properly. Confirmed against `disk.py`: all three really are
//! `is_bool=True`, same shape.
//!
//! Per-driver default resolution (`_default_bus` in upstream, ticket
//! 17's finding) isn't modeled yet — this struct is XML *binding* only
//! so far, not the default-resolution chain.

use virtinst_xml::XmlBound;

/// ```xml
/// <disk type="file" device="disk">
///   <driver name="qemu" type="qcow2"/>
///   <source file="/var/lib/libvirt/images/vm.qcow2"/>
///   <target dev="vda" bus="virtio"/>
///   <readonly/>
/// </disk>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "disk")]
pub struct DeviceDisk {
    #[xml(attribute = "type")]
    pub disk_type: Option<String>,

    #[xml(attribute = "device")]
    pub device: Option<String>,

    #[xml(path = "driver", attribute = "name")]
    pub driver_name: Option<String>,

    #[xml(path = "driver", attribute = "type")]
    pub driver_type: Option<String>,

    #[xml(path = "source", attribute = "file")]
    pub source_file: Option<String>,

    #[xml(path = "target", attribute = "dev")]
    pub target_dev: Option<String>,

    #[xml(path = "target", attribute = "bus")]
    pub target_bus: Option<String>,

    #[xml(path = "readonly", present)]
    pub read_only: bool,

    #[xml(path = "shareable", present)]
    pub shareable: bool,

    #[xml(path = "transient", present)]
    pub transient: bool,
}

// Editing an existing `<disk>` in place, for the details page. These
// work on the raw element so whatever this crate doesn't model
// survives the edit.

use virtinst_xml::{Document, Element};

/// `DeviceDisk.CACHE_MODES`.
pub const CACHE_MODES: [&str; 5] = ["none", "writethrough", "writeback", "directsync", "unsafe"];
/// `DeviceDisk.DISCARD_MODES`.
pub const DISCARD_MODES: [&str; 2] = ["ignore", "unmap"];

use super::edit::{attr, set_attr};

/// `_get_xmlpath`, plus the URL upstream builds for network disks.
pub fn source_path(doc: &Document, disk: Element) -> Option<String> {
    for name in ["file", "dev", "dir"] {
        if let Some(path) = attr(doc, disk, &["source"], name) {
            return Some(path);
        }
    }
    let protocol = attr(doc, disk, &["source"], "protocol")?;
    let name = attr(doc, disk, &["source"], "name").unwrap_or_default();
    let host = attr(doc, disk, &["source", "host"], "name");
    let port = attr(doc, disk, &["source", "host"], "port");
    let mut url = format!("{protocol}://");
    if let Some(host) = host {
        url += &host;
        if let Some(port) = port {
            url += &format!(":{port}");
        }
    }
    if !name.is_empty() {
        url += "/";
        url += name.trim_start_matches('/');
    }
    Some(url)
}

/// `<source pool= volume=>`, for disks backed by a named volume.
pub fn source_volume(doc: &Document, disk: Element) -> Option<(String, String)> {
    Some((
        attr(doc, disk, &["source"], "pool")?,
        attr(doc, disk, &["source"], "volume")?,
    ))
}

/// What the storage behind a new media path looks like, for
/// `sync_path_props`: libvirt's volume type (`file`/`block`/`dir`) and
/// its format, when the path is a managed volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathProps {
    pub dev_type: String,
    pub format: Option<String>,
}

/// `_qemu_sanitize_drvtype`.
fn qemu_sanitize_drvtype(phystype: &str, fmt: Option<&str>) -> Option<String> {
    if phystype == "block" && fmt.is_none() {
        return Some("raw".into());
    }
    match fmt {
        Some("iso") => Some("raw".into()),
        other => other.map(str::to_string),
    }
}

/// `set_source_path`, and when `sync` is given, `sync_path_props`:
/// the disk's `type` and `<driver name type>` follow the new storage.
/// `None` ejects the media. `driver_qemu` is
/// `conn.support.conn_disk_driver_name_qemu()`.
pub fn set_source_path(
    doc: &mut Document,
    disk: Element,
    path: Option<&str>,
    sync: Option<&PathProps>,
    driver_qemu: bool,
) {
    let path = path.map(|p| p.strip_prefix("file://").unwrap_or(p));
    let dev_type = match (sync, path) {
        (Some(props), Some(_)) => props.dev_type.clone(),
        _ => disk.attribute(doc, "type").unwrap_or("file").to_string(),
    };
    let dev_type = if sync.is_some() && path.is_none() {
        "file".to_string()
    } else {
        dev_type
    };
    if let Some(old) = disk.find(doc, "source") {
        let _ = old.detach(doc);
    }
    if let Some(path) = path {
        let prop = match dev_type.as_str() {
            "block" => Some("dev"),
            "dir" => Some("dir"),
            "file" => Some("file"),
            _ => None,
        };
        if let Some(prop) = prop {
            let source = virtinst_xml::resolve_or_create_path(doc, disk, &["source"]);
            source.set_attribute(doc, prop, path);
        }
    }
    let Some(props) = sync else {
        return;
    };
    disk.set_attribute(doc, "type", &dev_type);
    let driver_name = (path.is_some() && driver_qemu).then_some("qemu");
    set_attr(doc, disk, "driver", "name", driver_name);
    let driver_type = driver_name.and_then(|_| {
        qemu_sanitize_drvtype(
            &dev_type,
            props.format.as_deref().filter(|_| path.is_some()),
        )
    });
    set_attr(doc, disk, "driver", "type", driver_type.as_deref());
}

/// `is_bool=True` marker elements: `<readonly/>`, `<shareable/>`.
pub fn set_marker(doc: &mut Document, disk: Element, name: &str, val: bool) {
    match (disk.find(doc, name), val) {
        (None, true) => {
            let el = Element::new(doc, name);
            disk.push_child(doc, el)
                .expect("a fresh element always attaches");
        }
        (Some(el), false) => {
            let _ = el.detach(doc);
        }
        _ => {}
    }
}

/// `removable`: `./target/@removable`, written `on`/`off`.
pub fn set_removable(doc: &mut Document, disk: Element, val: bool) {
    let target = virtinst_xml::resolve_or_create_path(doc, disk, &["target"]);
    target.set_attribute(doc, "removable", if val { "on" } else { "off" });
}

/// `serial = val or None`.
pub fn set_serial(doc: &mut Document, disk: Element, val: Option<&str>) {
    if let Some(old) = disk.find(doc, "serial") {
        let _ = old.detach(doc);
    }
    if let Some(val) = val.filter(|v| !v.is_empty()) {
        let el = Element::new_with_text(doc, "serial", val);
        disk.push_child(doc, el)
            .expect("a fresh element always attaches");
    }
}

/// `driver_cache` / `driver_discard` (`= val or None`).
pub fn set_driver_attr(doc: &mut Document, disk: Element, name: &str, val: Option<&str>) {
    set_attr(doc, disk, "driver", name, val.filter(|v| !v.is_empty()));
}

/// `compare_device` for disks: the same `target@dev` and `target@bus`.
pub fn find_disk(
    doc: &Document,
    root: Element,
    target: Option<&str>,
    bus: Option<&str>,
) -> Option<Element> {
    let devices = root.find(doc, "devices")?;
    devices.find_all(doc, "disk").into_iter().find(|d| {
        attr(doc, *d, &["target"], "dev").as_deref() == target
            && attr(doc, *d, &["target"], "bus").as_deref() == bus
    })
}

#[cfg(test)]
mod edit_tests {
    use super::*;

    const XML: &str = "<domain><devices>\
        <disk type='file' device='disk'><driver name='qemu' type='qcow2' cache='none'/>\
        <source file='/a.qcow2'/><target dev='vda' bus='virtio'/></disk>\
        <disk type='file' device='cdrom'><driver name='qemu' type='raw'/>\
        <source file='/x.iso'/><target dev='sda' bus='sata'/><readonly/></disk>\
        <disk type='network' device='disk'><source protocol='rbd' name='pool/img'>\
        <host name='mon' port='6789'/></source><target dev='vdb' bus='virtio'/></disk>\
        </devices></domain>";

    fn parse() -> (Document, Element) {
        let doc = virtinst_xml::parse_libvirt_xml(XML).unwrap();
        let root = doc.root_element().unwrap();
        (doc, root)
    }

    #[test]
    fn finds_disks_and_paths() {
        let (doc, root) = parse();
        let cd = find_disk(&doc, root, Some("sda"), Some("sata")).unwrap();
        assert_eq!(source_path(&doc, cd).as_deref(), Some("/x.iso"));
        let net = find_disk(&doc, root, Some("vdb"), Some("virtio")).unwrap();
        assert_eq!(
            source_path(&doc, net).as_deref(),
            Some("rbd://mon:6789/pool/img")
        );
        assert!(find_disk(&doc, root, Some("sda"), Some("ide")).is_none());
    }

    #[test]
    fn media_change_and_eject() {
        let (mut doc, root) = parse();
        let cd = find_disk(&doc, root, Some("sda"), Some("sata")).unwrap();
        let block = PathProps {
            dev_type: "block".into(),
            format: None,
        };
        set_source_path(&mut doc, cd, Some("/dev/sr0"), Some(&block), true);
        assert_eq!(cd.attribute(&doc, "type"), Some("block"));
        assert_eq!(
            attr(&doc, cd, &["source"], "dev").as_deref(),
            Some("/dev/sr0")
        );
        assert_eq!(attr(&doc, cd, &["driver"], "name").as_deref(), Some("qemu"));
        assert_eq!(attr(&doc, cd, &["driver"], "type").as_deref(), Some("raw"));

        let file = PathProps {
            dev_type: "file".into(),
            format: None,
        };
        set_source_path(&mut doc, cd, None, Some(&file), true);
        let xml = doc.write_str().unwrap();
        let cd_xml = &xml[xml.find("device=\"cdrom\"").unwrap()..];
        let cd_xml = &cd_xml[..cd_xml.find("</disk>").unwrap()];
        assert!(!cd_xml.contains("<source"), "{cd_xml}");
        assert!(!cd_xml.contains("<driver"), "{cd_xml}");

        // hotplug: only the source changes
        set_source_path(&mut doc, cd, Some("file:///y.iso"), None, true);
        assert_eq!(source_path(&doc, cd).as_deref(), Some("/y.iso"));
    }

    #[test]
    fn advanced_options() {
        let (mut doc, root) = parse();
        let hd = find_disk(&doc, root, Some("vda"), Some("virtio")).unwrap();
        set_driver_attr(&mut doc, hd, "cache", None);
        set_driver_attr(&mut doc, hd, "discard", Some("unmap"));
        set_marker(&mut doc, hd, "readonly", true);
        set_marker(&mut doc, hd, "shareable", true);
        set_serial(&mut doc, hd, Some("ABC"));
        set_removable(&mut doc, hd, false);
        let xml = virtinst_xml::element_to_string(doc, hd).unwrap();
        let xml = xml.trim_start_matches(|c| c != '\n').trim_start();
        assert!(xml.contains("discard=\"unmap\""), "{xml}");
        assert!(!xml.contains("cache="), "{xml}");
        assert!(xml.contains("<readonly/>"), "{xml}");
        assert!(xml.contains("<shareable/>"), "{xml}");
        assert!(xml.contains("<serial>ABC</serial>"), "{xml}");
        assert!(xml.contains("removable=\"off\""), "{xml}");
        assert!(xml.starts_with("<disk"), "{xml}");
    }
}
