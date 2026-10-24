//! Upstream's `virtManager/vmwindow.py` + `ui/vmwindow.ui` (`vmmVMWindow`):
//! the per-VM window holding the Console, Details and Snapshots pages,
//! with its menubar (File, Virtual Machine, View, Send Key) and
//! toolbar (Console/Details toggles, Run, Pause, Shut Down, Snapshots,
//! Fullscreen).
//!
//! One window per VM, keyed by `uri+uuid` like upstream. The
//! "customize before install" mode arrives with the Create VM wizard.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use crate::config::{CONSOLE_SCALE_ALWAYS, CONSOLE_SCALE_FULLSCREEN, CONSOLE_SCALE_NEVER};
use crate::console::{ConsoleEvent, VmmConsolePages};
use crate::details::VmmDetails;
use crate::domain::VmmDomain;
use crate::engine;
use crate::error;
use crate::manager::{tool_button, tool_content};
use crate::vmmenu;

/// `DETAILS_PAGE_*`, as the stack's child names.
const PAGE_DETAILS: &str = "details";
const PAGE_CONSOLE: &str = "console";
const PAGE_SNAPSHOTS: &str = "snapshots";

thread_local! {
    static INSTANCES: RefCell<HashMap<String, VmmVMWindow>> = RefCell::default();
}

struct Inner {
    vm: VmmDomain,
    window: gtk::ApplicationWindow,
    menubar: gtk::PopoverMenuBar,
    toolbar_box: gtk::Box,
    pages: gtk::Stack,
    console: VmmConsolePages,
    details: VmmDetails,
    control_console: gtk::ToggleButton,
    control_details: gtk::ToggleButton,
    control_snapshots: gtk::ToggleButton,
    control_run: gtk::Button,
    control_pause: gtk::ToggleButton,
    control_shutdown: gtk::Button,
    control_shutdown_menu: gtk::MenuButton,
    control_fullscreen: gtk::ToggleButton,
    win_actions: gio::SimpleActionGroup,
    vm_actions: gio::SimpleActionGroup,
    /// `ignoreDetails`: set while syncing the page toggles.
    ignore_details: Cell<bool>,
    pause_guard: Cell<bool>,
    /// The Run/Restore label the Virtual Machine menu was built with,
    /// and whether it showed Resume, so it's rebuilt only on change.
    vm_menu_state: Cell<Option<(bool, bool)>>,
    window_size: Cell<Option<(i32, i32)>>,
    handlers: RefCell<Vec<(glib::Object, glib::SignalHandlerId)>>,
    settings_handlers: RefCell<Vec<(gio::Settings, glib::SignalHandlerId)>>,
}

#[derive(Clone)]
pub struct VmmVMWindow(Rc<Inner>);

fn stateful(
    group: &gio::SimpleActionGroup,
    name: &str,
    state: glib::Variant,
    weak: &Weak<Inner>,
    cb: fn(&VmmVMWindow, &glib::Variant),
) -> gio::SimpleAction {
    let ptype = (state.type_() != glib::VariantTy::BOOLEAN).then(|| state.type_().to_owned());
    let action = gio::SimpleAction::new_stateful(name, ptype.as_deref(), &state);
    let weak = weak.clone();
    action.connect_change_state(move |a, value| {
        let Some(value) = value else { return };
        a.set_state(value);
        if let Some(inner) = weak.upgrade() {
            cb(&VmmVMWindow(inner), value);
        }
    });
    if ptype.is_none() {
        // Boolean toggles: activating flips the state.
        action.connect_activate(|a, _| {
            let cur = a.state().and_then(|s| s.get::<bool>()).unwrap_or(false);
            a.change_state(&(!cur).to_variant());
        });
    }
    group.add_action(&action);
    action
}

fn simple(
    group: &gio::SimpleActionGroup,
    name: &str,
    weak: &Weak<Inner>,
    cb: fn(&VmmVMWindow),
) -> gio::SimpleAction {
    let action = gio::SimpleAction::new(name, None);
    let weak = weak.clone();
    action.connect_activate(move |_, _| {
        if let Some(inner) = weak.upgrade() {
            cb(&VmmVMWindow(inner));
        }
    });
    group.add_action(&action);
    action
}

fn action(group: &gio::SimpleActionGroup, name: &str) -> gio::SimpleAction {
    group
        .lookup_action(name)
        .and_then(|a| a.downcast::<gio::SimpleAction>().ok())
        .expect("action exists")
}

