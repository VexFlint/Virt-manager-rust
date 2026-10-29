//! Upstream's `virtManager/object/domain.py` (`vmmDomain`), the slice the
//! manager window needs: identity, run state, and the lifecycle actions
//! from `vmmenu.py`. XML-derived detail (devices, stats vectors, …)
//! comes with the details window and stats ports.
//!
//! A `VmmDomain` holds the last [`DomainSnapshot`] its connection's
//! tick polled, and emits `state-changed` when a new poll differs —
//! upstream's signal name, so later windows connect the same way.
//! Lifecycle actions look the domain up by UUID on a worker thread
//! (ticket 09) and ask the connection for a priority tick afterwards,
//! same as upstream's post-action `self._update_status()`.

use std::cell::{Cell, Ref, RefCell};
use std::collections::HashSet;
use std::sync::OnceLock;

use gtk::gio;
use gtk::gio::prelude::*;
use gtk::glib;
use gtk::glib::subclass::prelude::*;
use gtk::glib::subclass::Signal;
use virt::connect::Connect;
use virt::domain::Domain;
use virt::sys;

use crate::connection::VmmConnection;
use crate::libvirtenummap::{self as enummap, DomainState};
use crate::statsmanager::{self, PollFlags, RawSample, StatsList};
use std::rc::Rc;
use virtinst_core::capabilities::domcaps::DomainCapabilities;
use virtinst_core::domain::cpu::{self, CpuEditContext};
use virtinst_core::Guest;
use virtinst_xml::XmlBound;

pub mod migration;

/// One poll's worth of what the UI shows about a domain.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DomainSnapshot {
    pub uuid: String,
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub id: Option<u32>,
    pub state: DomainState,
    pub reason: i32,
    pub has_managed_save: bool,
}

impl DomainSnapshot {
    /// Blocking: reads one domain's state. Runs on the tick thread.
    pub fn fetch(dom: &Domain) -> Result<DomainSnapshot, virt::error::Error> {
        let metadata = |kind| {
            dom.get_metadata(kind as i32, None, 0)
                .ok()
                .filter(|s| !s.is_empty())
        };
        let (state, reason) = dom.get_state()?;
        Ok(DomainSnapshot {
            uuid: dom.get_uuid_string()?,
            name: dom.get_name()?,
            title: metadata(sys::VIR_DOMAIN_METADATA_TITLE),
            description: metadata(sys::VIR_DOMAIN_METADATA_DESCRIPTION),
            id: dom.get_id(),
            state,
            reason,
            // Not every driver implements managed save; upstream gates
            // this on `managedsave_supported` and treats it as False.
            has_managed_save: dom.has_managed_save(0).unwrap_or(false),
        })
    }

    /// Blocking: every domain on the connection, active and inactive.
    pub fn fetch_all(conn: &Connect) -> Result<Vec<DomainSnapshot>, virt::error::Error> {
        conn.list_all_domains(0)?
            .iter()
            .map(DomainSnapshot::fetch)
            .collect()
    }

    /// Blocking: one tick's poll — every domain's snapshot plus its
    /// stats sample. `mem_period_set` lists the UUIDs whose memory
    /// stats period was already set.
    pub fn fetch_all_with_stats(
        conn: &Connect,
        flags: PollFlags,
        is_lxc: bool,
        mem_period_set: &HashSet<String>,
    ) -> Result<Vec<(DomainSnapshot, RawSample)>, virt::error::Error> {
        conn.list_all_domains(0)?
            .iter()
            .map(|dom| {
                let snap = DomainSnapshot::fetch(dom)?;
                let set_period = !mem_period_set.contains(&snap.uuid);
                let raw = statsmanager::sample(dom, flags, is_lxc, set_period);
                Ok((snap, raw))
            })
            .collect()
    }
}

/// The domain's XML, parsed: upstream's `vm.get_xmlobj()`. `xml` is
/// kept for re-parsing into an editable DOM (ticket 06), `guest` is the
/// typed read view, `doc` the raw tree for the bits `Guest` doesn't
/// bind yet (`<emulator>`, every device's attributes for the hw list).
pub struct DomainXml {
    pub xml: String,
    pub doc: virtinst_xml::Document,
    pub guest: Guest,
}

impl DomainXml {
    pub fn parse(xml: String) -> Result<DomainXml, String> {
        let doc = virtinst_xml::parse_libvirt_xml(&xml).map_err(|e| e.to_string())?;
        let root = doc.root_element().ok_or("Empty domain XML")?;
        let guest = Guest::from_element(&doc, root);
        Ok(DomainXml { xml, doc, guest })
    }

    pub fn root(&self) -> virtinst_xml::Element {
        self.doc.root_element().expect("parsed with a root")
    }

    /// `./devices/emulator`.
    pub fn emulator(&self) -> Option<String> {
        virtinst_xml::resolve_path(&self.doc, self.root(), &["devices", "emulator"])
            .map(|e| e.text_content(&self.doc))
            .filter(|s| !s.is_empty())
    }
}

/// `_make_xmlobj_to_define`'s result: the inactive XML as an editable
/// document, with its typed view. Edits go into `guest` (or straight
/// into `doc` for the nested parts `Guest` keeps read-only), and
/// [`VmmDomain::redefine_xmlobj`] writes them back.
pub struct XmlToDefine {
    pub doc: virtinst_xml::Document,
    pub guest: Guest,
    origxml: String,
}

impl XmlToDefine {
    fn parse(xml: String) -> Result<XmlToDefine, String> {
        let doc = virtinst_xml::parse_libvirt_xml(&xml).map_err(|e| e.to_string())?;
        let root = doc.root_element().ok_or("Empty domain XML")?;
        let guest = Guest::from_element(&doc, root);
        Ok(XmlToDefine {
            doc,
            guest,
            origxml: xml,
        })
    }

    pub fn root(&self) -> virtinst_xml::Element {
        self.doc.root_element().expect("parsed with a root")
    }

    /// `get_xml()`: the document with `guest`'s edits written back.
    pub fn get_xml(&mut self) -> Result<String, String> {
        let root = self.root();
        self.guest.write_to(&mut self.doc, root);
        // An unset text field writes nothing; `guest` was read from
        // `doc`, so `None` here means the element was cleared.
        for (tag, unset) in [
            ("title", self.guest.title.is_none()),
            ("description", self.guest.description.is_none()),
        ] {
            if unset {
                virtinst_xml::remove_path_if_present(&mut self.doc, root, &[tag])
                    .map_err(|e| e.to_string())?;
            }
        }
        self.doc.write_str().map_err(|e| e.to_string())
    }

    /// `./os`, written back from an edited copy of `guest.os`.
    pub fn write_os(&mut self) {
        let root = self.root();
        let os = virtinst_xml::resolve_or_create_path(&mut self.doc, root, &["os"]);
        self.guest.os.write_to(&mut self.doc, os);
    }

    /// `is_uefi`.
    pub fn is_uefi(&self) -> bool {
        let os = &self.guest.os;
        if os.loader.is_some() && os.loader_type.as_deref() == Some("pflash") {
            return true;
        }
        os.firmware.as_deref() == Some("efi")
    }
}

/// `Guest.validate_name` (with `XMLBuilder.validate_generic_name`),
/// minus the collision check, which needs the connection.
pub fn validate_generic_name(name_label: &str, val: &str) -> Result<(), String> {
    if val.is_empty() {
        return Err(format!("A name must be specified for the {name_label}"));
    }
    if val.contains('/') {
        return Err(format!(
            "{name_label} name '{val}' can not contain '/' character."
        ));
    }
    Ok(())
}

/// The direct kernel boot fields of `define_boot`; empty means unset.
#[derive(Debug, Clone, Default)]
pub struct KernelBoot {
    pub kernel: String,
    pub initrd: String,
    pub dtb: String,
    pub kernel_args: String,
}

/// `define_boot`'s arguments; `None` leaves a part alone.
#[derive(Debug, Clone, Default)]
pub struct BootEdits {
    pub boot_order: Option<Vec<String>>,
    pub boot_menu: Option<bool>,
    pub kernel: Option<KernelBoot>,
    /// `(init, initargs)`.
    pub init: Option<(String, String)>,
}

/// `define_disk`'s arguments; `None` leaves a part alone.
#[derive(Debug, Clone, Default)]
pub struct DiskEdits {
    /// New media path, `Some(None)` to eject.
    pub path: Option<Option<String>>,
    pub readonly: Option<bool>,
    pub shareable: Option<bool>,
    pub removable: Option<bool>,
    pub cache: Option<Option<String>>,
    pub discard: Option<Option<String>>,
    pub serial: Option<Option<String>>,
    /// `change_bus`, from the customize dialog.
    pub bus: Option<String>,
}

/// `define_network`'s arguments; `None` leaves a part alone.
#[derive(Debug, Clone, Default)]
pub struct NetworkEdits {
    /// `(ntype, source, mode, portgroup)`.
    pub source: Option<crate::netlist::NetSelection>,
    pub model: Option<Option<String>>,
    pub linkstate: Option<bool>,
    /// The customize dialog's MAC address.
    pub macaddr: Option<String>,
}

/// Which NIC to edit: `compare_device` matches by MAC and index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NicId {
    pub mac: Option<String>,
    pub idx: usize,
}

fn edit_network(
    doc: &mut virtinst_xml::Document,
    iface: virtinst_xml::Element,
    edits: &NetworkEdits,
) {
    use virtinst_core::devices::network as n;
    if let Some((ntype, source, mode, portgroup)) = &edits.source {
        n::set_source(
            doc,
            iface,
            ntype,
            source.as_deref(),
            mode.as_deref(),
            portgroup.as_deref(),
        );
    }
    if let Some(model) = &edits.model {
        n::set_model(doc, iface, model.as_deref());
    }
    if let Some(up) = edits.linkstate {
        n::set_link_state(doc, iface, up);
    }
    if let Some(mac) = &edits.macaddr {
        let el = virtinst_xml::resolve_or_create_path(doc, iface, &["mac"]);
        el.set_attribute(doc, "address", mac.as_str());
    }
}

