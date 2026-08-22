---
id: 20
title: Capability-injection layer for virt-install's fake-arch corpus
type: grilling
status: resolved
blocked_by: [17, 19]
claimed_by: null
---

## Question

Ticket 19 named the capability-injection layer as the highest-leverage
follow-up (36 `URI-KVM-X86` fixtures alone) but didn't scope it beyond
"real infrastructure work." What exactly does upstream's `MagicURI` do,
what's the equivalent Rust-side seam, and — critically — does building
it actually unlock those fixtures, or is something else still missing?

## Resolution

**Traced `MagicURI` in full** (`virtinst/uri.py`, `tests/magicuri.py`,
`tests/utils.py`), not just the summary ticket 19 already had. It does
three separate things, once the connection is open:

1. `conn.getCapabilities = lambda: capsxml` — monkeypatches the host
   capabilities call to return canned XML from a file.
2. `conn.getDomainCapabilities = fake_domcaps` — same for domain
   capabilities, with a small `arch>...</arch` regex substitution when
   `--arch` overrides the reported arch on a shared fixture file.
3. `fakeuri` — the connection is *actually* opened against a real
   `test:///path/to/driver.xml` (always connectable, no daemon), but
   `conn.getURI()` is separately monkeypatched to report a different
   string entirely (e.g. `qemu:///system`) — everything that branches
   on the URI scheme (`is_qemu()`/`is_xen()`/etc.) sees the fake one,
   while the actual libvirt calls hit the safe, harmless test driver.

Every fake-arch URI in `tests/utils.py` (`kvm_x86`, `kvm_aarch64`,
`qemu_riscv64`, ...) is `test:///testdriver.xml` + `fakeuri=qemu:///
system` (or `xen:///`, etc.) + real captured `caps=.../domcaps=...`
files from `tests/data/capabilities/` — never a real qemu/xen/etc.
process, ever.

**Fixture count, counted directly** (not assumed from ticket 19's
one-URI figure):

| URI | fixture references |
|---|---|
| `URI-KVM-X86` | 36 |
| `URI-QEMU-RISCV64` | 7 |
| `URI-KVM-LOONGARCH64` | 7 |
| `URI-KVM-AARCH64` | 6 |
| `URI-KVM-S390X` | 4 |
| `URI-KVM-X86-NODOMCAPS` | 2 |
| `URI-KVM-PPC64LE` | 2 |
| `URI-KVM-ARMV7L` | 2 |
| **Total gated behind capability injection** | **66** |

(`URI-TEST-FULL`/`URI-TEST-REMOTE`, 18 references, need none of this —
already reachable, per ticket 19.)

### The real `virt` crate has no `libvirt.open()`-equivalent seam — confirmed again, but two exploitable seams already exist on this side

