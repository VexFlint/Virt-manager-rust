//! CPU options: `--cpu`, `--cputune` and `--vcpus` (`ParserCPU`,
//! `ParserCputune` and `ParserVCPU`).

use std::cell::RefCell;
use std::sync::OnceLock;

use virtinst_core::capabilities::domcaps::DomainCapabilities;
use virtinst_core::domain::cpu::{self as dcpu, CpuEditContext, SPECIAL_MODES};
use virtinst_core::domain::cputune;
use virtinst_core::guest::set_vcpus;
use virtinst_core::xmlbuilder::{Value, XmlObject};

use crate::parser::{connect, ArgDef, ArgInst, Error, OptDict, ParseCtx, ParserDef};

fn noset_cb(
    _ctx: &mut ParseCtx,
    _inst: &mut XmlObject,
    _val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    Ok(())
}

/// Separates the values of a feature policy that was given more than
/// once (`force=a,force=b`, upstream's list value).
const LIST_SEP: char = '\u{1f}';

/// What `DomainCpu` reads from the connection.
#[derive(Default)]
struct CpuConn {
    domcaps: Option<DomainCapabilities>,
    host_cpu_model: Option<String>,
    /// `domcaps.get_cpu_security_features()`, looked up when first needed.
    security_features: Option<Vec<String>>,
}

thread_local! {
    static CPU_CONN: RefCell<CpuConn> = RefCell::new(CpuConn::default());
}

/// The guest's `lookup_domcaps()` and `conn.caps.host.cpu.model`, for
/// `--cpu`.
pub fn set_cpu_conn(domcaps: Option<DomainCapabilities>, host_cpu_model: Option<String>) {
    CPU_CONN.with(|c| {
        *c.borrow_mut() = CpuConn {
            domcaps,
            host_cpu_model,
            security_features: None,
        }
    });
}

/// The `CpuEditContext` for `guest`; the security features only when
/// `with_security` (they need a CPU baseline).
fn cpu_context(guest: &XmlObject, with_security: bool) -> CpuEditContext {
    let info = super::guest_info(guest);
    CPU_CONN.with(|c| {
        let mut c = c.borrow_mut();
        if with_security && c.security_features.is_none() {
            let features = match (&c.domcaps, connect()) {
                (Some(dc), Some(conn)) => dc.lookup_cpu_security_features(&conn),
                _ => vec![],
            };
            c.security_features = Some(features);
        }
        let app_default_mode = match &c.domcaps {
            Some(dc) => dc.app_default_cpu_mode(
                info.domain_type.as_deref(),
                info.arch.as_deref(),
                info.machine.as_deref(),
            ),
            None => dcpu::SPECIAL_MODE_HOST_MODEL_ONLY,
        };
        CpuEditContext {
            app_default_mode: app_default_mode.to_string(),
            host_cpu_model: c.host_cpu_model.clone(),
            security_features: c.security_features.clone().unwrap_or_default(),
        }
    })
}

/// `ParserCPU._convert_old_feature_options`.
///
/// For old CLI compat, --cpu force=foo,force=bar should force enable
/// 'foo' and 'bar' features, but that doesn't fit with the CLI parser
/// infrastructure very well.
fn convert_old_feature_options(optstr: &str, o: &mut OptDict) -> Result<(), Error> {
    let mut converted: Vec<(String, Vec<String>)> = vec![];
    let mut push = |key: &str, val: String| match converted.iter_mut().find(|(k, _)| k == key) {
        Some((_, v)) => v.push(val),
        None => converted.push((key.to_string(), vec![val])),
    };
    for (key, value) in crate::parser::parse_optstr_tuples(optstr) {
        if ["force", "require", "optional", "disable", "forbid"].contains(&key.as_str()) {
            push(&key, value.unwrap_or_default());
        }
    }

    // Convert +feature, -feature into expected format
    let keys: Vec<(String, Option<String>)> = o.0.clone();
    for (key, value) in keys {
        if value.as_deref().is_some_and(|v| !v.is_empty()) || key.chars().count() == 1 {
            continue;
        }
        let policy = if key.starts_with('+') {
            "force"
        } else if key.starts_with('-') {
            "disable"
        } else {
            continue;
        };
        o.remove(&key);
        push(policy, key[1..].to_string());
    }

    for (key, vals) in converted {
        o.insert(key, Some(vals.join(&LIST_SEP.to_string())));
    }
    Ok(())
}

