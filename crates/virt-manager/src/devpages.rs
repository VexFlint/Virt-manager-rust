//! The details pages of the simpler devices: input, sound, video,
//! watchdog, smartcard, redirected USB, RNG and panic notifier. Each is
//! one frame from `details.ui`; the Apply side lives in `details.rs`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use virtinst_xml::{Document, Element};

/// `uiutil.build_simple_combo`: `[xml value, label]` rows sorted by
/// label. Unlike upstream, whose `set_list_selection` falls back to the
/// first row and so shows a model the device doesn't have (a `cirrus`
/// video reads "Bochs"), a value that isn't listed gets its own row.
pub struct SimpleCombo {
    pub widget: gtk::DropDown,
    /// The built rows, sorted by label.
    base: RefCell<Vec<(Option<String>, String)>>,
    /// What the drop-down shows: `base`, plus the unlisted value.
    values: RefCell<Vec<Option<String>>>,
    pretty: fn(&str) -> String,
    sort: bool,
}

impl SimpleCombo {
    /// `pretty` labels a value the device has that isn't listed.
    pub fn new(values: Vec<(Option<String>, String)>, pretty: fn(&str) -> String) -> SimpleCombo {
        let widget = gtk::DropDown::from_strings(&[]);
        widget.set_halign(gtk::Align::Start);
        let this = SimpleCombo {
            widget,
            base: RefCell::default(),
            values: RefCell::default(),
            pretty,
            sort: true,
        };
        this.set_values(values);
        this
    }

    /// Rows kept in the given order (a plain `ListStore`).
    pub fn unsorted(
        values: Vec<(Option<String>, String)>,
        pretty: fn(&str) -> String,
    ) -> SimpleCombo {
        let mut this = SimpleCombo::new(vec![], pretty);
        this.sort = false;
        this.set_values(values);
        this
    }

    pub fn set_values(&self, mut values: Vec<(Option<String>, String)>) {
        if self.sort {
            values.sort_by(|a, b| a.1.cmp(&b.1));
        }
        self.base.replace(values);
        self.show_rows(None);
    }

    fn show_rows(&self, extra: Option<&str>) {
        let mut rows = self.base.borrow().clone();
        if let Some(extra) = extra {
            rows.push((Some(extra.to_string()), (self.pretty)(extra)));
        }
        let labels: Vec<&str> = rows.iter().map(|(_, l)| l.as_str()).collect();
        self.values
            .replace(rows.iter().map(|(v, _)| v.clone()).collect());
        self.widget.set_model(Some(&gtk::StringList::new(&labels)));
    }

    pub fn get(&self) -> Option<String> {
        self.values
            .borrow()
            .get(self.widget.selected() as usize)
            .cloned()
            .flatten()
    }

    /// `set_list_selection`.
    pub fn set(&self, val: Option<&str>) {
        let listed = self
            .base
            .borrow()
            .iter()
            .position(|(v, _)| v.as_deref() == val);
        let idx = match (listed, val) {
            (Some(idx), _) => {
                self.show_rows(None);
                idx
            }
            (None, Some(val)) => {
                self.show_rows(Some(val));
                self.base.borrow().len()
            }
            (None, None) => {
                self.show_rows(None);
                0
            }
        };
        self.widget.set_selected(idx as u32);
    }
}

/// A combo with an entry (`graphics-address`, `char-target-name`): a
/// row, or typed text. GTK4 has no non-deprecated combo with an entry, so this is
/// an entry plus a drop-down button.
pub struct EntryCombo {
    pub top: gtk::Box,
    pub entry: gtk::Entry,
    popover: gtk::Popover,
    list: gtk::ListBox,
    rows: RefCell<Vec<(Option<String>, String)>>,
    active: Cell<Option<usize>>,
    setting: Cell<bool>,
}

impl EntryCombo {
    pub fn new(rows: Vec<(Option<String>, String)>) -> Rc<EntryCombo> {
        let entry = gtk::Entry::new();
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        let popover = gtk::Popover::new();
        popover.set_child(Some(&list));
        let button = gtk::MenuButton::new();
        button.set_popover(Some(&popover));
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        top.add_css_class("linked");
        top.set_halign(gtk::Align::Start);
        top.append(&entry);
        top.append(&button);
        let this = Rc::new(EntryCombo {
            top,
            entry,
            popover,
            list,
            rows: RefCell::default(),
            active: Cell::new(None),
            setting: Cell::new(false),
        });
        this.set_rows(rows);
        let weak = Rc::downgrade(&this);
        this.entry.connect_changed(move |_| {
            if let Some(this) = weak.upgrade() {
                if !this.setting.get() {
                    this.active.set(None);
                }
            }
        });
        let weak = Rc::downgrade(&this);
        this.list.connect_row_activated(move |_, row| {
            if let Some(this) = weak.upgrade() {
                this.set_active(row.index() as usize);
                this.popover.popdown();
            }
        });
        this
    }

