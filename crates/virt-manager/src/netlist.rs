//! `device/netlist.py`: the NIC source picker, shared by the details
//! page, Add Hardware and (later) the create wizard. Lists the
//! connection's virtual networks plus manual bridge, macvtap and vDPA
//! entries, with the portgroup and device name fields they need.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::connection::{NetInfo, VmmConnection};

/// `_NetRowData`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NetRow {
    nettype: String,
    source: Option<String>,
    label: String,
    manual: bool,
}

impl NetRow {
    fn manual(nettype: &str, label: &str) -> NetRow {
        NetRow {
            nettype: nettype.into(),
            source: None,
            label: label.into(),
            manual: true,
        }
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// `_pretty_network_desc`.
fn pretty_network_desc(nettype: &str, source: Option<&str>, netobj: Option<&NetInfo>) -> String {
    if nettype == "user" {
        return "Usermode networking".into();
    }
    let mut extra = None;
    let mut ret = if nettype == "network" {
        if let Some(net) = netobj {
            extra = Some(format!(": {}", net.pretty_forward_mode()));
        }
        "Virtual network".to_string()
    } else {
        capitalize(nettype)
    };
    if let Some(source) = source {
        ret += &format!(" '{source}'");
    }
    if let Some(extra) = extra {
        ret += &format!(" {extra}");
    }
    ret
}

/// `_find_virtual_networks`.
fn virtual_network_rows(nets: &[NetInfo]) -> Vec<NetRow> {
    nets.iter()
        .map(|net| {
            let mut label = pretty_network_desc("network", Some(&net.name), Some(net));
            if !net.active {
                label += " (Inactive)";
            }
            if net.virtualport_type.as_deref() == Some("openvswitch") {
                label += " (OpenVSwitch)";
            }
            NetRow {
                nettype: "network".into(),
                source: Some(net.name.clone()),
                label,
                manual: false,
            }
        })
        .collect()
}

/// interface.py's `_host_default_bridge`: the default route's device,
/// when it is a bridge.
fn host_default_bridge() -> Option<String> {
    let routes = std::fs::read_to_string("/proc/net/route").ok()?;
    let dev = routes.lines().find_map(|line| {
        let info: Vec<&str> = line.split_whitespace().collect();
        (info.len() == 11 && u32::from_str_radix(info[1], 16) == Ok(0)).then(|| info[0].to_string())
    })?;
    std::path::Path::new(&format!("/sys/class/net/{dev}/bridge"))
        .exists()
        .then_some(dev)
}

/// `_populate_network_model`: the rows, the index to default to, and
/// whether no usable default was found.
fn build_rows(
    nets: &[NetInfo],
    unprivileged: bool,
    default_bridge: Option<&str>,
) -> (Vec<NetRow>, usize, bool) {
    let mut rows = vec![];
    if unprivileged {
        rows.push(NetRow {
            nettype: "user".into(),
            source: None,
            label: pretty_network_desc("user", None, None),
            manual: false,
        });
    }
    let mut vnets = virtual_network_rows(nets);
    vnets.sort_by(|a, b| a.label.cmp(&b.label));
    let mut defaultnetidx = None;
    for row in vnets {
        if row.source.as_deref() == Some("default") {
            defaultnetidx = Some(rows.len());
        }
        rows.push(row);
    }
    let bridgeidx = rows.len();
    rows.push(NetRow::manual("bridge", "Bridge device..."));
    rows.push(NetRow::manual("direct", "Macvtap device..."));
    rows.push(NetRow::manual("vdpa", "vDPA device..."));

    let default_bridge = default_bridge.filter(|_| !unprivileged);
    if default_bridge.is_some() {
        return (rows, bridgeidx, false);
    }
    if let Some(idx) = defaultnetidx {
        return (rows, idx, false);
    }
    (rows, 0, bridgeidx == 0)
}

/// `_find_rowiter_for_dev`: the row for an existing NIC, as an index,
/// plus the manual device name to show. A source we don't list gets a
/// row of its own at the top.
fn find_row_for_dev(
    rows: &mut Vec<NetRow>,
    nettype: &str,
    source: Option<&str>,
    network: Option<&str>,
) -> (usize, Option<String>) {
    // A forward mode=bridge network shows up in a running VM as
    // type=bridge with both bridge and network set: treat it as the
    // network.
    let (nettype, source) = match network {
        Some(net) => ("network", Some(net)),
        None => (nettype, source),
    };
    let find = |rows: &[NetRow], src: Option<&str>, manual: bool| {
        rows.iter().position(|r| {
            r.nettype == nettype
                && (src.is_none() || r.source.as_deref() == src)
                && (!manual || r.manual)
        })
    };
    if source.is_some() {
        if let Some(idx) = find(rows, source, false) {
            return (idx, None);
        }
    } else if let Some(idx) = rows.iter().position(|r| r.nettype == nettype) {
        return (idx, None);
    }
    if matches!(nettype, "bridge" | "direct" | "vdpa") {
        if let Some(idx) = find(rows, None, true) {
            return (idx, Some(source.unwrap_or("").to_string()));
        }
    }
    rows.insert(
        0,
        NetRow {
            nettype: nettype.into(),
            source: source.map(str::to_string),
            label: pretty_network_desc(nettype, source, None),
            manual: false,
        },
    );
    (0, None)
}

/// `DeviceInterface.check_mac_in_use`, over the connection's VMs.
pub fn check_mac_in_use(conn: &VmmConnection, searchmac: &str) -> Result<(), String> {
    if searchmac.is_empty() {
        return Ok(());
    }
    let in_use = conn.list_vms().iter().any(|vm| {
        vm.xmlobj().is_some_and(|x| {
            let doc = &x.doc;
            virtinst_xml::resolve_path(doc, x.root(), &["devices"])
                .map(|d| d.find_all(doc, "interface"))
                .unwrap_or_default()
                .into_iter()
                .filter_map(|nic| nic.find(doc, "mac")?.attribute(doc, "address"))
                .any(|mac| mac.eq_ignore_ascii_case(searchmac))
        })
    });
    if in_use {
        return Err(format!(
            "The MAC address '{searchmac}' is in use by another virtual machine."
        ));
    }
    Ok(())
}

type ChangedCb = Box<dyn Fn()>;

/// `get_network_selection`'s `(type, source, mode, portgroup)`.
pub type NetSelection = (String, Option<String>, Option<String>, Option<String>);

/// What `set_dev` was asked to show, kept until the networks load.
#[derive(Debug, Clone)]
pub struct NetDev {
    pub nettype: String,
    pub source: Option<String>,
    pub network: Option<String>,
    pub portgroup: Option<String>,
}

pub struct NetList {
    pub top_label: gtk::Label,
    pub top_box: gtk::Grid,
    conn: Option<VmmConnection>,
    source: gtk::DropDown,
    manual_source: gtk::Entry,
    manual_label: gtk::Label,
    portgroup: gtk::DropDown,
    portgroup_label: gtk::Label,
    default_warn: gtk::Box,
    macvtap_warn: gtk::Box,
    rows: RefCell<Vec<NetRow>>,
    nets: RefCell<Vec<NetInfo>>,
    portgroups: RefCell<Vec<String>>,
    /// Set while the widgets are filled in, so it isn't an edit.
    quiet: Cell<bool>,
    /// Bumped per `set_dev`, so a slow network fetch can't win over a
    /// newer one.
    generation: Cell<u32>,
    changed: RefCell<Option<ChangedCb>>,
}

fn warn_box(text: &str) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 3);
    b.set_halign(gtk::Align::Start);
    b.append(&gtk::Image::from_icon_name("dialog-warning"));
    let l = gtk::Label::new(None);
    l.set_markup(&format!(
        "<small>{}</small>",
        glib::markup_escape_text(text)
    ));
    l.set_halign(gtk::Align::Start);
    l.set_wrap(true);
    b.append(&l);
    b.set_visible(false);
    b
}

