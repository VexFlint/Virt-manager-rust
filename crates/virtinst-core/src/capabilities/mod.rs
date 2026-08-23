//! Capability queries — upstream's `capabilities.py`/`domcapabilities.py`.
//!
//! `DomainCapabilities`/`Capabilities` both get their own lightweight
//! deserialize-only path, deliberately *separate* from ticket 06's
//! order-preserving DOM (ticket 17 for the former, ticket 20 for the
//! latter): both are fetched fresh per-connection via a real libvirt
//! call, never edited or serialized back — none of the DOM's
//! preservation machinery earns its keep on a read-only path.
//!
//! Ticket 20's test-mode injection lives on both `fetch` functions
//! directly (`VIRTINST_TEST_CAPS_FILE`/`VIRTINST_TEST_DOMCAPS_FILE`) —
//! see `caps.rs`'s doc comment for the full rationale (mirrors
//! `VIRTINST_TEST_PREDICTABLE`'s existing env-var precedent; upstream's
//! own equivalent, `MagicURI`, monkeypatches `conn.getCapabilities`/
//! `getDomainCapabilities` after the connection opens, a seam the real
//! `virt` crate doesn't have — ticket 19's finding). The other half of
//! `MagicURI` — faking the *reported* connection URI so `is_qemu()`-
//! style scheme checks pass while the real call stays on the harmless
//! `test://` driver — lives on `Connection::open` instead
//! (`VIRTINST_TEST_FAKE_URI`), not here; see `connection.rs`'s doc
//! comment.

pub mod domcaps;
pub use domcaps::{
    CapabilitiesError, CapsBlock, CapsEnum, DefaultCpuMode, DomainCapabilities,
    DomainCapabilitiesDevices,
};

pub mod caps;
pub use caps::{
    Capabilities, CapsCpu, CapsDomain, CapsGuest, CapsGuestFeatures, CapsHost, CapsInfo,
    CapsMachine,
};

// TODO: DomainCapabilities' os/features blocks — see domcaps.rs's doc
// comment for why those are a different shape and deliberately not
// built yet. `cpu` is now built (ticket 21 slice (f)).
//
// TODO: caching per-Guest (upstream's lookup_capsinfo()) — nothing
// calls DomainCapabilities::fetch repeatedly yet to make caching worth
// building ahead of a real caller. Same now applies to Capabilities::
// fetch/guest_lookup (upstream's own lookup_capsinfo caches this too).
//
// TODO (ticket 20, scoped but not started): virt-install's own
// call sites don't call Capabilities::guest_lookup yet at all -- today
// only DomainCapabilities::arch is threaded through (see domain_
// skeleton.rs's doc comment). Wiring guest_lookup's hypervisor_type/
// os_type/loader/emulator/machine into the actual builders is ticket
// 20's "(b)" piece, separate from this module's own injection layer.
