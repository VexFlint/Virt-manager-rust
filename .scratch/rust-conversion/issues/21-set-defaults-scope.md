---
id: 21
title: Scope of guest.py's set_defaults (loader/emulator/machine, CPU/clock/PM, the 9-file matrix)
type: grilling
status: resolved
blocked_by: [17, 20]
claimed_by: null
---

## Question

Ticket 20 named "loader/emulator/machine selection, CPU/clock/PM
defaults, and the 9-file per-driver device-default matrix" as piece
(b)'s remainder in one compressed sentence. Before writing any more
code: what does `guest.py`'s real `set_defaults()` actually do, item by
item, and how much of it is buildable today versus blocked on
infrastructure this crate doesn't have yet?

## Resolution

**The one-line label undersells this by roughly an order of magnitude.**
`set_defaults()` (`guest.py:1017`) isn't three things — it's `_set_
default_machine`, `_set_default_uefi`, `_add_default_graphics`, `_add_
default_video_device`, `_add_default_input_device`, `_add_default_
console_device`, `_add_default_usb_controller`, `_add_default_channels`,
`_add_default_rng`, `_add_default_memballoon`, `_add_default_tpm`,
`clock.set_defaults`, `cpu.set_defaults`, `features.set_defaults`,
`pm.set_defaults`, `os.set_defaults`, per-device `set_defaults` (the
9-file matrix), `add_virtioscsi_controller`, `add_q35_pcie_controllers`,
and the spice device cluster. Traced the reachable subset (this crate's
only real arches/drivers today: i686/x86_64 on `test:///` and
`qemu:///system`) line by line rather than assume the compressed label
was complete.

### The actual constraint: how much needs `OsInfo`

`OsInfo` (`libosinfo` bindings) doesn't exist in this crate yet — still
blocked on the `gir` codegen tool, the same wall `import.rs`'s disk-bus
comment and `push_default_nic`'s model fallback already documented.
Grepped every default-resolution call in scope for `self.osinfo`/
`guest.osinfo`/`supports_virtio*`/`prefers_uefi` and traced each hit to
its actual guard, not just its presence:

