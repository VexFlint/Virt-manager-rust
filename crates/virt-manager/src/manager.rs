//! Upstream's `virtManager/manager.py` + `ui/manager.ui` (`vmmManager`):
//! the main window. Menu bar, toolbar (New / Open / Run / Pause /
//! Shut Down), and the connection → VM list, ported per ticket 13 to
//! `GtkColumnView` + `GtkTreeListModel` + `GtkTreeExpander`.
//!
//! Still to come, each with its own port: the stats columns and their
//! sparklines (View > Graph), and the windows the disabled actions open
//! (New VM, VM/Connection details, Delete, Clone,
//! Migrate, Preferences).

use std::cell::Cell;
use std::rc::{Rc, Weak};

use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use crate::about;
use crate::connection::VmmConnection;
use crate::createconn;
use crate::domain::VmmDomain;
use crate::engine;
use crate::error;
use crate::manager_row::{ManagerRow, RowHandle};
use crate::vmmenu;

/// `_build_conn_markup`.
pub fn build_conn_markup(name: &str, disconnected: bool, connecting: bool) -> String {
    let name = glib::markup_escape_text(name);
    let text = if disconnected {
        format!("{name} - Not Connected")
    } else if connecting {
        format!("{name} - Connecting...")
    } else {
        name.to_string()
    };
    format!("<span size='smaller'>{text}</span>")
}

/// `_build_vm_markup`.
pub fn build_vm_markup(name: &str, status: &str) -> String {
    format!(
        "<span size='smaller' weight='bold'>{}</span>\n<span size='smaller'>{}</span>",
        glib::markup_escape_text(name),
        status
    )
}

/// `_build_conn_hint`.
pub fn build_conn_hint(uri: &str, disconnected: bool) -> String {
    if disconnected {
        format!("{uri} (Double click to connect)")
    } else {
        uri.to_string()
    }
}

/// Upstream's `update_current_selection` sensitivity table, as data so
/// it's testable without a window.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SelectionState {
    pub show_open: bool,
    pub show_details: bool,
    pub host_details: bool,
    pub can_delete: bool,
    pub show_run: bool,
    pub is_paused: bool,
    pub show_pause: bool,
    pub show_shutdown: bool,
}

impl SelectionState {
    pub fn compute(vm: Option<&VmmDomain>, conn: Option<&VmmConnection>) -> SelectionState {
        let is_paused = vm.is_some_and(|v| v.is_paused());
        let show_pause = if is_paused {
            vm.is_some_and(|v| v.is_unpauseable())
        } else {
            vm.is_some_and(|v| v.is_pauseable())
        };
        SelectionState {
            show_open: vm.is_some(),
            show_details: vm.is_some(),
            host_details: vm.is_some() || conn.is_some(),
            can_delete: vm.is_some() || conn.is_some(),
            show_run: vm.is_some_and(|v| v.is_runable()),
            is_paused,
            show_pause,
            show_shutdown: vm.is_some_and(|v| v.is_stoppable()),
        }
    }
}

struct Inner {
    window: gtk::ApplicationWindow,
    stack: gtk::Stack,
    error_label: gtk::Label,
    root: gio::ListStore,
    selection: gtk::SingleSelection,
    columnview: gtk::ColumnView,
    run_btn: gtk::Button,
    pause_btn: gtk::ToggleButton,
    pause_guard: Cell<bool>,
    shutdown_btn: gtk::Button,
    shutdown_menu_btn: gtk::MenuButton,
    win_actions: gio::SimpleActionGroup,
    vm_actions: gio::SimpleActionGroup,
    conn_actions: gio::SimpleActionGroup,
    visible: Cell<bool>,
}

#[derive(Clone)]
pub struct VmmManager(Rc<Inner>);

fn simple_action(
    group: &gio::SimpleActionGroup,
    name: &str,
    weak: &Weak<Inner>,
    cb: fn(&VmmManager),
) -> gio::SimpleAction {
    let action = gio::SimpleAction::new(name, None);
    let weak = weak.clone();
    action.connect_activate(move |_, _| {
        if let Some(inner) = weak.upgrade() {
            cb(&VmmManager(inner));
        }
    });
    group.add_action(&action);
    action
}

