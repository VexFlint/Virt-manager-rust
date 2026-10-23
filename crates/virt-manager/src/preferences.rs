//! Upstream's `virtManager/preferences.py` + `ui/preferences.ui`: the
//! Preferences window (Edit > Preferences). Every control reads and
//! writes the shared GSettings keys directly, so changes apply at once,
//! exactly like upstream; there is no OK/Apply.
//!
//! The `GtkComboBox` + `ListStore([value, label])` pairs become
//! `DropDown`s over the labels with the values kept alongside.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::gdk;
use gtk::glib;
use gtk::glib::translate::{FromGlib, IntoGlib};
use gtk::prelude::*;

use crate::config::{
    StatsColumn, DEFAULT_CONSOLE_RESIZEGUEST, DEFAULT_GRAPHICS_FROM_CONFIG,
    DEFAULT_STORAGE_FORMAT_FROM_CONFIG,
};
use crate::engine;

/// A `DropDown` plus the GSettings value behind each row
/// (`uiutil.set_list_selection`; `connect_changed` hands over the
/// selected row's value like `get_list_selection`).
struct Combo<T> {
    dropdown: gtk::DropDown,
    values: Vec<T>,
}

impl<T: PartialEq + Clone + 'static> Combo<T> {
    fn new(rows: Vec<(T, String)>) -> Combo<T> {
        let labels: Vec<&str> = rows.iter().map(|(_, l)| l.as_str()).collect();
        let dropdown = gtk::DropDown::from_strings(&labels);
        Combo {
            dropdown,
            values: rows.into_iter().map(|(v, _)| v).collect(),
        }
    }

    fn set_list_selection(&self, val: &T) {
        if let Some(idx) = self.values.iter().position(|v| v == val) {
            self.dropdown.set_selected(idx as u32);
        }
    }

    fn connect_changed(&self, cb: impl Fn(T) + 'static) {
        let values = self.values.clone();
        self.dropdown.connect_selected_notify(move |d| {
            if let Some(v) = values.get(d.selected() as usize) {
                cb(v.clone());
            }
        });
    }
}

/// `vmmSystray.systray_disabled_message`. No AppIndicator/StatusNotifier
/// backend is ported yet, so Wayland always gets the message.
fn systray_disabled_message() -> Option<&'static str> {
    std::env::var_os("WAYLAND_DISPLAY")?;
    Some("AppIndicator not installed or usable, system tray icon not available on Wayland")
}

/// `vmmInspection.libguestfs_installed`: no libguestfs binding is ported.
fn libguestfs_installed() -> bool {
    false
}

/// `_process_grabkeys`: "65507,65513" → "Control_L+Alt_L".
fn process_grabkeys(val: &str) -> String {
    val.split(',')
        .filter_map(|k| k.trim().parse::<u32>().ok())
        .filter_map(|k| unsafe { gdk::Key::from_glib(k) }.name())
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join("+")
}

fn grid_label(text: &str, target: &impl IsA<gtk::Widget>) -> gtk::Label {
    let label = gtk::Label::with_mnemonic(text);
    label.set_xalign(0.0);
    label.set_mnemonic_widget(Some(target));
    label
}

fn new_grid() -> gtk::Grid {
    gtk::Grid::builder()
        .row_spacing(6)
        .column_spacing(12)
        .build()
}

/// A glade `GtkFrame` with shadow none: bold title, indented child.
fn frame(title: &str, child: &impl IsA<gtk::Widget>) -> gtk::Box {
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 6);
    vbox.set_margin_top(6);
    vbox.set_margin_bottom(6);
    vbox.set_margin_start(6);
    vbox.set_margin_end(6);
    let label = gtk::Label::new(None);
    label.set_markup(&format!("<b>{title}</b>"));
    label.set_xalign(0.0);
    vbox.append(&label);
    child.set_margin_start(12);
    vbox.append(child);
    vbox
}

