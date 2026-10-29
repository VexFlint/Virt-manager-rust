//! `vmmDomain.migrate()` from `virtManager/object/domain.py`, with the
//! job helpers the Migrate dialog uses around it: `supports_domain_job_info`,
//! `job_info` (and `testmock.fake_job_info`), `abort_job` and
//! `start_job_progress_thread`.
//!
//! The libvirt calls are blocking and run on the asyncjob's worker;
//! [`MigrateOptions`] holds the dialog's choices and builds the
//! `flags` and `params` exactly as upstream does, so that part is
//! tested without a second host.

use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gtk::glib::subclass::prelude::*;
use virt::connect::Connect;
use virt::domain::{Domain, MigrateParameters};
use virt::sys;
use virtinst_core::progress::Meter as _;

use super::{delete_domain, DomainXml, VmmDomain};
use crate::asyncjob::Meter;

/// The `dest_uri`, `tunnel`, `unsafe`, `temporary` and `xml` arguments
/// of `vmmDomain.migrate`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MigrateOptions {
    /// The migration URI (`tcp:host:port`), or with `tunnel` the
    /// destination's libvirt URI.
    pub dest_uri: Option<String>,
    pub tunnel: bool,
    pub unsafe_migrate: bool,
    pub temporary: bool,
    /// The XML editor's text, for `VIR_MIGRATE_PARAM_DEST_XML`.
    pub xml: Option<String>,
}

/// Which libvirt call `migrate` ends in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MigrateCall {
    /// The test driver with a `TESTSUITE-FAKE` URI: upstream fakes a
    /// successful migration "so we can test more of the migration
    /// wizard" — define and start on the destination, delete here.
    TestsuiteFake,
    /// `migrateToURI3(dest_uri, params, flags)`.
    ToUri3,
    /// `migrate3(destconn, params, flags)`.
    Migrate3,
}

impl MigrateOptions {
    /// `flags`: always live; a permanent move persists the VM on the
    /// destination and undefines it here; tunnelled is peer to peer.
    pub fn flags(&self) -> u32 {
        let mut flags = sys::VIR_MIGRATE_LIVE;
        if !self.temporary {
            flags |= sys::VIR_MIGRATE_PERSIST_DEST;
            flags |= sys::VIR_MIGRATE_UNDEFINE_SOURCE;
        }
        if self.tunnel {
            flags |= sys::VIR_MIGRATE_PEER2PEER;
            flags |= sys::VIR_MIGRATE_TUNNELLED;
        }
        if self.unsafe_migrate {
            flags |= sys::VIR_MIGRATE_UNSAFE;
        }
        flags
    }

    /// `params`: the migration URI (not when tunnelled, where it is the
    /// call's own argument) and the edited XML.
    pub fn params(&self) -> MigrateParameters {
        let mut params = MigrateParameters::default();
        if !self.tunnel {
            params.uri = self.dest_uri.clone().filter(|u| !u.is_empty());
        }
        params.dest_xml = self.xml.clone().filter(|x| !x.is_empty());
        params
    }

    pub fn call(&self, is_test: bool) -> MigrateCall {
        if is_test
            && self
                .dest_uri
                .as_deref()
                .unwrap_or_default()
                .contains("TESTSUITE-FAKE")
        {
            MigrateCall::TestsuiteFake
        } else if self.tunnel {
            MigrateCall::ToUri3
        } else {
            MigrateCall::Migrate3
        }
    }
}

/// `supports_domain_job_info`: the test driver says yes "but this
/// tests more code"; elsewhere `jobInfo()` must not be unsupported.
pub fn supports_domain_job_info(is_test: bool, dom: &Domain) -> bool {
    if is_test {
        return true;
    }
    match dom.get_job_info() {
        Ok(_) => true,
        Err(e) => !matches!(
            e.code(),
            virt::error::ErrorNumber::NoSupport | virt::error::ErrorNumber::Rpc
        ),
    }
}

/// `testmock.fake_job_info`: 1 GiB with a random part left, as
/// `(data_total, data_remaining)`.
fn fake_job_info() -> (u64, u64) {
    let total: u64 = 1024 * 1024 * 1024;
    let random = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    let fakepcent = 1 + random % 99;
    (total, total / 100 * fakepcent)
}

/// `job_info`: `jobinfo[3]` and `jobinfo[5]`, the data total and what
/// is left of it.
pub fn job_info(is_test: bool, dom: &Domain) -> Result<(u64, u64), String> {
    if is_test {
        return Ok(fake_job_info());
    }
    let info = dom.get_job_info().map_err(|e| e.message().to_string())?;
    Ok((
        info.data_total.unwrap_or(0),
        info.data_remaining.unwrap_or(0),
    ))
}

/// `abort_job`.
pub fn abort_job(dom: &Domain) -> Result<(), String> {
    // SAFETY: `dom` is a live domain handle.
    if unsafe { sys::virDomainAbortJob(dom.as_ptr()) } < 0 {
        return Err(virt::error::Error::last_error().message().to_string());
    }
    Ok(())
}