fn set_enabled(group: &gio::SimpleActionGroup, name: &str, enabled: bool) {
    if let Some(a) = group
        .lookup_action(name)
        .and_then(|a| a.downcast::<gio::SimpleAction>().ok())
    {
        a.set_enabled(enabled);
    }
}

fn build_menubar() -> gio::Menu {
    let menubar = gio::Menu::new();

    let file = gio::Menu::new();
    let s1 = gio::Menu::new();
    s1.append(Some("_Add Connection..."), Some("win.add-connection"));
    s1.append(Some("_New Virtual Machine"), Some("win.new-vm"));
    file.append_section(None, &s1);
    let s2 = gio::Menu::new();
    s2.append(Some("_Close"), Some("win.close"));
    s2.append(Some("_Quit"), Some("win.quit"));
    file.append_section(None, &s2);
    menubar.append_submenu(Some("_File"), &file);

    let edit = gio::Menu::new();
    let e1 = gio::Menu::new();
    e1.append(Some("_Connection Details"), Some("win.host-details"));
    e1.append(Some("_Virtual Machine Details"), Some("win.vm-details"));
    e1.append(Some("_Delete"), Some("win.delete"));
    edit.append_section(None, &e1);
    let e2 = gio::Menu::new();
    e2.append(Some("_Preferences"), Some("win.preferences"));
    edit.append_section(None, &e2);
    menubar.append_submenu(Some("_Edit"), &edit);

    let view = gio::Menu::new();
    let graph = gio::Menu::new();
    graph.append(Some("_Guest CPU Usage"), Some("win.stats-guest-cpu"));
    graph.append(Some("_Host CPU Usage"), Some("win.stats-host-cpu"));
    graph.append(Some("_Memory Usage"), Some("win.stats-memory"));
    graph.append(Some("_Disk I/O"), Some("win.stats-disk"));
    graph.append(Some("_Network I/O"), Some("win.stats-network"));
    view.append_submenu(Some("_Graph"), &graph);
    menubar.append_submenu(Some("_View"), &view);

    let help = gio::Menu::new();
    help.append(Some("_About"), Some("win.about"));
    menubar.append_submenu(Some("_Help"), &help);
    menubar
}

/// The connection context menu (`init_context_menus`).
fn build_conn_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let s1 = gio::Menu::new();
    s1.append(Some("_New"), Some("conn.create"));
    s1.append(Some("_Connect"), Some("conn.connect"));
    s1.append(Some("Dis_connect"), Some("conn.disconnect"));
    menu.append_section(None, &s1);
    let s2 = gio::Menu::new();
    s2.append(Some("De_lete"), Some("conn.delete"));
    menu.append_section(None, &s2);
    let s3 = gio::Menu::new();
    s3.append(Some("_Details"), Some("conn.details"));
    menu.append_section(None, &s3);
    menu
}

/// A toolbar button: large-toolbar (24px) icon, label only for the
/// "is-important" Open button, as GTK3's default toolbar style showed.
fn tool_button(icon: &str, label: Option<&str>, tooltip: &str) -> gtk::Button {
    let btn = gtk::Button::new();
    btn.set_child(Some(&tool_content(icon, label)));
    btn.set_tooltip_text(Some(tooltip));
    btn.add_css_class("flat");
    btn
}

fn tool_content(icon: &str, label: Option<&str>) -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let image = gtk::Image::from_icon_name(icon);
    image.set_pixel_size(24);
    content.append(&image);
    if let Some(label) = label {
        content.append(&gtk::Label::with_mnemonic(label));
    }
    content
}

