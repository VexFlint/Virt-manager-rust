//! Guest level options: `--metadata`, `--events`, `--resource`,
//! `--features`, `--clock`, `--pm`, `--sysinfo`, `--qemu-commandline`
//! and `--xml` (`ParserMetadata`, `ParserEvents`, `ParserResource`,
//! `ParserFeatures`, `ParserClock`, `ParserPM`, `ParserSysinfo`,
//! `ParserQemuCLI` and `ParserXML`).

use std::sync::OnceLock;

use virtinst_core::domain::{clock, features, resource, sysinfo, xmlnsqemu};
use virtinst_core::osinfo::{OsDb, OsVariant};
use virtinst_core::xmlbuilder::{Value, XmlObject};

use crate::parser::{shlex_split, ArgDef, ArgInst, Error, OptDict, ParseCtx, ParserDef};

fn val_str(val: Option<Value>) -> String {
    val.map(|v| v.to_xml_string()).unwrap_or_default()
}

/// `Guest._set_os_obj`.
fn set_os_id(guest: &mut XmlObject, full_id: Option<String>) -> Result<(), Error> {
    guest.set_path("_metadata.libosinfo.os_id", full_id.map(Value::Str))
}

/// `ParserMetadata.set_os_name_cb`: `Guest.set_os_name`.
fn set_os_name_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    let name = val_str(val);
    let full_id = if name == "generic" {
        OsVariant::generic().full_id
    } else {
        let db = OsDb::load_default().map_err(|e| e.to_string())?;
        db.all_os()
            .into_iter()
            .find(|o| o.short_id() == name)
            .map(|o| Some(o.id()))
            .ok_or_else(|| {
                format!("Unknown OS name '{name}'. See `--osinfo list` for valid values.")
            })?
    };
    set_os_id(inst, full_id)
}

/// `ParserMetadata.set_os_full_id_cb`.
fn set_os_full_id_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    let full_id = val_str(val);
    let db = OsDb::load_default().map_err(|e| e.to_string())?;
    let os = db
        .os(&full_id)
        .ok_or_else(|| format!("Unknown libosinfo ID '{full_id}'"))?;
    set_os_id(inst, Some(os.id()))
}

/// `ParserMetadata`.
pub fn metadata() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new("metadata", None, None);
        def.arg(ArgDef::new("name", "name").can_comma());
        def.arg(ArgDef::new("title", "title").can_comma());
        def.arg(ArgDef::new("uuid", "uuid"));
        def.arg(ArgDef::new("genid", "genid"));
        def.arg(ArgDef::new("genid_enable", "genid_enable").onoff());
        def.arg(ArgDef::new("description", "description").can_comma());
        def.arg(ArgDef::cb_only("os_name", set_os_name_cb));
        def.arg(ArgDef::cb_only("os_full_id", set_os_full_id_cb));
        def
    })
}

/// `ParserEvents`.
pub fn events() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new("events", None, None);
        for name in ["on_poweroff", "on_reboot", "on_crash", "on_lockfailure"] {
            def.arg(ArgDef::new(name, name));
        }
        def
    })
}

/// `ParserResource`.
pub fn resource() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new(
            "resource",
            Some("resource"),
            Some(&resource::DOMAIN_RESOURCE),
        );
        def.remove_first = &["partition"];
        def.arg(ArgDef::new("partition", "partition"));
        def.arg(ArgDef::new("fibrechannel.appid", "fibrechannel_appid").can_comma());
        def
    })
}

