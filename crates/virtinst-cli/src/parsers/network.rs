//! `--network`: `ParserNetwork`.

use std::sync::OnceLock;

use virtinst_core::devices::network;
use virtinst_core::xmlbuilder::{Value, XmlObject};

use crate::parser::{
    add_boot_order_arg, add_common_device_args, conn_support, nodedevs, ArgDef, ArgInst, Error,
    ParseCtx, ParserDef,
};

/// `ParserNetwork._parse`, with `_add_advertised_aliases`.
fn network_parse(ctx: &mut ParseCtx) -> Result<Option<Vec<XmlObject>>, Error> {
    // These are essentially aliases for new style options, but we still
    // want to advertise them in --network=help output because they are
    // historically commonly used. This should rarely, if ever, be extended
    let o = &mut ctx.optdict;
    if let Some(v) = o.remove("model") {
        o.insert("model.type".into(), v);
    }
    if let Some(v) = o.remove("mac") {
        o.insert("mac.address".into(), v);
    }

    // Back compat with old style network= and bridge=
    if o.get("type").is_none() {
        if let Some(v) = o.remove("network") {
            o.insert("type".into(), Some("network".into()));
            o.insert("source".into(), v);
        } else if let Some(v) = o.remove("bridge") {
            o.insert("type".into(), Some("bridge".into()));
            o.insert("source".into(), v);
        }
    } else {
        o.remove("network");
        o.remove("bridge");
    }

    if ctx.optstr == "none" {
        return Ok(Some(vec![]));
    }
    Ok(None)
}

fn noset_cb(
    _ctx: &mut ParseCtx,
    _inst: &mut XmlObject,
    _val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    Ok(())
}

fn val_str(val: Option<Value>) -> Option<String> {
    val.map(|v| v.to_xml_string())
}

/// `DeviceInterface.source`'s setter.
fn set_source_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    network::set_source_prop(inst, val_str(val))
}

/// `DeviceInterface.source`, compared the way `lookup_param` does: set
/// on a copy and read back.
fn source_lookup_cb(
    _ctx: &ParseCtx,
    inst: &XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<bool, Error> {
    let typ = inst.get_str("type");
    Ok(match network::source_propname(typ.as_deref()) {
        Some(p) => inst.get_str(p) == val_str(val),
        None => true,
    })
}

/// `ParserNetwork.set_mac_cb`.
fn set_mac_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    if val.as_ref().and_then(|v| v.as_str()) == Some("RANDOM") {
        return Ok(());
    }
    inst.set("macaddr", val)
}

/// `DeviceInterface.set_default_source`.
fn set_default_source(inst: &mut XmlObject) -> Result<(), Error> {
    let support = conn_support();
    if support.is_qemu_unprivileged || support.is_test {
        return inst.set("type", Some("user".into()));
    }
    let (nettype, source) = match default_bridge(support.is_remote) {
        Some(br) => ("bridge", br),
        None => ("network", "default".to_string()),
    };
    inst.set("type", Some(nettype.into()))?;
    network::set_source_prop(inst, Some(source))
}

/// `ParserNetwork.set_type_cb`.
fn set_type_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    match val.as_ref().and_then(|v| v.as_str()) {
        Some("default") => set_default_source(inst),
        Some("passt") => {
            inst.set("type", Some("user".into()))?;
            inst.child_mut("backend").set("type", Some("passt".into()))
        }
        _ => inst.set("type", val),
    }
}

/// `ParserNetwork.set_link_state`.
fn set_link_state(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    let val = val_str(val);
    let val = match val.as_deref() {
        Some("up" | "down") | None => val,
        Some(v) => match crate::cli::raw_on_off_convert(v) {
            Some(true) => Some("up".into()),
            Some(false) => Some("down".into()),
            None => val,
        },
    };
    inst.set("link_state", val.map(Into::into))
}