/// `ParserCPU.set_model_cb`.
fn set_model_cb(
    ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    let mut val = val.map(|v| v.to_xml_string()).unwrap_or_default();
    if val == "host" {
        log::warn!("CPU model=host is deprecated, use model=host-model");
        val = dcpu::SPECIAL_MODE_HOST_MODEL.into();
    }
    if val == "none" {
        val = dcpu::SPECIAL_MODE_CLEAR.into();
    }
    let secure = ctx.stash.get("secure").cloned().flatten().as_deref() != Some("off");
    if SPECIAL_MODES.contains(&val.as_str()) {
        let plain = cpu_context(ctx.guest, false);
        let mode = if val == dcpu::SPECIAL_MODE_APP_DEFAULT {
            plain.app_default_mode.as_str()
        } else {
            val.as_str()
        };
        // Only these modes go on to `set_model`.
        let needs_features = matches!(
            mode,
            dcpu::SPECIAL_MODE_HOST_MODEL_ONLY | dcpu::SPECIAL_MODE_HOST_COPY
        ) && plain.host_cpu_model.is_some();
        let cctx = if needs_features {
            cpu_context(ctx.guest, true)
        } else {
            plain
        };
        dcpu::set_special_mode(inst, &cctx, &val, secure)?;
        crate::parser::update_guest_flags(|f| f.cpu_special_mode_was_set = true);
        Ok(())
    } else {
        let cctx = cpu_context(ctx.guest, !val.is_empty());
        dcpu::set_model(inst, &cctx, Some(&val), secure)
    }
}

/// `secure` isn't XML: it only steers `set_model`.
fn set_secure_cb(
    ctx: &mut ParseCtx,
    _inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    let on = matches!(val, Some(Value::Bool(true)));
    ctx.stash
        .insert("secure".into(), Some(if on { "on" } else { "off" }.into()));
    Ok(())
}

/// `ParserCPU.set_feature_cb`.
fn set_feature_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    arg: &ArgInst,
) -> Result<(), Error> {
    let policy = arg.def.cliname;
    let val = val.map(|v| v.to_xml_string()).unwrap_or_default();
    for feature_name in val.split(LIST_SEP) {
        let idx = inst
            .children("features")
            .iter()
            .position(|f| f.get_str("name").as_deref() == Some(feature_name));
        match idx {
            Some(i) => {
                inst.children_mut("features")[i].set("policy", Some(Value::Str(policy.into())))?
            }
            None => dcpu::add_feature(inst, feature_name, policy)?,
        }
    }
    Ok(())
}

