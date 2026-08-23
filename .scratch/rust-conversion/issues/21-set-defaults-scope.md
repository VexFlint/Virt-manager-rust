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
146/146 `virtinst-core` total. Slice (c) itself (actually pushing a
default `<graphics>` element from `virt-install.rs`) continues below.