/// `DeviceDisk.change_bus`: a new bus drops the address, and a new
/// target prefix picks a new target.
fn change_disk_bus(
    doc: &mut virtinst_xml::Document,
    root: virtinst_xml::Element,
    disk: virtinst_xml::Element,
    newbus: &str,
) -> Result<(), String> {
    use virtinst_core::devices::build::{generate_target, target_prefix};
    let device = disk.attribute(doc, "device").unwrap_or("disk").to_string();
    let target = virtinst_xml::resolve_or_create_path(doc, disk, &["target"]);
    let oldbus = target.attribute(doc, "bus").unwrap_or_default().to_string();
    if oldbus == newbus {
        return Ok(());
    }
    let oldprefix = target_prefix(&oldbus, &device).0;
    target.set_attribute(doc, "bus", newbus);
    virtinst_xml::remove_path_if_present(doc, disk, &["address"]).map_err(|e| e.to_string())?;
    if oldprefix == target_prefix(newbus, &device).0 {
        return Ok(());
    }
    let mut used: Vec<String> = root
        .find(doc, "devices")
        .map(|d| d.find_all(doc, "disk"))
        .unwrap_or_default()
        .iter()
        .filter_map(|d| d.find(doc, "target"))
        .filter_map(|t| t.attribute(doc, "dev").map(str::to_string))
        .collect();
    if let Some(cur) = target.attribute(doc, "dev").map(str::to_string) {
        if let Some(pos) = used.iter().position(|t| *t == cur) {
            used.remove(pos);
        }
    }
    let newtarget = generate_target(newbus, &device, &used)?;
    target.set_attribute(doc, "dev", newtarget);
    Ok(())
}

/// `virNetworkGetDHCPLeases`, which the virt crate doesn't wrap:
/// `(mac, type, ipaddr)` per lease.
fn dhcp_leases(net: &virt::network::Network) -> Vec<(String, i32, String)> {
    use std::ffi::CStr;
    let mut leases: *mut sys::virNetworkDHCPLeasePtr = std::ptr::null_mut();
    // SAFETY: libvirt fills `leases` with `n` lease pointers we own and
    // free below, along with the array itself.
    unsafe {
        let n = sys::virNetworkGetDHCPLeases(net.as_ptr(), std::ptr::null(), &mut leases, 0);
        if n < 0 || leases.is_null() {
            return vec![];
        }
        let str_of = |p: *mut std::os::raw::c_char| {
            if p.is_null() {
                String::new()
            } else {
                CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        };
        let mut ret = vec![];
        for i in 0..n as usize {
            let lease = *leases.add(i);
            ret.push((
                str_of((*lease).mac),
                (*lease).type_,
                str_of((*lease).ipaddr),
            ));
            sys::virNetworkDHCPLeaseFree(lease);
        }
        extern "C" {
            fn free(ptr: *mut std::ffi::c_void);
        }
        free(leases as *mut std::ffi::c_void);
        ret
    }
}

/// `_IPFetcher.get`'s `extract_dom` over `virDomainInterfaceAddresses`
/// results: the last IPv4 and non-link-local IPv6 (with prefix) for `mac`.
fn extract_dom_ips(
    ifaces: &[virt::domain::Interface],
    mac: &str,
) -> (Option<String>, Option<String>) {
    let (mut ipv4, mut ipv6) = (None, None);
    for iface in ifaces.iter().filter(|i| i.hwaddr == mac) {
        let (mut v4, mut v6) = (None, None);
        for a in &iface.addrs {
            if a.typed == sys::VIR_IP_ADDR_TYPE_IPV4 as i64 {
                v4 = Some(a.addr.clone());
            } else if a.typed == sys::VIR_IP_ADDR_TYPE_IPV6 as i64 && !a.addr.starts_with("fe80") {
                v6 = Some(format!("{}/{}", a.addr, a.prefix));
            }
        }
        ipv4 = v4.or(ipv4);
        ipv6 = v6.or(ipv6);
    }
    (ipv4, ipv6)
}

/// Which disk to edit: `compare_device` matches disks by target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskId {
    pub target: Option<String>,
    pub bus: Option<String>,
}

/// Applies `edits` to `disk`; `sync` is set for the persistent config,
/// where a new path also updates the disk type and driver.
fn edit_disk(
    doc: &mut virtinst_xml::Document,
    disk: virtinst_xml::Element,
    edits: &DiskEdits,
    sync: Option<&virtinst_core::devices::disk::PathProps>,
    driver_qemu: bool,
) {
    use virtinst_core::devices::disk as d;
    if let Some(path) = &edits.path {
        d::set_source_path(doc, disk, path.as_deref(), sync, driver_qemu);
    }
    if let Some(v) = edits.readonly {
        d::set_marker(doc, disk, "readonly", v);
    }
    if let Some(v) = edits.shareable {
        d::set_marker(doc, disk, "shareable", v);
    }
    if let Some(v) = edits.removable {
        d::set_removable(doc, disk, v);
    }
    if let Some(v) = &edits.serial {
        d::set_serial(doc, disk, v.as_deref());
    }
    if let Some(v) = &edits.cache {
        d::set_driver_attr(doc, disk, "cache", v.as_deref());
    }
    if let Some(v) = &edits.discard {
        d::set_driver_attr(doc, disk, "discard", v.as_deref());
    }
}

/// `diskbackend.manage_path`'s view of a path: a libvirt volume's type
/// and format when it is one, else what a local `stat` says.
pub(crate) fn path_props(
    conn: &Connect,
    path: &str,
    is_local: bool,
) -> virtinst_core::devices::disk::PathProps {
    use virtinst_core::devices::disk::PathProps;
    if let Ok(vol) = virt::storage_vol::StorageVol::lookup_by_path(conn, path) {
        return virtinst_core::storage::vol_path_props(&vol);
    }
    let dev_type = match std::fs::metadata(path) {
        Ok(md) if is_local && md.is_dir() => "dir",
        Ok(md) if is_local && std::os::unix::fs::FileTypeExt::is_block_device(&md.file_type()) => {
            "block"
        }
        _ => "file",
    };
    PathProps {
        dev_type: dev_type.into(),
        format: None,
    }
}

/// `_edit_shared_mem`.
fn edit_shared_mem(doc: &mut virtinst_xml::Document, root: virtinst_xml::Element, shared: bool) {
    if shared {
        let mb = virtinst_xml::resolve_or_create_path(doc, root, &["memoryBacking"]);
        let source = virtinst_xml::resolve_or_create_path(doc, mb, &["source"]);
        source.set_attribute(doc, "type", "memfd");
        let access = virtinst_xml::resolve_or_create_path(doc, mb, &["access"]);
        access.set_attribute(doc, "mode", "shared");
        return;
    }
    let Some(mb) = root.find(doc, "memoryBacking") else {
        return;
    };
    for (tag, attr) in [("source", "type"), ("access", "mode")] {
        if let Some(el) = mb.find(doc, tag) {
            el.mut_attributes(doc).remove(attr);
            if el.attributes(doc).is_empty() && el.child_elements(doc).is_empty() {
                let _ = el.detach(doc);
            }
        }
    }
    if mb.attributes(doc).is_empty() && mb.child_elements(doc).is_empty() {
        let _ = mb.detach(doc);
    }
}

/// `DeviceDisk.path_in_use_by`, blocking: the VMs on `conn` using
/// `path` for a disk, for direct kernel boot, or as the backing store
/// of a volume a disk uses.
pub(crate) fn path_in_use_by(
    conn: &Connect,
    path: &str,
    shareable: bool,
    read_only: bool,
) -> Result<Vec<String>, String> {
    virtinst_core::storage::path_in_use_by_conn(conn, path, shareable, read_only)
}

/// `vmmDomain.delete(force=True)`, blocking: undefine along with
/// snapshot metadata, managed save and NVRAM.
pub(crate) fn delete_domain(dom: &Domain, has_nvram: bool) -> Result<(), String> {
    let mut flags =
        sys::VIR_DOMAIN_UNDEFINE_SNAPSHOTS_METADATA | sys::VIR_DOMAIN_UNDEFINE_MANAGED_SAVE;
    if has_nvram {
        flags |= sys::VIR_DOMAIN_UNDEFINE_NVRAM;
    }
    if let Err(e) = dom.undefine_flags(flags) {
        eprintln!("libvirt undefineFlags failed, falling back to old style: {e}");
        dom.undefine().map_err(|e| e.message().to_string())?;
    }
    Ok(())
}

/// A storage volume or local file that `_copy_nvram_file` made or
/// will remove.
enum NvramFile {
    Vol(virt::storage_vol::StorageVol),
    Local(String),
}

impl NvramFile {
    fn path(&self) -> String {
        match self {
            NvramFile::Vol(v) => v.get_path().unwrap_or_default(),
            NvramFile::Local(p) => p.clone(),
        }
    }

    fn delete(&self) -> Result<(), String> {
        match self {
            NvramFile::Vol(v) => v.delete(0).map_err(|e| e.message().to_string()),
            NvramFile::Local(p) => std::fs::remove_file(p).map_err(|e| e.to_string()),
        }
    }
}

/// `_copy_nvram_file`'s copy, blocking: libvirt has no API to rename a
/// storage volume, so the NVRAM is cloned next to the old one under
/// the new name (`Cloner.build_clone_disk` + `build_storage`). A path
/// outside any storage pool is copied directly when the connection is
/// local. Returns `(new, old)`, or `None` when the old file can't be
/// found.
fn copy_nvram_file(
    conn: &Connect,
    old_path: &str,
    new_name: &str,
    is_local: bool,
) -> Result<Option<(NvramFile, NvramFile)>, String> {
    use virt::storage_pool::StoragePool;
    use virt::storage_vol::StorageVol;

    let old = std::path::Path::new(old_path);
    let ext = old
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_else(|| ".fd".into());
    let base = std::path::Path::new(new_name)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| new_name.to_string());
    let new_file = format!("{base}_VARS{ext}");
    let dir = old.parent().unwrap_or(std::path::Path::new("/"));
    let new_path = dir.join(&new_file).to_string_lossy().into_owned();

    if let Ok(vol) = StorageVol::lookup_by_path(conn, old_path) {
        let pool = StoragePool::lookup_by_volume(&vol).map_err(|e| e.message().to_string())?;
        let info = vol.get_info().map_err(|e| e.message().to_string())?;
        let format = vol
            .get_xml_desc(0)
            .ok()
            .and_then(|xml| volume_format(&xml))
            .map(|f| format!("<format type='{f}'/>"))
            .unwrap_or_default();
        let xml = format!(
            "<volume><name>{}</name><capacity>{}</capacity>\
             <target>{format}</target></volume>",
            glib::markup_escape_text(&new_file),
            info.capacity
        );
        let newvol = StorageVol::create_xml_from(&pool, &xml, &vol, 0)
            .map_err(|e| e.message().to_string())?;
        return Ok(Some((NvramFile::Vol(newvol), NvramFile::Vol(vol))));
    }
    if is_local && old.exists() {
        std::fs::copy(old, &new_path).map_err(|e| e.to_string())?;
        return Ok(Some((
            NvramFile::Local(new_path),
            NvramFile::Local(old_path.to_string()),
        )));
    }
    eprintln!(
        "old_nvram_path={old_path} but it doesn't appear to exist. \
         skipping rename nvram duplication"
    );
    Ok(None)
}

