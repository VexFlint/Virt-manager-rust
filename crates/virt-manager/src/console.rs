//! Upstream's `virtManager/details/console.py` (`vmmConsolePages`): the
//! VM window's Console page.
//!
//! The embedded VNC/SPICE viewers (ticket 14: gtk-vnc and spice-gtk
//! have no GTK4 widgets, so they come through their GTK-free cores) are
//! the next slice. This one ports the page's state handling around
//! them: the "Guest is not running." / "Graphical console not
//! configured for guest" pages, the Send Key menu and the Consoles
//! list, so the window behaves like upstream up to the point where a
//! viewer would connect.

use std::rc::Rc;

use gtk::gio;
use gtk::prelude::*;

use crate::details::graphics_pretty_type_simple;
use crate::domain::VmmDomain;

/// `_KEYCOMBO_MENU`'s entries: accelerator → keys sent.
const KEYCOMBOS: &[&[&str]] = &[
    &[
        "<Control><Alt>BackSpace",
        "<Control><Alt>Delete",
        "<Control><Alt><Shift>Escape",
    ],
    &[
        "<Control><Alt>F1",
        "<Control><Alt>F2",
        "<Control><Alt>F3",
        "<Control><Alt>F4",
        "<Control><Alt>F5",
        "<Control><Alt>F6",
        "<Control><Alt>F7",
        "<Control><Alt>F8",
        "<Control><Alt>F9",
        "<Control><Alt>F10",
        "<Control><Alt>F11",
        "<Control><Alt>F12",
    ],
    &["Print"],
];

/// `_make_keycombo_menu`: items call `console.send-key` with the
/// accelerator string as target.
fn make_keycombo_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    for section in KEYCOMBOS {
        let sec = gio::Menu::new();
        for accel in *section {
            let label = gtk::accelerator_parse(*accel)
                .map(|(key, mods)| gtk::accelerator_get_label(key, mods).to_string())
                .unwrap_or_else(|| accel.to_string());
            let item = gio::MenuItem::new(Some(&label), None);
            item.set_action_and_target_value(Some("console.send-key"), Some(&accel.to_variant()));
            sec.append_item(&item);
        }
        menu.append_section(None, &sec);
    }
    menu
}

/// `_ConsoleMenu._build_graphical_menu_items` + `_build_serial_menu_items`:
/// `(label, usable)` per entry, `None` for the separator.
fn console_menu_items(vm: &VmmDomain) -> Vec<Option<(String, bool)>> {
    let Some(x) = vm.xmlobj() else {
        return vec![];
    };
    let mut ret = vec![];
    let mut found_default = false;
    for (idx, g) in x.guest.devices.graphics.iter().enumerate() {
        let gtype = g.graphics_type.clone().unwrap_or_default();
        let mut label = format!("Graphical Console {}", graphics_pretty_type_simple(&gtype));
        if idx > 0 {
            label += &format!(" {}", idx + 1);
        }
        let can_embed = matches!(gtype.as_str(), "vnc" | "spice") && !found_default;
        found_default |= can_embed;
        ret.push(Some((label, can_embed)));
    }
    if ret.is_empty() {
        ret.push(Some((
            "Graphical console not configured for guest".into(),
            false,
        )));
    }
    ret.push(None);

    let mut serials = vec![];
    for (idx, s) in x.guest.devices.serials.iter().enumerate() {
        serials.push((
            format!("Serial {}", idx + 1),
            s.char_type.as_deref() == Some("pty"),
        ));
    }
    if serials.is_empty() {
        serials.push(("No text console available".into(), false));
    }
    ret.extend(serials.into_iter().map(Some));
    ret
}

struct Inner {
    vm: VmmDomain,
    top_box: gtk::Box,
    unavailable: gtk::Label,
    keycombo_menu: gio::Menu,
    console_list_menu: gio::Menu,
    actions: gio::SimpleActionGroup,
}

#[derive(Clone)]
pub struct VmmConsolePages(Rc<Inner>);

