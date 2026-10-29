//! `createnet.py` + `ui/createnet.ui` (`vmmCreateNetwork`): the "Create
//! a new virtual network" dialog — name, forward mode and device,
//! SR-IOV device list, IPv4/IPv6 addresses with their DHCP ranges, the
//! DNS domain name, and the XML tab. Building the `<network>` from the
//! fields is `virtinst_core::network::build_network`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use virtinst_core::network::{self, NetworkConfig, FORWARD_MODES};

use crate::asyncjob;
use crate::connection::VmmConnection;
use crate::error;
use crate::xmleditor::XmlEditor;

/// `uiutil.set_grid_row_visible`: show or hide every child of the
/// `GtkGrid` holding `child` that sits in the same row.
pub(crate) fn set_grid_row_visible(child: &impl IsA<gtk::Widget>, visible: bool) {
    let Some(grid) = child.parent().and_downcast::<gtk::Grid>() else {
        panic!("parent must be grid");
    };
    let row = grid.query_child(child).1;
    let mut c = grid.first_child();
    while let Some(w) = c {
        if grid.query_child(&w).1 == row {
            w.set_visible(visible);
        }
        c = w.next_sibling();
    }
}

fn end_label(text: &str) -> gtk::Label {
    let l = gtk::Label::with_mnemonic(text);
    l.set_halign(gtk::Align::End);
    l
}

/// A `GtkExpander` with a bold `<b>` markup label, as the .ui has them.
fn expander(markup: &str, child: &impl IsA<gtk::Widget>) -> gtk::Expander {
    let e = gtk::Expander::with_mnemonic(markup);
    e.set_use_markup(true);
    e.set_child(Some(child));
    e
}

/// A `GtkComboBox` with a text column: a drop-down over labels, with
/// the label ellipsized in the middle when `ellipsize` is set.
fn combo(labels: &[&str], ellipsize: bool) -> gtk::DropDown {
    let d = gtk::DropDown::from_strings(labels);
    if ellipsize {
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
            let l = gtk::Label::new(None);
            l.set_xalign(0.0);
            l.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            item.set_child(Some(&l));
        });
        factory.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
            if let (Some(l), Some(s)) = (
                item.child().and_downcast::<gtk::Label>(),
                item.item().and_downcast::<gtk::StringObject>(),
            ) {
                l.set_text(&s.string());
            }
        });
        d.set_factory(Some(&factory));
    }
    d
}

pub struct Inner {
    conn: VmmConnection,
    window: gtk::Window,
    xmleditor: XmlEditor,
    net_name: gtk::Entry,
    net_forward_mode: gtk::DropDown,
    net_forward_device: gtk::DropDown,
    net_forward_manual: gtk::Entry,
    net_hostdevs: gtk::DropDown,
    /// `net-hostdevs`' model: `[dev name, label]`.
    hostdev_values: RefCell<Vec<Option<String>>>,
    net_ipv4_expander: gtk::Expander,
    net_ipv4_enable: gtk::CheckButton,
    net_ipv4_box: gtk::Box,
    net_ipv4_network: gtk::Entry,
    net_dhcpv4_enable: gtk::CheckButton,
    net_dhcpv4_start: gtk::Entry,
    net_dhcpv4_end: gtk::Entry,
    net_ipv6_expander: gtk::Expander,
    net_ipv6_enable: gtk::CheckButton,
    net_ipv6_box: gtk::Box,
    net_ipv6_network: gtk::Entry,
    net_dhcpv6_enable: gtk::CheckButton,
    net_dhcpv6_start: gtk::Entry,
    net_dhcpv6_end: gtk::Entry,
    net_dns_expander: gtk::Expander,
    net_dns_use_netname: gtk::CheckButton,
    net_dns_use_custom: gtk::CheckButton,
    net_domain_name: gtk::Entry,
    /// Bumped per `reset_state`, so a stale name lookup is dropped.
    reset_generation: Cell<u64>,
    /// Set while `net-name` is filled in by `reset_state`.
    setting_name: Cell<bool>,
    /// The user typed in `net-name` since `reset_state`.
    name_edited: Cell<bool>,
    net_created: RefCell<Option<Box<dyn Fn()>>>,
}

