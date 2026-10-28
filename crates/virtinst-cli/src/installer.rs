//! `Installer` from upstream's `virtinst/install/installer.py`, on the
//! parser engine's guest object: the install media/bootdev choice, the
//! install-time XML changes, and creating the domain.

use std::rc::Rc;

use virtinst_core::connection::Connection;
use virtinst_core::devices::disk as dd;
use virtinst_core::diskbackend::StorageBackend;
use virtinst_core::guest::{DOMAIN_DEVICES, GUEST};
use virtinst_core::osinfo::OsDb;
use virtinst_core::xmlbuilder::{Value, XmlObject};

use crate::guest::{self, GuestOpts};
use crate::parsers;
use virtinst_core::install::cloudinit::CloudInitData;
use virtinst_core::install::installertreemedia::{self as treemedia, GuestInfo, TreeMedia};
use virtinst_core::install::unattended::{self, OSInstallScript, ScriptGuest, UnattendedData};

/// `_make_testsuite_path`.
fn make_testsuite_path(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    let tail = base.splitn(3, '-').last().unwrap_or(base);
    format!("/VIRTINST-TESTSUITE/{tail}")
}

/// What the tree media reads from the guest.
fn guest_info(guest: &XmlObject) -> GuestInfo {
    GuestInfo {
        arch: guest.child("os").get_str("arch").unwrap_or_default(),
        os_type: guest.child("os").get_str("os_type").unwrap_or_default(),
        os_name: guest::osinfo_name(guest),
    }
}

/// `conn.in_testsuite()`.
fn in_testsuite() -> bool {
    std::env::var_os("VIRTINST_TEST_SUITE").is_some()
}

/// `DeviceDisk.device` of a `<disk>`.
fn disk_device(disk: &XmlObject) -> String {
    dd::disk_device(disk)
}

/// A new `<disk device="cdrom">` for `path` (`_make_cdrom_device`), and
/// the storage behind it.
fn make_cdrom_device(
    conn: &Connection,
    path: Option<&str>,
) -> Result<(XmlObject, Rc<StorageBackend>), String> {
    let mut dev = XmlObject::new(&dd::DEVICE_DISK);
    dev.set("_device", Some(Value::Str("cdrom".into())))?;
    let remote = conn.uri().is_remote();
    let backend = dd::set_source_path_obj(&mut dev, path, conn.as_connect(), remote)?;
    let support = crate::cli::conn_support(conn);
    dd::sync_path_props(
        &mut dev,
        &backend,
        support.disk_driver_name_qemu,
        support.is_xen,
    )?;
    parsers::disk::set_backend_rc(Some(Rc::new(backend)));
    parsers::disk::validate(&dev)?;
    Ok((dev, parsers::disk::backend().expect("just set")))
}

/// `InstallerTreeMedia.validate_path`: the install media as a CDROM
/// source path.
pub fn validate_path(conn: &Connection, path: &str) -> Result<String, String> {
    match make_cdrom_device(conn, Some(path)) {
        Ok((_, backend)) => Ok(backend.get_path().unwrap_or_default()),
        Err(e) => {
            log::debug!("Error validating install location");
            if path.starts_with("nfs:") {
                log::warn!(
                    "NFS URL installs are no longer supported. Access your install media over an alternate transport like HTTP, or manually mount the NFS share and install from the local directory mount point."
                );
            }
            Err(format!("Validating install media '{path}' failed: {e}"))
        }
    }
}

/// What `start_install` made.
pub enum Started {
    /// `return_xml`/`dry`: the (initial, final) XML.
    Xml(Option<String>, String),
    /// The created domain.
    Domain(virt::domain::Domain),
}

/// `Installer`.
pub struct Installer {
    cdrom: Option<String>,
    install_bootdev: Option<String>,
    no_install: bool,
    is_reinstall: bool,
    pre_reinstall_xml: Option<String>,
    treemedia: Option<TreeMedia>,
    treemedia_bootconfig: Option<(String, String, String)>,
    install_cdrom_device_added: bool,
    defaults_are_set: bool,
    tmpvols: Vec<virt::storage_vol::StorageVol>,
    tmpfiles: Vec<String>,
    cloudinit_data: Option<CloudInitData>,
    unattended_data: Option<UnattendedData>,
    unattended_install_cdrom_device: Option<String>,
    have_default_tpm: bool,
    /// Entry point for virt-manager 'Customize' wizard to change autostart
    pub autostart: bool,
}

