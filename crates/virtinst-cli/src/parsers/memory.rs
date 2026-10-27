//! Guest memory options: `--memory`, `--memtune`, `--blkiotune`,
//! `--memorybacking`, `--numatune` and `--iothreads` (`ParserMemory`,
//! `ParserMemtune`, `ParserBlkiotune`, `ParserMemoryBacking`,
//! `ParserNumatune` and `ParserIOThreads`).

use std::sync::OnceLock;

use virtinst_core::domain::{blkiotune, memorybacking, memtune, numatune};
use virtinst_core::guest::set_current_memory;
use virtinst_core::xmlbuilder::{Value, XmlObject};

use crate::parser::{ArgDef, ArgInst, Error, OptDict, ParseCtx, ParserDef};

fn noset_cb(
    _ctx: &mut ParseCtx,
    _inst: &mut XmlObject,
    _val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    Ok(())
}

/// `ParserMemory._convert_old_memory_options`.
///
/// Historically the cli had:
///     memory -> ./currentMemory
///     maxmemory -> ./memory
/// Then later libvirt gained ./maxMemory. So things are quite a mess.
///
/// Try to convert the back compat cases. Basically if new style option
/// currentMemory is specified, interpret currentMemory and memory as
/// the XML values. Otherwise treat memory and maxmemory as the old
/// swapped names.
fn convert_old_memory_options(o: &mut OptDict) -> Result<(), Error> {
    let havecur = o.get("currentMemory").is_some();
    let havemax = o.get("maxmemory").is_some();
    let havemem = o.get("memory").is_some();
    if havecur {
        if havemax {
            let v = o.remove("maxmemory").flatten();
            o.insert("memory".into(), v);
        }
    } else if havemax {
        if havemem {
            let v = o.remove("memory").flatten();
            o.insert("currentMemory".into(), v);
        }
        let v = o.remove("maxmemory").flatten();
        o.insert("memory".into(), v);
    } else if havemem {
        let v = o.remove("memory").flatten();
        o.insert("currentMemory".into(), v);
    }
    Ok(())
}

/// `int(val) * 1024`.
fn mib_to_kib(val: Option<Value>) -> Result<i64, Error> {
    let s = val.map(|v| v.to_xml_string()).unwrap_or_default();
    let n = s
        .trim()
        .parse::<i64>()
        .map_err(|_| format!("invalid literal for int() with base 10: '{s}'"))?;
    Ok(n * 1024)
}

/// `ParserMemory.set_memory_cb`.
fn set_memory_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    arg: &ArgInst,
) -> Result<(), Error> {
    let kib = mib_to_kib(val)?;
    match arg.def.propname {
        Some("_currentMemory") => set_current_memory(inst, Some(kib)),
        Some(prop) => inst.set(prop, Some(Value::Int(kib))),
        None => Ok(()),
    }
}

/// `ParserMemory`.
pub fn memory() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new("memory", None, None);
        def.remove_first = &["memory"];
        def.pre_parse = Some(convert_old_memory_options);
        def.arg(ArgDef::new("memory", "memory").cb(set_memory_cb));
        def.arg(ArgDef::new("currentMemory", "_currentMemory").cb(set_memory_cb));
        def.arg(ArgDef::new("maxMemory", "maxMemory").cb(set_memory_cb));
        def.arg(ArgDef::new("maxMemory.slots", "maxMemorySlots"));

        // This is converted into either memory or currentMemory
        def.arg(ArgDef::cb_only("maxmemory", noset_cb).no_lookup());

        // New memoryBacking properties should be added to the --memorybacking
        def.arg(ArgDef::new("hugepages", "memoryBacking.hugepages").onoff());

        def.alias("maxMemory.slots", &["hotplugmemoryslots"]);
        def.alias("maxMemory", &["hotplugmemorymax"]);
        def
    })
}

/// `ParserMemtune`.
pub fn memtune() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new("memtune", Some("memtune"), Some(&memtune::DOMAIN_MEMTUNE));
        def.remove_first = &["soft_limit"];
        for name in [
            "hard_limit",
            "soft_limit",
            "swap_hard_limit",
            "min_guarantee",
        ] {
            def.arg(ArgDef::new(name, name));
        }
        def
    })
}

/// `ParserBlkiotune`.
pub fn blkiotune() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new(
            "blkiotune",
            Some("blkiotune"),
            Some(&blkiotune::DOMAIN_BLKIOTUNE),
        );
        def.remove_first = &["weight"];
        def.arg(ArgDef::new("weight", "weight"));
        for (cli, prop, alias) in [
            ("device[0-9]*.path", "path", "device_path"),
            ("device[0-9]*.weight", "weight", "device_weight"),
            (
                "device[0-9]*.read_bytes_sec",
                "read_bytes_sec",
                "read_bytes_sec",
            ),
            (
                "device[0-9]*.write_bytes_sec",
                "write_bytes_sec",
                "write_bytes_sec",
            ),
            (
                "device[0-9]*.read_iops_sec",
                "read_iops_sec",
                "read_iops_sec",
            ),
            (
                "device[0-9]*.write_iops_sec",
                "write_iops_sec",
                "write_iops_sec",
            ),
        ] {
            def.arg(ArgDef::new(cli, prop).find_inst("device", "devices"));
            def.alias(cli, &[alias]);
        }
        def
    })
}