#[derive(Clone)]
pub struct CreateNetwork(Rc<Inner>);

impl CreateNetwork {
    pub fn new(conn: &VmmConnection) -> CreateNetwork {
        let window = gtk::Window::builder()
            .title("Create a new virtual network")
            .hide_on_close(true)
            .build();
        window.set_size_request(400, 400);

        // header
        let icon = gtk::Image::from_icon_name("network-idle");
        icon.set_pixel_size(48);
        let header_label = gtk::Label::new(None);
        header_label.set_markup("<span size='large'>Create virtual network</span>");
        header_label.set_halign(gtk::Align::Start);
        header_label.add_css_class("vmm-header-text");
        let vbox3 = gtk::Box::new(gtk::Orientation::Vertical, 0);
        vbox3.set_hexpand(true);
        vbox3.append(&header_label);
        let box77 = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        box77.set_margin_start(6);
        box77.set_margin_end(6);
        box77.set_margin_top(6);
        box77.set_margin_bottom(6);
        box77.append(&icon);
        box77.append(&vbox3);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        header.add_css_class("vmm-header");
        header.append(&box77);

        // The top grid: name, mode, forward device, manual device, SR-IOV
        // device list
        let net_name = gtk::Entry::new();
        net_name.set_width_chars(25);
        net_name.update_property(&[gtk::accessible::Property::Label("Net Name Field")]);
        let mode_labels: Vec<&str> = FORWARD_MODES.iter().map(|(_, l)| *l).collect();
        let net_forward_mode = combo(&mode_labels, false);
        net_forward_mode.set_halign(gtk::Align::Start);
        net_forward_mode.update_property(&[gtk::accessible::Property::Label("net-mode")]);
        // [ show_manual, label ]
        let net_forward_device = combo(&["Any physical device", "Physical device..."], false);
        net_forward_device.update_property(&[gtk::accessible::Property::Label("net-forward")]);
        let net_forward_manual = gtk::Entry::new();
        net_forward_manual.update_property(&[gtk::accessible::Property::Label("net-device")]);
        let net_hostdevs = combo(&[], true);
        net_hostdevs.set_size_request(200, -1);
        net_hostdevs.set_halign(gtk::Align::Start);
        net_hostdevs.update_property(&[gtk::accessible::Property::Label("net-devicelist")]);

        let name_label = end_label("_Name:");
        name_label.set_mnemonic_widget(Some(&net_name));
        let forward_label = end_label("Fo_rward to:");
        forward_label.set_mnemonic_widget(Some(&net_forward_device));
        let manual_label = end_label("De_vice:");
        manual_label.set_mnemonic_widget(Some(&net_forward_manual));
        let hostdevs_label = end_label("Device _List:");
        hostdevs_label.set_mnemonic_widget(Some(&net_forward_device));
        let grid = gtk::Grid::new();
        grid.set_row_spacing(6);
        grid.set_column_spacing(6);
        grid.attach(&name_label, 0, 0, 1, 1);
        grid.attach(&net_name, 1, 0, 1, 1);
        grid.attach(&end_label("_Mode:"), 0, 1, 1, 1);
        grid.attach(&net_forward_mode, 1, 1, 1, 1);
        grid.attach(&forward_label, 0, 2, 1, 1);
        grid.attach(&net_forward_device, 1, 2, 1, 1);
        grid.attach(&manual_label, 0, 3, 1, 1);
        grid.attach(&net_forward_manual, 1, 3, 1, 1);
        grid.attach(&hostdevs_label, 0, 4, 1, 1);
        grid.attach(&net_hostdevs, 1, 4, 1, 1);

        // IPv4 and IPv6 configuration
        let ip_section = |enable_label: &str,
                          dhcp_label: &str,
                          max_length: i32,
                          dhcp_max_length: i32,
                          end_text_start: bool,
                          names: [&str; 3]| {
            let enable = gtk::CheckButton::with_mnemonic(enable_label);
            enable.set_halign(gtk::Align::Start);
            enable.set_active(true);
            let network = gtk::Entry::new();
            network.set_halign(gtk::Align::Start);
            network.set_max_length(max_length);
            network.set_width_chars(25);
            network.update_property(&[gtk::accessible::Property::Label(names[0])]);
            let box6 = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            box6.append(&end_label("_Network:"));
            box6.append(&network);
            let box5 = gtk::Box::new(gtk::Orientation::Vertical, 3);
            box5.append(&box6);

            let dhcp_enable = gtk::CheckButton::with_label(dhcp_label);
            dhcp_enable.set_halign(gtk::Align::Start);
            dhcp_enable.set_active(true);
            let start = gtk::Entry::new();
            start.set_max_length(dhcp_max_length);
            start.set_width_chars(25);
            start.update_property(&[gtk::accessible::Property::Label(names[1])]);
            let end = gtk::Entry::new();
            end.set_max_length(dhcp_max_length);
            end.update_property(&[gtk::accessible::Property::Label(names[2])]);
            let end_label_w = gtk::Label::new(Some("End:"));
            end_label_w.set_halign(if end_text_start {
                gtk::Align::Start
            } else {
                gtk::Align::End
            });
            let table = gtk::Grid::new();
            table.set_row_spacing(3);
            table.set_column_spacing(6);
            table.attach(&dhcp_enable, 0, 0, 2, 1);
            table.attach(&end_label("Start:"), 0, 1, 1, 1);
            table.attach(&start, 1, 1, 1, 1);
            table.attach(&end_label_w, 0, 2, 1, 1);
            table.attach(&end, 1, 2, 1, 1);

            let ip_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
            ip_box.set_margin_start(15);
            ip_box.append(&box5);
            ip_box.append(&table);
            let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
            content.append(&enable);
            content.append(&ip_box);
            (content, enable, ip_box, network, dhcp_enable, start, end)
        };
        let (
            ipv4_content,
            net_ipv4_enable,
            net_ipv4_box,
            net_ipv4_network,
            net_dhcpv4_enable,
            net_dhcpv4_start,
            net_dhcpv4_end,
        ) = ip_section(
            "_Enable IPv4",
            "Enable DHCPv4",
            20,
            15,
            false,
            ["ipv4-network", "ipv4-start", "ipv4-end"],
        );
        let net_ipv4_expander = expander("<b>IPv_4 configuration</b>", &ipv4_content);
        let (
            ipv6_content,
            net_ipv6_enable,
            net_ipv6_box,
            net_ipv6_network,
            net_dhcpv6_enable,
            net_dhcpv6_start,
            net_dhcpv6_end,
        ) = ip_section(
            "_Enable IPv6",
            "Enable DHCPv6",
            40,
            40,
            true,
            ["ipv6-network", "ipv6-start", "ipv6-end"],
        );
        let net_ipv6_expander = expander("<b>IPv_6 configuration</b>", &ipv6_content);

        // DNS domain name
        let net_dns_use_netname = gtk::CheckButton::with_mnemonic("Use net_work name");
        net_dns_use_netname.set_halign(gtk::Align::Start);
        let net_dns_use_custom = gtk::CheckButton::with_mnemonic("Cust_om");
        net_dns_use_netname.set_group(Some(&net_dns_use_custom));
        net_dns_use_netname.set_active(true);
        let net_domain_name = gtk::Entry::new();
        net_domain_name.set_width_chars(20);
        net_domain_name.update_property(&[gtk::accessible::Property::Label("domain-custom")]);
        let dns_grid = gtk::Grid::new();
        dns_grid.set_margin_start(15);
        dns_grid.set_row_spacing(6);
        dns_grid.set_column_spacing(6);
        dns_grid.attach(&net_dns_use_netname, 0, 0, 2, 1);
        dns_grid.attach(&net_dns_use_custom, 0, 1, 1, 1);
        dns_grid.attach(&net_domain_name, 1, 1, 1, 1);
        let net_dns_expander = gtk::Expander::new(None);
        let dns_label = gtk::Label::new(None);
        dns_label.set_markup("<b>DNS domain name</b>");
        net_dns_expander.set_label_widget(Some(&dns_label));
        net_dns_expander.set_child(Some(&dns_grid));

        let net_details = gtk::Box::new(gtk::Orientation::Vertical, 12);
        net_details.append(&grid);
        net_details.append(&net_ipv4_expander);
        net_details.append(&net_ipv6_expander);
        net_details.append(&net_dns_expander);
        let xmleditor = XmlEditor::new(&net_details);
        xmleditor.widget().set_vexpand(true);

        let create_cancel = gtk::Button::with_mnemonic("_Cancel");
        let create_finish = gtk::Button::with_mnemonic("_Finish");
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        buttons.set_halign(gtk::Align::End);
        buttons.append(&create_cancel);
        buttons.append(&create_finish);

        let vbox23 = gtk::Box::new(gtk::Orientation::Vertical, 12);
        vbox23.set_margin_start(12);
        vbox23.set_margin_end(12);
        vbox23.set_margin_top(12);
        vbox23.set_margin_bottom(12);
        vbox23.set_vexpand(true);
        vbox23.append(xmleditor.widget());
        vbox23.append(&buttons);
        let box1 = gtk::Box::new(gtk::Orientation::Vertical, 6);
        box1.append(&header);
        box1.append(&vbox23);
        window.set_child(Some(&box1));

        let this = CreateNetwork(Rc::new(Inner {
            conn: conn.clone(),
            window,
            xmleditor,
            net_name,
            net_forward_mode,
            net_forward_device,
            net_forward_manual,
            net_hostdevs,
            hostdev_values: RefCell::default(),
            net_ipv4_expander,
            net_ipv4_enable,
            net_ipv4_box,
            net_ipv4_network,
            net_dhcpv4_enable,
            net_dhcpv4_start,
            net_dhcpv4_end,
            net_ipv6_expander,
            net_ipv6_enable,
            net_ipv6_box,
            net_ipv6_network,
            net_dhcpv6_enable,
            net_dhcpv6_start,
            net_dhcpv6_end,
            net_dns_expander,
            net_dns_use_netname,
            net_dns_use_custom,
            net_domain_name,
            reset_generation: Cell::new(0),
            setting_name: Cell::new(false),
            name_edited: Cell::new(false),
            net_created: RefCell::default(),
        }));
        this.connect_signals(&create_cancel, &create_finish);
        this
    }

