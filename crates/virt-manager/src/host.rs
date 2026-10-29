//! `host.py` + `ui/host.ui` (`vmmHost`): the Connection Details window.
//! A File menu (View Manager, Close, Quit) over three tabs: Overview
//! (name, URI, autoconnect, and the host CPU and memory usage graphs),
//! Virtual Networks (`hostnets.rs`) and Storage (`hoststorage.rs`).
//!
//! One window per connection, keyed by URI like upstream's
//! `_instances`, dropped when the connection is removed.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use crate::connection::VmmConnection;
use crate::engine;
use crate::graphwidgets::SparklineGraph;
use crate::hostnets::HostNets;
use crate::hoststorage::HostStorage;

thread_local! {
    static INSTANCES: RefCell<HashMap<String, VmmHost>> = RefCell::default();
}

/// `uiutil.pretty_mem`: KiB as `"%2.2f GiB"` above 10 GiB, else
/// `"%2.0f MiB"`.
pub fn pretty_mem(val: u64) -> String {
    if val > 10 * 1024 * 1024 {
        format!("{:2.2} GiB", val as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:2.0} MiB", val as f64 / 1024.0)
    }
}

/// The `performance-memory` text.
fn memory_text(vm_memory: u64, host_memory: u64) -> String {
    format!("{} of {}", pretty_mem(vm_memory), pretty_mem(host_memory))
}

/// A glade `GtkFrame` with shadow none: the bold label over the child.
fn frame(title: &str, child: &impl IsA<gtk::Widget>) -> gtk::Box {
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let label = gtk::Label::new(None);
    label.set_markup(&format!("<b>{title}</b>"));
    label.set_xalign(0.0);
    vbox.append(&label);
    vbox.append(child);
    vbox
}

/// One of the `performance-*` frames: the graph in its 75px box over
/// the right-aligned value label.
fn performance_frame(title: &str) -> (gtk::Box, SparklineGraph, gtk::Label) {
    let graph = SparklineGraph::default();
    graph.set_hexpand(true);
    let align = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    align.set_size_request(-1, 75);
    align.append(&graph);
    let label = gtk::Label::new(None);
    label.set_halign(gtk::Align::End);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 3);
    content.set_margin_start(12);
    content.append(&align);
    content.append(&label);
    let f = frame(title, &content);
    f.set_vexpand(true);
    (f, graph, label)
}

struct Inner {
    conn: VmmConnection,
    window: gtk::ApplicationWindow,
    overview_name: gtk::Entry,
    config_autoconnect: gtk::CheckButton,
    performance_cpu: gtk::Label,
    performance_memory: gtk::Label,
    cpu_usage_graph: SparklineGraph,
    memory_usage_graph: SparklineGraph,
    hostnets: HostNets,
    storagelist: HostStorage,
    window_size: Cell<Option<(i32, i32)>>,
    /// Set while `refresh_conn_state` fills `overview-name` in.
    setting_name: Cell<bool>,
    handlers: RefCell<Vec<(glib::Object, glib::SignalHandlerId)>>,
}

#[derive(Clone)]
pub struct VmmHost(Rc<Inner>);

fn build_menubar() -> gio::Menu {
    let menubar = gio::Menu::new();
    let file = gio::Menu::new();
    let f1 = gio::Menu::new();
    f1.append(Some("_View Manager"), Some("host.view-manager"));
    file.append_section(None, &f1);
    let f2 = gio::Menu::new();
    f2.append(Some("_Close"), Some("host.close"));
    f2.append(Some("_Quit"), Some("host.quit"));
    file.append_section(None, &f2);
    menubar.append_submenu(Some("_File"), &file);
    menubar
}

impl VmmHost {
    /// `vmmHost.show_instance(parentobj, conn)`: one dialog per
    /// connection.
    pub fn show_instance(conn: &VmmConnection) {
        let uri = conn.get_uri();
        let this = INSTANCES.with(|i| {
            i.borrow_mut()
                .entry(uri)
                .or_insert_with(|| VmmHost::new(conn))
                .clone()
        });
        this.show();
    }

