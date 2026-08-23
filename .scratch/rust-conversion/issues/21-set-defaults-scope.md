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
