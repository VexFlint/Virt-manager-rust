//! The storage behind a disk path — upstream's `diskbackend.py`
//! classes (`StorageBackend`, `ManagedStorageCreator`,
//! `CloneStorageCreator`) and the path helpers they use (`_get_dev_type`,
//! `_get_size`). The pool/volume lookups they sit on (`manage_path`,
//! `_check_if_path_managed`) live in `storage.rs`.
//!
//! Also the ACL/path perm helpers behind `DeviceDisk.check_path_search`
//! and `fix_path_search` (in `devices/disk.rs`): `is_path_searchable`
//! and `set_dirs_searchable` (`setfacl`, falling back to `chmod o+x`).
//!
//! Only what `virt-clone`'s `Cloner` needs is here so far: an existing
//! path's backend, and the two ways of creating a cloned disk
//! (`createXMLFrom` through libvirt, or a local byte copy).

use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};

use virt::connect::Connect;
use virt::storage_pool::StoragePool;
use virt::storage_vol::StorageVol;
use virt::sys;

use crate::progress::Meter;
use crate::storage::{self, can_auto_manage, path_is_url, xml_escape, Pool, PoolXml, VolInstall};

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// The parts of a volume's XML the backends read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VolXml {
    pub name: String,
    /// `<volume type=>`.
    pub vol_type: Option<String>,
    pub target_path: Option<String>,
    pub format: Option<String>,
    pub capacity: u64,
    pub allocation: u64,
    pub perms_mode: Option<String>,
}

impl VolXml {
    pub fn parse(xml: &str) -> Option<VolXml> {
        let doc = virtinst_xml::parse_libvirt_xml(xml).ok()?;
        let root = doc.root_element()?;
        let el = |path: &[&str]| virtinst_xml::resolve_path(&doc, root, path);
        let text = |path: &[&str]| el(path).map(|e| e.text_content(&doc).trim().to_string());
        let num = |tag: &str| text(&[tag]).and_then(|t| t.parse().ok()).unwrap_or(0);
        Some(VolXml {
            name: text(&["name"]).unwrap_or_default(),
            vol_type: root.attribute(&doc, "type").map(str::to_string),
            target_path: text(&["target", "path"]),
            format: el(&["target", "format"])
                .and_then(|e| e.attribute(&doc, "type"))
                .map(str::to_string),
            capacity: num("capacity"),
            allocation: num("allocation"),
            perms_mode: text(&["target", "permissions", "mode"]),
        })
    }

    pub fn of(vol: &StorageVol) -> Option<VolXml> {
        VolXml::parse(&vol.get_xml_desc(0).ok()?)
    }

    /// `StorageVolume.file_type`: the volume's own type, else what its
    /// pool makes.
    fn file_type(&self, pool: Option<&PoolXml>) -> String {
        match self.vol_type.as_deref() {
            Some(t @ ("file" | "block" | "dir" | "network" | "netdir")) => t.into(),
            _ => pool.map(|p| p.get_disk_type()).unwrap_or("file").into(),
        }
    }

    /// `StorageVolume.supports_format`.
    pub fn supports_format(&self, pool: Option<&PoolXml>) -> bool {
        self.file_type(pool) == "file"
    }
}

/// `str(OSError)` as Python prints it: `[Errno 2] No such file or
/// directory: '/path'`, the filename only when the failing call had one.
/// Rust's own `Display` adds ` (os error 2)` instead.
pub fn oserror_str(e: &std::io::Error, filename: Option<&str>) -> String {
    let Some(errno) = e.raw_os_error() else {
        return e.to_string();
    };
    let msg = e.to_string();
    let strerror = msg
        .strip_suffix(&format!(" (os error {errno})"))
        .unwrap_or(&msg);
    match filename {
        Some(f) => format!("[Errno {errno}] {strerror}: '{f}'"),
        None => format!("[Errno {errno}] {strerror}"),
    }
}

/// `_stat_is_block`.
fn stat_is_block(path: &str) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.file_type().is_block_device())
}

/// `_get_size`: a local file's (or block device's) size in bytes.
pub fn get_local_size(path: &str) -> u64 {
    let Ok(meta) = std::fs::metadata(path) else {
        return 0;
    };
    if meta.file_type().is_block_device() {
        // `_get_block_size`: seek to the end.
        use std::io::Seek;
        return std::fs::File::open(path)
            .and_then(|mut f| f.seek(std::io::SeekFrom::End(0)))
            .unwrap_or(0);
    }
    meta.len()
}