/// The View menu (`view2_menu`).
fn build_view_menu(console: &VmmConsolePages) -> gio::Menu {
    let view = gio::Menu::new();
    let pages = gio::Menu::new();
    for (label, page) in [
        ("_Console", PAGE_CONSOLE),
        ("_Details", PAGE_DETAILS),
        ("Sna_pshots", PAGE_SNAPSHOTS),
    ] {
        let item = gio::MenuItem::new(Some(label), None);
        item.set_action_and_target_value(Some("vmwin.page"), Some(&page.to_variant()));
        pages.append_item(&item);
    }
    view.append_section(None, &pages);

    let display = gio::Menu::new();
    display.append(Some("_Fullscreen"), Some("vmwin.fullscreen"));
    display.append(Some("_Resize to VM"), Some("vmwin.size-to-vm"));
    let scale = gio::Menu::new();
    let scale_modes = gio::Menu::new();
    for (label, mode) in [
        ("_Always", CONSOLE_SCALE_ALWAYS),
        ("_Only when Fullscreen", CONSOLE_SCALE_FULLSCREEN),
        ("_Never", CONSOLE_SCALE_NEVER),
    ] {
        let item = gio::MenuItem::new(Some(label), None);
        item.set_action_and_target_value(Some("vmwin.scale"), Some(&mode.to_variant()));
        scale_modes.append_item(&item);
    }
    scale.append_section(None, &scale_modes);
    let resize = gio::Menu::new();
    resize.append(
        Some("Auto _resize VM with window"),
        Some("vmwin.resizeguest"),
    );
    scale.append_section(None, &resize);
    display.append_submenu(Some("_Scale Display"), &scale);
    display.append_submenu(Some("Co_nsoles"), console.vmwindow_get_console_list_menu());
    display.append(Some("_Autoconnect"), Some("vmwin.autoconnect"));
    view.append_section(None, &display);

    let toolbar = gio::Menu::new();
    toolbar.append(Some("T_oolbar"), Some("vmwin.toolbar"));
    view.append_section(None, &toolbar);
    view
}

/// The Virtual Machine menu: `VMActionMenu(show_open=False)` plus the
/// window's own Take Screenshot / Redirect USB device entries.
fn build_vm_menu(vm: &VmmDomain) -> gio::Menu {
    let menu = vmmenu::action_menu(Some(vm), false);
    let extra = gio::Menu::new();
    extra.append(Some("_Take Screenshot"), Some("vmwin.screenshot"));
    extra.append(Some("_Redirect USB device"), Some("vmwin.usb-redirection"));
    menu.append_section(None, &extra);
    menu
}

fn build_menubar(vm: &VmmDomain, console: &VmmConsolePages) -> gio::Menu {
    let menubar = gio::Menu::new();
    let file = gio::Menu::new();
    let f1 = gio::Menu::new();
    f1.append(Some("_View Manager"), Some("vmwin.view-manager"));
    file.append_section(None, &f1);
    let f2 = gio::Menu::new();
    f2.append(Some("_Close"), Some("vmwin.close"));
    f2.append(Some("_Quit"), Some("vmwin.quit"));
    file.append_section(None, &f2);
    menubar.append_submenu(Some("_File"), &file);
    menubar.append_submenu(Some("Virtual _Machine"), &build_vm_menu(vm));
    menubar.append_submenu(Some("_View"), &build_view_menu(console));
    menubar.append_submenu(Some("Send _Key"), console.vmwindow_get_keycombo_menu());
    menubar
}

fn page_toggle(icon: &str, label: &str, tooltip: &str) -> gtk::ToggleButton {
    let btn = gtk::ToggleButton::new();
    btn.set_child(Some(&tool_content(icon, None)));
    btn.set_tooltip_text(Some(tooltip));
    btn.update_property(&[gtk::accessible::Property::Label(label)]);
    btn.add_css_class("flat");
    btn
}

impl VmmVMWindow {
    /// `vmmVMWindow.get_instance(parentobj, vm).show()`.
    pub fn show_instance(vm: &VmmDomain) {
        let Some(conn) = vm.conn() else {
            return;
        };
        let key = format!("{}+{}", conn.get_uri(), vm.get_uuid());
        let this = INSTANCES.with(|i| {
            i.borrow_mut()
                .entry(key)
                .or_insert_with(|| VmmVMWindow::new(vm))
                .clone()
        });
        this.show();
    }

