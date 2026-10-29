//! Upstream's `virtManager/connection.py` (`vmmConnection`), the slice the
//! manager window needs: connect/disconnect state, the pretty labels,
//! and the domain list the engine's tick keeps current.
//!
//! Signals, named as upstream's: `state-changed`, `vm-added(VmmDomain)`,
//! `vm-removed(VmmDomain)`, `resources-sampled` (after each tick's
//! `_recalculate_stats`, for the Connection Details graphs). Network
//! and pool libvirt events come out as `net-event(name)` and
//! `pool-event(name)`.
//!
//! Connections open through `openAuth` with the credentials dialog
//! (`connectauth`), as upstream's `_do_open`.
//!
//! **Not ported yet**: nodedev events, and network/pool/nodedev objects
//! (their lists are fetched when a window needs them, and again on their
//! events).

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use gtk::gio;
use gtk::glib;
use gtk::glib::prelude::*;
use gtk::glib::subclass::prelude::*;
use gtk::glib::subclass::Signal;
use virt::connect::Connect;
use virt::sys;
use virtinst_core::capabilities::caps::Capabilities;
use virtinst_core::connection::Uri;

use crate::connectauth;
use crate::connevents::{self, DomainEvent, ObjectEvent};
use crate::domain::{DomainSnapshot, VmmDomain};
use crate::engine;
use crate::statsmanager::{PollFlags, RawSample};

/// Owns one libvirt connection reference and closes it on drop — the
/// `virt` crate's `Connect` has no `Drop` of its own. Shared via `Arc`
/// so a worker thread mid-call keeps the connection alive past a
/// user-initiated disconnect.
#[derive(Debug)]
/// The open connection plus its event callback ids.
pub struct ConnHandle(Connect, connevents::EventIds);

impl ConnHandle {
    pub fn connect(&self) -> &Connect {
        &self.0
    }
}

impl Drop for ConnHandle {
    fn drop(&mut self) {
        connevents::deregister(&self.0, &self.1);
        let _ = self.0.close();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ConnState {
    #[default]
    Disconnected,
    Connecting,
    Active,
}

/// Upstream's `ConnectError` namedtuple: what the error dialog shows.
#[derive(Clone, Debug)]
pub struct ConnectError {
    pub msg: String,
    pub details: String,
    pub title: String,
}

/// `vmmConnection.get_pretty_desc()` without the config `pretty-name`
/// override.
pub fn pretty_desc(uri: &str) -> String {
    let parsed = Uri::parse(uri);
    let driver = parsed.scheme.as_str();
    let hv = match driver {
        "esx" => "ESX",
        "lxc" => "LXC",
        "openvz" => "OpenVZ",
        "qemu" => "QEMU/KVM",
        "vbox" => "Virtualbox",
        "vmware" => "VMWare",
        "xen" => "Xen",
        other => other,
    };
    let mut ret = hv.to_string();
    let path = parsed.path.as_str();
    let basename = path.rsplit('/').next().unwrap_or("");
    if path == "/session" {
        ret.push_str(" User session");
    } else if !path.is_empty() && path != "/system" && !basename.is_empty() {
        // Used by test URIs to report what XML file they are using
        ret.push_str(&format!(" {basename}"));
    }
    if !parsed.hostname.is_empty() {
        ret.push_str(&format!(": {}", parsed.hostname));
    }
    ret
}

/// One `_recalculate_stats` record (`newStats`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConnStatsRecord {
    pub timestamp: f64,
    /// KiB, summed over the running VMs.
    pub memory: u64,
    pub memory_percent: f64,
    pub cpu_time: u64,
    pub cpu_host_percent: f64,
    pub disk_rd_rate: f64,
    pub disk_wr_rate: f64,
    pub net_rx_rate: f64,
    pub net_tx_rate: f64,
    pub disk_max_rate: f64,
    pub net_max_rate: f64,
}

/// What `_recalculate_stats` reads off one running VM: `cpu_time()`,
/// `stats_memory()`, the four rates, and the two max rates.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VmStatsSample {
    pub cpu_time: u64,
    pub memory: u64,
    pub disk_rd_rate: f64,
    pub disk_wr_rate: f64,
    pub net_rx_rate: f64,
    pub net_tx_rate: f64,
    pub disk_max_rate: f64,
    pub net_max_rate: f64,
}

