---
id: 15
title: virtinst-core module/crate boundaries
type: grilling
status: resolved
blocked_by: [02, 04, 06, 08, 09, 10, 12]
claimed_by: null
---

## Question

Six earlier tickets (02, 06, 08, 09, 10, 12) each deferred a piece of "the
domain/XML/connection layer underneath the GUI" to this not-yet-opened
ticket rather than design it themselves. Now that all of them are
resolved, what does `virtinst-core` actually look like — one crate or
several, what are the module boundaries, and where does each deferred
piece land?

## Resolution

Grounded in upstream's own `virtinst/` package layout (checked directly
rather than inventing a structure) — it's already a clean separation that
maps onto this project's decisions with very little translation:

```
virtinst/                    →  Rust module              →  decided by
  devices/*.py (20 files)    →  devices::{Disk,Network,…} →  08 (InstallMethod),
                                 one typed struct/device      12 (NewDevice) already
                                                               assume these exist
  domain/*.py (18 files)     →  domain::{Cpu,Os,SecLabel,…}→  06 (typed struct +
                                 one typed struct/subsystem    XPath derive macro)
  guest.py                   →  Guest (aggregates devices  →  06 (root DOM-bound
                                 + domain)                     type), 08 builds it
                                 directly, no DOM, per ticket 08's finding
  storage.py, network.py,    →  storage::Pool/Vol,          →  06's pattern,
  nodedev.py, snapshot.py,      network::Network, etc.         same shape as Guest
  cloner.py                     cloner::Cloner                 (added per 16)
  xmlbuilder.py, xmlapi.py   →  NOT PORTED                  →  06 explicitly
                                                                rejected this
                                                                (runtime dynamic-
                                                                dispatch descriptor
                                                                engine); the derive
                                                                macro replaces it
  connection.py              →  connection::Connection,     →  02 (virt crate +
                                 wraps `virt::Connect` +        event-registration
                                 gir-bound `libvirt-glib`       gap), 09 (libvirt-glib
                                 event registration             binds it)
  osdict.py                  →  osinfo::{OsDb, OsInfo}       →  04 (libosinfo
                                 wraps bindgen-bound             bindgen-bound,
                                 `libosinfo`                     not gir)
  install/*.py                →  install::{CloudInit,        →  08's InstallMethod
                                 Unattended, UrlDetect, …}       variants' backing
                                                                  logic
  progress.py                →  the shared async-job         →  09 (progress
                                 wrapper's progress-report       reporting piece
                                 piece                           of the shared
                                                                  wrapper)
  capabilities.py,           →  capabilities::{Caps,         →  feeds the not-yet-
  domcapabilities.py            DomCaps}                        opened per-driver-
                                                                  defaults item
```

**One crate, not several — for now.** `virtinst-core` stays a single
crate with these as internal modules mirroring the Python package's own
boundaries, rather than splitting into `virtinst-xml`/`virtinst-connection`/
`virtinst-install` crates upfront. Nothing in this map has surfaced an
actual reason to pay workspace/versioning overhead for a split yet — no
GUI-vs-CLI dependency-isolation need has come up (both need essentially
all of it), no compile-time pressure exists before there's code to
compile. Split later if a real reason shows up; starting split on
spec-only reasoning would be exactly the kind of structure nobody's
proven necessary yet.

**Live/persistent duality and device-default dispatch** (deferred by
tickets 10 and 12): land in `connection`/`domain` respectively as the
concrete mechanisms those tickets already found — `connection::Domain`
gets `attach_device()`+`add_device()` methods (ticket 12's traced
pattern), device-default logic lives as a method on each `devices::*`
type parameterized by `capabilities::DomCaps` (mirrors upstream's
`dev.set_defaults(vm.get_xmlobj())` call site) — still opaque to the GUI
layer, just now with a concrete home instead of an unnamed future.

**Shared async-job wrapper** (deferred by ticket 09): lives in the
GUI-shell layer, not `virtinst-core`. It shows a modal — that's
inherently GUI, `virtinst-core` has no GTK dependency at all. `progress.py`'s
piece (the `Meter`-equivalent progress-reporting trait `virtinst-core`'s
long-running operations report through) is the only part that lives in
core; the modal/threading/channel wrapper around it is GUI-shell.

**Flagging, not solving: CLI tools have had zero ticket attention.** The
map's own Destination explicitly includes `virt-install`/`virt-clone`/
`virt-xml` as deliverables, and upstream's `cli.py`/`virtinstall.py`/
`virtclone.py`/`virtxml.py` are real, separate entrypoints on top of this
same `virtinst-core` — but no ticket in this session has touched them.
Worth its own ticket before this map can call itself hand-off-ready; not
opened here since it's a distinct question (CLI argument surface, not
crate structure) from what this ticket was asked.
