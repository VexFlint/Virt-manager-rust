//! spice-gtk's `SpiceDisplay` widget (`src/spice-widget.c`) for GTK4:
//! a [`DisplayWidget`] showing display channel 0's primary surface,
//! feeding pointer and keyboard input to the inputs channel and
//! showing the cursor channel's cursor.
//!
//! It follows `spice-widget.c` closely: the same monitor area and
//! readiness rules, the same size request, input transform and mouse
//! modes, the same key state tracking with delayed key presses, the
//! same grab sequence (which releases the keyboard grab, and toggles
//! the pointer grab in server mouse mode) and the same guest resizing.
//! GTK4 has no pointer or keyboard grabs, so as with the VNC display
//! the keyboard grab is emulated (see [`display::connect_keys`]), and
//! a server-mode pointer grab sends motion deltas without warping the
//! pointer back. Only the cairo rendering path exists, as in a
//! spice-gtk built without EGL.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::glib::translate::IntoGlib;
use gtk::prelude::*;

use crate::display::{self, DisplayWidget, FrameCopy};
use crate::keymap;
use crate::spice::{self, ChannelKind, Session};
use crate::spicegtksession::SpiceGtkSession;

/// `DEFAULT_KEYPRESS_DELAY`, in milliseconds.
const DEFAULT_KEYPRESS_DELAY: u32 = 100;

/// The display channel and monitor shown: `spice_display_new(session,
/// 0)` with the default `monitor-id` of 0.
const CHANNEL_ID: i32 = 0;
const MONITOR_ID: u32 = 0;

/// `get_display_id`: the monitor id, as only channel 0 is shown.
const DISPLAY_ID: i32 = MONITOR_ID as i32;

/// `SpiceDisplay`'s grab signals.
#[derive(Debug, Clone, Copy)]
pub enum SpiceDisplayEvent {
    /// `mouse-grab`
    MouseGrab(bool),
    /// `keyboard-grab`
    KeyboardGrab(bool),
}

/// The primary surface as `display-primary-create` announced it.
#[derive(Clone, Copy)]
struct Canvas {
    format: i32,
    width: i32,
    height: i32,
    stride: i32,
    data: *const u8,
}

/// `d->mouse_pixbuf` and its hotspot.
struct Shape {
    texture: gdk::Texture,
    pixbuf: gtk::gdk_pixbuf::Pixbuf,
    hotx: i32,
    hoty: i32,
}

/// `(x, y, width, height)`
type Rect = (i32, i32, i32, i32);

struct State {
    session: Session,
    gtk_session: SpiceGtkSession,
    /// The session's `inhibit-keyboard-grab`, set while a USB device
    /// is being redirected.
    keyboard_grab_inhibit: bool,
    main: Option<glib::Object>,
    display: Option<glib::Object>,
    inputs: Option<glib::Object>,
    cursor: Option<glib::Object>,
    /// Channels already handled by `channel_new`.
    seen: Vec<glib::Object>,
    handlers: Vec<(glib::Object, glib::SignalHandlerId)>,

    canvas: Option<Canvas>,
    area: Rect,
    mark: i32,
    monitor_ready: bool,
    ready: bool,
    allow_scaling: bool,
    resize_guest_enable: bool,
    /// The allocation, `d->ww` / `d->wh`.
    ww: i32,
    wh: i32,

    mouse_mode: i32,
    mouse_grab_active: bool,
    mouse_have_pointer: bool,
    keyboard_have_focus: bool,
    keyboard_grab_active: bool,
    keyboard_grab_released: bool,
    mouse_last: (i32, i32),
    mouse_guest: (i32, i32),
    mouse_button_mask: i32,
    scroll_delta_y: f64,

    key_state: [u32; 512 / 32],
    key_delayed_scancode: u32,
    key_delayed_id: Option<glib::SourceId>,
    keypress_delay: u32,
    grabseq: Vec<u32>,
    activeseq: Vec<bool>,
    seq_pressed: bool,

    shape: Option<Shape>,
    mouse_cursor: Option<gdk::Cursor>,
    /// `d->show_cursor != NULL`: the guest hid its cursor.
    cursor_hidden: bool,
}

type Listener = Rc<dyn Fn(SpiceDisplayEvent)>;

struct Inner {
    area: DisplayWidget,
    state: RefCell<State>,
    callback: RefCell<Option<Listener>>,
}

#[derive(Clone)]
pub struct SpiceDisplay(Rc<Inner>);

macro_rules! upgrade_or {
    ($weak:expr) => {
        match $weak.upgrade() {
            Some(inner) => SpiceDisplay(inner),
            None => return,
        }
    };
    ($weak:expr, $ret:expr) => {
        match $weak.upgrade() {
            Some(inner) => SpiceDisplay(inner),
            None => return $ret,
        }
    };
}

impl SpiceDisplay {
    /// `spice_display_new(session, 0)`.
    pub fn new(session: &Session) -> SpiceDisplay {
        let grabseq = ["Control_L", "Alt_L"]
            .iter()
            .filter_map(|n| gdk::Key::from_name(*n))
            .map(|k| k.into_glib())
            .collect::<Vec<_>>();
        let this = SpiceDisplay(Rc::new(Inner {
            area: DisplayWidget::default(),
            state: RefCell::new(State {
                session: session.clone(),
                gtk_session: SpiceGtkSession::get(session),
                keyboard_grab_inhibit: false,
                main: None,
                display: None,
                inputs: None,
                cursor: None,
                seen: vec![],
                handlers: vec![],
                canvas: None,
                area: (0, 0, 0, 0),
                mark: 0,
                monitor_ready: false,
                ready: false,
                allow_scaling: false,
                resize_guest_enable: false,
                ww: 0,
                wh: 0,
                mouse_mode: 0,
                mouse_grab_active: false,
                mouse_have_pointer: false,
                keyboard_have_focus: false,
                keyboard_grab_active: false,
                keyboard_grab_released: false,
                mouse_last: (-1, -1),
                mouse_guest: (-1, -1),
                mouse_button_mask: 0,
                scroll_delta_y: 0.0,
                key_state: [0; 16],
                key_delayed_scancode: 0,
                key_delayed_id: None,
                keypress_delay: DEFAULT_KEYPRESS_DELAY,
                activeseq: vec![false; grabseq.len()],
                grabseq,
                seq_pressed: false,
                shape: None,
                mouse_cursor: None,
                cursor_hidden: false,
            }),
            callback: RefCell::default(),
        }));
        this.init_widget();
        this.constructed();
        this
    }

