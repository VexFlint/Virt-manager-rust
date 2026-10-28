//! `--watchdog`, `--memballoon`, `--panic`, `--shmem`, `--memdev`,
//! `--iommu` and `--pstore`: `ParserWatchdog`, `ParserMemballoon`,
//! `ParserPanic`, `ParserShMem`, `ParserMemdev`, `ParserIommu` and
//! `ParserPstore`.

use std::sync::OnceLock;

use virtinst_core::devices::{
    iommu, memballoon, memory, panic as panicdev, pstore, shmem, watchdog,
};
use virtinst_core::xmlbuilder::{Value, XmlObject};

use crate::parser::{add_common_device_args, ArgDef, ArgInst, Error, OptDict, ParseCtx, ParserDef};

/// `ParserWatchdog`.
pub fn watchdog() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new(
            "watchdog",
            Some("devices.watchdog"),
            Some(&watchdog::DEVICE_WATCHDOG),
        );
        def.remove_first = &["model"];
        add_common_device_args(&mut def, false, false);
        def.arg(ArgDef::new("model", "model").ignore_default());
        def.arg(ArgDef::new("action", "action").ignore_default());
        def
    })
}

/// `ParserMemballoon`.
pub fn memballoon() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new(
            "memballoon",
            Some("devices.memballoon"),
            Some(&memballoon::DEVICE_MEMBALLOON),
        );
        def.remove_first = &["model"];
        def.stub_none = false;
        add_common_device_args(&mut def, false, true);
        def.arg(ArgDef::new("model", "model").ignore_default());
        def.arg(ArgDef::new("autodeflate", "autodeflate").onoff());
        def.arg(ArgDef::new("stats.period", "stats_period"));
        def.arg(ArgDef::new("freePageReporting", "freePageReporting").onoff());
        def
    })
}

/// `ParserPanic._parse`: old style `--panic 0xFOO` sets the iobase.
fn panic_pre_parse(_optstr: &str, optdict: &mut OptDict) -> Result<(), String> {
    let model = optdict.get("model").cloned().flatten().unwrap_or_default();
    if optdict.len() == 1 && model.starts_with("0x") {
        optdict.insert("address.iobase".into(), Some(model));
        optdict.insert("model".into(), Some("isa".into()));
    }
    Ok(())
}

/// `ParserPanic`.
pub fn panic() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new(
            "panic",
            Some("devices.panic"),
            Some(&panicdev::DEVICE_PANIC),
        );
        def.remove_first = &["model"];
        def.pre_parse = Some(panic_pre_parse);
        add_common_device_args(&mut def, false, false);
        def.alias("address.iobase", &["iobase"]);
        def.arg(ArgDef::new("model", "model").ignore_default());
        def
    })
}

/// `ParserShMem`.
pub fn shmem() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new("shmem", Some("devices.shmem"), Some(&shmem::DEVICE_SHMEM));
        def.remove_first = &["name"];
        add_common_device_args(&mut def, false, false);
        def.arg(ArgDef::new("name", "name"));
        def.arg(ArgDef::new("role", "role"));
        def.arg(ArgDef::new("model.type", "type"));
        def.arg(ArgDef::new("size", "size"));
        def.arg(ArgDef::new("size.unit", "size_unit"));
        def.arg(ArgDef::new("server.path", "server_path"));
        def.arg(ArgDef::new("msi.vectors", "msi_vectors"));
        def.arg(ArgDef::new("msi.ioeventfd", "msi_ioeventfd"));
        def
    })
}

/// `ParserMemdev.set_target_size`: the value is in MiB, the XML in KiB.
fn set_target_size(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    arg: &ArgInst,
) -> Result<(), Error> {
    let kib = match &val {
        Some(Value::Int(i)) => *i,
        Some(v) => {
            let s = v.to_xml_string();
            s.trim()
                .parse::<i64>()
                .map_err(|_| format!("invalid literal for int() with base 10: '{s}'"))?
        }
        None => {
            return Err(
                "int() argument must be a string, a bytes-like object or a real \
                        number, not 'NoneType'"
                    .into(),
            )
        }
    } * 1024;
    inst.set_path(
        arg.propname().expect("has a propname"),
        Some(Value::Int(kib)),
    )
}

