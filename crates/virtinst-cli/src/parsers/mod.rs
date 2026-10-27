//! The `VirtCLIParser` subclasses of upstream's `cli.py`, as
//! [`ParserDef`]s, grouped by device family. Each `parser_*` function
//! builds its parser once and hands out a shared reference.

use virtinst_core::capabilities::caps::Capabilities;
use virtinst_core::capabilities::domcaps::DomainCapabilities;
use virtinst_core::connection::Connection;
use virtinst_core::devices::build::GuestInfo;
use virtinst_core::devices::{sound, video};
use virtinst_core::guest::Guest;
use virtinst_core::osinfo::OsDb;
use virtinst_core::xmlbuilder::XmlObject;
use virtinst_xml::XmlBound;

use crate::parser::{lookup_prop, ParserDef};

pub mod chardev;
pub mod charsrc;
pub mod devices;
pub mod hostdev;
pub mod misc;
pub mod network;

/// Every ported parser, in upstream's registration order (the order
/// `VIRT_PARSERS` runs them in).
pub fn all() -> Vec<&'static ParserDef> {
    vec![
        network::network(),
        devices::controller(),
        devices::input(),
        chardev::serial(),
        chardev::parallel(),
        chardev::channel(),
        chardev::console(),
        hostdev::hostdev(),
        devices::sound(),
        devices::audio(),
        misc::watchdog(),
        devices::video(),
        devices::smartcard(),
        hostdev::redirdev(),
        misc::memballoon(),
        devices::tpm(),
        devices::rng(),
        misc::panic(),
        misc::shmem(),
        misc::memdev(),
        misc::iommu(),
        misc::pstore(),
    ]
}

/// The parser for `--name` (the `cli_arg_name`), if ported.
pub fn by_name(cli_arg_name: &str) -> Option<&'static ParserDef> {
    all().into_iter().find(|p| p.cli_arg_name == cli_arg_name)
}

/// The guest facts device defaults read (`guest.os`, its graphics,
/// `guest.osinfo`), from the guest's XML.
pub fn guest_info(guest: &XmlObject) -> GuestInfo {
    if guest.element().is_none() {
        return GuestInfo::default();
    }
    guest.with_doc(|doc, root| {
        let g = Guest::from_element(doc, root);
        let graphics = root
            .find(doc, "devices")
            .map(|d| d.find_all(doc, "graphics"))
            .unwrap_or_default();
        let has_spice = graphics
            .iter()
            .any(|g| g.attribute(doc, "type") == Some("spice"));
        let has_gl = graphics
            .iter()
            .any(|g| g.find(doc, "gl").and_then(|gl| gl.attribute(doc, "enable")) == Some("yes"));
        let os = &g.os;
        let is_uefi = (os.loader.is_some() && os.loader_type.as_deref() == Some("pflash"))
            || os.firmware.as_deref() == Some("efi");
        let osinfo = g
            .metadata
            .libosinfo
            .os_id
            .as_deref()
            .and_then(|id| OsDb::load_default().ok().and_then(|db| db.os(id)));
        GuestInfo {
            os_type: os.os_type.clone(),
            arch: os.arch.clone(),
            machine: os.machine.clone(),
            domain_type: g.domain_type.clone(),
            has_spice,
            has_gl,
            is_uefi,
            os_virtiogpu: osinfo.as_ref().is_some_and(|o| o.supports_virtiogpu()),
            os_virtionet: osinfo.as_ref().is_some_and(|o| o.supports_virtionet()),
            os_virtioscsi: osinfo.as_ref().is_some_and(|o| o.supports_virtioscsi()),
            os_netmodels: osinfo
                .as_ref()
                .map(|o| o.supported_netmodels())
                .unwrap_or_default(),
            spice_fallback: false,
        }
    })
}

/// `guest.lookup_domcaps()`: a stub when fetching fails.
fn lookup_domcaps(info: &GuestInfo, conn: Option<&Connection>) -> Option<DomainCapabilities> {
    let domcaps = conn.and_then(|c| {
        DomainCapabilities::fetch(
            c,
            None,
            info.arch.as_deref(),
            info.machine.as_deref(),
            info.domain_type.as_deref(),
        )
        .map_err(|e| log::debug!("Error fetching domcapabilities XML: {e}"))
        .ok()
    });
    domcaps.or_else(|| DomainCapabilities::parse("<domainCapabilities/>").ok())
}

