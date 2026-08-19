//! Wraps `virt::connect::Connect` (ticket 02) — upstream's
//! `VirtinstConnection` (`connection.py`).
//!
//! **Minimal first cut**: open, close, the parsed [`Uri`], and
//! `is_alive()` — real libvirt calls, verified against libvirt's
//! built-in `test:///` driver (no daemon, no real hypervisor needed;
//! the same driver the `virt` crate's own test suite opens against).
//!
//! **Deliberately not ported yet**, all real upstream behavior beyond
//! opening a connection:
//! - `openAuth`'s credential-callback mirroring (upstream's
//!   `open(authcb, cbdata)`, mirroring `virConnectCredTypeDefault`).
//!   `Connect::open_auth` exists in the `virt` crate; wiring it needs a
//!   real callback from the (not yet built) GUI/CLI prompt layer, so
//!   there's nothing to plug it into yet. Plain `Connect::open`
//!   (`virConnectOpen`, no auth) covers every URI that doesn't need
//!   interactive credentials — `test:///default`, local `qemu:///system`.
//! - Capabilities caching (`_get_caps`/`self._caps`), domain/pool/vol/
//!   nodedev list caching (`_fetch_helper`/`_fetch_all_domains_raw`,
//!   etc.) — the domain-list path already has what it needs
//!   ([`crate::Guest`]), but pools/vols/nodedevs don't have Rust types
//!   yet (ticket 15's `storage`/`nodedev` modules, not started).
//! - `MagicURI` — not ported, see [`super::uri`]'s doc comment for why.
//! - libvirt-glib event registration (ticket 09) — blocked on the
//!   `gir` codegen tool itself, which isn't an apt package (would need
//!   building from `github.com/gtk-rs/gir`), separate from the `.gir`
//!   XML source files (present) that tool would consume.

use crate::connection::Uri;
use virt::connect::Connect;
use virt::error::Error;

/// A live connection to a libvirt driver.
#[derive(Debug)]
pub struct Connection {
    inner: Connect,
    uri: Uri,
}

impl Connection {
    /// Opens a connection to `uri` — upstream's
    /// `VirtinstConnection.open()`, minus auth-callback support (see
    /// this module's doc comment for why that's not here yet).
    pub fn open(uri: &str) -> Result<Connection, Error> {
        let inner = Connect::open(Some(uri))?;
        Ok(Connection {
            inner,
            uri: Uri::parse(uri),
        })
    }

    /// The URI this connection was opened with, already parsed —
    /// upstream's `uri` property plus the `URI` object it wraps
    /// ([`Uri`]'s driver-detection predicates, e.g. `is_qemu()`, are
    /// reachable straight off this).
    pub fn uri(&self) -> &Uri {
        &self.uri
    }

    /// Whether the underlying libvirt connection is still usable —
    /// direct passthrough to `virConnectIsAlive`.
    pub fn is_alive(&self) -> Result<bool, Error> {
        self.inner.is_alive()
    }

    /// Closes the connection — upstream's `close()`. Consumes `self`
    /// rather than upstream's leave-it-open-but-null-the-handle
    /// pattern; there's nothing else on this minimal struct that would
    /// need to keep working on a closed connection yet.
    pub fn close(mut self) -> Result<(), Error> {
        self.inner.close()?;
        Ok(())
    }

    /// The underlying `virt` handle — `Domain::lookup_by_name`/
    /// `define_xml` both need a bare `&Connect`, and `Connect` is
    /// cheaply `Clone` via ref-counting, so `Domain` borrows this
    /// rather than holding its own clone (each clone needs its own
    /// `close()`, a lifecycle this crate doesn't need to manage yet).
    pub(crate) fn as_connect(&self) -> &Connect {
        &self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real libvirt call, no daemon or real hypervisor needed —
    /// libvirt's `test:///` driver is compiled into `libvirt.so`
    /// itself, so if this crate builds at all (it links against
    /// libvirt-dev unconditionally now), this driver is guaranteed
    /// available. Same URI the `virt` crate's own test suite uses.
    #[test]
    fn open_close_against_the_builtin_test_driver() {
        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        assert_eq!(conn.uri().scheme, "test");
        assert!(conn.is_alive().expect("freshly-opened connection is alive"));
        conn.close().expect("closing a live connection succeeds");
    }

    #[test]
    fn open_rejects_a_garbage_uri() {
        let err = Connection::open("not-a-real-libvirt-driver:///whatever");
        assert!(err.is_err());
    }
}