    pub fn widget(&self) -> &DisplayWidget {
        &self.0.area
    }

    /// The single listener for [`SpiceDisplayEvent`]s.
    pub fn connect_event(&self, f: impl Fn(SpiceDisplayEvent) + 'static) {
        self.0.callback.replace(Some(Rc::new(f)));
    }

    fn emit(&self, ev: SpiceDisplayEvent) {
        let cb = self.0.callback.borrow().clone();
        if let Some(cb) = cb {
            cb(ev);
        }
    }

    fn weak(&self) -> Weak<Inner> {
        Rc::downgrade(&self.0)
    }

    fn st(&self) -> std::cell::RefMut<'_, State> {
        self.0.state.borrow_mut()
    }

    fn inputs(&self) -> Option<glib::Object> {
        self.0.state.borrow().inputs.clone()
    }

    fn init_widget(&self) {
        let area = &self.0.area;
        let weak = self.weak();
        area.imp().provider.replace(Some(Box::new(move || {
            let inner = weak.upgrade()?;
            SpiceDisplay(inner).build_texture()
        })));
        let weak = self.weak();
        area.imp().overlay.replace(Some(Box::new(move || {
            let inner = weak.upgrade()?;
            SpiceDisplay(inner).cursor_overlay()
        })));
        let weak = self.weak();
        area.imp().on_resize.replace(Some(Box::new(move |w, h| {
            upgrade_or!(weak).size_allocate(w, h);
        })));

        let motion = gtk::EventControllerMotion::new();
        let weak = self.weak();
        motion.connect_enter(move |_, _, _| upgrade_or!(weak).enter_event());
        let weak = self.weak();
        motion.connect_leave(move |_| upgrade_or!(weak).leave_event());
        let weak = self.weak();
        motion.connect_motion(move |c, x, y| {
            upgrade_or!(weak).motion_event(x, y, c.current_event_state());
        });
        area.add_controller(motion);

        let click = gtk::GestureClick::new();
        click.set_button(0);
        let weak = self.weak();
        click.connect_pressed(move |g, _, x, y| {
            let state = g.current_event_state();
            upgrade_or!(weak).button_event(g.current_button(), true, x, y, state);
        });
        let weak = self.weak();
        click.connect_released(move |g, _, x, y| {
            let state = g.current_event_state();
            upgrade_or!(weak).button_event(g.current_button(), false, x, y, state);
        });
        area.add_controller(click);

        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        let weak = self.weak();
        scroll.connect_scroll(move |c, _, dy| {
            upgrade_or!(weak, glib::Propagation::Stop).scroll_event(dy, c.current_event_state());
            glib::Propagation::Stop
        });
        area.add_controller(scroll);

        let focus = gtk::EventControllerFocus::new();
        let weak = self.weak();
        focus.connect_enter(move |_| upgrade_or!(weak).focus_in_event());
        let weak = self.weak();
        focus.connect_leave(move |_| upgrade_or!(weak).focus_out_event());
        area.add_controller(focus);

        let weak = self.weak();
        let grabbed = Rc::new(move || {
            weak.upgrade()
                .is_some_and(|inner| inner.state.borrow().keyboard_grab_active)
        });
        let weak = self.weak();
        let on_key = Rc::new(move |press, keyval, keycode| {
            upgrade_or!(weak, false).key_event(press, keyval, keycode)
        });
        display::connect_keys(area, grabbed, on_key);
    }

    /// `spice_display_constructed`: follow the session's channels,
    /// the main channel first.
    fn constructed(&self) {
        let session = self.0.state.borrow().session.clone();
        let obj = session.object().clone();
        let weak = self.weak();
        let id = obj.connect_local("channel-new", false, move |a| {
            if let Ok(ch) = a[1].get::<glib::Object>() {
                upgrade_or!(weak, None).channel_new(&ch);
            }
            None
        });
        self.st().handlers.push((obj.clone(), id));
        let weak = self.weak();
        let id = obj.connect_local("channel-destroy", false, move |a| {
            if let Ok(ch) = a[1].get::<glib::Object>() {
                upgrade_or!(weak, None).channel_destroy(&ch);
            }
            None
        });
        self.st().handlers.push((obj.clone(), id));
        let weak = self.weak();
        let id = obj.connect_notify_local(Some("inhibit-keyboard-grab"), move |obj, _| {
            upgrade_or!(weak).session_inhibit_keyboard_grab_changed(obj);
        });
        self.st().handlers.push((obj, id));

        let channels = session.channels();
        let (main, rest): (Vec<_>, Vec<_>) = channels
            .into_iter()
            .partition(|c| spice::channel_kind(c) == ChannelKind::Main);
        for ch in main.iter().take(1).chain(rest.iter()) {
            self.channel_new(ch);
        }
    }

    /// Drops every signal handler; the session outlives us.
    pub fn destroy(&self) {
        if self.0.state.borrow().keyboard_have_focus {
            self.update_keyboard_focus(false);
        }
        let handlers = {
            let mut st = self.st();
            if let Some(id) = st.key_delayed_id.take() {
                id.remove();
            }
            st.main = None;
            st.display = None;
            st.inputs = None;
            st.cursor = None;
            st.canvas = None;
            st.seen.clear();
            std::mem::take(&mut st.handlers)
        };
        for (obj, id) in handlers {
            obj.disconnect(id);
        }
        self.0.callback.replace(None);
    }

    // Channels

    fn connect(
        &self,
        ch: &glib::Object,
        name: &str,
        after: bool,
        f: impl Fn(&SpiceDisplay, &[glib::Value]) + 'static,
    ) {
        let weak = self.weak();
        let id = ch.connect_local(name, after, move |a| {
            if let Some(inner) = weak.upgrade() {
                f(&SpiceDisplay(inner), a);
            }
            None
        });
        self.st().handlers.push((ch.clone(), id));
    }

    /// `channel_new`.
    fn channel_new(&self, ch: &glib::Object) {
        {
            let mut st = self.st();
            if st.seen.contains(ch) {
                return;
            }
            st.seen.push(ch.clone());
        }
        let id = ch.property::<i32>("channel-id");
        match spice::channel_kind(ch) {
            ChannelKind::Main => {
                self.st().main = Some(ch.clone());
                self.connect(ch, "main-mouse-update", false, |this, _| {
                    this.update_mouse_mode();
                });
                self.update_mouse_mode();
            }
            ChannelKind::Display => {
                if id != CHANNEL_ID {
                    return;
                }
                self.st().display = Some(ch.clone());
                self.connect(ch, "display-primary-create", false, |this, _| {
                    this.primary_create();
                });
                self.connect(ch, "display-primary-destroy", false, |this, _| {
                    this.primary_destroy();
                });
                self.connect(ch, "display-invalidate", false, |this, a| {
                    let get = |i: usize| a[i].get::<i32>().unwrap_or(0);
                    this.invalidate((get(1), get(2), get(3), get(4)));
                });
                self.connect(ch, "display-mark", true, |this, a| {
                    this.mark(a[1].get::<i32>().unwrap_or(0));
                });
                self.connect(ch, "notify::monitors", true, |this, _| {
                    this.update_monitor_area();
                });
                if let Some(primary) = spice::display_primary(ch) {
                    self.primary_create();
                    self.mark(primary.marked as i32);
                }
                spice::channel_connect(ch);
            }
            ChannelKind::Cursor => {
                if id != CHANNEL_ID {
                    return;
                }
                self.st().cursor = Some(ch.clone());
                self.connect(ch, "notify::cursor", false, |this, _| this.cursor_set());
                self.connect(ch, "cursor-move", false, |this, a| {
                    let get = |i: usize| a[i].get::<i32>().unwrap_or(0);
                    this.cursor_move(get(1), get(2));
                });
                self.connect(ch, "cursor-hide", false, |this, _| this.cursor_hide());
                self.connect(ch, "cursor-reset", false, |this, _| this.cursor_reset());
                spice::channel_connect(ch);
                if spice::cursor_shape(ch).is_some() {
                    self.cursor_set();
                }
            }
            ChannelKind::Inputs => {
                self.st().inputs = Some(ch.clone());
                spice::channel_connect(ch);
                self.connect(ch, "channel-event", false, |this, a| {
                    this.inputs_channel_event(spice::value_enum(&a[1]));
                });
            }
            _ => {}
        }
    }

    /// `channel_destroy`.
    fn channel_destroy(&self, ch: &glib::Object) {
        let id = ch.property::<i32>("channel-id");
        self.st().seen.retain(|c| c != ch);
        match spice::channel_kind(ch) {
            ChannelKind::Main => self.st().main = None,
            ChannelKind::Display if id == CHANNEL_ID => {
                self.primary_destroy();
                self.st().display = None;
            }
            ChannelKind::Cursor if id == CHANNEL_ID => self.st().cursor = None,
            ChannelKind::Inputs => self.st().inputs = None,
            _ => {}
        }
    }

    /// `inputs_channel_event`: no key press delay over a local socket.
    fn inputs_channel_event(&self, event: i32) {
        if event != spice::CHANNEL_OPENED {
            return;
        }
        let Some(inputs) = self.inputs() else { return };
        let unix = inputs
            .property::<Option<gio::Socket>>("socket")
            .is_some_and(|s| s.family() == gio::SocketFamily::Unix);
        self.set_keypress_delay(if unix { 0 } else { DEFAULT_KEYPRESS_DELAY });
        self.sync_keyboard_modifiers();
    }

    /// `spice_display_set_keypress_delay`.
    fn set_keypress_delay(&self, delay: u32) {
        let pending = {
            let st = self.0.state.borrow();
            st.keypress_delay != 0 && delay == 0
        };
        if pending {
            self.key_press_delayed();
        }
        self.st().keypress_delay = delay;
    }

    /// `spice_gtk_session_sync_keyboard_modifiers`: the guest's lock
    /// keys follow the client's.
    fn sync_keyboard_modifiers(&self) {
        let gtk_session = self.0.state.borrow().gtk_session.clone();
        gtk_session.sync_keyboard_modifiers();
    }

    /// `session_inhibit_keyboard_grab_changed`.
    fn session_inhibit_keyboard_grab_changed(&self, session: &glib::Object) {
        self.st().keyboard_grab_inhibit = session.property::<bool>("inhibit-keyboard-grab");
        // `update_keyboard_grab` and `update_mouse_grab`
        if self.0.state.borrow().keyboard_grab_inhibit {
            self.try_keyboard_ungrab();
            self.try_mouse_ungrab();
        } else {
            self.try_keyboard_grab();
            self.try_mouse_grab();
        }
    }

    /// `update_keyboard_focus`: while a display has the keyboard, USB
    /// devices plugged in are auto-redirected (when enabled).
    fn update_keyboard_focus(&self, state: bool) {
        let (gtk_session, inhibit) = {
            let mut st = self.st();
            st.keyboard_have_focus = state;
            (st.gtk_session.clone(), st.keyboard_grab_inhibit)
        };
        gtk_session.set_keyboard_has_focus(state);
        // The device manager inhibits the keyboard grab while it
        // redirects a device (which may show a policykit dialog), so
        // leave auto-redirection alone meanwhile.
        if inhibit {
            return;
        }
        gtk_session.request_auto_usbredir(state);
    }

    // Surface and monitor area

    /// `primary_create`.
    fn primary_create(&self) {
        let Some(display) = self.0.state.borrow().display.clone() else {
            return;
        };
        let canvas = spice::display_primary(&display).map(|p| Canvas {
            format: p.format,
            width: p.width,
            height: p.height,
            stride: p.stride,
            data: p.data,
        });
        self.st().canvas = canvas;
        self.update_monitor_area();
    }

    /// `primary_destroy`.
    fn primary_destroy(&self) {
        self.st().canvas = None;
        self.0.area.mark_dirty();
        self.set_monitor_ready(false);
    }

    /// `spice_display_widget_update_monitor_area`.
    fn update_monitor_area(&self) {
        let (display, canvas) = {
            let st = self.0.state.borrow();
            (st.display.clone(), st.canvas)
        };
        let Some(display) = display else { return };
        let monitors = spice::display_monitors(&display).unwrap_or_default();
        let found = monitors.iter().find(|m| m.0 == MONITOR_ID);
        match found {
            None => {
                self.set_monitor_ready(false);
                // SPICE_DISPLAY_CAP_MONITORS_CONFIG: wait for it, as a
                // spice-server that has it always sends one.
                if spice::channel_test_capability(&display, spice::DISPLAY_CAP_MONITORS_CONFIG) {
                    return;
                }
            }
            Some(c) if c.1 == 0 => {
                if monitors.len() == 1 {
                    self.update_area((0, 0, c.4, c.5));
                } else {
                    self.update_area((c.2, c.3, c.4, c.5));
                }
                return;
            }
            Some(_) => {
                eprintln!(
                    "FIXME: only support monitor config with primary surface 0, \
                     but given config surface is not 0"
                );
            }
        }
        let (w, h) = canvas.map(|c| (c.width, c.height)).unwrap_or((0, 0));
        self.update_area((0, 0, w, h));
        self.set_monitor_ready(true);
    }

    /// `update_area`: the part of the primary surface shown.
    fn update_area(&self, area: Rect) {
        let primary = {
            let st = self.0.state.borrow();
            st.canvas
                .map(|c| (0, 0, c.width, c.height))
                .unwrap_or_default()
        };
        match intersect(primary, area) {
            Some(a) => self.st().area = a,
            None => {
                self.st().area = (0, 0, 0, 0);
                self.set_monitor_ready(false);
                return;
            }
        }
        self.0.area.mark_dirty();
        self.update_size_request();
        self.set_monitor_ready(true);
    }

    fn set_monitor_ready(&self, ready: bool) {
        self.st().monitor_ready = ready;
        self.update_ready();
    }

    /// `update_ready`.
    fn update_ready(&self) {
        let (ready, changed, main) = {
            let mut st = self.st();
            let ready = st.monitor_ready && st.mark != 0;
            let main = st.main.clone().filter(|_| st.resize_guest_enable);
            let changed = st.ready != ready;
            st.ready = ready;
            (ready, changed, main)
        };
        if let Some(main) = main {
            spice::main_update_display_enabled(&main, DISPLAY_ID, ready);
        }
        if changed {
            self.0.area.mark_dirty();
            self.update_mouse_cursor();
        }
    }

    /// `invalidate`: new pixels in the shown area.
    fn invalidate(&self, rect: Rect) {
        let area = self.0.state.borrow().area;
        if intersect(rect, area).is_some() {
            self.0.area.mark_dirty();
        }
    }

    /// `mark`.
    fn mark(&self, mark: i32) {
        self.st().mark = mark;
        self.update_ready();
    }

    /// `draw_event` / `spice_cairo_draw_event`'s image: the shown area
    /// as xRGB, converted from the 16bpp formats like
    /// `do_color_convert`.
    fn frame(&self) -> Option<FrameCopy> {
        let st = self.0.state.borrow();
        let c = st.canvas?;
        let (ax, ay, aw, ah) = st.area;
        if !st.ready || aw <= 0 || ah <= 0 || c.data.is_null() {
            return None;
        }
        let stride = aw as usize * 4;
        let mut pixels = Vec::with_capacity(stride * ah as usize);
        // SAFETY: the area lies inside the primary surface, whose
        // pixels the channel keeps until `display-primary-destroy`.
        let src =
            unsafe { std::slice::from_raw_parts(c.data, c.stride as usize * c.height as usize) };
        for row in ay..ay + ah {
            let line = &src[row as usize * c.stride as usize..];
            match c.format {
                spice::SURFACE_FMT_16_555 | spice::SURFACE_FMT_16_565 => {
                    for col in ax..ax + aw {
                        let i = col as usize * 2;
                        let v = u16::from_ne_bytes([line[i], line[i + 1]]);
                        let px = convert_16bpp(c.format, v);
                        pixels.extend_from_slice(&px.to_ne_bytes());
                    }
                }
                _ => {
                    let start = ax as usize * 4;
                    pixels.extend_from_slice(&line[start..start + stride]);
                }
            }
        }
        Some(FrameCopy {
            width: aw,
            height: ah,
            stride,
            pixels,
        })
    }

    fn build_texture(&self) -> Option<gdk::Texture> {
        let f = self.frame()?;
        let format = if cfg!(target_endian = "little") {
            gdk::MemoryFormat::B8g8r8x8
        } else {
            gdk::MemoryFormat::X8r8g8b8
        };
        let bytes = glib::Bytes::from_owned(f.pixels);
        Some(gdk::MemoryTexture::new(f.width, f.height, format, &bytes, f.stride).upcast())
    }

    // Sizing

    /// `update_size_request`.
    fn update_size_request(&self) {
        let req = {
            let st = self.0.state.borrow();
            if st.resize_guest_enable || st.allow_scaling {
                (640, 480)
            } else {
                (st.area.2, st.area.3)
            }
        };
        self.0.area.set_request(req, req);
        self.recalc_geometry();
        self.update_mouse_cursor();
    }

    /// `recalc_geometry`: with `resize-guest`, ask the guest for the
    /// widget's size.
    fn recalc_geometry(&self) {
        let (main, area, ww, wh) = {
            let st = self.0.state.borrow();
            (
                st.main.clone().filter(|_| st.resize_guest_enable),
                st.area,
                st.ww,
                st.wh,
            )
        };
        let Some(main) = main else { return };
        let widget = &self.0.area;
        let scale_factor = widget.scale_factor();
        let monitor = widget
            .native()
            .and_then(|n| n.surface())
            .and_then(|s| widget.display().monitor_at_surface(&s));
        if let Some(monitor) = monitor {
            let geometry = monitor.geometry();
            if geometry.width() > 0 && geometry.height() > 0 {
                let width_mm = monitor.width_mm() * ww / geometry.width() * scale_factor;
                let height_mm = monitor.height_mm() * wh / geometry.height() * scale_factor;
                spice::main_update_display_mm(&main, DISPLAY_ID, width_mm, height_mm);
            }
        }
        spice::main_update_display(
            &main,
            DISPLAY_ID,
            area.0,
            area.1,
            ww * scale_factor,
            wh * scale_factor,
        );
    }

    /// `size_allocate`.
    fn size_allocate(&self, w: i32, h: i32) {
        {
            let mut st = self.st();
            if (st.ww, st.wh) == (w, h) {
                return;
            }
            st.ww = w;
            st.wh = h;
        }
        self.recalc_geometry();
        self.update_mouse_cursor();
    }

    /// `spice_display_get_scaling`: `(scale, x, y, w, h)` of the image
    /// in the widget.
    fn get_scaling(&self) -> (f64, i32, i32, i32, i32) {
        let st = self.0.state.borrow();
        let (fbw, fbh) = (st.area.2, st.area.3);
        let (ww, wh) = if self.0.area.is_realized() {
            (self.0.area.width(), self.0.area.height())
        } else {
            (fbw, fbh)
        };
        if !st.allow_scaling || fbw <= 0 || fbh <= 0 {
            let x = if ww > fbw { (ww - fbw) / 2 } else { 0 };
            let y = if wh > fbh { (wh - fbh) / 2 } else { 0 };
            return (1.0, x, y, fbw, fbh);
        }
        let s = (ww as f64 / fbw as f64).min(wh as f64 / fbh as f64);
        let w = (fbw as f64 * s + 0.5).floor() as i32;
        let h = (fbh as f64 * s + 0.5).floor() as i32;
        (s, (ww - w) / 2, (wh - h) / 2, w, h)
    }

    // Cursor

    /// `cursor_set`.
    fn cursor_set(&self) {
        let Some(cursor) = self.0.state.borrow().cursor.clone() else {
            return;
        };
        let Some(shape) = spice::cursor_shape(&cursor) else {
            return;
        };
        if shape.width <= 0 || shape.height <= 0 {
            return;
        }
        let bytes = glib::Bytes::from_owned(shape.data);
        let rowstride = shape.width * 4;
        let pixbuf = gtk::gdk_pixbuf::Pixbuf::from_bytes(
            &bytes,
            gtk::gdk_pixbuf::Colorspace::Rgb,
            true,
            8,
            shape.width,
            shape.height,
            rowstride,
        );
        let texture = gdk::MemoryTexture::new(
            shape.width,
            shape.height,
            gdk::MemoryFormat::R8g8b8a8,
            &bytes,
            rowstride as usize,
        )
        .upcast();
        self.st().shape = Some(Shape {
            texture,
            pixbuf,
            hotx: shape.hotx,
            hoty: shape.hoty,
        });
        self.update_mouse_cursor();
    }

    /// `update_mouse_cursor`: the guest's cursor, scaled with the image.
    fn update_mouse_cursor(&self) {
        let (pixbuf, hot) = {
            let st = self.0.state.borrow();
            let Some(shape) = &st.shape else { return };
            if !st.ready || !st.monitor_ready {
                return;
            }
            (shape.pixbuf.clone(), (shape.hotx, shape.hoty))
        };
        let scale = self.get_scaling().0.max(0.5);
        let (w, h) = (
            ((pixbuf.width() as f64 * scale) as i32).max(1),
            ((pixbuf.height() as f64 * scale) as i32).max(1),
        );
        let scaled = if (w, h) == (pixbuf.width(), pixbuf.height()) {
            Some(pixbuf)
        } else {
            pixbuf.scale_simple(w, h, gtk::gdk_pixbuf::InterpType::Bilinear)
        };
        let Some(scaled) = scaled else { return };
        #[allow(deprecated)]
        let texture = gdk::Texture::for_pixbuf(&scaled);
        let cursor = gdk::Cursor::from_texture(
            &texture,
            (hot.0 as f64 * scale) as i32,
            (hot.1 as f64 * scale) as i32,
            None,
        );
        {
            let mut st = self.st();
            if st.cursor_hidden {
                if st.mouse_mode == spice::MOUSE_MODE_SERVER {
                    // Kept hidden, shown again on `cursor-move`.
                    st.mouse_cursor = Some(cursor);
                    return;
                }
                st.cursor_hidden = false;
            }
            st.mouse_cursor = Some(cursor);
        }
        self.update_mouse_pointer();
        self.0.area.queue_draw();
    }

    /// `update_mouse_pointer`: the local cursor for the mouse mode.
    fn update_mouse_pointer(&self) {
        let cursor = {
            let st = self.0.state.borrow();
            match st.mouse_mode {
                spice::MOUSE_MODE_CLIENT if st.cursor_hidden => blank_cursor(),
                spice::MOUSE_MODE_CLIENT => st.mouse_cursor.clone(),
                spice::MOUSE_MODE_SERVER if st.mouse_grab_active => blank_cursor(),
                _ => None,
            }
        };
        self.0.area.set_cursor(cursor.as_ref());
    }

    /// `cursor_hide`.
    fn cursor_hide(&self) {
        {
            let mut st = self.st();
            if st.cursor_hidden {
                return;
            }
            st.cursor_hidden = true;
        }
        self.0.area.queue_draw();
        self.update_mouse_pointer();
    }

    /// `cursor_move`.
    fn cursor_move(&self, x: i32, y: i32) {
        let unhide = {
            let mut st = self.st();
            st.mouse_guest = (x, y);
            std::mem::replace(&mut st.cursor_hidden, false)
        };
        self.0.area.queue_draw();
        if unhide {
            self.update_mouse_pointer();
        }
    }

    /// `cursor_reset`.
    fn cursor_reset(&self) {
        self.0.area.set_cursor(None);
    }

    /// `spice_cairo_draw_event`'s cursor: drawn into the image while
    /// the pointer is grabbed in server mode.
    fn cursor_overlay(&self) -> Option<(gdk::Texture, f64, f64)> {
        let st = self.0.state.borrow();
        let shape = st.shape.as_ref()?;
        let (gx, gy) = st.mouse_guest;
        if st.mouse_mode != spice::MOUSE_MODE_SERVER
            || gx == -1
            || gy == -1
            || st.cursor_hidden
            || !st.mouse_grab_active
        {
            return None;
        }
        Some((
            shape.texture.clone(),
            (gx - shape.hotx - st.area.0) as f64,
            (gy - shape.hoty - st.area.1) as f64,
        ))
    }

    // Mouse

    /// `update_mouse_mode`.
    fn update_mouse_mode(&self) {
        let Some(main) = self.0.state.borrow().main.clone() else {
            return;
        };
        let mode = main.property::<i32>("mouse-mode");
        self.st().mouse_mode = mode;
        match mode {
            spice::MOUSE_MODE_CLIENT => self.try_mouse_ungrab(),
            spice::MOUSE_MODE_SERVER => {
                self.st().mouse_guest = (-1, -1);
                let buttons = self
                    .0
                    .area
                    .display()
                    .default_seat()
                    .and_then(|s| s.pointer())
                    .map(|p| p.modifier_state())
                    .is_some_and(|m| m.intersects(BUTTONS_MASK));
                if buttons {
                    self.try_mouse_grab();
                }
            }
            _ => {}
        }
        self.update_mouse_pointer();
    }

    /// `transform_input`: widget coordinates to guest coordinates,
    /// scaled so the last widget pixel reaches the last guest pixel.
    fn transform_input(&self, wx: f64, wy: f64) -> (i32, i32) {
        let (_, dx, dy, dw, _) = self.get_scaling();
        let aw = self.0.state.borrow().area.2;
        let is = if dw > 1 {
            (aw - 1) as f64 / (dw - 1) as f64
        } else {
            1.0
        };
        (
            ((wx - dx as f64) * is).floor() as i32,
            ((wy - dy as f64) * is).floor() as i32,
        )
    }

    fn inside_area(&self, x: i32, y: i32) -> bool {
        let (_, _, w, h) = self.0.state.borrow().area;
        x >= 0 && x < w && y >= 0 && y < h
    }

    /// `motion_event`.
    fn motion_event(&self, wx: f64, wy: f64, state: gdk::ModifierType) {
        let Some(inputs) = self.inputs() else { return };
        let regrab = {
            let mut st = self.st();
            st.seq_pressed = false;
            if st.keyboard_grab_released && st.keyboard_have_focus {
                st.keyboard_grab_released = false;
                true
            } else {
                false
            }
        };
        if regrab {
            self.release_keys();
            self.try_keyboard_grab();
        }
        let (x, y) = self.transform_input(wx, wy);
        let mask = button_mask_gdk_to_spice(state);
        let (mode, grabbed) = {
            let st = self.0.state.borrow();
            (st.mouse_mode, st.mouse_grab_active)
        };
        match mode {
            spice::MOUSE_MODE_CLIENT => {
                if self.inside_area(x, y) {
                    spice::inputs_position(&inputs, x, y, DISPLAY_ID, mask);
                }
            }
            spice::MOUSE_MODE_SERVER if grabbed => {
                let (dx, dy) = {
                    let mut st = self.st();
                    let (lx, ly) = st.mouse_last;
                    let dx = if lx != -1 { x - lx } else { 0 };
                    let dy = if ly != -1 { y - ly } else { 0 };
                    st.mouse_last = (x, y);
                    (dx, dy)
                };
                spice::inputs_motion(&inputs, dx, dy, mask);
            }
            _ => {}
        }
    }

    /// `button_event`.
    fn button_event(&self, button: u32, press: bool, wx: f64, wy: f64, state: gdk::ModifierType) {
        let (x, y) = self.transform_input(wx, wy);
        let mode = self.0.state.borrow().mouse_mode;
        if !self.inside_area(x, y) && mode == spice::MOUSE_MODE_CLIENT {
            // rule out clicks in outside region
            return;
        }
        self.0.area.grab_focus();
        if mode == spice::MOUSE_MODE_SERVER && !self.0.state.borrow().mouse_grab_active {
            self.try_mouse_grab();
            return;
        }
        let Some(inputs) = self.inputs() else { return };
        let mask = button_mask_gdk_to_spice(state);
        let spice_button = button_gdk_to_spice(button);
        if press {
            spice::inputs_button_press(&inputs, spice_button, mask);
            self.st().mouse_button_mask = mask | button_gdk_to_spice_mask(button);
        } else {
            spice::inputs_button_release(&inputs, spice_button, mask);
            self.st().mouse_button_mask = mask ^ button_gdk_to_spice_mask(button);
        }
    }

    /// `scroll_event`, smooth deltas folded into wheel clicks.
    fn scroll_event(&self, dy: f64, state: gdk::ModifierType) {
        let Some(inputs) = self.inputs() else { return };
        let mask = button_mask_gdk_to_spice(state);
        let mut clicks = vec![];
        {
            let mut st = self.st();
            st.scroll_delta_y += dy;
            while st.scroll_delta_y.abs() >= 1.0 {
                if st.scroll_delta_y < 0.0 {
                    clicks.push(spice::MOUSE_BUTTON_UP);
                    st.scroll_delta_y += 1.0;
                } else {
                    clicks.push(spice::MOUSE_BUTTON_DOWN);
                    st.scroll_delta_y -= 1.0;
                }
            }
        }
        for button in clicks {
            spice::inputs_button_press(&inputs, button, mask);
            spice::inputs_button_release(&inputs, button, mask);
        }
    }

    fn enter_event(&self) {
        self.st().mouse_have_pointer = true;
        self.0
            .state
            .borrow()
            .gtk_session
            .set_mouse_has_pointer(true);
        self.try_keyboard_grab();
    }

    fn leave_event(&self) {
        {
            let mut st = self.st();
            if st.mouse_grab_active {
                return;
            }
            st.mouse_have_pointer = false;
            st.gtk_session.set_mouse_has_pointer(false);
        }
        self.try_keyboard_ungrab();
    }

    fn focus_in_event(&self) {
        if self.0.state.borrow().keyboard_have_focus {
            return;
        }
        self.release_keys();
        self.sync_keyboard_modifiers();
        {
            let mut st = self.st();
            if st.keyboard_grab_released {
                st.activeseq.iter_mut().for_each(|a| *a = false);
            }
        }
        self.update_keyboard_focus(true);
        self.try_keyboard_grab();
    }

    fn focus_out_event(&self) {
        self.release_keys();
        self.update_keyboard_focus(false);
        self.try_keyboard_ungrab();
    }

    /// `try_mouse_grab` / `do_pointer_grab`.
    fn try_mouse_grab(&self) {
        if std::env::var_os("SPICE_NOGRAB").is_some() {
            return;
        }
        {
            let st = self.0.state.borrow();
            if !st.mouse_have_pointer
                || !st.keyboard_have_focus
                || st.mouse_mode != spice::MOUSE_MODE_SERVER
                || st.mouse_grab_active
            {
                return;
            }
        }
        self.try_keyboard_grab();
        {
            let mut st = self.st();
            st.mouse_grab_active = true;
            st.mouse_last = (-1, -1);
        }
        self.update_mouse_pointer();
        self.0.area.queue_draw();
        self.emit(SpiceDisplayEvent::MouseGrab(true));
    }

    /// `try_mouse_ungrab`.
    fn try_mouse_ungrab(&self) {
        {
            let mut st = self.st();
            if !st.mouse_grab_active {
                return;
            }
            st.mouse_grab_active = false;
        }
        self.update_mouse_pointer();
        self.0.area.queue_draw();
        self.emit(SpiceDisplayEvent::MouseGrab(false));
    }

    // Keyboard

    /// `try_keyboard_grab`.
    fn try_keyboard_grab(&self) {
        if std::env::var_os("SPICE_NOGRAB").is_some() {
            return;
        }
        {
            let mut st = self.st();
            if st.keyboard_grab_inhibit
                || st.keyboard_grab_active
                || !st.gtk_session.keyboard_has_focus()
                || !st.gtk_session.mouse_has_pointer()
                || st.keyboard_grab_released
            {
                return;
            }
            st.keyboard_grab_active = true;
        }
        self.emit(SpiceDisplayEvent::KeyboardGrab(true));
    }

    /// `try_keyboard_ungrab`.
    fn try_keyboard_ungrab(&self) {
        {
            let mut st = self.st();
            if !st.keyboard_grab_active {
                return;
            }
            st.keyboard_grab_active = false;
        }
        self.emit(SpiceDisplayEvent::KeyboardGrab(false));
    }

    /// `check_for_grab_key`: records keys of the `check` kind (press or
    /// release), firing once all were seen; the other kind resets.
    fn check_for_grab_key(&self, press: bool, keyval: u32, check_press: bool) -> bool {
        let mut st = self.st();
        let st = &mut *st;
        if st.grabseq.is_empty() {
            return false;
        }
        if press == check_press {
            for (k, a) in st.grabseq.iter().zip(st.activeseq.iter_mut()) {
                if *k == keyval {
                    *a = true;
                }
            }
            if st.activeseq.iter().any(|a| !a) {
                return false;
            }
            st.activeseq.iter_mut().for_each(|a| *a = false);
            true
        } else {
            st.activeseq.iter_mut().for_each(|a| *a = false);
            st.seq_pressed = false;
            false
        }
    }

    /// `key_event`.
    fn key_event(&self, press: bool, keyval: u32, keycode: u32) -> bool {
        let seq_pressed = self.0.state.borrow().seq_pressed;
        if !seq_pressed && self.check_for_grab_key(press, keyval, true) {
            let (mode, grabbed) = {
                let st = self.0.state.borrow();
                (st.mouse_mode, st.mouse_grab_active)
            };
            if mode == spice::MOUSE_MODE_SERVER {
                if grabbed {
                    self.try_mouse_ungrab();
                } else {
                    self.try_mouse_grab();
                }
            }
            self.st().seq_pressed = true;
        } else if seq_pressed && self.check_for_grab_key(press, keyval, false) {
            self.release_keys();
            let released = self.0.state.borrow().keyboard_grab_released;
            if !released {
                self.st().keyboard_grab_released = true;
                self.try_keyboard_ungrab();
            } else {
                self.st().keyboard_grab_released = false;
                self.try_keyboard_grab();
            }
            self.st().seq_pressed = false;
        }

        let Some(inputs) = self.inputs() else {
            return true;
        };
        if keyval == gdk::Key::Pause.into_glib() {
            // The 0x21d is a sort of Third-Ctrl while 0x45 is the
            // NumLock: the same scancodes as hardware.
            if press {
                spice::inputs_key_press(&inputs, 0x21d);
                spice::inputs_key_press(&inputs, 0x45);
            } else {
                spice::inputs_key_release(&inputs, 0x21d);
                spice::inputs_key_release(&inputs, 0x45);
            }
            return true;
        }
        let scancode = keymap::gdk_keycode_to_xtkbd(keycode);
        self.send_key(scancode, press, !is_modifier(keyval));
        true
    }

    /// `key_press_and_release`.
    fn key_press_and_release(&self) {
        let (scancode, id) = {
            let mut st = self.st();
            let s = std::mem::take(&mut st.key_delayed_scancode);
            (s, st.key_delayed_id.take())
        };
        if scancode == 0 {
            return;
        }
        if let Some(inputs) = self.inputs() {
            spice::inputs_key_press_and_release(&inputs, scancode);
        }
        if let Some(id) = id {
            id.remove();
        }
    }

    /// `key_press_delayed`.
    fn key_press_delayed(&self) {
        let (scancode, id) = {
            let mut st = self.st();
            if st.key_delayed_scancode == 0 {
                return;
            }
            let s = std::mem::take(&mut st.key_delayed_scancode);
            (s, st.key_delayed_id.take())
        };
        if let Some(inputs) = self.inputs() {
            spice::inputs_key_press(&inputs, scancode);
        }
        if let Some(id) = id {
            id.remove();
        }
    }

    /// `send_key`, tracking which keys are down. A non-modifier press
    /// waits `keypress_delay` so a quick tap goes out as one message.
    fn send_key(&self, scancode: u32, press: bool, press_delayed: bool) {
        if scancode == 0 {
            return;
        }
        let Some(inputs) = self.inputs() else { return };
        let (i, m) = ((scancode / 32) as usize, 1u32 << (scancode % 32));
        if i >= 16 {
            return;
        }
        if press {
            // ensure delayed key is pressed before any new input event
            self.key_press_delayed();
            let (delay, down) = {
                let st = self.0.state.borrow();
                (st.keypress_delay, st.key_state[i] & m != 0)
            };
            if press_delayed && delay != 0 && !down {
                let weak = self.weak();
                let id = glib::timeout_add_local_once(
                    std::time::Duration::from_millis(delay.into()),
                    move || {
                        let this = upgrade_or!(weak);
                        // The source is gone once this runs.
                        this.st().key_delayed_id = None;
                        this.key_press_delayed();
                    },
                );
                let mut st = self.st();
                st.key_delayed_id = Some(id);
                st.key_delayed_scancode = scancode;
            } else {
                spice::inputs_key_press(&inputs, scancode);
            }
            self.st().key_state[i] |= m;
        } else {
            let (down, delayed) = {
                let st = self.0.state.borrow();
                (st.key_state[i] & m != 0, st.key_delayed_scancode)
            };
            if !down {
                return;
            }
            if delayed == scancode {
                self.key_press_and_release();
            } else {
                // ensure delayed key is pressed before other key are released
                self.key_press_delayed();
                spice::inputs_key_release(&inputs, scancode);
            }
            self.st().key_state[i] &= !m;
        }
    }

    /// `release_keys`: key-ups for anything still down.
    fn release_keys(&self) {
        let state = self.0.state.borrow().key_state;
        for (i, word) in state.iter().enumerate() {
            for b in 0..32 {
                let scancode = (i * 32 + b) as u32;
                if word & (1 << b) != 0 && scancode != 0 {
                    self.send_key(scancode, false, false);
                }
            }
        }
    }

    // Public API mirroring spice_display_* and its properties

    pub fn set_scaling(&self, scaling: bool) {
        self.st().allow_scaling = scaling;
        self.0.area.set_scaling(scaling);
        // scaling_updated
        self.recalc_geometry();
        self.update_mouse_cursor();
        self.update_size_request();
    }

    pub fn get_scaling_enabled(&self) -> bool {
        self.0.state.borrow().allow_scaling
    }

    pub fn set_resize_guest(&self, val: bool) {
        self.st().resize_guest_enable = val;
        self.update_ready();
        self.update_size_request();
    }

    pub fn get_resize_guest(&self) -> bool {
        self.0.state.borrow().resize_guest_enable
    }

    /// `spice_display_set_grab_keys`.
    pub fn set_grab_keys(&self, keys: Vec<u32>) {
        let mut st = self.st();
        st.activeseq = vec![false; keys.len()];
        st.grabseq = keys;
    }

    /// `spice_grab_sequence_as_string`.
    pub fn get_grab_keys_string(&self) -> String {
        self.0
            .state
            .borrow()
            .grabseq
            .iter()
            .map(|k| display::keyval_name(*k))
            .collect::<Vec<_>>()
            .join("+")
    }

    /// `spice_display_send_keys(..., SPICE_DISPLAY_KEY_EVENT_CLICK)`.
    pub fn send_keys(&self, keyvals: &[u32]) {
        let scancodes: Vec<u32> = keyvals
            .iter()
            .map(|k| {
                display::keycode_from_keyval(&self.0.area, *k)
                    .map(keymap::gdk_keycode_to_xtkbd)
                    .unwrap_or(0)
            })
            .collect();
        for s in &scancodes {
            self.send_key(*s, true, false);
        }
        for s in scancodes.iter().rev() {
            self.send_key(*s, false, false);
        }
    }

    /// `spice_display_get_pixbuf`.
    pub fn get_framebuffer(&self) -> Option<FrameCopy> {
        self.frame()
    }

    /// `get_preferred_width/height()[1]`.
    pub fn preferred_size(&self) -> (i32, i32) {
        self.0.area.natural_request()
    }
}