impl NetList {
    pub fn new(conn: Option<VmmConnection>) -> Rc<NetList> {
        let top_label = gtk::Label::with_mnemonic("_Network source:");
        top_label.set_halign(gtk::Align::End);
        top_label.set_valign(gtk::Align::Start);
        top_label.set_margin_top(6);

        let source = gtk::DropDown::from_strings(&[]);
        source.set_halign(gtk::Align::Start);
        source.update_property(&[gtk::accessible::Property::Label("net-source")]);
        top_label.set_mnemonic_widget(Some(&source));
        let manual_label = gtk::Label::with_mnemonic("De_vice name:");
        manual_label.set_halign(gtk::Align::Start);
        let manual_source = gtk::Entry::new();
        manual_source.set_halign(gtk::Align::Start);
        manual_label.set_mnemonic_widget(Some(&manual_source));
        let portgroup_label = gtk::Label::with_mnemonic("_Portgroup:");
        portgroup_label.set_halign(gtk::Align::Start);
        let portgroup = gtk::DropDown::from_strings(&[]);
        portgroup.set_halign(gtk::Align::Start);
        portgroup_label.set_mnemonic_widget(Some(&portgroup));
        let default_warn = warn_box("Failed to find a suitable default network.");
        let macvtap_warn = warn_box(
            "In most configurations, macvtap does not work for host to guest network communication.",
        );

        let top_box = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(6)
            .build();
        top_box.attach(&default_warn, 0, 0, 2, 1);
        top_box.attach(&source, 0, 1, 2, 1);
        top_box.attach(&manual_label, 0, 2, 1, 1);
        top_box.attach(&manual_source, 1, 2, 1, 1);
        top_box.attach(&portgroup_label, 0, 3, 1, 1);
        top_box.attach(&portgroup, 1, 3, 1, 1);
        top_box.attach(&macvtap_warn, 0, 4, 2, 1);
        for w in [
            manual_label.upcast_ref::<gtk::Widget>(),
            manual_source.upcast_ref(),
            portgroup_label.upcast_ref(),
            portgroup.upcast_ref(),
        ] {
            w.set_visible(false);
        }

        let this = Rc::new(NetList {
            top_label,
            top_box,
            conn,
            source,
            manual_source,
            manual_label,
            portgroup,
            portgroup_label,
            default_warn,
            macvtap_warn,
            rows: RefCell::default(),
            nets: RefCell::default(),
            portgroups: RefCell::default(),
            quiet: Cell::new(false),
            generation: Cell::new(0),
            changed: RefCell::default(),
        });
        let weak = Rc::downgrade(&this);
        this.source.connect_selected_notify(move |_| {
            if let Some(this) = weak.upgrade() {
                this.on_net_source_changed();
            }
        });
        let weak = Rc::downgrade(&this);
        this.portgroup.connect_selected_notify(move |_| {
            if let Some(this) = weak.upgrade() {
                this.emit_changed();
            }
        });
        let weak = Rc::downgrade(&this);
        this.manual_source.connect_changed(move |_| {
            if let Some(this) = weak.upgrade() {
                this.emit_changed();
            }
        });
        this
    }