/// `ParserCPU`.
pub fn cpu() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new("cpu", Some("cpu"), Some(&dcpu::DOMAIN_CPU));
        def.remove_first = &["model"];
        def.stub_none = false;
        def.pre_parse = Some(convert_old_feature_options);
        // 'secure' needs to be parsed before 'model'
        def.arg(ArgDef::new("secure", "secure").onoff().cb(set_secure_cb));
        def.arg(ArgDef::new("model", "model").cb(set_model_cb));
        // 'model.fallback' must be parsed after 'model' as it may otherwise be
        // overridden
        def.arg(ArgDef::new("model.fallback", "model_fallback"));
        def.arg(ArgDef::new("model.vendor_id", "model_vendor_id"));
        def.arg(ArgDef::new("vendor", "vendor"));

        def.arg(ArgDef::new("mode", "mode"));
        def.arg(ArgDef::new("match", "match"));
        def.arg(ArgDef::new("check", "check"));
        def.arg(ArgDef::new("migratable", "migratable").onoff());

        for (cli, prop) in [
            ("topology.sockets", "topology.sockets"),
            ("topology.dies", "topology.dies"),
            ("topology.cores", "topology.cores"),
            ("topology.threads", "topology.threads"),
            ("cache.level", "cache.level"),
            ("cache.mode", "cache.mode"),
            ("maxphysaddr.mode", "maxphysaddr.mode"),
            ("maxphysaddr.bits", "maxphysaddr.bits"),
        ] {
            def.arg(ArgDef::new(cli, prop));
        }

        // CPU features
        // These are handled specially in _parse
        for policy in ["force", "require", "optional", "disable", "forbid"] {
            def.arg(ArgDef::cb_only(policy, set_feature_cb).no_lookup());
        }

        // NUMA cells
        let cell = ("cell", "cells");
        for (cli, prop) in [
            ("numa.cell[0-9]*.id", "id"),
            ("numa.cell[0-9]*.cpus", "cpus"),
            ("numa.cell[0-9]*.memory", "memory"),
            ("numa.cell[0-9]*.unit", "unit"),
            ("numa.cell[0-9]*.memAccess", "memAccess"),
            ("numa.cell[0-9]*.discard", "discard"),
        ] {
            let mut a = ArgDef::new(cli, prop).find_inst(cell.0, cell.1);
            if prop == "cpus" {
                a = a.can_comma();
            }
            if prop == "discard" {
                a = a.onoff();
            }
            def.arg(a);
        }
        for (cli, prop) in [
            ("numa.cell[0-9]*.distances.sibling[0-9]*.id", "id"),
            ("numa.cell[0-9]*.distances.sibling[0-9]*.value", "value"),
        ] {
            def.arg(ArgDef::new(cli, prop).find_inst_in(cell, ("sibling", "siblings")));
        }
        // NUMA cell caches
        for (cli, prop) in [
            ("numa.cell[0-9]*.cache[0-9]*.level", "level"),
            ("numa.cell[0-9]*.cache[0-9]*.associativity", "associativity"),
            ("numa.cell[0-9]*.cache[0-9]*.policy", "policy"),
            ("numa.cell[0-9]*.cache[0-9]*.size.value", "size_value"),
            ("numa.cell[0-9]*.cache[0-9]*.size.unit", "size_unit"),
            ("numa.cell[0-9]*.cache[0-9]*.line.value", "line_value"),
            ("numa.cell[0-9]*.cache[0-9]*.line.unit", "line_unit"),
        ] {
            def.arg(ArgDef::new(cli, prop).find_inst_in(cell, ("cache", "caches")));
        }

        // Interconnections between NUMA cells
        for (cli, prop) in [
            ("numa.interconnects.latency[0-9]*.initiator", "initiator"),
            ("numa.interconnects.latency[0-9]*.target", "target"),
            ("numa.interconnects.latency[0-9]*.cache", "cache"),
            ("numa.interconnects.latency[0-9]*.type", "type"),
            ("numa.interconnects.latency[0-9]*.value", "value"),
            ("numa.interconnects.latency[0-9]*.unit", "unit"),
        ] {
            def.arg(ArgDef::new(cli, prop).find_inst("latency", "latencies"));
        }
        for (cli, prop) in [
            ("numa.interconnects.bandwidth[0-9]*.initiator", "initiator"),
            ("numa.interconnects.bandwidth[0-9]*.target", "target"),
            ("numa.interconnects.bandwidth[0-9]*.type", "type"),
            ("numa.interconnects.bandwidth[0-9]*.cache", "cache"),
            ("numa.interconnects.bandwidth[0-9]*.value", "value"),
            ("numa.interconnects.bandwidth[0-9]*.unit", "unit"),
        ] {
            def.arg(ArgDef::new(cli, prop).find_inst("bandwidth", "bandwidths"));
        }

        for (cli, alias) in [
            (
                "numa.cell[0-9]*.distances.sibling[0-9]*.id",
                "cell[0-9]*.distances.sibling[0-9]*.id",
            ),
            (
                "numa.cell[0-9]*.distances.sibling[0-9]*.value",
                "cell[0-9]*.distances.sibling[0-9]*.value",
            ),
            ("numa.cell[0-9]*.id", "cell[0-9]*.id"),
            ("numa.cell[0-9]*.cpus", "cell[0-9]*.cpus"),
            ("numa.cell[0-9]*.memory", "cell[0-9]*.memory"),
        ] {
            def.alias(cli, &[alias]);
        }
        def
    })
}

