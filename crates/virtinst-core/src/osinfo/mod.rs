//! Wraps `bindgen`-bound `libosinfo` (ticket 04) — upstream's
//! `osdict.py`. Pure GObject C API, no GTK in the link chain;
//! `osinfo-db` is a separately-versioned, distro-installed data
//! package with its own update tooling, not something this crate
//! vendors.
//!
//! **`bindgen`, not `gir`** — the module doc comment used to say
//! "gir-bound" (an assumption that crept in from tickets 09/14's
//! unrelated libvirt-glib/console decisions, never actually checked
//! against ticket 04's own research). Reasserted ticket 04's original
//! conclusion after an empirical comparison on this machine: `gir`'s
//! idiomatic-wrapper generation leaves most of the functions that
//! actually matter here (`Loader::process_default_path`/
//! `process_system_path`, `Db::os_list()`, `Db::guess_os_from_media()`,
//! `Db::identify_media()`) as commented-out stubs — its heuristics
//! don't auto-translate `(void, GError**)`-shaped functions or
//! libosinfo's custom `OsinfoList`-family container types, which is
//! most of this API's actual surface. Plain `bindgen` gets every
//! function unstubbed with a `build.rs` + `pkg-config`, no `Gir.toml`
//! override-authoring detour. See ticket 04's file for the raw numbers.

// TODO: pub struct OsDb { ... }
// TODO: pub struct OsInfo { ... }