/// `gdk_rectangle_intersect`.
fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let x = a.0.max(b.0);
    let y = a.1.max(b.1);
    let x2 = (a.0 + a.2).min(b.0 + b.2);
    let y2 = (a.1 + a.3).min(b.1 + b.3);
    (x2 > x && y2 > y).then_some((x, y, x2 - x, y2 - y))
}

/// `CONVERT_0555_TO_0888` / `CONVERT_0565_TO_0888`.
fn convert_16bpp(format: i32, s: u16) -> u32 {
    let s = u32::from(s);
    if format == spice::SURFACE_FMT_16_565 {
        (((s << 3) & 0xf8) | ((s >> 2) & 0x7))
            | (((s << 5) & 0xfc00) | ((s >> 1) & 0x300))
            | (((s << 8) & 0xf8_0000) | ((s << 3) & 0x7_0000))
    } else {
        (((s & 0x001f) << 3) | ((s & 0x001c) >> 2))
            | (((s & 0x03e0) << 6) | ((s & 0x0380) << 1))
            | (((s & 0x7c00) << 9) | ((s & 0x7000) << 4))
    }
}

/// `SPICE_GDK_BUTTONS_MASK`
const BUTTONS_MASK: gdk::ModifierType = gdk::ModifierType::BUTTON1_MASK
    .union(gdk::ModifierType::BUTTON2_MASK)
    .union(gdk::ModifierType::BUTTON3_MASK)
    .union(gdk::ModifierType::BUTTON4_MASK)
    .union(gdk::ModifierType::BUTTON5_MASK);