/// What `Installer::new` takes: upstream's keyword arguments.
#[derive(Default)]
pub struct InstallerArgs {
    pub cdrom: Option<String>,
    pub location: Option<String>,
    pub install_bootdev: Option<String>,
    pub location_kernel: Option<String>,
    pub location_initrd: Option<String>,
    pub install_kernel: Option<String>,
    pub install_initrd: Option<String>,
    pub install_kernel_args: Option<String>,
    pub no_install: bool,
    pub is_reinstall: bool,
}

impl Installer {
    /// An installer with no install media yet.
    pub fn empty() -> Installer {
        Installer {
            cdrom: None,
            install_bootdev: None,
            no_install: false,
            is_reinstall: false,
            pre_reinstall_xml: None,
            treemedia: None,
            treemedia_bootconfig: None,
            install_cdrom_device_added: false,
            defaults_are_set: false,
            tmpvols: vec![],
            tmpfiles: vec![],
            cloudinit_data: None,
            unattended_data: None,
            unattended_install_cdrom_device: None,
            have_default_tpm: false,
            autostart: false,
        }
    }

    pub fn new(conn: &Connection, a: InstallerArgs) -> Result<Installer, String> {
        let mut inst = Installer {
            install_bootdev: a.install_bootdev,
            no_install: a.no_install,
            is_reinstall: a.is_reinstall,
            ..Installer::empty()
        };
        if let Some(cdrom) = a.cdrom {
            inst.cdrom = Some(validate_path(conn, &cdrom)?);
            inst.install_bootdev = Some("cdrom".into());
        } else if a.location.is_some()
            || a.location_kernel.is_some()
            || a.location_initrd.is_some()
            || a.install_kernel.is_some()
            || a.install_initrd.is_some()
        {
            let tm = TreeMedia::new(
                conn.uri(),
                a.location,
                a.location_kernel,
                a.location_initrd,
                a.install_kernel,
                a.install_initrd,
                a.install_kernel_args,
            )?;
            if let Some(iso) = tm.cdrom_path() {
                validate_path(conn, iso)?;
            }
            inst.treemedia = Some(tm);
        }
        Ok(inst)
    }

    /// `_cdrom_path`.
    fn cdrom_path(&self) -> Option<&str> {
        match &self.treemedia {
            Some(t) => t.cdrom_path(),
            None => self.cdrom.as_deref(),
        }
    }

    /// `_add_install_cdrom_device`.
    fn add_install_cdrom_device(
        &mut self,
        guest: &mut XmlObject,
        opts: &mut GuestOpts,
        conn: &Connection,
    ) -> Result<(), String> {
        if self.install_cdrom_device_added {
            return Ok(());
        }
        let Some(path) = self.cdrom_path().map(str::to_string) else {
            return Ok(());
        };
        self.install_cdrom_device_added = true;

        if self.is_reinstall {
            let remote = conn.uri().is_remote();
            if let Some(dev) = guest
                .child_mut("devices")
                .children_mut("disk")
                .iter_mut()
                .find(|d| disk_device(d) == "cdrom")
            {
                dd::set_source_path_obj(dev, Some(&path), conn.as_connect(), remote)?;
                return Ok(());
            }
        }

        let (dev, backend) = make_cdrom_device(conn, Some(&path))?;

        // Insert the CDROM before any other CDROM, so boot=cdrom picks
        // it as the priority
        let disks = guest.child("devices").children("disk");
        let idx = disks
            .iter()
            .position(|d| disk_device(d) == "cdrom")
            .unwrap_or(disks.len());
        guest.child_mut("devices").add_child_at(dev, idx);
        if self.is_reinstall {
            parsers::disk::set_backend_rc(Some(backend.clone()));
            guest::device_set_defaults(guest, "devices.disk", idx, conn)?;
        }
        while opts.disk_backends.len() < idx {
            opts.disk_backends.push(None);
        }
        opts.disk_backends.insert(idx, Some(backend));
        Ok(())
    }

    /// `_remove_install_cdrom_media`.
    fn remove_install_cdrom_media(&self, guest: &mut XmlObject, conn: &Connection) {
        if !self.install_cdrom_device_added {
            return;
        }
        if guest::osinfo_is_windows(guest) {
            // Keep media attached for windows which has a multi stage install
            return;
        }
        let remote = conn.uri().is_remote();
        let cdrom = self.cdrom_path().map(str::to_string);
        for disk in guest.child_mut("devices").children_mut("disk") {
            if disk_device(disk) == "cdrom"
                && dd::resolve_backend(disk, conn.as_connect(), remote).get_path() == cdrom
            {
                if let Ok(backend) = dd::set_source_path_obj(disk, None, conn.as_connect(), remote)
                {
                    let support = crate::cli::conn_support(conn);
                    let _ = dd::sync_path_props(
                        disk,
                        &backend,
                        support.disk_driver_name_qemu,
                        support.is_xen,
                    );
                }
                break;
            }
        }
    }

