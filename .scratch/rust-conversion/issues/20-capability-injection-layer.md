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

Not starting either autonomously — this ticket's job was scoping, per
the explicit request. (a) alone is a reasonable, bounded next step if
wanted; (b) is a multi-session undertaking that deserves its own
explicit go-ahead once (a) exists and its shape is provable against a
first real fixture.