    /// Replaces the rows (`model.clear()` + `append`), keeping the
    /// entry's text.
    pub fn set_rows(&self, rows: Vec<(Option<String>, String)>) {
        self.list.remove_all();
        for (_, label) in &rows {
            let l = gtk::Label::new(Some(label));
            l.set_xalign(0.0);
            l.set_margin_top(3);
            l.set_margin_bottom(3);
            l.set_margin_start(3);
            self.list.append(&l);
        }
        *self.rows.borrow_mut() = rows;
        self.active.set(None);
    }

    pub fn set_active(&self, idx: usize) {
        let Some((_, label)) = self.rows.borrow().get(idx).cloned() else {
            return;
        };
        // Signals run with `active` already set, so listeners read the
        // row's value.
        self.active.set(Some(idx));
        self.setting.set(true);
        self.entry.set_text(&label);
        self.setting.set(false);
    }

    /// `get_list_selection`: the row's value, else the typed text.
    pub fn get(&self) -> Option<String> {
        match self.active.get() {
            Some(idx) => self.rows.borrow()[idx].0.clone(),
            None => Some(self.entry.text().trim().to_string()).filter(|t| !t.is_empty()),
        }
    }

    /// `set_list_selection`: an unlisted value goes in the entry.
    pub fn set(&self, val: Option<&str>) {
        let pos = self
            .rows
            .borrow()
            .iter()
            .position(|(v, _)| v.as_deref() == val);
        match pos {
            Some(idx) => self.set_active(idx),
            None => {
                self.active.set(None);
                self.entry.set_text(val.unwrap_or_default());
            }
        }
    }
}

fn value_label() -> gtk::Label {
    let l = gtk::Label::new(None);
    l.set_xalign(0.0);
    l.set_halign(gtk::Align::Start);
    l.set_selectable(true);
    l
}

/// A label/value grid row, kept so the row can be hidden whole
/// (`uiutil.set_grid_row_visible`).
#[derive(Clone)]
pub struct GridRow {
    pub(crate) label: gtk::Label,
    widget: gtk::Widget,
}

impl GridRow {
    pub fn set_visible(&self, visible: bool) {
        self.label.set_visible(visible);
        self.widget.set_visible(visible);
    }

    /// Points the row label's mnemonic at `widget` instead.
    pub fn set_mnemonic(&self, widget: &impl IsA<gtk::Widget>) {
        self.label.set_mnemonic_widget(Some(widget));
    }

    pub fn is_visible(&self) -> bool {
        self.widget.is_visible()
    }
}

pub(crate) struct Grid {
    grid: gtk::Grid,
    rows: i32,
}

impl Grid {
    pub(crate) fn new() -> Grid {
        let grid = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(12)
            .build();
        Grid { grid, rows: 0 }
    }

    pub(crate) fn row(&mut self, text: &str, widget: &impl IsA<gtk::Widget>) -> GridRow {
        let label = gtk::Label::with_mnemonic(text);
        label.set_halign(gtk::Align::End);
        label.set_mnemonic_widget(Some(widget));
        self.grid.attach(&label, 0, self.rows, 1, 1);
        self.grid.attach(widget, 1, self.rows, 1, 1);
        self.rows += 1;
        GridRow {
            label,
            widget: widget.clone().upcast(),
        }
    }

    /// The page: the bold frame title over the indented grid.
    pub(crate) fn into_grid(self) -> gtk::Grid {
        self.grid
    }

    pub(crate) fn page(self, title: &gtk::Label) -> gtk::Box {
        frame_page(title, &self.grid)
    }
}

/// A details page: the bold frame title over the indented body.
pub(crate) fn frame_page(title: &gtk::Label, body: &impl IsA<gtk::Widget>) -> gtk::Box {
    title.set_xalign(0.0);
    let frame = gtk::Box::new(gtk::Orientation::Vertical, 6);
    frame.append(title);
    body.set_margin_start(12);
    frame.append(body);
    let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    page.append(&frame);
    page
}

