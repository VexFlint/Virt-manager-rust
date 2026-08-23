//! `bindgen` against `libosinfo`'s plain C API (ticket 04, reasserted
//! 2026-08-23 over a `gir`-bound assumption that crept into other
//! tickets — see that ticket's amendment for the empirical comparison
//! this decision rests on).
//!
//! Two real gotchas, both found and documented in ticket 04's own
//! research (`docs/research/libosinfo-bindgen.md`) before this file
//! was written, not discovered here:
//! - `libosinfo-1.0.pc`'s declared `Requires` is `gobject-2.0` alone,
//!   but a public header (`osinfo_db.h`) directly `#include`s
//!   `<gio/gio.h>` — `gio-2.0`'s cflags have to be added by hand.
//! - libosinfo's headers transitively pull in the *entire* glib/
//!   gobject/gio type universe (`GError`, `GObject`, `GList`, …).
//!   Blocklisting all of it (`^G[A-Z_].*` — safe since every real
//!   `libosinfo` type is `Osinfo`-prefixed) and pointing bindgen at
//!   the real `glib-sys`/`gobject-sys`/`gio-sys` crates via
//!   `raw_line`s is what keeps the generated FFI interoperable with
//!   those crates' own types, instead of a second, incompatible set
//!   of definitions for the same C structs.

fn main() {
    let libosinfo = pkg_config::Config::new()
        .atleast_version("1.0")
        .probe("libosinfo-1.0")
        .expect("libosinfo-1.0 not found via pkg-config — install libosinfo-1.0-dev");
    // The Requires: gotcha above — gio-2.0's cflags aren't pulled in
    // by libosinfo-1.0's own .pc file, so ask for them directly.
    let gio = pkg_config::Config::new()
        .probe("gio-2.0")
        .expect("gio-2.0 not found via pkg-config — install libglib2.0-dev");

    let mut builder = bindgen::Builder::default()
        .header("osinfo_wrapper.h")
        .allowlist_function("osinfo_.*")
        .allowlist_type("Osinfo.*")
        .allowlist_var("OSINFO_.*")
        // Every real glib/gobject/gio type starts with `G` followed by
        // an uppercase letter or underscore (`GError`, `GObject`,
        // `GType`, `_GList`, …) — none of libosinfo's own types do
        // (all `Osinfo`-prefixed), so this blocklist can't shadow
        // anything this crate actually needs from libosinfo itself.
        .blocklist_type("_?G[A-Z_].*")
        .blocklist_function("g_.*")
        .blocklist_item("G_.*")
        // Pull the real definitions back in from the crates this
        // workspace already depends on, so the generated FFI's
        // `*mut GError`/`*mut GObject`/etc. fields and parameters
        // resolve to the *same* types calling code elsewhere already
        // uses — not a second, incompatible redefinition.
        // Broader than strictly needed on purpose — libosinfo's own
        // header surface shifts across versions/distros (see ticket
        // 04's osinfo-db-versioning note), so this errs toward "still
        // resolves after a minor libosinfo bump" over a minimal set
        // that breaks on the next added struct field. `ffi.rs`'s own
        // module-level `#![allow(unused_imports)]`-style attribute
        // absorbs whichever subset an actual build doesn't reference
        // (an inner attribute here would apply to this `use` alone,
        // not the whole included file).
        .raw_line("use glib_sys::{GError, GList, GSList, GQueue, GDate, GHashTable, GHashTableIter, GDestroyNotify, GQuark, GType};")
        .raw_line("use gobject_sys::{GObject, GObjectClass, GInitiallyUnowned, GInitiallyUnownedClass, GTypeInstance, GTypeClass, GTypeInterface, GParamSpec, GValue};")
        .raw_line("use gio_sys::{GCancellable, GAsyncResult, GAsyncReadyCallback, GFile};")
        .derive_default(true)
        .layout_tests(false);

    for path in libosinfo
        .include_paths
        .iter()
        .chain(gio.include_paths.iter())
    {
        builder = builder.clang_arg(format!("-I{}", path.display()));
    }

    let bindings = builder
        .generate()
        .expect("bindgen failed to generate libosinfo FFI bindings");

    let out_path = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("osinfo_bindings.rs"))
        .expect("failed to write osinfo_bindings.rs");
}
