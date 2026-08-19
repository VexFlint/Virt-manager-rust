//! Wraps the `virt` crate's `Connect` (libvirt bindings, ticket 02) plus
//! gir-bound `libvirt-glib` for event integration (ticket 09) — upstream's
//! `connection.py`.
//!
//! Also where two other tickets' concrete findings land:
//! - **Live + persistent duality** (ticket 10 deferred it, ticket 12
//!   found the concrete shape): `Domain::attach_device()` attempts live
//!   hotplug if the domain is active, degrading to persistent-only with
//!   a confirm on failure; `Domain::add_device()` always also updates
//!   the persistent config. Two explicit calls, not one
//!   `AFFECT_LIVE|AFFECT_CONFIG` flag.
//! - **Driver detection** (ticket 17): trivial URI-scheme-prefix checks
//!   — `is_qemu()`/`is_xen()`/`is_lxc()`/`is_bhyve()`/`is_vz()` — feeding
//!   the per-driver device-default chains in [`crate::devices`].

pub mod uri;
pub use uri::Uri;

// TODO: pub struct Connection { inner: virt::connect::Connect, ... }
// TODO: pub struct Domain { ... attach_device(), add_device() (ticket 12) }
// TODO: libvirt-glib event-loop registration (ticket 09) — LibvirtGLib::init()
// + event_register() equivalent, once the gir binding exists.
//
// All three TODOs above are blocked, not just unstarted: this machine
// has libvirt0 (the runtime lib) and libvirt-clients installed, but not
// libvirt-dev (`pkg-config --exists libvirt` fails, no libvirt.h found
// anywhere) — the `virt` crate needs that to build at all. Similarly
// gir1.2-libvirt-glib-1.0 only ships the compiled .typelib for runtime
// introspection; the .gir XML source `gir` codegen needs isn't present
// (no libvirt-glib-1.0-dev-equivalent package installed). Neither is
// something to install unilaterally — surfaced for Vex to decide.
//
// What *is* done and unblocked: `uri` — URI parsing plus the
// driver-detection predicates ticket 17 found feeding the per-driver
// device-default chains (`is_qemu()`/`is_xen()`/`is_lxc()`/etc.). Pure
// string parsing, no libvirt connection needed, fully covered by tests.
