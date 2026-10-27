//! The argument groups upstream shares between parsers:
//! `_add_device_seclabel_args` and `_add_char_source_args`.

use virtinst_core::devices::char as chardev;
use virtinst_core::xmlbuilder::{Value, XmlObject};

use crate::parser::{ArgDef, ArgInst, Error, ParseCtx, ParserDef};

/// `prefix + cliname`, kept for the parser's lifetime.
fn prefixed(prefix: &str, cliname: &str) -> &'static str {
    Box::leak(format!("{prefix}{cliname}").into_boxed_str())
}

/// `_add_device_seclabel_args`.
pub fn add_device_seclabel_args(def: &mut ParserDef, list_propname: &'static str, prefix: &str) {
    let arg = |cli: &str, prop: &'static str| {
        ArgDef::new(prefixed(prefix, cli), prop).find_inst("seclabel", list_propname)
    };
    // DeviceDisk.seclabels properties
    def.arg(arg("source.seclabel[0-9]*.model", "model"));
    def.arg(arg("source.seclabel[0-9]*.relabel", "relabel").onoff());
    def.arg(arg("source.seclabel[0-9]*.label", "label").can_comma());
}

fn str_val(val: &Option<Value>) -> Option<String> {
    val.as_ref().map(|v| v.to_xml_string())
}

fn set_sourcehost_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    chardev::set_friendly_host(inst.child_mut("source"), str_val(&val).as_deref())
}

fn set_bind_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    chardev::set_friendly_bind(inst.child_mut("source"), str_val(&val).as_deref())
}

fn set_connect_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    chardev::set_friendly_connect(inst.child_mut("source"), str_val(&val).as_deref())
}

/// `_add_char_source_args`: the `CharSource` options, shared among
/// several device types.
pub fn add_char_source_args(def: &mut ParserDef, prefix: &str) {
    let arg = |cli: &str, prop: &'static str| ArgDef::new(prefixed(prefix, cli), prop);
    def.arg(arg("source.path", "source.path"));
    def.arg(arg("source.host", "source.host").cb(set_sourcehost_cb));
    def.arg(arg("source.service", "source.service"));
    def.arg(arg("source.bind_host", "source.bind_host").cb(set_bind_cb));
    def.arg(arg("source.bind_service", "source.bind_service"));
    def.arg(arg("source.connect_host", "source.connect_host").cb(set_connect_cb));
    def.arg(arg("source.connect_service", "source.connect_service"));
    def.arg(arg("source.mode", "source.mode"));
    def.arg(arg("source.master", "source.master"));
    def.arg(arg("source.slave", "source.slave"));
    add_device_seclabel_args(def, "source.seclabels", prefix);
    def.arg(arg("protocol.type", "source.protocol"));
    def.arg(arg("log.file", "source.log_file"));
    def.arg(arg("log.append", "source.log_append").onoff());
    def.arg(arg("source.clipboard.copypaste", "source.clipboard_copypaste").onoff());
    def.arg(arg("source.mouse.mode", "source.mouse_mode"));
    def.arg(arg("source.channel", "source.channel"));
    def.arg(arg("source.tls", "source.tls").onoff());
}
