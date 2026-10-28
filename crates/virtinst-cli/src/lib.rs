//! Shared CLI-shell code for `virt-install`/`virt-clone`/`virt-xml`
//! (ticket 16) — the thin, GTK-free counterpart to the GUI-shell layer
//! in `virt-manager`: stdout progress instead of a modal, `y/n` prompts
//! instead of confirm dialogs. Sits on the exact same `virtinst-core`
//! the GUI uses; nothing here reaches back into core.
//!
//! The port of upstream's `VirtCLIParser` lives in [`parser`] and
//! [`parsers`].

pub mod cli;
pub mod parser;
pub mod parsers;
pub mod xmlutil;