/// `ParserMemdev`.
pub fn memdev() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new(
            "memdev",
            Some("devices.memory"),
            Some(&memory::DEVICE_MEMORY),
        );
        def.remove_first = &["model"];
        add_common_device_args(&mut def, false, false);
        def.arg(ArgDef::new("model", "model"));
        def.arg(ArgDef::new("access", "access"));
        def.arg(ArgDef::new("discard", "discard").onoff());
        def.arg(ArgDef::new("uuid", "uuid"));
        def.arg(ArgDef::new("target.size", "target.size").cb(set_target_size));
        def.arg(ArgDef::new("target.node", "target.node"));
        def.arg(ArgDef::new("target.label_size", "target.label_size").cb(set_target_size));
        def.arg(ArgDef::new("target.block", "target.block"));
        def.arg(ArgDef::new("target.current", "target.current"));
        def.arg(ArgDef::new("target.requested", "target.requested"));
        def.arg(ArgDef::new("target.readonly", "target.readonly").onoff());
        def.arg(ArgDef::new("target.address_base", "target.address_base"));
        def.arg(ArgDef::new("source.pagesize", "source.pagesize"));
        def.arg(ArgDef::new("source.path", "source.path"));
        def.arg(ArgDef::new("source.nodemask", "source.nodemask").can_comma());
        def.arg(ArgDef::new("source.pmem", "source.pmem").onoff());
        def.arg(ArgDef::new("source.alignsize", "source.alignsize").cb(set_target_size));
        def.arg(ArgDef::new(
            "target.dynamicMemslots",
            "target.dynamicMemslots",
        ));
        def.alias("target.size", &["target_size"]);
        def.alias("target.node", &["target_node"]);
        def.alias("target.label_size", &["target_label_size"]);
        def.alias("source.pagesize", &["source_pagesize"]);
        def.alias("source.path", &["source_path"]);
        def.alias("source.nodemask", &["source_nodemask"]);
        def
    })
}

/// `ParserIommu`.
pub fn iommu() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new("iommu", Some("devices.iommu"), Some(&iommu::DEVICE_IOMMU));
        def.remove_first = &["model"];
        def.arg(ArgDef::new("model", "model"));
        def.arg(ArgDef::new("driver.aw_bits", "aw_bits"));
        def.arg(ArgDef::new("driver.intremap", "intremap").onoff());
        def.arg(ArgDef::new("driver.caching_mode", "caching_mode").onoff());
        def.arg(ArgDef::new("driver.eim", "eim").onoff());
        def.arg(ArgDef::new("driver.iotlb", "iotlb").onoff());
        def
    })
}

/// `ParserPstore`.
pub fn pstore() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new(
            "pstore",
            Some("devices.pstore"),
            Some(&pstore::DEVICE_PSTORE),
        );
        def.arg(ArgDef::new("backend", "backend"));
        def.arg(ArgDef::new("path", "path"));
        def.arg(ArgDef::new("size", "size"));
        def
    })
}

