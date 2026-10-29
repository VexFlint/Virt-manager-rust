//! Upstream's `virtManager/systray.py` (`vmmSystray`, `_TrayMainMenu`,
//! `_SystrayIndicator`): the system tray icon and its menu of
//! connections, their VMs and each VM's action submenu.
//!
//! GTK4 has no `GtkStatusIcon`, so the backend is upstream's
//! AppIndicator one: a StatusNotifierItem with a D-Bus menu, served by
//! `ksni`. As with an AppIndicator, clicking the icon opens the menu
//! (`Activate` answers with an error, which hosts take as "show the
//! menu"), middle click toggles the manager window
//! (`set_secondary_activate_target`), and "_Show Virtual Machine
//! Manager" sits above Quit. D-Bus menus carry no markup, so active
//! connections aren't bold there (upstream's AppIndicator menus lose
//! the bold the same way). The tooltip is the `GtkStatusIcon` backend's.
//!
//! The menu model lives on the GTK thread ([`TrayMainMenu`]) and is
//! pushed whole to the ksni service after each change; menu clicks come
//! back to the GTK thread through `MainContext::invoke`.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gtk::gio;
use gtk::glib;
use gtk::glib::prelude::*;
use gtk::prelude::*;
use ksni::TrayMethods;

use crate::connection::VmmConnection;
use crate::domain::VmmDomain;
use crate::engine;
use crate::vmmenu;

/// What a tray menu item does, carried from the ksni thread back to the
/// GTK main loop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayAction {
    /// `_toggle_manager`.
    ToggleManager,
    /// `_exit_app_cb`.
    Exit,
    /// `_conn_connect_cb`.
    ConnConnect(String),
    /// `_conn_disconnect_cb`.
    ConnDisconnect(String),
    /// A `VMActionMenu` item: the `vmmenu` action, on the VM with this
    /// UUID on the connection with this URI.
    Vm {
        uri: String,
        uuid: String,
        action: &'static str,
    },
}

/// One entry of the tray menu as the host shows it.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuNode {
    Separator,
    Item {
        label: String,
        icon: &'static str,
        enabled: bool,
        visible: bool,
        action: TrayAction,
    },
    Submenu {
        label: String,
        enabled: bool,
        children: Vec<MenuNode>,
    },
}

impl MenuNode {
    fn item(label: &str, enabled: bool, visible: bool, action: TrayAction) -> MenuNode {
        MenuNode::Item {
            label: label.into(),
            icon: "",
            enabled,
            visible,
            action,
        }
    }

    /// `Gtk.ImageMenuItem.new_from_stock`: the stock label, and the
    /// stock id as icon name (what libdbusmenu sends for it).
    fn stock(label: &str, icon: &'static str, visible: bool, action: TrayAction) -> MenuNode {
        MenuNode::Item {
            label: label.into(),
            icon,
            enabled: true,
            visible,
            action,
        }
    }
}

/// The `vmmenu` actions a VM's submenu shows.
pub const VM_ACTIONS: [&str; 12] = [
    "run", "suspend", "resume", "shutdown", "reboot", "reset", "destroy", "save", "clone",
    "migrate", "delete", "show",
];

/// What `_set_vm_state` reads off a VM: its label and the action
/// menu's `update_widget_states`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VmState {
    pub uuid: String,
    /// `vm.get_name_or_title()`.
    pub label: String,
    /// Sensitivity per [`VM_ACTIONS`] entry.
    pub enabled: Vec<(&'static str, bool)>,
    pub paused: bool,
}

impl VmState {
    fn is_enabled(&self, action: &str) -> bool {
        self.enabled
            .iter()
            .any(|(name, enabled)| *name == action && *enabled)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct VmEntry {
    /// `_vmsortkey`: the name or title when the VM was added.
    sortkey: String,
    state: VmState,
}

#[derive(Clone, Debug, PartialEq)]
struct ConnEntry {
    uri: String,
    /// `_vmsortkey`: active connections first, then by description;
    /// fixed when the connection was added.
    sortkey: String,
    /// `conn.get_pretty_desc()`.
    label: String,
    active: bool,
    vms: Vec<VmEntry>,
}

/// GTK3 menu item labels from `new_with_label` show underscores as-is;
/// D-Bus menus treat `_` as a mnemonic unless doubled.
pub fn escape_label(label: &str) -> String {
    label.replace('_', "__")
}

/// `insert(item, idx)` at the first child whose sort key is greater, so
/// later items with an equal key go after earlier ones.
fn sorted_position<T>(items: &[T], key: &str, key_of: impl Fn(&T) -> &str) -> usize {
    items
        .iter()
        .position(|i| key_of(i) > key)
        .unwrap_or(items.len())
}

/// `_TrayMainMenu`: the conn + VM menu list, updated in place. Like
/// upstream, entries are placed by their sort key when added and never
/// moved afterwards.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrayMainMenu {
    conns: Vec<ConnEntry>,
}

impl TrayMainMenu {
    /// `_build_conn_menuitem`'s sort key.
    pub fn conn_sortkey(active: bool, pretty_desc: &str) -> String {
        format!("{}{}", i32::from(!active), pretty_desc.to_lowercase())
    }

