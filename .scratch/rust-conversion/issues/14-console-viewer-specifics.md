---
id: 14
title: Console-viewer implementation specifics
type: research
status: resolved
blocked_by: [05, 09]
claimed_by: null
---

## Question

Ticket 05 deferred per-channel specifics: pure-Rust protocol crates first
(`vnc-rs`/`rust-vnc` for VNC, `spice-client` for SPICE), escalating to
gir-bound `libgvnc`/`libspice-client-glib` only if a crate can't deliver a
required parity feature for that channel. What does that escalation
actually look like per channel, given what the crates really support
today — and does the SSH-tunnel transport (`sshtunnels.py`) need its own
design pass?

## Resolution

**SSH tunnel transport — settled, no crate needed.** Traced
`sshtunnels.py`'s `_Tunnel.open()`: it's `os.fork()` +
`os.execlp("ssh", "ssh", host)` with the socket fd `dup2`'d onto the
child's stdin/stdout — literally shelling out to the system `ssh` binary
as a netcat-mode transport, not using any SSH library. Direct Rust
equivalent: `std::process::Command` piping a socket, no SSH crate in the
dependency tree at all.

**VNC — a real conflict with ticket 09, not a clean pick.**
- `vnc-rs` (checked crates.io directly: v0.5.3, released Feb 2026, active):
  supports Tight/ZRLE/Raw encodings, tested against TightVNC and macOS's
  VNC server — but it's **built on `tokio`**, an async runtime. Ticket 09
  decided *no async runtime anywhere in this app*, specifically because
  nothing needed one; this crate would be the first exception.
- `whitequark/rust-vnc`: sync, no tokio — but its own README states **"No
  encryption or authentication"** outright, and it's a thin, 35-commit
  project. Likely can't even do basic VNC password auth against a real
  libvirt/QEMU VNC display, let alone anything TLS-based.
- Neither crate's VeNCrypt/TLS or clipboard (cut-text) support is
  confirmed either way — didn't chase this down to source level, it's a
  real gap in current knowledge, not a settled "good enough."

**SPICE — ticket 05's escalation condition is already triggered, not
hypothetical.** `spice-client` (pure-Rust, the crate ticket 05 named):
display channel + input work; **audio, clipboard, and USB redirection are
explicitly in-progress/planned, not implemented** (checked directly).
Upstream's actual `SpiceViewer` class (`viewers.py`) uses exactly those
three features — `SpiceClientGLib.Audio`, `SpiceClientGtk.UsbDeviceWidget`,
clipboard sync via the main channel. So the SPICE channel needs the
gir-bound `libspice-client-glib` escalation *today*, for real parity, not
as a someday-maybe fallback.

**VNC — decided: skip pure-Rust, go straight to gir-bound `libgvnc`.**
Same escalation SPICE already needs, chosen over both alternatives rather
than chasing more research to rescue a pure-Rust pick: `vnc-rs`'s `tokio`
dependency would be the one exception to ticket 09's no-async-runtime
decision, and `whitequark/rust-vnc`'s missing auth/encryption is a real
gap against real libvirt/QEMU VNC displays, not a paperwork gap. Neither
was worth the dependency risk for what it'd save.

**Net result: both channels end up gir-bound, not pure-Rust-first.**
`libgvnc` for VNC, `libspice-client-glib` for SPICE — both are the
GTK-independent protocol/session cores ticket 05 already confirmed exist
separately from the GTK3 widget layer (ticket 01's finding), so this
still doesn't reopen any GTK3-widget-in-a-GTK4-app risk; it's the same
"bind the mature C library via `gir`" pattern already used for
`libvirt-glib` (09), just applied here too instead of the pure-Rust-
crate path ticket 05 had hoped would hold — `libosinfo` (04) went the
other way, reasserting plain `bindgen` after an empirical comparison
found gir's idiomatic-wrapper generation stubs out most of that API's
init/list-getter/matching functions (see that ticket's amendment); VNC/
SPICE's signal-and-property-heavy session APIs don't share that shape.
**Amends
ticket 05** — its "pure-Rust protocol crates first, escalate only if
needed" framing was the right starting hypothesis, but in practice both
channels escalate immediately once actually checked against real crate
capabilities.
