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

// ---------------------------------------------------------------------
// `DeviceDisk` as an XML backed object, for the CLI parsers.
// ---------------------------------------------------------------------

use crate::xmlbuilder::{child, prop, Kind, XmlClass, XmlObject, DEVICE_ADDRESS, DEVICE_SECLABEL};

/// `_Host`.
pub static DISK_HOST: XmlClass = XmlClass {
    xml_name: "host",
    props: &[
        prop("name", "./@name", Kind::Str),
        prop("port", "./@port", Kind::Int),
        prop("transport", "./@transport", Kind::Str),
        prop("socket", "./@socket", Kind::Str),
    ],
    children: &[],
    prop_order: &["name", "port", "transport", "socket"],
};

/// `_DiskSource`.
pub static DISK_SOURCE: XmlClass = XmlClass {
    xml_name: "source",
    props: &[
        prop("file", "./@file", Kind::Str),
        prop("dev", "./@dev", Kind::Str),
        prop("dir", "./@dir", Kind::Str),
        prop("pool", "./@pool", Kind::Str),
        prop("volume", "./@volume", Kind::Str),
        prop("name", "./@name", Kind::Str),
        prop("protocol", "./@protocol", Kind::Str),
        prop("query", "./@query", Kind::Str),
        prop("type", "./@type", Kind::Str),
        prop("managed", "./@managed", Kind::YesNo),
        prop("namespace", "./@namespace", Kind::Int),
    ],
    children: &[
        child("hosts", &DISK_HOST, ".", false),
        child("address", &DEVICE_ADDRESS, ".", true),
    ],
    prop_order: &[
        "file",
        "dev",
        "dir",
        "volume",
        "pool",
        "protocol",
        "name",
        "hosts",
        "type",
        "managed",
        "namespace",
        "address",
    ],
};

