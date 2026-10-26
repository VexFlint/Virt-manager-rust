//! Upstream's `virtManager/connection.py` (`vmmConnection`), the slice the
//! manager window needs: connect/disconnect state, the pretty labels,
//! and the domain list the engine's tick keeps current.
//!
//! Signals, named as upstream's: `state-changed`, `vm-added(VmmDomain)`,
//! `vm-removed(VmmDomain)`.
//!
//! **Not ported yet**: the openAuth credentials dialog (`connectauth.py`
//! — plain `virConnectOpen` covers local URIs and polkit agents),
//! libvirt event callbacks (needs the libvirt-glib gir binding, ticket
//! 09 — the tick polls instead, which is upstream's own fallback for
//! drivers without events), networks/pools/nodedevs, and the
//! per-connection `pretty-name` setting.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use gtk::gio;
use gtk::glib;
use gtk::glib::prelude::*;
use gtk::glib::subclass::prelude::*;
use gtk::glib::subclass::Signal;
use virt::connect::Connect;
use virtinst_core::capabilities::caps::Capabilities;
use virtinst_core::connection::Uri;

use crate::domain::{DomainSnapshot, VmmDomain};
use crate::engine;
use crate::statsmanager::{PollFlags, RawSample};

/// Owns one libvirt connection reference and closes it on drop — the
/// `virt` crate's `Connect` has no `Drop` of its own. Shared via `Arc`
/// so a worker thread mid-call keeps the connection alive past a
/// user-initiated disconnect.
#[derive(Debug)]
pub struct ConnHandle(Connect);

impl ConnHandle {
    pub fn connect(&self) -> &Connect {
        &self.0
    }
}

impl Drop for ConnHandle {
    fn drop(&mut self) {
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

/// `connectauth.connect_error`, minus the netcat/askpass/xen hints that
/// need the remote and Xen paths ported first.
pub fn connect_error(uri: &str, errmsg: &str) -> ConnectError {
    let parsed = Uri::parse(uri);
    let errmsg = errmsg.trim_matches(|c| c == ' ' || c == '\n');
    let mut msg = format!("Unable to connect to libvirt {uri}.");
    let mut hint = String::new();
    let mut show_errmsg = true;
    if !parsed.hostname.is_empty() || !parsed.transport.is_empty() {
        hint.push_str("Verify that an appropriate libvirt daemon is running on the remote host.");
    } else if mentions_virt_socket(errmsg) {
        hint.push_str("Verify that an appropriate libvirt daemon is running.");
        show_errmsg = false;
    }
    if show_errmsg {
        msg.push_str(&format!("\n\n{errmsg}"));
    }
    if !hint.is_empty() {
        msg.push_str(&format!("\n\n{hint}"));
    }
    let msg = msg.trim_matches('\n').to_string();
    let details = format!("{msg}\n\nLibvirt URI is: {uri}\n\n{errmsg}");
    ConnectError {
        msg,
        details,
        title: "Virtual Machine Manager Connection Failure".into(),
    }
}

/// Upstream's `re.search(r"virt[a-z]*-sock", tb)`.
fn mentions_virt_socket(text: &str) -> bool {
    text.match_indices("virt").any(|(idx, _)| {
        let rest = text[idx + 4..].trim_start_matches(|c: char| c.is_ascii_lowercase());
        rest.starts_with("-sock")
    })
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

    pub fn get_pretty_desc(&self) -> String {
        pretty_desc(&self.get_uri())
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
        let Some(handle) = self.handle() else {
            return vec![];
        };
        gio::spawn_blocking(move || {
            let devs = handle
                .connect()
                .list_all_node_devices(0)
                .unwrap_or_default();
            devs.iter()
                .filter_map(|d| d.get_xml_desc(0).ok())
                .filter_map(|xml| virtinst_core::nodedev::NodeDevice::parse(&xml))
                .collect()
        })
        .await
        .unwrap_or_default()
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
    /// `open-completed` payload: `None` on success, the error otherwise.
    pub fn open(&self, done: impl FnOnce(Option<ConnectError>) + 'static) {
        if !self.is_disconnected() {
            return;
        }
        self.change_state(ConnState::Connecting);
        let uri = self.get_uri();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let uri2 = uri.clone();
            let res = gio::spawn_blocking(move || {
                let conn = Connect::open(Some(&uri2)).map_err(|e| e.message().to_string())?;
                // `_hostinfo = getInfo()`; [2] is the active CPU count.
                let info = conn.get_node_info().ok();
                let hostcpus = info.as_ref().map(|i| i.cpus).unwrap_or(0);
                let hostmem = info.map(|i| i.memory).unwrap_or(0);
                let caps = conn
                    .get_capabilities()
                    .ok()
                    .and_then(|xml| Capabilities::parse(&xml).ok());
                let host_cpu_model = caps.and_then(|c| c.host.cpu.model);
                Ok((conn, (hostcpus, hostmem, host_cpu_model)))
            })
            .await
            .unwrap_or_else(|_| Err("Connect thread panicked".to_string()));
            match res {
                Ok((conn, (hostcpus, hostmem, host_cpu_model))) => {
                    this.imp().hostcpus.set(hostcpus);
                    this.imp().hostmem.set(hostmem);
                    this.imp().host_cpu_model.replace(host_cpu_model);
                    this.imp().handle.replace(Some(Arc::new(ConnHandle(conn))));
                    this.change_state(ConnState::Active);
                    this.tick();
                    done(None);
                }
                Err(e) => {
                    this.change_state(ConnState::Disconnected);
                    done(Some(connect_error(&uri, &e)));
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
        for (snap, raw) in polled {
            let found = existing.iter().find(|v| v.get_uuid() == snap.uuid);
            let vm = match found {
                Some(vm) => {
                    vm.update(snap);
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
    fn connect_error_shapes() {
        let local = connect_error("qemu:///system", "  boom \n");
        assert_eq!(
            local.msg,
            "Unable to connect to libvirt qemu:///system.\n\nboom"
        );
        assert!(local
            .details
            .contains("Libvirt URI is: qemu:///system\n\nboom"));
        let remote = connect_error("qemu+ssh://h/system", "x");
        assert!(remote.msg.ends_with("running on the remote host."));
        let sock = connect_error(
            "qemu:///system",
            "Failed to connect socket to '/run/libvirt/virtqemud-sock'",
        );
        assert_eq!(
            sock.msg,
            "Unable to connect to libvirt qemu:///system.\n\nVerify that an appropriate libvirt daemon is running."
        );
    }
}
