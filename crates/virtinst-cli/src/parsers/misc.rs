//! `--watchdog`, `--memballoon` and `--panic`: `ParserWatchdog`,
//! `ParserMemballoon` and `ParserPanic`.

use std::sync::OnceLock;

use virtinst_core::devices::{memballoon, panic as panicdev, watchdog};

use crate::parser::{add_common_device_args, ArgDef, OptDict, ParserDef};

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
fn panic_pre_parse(optdict: &mut OptDict) -> Result<(), String> {
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
}