pub(crate) fn title(text: &str) -> gtk::Label {
    let l = gtk::Label::new(None);
    l.set_markup(&format!("<b>{text}</b>"));
    l
}

pub(crate) fn xml_attr(doc: &Document, el: Element, path: &[&str], name: &str) -> Option<String> {
    virtinst_xml::resolve_path(doc, el, path)
        .and_then(|e| e.attribute(doc, name))
        .map(str::to_string)
}

/// `vmmAddHardware.input_pretty_name`.
pub fn input_pretty_name(typ: &str, bus: &str) -> String {
    if typ == "tablet" && bus == "usb" {
        return "EvTouch USB Graphics Tablet".into();
    }
    let typ_label = match typ {
        "keyboard" => "Keyboard",
        "mouse" => "Mouse",
        "tablet" => "Tablet",
        other => other,
    };
    let bus_label = match bus {
        "ps2" => "PS/2",
        "usb" => "USB",
        "virtio" => "VirtIO",
        "xen" => "Xen",
        other => other,
    };
    format!("{bus_label} {typ_label}")
}

/// `vmmAddHardware.sound_pretty_model`.
pub fn sound_pretty_model(model: &str) -> String {
    let ret = model.to_uppercase();
    if matches!(model, "ich6" | "ich9") {
        format!("HDA ({ret})")
    } else {
        ret
    }
}

/// `vmmAddHardware.watchdog_pretty_action`.
pub fn watchdog_pretty_action(val: &str) -> String {
    match val {
        "reset" => "Forcefully reset the guest",
        "shutdown" => "Gracefully shutdown the guest",
        "poweroff" => "Forcefully power off the guest",
        "pause" => "Pause the guest",
        "none" => "No action",
        "dump" => "Dump guest memory core",
        other => other,
    }
    .into()
}

/// `vmmAddHardware.rng_pretty_type`.
pub fn rng_pretty_type(val: &str) -> String {
    match val {
        "random" => "Random",
        "egd" => "Entropy Gathering Daemon",
        "builtin" => "Builtin RNG",
        other => other,
    }
    .into()
}

/// `vmmAddHardware.redirdev_pretty_type`.
pub fn redirdev_pretty_type(typ: &str) -> String {
    match typ {
        "tcp" => "TCP".into(),
        "spicevmc" => "SpiceVMC".into(),
        other => {
            let mut c = other.chars();
            c.next()
                .map(|f| {
                    f.to_uppercase()
                        .chain(c.flat_map(char::to_lowercase))
                        .collect()
                })
                .unwrap_or_default()
        }
    }
}

/// `input-dev-*`.
pub struct InputPage {
    pub page: gtk::Box,
    dev_type: gtk::Label,
    mode: gtk::Label,
    mode_row: GridRow,
}

impl InputPage {
    pub fn new() -> InputPage {
        let mut g = Grid::new();
        let dev_type = value_label();
        let mode = value_label();
        g.row("Type:", &dev_type);
        let mode_row = g.row("Mode:", &mode);
        InputPage {
            page: g.page(&title("Virtual Input Device")),
            dev_type,
            mode,
            mode_row,
        }
    }

    /// `_refresh_input_page`.
    pub fn refresh(&self, doc: &Document, el: Element) {
        let typ = xml_attr(doc, el, &[], "type").unwrap_or_default();
        let bus = xml_attr(doc, el, &[], "bus").unwrap_or_default();
        let mode = match typ.as_str() {
            "tablet" => Some("Absolute Movement"),
            "mouse" => Some("Relative Movement"),
            _ => None,
        };
        self.dev_type.set_text(&input_pretty_name(&typ, &bus));
        self.mode.set_text(mode.unwrap_or_default());
        self.mode_row.set_visible(mode.is_some());
    }
}

/// `sound-model`.
pub struct SoundPage {
    pub page: gtk::Box,
    pub model: SimpleCombo,
}

impl SoundPage {
    /// `build_sound_combo`.
    pub fn new() -> SoundPage {
        let model = SimpleCombo::new(
            virtinst_core::devices::sound::RECOMMENDED_MODELS
                .iter()
                .map(|m| (Some(m.to_string()), sound_pretty_model(m)))
                .collect(),
            sound_pretty_model,
        );
        let mut g = Grid::new();
        g.row("M_odel:", &model.widget);
        SoundPage {
            page: g.page(&title("Sound Device")),
            model,
        }
    }

