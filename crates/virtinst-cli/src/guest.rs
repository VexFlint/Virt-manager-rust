//! `Guest.set_defaults` and `set_capabilities_defaults` from upstream's
//! `virtinst/guest.py`, on the parser engine's guest object, with the
//! `set_defaults` of the guest's own objects (`DomainClock`,
//! `DomainCpu`, `DomainFeatures`, `DomainPm`, `DomainOs`). Device
//! defaults come from [`crate::parsers::set_defaults`].

use virtinst_core::capabilities::caps::{Capabilities, CapsInfo};
use virtinst_core::capabilities::domcaps::DomainCapabilities;
use virtinst_core::connection::Connection;
use virtinst_core::devices;
use virtinst_core::domain::cpu::{self as dcpu, CpuEditContext};
use virtinst_core::install::newguest::OsFacts;
use virtinst_core::osinfo::OsDb;
use virtinst_core::xmlbuilder::{Value, XmlClass, XmlObject};

use std::rc::Rc;

use virtinst_core::diskbackend::StorageBackend;

use crate::parser::{conn_support, lookup_prop, ConnSupport, GuestFlags};
use crate::parsers;

/// The `Guest` attributes that aren't XML (`skip_default_*`,
/// `uefi_requested`, ...), as virt-install sets them.
#[derive(Clone)]
pub struct GuestOpts {
    pub skip_default_console: bool,
    pub skip_default_channel: bool,
    pub skip_default_sound: bool,
    pub skip_default_usbredir: bool,
    pub skip_default_graphics: bool,
    pub skip_default_rng: bool,
    pub skip_default_tpm: bool,
    pub skip_default_input: bool,
    pub uefi_requested: Option<bool>,
    /// `x86_cpu_default`.
    pub x86_cpu_default: &'static str,
    /// `num_pcie_root_ports`.
    pub num_pcie_root_ports: i64,
    /// `cpu.special_mode_was_set`.
    pub cpu_special_mode_was_set: bool,
    /// Set by `_add_default_tpm`.
    pub have_default_tpm: bool,
    /// Each `<disk>`'s storage backend, in the guest's disk order
    /// (`None`: a stub), for `DeviceDisk.set_defaults`.
    pub disk_backends: Vec<Option<Rc<StorageBackend>>>,
}

impl GuestOpts {
    /// The defaults with what the parsers set ([`GuestFlags`]).
    pub fn from_flags(f: &GuestFlags) -> GuestOpts {
        let mut o = GuestOpts {
            skip_default_console: f.skip_default_console,
            skip_default_channel: f.skip_default_channel,
            skip_default_sound: f.skip_default_sound,
            skip_default_usbredir: f.skip_default_usbredir,
            skip_default_graphics: f.skip_default_graphics,
            skip_default_rng: f.skip_default_rng,
            skip_default_tpm: f.skip_default_tpm,
            skip_default_input: f.skip_default_input,
            uefi_requested: f.uefi_requested,
            cpu_special_mode_was_set: f.cpu_special_mode_was_set,
            ..GuestOpts::default()
        };
        if let Some(n) = f.num_pcie_root_ports {
            o.num_pcie_root_ports = n;
        }
        o
    }
}

impl Default for GuestOpts {
    fn default() -> GuestOpts {
        GuestOpts {
            skip_default_console: false,
            skip_default_channel: false,
            skip_default_sound: false,
            skip_default_usbredir: false,
            skip_default_graphics: false,
            skip_default_rng: false,
            skip_default_tpm: false,
            skip_default_input: false,
            uefi_requested: None,
            x86_cpu_default: dcpu::SPECIAL_MODE_APP_DEFAULT,
            // qemu 6.1, fairly new when we added this option, has an unfortunate
            // bug with >= 15 root ports, so we choose 14 instead of our original 16
            // https://gitlab.com/qemu-project/qemu/-/issues/641
            num_pcie_root_ports: 14,
            cpu_special_mode_was_set: false,
            have_default_tpm: false,
            disk_backends: vec![],
        }
    }
}

fn s(v: &str) -> Option<Value> {
    Some(Value::Str(v.to_string()))
}

fn falsy(v: Option<String>) -> bool {
    v.map_or(true, |v| v.is_empty())
}

// `DomainOs` predicates, on the guest's `<os>`.

fn os_str(guest: &XmlObject, name: &str) -> Option<String> {
    guest.child("os").get_str(name)
}
fn arch(guest: &XmlObject) -> String {
    os_str(guest, "arch").unwrap_or_default()
}
fn machine(guest: &XmlObject) -> String {
    os_str(guest, "machine").unwrap_or_default()
}
fn is_hvm(guest: &XmlObject) -> bool {
    os_str(guest, "os_type").as_deref() == Some("hvm")
}
fn is_xenpv(guest: &XmlObject) -> bool {
    matches!(
        os_str(guest, "os_type").as_deref(),
        Some("xen" | "linux" | "xenpvh")
    )
}
fn is_container(guest: &XmlObject) -> bool {
    os_str(guest, "os_type").as_deref() == Some("exe")
}
fn is_x86(guest: &XmlObject) -> bool {
    matches!(arch(guest).as_str(), "x86_64" | "i686")
}
fn is_q35(guest: &XmlObject) -> bool {
    is_x86(guest) && machine(guest).contains("q35")
}
fn is_arm(guest: &XmlObject) -> bool {
    matches!(arch(guest).as_str(), "armv7l" | "aarch64")
}
fn is_arm_machvirt(guest: &XmlObject) -> bool {
    is_arm(guest) && machine(guest).starts_with("virt")
}
fn is_ppc64(guest: &XmlObject) -> bool {
    matches!(arch(guest).as_str(), "ppc64" | "ppc64le")
}
fn is_pseries(guest: &XmlObject) -> bool {
    is_ppc64(guest) && machine(guest).starts_with("pseries")
}
fn is_s390x(guest: &XmlObject) -> bool {
    arch(guest) == "s390x"
}
fn is_riscv_virt(guest: &XmlObject) -> bool {
    matches!(arch(guest).as_str(), "riscv64" | "riscv32") && machine(guest).starts_with("virt")
}
fn is_loongarch64(guest: &XmlObject) -> bool {
    arch(guest) == "loongarch64"
}