/// `ParserMemoryBacking`.
pub fn memorybacking() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new(
            "memorybacking",
            Some("memoryBacking"),
            Some(&memorybacking::DOMAIN_MEMORY_BACKING),
        );
        def.arg(ArgDef::new("hugepages", "hugepages").onoff());
        def.arg(ArgDef::new("hugepages.page[0-9]*.size", "size").find_inst("page", "pages"));
        def.arg(ArgDef::new("hugepages.page[0-9]*.unit", "unit").find_inst("page", "pages"));
        def.arg(ArgDef {
            can_comma: true,
            ..ArgDef::new("hugepages.page[0-9]*.nodeset", "nodeset").find_inst("page", "pages")
        });

        def.arg(ArgDef::new("nosharepages", "nosharepages").onoff());
        def.arg(ArgDef::new("locked", "locked").onoff());
        def.arg(ArgDef::new("access.mode", "access_mode"));
        def.arg(ArgDef::new("source.type", "source_type"));
        def.arg(ArgDef::new("discard", "discard").onoff());
        def.arg(ArgDef::new("allocation.mode", "allocation_mode"));
        def.arg(ArgDef::new("allocation.threads", "allocation_threads"));

        def.alias("hugepages.page[0-9]*.size", &["size"]);
        def.alias("hugepages.page[0-9]*.unit", &["unit"]);
        def.alias("hugepages.page[0-9]*.nodeset", &["nodeset"]);
        def.alias("access.mode", &["access_mode"]);
        def.alias("source.type", &["source_type"]);
        def
    })
}

/// `ParserNumatune`.
pub fn numatune() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new(
            "numatune",
            Some("numatune"),
            Some(&numatune::DOMAIN_NUMATUNE),
        );
        def.remove_first = &["nodeset"];
        def.arg(ArgDef::new("memory.nodeset", "memory_nodeset").can_comma());
        def.arg(ArgDef::new("memory.mode", "memory_mode"));
        def.arg(ArgDef::new("memory.placement", "memory_placement"));

        def.arg(
            ArgDef::new("memnode[0-9]*.cellid", "cellid")
                .can_comma()
                .find_inst("memnode", "memnode"),
        );
        def.arg(ArgDef::new("memnode[0-9]*.mode", "mode").find_inst("memnode", "memnode"));
        def.arg(
            ArgDef::new("memnode[0-9]*.nodeset", "nodeset")
                .can_comma()
                .find_inst("memnode", "memnode"),
        );

        def.alias("memory.mode", &["mode"]);
        def.alias("memory.nodeset", &["nodeset"]);
        def
    })
}

/// `ParserIOThreads`.
pub fn iothreads() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new("iothreads", Some("iothreads"), None);
        def.remove_first = &["iothreads"];
        // Options for IOThreads config
        def.arg(ArgDef::new("iothreads", "iothreads"));
        let ids = ("iothread", "iothreadids");
        let dflt = ("defaultiothread", "defaultiothread");
        def.arg(ArgDef::new("iothreadids.iothread[0-9]*.id", "id").find_inst(ids.0, ids.1));
        def.arg(
            ArgDef::new(
                "iothreadids.iothread[0-9]*.thread_pool_min",
                "thread_pool_min",
            )
            .find_inst(ids.0, ids.1),
        );
        def.arg(
            ArgDef::new(
                "iothreadids.iothread[0-9]*.thread_pool_max",
                "thread_pool_max",
            )
            .find_inst(ids.0, ids.1),
        );
        def.arg(
            ArgDef::new("defaultiothread.thread_pool_min", "thread_pool_min")
                .find_inst(dflt.0, dflt.1),
        );
        def.arg(
            ArgDef::new("defaultiothread.thread_pool_max", "thread_pool_max")
                .find_inst(dflt.0, dflt.1),
        );
        def.arg(ArgDef::new("iothreadids.iothread[0-9]*.poll.max", "max").find_inst(ids.0, ids.1));
        def.arg(
            ArgDef::new("iothreadids.iothread[0-9]*.poll.grow", "grow").find_inst(ids.0, ids.1),
        );
        def.arg(
            ArgDef::new("iothreadids.iothread[0-9]*.poll.shrink", "shrink").find_inst(ids.0, ids.1),
        );
        def
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_guest_edit;
    use virtinst_core::guest::GUEST;

    #[test]
    fn memory_backcompat_names() {
        let xml = "<domain><name>x</name><memory>4194304</memory>\
                   <currentMemory>4194304</currentMemory></domain>";
        let mut guest = XmlObject::parse(&GUEST, xml).unwrap();
        parse_guest_edit(memory(), &mut guest, "512,maxmemory=1024").unwrap();
        assert_eq!(guest.get("memory"), Some(Value::Int(1048576)));
        assert_eq!(guest.get("_currentMemory"), Some(Value::Int(524288)));
    }

    #[test]
    fn current_memory_grows_memory() {
        let xml = "<domain><name>x</name><memory>1024</memory></domain>";
        let mut guest = XmlObject::parse(&GUEST, xml).unwrap();
        parse_guest_edit(memory(), &mut guest, "currentMemory=8").unwrap();
        assert_eq!(guest.get("memory"), Some(Value::Int(8192)));
    }
}