/// `_get_dev_type`: the disk `type` for this storage.
pub fn get_dev_type(
    path: Option<&str>,
    vol_xml: Option<&VolXml>,
    vol: Option<&StorageVol>,
    pool_xml: Option<&PoolXml>,
    remote: bool,
) -> String {
    if let Some(t) = vol_xml.and_then(|v| v.vol_type.as_deref()) {
        return t.into();
    }
    if let Some(pool) = pool_xml {
        match pool.get_disk_type() {
            "block" => return "block".into(),
            "network" => return "network".into(),
            _ => {}
        }
    }
    if let Some(vol) = vol {
        match vol.get_info().map(|i| i.kind) {
            Ok(sys::VIR_STORAGE_VOL_FILE) => return "file".into(),
            Ok(sys::VIR_STORAGE_VOL_BLOCK) => return "block".into(),
            Ok(sys::VIR_STORAGE_VOL_NETWORK) => return "network".into(),
            _ => {}
        }
    }
    if let Some(path) = path {
        if path_is_url(path) {
            return "network".into();
        }
        if remote {
            if !can_auto_manage(path) {
                // Just a heurisitic, if this path is one of the ones
                // we don't try to auto-import, then consider it a
                // block device, because managing those correctly is difficult
                return "block".into();
            }
        } else if std::path::Path::new(path).is_dir() {
            return "dir".into();
        } else if stat_is_block(path) {
            return "block".into();
        }
    }
    "file".into()
}

/// `StorageBackend`: existing storage a disk points at — a managed
/// volume, a path in a pool, or a plain path.
pub struct StorageBackend {
    path: Option<String>,
    pub vol: Option<StorageVol>,
    pub pool: Option<Pool>,
    remote: bool,
    vol_xml: Option<VolXml>,
    /// `ManagedStorageCreator`: the volume to create in `pool`.
    vol_install: Option<Box<VolInstall>>,
}

impl StorageBackend {
    pub fn new(
        path: Option<String>,
        vol: Option<StorageVol>,
        pool: Option<Pool>,
        remote: bool,
    ) -> StorageBackend {
        let vol_xml = vol.as_ref().and_then(VolXml::of);
        StorageBackend {
            path: if vol.is_some() { None } else { path },
            vol,
            pool,
            remote,
            vol_xml,
            vol_install: None,
        }
    }

    /// `ManagedStorageCreator(vol_install)`: storage to create, `pool`
    /// being the volume's.
    pub fn for_vol_install(vol_install: VolInstall, pool: Pool, remote: bool) -> StorageBackend {
        StorageBackend {
            path: None,
            vol: None,
            pool: Some(pool),
            remote,
            vol_xml: None,
            vol_install: Some(Box::new(vol_install)),
        }
    }

    /// The volume this backend will create.
    pub fn vol_install(&self) -> Option<&VolInstall> {
        self.vol_install.as_deref()
    }

    /// `will_create_storage`.
    pub fn will_create_storage(&self) -> bool {
        self.vol_install.is_some()
    }

    /// `manage_path` then a backend for what it found: what
    /// `DeviceDisk.set_source_path` does.
    pub fn for_path(conn: &Connect, path: Option<&str>, remote: bool) -> Result<Self, String> {
        let Some(path) = path else {
            return Ok(StorageBackend::new(None, None, None, remote));
        };
        let managed = storage::manage_path(conn, path)?;
        Ok(StorageBackend::new(
            Some(managed.path),
            managed.vol,
            managed.pool,
            remote,
        ))
    }

    pub fn get_path(&self) -> Option<String> {
        if let Some(vi) = &self.vol_install {
            if self.get_dev_type() == "network" {
                return Some(vi.name.clone());
            }
            // `os.path.join(target_path, name)`
            let mut path = self
                .pool_xml()
                .and_then(|p| p.target_path.clone())
                .unwrap_or_default();
            if !path.is_empty() && !path.ends_with('/') {
                path.push('/');
            }
            return Some(path + &vi.name);
        }
        match &self.vol_xml {
            Some(v) => v.target_path.clone(),
            None => self.path.clone(),
        }
    }

    pub fn vol_xml(&self) -> Option<&VolXml> {
        self.vol_xml.as_ref()
    }

    pub fn pool_xml(&self) -> Option<&PoolXml> {
        self.pool.as_ref().map(|p| &p.xml)
    }

    /// Size in GiB.
    pub fn get_size(&self) -> f64 {
        let bytes = if let Some(vi) = &self.vol_install {
            vi.capacity as u64
        } else if let Some(v) = &self.vol_xml {
            v.capacity
        } else if let Some(path) = &self.path {
            get_local_size(path)
        } else {
            0
        };
        bytes as f64 / GIB
    }