fn guest_type(guest: &XmlObject) -> String {
    guest.get_str("type").unwrap_or_default()
}

fn devices(guest: &XmlObject, name: &str) -> usize {
    guest.child("devices").children(name).len()
}

fn add_device(guest: &mut XmlObject, dev: XmlObject) {
    guest.child_mut("devices").add_child(dev);
}

fn new_device(class: &'static XmlClass, props: &[(&str, Option<Value>)]) -> XmlObject {
    let mut dev = XmlObject::new(class);
    for (name, val) in props {
        dev.set(name, val.clone())
            .expect("default device values convert");
    }
    dev
}

/// `Guest.is_uefi`.
fn is_uefi(guest: &XmlObject) -> bool {
    let os = guest.child("os");
    if !falsy(os.get_str("loader")) && os.get_str("loader_type").as_deref() == Some("pflash") {
        return true;
    }
    os.get_str("firmware").as_deref() == Some("efi")
}

/// `Guest.is_full_os_container`.
fn is_full_os_container(guest: &XmlObject) -> bool {
    if !is_container(guest) {
        return false;
    }
    guest
        .child("devices")
        .children("filesystem")
        .iter()
        .any(|fs| fs.get_str("target_dir").as_deref() == Some("/"))
}

/// `guest.osinfo`: the libosinfo OS the metadata names, else generic.
fn osinfo(guest: &XmlObject) -> OsFacts {
    let id = guest.child("_metadata").child("libosinfo").get_str("os_id");
    id.and_then(|id| OsDb::load_default().ok()?.os(&id))
        .map(|os| OsFacts::from_osinfo(&os, &arch(guest)))
        .unwrap_or_default()
}

/// `guest.osinfo.is_windows()`.
pub fn osinfo_is_windows(guest: &XmlObject) -> bool {
    osinfo(guest).is_windows
}

/// The guest's libosinfo OS (`guest.osinfo`), `None` for generic.
pub fn osinfo_obj(guest: &XmlObject) -> Option<virtinst_core::osinfo::OsInfo> {
    let id = guest
        .child("_metadata")
        .child("libosinfo")
        .get_str("os_id")?;
    OsDb::load_default().ok()?.os(&id)
}

/// `guest.osinfo.name`.
pub fn osinfo_name(guest: &XmlObject) -> String {
    osinfo_obj(guest).map_or("generic".into(), |o| o.short_id())
}

/// `Guest.set_os_name`.
pub fn set_os_name(guest: &mut XmlObject, name: &str) -> Result<(), String> {
    let full_id = if name == "generic" {
        None
    } else {
        let db = OsDb::load_default().map_err(|e| e.to_string())?;
        let os = db.lookup_os(name).ok_or_else(|| {
            format!("Unknown OS name '{name}'. See `--osinfo list` for valid values.")
        })?;
        log::debug!("Setting Guest osinfo name {}", os.short_id());
        Some(os.id())
    };
    if full_id.is_none() {
        log::debug!("Setting Guest osinfo name generic");
    }
    guest.set_path("_metadata.libosinfo.os_id", full_id.map(Value::Str))
}

/// `Guest.validate_name`.
pub fn validate_name(conn: &Connection, name: &str) -> Result<(), String> {
    // `XMLBuilder.validate_generic_name`: only character that shouldn't
    // work is '/', matching QEMU
    if name.is_empty() {
        return Err("A name must be specified for the Guest".into());
    }
    if name.contains('/') {
        return Err(format!(
            "Guest name '{name}' can not contain '/' character."
        ));
    }
    if virt::domain::Domain::lookup_by_name(conn.as_connect(), name).is_ok() {
        return Err(format!("Guest name '{name}' is already in use."));
    }
    Ok(())
}

/// `Guest.generate_name`.
pub fn generate_name(guest: &XmlObject, conn: &Connection, caps: &Capabilities) -> String {
    virtinst_core::guest::generate_name(
        &osinfo_name(guest),
        is_container(guest),
        &arch(guest),
        caps.host.cpu.arch.as_deref(),
        |n| virt::domain::Domain::lookup_by_name(conn.as_connect(), n).is_ok(),
    )
}

/// `Guest._supports_virtio`.
fn supports_virtio(guest: &XmlObject, support: &ConnSupport, os_support: bool) -> bool {
    if !support.is_qemu {
        return false;
    }
    // These _only_ support virtio so don't check the OS
    if is_riscv_virt(guest) || is_s390x(guest) || is_pseries(guest) || is_loongarch64(guest) {
        return true;
    }
    os_support
}

/// `Guest.lookup_domcaps`.
fn lookup_domcaps(guest: &XmlObject, conn: &Connection) -> DomainCapabilities {
    parsers::lookup_domcaps(&parsers::guest_info(guest), Some(conn))
        .expect("an empty domcaps parses")
}

/// `Guest.lookup_capsinfo`.
fn lookup_capsinfo(guest: &XmlObject, caps: &Capabilities) -> Result<CapsInfo, String> {
    caps.guest_lookup(
        os_str(guest, "os_type").as_deref(),
        os_str(guest, "arch").as_deref(),
        guest.get_str("type").as_deref(),
        os_str(guest, "machine").as_deref(),
    )
}