    fn new(vm: &VmmDomain) -> VmmVMWindow {
        let app = engine::get().app();
        let window = gtk::ApplicationWindow::builder()
            .application(&app)
            .hide_on_close(true)
            .build();
        app.set_accels_for_action("vmwin.close", &["<Control><Shift>w"]);
        app.set_accels_for_action("vmwin.quit", &["<Control><Shift>q"]);

        let console = VmmConsolePages::new(vm, &window);
        let details = VmmDetails::new(vm, false);

        let menubar = gtk::PopoverMenuBar::from_model(Some(&build_menubar(vm, &console)));

        // details-toolbar
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        toolbar.add_css_class("toolbar");
        let control_console = page_toggle("icon_console", "Console", "Show the graphical console");
        let control_details = page_toggle(
            "dialog-information",
            "Details",
            "Show virtual hardware details",
        );
        control_details.set_group(Some(&control_console));
        control_console.set_active(true);
        let control_run = tool_button(
            "media-playback-start-symbolic",
            None,
            "Power on the virtual machine",
        );
        let control_pause = gtk::ToggleButton::new();
        control_pause.set_child(Some(&tool_content("media-playback-pause-symbolic", None)));
        control_pause.set_tooltip_text(Some("Pause the virtual machine"));
        control_pause.add_css_class("flat");
        let control_shutdown = tool_button(
            "system-shutdown-symbolic",
            None,
            "Shut down the virtual machine",
        );
        let control_shutdown_menu = gtk::MenuButton::new();
        control_shutdown_menu.set_menu_model(Some(&vmmenu::shutdown_menu()));
        control_shutdown_menu.add_css_class("flat");
        let shutdown_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        shutdown_box.add_css_class("linked");
        shutdown_box.append(&control_shutdown);
        shutdown_box.append(&control_shutdown_menu);
        let control_snapshots = page_toggle("vm_clone_wizard", "Snapshots", "Manage VM snapshots");
        control_snapshots.set_group(Some(&control_console));
        let control_fullscreen = gtk::ToggleButton::new();
        control_fullscreen.set_child(Some(&tool_content("view-fullscreen-symbolic", None)));
        control_fullscreen.set_tooltip_text(Some("Switch to fullscreen view"));
        control_fullscreen.add_css_class("flat");
        control_fullscreen.set_hexpand(true);
        control_fullscreen.set_halign(gtk::Align::End);
        toolbar.append(&control_console);
        toolbar.append(&control_details);
        toolbar.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        toolbar.append(&control_run);
        toolbar.append(&control_pause);
        toolbar.append(&shutdown_box);
        toolbar.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        toolbar.append(&control_snapshots);
        toolbar.append(&control_fullscreen);
        let toolbar_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        toolbar_box.append(&toolbar);

        let snapshots = gtk::Label::new(Some(
            "Snapshot management is not available yet in this version.",
        ));
        let pages = gtk::Stack::new();
        pages.set_vexpand(true);
        pages.add_named(details.top_box(), Some(PAGE_DETAILS));
        pages.add_named(console.top_box(), Some(PAGE_CONSOLE));
        pages.add_named(&snapshots, Some(PAGE_SNAPSHOTS));

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
        vbox.append(&menubar);
        vbox.append(&toolbar_box);
        vbox.append(&pages);
        window.set_child(Some(&vbox));

        let (w, h) = vm.get_details_window_size();
        if w <= 0 || h <= 0 {
            // _set_initial_window_size: a 1280x800 viewer plus chrome;
            // GTK4 sizes the window to its content's request, so the
            // default size is the viewer size.
            window.set_default_size(1280, 800);
        } else {
            window.set_default_size(w, h);
        }

        let inner = Rc::new_cyclic(|weak: &Weak<Inner>| {
            let win_actions = gio::SimpleActionGroup::new();
            simple(&win_actions, "view-manager", weak, |_| {
                engine::get().get_manager().show();
            });
            simple(&win_actions, "close", weak, VmmVMWindow::close);
            simple(&win_actions, "quit", weak, |_| engine::get().exit_app());
            simple(
                &win_actions,
                "screenshot",
                weak,
                VmmVMWindow::control_vm_screenshot,
            );
            simple(&win_actions, "usb-redirection", weak, |_| {});
            simple(&win_actions, "size-to-vm", weak, |w| {
                w.0.console.vmwindow_set_size_to_vm()
            });
            stateful(
                &win_actions,
                "page",
                PAGE_CONSOLE.to_variant(),
                weak,
                |w, v| {
                    if let Some(page) = v.get::<String>() {
                        w.details_console_changed(&page);
                    }
                },
            );
            stateful(
                &win_actions,
                "fullscreen",
                false.to_variant(),
                weak,
                |w, v| w.fullscreen_changed(v.get::<bool>().unwrap_or(false)),
            );
            stateful(
                &win_actions,
                "scale",
                CONSOLE_SCALE_ALWAYS.to_variant(),
                weak,
                |w, v| {
                    if let Some(mode) = v.get::<i32>() {
                        w.0.vm.set_console_scaling(mode);
                    }
                },
            );
            stateful(
                &win_actions,
                "resizeguest",
                false.to_variant(),
                weak,
                |w, v| {
                    w.0.vm
                        .set_console_resizeguest(v.get::<bool>().unwrap_or(false) as i32);
                    w.0.console.vmwindow_sync_resizeguest_with_display();
                },
            );
            stateful(
                &win_actions,
                "autoconnect",
                false.to_variant(),
                weak,
                |w, v| {
                    w.0.vm
                        .set_console_autoconnect(v.get::<bool>().unwrap_or(false))
                },
            );
            stateful(&win_actions, "toolbar", true.to_variant(), weak, |w, v| {
                w.toggle_toolbar(v.get::<bool>().unwrap_or(true))
            });

            let vm_for_actions = vm.clone();
            let vm_actions = vmmenu::create_actions(
                window.upcast_ref(),
                Rc::new(move || Some(vm_for_actions.clone())),
            );

            Inner {
                vm: vm.clone(),
                window,
                menubar,
                toolbar_box,
                pages,
                console,
                details,
                control_console,
                control_details,
                control_snapshots,
                control_run,
                control_pause,
                control_shutdown,
                control_shutdown_menu,
                control_fullscreen,
                win_actions,
                vm_actions,
                ignore_details: Cell::new(false),
                pause_guard: Cell::new(false),
                vm_menu_state: Cell::new(None),
                window_size: Cell::new(None),
                handlers: RefCell::default(),
                settings_handlers: RefCell::default(),
            }
        });
        let this = VmmVMWindow(inner);
        this.connect_signals();
        this.init_menus();
        this.sync_console_page_menu_state();
        this.console_refresh_scaling_from_settings();
        this.console_refresh_resizeguest_from_settings();
        this.console_refresh_autoconnect_from_settings();
        this.refresh_vm_state();
        this.activate_default_page();
        this
    }