impl VmmConsolePages {
    pub fn new(vm: &VmmDomain) -> VmmConsolePages {
        // console-pages: only the "unavailable" page exists until the
        // viewers are ported.
        let unavailable = gtk::Label::new(None);
        unavailable.set_justify(gtk::Justification::Center);
        unavailable.set_wrap(true);
        unavailable.set_vexpand(true);
        unavailable.set_hexpand(true);
        let top_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        top_box.append(&unavailable);

        let actions = gio::SimpleActionGroup::new();
        let send_key = gio::SimpleAction::new("send-key", Some(glib_str_type()));
        send_key.set_enabled(false);
        actions.add_action(&send_key);
        let select = gio::SimpleAction::new("select", Some(glib_str_type()));
        select.set_enabled(false);
        actions.add_action(&select);

        let this = VmmConsolePages(Rc::new(Inner {
            vm: vm.clone(),
            top_box,
            unavailable,
            keycombo_menu: make_keycombo_menu(),
            console_list_menu: gio::Menu::new(),
            actions,
        }));
        this.activate_vm_unavailable_page("Guest is not running.");
        this
    }

    pub fn top_box(&self) -> &gtk::Box {
        &self.0.top_box
    }

    /// The `console` action group the Send Key and Consoles menus use.
    pub fn actions(&self) -> &gio::SimpleActionGroup {
        &self.0.actions
    }

    /// `_activate_vm_unavailable_page`.
    fn activate_vm_unavailable_page(&self, msg: &str) {
        self.0
            .unavailable
            .set_markup(&format!("<b>{}</b>", gtk::glib::markup_escape_text(msg)));
    }

    /// `_show_vm_status_unavailable`.
    fn show_vm_status_unavailable(&self) {
        if self.0.vm.is_crashed() {
            self.activate_vm_unavailable_page("Guest has crashed.");
        } else {
            self.activate_vm_unavailable_page("Guest is not running.");
        }
    }

    /// `_populate_console_menu`.
    fn populate_console_menu(&self) {
        let menu = &self.0.console_list_menu;
        menu.remove_all();
        let mut section = gio::Menu::new();
        for item in console_menu_items(&self.0.vm) {
            match item {
                Some((label, _usable)) => {
                    let mi = gio::MenuItem::new(Some(&label), None);
                    mi.set_action_and_target_value(
                        Some("console.select"),
                        Some(&label.to_variant()),
                    );
                    section.append_item(&mi);
                }
                None => {
                    menu.append_section(None, &section);
                    section = gio::Menu::new();
                }
            }
        }
        menu.append_section(None, &section);
    }

    /// `_activate_default_console_page` / `_init_viewer` up to the
    /// point of opening a viewer.
    fn activate_default_console_page(&self) {
        self.populate_console_menu();
        let vm = &self.0.vm;
        if vm.is_runable() {
            self.show_vm_status_unavailable();
            return;
        }
        let Some(x) = vm.xmlobj() else {
            return;
        };
        match x.guest.devices.graphics.first() {
            None => self.activate_vm_unavailable_page("Graphical console not configured for guest"),
            Some(g) => {
                let gtype = g.graphics_type.clone().unwrap_or_default();
                if !matches!(gtype.as_str(), "vnc" | "spice") {
                    self.activate_vm_unavailable_page(&format!(
                        "Cannot display graphical console type '{gtype}'"
                    ));
                } else {
                    self.activate_vm_unavailable_page(&format!(
                        "Error connecting to graphical console:\n\
                         The {} viewer is not available in this version yet.",
                        graphics_pretty_type_simple(&gtype)
                    ));
                }
            }
        }
    }

    pub fn vmwindow_activate_default_console_page(&self) {
        self.activate_default_console_page();
    }

    pub fn vmwindow_refresh_vm_state(&self) {
        self.activate_default_console_page();
    }

    pub fn vmwindow_close(&self) {
        self.activate_vm_unavailable_page("Viewer window closed.");
    }

    pub fn vmwindow_get_keycombo_menu(&self) -> &gio::Menu {
        &self.0.keycombo_menu
    }

    pub fn vmwindow_get_console_list_menu(&self) -> &gio::Menu {
        &self.0.console_list_menu
    }

    /// No viewer can be open yet.
    pub fn vmwindow_get_viewer_is_visible(&self) -> bool {
        false
    }

    pub fn vmwindow_viewer_can_usb_redirect(&self) -> bool {
        false
    }
}

fn glib_str_type() -> &'static gtk::glib::VariantTy {
    gtk::glib::VariantTy::STRING
}
