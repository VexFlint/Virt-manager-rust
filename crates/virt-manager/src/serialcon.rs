//! Upstream's `virtManager/details/serialcon.py`: a VTE terminal on a
//! guest's serial or text console, fed by a libvirt console stream.
//!
//! The stream's callbacks are dispatched by the GLib main loop through
//! [`crate::libvirtglib`], as upstream's are through libvirt-glib.

use std::cell::RefCell;
use std::ffi::{c_int, c_void};
use std::rc::{Rc, Weak};

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use virt::sys;
use vte4::prelude::*;

use crate::details::serial_console_lists;
use crate::domain::VmmDomain;

const STREAM_EVENTS_READ: c_int = (sys::VIR_STREAM_EVENT_READABLE
    | sys::VIR_STREAM_EVENT_ERROR
    | sys::VIR_STREAM_EVENT_HANGUP) as c_int;
const STREAM_EVENTS_READ_WRITE: c_int =
    STREAM_EVENTS_READ | sys::VIR_STREAM_EVENT_WRITABLE as c_int;

/// `_DataStream`'s state.
#[derive(Default)]
struct StreamState {
    stream: Option<sys::virStreamPtr>,
    stream_to_terminal: Vec<u8>,
    terminal_to_stream: Vec<u8>,
}

/// `_DataStream`: wrapper for interacting with the libvirt console
/// stream.
struct DataStream {
    vm: VmmDomain,
    terminal: vte4::Terminal,
    state: RefCell<StreamState>,
}

impl DataStream {
    /// `_display_data`.
    fn display_data(&self) {
        let data = std::mem::take(&mut self.state.borrow_mut().stream_to_terminal);
        if !data.is_empty() {
            self.terminal.feed(&data);
        }
    }

    /// `_event_on_stream`.
    fn event_on_stream(self: &Rc<Self>, events: c_int) {
        let events = events as u32;
        let Some(stream) = self.state.borrow().stream else {
            return;
        };
        if events & (sys::VIR_STREAM_EVENT_ERROR | sys::VIR_STREAM_EVENT_HANGUP) != 0 {
            eprintln!("Received stream ERROR/HANGUP, closing console");
            self.close();
            return;
        }

        if events & sys::VIR_STREAM_EVENT_READABLE != 0 {
            let mut buf = vec![0u8; 1024 * 100];
            let got = unsafe { sys::virStreamRecv(stream, buf.as_mut_ptr().cast(), buf.len()) };
            if got == -2 {
                // This is basically EAGAIN
                return;
            }
            if got < 0 {
                eprintln!("Error receiving stream data");
                self.close();
                return;
            }
            if got == 0 {
                eprintln!("Received EOF from stream, closing");
                self.close();
                return;
            }
            buf.truncate(got as usize);
            let queued_text = {
                let mut st = self.state.borrow_mut();
                let queued = !st.stream_to_terminal.is_empty();
                st.stream_to_terminal.extend_from_slice(&buf);
                queued
            };
            if !queued_text {
                let weak = Rc::downgrade(self);
                glib::idle_add_local_once(move || {
                    if let Some(this) = weak.upgrade() {
                        this.display_data();
                    }
                });
            }
        }

        let pending = !self.state.borrow().terminal_to_stream.is_empty();
        if events & sys::VIR_STREAM_EVENT_WRITABLE != 0 && pending {
            let data = self.state.borrow().terminal_to_stream.clone();
            let done = unsafe { sys::virStreamSend(stream, data.as_ptr().cast(), data.len()) };
            if done == -2 {
                // This is basically EAGAIN
                return;
            }
            if done < 0 {
                eprintln!("Error sending stream data");
                self.close();
                return;
            }
            self.state
                .borrow_mut()
                .terminal_to_stream
                .drain(..done as usize);
        }

        if self.state.borrow().terminal_to_stream.is_empty() {
            unsafe { sys::virStreamEventUpdateCallback(stream, STREAM_EVENTS_READ) };
        }
    }

