//! Upstream's `virtManager/details/console.py` (`vmmConsolePages`): the
//! VM window's Console page, around the graphical viewer.
//!
//! VNC and SPICE go through [`crate::viewers::Viewer`]; the serial
//! consoles (VTE) are the next slice, and until then report that their
//! viewer isn't available.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use crate::details::{graphics_pretty_type_simple, serial_console_lists};
use crate::domain::VmmDomain;
use crate::error;
use crate::sshtunnels::{ConnectionInfo, GraphicsDev};
use crate::viewers::{Viewer, ViewerEvent};

// console-pages
const CONSOLE_PAGE_UNAVAILABLE: &str = "unavailable";
const CONSOLE_PAGE_SERIAL: &str = "serial";
const CONSOLE_PAGE_GRAPHICS: &str = "graphics";
const CONSOLE_PAGE_CONNECT: &str = "connect";

// console-gfx-pages
const GFX_PAGE_VIEWER: &str = "viewer";
const GFX_PAGE_AUTH: &str = "auth";
const GFX_PAGE_UNAVAILABLE: &str = "unavailable";

/// `build_keycombo_menu`'s entries: (accelerator shown, keys sent).
const KEYCOMBOS: &[&[(&str, &str)]] = &[
    &[
        ("<Control><Alt>BackSpace", "Control_L+Alt_L+BackSpace"),
        ("<Control><Alt>Delete", "Control_L+Alt_L+Delete"),
        (
            "<Control><Alt><Shift>Escape",
            "Control_L+Alt_L+Shift_L+Escape",
        ),
    ],
    &[
        ("<Control><Alt>F1", "Control_L+Alt_L+F1"),
        ("<Control><Alt>F2", "Control_L+Alt_L+F2"),
        ("<Control><Alt>F3", "Control_L+Alt_L+F3"),
        ("<Control><Alt>F4", "Control_L+Alt_L+F4"),
        ("<Control><Alt>F5", "Control_L+Alt_L+F5"),
        ("<Control><Alt>F6", "Control_L+Alt_L+F6"),
        ("<Control><Alt>F7", "Control_L+Alt_L+F7"),
        ("<Control><Alt>F8", "Control_L+Alt_L+F8"),
        ("<Control><Alt>F9", "Control_L+Alt_L+F9"),
        ("<Control><Alt>F10", "Control_L+Alt_L+F10"),
        ("<Control><Alt>F11", "Control_L+Alt_L+F11"),
        ("<Control><Alt>F12", "Control_L+Alt_L+F12"),
    ],
    &[("Print", "Print")],
];

/// `build_keycombo_menu`: items call `console.send-key` with the keys
/// (X keysym names joined by `+`) as target.
fn make_keycombo_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    for section in KEYCOMBOS {
        let sec = gio::Menu::new();
        for (accel, keys) in *section {
            let label = gtk::accelerator_parse(*accel)
                .map(|(key, mods)| gtk::accelerator_get_label(key, mods).to_string())
                .unwrap_or_else(|| accel.to_string());
            let item = gio::MenuItem::new(Some(&label), None);
            item.set_action_and_target_value(Some("console.send-key"), Some(&keys.to_variant()));
            sec.append_item(&item);
        }
        menu.append_section(None, &sec);
    }
    menu
}

/// `_cant_embed_graphics`.
fn cant_embed_graphics(ginfo: &ConnectionInfo) -> Option<String> {
    if matches!(ginfo.gtype.as_str(), "vnc" | "spice") {
        return None;
    }
    Some(format!(
        "Cannot display graphical console type '{}'",
        ginfo.gtype
    ))
}

#[derive(Clone, Debug, PartialEq)]
enum ConsoleDev {
    Graphics(Box<ConnectionInfo>),
    /// A `<serial>`/`<console>`, by tag and xml index.
    Serial(String, usize),
}

/// One `_ConsoleMenu` radio item.
#[derive(Clone, Debug)]
struct MenuEntry {
    label: String,
    dev: Option<ConsoleDev>,
    tooltip: Option<String>,
}

impl MenuEntry {
    fn sensitive(&self) -> bool {
        self.dev.is_some() && self.tooltip.is_none()
    }
}

