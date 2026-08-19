//! Wraps the `virt` crate's `Connect` (libvirt bindings, ticket 02) plus
//! gir-bound `libvirt-glib` for event integration (ticket 09) — upstream's
//! `connection.py`.
//!
//! Also where two other tickets' concrete findings land:
//! - **Live + persistent duality** (ticket 10 deferred it, ticket 12
//!   found the concrete shape, `domain.rs` now builds it): live hotplug
//!   via `Domain::attach_device()` (a no-op if the domain isn't
//!   running) and persistent-config update via `Domain::add_device()`
//!   (always runs) — two explicit calls, not one
//!   `AFFECT_LIVE|AFFECT_CONFIG` flag. The GUI-shell sequencing around
//!   them (confirm-and-degrade-to-persistent-only on hotplug failure,
//!   controller-first ordering) isn't — see `domain.rs`'s doc comment.
//! - **Driver detection** (ticket 17): trivial URI-scheme-prefix checks
//!   — `is_qemu()`/`is_xen()`/`is_lxc()`/`is_bhyve()`/`is_vz()` — feeding
//!   the per-driver device-default chains in [`crate::devices`].

pub mod uri;
pub use uri::Uri;

#[allow(clippy::module_inception)]
mod connection;
pub use connection::Connection;

mod domain;
pub use domain::{Domain, DomainError};

// TODO: libvirt-glib event-loop registration (ticket 09) — LibvirtGLib::init()
// + event_register() equivalent, once the gir binding exists.
//
// libvirt-glib event registration is still blocked, but only on
// tooling now, not missing packages: libvirt-dev and
// libvirt-glib-1.0-dev were installed 2026-08-19 (`pkg-config --exists
// libvirt`/`libvirt-glib-1.0` both succeed, the real .gir XML sources
// are present under /usr/share/gir-1.0/), which is what unblocked
// `Connection` itself. What's still missing is the `gir` codegen tool
// that turns those .gir files into Rust bindings — not an apt package,
// needs building from github.com/gtk-rs/gir. Surfaced for Vex to
// decide before spending a session on that build-tooling detour.
//
// `Connection` (this module's `connection.rs`) is done: open/close, the
// parsed `Uri`, `is_alive()` — real libvirt calls, verified against the
// builtin `test:///` driver (no daemon needed). Capabilities caching,
// domain/pool/vol/nodedev list caching, and the openAuth credential
// callback are deliberately not ported yet — see `connection.rs`'s own
// doc comment for the concrete reasons each is deferred, not just a
// generic "TODO".