/// `Guest.set_capabilities_defaults`.
pub fn set_capabilities_defaults(
    guest: &mut XmlObject,
    caps: &Capabilities,
    support: &ConnSupport,
) -> Result<(), String> {
    let capsinfo = lookup_capsinfo(guest, caps)?;
    let wants_default_type = guest.get("type").is_none() && os_str(guest, "os_type").is_none();

    guest.set("type", capsinfo.hypervisor_type.clone().map(Value::Str))?;
    let os = guest.child_mut("os");
    os.set("os_type", capsinfo.os_type.clone().map(Value::Str))?;
    os.set("arch", capsinfo.arch.clone().map(Value::Str))?;
    if falsy(os.get_str("loader")) {
        os.set("loader", capsinfo.loader.clone().map(Value::Str))?;
    }
    if falsy(guest.get_str("emulator")) && !is_xenpv(guest) && guest_type(guest) != "vz" {
        guest.set("emulator", capsinfo.emulator.clone().map(Value::Str))?;
    }
    if falsy(os_str(guest, "machine")) {
        let m = capsinfo.recommended_machine(support.is_qemu || support.is_test);
        guest.child_mut("os").set("machine", m.and_then(s))?;
    }

    if wants_default_type && support.is_qemu && is_x86(guest) && guest_type(guest) != "kvm" {
        log::warn!(
            "KVM acceleration not available, using '{}'",
            guest_type(guest)
        );
    }
    Ok(())
}

/// `Guest.generate_uuid`: a random UUID libvirt has no domain for.
fn generate_uuid(conn: &Connection) -> Option<String> {
    for _ in 0..256 {
        let uuid = virtinst_core::cloner::generate_uuid();
        if virt::domain::Domain::lookup_by_uuid_string(conn.as_connect(), &uuid).is_err() {
            return Some(uuid);
        }
    }
    log::error!("Failed to generate non-conflicting UUID");
    None
}

/// `_CPUTopology.total_vcpus`.
fn total_vcpus(topology: &XmlObject) -> i64 {
    ["sockets", "dies", "cores", "threads"]
        .iter()
        .map(|n| match topology.get(n) {
            Some(Value::Int(v)) if v != 0 => v,
            _ => 1,
        })
        .product()
}

/// `DomainCpu.has_topology`.
fn has_topology(cpu: &XmlObject) -> bool {
    !cpu.child("topology").get_xml().trim().is_empty()
}

/// `_CPUTopology.set_defaults_from_vcpus`.
fn topology_defaults_from_vcpus(topology: &mut XmlObject, vcpus: i64) -> Result<(), String> {
    // The hierarchy is sockets > dies > cores > threads.
    //
    // In real world silicon though it is rare to have
    // high socket/die counts, but common to have huge
    // core counts.
    //
    // Some OS will even refuse to use sockets over a
    // a certain count.
    //
    // Thus we prefer to expose cores to the guest rather
    // than sockets as the default for missing fields
    let unset = |t: &XmlObject, n: &str| !matches!(t.get(n), Some(Value::Int(v)) if v != 0);
    if unset(topology, "cores") {
        let v = vcpus / total_vcpus(topology);
        topology.set("cores", Some(Value::Int(v)))?;
    }
    if unset(topology, "sockets") {
        let v = vcpus / total_vcpus(topology);
        topology.set("sockets", Some(Value::Int(v)))?;
    }
    if unset(topology, "dies") {
        topology.set("dies", Some(Value::Int(1)))?;
    }
    if unset(topology, "threads") {
        let v = vcpus / total_vcpus(topology);
        topology.set("threads", Some(Value::Int(v)))?;
    }
    let total = total_vcpus(topology);
    if total != vcpus {
        let get = |n: &str| match topology.get(n) {
            Some(Value::Int(v)) => v,
            _ => 0,
        };
        return Err(format!(
            "Total CPUs implied by topology (sockets={} * dies={} * cores={} * threads={} == {total}) does not match vCPU count {vcpus}",
            get("sockets"),
            get("dies"),
            get("cores"),
            get("threads"),
        ));
    }
    Ok(())
}

/// `Guest.sync_vcpus_topology`: `<cpu>` topology count and `<vcpus>`
/// always need to match.
pub fn sync_vcpus_topology(guest: &mut XmlObject, def_cpus: i64) -> Result<(), String> {
    let vcpus = match guest.get("_vcpus") {
        Some(Value::Int(v)) if v != 0 => v,
        _ => {
            let v = if has_topology(guest.child("cpu")) {
                total_vcpus(guest.child("cpu").child("topology"))
            } else {
                def_cpus
            };
            virtinst_core::guest::set_vcpus(guest, Some(v))?;
            v
        }
    };
    // `cpu.set_topology_defaults(vcpus)`
    if has_topology(guest.child("cpu")) {
        topology_defaults_from_vcpus(guest.child_mut("cpu").child_mut("topology"), vcpus)?;
    }
    Ok(())
}

/// `Guest._set_default_machine`.
fn set_default_machine(
    guest: &mut XmlObject,
    caps: &Capabilities,
    support: &ConnSupport,
    os: &OsFacts,
) -> Result<(), String> {
    if !falsy(os_str(guest, "machine")) {
        return Ok(());
    }
    let capsinfo = lookup_capsinfo(guest, caps)?;
    if is_x86(guest)
        && support.is_qemu
        && capsinfo.machines.iter().any(|m| m == "q35")
        && os.chipset_q35
    {
        return guest.child_mut("os").set("machine", s("q35"));
    }
    let default = capsinfo.machines.first().cloned();
    guest
        .child_mut("os")
        .set("machine", default.map(Value::Str))
}

/// `Guest.prefers_uefi`.
fn prefers_uefi(guest: &XmlObject, support: &ConnSupport, os: &OsFacts) -> bool {
    let prefer_efi = if is_x86(guest) && (support.is_qemu || support.is_test) {
        // If OS has dropped support for 'bios', we have no
        // choice but to use EFI.
        // For other OS still prefer BIOS since it is faster
        // and doesn't break QEMU internal snapshots
        os.requires_efi
    } else {
        is_arm_machvirt(guest) || is_riscv_virt(guest) || is_loongarch64(guest) || support.is_bhyve
    };
    log::debug!(
        "Prefer EFI => {}",
        if prefer_efi { "True" } else { "False" }
    );
    prefer_efi
}