/// `button_gdk_to_spice`.
fn button_gdk_to_spice(button: u32) -> i32 {
    match button {
        1 => spice::MOUSE_BUTTON_LEFT,
        2 => spice::MOUSE_BUTTON_MIDDLE,
        3 => spice::MOUSE_BUTTON_RIGHT,
        4 => spice::MOUSE_BUTTON_UP,
        5 => spice::MOUSE_BUTTON_DOWN,
        8 => spice::MOUSE_BUTTON_SIDE,
        9 => spice::MOUSE_BUTTON_EXTRA,
        _ => 0,
    }
}

/// `button_gdk_to_spice_mask`.
fn button_gdk_to_spice_mask(button: u32) -> i32 {
    match button {
        1 => spice::MOUSE_BUTTON_MASK_LEFT,
        2 => spice::MOUSE_BUTTON_MASK_MIDDLE,
        3 => spice::MOUSE_BUTTON_MASK_RIGHT,
        8 => spice::MOUSE_BUTTON_MASK_SIDE,
        9 => spice::MOUSE_BUTTON_MASK_EXTRA,
        _ => 0,
    }
}

/// `button_mask_gdk_to_spice`.
fn button_mask_gdk_to_spice(state: gdk::ModifierType) -> i32 {
    let mut mask = 0;
    if state.contains(gdk::ModifierType::BUTTON1_MASK) {
        mask |= spice::MOUSE_BUTTON_MASK_LEFT;
    }
    if state.contains(gdk::ModifierType::BUTTON2_MASK) {
        mask |= spice::MOUSE_BUTTON_MASK_MIDDLE;
    }
    if state.contains(gdk::ModifierType::BUTTON3_MASK) {
        mask |= spice::MOUSE_BUTTON_MASK_RIGHT;
    }
    mask
}

