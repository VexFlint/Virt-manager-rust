//! Raw `bindgen`-generated FFI for `libosinfo` — see `build.rs` and
//! ticket 04. Not hand-written and not meant to be read like the rest
//! of this crate; the safe wrapper in `mod.rs` is the real API.
#![allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_imports,
    clippy::all
)]

include!(concat!(env!("OUT_DIR"), "/osinfo_bindings.rs"));
