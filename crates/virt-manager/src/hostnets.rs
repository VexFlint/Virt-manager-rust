//! `hostnets.py` + `ui/hostnets.ui` (`vmmHostNets`): the Connection
//! Details "Virtual Networks" tab — the network list, the selected
//! network's details (name, device, state, autostart, domain, IPv4 and
//! IPv6 configuration), its XML tab, and the add/start/stop/delete and
//! apply buttons.
//!
//! There are no network objects on the connection (`connection.rs`):
//! the list is fetched when the tab is shown, after each change made
//! here, and on every network lifecycle event (`net-event`), which
//! stands in for upstream's `net-added`, `net-removed` and the
//! networks' own `state-changed`.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gio, glib};
use virt::network::Network as VirNetwork;
use virtinst_core::network::{self, Network};

use crate::connection::VmmConnection;
use crate::createnet::{set_grid_row_visible, CreateNetwork};
use crate::error;
use crate::xmleditor::XmlEditor;

/// `EDIT_NET_IDS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NetEdit {
    Name,
    Autostart,
    Xml,
}

const ICON_RUNNING: &str = "state_running";
const ICON_SHUTOFF: &str = "state_shutoff";

/// A `net-list` row: `[netobj, label, icon name, icon size, is_active]`,
/// with what the details pane reads off the `vmmNetwork`.
#[derive(Clone, Debug)]
pub struct NetRow {
    pub name: String,
    pub uuid: String,
    pub active: bool,
    pub autostart: bool,
    /// `XMLDesc(0)`.
    pub xml: String,
    pub xmlobj: Network,
}

impl NetRow {
    /// `run_status`.
    fn run_status(&self) -> &'static str {
        if self.active {
            "Active"
        } else {
            "Inactive"
        }
    }
}

/// `conn.list_nets()`, fetched on a worker; `None` when the connection
/// can't list networks at all (`conn.support.conn_network()`).
async fn fetch_nets(conn: &VmmConnection) -> Option<Vec<NetRow>> {
    let Some(handle) = conn.handle() else {
        return Some(vec![]);
    };
    gio::spawn_blocking(move || {
        let nets = match handle.connect().list_all_networks(0) {
            Ok(nets) => nets,
            Err(e) if e.code() == virt::error::ErrorNumber::NoSupport => return None,
            Err(_) => vec![],
        };
        Some(
            nets.iter()
                .filter_map(|n| {
                    let xml = n.get_xml_desc(0).ok()?;
                    Some(NetRow {
                        name: n.get_name().ok()?,
                        uuid: n.get_uuid_string().ok()?,
                        active: n.is_active().unwrap_or(false),
                        autostart: n.get_autostart().unwrap_or(false),
                        xmlobj: Network::parse(&xml).ok()?,
                        xml,
                    })
                })
                .collect(),
        )
    })
    .await
    .unwrap_or_default()
}

/// Runs `f` on the network with `uuid` on a worker, as libvirt's
/// message on failure.
async fn with_net(
    conn: &VmmConnection,
    uuid: &str,
    f: impl FnOnce(&virt::connect::Connect, &VirNetwork) -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    let handle = conn.handle().ok_or("Not connected")?;
    let uuid = uuid.to_string();
    gio::spawn_blocking(move || {
        let conn = handle.connect();
        let net = VirNetwork::lookup_by_uuid_string(conn, &uuid).map_err(libvirt_err)?;
        f(conn, &net)
    })
    .await
    .map_err(|_| "Worker thread panicked".to_string())?
}

fn libvirt_err(e: virt::error::Error) -> String {
    e.message().to_string()
}

fn icon_button(icon: &str, tooltip: &str, name: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.set_tooltip_text(Some(tooltip));
    b.update_property(&[gtk::accessible::Property::Label(name)]);
    b
}

fn field_label(text: &str) -> gtk::Label {
    let l = gtk::Label::with_mnemonic(text);
    l.set_halign(gtk::Align::Start);
    l
}