/// `DeviceDisk`. `type` and `device` are Python properties over
/// `_xmltype`/`_device`: see [`disk_type`] and [`disk_device`].
pub static DEVICE_DISK: XmlClass = XmlClass {
    xml_name: "disk",
    props: &[
        prop("_xmltype", "./@type", Kind::Str),
        prop("_device", "./@device", Kind::Str),
        prop("driver_name", "./driver/@name", Kind::Str),
        prop("driver_type", "./driver/@type", Kind::Str),
        prop("auth_username", "./auth/@username", Kind::Str),
        prop("auth_secret_type", "./auth/secret/@type", Kind::Str),
        prop("auth_secret_uuid", "./auth/secret/@uuid", Kind::Str),
        prop("snapshot_policy", "./@snapshot", Kind::Str),
        prop("driver_copy_on_read", "./driver/@copy_on_read", Kind::OnOff),
        prop("sgio", "./@sgio", Kind::Str),
        prop("rawio", "./@rawio", Kind::Str),
        prop("bus", "./target/@bus", Kind::Str),
        prop("target", "./target/@dev", Kind::Str),
        prop("removable", "./target/@removable", Kind::OnOff),
        prop("rotation_rate", "./target/@rotation_rate", Kind::Int),
        prop("read_only", "./readonly", Kind::Bool),
        prop("shareable", "./shareable", Kind::Bool),
        prop("transient", "./transient", Kind::Bool),
        prop(
            "transient_shareBacking",
            "./transient/@shareBacking",
            Kind::YesNo,
        ),
        prop("driver_cache", "./driver/@cache", Kind::Str),
        prop("driver_discard", "./driver/@discard", Kind::Str),
        prop("driver_detect_zeroes", "./driver/@detect_zeroes", Kind::Str),
        prop("driver_io", "./driver/@io", Kind::Str),
        prop("driver_iothread", "./driver/@iothread", Kind::Int),
        prop("driver_queues", "./driver/@queues", Kind::Int),
        prop(
            "driver_discard_no_unref",
            "./driver/@discard_no_unref",
            Kind::OnOff,
        ),
        prop("driver_queue_size", "./driver/@queue_size", Kind::Int),
        prop(
            "driver_metadata_cache_max_size",
            "./driver/metadata_cache/max_size",
            Kind::Int,
        ),
        prop(
            "driver_metadata_cache_max_size_unit",
            "./driver/metadata_cache/max_size/@unit",
            Kind::Str,
        ),
        prop("error_policy", "./driver/@error_policy", Kind::Str),
        prop("serial", "./serial", Kind::Str),
        prop("wwn", "./wwn", Kind::Str),
        prop("startup_policy", "./source/@startupPolicy", Kind::Str),
        prop(
            "logical_block_size",
            "./blockio/@logical_block_size",
            Kind::Str,
        ),
        prop(
            "physical_block_size",
            "./blockio/@physical_block_size",
            Kind::Str,
        ),
        prop(
            "discard_granularity",
            "./blockio/@discard_granularity",
            Kind::Str,
        ),
        prop("iotune_rbs", "./iotune/read_bytes_sec", Kind::Int),
        prop("iotune_ris", "./iotune/read_iops_sec", Kind::Int),
        prop("iotune_tbs", "./iotune/total_bytes_sec", Kind::Int),
        prop("iotune_tis", "./iotune/total_iops_sec", Kind::Int),
        prop("iotune_wbs", "./iotune/write_bytes_sec", Kind::Int),
        prop("iotune_wis", "./iotune/write_iops_sec", Kind::Int),
        prop("geometry_cyls", "./geometry/@cyls", Kind::Int),
        prop("geometry_heads", "./geometry/@heads", Kind::Int),
        prop("geometry_secs", "./geometry/@secs", Kind::Int),
        prop("geometry_trans", "./geometry/@trans", Kind::Str),
        prop(
            "reservations_managed",
            "./source/reservations/@managed",
            Kind::Str,
        ),
        prop(
            "reservations_source_type",
            "./source/reservations/source/@type",
            Kind::Str,
        ),
        prop(
            "reservations_source_path",
            "./source/reservations/source/@path",
            Kind::Str,
        ),
        prop(
            "reservations_source_mode",
            "./source/reservations/source/@mode",
            Kind::Str,
        ),
    ],
    children: crate::device_children!(
        child("source", &DISK_SOURCE, ".", true),
        child("seclabels", &DEVICE_SECLABEL, "./source", false),
    ),
    prop_order: crate::device_prop_order!(
        "_xmltype",
        "_device",
        "snapshot_policy",
        "driver_name",
        "driver_type",
        "driver_cache",
        "driver_discard",
        "driver_detect_zeroes",
        "driver_io",
        "driver_iothread",
        "driver_queues",
        "error_policy",
        "auth_username",
        "auth_secret_type",
        "auth_secret_uuid",
        "source",
        "target",
        "bus",
    ),
};

use crate::diskbackend::StorageBackend;
use crate::xmlbuilder::Value;

fn str_val(s: impl Into<String>) -> Option<Value> {
    Some(Value::Str(s.into()))
}

/// `_DiskSource.clear_source`: unset everything that describes the
/// source media.
fn clear_source(disk: &mut XmlObject) -> Result<(), String> {
    let source = disk.child_mut("source");
    for p in ["file", "dev", "dir", "volume", "pool", "name", "protocol"] {
        source.set(p, None)?;
    }
    while !source.children("hosts").is_empty() {
        source.remove_child("hosts", 0);
    }
    Ok(())
}

/// `_get_xmlpath`: the source file/block/dir path.
pub fn xmlpath(disk: &XmlObject) -> Option<String> {
    let source = disk.child("source");
    ["file", "dev", "dir"]
        .into_iter()
        .find_map(|p| source.get_str(p).filter(|s| !s.is_empty()))
}

/// `_DiskSource.build_url_from_network`.
pub fn build_url_from_network(disk: &XmlObject) -> String {
    let source = disk.child("source");
    let host = source.children("hosts").first();
    let hget = |p: &str| host.and_then(|h| h.get(p)).map(|v| v.to_xml_string());
    let mut ret = source
        .get_str("protocol")
        .unwrap_or_else(|| "unknown".into());
    if let Some(t) = hget("transport") {
        ret += &format!("+{t}");
    }
    ret += "://";
    if let Some(name) = hget("name") {
        ret += &name;
        if let Some(port) = hget("port") {
            ret += &format!(":{port}");
        }
    }
    if let Some(name) = source.get_str("name") {
        if !name.starts_with('/') {
            ret.push('/');
        }
        ret += &name;
    } else if let Some(socket) = hget("socket") {
        if !socket.starts_with('/') {
            ret.push('/');
        }
        ret += &socket;
    }
    ret
}