    pub fn exists(&self) -> bool {
        if self.vol_install.is_some() {
            return false;
        }
        if self.vol.is_some() || self.path.is_none() {
            return true;
        }
        let path = self.path.as_deref().unwrap_or_default();
        let dev_type = self.get_dev_type();
        if dev_type != "network" && !self.remote && std::path::Path::new(path).exists() {
            true
        } else if self.pool.is_some() {
            false
        } else if dev_type == "network" {
            true
        } else {
            // This allows users to pass /dev/sdX and we don't try to
            // validate it exists on the remote connection, since
            // autopooling /dev is perilous. Libvirt will error if
            // the device doesn't exist.
            self.remote && !can_auto_manage(path)
        }
    }

    pub fn get_dev_type(&self) -> String {
        get_dev_type(
            self.path.as_deref(),
            self.vol_xml.as_ref(),
            self.vol.as_ref(),
            self.pool_xml(),
            self.remote,
        )
    }

    pub fn get_driver_type(&self) -> Option<String> {
        if let Some(vi) = &self.vol_install {
            if vi.supports_format() {
                return vi.format.clone();
            }
            return Some("raw".into());
        }
        let v = self.vol_xml.as_ref()?;
        if v.supports_format(self.pool_xml()) {
            v.format.clone()
        } else {
            Some("raw".into())
        }
    }
}

/// `StorageVolume` cloned from an input volume
/// (`_build_clone_vol_install` + `set_input_vol`), created with
/// `createXMLFrom` — `ManagedStorageCreator` for a clone.
pub struct CloneVolInstall {
    pub pool: Pool,
    pub name: String,
    pub capacity: u64,
    pub allocation: u64,
    pub format: Option<String>,
    pub perms_mode: Option<String>,
    pub input_vol: StorageVol,
    pub reflink: bool,
}

impl CloneVolInstall {
    /// `_build_clone_vol_install`: `name` in `pool`, with format,
    /// capacity, allocation and mode taken from `input_vol`.
    pub fn new(name: &str, pool: Pool, input_vol: StorageVol) -> Result<Self, String> {
        let input = VolXml::of(&input_vol).ok_or("Could not read the input volume's XML")?;
        Ok(CloneVolInstall {
            pool,
            name: name.into(),
            capacity: input.capacity,
            allocation: input.allocation,
            format: input.format,
            perms_mode: input.perms_mode,
            input_vol,
            reflink: false,
        })
    }

    fn file_type(&self) -> &'static str {
        self.pool.xml.get_disk_type()
    }

    pub fn supports_format(&self) -> bool {
        self.file_type() == "file"
    }

    /// The new volume's path: the pool's target plus its name.
    pub fn get_path(&self) -> String {
        if self.file_type() == "network" {
            return self.name.clone();
        }
        let dir = self.pool.xml.target_path.clone().unwrap_or_default();
        format!("{}/{}", dir.trim_end_matches('/'), self.name)
    }

    pub fn get_dev_type(&self, remote: bool) -> String {
        get_dev_type(
            Some(&self.get_path()),
            None,
            None,
            Some(&self.pool.xml),
            remote,
        )
    }

    /// `is_size_conflict`: `(fatal, message)`.
    pub fn is_size_conflict(&self) -> (bool, Option<String>) {
        const MIB: u64 = 1024 * 1024;
        let avail = self
            .pool
            .pool
            .get_info()
            .map(|i| i.available)
            .unwrap_or(u64::MAX);
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

    /// `StorageVolume.validate`.
    pub fn validate(&mut self) -> Result<(), String> {
        storage::validate_generic_name("Storage object", &self.name)?;
        if StorageVol::lookup_by_name(&self.pool.pool, &self.name).is_ok() {
            return Err(format!(
                "Name '{}' already in use by another volume.",
                self.name
            ));
        }
        if self.format.is_none() && self.supports_format() {
            self.format = Some("raw".into());
        }
        if self.pool.xml.pool_type == "logical" && self.allocation != self.capacity {
            log::warn!(
                "Sparse logical volumes are not supported, setting allocation equal to capacity"
            );
            self.allocation = self.capacity;
        }
        match self.is_size_conflict() {
            (true, Some(msg)) => return Err(msg),
            (false, Some(msg)) => log::warn!("{msg}"),
            _ => {}
        }
        Ok(())
    }

    pub fn to_xml(&self) -> String {
        let mut target = String::new();
        if let Some(fmt) = &self.format {
            target += &format!("<format type='{}'/>", xml_escape(fmt));
        }
        if let Some(mode) = &self.perms_mode {
            target += &format!(
                "<permissions><mode>{}</mode></permissions>",
                xml_escape(mode)
            );
        }
        if self.format.as_deref() == Some("qcow2") {
            target += "<features><lazy_refcounts/></features>";
        }
        format!(
            "<volume><name>{}</name><capacity>{}</capacity><allocation>{}</allocation>\
             <target>{target}</target></volume>",
            xml_escape(&self.name),
            self.capacity,
            self.allocation
        )
    }

    /// `StorageVolume.install`, cloning `input_vol`. `really_test`:
    /// libvirt's test driver takes no flags.
    pub fn install(&self, meter: &mut dyn Meter, really_test: bool) -> Result<StorageVol, String> {
        let xml = self.to_xml();
        log::debug!("Creating storage volume '{}' with xml:\n{xml}", self.name);
        let mut cloneflags = 0;
        if self.format.as_deref() == Some("qcow2") && self.capacity == self.allocation {
            // For cloning, this flag will make libvirt+qemu-img preallocate
            // the new disk image
            cloneflags |= sys::VIR_STORAGE_VOL_CREATE_PREALLOC_METADATA;
        }
        if self.reflink {
            if self.format.as_deref() != Some("raw") {
                log::warn!("skipping reflink for non-raw vol={}", self.name);
            } else {
                cloneflags |= sys::VIR_STORAGE_VOL_CREATE_REFLINK;
            }
        }
        if really_test {
            cloneflags = 0;
        }
        meter.start(&format!("Allocating '{}'", self.name), Some(self.capacity));
        let ret = StorageVol::create_xml_from(&self.pool.pool, &xml, &self.input_vol, cloneflags)
            .map_err(|e| {
            format!(
                "Couldn't create storage volume '{}': '{}'",
                self.name,
                e.message()
            )
        })?;
        meter.update(self.capacity);
        meter.end();
        log::debug!("Storage volume '{}' install complete.", self.name);
        Ok(ret)
    }
}