/// Runs `f` with the guest's `<os>` taken out of it, the way the
/// parsers hold it.
fn with_os<R>(guest: &mut XmlObject, f: impl FnOnce(&mut XmlObject, &mut XmlObject) -> R) -> R {
    let mut taken = std::mem::take(guest.children_mut("os"));
    let r = f(guest, &mut taken[0]);
    *guest.children_mut("os") = taken;
    r
}

/// `Guest._set_default_uefi`.
fn set_default_uefi(
    guest: &mut XmlObject,
    opts: &GuestOpts,
    support: &ConnSupport,
    os: &OsFacts,
) -> Result<(), String> {
    if opts.uefi_requested == Some(false) {
        return Ok(());
    }
    let o = guest.child("os");
    let use_default_uefi = prefers_uefi(guest, support, os)
        && falsy(o.get_str("kernel"))
        && falsy(o.get_str("loader"))
        && o.get("loader_ro").is_none()
        && o.get("nvram").is_none()
        && o.get("firmware").is_none();
    if !use_default_uefi && opts.uefi_requested != Some(true) {
        return Ok(());
    }
    match with_os(guest, parsers::boot::enable_uefi) {
        Ok(()) => Ok(()),
        Err(e) if opts.uefi_requested == Some(true) => Err(e),
        Err(e) => {
            log::debug!("Error setting UEFI default");
            log::warn!("Couldn't configure UEFI: {e}");
            log::warn!("Your VM may not boot successfully.");
            Ok(())
        }
    }
}

/// `Guest._usb_disabled`.
fn usb_disabled(guest: &XmlObject) -> bool {
    let controllers: Vec<&XmlObject> = guest
        .child("devices")
        .children("controller")
        .iter()
        .filter(|c| c.get_str("type").as_deref() == Some("usb"))
        .collect();
    if controllers.is_empty() {
        return false;
    }
    controllers
        .iter()
        .all(|c| c.get_str("model").as_deref() == Some("none"))
}

/// `Guest._add_default_input_device`.
fn add_default_input_device(
    guest: &mut XmlObject,
    opts: &GuestOpts,
    os: &OsFacts,
) -> Result<(), String> {
    if opts.skip_default_input
        || is_container(guest)
        || is_xenpv(guest)
        || devices(guest, "input") > 0
        || devices(guest, "graphics") == 0
    {
        return Ok(());
    }
    let tablet = true;
    // We don't historically add USB keyboard for x86,
    // default libvirt/qemu PS2 seems to be fine
    let keyboard = !is_x86(guest);

    let bus = if is_s390x(guest) {
        // s390x guests need VirtIO input devices
        os.virtioinput.then_some("virtio")
    } else if !usb_disabled(guest) {
        Some("usb")
    } else {
        None
    };
    let Some(bus) = bus else {
        return Ok(());
    };
    let mut add_input = |itype: &str| -> Result<(), String> {
        let mut dev = new_device(
            &devices::input::DEVICE_INPUT,
            &[("type", s(itype)), ("bus", s(bus))],
        );
        devices::input::set_defaults(&mut dev)?;
        add_device(guest, dev);
        Ok(())
    };
    if tablet {
        add_input("tablet")?;
    }
    if keyboard {
        add_input("keyboard")?;
    }
    Ok(())
}

/// `Guest._add_default_console_device`.
fn add_default_console_device(
    guest: &mut XmlObject,
    opts: &GuestOpts,
    support: &ConnSupport,
    conn: &Connection,
) {
    if opts.skip_default_console || devices(guest, "console") > 0 || devices(guest, "serial") > 0 {
        return;
    }
    let mut dev = XmlObject::new(&devices::char::DEVICE_CONSOLE);
    if support.is_bhyve {
        let prefix = format!("/dev/nmdm{}", generate_uuid(conn).unwrap_or_default());
        let _ = dev.set("type", s("nmdm"));
        let src = dev.child_mut("source");
        let _ = src.set("master", s(&format!("{prefix}A")));
        let _ = src.set("slave", s(&format!("{prefix}B")));
    } else {
        let _ = dev.set("type", s("pty"));
    }
    if is_s390x(guest) {
        let _ = dev.set("target_type", s("sclp"));
    }
    add_device(guest, dev);
}

/// `Guest._add_default_video_device`.
fn add_default_video_device(guest: &mut XmlObject) {
    if is_container(guest) || devices(guest, "video") > 0 || devices(guest, "graphics") == 0 {
        return;
    }
    add_device(guest, XmlObject::new(&devices::video::DEVICE_VIDEO));
}

/// `DeviceController.get_usb3_controller`.
fn usb3_controller(support: &ConnSupport) -> XmlObject {
    let model = if support.qemu_xhci {
        "qemu-xhci"
    } else {
        "nec-xhci"
    };
    let mut ctrl = new_device(
        &devices::controller::DEVICE_CONTROLLER,
        &[("type", s("usb")), ("model", s(model))],
    );
    if support.usb3_ports {
        // 15 is the max ports qemu supports, might as well
        // Add as many as possible
        let _ = ctrl.set("ports", Some(Value::Int(15)));
    }
    ctrl
}

/// `Guest._add_default_usb_controller`.
fn add_default_usb_controller(guest: &mut XmlObject, support: &ConnSupport, os: &OsFacts) {
    if guest
        .child("devices")
        .children("controller")
        .iter()
        .any(|c| c.get_str("type").as_deref() == Some("usb"))
    {
        return;
    }
    if !support.is_qemu && !support.is_test {
        return;
    }
    let qemu_usb3 = support.qemu_xhci;
    let mut usb2 = false;
    let mut usb3 = false;
    if is_x86(guest) {
        usb3 = os.usb3 && qemu_usb3;
        usb2 = !usb3;
    } else if is_arm_machvirt(guest) || is_riscv_virt(guest) {
        // For machvirt, we always assume OS supports usb3
        usb3 = qemu_usb3;
    } else if is_pseries(guest) || is_loongarch64(guest) {
        // For pseries, we always assume OS supports usb3
        usb3 = qemu_usb3;
    }

    if usb2 {
        for (model, startport) in [
            ("ich9-ehci1", None),
            ("ich9-uhci1", Some(0)),
            ("ich9-uhci2", Some(2)),
            ("ich9-uhci3", Some(4)),
        ] {
            let mut ctrl = new_device(
                &devices::controller::DEVICE_CONTROLLER,
                &[("type", s("usb")), ("model", s(model))],
            );
            if let Some(port) = startport {
                let _ = ctrl.set("master_startport", Some(Value::Int(port)));
            }
            add_device(guest, ctrl);
        }
    } else if usb3 {
        add_device(guest, usb3_controller(support));
    }
}