    /// `conn_add`.
    pub fn conn_add(&mut self, uri: &str, pretty_desc: &str, active: bool) {
        let sortkey = Self::conn_sortkey(active, pretty_desc);
        let idx = sorted_position(&self.conns, &sortkey, |c| &c.sortkey);
        self.conns.insert(
            idx,
            ConnEntry {
                uri: uri.into(),
                sortkey,
                label: pretty_desc.into(),
                active,
                vms: Vec::new(),
            },
        );
    }

    /// `conn_remove`.
    pub fn conn_remove(&mut self, uri: &str) {
        self.conns.retain(|c| c.uri != uri);
    }

    /// `conn_change` (`_set_conn_state`).
    pub fn conn_change(&mut self, uri: &str, pretty_desc: &str, active: bool) {
        if let Some(c) = self.conns.iter_mut().find(|c| c.uri == uri) {
            c.label = pretty_desc.into();
            c.active = active;
        }
    }

    /// `vm_add`.
    pub fn vm_add(&mut self, uri: &str, state: VmState) {
        let Some(conn) = self.conns.iter_mut().find(|c| c.uri == uri) else {
            return;
        };
        let sortkey = state.label.clone();
        let idx = sorted_position(&conn.vms, &sortkey, |v| &v.sortkey);
        conn.vms.insert(idx, VmEntry { sortkey, state });
    }

    /// `vm_remove`.
    pub fn vm_remove(&mut self, uri: &str, uuid: &str) {
        if let Some(conn) = self.conns.iter_mut().find(|c| c.uri == uri) {
            conn.vms.retain(|v| v.state.uuid != uuid);
        }
    }

    /// `vm_change` (`_set_vm_state`).
    pub fn vm_change(&mut self, uri: &str, state: VmState) {
        let vm = self
            .conns
            .iter_mut()
            .find(|c| c.uri == uri)
            .and_then(|c| c.vms.iter_mut().find(|v| v.state.uuid == state.uuid));
        if let Some(vm) = vm {
            vm.state = state;
        }
    }

    /// `VMActionMenu` (with `VMShutdownMenu`) for one VM, with
    /// `update_widget_states` applied.
    fn vm_menu(uri: &str, vm: &VmState) -> MenuNode {
        let item = |label: &str, action: &'static str, visible: bool| {
            MenuNode::item(
                label,
                vm.is_enabled(action),
                visible,
                TrayAction::Vm {
                    uri: uri.into(),
                    uuid: vm.uuid.clone(),
                    action,
                },
            )
        };
        let shutdown = vec![
            item("_Reboot", "reboot", true),
            item("_Shut Down", "shutdown", true),
            item("F_orce Reset", "reset", true),
            item("_Force Off", "destroy", true),
            MenuNode::Separator,
            item("Sa_ve", "save", true),
        ];
        MenuNode::Submenu {
            label: escape_label(&vm.label),
            enabled: true,
            children: vec![
                item("_Run", "run", true),
                item("_Pause", "suspend", !vm.paused),
                item("R_esume", "resume", vm.paused),
                MenuNode::Submenu {
                    label: "_Shut Down".into(),
                    enabled: vm.is_enabled("shutdown"),
                    children: shutdown,
                },
                MenuNode::Separator,
                item("Clone...", "clone", true),
                item("Migrate...", "migrate", true),
                item("_Delete", "delete", true),
                MenuNode::Separator,
                item("_Open", "show", true),
            ],
        }
    }

    /// The whole menu: `_build_menu`, the connection submenus
    /// (`_build_conn_menuitem`), and the item `_SystrayIndicator.set_menu`
    /// inserts before Quit.
    pub fn build(&self) -> Vec<MenuNode> {
        let mut menu: Vec<MenuNode> = self
            .conns
            .iter()
            .map(|conn| {
                let mut children: Vec<MenuNode> = conn
                    .vms
                    .iter()
                    .map(|vm| Self::vm_menu(&conn.uri, &vm.state))
                    .collect();
                children.push(MenuNode::Separator);
                children.push(MenuNode::stock(
                    "_Disconnect",
                    "gtk-disconnect",
                    conn.active,
                    TrayAction::ConnDisconnect(conn.uri.clone()),
                ));
                children.push(MenuNode::stock(
                    "C_onnect",
                    "gtk-connect",
                    !conn.active,
                    TrayAction::ConnConnect(conn.uri.clone()),
                ));
                MenuNode::Submenu {
                    label: escape_label(&conn.label),
                    enabled: true,
                    children,
                }
            })
            .collect();
        menu.push(MenuNode::Separator);
        menu.push(MenuNode::item(
            "_Show Virtual Machine Manager",
            true,
            true,
            TrayAction::ToggleManager,
        ));
        menu.push(MenuNode::stock("_Quit", "gtk-quit", true, TrayAction::Exit));
        menu
    }
}

