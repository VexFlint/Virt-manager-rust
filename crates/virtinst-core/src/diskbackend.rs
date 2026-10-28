//! The storage behind a disk path — upstream's `diskbackend.py`
//! classes (`StorageBackend`, `ManagedStorageCreator`,
//! `CloneStorageCreator`) and the path helpers they use (`_get_dev_type`,
//! `_get_size`, `check_path_search`). The pool/volume lookups they sit
//! on (`manage_path`, `_check_if_path_managed`) live in `storage.rs`.
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

/// `capabilities.host.get_qemu_baselabel`: the user and uid of the
/// DAC `qemu`/`kvm` base label.
fn qemu_baselabel(conn: &Connect) -> Option<(String, u32)> {
    // `conn.caps`: a magic URI's `caps=` file stands in for libvirt.
    let xml = match std::env::var_os("VIRTINST_TEST_CAPS_FILE") {
        Some(path) => std::fs::read_to_string(path).ok()?,
        None => conn.get_capabilities().ok()?,
    };
    let doc = virtinst_xml::parse_libvirt_xml(&xml).ok()?;
    let host = doc.root_element()?.find(&doc, "host")?;
    for secmodel in host.find_all(&doc, "secmodel") {
        let model = secmodel.find(&doc, "model").map(|m| m.text_content(&doc));
        if model.as_deref() != Some("dac") {
            continue;
        }
        let label = secmodel
            .find_all(&doc, "baselabel")
            .into_iter()
            .find(|b| matches!(b.attribute(&doc, "type"), Some("qemu" | "kvm")))
            .map(|b| b.text_content(&doc));
        let Some(label) = label else {
            continue;
        };
        let uid: u32 = label
            .split(':')
            .next()?
            .replace('+', "")
            .trim()
            .parse()
            .ok()?;
        return Some((username_of(uid)?, uid));
    }
    None
}

/// `pwd.getpwuid(uid)[0]`.
fn username_of(uid: u32) -> Option<String> {
    // SAFETY: getpwuid_r writes into our buffers; the name pointer is
    // read before they go out of scope.
    unsafe {
        let mut pwd: libc::passwd = std::mem::zeroed();
        let mut buf = vec![0 as libc::c_char; 4096];
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        if libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr(), buf.len(), &mut result) != 0
            || result.is_null()
        {
            return None;
        }
        Some(
            std::ffi::CStr::from_ptr(pwd.pw_name)
                .to_string_lossy()
                .into_owned(),
        )
    }
}

/// `_is_dir_searchable`: the mode bits, then the POSIX ACL.
fn is_dir_searchable(dir: &str, uid: u32, username: &str) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = std::fs::metadata(dir) else {
        return false;
    };
    let flag = if uid == meta.uid() {
        0o100
    } else if uid == meta.gid() {
        0o010
    } else {
        0o001
    };
    if meta.mode() & flag != 0 {
        return true;
    }
    // Check POSIX ACL (since that is what we use to 'fix' access)
    let Ok(out) = std::process::Command::new("getfacl").arg(dir).output() else {
        log::debug!("Didn't find the getfacl command.");
        return false;
    };
    if !out.status.success() {
        return false;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let prefix = format!("user:{username}:");
    text.lines().any(|l| {
        l.strip_prefix(&prefix)
            .is_some_and(|rest| rest.len() >= 3 && rest.as_bytes()[2] == b'x')
    })
}

/// `DeviceDisk.check_path_search`'s result.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SearchData {
    pub user: Option<String>,
    pub uid: Option<u32>,
    pub fixlist: Vec<String>,
}

/// `DeviceDisk.check_path_search`: the directories of `path` the
/// qemu DAC user can't search, on a local privileged qemu connection.
pub fn check_path_search(
    conn: &Connect,
    uri: &crate::connection::Uri,
    path: Option<&str>,
) -> SearchData {
    let mut data = SearchData::default();
    log::debug!(
        "DeviceDisk.check_path_search path={}",
        path.unwrap_or("None")
    );
    let Some(path) = path else {
        return data;
    };
    let privileged = uri.path != "/session";
    if uri.is_remote() || !(uri.is_qemu() && privileged) {
        return data;
    }
    if path_is_url(path) || path_is_network_vol(conn, path) {
        return data;
    }
    let path = storage::abspath(path);
    let Some((user, uid)) = qemu_baselabel(conn) else {
        return data;
    };
    if uid == 0 && std::env::var_os("VIRTINST_TEST_SUITE").is_none() {
        return data;
    }
    // `is_path_searchable`
    let (mut dir, mut base) = if std::path::Path::new(&path).is_dir() {
        (path.clone(), "-".to_string())
    } else {
        (
            storage::dirname(&path).to_string(),
            storage::basename(&path).to_string(),
        )
    };
    let mut fixlist = vec![];
    while !base.is_empty() {
        if !is_dir_searchable(&dir, uid, &user) {
            fixlist.push(dir.clone());
        }
        base = storage::basename(&dir).to_string();
        dir = storage::dirname(&dir).to_string();
    }
    fixlist.reverse();
    data.user = Some(user);
    data.uid = Some(uid);
    data.fixlist = fixlist;
    data
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
}