    /// `open`.
    fn open(self: &Rc<Self>, alias: Option<&str>) -> Result<(), String> {
        if self.state.borrow().stream.is_some() {
            return Ok(());
        }
        eprintln!(
            "Opening console stream for alias={}",
            alias.unwrap_or("None")
        );
        // libxl doesn't set aliases, their open_console just defaults
        // to opening the first console device, so don't force presence
        // of an alias
        let stream = self.vm.open_console(alias)?;

        let data = Box::into_raw(Box::new(Rc::downgrade(self))) as *mut c_void;
        let ret = unsafe {
            sys::virStreamEventAddCallback(
                stream,
                STREAM_EVENTS_READ,
                Some(stream_event_cb),
                data,
                Some(stream_event_free),
            )
        };
        if ret < 0 {
            unsafe {
                stream_event_free(data);
                sys::virStreamAbort(stream);
                sys::virStreamFree(stream);
            }
            return Err(last_libvirt_error());
        }
        self.state.borrow_mut().stream = Some(stream);
        Ok(())
    }

    /// `close`.
    fn close(&self) {
        let Some(stream) = self.state.borrow_mut().stream.take() else {
            return;
        };
        unsafe {
            if sys::virStreamEventRemoveCallback(stream) < 0 {
                eprintln!("Error removing stream callback");
            }
            if sys::virStreamFinish(stream) < 0 {
                eprintln!("Error finishing stream");
            }
            sys::virStreamFree(stream);
        }
    }

    /// `send_data`: callback when data has been entered into the VTE
    /// terminal.
    fn send_data(&self, text: &str) {
        let Some(stream) = self.state.borrow().stream else {
            return;
        };
        let pending = {
            let mut st = self.state.borrow_mut();
            st.terminal_to_stream.extend_from_slice(text.as_bytes());
            !st.terminal_to_stream.is_empty()
        };
        if pending {
            unsafe { sys::virStreamEventUpdateCallback(stream, STREAM_EVENTS_READ_WRITE) };
        }
    }
}

impl Drop for DataStream {
    fn drop(&mut self) {
        self.close();
    }
}

unsafe extern "C" fn stream_event_cb(
    _stream: sys::virStreamPtr,
    events: c_int,
    opaque: *mut c_void,
) {
    let weak = &*(opaque as *const Weak<DataStream>);
    if let Some(this) = weak.upgrade() {
        this.event_on_stream(events);
    }
}

unsafe extern "C" fn stream_event_free(opaque: *mut c_void) {
    drop(Box::from_raw(opaque as *mut Weak<DataStream>));
}

fn last_libvirt_error() -> String {
    virt::error::Error::last_error().message().to_string()
}