/// SNI pixmaps are ARGB32 in network byte order; a pixbuf is RGB(A).
pub fn rgba_to_argb(
    width: usize,
    height: usize,
    rowstride: usize,
    channels: usize,
    data: &[u8],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            let p = &data[y * rowstride + x * channels..];
            let alpha = if channels == 4 { p[3] } else { 0xff };
            out.extend_from_slice(&[alpha, p[0], p[1], p[2]]);
        }
    }
    out
}

/// The `virt-manager` icon for hosts that can't find it by name (a
/// build that isn't installed): upstream's PNGs from our GResource.
fn icon_pixmaps() -> Vec<ksni::Icon> {
    [16, 22, 24, 32, 48]
        .iter()
        .filter_map(|size| {
            let path =
                format!("/org/virt-manager/virt-manager/icons/{size}x{size}/apps/virt-manager.png");
            let pixbuf = gtk::gdk_pixbuf::Pixbuf::from_resource(&path).ok()?;
            let bytes = pixbuf.read_pixel_bytes();
            Some(ksni::Icon {
                width: pixbuf.width(),
                height: pixbuf.height(),
                data: rgba_to_argb(
                    pixbuf.width() as usize,
                    pixbuf.height() as usize,
                    pixbuf.rowstride() as usize,
                    pixbuf.n_channels() as usize,
                    &bytes,
                ),
            })
        })
        .collect()
}

/// Sends a menu click to the GTK thread. An idle source on the default
/// context, not `MainContext::invoke`: that one runs the closure on the
/// calling thread when the GTK thread is between iterations.
fn dispatch(action: TrayAction) {
    glib::idle_add_once(move || handle_action(action));
}

/// The ksni side: what the StatusNotifierItem and its menu export.
/// Lives on ksni's service thread.
struct TrayIcon {
    menu: Vec<MenuNode>,
    /// Ignore pushes older than the last one applied.
    generation: u64,
    /// `IndicatorStatus.ACTIVE` vs `PASSIVE`.
    active: bool,
    /// The indicator's `connected` property: registered with a
    /// StatusNotifierWatcher.
    connected: Arc<AtomicBool>,
    /// Why it isn't, for the "systray did not show up" details.
    embed_error: Arc<Mutex<Option<String>>>,
    pixmaps: Vec<ksni::Icon>,
}

fn set_embed_error(slot: &Mutex<Option<String>>, err: Option<String>) {
    if let Ok(mut slot) = slot.lock() {
        *slot = err;
    }
}

/// An offline reason as text: ksni only gives its errors a `Display`.
fn offline_text(reason: &ksni::OfflineReason) -> String {
    match reason {
        ksni::OfflineReason::Error(e) => e.to_string(),
        _ => "The StatusNotifierWatcher went away".into(),
    }
}

fn to_ksni(nodes: &[MenuNode]) -> Vec<ksni::MenuItem<TrayIcon>> {
    nodes
        .iter()
        .map(|node| match node {
            MenuNode::Separator => ksni::MenuItem::Separator,
            MenuNode::Item {
                label,
                icon,
                enabled,
                visible,
                action,
            } => {
                let action = action.clone();
                ksni::menu::StandardItem {
                    label: label.clone(),
                    icon_name: icon.to_string(),
                    enabled: *enabled,
                    visible: *visible,
                    activate: Box::new(move |_| dispatch(action.clone())),
                    ..Default::default()
                }
                .into()
            }
            MenuNode::Submenu {
                label,
                enabled,
                children,
            } => ksni::menu::SubMenu {
                label: label.clone(),
                enabled: *enabled,
                submenu: to_ksni(children),
                ..Default::default()
            }
            .into(),
        })
        .collect()
}