/// `GdkEventKey.is_modifier`, which GTK4's key controller doesn't
/// pass on: the modifier and lock keysyms.
fn is_modifier(keyval: u32) -> bool {
    matches!(keyval, 0xffe1..=0xffee | 0xfe01..=0xfe13 | 0xff7e | 0xff7f)
}

/// `spice_display_get_blank_cursor`.
fn blank_cursor() -> Option<gdk::Cursor> {
    gdk::Cursor::from_name("none", None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_intersection() {
        assert_eq!(
            intersect((0, 0, 800, 600), (0, 0, 1024, 768)),
            Some((0, 0, 800, 600))
        );
        assert_eq!(
            intersect((0, 0, 800, 600), (100, 50, 200, 100)),
            Some((100, 50, 200, 100))
        );
        assert_eq!(intersect((0, 0, 800, 600), (800, 0, 10, 10)), None);
        assert_eq!(intersect((0, 0, 0, 0), (0, 0, 10, 10)), None);
    }

    #[test]
    fn color_conversion() {
        assert_eq!(
            convert_16bpp(spice::SURFACE_FMT_16_565, 0xffff),
            0x00ff_ffff
        );
        assert_eq!(
            convert_16bpp(spice::SURFACE_FMT_16_565, 0xf800),
            0x00ff_0000
        );
        assert_eq!(
            convert_16bpp(spice::SURFACE_FMT_16_555, 0x7fff),
            0x00ff_ffff
        );
        assert_eq!(
            convert_16bpp(spice::SURFACE_FMT_16_555, 0x001f),
            0x0000_00ff
        );
    }

    #[test]
    fn buttons_map() {
        assert_eq!(button_gdk_to_spice(3), spice::MOUSE_BUTTON_RIGHT);
        assert_eq!(button_gdk_to_spice(6), 0);
        assert_eq!(
            button_mask_gdk_to_spice(
                gdk::ModifierType::BUTTON1_MASK | gdk::ModifierType::BUTTON3_MASK
            ),
            spice::MOUSE_BUTTON_MASK_LEFT | spice::MOUSE_BUTTON_MASK_RIGHT
        );
    }
}