    /// `_build_boot_order`.
    fn build_boot_order(guest: &XmlObject, bootdev: &str) -> Vec<String> {
        let mut bootorder = vec![bootdev.to_string()];
        // If guest has an attached disk, always have 'hd' in the boot
        // list, so disks are marked as bootable/installable (needed for
        // windows virtio installs, and booting local disk from PXE)
        if guest
            .child("devices")
            .children("disk")
            .iter()
            .any(|d| disk_device(d) == "disk")
            && !bootorder.iter().any(|b| b == "hd")
        {
            bootorder.push("hd".into());
        }
        bootorder
    }

    /// `_can_set_guest_bootorder`.
    fn can_set_guest_bootorder(guest: &XmlObject) -> bool {
        let os = guest.child("os");
        let any_boot_order = DOMAIN_DEVICES.prop_order.iter().any(|name| {
            guest
                .child("devices")
                .children(name)
                .iter()
                .any(|d| d.get_path("boot.order").is_some())
        });
        os.get_str("os_type").as_deref() != Some("exe")
            && os.get_str("kernel").map_or(true, |k| k.is_empty())
            && !any_boot_order
    }

    fn bootorder(guest: &XmlObject) -> Vec<String> {
        guest
            .child("os")
            .children("bootdevs")
            .iter()
            .filter_map(|d| d.get_str("dev"))
            .collect()
    }

    fn set_bootorder(guest: &mut XmlObject, order: &[String]) -> Result<(), String> {
        virtinst_core::domain::os::set_bootorder(guest.child_mut("os"), order)
    }

    /// `_alter_treemedia_bootconfig`.
    fn alter_treemedia_bootconfig(&self, guest: &mut XmlObject) -> Result<(), String> {
        if self.treemedia.is_none() {
            return Ok(());
        }
        let Some((kernel, initrd, kernel_args)) = &self.treemedia_bootconfig else {
            return Ok(());
        };
        let testpath = |p: &str| {
            if in_testsuite() {
                make_testsuite_path(p)
            } else {
                p.to_string()
            }
        };
        let os = guest.child_mut("os");
        if !kernel.is_empty() {
            os.set("kernel", Some(Value::Str(testpath(kernel))))?;
        }
        if !initrd.is_empty() {
            os.set("initrd", Some(Value::Str(testpath(initrd))))?;
        }
        if !kernel_args.is_empty() {
            os.set("kernel_args", Some(Value::Str(kernel_args.clone())))?;
        }
        Ok(())
    }

    /// `_alter_bootconfig`.
    fn alter_bootconfig(&self, guest: &mut XmlObject) -> Result<(), String> {
        guest.set("on_reboot", Some(Value::Str("destroy".into())))?;
        self.alter_treemedia_bootconfig(guest)?;
        match &self.install_bootdev {
            Some(bootdev) if Self::can_set_guest_bootorder(guest) => {
                let order = Self::build_boot_order(guest, bootdev);
                Self::set_bootorder(guest, &order)
            }
            _ => Self::set_bootorder(guest, &[]),
        }
    }

    /// `_get_postinstall_bootdev`.
    fn get_postinstall_bootdev(&self, guest: &XmlObject) -> String {
        if self.cdrom.is_some() && self.no_install {
            return "cdrom".into();
        }
        let disks = guest.child("devices").children("disk");
        if let Some(bootdev) = &self.install_bootdev {
            if disks.iter().any(|d| disk_device(d) == "disk") {
                return "hd".into();
            }
            return bootdev.clone();
        }
        match disks.first().map(disk_device).as_deref() {
            Some("cdrom") => "cdrom".into(),
            Some("floppy") => "fd".into(),
            _ => "hd".into(),
        }
    }

    pub fn cdrom(&self) -> Option<&str> {
        self.cdrom.as_deref()
    }

    /// `location`.
    pub fn location(&self) -> Option<&str> {
        self.treemedia.as_ref().and_then(|t| t.location.as_deref())
    }

    /// `_upload_media`.
    fn upload_media(
        &mut self,
        conn: &Connection,
        meter: &mut dyn virtinst_core::progress::Meter,
        paths: Vec<String>,
    ) -> Result<Vec<String>, String> {
        let (newpaths, tmpvols) =
            treemedia::upload_media(conn.as_connect(), conn.uri(), meter, paths)?;
        self.tmpvols.extend(tmpvols);
        Ok(newpaths)
    }