impl ksni::Tray for TrayIcon {
    // An AppIndicator has no Activate: hosts open the menu instead.
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        "virt-manager".into()
    }

    fn icon_name(&self) -> String {
        "virt-manager".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.pixmaps.clone()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Virtual Machine Manager".into(),
            ..Default::default()
        }
    }

    fn status(&self) -> ksni::Status {
        if self.active {
            ksni::Status::Active
        } else {
            ksni::Status::Passive
        }
    }

    // set_secondary_activate_target(hide_item)
    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        dispatch(TrayAction::ToggleManager);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        to_ksni(&self.menu)
    }

    fn watcher_online(&self) {
        self.connected.store(true, Ordering::SeqCst);
        set_embed_error(&self.embed_error, None);
    }

    fn watcher_offline(&self, reason: ksni::OfflineReason) -> bool {
        self.connected.store(false, Ordering::SeqCst);
        let text = offline_text(&reason);
        eprintln!("System tray icon not embedded: {text}");
        set_embed_error(&self.embed_error, Some(text));
        // Keep the item: it registers again when a watcher shows up.
        true
    }
}

thread_local! {
    static USING_APPINDICATOR: OnceCell<bool> = const { OnceCell::new() };
}

/// `_has_appindicator_dbus`: is a StatusNotifierWatcher on the session
/// bus.
fn has_appindicator_dbus() -> bool {
    let res = (|| -> Result<bool, glib::Error> {
        let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)?;
        let dbus = gio::DBusProxy::new_sync(
            &bus,
            gio::DBusProxyFlags::NONE,
            None,
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            gio::Cancellable::NONE,
        )?;
        for name in [
            "org.kde.StatusNotifierWatcher",
            "org.freedesktop.StatusNotifierWatcher",
        ] {
            let out = dbus.call_sync(
                "NameHasOwner",
                Some(&(name,).to_variant()),
                gio::DBusCallFlags::NONE,
                -1,
                gio::Cancellable::NONE,
            )?;
            if out.get::<(bool,)>().is_some_and(|(has,)| has) {
                return Ok(true);
            }
        }
        Ok(false)
    })();
    res.unwrap_or_else(|e| {
        eprintln!("Error checking for appindicator dbus: {e}");
        false
    })
}

/// `_USING_APPINDICATOR`. ksni is always built in, so this is just
/// whether a watcher was there the first time it's asked (upstream
/// checks once, at import).
fn using_appindicator() -> bool {
    USING_APPINDICATOR.with(|u| *u.get_or_init(has_appindicator_dbus))
}

/// `vmmSystray.systray_disabled_message`.
pub fn systray_disabled_message() -> Option<&'static str> {
    std::env::var_os("WAYLAND_DISPLAY")?;
    if using_appindicator() {
        return None;
    }
    Some("AppIndicator not installed or usable, system tray icon not available on Wayland")
}

/// `_toggle_manager`.
fn toggle_manager() {
    let manager = engine::get().get_manager();
    if manager.is_visible() {
        manager.close();
    } else {
        manager.show();
    }
}

/// `_conn_connect_cb`.
fn conn_connect_cb(uri: &str) {
    let Some(conn) = engine::get().connmanager().get_conn(uri) else {
        return;
    };
    if conn.is_disconnected() {
        conn.open(|err| {
            if let Some(err) = err {
                eprintln!("Connection error: {}", err.details);
            }
        });
    }
}

/// `_conn_disconnect_cb`.
fn conn_disconnect_cb(uri: &str) {
    let Some(conn) = engine::get().connmanager().get_conn(uri) else {
        return;
    };
    if !conn.is_disconnected() {
        conn.close();
    }
}

fn handle_action(action: TrayAction) {
    match action {
        TrayAction::ToggleManager => toggle_manager(),
        TrayAction::Exit => engine::get().exit_app(),
        TrayAction::ConnConnect(uri) => conn_connect_cb(&uri),
        TrayAction::ConnDisconnect(uri) => conn_disconnect_cb(&uri),
        TrayAction::Vm { uri, uuid, action } => {
            if let Some(tray) = VmmSystray::instance() {
                tray.vm_action(&uri, &uuid, action);
            }
        }
    }
}

struct Inner {
    /// `self._systray` was created (the ksni service started).
    created: Cell<bool>,
    handle: RefCell<Option<ksni::Handle<TrayIcon>>>,
    connected: Arc<AtomicBool>,
    embed_error: Arc<Mutex<Option<String>>>,
    /// `show()` vs `hide()`.
    shown: Cell<bool>,
    /// `self._mainmenu`, built when the icon is first shown.
    mainmenu: RefCell<Option<TrayMainMenu>>,
    pushed: RefCell<Option<(Vec<MenuNode>, bool)>>,
    generation: Cell<u64>,
    push_scheduled: Cell<bool>,
    /// The `vmmenu` actions the VM submenus trigger, on `current_vm`.
    actions: gio::SimpleActionGroup,
    current_vm: Rc<RefCell<Option<VmmDomain>>>,
    /// `_TrayMainMenu.topwin = None`: the VM actions' dialogs need a
    /// parent window; this one is never shown, so they come up on their
    /// own, as upstream's do.
    _topwin: gtk::Window,
}