/// Stops the `start_job_progress_thread` poller when dropped: upstream's
/// thread ends once the job thread it watches is gone.
struct ProgressThread(Arc<AtomicBool>);

impl Drop for ProgressThread {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// `start_job_progress_thread`: every half second, feed `job_info`
/// into the meter, starting it once libvirt knows the total.
fn start_job_progress_thread(
    is_test: bool,
    dom: &Domain,
    mut meter: Meter,
    progtext: &str,
) -> Option<ProgressThread> {
    if !supports_domain_job_info(is_test, dom) {
        return None;
    }
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let dom = dom.clone();
    let progtext = progtext.to_string();
    let spawned = std::thread::Builder::new()
        .name("job progress reporting".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_millis(500));
            if thread_stop.load(Ordering::Relaxed) {
                return;
            }
            let (data_total, data_remaining) = match job_info(is_test, &dom) {
                Ok(info) => info,
                Err(e) => {
                    eprintln!("Error calling jobinfo: {e}");
                    return;
                }
            };
            // data_total is 0 if the job hasn't started yet
            if data_total == 0 {
                continue;
            }
            if !meter.is_started() {
                meter.start(&progtext, Some(data_total));
            }
            meter.update(data_total.saturating_sub(data_remaining));
        });
    spawned.ok().map(|_| ProgressThread(stop))
}

/// `has_nvram`, from the domain's XML.
fn has_nvram(dom: &Domain) -> bool {
    let Some(x) = dom
        .get_xml_desc(0)
        .ok()
        .and_then(|xml| DomainXml::parse(xml).ok())
    else {
        return false;
    };
    let os = &x.guest.os;
    (os.loader.is_some() && os.loader_type.as_deref() == Some("pflash"))
        || os.firmware.as_deref() == Some("efi")
        || os.nvram.is_some()
}

/// `vmmDomain.migrate`, blocking. `meter` gets the job's progress.
pub fn migrate(
    is_test: bool,
    dom: &Domain,
    destconn: &Connect,
    opts: &MigrateOptions,
    meter: Option<Meter>,
) -> Result<(), String> {
    let flags = opts.flags();
    gtk::glib::g_debug!(
        "virt-manager",
        "Migrating: conn={:?} flags={flags} uri={:?} tunnel={} unsafe={} temporary={}",
        destconn.get_uri().unwrap_or_default(),
        opts.dest_uri,
        opts.tunnel,
        opts.unsafe_migrate,
        opts.temporary,
    );

    let _progress =
        meter.and_then(|m| start_job_progress_thread(is_test, dom, m, "Migrating domain"));

    let params = opts.params();
    let err = |e: virt::error::Error| e.message().to_string();
    match opts.call(is_test) {
        MigrateCall::TestsuiteFake => {
            std::thread::sleep(Duration::from_millis(1200));
            let xml = match opts.xml.clone().filter(|x| !x.is_empty()) {
                Some(xml) => xml,
                // `get_xml_to_define`
                None => dom
                    .get_xml_desc(sys::VIR_DOMAIN_XML_INACTIVE | sys::VIR_DOMAIN_XML_SECURE)
                    .map_err(err)?,
            };
            Domain::define_xml(destconn, &xml)
                .map_err(err)?
                .create()
                .map_err(err)?;
            delete_domain(dom, has_nvram(dom))?;
        }
        MigrateCall::ToUri3 => {
            dom.migrate_to_uri3(opts.dest_uri.as_deref(), params, flags)
                .map_err(err)?;
        }
        MigrateCall::Migrate3 => {
            dom.migrate3(destconn, params, flags).map_err(err)?;
        }
    }
    // Don't schedule any conn update, migrate dialog handles it for us
    Ok(())
}

impl VmmDomain {
    /// `supports_domain_job_info`, on a worker.
    pub async fn supports_domain_job_info(&self) -> bool {
        let is_test = self.conn().is_some_and(|c| c.is_test());
        self.with_domain(move |_, dom| Ok(supports_domain_job_info(is_test, dom)))
            .await
            .unwrap_or(false)
    }