    fn connect_signals(&self) {
        let i = &self.0;
        i.window.insert_action_group("vmwin", Some(&i.win_actions));
        i.window
            .insert_action_group(vmmenu::GROUP, Some(&i.vm_actions));
        i.window
            .insert_action_group("console", Some(i.console.actions()));

        let weak = Rc::downgrade(&self.0);
        let with = move |f: fn(&VmmVMWindow)| {
            let weak = weak.clone();
            move || {
                if let Some(inner) = weak.upgrade() {
                    f(&VmmVMWindow(inner));
                }
            }
        };

        for (btn, page) in [
            (&i.control_console, PAGE_CONSOLE),
            (&i.control_details, PAGE_DETAILS),
            (&i.control_snapshots, PAGE_SNAPSHOTS),
        ] {
            let weak = Rc::downgrade(&self.0);
            btn.connect_toggled(move |b| {
                if !b.is_active() {
                    return;
                }
                if let Some(inner) = weak.upgrade() {
                    VmmVMWindow(inner).details_console_changed(page);
                }
            });
        }
        let cb = with(VmmVMWindow::control_vm_run);
        i.control_run.connect_clicked(move |_| cb());
        let cb = with(VmmVMWindow::control_vm_shutdown);
        i.control_shutdown.connect_clicked(move |_| cb());
        let cb = with(VmmVMWindow::control_vm_pause);
        i.control_pause.connect_toggled(move |_| cb());
        let weak = Rc::downgrade(&self.0);
        i.control_fullscreen.connect_toggled(move |b| {
            // control_fullscreen: mirror into the menu's state.
            if let Some(inner) = weak.upgrade() {
                let action = action(&inner.win_actions, "fullscreen");
                if action.state().and_then(|s| s.get::<bool>()) != Some(b.is_active()) {
                    action.change_state(&b.is_active().to_variant());
                }
            }
        });

        let weak = Rc::downgrade(&self.0);
        i.console.connect_event(move |ev| {
            let Some(inner) = weak.upgrade() else { return };
            let this = VmmVMWindow(inner);
            match ev {
                ConsoleEvent::PageChanged => this.sync_console_page_menu_state(),
                // This will trigger de-fullscreening in a roundabout way
                ConsoleEvent::LeaveFullscreen => this.0.control_fullscreen.set_active(false),
                ConsoleEvent::ChangeTitle => this.refresh_title(),
            }
        });

        let cb = with(VmmVMWindow::close);
        i.window.connect_close_request(move |_| {
            cb();
            glib::Propagation::Stop
        });
        let weak = Rc::downgrade(&self.0);
        let size_cb = move |w: &gtk::ApplicationWindow| {
            // window_resized
            if let Some(inner) = weak.upgrade() {
                if w.is_visible() && !w.is_fullscreen() {
                    inner.window_size.set(Some(w.default_size()));
                }
            }
        };
        let cb = size_cb.clone();
        i.window.connect_default_width_notify(move |w| cb(w));
        i.window.connect_default_height_notify(move |w| size_cb(w));

        let vm = &i.vm;
        let cb = with(VmmVMWindow::vm_state_changed);
        let id = vm.connect_local("state-changed", false, move |_| {
            cb();
            None
        });
        i.handlers.borrow_mut().push((vm.clone().upcast(), id));
        if let Some(conn) = vm.conn() {
            let weak = Rc::downgrade(&self.0);
            let id = conn.connect_local("vm-removed", false, move |args| {
                let removed = args[1].get::<VmmDomain>().ok();
                if let Some(inner) = weak.upgrade() {
                    if removed.as_ref() == Some(&inner.vm) {
                        VmmVMWindow(inner).cleanup();
                    }
                }
                None
            });
            i.handlers.borrow_mut().push((conn.upcast(), id));
        }

        let settings = engine::get().config().vm_settings(&vm.get_uuid());
        let cb = with(VmmVMWindow::console_refresh_scaling_from_settings);
        let id = vm.on_console_scaling_changed(cb);
        i.settings_handlers
            .borrow_mut()
            .push((settings.clone(), id));
        let cb = with(VmmVMWindow::console_refresh_resizeguest_from_settings);
        let id = vm.on_console_resizeguest_changed(cb);
        i.settings_handlers
            .borrow_mut()
            .push((settings.clone(), id));
        let cb = with(VmmVMWindow::console_refresh_autoconnect_from_settings);
        let id = vm.on_console_autoconnect_changed(cb);
        i.settings_handlers.borrow_mut().push((settings, id));
    }