/// `<target><format type=...>` of a storage volume's XML.
fn volume_format(xml: &str) -> Option<String> {
    let doc = virtinst_xml::parse_libvirt_xml(xml).ok()?;
    let root = doc.root_element()?;
    let format = virtinst_xml::resolve_path(&doc, root, &["target", "format"])?;
    format.attribute(&doc, "type").map(str::to_string)
}

/// `vmmLibvirtObject.define_name` + `vmmConnection.rename_object`:
/// undefine the old name (keeping NVRAM and TPM state) and define the
/// new XML, putting the old one back if that fails; autostart carries
/// over. Blocking.
fn define_name(
    conn: &Connect,
    dom: &Domain,
    newname: &str,
    has_nvram: bool,
    has_tpm_state: bool,
) -> Result<(), String> {
    let msg = |e: virt::error::Error| e.message().to_string();
    let oldautostart = dom.get_autostart().unwrap_or(false);
    let origxml = dom
        .get_xml_desc(sys::VIR_DOMAIN_XML_INACTIVE | sys::VIR_DOMAIN_XML_SECURE)
        .map_err(msg)?;
    let mut xmlobj = XmlToDefine::parse(origxml.clone())?;
    if xmlobj.guest.name.as_deref() == Some(newname) {
        return Ok(());
    }
    eprintln!(
        "Changing domain name from {} to {newname}",
        xmlobj.guest.name.as_deref().unwrap_or("")
    );
    xmlobj.guest.name = Some(newname.to_string());
    let newxml = xmlobj.get_xml()?;

    // `delete(force=False)`
    let mut flags = 0;
    if has_nvram {
        flags |= sys::VIR_DOMAIN_UNDEFINE_KEEP_NVRAM;
    }
    // `domain_undefine_keep_tpm`: libvirt 8.9.0
    if has_tpm_state && conn.get_lib_version().unwrap_or(0) >= 8_009_000 {
        flags |= sys::VIR_DOMAIN_UNDEFINE_KEEP_TPM;
    }
    if let Err(e) = dom.undefine_flags(flags) {
        eprintln!("libvirt undefineFlags failed, falling back to old style: {e}");
        dom.undefine().map_err(msg)?;
    }

    let newdom = match Domain::define_xml(conn, &newxml) {
        Ok(d) => d,
        Err(renameerr) => {
            eprintln!("Error defining new name domain XML: {renameerr}");
            return match Domain::define_xml(conn, &origxml) {
                Ok(_) => Err(renameerr.message().to_string()),
                Err(fixerr) => Err(format!(
                    "domain rename failed. Attempting to recover also failed.\n\n\
                     Original error: {}\n\nRecover error: {}",
                    renameerr.message(),
                    fixerr.message()
                )),
            };
        }
    };
    if oldautostart {
        newdom.set_autostart(true).map_err(msg)?;
    }
    Ok(())
}

/// `vmmConnection.pretty_hv`: `<domain type>` + `<os><type>` to a label.
pub fn pretty_hv(gtype: &str, domtype: &str) -> String {
    let gtype = gtype.to_lowercase();
    let domtype = domtype.to_lowercase();
    match (domtype.as_str(), gtype.as_str()) {
        ("xen", "xen") => "xen (paravirt)".into(),
        ("xen", "hvm") => "xen (fullvirt)".into(),
        ("test", "xen") => "test (xen)".into(),
        ("test", "hvm") => "test (hvm)".into(),
        ("qemu", _) => "QEMU TCG".into(),
        ("kvm", _) => "KVM".into(),
        ("hvf", _) => "Hypervisor.framework".into(),
        _ => domtype,
    }
}

/// The lifecycle calls `VMActionUI` makes, as data so they can cross to
/// the worker thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomainAction {
    Startup,
    Suspend,
    Resume,
    Shutdown,
    Reboot,
    Reset,
    Destroy,
    Save,
    /// `vm.remove_saved_image()`: drop a managed-save image.
    RemoveSavedImage,
}

impl DomainAction {
    fn run(self, dom: &Domain) -> Result<(), virt::error::Error> {
        match self {
            DomainAction::Startup => dom.create().map(drop),
            DomainAction::Suspend => dom.suspend().map(drop),
            DomainAction::Resume => dom.resume().map(drop),
            DomainAction::Shutdown => dom.shutdown().map(drop),
            DomainAction::Reboot => dom.reboot(0),
            DomainAction::Reset => dom.reset().map(drop),
            DomainAction::Destroy => dom.destroy(),
            DomainAction::Save => dom.managed_save(0).map(drop),
            DomainAction::RemoveSavedImage => dom.managed_save_remove(0).map(drop),
        }
    }
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct VmmDomain {
        pub snapshot: RefCell<DomainSnapshot>,
        pub conn: glib::WeakRef<VmmConnection>,
        pub stats: RefCell<StatsList>,
        pub mem_period_set: Cell<bool>,
        pub xmlobj: RefCell<Option<Rc<DomainXml>>>,
        pub domcaps: RefCell<Option<Rc<DomainCapabilities>>>,
        /// `domcaps.get_cpu_security_features()`, cached like upstream.
        pub cpu_security_features: RefCell<Option<Rc<Vec<String>>>>,
        /// `_install_abort`: the user stopped the VM themselves, so a
        /// finished install doesn't restart it.
        pub install_abort: Cell<bool>,
        /// `vmmDomainVirtinst`: set for the guest the Create VM wizard
        /// is customizing, which isn't defined in libvirt yet.
        pub virtinst: RefCell<Option<super::VirtinstGuest>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for VmmDomain {
        const NAME: &'static str = "VmmDomain";
        type Type = super::VmmDomain;
    }

    impl ObjectImpl for VmmDomain {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    Signal::builder("state-changed").build(),
                    Signal::builder("resources-sampled").build(),
                ]
            })
        }
    }
}

glib::wrapper! {
    pub struct VmmDomain(ObjectSubclass<imp::VmmDomain>);
}

/// `vmmDomainVirtinst`'s backend: the guest's XML, which edits replace
/// instead of being defined, and the installer's autostart flag.
#[derive(Default)]
pub struct VirtinstGuest {
    xml: String,
    autostart: bool,
}

impl VmmDomain {
    pub fn new(conn: &VmmConnection, snapshot: DomainSnapshot) -> VmmDomain {
        let obj: VmmDomain = glib::Object::new();
        obj.imp().conn.set(Some(conn));
        obj.imp().snapshot.replace(snapshot);
        obj
    }

    /// `vmmDomainVirtinst`: a domain backed by the guest XML the Create
    /// VM wizard built, for its "customize before install" window.
    pub fn new_virtinst(conn: &VmmConnection, xml: String) -> Result<VmmDomain, String> {
        let parsed = DomainXml::parse(xml.clone())?;
        let snapshot = DomainSnapshot {
            uuid: parsed.guest.uuid.clone().unwrap_or_default(),
            name: parsed.guest.name.clone().unwrap_or_default(),
            state: sys::VIR_DOMAIN_SHUTOFF,
            ..Default::default()
        };
        eprintln!("vmmDomainVirtinst initialized with XML=\n{xml}");
        let obj = VmmDomain::new(conn, snapshot);
        obj.imp().xmlobj.replace(Some(Rc::new(parsed)));
        obj.imp().virtinst.replace(Some(VirtinstGuest {
            xml,
            autostart: false,
        }));
        Ok(obj)
    }

    pub fn is_virtinst(&self) -> bool {
        self.imp().virtinst.borrow().is_some()
    }

    /// `get_backend()` of a [`Self::new_virtinst`] domain: its XML
    /// with the user's edits, and the autostart flag.
    pub fn virtinst_backend(&self) -> Option<(String, bool)> {
        self.imp()
            .virtinst
            .borrow()
            .as_ref()
            .map(|v| (v.xml.clone(), v.autostart))
    }

    /// `_replace_domain_xml` + `_define`: the edited XML becomes the
    /// guest, and the name and title follow it.
    async fn virtinst_define(&self, xml: String) -> Result<(), String> {
        // `Guest.get_xml()` has no XML declaration.
        let xml = match xml.strip_prefix("<?xml") {
            Some(rest) => rest
                .split_once("?>")
                .map_or(xml.clone(), |(_, body)| body.trim_start().to_string()),
            None => xml,
        };
        let parsed = DomainXml::parse(xml.clone())?;
        if let Some(v) = self.imp().virtinst.borrow_mut().as_mut() {
            v.xml = xml;
        }
        {
            let mut snap = self.imp().snapshot.borrow_mut();
            snap.name = parsed.guest.name.clone().unwrap_or_default();
            snap.title = parsed.guest.title.clone();
            snap.description = parsed.guest.description.clone();
        }
        self.ensure_latest_xml().await;
        Ok(())
    }

    /// Applies a fresh poll; emits `state-changed` only on a real change.
    pub fn update(&self, snapshot: DomainSnapshot) {
        if *self.imp().snapshot.borrow() == snapshot {
            return;
        }
        self.imp().snapshot.replace(snapshot);
        self.emit_by_name::<()>("state-changed", &[]);
    }

    /// `statsmanager.refresh_vm_stats` with this tick's raw sample, then
    /// upstream's `resources-sampled` signal.
    pub fn apply_sample(&self, raw: &RawSample, flags: PollFlags, hostcpus: u32) {
        self.imp()
            .stats
            .borrow_mut()
            .append_sample(raw, flags, hostcpus);
        self.imp().mem_period_set.set(
            flags.mem && !matches!(raw.state, sys::VIR_DOMAIN_SHUTOFF | sys::VIR_DOMAIN_CRASHED),
        );
        self.emit_by_name::<()>("resources-sampled", &[]);
    }

    pub fn mem_period_set(&self) -> bool {
        self.imp().mem_period_set.get()
    }