    /// `migrate`'s `self._install_abort = True`: the VM is leaving (or
    /// only moving temporarily), so a finished install mustn't restart
    /// it here. Set on this object, which the Create VM wizard watches.
    pub fn set_install_abort(&self) {
        self.imp().install_abort.set(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(tunnel: bool, unsafe_migrate: bool, temporary: bool) -> MigrateOptions {
        MigrateOptions {
            dest_uri: Some("tcp:dest.example.com:49152".into()),
            tunnel,
            unsafe_migrate,
            temporary,
            xml: None,
        }
    }

    #[test]
    fn flags_follow_upstream() {
        let persist = sys::VIR_MIGRATE_PERSIST_DEST | sys::VIR_MIGRATE_UNDEFINE_SOURCE;
        let tunnelled = sys::VIR_MIGRATE_PEER2PEER | sys::VIR_MIGRATE_TUNNELLED;
        // The dialog's defaults: a live, permanent, direct move.
        assert_eq!(
            opts(false, false, false).flags(),
            sys::VIR_MIGRATE_LIVE | persist
        );
        // 'Temporary move' keeps the source config, persists nothing.
        assert_eq!(opts(false, false, true).flags(), sys::VIR_MIGRATE_LIVE);
        assert_eq!(
            opts(true, false, false).flags(),
            sys::VIR_MIGRATE_LIVE | persist | tunnelled
        );
        assert_eq!(
            opts(true, true, true).flags(),
            sys::VIR_MIGRATE_LIVE | tunnelled | sys::VIR_MIGRATE_UNSAFE
        );
        assert_eq!(
            opts(false, true, false).flags(),
            sys::VIR_MIGRATE_LIVE | persist | sys::VIR_MIGRATE_UNSAFE
        );
    }

    #[test]
    fn params_follow_upstream() {
        let direct = opts(false, false, false).params();
        assert_eq!(direct.uri.as_deref(), Some("tcp:dest.example.com:49152"));
        assert_eq!(direct.dest_xml, None);

        // Tunnelled: the URI is migrateToURI3's own argument.
        let mut tunnel = opts(true, false, false);
        tunnel.dest_uri = Some("qemu+ssh://dest/system".into());
        tunnel.xml = Some("<domain/>".into());
        let params = tunnel.params();
        assert_eq!(params.uri, None);
        assert_eq!(params.dest_xml.as_deref(), Some("<domain/>"));

        // 'Let libvirt decide' leaves the URI out; so does an empty one.
        let mut auto = opts(false, false, false);
        auto.dest_uri = None;
        assert_eq!(auto.params().uri, None);
        auto.dest_uri = Some(String::new());
        auto.xml = Some(String::new());
        let params = auto.params();
        assert_eq!((params.uri, params.dest_xml), (None, None));
    }

    #[test]
    fn call_follows_upstream() {
        let mut o = opts(false, false, false);
        assert_eq!(o.call(false), MigrateCall::Migrate3);
        o.tunnel = true;
        assert_eq!(o.call(false), MigrateCall::ToUri3);
        o.dest_uri = Some("tcp:TESTSUITE-FAKE".into());
        assert_eq!(o.call(false), MigrateCall::ToUri3);
        assert_eq!(o.call(true), MigrateCall::TestsuiteFake);
        o.tunnel = false;
        assert_eq!(o.call(true), MigrateCall::TestsuiteFake);
        o.dest_uri = None;
        assert_eq!(o.call(true), MigrateCall::Migrate3);
    }

    #[test]
    fn fake_job_info_is_a_partial_gigabyte() {
        for _ in 0..50 {
            let (total, remaining) = fake_job_info();
            assert_eq!(total, 1024 * 1024 * 1024);
            assert!(remaining > 0 && remaining < total, "{remaining}");
        }
    }

    /// A private test driver instance per file, so this doesn't touch
    /// the shared `test:///default` other tests use.
    fn test_conn(name: &str, domain: &str) -> (Connect, std::path::PathBuf) {
        let path =
            std::env::temp_dir().join(format!("vmm-migrate-{name}-{}.xml", std::process::id()));
        std::fs::write(&path, format!("<node>{domain}</node>")).unwrap();
        let conn = Connect::open(Some(&format!("test://{}", path.display()))).unwrap();
        (conn, path)
    }

    #[test]
    fn migrate_against_the_test_driver() {
        let domain = "<domain type='test'><name>vmm-mig</name>\
                      <uuid>2f6e1f6c-5c7e-4a39-9d5b-6b1f2a3c4d5e</uuid>\
                      <memory>65536</memory><os><type>hvm</type></os></domain>";
        let (mut src, src_path) = test_conn("src", domain);
        let (mut dst, dst_path) = test_conn("dst", "");
        let dom = Domain::lookup_by_name(&src, "vmm-mig").unwrap();
        assert!(dom.is_active().unwrap());
        assert!(supports_domain_job_info(true, &dom));
        // abortJob isn't implemented by the test driver, so the dialog's
        // cancel shows "Error cancelling migrate job".
        assert!(abort_job(&dom).is_err());

        // The real calls: the test driver can't migrate.
        let direct = opts(false, false, false);
        assert!(migrate(true, &dom, &dst, &direct, None).is_err());
        let mut tunnel = opts(true, false, false);
        tunnel.dest_uri = Some(dst.get_uri().unwrap());
        assert!(migrate(true, &dom, &dst, &tunnel, None).is_err());

        // TESTSUITE-FAKE moves it, under the edited XML's name.
        let mut fake = opts(false, false, false);
        fake.dest_uri = Some("tcp:TESTSUITE-FAKE:49152".into());
        fake.xml = Some(domain.replace("vmm-mig", "vmm-mig-new"));
        migrate(true, &dom, &dst, &fake, None).unwrap();
        let moved = Domain::lookup_by_name(&dst, "vmm-mig-new").unwrap();
        assert!(moved.is_active().unwrap() && moved.is_persistent().unwrap());
        assert!(!dom.is_persistent().unwrap());

        drop((dom, moved));
        src.close().unwrap();
        dst.close().unwrap();
        let _ = std::fs::remove_file(src_path);
        let _ = std::fs::remove_file(dst_path);
    }
}
