//! A new guest's domain XML, built the way `vmmCreateVM` builds it:
//! `_GuestData.build_guest`, `Installer.set_install_defaults` (with
//! `Guest.set_defaults` and every device's `set_defaults`), then the
//! installer's two XML phases (`_build_xml`).
//!
//! The guest is plain data ([`NewGuest`]) that serializes to a fresh
//! document, so the install phase and the final config are the same
//! guest with the installer's few alterations applied.
//!
//! `conn.support.*` checks are libvirt/qemu version gates from before
//! libvirt 5.3/qemu 4.0; every stack this crate builds against passes
//! the version half, so only their per-driver half is kept.

use virtinst_xml::{Document, Element};

use crate::capabilities::domcaps::DomainCapabilities;
use crate::connection::Uri;
use crate::devices::build::{self, GuestInfo, NewChar, NewDisk, NewInterface};
use crate::domain::cpu::{self as domcpu, CpuEditContext};
use crate::osinfo::OsInfo;

/// What the defaults read from the connection.
#[derive(Clone, Debug)]
pub struct ConnFacts {
    pub uri: Uri,
    /// `conn.caps.host.cpu.arch`.
    pub host_arch: String,
    /// `conn.caps.host.cpu.vendor`.
    pub host_cpu_vendor: Option<String>,
}

impl ConnFacts {
    fn is_qemu(&self) -> bool {
        self.uri.is_qemu()
    }
    fn is_test(&self) -> bool {
        self.uri.is_test()
    }
    /// A support check with `hv_version={"qemu": .., "test": 0}`.
    fn qemu_or_test(&self) -> bool {
        self.is_qemu() || self.is_test()
    }
}

/// The `guest.osinfo` answers the defaults use. [`OsFacts::default`]
/// is upstream's synthetic `generic` OS, which supports no devices.
#[derive(Clone, Debug, Default)]
pub struct OsFacts {
    /// `full_id`, for `<metadata>`; `None` for generic.
    pub id: Option<String>,
    pub is_windows: bool,
    /// `get_clock()`.
    pub clock: Option<&'static str>,
    pub virtiodisk: bool,
    pub virtioscsi: bool,
    pub virtionet: bool,
    pub virtiorng: bool,
    pub virtioballoon: bool,
    pub virtioserial: bool,
    pub virtioinput: bool,
    pub virtiogpu: bool,
    pub usb3: bool,
    pub chipset_q35: bool,
    pub netmodels: Vec<String>,
    /// `requires_firmware_efi(guest.os.arch)`.
    pub requires_efi: bool,
}

impl OsFacts {
    /// Reads every answer from `os` once, for a guest of `arch`.
    pub fn from_osinfo(os: &OsInfo, arch: &str) -> OsFacts {
        OsFacts {
            id: Some(os.id()),
            is_windows: os.is_windows(),
            clock: Some(os.clock_offset()),
            virtiodisk: os.supports_virtiodisk(),
            virtioscsi: os.supports_virtioscsi(),
            virtionet: os.supports_virtionet(),
            virtiorng: os.supports_virtiorng(),
            virtioballoon: os.supports_virtioballoon(),
            virtioserial: os.supports_virtioserial(),
            virtioinput: os.supports_virtioinput(),
            virtiogpu: os.supports_virtiogpu(),
            usb3: os.supports_usb3(),
            chipset_q35: os.supports_chipset_q35(),
            netmodels: os.supported_netmodels(),
            requires_efi: os.requires_firmware_efi(arch),
        }
    }
}

/// One device of the new guest, before or after its `set_defaults`.
#[derive(Clone, Debug)]
pub enum Device {
    Disk(NewDisk),
    Controller {
        ctype: String,
        model: Option<String>,
        ports: Option<u32>,
        master_startport: Option<u32>,
    },
    /// `fstype` `None` is libvirt's (and `set_defaults`') `mount`.
    Filesystem {
        fstype: Option<String>,
        accessmode: Option<String>,
        source: String,
        target: String,
    },
    Interface(NewInterface),
    Console {
        ctype: String,
        target_type: Option<String>,
    },
    Channel(NewChar),
    Input {
        itype: String,
        bus: String,
    },
    /// An emulated TPM.
    Tpm {
        model: Option<String>,
    },
    Graphics {
        gtype: String,
    },
    Sound {
        model: String,
    },
    Video {
        model: Option<String>,
    },
    Redirdev,
    Memballoon,
    Rng,
}

impl Device {
    fn tag(&self) -> &'static str {
        match self {
            Device::Disk(_) => "disk",
            Device::Controller { .. } => "controller",
            Device::Filesystem { .. } => "filesystem",
            Device::Interface(_) => "interface",
            Device::Console { .. } => "console",
            Device::Channel(_) => "channel",
            Device::Input { .. } => "input",
            Device::Tpm { .. } => "tpm",
            Device::Graphics { .. } => "graphics",
            Device::Sound { .. } => "sound",
            Device::Video { .. } => "video",
            Device::Redirdev => "redirdev",
            Device::Memballoon => "memballoon",
            Device::Rng => "rng",
        }
    }
}

/// `_DomainDevices._XML_PROP_ORDER`, for the device kinds a new guest
/// can have.
const DEVICE_ORDER: [&str; 14] = [
    "disk",
    "controller",
    "filesystem",
    "interface",
    "console",
    "channel",
    "input",
    "tpm",
    "graphics",
    "sound",
    "video",
    "redirdev",
    "memballoon",
    "rng",
];

/// `<features>` as `set_defaults` fills it.
#[derive(Clone, Debug, Default)]
pub struct Features {
    pub acpi: bool,
    pub apic: bool,
    pub pae: bool,
    pub privnet: bool,
    pub smm: bool,
    /// `<hyperv>` children in the order set: name and `retries`.
    pub hyperv: Vec<(&'static str, Option<u32>)>,
    pub vmport: Option<bool>,
}

/// Old style UEFI: `set_uefi_path`.
#[derive(Clone, Debug)]
pub struct Loader {
    pub path: String,
    pub secure: bool,
}

/// The guest `_GuestData.build_guest` makes: what the wizard picked,
/// on top of `set_capabilities_defaults`.
#[derive(Clone, Debug, Default)]
pub struct NewGuest {
    pub domain_type: String,
    pub os_type: String,
    pub arch: String,
    pub machine: Option<String>,
    pub emulator: Option<String>,
    /// `capsinfo.loader` (xen's hvmloader).
    pub loader: Option<String>,
    pub name: String,
    pub uuid: String,
    /// KiB.
    pub memory: u64,
    pub current_memory: u64,
    pub vcpus: u32,
    pub os: OsFacts,
    /// `uefi_requested`: `Some(true)` forces UEFI, `Some(false)` stops
    /// the default.
    pub uefi_requested: Option<bool>,
    pub init: Option<String>,
    /// Direct kernel boot (`<kernel>`, `<initrd>`, `<cmdline>`), set
    /// on the install XML of a tree install.
    pub kernel: Option<String>,
    pub initrd: Option<String>,
    pub kernel_args: Option<String>,
    pub devices: Vec<Device>,