    /// `_prepare_unattended_data`.
    fn prepare_unattended_data(
        &mut self,
        guest: &mut XmlObject,
        conn: &Connection,
        meter: &mut dyn virtinst_core::progress::Meter,
        scripts: &[OSInstallScript],
    ) -> Result<(), String> {
        let scratchdir = treemedia::make_scratchdir(conn.uri())?;
        let mut injections = vec![];
        for script in scripts {
            let expected_filename = script.get_expected_filename();
            let cmdline = script.generate_cmdline();
            log::debug!(
                "Generated unattended cmdline: {}",
                cmdline.as_deref().unwrap_or("None")
            );
            let scriptpath = script.write()?;
            self.tmpfiles.push(scriptpath.clone());
            injections.push((scriptpath, expected_filename));
        }

        let arch = guest.child("os").get_str("arch").unwrap_or_default();
        let locations = guest::osinfo_obj(guest)
            .map(|o| o.get_pre_installable_drivers_location(&arch))
            .unwrap_or_default();
        let drivers = unattended::download_drivers(&locations, &scratchdir, meter)?;
        self.tmpfiles.extend(drivers.iter().map(|(f, _)| f.clone()));
        injections.extend(drivers);

        let iso = virtinst_core::install::installerinject::perform_cdrom_injections(
            &injections,
            &scratchdir,
            false,
        )?;
        self.tmpfiles.push(iso.clone());
        let iso = self.upload_media(conn, meter, vec![iso])?.remove(0);
        self.add_unattended_install_cdrom_device(guest, conn, &iso)
    }

    /// `_prepare_unattended_scripts`.
    fn prepare_unattended_scripts(
        &mut self,
        guest: &XmlObject,
        conn: &Connection,
    ) -> Result<Vec<OSInstallScript>, String> {
        let osinfo = guest::osinfo_obj(guest)
            .ok_or("OS 'generic' does not support unattended installation.")?;
        let mut url = None;
        let mut os_tree = None;
        let os_media;
        let injection_method;
        if let Some(tm) = &mut self.treemedia {
            url = tm.is_network_url().map(str::to_string);
            let info = guest_info(guest);
            os_media = tm.get_os_media(conn.uri(), &info)?;
            os_tree = tm.get_os_tree(conn.uri(), &info)?;
            injection_method = "initrd";
        } else {
            if !osinfo.is_windows() {
                log::warn!(
                    "Attempting unattended method=cdrom injection for a non-windows OS. If \
                     this doesn't work, try passing install media to --location"
                );
            }
            os_media = self.cdrom.as_deref().and_then(|c| {
                OsDb::load_default()
                    .ok()
                    .and_then(|db| db.guess_media_by_iso(c))
                    .map(|(_, m)| m)
            });
            injection_method = "cdrom";
        }

        let arch = guest.child("os").get_str("arch").unwrap_or_default();
        let name = guest.get_str("name").unwrap_or_default();
        let sg = ScriptGuest {
            osinfo: &osinfo,
            arch: &arch,
            name: &name,
        };
        unattended::prepare_install_scripts(
            &sg,
            self.unattended_data
                .as_ref()
                .expect("checked by the caller"),
            url.as_deref(),
            os_media.as_ref(),
            os_tree.as_ref(),
            injection_method,
        )
    }

    /// `_prepare_treemedia`.
    fn prepare_treemedia(
        &mut self,
        guest: &XmlObject,
        conn: &Connection,
        meter: &mut dyn virtinst_core::progress::Meter,
        unattended_scripts: Option<&[OSInstallScript]>,
    ) -> Result<(), String> {
        let tm = self.treemedia.as_mut().expect("checked by the caller");
        let (kernel, initrd, kernel_args) =
            tm.prepare(conn.uri(), &guest_info(guest), meter, unattended_scripts)?;
        let paths = self.upload_media(conn, meter, vec![kernel, initrd])?;
        self.treemedia_bootconfig = Some((paths[0].clone(), paths[1].clone(), kernel_args));
        Ok(())
    }