impl VmmManager {
    pub fn new(app: &gtk::Application) -> VmmManager {
        let config = engine::get().config();
        let (w, h) = config.get_manager_window_size();
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("Virtual Machine Manager")
            .default_width(if w > 0 { w } else { 550 })
            .default_height(if h > 0 { h } else { 550 })
            .build();
        app.set_accels_for_action("win.close", &["<Control>w"]);
        app.set_accels_for_action("win.quit", &["<Control>q"]);

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
        vbox.append(&gtk::PopoverMenuBar::from_model(Some(&build_menubar())));

        // Toolbar
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        toolbar.add_css_class("toolbar");
        let new_btn = tool_button("vm_new", None, "Create a new virtual machine");
        new_btn.set_action_name(Some("win.new-vm"));
        toolbar.append(&new_btn);
        toolbar.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        let open_btn = tool_button(
            "icon_console",
            Some("_Open"),
            "Show the virtual machine console and details",
        );
        open_btn.set_action_name(Some("vm.show"));
        toolbar.append(&open_btn);
        let run_btn = tool_button(
            "media-playback-start-symbolic",
            None,
            "Power on the virtual machine",
        );
        toolbar.append(&run_btn);
        let pause_btn = gtk::ToggleButton::new();
        pause_btn.set_child(Some(&tool_content("media-playback-pause-symbolic", None)));
        pause_btn.set_tooltip_text(Some("Pause the virtual machine"));
        pause_btn.add_css_class("flat");
        toolbar.append(&pause_btn);
        let shutdown_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        shutdown_box.add_css_class("linked");
        let shutdown_btn = tool_button(
            "system-shutdown-symbolic",
            None,
            "Shut down the virtual machine",
        );
        let shutdown_menu_btn = gtk::MenuButton::new();
        shutdown_menu_btn.set_menu_model(Some(&vmmenu::shutdown_menu()));
        shutdown_menu_btn.add_css_class("flat");
        shutdown_box.append(&shutdown_btn);
        shutdown_box.append(&shutdown_menu_btn);
        toolbar.append(&shutdown_box);
        vbox.append(&toolbar);

        // VM list: TreeListModel over connections → VMs, sorted per-level.
        let root = gio::ListStore::new::<ManagerRow>();
        let tree = gtk::TreeListModel::new(root.clone(), false, true, |item| {
            item.downcast_ref::<ManagerRow>()
                .and_then(|r| r.children())
                .map(|s| s.upcast())
        });
        let columnview = gtk::ColumnView::new(None::<gtk::SingleSelection>);
        let row_sorter = gtk::TreeListRowSorter::new(columnview.sorter());
        let sorted = gtk::SortListModel::new(Some(tree), Some(row_sorter));
        let selection = gtk::SingleSelection::new(Some(sorted));
        selection.set_can_unselect(false);
        columnview.set_model(Some(&selection));
        columnview.set_show_column_separators(false);
        columnview.update_property(&[gtk::accessible::Property::Label("vm-list")]);

        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, obj| {
            let item = obj.downcast_ref::<gtk::ListItem>().expect("ListItem");
            let expander = gtk::TreeExpander::new();
            expander.set_indent_for_depth(false);
            let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let icon = gtk::Image::new();
            icon.set_pixel_size(32);
            let label = gtk::Label::new(None);
            label.set_use_markup(true);
            label.set_xalign(0.0);
            content.append(&icon);
            content.append(&label);
            content.set_margin_top(2);
            content.set_margin_bottom(2);
            expander.set_child(Some(&content));
            item.set_child(Some(&expander));

            let row_expr = item.property_expression("item");
            row_expr.bind(&expander, "list-row", gtk::Widget::NONE);
            let data = row_expr.chain_property::<gtk::TreeListRow>("item");
            data.chain_property::<ManagerRow>("markup")
                .bind(&label, "label", gtk::Widget::NONE);
            data.chain_property::<ManagerRow>("sensitive").bind(
                &label,
                "sensitive",
                gtk::Widget::NONE,
            );
            data.chain_property::<ManagerRow>("status-icon").bind(
                &icon,
                "icon-name",
                gtk::Widget::NONE,
            );
            data.chain_property::<ManagerRow>("is-vm")
                .bind(&icon, "visible", gtk::Widget::NONE);
            data.chain_property::<ManagerRow>("hint").bind(
                &expander,
                "tooltip-text",
                gtk::Widget::NONE,
            );
        });
        let name_col = gtk::ColumnViewColumn::new(Some("Name"), Some(factory));
        name_col.set_expand(true);
        name_col.set_sorter(Some(&gtk::CustomSorter::new(|a, b| {
            let key = |o: &glib::Object| {
                o.downcast_ref::<ManagerRow>()
                    .map(|r| r.sort_key().to_lowercase())
                    .unwrap_or_default()
            };
            key(a).cmp(&key(b)).into()
        })));
        columnview.append_column(&name_col);
        columnview.sort_by_column(Some(&name_col), gtk::SortType::Ascending);

        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_child(Some(&columnview));
        scrolled.set_vexpand(true);

