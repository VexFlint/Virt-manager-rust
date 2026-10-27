//! `--serial`, `--parallel`, `--channel` and `--console`: upstream's
//! `_ParserChar` subclasses.

use std::sync::OnceLock;

use virtinst_core::devices::char as chardev;
use virtinst_core::xmlbuilder::{Value, XmlClass, XmlObject};

use super::charsrc::add_char_source_args;
use crate::parser::{add_common_device_args, ArgDef, ArgInst, Error, ParseCtx, ParserDef};

/// `_ParserChar._parse`, with `_add_advertised_aliases`.
fn char_parse(ctx: &mut ParseCtx) -> Result<Option<Vec<XmlObject>>, Error> {
    let name = ctx.def.cli_arg_name;
    if ctx.optstr == "none" && (name == "console" || name == "channel") {
        // `self.guest.skip_default_console/channel = True`: only
        // virt-install's default devices read it.
        return Ok(Some(vec![]));
    }
    // These are essentially aliases for new style options, but we still
    // want to advertise them in --$OPT=help output because they are
    // historically commonly used. This should rarely, if ever, be extended
    for (old, new) in [
        ("path", "source.path"),
        ("mode", "source.mode"),
        ("bind_host", "source.bind_host"),
    ] {
        if let Some(val) = ctx.optdict.remove(old) {
            ctx.optdict.insert(new.into(), val);
        }
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

/// `_ParserChar.set_host_cb`.
fn set_host_cb(
    ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    let val = val.map(|v| v.to_xml_string());
    let mode = ctx.optdict.get("source.mode").cloned().flatten();
    if ctx.optdict.get("source.bind_host").is_none() && mode.as_deref() == Some("bind") {
        chardev::set_friendly_bind(inst.child_mut("source"), val.as_deref())
    } else {
        chardev::set_friendly_connect(inst.child_mut("source"), val.as_deref())
    }
}

/// `_ParserChar.set_target_cb`.
fn set_target_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    let val = val.map(|v| v.to_xml_string());
    chardev::set_friendly_target(inst, val.as_deref())
}

/// `_ParserChar._virtcli_class_init`.
fn char_parser(name: &'static str, propname: &'static str, class: &'static XmlClass) -> ParserDef {
    let mut def = ParserDef::new(name, Some(propname), Some(class));
    def.remove_first = &["type"];
    def.stub_none = false;
    def.custom_parse = Some(char_parse);
    add_common_device_args(&mut def, false, false);
    def.arg(ArgDef::new("type", "type"));
    // These are handled in _add_advertised_aliases
    def.arg(ArgDef::new("path", "source.path").cb(noset_cb));
    def.arg(ArgDef::new("mode", "source.mode").cb(noset_cb));
    def.arg(ArgDef::new("bind_host", "source.bind_host").cb(noset_cb));
    // Old backcompat argument
    def.arg(ArgDef::new("host", "source.host").cb(set_host_cb));
    add_char_source_args(&mut def, "");
    def.arg(ArgDef::new("target.address", "target_address").cb(set_target_cb));
    def.arg(ArgDef::new("target.type", "target_type"));
    def.arg(ArgDef::new("target.name", "target_name"));
    def.arg(ArgDef::new("target.port", "target_port"));
    def.arg(ArgDef::new("target.model.name", "target_model_name"));
    def.alias("type", &["char_type"]);
    def.alias("protocol.type", &["protocol"]);
    def.alias("target.address", &["target_address"]);
    def.alias("target.type", &["target_type"]);
    def.alias("target.name", &["name"]);
    def
}

/// `ParserSerial`.
pub fn serial() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| char_parser("serial", "devices.serial", &chardev::DEVICE_SERIAL))
}

/// `ParserParallel`.
pub fn parallel() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| char_parser("parallel", "devices.parallel", &chardev::DEVICE_PARALLEL))
}

/// `ParserChannel`.
pub fn channel() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| char_parser("channel", "devices.channel", &chardev::DEVICE_CHANNEL))
}

/// `ParserConsole`.
pub fn console() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| char_parser("console", "devices.console", &chardev::DEVICE_CONSOLE))
}
