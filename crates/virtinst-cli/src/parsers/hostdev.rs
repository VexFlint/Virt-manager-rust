//! `--hostdev` and `--redirdev`: `ParserHostdev` and `ParserRedir`.

use std::sync::OnceLock;

use virtinst_core::devices::{char as chardev, hostdev, redirdev};
use virtinst_core::nodedev::NodeDevice;
use virtinst_core::xmlbuilder::{Value, XmlObject};

use super::charsrc::add_char_source_args;
use crate::parser::{
    add_boot_order_arg, add_common_device_args, nodedevs, ArgDef, ArgInst, Error, ParseCtx,
    ParserDef,
};

/// Python's `int(val, 16)`.
fn int16(val: &str) -> Result<u64, Error> {
    let t = val.trim();
    let digits = t
        .strip_prefix("0x")
        .or_else(|| t.strip_prefix("0X"))
        .unwrap_or(t);
    u64::from_str_radix(digits, 16)
        .map_err(|_| format!("invalid literal for int() with base 16: '{val}'"))
}

/// `_AddressStringToHostdev`: decompose common USB and PCI address
/// string formats into a hostdev. The 3 expected formats are:
/// - bus.addr (ex. 001.003 for a usb device)
/// - vendor:product (ex. 0x1234:0x5678 for a usb device
/// - (domain:)bus:slot.func (ex. 00:10.0 for a pci device)
fn address_string_to_hostdev(addrstr: &str) -> Result<XmlObject, Error> {
    let mut dev = XmlObject::new(&hostdev::DEVICE_HOSTDEV);
    let colons = addrstr.matches(':').count();
    if (colons == 1 || colons == 2) && addrstr.contains('.') {
        // PCI address
        let (rest, func) = addrstr.split_once('.').expect("checked above");
        let Some((rest, slot)) = rest.rsplit_once(':') else {
            return Err("not enough values to unpack (expected 2, got 1)".into());
        };
        let (domain, bus) = rest.split_once(':').unwrap_or(("0", rest));
        dev.set("type", Some("pci".into()))?;
        dev.set("domain", Some(format!("0x{:04X}", int16(domain)?).into()))?;
        dev.set("function", Some(format!("0x{:02X}", int16(func)?).into()))?;
        dev.set("slot", Some(format!("0x{:02X}", int16(slot)?).into()))?;
        dev.set("bus", Some(format!("0x{:02X}", int16(bus)?).into()))?;
    } else if addrstr.contains(':') {
        // USB product:vendor
        let parts: Vec<&str> = addrstr.split(':').collect();
        if parts.len() != 2 {
            return Err("too many values to unpack (expected 2)".into());
        }
        dev.set("type", Some("usb".into()))?;
        dev.set("vendor", Some(format!("0x{:04X}", int16(parts[0])?).into()))?;
        dev.set(
            "product",
            Some(format!("0x{:04X}", int16(parts[1])?).into()),
        )?;
    } else if let Some((bus, device)) = addrstr.split_once('.') {
        // USB bus.device
        dev.set("type", Some("usb".into()))?;
        dev.set("bus", Some(bus.into()))?;
        dev.set("device", Some(device.into()))?;
    } else {
        return Err(format!("Unknown hostdev address string format '{addrstr}'"));
    }
    Ok(dev)
}

/// `nodedev.compare_to_hostdev(hostdev)`.
fn compare(nodedev: &NodeDevice, dev: &XmlObject) -> bool {
    if dev.is_build() {
        let xml = dev.get_xml();
        let Ok(doc) = virtinst_xml::parse_libvirt_xml(&xml) else {
            return false;
        };
        return doc
            .root_element()
            .is_some_and(|el| nodedev.compare_to_hostdev(&doc, el));
    }
    dev.with_doc(|doc, el| nodedev.compare_to_hostdev(doc, el))
}

