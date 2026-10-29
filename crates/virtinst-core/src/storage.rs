//! Storage pools and volumes, for new disks: upstream's
//! `virtinst/storage.py` (`StoragePool.lookup_default_pool`,
//! `StorageVolume.find_free_name`, the volume XML) and
//! `virtinst/diskbackend.py` (`manage_path`). Every function here that
//! takes a `Connect` blocks on libvirt.

use virt::connect::Connect;
use virt::storage_pool::StoragePool;
use virt::storage_vol::StorageVol;
use virt::sys;

use crate::devices::disk::PathProps;
use crate::xmlbuilder::{child, prop, Kind, Value, XmlClass, XmlObject};

/// The parts of a pool's XML these helpers read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PoolXml {
    pub name: String,
    pub pool_type: String,
    pub target_path: Option<String>,
    pub capacity: u64,
    pub allocation: u64,
    pub available: u64,
    /// `source_name`.
    pub source_name: Option<String>,
    /// `source_path`: the source property the pool type uses.
    pub source_path: Option<String>,
    /// `auth_type`, `auth_username`, `auth_secret_uuid`.
    pub auth_type: Option<String>,
    pub auth_username: Option<String>,
    pub auth_secret_uuid: Option<String>,
    /// `hosts`: `(name, port)` of each `<source><host>`.
    pub hosts: Vec<(Option<String>, Option<String>)>,
}

