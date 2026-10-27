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
//! - `MagicURI` itself — not ported, see [`super::uri`]'s doc comment
//!   for why. `open()` below does port one of its effects (ticket 20):
//!   `VIRTINST_TEST_FAKE_URI` overrides what [`Uri::parse`] sees,
//!   independent of what string actually gets opened — safety-critical
//!   on a machine with a real, working QEMU/KVM driver: a fake-arch
//!   test case must never actually open `qemu:///system` for real just
//!   because it wants `is_qemu()`-style checks to see that scheme.
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
    ///
    /// The *real* libvirt call always uses `uri` exactly as given —
    /// never overridden, never faked. `VIRTINST_TEST_FAKE_URI`, when
    /// set, only changes what [`Uri::parse`] sees for the returned
    /// [`Connection::uri`] — ticket 20's port of `MagicURI`'s
    /// `fakeuri` effect: a harness case can open the always-safe
    /// `test://...` driver while `is_qemu()`-style scheme checks
    /// downstream see `qemu:///system` (or `xen:///`, etc.) instead.
    /// Same env-var precedent as `VIRTINST_TEST_PREDICTABLE` — each
    /// harness case is its own OS process, so there's never a need for
    /// two different values in one process.
    pub fn open(uri: &str) -> Result<Connection, Error> {
        let inner = Connect::open(Some(uri))?;
        let reported_uri =
            std::env::var("VIRTINST_TEST_FAKE_URI").unwrap_or_else(|_| uri.to_string());
        Ok(Connection {
            inner,
            uri: Uri::parse(&reported_uri),
        })
    }

    /// `open(authcb, cbdata)`: `virConnectOpenAuth` with the credential
    /// types of libvirt's `virConnectCredTypeDefault`, answered by
    /// `authcb`. `None` opens libvirt's default URI, and the URI
    /// reported is then the one libvirt picked.
    pub fn open_auth(
        uri: Option<&str>,
        authcb: virt::connect::ConnectAuthCallback,
    ) -> Result<Connection, Error> {
        let mut auth = virt::connect::ConnectAuth::new(
            vec![
                virt::sys::VIR_CRED_AUTHNAME,
                virt::sys::VIR_CRED_ECHOPROMPT,
                virt::sys::VIR_CRED_REALM,
                virt::sys::VIR_CRED_PASSPHRASE,
                virt::sys::VIR_CRED_NOECHOPROMPT,
                virt::sys::VIR_CRED_EXTERNAL,
            ],
            authcb,
        );
        let inner = Connect::open_auth(uri, &mut auth, 0)?;
        let opened = match uri {
            Some(uri) => uri.to_string(),
            None => inner.get_uri()?,
        };
        let reported_uri = std::env::var("VIRTINST_TEST_FAKE_URI").unwrap_or(opened);
        Ok(Connection {
            inner,
            uri: Uri::parse(&reported_uri),
        })
    }

    /// The URI libvirt reports for the open connection (`getURI`),
    /// never faked.
    pub fn real_uri(&self) -> Result<String, Error> {
        self.inner.get_uri()
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
    pub fn as_connect(&self) -> &Connect {
        &self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TEST_DOMAIN_LOCK;

    /// Real libvirt call, no daemon or real hypervisor needed —
    /// libvirt's `test:///` driver is compiled into `libvirt.so`
    /// itself, so if this crate builds at all (it links against
    /// libvirt-dev unconditionally now), this driver is guaranteed
    /// available. Same URI the `virt` crate's own test suite uses.
    /// Holds `TEST_DOMAIN_LOCK` — asserts `conn.uri().scheme`, which a
    /// concurrently-running `VIRTINST_TEST_FAKE_URI` test below would
    /// otherwise change out from under it (confirmed for real, see
    /// `TEST_DOMAIN_LOCK`'s own doc comment).
    #[test]
    fn open_close_against_the_builtin_test_driver() {
        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        assert_eq!(conn.uri().scheme, "test");
        assert!(conn.is_alive().expect("freshly-opened connection is alive"));
        conn.close().expect("closing a live connection succeeds");
    }

    /// No lock needed: `Connect::open` fails before `open()` ever
    /// touches `VIRTINST_TEST_FAKE_URI` or reads `.uri()` at all, and
    /// this test never inspects either.
    #[test]
    fn open_rejects_a_garbage_uri() {
        let err = Connection::open("not-a-real-libvirt-driver:///whatever");
        assert!(err.is_err());
    }

    /// Ticket 20's port of `MagicURI`'s `fakeuri` effect: the real
    /// connection stays on the always-safe `test://` driver (this
    /// really opens it — a garbage `reported_uri` alone would prove
    /// nothing about whether the *real* open still uses `uri`), but
    /// `is_qemu()`-style scheme checks downstream see the faked
    /// scheme.
    #[test]
    fn open_reports_the_fake_uri_but_still_really_opens_the_real_one() {
        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("VIRTINST_TEST_FAKE_URI", "qemu:///system");

        let conn =
            Connection::open("test:///default").expect("the real open still targets test://");

        std::env::remove_var("VIRTINST_TEST_FAKE_URI");

        assert!(
            conn.uri().is_qemu(),
            "reported URI is the faked qemu:///system"
        );
        assert!(
            conn.is_alive()
                .expect("still a real, live test:// connection"),
            "the real libvirt call used the real uri, not the faked one"
        );
    }

    #[test]
    fn open_without_the_env_var_reports_the_real_uri_unchanged() {
        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("VIRTINST_TEST_FAKE_URI");

        let conn = Connection::open("test:///default").expect("test:/// driver always opens");

        assert_eq!(conn.uri().scheme, "test");
        assert!(!conn.uri().is_qemu());
    }
}
