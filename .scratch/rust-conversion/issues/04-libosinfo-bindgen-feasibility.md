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

## Built: the `-sys` layer plus a minimal safe wrapper

`virtinst-core/build.rs`: `bindgen` against a one-line wrapper header
(`#include <libosinfo-1.0/osinfo/osinfo.h>`), `allowlist_function("osinfo_.*")`/
`allowlist_type("Osinfo.*")`/`allowlist_var("OSINFO_.*")`, and a
`blocklist_type("_?G[A-Z_].*")` for the entire transitively-pulled-in
glib/gobject/gio type universe (safe since no real libosinfo type
starts with `G` — confirmed, not assumed, since a wrong blocklist
regex here would silently drop a real libosinfo type instead of a glib
one). `raw_line`s re-point the blocklisted names at `glib-sys`/
`gobject-sys`/`gio-sys`'s own real definitions instead of leaving them
undefined, so the generated FFI's `GError`/`GObject`/`GList`/etc.
fields are the *same* types any future `libvirt-glib`-consuming code
(ticket 09) will use, not a second incompatible redefinition. One
real gotcha hit and fixed while building this, not predicted from
ticket 04's own research: `libosinfo-1.0.pc`'s declared `Requires` is
`gobject-2.0` alone, but a header needs `gio-2.0`'s cflags too —
`build.rs` probes and adds them explicitly, exactly as the original
research flagged.

`osinfo/ffi.rs`: the generated bindings, `include!`'d verbatim with a
module-level `#[allow(...)]` (including `unused_imports`, since the
`raw_line` glib/gobject/gio imports are deliberately broader than any
one libosinfo minor version's actual header surface needs — see the
comment on that choice in `build.rs`).

`osinfo/mod.rs`: `OsDb`/`OsInfo`, matching ticket 15's planned shape.
`OsDb::load_default()` runs the real init sequence (`osinfo_loader_new`
→ `osinfo_loader_process_default_path` → `osinfo_loader_get_db`), with
the loader itself staying private plumbing — nothing outside this one
function ever touches it again, same as upstream's own `osdict.py`
shape. `OsDb::os(id)`/`OsDb::all_os()`/`OsInfo::{id, short_id, name}`
round out a deliberately minimal first slice; the deeper API (device
lists, install scripts, firmware requirements, the `guess_os_from_*`/
`identify_*` matching functions ticket 21 already flagged as blocked
elsewhere) is real future work once a caller needs it, not modeled yet.

**No `glib`/`gtk-rs` idiomatic-wrapper crate in the middle** — every
type here manages its own `g_object_ref`/`g_object_unref` by hand.
Confirmed safe (not assumed) that GObject subtype→supertype pointer
casts are valid here by reading the actual generated struct layout:
every `Osinfo*` struct's first field is its immediate parent type
(`#[repr(C)] struct OsinfoOs { parent_instance: OsinfoProduct, .. }`),
the standard GObject inheritance-by-embedding layout.

**A real mistake caught on review, not shipped**: a first draft
inferred each function's transfer-ownership (borrowed vs. owned
return) from its C name (`_get_` implies borrowed) rather than
checking it. That inference was backwards for one of the four calls —
`osinfo_db_get_os_list` is actually `transfer-ownership="full"`
despite the `get_` prefix — which would have shipped as a use-after-free
too subtle for a short-lived test process to reproduce (the `Db`
outlives the test, nothing re-reads the freed list afterward). Caught
by checking the real `.gir` file's `transfer-ownership` attribute on
each `<return-value>` directly (`/usr/share/gir-1.0/Libosinfo-1.0.gir`)
— the one piece of ground truth here that's strictly better than the C
headers `bindgen` itself reads, since C has no ownership-transfer
syntax at all. All four call sites (`osinfo_loader_get_db`/
`osinfo_db_get_os`/`osinfo_list_get_nth` transfer-none;
`osinfo_db_get_os_list` transfer-full) now cite the verified annotation
in their own comment, not the naming convention.

