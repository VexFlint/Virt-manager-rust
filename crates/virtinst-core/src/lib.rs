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
pub mod diskbackend;
pub mod domain;
pub mod install;
pub mod nodedev;
pub mod osinfo;
pub mod progress;
pub mod storage;
pub mod xmlbuilder;

pub mod guest;
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
///
/// Also guards ticket 20's test-mode env vars (`VIRTINST_TEST_CAPS_
/// FILE`/`VIRTINST_TEST_DOMCAPS_FILE`/`VIRTINST_TEST_FAKE_URI`) for the
/// same underlying reason: they're process-wide state too, and a test
/// that briefly sets one while calling `Connection::open`/
/// `{Capabilities,DomainCapabilities}::fetch` can leak into any other
/// concurrently-running test making the same calls — confirmed for
/// real (not theoretical) when `capabilities::caps`'s own injection
/// tests first landed: `open_close_against_the_builtin_test_driver`
/// and `fetch_against_the_builtin_test_driver` failed intermittently
/// under default parallel `cargo test`, picking up a sibling test's
/// injected fake URI/caps file. Every test anywhere in this crate that
/// calls `Connection::open`/either `fetch` must hold this lock, not
/// just the tests that mutate one of these vars — a reader that never
/// acquires it isn't protected by another thread's lock at all.
#[cfg(test)]
pub(crate) mod test_support {
    pub(crate) static TEST_DOMAIN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
}