/// `_viewer_sync_modifiers` / `_disable_modifiers`: while the terminal
/// has focus, keys go to it ahead of the window's shortcuts, as
/// upstream removes its accel groups then. A window-level capture
/// handler forwards them, the way the graphical displays take theirs.
fn disable_modifiers_while_focused(terminal: &vte4::Terminal) {
    terminal.connect_realize(|terminal| {
        let Some(root) = terminal.root() else { return };
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let t = terminal.downgrade();
        keys.connect_key_pressed(move |c, _, _, _| match t.upgrade() {
            Some(t) if t.has_focus() => {
                c.forward(&t);
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
        let t = terminal.downgrade();
        keys.connect_key_released(move |c, _, _, _| {
            if let Some(t) = t.upgrade().filter(|t| t.has_focus()) {
                c.forward(&t);
            }
        });
        root.add_controller(keys.clone());
        let weak_ctrl = keys.downgrade();
        terminal.connect_unrealize(move |terminal| {
            if let (Some(root), Some(ctrl)) = (terminal.root(), weak_ctrl.upgrade()) {
                root.remove_controller(&ctrl);
            }
        });
    });
}

/// `evbox.modify_bg(Gdk.Color(0, 0, 0))`, as a style class.
fn install_css(widget: &impl IsA<gtk::Widget>) {
    thread_local!(static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    if DONE.with(|d| d.replace(true)) {
        return;
    }
    let provider = gtk::CssProvider::new();
    provider.load_from_string(".vmm-serial-background { background-color: black; }");
    gtk::style_context_add_provider_for_display(
        &widget.display(),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

/// `vmmSerialConsole.can_connect`: why the device can't be opened, if
/// it can't.
pub fn can_connect(ctype: &str) -> Option<String> {
    let usable_types = ["pty"];
    (!usable_types.contains(&ctype))
        .then(|| format!("Console for device type '{ctype}' is not supported"))
}

struct Inner {
    vm: VmmDomain,
    target_port: usize,
    name: String,
    lastpath: RefCell<Option<String>>,
    datastream: Rc<DataStream>,
    terminal: vte4::Terminal,
    stack: gtk::Stack,
    error_label: gtk::Label,
    state_handler: RefCell<Option<glib::SignalHandlerId>>,
}

/// `vmmSerialConsole`.
#[derive(Clone)]
pub struct SerialConsole(Rc<Inner>);

impl SerialConsole {
    pub fn new(vm: &VmmDomain, target_port: usize, name: &str) -> SerialConsole {
        // _init_terminal
        let terminal = vte4::Terminal::new();
        terminal.set_scrollback_lines(1000);
        terminal.set_audible_bell(false);
        terminal.set_hexpand(true);
        terminal.set_vexpand(true);
        terminal.update_property(&[gtk::accessible::Property::Label("Serial Terminal")]);

        let datastream = Rc::new(DataStream {
            vm: vm.clone(),
            terminal: terminal.clone(),
            state: RefCell::default(),
        });
        let weak = Rc::downgrade(&datastream);
        terminal.connect_commit(move |_, text, _| {
            if let Some(ds) = weak.upgrade() {
                ds.send_data(text);
            }
        });

        // _init_ui
        let stack = gtk::Stack::new();
        let align = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        for m in [
            gtk::Widget::set_margin_top,
            gtk::Widget::set_margin_bottom,
            gtk::Widget::set_margin_start,
            gtk::Widget::set_margin_end,
        ] {
            m(terminal.upcast_ref(), 2);
        }
        align.append(&terminal);
        let evbox = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        evbox.add_css_class("vmm-serial-background");
        evbox.append(&align);
        evbox.set_hexpand(true);
        install_css(&evbox);

        let terminalbox = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let scrollbar =
            gtk::Scrollbar::new(gtk::Orientation::Vertical, terminal.vadjustment().as_ref());
        terminalbox.append(&evbox);
        terminalbox.append(&scrollbar);

        let error_label = gtk::Label::new(None);
        error_label.set_width_chars(40);
        error_label.set_wrap(true);

        stack.add_named(&terminalbox, Some("terminal"));
        stack.add_named(&error_label, Some("error"));

        scrollbar.set_visible(false);
        if let Some(adj) = terminal.vadjustment() {
            let sb = scrollbar.downgrade();
            adj.connect_changed(move |adj| {
                // _scrollbar_adjustment_changed
                if let Some(sb) = sb.upgrade() {
                    sb.set_visible(adj.upper() > adj.page_size());
                }
            });
        }

        let this = SerialConsole(Rc::new(Inner {
            vm: vm.clone(),
            target_port,
            name: name.to_string(),
            lastpath: RefCell::default(),
            datastream,
            terminal,
            stack,
            error_label,
            state_handler: RefCell::default(),
        }));
        this.init_popup();
        disable_modifiers_while_focused(&this.0.terminal);

        let weak = Rc::downgrade(&this.0);
        let id = vm.connect_local("state-changed", false, move |_| {
            if let Some(inner) = weak.upgrade() {
                SerialConsole(inner).vm_status_changed();
            }
            None
        });
        this.0.state_handler.replace(Some(id));
        this
    }

    /// `_init_popup` / `_show_serial_rcpopup`: Copy and Paste on a
    /// right click.
    fn init_popup(&self) {
        let terminal = &self.0.terminal;
        let actions = gtk::gio::SimpleActionGroup::new();
        let copy = gtk::gio::SimpleAction::new("copy", None);
        let t = terminal.downgrade();
        copy.connect_activate(move |_, _| {
            // _serial_copy_text
            if let Some(t) = t.upgrade() {
                t.copy_clipboard_format(vte4::Format::Text);
            }
        });
        let paste = gtk::gio::SimpleAction::new("paste", None);
        let t = terminal.downgrade();
        paste.connect_activate(move |_, _| {
            // _serial_paste_text
            if let Some(t) = t.upgrade() {
                t.paste_clipboard();
            }
        });
        actions.add_action(&copy);
        actions.add_action(&paste);
        terminal.insert_action_group("serial", Some(&actions));

        let menu = gtk::gio::Menu::new();
        menu.append(Some("_Copy"), Some("serial.copy"));
        menu.append(Some("_Paste"), Some("serial.paste"));
        let popup = gtk::PopoverMenu::from_model(Some(&menu));
        popup.set_parent(terminal);
        popup.set_has_arrow(false);
        popup.set_halign(gtk::Align::Start);
        popup.update_property(&[gtk::accessible::Property::Label("serial-popup-menu")]);

        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_SECONDARY);
        let t = terminal.downgrade();
        click.connect_pressed(move |_, _, x, y| {
            let Some(t) = t.upgrade() else { return };
            copy.set_enabled(t.has_selection());
            popup.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            popup.popup();
        });
        terminal.add_controller(click);
    }

    pub fn cleanup(&self) {
        self.0.datastream.close();
        if let Some(id) = self.0.state_handler.take() {
            self.0.vm.disconnect(id);
        }
    }

    pub fn name(&self) -> &str {
        &self.0.name
    }

    /// `_show_error`.
    fn show_error(&self, msg: &str) {
        self.0
            .error_label
            .set_markup(&format!("<b>{}</b>", glib::markup_escape_text(msg)));
        self.0.stack.set_visible_child_name("error");
    }

    /// `_lookup_dev`: the device's alias, if any.
    fn lookup_dev(&self) -> Option<Option<String>> {
        let xml = self.0.vm.xmlobj()?;
        let doc = &xml.doc;
        let devices = xml.root().find(doc, "devices")?;
        let (serials, consoles) = serial_console_lists(doc, devices);
        let found = serials
            .into_iter()
            .chain(consoles)
            .find(|(idx, _)| *idx == self.0.target_port);
        let Some((_, el)) = found else {
            eprintln!(
                "No devices found for serial target port '{}'",
                self.0.target_port
            );
            self.0.lastpath.replace(None);
            return None;
        };
        let path = el
            .find(doc, "source")
            .and_then(|s| s.attribute(doc, "path"))
            .map(str::to_string);
        if *self.0.lastpath.borrow() != path {
            eprintln!(
                "Serial console '{}' path changed to {}",
                self.0.target_port,
                path.as_deref().unwrap_or("None")
            );
        }
        self.0.lastpath.replace(path);
        let alias = el
            .find(doc, "alias")
            .and_then(|a| a.attribute(doc, "name"))
            .map(str::to_string);
        Some(alias)
    }

    // Public API

    pub fn close(&self) {
        self.0.datastream.close();
    }

    pub fn get_box(&self) -> gtk::Widget {
        self.0.stack.clone().upcast()
    }

    pub fn open_console(&self) -> bool {
        let alias = self.lookup_dev().flatten();
        match self.0.datastream.open(alias.as_deref()) {
            Ok(()) => {
                self.0.stack.set_visible_child_name("terminal");
                true
            }
            Err(e) => {
                eprintln!("Error opening serial console: {e}");
                self.show_error(&format!("Error connecting to text console: {e}"));
                self.0.datastream.close();
                false
            }
        }
    }

    /// `_vm_status_changed`.
    fn vm_status_changed(&self) {
        if self.0.vm.status() == sys::VIR_DOMAIN_RUNNING {
            self.open_console();
        } else {
            self.0.datastream.close();
        }
    }
}
