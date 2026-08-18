//! One typed struct per domain-XML subsystem — upstream's `domain/*.py`
//! (18 files: cpu, clock, os, features, seclabel, sysinfo, vcpus,
//! xmlnsqemu foreign-namespace preservation, etc.).
//!
//! Bound to XPaths via the derive macro from ticket 06, read/written
//! through the `edit-xml`-backed order-preserving DOM. `xmlnsqemu.py`'s
//! foreign-namespace preservation is exactly what makes the DOM
//! approach necessary in the first place (ticket 03's acceptance bar).

pub mod clock;
pub use clock::Clock;

pub mod memory;
pub use memory::CurrentMemory;

pub mod pm;
pub use pm::DomainPm;

pub mod memtune;
pub use memtune::DomainMemtune;

// TODO: the rest (cpu, os, features, seclabel, sysinfo, vcpus,
// numatune, xmlnsqemu, ...). `features.py` in particular needs a new
// macro capability first — most of its ~40 fields use libvirt's
// on/off boolean spelling (is_onoff), not yes/no (Option<bool>'s
// current mapping) — flagged, not silently modeled wrong.
