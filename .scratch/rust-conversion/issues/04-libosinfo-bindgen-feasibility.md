---
id: 04
title: libosinfo bindgen feasibility
type: research
status: resolved
blocked_by: []
claimed_by: null
---

## Question

Is `bindgen` against `libosinfo`'s C API (not the GObject-Introspection
`.gir`/`.typelib` path) actually clean in practice for full OS-detection/
osinfo-db parity? Cover: what GLib/GObject dependency footprint it pulls in
(and whether that's avoidable without pulling GTK itself), what
initialization the C API requires, and how osinfo-db's XML data files are
distributed/versioned on a target system (system package vs bundled copy) —
since virt-manager's OS-defaults logic depends on that data being current.

This feeds the (currently fog) OS-detection/create-VM-wizard work, once
tickets 05 and 06 land.

## Resolution

`bindgen` against libosinfo's plain C API is practical. Verified against the
real installed packages plus upstream source: the public API is pure
GObject with `Requires: gobject-2.0` and no GTK anywhere in the link chain
(`ldd` confirms only glib/gobject/gio/libsoup/libxml2/libxslt). Init is a
tiny synchronous sequence (`osinfo_loader_new` → `process_default_path` →
`osinfo_loader_get_db`), no `g_type_init()` needed on modern GLib. osinfo-db
XML is a separate, independently date-versioned data package
(`/usr/share/osinfo`, own `VERSION` file) installed by the distro by
default, with `osinfo-db-tools`' `--system`/`--local`/`--user` tiers as the
update-without-a-new-distro-release path — matching the loader's own
three-tier path precedence. One gotcha: the `.pc` file's declared
`Requires` omits `gio-2.0` even though a public header needs it, so a
`build.rs` must add `gio-2.0` cflags explicitly. Full writeup:
[`docs/research/libosinfo-bindgen.md`](../../../docs/research/libosinfo-bindgen.md).

## Reasserted 2026-08-23, after drift in tickets 09/14/15

While starting real implementation, found that tickets 09, 14, and 15
had all started describing `libosinfo` as "gir-bound" — an assumption
by analogy with `libvirt-glib` (09) and the console libraries (14),
never actually checked against this ticket's own conclusion, which
explicitly compared bindgen against gir and picked bindgen. The map's
own convention is amend-with-new-evidence, not silent restatement, so
before writing any code: tested both, for real, against this machine's
actual installed `libosinfo-1.0` (1.12.0) and its real `.gir`/
`.typelib` (also newly present — `gir` the codegen tool itself is now
installed too, unlike when ticket 09 was written).

- `gir -m sys` (raw FFI): clean, complete, no stubs — a viable `-sys`
  crate either way, same as bindgen's output.
- `gir -m normal` (idiomatic wrapper) against `Loader`/`Db`/`Os`: the
  functions that actually matter come out as commented-out `TODO`
  stubs — `Loader::process_default_path`/`process_system_path`/
  `process_user_path`/`process_uri` (the entire init sequence this
  ticket's own resolution centers on), and on `Db`: `os_list()`,
  `device_list()`, `install_script_list()`, `guess_os_from_media()`,
  `guess_os_from_tree()`, `identify_media()`, `identify_tree()`.
  gir's heuristics don't auto-translate `(void, GError**)`-shaped
  functions or libosinfo's custom `OsinfoList`-family container
  types — which is most of this API's actual surface (roughly 60 of
  ~95 functions across the three types tested). Closing that gap needs
  hand-written `Gir.toml` manual-override entries per function, a real
  and nontrivial config-authoring task, not a free ride.
- Plain `bindgen` against the real headers: all 463 `osinfo_*`
  functions present immediately, including every one gir stubbed —
  one `build.rs` + `pkg-config --cflags libosinfo-1.0 gio-2.0`, no
  `Gir.toml` at all.

**Conclusion unchanged, now with empirical grounding**: `bindgen`.
Since either path leaves the safe-wrapper layer to be hand-written
(gir's auto-idiomatic-wrapper doesn't reach this API's real shape),
the simpler build-time tooling wins. Corrected the "gir-bound"
references in tickets 09/14/15 and `osinfo/mod.rs`'s own doc comment
to point back here rather than to each other.