- **`prefers_uefi()`** (`_set_default_uefi`'s entire gate): for x86 +
  qemu/test — the *only* driver/arch combination this crate can reach
  today — routes straight to `osinfo.requires_firmware_efi(arch)`.
  **Blocked completely** on every currently-reachable path. UEFI/loader
  selection is not buildable at all right now, not partially.
- **`supports_virtiodisk()`/`supports_virtioscsi()`/`supports_
  virtionet()`** (`_supports_virtio`, `guest.py:411`): early-returns
  `True` unconditionally for `riscv_virt`/`s390x`/`pseries`/
  `loongarch64` — none reachable here — otherwise defers to
  `osinfo.supports_*`. **Blocked** for every arch this crate reaches;
  this is the exact gap `import.rs`'s disk-bus comment already named,
  now confirmed to be the *only* thing standing between hardcoding
  `bus="virtio"` (what every builder does today) and porting `disk.py`'s
  real `_default_bus` chain.
- **`osinfo.supports_virtiogpu()`** (`video.py`'s `default_model`,
  combined via `and` with a domcaps query): blocked the same way.
- **`hyperv_supported()`** (`clock.py`'s hypervclock timer):
  `osinfo.is_windows()` — blocked, but only gates one *additional*
  timer, not the base clock defaults (see below).
- **`osinfo.get_clock()`** (`clock.py`'s `offset` field): blocked
  outright — no fallback branch at all.

### What's buildable today, with zero `OsInfo` dependency — traced per item, not guessed

1. **`emulator`** (`set_capabilities_defaults`: `self.emulator =
   capsinfo.emulator` when unset, not xenpv, not `vz`) — `capsinfo.
   emulator` already exists on `CapsInfo` (ticket 20, populated with
   real values: `/usr/bin/qemu-system-x86_64` on this machine's real
   `qemu:///system`). No blocker. `<devices><emulator>` isn't emitted
   by any builder today.
2. **`machine`, fallback branch only** — `_set_default_machine`
   (`guest.py:1061`) splits in two, confirmed by reading the actual
   function rather than trusting `get_recommended_machine`'s summary
   (which *is* dead weight — `_qemu_machine()` has no x86 branch,
   returns `None` on every reachable arch, a different and narrower
   function `set_capabilities_defaults` calls, not `set_defaults`):
   - The q35-preferred branch needs **two** things this crate doesn't
     have: `osinfo.supports_chipset_q35()` (`OsInfo`, blocked) **and**
     `conn.support.qemu_q35_default()` — a hypervisor-*version* support
     check (`support.py`'s `_SupportCheck`, `qemu_q35_default =
     _make(hv_version={"qemu": "2.11.0", ...})`), a whole small
     mechanism (`virConnectGetVersion`) that doesn't exist in this
     crate at all yet. **Blocked on two independent things**, neither
     of which is `OsInfo` alone.
   - The fallback (`capsinfo.machines and capsinfo.machines[0] or
     None`) needs **neither** — `capsinfo.machines` already exists
     (ticket 20). Real value on this machine's actual `qemu:///system`:
     `pc-i440fx-11.1` (first in document order); empty on `test:///`
     (which reports no `<machine>` elements at all, confirmed against
     the real driver capture already pinned in `caps.rs`).
3. **`graphics` default type** (`_default_type`, `graphics.py:181`) —
   `guest.default_graphics_type` is an app-level constant
   (`BuildConfig.default_graphics`, not `OsInfo`), `is_xen()` forces
   vnc, and the domcaps fallback (`lookup_domcaps().supports_graphics_
   spice()`) needs one new thin query method on `DomainCapabilities`,
   not new XML-parsing infra — **the `devices.graphics` block is
   already parsed** (`domcaps.rs`'s doc comment: the 9 `_CapsBlock`
   device sections, including `graphics`, were built under ticket 17;
   only `os`/`cpu`/`features` are the "not yet built" remainder).
   Zero `OsInfo` dependency, one small domcaps addition.
4. **`clock`'s timer trio** (`clock.py:41`, x86 + qemu only, which is
   every case this crate reaches) — `rtc`/`pit`/`hpet` are added
   unconditionally with fixed policies, no `OsInfo` involved at all.
   Only `offset` (blocked) and the optional `hypervclock` timer
   (blocked, gated on `hyperv_supported()`) are out of reach; the base
   trio is real and buildable. Whether to emit `<clock>` with the trio
   but no `offset` attribute (letting libvirt's own default stand) or
   skip `<clock>` entirely until `offset` is resolvable is a real
   design choice, not decided here.
5. **`hostdev.set_defaults`** (`hostdev.py:154`) — `managed`/`mode`,
   two unconditional/URI-scheme-only lines, zero blockers. Lowest
   priority of this list: `virt-install` has no `--hostdev` flag today
   at all, so nothing would consume this yet.
6. **`filesystem.set_defaults`** — already substantially ported.
   `container.rs`'s builder already emits `type="mount"` and the real
   `accessmode="mapped"` override this session traced directly against
   `filesystem.py`'s `default_accessmode()` comment. Checked for gaps,
   found none blocking.

### What needs new (non-`OsInfo`) infrastructure, not just wiring

**CPU mode defaulting is the one surprise here** — traced deeper than
the ticket 20 label suggested and it is *not* `OsInfo`-blocked at all
for the reachable x86+kvm case. `cpu.py`'s `set_defaults` → `guest.
x86_cpu_default` (`SPECIAL_MODE_APP_DEFAULT`, a fixed constant, not
`OsInfo`) → `_get_app_default_mode` → `domcaps.supports_maximum_cpu_
mode()`/`supports_safe_host_passthrough()`/`supports_safe_host_model()`
— pure `DomainCapabilities` `<cpu>`-block queries. `domcaps.rs` doesn't
model `<cpu>` yet (ticket 17's own "not yet built" list: `os`/`cpu`/
`features`, each a distinct non-`_CapsBlock` shape). This is real,
valuable (arguably the single most user-visible default of the whole
list — it's what makes a real `qemu:///system` install actually get
host-passthrough or host-model CPU instead of qemu's bare default), and
buildable, but needs `domcaps.rs`'s `<cpu>` section built first — a
sub-piece of real size, not a one-liner.

### What stays out of scope regardless

`_add_default_video_device`/`_add_default_input_device`/`_add_default_
console_device`/`_add_default_usb_controller`/`_add_default_channels`/
`_add_default_rng`/`_add_default_memballoon`/`_add_default_tpm`/
`add_q35_pcie_controllers`/the spice cluster/`pm.set_defaults`/`os.
set_defaults`/`features.set_defaults` were not individually traced this
pass — flagging that explicitly rather than implying full coverage.
Given the pattern already found (osinfo-gated branches everywhere, a
few clean device-existence defaults mixed in), expect the same shape:
some zero-blocker items, some `OsInfo`-blocked, checked item by item
when actually picked up, not assumed from this list.

## Decision

Not building yet — this ticket is the scoping pass advisor asked for
before touching code, mirroring how ticket 20 was scoped before it was
built. Six independently-shippable, real slices identified:

- **(a) `emulator` threading** — trivial, zero blockers.
- **(b) `machine` fallback-only defaulting** — real values, zero
  blockers, q35-preferred branch explicitly left unbuilt/flagged (needs
  `OsInfo` + a new hv-version support-check mechanism).
- **(c) `graphics` default type (spice/vnc)** — zero `OsInfo` blockers,
  needs one new `DomainCapabilities` query method
  (`supports_graphics_spice`) on data already parsed.
- **(d) `clock`'s rtc/pit/hpet timer trio** for x86+qemu — zero
  blockers, `offset`/`hypervclock` explicitly left unbuilt.
- **(e) `hostdev.set_defaults`** — trivial but currently unreachable
  from any `virt-install` CLI flag; lowest priority.
- **(f) CPU mode defaulting (host-passthrough/host-model/host-model-
  only)** — the highest-value item found this pass, but gated on
  building `domcaps.rs`'s `<cpu>` section first; a real sub-piece, not
  a one-liner.

`--loader`/UEFI selection and the disk-bus virtio-vs-ide real default
are confirmed **fully blocked** on `OsInfo` for every arch/driver this
crate can currently reach — not worth picking up piecemeal; revisit
whole once `OsInfo` exists.

## (a)+(b) built: `emulator` threading + `machine` fallback defaulting

User picked slices (a)/(b) from the six above (via `AskUserQuestion`,
labeled together as "emulator + machine fallback"). Built both,
together, since both land in the same shared function.

`domain_skeleton::build_domain_skeleton` gained `machine: Option<&str>`
and `emulator: Option<&str>` parameters and now returns a fourth
element, `devices_el`, already created and attached to `root` (with
`<emulator>` pushed as its first child when `Some`) — matching the
existing "return an already-attached element, let callers push more
children into it" pattern `os_el` already used for `<boot>`. `machine`,
when `Some`, becomes `<os><type arch=... machine=...>`. Neither
attribute/element is emitted when the value is `None` — confirmed with
a dedicated test (`machine_and_emulator_are_omitted_entirely_when_
absent`), since `None` is the real, common case here, not a
placeholder: `test:///default`'s own real capabilities report no
`<machine>` elements at all (pinned in `caps.rs`'s existing fixture).

All five non-`--container` builders (`build_import_document`,
`build_iso_document`, `build_iso_two_phase_documents`, `build_manual_
document`, `build_manual_two_phase_documents`, plus their two private
per-phase helpers) gained the same two parameters and now return the
skeleton's own `devices_el` instead of creating a second one — each
caller's trailing `root.push_child(&mut doc, devices_el)` is gone,
since it's already attached. `build_container_os_document` deliberately
excluded, matching the precedent already set for `os_type`: it always
passes `None, None` for both, since no driver this crate reaches ever
resolves a real `capsinfo` for an `exe` guest — only ever the `hvm`/
`xen` guest a plain `guest_lookup` returns, which would be an actively
wrong value to borrow, not a missing one to backfill.

`virt-install.rs` computes `machine`/`emulator` once, right alongside
the existing `require_os_type()` closure, from the same `capsinfo`
ticket 20 already resolves — no second capability fetch. Threaded into
all five non-container call sites.

**Verified against the real driver, not just this crate's own
serialization**: `import_creates_a_running_and_persisted_domain`
(`import.rs`) was upgraded from `DomainCapabilities::fetch` to the same
`Capabilities::fetch` + `guest_lookup` sequence `virt-install.rs` now
runs, confirms `test:///default` really reports no machines
(`assert_eq!(machine, None, ...)`, not assumed), and asserts the
*returned* `inactive_xml()` from a real `Domain::define_new` call
contains `<emulator>` — proof the real driver accepts and keeps the
new element, not just that this crate's own DOM construction looks
right. `builds_a_complete_bootable_domain` (same file) separately pins
both fields present (`Some("q35")`/`Some("/usr/bin/qemu-system-
x86_64")`) through the DOM.

**A second advisor review caught that the actual composition
`virt-install.rs` runs was still untested for `machine`, the same class
of gap already closed for `os_type`.** `builds_a_complete_bootable_
domain` pins a literal `Some("q35")`; the real end-to-end test's driver
reports no machines at all (`test:///default`'s own real capture is
empty) — so `capsinfo.machines.first()` → `<os><type machine=...>` had
no test exercising it with an actual non-empty, real-shaped list. Added
`capsinfo_machine_fallback_actually_flows_into_the_built_document`
using the same `VIRTINST_TEST_CAPS_FILE` injection mechanism, a fixture
with a real multi-machine list (trimmed from the same real `qemu:///
system` capture as `caps.rs`'s `REAL_QEMU_SYSTEM_CAPS_EXCERPT`), and
found something worth stating plainly rather than assuming: `.first()`
on this shape returns `pc-i440fx-11.1` — the first `<machine>`
element's own version-pinned name, not the friendlier `pc` canonical
alias two entries later. Traced upstream's own `capabilities.py`
(`all_machine_names`) before treating this as a Rust-side bug: it
builds the same list the same way (name first, canonical only if not
already present) — **upstream's own fallback would land on the exact
same value on this exact data**. Not a divergence to fix; a real,
now-tested property of the ported fallback.

**One more thing this slice changed, not caught by any test and worth
naming rather than leaving as a silent diff**: in the two-phase
builders (`iso`/`manual`), `<devices>` now attaches to `root` *inside*
`build_domain_skeleton`, before `push_on_reboot_destroy(root)` runs —
previously `<devices>` attached last, after `<on_reboot>`. The initial
document of a two-phase pair therefore now emits `<on_reboot>` *after*
`<devices>` instead of before. Every existing assertion uses `find()`
(presence), not position, so nothing caught this, and libvirt
renormalizes element order on `defineXML` regardless — functionally
inert today. Matters only because ticket 20 names upstream's own
full-document `virt-install` fixtures (byte-comparable against
`_XML_PROP_ORDER`) as the eventual acceptance bar for the fake-arch
corpus; whoever reaches for byte-exact matching later will need
`domain_skeleton.rs` to control final element order explicitly, not
rely on push order like every builder does today.

**Verified**: 99/99 `virtinst-cli`, 138/138 `virtinst-core` (+2 new:
`machine_and_emulator_are_omitted_entirely_when_absent`,
`capsinfo_machine_fallback_actually_flows_into_the_built_document`),
harness 61/61, `cargo fmt`/`clippy` clean workspace-wide.

**Not done**: slices (c)–(f) — `graphics` default type, `clock`'s timer
trio, `hostdev.set_defaults`, and CPU mode defaulting — remain exactly
as scoped above, not started.

## A real, unrelated bug found and fixed while continuing this ticket

Not part of `set_defaults()`'s scope at all — surfaced while reasoning
about the difference between `hypervisor_type` (`capsinfo`-derived,
e.g. `"kvm"`) and `is_qemu()` (URI-scheme-derived) for slice (d)'s
`conn.is_qemu()` guard, and traced to something already live in every
builder.

Every domain builder (`import`/`iso`/`manual`/`container`) derived
`crate::cloner::generate_mac`'s `is_qemu` argument from `domain_type ==
"qemu"` — a literal string match on `hypervisor_type`, the value
`guest_lookup`/`DomainCapabilities` reports (`"qemu"`, `"kvm"`,
`"test"`, ...). Upstream's own `interface.py:30` keys on
`conn.is_qemu()` instead — the connection's URI scheme. These disagree
on the single most common real invocation this crate builds:
`--connect qemu:///system` (the CLI's own default) with KVM available
resolves `hypervisor_type` to the literal string `"kvm"`, not
`"qemu"` — confirmed against this machine's own real capture
(`domcaps.rs`'s `QEMU_DOMCAPS_EXCERPT`: `<domain>kvm</domain>`, and
`caps.rs`'s own pinned `guest_lookup` test asserting `hypervisor_type
== Some("kvm")` on the same real data). So **every real, non-`test:///`
domain this crate has ever built got the Xen OUI (`00:16:3e`) instead
of qemu's (`52:54:00`)** — not a ticket-21 regression, a pre-existing
bug dating to `virt-install.rs`'s first version, invisible in this
crate's own test suite because `test:///default`'s domain type
genuinely is the literal string `"test"` and never triggered it.

An earlier version of `push_default_nic`'s own doc comment had already
flagged the two sources "can disagree in principle" but claimed the
divergence was "not reachable through this crate's current CLI surface"
— false the entire time, since `--connect` has defaulted to
`qemu:///system` since day one. Corrected rather than left standing.

**Fix**: threaded a real `is_qemu: bool` (`conn.uri().is_qemu()`,
computed once in `virt-install.rs`) through every builder instead of
inferring it from `domain_type` — mechanically the same shape as
`machine`/`emulator`'s threading above, except `--container` *does*
get it (unlike `os_type`/`machine`/`emulator`, `is_qemu` isn't
`capsinfo`-derived, so there's no "wrong guest" hazard to avoid
there). `cloner.rs` was already correct (derives `is_qemu` from the
connection directly) and untouched.

**Tests**: the existing `nic_oui_follows_domain_type_not_just_qemu`
was reframed to `nic_oui_follows_is_qemu_not_domain_type` — its old
name and comment described the buggy proxy as the intended design.
Added `nic_oui_is_correct_for_a_real_kvm_accelerated_install`
(`domain_type = "kvm"` with `is_qemu = true` must still produce the
qemu OUI). Every other builder test's call site mechanically threaded
a matching `is_qemu` value (`true` for literal `"qemu"` domain types,
`false` for `"test"`/unrelated tests, `conn.uri().is_qemu()` for the
real end-to-end tests that have a live connection in scope).

Caught on review: every one of those, including the new one, passes
`is_qemu` as a literal (or as `conn.uri().is_qemu()` on `test:///`,
which is always `false`) — none of them exercise the actual
*derivation* the bug was in. The bug was never in `generate_mac`
consuming `is_qemu` wrong; it was in what used to feed it
(`hypervisor_type == "qemu"`). A literal at the call site proves the
callee, not the composition. Closed that gap with a dedicated
composition test in `import.rs`,
`nic_oui_is_correct_when_domain_type_is_kvm_on_a_real_qemu_connection`:
sets `VIRTINST_TEST_FAKE_URI=qemu:///system` *and*
`VIRTINST_TEST_CAPS_FILE` (a `hypervisor_type=kvm` fixture) together on
one `test:///default` connection, asserts both facts hold
simultaneously (`capsinfo.hypervisor_type == Some("kvm")` and
`conn.uri().is_qemu()` — the bug's actual precondition, stated nowhere
else in the suite), then builds the document with `conn.uri()
.is_qemu()` (not a literal) and checks the resulting MAC. Manually
confirmed this test fails pre-fix (temporarily substituted the old
`hypervisor_type == "qemu"` derivation at the call site — the run
failed with the Xen OUI as expected) and passes with the real fix
restored.

**Verified**: 99/99 `virtinst-cli`, 140/140 `virtinst-core` (+2 new
tests over the pre-fix baseline of 138), harness 61/61, `cargo
fmt`/`clippy` clean workspace-wide. Also smoke-tested the compiled
`virt-install` binary with `VIRTINST_TEST_CAPS_FILE` (a
`hypervisor_type=kvm` fixture) and `VIRTINST_TEST_FAKE_URI=qemu:///
system` together — the exact previously-mishandled combination — no
crash (that binary run couldn't observe the MAC itself, since it's a
separate process against `test:///`'s own state; the in-process
composition test above is what actually proves the fix).

## Refactor before (c): `GuestCapsDefaults` collapses the 6-arg cluster

Before starting slice (c), an advisor review flagged that `graphics_
type` would be the *third* individual field threaded onto every
affected builder's argument list in a row (`machine`/`emulator`, then
`is_qemu`, now this) — paying the same "add a param, fix N call sites"
tax a third time, each already past `#[allow(clippy::too_many_
arguments)]`. Collapsed `domain_type`/`os_type`/`arch`/`machine`/
`emulator`/`is_qemu` into one `GuestCapsDefaults<'a>` struct
(`domain_skeleton.rs`, re-exported at `install::GuestCapsDefaults`),
used by `build_domain_skeleton` and all five non-container builders'
public signatures (`build_import_document`, `build_iso_document`/
`build_iso_two_phase_documents`, `build_manual_document`/
`build_manual_two_phase_documents`). `--container` deliberately kept
its 3 individual params (`domain_type`/`arch`/`is_qemu`) rather than
being forced onto the same struct — it only needs half of it, and
giving it the rest would invite exactly the "wrong guest" hazard
already flagged for `os_type`/`machine`/`emulator` there; internally it
still builds a `GuestCapsDefaults` to hand to `build_domain_skeleton`,
with `os_type: "exe"`/`machine: None`/`emulator: None` hardcoded.

Pure refactor, verified as such: every existing test's assertions are
byte-for-byte unchanged, only call sites' argument lists changed shape
(positional args → a `GuestCapsDefaults { .. }` literal, or an already-
in-scope struct value for the real end-to-end tests). Re-ran the full
suite after — 99/99 `virtinst-cli`, 146/146 `virtinst-core` (the 6 new
`domcaps.rs` tests below, no regressions), harness 61/61, `fmt`/
`clippy` clean — with zero assertion changes as the actual proof the
refactor didn't alter behavior.

`domcaps.rs` also gained its first two `supports_*`-style query
methods ahead of wiring them into any builder yet:
`supports_graphics_spice(is_qemu, is_test, host_arch)` (upstream's
`domcapabilities.py:464`) and `default_graphics_type(is_xen, is_qemu,
is_test, host_arch)` (upstream's `DeviceGraphics._default_type`,
`graphics.py:181`, with `"spice"` standing in for the unconfigurable
`BuildConfig.default_graphics`) — 6 new tests (advertised-enum hit/
miss, pre-domcaps heuristic fallback across qemu/test/arch
combinations, xen-forces-vnc, spice-stays, spice-downgrades-to-vnc),
146/146 `virtinst-core` total.

## (c) built: `graphics` default type (spice/vnc)

`domain_skeleton::push_default_graphics` materializes `<graphics
type=.. port="-1" .../>` (`tlsPort="-1" autoport="yes"` added only for
spice) — the fixed shape upstream's `_get_default_port`/`_get_default_
tlsport`/`_get_default_autoport` chain collapses to once no builder
here ever adds a `<listen>` element (confirmed by reading the actual
chain: `port`/`tlsPort` both need `_listen_need_port()`, true whenever
no `<listen>` exists; `autoport` only for spice, "to maintain back
compat with old libvirt" for vnc). Threaded `graphics_type: &str` into
all five non-container builders (`build_import_document`,
`build_iso_document`/`build_iso_two_phase_documents`,
`build_manual_document`/`build_manual_two_phase_documents`) — not
`--container`: upstream's own guard (`self.os.is_container() and not
self.conn.is_vz()`) skips the whole device there on every driver this
crate reaches.

`virt-install.rs` fetches `DomainCapabilities` again (dropped from this
binary entirely under ticket 20 piece (b) once its only past use —
`.domain`/`.arch` — moved to `capsinfo`; back now for its real
`<graphics>`-block-shaped purpose, falling back to `DomainCapabilities
::default()` on fetch failure, matching upstream's own `build_from_
params` stub-object fallback) and resolves `graphics_type` once via
`domcaps.default_graphics_type(conn.uri(), host_arch)`, threaded
alongside `caps` into each of the five builder calls.

**Signature revised on review**: the first draft of `supports_graphics_
spice`/`default_graphics_type` took `is_qemu`/`is_test`/`is_xen` as
three separate same-typed `bool`s plus `host_arch: Option<&str>` — the
same "transposition compiles, no test catches it" shape the NIC-OUI
bug above already hit once for real. An advisor review caught this and
asked whether `Uri` could be taken directly instead of reasoned-about-
as-a-tradeoff; checking `uri.rs` directly showed `Uri::parse` is a
pure, connection-free parser (no libvirt call, no I/O at all) — so
there was no real tradeoff to accept, just an unchecked first draft.
Both methods now take `uri: &Uri` (`crate::connection::Uri`) instead —
transposition is a type error now, not a silent wrong answer, and
tests got *simpler*, not harder: `Uri::parse("xen:///")` replaces
threading a bare `true` through three positional slots. `host_arch`
stays a separate `Option<&str>` parameter (the *host's* CPU arch,
upstream's `self.conn.caps.host.cpu.arch` — deliberately not this
struct's own `arch` field, the *domain's* target arch) since plain
`DomainCapabilities` has no `Uri`-shaped wrapper to fold it into.

Added `default_graphics_type_forces_vnc_for_a_real_xen_uri_even_when_
domcaps_advertises_spice` as the actual composition test for this: a
**xen** URI specifically (not qemu), since a qemu URI makes `is_xen()`/
`is_test()` both `false` regardless of order — transposition is only
observable with xen in the mix. Constructs a real `Uri::parse("xen+ssh
://...")`  against a domcaps fixture that explicitly advertises spice
support, asserting both the fixture (spice really is supported) and
the result (`"vnc"` anyway, since xen forces it) — the same "assert the
precondition, not just the outcome" shape the NIC-OUI composition test
used.

Also renamed `no_disk_means_a_single_boot_entry_and_only_the_default_
nic` (`manual.rs`) to `..._and_no_disk_device` — its old name asserted
"only the default nic" as a device-count claim that a new `<graphics>`
element would make false; the assertions themselves never checked a
total device count, only per-tag presence, so nothing was actually
broken, but the name was making a claim the test didn't back up even
before this slice. And added `inactive_xml.contains("type='spice'")`
assertions to `import.rs`/`iso.rs`/`manual.rs`'s real end-to-end
tests, matching the existing `<emulator>` persistence check in
`import.rs` — proving the real `test:///` driver *keeps* the
`<graphics>` element across `create_new`/`define_new`/refetch, not
just that it accepts it once.

**Verified**: `import.rs` gained a `<graphics>` assertion on its main
pinned test plus a new `vnc_graphics_gets_only_a_port_no_tls_port_or_
autoport` test (the other branch); `iso.rs`/`manual.rs` gained
equivalent assertions on their own pinned tests and two-phase tests
(checking both initial and final documents agree); `domcaps.rs` gained
the xen composition test above. 99/99 `virtinst-cli`, 148/148
`virtinst-core` (+2 over the 146 pre-slice baseline), harness 61/61,
`fmt`/`clippy` clean. Manually smoke-tested the compiled `virt-install`
binary three times: plain `test:///default` (baseline, no env
injection, twice — confirming repeatability), and once with
`VIRTINST_TEST_CAPS_FILE` (`hypervisor_type=kvm`) +
`VIRTINST_TEST_DOMCAPS_FILE` (graphics `supported='yes'`, spice
advertised) + `VIRTINST_TEST_FAKE_URI=qemu:///system` together — the
exact combination that exercises the new domcaps-fetch-and-resolve
path — all completed with no crash.

## (d) built: `clock`'s rtc/pit/hpet timer trio

`domain_skeleton::push_default_clock` ports upstream's `DomainClock.
set_defaults` (`clock.py:41`): `<clock><timer name="rtc"
tickpolicy="catchup"/><timer name="pit" tickpolicy="delay"/><timer
name="hpet" present="no"/></clock>`, no `offset` attribute (blocked on
`OsInfo`'s `get_clock()`; omitted rather than guessed, letting
libvirt's own hypervisor-specific default stand — qemu's own default
offset is `utc` — a real design choice flagged in this ticket's
scoping pass, not decided arbitrarily). `hypervclock` (also `OsInfo`-
gated, plus an unbuilt hv-version support-check) stays unbuilt too.

**Structurally different from (c)**: `push_default_clock` self-gates
exactly like upstream's own three early-returns (`os_type != "hvm"`,
non-x86 `arch`, `!is_qemu`), so it's called *unconditionally* from all
six builders including `--container` — no "not container" exception
needed at any call site, since `os_type = "exe"` there already fails
the first gate. Matches upstream exactly: `guest.py`'s `set_defaults()`
calls `self.clock.set_defaults(self)` unconditionally too, relying on
the same internal gate rather than a caller-side check.

**A real gap found and closed while verifying this slice**: every one
of this module's other real end-to-end tests runs against
`test:///default`, where `conn.uri().is_qemu()` is always `false` — so
none of them could ever exercise `push_default_clock`'s `is_qemu` gate
against a live libvirt call, only this crate's own DOM construction.
Closed with `clock_trio_survives_a_real_libvirt_define_when_is_qemu_is_true`
(`import.rs`), reusing the `VIRTINST_TEST_FAKE_URI` technique from the
NIC-OUI composition test: physically opens `test:///default` but makes
the connection report itself as `qemu:///system`, so `is_qemu` really
is `true` for a real `create_new`/`define_new`/refetch round-trip
(`domain_type` deliberately stays `"test"` — the two fields disagree on
purpose, since the real driver reached is still `test:///`).

**Verified**: `import.rs` gained the pinned-test clock assertion plus
`clock_is_omitted_when_the_connection_is_not_qemu`/`clock_is_omitted_
for_a_non_x86_arch` (the two negative gates) and the real-driver test
above; `iso.rs`/`manual.rs` gained matching assertions on their own
pinned and two-phase tests; `container.rs` gained a clock-absence
assertion using inputs (`is_qemu=true`, `arch="x86_64"`) that would
otherwise pass the other two gates, isolating the `os_type` gate
specifically. 99/99 `virtinst-cli`, 151/151 `virtinst-core` (+3 over
the 148 slice-(c) baseline), harness 61/61, `fmt`/`clippy` clean.

## (e) built: `hostdev.set_defaults`

`DeviceHostdev::set_defaults` (`devices/hostdev.rs`) ports upstream's
`DeviceHostdev.set_defaults` (`hostdev.py:154`) exactly: `managed`
defaults to `!uri.is_xen()` (upstream: `self.conn.is_xen() and "no" or
"yes"`), `mode` defaults to `"subsystem"` — both only when unset,
neither touching an explicit value. Takes `uri: &Uri` rather than a
bare `is_xen: bool`, matching the correction slice (c) already applied
to `domcaps.rs`'s query methods (a same-typed `bool` here is a smaller
version of the same "wrong argument, no test catches it" risk, and
`Uri::parse` costs nothing to construct). One deliberate narrowing from
upstream, stated rather than left silent: gates on `self.mode.is_none()`,
not upstream's `if not self.mode` (which also treats `Some("")` as
falsy) — matches every other `Option` gate already in this crate, and
an empty-string `mode=""` attribute doesn't arise from real libvirt
XML.

**No real caller yet, built anyway**: this crate's `virt-install`/
`virt-xml` have no `--hostdev` flag, so nothing constructs a
`DeviceHostdev` to call this on — the ticket's own scoping pass already
flagged this as lowest priority for exactly that reason. Built and
tested regardless, the same precedent slice (c) set for `domcaps.rs`'s
query methods (real logic landing before its real caller in the same
slice) — wiring it into a builder is real future work once `--hostdev`
exists, not dropped by oversight.

**Verified**: 5 new tests on `DeviceHostdev::set_defaults` (managed
defaults for xen/non-xen, an explicit `managed` surviving, mode
defaulting, an explicit `mode` surviving). 99/99 `virtinst-cli`,
156/156 `virtinst-core` (+5 over the 151 slice-(d) baseline), harness
61/61, `fmt`/`clippy` clean.

## (f): CPU mode defaulting — scope is bigger than the six-slice list implies

Before writing any code: an advisor review flagged that slice (f)'s
one-line label ("CPU mode defaulting, needs `domcaps.rs`'s `<cpu>`
section built first") undersells it the same way ticket 20's one-line
label undersold this whole ticket. Two genuinely separate pieces, not
one:

1. **`domcaps.rs`'s `<cpu>` section** — upstream's `_CPU`: a list of
   `<mode>` elements, each with `name`/`supported`, `<model>` children
   carrying full CPU feature lists. Ticket 17 already flagged this as a
   *different shape* from `_CapsBlock` (`domcaps.rs`'s own doc comment:
   "each a different, non-`_CapsBlock` shape... `_CPU` is a list of
   `<mode>`s with their own `<model>` children including full CPU
   feature lists") — not another `from_optional_child` call, a real
   parser.
2. **Three query methods plus the defaulting chain on top**
   (`supports_maximum_cpu_mode`/`supports_safe_host_passthrough`/
   `supports_safe_host_model`, then `_get_app_default_mode`, then
   emitting `<cpu mode=...>`).

Per the review: build (1) as its own commit with its own tests against
a real captured `<cpu>` block (same discipline as `GuestCapsDefaults`
being split from slice (c)'s actual graphics wiring) — the parser is
where the shape risk lives and deserves isolated verification, not
bundled with the defaulting logic that consumes it.

### Grounding pass: is (2) actually finishable, or partially blocked like (b)'s q35 branch?

Checked before committing to build anything, per the review's explicit
warning that `_get_app_default_mode` might consult a hypervisor-
*version* check (`conn.support.*`), the same unbuilt mechanism that
blocked slice (b)'s q35-preferred machine branch.

**Conclusion: not blocked.** Read `cpu.py`'s whole chain
(`_get_app_default_mode` → `_should_use_maximum_cpu_mode` →
`supports_maximum_cpu_mode`/`supports_safe_host_passthrough`/
`supports_safe_host_model`) line by line: every one of those calls
resolves to a pure `domcaps.cpu`/`domcaps.devices` read, no
`conn.support.*` hypervisor-version check anywhere in that specific
call graph. The q35 blocker is scoped to a different branch entirely
(`_set_default_machine`'s preferred-machine selection), not to CPU
mode defaulting. Piece (2) is fully buildable now.

The pieces that *are* out of scope, found while tracing the rest of
`DomainCpu.set_defaults`/`set_special_mode`/`set_model` for piece (2)'s
design (see below): the `arm_machvirt`/`riscv_virt`/`loongarch64`
guest-type branches (this crate has no ARM/RISC-V/LoongArch guest
support at all yet — same "not this crate's scope" reasoning as
everywhere else `is_x86()`-adjacent logic has come up) and the
CPU-security-feature enrichment on the custom-model path
(`_add_security_features`/`get_cpu_security_features`, which requires
`_get_expanded_cpu` — a live libvirt round-trip to expand a CPU model
into its real feature list, a new kind of capability query this ticket
didn't scope in). The security-feature gap is stated, not silent: it
only matters on the `host-model-only` fallback branch, which is
unreachable on any host (including this one) where host-passthrough or
host-model is safely supported.

## (f) piece 1 built: `domcaps.rs`'s `<cpu>` section

`Cpu`/`CpuMode`/`CpuModel` structs (upstream's `_CPU`/`_CPUMode`/
`_CPUModel`) parse `<domainCapabilities><cpu>`'s list-of-modes shape —
each `<mode name=... supported=...>` carrying its own `<enum>`s and
`<model>` children, the "list of enum-blocks with typed children"
shape ticket 17 flagged as genuinely different from every other
`_CapsBlock`-based device section in this file. `find`/`find_all` on
`edit_xml::Element` are direct-children-only by construction (checked
the dependency source directly, not inferred from behavior), so
`CpuMode::from_element`'s `find_all(doc, "model")` cannot accidentally
pick up a sibling `<feature>`/`<vendor>`/`<maxphysaddr>` element under
the same `<mode>` — confirmed rather than assumed, since a mistaken
recursive `find_all` was exactly the kind of shape risk this piece was
split out to catch in isolation.

Deliberately not modeled, matching upstream's own `_CPUMode`/
`_CPUModel` scope (not exceeding it): `<vendor>`/`<maxphysaddr>`/
`<feature>` under `host-model`, and the `vendor`/`canonical` attributes
on `custom` `<model>` entries.

Three query methods on `DomainCapabilities`, next to
`default_graphics_type`: `supports_safe_host_model` (host-model mode
supported, with a model whose `fallback="forbid"`), `supports_safe_
host_passthrough` (host-passthrough mode supported, with
`hostPassthroughMigratable` advertising `"on"`), `supports_maximum_cpu_
mode` (maximum mode supported). `supports_safe_host_model` uses
`.first()` on the model list rather than upstream's bare `models[0]`
(which would panic on a modelless host-model block) — a defensive,
not-a-behavior-change divergence, confirmed by every real capture
always carrying exactly one model.

**Verified against a real capture, not just a hand-trimmed fixture**:
unit tests run against a trimmed `<cpu>` excerpt from this machine's
own `virsh -c qemu:///session domcapabilities` output (`qemu:///system`
hit a polkit denial on this session; `qemu:///session` reports the
identical `<cpu>` shape on the same host). As a one-off sanity pass
before committing, also parsed the full untrimmed 2891-line capture
(not committed as a fixture) and confirmed `supports_safe_host_
passthrough()` is `true`, `host-model` yields exactly one model named
`Skylake-Client-IBRS`, and `custom` yields all 171 real models —
proving the parser against data nobody hand-shaped to fit the code,
not just the excerpt trimmed to match it.

11 new tests: `parse_reads_the_cpu_block_and_its_nested_models`,
`cpu_block_absent_reads_as_default_not_a_parse_error`, plus
true/false pairs for each of the three query methods against the real
fixture and each of their individual failure conditions (mode absent,
`hostPassthroughMigratable` lacking `"on"`, host-model advertised with
no model child, `fallback` not `"forbid"`).

**Verified**: 99/99 `virtinst-cli`, 167/167 `virtinst-core` (+11 over
the 156 slice-(e) baseline), harness 61/61, `fmt`/`clippy` clean.

## (f) piece 2 built: CPU mode defaulting, and ticket 21 is fully closed

Ported upstream's `_get_app_default_mode`/`set_special_mode`/
`set_model`/`_validate_default_host_model_only` chain (`domain/cpu.py`),
collapsed into one query method rather than mirrored 1:1: this crate
builds domain XML through direct `edit_xml::Document`/`Element`
construction (`domain_skeleton.rs`), not through `DomainCpu`'s bound-
struct setters, so there's no `self.mode =` to mutate — a resolved
`DefaultCpuMode` (or `None`) is the whole result. Considered building
`DomainCpu::set_defaults` on the bound struct instead, following slice
(e)'s "build ahead of a real caller" precedent — advisor caught why
that precedent doesn't transfer: `<cpu>` defaulting is unconditional in
upstream's `guest.set_defaults()`, not gated behind a flag that doesn't
exist yet, so a bound-struct method nothing constructs would leave
`virt-install` emitting no `<cpu>` block at all — a real behavioral gap
from upstream, not a missing-API gap the way `DeviceHostdev` was.

**Shape**: `DomainCapabilities::default_cpu_mode(uri, domain_type,
guest_arch, host_arch, host_cpu_model) -> Option<DefaultCpuMode>`,
next to the three query methods piece 1 built. `DefaultCpuMode` is a
two-variant enum: `Mode(&'static str)` (`host-passthrough`/`host-
model`/`maximum`, a bare `<cpu mode=...>`) and `Custom { model:
Option<String> }` (`mode="custom" match="exact"`, with or without a
`<model>` child — see below). `domain_skeleton::push_default_cpu(doc,
root, resolved)` materializes whichever variant won, called from every
builder including `--container` (which always passes `None` — no
domcaps/host-CPU-model pair is resolved for a container guest at all,
same "no correct value to give it" reasoning `machine`/`emulator`
already carry there). Follows `push_default_graphics`'s "resolve once
in `virt-install.rs`, pass the answer in" pattern, not `push_default_
clock`'s self-gating one — advisor's call: clock's inputs (`os_type`/
`arch`/`is_qemu`) are already threaded to every builder, cpu's aren't
(`domcaps` plus the host's own reported CPU model from `<capabilities>`
are new inputs no builder carries), so self-gating would mean either
threading both through every builder's signature or resolving twice.

**Scope traced from upstream's real branches, not assumed**:
- Top gate: `uri.is_test() || uri.is_qemu()`, upstream's `conn.is_test()
  or conn.is_qemu()` — a real xen/vz connection never gets a `<cpu>`
  default here.
- `arm_machvirt`/`riscv_virt`/`loongarch64` guest-type branches: **not
  modeled** — no ARM/RISC-V/LoongArch guest support exists in this
  crate at all to ground it against, same posture as every other
  `is_x86()`-adjacent gap.
- x86 + `domain_type == "kvm"`: requires `guest_arch == host_arch`
  (cross-arch emulation gets no host-based default). Prefers `host-
  passthrough`, then `host-model`, then a custom model pinned to the
  host's own reported CPU. `_should_use_maximum_cpu_mode` cannot fire
  in this branch — it requires `guest.type == "qemu"`, which this
  branch's own `domain_type == "kvm"` guard already excludes; confirmed
  by reading the source, not assumed, since it's what let this branch
  skip straight to the three-way chain with no maximum-mode check.
- x86 + `domain_type == "qemu"` (TCG): `maximum` if domcaps advertises
  it, otherwise no default at all.
- **A deliberate superset, not a straight port**: `_validate_default_
  host_model_only` can strip just the model name (when domcaps says
  the host's own reported CPU isn't usable) while leaving `mode=
  "custom" match="exact"` standing with no `<model>` child. Caught on
  review: upstream only runs this check when `guest.x86_cpu_default ==
  HOST_MODEL_ONLY` — for `virt-install` specifically, `x86_cpu_default`
  is pinned to `APP_DEFAULT` (`guest.py:273`, never reassigned on that
  path) and `_set_cpu_x86_kvm_default` checks the *unresolved* sentinel
  against `HOST_MODEL_ONLY` before `set_special_mode` ever resolves it,
  so the check is dead code on `virt-install`'s own CLI path — it only
  fires via the GUI's CPU-preference setting (`createvm.py:1390`) or a
  test override. `default_cpu_mode` runs it unconditionally anyway:
  this crate has no `x86_cpu_default`-equivalent switch to gate it on,
  a 1:1 GUI port is this project's stated endpoint (where this exact
  behavior *is* live), and skipping it to match virt-install's dead
  branch would just mean re-adding it later. `DefaultCpuMode::Custom {
  model: None }` represents the stripped-model shape exactly, and
  `push_default_cpu` emits it as-is — a real, documented divergence
  from what `virt-install` itself would produce upstream, not
  "faithfully replicated," as an earlier draft of this section claimed
  before review caught the mistake.
- **Not modeled, stated rather than silently dropped**: the CPU-
  security-feature enrichment on the custom-model path (`_add_security_
  features`/`get_cpu_security_features`) needs `_get_expanded_cpu` — a
  live libvirt round-trip to expand a CPU model into its real feature
  list, a kind of capability query this crate doesn't have. Only
  matters on the `host-model-only` fallback, itself unreachable on any
  host (including the one this was grounded against) where host-
  passthrough or host-model is safely supported.

**Grounding confirmed empirically, not just by reading source**: on
this machine's own real `qemu:///session` capture, host-passthrough is
safely supported (`hostPassthroughMigratable` advertises `"on"`), so
`_get_app_default_mode` would resolve to `host-passthrough` and short-
circuit before host-model/host-model-only are ever reached — those two
branches are exercised by synthetic fixtures in this crate's own test
suite, not by a live driver, matching upstream's own real-world
behavior on modern libvirt+qemu rather than a gap.

**Real end-to-end proof, following slice (d)'s pattern exactly**: unlike
`clock_trio_survives_a_real_libvirt_define_when_is_qemu_is_true`, no
`VIRTINST_TEST_FAKE_URI` trick is needed here — `default_cpu_mode` is a
pure function over an already-parsed `DomainCapabilities` plus plain
strings, resolved entirely before `push_default_cpu` ever runs, so the
real end-to-end test only has to prove the other half: that a value
resolved from a real captured `<cpu>` fixture survives a real `Domain::
create_new`/`define_new` round trip against `test:///default`
unchanged (`cpu_mode_survives_a_real_libvirt_define`, `import.rs`).

**Verified**: 9 new `domcaps.rs` unit tests covering every branch
(host-passthrough preferred, host-model fallback, custom-model
fallback, the unusable-model-gets-stripped wrinkle, no-host-model-at-
all, cross-arch mismatch, TCG maximum-mode, TCG with no maximum
advertised, xen exclusion) plus 5 new `import.rs` tests (`<cpu>`
omitted when unresolved, each `DefaultCpuMode` variant's exact XML
shape, the real end-to-end proof). 99/99 `virtinst-cli`, 181/181
`virtinst-core` (+14 over the 167 piece-1 baseline), harness 61/61,
`fmt`/`clippy` clean (three newly-8-argument builder functions gained
`#[allow(clippy::too_many_arguments)]`, matching the private phase-
document helpers that already carried it).

**Ticket 21 is now fully closed**: all six slices from the Decision
section above are built — (a)+(b) emulator/machine, (c) graphics, (d)
clock, (e) hostdev, (f) CPU mode defaulting (both the `domcaps.rs`
parser and the defaulting chain on top). The two items the Decision
section named as fully blocked (`--loader`/UEFI selection, the disk-bus
virtio-vs-ide real default — both on `OsInfo`) remain exactly that:
blocked, not silently dropped, revisit whole once `OsInfo` exists. The
9-file per-driver device-default matrix and `pm`/`os`/`features`/spice
upstream's `set_defaults()` also touches were flagged in this ticket's
scoping pass as "not individually traced this pass" — still true, and
still real future work, but out of this ticket's six-slice scope as
decided.

**Gap caught on final review, closed before declaring done**: every
test up to this point either passed `cpu_mode: None` or hand-built a
`DefaultCpuMode` from a literal fixture — the actual live path
(`virt-install.rs`'s real `DomainCapabilities::fetch(...).unwrap_or_
default()` call, with the exact `emulator`/`arch`/`machine`/
`domain_type` arguments it passes) had never run end to end. This
mattered specifically because `.unwrap_or_default()` fails *silently*:
a wrong argument shape wouldn't error, it would just make `domcaps`
empty, which sends `default_cpu_mode` down a different-but-still-
successful branch (`Custom` fallback using the host's raw `<capabilities>`
model, since both `supports_safe_*` checks read `false` on empty
domcaps) instead of erroring loudly. Checked with a throwaway test
(same pattern as piece 1's full-capture sanity pass, removed before
committing) against a real `qemu:///session`, using `virt-install.rs`'s
exact fetch-argument shape: resolved `domain_type="kvm"`,
`arch="x86_64"`, `machine="pc-i440fx-11.1"`,
`emulator="/usr/bin/qemu-system-x86_64"`, and confirmed `domcaps.
supports_safe_host_passthrough()` reads `true` against that real
fetch — the live wiring resolves to `host-passthrough` as intended, not
silently falling back to `Custom`.

## Post-close follow-up: `clock`'s `offset` gap closed by ticket 04

Ticket 21's own Resolution section named `osinfo.get_clock()` (`clock.py`'s
`offset` field) as blocked outright — "no fallback branch at all." Ticket
04 built real `libosinfo` bindings after this ticket closed;
`osinfo::OsInfo::clock_offset()` ports `get_clock()`'s exact logic
(Windows/Solaris family → `"localtime"`, else `"utc"`) with zero new
libosinfo API surface beyond `get_family()` (no device-list/firmware-
list queries needed for this one field). Wired in as `push_default_
clock`'s new `offset` parameter, a `--os-variant <id>` CLI flag on
`virt-install` (accepting both short-id and full-id forms), and an
`OsDb::os_by_id_or_short_id` lookup helper.

**A real, deliberate output change**: `<clock>` now always carries an
explicit `offset` attribute (defaulting to `"utc"` with no `--os-
variant` given), where it previously omitted the attribute entirely and
let libvirt's own default stand. Matches upstream's actual behavior
more closely — upstream's `guest.osinfo` is never absent, even absent
`--os-variant` it falls back to a synthetic "generic" entry whose
`get_clock()` resolves to `"utc"` through the same logic, not a
separately-coded default. One pre-existing pinned test's assertion
(`clock.attribute(&doc, "offset") == None`) needed updating to match —
found and fixed via the full test suite, not silently left failing.

**Hit the same `is_qemu`-gate coverage gap slice (d) already solved,
on the first attempt at this test, not predicted in advance**: the new
real end-to-end test (`os_variant_clock_offset_survives_a_real_libvirt_
define`) initially ran against `test:///default` with the connection's
own real (always-`false`) `is_qemu()`, so `push_default_clock`'s gate
excluded the whole `<clock>` element and the test's "`localtime`
survived" assertion actually observed the *test driver's own*
synthesized `utc` default, not anything this crate emitted — caught by
reading the failing assertion's printed XML rather than assuming the
first failure was a real bug. Fixed with the same `VIRTINST_TEST_FAKE_
URI` technique `clock_trio_survives_a_real_libvirt_define_when_is_qemu_
is_true` already established.

**What `--os-variant` does *not* unblock yet**: `hyperv_supported()`
(needs `is_windows()` **and** `conn.support.conn_hyperv_clock()`, the
hv-version mechanism that's still missing entirely) and every other
`OsInfo`-gated default ticket 21's Resolution section named (disk-bus
virtio-vs-ide, `supports_virtiogpu()`, `requires_firmware_efi()`) —
those need `osinfo::OsInfo` query methods this slice didn't build
(device-list and firmware-list queries, a real device-relationship API
surface distinct from the plain string/family getters `clock_offset()`
needed). Real future work, scoped but not started.

**`supports_virtiodisk`/`supports_virtioscsi`/`supports_virtionet` now
exist on `OsInfo` (ticket 04's own file, "Second caller built, not yet
wired" section) but still aren't wired into `disk.py`'s `_default_bus`
decision tree** — that needs `is_floppy`/`is_xenpv`/`is_hvm`/`is_q35`/
`is_bhyve`/`is_x86`/machine-string checks this crate mostly doesn't
have yet, a materially bigger piece than threading one more string
through the existing builders the way `clock_offset` was. Every
builder in `install/` still hardcodes `bus="virtio"` unconditionally.

## `default_disk_bus`: the decision tree itself landed, wiring deliberately deferred

Built `domain_skeleton.rs`'s `is_hvm`/`is_xenpv`/`is_x86`/`is_q35`/
`is_riscv_virt`/`is_pseries`/`is_s390x`/`is_loongarch64` (upstream's
`domain/os.py` predicates, ported as free functions over the plain
`os_type`/`arch`/`machine` strings `GuestCapsDefaults` already
carries), `supports_virtio` (upstream's `guest.py`'s
`_supports_virtio`), `default_disk_bus` (upstream's `disk.py`'s
`_default_bus`, restricted to the two device shapes this crate's
builders actually construct — no floppy device exists anywhere here),
and `target_prefix_for_bus` (upstream's `get_target_prefix`, for
output fidelity — confirmed against a real `test:///` `define` that
libvirt itself doesn't reject a `bus`/target-`dev`-prefix mismatch, so
this isn't a correctness requirement, just matching upstream's shape).
8 new unit tests against synthetic `GuestCapsDefaults` values covering
every branch: qemu+virtio-support, q35-without-virtio-support (proving
virtio is checked *before* q35, not after), plain-x86-without-virtio
(both disk and cdrom → `ide`), non-x86 cdrom-with-virtioscsi (→
`scsi`), `!is_qemu` overriding a genuinely-virtio-capable OS (→ `ide`
regardless), xenpv/non-hvm short-circuiting before virtio is ever
considered, and `is_bhyve` winning over the plain-x86 `ide` default.
197/197 `virtinst-core` (+8), harness 61/61, `fmt`/`clippy` clean.

**Wired, in a deliberately separate follow-up commit** — the split
advised mid-session (build the tree, verify it in isolation, then wire
it) meant a future test failure while wiring could only be the wiring,
never the tree, and that's exactly how it played out: wiring hit zero
tree bugs. `push_virtio_disk`/`push_cdrom_disk` (renamed to `push_disk`,
`push_cdrom_disk` kept) now take a caller-resolved `bus: &str` argument
instead of a hardcoded `"virtio"`/`"ide"` constant, and every one of
`build_import_document`/`build_iso_document`/`build_iso_two_phase_
documents`/`build_manual_document`/`build_manual_two_phase_documents`
gained a `disk_bus`/`cdrom_bus` parameter to match — `virt-install.rs`
computes them via `default_disk_bus` (plus `Uri::is_bhyve()`,
`OsInfo::supports_virtiodisk()`/`supports_virtioscsi()`) at each of its
4 non-container call sites, the same "resolve once, pass the answer
in" pattern `clock_offset` already established. `is_bhyve` is threaded
as `Uri::is_bhyve()` exactly as flagged (not `caps.domain_type`,
avoiding the exact conflation `push_default_nic`'s own doc comment
already documents as a real, previously-shipped bug for `is_qemu`).

The blast radius stayed contained to `virt-install.rs`'s real output,
not the test suite: every builder's own unit tests pass a literal
`bus` string (`"virtio"`, `"ide"`) rather than routing through
`default_disk_bus`, so 0 existing assertions needed updating — 197/197
`virtinst-core` unchanged before and after. The one place `!is_qemu`
now genuinely flips a real default is `virt-install.rs`'s own output
for a non-qemu, `--os-variant`-less invocation (`bus="virtio"` →
`bus="ide"`) — upstream-faithful, confirmed with manual CLI runs
against `test:///default` (real connection: `ide`; `VIRTINST_TEST_
FAKE_URI=qemu:///system` with `--os-variant fedora25`: virtio-capable
path exercised without a panic). Not re-verified via a persisted
`inactive_xml()` round-trip the way `clock_offset` was, since
`test:///`'s per-connection state doesn't survive across separate
`virt-install` process invocations — the decision tree itself is
already exhaustively unit-tested (ticket 04's own file, 8 tests), and
`push_disk`/`push_cdrom_disk` placing whatever `bus` they're given
correctly is covered by every existing builder test.

**Verified**: 2 new tests in `osinfo::mod::tests` (`clock_offset`
across Linux/Windows/Solaris family branches, `os_by_id_or_short_id`'s
two lookup forms), 2 new tests in `import.rs` (`clock_offset` threading
to the emitted attribute, the real end-to-end proof), 1 pinned
assertion updated. 99/99 `virtinst-cli`, 188/188 `virtinst-core` (+4
over the 184 ticket-04 baseline), harness 61/61, `fmt`/`clippy` clean.
Manually confirmed the real CLI too: `--os-variant this-does-not-exist`
errors cleanly, `--os-variant win10` (short-id form) resolves and
defines successfully.
