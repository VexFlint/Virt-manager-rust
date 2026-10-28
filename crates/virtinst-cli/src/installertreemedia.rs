//! `InstallerTreeMedia` from upstream's
//! `virtinst/install/installertreemedia.py`: `--location` trees and
//! kernel/initrd installs.

use virtinst_core::connection::Connection;
use virtinst_core::install::installerinject::perform_initrd_injections;
use virtinst_core::install::unattended::OSInstallScript;
use virtinst_core::install::urldetect::{self, GuestFacts};
use virtinst_core::install::urlfetcher::{self, Fetcher};
use virtinst_core::osinfo::{OsDb, OsMedia, OsTree};
use virtinst_core::progress::Meter;

/// The kind of install media.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MediaType {
    Dir,
    Iso,
    Url,
    Kernel,
}

fn is_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://") || url.starts_with("ftp://")
}

/// `_LocationData`.
struct LocationData {
    osinfo: Option<String>,
    kernel_pairs: Vec<(String, String)>,
    os_media: Option<OsMedia>,
    os_tree: Option<OsTree>,
}

/// The guest facts the tree media reads.
pub struct GuestInfo {
    pub arch: String,
    pub os_type: String,
    pub os_name: String,
}

fn in_testsuite() -> bool {
    std::env::var_os("VIRTINST_TEST_SUITE").is_some()
}

/// `get_system_scratchdir`: the tmpdir that's accessible by VMs on
/// system libvirt URIs.
pub fn get_system_scratchdir(conn: &Connection) -> String {
    if conn.uri().is_xen() {
        return "/var/lib/xen".into();
    }
    "/var/lib/libvirt/boot".into()
}

/// `is_unprivileged`.
pub fn is_unprivileged(conn: &Connection) -> bool {
    match conn.uri().path.as_str() {
        "/session" => true,
        // SAFETY: getuid has no preconditions.
        "/embed" => (unsafe { libc::getuid() }) != 0,
        _ => false,
    }
}

fn writable(path: &str) -> bool {
    let Ok(c) = std::ffi::CString::new(path) else {
        return false;
    };
    // SAFETY: `c` is a valid NUL-terminated string.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

/// `make_scratchdir`: the directory that's accessible by VMs for this
/// URI, created if necessary.
pub fn make_scratchdir(conn: &Connection) -> Result<String, String> {
    let user_scratchdir = format!("{}/boot", crate::cli::app_cache_dir());
    let system_scratchdir = get_system_scratchdir(conn);

    // If we are a session URI, or we don't have access to the system
    // scratchdir, make sure the session scratchdir exists and use that.
    if is_unprivileged(conn)
        || !std::path::Path::new(&system_scratchdir).exists()
        || !writable(&system_scratchdir)
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o751)
            .create(&user_scratchdir)
            .map_err(|e| virtinst_core::diskbackend::oserror_str(&e, Some(&user_scratchdir)))?;
        return Ok(user_scratchdir);
    }
    Ok(system_scratchdir)
}

/// `InstallerTreeMedia`.
pub struct TreeMedia {
    pub location: Option<String>,
    location_kernel: Option<String>,
    location_initrd: Option<String>,
    install_kernel: Option<String>,
    install_initrd: Option<String>,
    install_kernel_args: Option<String>,
    initrd_injections: Vec<(String, String)>,
    extra_args: Vec<String>,
    media_type: MediaType,
    cached_fetcher: Option<Fetcher>,
    cached_data: Option<LocationData>,
    tmpfiles: Vec<String>,
}

impl TreeMedia {
    pub fn new(
        conn: &Connection,
        location: Option<String>,
        location_kernel: Option<String>,
        location_initrd: Option<String>,
        install_kernel: Option<String>,
        install_initrd: Option<String>,
        install_kernel_args: Option<String>,
    ) -> Result<TreeMedia, String> {
        if location_kernel.is_some() || location_initrd.is_some() {
            if location.is_none() {
                return Err(
                    "location kernel/initrd may only be specified with a location URL/path".into(),
                );
            }
            if !(location_kernel.is_some() && location_initrd.is_some()) {
                return Err("location kernel/initrd must be specified as a pair".into());
            }
        }

        let remote = conn.uri().is_remote();
        let mut location = location;
        let loc = location.clone().unwrap_or_default();
        let media_type = if install_kernel.is_some() || install_initrd.is_some() {
            MediaType::Kernel
        } else if !remote && std::path::Path::new(&loc).is_dir() {
            location = Some(virtinst_core::storage::abspath(&loc));
            MediaType::Dir
        } else if is_url(&loc) {
            MediaType::Url
        } else {
            MediaType::Iso
        };

        if remote && media_type != MediaType::Url && media_type != MediaType::Kernel {
            return Err(format!(
                "Cannot access install tree on remote connection: {loc}"
            ));
        }

        if media_type == MediaType::Iso {
            crate::installer::validate_path(conn, &loc)?;
        }

        Ok(TreeMedia {
            location,
            location_kernel,
            location_initrd,
            install_kernel,
            install_initrd,
            install_kernel_args,
            initrd_injections: vec![],
            extra_args: vec![],
            media_type,
            cached_fetcher: None,
            cached_data: None,
            tmpfiles: vec![],
        })
    }

