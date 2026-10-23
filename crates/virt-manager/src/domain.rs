//! Upstream's `virtManager/object/domain.py` (`vmmDomain`), the slice the
//! manager window needs: identity, run state, and the lifecycle actions
//! from `vmmenu.py`. XML-derived detail (devices, stats vectors, …)
//! comes with the details window and stats ports.
//!
//! A `VmmDomain` holds the last [`DomainSnapshot`] its connection's
//! tick polled, and emits `state-changed` when a new poll differs —
//! upstream's signal name, so later windows connect the same way.
//! Lifecycle actions look the domain up by UUID on a worker thread
//! (ticket 09) and ask the connection for a priority tick afterwards,
//! same as upstream's post-action `self._update_status()`.

use std::cell::{Cell, Ref, RefCell};
use std::collections::HashSet;
use std::sync::OnceLock;

use gtk::gio;
use gtk::glib;
use gtk::glib::prelude::*;
use gtk::glib::subclass::prelude::*;
use gtk::glib::subclass::Signal;
use virt::connect::Connect;
use virt::domain::Domain;
use virt::sys;

use crate::connection::VmmConnection;
use crate::libvirtenummap::{self as enummap, DomainState};
use crate::statsmanager::{self, PollFlags, RawSample, StatsList};

/// One poll's worth of what the UI shows about a domain.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DomainSnapshot {
    pub uuid: String,
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub id: Option<u32>,
    pub state: DomainState,
    pub has_managed_save: bool,
}

impl DomainSnapshot {
    /// Blocking: reads one domain's state. Runs on the tick thread.
    pub fn fetch(dom: &Domain) -> Result<DomainSnapshot, virt::error::Error> {
        let metadata = |kind| {
            dom.get_metadata(kind as i32, None, 0)
                .ok()
                .filter(|s| !s.is_empty())
        };
        let (state, _reason) = dom.get_state()?;
        Ok(DomainSnapshot {
            uuid: dom.get_uuid_string()?,
            name: dom.get_name()?,
            title: metadata(sys::VIR_DOMAIN_METADATA_TITLE),
            description: metadata(sys::VIR_DOMAIN_METADATA_DESCRIPTION),
            id: dom.get_id(),
            state,
            // Not every driver implements managed save; upstream gates
            // this on `managedsave_supported` and treats it as False.
            has_managed_save: dom.has_managed_save(0).unwrap_or(false),
        })
    }

    /// Blocking: every domain on the connection, active and inactive.
    pub fn fetch_all(conn: &Connect) -> Result<Vec<DomainSnapshot>, virt::error::Error> {
        conn.list_all_domains(0)?
            .iter()
            .map(DomainSnapshot::fetch)
            .collect()
    }

    /// Blocking: one tick's poll — every domain's snapshot plus its
    /// stats sample. `mem_period_set` lists the UUIDs whose memory
    /// stats period was already set.
    pub fn fetch_all_with_stats(
        conn: &Connect,
        flags: PollFlags,
        is_lxc: bool,
        mem_period_set: &HashSet<String>,
    ) -> Result<Vec<(DomainSnapshot, RawSample)>, virt::error::Error> {
        conn.list_all_domains(0)?
            .iter()
            .map(|dom| {
                let snap = DomainSnapshot::fetch(dom)?;
                let set_period = !mem_period_set.contains(&snap.uuid);
                let raw = statsmanager::sample(dom, flags, is_lxc, set_period);
                Ok((snap, raw))
            })
            .collect()
    }
}

/// The lifecycle calls `VMActionUI` makes, as data so they can cross to
/// the worker thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomainAction {
    Startup,
    Suspend,
    Resume,
    Shutdown,
    Reboot,
    Reset,
    Destroy,
    Save,
    /// `vm.remove_saved_image()`: drop a managed-save image.
    RemoveSavedImage,
}

impl DomainAction {
    fn run(self, dom: &Domain) -> Result<(), virt::error::Error> {
        match self {
            DomainAction::Startup => dom.create().map(drop),
            DomainAction::Suspend => dom.suspend().map(drop),
            DomainAction::Resume => dom.resume().map(drop),
            DomainAction::Shutdown => dom.shutdown().map(drop),
            DomainAction::Reboot => dom.reboot(0),
            DomainAction::Reset => dom.reset().map(drop),
            DomainAction::Destroy => dom.destroy(),
            DomainAction::Save => dom.managed_save(0).map(drop),
            DomainAction::RemoveSavedImage => dom.managed_save_remove(0).map(drop),
        }
    }
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct VmmDomain {
        pub snapshot: RefCell<DomainSnapshot>,
        pub conn: glib::WeakRef<VmmConnection>,
        pub stats: RefCell<StatsList>,
        pub mem_period_set: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for VmmDomain {
        const NAME: &'static str = "VmmDomain";
        type Type = super::VmmDomain;
    }

    impl ObjectImpl for VmmDomain {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    Signal::builder("state-changed").build(),
                    Signal::builder("resources-sampled").build(),
                ]
            })
        }
    }
}

glib::wrapper! {
    pub struct VmmDomain(ObjectSubclass<imp::VmmDomain>);
}

impl VmmDomain {
    pub fn new(conn: &VmmConnection, snapshot: DomainSnapshot) -> VmmDomain {
        let obj: VmmDomain = glib::Object::new();
        obj.imp().conn.set(Some(conn));
        obj.imp().snapshot.replace(snapshot);
        obj
    }

    /// Applies a fresh poll; emits `state-changed` only on a real change.
    pub fn update(&self, snapshot: DomainSnapshot) {
        if *self.imp().snapshot.borrow() == snapshot {
            return;
        }
        self.imp().snapshot.replace(snapshot);
        self.emit_by_name::<()>("state-changed", &[]);
    }