    fn connect_signals(&self, cancel: &gtk::Button, finish: &gtk::Button) {
        let i = &self.0;
        let weak = Rc::downgrade(&self.0);
        let with = move |f: fn(&CreateNetwork)| {
            let weak = weak.clone();
            move || {
                if let Some(inner) = weak.upgrade() {
                    f(&CreateNetwork(inner));
                }
            }
        };
        let cb = with(CreateNetwork::close);
        cancel.connect_clicked(move |_| cb());
        let cb = with(|t| {
            let t = t.clone();
            glib::spawn_future_local(async move { t.finish().await });
        });
        finish.connect_clicked(move |_| cb());
        let cb = with(|t| {
            if !t.0.setting_name.get() {
                t.0.name_edited.set(true);
            }
        });
        i.net_name.connect_changed(move |_| cb());
        let cb = with(CreateNetwork::net_forward_mode_changed);
        i.net_forward_mode.connect_selected_notify(move |_| cb());
        let cb = with(CreateNetwork::net_forward_device_changed);
        i.net_forward_device.connect_selected_notify(move |_| cb());
        let cb = with(CreateNetwork::net_dns_use_toggled);
        i.net_dns_use_custom.connect_toggled(move |_| cb());
        let cb = with(CreateNetwork::change_ipv4_enable);
        i.net_ipv4_enable.connect_toggled(move |_| cb());
        let cb = with(CreateNetwork::change_ipv4_network);
        i.net_ipv4_network.connect_changed(move |_| cb());
        let cb = with(CreateNetwork::change_dhcpv4_enable);
        i.net_dhcpv4_enable.connect_toggled(move |_| cb());
        let cb = with(CreateNetwork::change_ipv6_enable);
        i.net_ipv6_enable.connect_toggled(move |_| cb());
        let cb = with(CreateNetwork::change_ipv6_network);
        i.net_ipv6_network.connect_changed(move |_| cb());
        let cb = with(CreateNetwork::change_dhcpv6_enable);
        i.net_dhcpv6_enable.connect_toggled(move |_| cb());
        let cb = with(CreateNetwork::xmleditor_xml_requested);
        i.xmleditor.connect_xml_requested(cb);

        // `bind_escape_key_close`
        let esc = gtk::EventControllerKey::new();
        let cb = with(CreateNetwork::close);
        esc.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                cb();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        i.window.add_controller(esc);
    }