/// `ParserCputune`.
pub fn cputune() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new("cputune", Some("cputune"), Some(&cputune::DOMAIN_CPUTUNE));
        def.remove_first = &["model"];
        def.stub_none = false;
        // Resource quotas
        for name in [
            "shares",
            "period",
            "quota",
            "global_period",
            "global_quota",
            "emulator_period",
            "emulator_quota",
            "iothread_period",
            "iothread_quota",
        ] {
            def.arg(ArgDef::new(name, name));
        }

        // Options for CPU.vcpus config
        def.arg(ArgDef::new("vcpupin[0-9]*.vcpu", "vcpu").find_inst("vcpupin", "vcpupins"));
        def.arg(
            ArgDef::new("vcpupin[0-9]*.cpuset", "cpuset")
                .can_comma()
                .find_inst("vcpupin", "vcpupins"),
        );
        def.arg(ArgDef::new("emulatorpin.cpuset", "emulatorpin_cpuset").can_comma());
        def.arg(
            ArgDef::new("iothreadpin[0-9]*.iothread", "iothread")
                .find_inst("iothreadpin", "iothreadpins"),
        );
        def.arg(
            ArgDef::new("iothreadpin[0-9]*.cpuset", "cpuset")
                .can_comma()
                .find_inst("iothreadpin", "iothreadpins"),
        );

        // Scheduling
        def.arg(ArgDef::new(
            "emulatorsched.scheduler",
            "emulatorsched_scheduler",
        ));
        def.arg(ArgDef::new(
            "emulatorsched.priority",
            "emulatorsched_priority",
        ));
        let vs = ("vcpusched", "vcpuscheds");
        def.arg(
            ArgDef::new("vcpusched[0-9]*.vcpus", "vcpus")
                .can_comma()
                .find_inst(vs.0, vs.1),
        );
        def.arg(ArgDef::new("vcpusched[0-9]*.scheduler", "scheduler").find_inst(vs.0, vs.1));
        def.arg(ArgDef::new("vcpusched[0-9]*.priority", "priority").find_inst(vs.0, vs.1));
        let is = ("iothreadsched", "iothreadscheds");
        def.arg(
            ArgDef::new("iothreadsched[0-9]*.iothreads", "iothreads")
                .can_comma()
                .find_inst(is.0, is.1),
        );
        def.arg(ArgDef::new("iothreadsched[0-9]*.scheduler", "scheduler").find_inst(is.0, is.1));
        def.arg(ArgDef::new("iothreadsched[0-9]*.priority", "priority").find_inst(is.0, is.1));

        // CPU Cache & Memory Tunables
        let ct = ("cachetune", "cachetunes");
        def.arg(
            ArgDef::new("cachetune[0-9]*.vcpus", "vcpus")
                .find_inst(ct.0, ct.1)
                .can_comma(),
        );
        for (cli, prop) in [
            ("cachetune[0-9]*.cache[0-9]*.id", "id"),
            ("cachetune[0-9]*.cache[0-9]*.level", "level"),
            ("cachetune[0-9]*.cache[0-9]*.type", "type"),
            ("cachetune[0-9]*.cache[0-9]*.size", "size"),
            ("cachetune[0-9]*.cache[0-9]*.unit", "unit"),
        ] {
            def.arg(ArgDef::new(cli, prop).find_inst_in(ct, ("cache", "caches")));
        }
        def.arg(
            ArgDef::new("cachetune[0-9]*.monitor[0-9]*.level", "level")
                .find_inst_in(ct, ("monitor", "monitors")),
        );
        def.arg(
            ArgDef::new("cachetune[0-9]*.monitor[0-9]*.vcpus", "vcpus")
                .find_inst_in(ct, ("monitor", "monitors"))
                .can_comma(),
        );
        let mt = ("memorytune", "memorytunes");
        def.arg(
            ArgDef::new("memorytune[0-9]*.vcpus", "vcpus")
                .find_inst(mt.0, mt.1)
                .can_comma(),
        );
        def.arg(
            ArgDef::new("memorytune[0-9]*.node[0-9]*.id", "id").find_inst_in(mt, ("node", "nodes")),
        );
        def.arg(
            ArgDef::new("memorytune[0-9]*.node[0-9]*.bandwidth", "bandwidth")
                .find_inst_in(mt, ("node", "nodes")),
        );
        def
    })
}