/// `ParserNetwork.set_port_forward_pretty`.
fn set_port_forward_pretty(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    let val = val_str(val).unwrap_or_default();
    let (val, proto) = match val.rsplit_once('/') {
        Some((v, p)) => (v.to_string(), p.to_string()),
        None => (val, "tcp".to_string()),
    };
    let (val, port_to) = match val.rsplit_once(':') {
        Some((v, p)) => (v.to_string(), Some(p.to_string())),
        None => (val, None),
    };
    let (val, host) = match val.rsplit_once(':') {
        Some((h, v)) => (v.to_string(), Some(h.to_string())),
        None => (val, None),
    };
    let (port_start, port_end) = match val.rsplit_once('-') {
        Some((s, e)) => (s.to_string(), Some(e.to_string())),
        None => (val, None),
    };

    if inst.children("range").is_empty() {
        inst.add_child(XmlObject::new(&network::PORT_FORWARD_RANGE));
    }
    inst.set("proto", Some(proto.into()))?;
    inst.set("address", host.map(Into::into))?;
    let rangeobj = &mut inst.children_mut("range")[0];
    rangeobj.set("start", Some(port_start.into()))?;
    rangeobj.set("end", port_end.map(Into::into))?;
    rangeobj.set("to", port_to.map(Into::into))
}

/// `ParserNetwork.set_hostdev_cb`: `DeviceInterface.set_from_nodedev`.
fn set_hostdev_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    let val = val_str(val).unwrap_or_default();
    let nodedev = super::hostdev::lookup_nodedev(&nodedevs(), &val)?;
    log::debug!("set_from_nodedev name={}", nodedev.name);

    inst.set("type", Some("hostdev".into()))?;
    if inst.get("managed").is_none() {
        inst.set("managed", Some(true.into()))?;
    }
    if nodedev.device_type.as_deref() != Some("pci") {
        return Err(format!(
            "Unsupported node device type '{}'",
            nodedev.device_type.as_deref().unwrap_or("None")
        ));
    }
    let addr = inst.child_mut("source_address");
    addr.set("type", Some("pci".into()))?;
    addr.set("domain", nodedev.domain.clone().map(Into::into))?;
    addr.set("bus", nodedev.bus.clone().map(Into::into))?;
    addr.set("slot", nodedev.slot.clone().map(Into::into))?;
    addr.set("function", nodedev.function.clone().map(Into::into))
}