/// `ParserFeatures`.
pub fn features() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new(
            "features",
            Some("features"),
            Some(&features::DOMAIN_FEATURES),
        );
        for name in ["acpi", "apic", "pae", "privnet", "hap", "viridian"] {
            def.arg(ArgDef::new(name, name).onoff());
        }

        def.arg(ArgDef::new("apic.eoi", "eoi").onoff());
        def.arg(ArgDef::new("pmu.state", "pmu").onoff());

        for (cli, prop) in [
            ("hyperv.relaxed.state", "hyperv_relaxed"),
            ("hyperv.vapic.state", "hyperv_vapic"),
            ("hyperv.spinlocks.state", "hyperv_spinlocks"),
        ] {
            def.arg(ArgDef::new(cli, prop).onoff());
        }
        def.arg(ArgDef::new(
            "hyperv.spinlocks.retries",
            "hyperv_spinlocks_retries",
        ));
        for (cli, prop) in [
            ("hyperv.vpindex.state", "hyperv_vpindex"),
            ("hyperv.runtime.state", "hyperv_runtime"),
            ("hyperv.synic.state", "hyperv_synic"),
            ("hyperv.stimer.state", "hyperv_stimer"),
            ("hyperv.stimer.direct.state", "hyperv_stimer_direct"),
            ("hyperv.reset.state", "hyperv_reset"),
            ("hyperv.frequencies.state", "hyperv_frequencies"),
            ("hyperv.reenlightenment.state", "hyperv_reenlightenment"),
            ("hyperv.tlbflush.state", "hyperv_tlbflush"),
            ("hyperv.tlbflush.direct.state", "hyperv_tlbflush_direct"),
            ("hyperv.tlbflush.extended.state", "hyperv_tlbflush_extended"),
            ("hyperv.ipi.state", "hyperv_ipi"),
            ("hyperv.evmcs.state", "hyperv_evmcs"),
            ("hyperv.avic.state", "hyperv_avic"),
            ("hyperv.xmm_input.state", "hyperv_xmm_input"),
            ("hyperv.emsr_bitmap.state", "hyperv_emsr_bitmap"),
            ("vmport.state", "vmport"),
            ("kvm.hidden.state", "kvm_hidden"),
            ("kvm.hint-dedicated.state", "kvm_hint_dedicated"),
            ("kvm.poll-control.state", "kvm_poll_control"),
            ("kvm.pv-ipi.state", "kvm_pv_ipi"),
            ("pvspinlock.state", "pvspinlock"),
        ] {
            def.arg(ArgDef::new(cli, prop).onoff());
        }

        def.arg(ArgDef::new("gic.version", "gic_version"));

        def.arg(ArgDef::new("smm.state", "smm").onoff());
        def.arg(ArgDef::new("vmcoreinfo.state", "vmcoreinfo").onoff());
        def.arg(ArgDef::new("ioapic.driver", "ioapic_driver"));
        def.arg(ArgDef::new("msrs.unknown", "msrs_unknown"));

        for (cli, alias) in [
            ("apic.eoi", "eoi"),
            ("pmu.state", "pmu"),
            ("vmport.state", "vmport"),
            ("kvm.hidden.state", "kvm_hidden"),
            ("gic.version", "gic_version"),
            ("smm.state", "smm"),
            ("vmcoreinfo.state", "vmcoreinfo"),
            ("hyperv.relaxed.state", "hyperv_relaxed"),
            ("hyperv.vapic.state", "hyperv_vapic"),
            ("hyperv.spinlocks.state", "hyperv_spinlocks"),
            ("hyperv.spinlocks.retries", "hyperv_spinlocks_retries"),
            ("hyperv.synic.state", "hyperv_synic"),
            ("hyperv.reset.state", "hyperv_reset"),
        ] {
            def.alias(cli, &[alias]);
        }
        def
    })
}

/// `ParserClock._remove_old_options`: these _tickpolicy options have
/// never had any effect in libvirt, even though they aren't explicitly
/// rejected. Make them no-ops.
fn remove_old_clock_options(_optstr: &str, o: &mut OptDict) -> Result<(), Error> {
    for name in [
        "platform_tickpolicy",
        "hpet_tickpolicy",
        "tsc_tickpolicy",
        "kvmclock_tickpolicy",
        "hypervclock_tickpolicy",
    ] {
        o.remove(name);
    }
    Ok(())
}

/// `ParserClock.set_timer`: `rtc_tickpolicy` sets the `rtc` timer's
/// `tickpolicy`, adding the timer if needed.
fn set_timer_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    arg: &ArgInst,
) -> Result<(), Error> {
    let (tname, propname) = arg
        .def
        .cliname
        .split_once('_')
        .expect("timer helpers are named TIMER_PROP");
    let found = inst
        .children("timers")
        .iter()
        .position(|t| t.get_str("name").as_deref() == Some(tname));
    let idx = match found {
        Some(i) => i,
        None => {
            let mut timer = XmlObject::new(&clock::CLOCK_TIMER);
            timer.set("name", Some(Value::Str(tname.into())))?;
            inst.add_child(timer);
            inst.children("timers").len() - 1
        }
    };
    inst.children_mut("timers")[idx].set_path(propname, val)
}