    /// `_prepare`.
    fn prepare(
        &mut self,
        guest: &mut XmlObject,
        conn: &Connection,
        meter: &mut dyn virtinst_core::progress::Meter,
    ) -> Result<(), String> {
        if self.is_reinstall {
            self.pre_reinstall_xml = Some(guest.get_xml());
        }
        let mut unattended_scripts = None;
        if self.unattended_data.is_some() {
            unattended_scripts = Some(self.prepare_unattended_scripts(guest, conn)?);
        }

        if self.treemedia.is_some() {
            self.prepare_treemedia(guest, conn, meter, unattended_scripts.as_deref())?;
        } else if let Some(scripts) = &unattended_scripts {
            self.prepare_unattended_data(guest, conn, meter, scripts)?;
        }

        if self.cloudinit_data.is_some() {
            self.prepare_cloudinit(guest, conn, meter)?;
        }
        Ok(())
    }

    /// `_prepare_cloudinit`.
    fn prepare_cloudinit(
        &mut self,
        guest: &mut XmlObject,
        conn: &Connection,
        meter: &mut dyn virtinst_core::progress::Meter,
    ) -> Result<(), String> {
        let scratchdir = treemedia::make_scratchdir(conn.uri())?;
        let data = self.cloudinit_data.as_mut().expect("checked by the caller");
        let filepairs = data.create_files(&scratchdir)?;
        for (f, _) in &filepairs {
            self.tmpfiles.push(f.clone());
        }
        let iso = virtinst_core::install::installerinject::perform_cdrom_injections(
            &filepairs,
            &scratchdir,
            true,
        )?;
        self.tmpfiles.push(iso.clone());
        let iso = self.upload_media(conn, meter, vec![iso])?.remove(0);
        self.add_unattended_install_cdrom_device(guest, conn, &iso)
    }

    /// `_add_unattended_install_cdrom_device`.
    fn add_unattended_install_cdrom_device(
        &mut self,
        guest: &mut XmlObject,
        conn: &Connection,
        location: &str,
    ) -> Result<(), String> {
        if self.unattended_install_cdrom_device.is_some() {
            return Ok(());
        }
        let (dev, backend) = make_cdrom_device(conn, Some(location))?;
        guest.child_mut("devices").add_child(dev);
        let idx = guest.child("devices").children("disk").len() - 1;
        parsers::disk::set_backend_rc(Some(backend));
        guest::device_set_defaults(guest, "devices.disk", idx, conn)?;
        self.unattended_install_cdrom_device =
            guest.child("devices").children("disk")[idx].get_str("target");

        // The CDROM we just added might be a SCSI one. If that's the case,
        // libvirt will automatically add the necessary controller for us, but
        // depending on the architecture its choice of model might not be
        // usable by the guest OS. Try to add a virtio-scsi controller, if
        // supported, for the best chance of the CDROM actually being picked up
        guest::ensure_virtioscsi_controller(guest)?;

        if in_testsuite() {
            // Hack to set just the XML path differently for the test suite.
            // Setting this via regular 'path' will error that it doesn't exist
            guest.child_mut("devices").children_mut("disk")[idx]
                .child_mut("source")
                .set("file", Some(Value::Str(make_testsuite_path(location))))?;
        }
        Ok(())
    }

    /// `_remove_unattended_install_cdrom_device`.
    fn remove_unattended_install_cdrom_device(&self, guest: &mut XmlObject, conn: &Connection) {
        let Some(target) = &self.unattended_install_cdrom_device else {
            return;
        };
        let remote = conn.uri().is_remote();
        if let Some(disk) = guest
            .child_mut("devices")
            .children_mut("disk")
            .iter_mut()
            .find(|d| d.get_str("target").as_deref() == Some(target.as_str()))
        {
            if let Ok(backend) = dd::set_source_path_obj(disk, None, conn.as_connect(), remote) {
                let support = crate::cli::conn_support(conn);
                let _ = dd::sync_path_props(
                    disk,
                    &backend,
                    support.disk_driver_name_qemu,
                    support.is_xen,
                );
            }
        }
    }

    /// `set_cloudinit_data`.
    pub fn set_cloudinit_data(&mut self, data: CloudInitData) {
        self.cloudinit_data = Some(data);
    }

    /// `has_cloudinit`.
    pub fn has_cloudinit(&self) -> bool {
        self.cloudinit_data.is_some()
    }

    /// `set_unattended_data`.
    pub fn set_unattended_data(&mut self, data: UnattendedData) {
        self.unattended_data = Some(data);
    }

    /// `has_unattended`.
    pub fn has_unattended(&self) -> bool {
        self.unattended_data.is_some()
    }

    /// `get_generated_password`.
    pub fn get_generated_password(&mut self) -> Option<String> {
        self.cloudinit_data
            .as_mut()
            .and_then(|d| d.get_password_if_generated())
    }

