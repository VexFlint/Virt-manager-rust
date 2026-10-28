//! `Installer` from upstream's `virtinst/install/installer.py`, on the
//! parser engine's guest object: the install media/bootdev choice, the
//! install-time XML changes, and creating the domain.
//!
//! `--location`, kernel/initrd installs, `--unattended` and
//! `--cloud-init` (`InstallerTreeMedia`, the scratch dir uploads) are
//! the next step of the port and aren't here yet.

use std::rc::Rc;

use virtinst_core::connection::Connection;
use virtinst_core::devices::disk as dd;
use virtinst_core::diskbackend::StorageBackend;
use virtinst_core::guest::{DOMAIN_DEVICES, GUEST};
use virtinst_core::osinfo::OsDb;
use virtinst_core::xmlbuilder::{Value, XmlObject};

use crate::guest::{self, GuestOpts};
use crate::parsers;

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
fn validate_path(conn: &Connection, path: &str) -> Result<String, String> {
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
    install_cdrom_device_added: bool,
    defaults_are_set: bool,
    /// Entry point for virt-manager 'Customize' wizard to change autostart
    pub autostart: bool,
}

impl Installer {
    /// An installer with no install media yet.
    pub fn empty() -> Installer {
        Installer {
            cdrom: None,
            install_bootdev: None,
            no_install: false,
            install_cdrom_device_added: false,
            defaults_are_set: false,
            autostart: false,
        }
    }

    pub fn new(
        conn: &Connection,
        cdrom: Option<&str>,
        install_bootdev: Option<String>,
        no_install: bool,
    ) -> Result<Installer, String> {
        let mut inst = Installer {
            cdrom: None,
            install_bootdev,
            no_install,
            install_cdrom_device_added: false,
            defaults_are_set: false,
            autostart: false,
        };
        if let Some(cdrom) = cdrom {
            inst.cdrom = Some(validate_path(conn, cdrom)?);
            inst.install_bootdev = Some("cdrom".into());
        }
        Ok(inst)
    }

    fn cdrom_path(&self) -> Option<&str> {
        self.cdrom.as_deref()
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

        let (dev, backend) = make_cdrom_device(conn, Some(&path))?;
        let backend = Some(backend);

        // Insert the CDROM before any other CDROM, so boot=cdrom picks
        // it as the priority
        let disks = guest.child("devices").children("disk");
        let idx = disks
            .iter()
            .position(|d| disk_device(d) == "cdrom")
            .unwrap_or(disks.len());
        guest.child_mut("devices").add_child_at(dev, idx);
        while opts.disk_backends.len() < idx {
            opts.disk_backends.push(None);
        }
        opts.disk_backends.insert(idx, backend);
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

    /// `_alter_bootconfig`.
    fn alter_bootconfig(&self, guest: &mut XmlObject) -> Result<(), String> {
        guest.set("on_reboot", Some(Value::Str("destroy".into())))?;
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
        self.add_install_cdrom_device(guest, opts, conn)?;

        if Self::bootorder(guest).is_empty() && Self::can_set_guest_bootorder(guest) {
            let bootdev = self.get_postinstall_bootdev(guest);
            let order = Self::build_boot_order(guest, &bootdev);
            Self::set_bootorder(guest, &order)?;
        }

        guest::set_defaults(guest, opts, conn)?;
        self.defaults_are_set = true;
        Ok(())
    }

    /// `get_search_paths`: the paths the hypervisor will need search
    /// access for to perform this install.
    pub fn get_search_paths(&self) -> Vec<String> {
        self.cdrom_path().map(str::to_string).into_iter().collect()
    }

    /// `requires_postboot_xml_changes`: whether the install has 2 XML
    /// phases, and requires a hard VM poweroff to ensure the second
    /// stage is used for subsequent boots.
    pub fn requires_postboot_xml_changes(&self) -> bool {
        if self.no_install {
            return false;
        }
        self.cdrom.is_some() || self.install_bootdev.is_some()
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
    pub fn detect_distro(&self, conn: &Connection) -> Option<String> {
        let mut ret = None;
        if let Some(cdrom) = &self.cdrom {
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
        ret
    }

    /// `_build_postboot_xml`.
    fn build_postboot_xml(
        &self,
        final_xml: &str,
        conn: &Connection,
    ) -> Result<(String, String), String> {
        let mut initial_guest = XmlObject::parse(&GUEST, final_xml).map_err(|e| e.to_string())?;
        self.alter_bootconfig(&mut initial_guest)?;

        let mut final_guest = XmlObject::parse(&GUEST, final_xml).map_err(|e| e.to_string())?;
        self.remove_install_cdrom_media(&mut final_guest, conn);

        Ok((initial_guest.get_xml(), final_guest.get_xml()))
    }

    /// `_build_xml`.
    fn build_xml(
        &self,
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

        let domain = if guest.get_str("type").as_deref() == Some("vz") {
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
        if !return_xml {
            guest::validate_name(conn, &user_guest.get_str("name").unwrap_or_default())?;
        }
        self.set_install_defaults(user_guest, opts, conn)?;

        // All installer XML alterations are made on this guest instance,
        // so the user_guest instance is left intact
        user_guest.check_xml_actions()?;
        let guest = XmlObject::parse(&GUEST, &user_guest.get_xml()).map_err(|e| e.to_string())?;

        if !dry {
            build_storage()?;
        }

        let (initial_xml, final_xml) = self.build_xml(&guest, conn)?;
        if dry || return_xml {
            return Ok(Started::Xml(initial_xml, final_xml));
        }

        let domain = Self::create_guest(
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
    }
}