        let error_label = gtk::Label::new(Some("error"));
        error_label.set_halign(gtk::Align::Center);
        error_label.set_wrap(true);
        error_label.set_selectable(true);
        error_label.set_max_width_chars(40);
        // GTK4 sizes a wrapping label to its minimum; GTK3 used max-width.
        error_label.set_width_chars(40);
        error_label.update_property(&[gtk::accessible::Property::Label("error-label")]);

        let stack = gtk::Stack::new();
        stack.add_named(&scrolled, Some("manager"));
        stack.add_named(&error_label, Some("error"));
        stack.set_visible_child_name("manager");
        vbox.append(&stack);

        window.set_child(Some(&vbox));

        let inner = Rc::new_cyclic(|weak: &Weak<Inner>| {
            let win_actions = gio::SimpleActionGroup::new();
            for (name, cb) in [
                ("close", VmmManager::close as fn(&VmmManager)),
                ("quit", |_| engine::get().exit_app()),
                ("delete", VmmManager::do_delete),
                ("about", |m| about::show(m.window().upcast_ref())),
                ("add-connection", |m| {
                    createconn::VmmCreateConn::show_instance(&m.window())
                }),
                ("new-vm", |_| {}),
                ("host-details", |_| {}),
                ("vm-details", |_| {}),
                ("preferences", |_| {}),
                ("stats-guest-cpu", |_| {}),
                ("stats-host-cpu", |_| {}),
                ("stats-memory", |_| {}),
                ("stats-disk", |_| {}),
                ("stats-network", |_| {}),
            ] {
                simple_action(&win_actions, name, weak, cb);
            }

            let conn_actions = gio::SimpleActionGroup::new();
            simple_action(&conn_actions, "create", weak, |_| {});
            simple_action(&conn_actions, "connect", weak, VmmManager::open_conn);
            simple_action(&conn_actions, "disconnect", weak, VmmManager::close_conn);
            simple_action(&conn_actions, "delete", weak, VmmManager::do_delete);
            simple_action(&conn_actions, "details", weak, |_| {});

            let weak_vm = weak.clone();
            let vm_actions = vmmenu::create_actions(
                window.upcast_ref(),
                Rc::new(move || weak_vm.upgrade().and_then(|i| VmmManager(i).current_vm())),
            );

            Inner {
                window: window.clone(),
                stack,
                error_label,
                root,
                selection: selection.clone(),
                columnview: columnview.clone(),
                run_btn,
                pause_btn,
                pause_guard: Cell::new(false),
                shutdown_btn,
                shutdown_menu_btn,
                win_actions,
                vm_actions,
                conn_actions,
                visible: Cell::new(false),
            }
        });
        let this = VmmManager(inner);
        this.wire_signals();
        this.set_unported_actions();

        let cm = engine::get().connmanager();
        for conn in cm.conns() {
            this.conn_added(&conn);
        }
        let weak = this.downgrade();
        cm.connect_local("conn-added", false, move |args| {
            let conn = args[1].get::<VmmConnection>().ok()?;
            Self::upgrade(&weak)?.conn_added(&conn);
            None
        });
        let weak = this.downgrade();
        cm.connect_local("conn-removed", false, move |args| {
            let uri = args[1].get::<String>().ok()?;
            Self::upgrade(&weak)?.conn_removed(&uri);
            None
        });