    // Filled in by `set_defaults` and the installer.
    pub firmware: Option<String>,
    pub uefi_loader: Option<Loader>,
    pub bootorder: Vec<String>,
    pub on_reboot: Option<String>,
    pub features: Features,
    /// The special CPU mode `DomainCpu.set_defaults` picked.
    pub cpu_mode: Option<String>,
    pub cpu_ctx: CpuEditContext,
    pub clock_offset: Option<String>,
    pub timers: Vec<(&'static str, &'static str, &'static str)>,
    pub pm_suspend_off: bool,
    pub have_default_tpm: bool,
}

/// What `capsinfo` and the host tell `set_defaults`.
#[derive(Clone, Debug, Default)]
pub struct CapsFacts {
    /// `capsinfo.machines`.
    pub machines: Vec<String>,
    pub acpi: bool,
    pub apic: bool,
    pub pae: bool,
}

impl NewGuest {
    fn is_hvm(&self) -> bool {
        self.os_type == "hvm"
    }
    fn is_xenpv(&self) -> bool {
        matches!(self.os_type.as_str(), "xen" | "linux" | "xenpvh")
    }
    fn is_container(&self) -> bool {
        self.os_type == "exe"
    }
    fn machine(&self) -> &str {
        self.machine.as_deref().unwrap_or_default()
    }
    fn is_x86(&self) -> bool {
        matches!(self.arch.as_str(), "x86_64" | "i686")
    }
    fn is_q35(&self) -> bool {
        self.is_x86() && self.machine().contains("q35")
    }
    fn is_arm_machvirt(&self) -> bool {
        matches!(self.arch.as_str(), "armv7l" | "aarch64") && self.machine().starts_with("virt")
    }
    fn is_riscv_virt(&self) -> bool {
        matches!(self.arch.as_str(), "riscv64" | "riscv32") && self.machine().starts_with("virt")
    }
    fn is_pseries(&self) -> bool {
        matches!(self.arch.as_str(), "ppc64" | "ppc64le") && self.machine().starts_with("pseries")
    }
    fn is_s390x(&self) -> bool {
        self.arch == "s390x"
    }
    fn is_loongarch64(&self) -> bool {
        self.arch == "loongarch64"
    }

    fn has(&self, tag: &str) -> bool {
        self.devices.iter().any(|d| d.tag() == tag)
    }
    fn add(&mut self, dev: Device) {
        self.devices.push(dev);
    }
    fn has_spice(&self) -> bool {
        self.devices
            .iter()
            .any(|d| matches!(d, Device::Graphics { gtype } if gtype == "spice"))
    }
    fn is_uefi(&self) -> bool {
        self.firmware.as_deref() == Some("efi") || self.uefi_loader.is_some()
    }
    /// `is_full_os_container`.
    fn is_full_os_container(&self) -> bool {
        self.is_container()
            && self
                .devices
                .iter()
                .any(|d| matches!(d, Device::Filesystem { target, .. } if target == "/"))
    }
    fn has_disk_device(&self) -> bool {
        self.devices
            .iter()
            .any(|d| matches!(d, Device::Disk(disk) if disk.device == "disk"))
    }

    /// `_supports_virtio`.
    fn supports_virtio(&self, conn: &ConnFacts, os_support: bool) -> bool {
        if !conn.is_qemu() {
            return false;
        }
        if self.is_riscv_virt() || self.is_s390x() || self.is_pseries() || self.is_loongarch64() {
            return true;
        }
        os_support
    }

    /// The facts the device builders' defaults read.
    fn guest_info(&self) -> GuestInfo {
        GuestInfo {
            os_type: Some(self.os_type.clone()),
            arch: Some(self.arch.clone()),
            machine: self.machine.clone(),
            domain_type: Some(self.domain_type.clone()),
            has_spice: self.has_spice(),
            has_gl: false,
            is_uefi: self.is_uefi(),
            os_virtiogpu: self.os.virtiogpu,
            os_virtionet: self.os.virtionet,
            os_netmodels: self.os.netmodels.clone(),
            os_virtioscsi: self.os.virtioscsi,
            spice_fallback: false,
            ..Default::default()
        }
    }

    fn caps_defaults(&self, conn: &ConnFacts) -> crate::install::GuestCapsDefaults<'_> {
        crate::install::GuestCapsDefaults {
            domain_type: &self.domain_type,
            os_type: &self.os_type,
            arch: &self.arch,
            machine: self.machine.as_deref(),
            emulator: self.emulator.as_deref(),
            is_qemu: conn.is_qemu(),
        }
    }

    /// `Guest.set_defaults`. `domcaps` is `lookup_domcaps()`.
    pub fn set_defaults(
        &mut self,
        conn: &ConnFacts,
        caps: &CapsFacts,
        domcaps: &DomainCapabilities,
    ) -> Result<(), String> {
        if self.uuid.is_empty() {
            self.uuid = crate::cloner::generate_uuid();
        }
        // `sync_vcpus_topology(1)`
        if self.vcpus == 0 {
            self.vcpus = 1;
        }

        self.set_default_machine(conn, caps);
        self.set_default_uefi(conn, domcaps)?;

        self.add_default_graphics(conn, domcaps);
        self.add_default_video_device();
        self.add_default_input_device();
        self.add_default_console_device(conn);
        self.add_default_usb_controller(conn);
        self.add_default_channels(conn);
        self.add_default_rng(conn);
        self.add_default_memballoon(conn);
        self.add_default_tpm(domcaps);

        self.clock_set_defaults(conn);
        self.cpu_set_defaults(conn, domcaps);
        self.features_set_defaults(conn, caps, domcaps);
        self.pm_set_defaults(conn);
        // `os.set_defaults`
        if self.is_container() && self.init.is_none() {
            self.init = Some(if self.is_full_os_container() {
                "/sbin/init".into()
            } else {
                "/bin/sh".into()
            });
        }

        self.devices_set_defaults(conn, domcaps)?;

        self.add_virtioscsi_controller(conn);
        self.add_q35_pcie_controllers();
        self.add_spice_devices(conn, domcaps);
        Ok(())
    }

    /// `_set_default_machine`.
    pub fn set_default_machine(&mut self, conn: &ConnFacts, caps: &CapsFacts) {
        if self.machine.is_some() {
            return;
        }
        if self.is_x86()
            && conn.is_qemu()
            && caps.machines.iter().any(|m| m == "q35")
            && self.os.chipset_q35
        {
            self.machine = Some("q35".into());
            return;
        }
        self.machine = caps.machines.first().cloned();
    }

    /// `prefers_uefi`.
    fn prefers_uefi(&self, conn: &ConnFacts) -> bool {
        if self.is_x86() && conn.qemu_or_test() {
            self.os.requires_efi
        } else {
            self.is_arm_machvirt()
                || self.is_riscv_virt()
                || self.is_loongarch64()
                || conn.uri.is_bhyve()
        }
    }

    /// `_set_default_uefi`, with `enable_uefi`.
    fn set_default_uefi(
        &mut self,
        conn: &ConnFacts,
        domcaps: &DomainCapabilities,
    ) -> Result<(), String> {
        if self.uefi_requested == Some(false) {
            return Ok(());
        }
        let requested = self.uefi_requested == Some(true);
        let use_default =
            self.prefers_uefi(conn) && self.loader.is_none() && self.firmware.is_none();
        if !use_default && !requested {
            return Ok(());
        }
        match self.enable_uefi(conn, domcaps) {
            Ok(()) => Ok(()),
            Err(e) if requested => Err(e),
            Err(e) => {
                eprintln!("Couldn't configure UEFI: {e}");
                eprintln!("Your VM may not boot successfully.");
                Ok(())
            }
        }
    }