/// `CloneStorageCreator`: a local file copied byte by byte, for
/// storage libvirt doesn't manage.
pub struct CloneStorageCreator {
    pub output_path: String,
    pub input_path: String,
    /// GiB.
    pub size: f64,
    pub sparse: bool,
    remote: bool,
}

impl CloneStorageCreator {
    pub fn new(output_path: &str, input_path: &str, size: f64, sparse: bool, remote: bool) -> Self {
        CloneStorageCreator {
            output_path: output_path.into(),
            input_path: input_path.into(),
            size,
            sparse,
            remote,
        }
    }

    pub fn get_dev_type(&self) -> String {
        get_dev_type(Some(&self.output_path), None, None, None, self.remote)
    }

    /// `is_size_conflict`: `(fatal, message)`.
    pub fn is_size_conflict(&self) -> (bool, Option<String>) {
        let avail = if self.get_dev_type() == "block" {
            get_local_size(&self.output_path)
        } else {
            let dir = storage::dirname(&storage::abspath(&self.output_path)).to_string();
            statvfs_avail(&dir)
        };
        let need = (self.size as u64) * 1024 * 1024 * 1024;
        if need <= avail {
            return (false, None);
        }
        let (fatal, msg) = if self.sparse {
            (
                false,
                "The filesystem will not have enough free space to fully allocate the sparse \
                 file when the guest is running.",
            )
        } else {
            (true, "There is not enough free space to create the disk.")
        };
        (
            fatal,
            Some(format!(
                "{msg} {} M requested > {} M available",
                need / (1024 * 1024),
                avail / (1024 * 1024)
            )),
        )
    }

