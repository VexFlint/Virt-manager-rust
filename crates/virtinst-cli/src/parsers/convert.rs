//! `--convert-to-q35` and `--convert-to-vnc`: `ParserConvertToQ35` and
//! `ParserConvertToVNC`. Their `parse` hands the options to
//! `guest.convert_to_q35`/`convert_to_vnc`, which need the connection
//! for device defaults: the parse leaves the request in [`take_pending`]
//! for the caller to run on the guest.

use std::cell::RefCell;
use std::sync::OnceLock;

use virtinst_core::xmlbuilder::{Value, XmlObject};

use crate::parser::{ArgDef, ArgInst, Error, ParseCtx, ParserDef};

/// A parsed conversion, still to run on the guest.
#[derive(Clone, Debug, PartialEq)]
pub enum Convert {
    /// `convert_to_q35(num_pcie_root_ports=...)`.
    Q35 { num_pcie_root_ports: Option<String> },
    /// `convert_to_vnc(qemu_vdagent=...)`.
    Vnc { qemu_vdagent: bool },
}

thread_local! {
    static PENDING: RefCell<Vec<Convert>> = const { RefCell::new(Vec::new()) };
}

/// The conversions parsed since the last call, in parse order.
pub fn take_pending() -> Vec<Convert> {
    PENDING.with(|p| std::mem::take(&mut *p.borrow_mut()))
}

/// The options are attributes of a plain data object
/// (`ConvertToQ35Data`), not XML.
fn stash_cb(
    ctx: &mut ParseCtx,
    _inst: &mut XmlObject,
    val: Option<Value>,
    arg: &ArgInst,
) -> Result<(), Error> {
    let val = val.map(|v| v.to_xml_string());
    ctx.stash.insert(arg.def.cliname.into(), val);
    Ok(())
}

fn stashed(ctx: &ParseCtx, name: &str) -> Option<String> {
    ctx.stash.get(name).cloned().flatten()
}

fn q35_parse(ctx: &mut ParseCtx, _guest: &mut XmlObject) -> Result<(), Error> {
    let num_pcie_root_ports = stashed(ctx, "num_pcie_root_ports");
    PENDING.with(|p| {
        p.borrow_mut().push(Convert::Q35 {
            num_pcie_root_ports,
        })
    });
    Ok(())
}

fn vnc_parse(ctx: &mut ParseCtx, _guest: &mut XmlObject) -> Result<(), Error> {
    // `if qemu_vdagent:`: any non-empty string.
    let qemu_vdagent = stashed(ctx, "qemu-vdagent").is_some_and(|v| !v.is_empty());
    PENDING.with(|p| p.borrow_mut().push(Convert::Vnc { qemu_vdagent }));
    Ok(())
}

fn convert_parser(
    name: &'static str,
    arg: &'static str,
    post: crate::parser::PostParse,
) -> ParserDef {
    let mut def = ParserDef::new(name, None, None);
    def.supports_clearxml = false;
    def.args.retain(|a| a.cliname != "clearxml");
    def.parse_empty = true;
    def.arg(ArgDef::cb_only(arg, stash_cb));
    def.post_parse = Some(post);
    def
}

/// `ParserConvertToQ35`.
pub fn convert_to_q35() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| convert_parser("convert_to_q35", "num_pcie_root_ports", q35_parse))
}

/// `ParserConvertToVNC`.
pub fn convert_to_vnc() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| convert_parser("convert_to_vnc", "qemu-vdagent", vnc_parse))
}
