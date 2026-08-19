//! Capability queries — upstream's `capabilities.py`/`domcapabilities.py`.
//!
//! `DomainCapabilities` gets its own lightweight deserialize-only path,
//! deliberately *separate* from ticket 06's order-preserving DOM (ticket
//! 17): it's fetched fresh per-connection via
//! `virConnectGetDomainCapabilities`, cached per-`Guest`, and never
//! edited or serialized back — none of the DOM's preservation machinery
//! earns its keep on a read-only, cached path.

pub mod domcaps;
pub use domcaps::{
    CapabilitiesError, CapsBlock, CapsEnum, DomainCapabilities, DomainCapabilitiesDevices,
};

// TODO: host capabilities (upstream's capabilities.py — separate from
// domcapabilities.py, general host info rather than per-driver device
// defaults) — not started, ticket 17's own need was specifically
// DomainCapabilities.
//
// TODO: DomainCapabilities' os/cpu/features blocks — see domcaps.rs's
// doc comment for why those are a different shape and deliberately not
// built yet.
//
// TODO: caching per-Guest (upstream's lookup_capsinfo()) — nothing
// calls DomainCapabilities::fetch repeatedly yet to make caching worth
// building ahead of a real caller.