    ////////////////////
    // Standard methods //
    ////////////////////

    /// `show`.
    pub fn show(&self, parent: &gtk::Window) {
        eprintln!("Showing new network wizard");
        self.reset_state();
        self.0.window.set_transient_for(Some(parent));
        self.0.window.present();
    }

    /// `close`.
    pub fn close(&self) {
        eprintln!("Closing new network wizard");
        self.0.window.set_visible(false);
    }

    /// Called once a new network is defined and started (upstream's
    /// `schedule_priority_tick(pollnet=True)`, which the list hears as
    /// `net-added`).
    pub fn connect_network_created(&self, cb: impl Fn() + 'static) {
        *self.0.net_created.borrow_mut() = Some(Box::new(cb));
    }

    ///////////
    // UI init //
    ///////////

    /// Fills `net-name` in without counting it as the user's edit.
    fn set_name_text(&self, text: &str) {
        self.0.setting_name.set(true);
        self.0.net_name.set_text(text);
        self.0.setting_name.set(false);
    }

    /// `reset_state`.
    fn reset_state(&self) {
        let i = &self.0;
        i.xmleditor.reset_state();

        let gen = i.reset_generation.get() + 1;
        i.reset_generation.set(gen);
        // `generatename.generate_name("network", ...)`, on a worker: the
        // free name replaces the placeholder unless the user has typed
        // a name in the meantime
        self.set_name_text("network");
        i.name_edited.set(false);
        if let Some(handle) = i.conn.handle() {
            let this = self.clone();
            glib::spawn_future_local(async move {
                let name = gio::spawn_blocking(move || {
                    network::find_free_name(handle.connect(), "network")
                })
                .await
                .unwrap_or_else(|_| "network".into());
                if this.0.reset_generation.get() == gen && !this.0.name_edited.get() {
                    this.set_name_text(&name);
                }
            });
        }

        i.net_dns_use_netname.set_active(true);

        i.net_ipv4_expander.set_visible(true);
        i.net_ipv4_expander.set_expanded(false);
        i.net_ipv6_expander.set_visible(true);
        i.net_ipv6_expander.set_expanded(false);
        i.net_dns_expander.set_visible(true);
        i.net_dns_expander.set_expanded(false);

        i.net_ipv4_enable.set_active(true);
        i.net_ipv4_network.set_text("192.168.100.0/24");
        i.net_dhcpv4_enable.set_active(true);
        i.net_dhcpv4_start.set_text("192.168.100.128");
        i.net_dhcpv4_end.set_text("192.168.100.254");

        i.net_ipv6_enable.set_active(false);
        i.net_ipv6_network.set_text("");
        i.net_dhcpv6_enable.set_active(false);
        i.net_dhcpv6_start.set_text("");
        i.net_dhcpv6_end.set_text("");

        // A GTK3 combo starts with nothing active, so these set_active(0)
        // calls ran their changed handlers; a drop-down starts at 0.
        i.net_forward_device.set_selected(0);
        i.net_forward_mode.set_selected(0);
        i.net_forward_manual.set_text("");
        // The toggles' own handlers only run on a change; the widgets
        // start out the way the .ui has them.
        self.change_ipv4_enable();
        self.change_dhcpv4_enable();
        self.change_ipv6_enable();
        self.change_dhcpv6_enable();
        self.net_forward_mode_changed();

        // Populate hostdev forward devices
        self.set_hostdevs(vec![]);
        let this = self.clone();
        glib::spawn_future_local(async move { this.populate_hostdevs(gen).await });
    }