    fn init_menus(&self) {
        let toolbar = engine::get().config().get_details_show_toolbar();
        action(&self.0.win_actions, "toolbar").set_state(&toolbar.to_variant());
    }

    fn is_visible(&self) -> bool {
        self.0.window.is_visible()
    }

    pub fn show(&self) {
        let vis = self.is_visible();
        self.0.window.present();
        if vis {
            return;
        }
        engine::get().increment_window_counter();
        self.refresh_vm_state();
        self.refresh_xml();
    }

    /// Re-reads the XML, then refreshes whatever depends on it.
    fn refresh_xml(&self) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            if let Err(e) = this.0.vm.refresh_xml().await {
                error::show_err(
                    Some(this.0.window.upcast_ref()),
                    &format!("Error refreshing VM XML: {e}"),
                    None,
                    "",
                )
                .await;
                return;
            }
            this.refresh_current_page(None);
            this.sync_console_page_menu_state();
        });
    }

    pub fn close(&self) {
        self.close_inner();
    }

    /// `_close`.
    fn close_inner(&self) {
        let i = &self.0;
        let fs = action(&i.win_actions, "fullscreen");
        if fs.state().and_then(|s| s.get::<bool>()) == Some(true) {
            fs.change_state(&false.to_variant());
        }
        if !self.is_visible() {
            return;
        }
        if let Some((w, h)) = i.window_size.get() {
            i.vm.set_details_window_size(w, h);
        }
        i.window.set_visible(false);
        i.console.vmwindow_close();
        i.details.vmwindow_close();
        engine::get().decrement_window_counter();
    }

    /// `_cleanup` on `vm-removed`: close and forget this window.
    fn cleanup(&self) {
        self.close_inner();
        let i = &self.0;
        for (obj, id) in i.handlers.take() {
            obj.disconnect(id);
        }
        for (settings, id) in i.settings_handlers.take() {
            settings.disconnect(id);
        }
        if let Some(conn) = i.vm.conn() {
            let key = format!("{}+{}", conn.get_uri(), i.vm.get_uuid());
            INSTANCES.with(|m| m.borrow_mut().remove(&key));
        }
        i.window.destroy();
    }

    // Window state listeners

    fn toggle_toolbar(&self, active: bool) {
        engine::get().config().set_details_show_toolbar(active);
        let fsactive = action(&self.0.win_actions, "fullscreen")
            .state()
            .and_then(|s| s.get::<bool>())
            .unwrap_or(false);
        self.0.toolbar_box.set_visible(active && !fsactive);
    }

    /// `details_console_changed`: a page toggle (toolbar or View menu)
    /// was activated.
    fn details_console_changed(&self, page: &str) {
        let i = &self.0;
        if i.ignore_details.get() {
            return;
        }
        let current = i.pages.visible_child_name();
        if page == PAGE_SNAPSHOTS && !i.control_snapshots.is_sensitive() {
            // details-menu-view-snapshots is insensitive upstream.
            self.sync_toolbar_page_buttons(current.as_deref().unwrap_or(PAGE_CONSOLE));
            return;
        }
        if current.as_deref() == Some(PAGE_DETAILS) && i.details.vmwindow_has_unapplied_changes() {
            self.sync_toolbar_page_buttons(PAGE_DETAILS);
            return;
        }
        if current.as_deref() == Some(page) {
            self.sync_toolbar_page_buttons(page);
            return;
        }
        i.pages.set_visible_child_name(page);
        self.details_page_switch(page);
    }

    fn sync_toolbar_page_buttons(&self, newpage: &str) {
        let i = &self.0;
        i.ignore_details.set(true);
        i.control_details.set_active(newpage == PAGE_DETAILS);
        i.control_snapshots.set_active(newpage == PAGE_SNAPSHOTS);
        i.control_console
            .set_active(newpage != PAGE_DETAILS && newpage != PAGE_SNAPSHOTS);
        action(&i.win_actions, "page").set_state(&newpage.to_variant());
        i.ignore_details.set(false);
    }

    /// `_details_page_switch_cb`.
    fn details_page_switch(&self, newpage: &str) {
        self.refresh_current_page(Some(newpage));
        self.sync_toolbar_page_buttons(newpage);
        self.sync_console_page_menu_state();
    }

    fn change_run_text(&self, can_restore: bool) {
        let text = if can_restore { "Restore" } else { "Run" };
        self.0
            .control_run
            .update_property(&[gtk::accessible::Property::Label(text)]);
    }

    fn refresh_title(&self) {
        let vm = &self.0.vm;
        let conn_desc = vm.conn().map(|c| c.get_pretty_desc()).unwrap_or_default();
        let mut title = format!("{} on {}", vm.get_name_or_title(), conn_desc);
        if let Some(grabmsg) = self.0.console.vmwindow_get_title_message() {
            title = format!("{grabmsg} {title}");
        }
        self.0.window.set_title(Some(&title));
    }

    /// `_refresh_vm_state`.
    fn refresh_vm_state(&self) {
        let i = &self.0;
        let vm = &i.vm;
        self.refresh_title();

        let toolbar = engine::get().config().get_details_show_toolbar();
        action(&i.win_actions, "toolbar").set_state(&toolbar.to_variant());
        self.toggle_toolbar(toolbar);

        let run = vm.is_runable();
        let stop = vm.is_stoppable();
        let paused = vm.is_paused();
        self.change_run_text(vm.has_managed_save());

        i.control_run.set_sensitive(run);
        i.control_shutdown.set_sensitive(stop);
        i.control_shutdown_menu.set_sensitive(stop);
        i.control_pause.set_sensitive(stop);
        i.control_pause.set_tooltip_text(Some(if paused {
            "Resume the virtual machine"
        } else {
            "Pause the virtual machine"
        }));
        vmmenu::update_widget_states(&i.vm_actions, Some(vm));
        self.rebuild_vm_menu_if_needed();
        self.set_pause_state(paused);

        // snapshots_supported: the Snapshots page isn't ported yet.
        let mut tooltip = "Manage VM snapshots".to_string();
        tooltip += "\nSnapshot management is not available yet in this version.";
        i.control_snapshots.set_sensitive(false);
        i.control_snapshots.set_tooltip_text(Some(&tooltip));

        self.refresh_current_page(None);
    }

    /// The Run/Restore label and Pause/Resume entry live in the menu
    /// model; rebuild it when either flips.
    fn rebuild_vm_menu_if_needed(&self) {
        let i = &self.0;
        let state = (i.vm.has_managed_save(), i.vm.is_paused());
        if i.vm_menu_state.get() == Some(state) {
            return;
        }
        i.vm_menu_state.set(Some(state));
        i.menubar
            .set_menu_model(Some(&build_menubar(&i.vm, &i.console)));
    }

    fn set_pause_state(&self, state: bool) {
        self.0.pause_guard.set(true);
        self.0.control_pause.set_active(state);
        self.0.pause_guard.set(false);
    }

    fn control_vm_pause(&self) {
        let i = &self.0;
        if i.pause_guard.get() {
            return;
        }
        let do_pause = i.control_pause.is_active();
        // Set button state back to original value: just let the status
        // update function fix things for us
        self.set_pause_state(!do_pause);
        let action = if do_pause { "suspend" } else { "resume" };
        i.vm_actions.activate_action(action, None);
    }

    fn control_vm_run(&self) {
        if self.0.details.vmwindow_has_unapplied_changes() {
            return;
        }
        self.0.vm_actions.activate_action("run", None);
    }

    fn control_vm_shutdown(&self) {
        self.0.vm_actions.activate_action("shutdown", None);
    }

    fn activate_default_page(&self) {
        self.0.pages.set_visible_child_name(PAGE_CONSOLE);
        self.sync_toolbar_page_buttons(PAGE_CONSOLE);
        self.0.console.vmwindow_activate_default_console_page();
    }

    /// `_refresh_current_page`.
    fn refresh_current_page(&self, newpage: Option<&str>) {
        let i = &self.0;
        let current = i.pages.visible_child_name();
        let newpage = newpage.or(current.as_deref()).unwrap_or(PAGE_CONSOLE);
        i.details.vmwindow_refresh_vm_state(newpage == PAGE_DETAILS);
        if newpage == PAGE_CONSOLE {
            i.console.vmwindow_refresh_vm_state();
        }
    }

    // Console page handling

    fn sync_console_page_menu_state(&self) {
        let i = &self.0;
        let is_viewer = i.console.vmwindow_get_viewer_is_visible();
        let paused = i.vm.is_paused();
        action(&i.win_actions, "screenshot").set_enabled(is_viewer);
        if let Some(a) = i
            .console
            .actions()
            .lookup_action("send-key")
            .and_then(|a| a.downcast::<gio::SimpleAction>().ok())
        {
            a.set_enabled(is_viewer && !paused);
        }
        action(&i.win_actions, "usb-redirection")
            .set_enabled(i.console.vmwindow_viewer_can_usb_redirect());
        // _console_refresh_can_fullscreen
        i.control_fullscreen.set_sensitive(is_viewer);
        action(&i.win_actions, "fullscreen").set_enabled(is_viewer);
        action(&i.win_actions, "size-to-vm").set_enabled(is_viewer);
        self.console_refresh_resizeguest_from_settings();
    }

    fn console_refresh_scaling_from_settings(&self) {
        let scale = self.0.vm.get_console_scaling();
        action(&self.0.win_actions, "scale").set_state(&scale.to_variant());
        self.0.console.vmwindow_sync_scaling_with_display();
    }

    fn console_refresh_resizeguest_from_settings(&self) {
        let val = self.0.vm.get_console_resizeguest() != 0;
        action(&self.0.win_actions, "resizeguest").set_state(&val.to_variant());
        self.0.console.vmwindow_sync_resizeguest_with_display();
    }

    fn console_refresh_autoconnect_from_settings(&self) {
        let val = self.0.vm.get_console_autoconnect();
        action(&self.0.win_actions, "autoconnect").set_state(&val.to_variant());
    }

    fn fullscreen_changed(&self, do_fullscreen: bool) {
        let i = &self.0;
        i.control_fullscreen.set_active(do_fullscreen);
        if do_fullscreen {
            i.window.fullscreen();
        } else {
            i.window.unfullscreen();
        }
        i.console.vmwindow_set_fullscreen(do_fullscreen);
        i.menubar.set_visible(!do_fullscreen);
        let show_toolbar = !do_fullscreen
            && action(&i.win_actions, "toolbar")
                .state()
                .and_then(|s| s.get::<bool>())
                .unwrap_or(true);
        i.toolbar_box.set_visible(show_toolbar);
    }

    fn control_vm_screenshot(&self) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            if let Err(e) = this.take_screenshot().await {
                error::show_err(
                    Some(this.0.window.upcast_ref()),
                    &format!("Error taking screenshot: {e}"),
                    None,
                    "",
                )
                .await;
            }
        });
    }

    /// `_take_screenshot`: the display as PNG, with upstream's tEXt
    /// metadata, saved where the user picks.
    async fn take_screenshot(&self) -> Result<(), String> {
        let i = &self.0;
        let fb = i
            .console
            .vmwindow_viewer_get_framebuffer()
            .ok_or("No graphical console is connected")?;
        let pixbuf = framebuffer_to_pixbuf(&fb);
        let uri = i.vm.conn().map(|c| c.get_uri()).unwrap_or_default();
        let metadata = [
            ("tEXt::Hypervisor URI", uri),
            ("tEXt::Domain Name", i.vm.get_name()),
            ("tEXt::Domain UUID", i.vm.get_uuid()),
            ("tEXt::Generator App", "virt-manager".to_string()),
            (
                "tEXt::Generator Version",
                env!("CARGO_PKG_VERSION").to_string(),
            ),
        ];
        let options: Vec<(&str, &str)> = metadata.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let data = pixbuf
            .save_to_bufferv("png", &options)
            .map_err(|e| e.to_string())?;

        let now = glib::DateTime::now_local()
            .and_then(|d| d.format("%Y-%m-%d_%H:%M:%S"))
            .map(|s| s.to_string())
            .unwrap_or_default();
        let default = format!("Screenshot_{}_{}.png", i.vm.get_name(), now);
        let config = engine::get().config();
        let start_folder = config.get_default_directory("screenshot");

        let filter = gtk::FileFilter::new();
        filter.set_name(Some("PNG files"));
        filter.add_suffix("png");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title("Save Virtual Machine Screenshot")
            .accept_label("_Save")
            .modal(true)
            .initial_name(default.as_str())
            .filters(&filters)
            .build();
        if !start_folder.is_empty() {
            dialog.set_initial_folder(Some(&gio::File::for_path(&start_folder)));
        }
        let Ok(file) = dialog.save_future(Some(&i.window)).await else {
            // No screenshot path given, skipping save.
            return Ok(());
        };
        let Some(mut path) = file.path() else {
            return Ok(());
        };
        if path.extension().and_then(|e| e.to_str()) != Some("png") {
            path.as_mut_os_string().push(".png");
        }
        std::fs::write(&path, data).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent().and_then(|d| d.to_str()) {
            config.set_default_directory("screenshot", dir);
        }
        Ok(())
    }

    fn vm_state_changed(&self) {
        if self.is_visible() {
            self.refresh_vm_state();
            // Upstream gets XML-changed events from libvirt; a state
            // change is when the live XML most often changes.
            self.refresh_xml();
        }
    }
}

/// The console's xRGB framebuffer as an RGB pixbuf.
fn framebuffer_to_pixbuf(fb: &crate::display::FrameCopy) -> gtk::gdk_pixbuf::Pixbuf {
    let (w, h) = (fb.width, fb.height);
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for row in fb.pixels.chunks(fb.stride).take(h as usize) {
        for px in row[..(w as usize * 4)].chunks_exact(4) {
            let v = u32::from_ne_bytes([px[0], px[1], px[2], px[3]]);
            rgb.extend_from_slice(&[(v >> 16) as u8, (v >> 8) as u8, v as u8]);
        }
    }
    gtk::gdk_pixbuf::Pixbuf::from_bytes(
        &glib::Bytes::from_owned(rgb),
        gtk::gdk_pixbuf::Colorspace::Rgb,
        false,
        8,
        w,
        h,
        w * 3,
    )
}
