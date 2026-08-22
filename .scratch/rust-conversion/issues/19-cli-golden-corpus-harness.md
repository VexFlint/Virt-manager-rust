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
- **The `test://` driver's state is per-process and ephemeral** —
  checked directly: editing+defining via one process, then reading back
  via a second `virsh`/process invocation against the same seed file,
  sees none of the first process's changes (each connection gets its
  own throwaway in-memory copy). Rules out "run the CLI binary, then
  inspect via a separate `virsh dumpxml`" as a harness shape entirely.
  Resolved by porting upstream's `--print-xml` (`virt-xml`, commit
  below) — pure local preview, upstream's own doc comment says it
  outright ("perform requested XML edits locally, but don't submit them
  to libvirt"), so the edit and the inspection happen in one process
  invocation that can see both.
- **Compare fixtures with a `test-for-virtxml`-style domain name use a
  *different* seed file than the URI survey above implied**: `vixml`
  (virt-xml's `App`) defaults to `utils.URIs.test_suite` →
  `tests/data/testdriver/testsuite.xml`, not `testdriver.xml` — a
  separate, richer fixture file with more pre-defined domains. No
  `caps=`/`domcaps=` override on this URI at all, only `,predictable` —
  meaning **virt-xml's entire `--edit`/`--add-device`/`--remove-device`
  compare corpus needs none of the capability-injection layer** this
  ticket's main finding scoped as follow-up; that gap is specific to
  `virt-install`'s per-arch default-device-selection logic, which
  `--edit`-an-existing-domain never exercises.

## Harness built (commit range starting `<TODO fill after commit>`)

`.scratch/rust-conversion/harness/run.sh` — runs the real compiled
`virt-xml` binary against `test-for-virtxml` (loaded from upstream's own
`testsuite.xml`, `VIRTINST_TEST_PREDICTABLE=1` set) via `--print-xml`,
checks specific `xmllint --xpath` assertions against what each named
upstream compare file's diff documents, reports pass/fail per case. Two
cases wired so far (`edit-simple-watchdog`, `edit-simple-vsock` — the
same two the `golden_edit_simple_*` unit tests in `virtinst-cli/src/
lib.rs` already covered with a hand-typed fixture) — this is the
stronger proof of the two: real compiled binary, real upstream fixture
domain, real `test://` driver round-trip, not an in-memory synthetic
fixture. Confirmed the seed file is genuinely never mutated (`md5sum`
before/after, both directly and now built into how the harness reasons
about safety of reuse across cases).

Extended to 15 checks / 6 cases (commit after `060ae94`): `--add-device`/
`--remove-device` now also support `--print-xml` (same local-preview
pattern, `list_add`/`remove_elements` against a locally-parsed copy
instead of `Domain::add_device`/`edit_inactive_xml`'s real redefine —
factored `fetch_and_parse_inactive`/`print_doc` once a third caller
needed the same two steps). Checked `tests/test_cli.py` directly: no
`--add-device`/`--remove-device` fixture exists for watchdog or vsock
at all, so those 4 cases are self-verified against the real driver
instead — labeled as such in the harness output, same rigor as
`virtinst-cli/src/lib.rs`'s existing `add_then_remove_device_against_a_
real_domain`/`add_then_remove_vsock_against_a_real_domain`, one level
up through the real compiled binary.

Extended again to 25 checks / 9 cases: `--disk` now has a
`disk_from_optstr`/`apply_disk_optstr` pair (deliberately a **partial**
slice of upstream's real `ParserDisk` — by far the largest, most
irregular device parser in `cli.py`, 40+ properties plus real
storage-pool/volume creation; see that function's own doc comment for
exactly which fields this slice covers). Two of the three new harness
cases hit real upstream fixtures exactly (`add-disk-basic`,
`remove-disk-index` — the latter needed no new logic at all, generic
index-based `select_elements`/`remove_elements` already covered it);
the third (`--edit`) is self-verified — `edit-simple-disk`/
`edit-simple-disk-remove-path` both need block-device `source.dev`
handling this slice doesn't model, so they're out of reach for now,
correctly labeled as such in the harness rather than faked with a
different optstring.

Extended again to 28 checks / 12 cases: `--memballoon` (sixth device,
small field set — `model`/`autodeflate`/`stats.period`/
`freePageReporting`) has all three of its real upstream fixtures
covered exactly (`edit-simple-memballoon`, `edit-disable-memballoon`,
`remove-memballoon`) — no self-verified case needed this time, unlike
watchdog/vsock/disk. No add-device case either: found empirically
while writing the real-domain test in `virtinst-cli` that the real
`test://` driver rejects a second `<memballoon>` outright ("only a
single memory balloon device is supported") — matches upstream's own
compare corpus, which has no `add-memballoon` fixture either. A real
hardware constraint, not a gap.

Extended again to 34 checks / 16 cases: `--sound`/`--video` (seventh
and eighth devices, both structs already existed in `virtinst-core` —
pure CLI wiring). Four real upstream fixtures covered exactly
(`edit-simple-soundhw`, `add-sound`, `edit-simple-video`,
`remove-video-all`). `remove-sound-model` skipped — its real command
(`--remove-device --sound ich6`) needs the field-match selector form
`select_elements` still doesn't support (matching an existing device
by its own attribute value rather than index/`all`), same deferred gap
flagged since the very first version of `select_elements`.

**The field-match `--edit`/`--remove-device` selector form is now
built** (`select_elements`'s own doc comment had flagged it since the
very first version) — `select_elements_by_selector`/
`remove_elements_by_selector` in `virtinst-cli/src/lib.rs`: when a
selector isn't an index or `"all"`, parse it as the device's own
optstring (same `remove_first` splice, same parser upstream literally
reuses via `parserclass(editval, guest=guest)`) and match existing
elements whose current field values equal what was given. Grew the
harness to 44 checks / 21 cases: five real upstream fixtures unlocked
(`edit-select-sound-model`, `remove-sound-model`,
`edit-select-disk-target`, `edit-select-disk-path`, plus the exact
`add_invalid` error text for `--edit target=vvv --disk /dev/null`) —
all five match exactly, including the pinned error string
(`"No matching objects found for --edit target=vvv"`). A sixth
(`remove-disk-path`) needs block-device `source.dev` handling, same
already-documented gap `edit-simple-disk` has, not a new one.

Extended again to 50 checks / 23 cases: `--controller`/`--tpm` (ninth
and tenth devices, both structs already existed). Two real upstream
fixtures covered (`edit-simple-controller` exactly;
`edit-simple-tpm` on every attribute-level fact — the one documented
divergence is `backend.device.path`'s clear leaving an attributeless
`<device/>` behind instead of upstream's pruned-away result, the
project's existing no-pruning stance, now hit on a nested element).
`--tpm` is also the first fixture to actually exercise
`apply_nested_attr`'s three-level chain fix in a real upstream-grounded
case, not just a synthetic regression test.

Extended again to 57 checks / 25 cases: `--smartcard`/`--redirdev`
(eleventh and twelfth devices, both structs already existed). Two more
real upstream fixtures covered exactly (`edit-simple-smartcard`,
`edit-simple-redirdev`). `--redirdev` also surfaced a real libvirt
constraint while writing its real-domain test: the stock "test" domain
has USB disabled, so `defineXML` rejects a `redirdev` outright until a
`<controller type="usb"/>` exists — same class of discovery as
memballoon's "only one" limit, not a gap in this crate.

Extended again to 61 checks / 26 cases: `--filesystem` (thirteenth
device, struct already existed). One real upstream fixture
(`edit-simple-filesystem`) covered exactly. First device with a
two-slot `remove_first` (`["source", "target"]`).

**Not yet done**: growing case/device coverage further (blocked on more
devices having a `{device}_from_optstr`/`apply_{device}_optstr` pair at
all, not on the harness itself — and for `--disk` specifically, on
extending past this session's partial slice); extending to
`virt-install`'s corpus (blocked on the capability-injection layer,
still real future work — see above, this is *not* what the
`testsuite.xml` finding resolved); `--print-diff` (needs a real diff
algorithm, not built, flagged in `virt-xml.rs`'s own doc comment).