    /// Calls `cb` when the selection changes: the `changed` signal.
    pub fn connect_changed(&self, cb: impl Fn() + 'static) {
        self.changed.replace(Some(Box::new(cb)));
    }

    fn emit_changed(&self) {
        if self.quiet.get() {
            return;
        }
        if let Some(cb) = &*self.changed.borrow() {
            cb();
        }
    }

    fn quietly(&self, f: impl FnOnce()) {
        let old = self.quiet.replace(true);
        f();
        self.quiet.set(old);
    }

    fn selected_row(&self) -> Option<NetRow> {
        self.rows
            .borrow()
            .get(self.source.selected() as usize)
            .cloned()
    }

    fn set_rows(&self, rows: Vec<NetRow>) {
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        self.source.set_model(Some(&gtk::StringList::new(&labels)));
        self.rows.replace(rows);
    }

    /// `get_network_selection`: `(type, source, mode, portgroup)`.
    pub fn get_network_selection(&self) -> Option<NetSelection> {
        let row = self.selected_row()?;
        let source = if row.manual {
            Some(self.manual_source.text().to_string()).filter(|s| !s.is_empty())
        } else {
            row.source.clone()
        };
        // This is generally the safest and most featureful default
        let mode = (row.nettype == "direct").then(|| "bridge".to_string());
        let portgroup = if self.portgroup.is_visible() {
            self.portgroups
                .borrow()
                .get(self.portgroup.selected() as usize)
                .cloned()
        } else {
            None
        };
        Some((row.nettype, source, mode, portgroup))
    }