    pub fn refresh(&self, doc: &Document, el: Element) {
        self.model.set(xml_attr(doc, el, &[], "model").as_deref());
    }
}

/// `video-model`, `video-3d`.
pub struct VideoPage {
    pub page: gtk::Box,
    pub model: SimpleCombo,
    pub accel3d: gtk::CheckButton,
    accel3d_row: GridRow,
}

impl VideoPage {
    pub fn new() -> VideoPage {
        let model = SimpleCombo::new(vec![], crate::details::video_pretty_model);
        let accel3d = gtk::CheckButton::new();
        let mut g = Grid::new();
        g.row("M_odel:", &model.widget);
        let accel3d_row = g.row("_3D acceleration:", &accel3d);
        VideoPage {
            page: g.page(&title("Video")),
            model,
            accel3d,
            accel3d_row,
        }
    }

    /// `build_video_combo`.
    pub fn build_models(&self, conn: Option<&crate::connection::VmmConnection>) {
        let (is_xen, is_qemu) =
            conn.map_or((false, false), |c| (c.is_xen(), c.is_qemu() || c.is_test()));
        let mut values: Vec<_> = virtinst_core::devices::video::recommended_models(is_xen, is_qemu)
            .iter()
            .map(|m| (Some(m.to_string()), crate::details::video_pretty_model(m)))
            .collect();
        if values.is_empty() {
            values.push((None, "Hypervisor default".into()));
        }
        self.model.set_values(values);
    }

    /// `_video_model_changed_cb`'s row toggle.
    pub fn sync_3d_row(&self) {
        self.accel3d_row
            .set_visible(self.model.get().as_deref() == Some("virtio"));
    }

    /// `_refresh_video_page`.
    pub fn refresh(&self, doc: &Document, el: Element) {
        self.model
            .set(xml_attr(doc, el, &["model"], "type").as_deref());
        self.sync_3d_row();
        match xml_attr(doc, el, &["model", "acceleration"], "accel3d") {
            None => self.accel3d.set_inconsistent(true),
            Some(v) => {
                self.accel3d.set_inconsistent(false);
                self.accel3d.set_active(v == "yes");
            }
        }
    }
}

/// `watchdog-model`, `watchdog-action`.
pub struct WatchdogPage {
    pub page: gtk::Box,
    pub model: SimpleCombo,
    pub action: SimpleCombo,
}

impl WatchdogPage {
    /// `build_watchdogmodel_combo`, `build_watchdogaction_combo`.
    pub fn new() -> WatchdogPage {
        use virtinst_core::devices::watchdog::{ACTIONS, MODELS};
        let model = SimpleCombo::new(
            MODELS
                .iter()
                .map(|m| (Some(m.to_string()), m.to_uppercase()))
                .collect(),
            |m| m.to_uppercase(),
        );
        let action = SimpleCombo::new(
            ACTIONS
                .iter()
                .map(|a| (Some(a.to_string()), watchdog_pretty_action(a)))
                .collect(),
            watchdog_pretty_action,
        );
        let mut g = Grid::new();
        g.row("M_odel:", &model.widget);
        g.row("Ac_tion:", &action.widget);
        WatchdogPage {
            page: g.page(&title("Watchdog")),
            model,
            action,
        }
    }

    pub fn refresh(&self, doc: &Document, el: Element) {
        self.model.set(xml_attr(doc, el, &[], "model").as_deref());
        self.action.set(xml_attr(doc, el, &[], "action").as_deref());
    }
}

/// `smartcard-mode`.
pub struct SmartcardPage {
    pub page: gtk::Box,
    pub mode: SimpleCombo,
}

impl SmartcardPage {
    /// `build_smartcard_mode_combo`.
    pub fn new() -> SmartcardPage {
        let mode = SimpleCombo::new(
            vec![
                (Some("passthrough".into()), "Passthrough".into()),
                (Some("host".into()), "Host".into()),
            ],
            str::to_string,
        );
        let mut g = Grid::new();
        g.row("M_ode:", &mode.widget);
        SmartcardPage {
            page: g.page(&title("Smartcard Device")),
            mode,
        }
    }

    pub fn refresh(&self, doc: &Document, el: Element) {
        self.mode.set(xml_attr(doc, el, &[], "mode").as_deref());
    }
}