/// The small warning-icon + label rows (`prefs-*-warn-box`).
fn warn_box(markup: &str) -> (gtk::Box, gtk::Label) {
    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 3);
    let icon = gtk::Image::from_icon_name("dialog-warning");
    icon.set_valign(gtk::Align::Start);
    let label = gtk::Label::new(None);
    label.set_markup(markup);
    label.set_xalign(0.0);
    label.set_wrap(true);
    hbox.append(&icon);
    hbox.append(&label);
    (hbox, label)
}

/// A config setter for one check button.
type Setter = fn(bool);

struct Widgets {
    window: gtk::Window,
    system_tray: gtk::CheckButton,
    system_tray_warn_box: gtk::Box,
    system_tray_warn_label: gtk::Label,
    xmleditor: gtk::CheckButton,
    libguestfs: gtk::CheckButton,
    libguestfs_warn_box: gtk::Box,
    update_interval: gtk::SpinButton,
    enable_cpu: gtk::CheckButton,
    enable_disk: gtk::CheckButton,
    enable_net: gtk::CheckButton,
    enable_memory: gtk::CheckButton,
    graphics_type: Combo<String>,
    storage_format: Combo<String>,
    cpu_default: Combo<String>,
    firmware_default: Combo<String>,
    console_scaling: Combo<i32>,
    console_resizeguest: Combo<i32>,
    console_autoredir: Combo<bool>,
    console_autoconnect: gtk::CheckButton,
    keys_grab_sequence: gtk::Label,
    keys_grab_changebtn: gtk::Button,
    confirm_forcepoweroff: gtk::CheckButton,
    confirm_poweroff: gtk::CheckButton,
    confirm_pause: gtk::CheckButton,
    confirm_removedev: gtk::CheckButton,
    confirm_unapplied: gtk::CheckButton,
    confirm_delstorage: gtk::CheckButton,
    orig_libguestfs_val: Cell<Option<bool>>,
}

#[derive(Clone)]
pub struct VmmPreferences(Rc<Widgets>);

thread_local! {
    static INSTANCE: RefCell<Option<VmmPreferences>> = const { RefCell::new(None) };
}

impl VmmPreferences {
    /// `vmmPreferences.show_instance(parentobj)`.
    pub fn show_instance(parent: &gtk::Window) {
        let this = INSTANCE.with(|i| i.borrow_mut().get_or_insert_with(Self::new).clone());
        this.show(parent);
    }