    pub fn validate(&self) -> Result<(), String> {
        match self.is_size_conflict() {
            (true, Some(msg)) => Err(msg),
            (false, Some(msg)) => {
                log::warn!("{msg}");
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// `create`: copies `input_path` to `output_path`, skipping runs of
    /// zeros when the new file is sparse.
    pub fn create(&self, meter: &mut dyn Meter) -> Result<(), String> {
        use std::io::{Read, Seek, SeekFrom, Write};

        let name = storage::basename(&self.input_path);
        let size_bytes = (self.size * GIB) as u64;
        meter.start(&format!("Cloning {name}"), Some(size_bytes));

        if self.input_path == "/dev/null" {
            // Not really sure why this check is here,
            // but keeping for compat
            log::debug!("Source dev was /dev/null. Skipping");
            return Ok(());
        }
        if self.input_path == self.output_path {
            log::debug!("Source and destination are the same. Skipping.");
            return Ok(());
        }

        // Only failures inside upstream's inner `try` get the "Error
        // cloning" wrapping; the sparse pre-allocation is outside it.
        let err = |e: std::io::Error, filename: Option<&str>| {
            format!(
                "Error cloning diskimage {} to {}: {}",
                self.input_path,
                self.output_path,
                oserror_str(&e, filename)
            )
        };
        let out = Some(self.output_path.as_str());
        // If a destination file exists and sparse flag is True,
        // this priority takes an existing file.
        let (block_size, sparse) =
            if !std::path::Path::new(&self.output_path).exists() && self.sparse {
                let f = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .mode(0o640)
                    .open(&self.output_path)
                    .map_err(|e| oserror_str(&e, out))?;
                f.set_len(size_bytes).map_err(|e| oserror_str(&e, None))?;
                (4096, true)
            } else {
                (1024 * 1024 * 10, false)
            };
        log::debug!(
            "Local Cloning {} to {}, sparse={sparse}, block_size={block_size}",
            self.input_path,
            self.output_path
        );

        let mut src =
            std::fs::File::open(&self.input_path).map_err(|e| err(e, Some(&self.input_path)))?;
        let mut dst = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o640)
            .open(&self.output_path)
            .map_err(|e| err(e, out))?;
        let mut buf = vec![0u8; block_size];
        let mut total: u64 = 0;
        loop {
            let s = read_full(&mut src, &mut buf).map_err(|e| err(e, None))?;
            if s == 0 {
                meter.end();
                break;
            }
            let data = &buf[..s];
            // check sequence of zeros
            if sparse && s == 4096 && data.iter().all(|b| *b == 0) {
                dst.seek(SeekFrom::Current(s as i64))
                    .map_err(|e| err(e, None))?;
            } else {
                dst.write_all(data).map_err(|e| err(e, None))?;
            }
            total += s as u64;
            if total < size_bytes {
                meter.update(total);
            }
        }
        // `os.read` with Python's semantics: short reads only at EOF.
        fn read_full(f: &mut std::fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
            let mut n = 0;
            while n < buf.len() {
                match f.read(&mut buf[n..])? {
                    0 => break,
                    r => n += r,
                }
            }
            Ok(n)
        }
        Ok(())
    }
}

/// Free bytes for an unprivileged user on the filesystem holding `dir`.
fn statvfs_avail(dir: &str) -> u64 {
    let Ok(c) = std::ffi::CString::new(dir) else {
        return 0;
    };
    // SAFETY: statvfs fills a plain struct from a NUL-terminated path.
    unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut st) != 0 {
            return 0;
        }
        st.f_frsize as u64 * st.f_bavail as u64
    }
}

/// `StoragePool.lookup_pool_by_path` for a `Pool`, refreshed.
pub fn pool_of(pool: &StoragePool) -> Option<Pool> {
    let xml = PoolXml::parse(&pool.get_xml_desc(0).ok()?)?;
    Some(Pool {
        pool: pool.clone(),
        xml,
    })
}

/// `path_is_network_vol`: whether a volume with this target path is a
/// network volume.
pub fn path_is_network_vol(conn: &Connect, path: &str) -> bool {
    for pool in storage::fetch_all_pools(conn) {
        for vol in pool.pool.list_all_volumes(0).unwrap_or_default() {
            if let Some(v) = VolXml::of(&vol) {
                if v.target_path.as_deref() == Some(path) {
                    return v.vol_type.as_deref() == Some("network");
                }
            }
        }
    }
    false
}

// ---------------------------------------------------------------------
// ACL/path perm helpers
// ---------------------------------------------------------------------

/// `SETFACL`: the command `_fix_perms_acl` runs.
pub const SETFACL: &str = "setfacl";

/// Python's `os.path.split`: `(head, tail)`, with the head's trailing
/// slashes stripped unless it is all slashes.
fn os_path_split(path: &str) -> (String, String) {
    let i = path.rfind('/').map_or(0, |i| i + 1);
    let (head, tail) = path.split_at(i);
    let head = if !head.is_empty() && head.chars().any(|c| c != '/') {
        head.trim_end_matches('/')
    } else {
        head
    };
    (head.to_string(), tail.to_string())
}

/// Python's `repr()` of a `str` list, as upstream's debug messages
/// print the commands they run.
fn py_list_repr(items: &[&str]) -> String {
    let items: Vec<String> = items.iter().map(|i| format!("'{i}'")).collect();
    format!("[{}]", items.join(", "))
}