    fn set_hostdevs(&self, mut rows: Vec<(Option<String>, String)>) {
        if rows.is_empty() {
            rows.push((None, "No available device".into()));
        }
        let labels: Vec<&str> = rows.iter().map(|(_, l)| l.as_str()).collect();
        self.0
            .net_hostdevs
            .set_model(Some(&gtk::StringList::new(&labels)));
        self.0
            .hostdev_values
            .replace(rows.into_iter().map(|(v, _)| v).collect());
        self.0.net_hostdevs.set_selected(0);
    }

    /// `reset_state`'s SR-IOV list: each PCI device with virtual
    /// functions, by its first network interface.
    async fn populate_hostdevs(&self, gen: u64) {
        let all = self.0.conn.list_nodedevs().await;
        if self.0.reset_generation.get() != gen {
            return;
        }
        let mut rows = vec![];
        for pcidev in all
            .iter()
            .filter(|d| d.device_type.as_deref() == Some("pci"))
        {
            // `is_pci_sriov`
            if pcidev.capability_type.as_deref() != Some("virt_functions") {
                continue;
            }
            let devdesc = virtinst_core::nodedev::pretty_name(pcidev, &all);
            let netdev = all.iter().find(|n| {
                n.device_type.as_deref() == Some("net")
                    && n.parent.as_deref() == Some(pcidev.name.as_str())
            });
            if let Some(netdev) = netdev {
                let ifname = netdev.interface.clone().unwrap_or_default();
                rows.push((Some(ifname.clone()), format!("{ifname} ({devdesc})")));
            }
        }
        self.set_hostdevs(rows);
    }