/// `vmmConnection._stats`: newest record first.
#[derive(Debug, Default)]
pub struct ConnStats {
    stats: std::collections::VecDeque<ConnStatsRecord>,
}

impl ConnStats {
    fn latest(&self) -> ConnStatsRecord {
        self.stats.front().cloned().unwrap_or_default()
    }

    /// `_recalculate_stats`, for the running VMs' samples at `now`.
    /// `host_memory` is in KiB.
    pub fn recalculate(
        &mut self,
        now: f64,
        vms: &[VmStatsSample],
        host_memory: u64,
        host_cpus: u32,
    ) {
        let expected = crate::statsmanager::STATS_HISTORY_LENGTH;
        self.stats.truncate(expected);

        let prev = self.latest();
        let mut mem = 0;
        let mut cpu_time = 0;
        let (mut rd, mut wr, mut rx, mut tx) = (0.0, 0.0, 0.0, 0.0);
        let or10 = |v: f64| if v == 0.0 { 10.0 } else { v };
        let mut disk_max = or10(prev.disk_max_rate);
        let mut net_max = or10(prev.net_max_rate);
        for vm in vms {
            cpu_time += vm.cpu_time;
            mem += vm.memory;
            rd += vm.disk_rd_rate;
            wr += vm.disk_wr_rate;
            rx += vm.net_rx_rate;
            tx += vm.net_tx_rate;
            net_max = net_max.max(vm.net_max_rate);
            disk_max = disk_max.max(vm.disk_max_rate);
        }

        let mut pcent_host_cpu = 0.0;
        let pcent_mem = if host_memory > 0 {
            mem as f64 * 100.0 / host_memory as f64
        } else {
            0.0
        };
        if !self.stats.is_empty() && host_cpus > 0 {
            let elapsed = now - prev.timestamp;
            if elapsed > 0.0 {
                pcent_host_cpu = cpu_time as f64 * 100.0 / (elapsed * 1e9 * host_cpus as f64);
            }
        }

        self.stats.push_front(ConnStatsRecord {
            timestamp: now,
            memory: mem,
            memory_percent: pcent_mem.clamp(0.0, 100.0),
            cpu_time,
            cpu_host_percent: pcent_host_cpu.clamp(0.0, 100.0),
            disk_rd_rate: rd,
            disk_wr_rate: wr,
            net_rx_rate: rx,
            net_tx_rate: tx,
            disk_max_rate: disk_max,
            net_max_rate: net_max,
        });
    }

    /// `_vector_helper(record_name, limit=None)`: history length + 1
    /// values, newest first, divided by 100 and zero-padded.
    pub fn vector(&self, f: fn(&ConnStatsRecord) -> f64) -> Vec<f64> {
        (0..crate::statsmanager::STATS_HISTORY_LENGTH + 1)
            .map(|i| self.stats.get(i).map(|r| f(r) / 100.0).unwrap_or(0.0))
            .collect()
    }
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct VmmConnection {
        pub uri: RefCell<String>,
        pub state: Cell<ConnState>,
        pub handle: RefCell<Option<Arc<ConnHandle>>>,
        pub vms: RefCell<Vec<VmmDomain>>,
        pub ticking: Cell<bool>,
        pub hostcpus: Cell<u32>,
        /// `_hostinfo[1]`, in KiB.
        pub hostmem: Cell<u64>,
        /// `caps.host.cpu.model`.
        pub host_cpu_model: RefCell<Option<String>>,
        /// `_stats`: the host-wide records `_recalculate_stats` keeps.
        pub stats: RefCell<ConnStats>,
        /// The `pretty-name` settings handler, once connected.
        pub pretty_name_watch: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for VmmConnection {
        const NAME: &'static str = "VmmConnection";
        type Type = super::VmmConnection;
    }

    impl ObjectImpl for VmmConnection {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    Signal::builder("state-changed").build(),
                    Signal::builder("vm-added")
                        .param_types([VmmDomain::static_type()])
                        .build(),
                    Signal::builder("vm-removed")
                        .param_types([VmmDomain::static_type()])
                        .build(),
                    Signal::builder("resources-sampled").build(),
                    Signal::builder("net-event")
                        .param_types([String::static_type()])
                        .build(),
                    Signal::builder("pool-event")
                        .param_types([String::static_type()])
                        .build(),
                ]
            })
        }
    }
}