/// `_fix_perms_acl`: `setfacl --modify user:<username>:x <dirname>`,
/// with `setfacl` standing in for [`SETFACL`].
fn fix_perms_acl(dirname: &str, username: &str, setfacl: &str) -> Result<(), String> {
    let cmd = [setfacl, "--modify", &format!("user:{username}:x"), dirname];
    let out = std::process::Command::new(cmd[0])
        .args(&cmd[1..])
        .output()
        .map_err(|e| oserror_str(&e, Some(setfacl)))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    log::debug!("Ran command '{}'", py_list_repr(&cmd));
    if !stdout.is_empty() || !stderr.is_empty() {
        log::debug!("out={stdout}\nerr={stderr}");
    }

    if !out.status.success() {
        return Err(stderr.into_owned());
    }
    Ok(())
}

/// `_fix_perms_chmod`: add `o+x` to `dirname`.
fn fix_perms_chmod(dirname: &str) -> Result<(), String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    log::debug!("Setting +x on {dirname}");
    let stat = |d: &str| {
        std::fs::metadata(d)
            .map(|m| m.mode())
            .map_err(|e| oserror_str(&e, Some(d)))
    };
    let mode = stat(dirname)?;
    let newmode = mode | 0o001;
    std::fs::set_permissions(dirname, std::fs::Permissions::from_mode(newmode & 0o7777))
        .map_err(|e| oserror_str(&e, Some(dirname)))?;
    if stat(dirname)? != newmode {
        // Trying to change perms on vfat at least doesn't work
        // but also doesn't seem to error. Try and detect that
        return Err(format!("Permissions on '{dirname}' did not stick"));
    }
    Ok(())
}

/// `set_dirs_searchable`: give `username` search access to each of
/// `dirlist` — `setfacl` while it works, `chmod o+x` from the first
/// time it doesn't. The `(dir, error)` pairs that couldn't be fixed,
/// in `dirlist` order (upstream's `errdict`).
pub fn set_dirs_searchable(dirlist: &[String], username: &str) -> Vec<(String, String)> {
    set_dirs_searchable_with(dirlist, username, SETFACL)
}

/// [`set_dirs_searchable`] running `setfacl` as the ACL command — the
/// test suite's `SETFACL` override.
fn set_dirs_searchable_with(
    dirlist: &[String],
    username: &str,
    setfacl: &str,
) -> Vec<(String, String)> {
    let mut useacl = true;
    let mut errdict = vec![];
    for dirname in dirlist {
        if useacl {
            match fix_perms_acl(dirname, username, setfacl) {
                Ok(()) => continue,
                Err(e) => {
                    log::debug!("setfacl failed: {e}");
                    log::debug!("trying chmod");
                    useacl = false;
                }
            }
        }

        // If we reach here, ACL setting failed, try chmod
        if let Err(e) = fix_perms_chmod(dirname) {
            errdict.push((dirname.clone(), e));
        }
    }
    errdict
}

/// `_is_dir_searchable`'s mode check: the owner bit when `uid` owns the
/// directory, the group bit when `uid` equals its *gid* (upstream
/// compares the uid with `st_gid`, not the user's groups), the other
/// bit otherwise.
fn mode_is_searchable(mode: u32, st_uid: u32, st_gid: u32, uid: u32) -> bool {
    let flag = if uid == st_uid {
        0o100
    } else if uid == st_gid {
        0o010
    } else {
        0o001
    };
    mode & flag != 0
}

/// `_is_dir_searchable`'s ACL check: whether `getfacl` output has
/// search access for `username` (`re.search("user:%s:..x")`).
fn acl_is_searchable(getfacl_out: &str, username: &str) -> bool {
    regex::Regex::new(&format!("user:{username}:..x")).is_ok_and(|re| re.is_match(getfacl_out))
}

/// `_is_dir_searchable`: whether `dirname` is searchable by `uid`.
fn is_dir_searchable(dirname: &str, uid: u32, username: &str) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(statinfo) = std::fs::metadata(dirname) else {
        return false;
    };

    if mode_is_searchable(statinfo.mode(), statinfo.uid(), statinfo.gid(), uid) {
        return true;
    }

    // Check POSIX ACL (since that is what we use to 'fix' access)
    let cmd = ["getfacl", dirname];
    let Ok(out) = std::process::Command::new(cmd[0]).arg(cmd[1]).output() else {
        log::debug!("Didn't find the getfacl command.");
        return false;
    };

    if !out.status.success() {
        log::debug!(
            "Cmd '{}' failed: {}",
            py_list_repr(&cmd),
            String::from_utf8_lossy(&out.stderr)
        );
        return false;
    }

    acl_is_searchable(&String::from_utf8_lossy(&out.stdout), username)
}