    ////////////////////
    // UI get helpers //
    ////////////////////

    /// `widget.is_visible()` for a widget inside `expander`: GTK4 takes
    /// a collapsed expander's child out of the widget tree, so the
    /// expander is asked as well.
    fn shown_in(widget: &impl IsA<gtk::Widget>, expander: &gtk::Expander) -> bool {
        expander.is_visible() && widget.is_visible()
    }

    fn get_config_ipv4_enable(&self) -> bool {
        self.0.net_ipv4_expander.is_visible() && self.0.net_ipv4_enable.is_active()
    }

    fn get_config_ipv6_enable(&self) -> bool {
        self.0.net_ipv6_expander.is_visible() && self.0.net_ipv6_enable.is_active()
    }

    fn get_config_dhcpv4_enable(&self) -> bool {
        self.0.net_dhcpv4_enable.is_active()
    }

    fn get_config_dhcpv6_enable(&self) -> bool {
        self.0.net_dhcpv6_enable.is_active()
    }

    /// `get_config_domain_name`.
    fn get_config_domain_name(&self) -> Option<String> {
        let widget = &self.0.net_domain_name;
        if !Self::shown_in(widget, &self.0.net_dns_expander) {
            return None;
        }
        if self.0.net_dns_use_netname.is_active() {
            return Some(self.0.net_name.text().to_string());
        }
        Some(widget.text().to_string())
    }

    /// `_get_network_helper`.
    fn get_network_helper(
        widget: &gtk::Entry,
        expander: &gtk::Expander,
    ) -> Option<network::IpNetwork> {
        if !Self::shown_in(widget, expander) || !widget.is_sensitive() {
            return None;
        }
        network::make_ipaddr(&widget.text())
    }

    fn get_config_ip4(&self) -> Option<network::IpNetwork> {
        Self::get_network_helper(&self.0.net_ipv4_network, &self.0.net_ipv4_expander)
    }

    fn get_config_ip6(&self) -> Option<network::IpNetwork> {
        Self::get_network_helper(&self.0.net_ipv6_network, &self.0.net_ipv6_expander)
    }