/// `ParserClock`.
pub fn clock() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new("clock", Some("clock"), Some(&clock::DOMAIN_CLOCK));
        def.pre_parse = Some(remove_old_clock_options);

        // Timer convenience helpers. It's unclear if we should continue
        // extending this pattern, or just push users to use finegrained
        // timer* config
        def.arg(ArgDef::cb_only("pit_tickpolicy", set_timer_cb));
        def.arg(ArgDef::cb_only("rtc_tickpolicy", set_timer_cb));
        for name in [
            "platform_present",
            "pit_present",
            "rtc_present",
            "hpet_present",
            "tsc_present",
            "kvmclock_present",
            "hypervclock_present",
        ] {
            def.arg(ArgDef::cb_only(name, set_timer_cb).onoff());
        }

        // Standard XML options
        def.arg(ArgDef::new("offset", "offset"));
        let timer = |cli, prop| ArgDef::new(cli, prop).find_inst("timer", "timers");
        def.arg(timer("timer[0-9]*.name", "name"));
        def.arg(timer("timer[0-9]*.present", "present").onoff());
        def.arg(timer("timer[0-9]*.tickpolicy", "tickpolicy"));
        def.arg(timer("timer[0-9]*.track", "track"));
        def.arg(timer("timer[0-9]*.mode", "mode"));
        def.arg(timer("timer[0-9]*.frequency", "frequency"));
        def.arg(timer("timer[0-9]*.catchup.threshold", "threshold"));
        def.arg(timer("timer[0-9]*.catchup.slew", "slew"));
        def.arg(timer("timer[0-9]*.catchup.limit", "limit"));
        def
    })
}

/// `ParserPM`.
pub fn pm() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new(
            "pm",
            Some("pm"),
            Some(&virtinst_core::domain::pm::DOMAIN_PM),
        );
        def.arg(ArgDef::new("suspend_to_mem.enabled", "suspend_to_mem").onoff());
        def.arg(ArgDef::new("suspend_to_disk.enabled", "suspend_to_disk").onoff());
        def.alias("suspend_to_mem.enabled", &["suspend_to_mem"]);
        def.alias("suspend_to_disk.enabled", &["suspend_to_disk"]);
        def
    })
}

/// `ParserSysinfo.parse`: if any string specified, default to
/// type=smbios otherwise libvirt errors. User args can still override
/// this though.
fn sysinfo_default_type(_optstr: &str, o: &mut OptDict) -> Result<(), Error> {
    if o.get("type").is_none() {
        o.insert("type".into(), Some("smbios".into()));
    }
    Ok(())
}

/// `ParserSysinfo.parse`: previously libvirt treated sysinfo as a
/// singleton object, but that changed with fwcfg support. Maintain
/// sorta backcompat behavior by merging options if 'type' matches.
fn sysinfo_existing(guest: &XmlObject, o: &OptDict) -> Option<usize> {
    let typ = match o.get("type") {
        Some(t) => t.clone().unwrap_or_default(),
        None => "smbios".into(),
    };
    guest
        .children("sysinfo")
        .iter()
        .position(|s| s.get_str("type").as_deref() == Some(typ.as_str()))
}

/// `ParserSysinfo.set_type_cb`.
fn set_sysinfo_type_cb(
    ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    let typ = val_str(val);
    if typ == "host" || typ == "emulate" {
        return ctx.guest.set_path("os.smbios_mode", Some(Value::Str(typ)));
    }
    if typ == "smbios" {
        ctx.guest
            .set_path("os.smbios_mode", Some(Value::Str("sysinfo".into())))?;
    }
    inst.set("type", Some(Value::Str(typ)))
}

/// `ParserSysinfo.set_uuid_cb`: if a uuid is supplied it must match the
/// guest UUID, so overwrite the guest uuid with what is passed in.
fn set_sysinfo_uuid_cb(
    ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    inst.set("system_uuid", val.clone())?;
    ctx.guest.set("uuid", val)
}