/// `_DiskSource.set_from_url`.
fn set_from_url(disk: &mut XmlObject, url: &str) -> Result<(), String> {
    let uri = crate::connection::uri::Uri::parse(url);
    let source = disk.child_mut("source");
    if !uri.scheme.is_empty() {
        source.set("protocol", str_val(uri.scheme.clone()))?;
    }
    let has_host = !uri.hostname.is_empty() || !uri.port.is_empty() || !uri.transport.is_empty();
    if has_host && source.children("hosts").is_empty() {
        source.add_child(XmlObject::new(&DISK_HOST));
    }
    if !uri.transport.is_empty() {
        source.children_mut("hosts")[0].set("transport", str_val(uri.transport.clone()))?;
    }
    if !uri.hostname.is_empty() {
        source.children_mut("hosts")[0].set("name", str_val(uri.hostname.clone()))?;
    }
    if !uri.port.is_empty() {
        let port = uri
            .port
            .parse()
            .map_err(|_| format!("invalid literal for int() with base 10: '{}'", uri.port))?;
        source.children_mut("hosts")[0].set("port", Some(Value::Int(port)))?;
    }
    if !uri.path.is_empty() {
        let has_transport = source
            .children("hosts")
            .first()
            .is_some_and(|h| h.get_str("transport").is_some());
        if has_transport {
            source.children_mut("hosts")[0].set("socket", str_val(uri.path.clone()))?;
        } else {
            let name = uri.path.strip_prefix('/').unwrap_or(&uri.path).to_string();
            source.set("name", str_val(name))?;
        }
    }
    if !uri.query.is_empty() {
        source.set("query", str_val(uri.query.clone()))?;
    }
    Ok(())
}

/// `_get_default_type`. `backend` is `None` for upstream's stub
/// backend.
pub fn default_type(disk: &XmlObject, backend: Option<&StorageBackend>) -> String {
    let source = disk.child("source");
    if source.get_str("pool").is_some() || source.get_str("volume").is_some() {
        return "volume".into();
    }
    if let Some(b) = backend {
        return b.get_dev_type();
    }
    if source.get_str("protocol").is_some() {
        return "network".into();
    }
    "file".into()
}

/// `DeviceDisk.type`.
pub fn disk_type(disk: &XmlObject, backend: Option<&StorageBackend>) -> String {
    disk.get_str("_xmltype")
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| default_type(disk, backend))
}

/// `DeviceDisk.device`.
pub fn disk_device(disk: &XmlObject) -> String {
    disk.get_str("_device")
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "disk".into())
}

/// `_set_xmlpath`: the source path, in the property the disk type
/// uses (or the network source the backend describes).
pub fn set_xmlpath(
    disk: &mut XmlObject,
    val: Option<String>,
    backend: &StorageBackend,
) -> Result<(), String> {
    clear_source(disk)?;
    if backend.get_dev_type() == "network" {
        if let Some(path) = backend.get_path() {
            set_from_url(disk, &path)?;
        }
        return Ok(());
    }
    let propname = match disk_type(disk, Some(backend)).as_str() {
        "block" => "dev",
        "dir" => "dir",
        "file" => "file",
        _ => return Ok(()),
    };
    disk.child_mut("source").set(propname, val.map(Value::Str))
}

/// `_resolve_storage_backend`: the storage the XML source points at.
pub fn resolve_backend(
    disk: &XmlObject,
    conn: &virt::connect::Connect,
    remote: bool,
) -> StorageBackend {
    let mut path = None;
    let mut vol = None;
    let mut pool = None;
    if disk_type(disk, None) == "network" {
        // Fill in a completed URL for path comparison, etc
        path = Some(build_url_from_network(disk));
    }
    if default_type(disk, None) == "volume" {
        let source = disk.child("source");
        let p = virt::storage_pool::StoragePool::lookup_by_name(
            conn,
            &source.get_str("pool").unwrap_or_default(),
        )
        .ok();
        vol = p.as_ref().and_then(|p| {
            virt::storage_vol::StorageVol::lookup_by_name(
                p,
                &source.get_str("volume").unwrap_or_default(),
            )
            .ok()
        });
        pool = p.as_ref().and_then(crate::diskbackend::pool_of);
    }
    if vol.is_none() && path.is_none() {
        path = xmlpath(disk);
    }
    if let Some(p) = path.clone() {
        if vol.is_none() && pool.is_none() {
            if let Ok(m) = crate::storage::manage_path(conn, &p) {
                return StorageBackend::new(Some(m.path), m.vol, m.pool, remote);
            }
        }
    }
    StorageBackend::new(path, vol, pool, remote)
}