    /// `_cleanup`.
    fn cleanup(&mut self) {
        if let Some(tm) = &mut self.treemedia {
            tm.cleanup();
        }
        for vol in self.tmpvols.drain(..) {
            log::debug!("Removing volume '{}'", vol.get_name().unwrap_or_default());
            let _ = vol.delete(0);
        }
        for f in self.tmpfiles.drain(..) {
            log::debug!("Removing {f}");
            let _ = std::fs::remove_file(&f);
        }
    }

    /// `set_initrd_injections`.
    pub fn set_initrd_injections(&mut self, injections: Vec<String>) -> Result<(), String> {
        let Some(tm) = &mut self.treemedia else {
            return Err("Install method does not support initrd injections.".into());
        };
        tm.set_initrd_injections(injections);
        Ok(())
    }

    /// `set_extra_args`.
    pub fn set_extra_args(&mut self, extra_args: Vec<String>) -> Result<(), String> {
        let Some(tm) = &mut self.treemedia else {
            return Err(
                "Kernel arguments are only supported with location or kernel installs.".into(),
            );
        };
        tm.set_extra_args(extra_args);
        Ok(())
    }

    /// `set_install_defaults`.
    pub fn set_install_defaults(
        &mut self,
        guest: &mut XmlObject,
        opts: &mut GuestOpts,
        conn: &Connection,
    ) -> Result<(), String> {
        if self.defaults_are_set {
            return Ok(());
        }
        if self.is_reinstall {
            self.pre_reinstall_xml = Some(guest.get_xml());
        }
        self.add_install_cdrom_device(guest, opts, conn)?;

        if Self::bootorder(guest).is_empty() && Self::can_set_guest_bootorder(guest) {
            let bootdev = self.get_postinstall_bootdev(guest);
            let order = Self::build_boot_order(guest, &bootdev);
            Self::set_bootorder(guest, &order)?;
        }

        if !self.is_reinstall {
            guest::set_defaults(guest, opts, conn)?;
        }
        self.defaults_are_set = true;
        Ok(())
    }

    /// `get_search_paths`: the paths the hypervisor will need search
    /// access for to perform this install.
    pub fn get_search_paths(&self, conn: &Connection) -> Result<Vec<String>, String> {
        let mut search_paths = vec![];
        if (self.treemedia.is_some()
            || self.cloudinit_data.is_some()
            || self.unattended_data.is_some())
            && !treemedia::should_upload_media(conn.uri())?
        {
            search_paths.push(treemedia::make_scratchdir(conn.uri())?);
        }
        if let Some(p) = self.cdrom_path() {
            search_paths.push(p.to_string());
        }
        Ok(search_paths)
    }

    /// `requires_postboot_xml_changes`: whether the install has 2 XML
    /// phases, and requires a hard VM poweroff to ensure the second
    /// stage is used for subsequent boots.
    pub fn requires_postboot_xml_changes(&self) -> bool {
        if self.has_cloudinit() || self.has_unattended() {
            return true;
        }
        if self.no_install {
            return false;
        }
        self.cdrom.is_some() || self.install_bootdev.is_some() || self.treemedia.is_some()
    }

    /// `options_specified`: whether some explicit install option was
    /// actually passed in.
    pub fn options_specified(&self) -> bool {
        if self.no_install {
            return true;
        }
        self.requires_postboot_xml_changes()
    }

    /// `detect_distro`: the OS short id libosinfo detects from the
    /// install media, if any.
    pub fn detect_distro(
        &mut self,
        guest: &XmlObject,
        conn: &Connection,
    ) -> Result<Option<String>, String> {
        let mut ret = None;
        if let Some(tm) = &mut self.treemedia {
            ret = tm.detect_distro(conn.uri(), &guest_info(guest))?;
        } else if let Some(cdrom) = &self.cdrom {
            if conn.uri().is_remote() {
                log::debug!("Can't detect distro for cdrom remote connection.");
            } else {
                ret = OsDb::load_default()
                    .ok()
                    .and_then(|db| db.guess_os_by_iso(cdrom))
                    .map(|os| os.short_id());
            }
        } else {
            log::debug!("No media for distro detection.");
        }
        log::debug!(
            "installer.detect_distro returned={}",
            ret.as_deref().unwrap_or("None")
        );
        Ok(ret)
    }