/// `Guest._add_default_channels`.
fn add_default_channels(
    guest: &mut XmlObject,
    opts: &GuestOpts,
    support: &ConnSupport,
    os: &OsFacts,
) {
    if opts.skip_default_channel || devices(guest, "channel") > 0 {
        return;
    }
    if support.is_qemu && supports_virtio(guest, support, os.virtioserial) {
        let dev = new_device(
            &devices::char::DEVICE_CHANNEL,
            &[
                ("type", s("unix")),
                ("target_type", s("virtio")),
                ("target_name", s("org.qemu.guest_agent.0")),
            ],
        );
        add_device(guest, dev);
    }
}

/// `Guest._add_default_graphics`.
fn add_default_graphics(guest: &mut XmlObject, opts: &GuestOpts, conn: &Connection) {
    if opts.skip_default_graphics || devices(guest, "graphics") > 0 {
        return;
    }
    if is_container(guest) && !conn.uri().is_vz() {
        return;
    }
    if !is_x86(guest)
        && !is_pseries(guest)
        && !is_loongarch64(guest)
        && !is_arm_machvirt(guest)
        && !is_riscv_virt(guest)
    {
        return;
    }
    add_device(guest, XmlObject::new(&devices::graphics::DEVICE_GRAPHICS));
}

/// The arches `_add_default_rng`/`_add_default_memballoon` know these
/// devices are good to have on.
fn rng_balloon_arch(guest: &XmlObject) -> bool {
    is_x86(guest)
        || is_arm_machvirt(guest)
        || is_riscv_virt(guest)
        || is_s390x(guest)
        || is_pseries(guest)
        || is_loongarch64(guest)
}

/// `Guest._add_default_rng`.
fn add_default_rng(guest: &mut XmlObject, opts: &GuestOpts, support: &ConnSupport, os: &OsFacts) {
    if opts.skip_default_rng || devices(guest, "rng") > 0 || !rng_balloon_arch(guest) {
        return;
    }
    if support.is_qemu && os.virtiorng {
        let dev = new_device(
            &devices::rng::DEVICE_RNG,
            &[
                ("backend_model", s("random")),
                ("device", s("/dev/urandom")),
            ],
        );
        add_device(guest, dev);
    }
}

/// `Guest._add_default_tpm`.
fn add_default_tpm(guest: &mut XmlObject, opts: &mut GuestOpts, conn: &Connection) {
    if opts.skip_default_tpm || devices(guest, "tpm") > 0 {
        return;
    }
    // If the guest is using UEFI, we take that as a
    // flag that the VM is targeting a modern platform
    // and thus we should also provide an emulated TPM.
    if !is_uefi(guest) {
        return;
    }
    if !lookup_domcaps(guest, conn).supports_tpm_emulator() {
        log::debug!("Domain caps doesn't report TPM support");
        return;
    }
    log::debug!("Adding default TPM");
    let dev = new_device(&devices::tpm::DEVICE_TPM, &[("type", s("emulator"))]);
    add_device(guest, dev);
    opts.have_default_tpm = true;
}

/// `Guest._add_default_memballoon`.
fn add_default_memballoon(guest: &mut XmlObject, support: &ConnSupport, os: &OsFacts) {
    if devices(guest, "memballoon") > 0 || !support.is_qemu {
        return;
    }
    // We know for certain that a memballoon is good to have with these
    // machine types; for other machine types, we leave the decision up
    // to libvirt
    if !rng_balloon_arch(guest) {
        return;
    }
    if os.virtioballoon {
        let dev = new_device(
            &devices::memballoon::DEVICE_MEMBALLOON,
            &[("model", s("virtio"))],
        );
        add_device(guest, dev);
    }
}

/// `DomainClock.set_defaults`.
pub fn clock_set_defaults(
    guest: &mut XmlObject,
    support: &ConnSupport,
    os: &OsFacts,
) -> Result<(), String> {
    if !is_hvm(guest) {
        return Ok(());
    }
    let x86 = is_x86(guest);
    let clock = guest.child_mut("clock");
    if clock.get("offset").is_none() {
        clock.set("offset", s(os.clock.unwrap_or("utc")))?;
    }
    if !clock.children("timers").is_empty() || !x86 || !support.is_qemu {
        return Ok(());
    }

    // Set clock policy that maps to qemu options:
    //   -no-hpet -no-kvm-pit-reinjection -rtc driftfix=slew
    //
    // hpet: Is unneeded and has a performance penalty
    // pit: While it has no effect on windows, it doesn't hurt and
    //   is beneficial for linux
    //
    // If libvirt/qemu supports it and using a windows VM, also
    // specify hypervclock.
    //
    // This is what has been recommended by the RH qemu guys :)
    let timer = |name: &str, prop: &str, val: Value| {
        let mut t = XmlObject::new(&virtinst_core::domain::clock::CLOCK_TIMER);
        t.set("name", s(name)).expect("a timer name converts");
        t.set(prop, Some(val)).expect("a timer value converts");
        t
    };
    clock.add_child(timer("rtc", "tickpolicy", Value::Str("catchup".into())));
    clock.add_child(timer("pit", "tickpolicy", Value::Str("delay".into())));
    clock.add_child(timer("hpet", "present", Value::Bool(false)));
    if os.is_windows {
        clock.add_child(timer("hypervclock", "present", Value::Bool(true)));
    }
    Ok(())
}