    /// `enable_uefi`: firmware autoselection, else a loader path
    /// (`_lookup_default_uefi_path` and `set_uefi_path`).
    fn enable_uefi(
        &mut self,
        conn: &ConnFacts,
        domcaps: &DomainCapabilities,
    ) -> Result<(), String> {
        if domcaps.supports_firmware_efi() {
            self.firmware = Some("efi".into());
            return Ok(());
        }
        if !domcaps.supports_uefi_loader() {
            return Err("Libvirt version does not support UEFI.".into());
        }
        if !domcaps.arch_can_uefi() {
            return Err(format!(
                "Don't know how to setup UEFI for arch '{}'",
                self.arch
            ));
        }
        let path = domcaps
            .find_uefi_path_for_arch(conn.uri.is_bhyve())
            .ok_or_else(|| format!("Did not find any UEFI binary path for arch '{}'", self.arch))?;
        // Secure boot firmware needs SMM, which needs q35.
        let secure = self.is_x86() && path.contains("secboot");
        if secure {
            self.features.smm = true;
            if !self.is_q35() {
                eprintln!(
                    "Changing machine type from '{}' to 'q35' which is required for UEFI secure boot.",
                    self.machine()
                );
                self.machine = Some("q35".into());
            }
        }
        self.uefi_loader = Some(Loader { path, secure });
        Ok(())
    }

    fn usb_disabled(&self) -> bool {
        let usb: Vec<_> = self
            .devices
            .iter()
            .filter_map(|d| match d {
                Device::Controller { ctype, model, .. } if ctype == "usb" => Some(model),
                _ => None,
            })
            .collect();
        !usb.is_empty() && usb.iter().all(|m| m.as_deref() == Some("none"))
    }

    /// `_add_default_input_device`.
    fn add_default_input_device(&mut self) {
        if self.is_container() || self.is_xenpv() || self.has("input") || !self.has("graphics") {
            return;
        }
        // x86 has historically relied on the default PS/2 keyboard
        let keyboard = !self.is_x86();
        let bus = if self.is_s390x() {
            self.os.virtioinput.then_some("virtio")
        } else if !self.usb_disabled() {
            Some("usb")
        } else {
            None
        };
        let Some(bus) = bus else {
            return;
        };
        self.add(Device::Input {
            itype: "tablet".into(),
            bus: bus.into(),
        });
        if keyboard {
            self.add(Device::Input {
                itype: "keyboard".into(),
                bus: bus.into(),
            });
        }
    }

    /// `_add_default_console_device`. bhyve's nmdm console isn't built.
    fn add_default_console_device(&mut self, _conn: &ConnFacts) {
        if self.has("console") {
            return;
        }
        self.add(Device::Console {
            ctype: "pty".into(),
            target_type: self.is_s390x().then(|| "sclp".into()),
        });
    }

    /// `_add_default_video_device`.
    fn add_default_video_device(&mut self) {
        if self.is_container() || self.has("video") || !self.has("graphics") {
            return;
        }
        self.add(Device::Video { model: None });
    }

    /// `_add_default_usb_controller`.
    fn add_default_usb_controller(&mut self, conn: &ConnFacts) {
        let has_usb = self
            .devices
            .iter()
            .any(|d| matches!(d, Device::Controller { ctype, .. } if ctype == "usb"));
        if has_usb || !conn.qemu_or_test() {
            return;
        }
        // `conn_qemu_xhci` only holds for qemu.
        let qemu_usb3 = conn.is_qemu();
        let (mut usb2, mut usb3) = (false, false);
        if self.is_x86() {
            usb3 = self.os.usb3 && qemu_usb3;
            usb2 = !usb3;
        } else if self.is_arm_machvirt()
            || self.is_riscv_virt()
            || self.is_pseries()
            || self.is_loongarch64()
        {
            usb3 = qemu_usb3;
        }
        if usb2 {
            self.add(controller("usb", Some("ich9-ehci1"), None, None));
            for (i, model) in ["ich9-uhci1", "ich9-uhci2", "ich9-uhci3"]
                .iter()
                .enumerate()
            {
                self.add(controller("usb", Some(model), None, Some(i as u32 * 2)));
            }
        } else if usb3 {
            // `get_usb3_controller`
            let model = if conn.is_qemu() {
                "qemu-xhci"
            } else {
                "nec-xhci"
            };
            self.add(controller("usb", Some(model), Some(15), None));
        }
    }

    /// `_add_default_channels`: the qemu guest agent channel.
    fn add_default_channels(&mut self, conn: &ConnFacts) {
        if self.has("channel") {
            return;
        }
        if conn.is_qemu() && self.supports_virtio(conn, self.os.virtioserial) {
            self.add(Device::Channel(NewChar {
                tag: "channel".into(),
                char_type: "unix".into(),
                target_type: Some("virtio".into()),
                target_name: Some(build::CHANNEL_NAME_QEMUGA.into()),
                ..Default::default()
            }));
        }
    }

    /// `_add_default_graphics`.
    fn add_default_graphics(&mut self, conn: &ConnFacts, domcaps: &DomainCapabilities) {
        if self.has("graphics") {
            return;
        }
        if self.is_container() && !conn.uri.is_vz() {
            return;
        }
        if !self.is_x86()
            && !self.is_pseries()
            && !self.is_loongarch64()
            && !self.is_arm_machvirt()
            && !self.is_riscv_virt()
        {
            return;
        }
        // `DeviceGraphics._default_type`
        let gtype = domcaps.default_graphics_type(&conn.uri, Some(&conn.host_arch));
        self.add(Device::Graphics {
            gtype: gtype.into(),
        });
    }

    fn rng_balloon_arch(&self) -> bool {
        self.is_x86()
            || self.is_arm_machvirt()
            || self.is_riscv_virt()
            || self.is_s390x()
            || self.is_pseries()
            || self.is_loongarch64()
    }

    /// `_add_default_rng`.
    fn add_default_rng(&mut self, conn: &ConnFacts) {
        if self.has("rng") || !self.rng_balloon_arch() {
            return;
        }
        if conn.is_qemu() && self.os.virtiorng {
            self.add(Device::Rng);
        }
    }

    /// `_add_default_memballoon`.
    fn add_default_memballoon(&mut self, conn: &ConnFacts) {
        if self.has("memballoon") || !conn.is_qemu() || !self.rng_balloon_arch() {
            return;
        }
        if self.os.virtioballoon {
            self.add(Device::Memballoon);
        }
    }

    /// `_add_default_tpm`: UEFI guests target a modern platform, so
    /// they get an emulated TPM.
    fn add_default_tpm(&mut self, domcaps: &DomainCapabilities) {
        if self.has("tpm") || !self.is_uefi() {
            return;
        }
        if !domcaps.supports_tpm_emulator() {
            return;
        }
        self.add(Device::Tpm { model: None });
        self.have_default_tpm = true;
    }

    /// `DomainClock.set_defaults`.
    fn clock_set_defaults(&mut self, conn: &ConnFacts) {
        if !self.is_hvm() {
            return;
        }
        if self.clock_offset.is_none() {
            self.clock_offset = Some(self.os.clock.unwrap_or("utc").into());
        }
        if !self.timers.is_empty() || !self.is_x86() || !conn.is_qemu() {
            return;
        }
        // -no-hpet -no-kvm-pit-reinjection -rtc driftfix=slew
        self.timers = vec![
            ("rtc", "tickpolicy", "catchup"),
            ("pit", "tickpolicy", "delay"),
            ("hpet", "present", "no"),
        ];
        if self.os.is_windows && conn.qemu_or_test() {
            self.timers.push(("hypervclock", "present", "yes"));
        }
    }