    /// `_build_postboot_xml`.
    fn build_postboot_xml(
        &mut self,
        final_xml: &str,
        conn: &Connection,
    ) -> Result<(String, String), String> {
        let mut initial_guest = XmlObject::parse(&GUEST, final_xml).map_err(|e| e.to_string())?;
        self.alter_bootconfig(&mut initial_guest)?;
        self.alter_install_resources(&mut initial_guest, conn)?;
        if self.has_cloudinit() {
            guest::set_smbios_serial_cloudinit(&mut initial_guest)?;

            // When shim in the guest sees unpopulated EFI NVRAM, like when
            // we create a new UEFI VM, it invokes fallback.efi to populate
            // initial NVRAM boot entries. When the guest also has a TPM device,
            // shim will do a one time VM reset. This reset throws off the
            // reboot detection that is central to virt-install's install
            // process.
            //
            // The main install case that this will usually be relevant is
            // the combo of UEFI and --cloud-init. The latter usually implies
            // use of a distro cloud image, which will be using shim, and the
            // --cloud-init process requires a multi stage install compared
            // to just a plain import install.
            //
            // For that case, we disable the default TPM device for the first
            // boot.
            let guest_ro = XmlObject::parse(&GUEST, final_xml).map_err(|e| e.to_string())?;
            if self.have_default_tpm
                && guest::guest_is_uefi(&guest_ro)
                && !initial_guest.child("devices").children("tpm").is_empty()
            {
                log::debug!(
                    "combo of default TPM, UEFI, and cloudinit is used. assuming this VM is \
                     using a linux distro cloud image with shim in the boot path. disabling \
                     TPM for the first boot"
                );
                initial_guest.child_mut("devices").remove_child("tpm", 0);
            }
        }

        let mut final_guest = XmlObject::parse(&GUEST, final_xml).map_err(|e| e.to_string())?;
        self.remove_install_cdrom_media(&mut final_guest, conn);
        self.remove_unattended_install_cdrom_device(&mut final_guest, conn);

        Ok((initial_guest.get_xml(), final_guest.get_xml()))
    }

    /// `_alter_install_resources`: sets the appropriate amount of ram
    /// needed when performing a "network" based installation.
    fn alter_install_resources(
        &mut self,
        guest: &mut XmlObject,
        conn: &Connection,
    ) -> Result<(), String> {
        let Some(tm) = &mut self.treemedia else {
            return Ok(());
        };
        if !tm.requires_internet(conn.uri(), &guest_info(guest))? {
            return Ok(());
        }
        let Some(os) = guest::osinfo_obj(guest) else {
            return Ok(());
        };
        let arch = guest.child("os").get_str("arch").unwrap_or_default();
        let ram = os.get_network_install_required_ram(&arch).unwrap_or(0) / 1024;
        let current = match guest.get("_currentMemory") {
            Some(Value::Int(m)) => m,
            _ => 0,
        };
        if ram > current {
            log::warn!(
                "Overriding memory to {} MiB needed for {} network install.",
                ram / 1024,
                os.short_id()
            );
            virtinst_core::guest::set_current_memory(guest, Some(ram))?;
        }
        Ok(())
    }

    /// `_build_xml`.
    fn build_xml(
        &mut self,
        guest: &XmlObject,
        conn: &Connection,
    ) -> Result<(Option<String>, String), String> {
        let mut initial_xml = None;
        let mut final_xml = guest.get_xml();
        if self.requires_postboot_xml_changes() {
            let (i, f) = self.build_postboot_xml(&final_xml, conn)?;
            initial_xml = Some(i);
            final_xml = f;
        }
        if let Some(x) = &self.pre_reinstall_xml {
            final_xml = x.clone();
        }
        log::debug!(
            "Generated initial_xml: {}",
            initial_xml
                .as_ref()
                .map_or("None required".to_string(), |x| format!("\n{x}"))
        );
        log::debug!("Generated final_xml: \n{final_xml}");
        Ok((initial_xml, final_xml))
    }

    /// `_manual_transient_create`: for hypervisors (like vz) that don't
    /// implement createXML, we need to define+start, and undefine on
    /// start failure
    fn manual_transient_create(
        conn: &Connection,
        initial_xml: Option<&str>,
        final_xml: &str,
        needs_boot: bool,
    ) -> Result<virt::domain::Domain, String> {
        let c = conn.as_connect();
        let domain = virt::domain::Domain::define_xml(c, initial_xml.unwrap_or(final_xml))
            .map_err(|e| e.message().to_string())?;
        if !needs_boot {
            return Ok(domain);
        }
        // Handle undefining the VM if the initial startup fails
        if let Err(e) = domain.create() {
            let _ = domain.undefine();
            return Err(e.message().to_string());
        }
        if let Some(initial) = initial_xml {
            if initial != final_xml {
                return virt::domain::Domain::define_xml(c, final_xml)
                    .map_err(|e| e.message().to_string());
            }
        }
        Ok(domain)
    }