thread_local! {
    static INSTANCE: RefCell<Option<VmmSystray>> = const { RefCell::new(None) };
}

/// `vmmSystray`: the tray icon, shown while the `system-tray` setting
/// is on (or `--show-systray` forced it).
#[derive(Clone)]
pub struct VmmSystray(Rc<Inner>);

impl VmmSystray {
    /// `vmmSystray.get_instance()`.
    pub fn get_instance() -> VmmSystray {
        if let Some(this) = Self::instance() {
            return this;
        }
        let topwin = gtk::Window::new();
        let current_vm: Rc<RefCell<Option<VmmDomain>>> = Rc::default();
        let current = current_vm.clone();
        let actions = vmmenu::create_actions(&topwin, Rc::new(move || current.borrow().clone()));
        let this = VmmSystray(Rc::new(Inner {
            created: Cell::new(false),
            handle: RefCell::new(None),
            connected: Arc::new(AtomicBool::new(false)),
            embed_error: Arc::default(),
            shown: Cell::new(false),
            mainmenu: RefCell::new(None),
            pushed: RefCell::new(None),
            generation: Cell::new(0),
            push_scheduled: Cell::new(false),
            actions,
            current_vm,
            _topwin: topwin,
        }));
        INSTANCE.with(|i| i.replace(Some(this.clone())));
        engine::get().config().on_view_system_tray_changed(|| {
            if let Some(this) = Self::instance() {
                this.show_systray_changed();
            }
        });
        // This will trigger the actual UI showing
        this.show_systray_changed();
        this
    }

    fn instance() -> Option<VmmSystray> {
        INSTANCE.with(|i| i.borrow().clone())
    }

    fn downgrade(&self) -> Weak<Inner> {
        Rc::downgrade(&self.0)
    }

    fn upgrade(weak: &Weak<Inner>) -> Option<VmmSystray> {
        weak.upgrade().map(VmmSystray)
    }

    /// `is_embedded`: shown, and registered with a tray host.
    pub fn is_embedded(&self) -> bool {
        let running = self
            .0
            .handle
            .borrow()
            .as_ref()
            .is_some_and(|h| !h.is_closed());
        running && self.0.connected.load(Ordering::SeqCst) && self.0.shown.get()
    }

    /// Why the icon isn't embedded, when the D-Bus side said so.
    pub fn embed_error(&self) -> Option<String> {
        self.0.embed_error.lock().ok().and_then(|e| e.clone())
    }

    /// `show_from_cli`.
    pub fn show_from_cli(&self) {
        self.show_systray();
    }

    /// `_cleanup`, on app exit.
    pub fn cleanup(&self) {
        self.hide_systray();
        if let Some(handle) = self.0.handle.take() {
            // The request is sent right away; nothing to wait for on exit.
            std::mem::drop(handle.shutdown());
        }
        self.0.mainmenu.take();
    }

    // Initialization routines

    /// `_init_mainmenu`.
    fn init_mainmenu(&self) {
        self.0.mainmenu.replace(Some(TrayMainMenu::default()));
        let cm = engine::get().connmanager();
        let weak = self.downgrade();
        cm.connect_local("conn-added", false, move |args| {
            let conn = args[1].get::<VmmConnection>().ok()?;
            Self::upgrade(&weak)?.conn_added_cb(&conn);
            None
        });
        let weak = self.downgrade();
        cm.connect_local("conn-removed", false, move |args| {
            let uri = args[1].get::<String>().ok()?;
            Self::upgrade(&weak)?.conn_removed_cb(&uri);
            None
        });
        for conn in cm.conns() {
            self.conn_added_cb(&conn);
        }
    }

    /// `_show_systray`.
    fn show_systray(&self) {
        self.0.shown.set(true);
        if !self.0.created.replace(true) {
            self.init_mainmenu();
            self.spawn();
        }
        self.schedule_push();
    }

    /// `_hide_systray`.
    fn hide_systray(&self) {
        if !self.0.created.get() {
            return;
        }
        self.0.shown.set(false);
        self.schedule_push();
    }

    /// `_show_systray_changed_cb`.
    fn show_systray_changed(&self) {
        if engine::get().config().get_view_system_tray() {
            self.show_systray();
        } else {
            self.hide_systray();
        }
    }