    fn new() -> VmmPreferences {
        let window = gtk::Window::builder()
            .title("Preferences")
            .resizable(false)
            .hide_on_close(true)
            .build();

        // General
        let system_tray = gtk::CheckButton::with_mnemonic("Enable _system tray icon");
        let (system_tray_warn_box, system_tray_warn_label) = warn_box("");
        let xmleditor = gtk::CheckButton::with_mnemonic("Enable _XML editing");
        let libguestfs = gtk::CheckButton::with_mnemonic("Enable libgues_tfs VM introspection");
        let (libguestfs_warn_box, _) = warn_box(
            "<small>You must restart the application for this change to take effect</small>",
        );
        let lg_grid = new_grid();
        lg_grid.attach(&libguestfs, 0, 0, 1, 1);
        lg_grid.attach(&libguestfs_warn_box, 0, 1, 1, 1);
        let general = new_grid();
        general.attach(&system_tray, 0, 0, 1, 1);
        general.attach(&system_tray_warn_box, 0, 1, 1, 1);
        general.attach(&xmleditor, 0, 2, 1, 1);
        general.attach(&lg_grid, 0, 3, 1, 1);

        // Polling
        let update_interval = gtk::SpinButton::with_range(1.0, 60.0, 1.0);
        update_interval.set_increments(1.0, 5.0);
        let enable_cpu = gtk::CheckButton::new();
        let enable_disk = gtk::CheckButton::new();
        let enable_net = gtk::CheckButton::new();
        let enable_memory = gtk::CheckButton::new();
        let interval_box = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        interval_box.append(&update_interval);
        interval_box.append(&gtk::Label::new(Some("seconds")));
        let polling = new_grid();
        polling.attach(
            &grid_label("_Update status every", &update_interval),
            0,
            0,
            1,
            1,
        );
        polling.attach(&interval_box, 1, 0, 1, 1);
        polling.attach(&grid_label("Poll C_PU usage", &enable_cpu), 0, 1, 1, 1);
        polling.attach(&enable_cpu, 1, 1, 1, 1);
        polling.attach(&grid_label("Poll _Disk I/O", &enable_disk), 0, 2, 1, 1);
        polling.attach(&enable_disk, 1, 2, 1, 1);
        polling.attach(&grid_label("Poll _Network I/O", &enable_net), 0, 3, 1, 1);
        polling.attach(&enable_net, 1, 3, 1, 1);
        polling.attach(
            &grid_label("Poll _Memory stats", &enable_memory),
            0,
            4,
            1,
            1,
        );
        polling.attach(&enable_memory, 1, 4, 1, 1);

        // New VM
        let graphics_type = Combo::new(vec![
            (
                "system".to_string(),
                format!("System default ({DEFAULT_GRAPHICS_FROM_CONFIG})"),
            ),
            ("vnc".to_string(), "VNC".to_string()),
            ("spice".to_string(), "Spice".to_string()),
        ]);
        let storage_format = Combo::new(vec![
            (
                "default".to_string(),
                format!("System default ({DEFAULT_STORAGE_FORMAT_FROM_CONFIG})"),
            ),
            ("raw".to_string(), "Raw".to_string()),
            ("qcow2".to_string(), "QCOW2".to_string()),
        ]);
        let cpu_default = Combo::new(
            [
                ("default", "Application default"),
                ("hv-default", "Hypervisor default"),
                ("host-model-only", "Nearest host CPU model"),
                ("host-model", "host-model"),
                ("host-passthrough", "host-passthrough"),
                ("maximum", "maximum"),
            ]
            .map(|(v, l)| (v.to_string(), l.to_string()))
            .to_vec(),
        );
        let firmware_default = Combo::new(vec![
            ("default".to_string(), "System default".to_string()),
            ("uefi".to_string(), "UEFI".to_string()),
        ]);
        let storage_tip = "Default storage format for new disk images.";
        storage_format.dropdown.set_tooltip_text(Some(storage_tip));
        let cpu_tip = "Default CPU setting for new VMs. This is typically a tradeoff \
                       between performance and migration compatibility";
        let fw_tip = "Default Firmware for new VMs. Boot using either BIOS or UEFI.";
        let newvm = new_grid();
        newvm.attach(
            &grid_label("Gra_phics type:", &graphics_type.dropdown),
            0,
            0,
            1,
            1,
        );
        newvm.attach(&graphics_type.dropdown, 1, 0, 1, 1);
        let l = grid_label("_Storage format:", &storage_format.dropdown);
        l.set_tooltip_text(Some(storage_tip));
        newvm.attach(&l, 0, 1, 1, 1);
        newvm.attach(&storage_format.dropdown, 1, 1, 1, 1);
        let l = grid_label("CPU _default:", &cpu_default.dropdown);
        l.set_tooltip_text(Some(cpu_tip));
        cpu_default.dropdown.set_tooltip_text(Some(cpu_tip));
        newvm.attach(&l, 0, 2, 1, 1);
        newvm.attach(&cpu_default.dropdown, 1, 2, 1, 1);
        let l = grid_label("x86 _Firmware:", &firmware_default.dropdown);
        l.set_tooltip_text(Some(fw_tip));
        firmware_default.dropdown.set_tooltip_text(Some(fw_tip));
        newvm.attach(&l, 0, 3, 1, 1);
        newvm.attach(&firmware_default.dropdown, 1, 3, 1, 1);

        // Console
        let console_scaling = Combo::new(
            [(0, "Never"), (1, "Fullscreen only"), (2, "Always")]
                .map(|(v, l)| (v, l.to_string()))
                .to_vec(),
        );
        let resize_vals = [(0, "Off"), (1, "On")];
        let default_resize = resize_vals
            .iter()
            .find(|(v, _)| *v == DEFAULT_CONSOLE_RESIZEGUEST)
            .map(|(_, l)| *l)
            .unwrap_or("Off");
        let mut resize_rows = vec![(-1, format!("System default ({default_resize})"))];
        resize_rows.extend(resize_vals.map(|(v, l)| (v, l.to_string())));
        let console_resizeguest = Combo::new(resize_rows);
        let console_autoredir = Combo::new(vec![
            (false, "Manual redirect only".to_string()),
            (true, "Auto redirect on USB attach".to_string()),
        ]);
        let console_autoconnect = gtk::CheckButton::new();
        let keys_grab_sequence = gtk::Label::new(Some("Not supported"));
        let keys_grab_changebtn = gtk::Button::with_label("Change...");
        let resize_tip = "Change guest resolution when the guest window size is changed. \
                          Only works with properly configured guest using spice and the \
                          desktop agent.";
        let autoconnect_tip = "If disabled, the VM window will not automatically connect \
                               to the running VM graphical console.";
        let console = new_grid();
        console.attach(
            &grid_label("Graphical console _scaling:", &console_scaling.dropdown),
            0,
            0,
            1,
            1,
        );
        console.attach(&console_scaling.dropdown, 1, 0, 1, 1);
        let l = grid_label("_Resize guest with window:", &console_resizeguest.dropdown);
        l.set_tooltip_text(Some(resize_tip));
        console.attach(&l, 0, 1, 1, 1);
        console.attach(&console_resizeguest.dropdown, 1, 1, 1, 1);
        let keys_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        keys_box.append(&grid_label("Gr_ab keys:", &keys_grab_changebtn));
        keys_box.append(&keys_grab_sequence);
        console.attach(&keys_box, 0, 2, 1, 1);
        console.attach(&keys_grab_changebtn, 1, 2, 1, 1);
        console.attach(
            &grid_label("SPICE _USB Redirection:", &console_autoredir.dropdown),
            0,
            3,
            1,
            1,
        );
        console.attach(&console_autoredir.dropdown, 1, 3, 1, 1);
        let l = grid_label("Console autoconnec_t:", &console_autoconnect);
        l.set_tooltip_text(Some(autoconnect_tip));
        console.attach(&l, 0, 4, 1, 1);
        console.attach(&console_autoconnect, 1, 4, 1, 1);

        // Feedback
        let confirm_forcepoweroff = gtk::CheckButton::new();
        let confirm_poweroff = gtk::CheckButton::new();
        let confirm_pause = gtk::CheckButton::new();
        let confirm_removedev = gtk::CheckButton::new();
        let confirm_unapplied = gtk::CheckButton::new();
        let confirm_delstorage = gtk::CheckButton::new();
        let feedback = new_grid();
        for (row, (text, check)) in [
            ("_Force Poweroff:", &confirm_forcepoweroff),
            ("Poweroff/_Reboot/Save:", &confirm_poweroff),
            ("_Pause:", &confirm_pause),
            ("Device re_moval:", &confirm_removedev),
            ("_Unapplied changes:", &confirm_unapplied),
            ("_Deleting storage:", &confirm_delstorage),
        ]
        .into_iter()
        .enumerate()
        {
            feedback.attach(&grid_label(text, check), 0, row as i32, 1, 1);
            feedback.attach(check, 1, row as i32, 1, 1);
        }

        let notebook = gtk::Notebook::new();
        for (tab, title, page) in [
            ("_General", "General", general),
            ("P_olling", "Stats Options", polling),
            ("N_ew VM", "New VM Defaults", newvm),
            ("Conso_le", "Graphical Consoles", console),
            ("Feed_back", "Confirmations", feedback),
        ] {
            notebook.append_page(&frame(title, &page), Some(&gtk::Label::with_mnemonic(tab)));
        }

        let close = gtk::Button::with_mnemonic("_Close");
        close.set_halign(gtk::Align::End);
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 12);
        vbox.set_margin_top(12);
        vbox.set_margin_bottom(12);
        vbox.set_margin_start(12);
        vbox.set_margin_end(12);
        vbox.append(&notebook);
        vbox.append(&close);
        window.set_child(Some(&vbox));