    fn new(conn: &VmmConnection) -> VmmHost {
        let app = engine::get().app();
        let window = gtk::ApplicationWindow::builder()
            .application(&app)
            .title("Connection Details")
            .build();
        window.set_size_request(800, 600);
        app.set_accels_for_action("host.close", &["<Control>w"]);
        app.set_accels_for_action("host.quit", &["<Control>q"]);

        // Set default window size
        let (mut w, mut h) = conn.get_details_window_size();
        if w <= 0 {
            w = 800;
        }
        if h <= 0 {
            h = 600;
        }
        window.set_default_size(w, h);

        let menubar = gtk::PopoverMenuBar::from_model(Some(&build_menubar()));

        // Overview: Basic details (table1)
        let overview_name = gtk::Entry::new();
        overview_name.set_max_width_chars(30);
        let overview_uri = gtk::Label::new(Some("example.com"));
        overview_uri.set_halign(gtk::Align::Start);
        overview_uri.set_selectable(true);
        overview_uri.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let config_autoconnect = gtk::CheckButton::new();
        let name_label = gtk::Label::with_mnemonic("_Name:");
        name_label.set_halign(gtk::Align::End);
        name_label.set_mnemonic_widget(Some(&overview_name));
        let uri_label = gtk::Label::new(Some("Libvirt URI:"));
        uri_label.set_halign(gtk::Align::End);
        let autoconnect_label = gtk::Label::with_mnemonic("A_utoconnect:");
        autoconnect_label.set_halign(gtk::Align::End);
        autoconnect_label.set_mnemonic_widget(Some(&config_autoconnect));
        let table1 = gtk::Grid::new();
        table1.set_row_spacing(3);
        table1.set_column_spacing(6);
        // margin-start 12 plus border-width 3
        table1.set_margin_start(15);
        table1.set_margin_end(3);
        table1.set_margin_top(3);
        table1.set_margin_bottom(3);
        table1.attach(&name_label, 0, 0, 1, 1);
        table1.attach(&overview_name, 1, 0, 1, 1);
        table1.attach(&uri_label, 0, 1, 1, 1);
        table1.attach(&overview_uri, 1, 1, 1, 1);
        table1.attach(&autoconnect_label, 0, 2, 1, 1);
        table1.attach(&config_autoconnect, 1, 2, 1, 1);

        // CPU and memory usage (box4)
        let (cpu_frame, cpu_usage_graph, performance_cpu) = performance_frame("CPU usage");
        performance_cpu.set_text("60%");
        let (memory_frame, memory_usage_graph, performance_memory) =
            performance_frame("Memory usage");
        performance_memory.set_text("1.59 GiB of 2.2 GiB");
        let box4 = gtk::Box::new(gtk::Orientation::Vertical, 0);
        box4.set_vexpand(true);
        box4.append(&cpu_frame);
        box4.append(&memory_frame);

        let vbox2 = gtk::Box::new(gtk::Orientation::Vertical, 18);
        // Packed with expand=False: box4's own expand stays inside
        vbox2.set_vexpand(false);
        vbox2.set_margin_start(6);
        vbox2.set_margin_end(6);
        vbox2.set_margin_top(6);
        vbox2.set_margin_bottom(6);
        vbox2.append(&frame("Basic details", &table1));
        vbox2.append(&box4);
        let box1 = gtk::Box::new(gtk::Orientation::Vertical, 0);
        box1.append(&vbox2);

        // `_init_net_state` / `_init_storage_state`
        let hostnets = HostNets::new(conn);
        let net_align = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        net_align.append(&hostnets.top_box);
        let storagelist = HostStorage::new(conn, None);
        let storage_align = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        storage_align.append(&storagelist.top_box);

        let notebook = gtk::Notebook::new();
        notebook.set_margin_start(6);
        notebook.set_margin_end(6);
        notebook.set_margin_top(6);
        notebook.set_margin_bottom(6);
        notebook.set_vexpand(true);
        for (page, label) in [
            (box1.upcast_ref::<gtk::Widget>(), "_Overview"),
            (net_align.upcast_ref(), "_Virtual Networks"),
            (storage_align.upcast_ref(), "_Storage"),
        ] {
            notebook.append_page(page, Some(&gtk::Label::with_mnemonic(label)));
        }

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
        vbox.append(&menubar);
        vbox.append(&notebook);
        window.set_child(Some(&vbox));

        let inner = Rc::new_cyclic(|weak: &Weak<Inner>| {
            let actions = gio::SimpleActionGroup::new();
            for (name, cb) in [
                (
                    "view-manager",
                    (|_| engine::get().get_manager().show()) as fn(&VmmHost),
                ),
                ("close", VmmHost::close),
                ("quit", |_| engine::get().exit_app()),
            ] {
                let action = gio::SimpleAction::new(name, None);
                let weak = weak.clone();
                action.connect_activate(move |_, _| {
                    if let Some(inner) = weak.upgrade() {
                        cb(&VmmHost(inner));
                    }
                });
                actions.add_action(&action);
            }
            window.insert_action_group("host", Some(&actions));

            Inner {
                conn: conn.clone(),
                window: window.clone(),
                overview_name,
                config_autoconnect,
                performance_cpu,
                performance_memory,
                cpu_usage_graph,
                memory_usage_graph,
                hostnets,
                storagelist,
                window_size: Cell::new(None),
                setting_name: Cell::new(false),
                handlers: RefCell::default(),
            }
        });
        let this = VmmHost(inner);

        // `_init_conn_state`
        overview_uri.set_text(&conn.get_uri());
        this.0
            .config_autoconnect
            .set_active(engine::get().connmanager().get_autoconnect(conn));

        this.connect_signals(&notebook);
        this.refresh_resources();
        this.refresh_conn_state();
        this
    }