    /// `DomainCpu.set_defaults`.
    fn cpu_set_defaults(&mut self, conn: &ConnFacts, domcaps: &DomainCapabilities) {
        if !conn.qemu_or_test() || self.cpu_mode.is_some() {
            return;
        }
        if self.is_arm_machvirt() && self.domain_type == "kvm" {
            self.cpu_mode = Some(domcpu::SPECIAL_MODE_HOST_PASSTHROUGH.into());
        } else if self.is_x86() && self.domain_type == "kvm" {
            // `_set_cpu_x86_kvm_default`
            if self.arch == conn.host_arch {
                self.cpu_mode = Some(self.cpu_ctx.app_default_mode.clone());
            }
        } else if domcaps.supports_maximum_cpu_mode()
            && self.domain_type == "qemu"
            && (self.is_x86()
                || self.is_arm_machvirt()
                || self.is_riscv_virt()
                || self.is_loongarch64())
        {
            self.cpu_mode = Some(domcpu::SPECIAL_MODE_MAXIMUM.into());
        }
    }

    /// `DomainFeatures.set_defaults`.
    fn features_set_defaults(
        &mut self,
        conn: &ConnFacts,
        caps: &CapsFacts,
        domcaps: &DomainCapabilities,
    ) {
        if self.is_container() {
            if self.is_full_os_container() && self.domain_type != "vz" {
                self.features.privnet = true;
            }
            return;
        }
        if !self.is_hvm() {
            return;
        }
        self.features.acpi = caps.acpi;
        self.features.apic = caps.apic;
        self.features.pae = (self.domain_type == "xen" && self.arch == "x86_64") || caps.pae;
        self.set_hyperv_defaults(conn, domcaps);
    }

    /// `_set_hyperv_defaults`.
    fn set_hyperv_defaults(&mut self, conn: &ConnFacts, domcaps: &DomainCapabilities) {
        if !self.os.is_windows {
            return;
        }
        let supported = domcaps.supported_hyperv_features();
        let mut on: Vec<(&'static str, Option<u32>)> = vec![];
        let mut enable = |name: &'static str, feature: &str, requires: &[&str], retries| {
            let set = |on: &Vec<(&str, Option<u32>)>, n: &str| on.iter().any(|(x, _)| *x == n);
            if !supported.iter().any(|f| f == feature) || set(&on, name) {
                return;
            }
            if requires.iter().all(|r| set(&on, r)) {
                on.push((name, retries));
            }
        };
        enable("relaxed", "relaxed", &[], None);
        enable("vapic", "vapic", &[], None);
        enable("spinlocks", "spinlocks", &[], Some(8191));
        enable("vpindex", "vpindex", &[], None);
        enable("runtime", "runtime", &[], None);
        enable("synic", "synic", &["vpindex"], None);
        if self.timers.iter().any(|(n, _, _)| *n == "hypervclock") {
            enable("stimer", "stimer", &["vpindex", "synic"], None);
            enable(
                "stimer_direct",
                "stimer_direct",
                &["vpindex", "synic", "stimer"],
                None,
            );
        }
        enable("frequencies", "frequencies", &[], None);
        enable("tlbflush", "tlbflush", &["vpindex"], None);
        enable("ipi", "ipi", &["vpindex"], None);
        if conn.host_cpu_vendor.as_deref() == Some("Intel") {
            enable("evmcs", "evmcs", &["vapic"], None);
        }
        if self.features.apic {
            enable("avic", "avic", &[], None);
        }
        self.features.hyperv = on;
    }

    /// `DomainPm.set_defaults`: suspend from an ACPI shutdown confuses
    /// people, so it's off.
    fn pm_set_defaults(&mut self, conn: &ConnFacts) {
        if self.is_x86() && conn.qemu_or_test() {
            self.pm_suspend_off = true;
        }
    }

    /// Every device's `set_defaults`, in device order.
    fn devices_set_defaults(
        &mut self,
        conn: &ConnFacts,
        domcaps: &DomainCapabilities,
    ) -> Result<(), String> {
        let mut info = self.guest_info();
        info.spice_fallback =
            conn.qemu_or_test() && matches!(conn.host_arch.as_str(), "i686" | "x86_64");
        let caps = self.caps_defaults(conn);
        let bus = |is_cdrom: bool| {
            crate::install::default_disk_bus(
                caps,
                is_cdrom,
                conn.uri.is_bhyve(),
                self.os.virtiodisk,
                self.os.virtioscsi,
            )
        };
        let (disk_bus, cdrom_bus) = (bus(false), bus(true));
        let mut used_targets: Vec<String> = self
            .devices
            .iter()
            .filter_map(|d| match d {
                Device::Disk(d) if !d.target.is_empty() => Some(d.target.clone()),
                _ => None,
            })
            .collect();
        let video_model = info.default_video_model(Some(domcaps), conn.is_qemu());
        let nic_model = info.default_nic_model(conn.is_qemu());
        let tpm_model = build::default_tpm_model(Some(domcaps), &info);
        let fs_defaults = conn.qemu_or_test() || conn.uri.is_lxc();

        let mut devices = std::mem::take(&mut self.devices);
        for dev in devices.iter_mut() {
            match dev {
                Device::Disk(d) => {
                    if d.bus.is_empty() {
                        d.bus = if d.device == "cdrom" {
                            cdrom_bus
                        } else {
                            disk_bus
                        }
                        .into();
                    }
                    if d.target.is_empty() {
                        d.target = build::generate_target(&d.bus, &d.device, &used_targets)?;
                        used_targets.push(d.target.clone());
                    }
                }
                Device::Interface(n) => {
                    if n.model.is_none() {
                        n.model = nic_model.map(str::to_string);
                    }
                }
                Device::Video { model } => {
                    if model.is_none() {
                        *model = video_model.map(str::to_string);
                    }
                }
                Device::Tpm { model } => {
                    if model.is_none() {
                        *model = tpm_model.map(str::to_string);
                    }
                }
                Device::Filesystem {
                    fstype, accessmode, ..
                } if fs_defaults => {
                    fstype.get_or_insert_with(|| crate::devices::filesystem::TYPE_MOUNT.into());
                    if accessmode.is_none() {
                        *accessmode = crate::devices::filesystem::default_accessmode(None)
                            .map(str::to_string);
                    }
                }
                _ => {}
            }
        }
        self.devices = devices;
        Ok(())
    }

    /// `add_virtioscsi_controller`: for SCSI disks, when the guest can
    /// use virtio-scsi.
    fn add_virtioscsi_controller(&mut self, conn: &ConnFacts) {
        let has_scsi_ctrl = self
            .devices
            .iter()
            .any(|d| matches!(d, Device::Controller { ctype, .. } if ctype == "scsi"));
        if has_scsi_ctrl || !self.supports_virtio(conn, self.os.virtioscsi) {
            return;
        }
        let scsi_disk = self
            .devices
            .iter()
            .any(|d| matches!(d, Device::Disk(disk) if disk.bus == "scsi"));
        if scsi_disk {
            self.add(controller("scsi", Some("virtio-scsi"), None, None));
        }
    }

