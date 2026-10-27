//! The `VirtCLIParser` subclasses of upstream's `cli.py`, as
//! [`ParserDef`]s, grouped by device family. Each `parser_*` function
//! builds its parser once and hands out a shared reference.

use crate::parser::ParserDef;

pub mod misc;

/// Every ported parser, in upstream's registration order (the order
/// `VIRT_PARSERS` runs them in).
pub fn all() -> Vec<&'static ParserDef> {
    vec![misc::watchdog(), misc::memballoon(), misc::panic()]
}

/// The parser for `--name` (the `cli_arg_name`), if ported.
pub fn by_name(cli_arg_name: &str) -> Option<&'static ParserDef> {
    all().into_iter().find(|p| p.cli_arg_name == cli_arg_name)
}
