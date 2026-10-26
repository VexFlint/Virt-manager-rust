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

/// The parts of a pool's XML these helpers read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PoolXml {
    pub name: String,
    pub pool_type: String,
    pub target_path: Option<String>,
}

impl PoolXml {
    pub fn parse(xml: &str) -> Option<PoolXml> {
        let doc = virtinst_xml::parse_libvirt_xml(xml).ok()?;
        let root = doc.root_element()?;
        let text = |path: &[&str]| {
            virtinst_xml::resolve_path(&doc, root, path).map(|e| e.text_content(&doc))
        };
        Some(PoolXml {
            name: text(&["name"])?,
            pool_type: root.attribute(&doc, "type").unwrap_or_default().to_string(),
            target_path: text(&["target", "path"]),
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

/// A pool and its XML.
pub struct Pool {
    pub pool: StoragePool,
    pub xml: PoolXml,
}

fn fetch_all_pools(conn: &Connect) -> Vec<Pool> {
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

fn dirname(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(i) => &path[..i],
        None => "",
    }
}

fn basename(path: &str) -> &str {
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

fn lookup_vol_by_path(conn: &Connect, path: &str) -> Option<StorageVol> {
    let vol = StorageVol::lookup_by_path(conn, path).ok()?;
    // Make sure it wasn't removed behind libvirt's back.
    vol.get_info().ok()?;
    Some(vol)
}

/// `_check_if_path_managed`.
fn check_if_path_managed(
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
fn path_is_url(path: &str) -> bool {
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
fn can_auto_manage(path: &str) -> bool {
    if path_is_url(path) {
        return false;
    }
    !["/dev", "/sys", "/proc"]
        .iter()
        .any(|prefix| path == *prefix || path.starts_with(&format!("{prefix}/")))
}

/// `os.path.abspath`.
fn abspath(path: &str) -> String {
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

fn xml_escape(s: &str) -> String {
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
    pub capacity: u64,
    pub allocation: u64,
    pub format: Option<String>,
    /// `file_type`: what the pool makes, `file` for a directory.
    pub file_type: String,
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
        let capacity = (size * 1024.0 * 1024.0 * 1024.0) as u64;
        let file_type = pool.get_disk_type().to_string();
        let mut vol = VolInstall {
            pool_name: pool.name.clone(),
            name: volname.into(),
            capacity,
            allocation: if sparse { 0 } else { capacity },
            format: None,
            file_type,
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
        if pool.pool_type == "logical" {
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
        format!(
            "<volume><name>{}</name><capacity>{}</capacity><allocation>{}</allocation>\
             <target>{target}</target></volume>",
            xml_escape(&self.name),
            self.capacity,
            self.allocation
        )
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
        let pool = StoragePool::lookup_by_name(conn, &self.pool_name).map_err(|e| e.to_string())?;
        let flags = if self.format.as_deref() == Some("qcow2") {
            sys::VIR_STORAGE_VOL_CREATE_PREALLOC_METADATA
        } else {
            0
        };
        StorageVol::create_xml(&pool, &self.to_xml(), flags)
            .map_err(|e| format!("Couldn't create storage volume '{}': '{e}'", self.name))
    }
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
            let checkpath = crate::devices::disk::source_path(&doc, disk);
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
    fn vol_install() {
        let pool = PoolXml {
            name: "default".into(),
            pool_type: "dir".into(),
            target_path: Some("/var/lib/libvirt/images".into()),
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
}