    /// `add_q35_pcie_controllers`: pcie-root and 14 root ports (qemu
    /// 6.1 breaks with 15 or more).
    fn add_q35_pcie_controllers(&mut self) {
        let has_pci = self
            .devices
            .iter()
            .any(|d| matches!(d, Device::Controller { ctype, .. } if ctype == "pci"));
        let pcie = self.is_q35()
            || self.is_arm_machvirt()
            || self.is_riscv_virt()
            || self.is_loongarch64();
        if has_pci || !pcie {
            return;
        }
        self.add(controller("pci", Some("pcie-root"), None, None));
        for _ in 0..14 {
            self.add(controller("pci", Some("pcie-root-port"), None, None));
        }
    }

    /// `_add_spice_devices`.
    fn add_spice_devices(&mut self, conn: &ConnFacts, domcaps: &DomainCapabilities) {
        if !self.has_spice() {
            return;
        }
        if self.features.vmport.is_none() && self.is_x86() && conn.qemu_or_test() {
            self.features.vmport = Some(false);
        }
        // `_add_spice_channels`
        let has_spicevmc = self
            .devices
            .iter()
            .any(|d| matches!(d, Device::Channel(c) if c.char_type == "spicevmc"));
        if domcaps.supports_channel_spicevmc() && !has_spicevmc {
            self.add(Device::Channel(NewChar {
                tag: "channel".into(),
                char_type: "spicevmc".into(),
                ..Default::default()
            }));
        }
        // `_add_spice_sound`
        if !self.has("sound") && self.is_hvm() {
            let model = self.guest_info().default_sound_model().into();
            self.add(Device::Sound { model });
        }
        // `_add_spice_usbredir`: two, so assigned USB2 devices still
        // find free slots.
        if domcaps.supports_redirdev_usb()
            && !self.has("redirdev")
            && !self.usb_disabled()
            && self.is_x86()
        {
            self.add(Device::Redirdev);
            self.add(Device::Redirdev);
        }
    }

    /// `Guest.get_xml()`.
    pub fn to_xml(&self, conn: &ConnFacts) -> String {
        let mut doc = Document::new();
        let root = Element::new(&mut doc, "domain");
        root.set_attribute(&mut doc, "type", &self.domain_type);
        doc.push_root_node(root)
            .expect("a fresh element always attaches");
        let text = |doc: &mut Document, parent: Element, tag: &str, val: &str| {
            Element::build(tag).add_text(val).push_to(doc, parent);
        };
        text(&mut doc, root, "name", &self.name);
        text(&mut doc, root, "uuid", &self.uuid);
        if let Some(id) = &self.os.id {
            let meta = Element::build("metadata").push_to(&mut doc, root);
            let osinfo = Element::build("libosinfo:libosinfo")
                .attribute(
                    "xmlns:libosinfo",
                    "http://libosinfo.org/xmlns/libvirt/domain/1.0",
                )
                .push_to(&mut doc, meta);
            Element::build("libosinfo:os")
                .attribute("id", id)
                .push_to(&mut doc, osinfo);
        }
        text(&mut doc, root, "memory", &self.memory.to_string());
        text(
            &mut doc,
            root,
            "currentMemory",
            &self.current_memory.to_string(),
        );
        text(&mut doc, root, "vcpu", &self.vcpus.to_string());

        let os = Element::build("os").push_to(&mut doc, root);
        if let Some(fw) = &self.firmware {
            os.set_attribute(&mut doc, "firmware", fw);
        }
        let ostype = Element::build("type")
            .attribute("arch", &self.arch)
            .add_text(&self.os_type)
            .push_to(&mut doc, os);
        if let Some(m) = &self.machine {
            ostype.set_attribute(&mut doc, "machine", m);
        }
        if let Some(l) = &self.uefi_loader {
            let loader = Element::build("loader")
                .attribute("readonly", "yes")
                .attribute("type", "pflash")
                .add_text(&l.path)
                .push_to(&mut doc, os);
            if l.secure {
                loader.set_attribute(&mut doc, "secure", "yes");
            }
        } else if let Some(l) = &self.loader {
            text(&mut doc, os, "loader", l);
        }
        if let Some(init) = &self.init {
            text(&mut doc, os, "init", init);
        }
        for (name, val) in [
            ("kernel", &self.kernel),
            ("initrd", &self.initrd),
            ("cmdline", &self.kernel_args),
        ] {
            if let Some(v) = val {
                text(&mut doc, os, name, v);
            }
        }
        for dev in &self.bootorder {
            Element::build("boot")
                .attribute("dev", dev)
                .push_to(&mut doc, os);
        }

        let f = &self.features;
        let any_feature = f.acpi
            || f.apic
            || f.pae
            || f.privnet
            || f.smm
            || !f.hyperv.is_empty()
            || f.vmport.is_some();
        if any_feature {
            let features = Element::build("features").push_to(&mut doc, root);
            for (on, tag) in [
                (f.acpi, "acpi"),
                (f.apic, "apic"),
                (f.pae, "pae"),
                (f.privnet, "privnet"),
            ] {
                if on {
                    Element::build(tag).push_to(&mut doc, features);
                }
            }
            if !f.hyperv.is_empty() {
                let hv = Element::build("hyperv").push_to(&mut doc, features);
                for (name, retries) in &f.hyperv {
                    if *name == "stimer_direct" {
                        let stimer = hv
                            .find(&doc, "stimer")
                            .unwrap_or_else(|| Element::build("stimer").push_to(&mut doc, hv));
                        Element::build("direct")
                            .attribute("state", "on")
                            .push_to(&mut doc, stimer);
                        continue;
                    }
                    let el = Element::build(*name)
                        .attribute("state", "on")
                        .push_to(&mut doc, hv);
                    if let Some(r) = retries {
                        el.set_attribute(&mut doc, "retries", r.to_string());
                    }
                }
            }
            if let Some(v) = f.vmport {
                Element::build("vmport")
                    .attribute("state", if v { "on" } else { "off" })
                    .push_to(&mut doc, features);
            }
            if f.smm {
                Element::build("smm")
                    .attribute("state", "on")
                    .push_to(&mut doc, features);
            }
        }

        if let Some(mode) = &self.cpu_mode {
            if let Err(e) = domcpu::edit_set_special_mode(&mut doc, root, &self.cpu_ctx, mode, true)
            {
                eprintln!("{e}");
            }
        }

        if let Some(offset) = &self.clock_offset {
            let clock = Element::build("clock")
                .attribute("offset", offset)
                .push_to(&mut doc, root);
            for (name, attr, val) in &self.timers {
                Element::build("timer")
                    .attribute("name", *name)
                    .attribute(*attr, *val)
                    .push_to(&mut doc, clock);
            }
        }
        if self.pm_suspend_off {
            let pm = Element::build("pm").push_to(&mut doc, root);
            for tag in ["suspend-to-mem", "suspend-to-disk"] {
                Element::build(tag)
                    .attribute("enabled", "no")
                    .push_to(&mut doc, pm);
            }
        }

        let devices = Element::build("devices").push_to(&mut doc, root);
        if let Some(e) = &self.emulator {
            text(&mut doc, devices, "emulator", e);
        }
        for tag in DEVICE_ORDER {
            for dev in self.devices.iter().filter(|d| d.tag() == tag) {
                let el = self.build_device(&mut doc, dev, conn);
                devices
                    .push_child(&mut doc, el)
                    .expect("a fresh element always attaches");
            }
        }
        // The installer sets it on the parsed final XML, which puts it
        // last.
        if let Some(r) = &self.on_reboot {
            text(&mut doc, root, "on_reboot", r);
        }
        doc.write_str().unwrap_or_default()
    }