    /// `_SystrayIndicator.__init__` + `set_menu`: start the ksni
    /// service. Without a session bus it just never embeds.
    fn spawn(&self) {
        let menu = self.build_menu();
        let active = self.0.shown.get();
        self.0.pushed.replace(Some((menu.clone(), active)));
        // Registered unless the watcher reports otherwise during spawn.
        self.0.connected.store(true, Ordering::SeqCst);
        let icon = TrayIcon {
            menu,
            generation: 0,
            active,
            connected: self.0.connected.clone(),
            embed_error: self.0.embed_error.clone(),
            pixmaps: icon_pixmaps(),
        };
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let res = icon.assume_sni_available(true).spawn().await;
            let Some(this) = Self::upgrade(&weak) else {
                return;
            };
            match res {
                Ok(handle) => {
                    this.0.handle.replace(Some(handle));
                    // Catch up with anything that changed meanwhile.
                    this.schedule_push();
                }
                Err(e) => {
                    this.0.connected.store(false, Ordering::SeqCst);
                    let text = format!("Error creating the system tray icon: {e}");
                    eprintln!("{text}");
                    set_embed_error(&this.0.embed_error, Some(text));
                }
            }
        });
    }

    fn build_menu(&self) -> Vec<MenuNode> {
        self.0
            .mainmenu
            .borrow()
            .as_ref()
            .map(TrayMainMenu::build)
            .unwrap_or_default()
    }

    /// Pushes the current menu and status to the ksni service, once per
    /// main loop iteration at most.
    fn schedule_push(&self) {
        if self.0.push_scheduled.replace(true) {
            return;
        }
        let weak = self.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(this) = Self::upgrade(&weak) {
                this.0.push_scheduled.set(false);
                this.push();
            }
        });
    }

    fn push(&self) {
        let Some(handle) = self.0.handle.borrow().clone() else {
            return;
        };
        let state = (self.build_menu(), self.0.shown.get());
        if self.0.pushed.borrow().as_ref() == Some(&state) {
            return;
        }
        self.0.pushed.replace(Some(state.clone()));
        let generation = self.0.generation.get() + 1;
        self.0.generation.set(generation);
        glib::spawn_future_local(async move {
            let (menu, active) = state;
            handle
                .update(move |tray| {
                    if generation > tray.generation {
                        tray.generation = generation;
                        tray.menu = menu;
                        tray.active = active;
                    }
                })
                .await;
        });
    }

    fn with_menu(&self, f: impl FnOnce(&mut TrayMainMenu)) {
        if let Some(menu) = self.0.mainmenu.borrow_mut().as_mut() {
            f(menu);
        }
        self.schedule_push();
    }

    /// `_set_vm_state`'s inputs, via the action menu's
    /// `update_widget_states`.
    fn vm_state(&self, vm: &VmmDomain) -> VmState {
        let actions = &self.0.actions;
        vmmenu::update_widget_states(actions, Some(vm));
        VmState {
            uuid: vm.get_uuid(),
            label: vm.get_name_or_title(),
            enabled: VM_ACTIONS
                .iter()
                .map(|&a| (a, actions.is_action_enabled(a)))
                .collect(),
            paused: vm.is_paused(),
        }
    }

    /// A `VMActionMenu` item: `VMActionUI.<action>(src, vm)`, unless the
    /// item has gone insensitive since the menu was drawn.
    fn vm_action(&self, uri: &str, uuid: &str, action: &str) {
        let vm = engine::get()
            .connmanager()
            .get_conn(uri)
            .and_then(|c| c.list_vms().into_iter().find(|v| v.get_uuid() == uuid));
        let Some(vm) = vm else {
            return;
        };
        let actions = &self.0.actions;
        vmmenu::update_widget_states(actions, Some(&vm));
        self.0.current_vm.replace(Some(vm));
        if actions.is_action_enabled(action) {
            actions.activate_action(action, None);
        }
    }

    // UI listeners

    /// `_conn_added_cb`.
    fn conn_added_cb(&self, conn: &VmmConnection) {
        let weak = self.downgrade();
        conn.connect_local("vm-added", false, move |args| {
            let conn = args[0].get::<VmmConnection>().ok()?;
            let vm = args[1].get::<VmmDomain>().ok()?;
            Self::upgrade(&weak)?.vm_added_cb(&conn, &vm);
            None
        });
        let weak = self.downgrade();
        conn.connect_local("vm-removed", false, move |args| {
            let conn = args[0].get::<VmmConnection>().ok()?;
            let vm = args[1].get::<VmmDomain>().ok()?;
            Self::upgrade(&weak)?.vm_removed_cb(&conn, &vm);
            None
        });
        let weak = self.downgrade();
        conn.connect_local("state-changed", false, move |args| {
            let conn = args[0].get::<VmmConnection>().ok()?;
            Self::upgrade(&weak)?.conn_state_changed_cb(&conn);
            None
        });
        let (uri, desc, active) = (conn.get_uri(), conn.get_pretty_desc(), conn.is_active());
        self.with_menu(|m| m.conn_add(&uri, &desc, active));
        for vm in conn.list_vms() {
            self.vm_added_cb(conn, &vm);
        }
    }

    /// `_conn_removed_cb`.
    fn conn_removed_cb(&self, uri: &str) {
        self.with_menu(|m| m.conn_remove(uri));
    }

    /// `_conn_state_changed_cb`.
    fn conn_state_changed_cb(&self, conn: &VmmConnection) {
        let (uri, desc, active) = (conn.get_uri(), conn.get_pretty_desc(), conn.is_active());
        self.with_menu(|m| m.conn_change(&uri, &desc, active));
    }

    /// `_vm_added_cb`.
    fn vm_added_cb(&self, conn: &VmmConnection, vm: &VmmDomain) {
        let weak = self.downgrade();
        vm.connect_local("state-changed", false, move |args| {
            let vm = args[0].get::<VmmDomain>().ok()?;
            Self::upgrade(&weak)?.vm_state_changed_cb(&vm);
            None
        });
        let state = self.vm_state(vm);
        let uri = conn.get_uri();
        self.with_menu(|m| m.vm_add(&uri, state));
    }

    /// `_vm_removed_cb`.
    fn vm_removed_cb(&self, conn: &VmmConnection, vm: &VmmDomain) {
        let (uri, uuid) = (conn.get_uri(), vm.get_uuid());
        self.with_menu(|m| m.vm_remove(&uri, &uuid));
    }

    /// `_vm_state_changed_cb`.
    fn vm_state_changed_cb(&self, vm: &VmmDomain) {
        let Some(conn) = vm.conn() else {
            return;
        };
        let state = self.vm_state(vm);
        let uri = conn.get_uri();
        self.with_menu(|m| m.vm_change(&uri, state));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vm(uuid: &str, label: &str, paused: bool, enabled: &[&'static str]) -> VmState {
        VmState {
            uuid: uuid.into(),
            label: label.into(),
            enabled: VM_ACTIONS
                .iter()
                .map(|&a| (a, enabled.contains(&a)))
                .collect(),
            paused,
        }
    }

    fn labels(nodes: &[MenuNode]) -> Vec<String> {
        nodes
            .iter()
            .map(|n| match n {
                MenuNode::Separator => "---".to_string(),
                MenuNode::Item { label, .. } | MenuNode::Submenu { label, .. } => label.clone(),
            })
            .collect()
    }

    fn submenu<'a>(nodes: &'a [MenuNode], label: &str) -> &'a [MenuNode] {
        nodes
            .iter()
            .find_map(|n| match n {
                MenuNode::Submenu {
                    label: l, children, ..
                } if l == label => Some(children.as_slice()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no submenu {label}"))
    }

    fn item<'a>(nodes: &'a [MenuNode], label: &str) -> (&'a str, bool, bool, &'a TrayAction) {
        nodes
            .iter()
            .find_map(|n| match n {
                MenuNode::Item {
                    label: l,
                    icon,
                    enabled,
                    visible,
                    action,
                } if l == label => Some((*icon, *enabled, *visible, action)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no item {label}"))
    }

    #[test]
    fn empty_menu_is_show_and_quit() {
        let menu = TrayMainMenu::default().build();
        assert_eq!(
            labels(&menu),
            ["---", "_Show Virtual Machine Manager", "_Quit"]
        );
        assert_eq!(
            item(&menu, "_Show Virtual Machine Manager").3,
            &TrayAction::ToggleManager
        );
        let quit = item(&menu, "_Quit");
        assert_eq!((quit.0, quit.3), ("gtk-quit", &TrayAction::Exit));
    }

    #[test]
    fn conns_sort_active_first_when_added_and_stay_put() {
        let mut m = TrayMainMenu::default();
        m.conn_add("qemu:///system", "QEMU/KVM", false);
        m.conn_add("test:///default", "test default", true);
        m.conn_add("lxc:///", "LXC", false);
        assert_eq!(
            labels(&m.build()),
            [
                "test default",
                "LXC",
                "QEMU/KVM",
                "---",
                "_Show Virtual Machine Manager",
                "_Quit"
            ]
        );
        // conn_change updates in place, without resorting.
        m.conn_change("qemu:///system", "QEMU/KVM", true);
        assert_eq!(labels(&m.build())[..3], ["test default", "LXC", "QEMU/KVM"]);
        m.conn_remove("lxc:///");
        assert_eq!(labels(&m.build())[..2], ["test default", "QEMU/KVM"]);
        assert_eq!(
            TrayMainMenu::conn_sortkey(false, "QEMU/KVM User session"),
            "1qemu/kvm user session"
        );
    }

    #[test]
    fn conn_submenu_shows_connect_or_disconnect() {
        let mut m = TrayMainMenu::default();
        m.conn_add("test:///default", "test default", false);
        let menu = m.build();
        let conn = submenu(&menu, "test default");
        assert_eq!(labels(conn), ["---", "_Disconnect", "C_onnect"]);
        let disc = item(conn, "_Disconnect");
        assert_eq!((disc.0, disc.2), ("gtk-disconnect", false));
        let connect = item(conn, "C_onnect");
        assert_eq!((connect.0, connect.2), ("gtk-connect", true));
        assert_eq!(
            connect.3,
            &TrayAction::ConnConnect("test:///default".into())
        );
        m.conn_change("test:///default", "test default", true);
        let menu = m.build();
        let conn = submenu(&menu, "test default");
        assert!(item(conn, "_Disconnect").2);
        assert!(!item(conn, "C_onnect").2);
    }

    #[test]
    fn vms_sorted_by_name_when_added() {
        let mut m = TrayMainMenu::default();
        m.conn_add("test:///default", "test default", true);
        m.vm_add("test:///default", vm("2", "vm_b", false, &[]));
        m.vm_add("test:///default", vm("1", "vm_a", false, &[]));
        m.vm_add("test:///default", vm("3", "Zeta", false, &[]));
        let menu = m.build();
        // Labels escape underscores: D-Bus menus read them as mnemonics.
        assert_eq!(
            labels(submenu(&menu, "test default")),
            ["Zeta", "vm__a", "vm__b", "---", "_Disconnect", "C_onnect"]
        );
        // A rename relabels in place.
        m.vm_change("test:///default", vm("3", "zz", false, &[]));
        m.vm_remove("test:///default", "1");
        let menu = m.build();
        assert_eq!(labels(submenu(&menu, "test default"))[..2], ["zz", "vm__b"]);
    }

    #[test]
    fn vm_submenu_matches_vm_action_menu() {
        let mut m = TrayMainMenu::default();
        m.conn_add("test:///default", "test default", true);
        let running = [
            "suspend", "shutdown", "reboot", "reset", "destroy", "save", "delete", "show",
        ];
        m.vm_add("test:///default", vm("u1", "test", false, &running));
        let menu = m.build();
        let vmmenu = submenu(submenu(&menu, "test default"), "test");
        assert_eq!(
            labels(vmmenu),
            [
                "_Run",
                "_Pause",
                "R_esume",
                "_Shut Down",
                "---",
                "Clone...",
                "Migrate...",
                "_Delete",
                "---",
                "_Open"
            ]
        );
        let run = item(vmmenu, "_Run");
        assert!(!run.1);
        assert_eq!(
            run.3,
            &TrayAction::Vm {
                uri: "test:///default".into(),
                uuid: "u1".into(),
                action: "run"
            }
        );
        // Pause shows while running, Resume while paused.
        assert!(item(vmmenu, "_Pause").2);
        assert!(!item(vmmenu, "R_esume").2);
        assert!(!item(vmmenu, "Clone...").1);
        let shutdown = submenu(vmmenu, "_Shut Down");
        assert_eq!(
            labels(shutdown),
            [
                "_Reboot",
                "_Shut Down",
                "F_orce Reset",
                "_Force Off",
                "---",
                "Sa_ve"
            ]
        );
        assert!(shutdown
            .iter()
            .all(|n| !matches!(n, MenuNode::Item { enabled: false, .. })));

        m.vm_change(
            "test:///default",
            vm("u1", "test", true, &["resume", "destroy", "save"]),
        );
        let menu = m.build();
        let vmmenu = submenu(submenu(&menu, "test default"), "test");
        assert!(!item(vmmenu, "_Pause").2);
        assert!(item(vmmenu, "R_esume").2);
        assert!(item(vmmenu, "R_esume").1);
        let sd = vmmenu
            .iter()
            .find(|n| matches!(n, MenuNode::Submenu { label, .. } if label == "_Shut Down"));
        assert!(matches!(sd, Some(MenuNode::Submenu { enabled: false, .. })));
    }

    #[test]
    fn pixmaps_are_argb() {
        // One opaque red pixel, one half-transparent blue, rowstride 8.
        let rgba = [255, 0, 0, 255, 0, 0, 255, 128];
        assert_eq!(
            rgba_to_argb(2, 1, 8, 4, &rgba),
            [255, 255, 0, 0, 128, 0, 0, 255]
        );
        // RGB with row padding.
        let rgb = [1, 2, 3, 0, 4, 5, 6, 0];
        assert_eq!(rgba_to_argb(1, 2, 4, 3, &rgb), [255, 1, 2, 3, 255, 4, 5, 6]);
    }
}