/// `dev.set_defaults(guest)` for the devices these parsers make.
pub fn set_defaults(
    cli_arg_name: &str,
    dev: &mut virtinst_core::xmlbuilder::XmlObject,
) -> Result<(), String> {
    match cli_arg_name {
        "watchdog" => watchdog::set_defaults(dev),
        "memballoon" => memballoon::set_defaults(dev),
        "panic" => panicdev::set_defaults(dev),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{find_objects_to_edit, lookup_prop, parse_edit, parse_new};
    use virtinst_core::guest::GUEST;
    use virtinst_core::xmlbuilder::XmlObject;

    fn build(def: &ParserDef, optstr: &str) -> Result<String, String> {
        let mut guest = XmlObject::new(&GUEST);
        let idx = parse_new(def, &mut guest, optstr)?;
        let dev = &mut lookup_prop(&mut guest, def)[idx[0]];
        set_defaults(def.cli_arg_name, dev)?;
        Ok(dev.get_xml().trim_end().to_string())
    }

    /// `virt-install --watchdog ib700,action=pause` (test_cli.py's
    /// many-devices case) and the defaults of `--watchdog default`.
    #[test]
    fn watchdog_builds_like_upstream() {
        assert_eq!(
            build(watchdog(), "ib700,action=pause").unwrap(),
            r#"<watchdog model="ib700" action="pause"/>"#
        );
        assert_eq!(
            build(watchdog(), "default").unwrap(),
            r#"<watchdog model="i6300esb" action="reset"/>"#
        );
        assert_eq!(
            build(watchdog(), "bogus=1").unwrap_err(),
            "Unknown --watchdog options: ['bogus']"
        );
        assert_eq!(
            build(watchdog(), "ib700,action").unwrap_err(),
            "Error: --watchdog ib700,action: Option 'action' had no value set."
        );
    }

    #[test]
    fn memballoon_builds_like_upstream() {
        assert_eq!(
            build(
                memballoon(),
                "virtio,autodeflate=on,stats.period=10,freePageReporting=on"
            )
            .unwrap(),
            "<memballoon model=\"virtio\" autodeflate=\"on\" freePageReporting=\"on\">\n  \
             <stats period=\"10\"/>\n</memballoon>"
        );
        assert_eq!(
            build(memballoon(), "none").unwrap(),
            r#"<memballoon model="none"/>"#
        );
        assert_eq!(
            build(memballoon(), "autodeflate=maybe").unwrap_err(),
            "autodeflate must be 'yes' or 'no'"
        );
    }

    #[test]
    fn panic_builds_like_upstream() {
        assert_eq!(
            build(panic(), "0x505").unwrap(),
            "<panic model=\"isa\">\n  <address type=\"isa\" iobase=\"0x505\"/>\n</panic>"
        );
        assert_eq!(build(panic(), "default").unwrap(), "<panic/>");
        assert_eq!(
            build(
                panic(),
                "iobase=507,,address.type=isa,address.iobase=0x500,address.irq=5"
            )
            .unwrap(),
            "<panic>\n  <address type=\"isa\" iobase=\"0x500\" irq=\"5\"/>\n</panic>"
        );
    }

    #[test]
    fn edit_and_select_existing_devices() {
        let xml = "<domain><name>x</name><devices>\
                   <watchdog model='ib700' action='poweroff'/>\
                   <memballoon model='virtio'/></devices></domain>";
        let mut guest = XmlObject::parse(&GUEST, xml).unwrap();
        let idx = find_objects_to_edit(watchdog(), &mut guest, "edit", None).unwrap();
        parse_edit(watchdog(), &mut guest, idx[0], "action=reset").unwrap();
        let out = guest.get_document_xml();
        assert!(
            out.contains(r#"<watchdog model="ib700" action="reset"/>"#),
            "{out}"
        );

        assert_eq!(
            find_objects_to_edit(watchdog(), &mut guest, "edit", Some("ib700")).unwrap(),
            [0]
        );
        assert_eq!(
            find_objects_to_edit(watchdog(), &mut guest, "edit", Some("i6300esb")).unwrap_err(),
            "No matching objects found for --edit i6300esb"
        );
        assert_eq!(
            find_objects_to_edit(watchdog(), &mut guest, "edit", Some("3")).unwrap_err(),
            "'--edit 3' requested but there's only 1 --watchdog object in the XML"
        );
        assert_eq!(
            find_objects_to_edit(panic(), &mut guest, "edit", None).unwrap_err(),
            "No --panic objects found in the XML"
        );

        let idx = find_objects_to_edit(memballoon(), &mut guest, "edit", None).unwrap();
        parse_edit(memballoon(), &mut guest, idx[0], "none").unwrap();
        let out = guest.get_document_xml();
        assert!(out.contains(r#"<memballoon model="none"/>"#), "{out}");
    }

    /// `--memdev` sizes are MiB on the command line, KiB in the XML
    /// (test_cli.py's build-memdev case).
    #[test]
    fn memdev_sizes_are_mib() {
        let xml = build(
            memdev(),
            "nvdimm,source.path=/path/to/nvdimm,target.size=2,target.node=0,target.label_size=1",
        )
        .unwrap();
        assert!(xml.contains("<size>2048</size>"), "{xml}");
        assert!(xml.contains("<label>\n      <size>1024</size>"), "{xml}");
        assert_eq!(
            build(memdev(), "dimm,target.size=1G").unwrap_err(),
            "Error: --memdev dimm,target.size=1G: invalid literal for int() with base 10: '1G'"
        );
    }
}