/// `_build_graphical_menu_items`.
fn build_graphical_menu_items(vm: &VmmDomain) -> Vec<MenuEntry> {
    let mut ret = vec![];
    if let (Some(x), Some(conn)) = (vm.xmlobj(), vm.conn()) {
        let uri = conn.get_uri();
        let mut found_default = false;
        for gdev in GraphicsDev::all(&x.doc, x.root()) {
            let ginfo = ConnectionInfo::new(&uri, &gdev);
            let mut label = format!(
                "Graphical Console {}",
                graphics_pretty_type_simple(&gdev.gtype)
            );
            if gdev.idx > 0 {
                label += &format!(" {}", gdev.idx + 1);
            }
            let mut tooltip = cant_embed_graphics(&ginfo);
            if tooltip.is_none() {
                if !found_default {
                    found_default = true;
                } else {
                    tooltip = Some(
                        "virt-manager does not support more than one graphical console".into(),
                    );
                }
            }
            ret.push(MenuEntry {
                label,
                dev: Some(ConsoleDev::Graphics(Box::new(ginfo))),
                tooltip,
            });
        }
    }
    if ret.is_empty() {
        ret.push(MenuEntry {
            label: "Graphical console not configured for guest".into(),
            dev: None,
            tooltip: None,
        });
    }
    ret
}

/// `_build_serial_menu_items`.
fn build_serial_menu_items(vm: &VmmDomain) -> Vec<MenuEntry> {
    let mut ret = vec![];
    if let Some(x) = vm.xmlobj() {
        let doc = &x.doc;
        if let Some(devices) = x.root().find(doc, "devices") {
            let (serials, consoles) = serial_console_lists(doc, devices);
            let tagged = serials
                .into_iter()
                .map(|d| ("serial", d))
                .chain(consoles.into_iter().map(|d| ("console", d)));
            for (tag, (idx, el)) in tagged {
                let label = if tag == "console" {
                    format!("Text Console {}", idx + 1)
                } else {
                    format!("Serial {}", idx + 1)
                };
                // vmmSerialConsole.can_connect
                let ctype = el.attribute(doc, "type").unwrap_or_default().to_string();
                let tooltip = if ctype != "pty" {
                    Some(format!(
                        "Console for device type '{ctype}' is not supported"
                    ))
                } else {
                    Some("The serial console is not available in this version yet.".into())
                };
                ret.push(MenuEntry {
                    label,
                    dev: Some(ConsoleDev::Serial(tag.into(), idx)),
                    tooltip,
                });
            }
        }
    }
    if ret.is_empty() {
        ret.push(MenuEntry {
            label: "No text console available".into(),
            dev: None,
            tooltip: None,
        });
    }
    ret
}

/// What `vmmConsolePages` emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleEvent {
    PageChanged,
    LeaveFullscreen,
    ChangeTitle,
}

type Listener = Rc<dyn Fn(ConsoleEvent)>;

struct Inner {
    vm: VmmDomain,
    topwin: gtk::Window,
    top_box: gtk::Box,
    console_pages: gtk::Stack,
    unavailable: gtk::Label,
    gfx_pages: gtk::Stack,
    gfx_unavailable: gtk::Label,
    gfx_scroll: gtk::ScrolledWindow,
    gfx_viewport: gtk::Box,
    auth_password: gtk::PasswordEntry,
    auth_username: gtk::Entry,
    label_auth_password: gtk::Label,
    label_auth_username: gtk::Label,
    auth_remember: gtk::CheckButton,
    revealer: gtk::Revealer,
    revealer_timeout: RefCell<Option<glib::SourceId>>,
    keycombo_menu: gio::Menu,
    console_list_menu: gio::Menu,
    menu_entries: RefCell<Vec<MenuEntry>>,
    menu_selected: RefCell<Option<String>>,
    actions: gio::SimpleActionGroup,
    viewer: RefCell<Option<Viewer>>,
    viewer_connect_clicked: Cell<bool>,
    in_fullscreen: Cell<bool>,
    pointer_is_grabbed: Cell<bool>,
    callback: RefCell<Option<Listener>>,
}

#[derive(Clone)]
pub struct VmmConsolePages(Rc<Inner>);

fn centered_label() -> gtk::Label {
    let label = gtk::Label::new(None);
    label.set_justify(gtk::Justification::Center);
    label.set_wrap(true);
    label.set_selectable(true);
    label.set_width_chars(60);
    label.set_max_width_chars(60);
    label.set_halign(gtk::Align::Center);
    label.set_valign(gtk::Align::Center);
    label
}