/// `DomainCpu.set_defaults`.
pub fn cpu_set_defaults(
    guest: &mut XmlObject,
    opts: &GuestOpts,
    support: &ConnSupport,
    caps: &Capabilities,
    conn: &Connection,
) -> Result<(), String> {
    if !support.is_test && !support.is_qemu {
        return Ok(());
    }
    if opts.cpu_special_mode_was_set {
        return Ok(());
    }
    let cpu = guest.child("cpu");
    if !falsy(cpu.get_str("model")) || !falsy(cpu.get_str("mode")) {
        return Ok(());
    }

    let typ = guest_type(guest);
    if is_arm_machvirt(guest) && typ == "kvm" {
        return guest
            .child_mut("cpu")
            .set("mode", s(dcpu::SPECIAL_MODE_HOST_PASSTHROUGH));
    }
    let domcaps = lookup_domcaps(guest, conn);
    let info = parsers::guest_info(guest);
    let ctx = || CpuEditContext {
        app_default_mode: domcaps
            .app_default_cpu_mode(
                info.domain_type.as_deref(),
                info.arch.as_deref(),
                info.machine.as_deref(),
            )
            .to_string(),
        host_cpu_model: caps.host.cpu.model.clone(),
        security_features: domcaps.lookup_cpu_security_features(conn.as_connect()),
    };
    if is_x86(guest) && typ == "kvm" {
        // `_set_cpu_x86_kvm_default`
        if Some(arch(guest)) != caps.host.cpu.arch {
            return Ok(());
        }
        let mode = opts.x86_cpu_default;
        let cctx = ctx();
        let cpu = guest.child_mut("cpu");
        dcpu::set_special_mode(cpu, &cctx, mode, true)?;
        let resolved = if mode == dcpu::SPECIAL_MODE_APP_DEFAULT {
            cctx.app_default_mode.as_str()
        } else {
            mode
        };
        if resolved == dcpu::SPECIAL_MODE_HOST_MODEL_ONLY {
            // `_validate_default_host_model_only`: it's possible that the
            // value HOST_MODEL_ONLY gets from <capabilities> is not
            // actually supported by qemu/kvm combo which will be
            // reported in <domainCapabilities>
            let Some(model) = cpu.get_str("model") else {
                return Ok(());
            };
            let Some(mode) = domcaps.cpu.get_mode("custom") else {
                return Ok(());
            };
            if mode
                .get_model(&model)
                .is_some_and(|m| m.usable.as_deref() != Some("no"))
            {
                return Ok(());
            }
            log::debug!(
                "Host capabilities CPU '{model}' is not supported according to domain capabilities. Unsetting CPU model"
            );
            cpu.set("model", None)?;
        }
        return Ok(());
    }
    // Prefer to emulate a feature-rich CPU instead of a basic one
    if domcaps.supports_maximum_cpu_mode()
        && typ == "qemu"
        && (is_x86(guest)
            || is_arm_machvirt(guest)
            || is_riscv_virt(guest)
            || is_loongarch64(guest))
    {
        let cctx = ctx();
        dcpu::set_special_mode(
            guest.child_mut("cpu"),
            &cctx,
            dcpu::SPECIAL_MODE_MAXIMUM,
            true,
        )?;
    }
    Ok(())
}

/// `XMLBuilder._prop_is_unset`.
fn prop_is_unset(obj: &XmlObject, name: &str) -> bool {
    let set = match obj.get(name) {
        None => false,
        Some(Value::Bool(b)) => b,
        Some(Value::Int(i)) => i != 0,
        Some(Value::Str(s)) => !s.is_empty(),
    };
    !set && !obj.prop_in_store(name)
}

/// `DomainFeatures._set_hyperv_defaults`.
fn set_hyperv_defaults(
    guest: &mut XmlObject,
    caps: &Capabilities,
    conn: &Connection,
    os: &OsFacts,
) -> Result<(), String> {
    if !os.is_windows {
        return Ok(());
    }
    let features = lookup_domcaps(guest, conn).supported_hyperv_features();
    let has_hyperv_timer = guest
        .child("clock")
        .children("timers")
        .iter()
        .any(|t| t.get_str("name").as_deref() == Some("hypervclock"));
    let feats = guest.child_mut("features");
    let apic = feats.get("apic") == Some(Value::Bool(true));

    let mut enable = |name: &str, requires: &[&str], feature: Option<&str>, value: Value| {
        let feature = feature.unwrap_or(name);
        if !features.iter().any(|f| f == feature) {
            return Ok(());
        }
        let prop = format!("hyperv_{name}");
        if feats.get(&prop).is_some() {
            return Ok(());
        }
        for val in requires {
            if feats.get(&format!("hyperv_{val}")) != Some(Value::Bool(true)) {
                return Ok(());
            }
        }
        feats.set(&prop, Some(value))
    };
    let on = || Value::Bool(true);
    enable("relaxed", &[], None, on())?;
    enable("vapic", &[], None, on())?;
    enable("spinlocks", &[], None, on())?;
    enable(
        "spinlocks_retries",
        &[],
        Some("spinlocks"),
        Value::Int(8191),
    )?;
    enable("vpindex", &[], None, on())?;
    enable("runtime", &[], None, on())?;
    enable("synic", &["vpindex"], None, on())?;

    // Both hyperv_stimer and hyperv_stimer requires hv-timer to be enabled
    // which libvirt hides under hypervclock timer.
    if has_hyperv_timer {
        enable("stimer", &["vpindex", "synic"], None, on())?;
        enable("stimer_direct", &["vpindex", "synic", "stimer"], None, on())?;
    }

    enable("frequencies", &[], None, on())?;

    enable("tlbflush", &["vpindex"], None, on())?;
    enable("ipi", &["vpindex"], None, on())?;

    if caps.host.cpu.vendor.as_deref() == Some("Intel") {
        enable("evmcs", &["vapic"], None, on())?;
    }

    if apic {
        enable("avic", &[], None, on())?;
    }
    Ok(())
}

