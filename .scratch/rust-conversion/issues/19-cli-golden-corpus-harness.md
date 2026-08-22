---
id: 19
title: CLI golden-corpus comparison harness
type: grilling
status: resolved
blocked_by: [03, 16]
claimed_by: null
---

## Question

Ticket 16 resolved the CLI's architecture; ticket 03 designated upstream's
own `tests/test_cli.py` + `tests/data/cli/` (254 CLI-compare + 62
xmlparse fixtures) as the acceptance bar for "full CLI parity." Neither
ticket built a way to actually run that corpus against this repo's
binaries. Before grinding through upstream's ~50 `Parser*` classes flag
by flag, what does the comparison harness look like, and is the full
corpus even reachable as-is?

## Resolution

**The corpus isn't uniformly reachable against a real libvirt driver.**
Traced `Command._launch_command` in `tests/test_cli.py`: every compare
test runs `virtinstall.main(conn=conn)` in-process against a shared fake
`conn`, built through `tests/magicuri.py`'s `MagicURI` scheme
(`__virtinst_test__test://...,caps=...,domcaps=...`) — a Python-level
interception of `libvirt.open()` (`virtinst/uri.py`) that swaps in canned
`virConnectGetCapabilities`/`GetDomainCapabilities` XML instead of what a
real driver returns. Counted URI usage across `test_cli.py`: `URI-KVM-X86`
alone (fake qemu/x86_64 caps+domcaps) accounts for 36 references; the two
URIs that run against the *real* `test:///` driver with no fake-capability
injection (`URI-TEST-FULL`, `URI-TEST-REMOTE`) account for only 18. The
rest (`KVM-AARCH64`, `QEMU-RISCV64`, `KVM-S390X`, etc.) all depend on the
same interception for a hypervisor/arch this machine doesn't run real
qemu for anyway.

The real `virt` crate binds real libvirt C calls — there's no
`libvirt.open()`-equivalent seam to intercept the way upstream's `ctypes`-
adjacent Python binding lets `MagicURI` do. Reproducing the full corpus
means building a Rust-side capability-override layer in `virtinst-core`'s
`connection` module (inject canned caps/domcaps XML instead of querying
the real driver, gated to test builds) — real infrastructure work, not
a config flag.

**Decision: don't build that before a harness exists at all.** Start
against the `URI-TEST-FULL`/`URI-TEST-REMOTE` subset (real `test:///`
driver, no injection needed) to get a running harness and a real
baseline pass count today. The capability-override layer is real,
necessary work for full parity — but it's now a scoped, named follow-up
(the qemu/x86_64 subset alone unlocks 36 more fixtures, the single
highest-leverage next slice) rather than a blocker on starting.

**Not yet built**: the harness script itself, and the capability-override
layer for the KVM/x86 (and other arch) fixture subset.

## Groundwork found since (not yet built on)

- **Compare fixtures are unified diffs, not full documents.**
  `tests/data/cli/compare/*.xml` hold a before/after diff (domain XML
  before the CLI action, arrow to after) plus the command's stdout —
  much cheaper to hand-verify or generate than a full golden document.
  Two already hand-verified directly against these real files:
  `virt-xml-edit-simple-watchdog.xml`, `virt-xml-edit-simple-vsock.xml`
  (see the `golden_edit_simple_*` tests in `virtinst-cli/src/lib.rs`,
  commit `2ea8f52`).
- **Even the `test:///`-only subset needs a "predictable" mode — built,
  commit `86ea310`.** `URI-TEST-FULL`/`URI-TEST-REMOTE` both route
  through upstream's `MagicURI` with a `,predictable` flag — real
  UUIDs/MACs would never match a golden fixture otherwise. Ported the
  exact mechanism (`virtinst/guest.py:125`, `virtinst/devices/
  interface.py:111`) into `virtinst-core::cloner`'s `generate_uuid`/
  `generate_mac`: a fixed UUID constant, and a MAC that's always
  `"00:11:22:33:44:55"` unless `VIRTINST_TEST_SUITE_INCREMENT_MACADDR`
  is also set (then a process-global counter increments the last
  digit). Gated by `VIRTINST_TEST_PREDICTABLE`, an env var rather than
  a threaded flag — see the commit message for why that's sufficient
  (this harness spawns each CLI binary as its own OS process, so
  predictable/non-predictable are never needed in the same one).
  **Not yet used by anything** — the harness script itself still
  doesn't exist to set this var.
- **`test:///<path-to-xml>` as the connection URI is a real native
  libvirt `test://` driver feature** (loading `tests/data/testdriver/
  testdriver.xml` as backing state), separate from the `caps=`/
  `domcaps=` override which is the Python-only `MagicURI` interception
  this ticket's main finding is about. The harness can open that XML
  file directly through the real `virt` crate with no interception
  layer needed for *this* part.