impl VmmConsolePages {
    pub fn new(vm: &VmmDomain, topwin: &impl IsA<gtk::Window>) -> VmmConsolePages {
        let console_pages = gtk::Stack::new();
        console_pages.set_vexpand(true);
        console_pages.set_hexpand(true);

        let unavailable = centered_label();
        console_pages.add_named(&unavailable, Some(CONSOLE_PAGE_UNAVAILABLE));

        // serial-pages: VTE terminals arrive with the serial console port.
        let serial = centered_label();
        console_pages.add_named(&serial, Some(CONSOLE_PAGE_SERIAL));

        // console-gfx-pages
        let gfx_pages = gtk::Stack::new();
        let gfx_viewport = gtk::Box::new(gtk::Orientation::Vertical, 0);
        gfx_viewport.set_hexpand(true);
        gfx_viewport.set_vexpand(true);
        let gfx_scroll = gtk::ScrolledWindow::new();
        gfx_scroll.set_child(Some(&gfx_viewport));
        gfx_scroll.update_property(&[gtk::accessible::Property::Label("console-gfx-viewport")]);
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&gfx_scroll));
        let revealer = gtk::Revealer::new();
        overlay.add_overlay(&build_overlay_hotspot(&revealer));
        gfx_pages.add_named(&overlay, Some(GFX_PAGE_VIEWER));

        // console-auth
        let auth = gtk::Grid::new();
        auth.set_halign(gtk::Align::Center);
        auth.set_valign(gtk::Align::Center);
        auth.set_hexpand(true);
        auth.set_row_spacing(6);
        auth.set_column_spacing(6);
        let label_auth_password = gtk::Label::with_mnemonic("_Password:");
        label_auth_password.set_xalign(1.0);
        let label_auth_username = gtk::Label::with_mnemonic("_Username:");
        label_auth_username.set_xalign(1.0);
        let auth_username = gtk::Entry::new();
        let auth_password = gtk::PasswordEntry::new();
        auth_password.set_show_peek_icon(true);
        label_auth_username.set_mnemonic_widget(Some(&auth_username));
        label_auth_password.set_mnemonic_widget(Some(&auth_password));
        let auth_login = gtk::Button::with_mnemonic("_Login");
        let auth_remember = gtk::CheckButton::with_mnemonic("_Save this password in your keyring");
        auth_remember.set_tooltip_text(Some("Check to save password, uncheck to forget password."));
        auth.attach(&label_auth_username, 0, 0, 1, 1);
        auth.attach(&auth_username, 1, 0, 1, 1);
        auth.attach(&label_auth_password, 0, 1, 1, 1);
        auth.attach(&auth_password, 1, 1, 1, 1);
        auth.attach(&auth_login, 2, 1, 1, 1);
        auth.attach(&auth_remember, 1, 2, 1, 1);
        gfx_pages.add_named(&auth, Some(GFX_PAGE_AUTH));

        let gfx_unavailable = centered_label();
        gfx_pages.add_named(&gfx_unavailable, Some(GFX_PAGE_UNAVAILABLE));
        console_pages.add_named(&gfx_pages, Some(CONSOLE_PAGE_GRAPHICS));

        let connect_button = gtk::Button::with_mnemonic("_Connect to console");
        connect_button.set_halign(gtk::Align::Center);
        connect_button.set_valign(gtk::Align::Center);
        console_pages.add_named(&connect_button, Some(CONSOLE_PAGE_CONNECT));
        console_pages.update_property(&[gtk::accessible::Property::Label("console-pages")]);

        let top_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        top_box.append(&console_pages);

        let actions = gio::SimpleActionGroup::new();
        let keycombo_menu = make_keycombo_menu();

        let this = VmmConsolePages(Rc::new(Inner {
            vm: vm.clone(),
            topwin: topwin.clone().upcast(),
            top_box,
            console_pages,
            unavailable,
            gfx_pages,
            gfx_unavailable,
            gfx_scroll,
            gfx_viewport,
            auth_password,
            auth_username,
            label_auth_password,
            label_auth_username,
            auth_remember,
            revealer,
            revealer_timeout: RefCell::default(),
            keycombo_menu,
            console_list_menu: gio::Menu::new(),
            menu_entries: RefCell::default(),
            menu_selected: RefCell::default(),
            actions,
            viewer: RefCell::default(),
            viewer_connect_clicked: Cell::new(false),
            in_fullscreen: Cell::new(false),
            pointer_is_grabbed: Cell::new(false),
            callback: RefCell::default(),
        }));
        this.init_actions();
        this.init_overlay_toolbar();

        let weak = this.weak();
        let login = move || {
            if let Some(inner) = weak.upgrade() {
                VmmConsolePages(inner).set_credentials();
            }
        };
        let cb = login.clone();
        auth_login.connect_clicked(move |_| cb());
        this.0.auth_password.connect_activate(move |_| login());
        let weak = this.weak();
        connect_button.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                let this = VmmConsolePages(inner);
                this.0.viewer_connect_clicked.set(true);
                this.console_menu_view_selected();
            }
        });
        for stack in [&this.0.console_pages, &this.0.gfx_pages] {
            let weak = this.weak();
            stack.connect_visible_child_notify(move |_| {
                // _page_changed_cb: dispatched from idle so the UI size
                // can change first.
                let weak = weak.clone();
                glib::idle_add_local_once(move || {
                    if let Some(inner) = weak.upgrade() {
                        VmmConsolePages(inner).emit(ConsoleEvent::PageChanged);
                    }
                });
            });
        }

        this.activate_vm_unavailable_page("The console is currently unavailable");
        this
    }

    pub fn top_box(&self) -> &gtk::Box {
        &self.0.top_box
    }

    /// The `console` action group the Send Key and Consoles menus use.
    pub fn actions(&self) -> &gio::SimpleActionGroup {
        &self.0.actions
    }

    pub fn connect_event(&self, f: impl Fn(ConsoleEvent) + 'static) {
        self.0.callback.replace(Some(Rc::new(f)));
    }

    fn emit(&self, ev: ConsoleEvent) {
        let cb = self.0.callback.borrow().clone();
        if let Some(cb) = cb {
            cb(ev);
        }
    }

    fn weak(&self) -> Weak<Inner> {
        Rc::downgrade(&self.0)
    }

    fn viewer(&self) -> Option<Viewer> {
        self.0.viewer.borrow().clone()
    }

    fn init_actions(&self) {
        let send_key = gio::SimpleAction::new("send-key", Some(glib::VariantTy::STRING));
        let weak = self.weak();
        send_key.connect_activate(move |_, target| {
            let keys = target.and_then(|t| t.get::<String>()).unwrap_or_default();
            if let Some(inner) = weak.upgrade() {
                VmmConsolePages(inner).do_send_key(&keys);
            }
        });
        send_key.set_enabled(false);
        self.0.actions.add_action(&send_key);

        let select = gio::SimpleAction::new_stateful(
            "select",
            Some(glib::VariantTy::STRING),
            &"".to_variant(),
        );
        let weak = self.weak();
        select.connect_change_state(move |a, value| {
            let Some(value) = value else { return };
            a.set_state(value);
            if let Some(inner) = weak.upgrade() {
                let this = VmmConsolePages(inner);
                this.0.menu_selected.replace(value.get::<String>());
                this.console_menu_view_selected();
            }
        });
        self.0.actions.add_action(&select);
    }

    /// `vmmOverlayToolbar`: Leave Fullscreen + Send Key, revealed from
    /// the top edge while fullscreen.
    fn init_overlay_toolbar(&self) {
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        toolbar.add_css_class("toolbar");
        toolbar.add_css_class("osd");
        toolbar.update_property(&[gtk::accessible::Property::Label("Fullscreen Toolbar")]);

        let leave = gtk::Button::new();
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        content.append(&gtk::Image::from_icon_name("view-restore-symbolic"));
        content.append(&gtk::Label::new(Some("Leave Fullscreen")));
        leave.set_child(Some(&content));
        leave.set_tooltip_text(Some("Leave fullscreen"));
        leave.update_property(&[gtk::accessible::Property::Label("Fullscreen Exit")]);
        leave.add_css_class("flat");
        let weak = self.weak();
        leave.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                VmmConsolePages(inner).emit(ConsoleEvent::LeaveFullscreen);
            }
        });
        toolbar.append(&leave);

        let send_key = gtk::MenuButton::new();
        send_key.set_icon_name("preferences-desktop-keyboard-shortcuts-symbolic");
        send_key.set_tooltip_text(Some("Send key combination"));
        send_key.update_property(&[gtk::accessible::Property::Label("Fullscreen Send Key")]);
        send_key.set_menu_model(Some(&self.0.keycombo_menu));
        send_key.add_css_class("flat");
        toolbar.append(&send_key);

        self.0.revealer.set_child(Some(&toolbar));

        let hotspot = self
            .0
            .revealer
            .parent()
            .expect("revealer is in its hotspot");
        let motion = gtk::EventControllerMotion::new();
        let weak = self.weak();
        motion.connect_enter(move |_, _, _| {
            if let Some(inner) = weak.upgrade() {
                VmmConsolePages(inner).overlay_enter_notify(true);
            }
        });
        let weak = self.weak();
        motion.connect_leave(move |_| {
            if let Some(inner) = weak.upgrade() {
                VmmConsolePages(inner).overlay_enter_notify(false);
            }
        });
        hotspot.add_controller(motion);
    }

    /// `_TimedRevealer._enter_notify`.
    fn overlay_enter_notify(&self, entered: bool) {
        if !self.0.in_fullscreen.get() {
            return;
        }
        let revealer = &self.0.revealer;
        if !entered && revealer.reveals_child() {
            self.schedule_unreveal_timeout(1000);
            return;
        }
        self.unregister_unreveal_timeout();
        if entered && !revealer.reveals_child() {
            revealer.set_reveal_child(true);
        }
    }

    fn schedule_unreveal_timeout(&self, ms: u64) {
        if self.0.revealer_timeout.borrow().is_some() {
            return;
        }
        let weak = self.weak();
        let id = glib::timeout_add_local_once(Duration::from_millis(ms), move || {
            if let Some(inner) = weak.upgrade() {
                inner.revealer_timeout.replace(None);
                inner.revealer.set_reveal_child(false);
            }
        });
        self.0.revealer_timeout.replace(Some(id));
    }

    fn unregister_unreveal_timeout(&self) {
        if let Some(id) = self.0.revealer_timeout.take() {
            id.remove();
        }
    }

    /// `_TimedRevealer.force_reveal`.
    fn overlay_force_reveal(&self, val: bool) {
        self.unregister_unreveal_timeout();
        self.0.in_fullscreen.set(val);
        self.0.revealer.set_reveal_child(val);
        self.schedule_unreveal_timeout(2000);
    }

    // Internal APIs

    /// `_do_send_key`.
    fn do_send_key(&self, keys: &str) {
        if let Some(v) = self.viewer() {
            let keys: Vec<&str> = keys.split('+').filter(|k| !k.is_empty()).collect();
            v.console_send_keys(&keys);
        }
    }

    // Resize and scaling APIs

    fn sync_resizeguest_with_display(&self) {
        let Some(v) = self.viewer() else { return };
        v.console_set_resizeguest(self.0.vm.get_console_resizeguest() != 0);
    }

    /// `_set_size_to_vm`.
    fn set_size_to_vm(&self) {
        if !self.viewer_is_visible() {
            return;
        }
        let Some(v) = self.viewer() else { return };
        let (w, h) = v.console_get_preferred_size();
        if w <= 0 || h <= 0 {
            return;
        }
        let top = &self.0.topwin;
        let scroll = &self.0.gfx_scroll;
        let valw = w + (top.width() - scroll.width());
        let valh = h + (top.height() - scroll.height());
        top.unmaximize();
        top.set_default_size(valw, valh);
    }

    /// `_sync_scaling_with_display`.
    fn sync_scaling_with_display(&self) {
        let Some(v) = self.viewer() else { return };
        let scaling = match self.0.vm.get_console_scaling() {
            crate::config::CONSOLE_SCALE_NEVER => false,
            crate::config::CONSOLE_SCALE_ALWAYS => true,
            _ => self.0.in_fullscreen.get(),
        };
        v.console_set_scaling(scaling);
    }

    /// `_change_fullscreen`, minus the window state itself, which the
    /// VM window owns.
    fn change_fullscreen(&self, do_fullscreen: bool) {
        self.overlay_force_reveal(do_fullscreen);
        self.0.in_fullscreen.set(do_fullscreen);
        self.sync_scaling_with_display();
    }

    // State tracking

    fn show_vm_status_unavailable(&self) {
        if self.0.vm.is_crashed() {
            self.activate_vm_unavailable_page("Guest has crashed.");
        } else {
            self.activate_vm_unavailable_page("Guest is not running.");
        }
    }

    /// `_close_viewer`.
    fn close_viewer(&self) {
        if self.0.in_fullscreen.get() {
            self.emit(ConsoleEvent::LeaveFullscreen);
        }
        self.0.viewer_connect_clicked.set(false);
        let Some(viewer) = self.0.viewer.take() else {
            return;
        };
        viewer.cleanup();
        while let Some(child) = self.0.gfx_viewport.first_child() {
            self.0.gfx_viewport.remove(&child);
        }
        if self.0.pointer_is_grabbed.replace(false) {
            self.emit(ConsoleEvent::ChangeTitle);
        }
    }

    fn refresh_vm_state(&self) {
        self.activate_default_console_page();
    }

    // console page navigation

    fn activate_gfx_unavailable_page(&self, msg: &str) {
        self.close_viewer();
        self.0
            .gfx_pages
            .set_visible_child_name(GFX_PAGE_UNAVAILABLE);
        if !msg.is_empty() {
            self.0
                .gfx_unavailable
                .set_markup(&format!("<b>{}</b>", glib::markup_escape_text(msg)));
        }
    }

    /// `_activate_vm_unavailable_page`: the top level error page. When
    /// it's showing and the VM runs, the default console is connected.
    fn activate_vm_unavailable_page(&self, msg: &str) {
        self.close_viewer();
        self.0
            .console_pages
            .set_visible_child_name(CONSOLE_PAGE_UNAVAILABLE);
        if !msg.is_empty() {
            self.0
                .unavailable
                .set_markup(&format!("<b>{}</b>", glib::markup_escape_text(msg)));
        }
        self.activate_gfx_unavailable_page(msg);
    }

    /// `_activate_auth_page`. No keyring is wired up yet, so nothing
    /// is prefilled and "remember" stays insensitive, as upstream does
    /// without one.
    fn activate_auth_page(&self, with_password: bool, with_username: bool) {
        let i = &self.0;
        i.auth_password.set_visible(with_password);
        i.label_auth_password.set_visible(with_password);
        i.auth_username.set_visible(with_username);
        i.label_auth_username.set_visible(with_username);
        i.auth_username.set_text("");
        i.auth_password.set_text("");
        i.auth_remember.set_sensitive(false);
        i.auth_remember.set_active(false);
        i.gfx_pages.set_visible_child_name(GFX_PAGE_AUTH);
        if with_username {
            i.auth_username.grab_focus();
        } else {
            i.auth_password.grab_focus();
        }
    }

    fn activate_gfx_viewer_page(&self) {
        self.0
            .console_pages
            .set_visible_child_name(CONSOLE_PAGE_GRAPHICS);
        self.0.gfx_pages.set_visible_child_name(GFX_PAGE_VIEWER);
        if let Some(v) = self.viewer() {
            v.console_grab_focus();
        }
    }

    fn activate_console_connect_page(&self) {
        self.0
            .console_pages
            .set_visible_child_name(CONSOLE_PAGE_CONNECT);
    }

    fn viewer_is_visible(&self) -> bool {
        let i = &self.0;
        i.console_pages.is_visible()
            && i.console_pages.visible_child_name().as_deref() == Some(CONSOLE_PAGE_GRAPHICS)
            && i.gfx_pages.visible_child_name().as_deref() == Some(GFX_PAGE_VIEWER)
            && self.viewer().is_some_and(|v| v.console_is_open())
    }

    fn viewer_can_usb_redirect(&self) -> bool {
        self.viewer_is_visible()
            && self
                .viewer()
                .is_some_and(|v| v.console_has_usb_redirection())
    }

    // Viewer login attempts

    /// `_init_viewer`.
    fn init_viewer(&self, ginfo: Option<ConnectionInfo>, errmsg: Option<String>) {
        if self.viewer().is_some() || !self.0.topwin.is_visible() {
            return;
        }
        if let Some(errmsg) = errmsg {
            self.activate_gfx_unavailable_page(&errmsg);
            return;
        }
        let Some(ginfo) = ginfo else {
            return;
        };
        if !self.0.vm.get_console_autoconnect() && !self.0.viewer_connect_clicked.get() {
            self.activate_console_connect_page();
            return;
        }

        self.activate_gfx_unavailable_page("Connecting to graphical console for guest");

        eprintln!("Starting connect process for {}", ginfo.logstring());
        let viewer = if ginfo.gtype == "vnc" {
            Viewer::new_vnc(&self.0.vm, ginfo)
        } else {
            Viewer::new_spice(&self.0.vm, ginfo)
        };
        self.0.viewer.replace(Some(viewer.clone()));
        self.connect_viewer_signals(&viewer);
        let this = self.clone();
        glib::spawn_future_local(async move {
            if let Err(e) = viewer.console_open().await {
                // Only if this is still the current viewer.
                if this.viewer().is_some_and(|v| v.same(&viewer)) {
                    this.activate_gfx_unavailable_page(&format!(
                        "Error connecting to graphical console:\n{e}"
                    ));
                }
            }
        });
    }

    /// `_set_credentials`.
    fn set_credentials(&self) {
        let Some(v) = self.viewer() else { return };
        let i = &self.0;
        if i.auth_password.is_visible() {
            v.console_set_password(&i.auth_password.text());
        }
        if WidgetExt::is_visible(&i.auth_username) {
            v.console_set_username(&i.auth_username.text());
        }
    }

    // Viewer signal handling

    fn connect_viewer_signals(&self, viewer: &Viewer) {
        let weak = self.weak();
        viewer.connect_event(move |ev| {
            let Some(inner) = weak.upgrade() else { return };
            let this = VmmConsolePages(inner);
            match ev {
                ViewerEvent::AddDisplayWidget(w) => this.viewer_add_display(&w),
                ViewerEvent::DesktopResolutionChanged => {}
                ViewerEvent::PointerGrab => {
                    this.0.pointer_is_grabbed.set(true);
                    this.emit(ConsoleEvent::ChangeTitle);
                }
                ViewerEvent::PointerUngrab => {
                    this.0.pointer_is_grabbed.set(false);
                    this.emit(ConsoleEvent::ChangeTitle);
                }
                // _viewer_sync_modifiers: the display bypasses window
                // shortcuts itself while it holds the keyboard.
                ViewerEvent::KeyboardGrab | ViewerEvent::KeyboardUngrab => {}
                ViewerEvent::Connected => this.viewer_connected(),
                ViewerEvent::Disconnected(errdetails, ssherr) => {
                    this.viewer_disconnected(errdetails, ssherr)
                }
                ViewerEvent::AuthError(errmsg, will_disconnect) => {
                    this.viewer_auth_error(&errmsg, will_disconnect)
                }
                ViewerEvent::NeedAuth(pw, user) => this.activate_auth_page(pw, user),
            }
        });
    }

    fn viewer_add_display(&self, display: &gtk::Widget) {
        self.0.gfx_viewport.append(display);
        // Sync initial settings
        self.sync_scaling_with_display();
        self.sync_resizeguest_with_display();
    }

    fn viewer_auth_error(&self, errmsg: &str, viewer_will_disconnect: bool) {
        let errmsg = format!("Viewer authentication error: {errmsg}");
        let parent = self.0.topwin.clone();
        let msg = errmsg.clone();
        glib::spawn_future_local(async move {
            error::val_err(Some(&parent), &msg).await;
        });
        if viewer_will_disconnect {
            // GtkVNC will disconnect after an auth error, so do it for
            // them and re-init the viewer (triggered by
            // _refresh_vm_state if needed)
            self.activate_vm_unavailable_page(&errmsg);
        }
        self.refresh_vm_state();
    }

    /// `_viewer_disconnected_set_page`.
    fn viewer_disconnected_set_page(&self, errdetails: Option<String>, ssherr: String) {
        if self.0.vm.is_runable() {
            // Exit was probably for legitimate reasons
            self.show_vm_status_unavailable();
            return;
        }
        let msg = "Viewer was disconnected.";
        let mut errmsg = String::new();
        if let Some(d) = errdetails.filter(|d| !d.is_empty()) {
            errmsg += &format!("\n{d}");
        }
        if !ssherr.is_empty() {
            errmsg += &format!("\n\nSSH tunnel error output: {ssherr}");
        }
        if !errmsg.is_empty() {
            self.activate_gfx_unavailable_page(&format!("{msg}{errmsg}"));
            return;
        }
        // No clear graphics error that should block reconnecting, so use
        // the top level page, from which reconnecting is easier.
        self.activate_vm_unavailable_page(msg);
    }

    fn viewer_disconnected(&self, errdetails: Option<String>, ssherr: String) {
        self.activate_gfx_unavailable_page("Viewer is disconnecting.");
        self.viewer_disconnected_set_page(errdetails, ssherr);
    }

    fn viewer_connected(&self) {
        self.activate_gfx_viewer_page();
    }

    // Console list menu handling

    /// `_ConsoleMenu.rebuild_menu`.
    fn populate_console_menu(&self) {
        let vm = &self.0.vm;
        let graphics = build_graphical_menu_items(vm);
        let serials = build_serial_menu_items(vm);

        let mut oldlabel = self.0.menu_selected.borrow().clone();
        if oldlabel.as_ref().is_some_and(|l| {
            !graphics
                .iter()
                .chain(&serials)
                .any(|e| &e.label == l && e.sensitive())
        }) {
            oldlabel = None;
        }

        let menu = &self.0.console_list_menu;
        menu.remove_all();
        for group in [&graphics, &serials] {
            let section = gio::Menu::new();
            for entry in group.iter() {
                let sensitive = entry.sensitive();
                if oldlabel.is_none() && sensitive {
                    // Select the first selectable option
                    oldlabel = Some(entry.label.clone());
                }
                let item = gio::MenuItem::new(Some(&entry.label), None);
                if sensitive {
                    item.set_action_and_target_value(
                        Some("console.select"),
                        Some(&entry.label.to_variant()),
                    );
                } else {
                    // An action that doesn't exist: shown insensitive.
                    item.set_detailed_action("console.unavailable");
                }
                section.append_item(&item);
            }
            menu.append_section(None, &section);
        }

        let mut entries = graphics;
        entries.extend(serials);
        self.0.menu_entries.replace(entries);
        self.0.menu_selected.replace(oldlabel.clone());
        if let Some(a) = self
            .0
            .actions
            .lookup_action("select")
            .and_then(|a| a.downcast::<gio::SimpleAction>().ok())
        {
            a.set_state(&oldlabel.unwrap_or_default().to_variant());
        }
    }

    /// `_ConsoleMenu.get_selected`.
    fn get_selected(&self) -> Option<MenuEntry> {
        let entries = self.0.menu_entries.borrow();
        let selected = self.0.menu_selected.borrow();
        entries
            .iter()
            .find(|e| Some(&e.label) == selected.as_ref())
            .or_else(|| entries.first())
            .cloned()
    }

    /// `_console_menu_view_selected`.
    fn console_menu_view_selected(&self) {
        let Some(entry) = self.get_selected() else {
            return;
        };
        if self.0.vm.is_runable() {
            self.show_vm_status_unavailable();
            return;
        }
        match entry.dev {
            Some(ConsoleDev::Serial(..)) if entry.tooltip.is_none() => {
                self.0
                    .console_pages
                    .set_visible_child_name(CONSOLE_PAGE_SERIAL);
            }
            dev => {
                self.0
                    .console_pages
                    .set_visible_child_name(CONSOLE_PAGE_GRAPHICS);
                let ginfo = match dev {
                    Some(ConsoleDev::Graphics(g)) => Some(*g),
                    _ => None,
                };
                // A graphics entry's tooltip is the reason it can't be
                // embedded; a missing device shows its label.
                let errmsg = entry
                    .tooltip
                    .clone()
                    .or_else(|| ginfo.is_none().then(|| entry.label.clone()));
                let weak = self.weak();
                glib::idle_add_local_once(move || {
                    if let Some(inner) = weak.upgrade() {
                        VmmConsolePages(inner).init_viewer(ginfo, errmsg);
                    }
                });
            }
        }
    }

    /// `_toggle_first_console_menu_item`.
    fn toggle_first_console_menu_item(&self) {
        self.populate_console_menu();
        let first = self
            .0
            .menu_entries
            .borrow()
            .iter()
            .find(|e| e.sensitive())
            .map(|e| e.label.clone());
        if let Some(label) = first {
            self.0.menu_selected.replace(Some(label));
        }
        self.console_menu_view_selected();
    }

    /// `_activate_default_console_page`.
    fn activate_default_console_page(&self) {
        self.populate_console_menu();
        if self.0.vm.is_runable() {
            self.show_vm_status_unavailable();
            return;
        }
        if self.0.vm.xmlobj().is_none() {
            // The XML is still being fetched; the VM window refreshes
            // this page again once it arrives.
            return;
        }
        if self.viewer().is_some_and(|v| v.console_is_open()) {
            return;
        }
        if self.0.console_pages.visible_child_name().as_deref() != Some(CONSOLE_PAGE_UNAVAILABLE) {
            return;
        }
        // The VM was presumably just started: connect the default page.
        self.toggle_first_console_menu_item();
    }

    // API used by vmmVMWindow

    pub fn vmwindow_viewer_can_usb_redirect(&self) -> bool {
        self.viewer_can_usb_redirect()
    }

    pub fn vmwindow_viewer_get_framebuffer(&self) -> Option<crate::display::FrameCopy> {
        self.viewer().and_then(|v| v.console_get_framebuffer())
    }

    pub fn vmwindow_close(&self) {
        self.activate_vm_unavailable_page("Viewer window closed.");
    }

    /// `vmwindow_get_title_message`.
    pub fn vmwindow_get_title_message(&self) -> Option<String> {
        if !self.0.pointer_is_grabbed.get() {
            return None;
        }
        let keystr = self.viewer()?.console_get_grab_keys();
        Some(format!("Press {keystr} to release pointer."))
    }

    pub fn vmwindow_activate_default_console_page(&self) {
        self.activate_default_console_page();
    }

    pub fn vmwindow_refresh_vm_state(&self) {
        self.refresh_vm_state();
    }

    pub fn vmwindow_set_size_to_vm(&self) {
        self.set_size_to_vm();
    }

    pub fn vmwindow_set_fullscreen(&self, do_fullscreen: bool) {
        self.change_fullscreen(do_fullscreen);
    }

    pub fn vmwindow_get_keycombo_menu(&self) -> &gio::Menu {
        &self.0.keycombo_menu
    }

    pub fn vmwindow_get_console_list_menu(&self) -> &gio::Menu {
        &self.0.console_list_menu
    }

    pub fn vmwindow_get_viewer_is_visible(&self) -> bool {
        self.viewer_is_visible()
    }

    pub fn vmwindow_sync_scaling_with_display(&self) {
        self.sync_scaling_with_display();
    }

    pub fn vmwindow_sync_resizeguest_with_display(&self) {
        self.sync_resizeguest_with_display();
    }
}

/// `_TimedRevealer`'s event box: the revealer at the top centre, in a
/// box that keeps a 1px strip to catch the pointer while hidden.
fn build_overlay_hotspot(revealer: &gtk::Revealer) -> gtk::Box {
    let hotspot = gtk::Box::new(gtk::Orientation::Vertical, 0);
    hotspot.set_halign(gtk::Align::Center);
    hotspot.set_valign(gtk::Align::Start);
    hotspot.set_size_request(-1, 1);
    revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    hotspot.append(revealer);
    hotspot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keycombo_targets_are_keysym_names() {
        for section in KEYCOMBOS {
            for (_, keys) in *section {
                for k in keys.split('+') {
                    assert!(gtk::gdk::Key::from_name(k).is_some(), "{k}");
                }
            }
        }
    }

    #[test]
    fn cant_embed() {
        let mut g = ConnectionInfo::new("qemu:///system", &GraphicsDev::default());
        g.gtype = "vnc".into();
        assert_eq!(cant_embed_graphics(&g), None);
        g.gtype = "sdl".into();
        assert_eq!(
            cant_embed_graphics(&g).as_deref(),
            Some("Cannot display graphical console type 'sdl'")
        );
    }
}