/// `is_path_searchable`: each dir component of `path` that `uid`/
/// `username` can't search, deepest first.
pub fn is_path_searchable(path: &str, uid: u32, username: &str) -> Vec<String> {
    let (mut dirname, mut base) = if std::path::Path::new(path).is_dir() {
        (path.to_string(), "-".to_string())
    } else {
        os_path_split(path)
    };

    let mut fixlist = vec![];
    while !base.is_empty() {
        if !is_dir_searchable(&dirname, uid, username) {
            fixlist.push(dirname.clone());
        }
        (dirname, base) = os_path_split(&dirname);
    }

    fixlist
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::make_meter;

    #[test]
    fn vol_xml_parses_the_fields_cloning_copies() {
        let v = VolXml::parse(
            "<volume type='file'><name>a.img</name><capacity unit='bytes'>1000</capacity>\
             <allocation unit='bytes'>10</allocation><target><path>/p/a.img</path>\
             <format type='qcow2'/><permissions><mode>0600</mode></permissions></target></volume>",
        )
        .unwrap();
        assert_eq!(v.name, "a.img");
        assert_eq!(v.vol_type.as_deref(), Some("file"));
        assert_eq!(v.target_path.as_deref(), Some("/p/a.img"));
        assert_eq!(v.format.as_deref(), Some("qcow2"));
        assert_eq!((v.capacity, v.allocation), (1000, 10));
        assert_eq!(v.perms_mode.as_deref(), Some("0600"));
    }

    #[test]
    fn oserror_reads_like_python() {
        let e = std::fs::File::open("/nonexistent/x.img").unwrap_err();
        assert_eq!(
            oserror_str(&e, Some("/nonexistent/x.img")),
            "[Errno 2] No such file or directory: '/nonexistent/x.img'"
        );
        assert_eq!(oserror_str(&e, None), "[Errno 2] No such file or directory");
    }

    #[test]
    fn dev_type_of_plain_paths() {
        assert_eq!(
            get_dev_type(Some("http://x/y"), None, None, None, false),
            "network"
        );
        assert_eq!(get_dev_type(Some("/"), None, None, None, false), "dir");
        assert_eq!(
            get_dev_type(Some("/nonexistent"), None, None, None, false),
            "file"
        );
        assert_eq!(
            get_dev_type(Some("/dev/sdz"), None, None, None, true),
            "block"
        );
    }

    #[test]
    fn local_clone_copies_content_sparsely() {
        let dir = std::env::temp_dir().join(format!("virtinst-clone-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.img");
        let dst = dir.join("dst.img");
        let mut data = vec![0u8; 3 * 4096];
        data[5000] = 7;
        data.extend_from_slice(b"tail");
        std::fs::write(&src, &data).unwrap();
        let size = data.len() as f64 / GIB;
        let c = CloneStorageCreator::new(
            dst.to_str().unwrap(),
            src.to_str().unwrap(),
            size,
            true,
            false,
        );
        c.create(&mut make_meter(true)).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), data);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn os_path_split_matches_python() {
        let split = os_path_split;
        let s = |a: &str, b: &str| (a.to_string(), b.to_string());
        assert_eq!(split("/a/b"), s("/a", "b"));
        assert_eq!(split("/a"), s("/", "a"));
        assert_eq!(split("/"), s("/", ""));
        assert_eq!(split("//"), s("//", ""));
        assert_eq!(split("/a//b"), s("/a", "b"));
        assert_eq!(split("/a/"), s("/a", ""));
        assert_eq!(split("a"), s("", "a"));
        assert_eq!(split(""), s("", ""));
    }

    #[test]
    fn mode_check_uses_the_uid_and_upstreams_gid_comparison() {
        // The owner's bit when the uid owns the directory
        assert!(mode_is_searchable(0o100, 107, 36, 107));
        assert!(!mode_is_searchable(0o011, 107, 36, 107));
        // The group bit when the uid equals the directory's gid
        assert!(mode_is_searchable(0o010, 0, 107, 107));
        assert!(!mode_is_searchable(0o101, 0, 107, 107));
        // The other bit for everybody else, groups aren't consulted
        assert!(mode_is_searchable(0o001, 0, 0, 107));
        assert!(!mode_is_searchable(0o770, 0, 36, 107));
    }

    #[test]
    fn acl_check_is_upstreams_regex() {
        let out = "# file: var/lib/foo\n# owner: root\n# group: root\nuser::rwx\n\
                   user:qemu:r-x\nuser:other:rw-\ngroup::r-x\nmask::r-x\nother::---\n";
        assert!(acl_is_searchable(out, "qemu"));
        assert!(!acl_is_searchable(out, "other"));
        assert!(!acl_is_searchable(out, "nobody"));
        // re.search: matches anywhere, default ACLs included
        assert!(acl_is_searchable("default:user:qemu:--x\n", "qemu"));
        assert!(!acl_is_searchable("user:qemux:--x\n", "qemu"));
        // `.` doesn't cross lines
        assert!(!acl_is_searchable("user:qemu:\nx\n", "qemu"));
    }

    /// A fresh directory tree under the temp dir, removed on drop.
    struct TmpTree(std::path::PathBuf);

    impl TmpTree {
        fn new(name: &str) -> TmpTree {
            let dir =
                std::env::temp_dir().join(format!("virtinst-test-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TmpTree(dir)
        }

        fn path(&self, rel: &str) -> String {
            let p = if rel.is_empty() {
                self.0.clone()
            } else {
                self.0.join(rel)
            };
            p.to_str().unwrap().to_string()
        }

        fn chmod(&self, rel: &str, mode: u32) {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(self.path(rel), std::fs::Permissions::from_mode(mode))
                .unwrap();
        }

        fn mode(&self, rel: &str) -> u32 {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(self.path(rel)).unwrap().mode() & 0o7777
        }
    }

    impl Drop for TmpTree {
        fn drop(&mut self) {
            let _ = std::process::Command::new("chmod")
                .args(["-R", "u+rwx"])
                .arg(&self.0)
                .status();
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn path_searchable_walks_every_parent() {
        let tree = TmpTree::new("searchable");
        std::fs::create_dir_all(tree.path("top/mid")).unwrap();
        tree.chmod("top", 0o700);
        tree.chmod("top/mid", 0o755);
        let ours = |fixlist: Vec<String>| -> Vec<String> {
            fixlist
                .into_iter()
                .filter(|d| d.starts_with(&tree.path("")))
                .collect()
        };
        // SAFETY: getuid has no preconditions.
        let me = unsafe { libc::getuid() };

        // Our own uid owns the tree: everything searchable
        let disk = tree.path("top/mid/disk.img");
        assert_eq!(
            ours(is_path_searchable(&disk, me, "me")),
            Vec::<String>::new()
        );

        // Some other uid only has the "other" bits: top is 0700
        let fake = 4_242_424;
        assert_eq!(
            ours(is_path_searchable(&disk, fake, "virtinst-test-user")),
            vec![tree.path("top")]
        );

        // A directory path checks itself too, deepest first
        tree.chmod("top/mid", 0o000);
        assert_eq!(
            ours(is_path_searchable(&tree.path("top/mid"), me, "me")),
            vec![tree.path("top/mid")]
        );
        assert_eq!(
            ours(is_path_searchable(&tree.path("top/mid/disk.img"), me, "me")),
            vec![tree.path("top/mid")]
        );
        assert_eq!(
            ours(is_path_searchable(
                &tree.path("top/mid"),
                fake,
                "virtinst-test-user"
            )),
            vec![tree.path("top/mid"), tree.path("top")]
        );
    }

    #[test]
    fn set_dirs_searchable_tries_setfacl_then_chmod() {
        let tree = TmpTree::new("setfacl");
        std::fs::create_dir_all(tree.path("a")).unwrap();
        std::fs::create_dir_all(tree.path("b")).unwrap();
        let dirs = vec![tree.path("a"), tree.path("b")];

        // setfacl worked: no chmod
        tree.chmod("a", 0o700);
        tree.chmod("b", 0o700);
        assert_eq!(set_dirs_searchable_with(&dirs, "qemu", "true"), vec![]);
        assert_eq!((tree.mode("a"), tree.mode("b")), (0o700, 0o700));

        // setfacl failed: chmod o+x from then on
        assert_eq!(set_dirs_searchable_with(&dirs, "qemu", "false"), vec![]);
        assert_eq!((tree.mode("a"), tree.mode("b")), (0o701, 0o701));

        // No setfacl at all is a failure like any other
        tree.chmod("a", 0o700);
        let missing = "virtinst-test-no-such-setfacl";
        assert_eq!(
            set_dirs_searchable_with(&dirs[..1], "qemu", missing),
            vec![]
        );
        assert_eq!(tree.mode("a"), 0o701);

        // What chmod can't fix is reported, in order
        let gone = tree.path("gone");
        let res = set_dirs_searchable_with(&[gone.clone(), tree.path("a")], "qemu", "false");
        assert_eq!(
            res,
            vec![(
                gone.clone(),
                format!("[Errno 2] No such file or directory: '{gone}'")
            )]
        );
    }
}