/// `DomainFeatures.set_defaults`.
pub fn features_set_defaults(
    guest: &mut XmlObject,
    caps: &Capabilities,
    conn: &Connection,
    os: &OsFacts,
) -> Result<(), String> {
    if is_container(guest) {
        let full = is_full_os_container(guest) && guest_type(guest) != "vz";
        let feats = guest.child_mut("features");
        feats.set("acpi", None)?;
        feats.set("apic", None)?;
        feats.set("pae", None)?;
        if full {
            feats.set("privnet", Some(Value::Bool(true)))?;
        }
        return Ok(());
    }
    if !is_hvm(guest) {
        return Ok(());
    }

    let capsinfo = lookup_capsinfo(guest, caps)?;
    let cfeats = caps
        .guest_for_info(&capsinfo)
        .map(|g| g.features.clone())
        .unwrap_or_default();
    let xen_x86_64 = guest_type(guest) == "xen" && arch(guest) == "x86_64";
    let feats = guest.child_mut("features");
    if prop_is_unset(feats, "acpi") {
        feats.set("acpi", Some(Value::Bool(cfeats.acpi == Some(true))))?;
    }
    if prop_is_unset(feats, "apic") {
        feats.set("apic", Some(Value::Bool(cfeats.apic == Some(true))))?;
    }
    if prop_is_unset(feats, "pae") {
        feats.set("pae", Some(Value::Bool(xen_x86_64 || cfeats.pae)))?;
    }
    set_hyperv_defaults(guest, caps, conn, os)
}

/// `DomainPm.set_defaults`: when the suspend feature is exposed to
/// VMs, an ACPI shutdown event triggers a suspend in the guest, which
/// causes a lot of user confusion.
pub fn pm_set_defaults(guest: &mut XmlObject, support: &ConnSupport) -> Result<(), String> {
    if is_x86(guest) && (support.is_qemu || support.is_test) {
        let pm = guest.child_mut("pm");
        if pm.get("suspend_to_mem").is_none() {
            pm.set("suspend_to_mem", Some(Value::Bool(false)))?;
        }
        if pm.get("suspend_to_disk").is_none() {
            pm.set("suspend_to_disk", Some(Value::Bool(false)))?;
        }
    }
    Ok(())
}

/// `DomainOs.set_defaults`.
pub fn os_set_defaults(guest: &mut XmlObject) -> Result<(), String> {
    if is_container(guest) && falsy(os_str(guest, "init")) {
        let init = if is_full_os_container(guest) {
            "/sbin/init"
        } else {
            "/bin/sh"
        };
        guest.child_mut("os").set("init", s(init))?;
    }
    Ok(())
}

/// The parser whose objects live at `guest_propname`.
fn parser_for(guest_propname: &str) -> Option<&'static crate::parser::ParserDef> {
    parsers::all()
        .into_iter()
        .find(|p| p.guest_propname == Some(guest_propname))
}

/// `dev.set_defaults(guest)` for the object at `guest_propname[idx]`.
pub fn device_set_defaults(
    guest: &mut XmlObject,
    guest_propname: &str,
    idx: usize,
    conn: &Connection,
) -> Result<(), String> {
    match parser_for(guest_propname) {
        Some(def) => parsers::set_defaults(def, guest, idx, Some(conn)),
        None => Ok(()),
    }
}

/// `dev.set_defaults(guest)` for every object at `guest_propname`.
fn objects_set_defaults(
    guest: &mut XmlObject,
    guest_propname: &str,
    opts: &GuestOpts,
    conn: &Connection,
) -> Result<(), String> {
    let Some(def) = parser_for(guest_propname) else {
        return Ok(());
    };
    for idx in 0..lookup_prop(guest, def).len() {
        if guest_propname == "devices.disk" {
            parsers::disk::set_backend_rc(opts.disk_backends.get(idx).cloned().flatten());
        }
        parsers::set_defaults(def, guest, idx, Some(conn))?;
    }
    Ok(())
}

/// `Guest.add_virtioscsi_controller`.
fn add_virtioscsi_controller(
    guest: &mut XmlObject,
    support: &ConnSupport,
    os: &OsFacts,
) -> Result<(), String> {
    let devs = guest.child("devices");
    let has_any_scsi = devs
        .children("controller")
        .iter()
        .any(|c| c.get_str("type").as_deref() == Some("scsi"));
    if has_any_scsi || !supports_virtio(guest, support, os.virtioscsi) {
        return Ok(());
    }
    if !devs
        .children("disk")
        .iter()
        .any(|d| d.get_str("bus").as_deref() == Some("scsi"))
    {
        return Ok(());
    }
    let ctrl = new_device(
        &devices::controller::DEVICE_CONTROLLER,
        &[("type", s("scsi")), ("model", s("virtio-scsi"))],
    );
    add_device(guest, ctrl);
    Ok(())
}

/// `Guest.defaults_to_pcie`.
fn defaults_to_pcie(guest: &XmlObject) -> bool {
    is_q35(guest) || is_arm_machvirt(guest) || is_riscv_virt(guest) || is_loongarch64(guest)
}

/// `Guest.add_q35_pcie_controllers`.
fn add_q35_pcie_controllers(guest: &mut XmlObject, opts: &GuestOpts) {
    if guest
        .child("devices")
        .children("controller")
        .iter()
        .any(|c| c.get_str("type").as_deref() == Some("pci"))
    {
        return;
    }
    if !defaults_to_pcie(guest) {
        return;
    }
    log::debug!("Using num_pcie_root_ports={}", opts.num_pcie_root_ports);
    let mut added = false;
    for _ in 0..opts.num_pcie_root_ports.max(0) {
        if !added {
            // Libvirt forces pcie-root to come first
            add_device(
                guest,
                new_device(
                    &devices::controller::DEVICE_CONTROLLER,
                    &[("type", s("pci")), ("model", s("pcie-root"))],
                ),
            );
            added = true;
        }
        add_device(
            guest,
            new_device(
                &devices::controller::DEVICE_CONTROLLER,
                &[("type", s("pci")), ("model", s("pcie-root-port"))],
            ),
        );
    }
}