/// A virtual network, reduced to what the NIC source list shows
/// (`vmmNetwork` plus its `xmlobj`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetInfo {
    pub name: String,
    pub active: bool,
    pub forward_mode: Option<String>,
    pub forward_dev: Option<String>,
    pub virtualport_type: Option<String>,
    /// `(name, default)`.
    pub portgroups: Vec<(String, bool)>,
}

impl NetInfo {
    /// Parses `<network>` XML.
    pub fn parse(name: String, active: bool, xml: &str) -> NetInfo {
        let mut info = NetInfo {
            name,
            active,
            ..Default::default()
        };
        let Ok(doc) = virtinst_xml::parse_libvirt_xml(xml) else {
            return info;
        };
        let Some(root) = doc.root_element() else {
            return info;
        };
        let attr = |path: &[&str], name: &str| {
            virtinst_xml::resolve_path(&doc, root, path)
                .and_then(|e| e.attribute(&doc, name))
                .map(str::to_string)
        };
        info.forward_mode = attr(&["forward"], "mode");
        info.forward_dev = attr(&["forward"], "dev");
        info.virtualport_type = attr(&["virtualport"], "type");
        info.portgroups = root
            .find_all(&doc, "portgroup")
            .into_iter()
            .filter_map(|pg| {
                let name = pg.attribute(&doc, "name")?.to_string();
                Some((name, pg.attribute(&doc, "default") == Some("yes")))
            })
            .collect();
        info
    }

    /// `vmmNetwork.pretty_forward_mode`.
    pub fn pretty_forward_mode(&self) -> String {
        let dev = self.forward_dev.as_deref();
        match (self.forward_mode.as_deref(), dev) {
            (None, _) => "Isolated network".into(),
            (Some("nat"), Some(dev)) => format!("NAT to {dev}"),
            (Some("nat"), None) => "NAT".into(),
            (Some("route"), Some(dev)) => format!("Route to {dev}"),
            (Some("route"), None) => "Routed network".into(),
            (Some(mode), _) => {
                let mut c = mode.chars();
                let cap: String = c
                    .next()
                    .map(|f| f.to_uppercase().chain(c).collect())
                    .unwrap_or_default();
                format!("{cap} network")
            }
        }
    }
}

glib::wrapper! {
    pub struct VmmConnection(ObjectSubclass<imp::VmmConnection>);
}

impl VmmConnection {
    pub fn new(uri: &str) -> VmmConnection {
        let obj: VmmConnection = glib::Object::new();
        obj.imp().uri.replace(uri.to_string());
        obj
    }

    pub fn get_uri(&self) -> String {
        self.imp().uri.borrow().clone()
    }

    pub fn state(&self) -> ConnState {
        self.imp().state.get()
    }

    pub fn is_disconnected(&self) -> bool {
        self.state() == ConnState::Disconnected
    }
    pub fn is_connecting(&self) -> bool {
        self.state() == ConnState::Connecting
    }
    pub fn is_active(&self) -> bool {
        self.state() == ConnState::Active
    }