/// `redir-*`.
pub struct RedirPage {
    pub page: gtk::Box,
    title: gtk::Label,
    dev_type: gtk::Label,
    address: gtk::Label,
    address_row: GridRow,
}

impl RedirPage {
    pub fn new() -> RedirPage {
        let title = title("Redirected device");
        let dev_type = value_label();
        let address = value_label();
        let mut g = Grid::new();
        g.row("Type:", &dev_type);
        let address_row = g.row("Address:", &address);
        RedirPage {
            page: g.page(&title),
            title,
            dev_type,
            address,
            address_row,
        }
    }

    /// `_refresh_redir_page`; the title is the device's hw-list label.
    pub fn refresh(&self, doc: &Document, el: Element, label: &str) {
        let typ = xml_attr(doc, el, &[], "type").unwrap_or_default();
        let address = (typ == "tcp").then(|| {
            format!(
                "{}:{}",
                xml_attr(doc, el, &["source"], "host").unwrap_or_default(),
                xml_attr(doc, el, &["source"], "service").unwrap_or_default()
            )
        });
        self.title.set_markup(&gtk::glib::markup_escape_text(label));
        self.dev_type.set_text(&redirdev_pretty_type(&typ));
        self.address
            .set_text(address.as_deref().unwrap_or_default());
        self.address_row.set_visible(address.is_some());
    }
}

/// `rng-*`.
pub struct RngPage {
    pub page: gtk::Box,
    rng_type: gtk::Label,
    device: gtk::Label,
    device_row: GridRow,
}

impl RngPage {
    pub fn new() -> RngPage {
        let rng_type = value_label();
        let device = value_label();
        let mut g = Grid::new();
        g.row("Type:", &rng_type);
        let device_row = g.row("Host Device:", &device);
        RngPage {
            page: g.page(&title("Random Number Generator")),
            rng_type,
            device,
            device_row,
        }
    }

    /// `_refresh_rng_page`. `device` is `<backend>`'s text.
    pub fn refresh(&self, doc: &Document, el: Element) {
        let backend = el.find(doc, "backend");
        let model = backend
            .and_then(|b| b.attribute(doc, "model"))
            .unwrap_or_default()
            .to_string();
        let device = backend.map(|b| b.text_content(doc)).unwrap_or_default();
        self.device_row.set_visible(model == "random");
        self.rng_type.set_text(&rng_pretty_type(&model));
        self.device.set_text(device.trim());
    }
}

/// `panic-model`.
pub struct PanicPage {
    pub page: gtk::Box,
    model: gtk::Label,
}

impl PanicPage {
    pub fn new() -> PanicPage {
        let model = value_label();
        let mut g = Grid::new();
        g.row("Model:", &model);
        PanicPage {
            page: g.page(&title("Panic Notifier")),
            model,
        }
    }

    pub fn refresh(&self, doc: &Document, el: Element) {
        self.model
            .set_text(&xml_attr(doc, el, &[], "model").unwrap_or_default());
    }
}

/// `char-*`: serial, parallel, console and channel devices.
pub struct CharPage {
    pub page: gtk::Box,
    title: gtk::Label,
    dev_type: gtk::Label,
    /// `(row, value)` for source path, source host, bind host, target
    /// type, target name, state and clipboard.
    rows: Vec<(GridRow, gtk::Label)>,
}

impl CharPage {
    pub fn new() -> CharPage {
        let title = title("insert type");
        let dev_type = value_label();
        let mut g = Grid::new();
        g.row("Device type:", &dev_type);
        let rows = [
            "Source path:",
            "Source host:",
            "Bind host:",
            "Target type:",
            "Target name:",
            "State:",
            "Clipboard:",
        ]
        .iter()
        .map(|text| {
            let value = value_label();
            (g.row(text, &value), value)
        })
        .collect();
        CharPage {
            page: g.page(&title),
            title,
            dev_type,
            rows,
        }
    }