fn value_label(name: Option<&str>) -> gtk::Label {
    let l = gtk::Label::new(Some("label"));
    l.set_halign(gtk::Align::Start);
    l.set_selectable(true);
    if let Some(name) = name {
        l.update_property(&[gtk::accessible::Property::Label(name)]);
    }
    l
}

/// One of the IPv4/IPv6 expanders: `(expander, network, dhcp range,
/// forwarding)`.
fn ip_expander(markup: &str) -> (gtk::Expander, gtk::Label, gtk::Label, gtk::Label) {
    let network = value_label(None);
    let dhcp_range = value_label(None);
    let forwarding = gtk::Label::new(None);
    forwarding.set_halign(gtk::Align::Start);
    forwarding.set_hexpand(true);
    let table = gtk::Grid::new();
    table.set_row_spacing(5);
    table.set_column_spacing(6);
    table.attach(&field_label("Network:"), 0, 0, 1, 1);
    table.attach(&network, 1, 0, 1, 1);
    table.attach(&field_label("DHCP range:"), 0, 1, 1, 1);
    table.attach(&dhcp_range, 1, 1, 1, 1);
    table.attach(&field_label("Forwarding:"), 0, 2, 1, 1);
    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 3);
    hbox.append(&forwarding);
    table.attach(&hbox, 1, 2, 1, 1);
    let expander = gtk::Expander::with_mnemonic(markup);
    expander.set_use_markup(true);
    expander.set_expanded(true);
    expander.set_child(Some(&table));
    (expander, network, dhcp_range, forwarding)
}

pub struct Inner {
    conn: VmmConnection,
    /// `top-box`.
    pub top_box: gtk::Box,
    net_list: gtk::ListBox,
    nets: RefCell<Vec<NetRow>>,
    /// The network the details show, for applying its edits.
    shown_net: RefCell<Option<NetRow>>,
    network_pages: gtk::Stack,
    network_error_label: gtk::Label,
    xmleditor: XmlEditor,
    net_details: gtk::Box,
    net_name: gtk::Entry,
    net_device: gtk::Label,
    net_state: gtk::Label,
    net_state_icon: gtk::Image,
    net_autostart: gtk::CheckButton,
    net_name_domain: gtk::Label,
    net_ipv4_expander: gtk::Expander,
    net_ipv4_network: gtk::Label,
    net_ipv4_dhcp_range: gtk::Label,
    net_ipv4_forwarding: gtk::Label,
    net_ipv6_expander: gtk::Expander,
    net_ipv6_network: gtk::Label,
    net_ipv6_dhcp_range: gtk::Label,
    net_ipv6_forwarding: gtk::Label,
    net_add: gtk::Button,
    net_start: gtk::Button,
    net_stop: gtk::Button,
    net_delete: gtk::Button,
    net_apply: gtk::Button,
    active_edits: RefCell<Vec<NetEdit>>,
    /// Set while the widgets are filled in, so they aren't edits.
    quiet: Cell<bool>,
    /// Bumped per list fetch, so a stale one is dropped.
    generation: Cell<u64>,
    addnet: RefCell<Option<CreateNetwork>>,
    /// The connection's `state-changed` and `net-event` handlers.
    conn_handlers: RefCell<Vec<glib::SignalHandlerId>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        for id in self.conn_handlers.take() {
            self.conn.disconnect(id);
        }
    }
}

#[derive(Clone)]
pub struct HostNets(Rc<Inner>);

