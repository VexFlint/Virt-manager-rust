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