impl PoolXml {
    pub fn parse(xml: &str) -> Option<PoolXml> {
        let doc = virtinst_xml::parse_libvirt_xml(xml).ok()?;
        let root = doc.root_element()?;
        let text = |path: &[&str]| {
            virtinst_xml::resolve_path(&doc, root, path).map(|e| e.text_content(&doc))
        };
        let num = |tag: &str| text(&[tag]).and_then(|t| t.trim().parse().ok());
        Some(PoolXml {
            name: text(&["name"])?,
            pool_type: root.attribute(&doc, "type").unwrap_or_default().to_string(),
            target_path: text(&["target", "path"]),
            capacity: num("capacity").unwrap_or(0),
            allocation: num("allocation").unwrap_or(0),
            available: num("available").unwrap_or(0),
            source_name: text(&["source", "name"]),
            source_path: {
                let pool_type = root.attribute(&doc, "type").unwrap_or_default();
                let (tag, attr) = match pool_type {
                    "netfs" | "gluster" => ("dir", "path"),
                    "scsi" => ("adapter", "name"),
                    _ => ("device", "path"),
                };
                virtinst_xml::resolve_path(&doc, root, &["source", tag])
                    .and_then(|e| e.attribute(&doc, attr).map(str::to_string))
            },
            auth_type: virtinst_xml::resolve_path(&doc, root, &["source", "auth"])
                .and_then(|e| e.attribute(&doc, "type").map(str::to_string)),
            auth_username: virtinst_xml::resolve_path(&doc, root, &["source", "auth"])
                .and_then(|e| e.attribute(&doc, "username").map(str::to_string)),
            auth_secret_uuid: virtinst_xml::resolve_path(&doc, root, &["source", "auth", "secret"])
                .and_then(|e| e.attribute(&doc, "uuid").map(str::to_string)),
            hosts: virtinst_xml::resolve_path(&doc, root, &["source"])
                .map(|src| {
                    src.find_all(&doc, "host")
                        .into_iter()
                        .map(|h| {
                            (
                                h.attribute(&doc, "name").map(str::to_string),
                                h.attribute(&doc, "port").map(str::to_string),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    /// `StoragePool.get_disk_type`: the volume type the pool makes.
    pub fn get_disk_type(&self) -> &'static str {
        match self.pool_type.as_str() {
            "disk" | "logical" | "scsi" | "mpath" | "zfs" => "block",
            "gluster" | "rbd" | "iscsi" => "network",
            _ => "file",
        }
    }
}

/// `POOL_TYPE_DESCS`/`vmmStoragePool.pretty_type`.
pub fn pretty_pool_type(pool_type: &str) -> String {
    let desc = match pool_type {
        "dir" => "Filesystem Directory",
        "fs" => "Pre-Formatted Block Device",
        "netfs" => "Network Exported Directory",
        "logical" => "LVM Volume Group",
        "disk" => "Physical Disk Device",
        "iscsi" => "iSCSI Target",
        "scsi" => "SCSI Host Adapter",
        "mpath" => "Multipath Device Enumerator",
        "gluster" => "Gluster Filesystem",
        "rbd" => "RADOS Block Device/Ceph",
        "zfs" => "ZFS Pool",
        _ => return format!("{pool_type} pool"),
    };
    desc.to_string()
}

/// `vmmStoragePool.supports_volume_creation`: with `clone`, whether
/// the pool can clone volumes (`virStorageVolCreateXMLFrom`).
pub fn supports_volume_creation(pool_type: &str, clone: bool) -> bool {
    matches!(
        pool_type,
        "dir" | "fs" | "netfs" | "disk" | "logical" | "rbd"
    ) || (!clone && pool_type == "zfs")
}

/// `_pretty_bytes`: GiB above 1 GiB, else MiB.
pub fn pretty_bytes(val: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    if val > 1024 * 1024 * 1024 {
        format!("{:2.2} GiB", val as f64 / GIB)
    } else {
        format!("{:2.2} MiB", val as f64 / (1024.0 * 1024.0))
    }
}

/// The parts of a volume's XML the storage browser shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VolXml {
    pub name: String,
    pub key: String,
    pub target_path: String,
    pub format: Option<String>,
    pub capacity: u64,
}

impl VolXml {
    pub fn parse(xml: &str) -> Option<VolXml> {
        let doc = virtinst_xml::parse_libvirt_xml(xml).ok()?;
        let root = doc.root_element()?;
        let el = |path: &[&str]| virtinst_xml::resolve_path(&doc, root, path);
        let text = |path: &[&str]| el(path).map(|e| e.text_content(&doc));
        Some(VolXml {
            name: text(&["name"])?,
            key: text(&["key"]).unwrap_or_default(),
            target_path: text(&["target", "path"]).unwrap_or_default(),
            format: el(&["target", "format"])
                .and_then(|e| e.attribute(&doc, "type"))
                .map(str::to_string),
            capacity: text(&["capacity"])
                .and_then(|t| t.trim().parse().ok())
                .unwrap_or(0),
        })
    }

    /// `vmmStorageVolume.get_pretty_name`: iSCSI volumes add their key.
    pub fn pretty_name(&self, pool_type: &str) -> String {
        if pool_type != "iscsi" || self.key.is_empty() {
            return self.name.clone();
        }
        format!("{} ({})", self.name, self.key)
    }
}

/// `vmmLibvirtObject.define_name` + `vmmConnection.rename_object` for
/// a pool: undefine it and define its XML under `newname`, putting the
/// old XML back if that fails; autostart carries over.
pub fn rename_pool(conn: &Connect, pool: &StoragePool, newname: &str) -> Result<(), String> {
    let msg = |e: virt::error::Error| e.message().to_string();
    let oldautostart = pool.get_autostart().unwrap_or(false);
    let origxml = pool
        .get_xml_desc(sys::VIR_STORAGE_XML_INACTIVE)
        .map_err(msg)?;
    let mut doc = virtinst_xml::parse_libvirt_xml(&origxml).map_err(|e| e.to_string())?;
    let root = doc.root_element().ok_or("Pool XML has no root")?;
    let name_el = root.find(&doc, "name").ok_or("Pool XML has no name")?;
    if name_el.text_content(&doc) == newname {
        return Ok(());
    }
    name_el.set_text_content(&mut doc, newname);
    let newxml = doc
        .write_str_with_opts(virtinst_xml::WriteOptions {
            write_decl: false,
            ..Default::default()
        })
        .map_err(|e| e.to_string())?;

    pool.undefine().map_err(msg)?;
    let newpool = match StoragePool::define_xml(conn, &newxml, 0) {
        Ok(p) => p,
        Err(renameerr) => {
            return match StoragePool::define_xml(conn, &origxml, 0) {
                Ok(_) => Err(renameerr.message().to_string()),
                Err(fixerr) => Err(format!(
                    "pool rename failed. Attempting to recover also failed.\n\n\
                     Original error: {}\n\nRecover error: {}",
                    renameerr.message(),
                    fixerr.message()
                )),
            };
        }
    };
    newpool.set_autostart(oldautostart).map_err(msg)?;
    Ok(())
}

/// A pool and its XML.
pub struct Pool {
    pub pool: StoragePool,
    pub xml: PoolXml,
}

pub(crate) fn fetch_all_pools(conn: &Connect) -> Vec<Pool> {
    conn.list_all_storage_pools(0)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|pool| {
            let xml = PoolXml::parse(&pool.get_xml_desc(0).ok()?)?;
            Some(Pool { pool, xml })
        })
        .collect()
}

/// `_preferred_default_pool_path`: `images` under the libvirt data
/// root, `/var/lib/libvirt` for a system connection and
/// `~/.local/share/libvirt` for a session one.
pub fn preferred_default_pool_path(is_session: bool) -> String {
    if is_session {
        let data = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| {
            format!("{}/.local/share", std::env::var("HOME").unwrap_or_default())
        });
        format!("{data}/libvirt/images")
    } else {
        "/var/lib/libvirt/images".into()
    }
}

/// `StoragePool.lookup_default_pool`: the pool named `default`, else
/// the one at the default path.
pub fn lookup_default_pool(conn: &Connect, is_session: bool) -> Option<Pool> {
    let path = preferred_default_pool_path(is_session);
    let pools = fetch_all_pools(conn);
    let idx = pools
        .iter()
        .position(|p| p.xml.name == "default")
        .or_else(|| {
            pools
                .iter()
                .position(|p| p.xml.target_path.as_deref() == Some(path.as_str()))
        })?;
    pools.into_iter().nth(idx)
}

/// `StoragePool.build_default_pool`: the default pool, defined (and
/// built, started, autostarted) at the preferred path when missing.
pub fn build_default_pool(conn: &Connect, is_session: bool) -> Result<Pool, String> {
    if let Some(pool) = lookup_default_pool(conn, is_session) {
        return Ok(pool);
    }
    let path = preferred_default_pool_path(is_session);
    log::debug!("Attempting to build default pool with target '{path}'");
    let defpool = NewPool {
        name: "default".into(),
        pool_type: "dir".into(),
        target_path: Some(path.clone()),
        ..Default::default()
    };
    defpool
        .install(conn, true)
        .and_then(|()| {
            fetch_all_pools(conn)
                .into_iter()
                .find(|p| p.xml.name == "default")
                .ok_or_else(|| "pool 'default' not found".to_string())
        })
        .map_err(|e| format!("Couldn't create default storage pool '{path}': {e}"))
}

/// `StoragePool.lookup_pool_by_path`.
pub fn lookup_pool_by_path(conn: &Connect, path: &str) -> Option<Pool> {
    fetch_all_pools(conn)
        .into_iter()
        .find(|p| p.xml.target_path.as_deref() == Some(path))
}

/// `StoragePool.ensure_pool_is_running`.
pub fn ensure_pool_is_running(pool: &StoragePool, refresh: bool) -> Result<(), String> {
    if !pool.is_active().map_err(|e| e.to_string())? {
        pool.create(0).map_err(|e| e.to_string())?;
    }
    if refresh {
        pool.refresh(0).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// `StorageVolume.get_file_extension_for_format`.
pub fn file_extension_for_format(fmt: Option<&str>) -> String {
    match fmt {
        None | Some("") => String::new(),
        Some("raw") => ".img".into(),
        Some(fmt) => format!(".{fmt}"),
    }
}

/// `generatename.generate_name`: `base`, then `base-1`, `base-2`, ...,
/// each with `suffix`, until `collides` says no.
pub fn generate_name(base: &str, suffix: &str, collides: impl Fn(&str) -> bool) -> String {
    std::iter::once(None)
        .chain((1..100_001).map(Some))
        .map(|i| match i {
            Some(i) => format!("{base}-{i}{suffix}"),
            None => format!("{base}{suffix}"),
        })
        .find(|name| !collides(name))
        .unwrap_or_else(|| format!("{base}{suffix}"))
}

pub(crate) fn dirname(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(i) => &path[..i],
        None => "",
    }
}

pub(crate) fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// `StorageVolume.find_free_name`: a volume name in `pool` that no
/// volume has, and that none of `collide_paths` (the guest's disk
/// paths) has under the pool's directory.
pub fn find_free_name(
    pool: &Pool,
    basename_: &str,
    suffix: &str,
    collide_paths: &[String],
) -> Result<String, String> {
    let collidelist: Vec<&str> = collide_paths
        .iter()
        .filter(|p| Some(dirname(p)) == pool.xml.target_path.as_deref())
        .map(|p| basename(p))
        .collect();
    ensure_pool_is_running(&pool.pool, true)?;
    Ok(generate_name(basename_, suffix, |tryname| {
        collidelist.contains(&tryname) || StorageVol::lookup_by_name(&pool.pool, tryname).is_ok()
    }))
}

/// `diskbackend.manage_path`'s result: the absolute path, its volume
/// and the volume's (or the directory's) pool.
pub struct ManagedPath {
    pub path: String,
    pub vol: Option<StorageVol>,
    pub pool: Option<Pool>,
}

pub(crate) fn lookup_vol_by_path(conn: &Connect, path: &str) -> Option<StorageVol> {
    let vol = StorageVol::lookup_by_path(conn, path).ok()?;
    // Make sure it wasn't removed behind libvirt's back.
    vol.get_info().ok()?;
    Some(vol)
}

/// `_check_if_path_managed`.
pub(crate) fn check_if_path_managed(
    conn: &Connect,
    path: &str,
) -> Result<(Option<StorageVol>, Option<Pool>), String> {
    if let Some(vol) = lookup_vol_by_path(conn, path) {
        let pool = StoragePool::lookup_by_volume(&vol).ok().and_then(|pool| {
            let xml = PoolXml::parse(&pool.get_xml_desc(0).ok()?)?;
            Some(Pool { pool, xml })
        });
        return Ok((Some(vol), pool));
    }
    let Some(pool) = lookup_pool_by_path(conn, dirname(path)) else {
        return Ok((None, None));
    };
    // We have the parent pool, but didn't find a volume on first lookup
    // attempt. Refresh the pool and try again, in case we were just out
    // of date or the pool was inactive.
    let vol = match ensure_pool_is_running(&pool.pool, true) {
        Ok(()) => lookup_vol_by_path(conn, path).or_else(|| {
            let name = basename(path);
            let names = pool.pool.list_volumes().unwrap_or_default();
            names
                .iter()
                .any(|n| n == name)
                .then(|| StorageVol::lookup_by_name(&pool.pool, name).ok())
                .flatten()
        }),
        Err(e) => {
            return Err(format!("Cannot use storage {path}: {e}"));
        }
    };
    Ok((vol, Some(pool)))
}

/// `path_is_url`.
pub(crate) fn path_is_url(path: &str) -> bool {
    let Some((scheme, rest)) = path.split_once("://") else {
        return false;
    };
    let _ = rest;
    let mut parts = scheme.split('+');
    let ok = |s: Option<&str>| {
        s.is_some_and(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphabetic()))
    };
    let first = parts.next();
    let second = parts.next();
    ok(first) && (second.is_none() || ok(second)) && parts.next().is_none()
}

/// `_can_auto_manage`.
pub(crate) fn can_auto_manage(path: &str) -> bool {
    if path_is_url(path) {
        return false;
    }
    !["/dev", "/sys", "/proc"]
        .iter()
        .any(|prefix| path == *prefix || path.starts_with(&format!("{prefix}/")))
}

/// `os.path.abspath`.
pub fn abspath(path: &str) -> String {
    let joined = if path.starts_with('/') {
        path.to_string()
    } else {
        let cwd = std::env::current_dir().unwrap_or_default();
        format!("{}/{path}", cwd.display())
    };
    let mut parts: Vec<&str> = vec![];
    for part in joined.split('/') {
        match part {
            "" | "." => (),
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    format!("/{}", parts.join("/"))
}

/// `diskbackend.path_definitely_exists`: a managed volume, or a local
/// file on a local connection. `false` means unsure, not absent.
pub fn path_definitely_exists(conn: &Connect, path: &str, is_remote: bool) -> bool {
    match check_if_path_managed(conn, path) {
        Ok((Some(_), _)) => true,
        Ok(_) => !is_remote && std::path::Path::new(path).exists(),
        Err(_) => false,
    }
}

/// `diskbackend.manage_path`: looks up the path's volume and pool. A
/// local directory that isn't a pool yet becomes one (named after the
/// directory, autostarted), as upstream does, so the path can be
/// created and tracked as a volume.
pub fn manage_path(conn: &Connect, path: &str) -> Result<ManagedPath, String> {
    let path = if path_is_url(path) {
        path.to_string()
    } else {
        abspath(path)
    };
    let (vol, pool) = check_if_path_managed(conn, &path)?;
    if vol.is_some() || pool.is_some() || !can_auto_manage(&path) {
        return Ok(ManagedPath { path, vol, pool });
    }

    let dir = dirname(&path).to_string();
    let mut poolname = basename(&dir).replace(' ', "_");
    if poolname.is_empty() {
        poolname = "dirpool".into();
    }
    let names: Vec<String> = fetch_all_pools(conn)
        .into_iter()
        .map(|p| p.xml.name)
        .collect();
    let poolname = generate_name(&poolname, "", |n| names.iter().any(|x| x == n));
    let xml = format!(
        "<pool type='dir'><name>{}</name><target><path>{}</path></target></pool>",
        xml_escape(&poolname),
        xml_escape(&dir)
    );
    // `poolxml.install(build=False, create=True, autostart=True)`
    let pool = StoragePool::define_xml(conn, &xml, 0).map_err(|e| e.to_string())?;
    pool.create(0).map_err(|e| e.to_string())?;
    let _ = pool.set_autostart(true);
    let pool = Pool {
        xml: PoolXml::parse(&pool.get_xml_desc(0).map_err(|e| e.to_string())?)
            .ok_or("Could not parse the new pool's XML")?,
        pool,
    };
    let name = basename(&path);
    let vol = pool
        .pool
        .list_volumes()
        .unwrap_or_default()
        .iter()
        .any(|n| n == name)
        .then(|| StorageVol::lookup_by_name(&pool.pool, name).ok())
        .flatten();
    Ok(ManagedPath {
        path,
        vol,
        pool: Some(pool),
    })
}

pub(crate) fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&apos;")
        .replace('"', "&quot;")
}

/// `sync_path_props` for a volume: its type and format.
pub fn vol_path_props(vol: &StorageVol) -> PathProps {
    let dev_type = match vol.get_info().map(|i| i.kind) {
        Ok(sys::VIR_STORAGE_VOL_BLOCK) => "block",
        Ok(sys::VIR_STORAGE_VOL_DIR) => "dir",
        Ok(sys::VIR_STORAGE_VOL_NETWORK) | Ok(sys::VIR_STORAGE_VOL_NETDIR) => "network",
        _ => "file",
    };
    let format = vol.get_xml_desc(0).ok().and_then(|xml| {
        let doc = virtinst_xml::parse_libvirt_xml(&xml).ok()?;
        let root = doc.root_element()?;
        virtinst_xml::resolve_path(&doc, root, &["target", "format"])?
            .attribute(&doc, "type")
            .map(str::to_string)
    });
    PathProps {
        dev_type: dev_type.into(),
        format,
    }
}

/// A volume to create: `DeviceDisk.build_vol_install`'s
/// `StorageVolume`, after `validate`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VolInstall {
    pub pool_name: String,
    pub name: String,
    /// Bytes.
    pub capacity: u128,
    pub allocation: u128,
    pub format: Option<String>,
    /// `file_type`: what the pool makes, `file` for a directory.
    pub file_type: String,
    pub backing_store: Option<String>,
    pub backing_format: Option<String>,
}

impl VolInstall {
    /// `DeviceDisk.build_vol_install` plus `validate`'s defaults.
    /// `size` is in GiB.
    pub fn new(
        volname: &str,
        pool: Option<&PoolXml>,
        size: f64,
        sparse: bool,
        fmt: Option<&str>,
    ) -> Result<VolInstall, String> {
        // This catches --disk /dev/idontexist,size=1 if /dev is unmanaged
        let pool = pool.ok_or_else(|| {
            format!(
                "Don't know how to create storage for path '{volname}'. Use libvirt \
                 APIs to manage the parent directory as a pool first."
            )
        })?;
        let capacity = (size * 1024.0 * 1024.0 * 1024.0) as u128;
        let file_type = pool.get_disk_type().to_string();
        let mut vol = VolInstall {
            pool_name: pool.name.clone(),
            name: volname.into(),
            capacity,
            allocation: if sparse { 0 } else { capacity },
            format: None,
            file_type,
            backing_store: None,
            backing_format: None,
        };
        if let Some(fmt) = fmt {
            if !vol.supports_format() {
                return Err("Format attribute not supported for this volume type".into());
            }
            vol.format = Some(fmt.into());
        }
        if vol.format.is_none() && vol.supports_format() {
            vol.format = Some("raw".into());
        }
        if pool.pool_type == "logical" && vol.allocation != vol.capacity {
            log::warn!(
                "Sparse logical volumes are not supported, setting allocation equal to capacity"
            );
            vol.allocation = vol.capacity;
        }
        Ok(vol)
    }

    /// `StorageVolume.supports_format`.
    pub fn supports_format(&self) -> bool {
        self.file_type == "file"
    }

    /// The `<volume>` XML: qcow2 gets `lazy_refcounts`, as every
    /// supported qemu has it.
    pub fn to_xml(&self) -> String {
        let mut target = String::new();
        if let Some(fmt) = &self.format {
            target += &format!("<format type='{}'/>", xml_escape(fmt));
            if fmt == "qcow2" {
                target += "<features><lazy_refcounts/></features>";
            }
        }
        let mut backing = String::new();
        if let Some(path) = &self.backing_store {
            backing += &format!("<path>{}</path>", xml_escape(path));
            if let Some(fmt) = &self.backing_format {
                backing += &format!("<format type='{}'/>", xml_escape(fmt));
            }
            backing = format!("<backingStore>{backing}</backingStore>");
        }
        format!(
            "<volume><name>{}</name><capacity>{}</capacity><allocation>{}</allocation>\
             <target>{target}</target>{backing}</volume>",
            xml_escape(&self.name),
            self.capacity,
            self.allocation
        )
    }

    /// `StorageVolume.validate`'s checks: the name, and
    /// `is_size_conflict` against the pool's free space. A capacity
    /// that only fits sparse is a warning, logged.
    pub fn validate(&self, conn: &Connect) -> Result<(), String> {
        validate_generic_name("Storage object", &self.name)?;
        let pool = StoragePool::lookup_by_name(conn, &self.pool_name).map_err(|e| e.to_string())?;
        if StorageVol::lookup_by_name(&pool, &self.name).is_ok() {
            return Err(format!(
                "Name '{}' already in use by another volume.",
                self.name
            ));
        }
        let avail = pool.get_info().map(|i| i.available).unwrap_or(u64::MAX) as u128;
        const MIB: u128 = 1024 * 1024;
        if self.allocation > avail {
            return Err(format!(
                "There is not enough free space on the storage pool to create the volume. \
                 ({} M requested allocation > {} M available)",
                self.allocation / MIB,
                avail / MIB
            ));
        }
        if self.capacity > avail {
            log::warn!(
                "The requested volume capacity will exceed the available pool space when \
                 the volume is fully allocated. ({} M requested capacity > {} M available)",
                self.capacity / MIB,
                avail / MIB
            );
        }
        Ok(())
    }

    /// `is_size_conflict`: whether the requested size exceeds the
    /// pool's available space, `(fatal, message)`.
    pub fn is_size_conflict(&self, conn: &Connect) -> (bool, Option<String>) {
        let Ok(pool) = StoragePool::lookup_by_name(conn, &self.pool_name) else {
            return (false, None);
        };
        let avail = pool.get_info().map(|i| i.available).unwrap_or(u64::MAX) as u128;
        const MIB: u128 = 1024 * 1024;
        if self.allocation > avail {
            return (
                true,
                Some(format!(
                    "There is not enough free space on the storage pool to create the volume. \
                     ({} M requested allocation > {} M available)",
                    self.allocation / MIB,
                    avail / MIB
                )),
            );
        }
        if self.capacity > avail {
            return (
                false,
                Some(format!(
                    "The requested volume capacity will exceed the available pool space when \
                     the volume is fully allocated. ({} M requested capacity > {} M available)",
                    self.capacity / MIB,
                    avail / MIB
                )),
            );
        }
        (false, None)
    }

    /// `_detect_backing_store_format`: the format of the volume at
    /// `backing_store`, when it's a file volume.
    fn detect_backing_store_format(&self, conn: &Connect) -> Option<String> {
        detect_backing_store_format(conn, self.backing_store.as_deref()?)
    }

    /// The disk's view of the volume once made.
    pub fn path_props(&self) -> PathProps {
        PathProps {
            dev_type: self.file_type.clone(),
            format: self.format.clone(),
        }
    }

    /// `StorageVolume.install`: creates the volume, with qcow2 metadata
    /// preallocated.
    pub fn install(&self, conn: &Connect) -> Result<StorageVol, String> {
        let mut vol = self.clone();
        if vol.backing_store.is_some() && vol.backing_format.is_none() {
            vol.backing_format = vol.detect_backing_store_format(conn);
        }
        let pool = StoragePool::lookup_by_name(conn, &self.pool_name).map_err(|e| e.to_string())?;
        // `is_really_test`: the test driver takes no flags.
        let is_test = conn.get_uri().is_ok_and(|u| u.starts_with("test:"));
        let flags =
            if !is_test && vol.format.as_deref() == Some("qcow2") && vol.backing_store.is_none() {
                sys::VIR_STORAGE_VOL_CREATE_PREALLOC_METADATA
            } else {
                0
            };
        StorageVol::create_xml(&pool, &vol.to_xml(), flags)
            .map_err(|e| format!("Couldn't create storage volume '{}': '{e}'", self.name))
    }
}

const DEFAULT_DEV_TARGET: &str = "/dev";
const DEFAULT_SCSI_TARGET: &str = "/dev/disk/by-path";
const DEFAULT_MPATH_TARGET: &str = "/dev/mapper";

/// `StoragePool.list_types`: the types `pretty_pool_type` names.
pub const POOL_TYPES: &[&str] = &[
    "dir", "disk", "fs", "gluster", "iscsi", "logical", "mpath", "netfs", "rbd", "scsi", "zfs",
];

/// A pool to define: `virtinst.StoragePool` as the new pool wizard
/// builds it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewPool {
    pub name: String,
    pub pool_type: String,
    pub target_path: Option<String>,
    pub host: Option<String>,
    /// `source_path`: `<dir>`, `<adapter>` or `<device>` by type.
    pub source_path: Option<String>,
    pub format: Option<String>,
    pub iqn: Option<String>,
    pub source_name: Option<String>,
}

impl NewPool {
    pub fn supports_target_path(&self) -> bool {
        matches!(
            self.pool_type.as_str(),
            "dir" | "fs" | "netfs" | "iscsi" | "scsi" | "mpath"
        )
    }

    pub fn supports_source_name(&self) -> bool {
        matches!(
            self.pool_type.as_str(),
            "logical" | "gluster" | "rbd" | "zfs"
        )
    }

    pub fn supports_source_path(&self) -> bool {
        matches!(
            self.pool_type.as_str(),
            "fs" | "netfs" | "disk" | "iscsi" | "scsi" | "gluster"
        )
    }

    pub fn supports_hosts(&self) -> bool {
        matches!(
            self.pool_type.as_str(),
            "netfs" | "iscsi" | "gluster" | "rbd"
        )
    }

    pub fn supports_format(&self) -> bool {
        matches!(self.pool_type.as_str(), "fs" | "netfs" | "disk")
    }

    pub fn supports_iqn(&self) -> bool {
        self.pool_type == "iscsi"
    }

    /// `default_target_path`.
    pub fn default_target_path(&self, is_session: bool) -> Option<String> {
        if !self.supports_target_path() {
            return None;
        }
        match self.pool_type.as_str() {
            "dir" | "netfs" | "fs" => Some(format!(
                "{}/{}",
                preferred_default_pool_path(is_session),
                self.name
            )),
            "iscsi" | "scsi" => Some(DEFAULT_SCSI_TARGET.into()),
            "mpath" => Some(DEFAULT_MPATH_TARGET.into()),
            _ => None,
        }
    }

    /// `default_source_name`.
    pub fn default_source_name(&self) -> Option<String> {
        match self.pool_type.as_str() {
            "rbd" => Some("rbd".into()),
            "gluster" => Some("gv0".into()),
            _ => None,
        }
    }

    /// `validate`: the name, then the defaults for what's unset.
    pub fn validate(&mut self, conn: &Connect, is_session: bool) -> Result<(), String> {
        validate_generic_name("Storage object", &self.name)?;
        if StoragePool::lookup_by_name(conn, &self.name).is_ok() {
            return Err(format!(
                "Name '{}' already in use by another pool.",
                self.name
            ));
        }
        if self.target_path.as_deref().unwrap_or_default().is_empty() {
            // disk is a bit special, in that it demands a target path,
            // but basically can't handle anything other than /dev
            self.target_path = if self.pool_type == "disk" {
                Some(DEFAULT_DEV_TARGET.into())
            } else {
                self.default_target_path(is_session)
            };
        }
        if self.source_name.is_none() {
            self.source_name = self.default_source_name();
        }
        if self.format.is_none() && self.supports_format() {
            self.format = Some("auto".into());
        }
        // There is no explicit "auto" type for disk pools, but leaving out
        // the format type seems to do the job for existing formatted disks
        if self.pool_type == "disk" && self.format.as_deref() == Some("auto") {
            self.format = None;
        }
        Ok(())
    }

    /// `get_disk_type`'s `<pool>` XML.
    pub fn to_xml(&self) -> String {
        let mut source = String::new();
        if let Some(fmt) = &self.format {
            source += &format!("<format type='{}'/>", xml_escape(fmt));
        }
        if let Some(host) = &self.host {
            source += &format!("<host name='{}'/>", xml_escape(host));
        }
        if let Some(path) = &self.source_path {
            let path = xml_escape(path);
            source += &match self.pool_type.as_str() {
                "netfs" | "gluster" => format!("<dir path='{path}'/>"),
                "scsi" => format!("<adapter name='{path}'/>"),
                _ => format!("<device path='{path}'/>"),
            };
        }
        if let Some(name) = &self.source_name {
            source += &format!("<name>{}</name>", xml_escape(name));
        }
        if let Some(iqn) = &self.iqn {
            source += &format!("<initiator><iqn name='{}'/></initiator>", xml_escape(iqn));
        }
        let mut xml = format!(
            "<pool type='{}'><name>{}</name>",
            xml_escape(&self.pool_type),
            xml_escape(&self.name)
        );
        if !source.is_empty() {
            xml += &format!("<source>{source}</source>");
        }
        if let Some(target) = &self.target_path {
            xml += &format!("<target><path>{}</path></target>", xml_escape(target));
        }
        xml + "</pool>"
    }

    /// `install(create=True, build=build)` then autostart: on a
    /// failure past the define, the pool is undefined again.
    pub fn install(&self, conn: &Connect, build: bool) -> Result<(), String> {
        install_pool(conn, &self.to_xml(), build)
    }
}

/// `StoragePool.find_free_name`: `basename`, or with a number added,
/// not used by any pool.
pub fn find_free_pool_name(conn: &Connect, basename_: &str) -> String {
    let names: Vec<String> = fetch_all_pools(conn)
        .into_iter()
        .map(|p| p.xml.name)
        .collect();
    generate_name(basename_, "", |n| names.iter().any(|x| x == n))
}

/// `StoragePool.pool_list_from_sources`: the source names libvirt
/// finds for `pool_type` (volume groups, for `logical`).
pub fn pool_list_from_sources(conn: &Connect, pool_type: &str) -> Vec<String> {
    let xml = match conn.find_storage_pool_sources(pool_type, Some("<source/>"), 0) {
        Ok(xml) => xml,
        Err(e) => {
            eprintln!("Pool enumeration failed: {e}");
            return vec![];
        }
    };
    let Ok(doc) = virtinst_xml::parse_libvirt_xml(&xml) else {
        return vec![];
    };
    let Some(root) = doc.root_element() else {
        return vec![];
    };
    root.find_all(&doc, "source")
        .into_iter()
        .filter_map(|s| s.find(&doc, "name").map(|n| n.text_content(&doc)))
        .collect()
}

/// `XMLBuilder.validate_generic_name`: only `/` is forbidden,
/// matching QEMU.
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

/// `DeviceDisk.get_volmap`: each volume, keyed by its backing store's
/// path, as its own path.
pub fn get_volmap(conn: &Connect) -> Vec<(String, String)> {
    let mut ret = vec![];
    for pool in fetch_all_pools(conn) {
        for vol in pool.pool.list_all_volumes(0).unwrap_or_default() {
            let Some(xml) = vol.get_xml_desc(0).ok() else {
                continue;
            };
            let Ok(doc) = virtinst_xml::parse_libvirt_xml(&xml) else {
                continue;
            };
            let Some(root) = doc.root_element() else {
                continue;
            };
            let text = |path: &[&str]| {
                virtinst_xml::resolve_path(&doc, root, path).map(|e| e.text_content(&doc))
            };
            if let (Some(backing), Some(path)) =
                (text(&["backingStore", "path"]), text(&["target", "path"]))
            {
                ret.push((backing, path));
            }
        }
    }
    ret
}

/// `DeviceDisk._path_in_use_by`: the names of the VMs (name, domain
/// XML) using `path`, directly or as the backing store of a volume
/// they use. A shareable (read-only) path doesn't clash with another
/// shareable (read-only) use.
pub fn path_in_use_by(
    path: &str,
    volmap: &[(String, String)],
    vms: &[(String, String)],
    shareable: bool,
    read_only: bool,
) -> Vec<String> {
    path_in_use_by_resolving(path, volmap, vms, shareable, read_only, &|_, _| None)
}

/// [`path_in_use_by`] against everything on `conn`, with disks on a
/// `<source pool= volume=>` checked by their volume's path, as
/// `disk.get_source_path()` resolves them.
pub fn path_in_use_by_conn(
    conn: &Connect,
    path: &str,
    shareable: bool,
    read_only: bool,
) -> Result<Vec<String>, String> {
    let doms = conn
        .list_all_domains(0)
        .map_err(|e| e.message().to_string())?;
    let vms: Vec<(String, String)> = doms
        .iter()
        .filter_map(|dom| Some((dom.get_name().ok()?, dom.get_xml_desc(0).ok()?)))
        .collect();
    let volmap = get_volmap(conn);
    let resolve = |pool: &str, vol: &str| {
        let pool = virt::storage_pool::StoragePool::lookup_by_name(conn, pool).ok()?;
        let vol = virt::storage_vol::StorageVol::lookup_by_name(&pool, vol).ok()?;
        vol.get_path().ok()
    };
    Ok(path_in_use_by_resolving(
        path, &volmap, &vms, shareable, read_only, &resolve,
    ))
}

fn path_in_use_by_resolving(
    path: &str,
    volmap: &[(String, String)],
    vms: &[(String, String)],
    shareable: bool,
    read_only: bool,
    resolve_volume: &dyn Fn(&str, &str) -> Option<String>,
) -> Vec<String> {
    if path.is_empty() {
        return vec![];
    }
    // Find all volumes that have 'path' somewhere in their backing chain
    let mut vols: Vec<String> = vec![];
    let mut backpath = path.to_string();
    while let Some((_, vol)) = volmap.iter().find(|(b, _)| *b == backpath) {
        if vols.contains(vol) {
            break;
        }
        backpath = vol.clone();
        vols.push(backpath.clone());
    }

    let mut ret: Vec<String> = vec![];
    for (name, xml) in vms {
        let Ok(doc) = virtinst_xml::parse_libvirt_xml(xml) else {
            continue;
        };
        let Some(root) = doc.root_element() else {
            continue;
        };
        if !read_only {
            let os_path = |tag: &str| {
                virtinst_xml::resolve_path(&doc, root, &["os", tag]).map(|e| e.text_content(&doc))
            };
            if ["kernel", "initrd", "dtb"]
                .iter()
                .any(|t| os_path(t).as_deref() == Some(path))
            {
                ret.push(name.clone());
                continue;
            }
        }
        let disks = virtinst_xml::resolve_path(&doc, root, &["devices"])
            .map(|d| d.find_all(&doc, "disk"))
            .unwrap_or_default();
        for disk in disks {
            let checkpath = crate::devices::disk::source_path(&doc, disk).or_else(|| {
                let (pool, vol) = crate::devices::disk::source_volume(&doc, disk)?;
                resolve_volume(&pool, &vol)
            });
            if checkpath.as_ref().is_some_and(|c| vols.contains(c)) && !ret.contains(name) {
                // VM uses the path indirectly via backing store
                ret.push(name.clone());
                break;
            }
            if checkpath.as_deref() != Some(path) {
                continue;
            }
            if shareable && disk.find(&doc, "shareable").is_some() {
                continue;
            }
            if read_only && disk.find(&doc, "readonly").is_some() {
                continue;
            }
            ret.push(name.clone());
            break;
        }
    }
    ret
}

// ---------------------------------------------------------------------
// The XMLBuilder classes (`StoragePool`, `StorageVolume`): the XML the
// pool and volume wizards build and their XML tabs edit, and the pool
// details' XML tab
// ---------------------------------------------------------------------

/// `_StoragePermissions`.
pub static STORAGE_PERMISSIONS: XmlClass = XmlClass {
    xml_name: "permissions",
    props: &[
        prop("mode", "./mode", Kind::Str),
        prop("owner", "./owner", Kind::Str),
        prop("group", "./group", Kind::Str),
        prop("label", "./label", Kind::Str),
    ],
    children: &[],
    prop_order: &["mode", "owner", "group", "label"],
};

/// `_Host`.
pub static STORAGE_POOL_HOST: XmlClass = XmlClass {
    xml_name: "host",
    props: &[
        prop("name", "./@name", Kind::Str),
        prop("port", "./@port", Kind::Int),
    ],
    children: &[],
    prop_order: &["name", "port"],
};

/// `StoragePool`.
pub static STORAGE_POOL: XmlClass = XmlClass {
    xml_name: "pool",
    props: &[
        prop("name", "./name", Kind::Str),
        prop("_source_dir", "./source/dir/@path", Kind::Str),
        prop("_source_adapter", "./source/adapter/@name", Kind::Str),
        prop("_source_device", "./source/device/@path", Kind::Str),
        prop("type", "./@type", Kind::Str),
        prop("uuid", "./uuid", Kind::Str),
        prop("capacity", "./capacity", Kind::Int),
        prop("allocation", "./allocation", Kind::Int),
        prop("available", "./available", Kind::Int),
        prop("format", "./source/format/@type", Kind::Str),
        prop("iqn", "./source/initiator/iqn/@name", Kind::Str),
        prop("source_name", "./source/name", Kind::Str),
        prop("auth_type", "./source/auth/@type", Kind::Str),
        prop("auth_username", "./source/auth/@username", Kind::Str),
        prop("auth_secret_uuid", "./source/auth/secret/@uuid", Kind::Str),
        prop("target_path", "./target/path", Kind::Str),
    ],
    children: &[
        child("permissions", &STORAGE_PERMISSIONS, "./target", true),
        child("hosts", &STORAGE_POOL_HOST, "./source", false),
    ],
    prop_order: &[
        "name",
        "type",
        "uuid",
        "capacity",
        "allocation",
        "available",
        "format",
        "hosts",
        "_source_dir",
        "_source_adapter",
        "_source_device",
        "source_name",
        "target_path",
        "permissions",
        "auth_type",
        "auth_username",
        "auth_secret_uuid",
    ],
};

/// `StorageVolume`.
pub static STORAGE_VOLUME: XmlClass = XmlClass {
    xml_name: "volume",
    props: &[
        prop("name", "./name", Kind::Str),
        prop("type", "./@type", Kind::Str),
        prop("key", "./key", Kind::Str),
        prop("capacity", "./capacity", Kind::Int),
        prop("allocation", "./allocation", Kind::Int),
        prop("format", "./target/format/@type", Kind::Str),
        prop("target_path", "./target/path", Kind::Str),
        prop("backing_store", "./backingStore/path", Kind::Str),
        prop("backing_format", "./backingStore/format/@type", Kind::Str),
        prop(
            "lazy_refcounts",
            "./target/features/lazy_refcounts",
            Kind::Bool,
        ),
    ],
    children: &[child("permissions", &STORAGE_PERMISSIONS, "./target", true)],
    prop_order: &[
        "name",
        "key",
        "capacity",
        "allocation",
        "format",
        "target_path",
        "permissions",
    ],
};

fn sval(s: impl Into<String>) -> Option<Value> {
    Some(Value::Str(s.into()))
}

/// A property's string value, `None` when unset or empty (Python's
/// falsy `""`).
fn str_prop(obj: &XmlObject, name: &str) -> Option<String> {
    obj.get_str(name).filter(|s| !s.is_empty())
}

/// An `is_int` property, 0 when unset; an unparsable value is
/// Python's `int()` error.
fn int_prop(obj: &XmlObject, name: &str) -> Result<i128, String> {
    match obj.get(name) {
        Some(Value::Int(i)) => Ok(i128::from(i)),
        Some(Value::Str(s)) => crate::xmlbuilder::py_int(&s).map(i128::from),
        _ => Ok(0),
    }
}

impl NewPool {
    /// `_build_xmlobj_from_ui`'s `StoragePool`: the stub's type and
    /// name, then the fields the wizard filled in.
    pub fn build_xmlobj(&self) -> Result<XmlObject, String> {
        let mut pool = XmlObject::new(&STORAGE_POOL);
        // `_make_stub_pool`
        pool.set("type", sval(self.pool_type.clone()))?;
        pool.set("name", sval(self.name.clone()))?;

        pool.set("target_path", self.target_path.clone().and_then(sval))?;
        if let Some(host) = &self.host {
            let mut hostobj = XmlObject::new(&STORAGE_POOL_HOST);
            hostobj.set("name", sval(host.clone()))?;
            pool.add_child(hostobj);
        }
        if let Some(source) = &self.source_path {
            pool.set(pool_source_prop(&self.pool_type), sval(source.clone()))?;
        }
        if let Some(fmt) = self.format.as_ref().filter(|_| self.supports_format()) {
            pool.set("format", sval(fmt.clone()))?;
        }
        if let Some(iqn) = &self.iqn {
            pool.set("iqn", sval(iqn.clone()))?;
        }
        if let Some(source_name) = &self.source_name {
            pool.set("source_name", sval(source_name.clone()))?;
        }
        Ok(pool)
    }
}

/// `_type_to_source_prop`: the property `source_path` stands for.
fn pool_source_prop(pool_type: &str) -> &'static str {
    match pool_type {
        "netfs" | "gluster" => "_source_dir",
        "scsi" => "_source_adapter",
        _ => "_source_device",
    }
}

/// `StoragePool(conn, parsexml=xml)`.
pub fn parse_pool(xml: &str) -> Result<XmlObject, String> {
    XmlObject::parse(&STORAGE_POOL, xml)
}

/// `vmmStoragePool.get_xml_to_define`: the pool's XML (`XMLDesc(0)`)
/// as virtinst re-serializes it, for the XML editor.
pub fn pool_xml_to_define(xml: &str) -> Result<String, String> {
    parse_pool(xml).map(|o| o.get_xml())
}

/// `StoragePool.validate` on the pool `obj`: the name, then the
/// defaults for what's unset.
pub fn validate_pool(conn: &Connect, obj: &mut XmlObject, is_session: bool) -> Result<(), String> {
    let name = obj.get_str("name").unwrap_or_default();
    // `validate_name`
    validate_generic_name("Storage object", &name)?;
    if StoragePool::lookup_by_name(conn, &name).is_ok() {
        return Err(format!("Name '{name}' already in use by another pool."));
    }

    let stub = NewPool {
        name,
        pool_type: obj.get_str("type").unwrap_or_default(),
        ..Default::default()
    };
    if str_prop(obj, "target_path").is_none() {
        let target = if stub.pool_type == "disk" {
            // disk is a bit special, in that it demands a target path,
            // but basically can't handle anything other than /dev
            Some(DEFAULT_DEV_TARGET.to_string())
        } else {
            stub.default_target_path(is_session)
        };
        obj.set("target_path", target.and_then(sval))?;
    }
    if str_prop(obj, "source_name").is_none() {
        obj.set("source_name", stub.default_source_name().and_then(sval))?;
    }
    if str_prop(obj, "format").is_none() && stub.supports_format() {
        obj.set("format", sval("auto"))?;
    }

    if stub.pool_type == "disk" && str_prop(obj, "format").as_deref() == Some("auto") {
        // There is no explicit "auto" type for disk pools, but leaving out
        // the format type seems to do the job for existing formatted disks
        obj.set("format", None)?;
    }
    Ok(())
}

/// `StoragePool.install(create=True, build=build)` of the pool `xml`,
/// then autostart: on a failure past the define, the pool is undefined
/// again.
pub fn install_pool(conn: &Connect, xml: &str, build: bool) -> Result<(), String> {
    let pool = StoragePool::define_xml(conn, xml, 0)
        .map_err(|e| format!("Could not define storage pool: {e}"))?;
    let mut res = Ok(());
    if build {
        res = pool
            .build(sys::VIR_STORAGE_POOL_BUILD_NEW)
            .map(|_| ())
            .map_err(|e| format!("Could not build storage pool: {e}"));
    }
    if res.is_ok() {
        res = pool
            .create(0)
            .map(|_| ())
            .map_err(|e| format!("Could not start storage pool: {e}"));
    }
    if res.is_ok() {
        res = pool
            .set_autostart(true)
            .map(|_| ())
            .map_err(|e| format!("Could not set pool autostart flag: {e}"));
    }
    if res.is_err() {
        // Try and clean up the leftover pool
        if let Err(e) = pool.undefine() {
            eprintln!("Error cleaning up pool after failure: {e}");
        }
    }
    res
}

/// The fields `createvol`'s `_build_xmlobj_from_ui` sets on its
/// `StorageVolume`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NewVolume {
    pub name: String,
    /// GiB, as the capacity spin button has it.
    pub capacity: f64,
    pub allocation: f64,
    pub backing_store: Option<String>,
    pub format: Option<String>,
}

impl NewVolume {
    /// `_build_xmlobj_from_ui`.
    pub fn build_xmlobj(&self) -> Result<XmlObject, String> {
        const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
        let mut vol = XmlObject::new(&STORAGE_VOLUME);
        vol.set("name", sval(self.name.clone()))?;
        // `int()` of the float, as `XMLProperty(is_int=True)` does
        vol.set("capacity", Some(Value::Int((self.capacity * GIB) as i64)))?;
        vol.set(
            "allocation",
            Some(Value::Int((self.allocation * GIB) as i64)),
        )?;
        if let Some(backing) = &self.backing_store {
            vol.set("backing_store", sval(backing.clone()))?;
        }
        if let Some(fmt) = &self.format {
            vol.set("format", sval(fmt.clone()))?;
        }
        Ok(vol)
    }
}

/// `StorageVolume(conn, parsexml=xml)`.
pub fn parse_volume(xml: &str) -> Result<XmlObject, String> {
    XmlObject::parse(&STORAGE_VOLUME, xml)
}

/// `StorageVolume.file_type`: the volume's own `type`, else the type
/// of volume its pool makes.
pub fn volume_file_type(vol: &XmlObject, pool: &PoolXml) -> String {
    match vol.get_str("type").as_deref() {
        Some(t @ ("file" | "block" | "dir" | "network")) => t.to_string(),
        _ => pool.get_disk_type().to_string(),
    }
}

/// `conn.support.conn_qcow2_lazy_refcounts`: qemu (every one libvirt
/// still supports has it) and the test driver.
fn conn_qcow2_lazy_refcounts(conn: &Connect) -> bool {
    conn.get_type()
        .is_ok_and(|t| matches!(t.to_lowercase().as_str(), "qemu" | "test"))
}

/// The pool's `XMLDesc(0)`, parsed (`vol.pool = ...`'s `_pool_xml`).
fn pool_xml_of(pool: &StoragePool) -> Result<PoolXml, String> {
    let xml = pool.get_xml_desc(0).map_err(|e| e.to_string())?;
    PoolXml::parse(&xml).ok_or_else(|| "Unparseable pool XML".to_string())
}

/// `StorageVolume.validate` for the volume `vol` in `pool`: the name,
/// the format and `lazy_refcounts` defaults, a logical volume fully
/// allocated, and `is_size_conflict` (a capacity that only fits sparse
/// is a warning, logged).
pub fn validate_volume(
    conn: &Connect,
    pool: &StoragePool,
    vol: &mut XmlObject,
) -> Result<(), String> {
    let name = vol.get_str("name").unwrap_or_default();
    // `validate_name`
    validate_generic_name("Storage object", &name)?;
    if StorageVol::lookup_by_name(pool, &name).is_ok() {
        return Err(format!("Name '{name}' already in use by another volume."));
    }
    let pool_xml = pool_xml_of(pool)?;

    if str_prop(vol, "format").is_none() && volume_file_type(vol, &pool_xml) == "file" {
        vol.set("format", sval("raw"))?;
    }
    // `_prop_is_unset("lazy_refcounts")`
    let lazy_unset = vol.get("lazy_refcounts") != Some(Value::Bool(true))
        && !vol.prop_in_store("lazy_refcounts");
    if lazy_unset && str_prop(vol, "format").as_deref() == Some("qcow2") {
        vol.set(
            "lazy_refcounts",
            Some(Value::Bool(conn_qcow2_lazy_refcounts(conn))),
        )?;
    }

    if pool_xml.pool_type == "logical" && int_prop(vol, "allocation")? != int_prop(vol, "capacity")?
    {
        log::warn!(
            "Sparse logical volumes are not supported, setting allocation equal to capacity"
        );
        vol.set("allocation", vol.get("capacity"))?;
    }

    // `is_size_conflict`
    let avail = pool.get_info().map(|i| i.available).unwrap_or(u64::MAX) as i128;
    const MIB: i128 = 1024 * 1024;
    let (allocation, capacity) = (int_prop(vol, "allocation")?, int_prop(vol, "capacity")?);
    if allocation > avail {
        return Err(format!(
            "There is not enough free space on the storage pool to create the volume. \
             ({} M requested allocation > {} M available)",
            allocation / MIB,
            avail / MIB
        ));
    }
    if capacity > avail {
        log::warn!(
            "The requested volume capacity will exceed the available pool space when the \
             volume is fully allocated. ({} M requested capacity > {} M available)",
            capacity / MIB,
            avail / MIB
        );
    }
    Ok(())
}

/// `_detect_backing_store_format`: the format of the volume at `path`,
/// when it's a file volume.
fn detect_backing_store_format(conn: &Connect, path: &str) -> Option<String> {
    let managed = manage_path(conn, path).ok()?;
    let vol = managed.vol?;
    let pool = managed.pool?;
    if pool.xml.get_disk_type() != "file" {
        return None;
    }
    vol_path_props(&vol).format
}

/// `StorageVolume.install` of the volume `xml` in `pool`: the backing
/// store's format filled in, and qcow2 metadata preallocated.
pub fn install_volume(conn: &Connect, pool: &StoragePool, xml: &str) -> Result<StorageVol, String> {
    let mut vol = parse_volume(xml)?;
    let name = vol.get_str("name").unwrap_or_default();
    let backing_store = str_prop(&vol, "backing_store");
    if let Some(backing) = &backing_store {
        if str_prop(&vol, "backing_format").is_none() {
            log::debug!("Attempting to detect format for backing_store={backing}");
            let fmt = detect_backing_store_format(conn, backing);
            vol.set("backing_format", fmt.and_then(sval))?;
        }
    }
    let xml = vol.get_xml();
    log::debug!("Creating storage volume '{name}' with xml:\n{xml}");

    // `is_really_test`: the test suite doesn't support any flags
    let is_test = conn.get_uri().is_ok_and(|u| u.starts_with("test:"));
    let flags = if !is_test
        && str_prop(&vol, "format").as_deref() == Some("qcow2")
        && backing_store.is_none()
    {
        sys::VIR_STORAGE_VOL_CREATE_PREALLOC_METADATA
    } else {
        0
    };
    log::debug!("Using vol create flags={flags}");
    StorageVol::create_xml(pool, &xml, flags)
        .map_err(|e| format!("Couldn't create storage volume '{name}': '{e}'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(file_extension_for_format(Some("raw")), ".img");
        assert_eq!(file_extension_for_format(Some("qcow2")), ".qcow2");
        assert_eq!(file_extension_for_format(None), "");
        let used = ["vm.qcow2", "vm-1.qcow2"];
        assert_eq!(
            generate_name("vm", ".qcow2", |n| used.contains(&n)),
            "vm-2.qcow2"
        );
        assert_eq!(generate_name("vm", "", |_| false), "vm");
        assert_eq!(dirname("/var/lib/x.img"), "/var/lib");
        assert_eq!(dirname("/x.img"), "/");
        assert_eq!(basename("/var/lib/x.img"), "x.img");
        assert_eq!(abspath("/a/./b/../c.img"), "/a/c.img");
    }

    #[test]
    fn in_use() {
        let vm = |disks: &str| {
            format!("<domain><os><kernel>/boot/k</kernel></os><devices>{disks}</devices></domain>")
        };
        let vms = vec![
            (
                "a".to_string(),
                vm("<disk type='file'><source file='/img/base.qcow2'/><shareable/></disk>"),
            ),
            (
                "b".to_string(),
                vm("<disk type='file'><source file='/img/top.qcow2'/></disk>"),
            ),
        ];
        let volmap = vec![("/img/base.qcow2".to_string(), "/img/top.qcow2".to_string())];
        assert_eq!(
            path_in_use_by("/img/base.qcow2", &volmap, &vms, false, false),
            ["a", "b"]
        );
        assert_eq!(
            path_in_use_by("/img/base.qcow2", &[], &vms, true, false),
            Vec::<String>::new()
        );
        assert_eq!(
            path_in_use_by("/boot/k", &[], &vms, false, false),
            ["a", "b"]
        );
        assert!(path_in_use_by("/boot/k", &[], &vms, false, true).is_empty());
    }

    #[test]
    fn auto_manage() {
        assert!(path_is_url("gluster+tcp://host/vol"));
        assert!(!path_is_url("/var/lib/x"));
        assert!(!can_auto_manage("/dev/sda"));
        assert!(!can_auto_manage("/dev"));
        assert!(can_auto_manage("/devices/x.img"));
        assert!(can_auto_manage("/tmp/x.img"));
    }

    #[test]
    fn pretty() {
        assert_eq!(pretty_bytes(20 * 1024 * 1024 * 1024), "20.00 GiB");
        assert_eq!(pretty_bytes(512 * 1024 * 1024), "512.00 MiB");
        assert_eq!(pretty_pool_type("dir"), "Filesystem Directory");
        assert_eq!(pretty_pool_type("vstorage"), "vstorage pool");
        assert!(supports_volume_creation("zfs", false));
        assert!(!supports_volume_creation("zfs", true));
        assert!(!supports_volume_creation("iscsi", false));
        let pool = PoolXml::parse(
            "<pool type='dir'><name>default</name><capacity unit='bytes'>100</capacity>\
             <allocation unit='bytes'>40</allocation><available unit='bytes'>60</available>\
             <target><path>/var/lib/libvirt/images</path></target></pool>",
        )
        .unwrap();
        assert_eq!(
            (pool.capacity, pool.allocation, pool.available),
            (100, 40, 60)
        );
        let vol = VolXml::parse(
            "<volume type='file'><name>a.qcow2</name><key>/i/a.qcow2</key>\
             <capacity unit='bytes'>1024</capacity><target><path>/i/a.qcow2</path>\
             <format type='qcow2'/></target></volume>",
        )
        .unwrap();
        assert_eq!(vol.format.as_deref(), Some("qcow2"));
        assert_eq!(vol.capacity, 1024);
        assert_eq!(vol.pretty_name("dir"), "a.qcow2");
        assert_eq!(vol.pretty_name("iscsi"), "a.qcow2 (/i/a.qcow2)");
    }

    #[test]
    fn new_pool() {
        let mut pool = NewPool {
            name: "nfs".into(),
            pool_type: "netfs".into(),
            host: Some("srv".into()),
            source_path: Some("/export".into()),
            format: Some("auto".into()),
            ..Default::default()
        };
        assert!(pool.supports_hosts() && pool.supports_format());
        assert_eq!(
            pool.default_target_path(false).as_deref(),
            Some("/var/lib/libvirt/images/nfs")
        );
        pool.target_path = pool.default_target_path(false);
        assert_eq!(
            pool.to_xml(),
            "<pool type='netfs'><name>nfs</name><source><format type='auto'/>\
             <host name='srv'/><dir path='/export'/></source>\
             <target><path>/var/lib/libvirt/images/nfs</path></target></pool>"
        );
        let scsi = NewPool {
            name: "s".into(),
            pool_type: "scsi".into(),
            source_path: Some("host0".into()),
            ..Default::default()
        };
        assert!(scsi.to_xml().contains("<adapter name='host0'/>"));
        assert_eq!(
            NewPool {
                pool_type: "rbd".into(),
                ..Default::default()
            }
            .default_source_name()
            .as_deref(),
            Some("rbd")
        );
    }

    #[test]
    fn vol_install() {
        let pool = PoolXml {
            name: "default".into(),
            pool_type: "dir".into(),
            target_path: Some("/var/lib/libvirt/images".into()),
            ..Default::default()
        };
        let vol = VolInstall::new("vm.qcow2", Some(&pool), 2.0, true, Some("qcow2")).unwrap();
        assert_eq!(vol.capacity, 2 * 1024 * 1024 * 1024);
        assert_eq!(vol.allocation, 0);
        assert_eq!(
            vol.to_xml(),
            "<volume><name>vm.qcow2</name><capacity>2147483648</capacity>\
             <allocation>0</allocation><target><format type='qcow2'/>\
             <features><lazy_refcounts/></features></target></volume>"
        );
        let mut backed = vol.clone();
        backed.backing_store = Some("/i/base.qcow2".into());
        backed.backing_format = Some("qcow2".into());
        assert!(backed.to_xml().ends_with(
            "</target><backingStore><path>/i/base.qcow2</path>\
             <format type='qcow2'/></backingStore></volume>"
        ));
        assert!(validate_generic_name("Storage object", "a/b").is_err());
        assert!(validate_generic_name("Storage object", "").is_err());
        let raw = VolInstall::new("vm.img", Some(&pool), 1.0, false, None).unwrap();
        assert_eq!(raw.format.as_deref(), Some("raw"));
        assert_eq!(raw.allocation, raw.capacity);
        assert!(VolInstall::new("x", None, 1.0, true, None).is_err());
        let lvm = PoolXml {
            pool_type: "logical".into(),
            ..pool
        };
        let vol = VolInstall::new("lv", Some(&lvm), 1.0, true, None).unwrap();
        assert_eq!((vol.format, vol.allocation), (None, vol.capacity));
    }

    /// The XML createpool's XML tab shows, as upstream's
    /// `StoragePool.get_xml()` prints it.
    #[test]
    fn pool_build_xmlobj() {
        let pool = NewPool {
            name: "nfs".into(),
            pool_type: "netfs".into(),
            target_path: Some("/var/lib/libvirt/images/nfs".into()),
            host: Some("srv".into()),
            source_path: Some("/export".into()),
            format: Some("auto".into()),
            ..Default::default()
        };
        assert_eq!(
            pool.build_xmlobj().unwrap().get_xml(),
            "<pool type=\"netfs\">\n  <name>nfs</name>\n  <source>\n    \
             <format type=\"auto\"/>\n    <host name=\"srv\"/>\n    \
             <dir path=\"/export\"/>\n  </source>\n  <target>\n    \
             <path>/var/lib/libvirt/images/nfs</path>\n  </target>\n</pool>\n"
        );
        let iscsi = NewPool {
            name: "is".into(),
            pool_type: "iscsi".into(),
            target_path: Some("/dev/disk/by-path".into()),
            host: Some("srv".into()),
            source_path: Some("iqn.x".into()),
            iqn: Some("iqn.init".into()),
            // Dropped: iSCSI pools take no format
            format: Some("auto".into()),
            ..Default::default()
        };
        assert_eq!(
            iscsi.build_xmlobj().unwrap().get_xml(),
            "<pool type=\"iscsi\">\n  <name>is</name>\n  <source>\n    \
             <host name=\"srv\"/>\n    <device path=\"iqn.x\"/>\n    <initiator>\n      \
             <iqn name=\"iqn.init\"/>\n    </initiator>\n  </source>\n  <target>\n    \
             <path>/dev/disk/by-path</path>\n  </target>\n</pool>\n"
        );
        let scsi = NewPool {
            name: "s".into(),
            pool_type: "scsi".into(),
            source_path: Some("host0".into()),
            ..Default::default()
        };
        assert!(scsi
            .build_xmlobj()
            .unwrap()
            .get_xml()
            .contains("<adapter name=\"host0\"/>"));

        let xml = "<pool type='dir'><name>default-pool</name>\
                   <capacity unit='bytes'>100</capacity><target><path>/p</path></target></pool>";
        assert_eq!(
            pool_xml_to_define(xml).unwrap(),
            "<pool type=\"dir\">\n  <name>default-pool</name>\n  \
             <capacity unit=\"bytes\">100</capacity>\n  <target>\n    <path>/p</path>\n  \
             </target>\n</pool>\n"
        );
        assert_eq!(
            parse_pool("<volume/>").unwrap_err(),
            "XML did not have expected root element name 'pool', found 'volume'"
        );
    }

    /// The XML createvol's XML tab shows, before and after `validate`.
    #[test]
    fn volume_build_xmlobj() {
        let vol = NewVolume {
            name: "vol.qcow2".into(),
            capacity: 20.0,
            allocation: 0.0,
            backing_store: Some("/x/base.qcow2".into()),
            format: Some("qcow2".into()),
        };
        assert_eq!(
            vol.build_xmlobj().unwrap().get_xml(),
            "<volume>\n  <name>vol.qcow2</name>\n  <capacity>21474836480</capacity>\n  \
             <allocation>0</allocation>\n  <target>\n    <format type=\"qcow2\"/>\n  \
             </target>\n  <backingStore>\n    <path>/x/base.qcow2</path>\n  \
             </backingStore>\n</volume>\n"
        );
        let pool = PoolXml {
            pool_type: "logical".into(),
            ..Default::default()
        };
        let parsed = parse_volume("<volume type='dir'><name>a</name></volume>").unwrap();
        assert_eq!(volume_file_type(&parsed, &pool), "dir");
        let parsed = parse_volume("<volume><name>a</name></volume>").unwrap();
        assert_eq!(volume_file_type(&parsed, &pool), "block");
    }

    /// Validate and create pools and volumes against the test driver,
    /// whose state is process-wide: unique names, and the crate's lock.
    #[test]
    fn validate_and_install_against_the_test_driver() {
        let _lock = crate::test_support::TEST_DOMAIN_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut conn = Connect::open(Some("test:///default")).unwrap();

        // Pools
        let mut taken = parse_pool("<pool type='dir'><name>default-pool</name></pool>").unwrap();
        assert_eq!(
            validate_pool(&conn, &mut taken, false).unwrap_err(),
            "Name 'default-pool' already in use by another pool."
        );
        let name = find_free_pool_name(&conn, "rusttestpool");
        let mut disk = parse_pool(&format!(
            "<pool type='disk'><name>{name}</name><source><format type='auto'/></source></pool>"
        ))
        .unwrap();
        validate_pool(&conn, &mut disk, false).unwrap();
        assert_eq!(disk.get_str("target_path").as_deref(), Some("/dev"));
        assert_eq!(disk.get_str("format"), None);
        let mut dir = NewPool {
            name: name.clone(),
            pool_type: "dir".into(),
            ..Default::default()
        }
        .build_xmlobj()
        .unwrap();
        validate_pool(&conn, &mut dir, false).unwrap();
        assert_eq!(
            dir.get_str("target_path"),
            Some(format!("/var/lib/libvirt/images/{name}"))
        );
        install_pool(&conn, &dir.get_xml(), false).unwrap();
        let p = StoragePool::lookup_by_name(&conn, &name).unwrap();
        assert!(p.is_active().unwrap() && p.get_autostart().unwrap());
        p.destroy().unwrap();
        p.undefine().unwrap();

        // Volumes
        let pool = StoragePool::lookup_by_name(&conn, "default-pool").unwrap();
        let volname = format!("{name}.qcow2");
        let mut vol = NewVolume {
            name: volname.clone(),
            capacity: 1.0,
            allocation: 0.0,
            backing_store: None,
            format: Some("qcow2".into()),
        }
        .build_xmlobj()
        .unwrap();
        validate_volume(&conn, &pool, &mut vol).unwrap();
        assert!(vol.get_xml().contains("<lazy_refcounts/>"));
        let mut huge = parse_volume(&format!(
            "<volume><name>{name}.img</name><capacity>{0}</capacity>\
             <allocation>{0}</allocation></volume>",
            u64::MAX / 2
        ))
        .unwrap();
        assert!(validate_volume(&conn, &pool, &mut huge)
            .unwrap_err()
            .starts_with("There is not enough free space on the storage pool"));
        assert_eq!(huge.get_str("format").as_deref(), Some("raw"));
        let mut bad = parse_volume(&format!(
            "<volume><name>{name}.raw</name><capacity>lots</capacity></volume>"
        ))
        .unwrap();
        assert_eq!(
            validate_volume(&conn, &pool, &mut bad).unwrap_err(),
            "invalid literal for int() with base 10: 'lots'"
        );
        let created = install_volume(&conn, &pool, &vol.get_xml()).unwrap();
        let mut again = parse_volume(&vol.get_xml()).unwrap();
        assert_eq!(
            validate_volume(&conn, &pool, &mut again).unwrap_err(),
            format!("Name '{volname}' already in use by another volume.")
        );
        created.delete(0).unwrap();
        conn.close().unwrap();
    }
}