    /// `_refresh_char_page`. `primary` is `serial_is_console_dup`.
    pub fn refresh(&self, doc: &Document, el: Element, tag: &str, primary: bool) {
        let target_port: Option<i64> =
            xml_attr(doc, el, &["target"], "port").and_then(|p| p.parse().ok());
        let dev_type = xml_attr(doc, el, &[], "type").unwrap_or_else(|| "pty".into());
        let show_target_type = !matches!(tag, "serial" | "parallel");
        let target_name = xml_attr(doc, el, &["target"], "name");
        let is_qemuga = target_name.as_deref() == Some("org.qemu.guest_agent.0");
        let show_clipboard = xml_attr(doc, el, &[], "type").as_deref() == Some("qemu-vdagent");

        let mut typelabel = match tag {
            "serial" => "Serial Device".to_string(),
            "parallel" => "Parallel Device".to_string(),
            "console" => "Console Device".to_string(),
            "channel" => "Channel Device".to_string(),
            other => {
                let mut c = other.chars();
                let cap: String = c
                    .next()
                    .map(|f| f.to_uppercase().chain(c).collect())
                    .unwrap_or_default();
                format!("{cap} Device")
            }
        };
        if let Some(port) = target_port {
            if tag == "console" || !show_target_type {
                typelabel += &format!(" {}", port + 1);
            }
        }
        if primary {
            typelabel += " (Primary Console)";
        }
        self.title.set_markup(&format!(
            "<b>{}</b>",
            gtk::glib::markup_escape_text(&typelabel)
        ));
        self.dev_type.set_text(&dev_type);

        // `CharSource`: the first `<source>`, plus the connect/bind
        // sources by mode.
        let sources = el.find_all(doc, "source");
        let by_mode = |mode: &str, name: &str| {
            sources
                .iter()
                .find(|s| s.attribute(doc, "mode") == Some(mode))
                .and_then(|s| s.attribute(doc, name))
                .map(str::to_string)
        };
        let build_host_str = |host: Option<String>, port: Option<String>| {
            let mut ret = host.unwrap_or_default();
            if let Some(port) = port.and_then(|p| p.parse::<i64>().ok()) {
                ret += &format!(":{port}");
            }
            Some(ret).filter(|r| !r.is_empty())
        };
        let first = sources.first().copied();
        let path = first
            .and_then(|s| s.attribute(doc, "path"))
            .map(str::to_string);
        let clipboard = first
            .and_then(|s| s.find(doc, "clipboard"))
            .and_then(|c| c.attribute(doc, "copypaste"))
            == Some("yes");
        let values: [(Option<String>, Option<bool>); 7] = [
            (path, None),
            (
                build_host_str(by_mode("connect", "host"), by_mode("connect", "service")),
                None,
            ),
            (
                build_host_str(by_mode("bind", "host"), by_mode("bind", "service")),
                None,
            ),
            (
                show_target_type
                    .then(|| xml_attr(doc, el, &["target"], "type"))
                    .flatten(),
                None,
            ),
            (target_name, None),
            // Only for the qemu guest agent, which we get async
            // notifications about connection state.
            (xml_attr(doc, el, &["target"], "state"), Some(is_qemuga)),
            (
                Some(if clipboard { "On" } else { "Off" }.to_string()),
                Some(show_clipboard),
            ),
        ];
        for ((row, label), (val, doshow)) in self.rows.iter().zip(values) {
            row.set_visible(doshow.unwrap_or(val.is_some()));
            label.set_text(val.as_deref().unwrap_or("-"));
        }
    }
}

/// `controller-*`.
pub struct ControllerPage {
    pub page: gtk::Box,
    ctype: gtk::Label,
    pub model: SimpleCombo,
    model_row: GridRow,
    devices: gtk::ListBox,
    devices_row: GridRow,
}

impl ControllerPage {
    pub fn new() -> ControllerPage {
        let ctype = value_label();
        let model = SimpleCombo::unsorted(vec![], str::to_string);
        let devices = gtk::ListBox::new();
        devices.set_selection_mode(gtk::SelectionMode::None);
        let scroll = gtk::ScrolledWindow::builder()
            .child(&devices)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .min_content_height(80)
            .min_content_width(300)
            .has_frame(true)
            .build();
        let mut g = Grid::new();
        g.row("Type:", &ctype);
        let model_row = g.row("M_odel:", &model.widget);
        let devices_row = g.row("Devices:", &scroll);
        devices_row.label.set_valign(gtk::Align::Start);
        ControllerPage {
            page: g.page(&title("Controller")),
            ctype,
            model,
            model_row,
            devices,
            devices_row,
        }
    }