    /// `_get_config_net_forward_dev`.
    fn get_config_net_forward_dev(&self) -> Option<String> {
        if !self.0.net_forward_device.is_visible() {
            return None;
        }
        let manual = self.0.net_forward_device.selected() == 1;
        if !manual {
            return None;
        }
        Some(self.0.net_forward_manual.text().to_string())
    }

    fn forward_mode(&self) -> String {
        FORWARD_MODES
            .get(self.0.net_forward_mode.selected() as usize)
            .map(|(m, _)| m.to_string())
            .unwrap_or_default()
    }

    //////////////
    // Listeners //
    //////////////

    /// `_net_forward_mode_changed_cb`.
    fn net_forward_mode_changed(&self) {
        let i = &self.0;
        let mode = self.forward_mode();
        let fw_visible = !matches!(mode.as_str(), "open" | "isolated" | "hostdev");
        let is_hostdev = mode == "hostdev";

        set_grid_row_visible(&i.net_forward_device, fw_visible);
        self.net_forward_device_changed();
        set_grid_row_visible(&i.net_hostdevs, is_hostdev);

        i.net_ipv4_expander.set_visible(!is_hostdev);
        i.net_ipv6_expander.set_visible(!is_hostdev);
        i.net_dns_expander.set_visible(!is_hostdev);
    }

    /// `_net_forward_device_changed_cb`.
    fn net_forward_device_changed(&self) {
        let src = &self.0.net_forward_device;
        let mut manual = src.selected() == 1;
        if !src.is_visible() {
            manual = false;
        }
        set_grid_row_visible(&self.0.net_forward_manual, manual);
    }

    /// `_net_dns_use_toggled_cb`.
    fn net_dns_use_toggled(&self) {
        let custom = self.0.net_dns_use_custom.is_active();
        self.0.net_domain_name.set_sensitive(custom);
    }

    /// `change_ipv4_enable`.
    fn change_ipv4_enable(&self) {
        let enabled = self.0.net_ipv4_enable.is_active();
        self.0.net_ipv4_box.set_visible(enabled);
    }

    /// `change_ipv6_enable`.
    fn change_ipv6_enable(&self) {
        let enabled = self.0.net_ipv6_enable.is_active();
        self.0.net_ipv6_box.set_visible(enabled);
    }

    /// `change_dhcpv4_enable`.
    fn change_dhcpv4_enable(&self) {
        let enabled = self.get_config_dhcpv4_enable();
        set_grid_row_visible(&self.0.net_dhcpv4_start, enabled);
        set_grid_row_visible(&self.0.net_dhcpv4_end, enabled);
    }

    /// `change_dhcpv6_enable`.
    fn change_dhcpv6_enable(&self) {
        let enabled = self.get_config_dhcpv6_enable();
        set_grid_row_visible(&self.0.net_dhcpv6_start, enabled);
        set_grid_row_visible(&self.0.net_dhcpv6_end, enabled);
    }

    /// `_change_ipv4_network_cb`.
    fn change_ipv4_network(&self) {
        // No IP specified or invalid IP
        let Some(ip) = self.get_config_ip4() else {
            return;
        };
        let Some((start, end)) = network::dhcpv4_defaults(&ip) else {
            return;
        };
        self.0.net_dhcpv4_start.set_text(&start);
        if let Ok(end) = end {
            self.0.net_dhcpv4_end.set_text(&end);
        }
    }

    /// `_change_ipv6_network_cb`.
    fn change_ipv6_network(&self) {
        let Some(ip) = self.get_config_ip6() else {
            return;
        };
        let Some((start, end)) = network::dhcpv6_defaults(&ip) else {
            return;
        };
        self.0.net_dhcpv6_start.set_text(&start);
        if let Ok(end) = end {
            self.0.net_dhcpv6_end.set_text(&end);
        }
    }

    ///////////////////////////
    // XML build and install //
    ///////////////////////////