    pub fn stats(&self) -> Ref<'_, StatsList> {
        self.imp().stats.borrow()
    }

    pub fn conn(&self) -> Option<VmmConnection> {
        self.imp().conn.upgrade()
    }

    fn snap<T>(&self, f: impl FnOnce(&DomainSnapshot) -> T) -> T {
        f(&self.imp().snapshot.borrow())
    }

    pub fn get_uuid(&self) -> String {
        self.snap(|s| s.uuid.clone())
    }
    pub fn get_name(&self) -> String {
        self.snap(|s| s.name.clone())
    }
    pub fn get_id(&self) -> Option<u32> {
        self.snap(|s| s.id)
    }
    pub fn get_name_or_title(&self) -> String {
        self.snap(|s| s.title.clone().unwrap_or_else(|| s.name.clone()))
    }
    pub fn get_title(&self) -> Option<String> {
        self.snap(|s| s.title.clone())
    }
    pub fn get_description(&self) -> String {
        self.snap(|s| s.description.clone().unwrap_or_default())
    }
    pub fn status(&self) -> DomainState {
        self.snap(|s| s.state)
    }
    pub fn has_managed_save(&self) -> bool {
        self.snap(|s| s.has_managed_save)
    }

    pub fn run_status(&self) -> &'static str {
        enummap::pretty_run_status(self.status(), self.has_managed_save())
    }
    /// `run_status_reason`.
    pub fn run_status_reason(&self) -> Option<&'static str> {
        self.snap(|s| enummap::pretty_status_reason(s.state, s.reason))
    }
    pub fn get_install_abort(&self) -> bool {
        self.imp().install_abort.get()
    }
    pub fn is_shutoff(&self) -> bool {
        enummap::is_shutoff(self.status())
    }
    pub fn is_crashed(&self) -> bool {
        enummap::is_crashed(self.status())
    }
    pub fn run_status_icon_name(&self) -> &'static str {
        enummap::run_status_icon_name(self.status())
    }
    pub fn is_active(&self) -> bool {
        enummap::is_active(self.status())
    }
    pub fn is_runable(&self) -> bool {
        enummap::is_runable(self.status())
    }
    pub fn is_paused(&self) -> bool {
        enummap::is_paused(self.status())
    }
    pub fn is_pauseable(&self) -> bool {
        enummap::is_pauseable(self.status())
    }
    pub fn is_unpauseable(&self) -> bool {
        enummap::is_unpauseable(self.status())
    }
    pub fn is_stoppable(&self) -> bool {
        enummap::is_stoppable(self.status())
    }
    pub fn is_destroyable(&self) -> bool {
        enummap::is_destroyable(self.status())
    }
    pub fn is_cloneable(&self) -> bool {
        enummap::is_cloneable(self.status())
    }

    /// The last fetched XML; `None` until [`Self::refresh_xml`] ran once.
    pub fn xmlobj(&self) -> Option<Rc<DomainXml>> {
        self.imp().xmlobj.borrow().clone()
    }

    /// Re-reads the domain XML on a worker: on libvirt's domain events
    /// (`recache_from_event_loop`), when the VM window shows, and when
    /// the state changes. Returns whether it changed.
    pub async fn refresh_xml(&self) -> Result<bool, String> {
        let virtinst_xml = self.virtinst_backend().map(|(xml, _)| xml);
        if let Some(xml) = virtinst_xml {
            if self.xmlobj().is_some_and(|x| x.xml == xml) {
                return Ok(false);
            }
            let parsed = DomainXml::parse(xml)?;
            self.imp().xmlobj.replace(Some(Rc::new(parsed)));
            return Ok(true);
        }
        let conn = self.conn().ok_or("Connection closed")?;
        let handle = conn.handle().ok_or("Not connected")?;
        let uuid = self.get_uuid();
        let xml = gio::spawn_blocking(move || {
            let dom = Domain::lookup_by_uuid_string(handle.connect(), &uuid)?;
            // `get_dom_flags`: the secure flag shows graphics passwords;
            // read-only connections refuse it.
            dom.get_xml_desc(sys::VIR_DOMAIN_XML_SECURE)
                .or_else(|_| dom.get_xml_desc(0))
        })
        .await
        .map_err(|_| "Worker thread panicked".to_string())?
        .map_err(|e| e.message().to_string())?;
        if self.xmlobj().is_some_and(|x| x.xml == xml) {
            return Ok(false);
        }
        let parsed = DomainXml::parse(xml)?;
        self.imp().xmlobj.replace(Some(Rc::new(parsed)));
        Ok(true)
    }

    /// `get_domain_capabilities`, fetched once per domain from the
    /// emulator/arch/machine/type in its XML.
    pub async fn get_domain_capabilities(&self) -> Option<Rc<DomainCapabilities>> {
        if let Some(caps) = self.imp().domcaps.borrow().clone() {
            return Some(caps);
        }
        let x = self.xmlobj()?;
        let handle = self.conn()?.handle()?;
        let emulator = x.emulator();
        let arch = x.guest.os.arch.clone();
        let machine = x.guest.os.machine.clone();
        let virttype = x.guest.domain_type.clone();
        let xml = gio::spawn_blocking(move || {
            handle.connect().get_domain_capabilities(
                emulator.as_deref(),
                arch.as_deref(),
                machine.as_deref(),
                virttype.as_deref(),
                0,
            )
        })
        .await
        .ok()?
        .ok()?;
        let caps = Rc::new(DomainCapabilities::parse(&xml).ok()?);
        self.imp().domcaps.replace(Some(caps.clone()));
        Some(caps)
    }

    /// `recache_from_event_loop`: the XML and status, re-read because
    /// libvirt sent an event. `state-changed` is always emitted. A
    /// failure usually means the domain went away, so the connection
    /// ticks to find out.
    pub async fn recache_from_event_loop(&self) {
        let xml = self.refresh_xml().await;
        let snap = self
            .with_domain(|_, dom| DomainSnapshot::fetch(dom).map_err(|e| e.message().to_string()))
            .await;
        match (xml, snap) {
            (Ok(_), Ok(snap)) => {
                self.imp().snapshot.replace(snap);
                self.emit_state_changed();
            }
            _ => {
                if let Some(conn) = self.conn() {
                    conn.tick();
                }
            }
        }
    }

    pub fn emit_state_changed(&self) {
        self.emit_by_name::<()>("state-changed", &[]);
    }

    pub(crate) fn handle(&self) -> Result<std::sync::Arc<crate::connection::ConnHandle>, String> {
        let conn = self.conn().ok_or("Connection closed")?;
        conn.handle().ok_or_else(|| "Not connected".to_string())
    }

    /// Runs `f` on the connection, on a worker thread.
    async fn with_conn<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connect) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let handle = self.handle()?;
        gio::spawn_blocking(move || f(handle.connect()))
            .await
            .map_err(|_| "Worker thread panicked".to_string())?
    }

    /// Runs `f` on the domain, looked up by UUID, on a worker thread.
    async fn with_domain<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connect, &Domain) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        if self.is_virtinst() {
            return Err("The virtual machine isn't defined yet".into());
        }
        let handle = self.handle()?;
        let uuid = self.get_uuid();
        gio::spawn_blocking(move || {
            let conn = handle.connect();
            let dom =
                Domain::lookup_by_uuid_string(conn, &uuid).map_err(|e| e.message().to_string())?;
            f(conn, &dom)
        })
        .await
        .map_err(|_| "Worker thread panicked".to_string())?
    }

    /// `_make_xmlobj_to_define`: the inactive XML, for editing.
    pub async fn make_xmlobj_to_define(&self) -> Result<XmlToDefine, String> {
        if let Some((xml, _)) = self.virtinst_backend() {
            return XmlToDefine::parse(xml);
        }
        let xml = self
            .with_domain(|_, dom| {
                dom.get_xml_desc(sys::VIR_DOMAIN_XML_INACTIVE | sys::VIR_DOMAIN_XML_SECURE)
                    .map_err(|e| e.message().to_string())
            })
            .await?;
        XmlToDefine::parse(xml)
    }

    /// `_redefine_xmlobj` / `_redefine_xml_internal`: define the edited
    /// XML, then pick it up.
    pub async fn redefine_xmlobj(&self, mut xmlobj: XmlToDefine) -> Result<(), String> {
        let newxml = xmlobj.get_xml()?;
        if newxml == xmlobj.origxml {
            eprintln!(
                "Redefine requested for {}, but XML didn't change!",
                self.get_name()
            );
        }
        if self.is_virtinst() {
            return self.virtinst_define(newxml).await;
        }
        let handle = self.handle()?;
        gio::spawn_blocking(move || {
            Domain::define_xml(handle.connect(), &newxml)
                .map(|_| ())
                .map_err(|e| e.message().to_string())
        })
        .await
        .map_err(|_| "Worker thread panicked".to_string())??;
        self.ensure_latest_xml().await;
        Ok(())
    }

    /// `ensure_latest_xml` + `idle_emit("state-changed")`.
    async fn ensure_latest_xml(&self) {
        let _ = self.refresh_xml().await;
        self.emit_state_changed();
        if let Some(conn) = self.conn() {
            conn.tick();
        }
    }

    /// `domcaps.get_cpu_security_features()`: the security features
    /// of the expanded host-model CPU.
    pub async fn get_cpu_security_features(&self) -> Rc<Vec<String>> {
        if let Some(f) = self.imp().cpu_security_features.borrow().clone() {
            return f;
        }
        let mut features = vec![];
        if let (Some(caps), Ok(handle)) = (self.get_domain_capabilities().await, self.handle()) {
            if let Some(cpu_xml) = caps.host_model_cpu_xml.clone() {
                let params = (
                    caps.path.clone(),
                    caps.arch.clone(),
                    caps.machine.clone(),
                    caps.domain.clone(),
                );
                let expanded = gio::spawn_blocking(move || {
                    virtinst_core::capabilities::domcaps::baseline_hypervisor_cpu(
                        handle.connect(),
                        params,
                        &cpu_xml,
                    )
                })
                .await
                .ok()
                .flatten();
                match expanded {
                    Some(xml) => {
                        features = virtinst_core::capabilities::domcaps::cpu_security_features(&xml)
                    }
                    None => glib::g_warning!("virt-manager", "Failed to get expanded CPU XML"),
                }
            }
        }
        glib::g_debug!(
            "virt-manager",
            "Found host-model security features: {features:?}"
        );
        let features = Rc::new(features);
        self.imp()
            .cpu_security_features
            .replace(Some(features.clone()));
        features
    }

    /// What `DomainCpu`'s edits need: see [`CpuEditContext`].
    pub async fn cpu_edit_context(&self) -> CpuEditContext {
        let x = self.xmlobj();
        let os = x.as_ref().map(|x| &x.guest.os);
        let app_default_mode = match self.get_domain_capabilities().await {
            Some(caps) => caps.app_default_cpu_mode(
                x.as_ref().and_then(|x| x.guest.domain_type.as_deref()),
                os.and_then(|o| o.arch.as_deref()),
                os.and_then(|o| o.machine.as_deref()),
            ),
            None => "host-model-only",
        };
        CpuEditContext {
            app_default_mode: app_default_mode.to_string(),
            host_cpu_model: self.conn().and_then(|c| c.host_cpu_model()),
            security_features: self.get_cpu_security_features().await.to_vec(),
        }
    }

    /// `define_cpu`. `model` is `(model or special mode, secure)`;
    /// `topology` is `Some(None)` to clear it.
    pub async fn define_cpu(
        &self,
        vcpus: Option<u32>,
        model: Option<(String, bool)>,
        topology: Option<Option<(u32, u32, u32)>>,
    ) -> Result<(), String> {
        let ctx = if model.is_some() {
            Some(self.cpu_edit_context().await)
        } else {
            None
        };
        let mut guest = self.make_xmlobj_to_define().await?;
        let root = guest.root();
        if let Some(vcpus) = vcpus {
            guest.guest.vcpus = Some(vcpus);
            guest.guest.vcpu_current = Some(vcpus);
        }
        // `guest.write_to` in `get_xml` writes vcpus; the rest goes
        // straight into the document.
        let doc = &mut guest.doc;
        match topology {
            Some(None) => cpu::edit_clear_topology(doc, root),
            Some(Some((sockets, cores, threads))) => {
                cpu::edit_set_topology(doc, root, sockets, cores, threads)
            }
            None => {}
        }
        if let (Some((model, secure)), Some(ctx)) = (model, ctx) {
            if cpu::SPECIAL_MODES.contains(&model.as_str()) {
                cpu::edit_set_special_mode(doc, root, &ctx, &model, secure)?;
            } else {
                cpu::edit_set_model(doc, root, &ctx, Some(&model), secure);
            }
        }
        self.redefine_xmlobj(guest).await
    }

    /// `define_memory` (with `_edit_shared_mem`); memory in KiB.
    pub async fn define_memory(
        &self,
        memory: Option<(u64, u64)>,
        mem_shared: Option<bool>,
    ) -> Result<(), String> {
        let mut guest = self.make_xmlobj_to_define().await?;
        let root = guest.root();
        let doc = &mut guest.doc;
        if let Some((cur, max)) = memory {
            for (tag, val) in [("currentMemory", cur), ("memory", max)] {
                let el = virtinst_xml::resolve_or_create_path(doc, root, &[tag]);
                el.set_text_content(doc, val.to_string());
                el.set_attribute(doc, "unit", "KiB");
            }
            guest.guest.current_memory = None;
            guest.guest.memory = None;
        }
        if let Some(shared) = mem_shared {
            edit_shared_mem(doc, root, shared);
        }
        self.redefine_xmlobj(guest).await
    }

    /// `hotplug(memory=..., maxmem=...)`, in KiB.
    pub async fn hotplug_memory(&self, memory: u64, maxmem: u64) -> Result<(), String> {
        if !self.is_active() {
            return Ok(());
        }
        let Some(x) = self.xmlobj() else {
            return Ok(());
        };
        glib::g_debug!(
            "virt-manager",
            "Hotplugging curmem={memory} maxmem={maxmem} for VM '{}'",
            self.get_name()
        );
        let actual_cur = u64::from(x.guest.current_memory.unwrap_or(0));
        let actual_max = u64::from(x.guest.memory.unwrap_or(0));
        self.with_domain(move |_, dom| {
            let msg = |e: virt::error::Error| e.message().to_string();
            let set_cur = || -> Result<(), String> {
                if memory != actual_cur {
                    dom.set_memory(memory).map_err(msg)?;
                }
                Ok(())
            };
            let set_max = || -> Result<(), String> {
                if maxmem != actual_max {
                    dom.set_max_memory(maxmem).map_err(msg)?;
                }
                Ok(())
            };
            if maxmem < actual_cur {
                // Set current first to avoid error
                set_cur()?;
                set_max()
            } else {
                set_max()?;
                set_cur()
            }
        })
        .await?;
        self.ensure_latest_xml().await;
        Ok(())
    }

    /// `has_shared_mem`: the "Enable shared memory" value, and why it
    /// can't be changed, if it can't.
    pub async fn has_shared_mem(&self) -> (bool, Option<String>) {
        let Some(x) = self.xmlobj() else {
            return (false, None);
        };
        let domcaps = self.get_domain_capabilities().await;
        if !x.guest.cpu.cells.is_empty() {
            return (
                false,
                Some("Can not change shared memory setting when <numa> is configured.".into()),
            );
        }
        let supported = domcaps
            .is_some_and(|c| c.supports_filesystem_virtiofs() && c.supports_memorybacking_memfd());
        if !supported {
            return (
                false,
                Some("Libvirt may not be new enough to support memfd.".into()),
            );
        }
        (
            x.guest.memory_backing.source_type.as_deref() == Some("memfd"),
            None,
        )
    }

    /// `get_autostart`; `None` when libvirt can't say.
    pub async fn get_autostart(&self) -> Option<bool> {
        if let Some((_, autostart)) = self.virtinst_backend() {
            return Some(autostart);
        }
        self.with_domain(|_, dom| dom.get_autostart().map_err(|e| e.message().to_string()))
            .await
            .ok()
    }

    /// `set_autostart`.
    pub async fn set_autostart(&self, val: bool) -> Result<(), String> {
        if let Some(v) = self.imp().virtinst.borrow_mut().as_mut() {
            v.autostart = val;
        }
        if self.is_virtinst() {
            self.emit_state_changed();
            return Ok(());
        }
        self.with_domain(move |_, dom| {
            dom.set_autostart(val)
                .map(|_| ())
                .map_err(|e| e.message().to_string())
        })
        .await
    }

    /// `can_use_device_boot_order`: `conn_device_boot_order`, which
    /// only the qemu and test drivers support.
    pub fn can_use_device_boot_order(&self) -> bool {
        let uri = self.conn().map(|c| c.get_uri()).unwrap_or_default();
        let scheme = virtinst_core::connection::Uri::parse(&uri).scheme;
        let driver = scheme.split('+').next().unwrap_or("");
        matches!(driver, "qemu" | "test")
    }

    /// `get_boot_order`.
    pub fn get_boot_order(&self) -> Vec<String> {
        match self.xmlobj() {
            Some(x) => virtinst_core::guest::get_boot_order(
                &x.doc,
                x.root(),
                !self.can_use_device_boot_order(),
            ),
            None => vec![],
        }
    }

    /// `define_boot`.
    pub async fn define_boot(&self, edits: BootEdits) -> Result<(), String> {
        use virtinst_core::guest as g;
        let legacy = !self.can_use_device_boot_order();
        let mut guest = self.make_xmlobj_to_define().await?;
        let root = guest.root();
        let doc = &mut guest.doc;
        if let Some(order) = &edits.boot_order {
            g::set_boot_order(doc, root, order, legacy);
        }
        if let Some(menu) = edits.boot_menu {
            g::set_bootmenu(doc, root, menu);
        }
        if let Some((init, initargs)) = &edits.init {
            g::set_os_text(doc, root, "init", Some(init));
            g::set_initargs_string(doc, root, initargs)?;
        }
        if let Some(k) = &edits.kernel {
            fn some(s: &str) -> Option<&str> {
                Some(s).filter(|s| !s.is_empty())
            }
            g::set_os_text(doc, root, "kernel", some(&k.kernel));
            g::set_os_text(doc, root, "initrd", some(&k.initrd));
            g::set_os_text(doc, root, "dtb", some(&k.dtb));
            g::set_os_text(doc, root, "cmdline", some(&k.kernel_args));
        }
        self.redefine_xmlobj(guest).await
    }

    /// `define_os`: `guest.set_os_name`, storing the OS's libosinfo id
    /// in the metadata (`None` for the generic OS).
    pub async fn define_os(&self, os_id: Option<String>) -> Result<(), String> {
        let mut guest = self.make_xmlobj_to_define().await?;
        let root = guest.root();
        virtinst_core::guest::set_os_id(&mut guest.doc, root, os_id.as_deref());
        self.redefine_xmlobj(guest).await
    }

    fn is_local_conn(&self) -> bool {
        let uri = self.conn().map(|c| c.get_uri()).unwrap_or_default();
        virtinst_core::connection::Uri::parse(&uri)
            .hostname
            .is_empty()
    }

    /// `conn.support.conn_disk_driver_name_qemu()`.
    fn disk_driver_name_qemu(&self) -> bool {
        let uri = self.conn().map(|c| c.get_uri()).unwrap_or_default();
        let scheme = virtinst_core::connection::Uri::parse(&uri).scheme;
        matches!(scheme.split('+').next(), Some("qemu" | "test"))
    }

    /// `DeviceDisk.path_in_use_by`: the VMs on this connection using
    /// `path`.
    pub async fn path_in_use_by(
        &self,
        path: String,
        shareable: bool,
        read_only: bool,
    ) -> Result<Vec<String>, String> {
        self.with_conn(move |conn| path_in_use_by(conn, &path, shareable, read_only))
            .await
    }

    /// The storage volume behind a disk, as `(path, capacity)`:
    /// `conn.get_vol_by_path`, or the named pool volume.
    pub async fn disk_volume_info(
        &self,
        path: Option<String>,
        volume: Option<(String, String)>,
    ) -> Option<(String, u64)> {
        self.with_conn(move |conn| {
            let vol = match (&volume, &path) {
                (Some((pool, vol)), _) => {
                    virt::storage_pool::StoragePool::lookup_by_name(conn, pool)
                        .and_then(|p| virt::storage_vol::StorageVol::lookup_by_name(&p, vol))
                }
                (None, Some(path)) => virt::storage_vol::StorageVol::lookup_by_path(conn, path),
                (None, None) => return Ok(None),
            };
            Ok(vol.ok().and_then(|v| {
                let capacity = v.get_info().ok()?.capacity;
                Some((v.get_path().ok()?, capacity))
            }))
        })
        .await
        .ok()
        .flatten()
    }

    /// `define_*(do_hotplug=False)` for the simple devices: `edit` runs
    /// on the device `find_device` matches in the inactive config.
    pub async fn define_device(
        &self,
        id: &virtinst_core::devices::DeviceId,
        edit: impl Fn(&mut virtinst_xml::Document, virtinst_xml::Element) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut guest = self.make_xmlobj_to_define().await?;
        let root = guest.root();
        let dev = virtinst_core::devices::find_device(&guest.doc, root, id)
            .ok_or("Could not find specified device in the inactive VM configuration")?;
        edit(&mut guest.doc, dev)?;
        self.redefine_xmlobj(guest).await
    }

    /// `replace_device_xml`: the XML editor's edit of one device,
    /// swapped in for it in the inactive config.
    pub async fn replace_device_xml(
        &self,
        id: &virtinst_core::devices::DeviceId,
        newxml: String,
    ) -> Result<(), String> {
        let newdoc = virtinst_xml::parse_libvirt_xml(&newxml).map_err(|e| e.to_string())?;
        let newroot = newdoc.root_element().ok_or("Empty device XML")?;
        let mut guest = self.make_xmlobj_to_define().await?;
        let root = guest.root();
        let Some(editdev) = virtinst_core::devices::find_device(&guest.doc, root, id) else {
            return Ok(());
        };
        let devices = editdev.parent(&guest.doc).ok_or("Device has no parent")?;
        let idx = devices
            .children(&guest.doc)
            .iter()
            .position(|n| *n == virtinst_xml::Node::Element(editdev))
            .ok_or("Device not found under its parent")?;
        let newdev = virtinst_core::xmlbuilder::copy_element(&newdoc, newroot, &mut guest.doc);
        devices
            .insert_child(&mut guest.doc, idx, newdev.into())
            .map_err(|e| e.to_string())?;
        editdev.detach(&mut guest.doc).map_err(|e| e.to_string())?;
        self.redefine_xmlobj(guest).await
    }

    /// `define_xml`: the XML editor's edit of the whole domain.
    pub async fn define_xml(&self, xml: String) -> Result<(), String> {
        if self.is_virtinst() {
            return self.virtinst_define(xml).await;
        }
        let handle = self.handle()?;
        gio::spawn_blocking(move || {
            Domain::define_xml(handle.connect(), &xml)
                .map(|_| ())
                .map_err(|e| e.message().to_string())
        })
        .await
        .map_err(|_| "Worker thread panicked".to_string())??;
        self.ensure_latest_xml().await;
        Ok(())
    }

    /// `get_xml_to_define`: the inactive XML, as the XML editor shows it.
    pub async fn get_xml_to_define(&self) -> Result<String, String> {
        if let Some((xml, _)) = self.virtinst_backend() {
            return Ok(xml);
        }
        self.with_domain(|_, dom| {
            dom.get_xml_desc(sys::VIR_DOMAIN_XML_INACTIVE | sys::VIR_DOMAIN_XML_SECURE)
                .map_err(|e| e.message().to_string())
        })
        .await
    }

    /// `remove_device`: drop the device from the inactive config. A
    /// device only the running VM has is left alone, as upstream's
    /// `_lookup_device_to_define` does.
    pub async fn remove_device(&self, id: &virtinst_core::devices::DeviceId) -> Result<(), String> {
        let mut guest = self.make_xmlobj_to_define().await?;
        let root = guest.root();
        let Some(dev) = virtinst_core::devices::find_device(&guest.doc, root, id) else {
            let live = self.xmlobj().and_then(|x| {
                virtinst_core::devices::find_device(&x.doc, x.root(), id).map(|_| ())
            });
            if live.is_some() {
                glib::g_debug!(
                    "virt-manager",
                    "Device in active config but not inactive config."
                );
                return Ok(());
            }
            return Err("Could not find specified device in the inactive VM configuration".into());
        };
        let devices = dev.parent(&guest.doc).ok_or("Device has no parent")?;
        virtinst_core::devices::remove_device(&mut guest.doc, devices, dev);
        self.redefine_xmlobj(guest).await
    }

    /// `detach_device`: hot-unplug `devxml` from the running VM.
    pub async fn detach_device(&self, devxml: String) -> Result<(), String> {
        if !self.is_active() {
            return Ok(());
        }
        glib::g_debug!("virt-manager", "detach_device with xml=\n{devxml}");
        self.with_domain(move |_, dom| {
            dom.detach_device(&devxml)
                .map(|_| ())
                .map_err(|e| e.message().to_string())
        })
        .await?;
        self.ensure_latest_xml().await;
        Ok(())
    }

    /// `attach_device`: hotplug `devxml` into the running VM.
    pub async fn attach_device(&self, devxml: String) -> Result<(), String> {
        if !self.is_active() {
            return Ok(());
        }
        glib::g_debug!("virt-manager", "attach_device with xml=\n{devxml}");
        self.with_domain(move |_, dom| {
            dom.attach_device(&devxml)
                .map(|_| ())
                .map_err(|e| e.message().to_string())
        })
        .await?;
        self.ensure_latest_xml().await;
        Ok(())
    }

    /// `add_device`: append the device `build` makes to the inactive
    /// config.
    pub async fn add_device(
        &self,
        build: impl Fn(&mut virtinst_xml::Document) -> Result<virtinst_xml::Element, String>,
    ) -> Result<(), String> {
        let mut guest = self.make_xmlobj_to_define().await?;
        let root = guest.root();
        let dev = build(&mut guest.doc)?;
        let devices = virtinst_xml::resolve_or_create_path(&mut guest.doc, root, &["devices"]);
        devices
            .push_child(&mut guest.doc, dev)
            .map_err(|e| e.to_string())?;
        self.redefine_xmlobj(guest).await
    }

    /// `define_*(do_hotplug=True)`, then `hotplug(device=...)`: the
    /// edited live device goes to `updateDeviceFlags`.
    pub async fn hotplug_device(
        &self,
        id: &virtinst_core::devices::DeviceId,
        edit: impl Fn(&mut virtinst_xml::Document, virtinst_xml::Element) -> Result<(), String>,
    ) -> Result<(), String> {
        if !self.is_active() {
            return Ok(());
        }
        let x = self.xmlobj().ok_or("No domain XML")?;
        let mut doc = virtinst_xml::parse_libvirt_xml(&x.xml).map_err(|e| e.to_string())?;
        let root = doc.root_element().ok_or("Empty domain XML")?;
        let dev = virtinst_core::devices::find_device(&doc, root, id)
            .ok_or("Could not find the device in the running VM")?;
        edit(&mut doc, dev)?;
        let devxml = virtinst_xml::element_to_string(doc, dev).map_err(|e| e.to_string())?;
        glib::g_debug!("virt-manager", "update_device with xml=\n{devxml}");
        self.with_domain(move |_, dom| {
            dom.update_device_flags(&devxml, sys::VIR_DOMAIN_DEVICE_MODIFY_LIVE)
                .map(|_| ())
                .map_err(|e| e.message().to_string())
        })
        .await?;
        self.ensure_latest_xml().await;
        Ok(())
    }

    /// `define_network` for the persistent config.
    pub async fn define_network(&self, id: NicId, edits: NetworkEdits) -> Result<(), String> {
        let mut guest = self.make_xmlobj_to_define().await?;
        let root = guest.root();
        let iface = virtinst_core::devices::network::find_interface(
            &guest.doc,
            root,
            id.mac.as_deref(),
            id.idx,
        )
        .ok_or("Could not find specified device in the inactive VM configuration")?;
        edit_network(&mut guest.doc, iface, &edits);
        self.redefine_xmlobj(guest).await
    }

    /// `define_network(do_hotplug=True)`.
    pub async fn hotplug_network(&self, id: NicId, edits: NetworkEdits) -> Result<(), String> {
        if !self.is_active() {
            return Ok(());
        }
        let x = self.xmlobj().ok_or("No domain XML")?;
        let mut doc = virtinst_xml::parse_libvirt_xml(&x.xml).map_err(|e| e.to_string())?;
        let root = doc.root_element().ok_or("Empty domain XML")?;
        let iface =
            virtinst_core::devices::network::find_interface(&doc, root, id.mac.as_deref(), id.idx)
                .ok_or("Could not find the device in the running VM")?;
        edit_network(&mut doc, iface, &edits);
        let devxml = virtinst_xml::element_to_string(doc, iface).map_err(|e| e.to_string())?;
        glib::g_debug!("virt-manager", "update_device with xml=\n{devxml}");
        self.with_domain(move |_, dom| {
            dom.update_device_flags(&devxml, sys::VIR_DOMAIN_DEVICE_MODIFY_LIVE)
                .map(|_| ())
                .map_err(|e| e.message().to_string())
        })
        .await?;
        self.ensure_latest_xml().await;
        Ok(())
    }

    /// `agent_ready`: the guest agent channel reports `connected`.
    fn agent_ready(&self) -> bool {
        let Some(x) = self.xmlobj() else {
            return false;
        };
        let doc = &x.doc;
        virtinst_xml::resolve_path(doc, x.root(), &["devices"])
            .map(|d| d.find_all(doc, "channel"))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|c| c.find(doc, "target"))
            .any(|t| {
                t.attribute(doc, "name") == Some("org.qemu.guest_agent.0")
                    && t.attribute(doc, "state") == Some("connected")
            })
    }

    /// `get_ips` / `refresh_ips`: the NIC's addresses from the guest
    /// agent, then the network's DHCP leases, then the ARP table.
    pub async fn get_ips(
        &self,
        mac: String,
        network: Option<String>,
    ) -> (Option<String>, Option<String>) {
        let active = self.is_active();
        let agent = active && self.agent_ready();
        self.with_domain(move |conn, dom| {
            let addrs = |src: u32| {
                if active {
                    dom.interface_addresses(src, 0).unwrap_or_default()
                } else {
                    vec![]
                }
            };
            if agent {
                let r =
                    extract_dom_ips(&addrs(sys::VIR_DOMAIN_INTERFACE_ADDRESSES_SRC_AGENT), &mac);
                if r.0.is_some() || r.1.is_some() {
                    return Ok(r);
                }
            }
            if let Some(net) = network
                .as_deref()
                .and_then(|n| virt::network::Network::lookup_by_name(conn, n).ok())
            {
                let (mut ipv4, mut ipv6) = (None, None);
                for (lmac, ltype, ip) in dhcp_leases(&net) {
                    if lmac == mac {
                        if ltype == sys::VIR_IP_ADDR_TYPE_IPV4 as i32 {
                            ipv4 = Some(ip);
                        } else if ltype == sys::VIR_IP_ADDR_TYPE_IPV6 as i32 {
                            ipv6 = Some(ip);
                        }
                    }
                }
                if ipv4.is_some() || ipv6.is_some() {
                    return Ok((ipv4, ipv6));
                }
            }
            Ok(extract_dom_ips(
                &addrs(sys::VIR_DOMAIN_INTERFACE_ADDRESSES_SRC_ARP),
                &mac,
            ))
        })
        .await
        .unwrap_or((None, None))
    }

    /// `define_disk` for the persistent config.
    pub async fn define_disk(&self, id: DiskId, edits: DiskEdits) -> Result<(), String> {
        let mut guest = self.make_xmlobj_to_define().await?;
        let root = guest.root();
        let disk = virtinst_core::devices::disk::find_disk(
            &guest.doc,
            root,
            id.target.as_deref(),
            id.bus.as_deref(),
        )
        .ok_or("Could not find specified device in the inactive VM configuration")?;
        if let Some(bus) = &edits.bus {
            change_disk_bus(&mut guest.doc, root, disk, bus)?;
        }
        let sync = match &edits.path {
            Some(Some(path)) => {
                let path = path.clone();
                let is_local = self.is_local_conn();
                Some(
                    self.with_conn(move |conn| Ok(path_props(conn, &path, is_local)))
                        .await?,
                )
            }
            _ => Some(virtinst_core::devices::disk::PathProps {
                dev_type: "file".into(),
                format: None,
            }),
        };
        edit_disk(
            &mut guest.doc,
            disk,
            &edits,
            sync.as_ref(),
            self.disk_driver_name_qemu(),
        );
        self.redefine_xmlobj(guest).await
    }

    /// `define_disk(do_hotplug=True)`: the same edits on the live
    /// device, pushed with `updateDeviceFlags`.
    pub async fn hotplug_disk(&self, id: DiskId, edits: DiskEdits) -> Result<(), String> {
        if !self.is_active() {
            return Ok(());
        }
        let x = self.xmlobj().ok_or("No domain XML")?;
        let mut doc = virtinst_xml::parse_libvirt_xml(&x.xml).map_err(|e| e.to_string())?;
        let root = doc.root_element().ok_or("Empty domain XML")?;
        let disk = virtinst_core::devices::disk::find_disk(
            &doc,
            root,
            id.target.as_deref(),
            id.bus.as_deref(),
        )
        .ok_or("Could not find the device in the running VM")?;
        edit_disk(&mut doc, disk, &edits, None, false);
        let devxml = virtinst_xml::element_to_string(doc, disk).map_err(|e| e.to_string())?;
        glib::g_debug!("virt-manager", "update_device with xml=\n{devxml}");
        self.with_domain(move |_, dom| {
            dom.update_device_flags(&devxml, sys::VIR_DOMAIN_DEVICE_MODIFY_LIVE)
                .map(|_| ())
                .map_err(|e| e.message().to_string())
        })
        .await?;
        self.ensure_latest_xml().await;
        Ok(())
    }

    /// `define_overview`: `None` leaves a field alone, `Some(None)`
    /// clears it.
    pub async fn define_overview(
        &self,
        description: Option<Option<String>>,
        title: Option<Option<String>>,
        nvram: Option<String>,
    ) -> Result<(), String> {
        self.define_overview_full(description, title, nvram, None, None)
            .await
    }

    /// `define_overview` with the customize dialog's `machine` and
    /// `(loader, firmware)` edits.
    pub async fn define_overview_full(
        &self,
        description: Option<Option<String>>,
        title: Option<Option<String>>,
        nvram: Option<String>,
        machine: Option<String>,
        firmware: Option<(Option<String>, Option<String>)>,
    ) -> Result<(), String> {
        let mut guest = self.make_xmlobj_to_define().await?;
        let mut old_machine = None;
        if let Some(machine) = machine {
            old_machine = guest.guest.os.machine.replace(machine);
            self.imp().domcaps.replace(None);
            guest.write_os();
        }
        if let Some(description) = description {
            guest.guest.description = description.filter(|s| !s.is_empty());
        }
        if let Some(title) = title {
            guest.guest.title = title.filter(|s| !s.is_empty());
        }
        if let Some((loader, firmware)) = firmware {
            let root = guest.root();
            let os_el = virtinst_xml::resolve_or_create_path(&mut guest.doc, root, &["os"]);
            guest.guest.os.firmware = firmware.clone();
            if firmware.is_none() {
                os_el.mut_attributes(&mut guest.doc).remove("firmware");
            }
            match loader {
                None => {
                    guest.guest.os.loader = None;
                    // But if switching to firmware=efi we may need to
                    // preserve NVRAM paths, so skip clearing all the properties
                    // and let libvirt do it for us.
                    if firmware.is_none() {
                        // `disable_uefi`
                        let os = &mut guest.guest.os;
                        os.loader_ro = None;
                        os.loader_type = None;
                        os.loader_secure = None;
                        os.nvram = None;
                        os.nvram_template = None;
                        os.firmware_features.clear();
                        for tag in ["firmware", "nvram", "loader"] {
                            virtinst_xml::remove_path_if_present(&mut guest.doc, os_el, &[tag])
                                .map_err(|e| e.to_string())?;
                        }
                    }
                }
                // Implies UEFI
                Some(path) => {
                    // `set_uefi_path`
                    let os = &mut guest.guest.os;
                    os.loader_ro = Some(true);
                    os.loader_type = Some("pflash".into());
                    let secboot = path.contains("secboot");
                    os.loader = Some(path);
                    let is_x86 = matches!(os.arch.as_deref(), Some("i686" | "x86_64"));
                    if is_x86 && secboot {
                        guest.guest.features.smm = Some(virtinst_xml::OnOff(true));
                        let os = &mut guest.guest.os;
                        os.loader_secure = Some(true);
                        if !os.machine.as_deref().is_some_and(|m| m.contains("q35")) {
                            eprintln!(
                                "Changing machine type from '{}' to 'q35' which is required \
                                 for UEFI secure boot.",
                                os.machine.as_deref().unwrap_or("None")
                            );
                            os.machine = Some("q35".into());
                        }
                    }
                }
            }
            guest.write_os();
        }
        if let Some(nvram) = nvram {
            guest.guest.os.nvram = Some(nvram);
            guest.write_os();
        }

        let new_machine = guest.guest.os.machine.clone();
        let root = guest.root();
        let devices = virtinst_xml::resolve_or_create_path(&mut guest.doc, root, &["devices"]);
        let pci: Vec<virtinst_xml::Element> = devices
            .find_all(&guest.doc, "controller")
            .into_iter()
            .filter(|c| c.attribute(&guest.doc, "type") == Some("pci"))
            .collect();
        match (old_machine.as_deref(), new_machine.as_deref()) {
            (Some("pc"), Some("q35")) if pci.is_empty() => {
                // `add_q35_pcie_controllers`
                for model in std::iter::once("pcie-root").chain(["pcie-root-port"; 14]) {
                    let ctrl = virtinst_xml::Element::new(&mut guest.doc, "controller");
                    ctrl.set_attribute(&mut guest.doc, "type", "pci");
                    ctrl.set_attribute(&mut guest.doc, "model", model);
                    devices
                        .push_child(&mut guest.doc, ctrl)
                        .map_err(|e| e.to_string())?;
                }
            }
            (Some("q35"), Some("pc")) => {
                for ctrl in pci {
                    let model = ctrl.attribute(&guest.doc, "model");
                    if matches!(model, Some("pcie-root" | "pcie-root-port")) {
                        ctrl.detach(&mut guest.doc).map_err(|e| e.to_string())?;
                    }
                }
            }
            _ => {}
        }
        self.redefine_xmlobj(guest).await
    }

    /// `hotplug(description=..., title=...)`: the live metadata.
    pub async fn hotplug_metadata(
        &self,
        description: Option<String>,
        title: Option<String>,
    ) -> Result<(), String> {
        if !self.is_active() {
            return Ok(());
        }
        self.with_domain(move |_, dom| {
            let flags = sys::VIR_DOMAIN_AFFECT_LIVE | sys::VIR_DOMAIN_AFFECT_CONFIG;
            let set = |kind: u32, val: &str| {
                dom.set_metadata(kind as i32, Some(val), None, None, flags)
                    .map(|_| ())
                    .map_err(|e| e.message().to_string())
            };
            if let Some(d) = &description {
                set(sys::VIR_DOMAIN_METADATA_DESCRIPTION, d)?;
            }
            if let Some(t) = &title {
                set(sys::VIR_DOMAIN_METADATA_TITLE, t)?;
            }
            Ok(())
        })
        .await?;
        self.ensure_latest_xml().await;
        Ok(())
    }

    /// `rename_domain`: validate, carry the NVRAM file over to the new
    /// name, then `define_name`.
    pub async fn rename_domain(&self, new_name: &str) -> Result<(), String> {
        if new_name == self.get_name() {
            return Ok(());
        }
        validate_generic_name("Guest", new_name)?;
        let name = new_name.to_string();
        self.with_conn(move |conn| match Domain::lookup_by_name(conn, &name) {
            Ok(_) => Err(format!("Guest name '{name}' is already in use.")),
            Err(_) => Ok(()),
        })
        .await?;
        if self.is_virtinst() {
            // vmmDomainVirtinst.define_name
            let mut xmlobj = self.make_xmlobj_to_define().await?;
            xmlobj.guest.name = Some(new_name.to_string());
            return self.redefine_xmlobj(xmlobj).await;
        }

        let xmlobj = self.make_xmlobj_to_define().await?;
        let has_nvram = xmlobj.is_uefi() || xmlobj.guest.os.nvram.is_some();
        let has_tpm_state = xmlobj
            .guest
            .devices
            .tpms
            .iter()
            .any(|t| t.backend_type.as_deref() == Some("emulator"));
        let uri = self.conn().map(|c| c.get_uri()).unwrap_or_default();
        let parsed = virtinst_core::connection::Uri::parse(&uri);
        let is_local = !parsed.is_remote();
        let old_nvram_path = xmlobj.guest.os.nvram.clone().unwrap_or_else(|| {
            // Probably using firmware=efi which doesn't put nvram
            // path in the XML on older libvirt. Build the implied path
            let root = if parsed.path == "/session" {
                std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| {
                    glib::home_dir()
                        .join(".local/share/libvirt")
                        .to_string_lossy()
                        .into_owned()
                })
            } else {
                "/var/lib/libvirt".to_string()
            };
            let driver = parsed
                .scheme
                .split('+')
                .next()
                .unwrap_or("qemu")
                .to_string();
            format!("{root}/{driver}/nvram/{}_VARS.fd", self.get_name())
        });

        let name = new_name.to_string();
        let renamed = self
            .with_domain(move |conn, dom| {
                let copied = if has_nvram {
                    copy_nvram_file(conn, &old_nvram_path, &name, is_local)?
                } else {
                    None
                };
                let res = define_name(conn, dom, &name, has_nvram, has_tpm_state);
                if let Err(e) = res {
                    if let Some((new, _)) = &copied {
                        if let Err(warn) = new.delete() {
                            eprintln!("rename failed and new nvram was not removed: '{warn}'");
                        }
                    }
                    return Err(e);
                }
                let Some((new, old)) = copied else {
                    return Ok(None);
                };
                if let Err(warn) = old.delete() {
                    eprintln!("old nvram file was not removed: '{warn}'");
                }
                Ok(Some(new.path()))
            })
            .await;
        self.ensure_latest_xml().await;
        if let Some(new_path) = renamed? {
            self.define_overview(None, None, Some(new_path)).await?;
        }
        Ok(())
    }

    /// `get_pretty_hv_type`.
    pub fn get_pretty_hv_type(&self) -> String {
        let Some(x) = self.xmlobj() else {
            return String::new();
        };
        let gtype = x.guest.os.os_type.clone().unwrap_or_default();
        let domtype = x.guest.domain_type.clone().unwrap_or_default();
        pretty_hv(&gtype, &domtype)
    }

    // Per-VM console settings: -1 means "use the global preference".
    fn pervm(&self) -> gio::Settings {
        crate::engine::get().config().vm_settings(&self.get_uuid())
    }

    pub fn get_console_scaling(&self) -> i32 {
        match self.pervm().int("scaling") {
            -1 => crate::engine::get().config().get_console_scaling(),
            v => v,
        }
    }
    pub fn set_console_scaling(&self, val: i32) {
        let _ = self.pervm().set_int("scaling", val);
    }
    pub fn on_console_scaling_changed(&self, cb: impl Fn() + 'static) -> glib::SignalHandlerId {
        self.pervm()
            .connect_changed(Some("scaling"), move |_, _| cb())
    }

    pub fn get_console_resizeguest(&self) -> i32 {
        match self.pervm().int("resize-guest") {
            -1 => crate::engine::get().config().get_console_resizeguest(),
            v => v,
        }
    }
    pub fn set_console_resizeguest(&self, val: i32) {
        let _ = self.pervm().set_int("resize-guest", val);
    }
    pub fn on_console_resizeguest_changed(&self, cb: impl Fn() + 'static) -> glib::SignalHandlerId {
        self.pervm()
            .connect_changed(Some("resize-guest"), move |_, _| cb())
    }

    pub fn get_console_autoconnect(&self) -> bool {
        match self.pervm().int("autoconnect") {
            -1 => crate::engine::get().config().get_console_autoconnect(),
            v => v != 0,
        }
    }
    pub fn set_console_autoconnect(&self, val: bool) {
        let _ = self.pervm().set_int("autoconnect", val as i32);
    }
    /// Upstream listens on `/resize-guest` here, a copy/paste slip; this
    /// listens on the key it's named after.
    pub fn on_console_autoconnect_changed(&self, cb: impl Fn() + 'static) -> glib::SignalHandlerId {
        self.pervm()
            .connect_changed(Some("autoconnect"), move |_, _| cb())
    }

    /// `get_console_username`: the keyring keeps only the password.
    pub fn get_console_username(&self) -> String {
        self.pervm().string("console-username").to_string()
    }
    pub fn set_console_username(&self, username: &str) {
        let _ = self.pervm().set_string("console-username", username);
    }
    pub fn del_console_username(&self) {
        let _ = self.pervm().set_string("console-username", "");
    }

    pub fn get_details_window_size(&self) -> (i32, i32) {
        self.pervm().get::<(i32, i32)>("vm-window-size")
    }
    pub fn set_details_window_size(&self, w: i32, h: i32) {
        let _ = self.pervm().set("vm-window-size", (w, h));
    }

    /// Runs `action` on a worker thread, then schedules a priority tick
    /// so the list reflects the new state without waiting a full
    /// interval. The error string is libvirt's message, for
    /// `asyncjob::simple_async_noshow`'s dialog.
    pub async fn run_action(&self, action: DomainAction) -> Result<(), String> {
        if matches!(
            action,
            DomainAction::Shutdown
                | DomainAction::Reboot
                | DomainAction::Destroy
                | DomainAction::Reset
                | DomainAction::Save
        ) {
            self.imp().install_abort.set(true);
        }
        let conn = self.conn().ok_or("Connection closed")?;
        let handle = conn.handle().ok_or("Not connected")?;
        let uuid = self.get_uuid();
        let res = gio::spawn_blocking(move || {
            let dom = Domain::lookup_by_uuid_string(handle.connect(), &uuid)?;
            action.run(&dom)
        })
        .await
        .map_err(|_| "Worker thread panicked".to_string())?;
        conn.tick();
        res.map_err(|e| e.message().to_string())
    }

    /// `open_graphics_fd`: a connected socket to the `idx`th graphical
    /// console (`virDomainOpenGraphicsFD`), which the caller owns.
    /// `open_console`: a new non-blocking stream on the named console
    /// device (the first one when `None`). The caller owns the stream.
    pub fn open_console(&self, devname: Option<&str>) -> Result<sys::virStreamPtr, String> {
        let conn = self.conn().ok_or("Connection closed")?;
        let handle = conn.handle().ok_or("Not connected")?;
        let dom = Domain::lookup_by_uuid_string(handle.connect(), &self.get_uuid())
            .map_err(|e| e.message().to_string())?;
        let name = devname.and_then(|n| std::ffi::CString::new(n).ok());
        let last_error = || virt::error::Error::last_error().message().to_string();
        unsafe {
            let stream = sys::virStreamNew(handle.connect().as_ptr(), sys::VIR_STREAM_NONBLOCK);
            if stream.is_null() {
                return Err(last_error());
            }
            let ret = sys::virDomainOpenConsole(
                dom.as_ptr(),
                name.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()),
                stream,
                0,
            );
            if ret < 0 {
                let err = last_error();
                sys::virStreamFree(stream);
                return Err(err);
            }
            Ok(stream)
        }
    }

    pub async fn open_graphics_fd(&self, idx: u32) -> Result<i32, String> {
        let conn = self.conn().ok_or("Connection closed")?;
        let handle = conn.handle().ok_or("Not connected")?;
        let uuid = self.get_uuid();
        gio::spawn_blocking(move || {
            let dom = Domain::lookup_by_uuid_string(handle.connect(), &uuid)?;
            dom.open_graphics_fd(idx, 0)
        })
        .await
        .map_err(|_| "Worker thread panicked".to_string())?
        .map(|fd| fd as i32)
        .map_err(|e| e.message().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `test:///default` is one shared in-process driver instance, so
    /// tests that change its domains' state must not overlap.
    static TEST_DRIVER: Mutex<()> = Mutex::new(());

    #[test]
    fn validate_generic_name_like_upstream() {
        assert!(validate_generic_name("Guest", "ok-name").is_ok());
        assert_eq!(
            validate_generic_name("Guest", "").unwrap_err(),
            "A name must be specified for the Guest"
        );
        assert_eq!(
            validate_generic_name("Guest", "a/b").unwrap_err(),
            "Guest name 'a/b' can not contain '/' character."
        );
    }

    #[test]
    fn xml_to_define_edits_round_trip() {
        let xml = "<domain type='test'><name>x</name><title>t</title>\
                   <os><type>hvm</type><loader type='pflash'>/l.fd</loader></os></domain>";
        let mut x = XmlToDefine::parse(xml.to_string()).unwrap();
        assert!(x.is_uefi());
        x.guest.title = None;
        x.guest.description = Some("d".into());
        x.guest.os.nvram = Some("/n.fd".into());
        x.write_os();
        let out = x.get_xml().unwrap();
        assert!(!out.contains("<title>"), "{out}");
        assert!(out.contains("<description>d</description>"), "{out}");
        assert!(out.contains("<nvram>/n.fd</nvram>"), "{out}");
        assert!(out.contains("pflash"), "{out}");
    }

    #[test]
    fn shared_mem_toggles_memfd_backing() {
        let mut doc = virtinst_xml::parse_libvirt_xml(
            "<domain><memoryBacking><locked/></memoryBacking></domain>",
        )
        .unwrap();
        let root = doc.root_element().unwrap();
        edit_shared_mem(&mut doc, root, true);
        let x = doc.write_str().unwrap();
        assert!(
            x.contains("<source type=\"memfd\"/>") && x.contains("<access mode=\"shared\"/>"),
            "{x}"
        );
        edit_shared_mem(&mut doc, root, false);
        let x = doc.write_str().unwrap();
        assert!(
            !x.contains("source") && !x.contains("access") && x.contains("<locked/>"),
            "{x}"
        );
        let mut doc = virtinst_xml::parse_libvirt_xml("<domain/>").unwrap();
        let root = doc.root_element().unwrap();
        edit_shared_mem(&mut doc, root, true);
        edit_shared_mem(&mut doc, root, false);
        assert!(!doc.write_str().unwrap().contains("memoryBacking"));
    }

    #[test]
    fn local_nvram_copy_uses_the_new_name() {
        let dir = std::env::temp_dir().join(format!("vmm-nvram-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let old = dir.join("old_VARS.fd");
        std::fs::write(&old, b"vars").unwrap();
        let mut conn = Connect::open(Some("test:///default")).unwrap();
        let (new, oldf) = copy_nvram_file(&conn, old.to_str().unwrap(), "new", true)
            .unwrap()
            .expect("copied");
        assert_eq!(new.path(), dir.join("new_VARS.fd").to_str().unwrap());
        assert_eq!(std::fs::read(new.path()).unwrap(), b"vars");
        oldf.delete().unwrap();
        assert!(!old.exists());
        new.delete().unwrap();
        let _ = std::fs::remove_dir(&dir);
        conn.close().unwrap();
    }

    #[test]
    fn define_name_renames_and_keeps_autostart() {
        let _guard = TEST_DRIVER.lock().unwrap_or_else(|e| e.into_inner());
        let mut conn = Connect::open(Some("test:///default")).unwrap();
        let xml = "<domain type='test'><name>vmm-rename-a</name><memory>1024</memory>\
                   <os><type>hvm</type></os></domain>";
        let dom = Domain::define_xml(&conn, xml).unwrap();
        dom.set_autostart(true).unwrap();
        define_name(&conn, &dom, "vmm-rename-b", false, false).unwrap();
        assert!(Domain::lookup_by_name(&conn, "vmm-rename-a").is_err());
        let renamed = Domain::lookup_by_name(&conn, "vmm-rename-b").unwrap();
        assert!(renamed.get_autostart().unwrap());
        renamed.undefine().unwrap();
        conn.close().unwrap();
    }

    /// libvirt's built-in `test:///default` driver ships one running
    /// domain called `test`.
    #[test]
    fn fetch_all_against_the_builtin_test_driver() {
        let _guard = TEST_DRIVER.lock().unwrap_or_else(|e| e.into_inner());
        let mut conn = Connect::open(Some("test:///default")).expect("test driver opens");
        let snaps = DomainSnapshot::fetch_all(&conn).expect("listing works");
        let test = snaps
            .iter()
            .find(|s| s.name == "test")
            .expect("default test domain");
        assert_eq!(test.state, sys::VIR_DOMAIN_RUNNING);
        assert!(test.id.is_some());
        assert_eq!(test.uuid.len(), 36);
        conn.close().unwrap();
    }

    #[test]
    fn actions_change_state_on_the_test_driver() {
        let _guard = TEST_DRIVER.lock().unwrap_or_else(|e| e.into_inner());
        let mut conn = Connect::open(Some("test:///default")).expect("test driver opens");
        let dom = Domain::lookup_by_name(&conn, "test").unwrap();
        DomainAction::Suspend.run(&dom).unwrap();
        assert_eq!(dom.get_state().unwrap().0, sys::VIR_DOMAIN_PAUSED);
        DomainAction::Resume.run(&dom).unwrap();
        DomainAction::Destroy.run(&dom).unwrap();
        assert_eq!(dom.get_state().unwrap().0, sys::VIR_DOMAIN_SHUTOFF);
        DomainAction::Startup.run(&dom).unwrap();
        assert_eq!(dom.get_state().unwrap().0, sys::VIR_DOMAIN_RUNNING);
        drop(dom);
        conn.close().unwrap();
    }
}