    pub fn get_state_text(&self) -> &'static str {
        match self.state() {
            ConnState::Disconnected => "Disconnected",
            ConnState::Connecting => "Connecting",
            ConnState::Active => "Active",
        }
    }

    /// `host_active_processor_count`.
    pub fn host_active_processor_count(&self) -> u32 {
        self.imp().hostcpus.get()
    }

    /// `host_memory_size`, in KiB.
    pub fn host_memory_size(&self) -> u64 {
        self.imp().hostmem.get()
    }

    /// `caps.host.cpu.model`.
    pub fn host_cpu_model(&self) -> Option<String> {
        self.imp().host_cpu_model.borrow().clone()
    }

    pub fn handle(&self) -> Option<Arc<ConnHandle>> {
        self.imp().handle.borrow().clone()
    }

    /// `list_nets`, fetched on a worker.
    pub async fn list_nets(&self) -> Vec<NetInfo> {
        let Some(handle) = self.handle() else {
            return vec![];
        };
        gio::spawn_blocking(move || {
            let nets = handle.connect().list_all_networks(0).unwrap_or_default();
            nets.iter()
                .filter_map(|n| {
                    let name = n.get_name().ok()?;
                    let active = n.is_active().unwrap_or(false);
                    let xml = n.get_xml_desc(0).ok()?;
                    Some(NetInfo::parse(name, active, &xml))
                })
                .collect()
        })
        .await
        .unwrap_or_default()
    }

    /// The host's node devices, for the graphics rendernode list and
    /// hostdev names (`filter_nodedevs`/`list_nodedevs`).
    pub async fn list_nodedevs(&self) -> Vec<virtinst_core::nodedev::NodeDevice> {
        self.list_nodedevs_with_state()
            .await
            .into_iter()
            .map(|(d, _)| d)
            .collect()
    }

    /// [`Self::list_nodedevs`] with each device's `is_active()`: a
    /// libvirt without `virNodeDeviceIsActive` counts all as active.
    pub async fn list_nodedevs_with_state(
        &self,
    ) -> Vec<(virtinst_core::nodedev::NodeDevice, bool)> {
        let Some(handle) = self.handle() else {
            return vec![];
        };
        gio::spawn_blocking(move || {
            let devs = handle
                .connect()
                .list_all_node_devices(0)
                .unwrap_or_default();
            devs.iter()
                .filter_map(|d| {
                    let xml = d.get_xml_desc(0).ok()?;
                    let dev = virtinst_core::nodedev::NodeDevice::parse(&xml)?;
                    // SAFETY: `d` is a live node device handle.
                    let active = unsafe { virt::sys::virNodeDeviceIsActive(d.as_ptr()) } != 0;
                    Some((dev, active))
                })
                .collect()
        })
        .await
        .unwrap_or_default()
    }

    /// `_host_disk_space`: the default pool's free bytes, refreshed
    /// first; `None` when there's no active default pool.
    pub async fn default_pool_available(&self) -> Option<u64> {
        let handle = self.handle()?;
        let session = self.is_qemu_unprivileged();
        gio::spawn_blocking(move || {
            let pool = virtinst_core::storage::lookup_default_pool(handle.connect(), session)?;
            if !pool.pool.is_active().ok()? {
                return None;
            }
            let _ = pool.pool.refresh(0);
            pool.pool.get_info().ok().map(|i| i.available)
        })
        .await
        .ok()
        .flatten()
    }

    /// `get_default_storage_format`: the preference, with `default`
    /// meaning qcow2 (every qemu we support has it).
    pub fn get_default_storage_format(&self) -> Option<String> {
        let raw = engine::get().config().get_default_storage_format_raw();
        if raw != "default" {
            return Some(raw);
        }
        let fmt = crate::config::DEFAULT_STORAGE_FORMAT_FROM_CONFIG;
        if fmt != "qcow2" || self.is_qemu() || self.is_test() {
            return Some(fmt.to_string());
        }
        None
    }

    /// `vmmNetwork.start`, by name.
    pub async fn start_net(&self, name: &str) -> Result<(), String> {
        let handle = self.handle().ok_or("Not connected")?;
        let name = name.to_string();
        gio::spawn_blocking(move || {
            let net = virt::network::Network::lookup_by_name(handle.connect(), &name)
                .map_err(|e| e.to_string())?;
            net.create().map(|_| ()).map_err(|e| e.to_string())
        })
        .await
        .map_err(|_| "Worker thread panicked".to_string())?
    }

    pub fn is_remote(&self) -> bool {
        !virtinst_core::connection::Uri::parse(&self.get_uri())
            .hostname
            .is_empty()
    }

    /// The URI's driver: `qemu` for `qemu+ssh://...`.
    fn driver(&self) -> String {
        let uri = virtinst_core::connection::Uri::parse(&self.get_uri());
        uri.scheme.split('+').next().unwrap_or_default().to_string()
    }

    pub fn is_qemu(&self) -> bool {
        self.driver() == "qemu"
    }

    pub fn is_xen(&self) -> bool {
        let d = self.driver();
        d.starts_with("xen") || d.starts_with("libxl")
    }

    pub fn is_test(&self) -> bool {
        self.driver() == "test"
    }

    pub fn is_lxc(&self) -> bool {
        virtinst_core::connection::Uri::parse(&self.get_uri()).is_lxc()
    }

    pub fn is_container_only(&self) -> bool {
        virtinst_core::connection::Uri::parse(&self.get_uri()).is_container_only()
    }

    pub fn is_vz(&self) -> bool {
        virtinst_core::connection::Uri::parse(&self.get_uri()).is_vz()
    }

    /// `invalidate_caps()` then `caps`: the host capabilities, fetched
    /// fresh on a worker.
    pub async fn fetch_caps(&self) -> Result<Capabilities, String> {
        let handle = self.handle().ok_or("Not connected")?;
        gio::spawn_blocking(move || {
            let xml = handle
                .connect()
                .get_capabilities()
                .map_err(|e| e.to_string())?;
            Capabilities::parse(&xml).map_err(|e| e.to_string())
        })
        .await
        .map_err(|_| "Worker thread panicked".to_string())?
    }

    /// `DomainCapabilities.build_from_params`: `None` when libvirt
    /// can't answer, like upstream's stub object.
    pub async fn fetch_domcaps(
        &self,
        emulator: Option<String>,
        arch: Option<String>,
        machine: Option<String>,
        virttype: Option<String>,
    ) -> Option<virtinst_core::capabilities::DomainCapabilities> {
        let handle = self.handle()?;
        gio::spawn_blocking(move || {
            let xml = handle
                .connect()
                .get_domain_capabilities(
                    emulator.as_deref(),
                    arch.as_deref(),
                    machine.as_deref(),
                    virttype.as_deref(),
                    0,
                )
                .ok()?;
            virtinst_core::capabilities::DomainCapabilities::parse(&xml).ok()
        })
        .await
        .ok()
        .flatten()
    }

    /// `is_qemu_unprivileged`: a `qemu:///session` style connection.
    pub fn is_qemu_unprivileged(&self) -> bool {
        let uri = virtinst_core::connection::Uri::parse(&self.get_uri());
        uri.scheme.split('+').next() == Some("qemu") && uri.path.trim_matches('/') == "session"
    }

    pub fn list_vms(&self) -> Vec<VmmDomain> {
        self.imp().vms.borrow().clone()
    }

    fn change_state(&self, state: ConnState) {
        if self.state() != state {
            self.imp().state.set(state);
            self.emit_by_name::<()>("state-changed", &[]);
        }
    }

    /// Opens the connection on a worker thread. `done` gets upstream's
    /// `open-completed` payload: `None` on success or a cancelled auth,
    /// the error otherwise.
    pub fn open(&self, done: impl FnOnce(Option<ConnectError>) + 'static) {
        if !self.is_disconnected() {
            return;
        }
        self.change_state(ConnState::Connecting);
        let uri = self.get_uri();
        let this = self.clone();
        let weak = glib::SendWeakRef::from(self.downgrade());
        glib::spawn_future_local(async move {
            let uri2 = uri.clone();
            let res = gio::spawn_blocking(move || {
                // `_do_open`
                let conn = connectauth::open_auth(&uri2)
                    .map_err(|e| connectauth::open_error(&uri2, &e))?;
                // `_add_conn_events`
                let event_ids = connevents::register(&conn, &weak);
                // `_hostinfo = getInfo()`; [2] is the active CPU count.
                let info = conn.get_node_info().ok();
                let hostcpus = info.as_ref().map(|i| i.cpus).unwrap_or(0);
                let hostmem = info.map(|i| i.memory).unwrap_or(0);
                let caps = conn
                    .get_capabilities()
                    .ok()
                    .and_then(|xml| Capabilities::parse(&xml).ok());
                let host_cpu_model = caps.and_then(|c| c.host.cpu.model);
                Ok((
                    ConnHandle(conn, event_ids),
                    (hostcpus, hostmem, host_cpu_model),
                ))
            })
            .await
            .unwrap_or_else(|_| {
                let msg = "Connect thread panicked";
                Err(Some(connectauth::connect_error(&uri, msg, msg, false)))
            });
            match res {
                Ok((conn, (hostcpus, hostmem, host_cpu_model))) => {
                    this.imp().hostcpus.set(hostcpus);
                    this.imp().hostmem.set(hostmem);
                    this.imp().host_cpu_model.replace(host_cpu_model);
                    this.imp().handle.replace(Some(Arc::new(conn)));
                    this.change_state(ConnState::Active);
                    this.tick();
                    done(None);
                }
                Err(err) => {
                    this.change_state(ConnState::Disconnected);
                    done(err);
                }
            }
        });
    }

    pub fn close(&self) {
        for vm in self.imp().vms.take() {
            self.emit_by_name::<()>("vm-removed", &[&vm]);
        }
        // Dropping the last Arc closes it; a worker still mid-call holds
        // its own clone and closes it when that call returns.
        self.imp().handle.take();
        self.change_state(ConnState::Disconnected);
    }

    /// One engine tick: poll the domain list off-thread, then reconcile
    /// into vm-added / vm-removed / per-domain state-changed. A tick
    /// already in flight makes this a no-op, like upstream's bounded
    /// tick queue.
    pub fn tick(&self) {
        let Some(handle) = self.handle() else {
            return;
        };
        if self.imp().ticking.replace(true) {
            return;
        }
        let flags = engine::get().config().get_poll_flags();
        let is_lxc = Uri::parse(&self.get_uri()).scheme == "lxc";
        let mem_period_set: HashSet<String> = self
            .list_vms()
            .iter()
            .filter(|v| v.mem_period_set())
            .map(|v| v.get_uuid())
            .collect();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let res = gio::spawn_blocking(move || {
                let res = DomainSnapshot::fetch_all_with_stats(
                    handle.connect(),
                    flags,
                    is_lxc,
                    &mem_period_set,
                );
                let alive = handle.connect().is_alive().unwrap_or(false);
                (res, alive)
            })
            .await;
            this.imp().ticking.set(false);
            match res {
                Ok((Ok(polled), _)) => this.reconcile(polled, flags),
                Ok((Err(_), false)) => this.close(),
                // A transient listing error on a live connection (a
                // domain vanishing mid-list): try again next tick.
                _ => {}
            }
        });
    }

    /// `_domain_lifecycle_event`, `_domain_agent_lifecycle_event` and
    /// `_domain_xml_misc_event`, matched by UUID rather than name.
    pub(crate) fn domain_event(&self, uuid: &str, event: DomainEvent) {
        let Some(vm) = self.list_vms().into_iter().find(|v| v.get_uuid() == uuid) else {
            // A domain we don't know yet (newly defined or started).
            self.tick();
            return;
        };
        // Deleting external snapshots sends this for a shutoff VM, and
        // recaching then shows it as paused.
        if event == DomainEvent::AgentLifecycle && vm.status() == sys::VIR_DOMAIN_SHUTOFF {
            return;
        }
        glib::spawn_future_local(async move { vm.recache_from_event_loop().await });
    }

    /// `_network_lifecycle_event`, `_storage_pool_lifecycle_event` and
    /// `_storage_pool_refresh_event`. There are no network or pool
    /// objects here to recache, so the windows that list them hear
    /// `net-event(name)` / `pool-event(name)` and re-read what they show:
    /// upstream's `net-added`/`net-removed`/`pool-added`/`pool-removed`
    /// and the objects' `state-changed`/`refreshed`.
    pub(crate) fn object_event(&self, event: ObjectEvent, name: &str) {
        if !self.is_active() {
            return;
        }
        let signal = match event {
            ObjectEvent::NetworkLifecycle => "net-event",
            ObjectEvent::PoolLifecycle | ObjectEvent::PoolRefresh => "pool-event",
        };
        self.emit_by_name::<()>(signal, &[&name.to_string()]);
    }

    fn reconcile(&self, polled: Vec<(DomainSnapshot, RawSample)>, flags: PollFlags) {
        if !self.is_active() {
            return;
        }
        let hostcpus = self.imp().hostcpus.get();
        let existing = self.list_vms();
        for vm in &existing {
            if !polled.iter().any(|(s, _)| s.uuid == vm.get_uuid()) {
                self.imp().vms.borrow_mut().retain(|v| v != vm);
                self.emit_by_name::<()>("vm-removed", &[vm]);
            }
        }
        let mut preexisting = vec![];
        for (snap, raw) in polled {
            let found = existing.iter().find(|v| v.get_uuid() == snap.uuid);
            let vm = match found {
                Some(vm) => {
                    vm.update(snap);
                    preexisting.push(vm.clone());
                    vm.clone()
                }
                None => {
                    let vm = VmmDomain::new(self, snap);
                    self.imp().vms.borrow_mut().push(vm.clone());
                    self.emit_by_name::<()>("vm-added", &[&vm]);
                    vm
                }
            };
            vm.apply_sample(&raw, flags, hostcpus);
        }
        self.recalculate_stats(&preexisting);
        self.emit_by_name::<()>("resources-sampled", &[]);
    }

    /// `_recalculate_stats` over the VMs that were already known before
    /// this tick.
    fn recalculate_stats(&self, vms: &[VmmDomain]) {
        let samples: Vec<VmStatsSample> = vms
            .iter()
            .filter(|vm| vm.is_active())
            .map(|vm| {
                let stats = vm.stats();
                let r = stats.latest();
                VmStatsSample {
                    cpu_time: r.cpu_time,
                    memory: r.curmem,
                    disk_rd_rate: r.disk_rd_rate,
                    disk_wr_rate: r.disk_wr_rate,
                    net_rx_rate: r.net_rx_rate,
                    net_tx_rate: r.net_tx_rate,
                    disk_max_rate: stats.disk_io_max_rate(),
                    net_max_rate: stats.network_traffic_max_rate(),
                }
            })
            .collect();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        self.imp().stats.borrow_mut().recalculate(
            now,
            &samples,
            self.host_memory_size(),
            self.host_active_processor_count(),
        );
    }

    /// `stats_memory_vector`.
    pub fn stats_memory_vector(&self) -> Vec<f64> {
        self.imp().stats.borrow().vector(|r| r.memory_percent)
    }

    /// `host_cpu_time_vector`.
    pub fn host_cpu_time_vector(&self) -> Vec<f64> {
        self.imp().stats.borrow().vector(|r| r.cpu_host_percent)
    }

    /// `stats_memory`, in KiB.
    pub fn stats_memory(&self) -> u64 {
        self.imp().stats.borrow().latest().memory
    }

    /// `host_cpu_time_percentage`.
    pub fn host_cpu_time_percentage(&self) -> f64 {
        self.imp().stats.borrow().latest().cpu_host_percent
    }

    /// `get_pretty_desc`: the configured `pretty-name`, else the
    /// default label. The first call starts listening for changes to
    /// it (`_on_config_pretty_name_changed`), which upstream does at
    /// construction; here the engine and its config come later.
    pub fn get_pretty_desc(&self) -> String {
        let config = engine::get().config();
        let uri = self.get_uri();
        if !self.imp().pretty_name_watch.replace(true) {
            use gio::prelude::SettingsExt;
            let weak = self.downgrade();
            // `_config_pretty_name_changed_cb`
            config
                .conn_settings(&uri)
                .connect_changed(Some("pretty-name"), move |_, _| {
                    if let Some(conn) = weak.upgrade() {
                        conn.emit_by_name::<()>("state-changed", &[]);
                    }
                });
        }
        let cfgname = config.get_perconn_pretty_name(&uri);
        if !cfgname.is_empty() {
            return cfgname;
        }
        pretty_desc(&uri)
    }

    /// `set_config_pretty_name`.
    pub fn set_config_pretty_name(&self, value: &str) {
        let config = engine::get().config();
        let uri = self.get_uri();
        let cfgname = config.get_perconn_pretty_name(&uri);
        if value == cfgname {
            return;
        }
        if cfgname.is_empty() && value == self.get_pretty_desc() {
            // Don't encode the default connection value into gconf right
            // away, require the user to edit it first
            return;
        }
        config.set_perconn_pretty_name(&uri, value);
    }

    /// `get_details_window_size`.
    pub fn get_details_window_size(&self) -> (i32, i32) {
        engine::get()
            .config()
            .get_perconn_window_size(&self.get_uri())
    }

    /// `set_details_window_size`.
    pub fn set_details_window_size(&self, w: i32, h: i32) {
        engine::get()
            .config()
            .set_perconn_window_size(&self.get_uri(), w, h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_desc_matches_upstream() {
        assert_eq!(pretty_desc("qemu:///system"), "QEMU/KVM");
        assert_eq!(pretty_desc("qemu:///session"), "QEMU/KVM User session");
        assert_eq!(pretty_desc("qemu+ssh://root@host/system"), "QEMU/KVM: host");
        assert_eq!(pretty_desc("test:///default"), "test default");
        assert_eq!(pretty_desc("lxc:///"), "LXC");
        assert_eq!(pretty_desc("xen:///"), "Xen");
    }

    #[test]
    fn conn_stats_follow_recalculate_stats() {
        let mut stats = ConnStats::default();
        let vm = |cpu_time, memory| VmStatsSample {
            cpu_time,
            memory,
            disk_max_rate: 10.0,
            net_max_rate: 50.0,
            ..Default::default()
        };
        // The first record has no previous timestamp: no CPU percentage
        stats.recalculate(100.0, &[vm(1_000_000_000, 1024)], 4096, 2);
        let r = stats.latest();
        assert_eq!(r.cpu_host_percent, 0.0);
        assert_eq!(r.memory, 1024);
        assert_eq!(r.memory_percent, 25.0);
        assert_eq!(r.net_max_rate, 50.0);
        // 1s of CPU time over 2s on 2 host CPUs, two VMs
        stats.recalculate(
            102.0,
            &[vm(1_000_000_000, 1024), vm(1_000_000_000, 2048)],
            4096,
            2,
        );
        let r = stats.latest();
        assert!((r.cpu_host_percent - 50.0).abs() < 1e-9);
        assert_eq!(r.memory_percent, 75.0);
        // Clamped to 100
        stats.recalculate(103.0, &[vm(10_000_000_000, 8192)], 4096, 1);
        let r = stats.latest();
        assert_eq!(r.cpu_host_percent, 100.0);
        assert_eq!(r.memory_percent, 100.0);

        let v = stats.vector(|r| r.cpu_host_percent);
        assert_eq!(v.len(), crate::statsmanager::STATS_HISTORY_LENGTH + 1);
        assert_eq!(&v[..3], &[1.0, 0.5, 0.0]);
        assert_eq!(v[3], 0.0);

        for i in 0..300 {
            stats.recalculate(200.0 + i as f64, &[], 4096, 1);
        }
        assert_eq!(
            stats.stats.len(),
            crate::statsmanager::STATS_HISTORY_LENGTH + 1
        );
    }
}