**Verified against this machine's real, installed `osinfo-db`** — no
fixture, no injection seam (libosinfo has no `test:///`-equivalent to
fake): `OsDb::load_default()` really loads `/usr/share/osinfo`, then
looks up a specific, permanently-archived, decade-old entry (Fedora
25 — EOL for years, chosen so the test stays stable as osinfo-db keeps
growing new entries under it) by exact id/short-id/name, confirms an
unknown id reads as `None` rather than panicking, confirms the same
entry surfaces through the list-iteration path too, and confirms a
cloned `OsInfo` handle survives dropping the original (the actual risk
a hand-rolled refcount pair carries that a derived `Clone` wouldn't).
3 new tests, 184/184 `virtinst-core`, harness 61/61, `fmt`/`clippy`
clean.

**Not yet tested under a memory sanitizer** — no `valgrind` and no
nightly toolchain (ASan needs one) available in this environment.
The transfer-ownership review above is the real safety net for this
slice; a sanitizer pass is worth running before this code carries
much more weight, flagged rather than silently skipped.

## First real caller: `--os-variant`, closing ticket 21's clock-offset gap

`OsInfo` gained `family()`/`is_windows()`/`clock_offset()` (`osinfo_
os_get_family`, `transfer-ownership="none"` per the real `.gir`, same
verification discipline as the four calls above) and `OsDb` gained
`os_by_id_or_short_id()` for `--os-variant`'s two accepted forms.
`virt-install` gained a real `--os-variant <id>` flag, resolved once
next to the `domcaps`/`cpu_mode` resolution and threaded into
`push_default_clock`'s new `offset` parameter — closing the one gap
ticket 21's own Resolution section called blocked "outright, no
fallback branch at all." Full writeup of the wiring, the deliberate
`<clock offset=...>`-always-emitted behavior change, and a real
`is_qemu`-gate test gap hit and fixed on the first attempt: ticket 21's
own "Post-close follow-up" section.

Every other `OsInfo`-gated default ticket 21 named (disk-bus, video
model, UEFI firmware) stays unbuilt — they need device-list/firmware-
list query methods this slice didn't add, a distinct chunk of libosinfo
API surface from the plain string/family getters `clock_offset()`
needed.

## Second caller: `supports_virtiodisk`/`supports_virtioscsi`/`supports_virtionet`

Added `OsInfo::has_device_id` (private) + the three public
`supports_virtio*` methods, porting upstream's `osdict.py`
`supports_virtiodisk`/`supports_virtioscsi`/`supports_virtionet` PCI
device-id lists (`http://pcisig.com/pci/1af4/*`) literally. Uses
`osinfo_os_get_all_devices(os, NULL)` — `transfer-ownership="full"`
returning an `OsinfoDeviceList*` (checked against the real `.gir`, not
inferred), whose parent class is `OsinfoList` (confirmed against the
generated struct layout), so it walks with the exact same
`osinfo_list_get_length`/`osinfo_list_get_nth` pair `all_os()` already
established — no new container-iteration shape needed, contrary to one
plausible-sounding concern raised before checking (upstream's own
`_OsinfoIter` wraps this call the same `get_length`/`get_nth` way it
wraps `Db.get_os_list()`).

Verified against this machine's real `osinfo-db`, both branches: a
modern entry (Fedora 25) that declares all three device ids, and a
real pre-virtio entry (Windows XP, EOL 2014, predates virtio outright)
that declares none. 1 new test, 189/189 `virtinst-core`.

**Now wired to disk/cdrom bus selection** (a follow-up commit, not this
one — see ticket 21's own "Wired, in a deliberately separate follow-up
commit" section for the full writeup): `supports_virtiodisk`/
`supports_virtioscsi` feed `domain_skeleton::default_disk_bus`
(upstream's `disk.py`'s `_default_bus`, ported in full — `is_floppy`/
`is_xenpv`/`is_hvm`/`is_q35`/`is_bhyve`/`is_x86` and all), which
`virt-install.rs` now calls at every non-container document-builder
call site instead of hardcoding `bus="virtio"`.

**Third `OsInfo` caller: `supported_netmodels()`, plus `default_nic_
model` (built, not yet wired)**. `OsInfo::supported_netmodels()` ports
`osdict.py`'s `_device_filter(cls="net")` — walks `osinfo_os_get_all_
devices` (same call/container-type `supports_virtiodisk()` already
established) but filters by `osinfo_device_get_class() == "net"`
instead of matching device ids, collecting `osinfo_device_get_name()`
(both `transfer-ownership="none"` per the real `.gir`, checked
directly). Confirmed against real `osinfo-db` XML that `<class>` is a
plain literal (`"net"`), not a hierarchical value, so equality
faithfully replaces upstream's `re.match` prefix check here. Verified
against Fedora 25 (declares `virtio-net` among its net devices) and
Windows XP (exactly one net device, resolves to `["e1000"]` — the real
target upstream's own `prefs` fallback chain is built to find). 1 new
test, 200/200 `virtinst-core`.

`domain_skeleton::default_nic_model` ports `interface.py`'s
`DeviceInterface.default_model` in full (`is_hvm`/virtio/`is_q35`/
`is_x86`/`prefs` fallback chain, `None` meaning "omit `<model>`
entirely" matching upstream's own `self.model = None`), reusing the
same `supports_virtio` gate `default_disk_bus` already established. 7
new unit tests, 206/206 `virtinst-core`.

**Now wired**, in a deliberately separate follow-up commit (same
build-then-wire split, this time with the composition test written
alongside the wiring instead of found missing afterward): `push_
default_nic` takes the caller's resolved `Option<&str>` instead of a
hardcoded `"virtio"` constant, omitting `<model>` entirely on `None`.
Every builder (`build_import_document`/`build_iso_document`/`build_
iso_two_phase_documents`/`build_manual_document`/`build_manual_two_
phase_documents`) gained a `nic_model: Option<&str>` parameter, and
`virt-install.rs` computes it via `default_nic_model` (fed by
`OsInfo::supports_virtionet()`/`supported_netmodels()`, `false`/empty
with no `--os-variant` given) at each of its 4 non-container call
sites — `--container`'s own call site passes `None` directly instead
(`default_nic_model`'s own `!is_hvm` branch always resolves to `None`
for a container's `os_type`, so there's no `GuestCapsDefaults` to
build just to ask). 2 new tests prove the composition end to end: a
real Windows XP `OsInfo` entry (`supports_virtionet() == false`,
`supported_netmodels() == ["e1000"]`) surviving a real `Domain::
create_new`/`define_new` round trip (`model type="e1000"` in the
persisted XML, folded into the same test `default_disk_bus`'s own
composition proof already used, since both resolve from the same real
`OsInfo`/connection), plus a synthetic test proving `None` actually
omits `<model>` rather than falling back to a constant — the one
branch no existing test exercised at all. 207/207 `virtinst-core`.

## Fourth `OsInfo` caller: `requires_firmware_efi`, plus `domcaps::supports_firmware_efi` and `domain_skeleton::prefers_uefi` (built, not yet wired)

`OsInfo::requires_firmware_efi(arch)` ports `osdict.py`'s
`_get_firmware_list`/`_supports_firmware_type`/`requires_firmware_efi`
in full, minus upstream's `hasattr`/`try-except` guards (this crate's
binding is generated at build time against whatever `libosinfo` is
actually installed — ticket 04's own premise — so there's no "maybe
absent at runtime" case a dynamically-typed `hasattr` check guards
against). `osinfo_os_get_complete_firmware_list` is `transfer-
ownership="full"` returning an `OsinfoFirmwareList*` whose parent is
`OsinfoList` (both checked directly, same pattern `all_os()`/
`supported_netmodels()` already established); `OsinfoFirmware`'s
`get_architecture`/`get_firmware_type`/`is_supported` are all
`transfer-ownership="none"`/plain `gboolean`. Verified against a real,
permanently-relevant `osinfo-db` fact rather than a stand-in: Windows
11 genuinely requires UEFI (`<firmware arch="x86_64" type="efi"/>`
present, `<firmware arch="x86_64" type="bios" supported="false"/>`
explicit) — one of the few hardware-requirement facts unlikely to ever
change in osinfo-db. Fedora 25 (no `<firmware>` declarations at all)
proves the `default`-fallback branch. 1 new test, 208/208
`virtinst-core`.

`DomainCapabilities` gained an `os: CapsBlock` field (a sibling of
`devices`, not nested in it — confirmed against this machine's real
`qemu:///session domcapabilities` capture) and `supports_firmware_efi()`
(`self.os.get_enum("firmware").has_value("efi")`, ported literally) —
the modern libvirt "firmware auto-select" capability, real and present
on this machine's own QEMU/libvirt install (captured as a new fixture,
`REAL_QEMU_OS_FIRMWARE_EXCERPT`). 2 new tests (advertised-true,
absent-block-false — no pre-domcaps fallback heuristic exists here
unlike `supports_graphics_spice`, since upstream's own fallback for an
unadvertised driver is a real filesystem scan, not a guessable bool).

`domain_skeleton::prefers_uefi` ports `guest.py`'s `prefers_uefi()` in
full — x86 on a qemu/test connection defers entirely to `OsInfo`;
every other case (arm/riscv/loongarch64 `machvirt`-style machines,
bhyve) is a fixed arch/machine/URI-scheme check that never consults
`OsInfo` at all, matching upstream's own disjoint `if`/`else` rather
than one merged condition. Needed one new predicate, `is_arm_machvirt`
(upstream's `is_arm() and machine.startswith("virt")`), not previously
ported since nothing needed it. 6 new unit tests. 216/216
`virtinst-core`.

**Now wired** (a deliberately separate follow-up commit, composition
test written alongside the wiring, same discipline as the NIC-model
slice): `build_domain_skeleton` gained a `firmware: Option<&str>`
parameter, setting `<os firmware=...>` directly (omitted on `None`);
every public builder threads it the same way `disk_bus`/`nic_model`
are threaded. `virt-install.rs` computes it via `default_firmware`
(upstream's `enable_uefi()`, restricted to its fast path) fed by
`prefers_uefi`/`domcaps.supports_firmware_efi()` at each of its 4
non-container call sites; `--container`'s own call site passes `None`
directly (its connection is never qemu/test/bhyve in practice, so
`prefers_uefi` would resolve `false` regardless — no `DomainCapabilities`
to build just to confirm it). 1 new composition test: a real Windows 11
`OsInfo` entry (`requires_firmware_efi("x86_64") == true`, a genuine
hardware fact) plus a real `supports_firmware_efi()` parse survive a
real `Domain::create_new`/`define_new` round trip
(`firmware='efi'` in the persisted XML). 217/217 `virtinst-core`.
Manually confirmed no panics against `test:///default` with
`VIRTINST_TEST_FAKE_URI=qemu:///system`, both with and without
`--os-variant win11`.

This crate can only ever reach `supports_firmware_efi()`'s fast path
(`<os firmware="efi">` alone, no `<loader>`/`<nvram>` needed) — the
loader-path fallback for older libvirt (`_lookup_default_uefi_path()`,
a real JSON-firmware-descriptor filesystem scan) stays a distinct,
unbuilt subsystem, not a corner cut inside any of these functions.