    fn build_device(&self, doc: &mut Document, dev: &Device, conn: &ConnFacts) -> Element {
        match dev {
            Device::Disk(d) => build::disk(doc, d, conn.is_qemu(), conn.uri.is_xen()),
            Device::Controller {
                ctype,
                model,
                ports,
                master_startport,
            } => {
                let el = build::controller(doc, ctype, model.as_deref(), None);
                if let Some(p) = ports {
                    el.set_attribute(doc, "ports", p.to_string());
                }
                if let Some(p) = master_startport {
                    Element::build("master")
                        .attribute("startport", p.to_string())
                        .push_to(doc, el);
                }
                el
            }
            Device::Filesystem {
                fstype,
                accessmode,
                source,
                target,
            } => {
                let el = Element::new(doc, "filesystem");
                if let Some(t) = fstype {
                    el.set_attribute(doc, "type", t);
                }
                if let Some(m) = accessmode {
                    el.set_attribute(doc, "accessmode", m);
                }
                let attr = match fstype.as_deref() {
                    Some("template") => "name",
                    Some("file") => "file",
                    Some("block") => "dev",
                    _ => "dir",
                };
                Element::build("source")
                    .attribute(attr, source)
                    .push_to(doc, el);
                Element::build("target")
                    .attribute("dir", target)
                    .push_to(doc, el);
                el
            }
            Device::Interface(n) => build::interface(doc, n, None),
            Device::Console { ctype, target_type } => {
                let el = Element::new(doc, "console");
                el.set_attribute(doc, "type", ctype);
                if let Some(t) = target_type {
                    Element::build("target")
                        .attribute("type", t)
                        .push_to(doc, el);
                }
                el
            }
            Device::Channel(c) => build::char_device(doc, c),
            Device::Input { itype, bus } => build::input(doc, itype, bus),
            Device::Tpm { model } => {
                let el = Element::new(doc, "tpm");
                if let Some(m) = model {
                    el.set_attribute(doc, "model", m);
                }
                Element::build("backend")
                    .attribute("type", "emulator")
                    .push_to(doc, el);
                el
            }
            Device::Graphics { gtype } => {
                let el = Element::new(doc, "graphics");
                el.set_attribute(doc, "type", gtype);
                build::graphics_defaults(doc, el, conn.uri.is_remote());
                el
            }
            Device::Sound { model } => {
                let el = Element::new(doc, "sound");
                el.set_attribute(doc, "model", model);
                el
            }
            Device::Video { model } => {
                build::video(doc, model.as_deref(), None, &self.guest_info())
            }
            Device::Redirdev => build::redirdev(doc, Some("spicevmc")),
            Device::Memballoon => {
                let el = Element::new(doc, "memballoon");
                el.set_attribute(doc, "model", "virtio");
                el
            }
            Device::Rng => build::rng(doc, "/dev/urandom"),
        }
    }
}

fn controller(
    ctype: &str,
    model: Option<&str>,
    ports: Option<u32>,
    master_startport: Option<u32>,
) -> Device {
    Device::Controller {
        ctype: ctype.into(),
        model: model.map(str::to_string),
        ports,
        master_startport,
    }
}

/// The `Installer` `vmmCreateVM` builds: install media and the
/// boot device that goes with it.
#[derive(Clone, Debug, Default)]
pub struct Installer {
    /// The install CD-ROM: path and storage props, bus and target
    /// left for `set_defaults`.
    pub cdrom: Option<NewDisk>,
    /// `_install_bootdev`: `cdrom` for a CD install.
    pub install_bootdev: Option<String>,
    /// A tree install (`_treemedia`): it boots the installer's
    /// kernel and initrd.
    pub treemedia: bool,
    /// `_treemedia_bootconfig`: the fetched kernel, initrd and kernel
    /// args, once `prepare` ran.
    pub treemedia_bootconfig: Option<(String, String, String)>,
    /// `_alter_install_resources`: the raised install `currentMemory`
    /// (KiB) a network install needs.
    pub install_memory: Option<u64>,
}

/// The two XML phases of `Installer._build_xml`: `initial` boots the
/// installer (and goes away on reboot), `final_xml` is what's defined.
#[derive(Clone, Debug)]
pub struct InstallXml {
    pub initial: Option<String>,
    pub final_xml: String,
}

impl Installer {
    /// For a CD-ROM install from `path`.
    pub fn for_cdrom(path: &str, props: Option<crate::devices::disk::PathProps>) -> Installer {
        Installer {
            cdrom: Some(NewDisk {
                device: "cdrom".into(),
                path: Some(path.to_string()),
                props,
                ..Default::default()
            }),
            install_bootdev: Some("cdrom".into()),
            ..Default::default()
        }
    }

    /// For a tree install from `location`; `cdrom` is the location
    /// when it is an ISO, attached like an install CD-ROM.
    pub fn for_location(cdrom: Option<NewDisk>) -> Installer {
        Installer {
            cdrom,
            treemedia: true,
            ..Default::default()
        }
    }

    /// `_can_set_guest_bootorder`.
    fn can_set_bootorder(guest: &NewGuest) -> bool {
        !guest.is_container()
    }

    /// `_build_boot_order`: an attached disk is always bootable too.
    fn build_boot_order(guest: &NewGuest, bootdev: &str) -> Vec<String> {
        let mut order = vec![bootdev.to_string()];
        if guest.has_disk_device() && bootdev != "hd" {
            order.push("hd".into());
        }
        order
    }

    /// `_get_postinstall_bootdev`.
    fn postinstall_bootdev(&self, guest: &NewGuest) -> String {
        if let Some(dev) = &self.install_bootdev {
            if guest.has_disk_device() {
                return "hd".into();
            }
            return dev.clone();
        }
        let first = guest.devices.iter().find_map(|d| match d {
            Device::Disk(disk) => Some(disk.device.as_str()),
            _ => None,
        });
        match first {
            Some("cdrom") => "cdrom".into(),
            Some("floppy") => "fd".into(),
            _ => "hd".into(),
        }
    }

    /// `set_install_defaults`: adds the install CD-ROM, sets the
    /// post-install boot order and runs `guest.set_defaults`.
    pub fn set_install_defaults(
        &self,
        guest: &mut NewGuest,
        conn: &ConnFacts,
        caps: &CapsFacts,
        domcaps: &DomainCapabilities,
    ) -> Result<(), String> {
        // `_add_install_cdrom_device`: before any other CD-ROM, so
        // boot=cdrom picks it.
        if let Some(cdrom) = &self.cdrom {
            let idx = guest
                .devices
                .iter()
                .position(|d| matches!(d, Device::Disk(disk) if disk.device == "cdrom"));
            let dev = Device::Disk(cdrom.clone());
            match idx {
                Some(i) => guest.devices.insert(i, dev),
                None => guest.devices.push(dev),
            }
        }
        if guest.bootorder.is_empty() && Self::can_set_bootorder(guest) {
            let bootdev = self.postinstall_bootdev(guest);
            guest.bootorder = Self::build_boot_order(guest, &bootdev);
        }
        guest.set_defaults(conn, caps, domcaps)
    }

    /// `requires_postboot_xml_changes`.
    pub fn requires_postboot_xml_changes(&self) -> bool {
        self.cdrom.is_some() || self.install_bootdev.is_some() || self.treemedia
    }