impl std::ops::Deref for HostNets {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

impl HostNets {
    pub fn new(conn: &VmmConnection) -> HostNets {
        // net-list
        let net_list = gtk::ListBox::new();
        net_list.set_selection_mode(gtk::SelectionMode::Single);
        net_list.set_size_request(134, -1);
        net_list.update_property(&[gtk::accessible::Property::Label("net-list")]);
        let net_scroll = gtk::ScrolledWindow::builder()
            .child(&net_list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .has_frame(true)
            .build();

        // net-details: table4
        let net_name = gtk::Entry::new();
        net_name.update_property(&[gtk::accessible::Property::Label("net-name")]);
        let net_device = value_label(Some("net-device"));
        let net_state_icon = gtk::Image::from_icon_name("image-missing");
        let net_state = gtk::Label::new(Some("Running"));
        net_state.set_halign(gtk::Align::Start);
        net_state.set_hexpand(true);
        let hbox3 = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        hbox3.append(&net_state_icon);
        hbox3.append(&net_state);
        let net_autostart = gtk::CheckButton::with_mnemonic("Some Label");
        net_autostart.set_halign(gtk::Align::Start);
        net_autostart.update_property(&[gtk::accessible::Property::Label("net-autostart")]);
        let net_name_domain = gtk::Label::new(Some("label"));
        net_name_domain.set_halign(gtk::Align::Start);
        let autostart_label = field_label("A_utostart:");
        autostart_label.set_mnemonic_widget(Some(&net_autostart));
        let domain_label = gtk::Label::new(None);
        domain_label.set_markup("Domain:");
        domain_label.set_halign(gtk::Align::Start);
        let table4 = gtk::Grid::new();
        table4.set_row_spacing(5);
        table4.set_column_spacing(6);
        table4.attach(&field_label("Name:"), 0, 0, 1, 1);
        table4.attach(&net_name, 1, 0, 1, 1);
        table4.attach(&field_label("Device:"), 0, 1, 1, 1);
        table4.attach(&net_device, 1, 1, 1, 1);
        table4.attach(&field_label("State:"), 0, 2, 1, 1);
        table4.attach(&hbox3, 1, 2, 1, 1);
        table4.attach(&autostart_label, 0, 3, 1, 1);
        table4.attach(&net_autostart, 1, 3, 1, 1);
        table4.attach(&domain_label, 0, 4, 1, 1);
        table4.attach(&net_name_domain, 1, 4, 1, 1);

        let (net_ipv4_expander, net_ipv4_network, net_ipv4_dhcp_range, net_ipv4_forwarding) =
            ip_expander("<b>IPv_4 configuration</b>");
        net_ipv4_forwarding.set_text("NAT to any device");
        let (net_ipv6_expander, net_ipv6_network, net_ipv6_dhcp_range, net_ipv6_forwarding) =
            ip_expander("<b>IPv_6 configuration</b>");
        net_ipv6_forwarding.set_text("Routed");

        let net_details = gtk::Box::new(gtk::Orientation::Vertical, 12);
        net_details.append(&table4);
        net_details.append(&net_ipv4_expander);
        net_details.append(&net_ipv6_expander);
        // `vmmXMLEditor(..., net-details-align, net-details)`
        let xmleditor = XmlEditor::new(&net_details);
        let net_details_align = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        xmleditor.widget().set_hexpand(true);
        net_details_align.append(xmleditor.widget());

        // network-pages: the details, or an error
        let network_error_label = gtk::Label::new(Some("some error here"));
        network_error_label.update_property(&[gtk::accessible::Property::Label("net-error-label")]);
        let network_pages = gtk::Stack::new();
        network_pages.add_named(&net_details_align, Some("details"));
        network_pages.add_named(&network_error_label, Some("error"));

        let pane = gtk::Paned::new(gtk::Orientation::Horizontal);
        pane.set_start_child(Some(&net_scroll));
        pane.set_end_child(Some(&network_pages));
        pane.set_resize_start_child(false);
        pane.set_shrink_start_child(true);
        pane.set_resize_end_child(true);
        pane.set_shrink_end_child(true);
        pane.set_position(200);
        pane.set_vexpand(true);
        pane.set_hexpand(true);
        pane.set_margin_start(3);
        pane.set_margin_end(3);
        pane.set_margin_top(3);
        pane.set_margin_bottom(3);

        // Bottom bar: network buttons on the left, Apply on the right
        let net_add = icon_button("list-add", "Add Network", "net-add");
        let net_start = icon_button("media-playback-start", "Start Network", "net-start");
        let net_stop = icon_button("process-stop", "Stop Network", "net-stop");
        let net_delete = icon_button("edit-delete", "Delete Network", "net-delete");
        let hbox16 = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        hbox16.set_hexpand(true);
        for b in [&net_add, &net_start, &net_stop, &net_delete] {
            hbox16.append(b);
        }
        let net_apply = gtk::Button::with_mnemonic("_Apply");
        let hbox17 = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        hbox17.append(&net_apply);
        let hbox15 = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        hbox15.set_margin_start(3);
        hbox15.set_margin_end(3);
        hbox15.set_margin_top(3);
        hbox15.set_margin_bottom(3);
        hbox15.append(&hbox16);
        hbox15.append(&hbox17);

        let top_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        top_box.set_size_request(400, 400);
        top_box.set_margin_start(3);
        top_box.set_margin_end(3);
        top_box.set_margin_top(3);
        top_box.set_margin_bottom(3);
        top_box.set_hexpand(true);
        top_box.update_property(&[gtk::accessible::Property::Label("network-grid")]);
        top_box.append(&pane);
        top_box.append(&hbox15);

        let this = HostNets(Rc::new(Inner {
            conn: conn.clone(),
            top_box,
            net_list,
            nets: RefCell::default(),
            shown_net: RefCell::default(),
            network_pages,
            network_error_label,
            xmleditor,
            net_details,
            net_name,
            net_device,
            net_state,
            net_state_icon,
            net_autostart,
            net_name_domain,
            net_ipv4_expander,
            net_ipv4_network,
            net_ipv4_dhcp_range,
            net_ipv4_forwarding,
            net_ipv6_expander,
            net_ipv6_network,
            net_ipv6_dhcp_range,
            net_ipv6_forwarding,
            net_add,
            net_start,
            net_stop,
            net_delete,
            net_apply,
            active_edits: RefCell::default(),
            quiet: Cell::new(false),
            generation: Cell::new(0),
            addnet: RefCell::default(),
            conn_handlers: RefCell::default(),
        }));
        this.connect_signals();
        this.refresh_conn_state();
        this
    }

    fn weak(&self) -> Weak<Inner> {
        Rc::downgrade(&self.0)
    }

    fn with(&self, f: impl Fn(&HostNets) + 'static) -> impl Fn() + 'static {
        let weak = self.weak();
        move || {
            if let Some(inner) = weak.upgrade() {
                f(&HostNets(inner));
            }
        }
    }

    fn spawn<F: std::future::Future<Output = ()> + 'static>(
        &self,
        f: impl FnOnce(HostNets) -> F + 'static,
    ) {
        let this = self.clone();
        glib::spawn_future_local(async move { f(this).await });
    }

    fn connect_signals(&self) {
        let cb = self.with(HostNets::add_network_clicked);
        self.net_add.connect_clicked(move |_| cb());
        let cb = self.with(|t| t.spawn(|t| async move { t.delete_network_clicked().await }));
        self.net_delete.connect_clicked(move |_| cb());
        let cb = self.with(|t| t.spawn(|t| async move { t.stop_network_clicked().await }));
        self.net_stop.connect_clicked(move |_| cb());
        let cb = self.with(|t| t.spawn(|t| async move { t.start_network_clicked().await }));
        self.net_start.connect_clicked(move |_| cb());
        let cb = self.with(|t| t.spawn(|t| async move { t.net_apply().await }));
        self.net_apply.connect_clicked(move |_| cb());
        let cb = self.with(HostNets::net_selected);
        self.net_list.connect_row_selected(move |_, _| cb());
        let cb = self.with(|t| t.enable_net_apply(NetEdit::Autostart));
        self.net_autostart.connect_toggled(move |_| cb());
        let cb = self.with(|t| t.enable_net_apply(NetEdit::Name));
        self.net_name.connect_changed(move |_| cb());

        let cb = self.with(|t| t.enable_net_apply(NetEdit::Xml));
        self.xmleditor.connect_changed(cb);
        // `_xmleditor_xml_requested_cb`/`_xmleditor_xml_reset_cb`:
        // refetch, so the XML tab gets the network's current XML.
        let cb = self.with(HostNets::refresh_page);
        self.xmleditor.connect_xml_requested(cb);
        let cb = self.with(HostNets::refresh_page);
        self.xmleditor.connect_xml_reset(cb);

        // `_conn_state_changed_cb`
        let cb = self.with(HostNets::refresh_conn_state);
        let id = self.conn.connect_local("state-changed", false, move |_| {
            cb();
            None
        });
        self.conn_handlers.borrow_mut().push(id);
        // `_conn_nets_changed_cb` / `_net_state_changed_cb`
        let weak = self.weak();
        let id = self.conn.connect_local("net-event", false, move |args| {
            let name = args[1].get::<String>().ok()?;
            let this = HostNets(weak.upgrade()?);
            this.spawn(|t| async move { t.net_event(&name).await });
            None
        });
        self.conn_handlers.borrow_mut().push(id);
    }

    ///////////////////////
    // Standard UI methods //
    ///////////////////////

    pub fn close(&self) {
        if let Some(a) = self.addnet.borrow().as_ref() {
            a.close();
        }
    }

    /// `_cleanup`.
    pub fn cleanup(&self) {
        self.close();
        self.addnet.borrow_mut().take();
        for id in self.conn_handlers.take() {
            self.conn.disconnect(id);
        }
    }

    fn parent(&self) -> Option<gtk::Window> {
        self.top_box.root().and_downcast::<gtk::Window>()
    }

    //////////////
    // Public API //
    //////////////

    /// `refresh_page`: upstream's `schedule_priority_tick(pollnet=True)`,
    /// here a refetch of the list.
    pub fn refresh_page(&self) {
        // A priority tick does nothing for an inactive connection
        if !self.conn.is_active() {
            return;
        }
        self.spawn(|t| async move {
            t.populate_networks().await;
        });
    }

    /////////////////
    // UI populating //
    /////////////////

    /// `_refresh_conn_state`. The error pages for an inactive
    /// connection, or one without network support, are set by
    /// `apply_networks`, for whichever fetch lands last.
    fn refresh_conn_state(&self) {
        self.net_add.set_sensitive(self.conn.is_active());
        self.spawn(|t| async move { t.populate_networks().await });
    }

    fn current_network(&self) -> Option<NetRow> {
        let idx = self.net_list.selected_row()?.index();
        self.nets.borrow().get(idx as usize).cloned()
    }

    /// `_set_error_page`.
    fn set_error_page(&self, msg: &str) {
        self.network_pages.set_visible_child_name("error");
        self.network_error_label.set_text(msg);
        self.net_start.set_sensitive(false);
        self.net_stop.set_sensitive(false);
        self.net_delete.set_sensitive(false);
        self.disable_net_apply();
    }

    /// `_refresh_current_network`.
    fn refresh_current_network(&self) {
        let net = self.current_network();
        *self.shown_net.borrow_mut() = net.clone();
        let Some(net) = net else {
            self.set_error_page("No virtual network selected.");
            return;
        };

        self.network_pages.set_visible_child_name("details");
        if let Err(e) = self.populate_net_state(&net) {
            eprintln!("{e}");
            self.set_error_page(&format!("Error selecting network: {e}"));
        }
        self.disable_net_apply();
    }

    /// `_populate_networks`: refetches the networks, keeping the
    /// selection.
    async fn populate_networks(&self) {
        let gen = self.generation.get() + 1;
        self.generation.set(gen);
        let fetched = fetch_nets(&self.conn).await;
        // A newer fetch owns the list (and the error page) now
        if self.generation.get() != gen {
            return;
        }
        self.apply_networks(fetched);
    }

    /// The rest of `_populate_networks`, with the fetched list: `None`
    /// when the connection can't list networks.
    fn apply_networks(&self, fetched: Option<Vec<NetRow>>) {
        let supported = fetched.is_some();
        let mut nets = fetched.unwrap_or_default();
        // The model's sort, by `g_utf8_collate`
        nets.sort_by_cached_key(|n| glib::CollationKey::from(&n.name));
        let curnet = self.current_network().map(|n| n.uuid);

        // Prevent events while the list is rebuilt
        self.quiet.set(true);
        self.net_list.remove_all();
        for net in &nets {
            self.net_list.append(&net_row_widget(net));
        }
        *self.nets.borrow_mut() = nets;

        // `uiutil.set_list_selection`: the current network, else the
        // first one
        let pos = curnet
            .and_then(|uuid| self.nets.borrow().iter().position(|n| n.uuid == uuid))
            .or_else(|| (!self.nets.borrow().is_empty()).then_some(0));
        if let Some(row) = pos.and_then(|p| self.net_list.row_at_index(p as i32)) {
            self.net_list.select_row(Some(&row));
        }
        self.quiet.set(false);

        // `_refresh_conn_state`'s pages, set after the list is filled
        // and kept until the connection comes back
        if !self.conn.is_active() {
            self.set_error_page("Connection not active.");
        } else if !supported {
            self.net_add.set_sensitive(false);
            self.set_error_page("Libvirt connection does not support virtual network management.");
        } else {
            self.refresh_current_network();
        }
    }

    /// A `net-event` for `name`. Upstream recaches that network, whose
    /// `state-changed` updates its row and, when it's the one shown,
    /// the details (`_net_state_changed_cb`); or it polls, and the
    /// `net-added`/`net-removed` that follow repopulate the list
    /// (`_conn_nets_changed_cb`).
    async fn net_event(&self, name: &str) {
        let gen = self.generation.get() + 1;
        self.generation.set(gen);
        let fetched = fetch_nets(&self.conn).await;
        if self.generation.get() != gen {
            return;
        }
        let Some(mut nets) = fetched else {
            self.apply_networks(None);
            return;
        };
        nets.sort_by_cached_key(|n| glib::CollationKey::from(&n.name));
        let same_list = {
            let old = self.nets.borrow();
            old.len() == nets.len() && old.iter().zip(&nets).all(|(a, b)| a.uuid == b.uuid)
        };
        if !same_list {
            self.apply_networks(Some(nets));
            return;
        }

        // Update net state inline in the list
        for (i, net) in nets.iter().enumerate() {
            if let Some(row) = self.net_list.row_at_index(i as i32) {
                row.set_child(Some(&net_row_widget(net)));
            }
        }
        *self.nets.borrow_mut() = nets;
        // If refreshed network is the current net, refresh the UI
        if self.current_network().is_some_and(|n| n.name == name) {
            self.refresh_current_network();
        }
    }

    /// `_populate_net_ipv4_state`.
    fn populate_net_ipv4_state(&self, net: &NetRow) -> Result<(), String> {
        let summary = net.xmlobj.get_ipv4_network()?;
        let netstr = summary.network.filter(|s| !s.is_empty());

        self.net_ipv4_expander.set_visible(netstr.is_some());
        let Some(netstr) = netstr else {
            return Ok(());
        };

        self.net_ipv4_forwarding
            .set_text(&net.xmlobj.pretty_forward_mode());

        let dhcpstr = match summary.dhcp {
            Some((start, end)) => format!("{start} - {end}"),
            None => "Disabled".into(),
        };
        self.net_ipv4_dhcp_range.set_text(&dhcpstr);
        self.net_ipv4_network.set_text(&netstr);
        Ok(())
    }

    /// `_populate_net_ipv6_state`.
    fn populate_net_ipv6_state(&self, net: &NetRow) -> Result<(), String> {
        let summary = net.xmlobj.get_ipv6_network()?;
        let netstr = summary.network.filter(|s| !s.is_empty());

        self.net_ipv6_expander.set_visible(netstr.is_some());

        let prettymode = if netstr.is_some() {
            "Routed network"
        } else if net.xmlobj.get_ipv6_enabled() {
            "Isolated network, internal routing only"
        } else {
            "Isolated network, routing disabled"
        };
        self.net_ipv6_forwarding.set_text(prettymode);

        let dhcpstr = match summary.dhcp {
            Some((start, end)) => format!("{start} - {end}"),
            None => "Disabled".into(),
        };
        self.net_ipv6_dhcp_range.set_text(&dhcpstr);
        self.net_ipv6_network
            .set_text(netstr.as_deref().unwrap_or(""));
        Ok(())
    }

    /// `_populate_net_state`.
    fn populate_net_state(&self, net: &NetRow) -> Result<(), String> {
        let active = net.active;

        self.quiet.set(true);
        self.net_details.set_sensitive(true);
        self.net_name.set_text(&net.name);
        self.net_name.set_editable(!active);
        self.net_device
            .set_text(net.xmlobj.bridge.as_deref().unwrap_or(""));
        let domain = net.xmlobj.domain_name.as_deref().unwrap_or("");
        self.net_name_domain.set_text(domain);
        set_grid_row_visible(&self.net_name_domain, !domain.is_empty());

        let icon = if active { ICON_RUNNING } else { ICON_SHUTOFF };
        self.net_state.set_text(net.run_status());
        self.net_state_icon.set_icon_name(Some(icon));

        self.net_start.set_sensitive(!active);
        self.net_stop.set_sensitive(active);
        self.net_delete.set_sensitive(!active);

        self.net_autostart.set_active(net.autostart);
        self.net_autostart.set_label(Some("On Boot"));
        self.quiet.set(false);

        self.populate_net_ipv4_state(net)?;
        self.populate_net_ipv6_state(net)?;

        // `set_xml_from_libvirtobject`: only while the XML tab shows
        if self.xmleditor.is_xml_selected() {
            let xml = network::get_xml_to_define(&net.xml)?;
            self.xmleditor.set_xml(&xml);
        }
        Ok(())
    }

    /////////////////////////////
    // Network lifecycle actions //
    /////////////////////////////

    async fn lifecycle_action(
        &self,
        net: &NetRow,
        errorintro: &str,
        f: impl FnOnce(&virt::connect::Connect, &VirNetwork) -> Result<(), String> + Send + 'static,
    ) {
        // `vmmAsyncJob.simple_async_noshow`
        if let Err(e) = with_net(&self.conn, &net.uuid, f).await {
            error::show_err(
                self.parent().as_ref(),
                &format!("{errorintro}: {e}"),
                Some(&e),
                "",
            )
            .await;
        }
        // `lifecycle_action`'s `schedule_priority_tick(pollnet=True)`
        self.populate_networks().await;
    }

    /// `_delete_network_cb`.
    async fn delete_network_clicked(&self) {
        let Some(net) = self.current_network() else {
            return;
        };
        let msg = format!(
            "Are you sure you want to permanently delete the network {}?",
            net.name
        );
        if !error::yes_no(self.parent().as_ref(), &msg, None).await {
            return;
        }
        eprintln!("Deleting network '{}'", net.name);
        let intro = format!("Error deleting network '{}'", net.name);
        self.lifecycle_action(&net, &intro, |_, n| n.undefine().map_err(libvirt_err))
            .await;
    }

    /// `_start_network_cb`.
    async fn start_network_clicked(&self) {
        let Some(net) = self.current_network() else {
            return;
        };
        eprintln!("Starting network '{}'", net.name);
        let intro = format!("Error starting network '{}'", net.name);
        self.lifecycle_action(&net, &intro, |_, n| {
            n.create().map(|_| ()).map_err(libvirt_err)
        })
        .await;
    }

    /// `_stop_network_cb`.
    async fn stop_network_clicked(&self) {
        let Some(net) = self.current_network() else {
            return;
        };
        eprintln!("Stopping network '{}'", net.name);
        let intro = format!("Error stopping network '{}'", net.name);
        self.lifecycle_action(&net, &intro, |_, n| n.destroy().map_err(libvirt_err))
            .await;
    }

    /// `_add_network_cb`.
    fn add_network_clicked(&self) {
        eprintln!("Launching 'Add Network'");
        let addnet = self.addnet.borrow().clone();
        let addnet = addnet.unwrap_or_else(|| {
            let a = CreateNetwork::new(&self.conn);
            let weak = self.weak();
            a.connect_network_created(move || {
                if let Some(inner) = weak.upgrade() {
                    HostNets(inner).refresh_page();
                }
            });
            *self.addnet.borrow_mut() = Some(a.clone());
            a
        });
        if let Some(parent) = self.parent() {
            addnet.show(&parent);
        }
    }

    //////////////////////////////
    // Net apply/config actions //
    //////////////////////////////

    /// `_net_apply`, for the network the details show.
    async fn net_apply(&self) {
        let Some(net) = self.shown_net.borrow().clone() else {
            return;
        };
        eprintln!("Applying changes for network '{}'", net.name);
        let edits = self.active_edits.borrow().clone();
        let auto = self.net_autostart.is_active();
        let newname = self.net_name.text().to_string();
        let newxml = self.xmleditor.get_xml();
        let res = with_net(&self.conn, &net.uuid, move |conn, n| {
            if edits.contains(&NetEdit::Autostart) {
                n.set_autostart(auto).map_err(libvirt_err)?;
            }
            if edits.contains(&NetEdit::Name) {
                network::rename(conn, n, &newname)?;
            }
            if edits.contains(&NetEdit::Xml) {
                // `define_xml`
                VirNetwork::define_xml(conn, &newxml).map_err(libvirt_err)?;
            }
            Ok(())
        })
        .await;
        // finally: `_disable_net_apply`
        self.disable_net_apply();
        if let Err(e) = res {
            error::show_err(
                self.parent().as_ref(),
                &format!("Error changing network settings: {e}"),
                None,
                "",
            )
            .await;
            return;
        }
        // A renamed network keeps its UUID, so the selection stays put:
        // on it after Apply, on the network just clicked after
        // `_confirm_changes`
        self.populate_networks().await;
    }

    fn disable_net_apply(&self) {
        self.active_edits.borrow_mut().clear();
        self.net_apply.set_sensitive(false);
        self.xmleditor.set_details_changed(false);
    }

    fn enable_net_apply(&self, edittype: NetEdit) {
        if self.quiet.get() {
            return;
        }
        self.net_apply.set_sensitive(true);
        let mut edits = self.active_edits.borrow_mut();
        if !edits.contains(&edittype) {
            edits.push(edittype);
        }
        self.xmleditor.set_details_changed(true);
    }

    /// `_confirm_changes`: the selection function, asked before the
    /// network being left is let go.
    async fn confirm_changes(&self) {
        let visible = self.top_box.is_mapped();
        if visible
            && !self.active_edits.borrow().is_empty()
            && error::confirm_unapplied_changes(self.parent().as_ref()).await
        {
            self.net_apply().await;
        }
        self.disable_net_apply();
    }

    ////////////////
    // UI listeners //
    ////////////////

    /// `_net_selected_cb`, after the select function's
    /// `_confirm_changes` for the network being left.
    fn net_selected(&self) {
        if self.quiet.get() {
            return;
        }
        if self.active_edits.borrow().is_empty() {
            self.refresh_current_network();
            return;
        }
        self.spawn(|t| async move {
            t.confirm_changes().await;
            t.refresh_current_network();
        });
    }
}

/// A `net-list` row: the `network-idle` icon at large-toolbar size and
/// the name, greyed out when inactive.
fn net_row_widget(net: &NetRow) -> gtk::Widget {
    let img = gtk::Image::from_icon_name("network-idle");
    img.set_pixel_size(24);
    let label = gtk::Label::new(Some(&net.name));
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_hexpand(true);
    label.set_sensitive(net.active);
    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    hbox.set_margin_start(2);
    hbox.set_margin_end(2);
    hbox.set_margin_top(2);
    hbox.set_margin_bottom(2);
    hbox.append(&img);
    hbox.append(&label);
    hbox.upcast()
}