        this.update_current_selection();
        this
    }

    fn downgrade(&self) -> Weak<Inner> {
        Rc::downgrade(&self.0)
    }

    fn upgrade(weak: &Weak<Inner>) -> Option<VmmManager> {
        weak.upgrade().map(VmmManager)
    }

    pub fn window(&self) -> gtk::Window {
        self.0.window.clone().upcast()
    }

    fn wire_signals(&self) {
        let i = &self.0;
        i.window.insert_action_group("win", Some(&i.win_actions));
        i.window
            .insert_action_group(vmmenu::GROUP, Some(&i.vm_actions));
        i.window.insert_action_group("conn", Some(&i.conn_actions));

        let weak = self.downgrade();
        i.window.connect_close_request(move |_| {
            if let Some(m) = Self::upgrade(&weak) {
                m.close();
            }
            glib::Propagation::Stop
        });

        let weak = self.downgrade();
        i.selection.connect_selected_item_notify(move |_| {
            if let Some(m) = Self::upgrade(&weak) {
                m.update_current_selection();
            }
        });

        let weak = self.downgrade();
        i.columnview.connect_activate(move |_, _| {
            if let Some(m) = Self::upgrade(&weak) {
                m.row_activated();
            }
        });

        let weak = self.downgrade();
        i.run_btn.connect_clicked(move |_| {
            if let Some(m) = Self::upgrade(&weak) {
                m.0.vm_actions.activate_action("run", None);
            }
        });
        let weak = self.downgrade();
        i.shutdown_btn.connect_clicked(move |_| {
            if let Some(m) = Self::upgrade(&weak) {
                m.0.vm_actions.activate_action("shutdown", None);
            }
        });
        let weak = self.downgrade();
        i.pause_btn.connect_toggled(move |btn| {
            if let Some(m) = Self::upgrade(&weak) {
                m.pause_vm_button(btn.is_active());
            }
        });

        // Right click: select the row under the pointer, pop up its menu.
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_SECONDARY);
        let weak = self.downgrade();
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(m) = Self::upgrade(&weak) else {
                return;
            };
            let cv = &m.0.columnview;
            let Some(picked) = cv.pick(x, y, gtk::PickFlags::DEFAULT) else {
                return;
            };
            let expander = std::iter::successors(Some(picked), |w| w.parent())
                .find_map(|w| w.downcast::<gtk::TreeExpander>().ok());
            let Some(row) = expander.and_then(|e| e.list_row()) else {
                return;
            };
            if let Some(pos) = m.position_of_row(&row) {
                m.0.selection.set_selected(pos);
            }
            gesture.set_state(gtk::EventSequenceState::Claimed);
            m.popup_menu(x, y);
        });
        i.columnview.add_controller(click);

        let keys = gtk::EventControllerKey::new();
        let weak = self.downgrade();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key != gdk::Key::Menu {
                return glib::Propagation::Proceed;
            }
            if let Some(m) = Self::upgrade(&weak) {
                let cv = &m.0.columnview;
                m.popup_menu(cv.width() as f64 / 2.0, cv.height() as f64 / 2.0);
            }
            glib::Propagation::Stop
        });
        i.columnview.add_controller(keys);
    }

    /// Windows not ported yet: their entry points stay visible (same
    /// menus as upstream) but insensitive.
    fn set_unported_actions(&self) {
        for name in [
            "new-vm",
            "host-details",
            "vm-details",
            "preferences",
            "stats-guest-cpu",
            "stats-host-cpu",
            "stats-memory",
            "stats-disk",
            "stats-network",
        ] {
            set_enabled(&self.0.win_actions, name, false);
        }
        set_enabled(&self.0.conn_actions, "create", false);
        set_enabled(&self.0.conn_actions, "details", false);
    }

    fn position_of_row(&self, row: &gtk::TreeListRow) -> Option<u32> {
        let model = &self.0.selection;
        (0..model.n_items()).find(|&i| {
            model
                .item(i)
                .and_then(|o| o.downcast::<gtk::TreeListRow>().ok())
                .is_some_and(|r| &r == row)
        })
    }

    fn popup_menu(&self, x: f64, y: f64) {
        let Some(row) = self.current_row() else {
            return;
        };
        let model = match row.handle() {
            RowHandle::Vm(vm) => {
                vmmenu::update_widget_states(&self.0.vm_actions, Some(&vm));
                vmmenu::action_menu(Some(&vm), true)
            }
            RowHandle::Conn(conn) => {
                let disconn = conn.is_disconnected();
                let conning = conn.is_connecting();
                let ca = &self.0.conn_actions;
                set_enabled(ca, "disconnect", !(disconn || conning));
                set_enabled(ca, "connect", disconn);
                set_enabled(ca, "delete", disconn);
                build_conn_menu()
            }
        };
        // NESTED: submenus open beside the menu, like GTK3's Gtk.Menu.
        let p = gtk::PopoverMenu::from_model_full(&model, gtk::PopoverMenuFlags::NESTED);
        p.set_parent(&self.0.columnview);
        p.set_has_arrow(false);
        p.set_halign(gtk::Align::Start);
        p.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        p.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        p.popup();
    }

    // Common methods

    pub fn show(&self) {
        let was_visible = self.0.visible.replace(true);
        self.0.window.present();
        if !was_visible {
            engine::get().increment_window_counter();
        }
    }

    pub fn close(&self) {
        if !self.0.visible.replace(false) {
            return;
        }
        self.save_window_size();
        self.0.window.set_visible(false);
        engine::get().decrement_window_counter();
    }

    pub fn save_window_size(&self) {
        let w = &self.0.window;
        engine::get()
            .config()
            .set_manager_window_size(w.default_width(), w.default_height());
    }

    pub fn set_startup_error(&self, msg: &str) {
        self.0.stack.set_visible_child_name("error");
        self.0.error_label.set_text(msg);
    }

    // Helper methods

    fn current_row(&self) -> Option<ManagerRow> {
        self.0
            .selection
            .selected_item()
            .and_downcast::<gtk::TreeListRow>()
            .and_then(|r| r.item())
            .and_downcast::<ManagerRow>()
    }

    pub fn current_vm(&self) -> Option<VmmDomain> {
        self.current_row().and_then(|r| r.vm())
    }

    pub fn current_conn(&self) -> Option<VmmConnection> {
        match self.current_row()?.handle() {
            RowHandle::Conn(c) => Some(c),
            RowHandle::Vm(v) => v.conn(),
        }
    }

    fn get_conn_row(&self, conn: &VmmConnection) -> Option<ManagerRow> {
        self.0
            .root
            .iter::<ManagerRow>()
            .flatten()
            .find(|r| r.conn().as_ref() == Some(conn))
    }

    fn get_vm_row(&self, vm: &VmmDomain) -> Option<(gio::ListStore, u32, ManagerRow)> {
        let conn_row = self.get_conn_row(&vm.conn()?)?;
        let children = conn_row.children()?;
        let pos = children
            .iter::<ManagerRow>()
            .flatten()
            .position(|r| r.vm().as_ref() == Some(vm))?;
        let row = children.item(pos as u32)?.downcast::<ManagerRow>().ok()?;
        Some((children, pos as u32, row))
    }

    // Action listeners

    fn row_activated(&self) {
        let Some(conn) = self.current_conn() else {
            return;
        };
        if self.current_vm().is_some() {
            // show_vm: the VM window isn't ported yet.
            self.0.vm_actions.activate_action("show", None);
        } else if conn.is_disconnected() {
            self.open_conn();
        }
        // else show_host: the Connection Details window isn't ported yet.
    }

    fn do_delete(&self) {
        let Some(conn) = self.current_conn() else {
            return;
        };
        if self.current_vm().is_some() {
            // VMActionUI.delete: the Delete dialog isn't ported yet.
            self.0.vm_actions.activate_action("delete", None);
            return;
        }
        let parent = self.window();
        glib::spawn_future_local(async move {
            let text = format!(
                "This will remove the connection:\n\n{}\n\nAre you sure?",
                conn.get_uri()
            );
            if error::yes_no(Some(&parent), &text, None).await {
                engine::get().connmanager().remove_conn(&conn.get_uri());
            }
        });
    }

    fn set_pause_state(&self, state: bool) {
        self.0.pause_guard.set(true);
        self.0.pause_btn.set_active(state);
        self.0.pause_guard.set(false);
    }

    fn pause_vm_button(&self, do_pause: bool) {
        if self.0.pause_guard.get() {
            return;
        }
        // Set button state back to original value: just let the status
        // update function fix things for us
        self.set_pause_state(!do_pause);
        let action = if do_pause { "suspend" } else { "resume" };
        self.0.vm_actions.activate_action(action, None);
    }

    fn close_conn(&self) {
        if let Some(conn) = self.current_conn() {
            if !conn.is_disconnected() {
                conn.close();
            }
        }
    }

    fn open_conn(&self) {
        let Some(conn) = self.current_conn() else {
            return;
        };
        if !conn.is_disconnected() {
            return;
        }
        let parent = self.window();
        conn.open(move |err| {
            if let Some(err) = err {
                glib::spawn_future_local(async move {
                    error::show_err(Some(&parent), &err.msg, Some(&err.details), &err.title).await;
                });
            }
        });
    }

    // VM add/remove management methods

    fn build_vm_row(vm: &VmmDomain) -> ManagerRow {
        let row = ManagerRow::new(RowHandle::Vm(vm.clone()));
        Self::refresh_vm_row(&row, vm);
        row
    }

    fn refresh_vm_row(row: &ManagerRow, vm: &VmmDomain) {
        let name = vm.get_name_or_title();
        row.set_sort_key(&name);
        row.set_markup(build_vm_markup(&name, vm.run_status()));
        row.set_status_icon(vm.run_status_icon_name());
        row.set_hint(vm.get_description());
        row.set_sensitive(true);
    }

    fn refresh_conn_row(row: &ManagerRow, conn: &VmmConnection) {
        let name = conn.get_pretty_desc();
        row.set_sort_key(&name);
        row.set_markup(build_conn_markup(
            &name,
            conn.is_disconnected(),
            conn.is_connecting(),
        ));
        row.set_hint(build_conn_hint(&conn.get_uri(), conn.is_disconnected()));
        row.set_sensitive(!conn.is_disconnected());
    }

    fn vm_added(&self, conn: &VmmConnection, vm: &VmmDomain) {
        let Some(conn_row) = self.get_conn_row(conn) else {
            return;
        };
        if self.get_vm_row(vm).is_some() {
            return;
        }
        conn_row
            .children()
            .expect("conn rows have children")
            .append(&Self::build_vm_row(vm));

        let weak = self.downgrade();
        vm.connect_local("state-changed", false, move |args| {
            let vm = args[0].get::<VmmDomain>().ok()?;
            Self::upgrade(&weak)?.vm_changed(&vm);
            None
        });
        self.resort();
    }

    fn vm_removed(&self, vm: &VmmDomain) {
        if let Some((store, pos, _)) = self.get_vm_row(vm) {
            store.remove(pos);
        }
    }

    fn conn_added(&self, conn: &VmmConnection) {
        // Make sure error page isn't showing
        self.0.stack.set_visible_child_name("manager");
        if self.get_conn_row(conn).is_some() {
            return;
        }
        let row = ManagerRow::new(RowHandle::Conn(conn.clone()));
        Self::refresh_conn_row(&row, conn);
        self.0.root.append(&row);

        let weak = self.downgrade();
        conn.connect_local("vm-added", false, move |args| {
            let conn = args[0].get::<VmmConnection>().ok()?;
            let vm = args[1].get::<VmmDomain>().ok()?;
            Self::upgrade(&weak)?.vm_added(&conn, &vm);
            None
        });
        let weak = self.downgrade();
        conn.connect_local("vm-removed", false, move |args| {
            let vm = args[1].get::<VmmDomain>().ok()?;
            Self::upgrade(&weak)?.vm_removed(&vm);
            None
        });
        let weak = self.downgrade();
        conn.connect_local("state-changed", false, move |args| {
            let conn = args[0].get::<VmmConnection>().ok()?;
            Self::upgrade(&weak)?.conn_state_changed(&conn);
            None
        });

        for vm in conn.list_vms() {
            self.vm_added(conn, &vm);
        }
        self.resort();
    }

    fn conn_removed(&self, uri: &str) {
        let root = &self.0.root;
        if let Some(pos) = root
            .iter::<ManagerRow>()
            .flatten()
            .position(|r| r.conn().is_some_and(|c| c.get_uri() == uri))
        {
            if let Some(children) = root
                .item(pos as u32)
                .and_downcast::<ManagerRow>()
                .and_then(|r| r.children())
            {
                children.remove_all();
            }
            root.remove(pos as u32);
        }
    }

    // State/UI updating methods

    fn vm_changed(&self, vm: &VmmDomain) {
        let Some((_, _, row)) = self.get_vm_row(vm) else {
            return;
        };
        Self::refresh_vm_row(&row, vm);
        if self.current_vm().as_ref() == Some(vm) {
            self.update_current_selection();
        }
        self.resort();
    }

    pub fn set_initial_selection(&self, uri: &str) {
        let model = &self.0.selection;
        for i in 0..model.n_items() {
            let row = model
                .item(i)
                .and_downcast::<gtk::TreeListRow>()
                .and_then(|r| r.item())
                .and_downcast::<ManagerRow>();
            if row
                .and_then(|r| r.conn())
                .is_some_and(|c| c.get_uri() == uri)
            {
                model.set_selected(i);
                return;
            }
        }
    }

    fn conn_state_changed(&self, conn: &VmmConnection) {
        let Some(row) = self.get_conn_row(conn) else {
            return;
        };
        Self::refresh_conn_row(&row, conn);
        if !conn.is_active() {
            // vm-removed signals should handle this, this is a fallback
            if let Some(children) = row.children() {
                children.remove_all();
            }
        }
        self.resort();
        self.update_current_selection();
    }

    /// Row text changed in place: tell the sorter, since a ListStore
    /// item's own property changes don't re-sort.
    fn resort(&self) {
        if let Some(sorter) = self.0.columnview.sorter() {
            sorter.changed(gtk::SorterChange::Different);
        }
    }

    fn change_run_text(&self, can_restore: bool) {
        let text = if can_restore { "Restore" } else { "Run" };
        self.0.run_btn.set_tooltip_text(Some(if can_restore {
            "Restore the virtual machine"
        } else {
            "Power on the virtual machine"
        }));
        self.0
            .run_btn
            .update_property(&[gtk::accessible::Property::Label(text)]);
    }

    pub fn update_current_selection(&self) {
        let vm = self.current_vm();
        let conn = self.current_conn();
        let st = SelectionState::compute(vm.as_ref(), conn.as_ref());
        let i = &self.0;

        if let Some(vm) = &vm {
            self.change_run_text(vm.has_managed_save());
        }

        vmmenu::update_widget_states(&i.vm_actions, vm.as_ref());
        i.run_btn.set_sensitive(st.show_run);
        i.shutdown_btn.set_sensitive(st.show_shutdown);
        i.shutdown_menu_btn.set_sensitive(st.show_shutdown);

        self.set_pause_state(st.is_paused);
        i.pause_btn.set_sensitive(st.show_pause);
        i.pause_btn.set_tooltip_text(Some(if st.is_paused {
            "Resume the virtual machine"
        } else {
            "Pause the virtual machine"
        }));

        // Edit > Delete: VM deletion waits on the Delete dialog port;
        // a connection can be removed now.
        set_enabled(&i.win_actions, "delete", st.can_delete && vm.is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conn_markup_matches_upstream() {
        assert_eq!(
            build_conn_markup("QEMU/KVM", true, false),
            "<span size='smaller'>QEMU/KVM - Not Connected</span>"
        );
        assert_eq!(
            build_conn_markup("QEMU/KVM", false, true),
            "<span size='smaller'>QEMU/KVM - Connecting...</span>"
        );
        assert_eq!(
            build_conn_markup("a<b", false, false),
            "<span size='smaller'>a&lt;b</span>"
        );
    }

    #[test]
    fn vm_markup_matches_upstream() {
        assert_eq!(
            build_vm_markup("my&vm", "Running"),
            "<span size='smaller' weight='bold'>my&amp;vm</span>\n<span size='smaller'>Running</span>"
        );
    }

    #[test]
    fn conn_hint() {
        assert_eq!(
            build_conn_hint("qemu:///system", true),
            "qemu:///system (Double click to connect)"
        );
        assert_eq!(build_conn_hint("qemu:///system", false), "qemu:///system");
    }

    #[test]
    fn nothing_selected_disables_everything() {
        assert_eq!(
            SelectionState::compute(None, None),
            SelectionState::default()
        );
    }
}