/// `_lookupNodedevFromString`.
pub(crate) fn lookup_nodedev(all: &[NodeDevice], idstring: &str) -> Result<NodeDevice, Error> {
    // First try and see if this is a libvirt nodedev name
    if let Some(dev) = all.iter().find(|d| d.name == idstring) {
        return Ok(dev.clone());
    }
    // If not it must be a special CLI format that we need to parse
    let hostdev = address_string_to_hostdev(idstring)?;
    let found: Vec<&NodeDevice> = all.iter().filter(|d| compare(d, &hostdev)).collect();
    if found.len() > 1 {
        return Err(format!("{idstring} corresponds to multiple node devices"));
    }
    match found.first() {
        Some(dev) => Ok((*dev).clone()),
        None => Err(format!(
            "Did not find a matching node device for '{idstring}'"
        )),
    }
}

/// `ParserHostdev.set_name_cb`.
fn set_name_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    let typ = inst.get_str("type");
    let prop = match typ.as_deref() {
        Some("net") => Some("net_interface"),
        Some("misc") => Some("misc_char"),
        Some("storage") => Some("storage_block"),
        _ => None,
    };
    if let Some(prop) = prop {
        inst.set("mode", Some("capabilities".into()))?;
        return inst.set(prop, val);
    }
    let val = val.map(|v| v.to_xml_string()).unwrap_or_default();
    let all = nodedevs();
    let nodedev = lookup_nodedev(&all, &val)?;
    hostdev::set_from_nodedev(inst, Some(&nodedev), &all)
}

/// `ParserHostdev.name_lookup_cb`.
fn name_lookup_cb(
    _ctx: &ParseCtx,
    inst: &XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<bool, Error> {
    let val = val.map(|v| v.to_xml_string()).unwrap_or_default();
    let nodedev = lookup_nodedev(&nodedevs(), &val)?;
    Ok(compare(&nodedev, inst))
}

/// `ParserHostdev`.
pub fn hostdev() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new(
            "hostdev",
            Some("devices.hostdev"),
            Some(&hostdev::DEVICE_HOSTDEV),
        );
        def.remove_first = &["name"];
        add_common_device_args(&mut def, false, false);
        add_boot_order_arg(&mut def);
        def.arg(ArgDef::new("type", "type"));
        def.arg(ArgDef {
            propname: None,
            ..ArgDef::new("name", "")
                .cb(set_name_cb)
                .lookup(name_lookup_cb)
        });
        def.arg(ArgDef::new("driver.name", "driver_name"));
        def.arg(ArgDef::new("rom.bar", "rom_bar").onoff());
        def.arg(ArgDef::new("source.startupPolicy", "startup_policy"));
        def.alias("driver.name", &["driver_name"]);
        def.alias("rom.bar", &["rom_bar"]);
        def
    })
}

/// `ParserRedir._parse`.
fn redir_parse(ctx: &mut ParseCtx) -> Result<Option<Vec<XmlObject>>, Error> {
    if ctx.optstr == "none" {
        crate::parser::update_guest_flags(|f| f.skip_default_usbredir = true);
        return Ok(Some(vec![]));
    }
    Ok(None)
}

/// `ParserRedir.set_server_cb`.
fn set_server_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    let val = val.map(|v| v.to_xml_string());
    chardev::set_friendly_host(inst.child_mut("source"), val.as_deref())
}

/// `ParserRedir`.
pub fn redirdev() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new(
            "redirdev",
            Some("devices.redirdev"),
            Some(&redirdev::DEVICE_REDIRDEV),
        );
        def.remove_first = &["bus"];
        def.stub_none = false;
        def.custom_parse = Some(redir_parse);
        add_common_device_args(&mut def, false, false);
        add_boot_order_arg(&mut def);
        def.arg(ArgDef::new("bus", "bus").ignore_default());
        def.arg(ArgDef::new("type", "type").ignore_default());
        def.arg(ArgDef::cb_only("server", set_server_cb));
        add_char_source_args(&mut def, "");
        def
    })
}