    /// `set_dev`: `reset_state`, fetch the networks, then select the
    /// NIC's source. The fetch is async, and none of it counts as an
    /// edit.
    pub fn set_dev(self: &Rc<Self>, dev: NetDev) {
        let gen = self.generation.get() + 1;
        self.generation.set(gen);
        self.quietly(|| {
            self.default_warn.set_visible(false);
            self.manual_source.set_text("");
        });
        let weak = Rc::downgrade(self);
        let conn = self.conn.clone();
        glib::spawn_future_local(async move {
            let nets = match &conn {
                Some(c) => c.list_nets().await,
                None => vec![],
            };
            let Some(this) = weak.upgrade() else {
                return;
            };
            if this.generation.get() != gen {
                return;
            }
            this.quietly(|| this.show_dev(nets, &dev));
        });
    }

    /// `reset_state`: refetch the networks, keeping the picked row
    /// when it's still there, else the default one.
    pub fn reset_state(self: &Rc<Self>) {
        let gen = self.generation.get() + 1;
        self.generation.set(gen);
        let current = self.selected_row().map(|r| r.label);
        self.quietly(|| {
            self.default_warn.set_visible(false);
            self.manual_source.set_text("");
        });
        let weak = Rc::downgrade(self);
        let conn = self.conn.clone();
        glib::spawn_future_local(async move {
            let nets = match &conn {
                Some(c) => c.list_nets().await,
                None => vec![],
            };
            let Some(this) = weak.upgrade() else {
                return;
            };
            if this.generation.get() != gen {
                return;
            }
            this.quietly(|| {
                let (rows, default_idx) = this.populate(nets);
                let idx = current
                    .and_then(|l| rows.iter().position(|r| r.label == l))
                    .unwrap_or(default_idx);
                this.set_rows(rows);
                this.source.set_selected(idx as u32);
                this.on_net_source_changed();
            });
            this.emit_changed();
        });
    }

    /// `_populate_network_model`'s side effects: the default bridge in
    /// the manual entry and the warning; returns the rows and the
    /// default index.
    fn populate(&self, nets: Vec<NetInfo>) -> (Vec<NetRow>, usize) {
        let (unprivileged, remote) = self
            .conn
            .as_ref()
            .map(|c| (c.is_qemu_unprivileged(), c.is_remote()))
            .unwrap_or((false, true));
        let default_bridge = if remote { None } else { host_default_bridge() };
        let (rows, default_idx, warn) = build_rows(&nets, unprivileged, default_bridge.as_deref());
        if let Some(br) = default_bridge.filter(|_| !unprivileged) {
            self.manual_source.set_text(&br);
        }
        self.default_warn.set_visible(warn);
        self.nets.replace(nets);
        (rows, default_idx)
    }

    /// `validate_device`: offers to start an inactive virtual network,
    /// then `check_mac_in_use`.
    pub async fn validate_device(
        &self,
        parent: &gtk::Window,
        nettype: &str,
        source: Option<&str>,
        mac: &str,
    ) -> Result<(), String> {
        // `_check_network_is_running`
        let inactive = nettype == "network"
            && self
                .nets
                .borrow()
                .iter()
                .any(|n| Some(n.name.as_str()) == source && !n.active);
        if let (true, Some(devname), Some(conn)) = (inactive, source, &self.conn) {
            let res = crate::error::yes_no(
                Some(parent),
                "Virtual Network is not active.",
                Some(&format!(
                    "Virtual Network '{devname}' is not active. \
                     Would you like to start the network now?"
                )),
            )
            .await;
            if res {
                match conn.start_net(devname).await {
                    Ok(()) => glib::g_debug!("virt-manager", "Started network '{devname}'"),
                    Err(e) => {
                        crate::error::show_err(
                            Some(parent),
                            &format!("Could not start virtual network '{devname}': {e}"),
                            None,
                            "",
                        )
                        .await
                    }
                }
            }
        }
        if let Some(conn) = &self.conn {
            check_mac_in_use(conn, mac)?;
        }
        Ok(())
    }

    fn show_dev(&self, nets: Vec<NetInfo>, dev: &NetDev) {
        let (mut rows, _default_idx) = self.populate(nets);
        let (idx, manual) = find_row_for_dev(
            &mut rows,
            &dev.nettype,
            dev.source.as_deref(),
            dev.network.as_deref(),
        );
        if let Some(manual) = manual {
            self.manual_source.set_text(&manual);
        }
        self.set_rows(rows);
        self.source.set_selected(idx as u32);
        self.on_net_source_changed();
        if let Some(pg) = &dev.portgroup {
            let pos = self.portgroups.borrow().iter().position(|p| p == pg);
            let pos = pos.unwrap_or_else(|| {
                self.portgroups.borrow_mut().push(pg.clone());
                if let Some(m) = self.portgroup.model().and_downcast::<gtk::StringList>() {
                    m.append(pg);
                }
                self.portgroups.borrow().len() - 1
            });
            self.portgroup.set_selected(pos as u32);
        }
    }

