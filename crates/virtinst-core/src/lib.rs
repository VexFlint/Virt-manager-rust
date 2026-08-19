//! Domain/device XML modeling, libvirt connection handling, and
//! install/clone logic — shared by the GUI (`virt-manager`) and CLI
//! tools (`virtinst-cli`). No GTK dependency: see
//! `.scratch/rust-conversion/issues/15-virtinst-core-crate-boundaries.md`
//! for why this is one crate with these module boundaries, and which
//! upstream `virtinst/*.py` file each one replaces.
//!
//! Deliberately *not* ported: upstream's `xmlbuilder.py`/`xmlapi.py`
//! runtime dynamic-dispatch descriptor engine (ticket 06) — replaced by
//! the typed-struct + XPath derive-macro + `edit-xml`-backed DOM
//! approach these modules use instead.

pub mod capabilities;
pub mod cloner;
pub mod connection;
pub mod devices;
pub mod domain;
pub mod install;
pub mod osinfo;

mod guest;
pub use guest::Guest;

#[cfg(test)]
mod xml_binding_tests;

/// Shared by every test module that makes real libvirt calls against
/// `test:///default`'s `"test"` domain — that domain is one shared,
/// process-wide object, and `cargo test`'s default parallelism can run
/// two such tests' fetch/mutate/define cycles concurrently and lose one
/// side's change. `connection::domain`'s own tests found this for real
/// (~20% failure rate) and fixed it with a lock plus each test cleaning
/// up what it added; this is that same lock, pulled up to crate level
/// so `cloner`'s real-libvirt test can share it too instead of getting
/// its own lock that doesn't actually protect anything.
#[cfg(test)]
pub(crate) mod test_support {
    pub(crate) static TEST_DOMAIN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
}