    /// `_create_guest`.
    fn create_guest(
        &self,
        guest: &XmlObject,
        conn: &Connection,
        initial_xml: Option<&str>,
        final_xml: &str,
        doboot: bool,
        transient: bool,
    ) -> Result<virt::domain::Domain, String> {
        use virtinst_core::progress::Meter;
        let mut meter = crate::cli::get_meter();
        meter.start("Creating domain...", None);
        let needs_boot = doboot || initial_xml.is_some();

        let domain = if guest.get_str("type").as_deref() == Some("vz") && !self.is_reinstall {
            if transient {
                return Err("Domain type 'vz' doesn't support transient installs.".into());
            }
            Self::manual_transient_create(conn, initial_xml, final_xml, needs_boot)?
        } else {
            let c = conn.as_connect();
            let mut domain = None;
            if transient || needs_boot {
                domain = Some(
                    virt::domain::Domain::create_xml(c, initial_xml.unwrap_or(final_xml), 0)
                        .map_err(|e| e.message().to_string())?,
                );
            }
            if !transient {
                domain = Some(
                    virt::domain::Domain::define_xml(c, final_xml)
                        .map_err(|e| e.message().to_string())?,
                );
            }
            domain.expect("transient or defined")
        };
        match domain.get_xml_desc(0) {
            Ok(xml) => log::debug!("XML fetched from libvirt object:\n{xml}"),
            Err(e) => log::debug!("Error fetching XML from libvirt object: {}", e.message()),
        }
        meter.end();
        Ok(domain)
    }

    /// `_flag_autostart`.
    fn flag_autostart(domain: &virt::domain::Domain) -> Result<(), String> {
        if let Err(e) = domain.set_autostart(true) {
            if e.code() != virt::error::ErrorNumber::NoSupport {
                return Err(e.message().to_string());
            }
            log::warn!(
                "Could not set autostart flag: libvirt connection does not support autostart."
            );
        }
        Ok(())
    }

    /// `start_install`: add install media to the guest config, launch
    /// it, then redefine the XML with the postinstall config.
    /// `return_xml`: don't create the guest, just return the XML.
    #[allow(clippy::too_many_arguments)]
    pub fn start_install(
        &mut self,
        user_guest: &mut XmlObject,
        opts: &mut GuestOpts,
        conn: &Connection,
        dry: bool,
        return_xml: bool,
        doboot: bool,
        transient: bool,
        build_storage: &dyn Fn() -> Result<(), String>,
    ) -> Result<Started, String> {
        if !self.is_reinstall && !return_xml {
            guest::validate_name(conn, &user_guest.get_str("name").unwrap_or_default())?;
        }
        self.set_install_defaults(user_guest, opts, conn)?;

        // All installer XML alterations are made on this guest instance,
        // so the user_guest instance is left intact
        user_guest.check_xml_actions()?;
        let mut guest =
            XmlObject::parse(&GUEST, &user_guest.get_xml()).map_err(|e| e.to_string())?;
        self.have_default_tpm = opts.have_default_tpm;

        let res = (|| {
            let mut meter = crate::cli::get_meter();
            self.prepare(&mut guest, conn, &mut meter)?;

            if !dry && !self.is_reinstall {
                build_storage()?;
            }

            let (initial_xml, final_xml) = self.build_xml(&guest, conn)?;
            if dry || return_xml {
                return Ok(Started::Xml(initial_xml, final_xml));
            }

            let domain = self.create_guest(
                &guest,
                conn,
                initial_xml.as_deref(),
                &final_xml,
                doboot,
                transient,
            )?;
            if self.autostart {
                Self::flag_autostart(&domain)?;
            }
            Ok(Started::Domain(domain))
        })();
        self.cleanup();
        res
    }
}

#[cfg(test)]
mod tests {
    use super::make_testsuite_path;

    #[test]
    fn testsuite_path_keeps_the_tail_after_two_dashes() {
        assert_eq!(
            make_testsuite_path("/var/lib/libvirt/boot/fake-win7.iso"),
            "/VIRTINST-TESTSUITE/win7.iso"
        );
        assert_eq!(
            make_testsuite_path("/tmp/virtinst-abc-vmlinuz"),
            "/VIRTINST-TESTSUITE/vmlinuz"
        );
        assert_eq!(
            make_testsuite_path("initrd.img"),
            "/VIRTINST-TESTSUITE/initrd.img"
        );
    }
}