    /// The fields `_build_xmlobj_from_ui` reads.
    fn config_from_ui(&self) -> NetworkConfig {
        let i = &self.0;
        let helper = |w: &gtk::Entry, e: &gtk::Expander| Self::get_network_helper(w, e);
        NetworkConfig {
            name: i.net_name.text().to_string(),
            domain_name: self.get_config_domain_name(),
            forward_mode: self.forward_mode(),
            forward_dev: self.get_config_net_forward_dev(),
            hostdev: i
                .hostdev_values
                .borrow()
                .get(i.net_hostdevs.selected() as usize)
                .cloned()
                .flatten(),
            ipv4_enable: self.get_config_ipv4_enable(),
            ip4: self.get_config_ip4(),
            dhcpv4_enable: self.get_config_dhcpv4_enable(),
            dhcpv4_start: helper(&i.net_dhcpv4_start, &i.net_ipv4_expander),
            dhcpv4_end: helper(&i.net_dhcpv4_end, &i.net_ipv4_expander),
            ipv6_enable: self.get_config_ipv6_enable(),
            ip6: self.get_config_ip6(),
            dhcpv6_enable: self.get_config_dhcpv6_enable(),
            dhcpv6_start: helper(&i.net_dhcpv6_start, &i.net_ipv6_expander),
            dhcpv6_end: helper(&i.net_dhcpv6_end, &i.net_ipv6_expander),
        }
    }

    /// `_build_xmlobj`: `(name, xml)` of the network to create, from
    /// the fields, or the XML tab when it's showing and asked for.
    async fn build_xmlobj(&self, check_xmleditor: bool) -> Option<(String, String)> {
        let res = network::build_network(&self.config_from_ui()).and_then(|obj| {
            if check_xmleditor && self.0.xmleditor.is_xml_selected() {
                // `_build_xmlobj_from_xmleditor`
                let xml = self.0.xmleditor.get_xml();
                eprintln!("Using XML from xmleditor:\n{xml}");
                return network::parse_network(&xml);
            }
            Ok(obj)
        });
        match res {
            Ok(obj) => Some((obj.get_str("name").unwrap_or_default(), obj.get_xml())),
            Err(e) => {
                error::show_err(
                    Some(&self.0.window),
                    &format!("Error building XML: {e}"),
                    None,
                    "",
                )
                .await;
                None
            }
        }
    }

    /// `finish`.
    async fn finish(&self) {
        let i = &self.0;
        let Some((name, xml)) = self.build_xmlobj(true).await else {
            return;
        };
        let Some(handle) = i.conn.handle() else {
            return;
        };

        let name2 = name.clone();
        let validated = gio::spawn_blocking(move || network::validate(handle.connect(), &name2))
            .await
            .unwrap_or_else(|_| Err("Worker thread panicked".into()));
        if let Err(e) = validated {
            error::show_err(
                Some(&i.window),
                &format!("Error validating network: {e}"),
                None,
                "",
            )
            .await;
            return;
        }

        let Some(handle) = i.conn.handle() else {
            return;
        };
        // `set_finish_cursor`
        i.window.set_sensitive(false);
        i.window.set_cursor_from_name(Some("progress"));
        let err = asyncjob::run(
            Some(&i.window),
            "Creating virtual network...",
            "Creating the virtual network may take a while...",
            true,
            move |_job| {
                // `_async_net_create`
                eprintln!("Creating virtual network '{name}' with xml:\n{xml}");
                network::create(handle.connect(), &xml).map_err(|e| (e.clone(), e))
            },
        )
        .await;
        // `_finish_cb`: `reset_finish_cursor`
        i.window.set_sensitive(true);
        i.window.set_cursor_from_name(None);
        if let Some((error, details)) = err {
            error::show_err(
                Some(&i.window),
                &format!("Error creating virtual network: {error}"),
                Some(&details),
                "",
            )
            .await;
            return;
        }
        if let Some(cb) = i.net_created.borrow().as_ref() {
            cb();
        }
        self.close();
    }

    ////////////////
    // UI listeners //
    ////////////////

    /// `_xmleditor_xml_requested_cb`.
    fn xmleditor_xml_requested(&self) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            let xml = this.build_xmlobj(false).await.map(|(_, xml)| xml);
            this.0.xmleditor.set_xml(xml.as_deref().unwrap_or(""));
        });
    }
}