        let this = VmmPreferences(Rc::new(Widgets {
            window,
            system_tray,
            system_tray_warn_box,
            system_tray_warn_label,
            xmleditor,
            libguestfs,
            libguestfs_warn_box,
            update_interval,
            enable_cpu,
            enable_disk,
            enable_net,
            enable_memory,
            graphics_type,
            storage_format,
            cpu_default,
            firmware_default,
            console_scaling,
            console_resizeguest,
            console_autoredir,
            console_autoconnect,
            keys_grab_sequence,
            keys_grab_changebtn,
            confirm_forcepoweroff,
            confirm_poweroff,
            confirm_pause,
            confirm_removedev,
            confirm_unapplied,
            confirm_delstorage,
            orig_libguestfs_val: Cell::new(None),
        }));

        this.init_ui();
        this.refresh_all();
        this.connect_signals(&close);
        this
    }

    fn init_ui(&self) {
        let w = &self.0;
        if !libguestfs_installed() {
            w.libguestfs.set_sensitive(false);
            w.libguestfs
                .set_tooltip_text(Some("libguestfs support is not installed"));
        }
        w.libguestfs_warn_box.set_visible(false);
    }

    fn refresh_all(&self) {
        let w = &self.0;
        let config = engine::get().config();

        // refresh_view_system_tray
        let errmsg = systray_disabled_message();
        w.system_tray.set_sensitive(errmsg.is_none());
        w.system_tray
            .set_active(config.get_view_system_tray() && errmsg.is_none());
        if let Some(msg) = errmsg {
            w.system_tray_warn_label
                .set_markup(&format!("<small>{}</small>", glib::markup_escape_text(msg)));
        }
        w.system_tray_warn_box.set_visible(errmsg.is_some());

        w.xmleditor.set_active(config.get_xmleditor_enabled());

        // refresh_libguestfs
        let val = config.get_libguestfs_inspect_vms();
        if w.orig_libguestfs_val.get().is_none() {
            w.orig_libguestfs_val.set(Some(val));
        }
        w.libguestfs.set_active(val);

        w.update_interval
            .set_value(config.get_stats_update_interval() as f64);
        w.console_scaling
            .set_list_selection(&config.get_console_scaling());
        w.console_resizeguest
            .set_list_selection(&config.get_console_resizeguest_raw());
        w.console_autoredir
            .set_list_selection(&config.get_auto_usbredir());
        w.console_autoconnect
            .set_active(config.get_console_autoconnect());
        w.graphics_type
            .set_list_selection(&config.get_graphics_type_raw());
        w.storage_format
            .set_list_selection(&config.get_default_storage_format_raw());
        w.cpu_default
            .set_list_selection(&config.get_default_cpu_setting());
        w.firmware_default
            .set_list_selection(&config.get_default_firmware_setting());
        w.enable_cpu
            .set_active(config.get_stats_enable_poll(StatsColumn::GuestCpu));
        w.enable_disk
            .set_active(config.get_stats_enable_poll(StatsColumn::Disk));
        w.enable_net
            .set_active(config.get_stats_enable_poll(StatsColumn::Network));
        w.enable_memory
            .set_active(config.get_stats_enable_poll(StatsColumn::Memory));
        self.refresh_grabkeys_combination();
        w.confirm_forcepoweroff
            .set_active(config.get_confirm_forcepoweroff());
        w.confirm_poweroff.set_active(config.get_confirm_poweroff());
        w.confirm_pause.set_active(config.get_confirm_pause());
        w.confirm_removedev
            .set_active(config.get_confirm_removedev());
        w.confirm_unapplied
            .set_active(config.get_confirm_unapplied());
        w.confirm_delstorage
            .set_active(config.get_confirm_delstorage());
    }

    fn refresh_grabkeys_combination(&self) {
        let val = engine::get().config().get_keys_combination();
        self.0.keys_grab_sequence.set_text(&process_grabkeys(&val));
    }

    fn connect_signals(&self, close: &gtk::Button) {
        let w = &self.0;
        let toggles: [(&gtk::CheckButton, Setter); 13] = [
            (&w.system_tray, |v| {
                engine::get().config().set_view_system_tray(v)
            }),
            (&w.xmleditor, |v| {
                engine::get().config().set_xmleditor_enabled(v)
            }),
            (&w.console_autoconnect, |v| {
                engine::get().config().set_console_autoconnect(v)
            }),
            (&w.enable_cpu, |v| {
                engine::get()
                    .config()
                    .set_stats_enable_poll(StatsColumn::GuestCpu, v)
            }),
            (&w.enable_disk, |v| {
                engine::get()
                    .config()
                    .set_stats_enable_poll(StatsColumn::Disk, v)
            }),
            (&w.enable_net, |v| {
                engine::get()
                    .config()
                    .set_stats_enable_poll(StatsColumn::Network, v)
            }),
            (&w.enable_memory, |v| {
                engine::get()
                    .config()
                    .set_stats_enable_poll(StatsColumn::Memory, v)
            }),
            (&w.confirm_forcepoweroff, |v| {
                engine::get().config().set_confirm_forcepoweroff(v)
            }),
            (&w.confirm_poweroff, |v| {
                engine::get().config().set_confirm_poweroff(v)
            }),
            (&w.confirm_pause, |v| {
                engine::get().config().set_confirm_pause(v)
            }),
            (&w.confirm_removedev, |v| {
                engine::get().config().set_confirm_removedev(v)
            }),
            (&w.confirm_unapplied, |v| {
                engine::get().config().set_confirm_unapplied(v)
            }),
            (&w.confirm_delstorage, |v| {
                engine::get().config().set_confirm_delstorage(v)
            }),
        ];
        for (check, set) in toggles {
            check.connect_toggled(move |c| set(c.is_active()));
        }

        let weak = Rc::downgrade(&self.0);
        w.libguestfs.connect_toggled(move |c| {
            let Some(w) = weak.upgrade() else { return };
            let val = c.is_active();
            engine::get().config().set_libguestfs_inspect_vms(val);
            let vis = Some(val) != w.orig_libguestfs_val.get() && c.is_sensitive();
            w.libguestfs_warn_box.set_visible(vis);
        });

        w.update_interval.connect_value_changed(|s| {
            engine::get()
                .config()
                .set_stats_update_interval(s.value_as_int())
        });
        w.console_scaling
            .connect_changed(|v| engine::get().config().set_console_scaling(v));
        w.console_resizeguest
            .connect_changed(|v| engine::get().config().set_console_resizeguest(v));
        w.console_autoredir
            .connect_changed(|v| engine::get().config().set_auto_usbredir(v));
        w.graphics_type
            .connect_changed(|v| engine::get().config().set_graphics_type(&v));
        w.storage_format
            .connect_changed(|v| engine::get().config().set_storage_format(&v));
        w.cpu_default
            .connect_changed(|v| engine::get().config().set_default_cpu_setting(&v));
        w.firmware_default
            .connect_changed(|v| engine::get().config().set_firmware_setting(&v));

        let this = self.clone();
        w.keys_grab_changebtn.connect_clicked(move |_| {
            let this = this.clone();
            glib::spawn_future_local(async move { this.change_grab_keys().await });
        });

        let this = self.clone();
        close.connect_clicked(move |_| this.close());

        // bind_escape_key_close
        let keys = gtk::EventControllerKey::new();
        let this = self.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                this.close();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        w.window.add_controller(keys);
    }

    fn show(&self, parent: &gtk::Window) {
        self.0.window.set_transient_for(Some(parent));
        self.0.window.present();
    }

    fn close(&self) {
        self.0.window.set_visible(false);
    }

    /// `change_grab_keys`: a modal dialog that records the keys held
    /// down when OK is clicked.
    async fn change_grab_keys(&self) {
        let dialog = gtk::Window::builder()
            .title("Configure grab key combination")
            .transient_for(&self.0.window)
            .modal(true)
            .destroy_with_parent(true)
            .default_width(325)
            .default_height(160)
            .build();

        let infolabel = gtk::Label::new(Some(
            "You can now define grab keys by pressing them.\n\
             To confirm your selection please click OK button\n\
             while you have desired keys pressed.",
        ));
        let keylabel = gtk::Label::new(Some("Please press desired grab key combination"));
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 12);
        vbox.set_margin_top(6);
        vbox.set_margin_bottom(6);
        vbox.set_margin_start(6);
        vbox.set_margin_end(6);
        vbox.append(&infolabel);
        vbox.append(&keylabel);
        let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        spacer.set_vexpand(true);
        vbox.append(&spacer);
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.set_halign(gtk::Align::End);
        let cancel = gtk::Button::with_mnemonic("_Cancel");
        let ok = gtk::Button::with_mnemonic("_OK");
        buttons.append(&cancel);
        buttons.append(&ok);
        vbox.append(&buttons);
        dialog.set_child(Some(&vbox));

        // (hardware keycode, keyval) of each key currently held.
        let events: Rc<RefCell<Vec<(u32, gdk::Key)>>> = Rc::default();
        let get_string = |events: &[(u32, gdk::Key)]| {
            events
                .iter()
                .filter_map(|(_, k)| k.name())
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("+")
        };
        let keys = gtk::EventControllerKey::new();
        {
            let events = events.clone();
            let keylabel = keylabel.clone();
            keys.connect_key_pressed(move |_, keyval, keycode, _| {
                let mut ev = events.borrow_mut();
                if !ev.iter().any(|(c, _)| *c == keycode) {
                    ev.push((keycode, keyval));
                }
                keylabel.set_text(&get_string(&ev));
                glib::Propagation::Stop
            });
        }
        {
            let events = events.clone();
            let keylabel = keylabel.clone();
            keys.connect_key_released(move |_, _, keycode, _| {
                let mut ev = events.borrow_mut();
                ev.retain(|(c, _)| *c != keycode);
                keylabel.set_text(&get_string(&ev));
            });
        }
        dialog.add_controller(keys);

        let (tx, rx) = futures_channel::oneshot::channel::<bool>();
        let tx = Rc::new(RefCell::new(Some(tx)));
        let send = move |val: bool| {
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(val);
            }
        };
        {
            let send = send.clone();
            ok.connect_clicked(move |_| send(true));
        }
        {
            let send = send.clone();
            cancel.connect_clicked(move |_| send(false));
        }
        dialog.connect_close_request(move |_| {
            send(false);
            glib::Propagation::Proceed
        });
        dialog.present();

        let accepted = rx.await.unwrap_or(false);
        if accepted {
            let keyvals: Vec<u32> = events.borrow().iter().map(|(_, k)| k.into_glib()).collect();
            engine::get().config().set_keys_combination(&keyvals);
        }
        self.refresh_grabkeys_combination();
        dialog.destroy();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grabkeys_default_is_ctrl_alt() {
        assert_eq!(process_grabkeys("65507,65513"), "Control_L+Alt_L");
        assert_eq!(process_grabkeys("65507,bogus"), "Control_L");
        assert_eq!(process_grabkeys(""), "");
    }
}