/// `DeviceVideo.default_model(guest)`: `guest.lookup_domcaps()` is
/// fetched only when the answer needs it, a stub if that fails.
fn default_video_model(info: &mut GuestInfo, conn: Option<&Connection>) -> Option<&'static str> {
    let is_qemu = conn.is_some_and(|c| c.uri().is_qemu());
    let domcaps = lookup_domcaps(info, conn);
    // `supports_graphics_spice`'s fallback for domcaps without graphics.
    info.spice_fallback = conn.is_some_and(|c| {
        (c.uri().is_qemu() || c.uri().is_test())
            && Capabilities::fetch(c)
                .is_ok_and(|caps| matches!(caps.host.cpu.arch.as_deref(), Some("i686" | "x86_64")))
    });
    info.default_video_model(domcaps.as_ref(), is_qemu)
}

/// `DeviceInterface.generate_mac`: a random MAC that doesn't conflict
/// with any VMs on the connection.
fn generate_mac(conn: Option<&Connection>, is_qemu: bool) -> Option<String> {
    for _ in 0..256 {
        let mac = virtinst_core::cloner::generate_mac(is_qemu);
        match conn {
            Some(c) if virtinst_core::cloner::check_mac_in_use(c, &mac).is_err() => continue,
            _ => return Some(mac),
        }
    }
    log::debug!("Failed to generate non-conflicting MAC");
    None
}

/// `dev.set_defaults(guest)` for the `idx`th device a parser made.
pub fn set_defaults(
    def: &ParserDef,
    guest: &mut XmlObject,
    idx: usize,
    conn: Option<&Connection>,
) -> Result<(), String> {
    match def.cli_arg_name {
        "sound" => {
            let pcie = guest_info(guest).defaults_to_pcie();
            sound::set_defaults(&mut lookup_prop(guest, def)[idx], pcie)
        }
        "video" => {
            let unset = lookup_prop(guest, def)[idx]
                .get_str("_model")
                .map_or(true, |m| m.is_empty());
            let mut info = guest_info(guest);
            let model = if unset {
                default_video_model(&mut info, conn)
            } else {
                None
            };
            video::set_defaults(&mut lookup_prop(guest, def)[idx], model, info.has_gl)
        }
        "input" => virtinst_core::devices::input::set_defaults(&mut lookup_prop(guest, def)[idx]),
        "serial" | "parallel" | "channel" | "console" => {
            virtinst_core::devices::char::set_defaults(&mut lookup_prop(guest, def)[idx])
        }
        "hostdev" => virtinst_core::devices::hostdev::set_defaults(
            &mut lookup_prop(guest, def)[idx],
            crate::parser::conn_support().is_xen,
        ),
        "redirdev" => {
            virtinst_core::devices::redirdev::set_defaults(&mut lookup_prop(guest, def)[idx])
        }
        "rng" => virtinst_core::devices::rng::set_defaults(&mut lookup_prop(guest, def)[idx]),
        "smartcard" => {
            virtinst_core::devices::smartcard::set_defaults(&mut lookup_prop(guest, def)[idx])
        }
        "tpm" => {
            let info = guest_info(guest);
            virtinst_core::devices::tpm::set_defaults(&mut lookup_prop(guest, def)[idx], || {
                virtinst_core::devices::build::default_tpm_model(
                    lookup_domcaps(&info, conn).as_ref(),
                    &info,
                )
            })
        }
        "network" => {
            let info = guest_info(guest);
            let is_qemu = conn.is_some_and(|c| c.uri().is_qemu());
            let is_remote = conn.is_some_and(|c| c.uri().is_remote());
            let generate_mac = || generate_mac(conn, is_qemu);
            let default_bridge = || network::default_bridge(is_remote);
            let d = virtinst_core::devices::network::InterfaceDefaults {
                generate_mac: &generate_mac,
                default_bridge: &default_bridge,
                default_model: info.default_nic_model(is_qemu),
            };
            virtinst_core::devices::network::set_defaults(&mut lookup_prop(guest, def)[idx], &d)
        }
        name => misc::set_defaults(name, &mut lookup_prop(guest, def)[idx]),
    }
}