/// `ParserSysinfo`.
pub fn sysinfo() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new("sysinfo", Some("sysinfo"), Some(&sysinfo::DOMAIN_SYSINFO));
        def.remove_first = &["type"];
        def.pre_parse = Some(sysinfo_default_type);
        def.existing_inst = Some(sysinfo_existing);
        // <sysinfo type='smbios'>
        def.arg(
            ArgDef::new("type", "type")
                .cb(set_sysinfo_type_cb)
                .can_comma(),
        );
        let groups: [(&str, &[&str]); 4] = [
            // <bios> type 0 BIOS Information
            ("bios", &["vendor", "version", "date", "release"]),
            // <system> type 1 System Information
            (
                "system",
                &[
                    "manufacturer",
                    "product",
                    "version",
                    "serial",
                    "uuid",
                    "sku",
                    "family",
                ],
            ),
            // <baseBoard> type 2 Baseboard (or Module) Information
            (
                "baseBoard",
                &[
                    "manufacturer",
                    "product",
                    "version",
                    "serial",
                    "asset",
                    "location",
                ],
            ),
            (
                "chassis",
                &["manufacturer", "version", "serial", "asset", "sku"],
            ),
        ];
        for (group, names) in groups {
            for name in names {
                let cli: &'static str = Box::leak(format!("{group}.{name}").into_boxed_str());
                let prop: &'static str = Box::leak(format!("{group}_{name}").into_boxed_str());
                let mut arg = ArgDef::new(cli, prop);
                if prop == "system_uuid" {
                    arg = arg.cb(set_sysinfo_uuid_cb);
                }
                def.arg(arg);
                if group != "chassis" {
                    def.alias(cli, &[prop]);
                }
            }
        }
        def.arg(
            ArgDef::new("oemStrings.entry[0-9]*", "value")
                .can_comma()
                .find_inst("entry", "oemStrings"),
        );
        let entry = |cli| {
            ArgDef::new(cli, "")
                .can_comma()
                .find_inst("entry", "entries")
        };
        def.arg(ArgDef {
            propname: Some("value"),
            ..entry("entry[0-9]*")
        });
        def.arg(ArgDef {
            propname: Some("name"),
            ..entry("entry[0-9]*.name")
        });
        def.arg(ArgDef {
            propname: Some("file"),
            ..entry("entry[0-9]*.file")
        });
        def
    })
}

/// `ParserQemuCLI._parse`: the whole option string is one value.
fn qemu_cli_optdict(optstr: &str, o: &mut OptDict) -> Result<(), Error> {
    *o = OptDict::default();
    let rest = || optstr.split_once('=').map(|(_, r)| r.to_string());
    for key in ["env", "args", "clearxml"] {
        if optstr.starts_with(&format!("{key}=")) {
            o.insert(key.into(), rest());
            return Ok(());
        }
    }
    o.insert("args".into(), Some(optstr.to_string()));
    Ok(())
}

/// `ParserQemuCLI.args_cb`.
fn qemu_args_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    for opt in shlex_split(&val_str(val)) {
        let mut obj = XmlObject::new(&xmlnsqemu::XMLNS_QEMU_ARG);
        obj.set("value", Some(Value::Str(opt)))?;
        inst.add_child(obj);
    }
    Ok(())
}

/// `ParserQemuCLI.env_cb`.
fn qemu_env_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    let val = val_str(val);
    let (name, envval) = val
        .split_once('=')
        .ok_or("not enough values to unpack (expected 2, got 1)")?;
    let mut obj = XmlObject::new(&xmlnsqemu::XMLNS_QEMU_ENV);
    obj.set("name", Some(Value::Str(name.into())))?;
    obj.set("value", Some(Value::Str(envval.into())))?;
    inst.add_child(obj);
    Ok(())
}

/// `ParserQemuCLI`.
pub fn qemu_commandline() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new(
            "qemu_commandline",
            Some("xmlns_qemu"),
            Some(&xmlnsqemu::DOMAIN_XMLNS_QEMU),
        );
        def.pre_parse = Some(qemu_cli_optdict);
        def.arg(ArgDef::cb_only("args", qemu_args_cb).can_comma());
        def.arg(ArgDef::cb_only("env", qemu_env_cb).can_comma());
        def
    })
}

/// `ParserXML`: sets values on `XMLManualAction` instances, tracked in
/// the guest's `xml_actions`.
pub fn xml() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new("xml", None, None);
        def.supports_clearxml = false;
        def.args.retain(|a| a.cliname != "clearxml");
        def.is_xml_action = true;
        for (cli, prop) in [
            ("xpath.delete", "xpath_delete"),
            ("xpath.set", "xpath_set"),
            ("xpath.create", "xpath_create"),
            ("xpath.value", "xpath_value"),
        ] {
            def.arg(ArgDef {
                can_comma: true,
                no_lookup: true,
                ..ArgDef::new(cli, prop).find_inst("xpath", "xml_actions")
            });
        }
        def
    })
}