/// `ParserVCPU._parse`: `_add_advertised_aliases` then
/// `_convert_old_vcpu_opts`.
fn vcpu_pre_parse(_optstr: &str, o: &mut OptDict) -> Result<(), Error> {
    // These are essentially aliases for new style options, but we still
    // want to advertise them in --vcpus=help output because they are
    // historically commonly used. This should rarely, if ever, be extended
    if let Some(v) = o.remove("cpuset") {
        o.insert("vcpu.cpuset".into(), v);
    }
    if let Some(v) = o.remove("vcpus") {
        o.insert("vcpu".into(), v);
    }

    let havemax = o.get("maxvcpus").is_some();
    let havecur = o.get("vcpu.current").is_some();
    let havevcp = o.get("vcpu").is_some();
    if havecur {
        if havemax {
            let v = o.remove("maxvcpus").flatten();
            o.insert("vcpu".into(), v);
        }
    } else if havemax {
        if havevcp {
            let v = o.remove("vcpu").flatten();
            o.insert("vcpu.current".into(), v);
        }
        let v = o.remove("maxvcpus").flatten();
        o.insert("vcpu".into(), v);
    }
    Ok(())
}

/// `Guest.vcpus = val`.
fn set_vcpus_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    let val = match val {
        None => None,
        Some(v) => {
            let s = v.to_xml_string();
            Some(
                s.trim()
                    .parse::<i64>()
                    .map_err(|_| format!("invalid literal for int() with base 10: '{s}'"))?,
            )
        }
    };
    set_vcpus(inst, val)
}

/// `ParserVCPU.set_cpuset_cb`.
fn set_cpuset_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    let val = val.map(|v| v.to_xml_string());
    if val.as_deref() != Some("auto") {
        return inst.set("vcpu_cpuset", val.map(Value::Str));
    }
    // Previously we did our own one-time cpuset placement
    // based on current NUMA memory availability, but that's
    // pretty dumb unless the conditions on the host never change.
    // So instead use newer vcpu placement=
    inst.set("vcpu_placement", Some(Value::Str("auto".into())))
}

/// `ParserVCPU`.
pub fn vcpus() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new("vcpus", None, None);
        def.remove_first = &["vcpu"];
        def.pre_parse = Some(vcpu_pre_parse);
        // This is converted into either vcpu.current or vcpu
        def.arg(ArgDef::new("maxvcpus", "vcpus").cb(noset_cb));
        // These are handled in _add_advertised_aliases
        def.arg(
            ArgDef::new("cpuset", "vcpu_cpuset")
                .can_comma()
                .cb(noset_cb),
        );
        def.arg(ArgDef::new("vcpus", "vcpus").cb(noset_cb));

        // Further CPU options should be added to --cpu
        for (cli, prop) in [
            ("sockets", "cpu.topology.sockets"),
            ("dies", "cpu.topology.dies"),
            ("cores", "cpu.topology.cores"),
            ("threads", "cpu.topology.threads"),
        ] {
            def.arg(ArgDef::new(cli, prop));
        }

        // <domain><vcpu> options
        def.arg(ArgDef::new("vcpu", "vcpus").cb(set_vcpus_cb));
        def.arg(ArgDef::new("vcpu.current", "vcpu_current"));
        def.arg(
            ArgDef::new("vcpu.cpuset", "vcpu_cpuset")
                .can_comma()
                .cb(set_cpuset_cb),
        );
        def.arg(ArgDef::new("vcpu.placement", "vcpu_placement"));

        // <domain><vcpus> options
        let v = ("vcpu", "vcpulist.vcpu");
        def.arg(ArgDef::new("vcpus.vcpu[0-9]*.id", "id").find_inst(v.0, v.1));
        def.arg(
            ArgDef::new("vcpus.vcpu[0-9]*.enabled", "enabled")
                .find_inst(v.0, v.1)
                .onoff(),
        );
        def.arg(
            ArgDef::new("vcpus.vcpu[0-9]*.hotpluggable", "hotpluggable")
                .find_inst(v.0, v.1)
                .onoff(),
        );
        def.arg(ArgDef::new("vcpus.vcpu[0-9]*.order", "order").find_inst(v.0, v.1));

        def.alias("vcpu.placement", &["placement"]);
        def
    })
}