    /// `_build_xml`, for a guest `set_install_defaults` already ran on.
    pub fn build_xml(&self, guest: &NewGuest, conn: &ConnFacts) -> InstallXml {
        if !self.requires_postboot_xml_changes() {
            return InstallXml {
                initial: None,
                final_xml: guest.to_xml(conn),
            };
        }
        // `_alter_bootconfig`
        let mut initial = guest.clone();
        initial.on_reboot = Some("destroy".into());
        initial.bootorder = match &self.install_bootdev {
            Some(dev) if Self::can_set_bootorder(guest) => Self::build_boot_order(guest, dev),
            _ => vec![],
        };
        // `_alter_treemedia_bootconfig`
        if let Some((kernel, initrd, kernel_args)) = &self.treemedia_bootconfig {
            initial.kernel = Some(kernel.clone());
            initial.initrd = Some(initrd.clone());
            initial.kernel_args = Some(kernel_args.clone()).filter(|a| !a.is_empty());
        }
        // `_alter_install_resources`
        if let Some(mem) = self.install_memory {
            initial.current_memory = mem;
        }

        // `_remove_install_cdrom_media`: Windows installs reboot into
        // the media again, so it stays.
        let mut final_guest = guest.clone();
        if let Some(cdrom) = &self.cdrom {
            if !guest.os.is_windows {
                let found = final_guest.devices.iter_mut().find_map(|d| match d {
                    Device::Disk(disk) if disk.device == "cdrom" && disk.path == cdrom.path => {
                        Some(disk)
                    }
                    _ => None,
                });
                if let Some(disk) = found {
                    disk.path = None;
                }
            }
        }
        InstallXml {
            initial: Some(initial.to_xml(conn)),
            final_xml: final_guest.to_xml(conn),
        }
    }
}

impl Installer {
    /// `_build_xml` for a guest the "customize before install" dialog
    /// edited, so only its XML is left: the same alterations as
    /// [`Installer::build_xml`], made on that XML.
    pub fn build_xml_from_xml(&self, xml: &str, is_windows: bool) -> Result<InstallXml, String> {
        if !self.requires_postboot_xml_changes() {
            return Ok(InstallXml {
                initial: None,
                final_xml: xml.to_string(),
            });
        }
        let parse = |x: &str| virtinst_xml::parse_libvirt_xml(x).map_err(|e| e.to_string());
        let write = |doc: &virtinst_xml::Document| doc.write_str().map_err(|e| e.to_string());

        // `_alter_bootconfig`
        let mut doc = parse(xml)?;
        let root = doc.root_element().ok_or("Empty domain XML")?;
        let on_reboot = virtinst_xml::resolve_or_create_path(&mut doc, root, &["on_reboot"]);
        on_reboot.set_text_content(&mut doc, "destroy");
        let os = virtinst_xml::resolve_or_create_path(&mut doc, root, &["os"]);
        // `_alter_treemedia_bootconfig`
        if let Some((kernel, initrd, kernel_args)) = &self.treemedia_bootconfig {
            for (tag, val) in [
                ("kernel", kernel),
                ("initrd", initrd),
                ("cmdline", kernel_args),
            ] {
                if !val.is_empty() {
                    let el = virtinst_xml::resolve_or_create_path(&mut doc, os, &[tag]);
                    el.set_text_content(&mut doc, val.as_str());
                }
            }
        }
        let devices = root.find(&doc, "devices");
        let disk_devices: Vec<String> = devices
            .map(|d| {
                d.find_all(&doc, "disk")
                    .iter()
                    .map(|disk| disk.attribute(&doc, "device").unwrap_or("disk").to_string())
                    .collect()
            })
            .unwrap_or_default();
        // `_can_set_guest_bootorder`
        let is_container = os
            .find(&doc, "type")
            .is_some_and(|t| t.text_content(&doc) == "exe");
        let has_device_boot = devices.is_some_and(|d| {
            d.child_elements(&doc).iter().any(|dev| {
                dev.find(&doc, "boot")
                    .is_some_and(|b| b.attribute(&doc, "order").is_some())
            })
        });
        let can_set = !is_container && os.find(&doc, "kernel").is_none() && !has_device_boot;
        let bootorder = match &self.install_bootdev {
            Some(dev) if can_set => {
                // `_build_boot_order`
                let mut order = vec![dev.clone()];
                if disk_devices.iter().any(|d| d == "disk") && !order.iter().any(|d| d == "hd") {
                    order.push("hd".into());
                }
                order
            }
            _ => vec![],
        };
        for boot in os.find_all(&doc, "boot") {
            boot.detach(&mut doc).map_err(|e| e.to_string())?;
        }
        for dev in bootorder {
            let boot = Element::new(&mut doc, "boot");
            boot.set_attribute(&mut doc, "dev", dev);
            os.push_child(&mut doc, boot).map_err(|e| e.to_string())?;
        }
        // `_alter_install_resources`
        if let Some(mem) = self.install_memory {
            let el = virtinst_xml::resolve_or_create_path(&mut doc, root, &["currentMemory"]);
            el.set_text_content(&mut doc, mem.to_string());
        }
        let initial = write(&doc)?;

        // `_remove_install_cdrom_media`: Windows installs reboot into
        // the media again, so it stays.
        let mut doc = parse(xml)?;
        let root = doc.root_element().ok_or("Empty domain XML")?;
        let cdrom_path = self.cdrom.as_ref().and_then(|c| c.path.clone());
        if let (Some(path), false) = (cdrom_path, is_windows) {
            let disks = root
                .find(&doc, "devices")
                .map(|d| d.find_all(&doc, "disk"))
                .unwrap_or_default();
            for disk in disks {
                if disk.attribute(&doc, "device") != Some("cdrom") {
                    continue;
                }
                let Some(source) = disk.find(&doc, "source") else {
                    continue;
                };
                let src_path = ["file", "dev", "dir"]
                    .iter()
                    .find_map(|a| source.attribute(&doc, a));
                if src_path != Some(path.as_str()) {
                    continue;
                }
                source.detach(&mut doc).map_err(|e| e.to_string())?;
                // `sync_path_props` with no path.
                disk.set_attribute(&mut doc, "type", "file");
                break;
            }
        }
        Ok(InstallXml {
            initial: Some(initial),
            final_xml: write(&doc)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::disk::PathProps;
    use crate::osinfo::OsDb;
    use virtinst_xml::parse_libvirt_xml;

    const DOMCAPS: &str = include_str!("testdata/domcaps-q35.xml");
    const ISO: &str = "/var/lib/libvirt/images/f41.iso";

    fn conn() -> ConnFacts {
        ConnFacts {
            uri: Uri::parse("qemu:///system"),
            host_arch: "x86_64".into(),
            host_cpu_vendor: Some("Intel".into()),
        }
    }

    fn caps() -> CapsFacts {
        CapsFacts {
            machines: vec!["pc-i440fx-9.0".into(), "pc".into(), "q35".into()],
            acpi: true,
            apic: true,
            pae: true,
        }
    }

    fn guest(os: OsFacts, with_disk: bool) -> NewGuest {
        let mut g = NewGuest {
            domain_type: "qemu".into(),
            os_type: "hvm".into(),
            arch: "x86_64".into(),
            emulator: Some("/usr/bin/qemu-system-x86_64".into()),
            name: "vm1".into(),
            memory: 4194304,
            current_memory: 4194304,
            vcpus: 2,
            os,
            ..Default::default()
        };
        g.cpu_ctx.app_default_mode = "host-passthrough".into();
        if with_disk {
            g.devices.push(Device::Disk(NewDisk {
                device: "disk".into(),
                path: Some("/var/lib/libvirt/images/vm1.qcow2".into()),
                props: Some(PathProps {
                    dev_type: "file".into(),
                    format: Some("qcow2".into()),
                }),
                sparse_install: true,
                ..Default::default()
            }));
        }
        g.devices.push(Device::Interface(NewInterface {
            nettype: "network".into(),
            source: Some("default".into()),
            mac: "52:54:00:11:22:33".into(),
            ..Default::default()
        }));
        g
    }

    fn os(short_id: &str) -> OsFacts {
        let db = OsDb::load_default().expect("osinfo-db is installed on this machine");
        OsFacts::from_osinfo(&db.os_by_id_or_short_id(short_id).unwrap(), "x86_64")
    }

    fn install(mut g: NewGuest, inst: Installer) -> InstallXml {
        let domcaps = DomainCapabilities::parse(DOMCAPS).unwrap();
        inst.set_install_defaults(&mut g, &conn(), &caps(), &domcaps)
            .unwrap();
        inst.build_xml(&g, &conn())
    }

    fn attr(xml: &str, path: &[&str], name: &str) -> Option<String> {
        let doc = parse_libvirt_xml(xml).unwrap();
        let mut el = doc.root_element().unwrap();
        for p in path {
            el = el.find(&doc, p)?;
        }
        el.attribute(&doc, name).map(String::from)
    }

    fn boot_devs(xml: &str) -> Vec<String> {
        let doc = parse_libvirt_xml(xml).unwrap();
        let os = doc.root_element().unwrap().find(&doc, "os").unwrap();
        os.find_all(&doc, "boot")
            .iter()
            .filter_map(|b| b.attribute(&doc, "dev").map(String::from))
            .collect()
    }

    fn cdrom() -> Installer {
        Installer::for_cdrom(
            ISO,
            Some(PathProps {
                dev_type: "file".into(),
                format: Some("iso".into()),
            }),
        )
    }

    #[test]
    fn cdrom_install_is_two_phase() {
        let x = install(guest(os("fedora41"), true), cdrom());
        let initial = x.initial.expect("a CD-ROM install boots the media first");
        assert_eq!(boot_devs(&initial), ["cdrom", "hd"]);
        assert!(initial.contains("<on_reboot>destroy</on_reboot>"));
        assert!(initial.contains(ISO));

        // The installed guest boots its disk, with the media gone.
        assert_eq!(boot_devs(&x.final_xml), ["hd"]);
        assert!(!x.final_xml.contains("on_reboot"));
        assert!(!x.final_xml.contains(ISO));
    }

    /// The customize dialog's path: the same install XML, built from
    /// the guest's XML.
    fn install_from_xml(mut g: NewGuest, inst: Installer) -> InstallXml {
        let domcaps = DomainCapabilities::parse(DOMCAPS).unwrap();
        inst.set_install_defaults(&mut g, &conn(), &caps(), &domcaps)
            .unwrap();
        inst.build_xml_from_xml(&g.to_xml(&conn()), g.os.is_windows)
            .unwrap()
    }

    #[test]
    fn customized_cdrom_install_is_two_phase() {
        let x = install_from_xml(guest(os("fedora41"), true), cdrom());
        let initial = x.initial.expect("a CD-ROM install boots the media first");
        assert_eq!(boot_devs(&initial), ["cdrom", "hd"]);
        assert!(initial.contains("<on_reboot>destroy</on_reboot>"));
        assert!(initial.contains(ISO));
        assert_eq!(boot_devs(&x.final_xml), ["hd"]);
        assert!(!x.final_xml.contains("on_reboot"));
        assert!(!x.final_xml.contains(ISO));
    }

    #[test]
    fn customized_tree_install_boots_the_kernel() {
        let mut inst = Installer::for_location(None);
        inst.treemedia_bootconfig = Some(("/k".into(), "/i".into(), "console=ttyS0".into()));
        inst.install_memory = Some(2097152);
        let x = install_from_xml(guest(os("fedora41"), true), inst.clone());
        let initial = x.initial.unwrap();
        assert!(initial.contains("<kernel>/k</kernel>"));
        assert!(initial.contains("<initrd>/i</initrd>"));
        assert!(initial.contains("<cmdline>console=ttyS0</cmdline>"));
        assert!(initial.contains("<currentMemory>2097152</currentMemory>"));
        assert!(boot_devs(&initial).is_empty());
        assert!(!x.final_xml.contains("<kernel>"));
        // Same as the wizard's own path.
        let y = install(guest(os("fedora41"), true), inst);
        assert_eq!(boot_devs(&y.final_xml), boot_devs(&x.final_xml));
    }

    #[test]
    fn modern_linux_gets_q35_and_virtio() {
        let x = install(guest(os("fedora41"), true), cdrom());
        let f = &x.final_xml;
        assert_eq!(attr(f, &["os", "type"], "machine").as_deref(), Some("q35"));
        assert_eq!(
            attr(f, &["devices", "disk", "target"], "bus").as_deref(),
            Some("virtio")
        );
        assert_eq!(
            attr(f, &["devices", "interface", "model"], "type").as_deref(),
            Some("virtio")
        );
        assert_eq!(
            attr(f, &["devices", "video", "model"], "type").as_deref(),
            Some("virtio")
        );
        assert_eq!(
            attr(f, &["devices", "controller"], "model").as_deref(),
            Some("qemu-xhci")
        );
        // TCG: the app default is only for KVM.
        assert_eq!(attr(f, &["cpu"], "mode").as_deref(), Some("maximum"));
        assert!(f.contains("http://fedoraproject.org/fedora/41"));
        assert!(f.contains("<rng model=\"virtio\">"));
        assert!(f.contains("org.qemu.guest_agent.0"));
    }

    #[test]
    fn generic_os_falls_back_to_legacy_devices() {
        let pxe = Installer {
            install_bootdev: Some("network".into()),
            ..Default::default()
        };
        let x = install(guest(OsFacts::default(), true), pxe);
        assert_eq!(boot_devs(x.initial.as_deref().unwrap()), ["network", "hd"]);
        let f = &x.final_xml;
        assert_eq!(
            attr(f, &["os", "type"], "machine").as_deref(),
            Some("pc-i440fx-9.0")
        );
        assert_eq!(
            attr(f, &["devices", "disk", "target"], "bus").as_deref(),
            Some("ide")
        );
        assert_eq!(
            attr(f, &["devices", "interface", "model"], "type").as_deref(),
            Some("e1000")
        );
        assert!(!f.contains("libosinfo"));
    }

    #[test]
    fn manual_install_is_one_phase() {
        let x = install(guest(os("fedora41"), true), Installer::default());
        assert!(x.initial.is_none());
        assert_eq!(boot_devs(&x.final_xml), ["hd"]);
    }

    #[test]
    fn windows_gets_hyperv_and_localtime() {
        let x = install(guest(os("win11"), true), cdrom());
        let f = &x.final_xml;
        assert_eq!(attr(f, &["clock"], "offset").as_deref(), Some("localtime"));
        assert!(f.contains("<hyperv"));
        assert_eq!(attr(f, &["os"], "firmware").as_deref(), Some("efi"));
        assert_eq!(
            attr(f, &["devices", "disk", "target"], "bus").as_deref(),
            Some("sata")
        );
    }
}