- **`DomainCapabilities::fetch`/`::parse` are already split** (ticket
  17's own design, `capabilities/domcaps.rs`) specifically so a
  captured fixture can be tested without a live connection. The
  injection point already exists structurally — it just isn't gated on
  anything yet. Same shape needed for host `Capabilities`, which
  **doesn't exist in Rust at all yet** — a real gap ticket 17 didn't
  flag (it scoped `DomainCapabilities` only; host `capabilities.py`'s
  `Capabilities`/`guest_lookup` never came up because nothing consumed
  it at the time).
- **`Connection::uri()` is client-side-parsed from the literal string
  passed to `open()`** (`connection.rs`: `Uri::parse(uri)`, no
  `virConnectGetURI()` call at all) — confirmed by reading the actual
  code, not assumed. This is good news structurally (no libvirt call to
  intercept) but means today `Connection::open("qemu:///system")`
  passes that exact string to the *real* `Connect::open` too — there's
  no split yet between "the URI that must actually be openable" and
  "the URI whose scheme drives `is_qemu()`-style checks," which
  upstream's `fakeuri` provides and this doesn't yet.

### Safety-critical finding, not a stylistic one

This machine has `virtqemud.socket` enabled with a real, working
QEMU/KVM x86_64 driver (fixed earlier this session, see `memory/
upstream-reference-restored.md`'s sibling notes). **A harness that
opened `qemu:///system` directly to simulate a "fake" KVM-x86 fixture
would hit the real driver on this machine, not a safe stand-in** — a
real correctness and safety hazard, not a hypothetical one. Any
injection design must route the actual connection through `test://`
even when simulating `qemu:///system`/`xen:///`/etc. — never let the
"reported" URI and the "opened" URI be the same string for a fake-arch
case.

### Proposed architecture (scoped, not built)

1. **Host `Capabilities`** (new — `virtinst-core::capabilities::caps`
   or similar): direct port of `capabilities.py`'s ~10 small
   `XMLBuilder` classes (`_CapsHost`/`_CapsCPU`/`_CapsSecmodel`/
   `_CapsGuest`/`_CapsDomain`/`_CapsMachine`/`_CapsGuestFeatures`) —
   comparable size and shape to `domcaps.rs`, same deserialize-only,
   no-DOM approach ticket 17 already decided for `DomainCapabilities`.
   Plus the one real algorithm in the file, `guest_lookup`
   (`_guestForOSType`/`_bestDomainType`, a priority list
   `kvm > xen > hvf > qemu`) — worth porting faithfully since it's the
   actual decision logic behind hypervisor-type/arch/machine selection,
   not just data.
2. **`Capabilities::fetch`/`::parse` split**, mirroring
   `DomainCapabilities` exactly.
3. **Env-var-gated injection inside `fetch`** on both types —
   `VIRTINST_TEST_CAPS_FILE`/`VIRTINST_TEST_DOMCAPS_FILE`: when set,
   read the file directly and `parse()` it instead of calling libvirt.
   Same precedent and rationale as `VIRTINST_TEST_PREDICTABLE` (env var,
   not a threaded flag — the harness spawns each case as its own OS
   process, so no invocation ever needs two different values in the
   same process). Zero changes needed at any of `fetch`'s existing
   call sites (`import.rs`/`container.rs`/`iso.rs`/`manual.rs`) — the
   gate lives once, inside `fetch` itself.
4. **A reported-URI override**, analogous to 3 — e.g.
   `VIRTINST_TEST_FAKE_URI`, checked by `Connection::open` (or a
   sibling constructor) to build `self.uri` from the *env var's* value
   while still calling `Connect::open` with the real, always-safe
   `test://...` argument. This is the piece that doesn't have a
   half-built precedent yet and needs its own small design pass when
   actually built.

This is a bounded, buildable slice — comparable in size to what
`domcaps.rs` already took, plus one new env-var gate and one new
URI-override mechanism. **Not itself the blocker on reaching the 66
fixtures.**

### The actual blocker: virt-install's own default-resolution pipeline isn't there yet

Checked what today's Rust `virt-install` actually does with
`DomainCapabilities` at each of its six call sites
(`import.rs`/`container.rs`/`iso.rs`×2/`manual.rs`×2): **only `arch` is
threaded through.** Domain `type` (`kvm`/`qemu`/`test`/...), `os_type`,
`loader`, `emulator`, and `machine` are either hardcoded per install
method or caller-supplied some other way — none of them come from a
real `guest_lookup`-equivalent decision. `domain_skeleton.rs`'s own doc
comment already says as much for `arch` specifically ("still always
passed through... even for containers"), but the other capsinfo fields
have no equivalent path *at all* yet.

Upstream's own `virt-install` compare fixtures are also a different
shape from `virt-xml`'s: **full generated documents, not diffs**
(checked directly — `virt-install-w2k3-cdrom.xml` etc. are complete
`<domain>` trees, since there's no "before" state to diff against when
building a domain from scratch). Matching one exactly means reproducing
upstream's *entire* default-resolution output for that fixture — CPU
topology, clock offset, PM block, controller defaults, the 9-file
per-driver device-default matrix ticket 17 already catalogued — not
just the arch/hypervisor-type selection this ticket's injection layer
would add.

**So: the capability-injection layer is necessary but not sufficient.**
Building it unlocks the *data* (fake caps/domcaps reach virt-install's
code), but reaching an actual passing fixture also needs the
*consumer* — ticket 17's per-driver default chains, applied to
`virt-install` specifically, well beyond where it is today. That is
real, separate, and substantially larger work than the injection layer
itself.

## Decision

Two distinct pieces of future work, not one:

- **(a) The capability-injection layer itself** — scoped above,
  bounded, roughly `domcaps.rs`-sized plus two env-var gates. A
  reasonable next slice on its own.
- **(b) Reaching the 66 fake-arch fixtures** — needs (a) *plus*
  substantially completing virt-install's own capability-driven
  defaulting (hypervisor type, os_type, loader, emulator, machine, CPU/
  clock/PM defaults, the 9-file driver-family matrix). Much larger,
  effectively "finish `virt-install`'s default-resolution logic,"
  ticket 17's real remaining work, not a byproduct of (a).

## (a) built

`virtinst-core::capabilities::caps` — direct port of `capabilities.py`'s
real classes (`Capabilities`/`CapsHost`/`CapsCpu`/`CapsGuest`/
`CapsDomain`/`CapsMachine`/`CapsGuestFeatures`/`CapsInfo`), same
deserialize-only no-DOM shape `DomainCapabilities` already established,
plus `guest_lookup`'s real algorithm in full (`_guestForOSType`'s
two-pass arch fallback, `_bestDomainType`'s `kvm > xen > hvf > qemu`
priority, the `os_type == "linux"` xen quirk, all four upstream error
messages verbatim). `<host><secmodel>`/query-helper methods/
`is_machine_alias` explicitly not modeled — no current consumer, same
as `domcaps.rs`'s own precedent.

Both `Capabilities::fetch` and `DomainCapabilities::fetch` now check
`VIRTINST_TEST_CAPS_FILE`/`VIRTINST_TEST_DOMCAPS_FILE` before making a
real libvirt call, reading and parsing that file instead when set —
zero changes needed at any existing call site. `Connection::open` checks
`VIRTINST_TEST_FAKE_URI` to build the *reported* URI independently of
what's actually opened — the real call always uses the literal `uri`
argument, verified by a real, live `test://` connection staying alive
under the override (not just that the reported scheme changed).

**A real, confirmed test race found and fixed along the way**: the
first version of these tests used their own local locks, and two
*pre-existing*, previously-unguarded tests (`open_close_against_the_
builtin_test_driver`, `DomainCapabilities`'s/`Capabilities`'s own
`fetch_against_the_builtin_test_driver`) failed intermittently under
default parallel `cargo test` — picking up a sibling test's injected
fake URI/caps file mid-run. Fixed by reusing the crate's existing
`test_support::TEST_DOMAIN_LOCK` everywhere instead (broadened its
documented scope to cover this too) — every test in the crate that
calls `Connection::open` or either `fetch` now holds it, not just the
ones that mutate an env var; a reader that never acquires a lock isn't
protected by another thread holding it.

**Verified**: 134/134 `virtinst-core` tests (16 new) across 5
consecutive parallel `cargo test` runs with zero flakes, 98/98
`virtinst-cli`, harness 61/61, `cargo fmt`/`clippy` clean.

**Not done — piece (b), deliberately**: nothing yet calls
`Capabilities::guest_lookup` from any real `virt-install` code path.
Today's install methods still only thread `DomainCapabilities::arch`
through (see `domain_skeleton.rs`'s doc comment) — wiring
`hypervisor_type`/`os_type`/`loader`/`emulator`/`machine` from a real
`guest_lookup` result into the actual domain builders, and the
CPU/clock/PM/9-file driver-family defaulting beyond that, is the
separate, larger undertaking this ticket's own "Decision" section
already named. The injection layer is ready for it; nothing consumes
it yet.

## (b) started: `guest_lookup` wired into `virt-install`, minimal slice

Traced upstream's real pipeline first (`guest.py`'s `lookup_capsinfo`/
`set_capabilities_defaults`/`DomainCapabilities.build_from_guest`), not
assumed: `guest_lookup` runs off *host* `Capabilities`
(`self.conn.caps.guest_lookup(...)`), not `DomainCapabilities` — the
crate's own `virt-install.rs` had been calling
`DomainCapabilities::fetch(&conn, None, None, None, None)` directly and
reading `.domain`/`.arch` off it, which happened to agree with
`guest_lookup`'s real algorithm on every fixture this crate's tests
reach (confirmed directly: `test:///default`'s real capture resolves to
`hypervisor_type="test"`, `arch="i686"` either way — pinned by
`caps.rs`'s `guest_lookup_finds_hvm_over_xen_by_default` test using the
exact same capture `domcaps.rs`'s equivalent test already used), but
only `guest_lookup` is the actual upstream algorithm rather than
libvirt's incidental default-param behavior.

**What changed**: `virt-install.rs` now fetches host `Capabilities`,
calls `guest_lookup(None, None, None, None)` (no `--arch`/`--os-
variant`/`--virt-type`/`--machine` CLI surface exists yet to override
any of the four filters, matching upstream's own no-flags-given case —
a freshly constructed `Guest` starts with all four unset), and reads
`hypervisor_type`/`arch`/`os_type` off the resulting `CapsInfo` instead
of `DomainCapabilities`. `os_type` is a new value threaded all the way
through — `build_import_document`/`build_iso_document`/`build_iso_two_
phase_documents`/`build_manual_document`/`build_manual_two_phase_
documents` all gained an `os_type: &str` parameter, replacing each
one's previously-hardcoded `"hvm"` literal passed to `build_domain_
skeleton`. `DomainCapabilities::fetch` is no longer called anywhere in
`virt-install.rs` at all — its only past consumer there was `.domain`/
`.arch`, both now sourced from `capsinfo`; upstream still fetches
domcaps separately (`build_from_guest`, keyed off the *resolved*
emulator/arch/machine/type) for UEFI firmware lookup and device-support
queries this crate hasn't ported, so this isn't a regression, just an
honest "no current consumer" the way `domcaps.rs`/`caps.rs` already
posture other unmodeled fields.

**`--container` deliberately excluded from the `os_type` thread**:
`build_container_os_document` keeps its own hardcoded `"exe"`, unchanged.
Traced why before assuming it should also take a resolved `os_type`:
upstream's `--container` sets `options.os_type = "exe"` *before*
`lookup_capsinfo` runs, which then calls `guest_lookup(os_type="exe",
...)` — the filter, not the output. That only works because a real LXC
connection's host capabilities XML advertises an `exe` guest type; no
driver reachable from this crate does (confirmed against the real
`test:///default` capture used throughout this ticket — only `hvm`/
`xen` guest entries exist), so filtering `guest_lookup` on `"exe"` here
would just fail the lookup outright. Threading a plain, unfiltered
`guest_lookup(None,...)` result's `os_type` into the container builder
instead would be actively wrong (it would silently overwrite `"exe"`
with whatever `hvm`/`xen` guest_lookup's default path picks) — so this
one builder correctly stays exactly as it was.

**The existing end-to-end tests only prove half of this — caught by
advisor review, not assumed adequate.** Every pre-existing install-
module end-to-end test now passes a literal `"hvm"` for the new
`os_type` parameter, so they pin the *old* hardcoded value and prove
nothing about the new `capsinfo.os_type → build_*_document` path
`virt-install.rs` actually runs. Two gaps that review surfaced, both
closed rather than left as caveats:

1. **The composition itself was untested.** Added `capsinfo_os_type_
   actually_flows_into_the_built_document` (`import.rs`) — runs
   `virt-install.rs`'s exact sequence (`Capabilities::fetch` →
   `guest_lookup` → `capsinfo.os_type` → `build_import_document`)
   against `test:///default` with `VIRTINST_TEST_CAPS_FILE` pointed at
   a fixture whose only guest reports `os_type=xen` (every real capture
   used elsewhere in this crate is `hvm`, so a `"hvm"` in the output
   could only mean a stray hardcoded literal survived upstream of the
   call). Asserts the built document's `<os><type>` text is literally
   `xen`, not `hvm`.
2. **Every prior check used `test:///default`'s single-domain-per-guest
   shape.** `--connect`'s real default is `qemu:///system`, and this
   machine has a real, live KVM/x86_64 driver with multiple `<domain>`
   entries and populated machine lists per guest — exactly the shape
   `best_domain_type`'s `kvm > xen > hvf > qemu` priority chain and the
   two-pass arch fallback exist to handle, untested until now. A live
   connection attempt hit this environment's own polkit/DBus flakiness
   (`qemu:///system` auth timing out non-interactively, unrelated to
   this crate), so verified instead the same way `domcaps.rs`'s own
   `QEMU_DOMCAPS_EXCERPT` does: captured `virsh -c qemu:///system
   capabilities` on this machine (real KVM/Skylake host) and added
   `guest_lookup_on_a_real_qemu_system_capture_prefers_kvm_x86_64`
   (`caps.rs`) against a trimmed, real excerpt — confirms `guest_lookup`
   resolves `kvm`/`hvm`/`x86_64` (not e.g. `qemu` or the wrong arch) on
   real multi-domain, multi-machine production-shaped data, including
   the arch-fallback correctly preferring the host's `x86_64` guest
   over the i686 one that comes first in document order.

The five pre-existing end-to-end tests still pass unchanged too (byte-
identical prior output, now with an explicit `"hvm"` argument instead
of a hardcoded literal) — real evidence the refactor didn't disturb
existing behavior, just not evidence of the new path on its own, which
is what the two additions above are for.

**Still not done, explicitly out of scope for this slice**: `loader`/
`emulator`/`machine` are not threaded anywhere — `set_capabilities_
defaults` only sets them when unset AND (for `emulator`) not xenpv/not
`vz`, guards this crate has no CLI surface to ever make false, and
porting `Guest::get_recommended_machine` now would add a function
verified to return `None` on every arch this crate can currently reach
(`test:///`'s i686, a real `qemu:///system`'s x86_64 — `_qemu_machine()`
has no x86 branch at all). CPU/clock/PM defaults and the 9-file
driver-family device matrix remain entirely unstarted, per this
ticket's own "Decision" section.

**One more advisor finding, minor but real, also fixed**: `os_type`
was originally resolved eagerly (unwrap-or-exit) before the install-
method dispatch, so a missing `os_type` would hard-exit even on the
`--container` branch, which never uses the value at all
(`build_container_os_document` hardcodes `"exe"`). Changed to a
`require_os_type()` closure resolved lazily at each of the five non-
container call sites instead — `--container` no longer depends on a
field it doesn't consume. Can't actually fire in practice (`<guest>
<os_type>` is a required element in every real driver's capabilities
XML), but there's no reason for that path's success to depend on it.

**Verified**: 99/99 `virtinst-cli`, 136/136 `virtinst-core` (2 new: the
composition test and the real-qemu-capture test), harness 61/61,
`cargo fmt`/`clippy` clean workspace-wide.