    fn connect_signals(&self, notebook: &gtk::Notebook) {
        let i = &self.0;
        let weak = Rc::downgrade(&self.0);
        let with = move |f: fn(&VmmHost)| {
            let weak = weak.clone();
            move || {
                if let Some(inner) = weak.upgrade() {
                    f(&VmmHost(inner));
                }
            }
        };

        let cb = with(VmmHost::close);
        i.window.connect_close_request(move |_| {
            cb();
            glib::Propagation::Stop
        });
        // `_window_resized_cb`
        let weak = Rc::downgrade(&self.0);
        let size_cb = move |w: &gtk::ApplicationWindow| {
            if let Some(inner) = weak.upgrade() {
                if w.is_visible() {
                    inner.window_size.set(Some(w.default_size()));
                }
            }
        };
        let cb = size_cb.clone();
        i.window.connect_default_width_notify(move |w| cb(w));
        i.window.connect_default_height_notify(move |w| size_cb(w));

        // `_page_changed_cb`
        let weak = Rc::downgrade(&self.0);
        notebook.connect_switch_page(move |_, _, pagenum| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            if pagenum == 1 {
                inner.hostnets.refresh_page();
            } else if pagenum == 2 {
                inner.storagelist.refresh_page();
            }
        });

        // `_overview_name_changed_cb`
        let weak = Rc::downgrade(&self.0);
        i.overview_name.connect_changed(move |e| {
            if let Some(inner) = weak.upgrade() {
                if !inner.setting_name.get() {
                    inner.conn.set_config_pretty_name(&e.text());
                }
            }
        });
        // `_autoconnect_toggled_cb`
        let weak = Rc::downgrade(&self.0);
        i.config_autoconnect.connect_toggled(move |b| {
            if let Some(inner) = weak.upgrade() {
                engine::get()
                    .connmanager()
                    .set_autoconnect(&inner.conn, b.is_active());
            }
        });

        let conn = &i.conn;
        let cb = with(VmmHost::refresh_conn_state);
        let id = conn.connect_local("state-changed", false, move |_| {
            cb();
            None
        });
        i.handlers.borrow_mut().push((conn.clone().upcast(), id));
        let cb = with(VmmHost::refresh_resources);
        let id = conn.connect_local("resources-sampled", false, move |_| {
            cb();
            None
        });
        i.handlers.borrow_mut().push((conn.clone().upcast(), id));