    /// `_get_fetcher`.
    fn fetcher(&mut self, conn: &Connection) -> Result<&mut Fetcher, String> {
        if self.cached_fetcher.is_none() {
            let scratchdir = make_scratchdir(conn)?;
            self.cached_fetcher = Some(if self.media_type == MediaType::Kernel {
                urlfetcher::direct_fetcher(&scratchdir)
            } else {
                urlfetcher::fetcher_for_uri(
                    self.location.as_deref().unwrap_or_default(),
                    &scratchdir,
                    false,
                )?
            });
        }
        Ok(self.cached_fetcher.as_mut().expect("just set"))
    }

    /// `_get_cached_data`.
    fn cached_data(
        &mut self,
        conn: &Connection,
        guest: &GuestInfo,
    ) -> Result<&LocationData, String> {
        if self.cached_data.is_none() {
            let has_location_kernel =
                self.location_kernel.is_some() && self.location_initrd.is_some();
            let mut data = LocationData {
                osinfo: None,
                kernel_pairs: vec![],
                os_media: None,
                os_tree: None,
            };
            if self.media_type == MediaType::Kernel {
                data.kernel_pairs = vec![(
                    self.install_kernel.clone().unwrap_or_default(),
                    self.install_initrd.clone().unwrap_or_default(),
                )];
            } else {
                let facts = GuestFacts {
                    arch: &guest.arch,
                    os_type: &guest.os_type,
                    os_name: &guest.os_name,
                };
                let fetcher = self.fetcher(conn)?;
                if let Some(store) = urldetect::get_distro_store(
                    &facts,
                    fetcher,
                    has_location_kernel,
                    in_testsuite(),
                )? {
                    data.kernel_pairs = store.kernel_paths;
                    data.osinfo = store.os_variant;
                    data.os_media = store.os_media;
                    data.os_tree = store.os_tree;
                }
            }
            if has_location_kernel {
                data.kernel_pairs = vec![(
                    self.location_kernel.clone().unwrap_or_default(),
                    self.location_initrd.clone().unwrap_or_default(),
                )];
            }
            self.cached_data = Some(data);
        }
        Ok(self.cached_data.as_ref().expect("just set"))
    }

    /// `_prepare_kernel_url`.
    fn prepare_kernel_url(
        &mut self,
        conn: &Connection,
        guest: &GuestInfo,
        meter: &mut dyn Meter,
    ) -> Result<(String, String), String> {
        let pairs = self.cached_data(conn, guest)?.kernel_pairs.clone();
        let fetcher = self.fetcher(conn)?;
        let (kernelpath, initrdpath) = pairs
            .into_iter()
            .find(|(k, i)| fetcher.has_file(k) && fetcher.has_file(i))
            .ok_or("Couldn't find kernel for install tree.")?;
        let fetcher = self.fetcher(conn)?;
        let kernel = fetcher.acquire_file(&kernelpath, None, meter)?;
        self.tmpfiles.push(kernel.clone());
        let fetcher = self.fetcher(conn)?;
        let initrd = fetcher.acquire_file(&initrdpath, None, meter)?;
        self.tmpfiles.push(initrd.clone());

        let scratchdir = self.fetcher(conn)?.scratchdir.clone();
        perform_initrd_injections(&initrd, &self.initrd_injections, &scratchdir)?;

        Ok((kernel, initrd))
    }

    /// `_prepare_kernel_url_arg`.
    fn prepare_kernel_url_arg(&self, guest: &GuestInfo) -> Option<String> {
        let osinfo = self
            .cached_data
            .as_ref()
            .and_then(|d| d.osinfo.clone())
            .unwrap_or_else(|| guest.os_name.clone());
        let db = OsDb::load_default().ok()?;
        db.lookup_os(&osinfo)?.get_kernel_url_arg()
    }