    /// `statsmanager.refresh_vm_stats` with this tick's raw sample, then
    /// upstream's `resources-sampled` signal.
    pub fn apply_sample(&self, raw: &RawSample, flags: PollFlags, hostcpus: u32) {
        self.imp()
            .stats
            .borrow_mut()
            .append_sample(raw, flags, hostcpus);
        self.imp().mem_period_set.set(
            flags.mem && !matches!(raw.state, sys::VIR_DOMAIN_SHUTOFF | sys::VIR_DOMAIN_CRASHED),
        );
        self.emit_by_name::<()>("resources-sampled", &[]);
    }

    pub fn mem_period_set(&self) -> bool {
        self.imp().mem_period_set.get()
    }

    pub fn stats(&self) -> Ref<'_, StatsList> {
        self.imp().stats.borrow()
    }

    pub fn conn(&self) -> Option<VmmConnection> {
        self.imp().conn.upgrade()
    }

    fn snap<T>(&self, f: impl FnOnce(&DomainSnapshot) -> T) -> T {
        f(&self.imp().snapshot.borrow())
    }

    pub fn get_uuid(&self) -> String {
        self.snap(|s| s.uuid.clone())
    }
    pub fn get_name(&self) -> String {
        self.snap(|s| s.name.clone())
    }
    pub fn get_id(&self) -> Option<u32> {
        self.snap(|s| s.id)
    }
    pub fn get_name_or_title(&self) -> String {
        self.snap(|s| s.title.clone().unwrap_or_else(|| s.name.clone()))
    }
    pub fn get_description(&self) -> String {
        self.snap(|s| s.description.clone().unwrap_or_default())
    }
    pub fn status(&self) -> DomainState {
        self.snap(|s| s.state)
    }
    pub fn has_managed_save(&self) -> bool {
        self.snap(|s| s.has_managed_save)
    }

    pub fn run_status(&self) -> &'static str {
        enummap::pretty_run_status(self.status(), self.has_managed_save())
    }
    pub fn run_status_icon_name(&self) -> &'static str {
        enummap::run_status_icon_name(self.status())
    }
    pub fn is_active(&self) -> bool {
        enummap::is_active(self.status())
    }
    pub fn is_runable(&self) -> bool {
        enummap::is_runable(self.status())
    }
    pub fn is_paused(&self) -> bool {
        enummap::is_paused(self.status())
    }
    pub fn is_pauseable(&self) -> bool {
        enummap::is_pauseable(self.status())
    }
    pub fn is_unpauseable(&self) -> bool {
        enummap::is_unpauseable(self.status())
    }
    pub fn is_stoppable(&self) -> bool {
        enummap::is_stoppable(self.status())
    }
    pub fn is_destroyable(&self) -> bool {
        enummap::is_destroyable(self.status())
    }
    pub fn is_cloneable(&self) -> bool {
        enummap::is_cloneable(self.status())
    }

    /// Runs `action` on a worker thread, then schedules a priority tick
    /// so the list reflects the new state without waiting a full
    /// interval. The error string is libvirt's message, for
    /// `asyncjob::simple_async_noshow`'s dialog.
    pub async fn run_action(&self, action: DomainAction) -> Result<(), String> {
        let conn = self.conn().ok_or("Connection closed")?;
        let handle = conn.handle().ok_or("Not connected")?;
        let uuid = self.get_uuid();
        let res = gio::spawn_blocking(move || {
            let dom = Domain::lookup_by_uuid_string(handle.connect(), &uuid)?;
            action.run(&dom)
        })
        .await
        .map_err(|_| "Worker thread panicked".to_string())?;
        conn.tick();
        res.map_err(|e| e.message().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `test:///default` is one shared in-process driver instance, so
    /// tests that change its domains' state must not overlap.
    static TEST_DRIVER: Mutex<()> = Mutex::new(());

    /// libvirt's built-in `test:///default` driver ships one running
    /// domain called `test`.
    #[test]
    fn fetch_all_against_the_builtin_test_driver() {
        let _guard = TEST_DRIVER.lock().unwrap_or_else(|e| e.into_inner());
        let mut conn = Connect::open(Some("test:///default")).expect("test driver opens");
        let snaps = DomainSnapshot::fetch_all(&conn).expect("listing works");
        let test = snaps
            .iter()
            .find(|s| s.name == "test")
            .expect("default test domain");
        assert_eq!(test.state, sys::VIR_DOMAIN_RUNNING);
        assert!(test.id.is_some());
        assert_eq!(test.uuid.len(), 36);
        conn.close().unwrap();
    }

    #[test]
    fn actions_change_state_on_the_test_driver() {
        let _guard = TEST_DRIVER.lock().unwrap_or_else(|e| e.into_inner());
        let mut conn = Connect::open(Some("test:///default")).expect("test driver opens");
        let dom = Domain::lookup_by_name(&conn, "test").unwrap();
        DomainAction::Suspend.run(&dom).unwrap();
        assert_eq!(dom.get_state().unwrap().0, sys::VIR_DOMAIN_PAUSED);
        DomainAction::Resume.run(&dom).unwrap();
        DomainAction::Destroy.run(&dom).unwrap();
        assert_eq!(dom.get_state().unwrap().0, sys::VIR_DOMAIN_SHUTOFF);
        DomainAction::Startup.run(&dom).unwrap();
        assert_eq!(dom.get_state().unwrap().0, sys::VIR_DOMAIN_RUNNING);
        drop(dom);
        conn.close().unwrap();
    }
}