/// `Guest._add_spice_devices`.
fn add_spice_devices(
    guest: &mut XmlObject,
    opts: &GuestOpts,
    support: &ConnSupport,
    conn: &Connection,
) -> Result<(), String> {
    if !parsers::guest_info(guest).has_spice {
        return Ok(());
    }
    if guest.child("features").get("vmport").is_none() && is_x86(guest) && support.vmport {
        guest
            .child_mut("features")
            .set("vmport", Some(Value::Bool(false)))?;
    }
    let domcaps = lookup_domcaps(guest, conn);

    // `_add_spice_channels`
    let has_spicevmc = guest
        .child("devices")
        .children("channel")
        .iter()
        .any(|c| c.get_str("type").as_deref() == Some("spicevmc"));
    if domcaps.supports_channel_spicevmc() && !opts.skip_default_channel && !has_spicevmc {
        // We explicitly don't check for virtioserial support here.
        // We did that for a while, which excluded windows, and
        // we received some complaints.
        // https://bugzilla.redhat.com/show_bug.cgi?id=1660123
        let mut dev = new_device(&devices::char::DEVICE_CHANNEL, &[("type", s("spicevmc"))]);
        devices::char::set_defaults(&mut dev)?;
        add_device(guest, dev);
    }

    // `_add_spice_sound`
    if !opts.skip_default_sound && devices(guest, "sound") == 0 && is_hvm(guest) {
        let mut dev = XmlObject::new(&devices::sound::DEVICE_SOUND);
        devices::sound::set_defaults(&mut dev, defaults_to_pcie(guest))?;
        add_device(guest, dev);
    }

    // `_add_spice_usbredir`
    if domcaps.supports_redirdev_usb()
        && !opts.skip_default_usbredir
        && devices(guest, "redirdev") == 0
        && !usb_disabled(guest)
        && is_x86(guest)
    {
        // If we use 4 devices here, we fill up all the emulated USB2 slots,
        // and directly assigned devices are forced to fall back to USB1
        // https://bugzilla.redhat.com/show_bug.cgi?id=1135488
        for _ in 0..2 {
            let mut dev = new_device(
                &devices::redirdev::DEVICE_REDIRDEV,
                &[("bus", s("usb")), ("type", s("spicevmc"))],
            );
            devices::redirdev::set_defaults(&mut dev)?;
            add_device(guest, dev);
        }
    }
    Ok(())
}

/// `Guest.add_virtioscsi_controller`, for callers outside
/// `set_defaults`.
pub fn ensure_virtioscsi_controller(guest: &mut XmlObject) -> Result<(), String> {
    let support = conn_support();
    let os = osinfo(guest);
    add_virtioscsi_controller(guest, &support, &os)
}

/// `Guest.is_uefi`.
pub fn guest_is_uefi(guest: &XmlObject) -> bool {
    is_uefi(guest)
}

/// `Guest.set_smbios_serial_cloudinit`.
pub fn set_smbios_serial_cloudinit(guest: &mut XmlObject) -> Result<(), String> {
    let support = conn_support();
    if !support.is_qemu && !support.is_test {
        return Ok(());
    }
    if !is_x86(guest) && !is_arm_machvirt(guest) {
        return Ok(());
    }
    if !matches!(
        guest.child("os").get_str("smbios_mode").as_deref(),
        None | Some("sysinfo")
    ) {
        return Ok(());
    }
    let idx = match guest
        .children("sysinfo")
        .iter()
        .position(|s| s.get_str("type").as_deref() == Some("smbios"))
    {
        Some(i) => i,
        None => {
            guest.add_child(XmlObject::new(
                &virtinst_core::domain::sysinfo::DOMAIN_SYSINFO,
            ));
            guest.children("sysinfo").len() - 1
        }
    };
    if !falsy(guest.children("sysinfo")[idx].get_str("system_serial")) {
        return Ok(());
    }
    guest.child_mut("os").set("smbios_mode", s("sysinfo"))?;
    let sysinfo = &mut guest.children_mut("sysinfo")[idx];
    sysinfo.set("type", s("smbios"))?;
    sysinfo.set("system_serial", s("ds=nocloud"))
}

/// `Guest.set_defaults`.
pub fn set_defaults(
    guest: &mut XmlObject,
    opts: &mut GuestOpts,
    conn: &Connection,
) -> Result<(), String> {
    let support = conn_support();
    let caps = Capabilities::fetch(conn).map_err(|e| e.to_string())?;
    set_capabilities_defaults(guest, &caps, &support)?;

    if falsy(guest.get_str("uuid")) {
        guest.set("uuid", generate_uuid(conn).map(Value::Str))?;
    }

    sync_vcpus_topology(guest, 1)?;

    let os = osinfo(guest);
    set_default_machine(guest, &caps, &support, &os)?;
    set_default_uefi(guest, opts, &support, &os)?;

    add_default_graphics(guest, opts, conn);
    add_default_video_device(guest);
    add_default_input_device(guest, opts, &os)?;
    add_default_console_device(guest, opts, &support, conn);
    add_default_usb_controller(guest, &support, &os);
    add_default_channels(guest, opts, &support, &os);
    add_default_rng(guest, opts, &support, &os);
    add_default_memballoon(guest, &support, &os);
    add_default_tpm(guest, opts, conn);

    clock_set_defaults(guest, &support, &os)?;
    cpu_set_defaults(guest, opts, &support, &caps, conn)?;
    features_set_defaults(guest, &caps, conn, &os)?;
    objects_set_defaults(guest, "seclabels", opts, conn)?;
    pm_set_defaults(guest, &support)?;
    os_set_defaults(guest)?;
    objects_set_defaults(guest, "launchSecurity", opts, conn)?;

    for name in virtinst_core::guest::DOMAIN_DEVICES.prop_order {
        objects_set_defaults(guest, &format!("devices.{name}"), opts, conn)?;
    }

    add_virtioscsi_controller(guest, &support, &os)?;
    add_q35_pcie_controllers(guest, opts);
    add_spice_devices(guest, opts, &support, conn)
}