    /// `_prepare_unattended_data`.
    fn prepare_unattended_data(&mut self, scripts: &[OSInstallScript]) -> Result<(), String> {
        for script in scripts {
            let expected_filename = script.get_expected_filename();
            let scriptpath = script.write()?;
            self.tmpfiles.push(scriptpath.clone());
            self.initrd_injections.push((scriptpath, expected_filename));
        }
        Ok(())
    }

    /// `_prepare_kernel_args`.
    fn prepare_kernel_args(
        &mut self,
        guest: &GuestInfo,
        unattended_scripts: Option<&[OSInstallScript]>,
    ) -> String {
        let mut install_args = None;
        if let Some(scripts) = unattended_scripts {
            let args: Vec<String> = scripts
                .iter()
                .filter_map(|s| s.generate_cmdline())
                .filter(|c| !c.is_empty())
                .collect();
            let a = args.join(" ");
            log::debug!("Generated unattended cmdline: {a}");
            install_args = Some(a);
        } else if let Some(location) = self.is_network_url().map(str::to_string) {
            if let Some(kernel_url_arg) = self.prepare_kernel_url_arg(guest) {
                install_args = Some(format!("{kernel_url_arg}={location}"));
            }
        }
        if let Some(a) = install_args.filter(|a| !a.is_empty()) {
            self.extra_args.push(a);
        }

        let ret = match &self.install_kernel_args {
            Some(a) if !a.is_empty() => a.clone(),
            _ => self.extra_args.join(" "),
        };
        if self.media_type == MediaType::Dir && ret.is_empty() {
            log::warn!(
                "Directory tree installs typically do not work unless extra kernel args are \
                 passed to point the installer at a network accessible install tree."
            );
        }
        ret
    }

    /// `prepare`: fetch the kernel and initrd, returning them with the
    /// kernel args.
    pub fn prepare(
        &mut self,
        conn: &Connection,
        guest: &GuestInfo,
        meter: &mut dyn Meter,
        unattended_scripts: Option<&[OSInstallScript]>,
    ) -> Result<(String, String, String), String> {
        self.cached_data(conn, guest)?;
        self.prepare_unattended_data(unattended_scripts.unwrap_or_default())?;
        let kernel_args = self.prepare_kernel_args(guest, unattended_scripts);
        let (kernel, initrd) = self.prepare_kernel_url(conn, guest, meter)?;
        Ok((kernel, initrd, kernel_args))
    }

    /// `cleanup`.
    pub fn cleanup(&mut self) {
        for f in self.tmpfiles.drain(..) {
            log::debug!("Removing {f}");
            let _ = std::fs::remove_file(&f);
        }
    }

    pub fn set_initrd_injections(&mut self, injections: Vec<String>) {
        self.initrd_injections = injections
            .iter()
            .map(|f| virtinst_core::install::installerinject::injection_pair(f))
            .collect();
    }

    /// `get_os_media`.
    pub fn get_os_media(
        &mut self,
        conn: &Connection,
        guest: &GuestInfo,
    ) -> Result<Option<OsMedia>, String> {
        Ok(self.cached_data(conn, guest)?.os_media.clone())
    }

    /// `get_os_tree`.
    pub fn get_os_tree(
        &mut self,
        conn: &Connection,
        guest: &GuestInfo,
    ) -> Result<Option<OsTree>, String> {
        Ok(self.cached_data(conn, guest)?.os_tree.clone())
    }

    pub fn set_extra_args(&mut self, extra_args: Vec<String>) {
        self.extra_args = extra_args;
    }

    /// `cdrom_path`.
    pub fn cdrom_path(&self) -> Option<&str> {
        match self.media_type {
            MediaType::Iso => self.location.as_deref(),
            _ => None,
        }
    }

    /// `is_network_url`.
    pub fn is_network_url(&self) -> Option<&str> {
        match self.media_type {
            MediaType::Url => self.location.as_deref(),
            _ => None,
        }
    }

    /// `detect_distro`.
    pub fn detect_distro(
        &mut self,
        conn: &Connection,
        guest: &GuestInfo,
    ) -> Result<Option<String>, String> {
        Ok(self.cached_data(conn, guest)?.osinfo.clone())
    }

    /// `requires_internet`.
    pub fn requires_internet(
        &mut self,
        conn: &Connection,
        guest: &GuestInfo,
    ) -> Result<bool, String> {
        if matches!(self.media_type, MediaType::Url | MediaType::Dir) {
            return Ok(true);
        }
        Ok(self
            .cached_data(conn, guest)?
            .os_media
            .as_ref()
            .is_some_and(|m| m.is_netinst()))
    }
}