/// `set_source_path`: `newpath` looked up as storage, then written
/// into the source. Returns the backend now behind the disk.
pub fn set_source_path_obj(
    disk: &mut XmlObject,
    newpath: Option<&str>,
    conn: &virt::connect::Connect,
    remote: bool,
) -> Result<StorageBackend, String> {
    // Some file managers use 'file://' when passing files to
    // virt-manager, we need to strip it from the newpath.
    let newpath = newpath.map(|p| p.strip_prefix("file://").unwrap_or(p));
    let backend = StorageBackend::for_path(conn, newpath, remote)?;
    set_xmlpath(disk, backend.get_path(), &backend)?;
    Ok(backend)
}

/// What `DeviceDisk.set_defaults` reads from the guest and connection.
pub struct DiskDefaults {
    /// `_default_bus(guest)`, computed for this disk's device.
    pub default_bus: String,
    /// `conn.support.conn_disk_driver_name_qemu()`.
    pub driver_name_qemu: bool,
    pub is_xen: bool,
    pub is_qemu: bool,
    /// The targets the guest's other disks use.
    pub used_targets: Vec<String>,
}

/// `DeviceDisk.set_defaults`.
pub fn set_defaults_obj(
    disk: &mut XmlObject,
    backend: Option<&StorageBackend>,
    d: &DiskDefaults,
) -> Result<(), String> {
    let unset = |disk: &XmlObject, p: &str| disk.get_str(p).map_or(true, |v| v.is_empty());
    if unset(disk, "_device") {
        disk.set("_device", str_val(disk_device(disk)))?;
    }
    if unset(disk, "_xmltype") {
        disk.set("_xmltype", str_val(default_type(disk, backend)))?;
    }
    let path = backend.and_then(StorageBackend::get_path);
    let is_empty = path.is_none();
    let typ = disk_type(disk, backend);
    if unset(disk, "driver_name") {
        // `_get_default_driver_name`
        let name = if is_empty {
            None
        } else if d.is_xen && typ == "block" {
            Some("phy")
        } else if d.driver_name_qemu {
            Some("qemu")
        } else {
            None
        };
        disk.set("driver_name", name.map(|n| Value::Str(n.into())))?;
    }
    if unset(disk, "driver_type") {
        // `_get_default_driver_type`
        let drvtype = if disk.get_str("driver_name").as_deref() == Some("qemu") {
            let fmt = backend.and_then(StorageBackend::get_driver_type);
            qemu_sanitize_drvtype(&typ, fmt.as_deref())
        } else {
            None
        };
        disk.set("driver_type", drvtype.map(Value::Str))?;
    }
    if unset(disk, "bus") {
        disk.set("bus", str_val(d.default_bus.clone()))?;
    }
    let device = disk_device(disk);
    if device == "cdrom" {
        disk.set("read_only", Some(Value::Bool(true)))?;
    }
    if d.is_qemu && device == "disk" && typ == "block" {
        if unset(disk, "driver_cache") {
            disk.set("driver_cache", str_val("none"))?;
        }
        if unset(disk, "driver_io") {
            disk.set("driver_io", str_val("native"))?;
        }
        if unset(disk, "driver_discard") {
            disk.set("driver_discard", str_val("unmap"))?;
        }
    }
    if unset(disk, "target") {
        let used: Vec<String> = d
            .used_targets
            .iter()
            .filter(|t| !t.is_empty())
            .cloned()
            .collect();
        let bus = disk.get_str("bus").unwrap_or_default();
        let target = super::build::generate_target(&bus, &device, &used)?;
        disk.set("target", str_val(target))?;
    }
    Ok(())
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