        // `_cleanup_on_conn_removed`
        let cm = engine::get().connmanager();
        let weak = Rc::downgrade(&self.0);
        let id = cm.connect_local("conn-removed", false, move |args| {
            let uri = args[1].get::<String>().ok()?;
            let inner = weak.upgrade()?;
            if inner.conn.get_uri() == uri {
                VmmHost(inner).cleanup();
            }
            None
        });
        i.handlers.borrow_mut().push((cm.upcast(), id));
    }

    /////////////////////////
    // Standard UI methods //
    /////////////////////////

    fn is_visible(&self) -> bool {
        self.0.window.is_visible()
    }

    /// `show`.
    pub fn show(&self) {
        eprintln!("Showing host details: {}", self.0.conn.get_uri());
        let vis = self.is_visible();
        self.0.window.present();
        if vis {
            return;
        }
        engine::get().increment_window_counter();
    }

    /// `close`.
    pub fn close(&self) {
        eprintln!("Closing host window for {}", self.0.conn.get_uri());
        if !self.is_visible() {
            return;
        }
        // Upstream saves the size at `_cleanup`; a window hidden here
        // may never get there before the app exits.
        if let Some((w, h)) = self.0.window_size.get() {
            self.0.conn.set_details_window_size(w, h);
        }
        self.0.window.set_visible(false);
        engine::get().decrement_window_counter();
    }

    /// `_cleanup`, on `conn-removed`: close and forget this window.
    fn cleanup(&self) {
        self.close();
        let i = &self.0;
        if let Some((w, h)) = i.window_size.get() {
            i.conn.set_details_window_size(w, h);
        }
        i.storagelist.close();
        i.hostnets.cleanup();
        for (obj, id) in i.handlers.take() {
            obj.disconnect(id);
        }
        let uri = i.conn.get_uri();
        INSTANCES.with(|m| m.borrow_mut().remove(&uri));
        i.window.destroy();
    }

    ////////////////////////
    // UI conn populating //
    ////////////////////////

    /// `_refresh_resources`.
    fn refresh_resources(&self) {
        let i = &self.0;
        let conn = &i.conn;
        let mut cpu_vector = conn.host_cpu_time_vector();
        let mut memory_vector = conn.stats_memory_vector();
        cpu_vector.reverse();
        memory_vector.reverse();

        i.performance_cpu
            .set_text(&format!("{} %", conn.host_cpu_time_percentage() as i64));
        i.performance_memory
            .set_text(&memory_text(conn.stats_memory(), conn.host_memory_size()));

        i.cpu_usage_graph.set_data_array(cpu_vector);
        i.memory_usage_graph.set_data_array(memory_vector);
    }

    /// `_refresh_conn_state`.
    fn refresh_conn_state(&self) {
        let i = &self.0;
        let conn_active = i.conn.is_active();

        let desc = i.conn.get_pretty_desc();
        i.window
            .set_title(Some(&format!("{desc} - Connection Details")));
        // `has_focus()`: the entry's text child holds the focus in GTK4
        let has_focus = gtk::prelude::GtkWindowExt::focus(&i.window)
            .is_some_and(|f| f.is_ancestor(&i.overview_name) || f == i.overview_name);
        if !has_focus && i.overview_name.text() != desc {
            // GTK4's set_text emits `changed` for the cleared entry too:
            // that "" must not reach the `pretty-name` setting.
            i.setting_name.set(true);
            i.overview_name.set_text(&desc);
            i.setting_name.set(false);
        }

        if conn_active {
            return;
        }
        i.hostnets.close();
        i.storagelist.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_mem_matches_uiutil() {
        assert_eq!(pretty_mem(0), " 0 MiB");
        assert_eq!(pretty_mem(2048 * 1024), "2048 MiB");
        assert_eq!(pretty_mem(10 * 1024 * 1024), "10240 MiB");
        assert_eq!(pretty_mem(10 * 1024 * 1024 + 1), "10.00 GiB");
        assert_eq!(pretty_mem(16 * 1024 * 1024), "16.00 GiB");
        assert_eq!(memory_text(1536, 8 * 1024 * 1024), " 2 MiB of 8192 MiB");
    }
}