    /// `_refresh_controller_page`. `type_label` is
    /// `controller_pretty_desc`; `attached` lists the disks on a
    /// disk controller as "<hw-list label> on <address>".
    pub fn refresh(
        &self,
        doc: &Document,
        el: Element,
        type_label: &str,
        attached: Option<Vec<String>>,
    ) {
        let ctype = xml_attr(doc, el, &[], "type").unwrap_or_default();
        self.devices.remove_all();
        self.devices_row.set_visible(attached.is_some());
        for info in attached.iter().flatten() {
            let l = gtk::Label::new(Some(info));
            l.set_xalign(0.0);
            l.set_margin_start(3);
            self.devices.append(&l);
        }
        self.ctype.set_text(type_label);

        // `populate_controller_model_combo`
        let rows = virtinst_core::devices::controller::model_rows(&ctype);
        let n_rows = rows.len();
        self.model.set_values(
            rows.into_iter()
                .map(|(v, l)| (v.map(str::to_string), l.to_string()))
                .collect(),
        );
        let mut model = xml_attr(doc, el, &[], "model");
        let show_model = (model.is_some() || n_rows > 1) && ctype != "pci";
        self.model_row.set_visible(show_model);
        if ctype == "usb" && model.as_deref().is_some_and(|m| m.contains("xhci")) {
            model = Some("usb3".into());
        }
        self.model.set(model.as_deref());
    }
}

/// `hostdev-*`.
pub struct HostdevPage {
    pub page: gtk::Box,
    title: gtk::Label,
    source: gtk::Label,
    pub rombar: gtk::CheckButton,
    rombar_row: GridRow,
    pub startup_policy: SimpleCombo,
    startup_policy_row: GridRow,
}

impl HostdevPage {
    /// `build_hostdev_usb_startup_policy_combo`.
    pub fn new() -> HostdevPage {
        use virtinst_core::devices::hostdev::STARTUP_POLICIES;
        let source = value_label();
        let rombar = gtk::CheckButton::new();
        let mut rows = vec![(None, "Hypervisor default".to_string())];
        rows.extend(
            STARTUP_POLICIES
                .iter()
                .map(|p| (Some(p.to_string()), p.to_string())),
        );
        let startup_policy = SimpleCombo::new(rows, str::to_string);
        let mut g = Grid::new();
        g.row("Device:", &source);
        let rombar_row = g.row("ROM _BAR:", &rombar);
        let startup_policy_row = g.row("Startup Policy:", &startup_policy.widget);
        let title = title("");
        HostdevPage {
            page: g.page(&title),
            title,
            source,
            rombar,
            rombar_row,
            startup_policy,
            startup_policy_row,
        }
    }

    /// `_refresh_hostdev_page`; `pretty_name` is the matching node
    /// device's, else `hostdev_pretty_name`.
    pub fn refresh(&self, doc: &Document, el: Element, pretty_name: &str) {
        let htype = xml_attr(doc, el, &[], "type").unwrap_or_default();
        // `rom_bar` is `is_onoff`, and unset means on
        let rom_bar = xml_attr(doc, el, &["rom"], "bar").map_or(true, |b| b == "on");
        self.rombar_row.set_visible(htype == "pci");
        self.startup_policy_row.set_visible(htype == "usb");
        if htype == "usb" {
            self.startup_policy
                .set(xml_attr(doc, el, &["source"], "startupPolicy").as_deref());
        }
        self.title.set_markup(&format!(
            "<b>Physical {} Device</b>",
            gtk::glib::markup_escape_text(&htype.to_uppercase())
        ));
        self.set_source(pretty_name);
        self.rombar.set_active(rom_bar);
    }

    pub fn set_source(&self, pretty_name: &str) {
        self.source.set_text(pretty_name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_names() {
        assert_eq!(
            input_pretty_name("tablet", "usb"),
            "EvTouch USB Graphics Tablet"
        );
        assert_eq!(input_pretty_name("mouse", "ps2"), "PS/2 Mouse");
        assert_eq!(input_pretty_name("keyboard", "virtio"), "VirtIO Keyboard");
        assert_eq!(sound_pretty_model("ich9"), "HDA (ICH9)");
        assert_eq!(sound_pretty_model("ac97"), "AC97");
        assert_eq!(redirdev_pretty_type("spicevmc"), "SpiceVMC");
        assert_eq!(redirdev_pretty_type("pty"), "Pty");
        assert_eq!(rng_pretty_type("egd"), "Entropy Gathering Daemon");
        assert_eq!(watchdog_pretty_action("dump"), "Dump guest memory core");
    }
}