/// `ParserNetwork`.
pub fn network() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new(
            "network",
            Some("devices.interface"),
            Some(&network::DEVICE_INTERFACE),
        );
        def.remove_first = &["type"];
        def.stub_none = false;
        def.custom_parse = Some(network_parse);
        add_common_device_args(&mut def, false, false);
        add_boot_order_arg(&mut def);
        def.arg(ArgDef::new("boot.loadparm", "boot.loadparm"));
        def.arg(ArgDef::new("driver.ats", "virtio_driver.ats").onoff());
        def.arg(ArgDef::new("driver.iommu", "virtio_driver.iommu").onoff());
        def.arg(ArgDef::new("driver.packed", "virtio_driver.packed").onoff());
        def.arg(ArgDef::new("driver.page_per_vq", "virtio_driver.page_per_vq").onoff());

        // These are handled in _add_advertised_aliases
        def.arg(ArgDef::new("model", "model").cb(noset_cb));
        def.arg(ArgDef::new("mac", "macaddr").cb(noset_cb));
        def.arg(
            ArgDef::new("network", "source")
                .cb(noset_cb)
                .lookup(source_lookup_cb),
        );
        def.arg(
            ArgDef::new("bridge", "source")
                .cb(noset_cb)
                .lookup(source_lookup_cb),
        );

        // Standard XML options
        def.arg(ArgDef::new("type", "type").cb(set_type_cb));
        def.arg(ArgDef::new("backend.type", "backend.type"));
        def.arg(ArgDef::new("backend.logFile", "backend.logFile"));
        def.arg(ArgDef::new("trustGuestRxFilters", "trustGuestRxFilters").onoff());

        def.arg(
            ArgDef::new("source", "source")
                .cb(set_source_cb)
                .lookup(source_lookup_cb),
        );
        def.arg(ArgDef::new("source.mode", "source_mode"));
        def.arg(ArgDef::new("source.type", "source_type"));
        def.arg(ArgDef::new("source.path", "source_path"));
        def.arg(ArgDef::new("source.portgroup", "portgroup"));
        def.arg(ArgDef::new("source.address.type", "source_address.type"));
        def.arg(ArgDef::new(
            "source.address.domain",
            "source_address.domain",
        ));
        def.arg(ArgDef::new("source.address.bus", "source_address.bus"));
        def.arg(ArgDef::new("source.address.slot", "source_address.slot"));
        def.arg(ArgDef::new(
            "source.address.function",
            "source_address.function",
        ));

        def.arg(ArgDef::cb_only("hostdev", set_hostdev_cb));

        def.arg(ArgDef::new("target.dev", "target_dev"));
        def.arg(ArgDef::new("model.type", "model"));
        def.arg(ArgDef::new("mac.address", "macaddr").cb(set_mac_cb));
        def.arg(ArgDef::new("filterref.filter", "filterref"));
        def.arg(ArgDef::new("link.state", "link_state").cb(set_link_state));

        def.arg(ArgDef::new("driver.name", "driver_name"));
        def.arg(ArgDef::new("driver.queues", "driver_queues"));

        def.arg(ArgDef::new("rom.file", "rom_file"));
        def.arg(ArgDef::new("rom.bar", "rom_bar").onoff());

        def.arg(ArgDef::new("mtu.size", "mtu_size"));

        def.arg(ArgDef::new("virtualport.type", "virtualport.type"));
        for (cli, prop) in [
            ("virtualport.parameters.managerid", "virtualport.managerid"),
            ("virtualport.parameters.typeid", "virtualport.typeid"),
            (
                "virtualport.parameters.typeidversion",
                "virtualport.typeidversion",
            ),
            (
                "virtualport.parameters.instanceid",
                "virtualport.instanceid",
            ),
            ("virtualport.parameters.profileid", "virtualport.profileid"),
            (
                "virtualport.parameters.interfaceid",
                "virtualport.interfaceid",
            ),
        ] {
            def.arg(ArgDef::new(cli, prop));
        }
        let pf = ("portForward", "portForward");
        let range = ("range", "range");
        def.arg(ArgDef::new("portForward[0-9]*.proto", "proto").find_inst(pf.0, pf.1));
        def.arg(ArgDef::new("portForward[0-9]*.address", "address").find_inst(pf.0, pf.1));
        def.arg(ArgDef::new("portForward[0-9]*.dev", "dev").find_inst(pf.0, pf.1));
        def.arg(
            ArgDef::new("portForward[0-9]*.range[0-9]*.start", "start").find_inst_in(pf, range),
        );
        def.arg(ArgDef::new("portForward[0-9]*.range[0-9]*.end", "end").find_inst_in(pf, range));
        def.arg(ArgDef::new("portForward[0-9]*.range[0-9]*.to", "to").find_inst_in(pf, range));
        def.arg(
            ArgDef::new("portForward[0-9]*.range[0-9]*.exclude", "exclude")
                .onoff()
                .find_inst_in(pf, range),
        );
        def.arg(
            ArgDef::cb_only("portForward[0-9]*", set_port_forward_pretty).find_inst(pf.0, pf.1),
        );

        for (cli, alias) in [
            ("driver.name", "driver_name"),
            ("driver.queues", "driver_queues"),
            ("filterref.filter", "filterref"),
            ("link.state", "link_state"),
            ("mac.address", "mac"),
            ("model.type", "model"),
            ("rom.file", "rom_file"),
            ("rom.bar", "rom_bar"),
            ("target.dev", "target"),
            ("source.portgroup", "portgroup"),
            ("source.type", "source_type"),
            ("source.path", "source_path"),
            ("source.mode", "source_mode"),
            ("virtualport.type", "virtualport_type"),
            ("virtualport.parameters.managerid", "virtualport_managerid"),
            ("virtualport.parameters.typeid", "virtualport_typeid"),
            (
                "virtualport.parameters.typeidversion",
                "virtualport_typeidversion",
            ),
            (
                "virtualport.parameters.instanceid",
                "virtualport_instanceid",
            ),
            ("virtualport.parameters.profileid", "virtualport_profileid"),
            (
                "virtualport.parameters.interfaceid",
                "virtualport_interfaceid",
            ),
        ] {
            def.alias(cli, &[alias]);
        }
        def
    })
}

/// `_default_bridge(conn)`.
pub fn default_bridge(is_remote: bool) -> Option<String> {
    if is_remote {
        return None;
    }
    static HOST_DEFAULT_BRIDGE: OnceLock<Option<String>> = OnceLock::new();
    let ret = HOST_DEFAULT_BRIDGE
        .get_or_init(network::host_default_bridge)
        .clone();
    if std::env::var_os("VIRTINST_TEST_SUITE").is_some() {
        return Some("testsuitebr0".into());
    }
    ret
}