    /// `_on_net_source_changed`.
    fn on_net_source_changed(&self) {
        self.emit_changed();
        let Some(row) = self.selected_row() else {
            return;
        };
        self.macvtap_warn.set_visible(row.nettype == "direct");
        self.manual_label.set_visible(row.manual);
        self.manual_source.set_visible(row.manual);

        let portgroups: Vec<(String, bool)> = if row.nettype == "network" {
            self.nets
                .borrow()
                .iter()
                .find(|n| Some(&n.name) == row.source.as_ref())
                .map(|n| n.portgroups.clone())
                .unwrap_or_default()
        } else {
            vec![]
        };
        self.portgroup_label.set_visible(!portgroups.is_empty());
        self.portgroup.set_visible(!portgroups.is_empty());
        // _populate_portgroups
        let names: Vec<String> = portgroups.iter().map(|(n, _)| n.clone()).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let default = portgroups.iter().position(|(_, d)| *d);
        let old = self.quiet.replace(true);
        self.portgroup.set_model(Some(&gtk::StringList::new(&refs)));
        if let Some(d) = default {
            self.portgroup.set_selected(d as u32);
        }
        self.quiet.set(old);
        self.portgroups.replace(names);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(name: &str, active: bool, mode: Option<&str>) -> NetInfo {
        NetInfo {
            name: name.into(),
            active,
            forward_mode: mode.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn rows_and_defaults() {
        let nets = [net("zeta", false, None), net("default", true, Some("nat"))];
        let (rows, idx, warn) = build_rows(&nets, false, None);
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Virtual network 'default' : NAT",
                "Virtual network 'zeta' : Isolated network (Inactive)",
                "Bridge device...",
                "Macvtap device...",
                "vDPA device...",
            ]
        );
        assert_eq!((idx, warn), (0, false));

        let (rows, idx, warn) = build_rows(&[], true, Some("br0"));
        assert_eq!(rows[0].label, "Usermode networking");
        assert_eq!((idx, warn), (0, false));

        let (_, idx, warn) = build_rows(&[], false, None);
        assert_eq!((idx, warn), (0, true));
        let (_, idx, _) = build_rows(&[], false, Some("br0"));
        assert_eq!(idx, 0);
    }

    #[test]
    fn finds_rows_for_devices() {
        let nets = [net("default", true, Some("nat"))];
        let (rows, _, _) = build_rows(&nets, false, None);

        let mut r = rows.clone();
        assert_eq!(
            find_row_for_dev(&mut r, "network", Some("default"), Some("default")),
            (0, None)
        );
        // running VM on a forward=bridge network
        let mut r = rows.clone();
        assert_eq!(
            find_row_for_dev(&mut r, "bridge", Some("br9"), Some("default")),
            (0, None)
        );
        let mut r = rows.clone();
        assert_eq!(
            find_row_for_dev(&mut r, "bridge", Some("br0"), None),
            (1, Some("br0".into()))
        );
        let mut r = rows.clone();
        assert_eq!(
            find_row_for_dev(&mut r, "direct", Some("eth0"), None),
            (2, Some("eth0".into()))
        );
        let mut r = rows.clone();
        assert_eq!(find_row_for_dev(&mut r, "vhostuser", None, None), (0, None));
        assert_eq!(r[0].label, "Vhostuser");
        assert_eq!(r.len(), rows.len() + 1);
    }

    #[test]
    fn pretty_descs() {
        assert_eq!(
            pretty_network_desc("bridge", Some("br0"), None),
            "Bridge 'br0'"
        );
        let mut n = net("x", true, Some("route"));
        n.forward_dev = Some("eth1".into());
        assert_eq!(
            pretty_network_desc("network", Some("x"), Some(&n)),
            "Virtual network 'x' : Route to eth1"
        );
        assert_eq!(
            net("y", true, Some("bridge")).pretty_forward_mode(),
            "Bridge network"
        );
    }
}
