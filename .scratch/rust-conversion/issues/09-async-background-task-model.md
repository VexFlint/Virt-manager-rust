---
id: 09
title: Async / background-task model
type: grilling
status: resolved
blocked_by: [05]
claimed_by: null
---

## Question

Spun out of ticket 08: multiple upstream screens run background work off
the GTK main thread and resume it on completion — `createvm.py`'s
OS-detection probe on Install→Next, `asyncjob.py` (the shared progress
modal used by any long-running libvirt call), `migrate.py`, `clone.py`.
None of this has a Rust equivalent decided yet.

What's the async/background-task model for a `gtk4-rs` app — a full async
runtime (`tokio`) bridged into the GLib main loop, or the idiomatic
`gtk4-rs` pattern of `glib::MainContext::spawn_local` + channels back to
widget-owning code, or plain OS threads + `glib::idle_add`/channels (closer
to what upstream's Python actually does — a real thread reporting progress
via `GLib.idle_add`)? And what's the shared "modal blocks input, shows
progress, is cancellable" widget wrapper (`asyncjob.ui` equivalent) that
every long-running call routes through?

## Resolution

Grounded in source archaeology, not assumption: `asyncjob.py`'s actual
pattern (a plain `threading.Thread` running the blocking call, results
bridged back via `GLib.idle_add`, cancellation a cooperative flag checked
between steps since a blocking C call can't be force-killed mid-flight),
the `virt` crate's `Connect` (`unsafe impl Send + Sync`, verified in
source — safe to hold/use from a real OS thread), and — the big find —
`virtmanager.py`'s bootstrap hooking libvirt's event loop into GLib via
`libvirt-glib` (`LibvirtGLib.init()`/`event_register()`), not a hand-rolled
pump. That last one reaches back into ticket 02; amended there rather than
re-litigated here.

**Two separate problems, two separate (but related) answers:**

1. **libvirt's own event loop** (needed for ticket 02's domain/network/etc.
   event-registration gap): bind `libvirt-glib` via `gir` — a different
   binding approach from `libosinfo` (ticket 04), which reasserted plain
   `bindgen` after an empirical comparison (see that ticket's amendment) —
   `libvirt-glib`'s event/signal-heavy API is exactly the shape gir's
   idiomatic-wrapper generation is built for, unlike libosinfo's
   out-param/custom-list-type shape. It's what upstream actually uses, it's
   already installed and gir-bound on this machine, and it's pure GObject/GLib so it
   carries none of ticket 01's GTK-version binding risk. Once registered, it
   pumps natively on the same `GMainContext` the GTK app already runs — no
   dedicated thread, no hand-written FFI shim for the loop-pumping half of
   the problem. (Full detail on how much of the *registration* half this
   also covers: see ticket 02's amendment.)

2. **Blocking-call offload** (every long-running libvirt call — Create VM's
   Finish, migrate, clone): plain OS threads (`std::thread::spawn`) running
   the blocking `virt` call, results bridged back via
   `glib::MainContext::channel()` (the `gtk4-rs`-idiomatic equivalent of
   `GLib.idle_add`). No async runtime (`tokio`) — every `virt` call is fully
   synchronous, there's no async I/O in this dependency for a runtime to
   multiplex, and `glib` is already a hard dependency via `gtk4-rs` where
   `tokio` would be a new one. Matches upstream's own 19-years-proven
   pattern exactly.

**Cancellation:** cooperative, same as upstream — a shared `Arc<AtomicBool>`
(or equivalent cheap flag) checked at the same between-steps points
upstream's `job_canceled` is. Not a Python limitation being ported forward;
a property of blocking C calls having no interrupt mechanism, true in any
language wrapping this API.

**Shared wrapper:** one generic wrapper (spawn closure, show a cancellable
progress modal, bridge the result back) that every long-running call routes
through, mirroring upstream's single `asyncjob.py`/`_vmmAsyncJob` class —
already flagged as shared infra in ticket 07. Its exact signature and which
crate it lives in are implementation detail deferred to whoever writes it /
the not-yet-opened `virtinst-core` crate-boundaries ticket, not decided
here — same scope discipline ticket 08 used for its own deferrals.
