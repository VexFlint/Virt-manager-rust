//! gtk-vnc's `VncDisplay` widget (`src/vncdisplay.c`) for GTK4: a
//! [`DisplayWidget`] painting a [`gvnc::Framebuffer`], feeding pointer
//! and keyboard input to a [`gvnc::Connection`].
//!
//! It follows `vncdisplay.c` closely: the same pixel format and
//! encoding negotiation, the same framebuffer update requests, the same
//! key tracking (releases for keys still down when focus goes, the
//! grab-key sequence), the same scaling/aspect/force-size rules and the
//! same delayed desktop resize. Differences come from GTK4 itself: it
//! has no pointer or keyboard grabs, so "keyboard grab" is the pointer
//! being over the focused display (the window's own shortcuts are
//! bypassed then, see [`display::connect_keys`]), and in relative
//! pointer mode motion deltas are sent without warping the pointer.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use gtk::gdk;
use gtk::glib;
use gtk::glib::translate::IntoGlib;
use gtk::prelude::*;

use crate::display::{self, render_region, DisplayWidget, FrameCopy};
use crate::gvnc::{self, Connection, Framebuffer};
use crate::keymap;

/// What `VncDisplay` emits, as one callback.
#[derive(Debug, Clone)]
pub enum DisplayEvent {
    /// `vnc-initialized`
    Initialized,
    /// `vnc-disconnected`
    Disconnected,
    /// `vnc-error`
    Error(String),
    /// `vnc-auth-credential`, the `gvnc::CREDENTIAL_*` wanted.
    AuthCredential(Vec<i32>),
    /// `vnc-auth-failure`
    AuthFailure(String),
    /// `vnc-desktop-resize`
    DesktopResize(i32, i32),
    PointerGrab,
    PointerUngrab,
    KeyboardGrab,
    KeyboardUngrab,
}

/// `vnc_display_set_preferred_pixel_format`'s encodings, in preference
/// order. TIGHT_JPEG5 is dropped as `allow_lossy` is never set.
const ENCODINGS: &[i32] = &[
    gvnc::ENCODING_TIGHT,
    gvnc::ENCODING_XVP,
    gvnc::ENCODING_EXT_KEY_EVENT,
    gvnc::ENCODING_LED_STATE,
    gvnc::ENCODING_EXTENDED_DESKTOP_RESIZE,
    gvnc::ENCODING_DESKTOP_RESIZE,
    gvnc::ENCODING_DESKTOP_NAME,
    gvnc::ENCODING_LAST_RECT,
    gvnc::ENCODING_WMVI,
    gvnc::ENCODING_AUDIO,
    gvnc::ENCODING_ALPHA_CURSOR,
    gvnc::ENCODING_RICH_CURSOR,
    gvnc::ENCODING_XCURSOR,
    gvnc::ENCODING_POINTER_CHANGE,
    gvnc::ENCODING_ZRLE,
    gvnc::ENCODING_HEXTILE,
    gvnc::ENCODING_RRE,
    gvnc::ENCODING_COPY_RECT,
    gvnc::ENCODING_RAW,
];

/// `preferable_auths`: VeNCrypt/TLS first as they can wrap the rest.
const PREFERABLE_AUTHS: &[u32] = &[
    gvnc::AUTH_VENCRYPT,
    gvnc::AUTH_TLS,
    gvnc::AUTH_SASL,
    gvnc::AUTH_MSLOGONII,
    gvnc::AUTH_MSLOGON,
    gvnc::AUTH_ARD,
    gvnc::AUTH_VNC,
    gvnc::AUTH_NONE,
];

/// `preferable_vencrypt_subauths` (cleartext PLAIN refused).
const PREFERABLE_VENCRYPT_SUBAUTHS: &[u32] = &[
    gvnc::AUTH_VENCRYPT_X509SASL,
    gvnc::AUTH_VENCRYPT_X509PLAIN,
    gvnc::AUTH_VENCRYPT_X509VNC,
    gvnc::AUTH_VENCRYPT_X509NONE,
    gvnc::AUTH_VENCRYPT_TLSSASL,
    gvnc::AUTH_VENCRYPT_TLSPLAIN,
    gvnc::AUTH_VENCRYPT_TLSVNC,
    gvnc::AUTH_VENCRYPT_TLSNONE,
];

/// `on_auth_choose_type`.
pub fn choose_auth_type(offered: &[i32]) -> Option<u32> {
    PREFERABLE_AUTHS
        .iter()
        .copied()
        .find(|p| offered.iter().any(|o| *o as u32 == *p))
}

/// `on_auth_choose_subtype`.
pub fn choose_auth_subtype(major: u32, offered: &[i32]) -> Option<u32> {
    let prefs = match major {
        gvnc::AUTH_TLS => PREFERABLE_AUTHS,
        gvnc::AUTH_VENCRYPT => PREFERABLE_VENCRYPT_SUBAUTHS,
        _ => return None,
    };
    prefs
        .iter()
        .copied()
        .filter(|p| *p != major)
        .find(|p| offered.iter().any(|o| *o as u32 == *p))
}

/// `check_for_grab_key`'s state.
#[derive(Default)]
struct GrabSeq {
    keys: Vec<u32>,
    active: Vec<bool>,
    pending: bool,
}

impl GrabSeq {
    fn set(&mut self, keys: Vec<u32>) {
        self.active = vec![false; keys.len()];
        self.keys = keys;
        self.pending = false;
    }

    /// Returns whether the sequence fired (on release).
    fn check(&mut self, press: bool, keyval: u32) -> bool {
        if self.keys.is_empty() {
            return false;
        }
        if !press {
            let fired = self.pending;
            self.active.iter_mut().for_each(|a| *a = false);
            self.pending = false;
            return fired;
        }
        let mut setone = false;
        for (k, a) in self.keys.iter().zip(self.active.iter_mut()) {
            if *k == keyval {
                *a = true;
                setone = true;
            }
        }
        if setone {
            if self.active.iter().all(|a| *a) {
                self.pending = true;
            }
        } else {
            self.active.iter_mut().for_each(|a| *a = false);
            self.pending = false;
        }
        false
    }
}

#[derive(Default)]
struct State {
    conn: Option<Connection>,
    handlers: Vec<glib::SignalHandlerId>,
    fb: Option<Framebuffer>,
    scaling: bool,
    keep_aspect: bool,
    force_size: bool,
    allow_resize: bool,
    absolute: bool,
    button_mask: u8,
    last_pos: Option<(f64, f64)>,
    /// `down_keyval` / `down_scancode`: (hardware keycode, keyval).
    down_keys: Vec<(u32, u32)>,
    grab_seq: GrabSeq,
    in_keyboard_grab: bool,
    in_pointer_grab: bool,
    pending_resize: Option<glib::SourceId>,
    last_resize_req: (i32, i32),
    remote_cursor: Option<gdk::Cursor>,
}

type Listener = Rc<dyn Fn(DisplayEvent)>;

struct Inner {
    area: DisplayWidget,
    state: RefCell<State>,
    callback: RefCell<Option<Listener>>,
}

#[derive(Clone)]
pub struct VncDisplay(Rc<Inner>);

impl VncDisplay {
    pub fn new() -> VncDisplay {
        gvnc::init_debug();
        let area = DisplayWidget::default();
        let this = VncDisplay(Rc::new(Inner {
            area,
            state: RefCell::new(State {
                keep_aspect: true,
                last_resize_req: (-1, -1),
                ..Default::default()
            }),
            callback: RefCell::default(),
        }));
        this.init_widget();
        this
    }

    pub fn widget(&self) -> &DisplayWidget {
        &self.0.area
    }

    /// The single listener for [`DisplayEvent`]s.
    pub fn connect_event(&self, f: impl Fn(DisplayEvent) + 'static) {
        self.0.callback.replace(Some(Rc::new(f)));
    }

    fn emit(&self, ev: DisplayEvent) {
        // Cloned out so the listener may drop or reconfigure us.
        let cb = self.0.callback.borrow().clone();
        if let Some(cb) = cb {
            cb(ev);
        }
    }

    fn conn(&self) -> Option<Connection> {
        self.0.state.borrow().conn.clone()
    }

    fn initialized_conn(&self) -> Option<Connection> {
        self.conn().filter(|c| c.is_initialized())
    }

    fn weak(&self) -> Weak<Inner> {
        Rc::downgrade(&self.0)
    }

    fn init_widget(&self) {
        let area = &self.0.area;
        let weak = self.weak();
        area.imp().provider.replace(Some(Box::new(move || {
            let inner = weak.upgrade()?;
            let st = inner.state.borrow();
            st.fb.as_ref().map(Framebuffer::texture)
        })));

        let weak = self.weak();
        area.imp().on_resize.replace(Some(Box::new(move |w, h| {
            if let Some(inner) = weak.upgrade() {
                VncDisplay(inner).configure(w, h);
            }
        })));

        let motion = gtk::EventControllerMotion::new();
        let weak = self.weak();
        motion.connect_enter(move |_, x, y| {
            if let Some(inner) = weak.upgrade() {
                VncDisplay(inner).enter(x, y);
            }
        });
        let weak = self.weak();
        motion.connect_leave(move |_| {
            if let Some(inner) = weak.upgrade() {
                VncDisplay(inner).leave();
            }
        });
        let weak = self.weak();
        motion.connect_motion(move |_, x, y| {
            if let Some(inner) = weak.upgrade() {
                VncDisplay(inner).motion(x, y);
            }
        });
        area.add_controller(motion);

        let click = gtk::GestureClick::new();
        click.set_button(0);
        let weak = self.weak();
        click.connect_pressed(move |g, _, x, y| {
            if let Some(inner) = weak.upgrade() {
                VncDisplay(inner).button(g.current_button(), true, x, y);
            }
        });
        let weak = self.weak();
        click.connect_released(move |g, _, x, y| {
            if let Some(inner) = weak.upgrade() {
                VncDisplay(inner).button(g.current_button(), false, x, y);
            }
        });
        area.add_controller(click);

        let scroll = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::BOTH_AXES | gtk::EventControllerScrollFlags::DISCRETE,
        );
        let weak = self.weak();
        scroll.connect_scroll(move |_, dx, dy| {
            if let Some(inner) = weak.upgrade() {
                VncDisplay(inner).scroll(dx, dy);
            }
            glib::Propagation::Stop
        });
        area.add_controller(scroll);

        let focus = gtk::EventControllerFocus::new();
        let weak = self.weak();
        focus.connect_leave(move |_| {
            if let Some(inner) = weak.upgrade() {
                let this = VncDisplay(inner);
                this.release_keys();
                this.keyboard_ungrab();
            }
        });
        area.add_controller(focus);

        let weak = self.weak();
        let grabbed = Rc::new(move || {
            weak.upgrade()
                .is_some_and(|inner| inner.state.borrow().in_keyboard_grab)
        });
        let weak = self.weak();
        let on_key = Rc::new(move |press, keyval, keycode| {
            weak.upgrade()
                .is_some_and(|inner| VncDisplay(inner).key(press, keyval, keycode))
        });
        display::connect_keys(area, grabbed, on_key);
    }

    // Public API mirroring vnc_display_*

    /// `vnc_display_open_host`.
    pub fn open_host(&self, host: &str, port: &str) -> bool {
        let conn = self.new_connection();
        conn.open_host(host, port)
    }

    /// `vnc_display_open_fd`.
    pub fn open_fd(&self, fd: i32) -> bool {
        let conn = self.new_connection();
        conn.open_fd(fd)
    }

    pub fn is_open(&self) -> bool {
        self.conn().is_some_and(|c| c.is_open())
    }

    /// `vnc_display_close`.
    pub fn close(&self) {
        let conn = {
            let mut st = self.0.state.borrow_mut();
            if let Some(id) = st.pending_resize.take() {
                id.remove();
            }
            st.conn.clone()
        };
        if let Some(conn) = conn {
            if conn.is_open() {
                conn.shutdown();
            }
        }
    }

    /// Drops the connection's signal handlers; the connection itself
    /// lives until its coroutine exits.
    pub fn destroy(&self) {
        self.close();
        let (conn, handlers) = {
            let mut st = self.0.state.borrow_mut();
            (st.conn.take(), std::mem::take(&mut st.handlers))
        };
        if let Some(conn) = conn {
            for id in handlers {
                conn.object().disconnect(id);
            }
        }
        self.0.callback.replace(None);
    }

    pub fn set_credential(&self, cred: i32, value: &str) -> bool {
        self.conn().is_some_and(|c| c.set_credential(cred, value))
    }

    pub fn set_scaling(&self, scaling: bool) {
        self.0.state.borrow_mut().scaling = scaling;
        self.0.area.set_scaling(scaling);
        self.update_size_request();
    }

    pub fn get_scaling(&self) -> bool {
        self.0.state.borrow().scaling
    }

    pub fn set_keep_aspect_ratio(&self, keep: bool) {
        self.0.state.borrow_mut().keep_aspect = keep;
        self.0.area.set_keep_aspect(keep);
    }

    pub fn set_force_size(&self, force: bool) {
        self.0.state.borrow_mut().force_size = force;
        self.update_size_request();
    }

    pub fn set_allow_resize(&self, allow: bool) {
        self.0.state.borrow_mut().allow_resize = allow;
        if allow {
            let (w, h) = (self.0.area.width(), self.0.area.height());
            self.configure(w, h);
        }
    }

    pub fn get_allow_resize(&self) -> bool {
        self.0.state.borrow().allow_resize
    }

    /// `vnc_display_set_grab_keys`.
    pub fn set_grab_keys(&self, keys: Vec<u32>) {
        self.0.state.borrow_mut().grab_seq.set(keys);
    }

    /// `vnc_grab_sequence_as_string`: "Control_L+Alt_L".
    pub fn get_grab_keys_string(&self) -> String {
        self.0
            .state
            .borrow()
            .grab_seq
            .keys
            .iter()
            .map(|k| display::keyval_name(*k))
            .collect::<Vec<_>>()
            .join("+")
    }

    /// `vnc_display_send_keys`: press all in order, release in reverse.
    pub fn send_keys(&self, keyvals: &[u32]) {
        let Some(conn) = self.conn().filter(|c| c.is_open()) else {
            return;
        };
        let scancodes: Vec<u16> = keyvals
            .iter()
            .map(|k| scancode_from_keyval(&self.0.area, *k))
            .collect();
        for (k, s) in keyvals.iter().zip(&scancodes) {
            conn.key_event(true, *k, *s);
        }
        for (k, s) in keyvals.iter().zip(&scancodes).rev() {
            conn.key_event(false, *k, *s);
        }
    }

    /// `vnc_display_get_pixbuf`: a copy of the framebuffer.
    pub fn get_framebuffer(&self) -> Option<FrameCopy> {
        self.0.state.borrow().fb.as_ref().map(|fb| FrameCopy {
            width: fb.width(),
            height: fb.height(),
            stride: fb.stride(),
            pixels: fb.pixels().to_vec(),
        })
    }

    // Connection setup

    fn new_connection(&self) -> Connection {
        self.destroy_connection_only();
        let conn = Connection::new();
        let mut handlers = vec![];
        let obj = conn.object();

        macro_rules! on {
            ($name:expr, |$this:ident, $args:ident| $body:block) => {{
                let weak = self.weak();
                handlers.push(obj.connect_local($name, false, move |$args| {
                    if let Some(inner) = weak.upgrade() {
                        let $this = VncDisplay(inner);
                        $body
                    }
                    None
                }));
            }};
        }

        on!("vnc-initialized", |this, _a| {
            this.on_initialized();
        });
        on!("vnc-framebuffer-update", |this, a| {
            let get = |i: usize| a[i].get::<i32>().unwrap_or(0);
            this.on_framebuffer_update(get(1), get(2), get(3), get(4));
        });
        on!("vnc-desktop-resize", |this, a| {
            let w = a[1].get::<i32>().unwrap_or(0);
            let h = a[2].get::<i32>().unwrap_or(0);
            this.on_desktop_resize(w, h);
        });
        on!("vnc-pixel-format-changed", |this, _a| {
            this.on_pixel_format_changed();
        });
        on!("vnc-pointer-mode-changed", |this, a| {
            this.on_pointer_mode_changed(a[1].get::<bool>().unwrap_or(false));
        });
        on!("vnc-cursor-changed", |this, a| {
            let cursor = a[1].get::<Option<glib::Object>>().ok().flatten();
            this.on_cursor_changed(cursor);
        });
        on!("vnc-auth-credential", |this, a| {
            this.emit(DisplayEvent::AuthCredential(gvnc::value_array_enums(&a[1])));
        });
        on!("vnc-auth-choose-type", |this, a| {
            let offered = gvnc::value_array_enums(&a[1]);
            if let Some(conn) = this.conn() {
                match choose_auth_type(&offered) {
                    Some(t) => {
                        conn.set_auth_type(t);
                    }
                    None => conn.shutdown(),
                }
            }
        });
        on!("vnc-auth-choose-subtype", |this, a| {
            let major = a[1].get::<u32>().unwrap_or(0);
            let offered = gvnc::value_array_enums(&a[2]);
            if let Some(conn) = this.conn() {
                match choose_auth_subtype(major, &offered) {
                    Some(t) => {
                        conn.set_auth_subtype(t);
                    }
                    None => conn.shutdown(),
                }
            }
        });
        on!("vnc-auth-failure", |this, a| {
            this.emit(DisplayEvent::AuthFailure(gvnc::value_str(&a[1])));
        });
        on!("vnc-error", |this, a| {
            this.emit(DisplayEvent::Error(gvnc::value_str(&a[1])));
        });
        on!("vnc-disconnected", |this, _a| {
            this.on_disconnected();
        });

        let mut st = self.0.state.borrow_mut();
        st.conn = Some(conn.clone());
        st.handlers = handlers;
        conn
    }

    fn destroy_connection_only(&self) {
        let (conn, handlers) = {
            let mut st = self.0.state.borrow_mut();
            (st.conn.take(), std::mem::take(&mut st.handlers))
        };
        if let Some(conn) = conn {
            for id in handlers {
                conn.object().disconnect(id);
            }
            if conn.is_open() {
                conn.shutdown();
            }
        }
    }

    /// `on_initialized`.
    fn on_initialized(&self) {
        let Some(conn) = self.conn() else { return };
        // VNC_DISPLAY_DEPTH_COLOR_DEFAULT: keep the server's format
        // (some servers only send colour maps when it's echoed back),
        // in our byte order.
        let mut fmt = conn.pixel_format();
        fmt.byte_order = gvnc::native_byte_order();
        if !conn.set_pixel_format(&fmt) {
            conn.shutdown();
            return;
        }
        let (w, h) = conn.size();
        self.framebuffer_init(&conn, w, h, false);
        if !conn.set_encodings(ENCODINGS) || !conn.framebuffer_update_request(false) {
            conn.shutdown();
            return;
        }
        self.0.state.borrow_mut().absolute = conn.abs_pointer();
        self.emit(DisplayEvent::Initialized);
    }

    /// `do_framebuffer_init`.
    fn framebuffer_init(&self, conn: &Connection, width: i32, height: i32, quiet: bool) {
        if !conn.is_initialized() {
            return;
        }
        let remote = conn.pixel_format();
        {
            let st = self.0.state.borrow();
            if let Some(fb) = &st.fb {
                if fb.width() == width && fb.height() == height && fb.remote() == remote {
                    return;
                }
            }
        }
        let Some(fb) = Framebuffer::new(width, height, &remote) else {
            return;
        };
        conn.set_framebuffer(&fb);
        self.0.state.borrow_mut().fb = Some(fb);
        self.update_size_request();
        self.0.area.mark_dirty();
        if !quiet {
            self.emit(DisplayEvent::DesktopResize(width, height));
        }
    }

    fn on_framebuffer_update(&self, x: i32, y: i32, w: i32, h: i32) {
        // The whole texture is rebuilt on the next frame; GTK 4.14 has
        // no partial texture updates.
        let _ = (x, y, w, h);
        self.0.area.mark_dirty();
        if let Some(conn) = self.conn() {
            conn.framebuffer_update_request(true);
        }
    }

    fn on_desktop_resize(&self, w: i32, h: i32) {
        {
            let mut st = self.0.state.borrow_mut();
            if let Some(id) = st.pending_resize.take() {
                id.remove();
                st.last_resize_req = (-1, -1);
            }
        }
        let Some(conn) = self.conn() else { return };
        self.framebuffer_init(&conn, w, h, false);
        conn.framebuffer_update_request(false);
    }

    fn on_pixel_format_changed(&self) {
        let Some(conn) = self.conn() else { return };
        let (w, h) = conn.size();
        self.framebuffer_init(&conn, w, h, true);
    }

    fn on_pointer_mode_changed(&self, absolute: bool) {
        let ungrab = {
            let mut st = self.0.state.borrow_mut();
            st.absolute = absolute;
            absolute && st.in_pointer_grab
        };
        if ungrab {
            self.pointer_ungrab();
        }
        self.sync_cursor();
    }

    fn on_cursor_changed(&self, cursor: Option<glib::Object>) {
        let gcursor = cursor.map(|c| gvnc::Cursor::from_object(&c)).and_then(|c| {
            if c.width <= 0 || c.height <= 0 {
                return None;
            }
            let bytes = glib::Bytes::from_owned(c.data);
            let texture = gdk::MemoryTexture::new(
                c.width,
                c.height,
                gdk::MemoryFormat::R8g8b8a8,
                &bytes,
                (c.width * 4) as usize,
            );
            Some(gdk::Cursor::from_texture(&texture, c.hotx, c.hoty, None))
        });
        self.0.state.borrow_mut().remote_cursor = gcursor;
        self.sync_cursor();
    }

    /// `do_pointer_show` / `do_pointer_hide`: the guest's cursor when it
    /// sent one, no local cursor otherwise.
    fn sync_cursor(&self) {
        let st = self.0.state.borrow();
        let cursor = match &st.remote_cursor {
            Some(c) => Some(c.clone()),
            None if st.conn.is_some() => gdk::Cursor::from_name("none", None),
            None => None,
        };
        drop(st);
        self.0.area.set_cursor(cursor.as_ref());
    }

    fn on_disconnected(&self) {
        {
            let mut st = self.0.state.borrow_mut();
            st.in_keyboard_grab = false;
            st.in_pointer_grab = false;
        }
        self.0.area.set_cursor(None);
        self.emit(DisplayEvent::Disconnected);
    }

    // Rendering and sizing

    /// `get_preferred_width/height`: the framebuffer size as natural
    /// size when forcing it, and as minimum too unless scaling.
    fn update_size_request(&self) {
        let st = self.0.state.borrow();
        let natural = match &st.fb {
            Some(fb) if st.force_size => (fb.width(), fb.height()),
            _ => (0, 0),
        };
        let min = if st.force_size && !st.scaling {
            natural
        } else {
            (0, 0)
        };
        self.0.area.set_request(min, natural);
    }

    /// `configure_event`: schedule a guest resize to the widget size.
    fn configure(&self, w: i32, h: i32) {
        if self.initialized_conn().is_none() {
            return;
        }
        let mut st = self.0.state.borrow_mut();
        if !st.allow_resize || w <= 0 || h <= 0 {
            return;
        }
        if let Some(fb) = &st.fb {
            if fb.width() == w && fb.height() == h {
                return;
            }
        }
        if st.last_resize_req == (w, h) {
            return;
        }
        st.last_resize_req = (w, h);
        if let Some(id) = st.pending_resize.take() {
            id.remove();
        }
        let weak = self.weak();
        st.pending_resize = Some(glib::timeout_add_local_once(
            std::time::Duration::from_millis(500),
            move || {
                if let Some(inner) = weak.upgrade() {
                    let this = VncDisplay(inner);
                    let req = {
                        let mut st = this.0.state.borrow_mut();
                        st.pending_resize = None;
                        st.last_resize_req
                    };
                    if let Some(conn) = this.initialized_conn() {
                        conn.set_size(req.0 as u32, req.1 as u32);
                    }
                }
            },
        ));
    }

    // Input

    /// Widget coordinates → framebuffer coordinates.
    fn to_fb(&self, x: f64, y: f64) -> Option<(u16, u16)> {
        let st = self.0.state.borrow();
        let fb = st.fb.as_ref()?;
        let area = &self.0.area;
        let (ox, oy, sx, sy) = render_region(
            (area.width() as f64, area.height() as f64),
            (fb.width() as f64, fb.height() as f64),
            st.scaling,
            st.keep_aspect,
        );
        let fx = ((x - ox) / sx).clamp(0.0, (fb.width() - 1).max(0) as f64);
        let fy = ((y - oy) / sy).clamp(0.0, (fb.height() - 1).max(0) as f64);
        Some((fx as u16, fy as u16))
    }

    fn send_pointer(&self, x: f64, y: f64) {
        let Some(conn) = self.initialized_conn() else {
            return;
        };
        let Some((fx, fy)) = self.to_fb(x, y) else {
            return;
        };
        let (absolute, mask, last) = {
            let mut st = self.0.state.borrow_mut();
            let last = st.last_pos.replace((x, y));
            (st.absolute, st.button_mask, last)
        };
        if absolute {
            conn.pointer_event(mask, fx, fy);
        } else {
            // Relative mode with POINTER_CHANGE: deltas around 0x7FFF.
            let (lx, ly) = last.unwrap_or((x, y));
            let dx = (x - lx).round() as i32 + 0x7FFF;
            let dy = (y - ly).round() as i32 + 0x7FFF;
            conn.pointer_event(mask, dx as u16, dy as u16);
        }
    }

    fn enter(&self, x: f64, y: f64) {
        self.0.state.borrow_mut().last_pos = Some((x, y));
        if self.initialized_conn().is_none() {
            return;
        }
        self.0.area.grab_focus();
        self.keyboard_grab();
        self.sync_cursor();
    }

    fn leave(&self) {
        if self.initialized_conn().is_none() {
            return;
        }
        self.keyboard_ungrab();
    }

    fn motion(&self, x: f64, y: f64) {
        self.send_pointer(x, y);
    }

    /// `button_event`.
    fn button(&self, button: u32, press: bool, x: f64, y: f64) {
        if self.initialized_conn().is_none() {
            return;
        }
        if press && !self.0.area.has_focus() {
            self.0.area.grab_focus();
            self.keyboard_grab();
        }
        {
            let st = self.0.state.borrow();
            if press && !st.absolute && !st.in_pointer_grab {
                drop(st);
                self.pointer_grab();
                return;
            }
        }
        let bit = match button {
            1..=3 => 1u8 << (button - 1),
            _ => return,
        };
        {
            let mut st = self.0.state.borrow_mut();
            if press {
                st.button_mask |= bit;
            } else {
                st.button_mask &= !bit;
            }
        }
        self.send_pointer(x, y);
    }

    /// `scroll_event`: wheel "buttons" 4-7, pressed then released.
    fn scroll(&self, dx: f64, dy: f64) {
        let Some(conn) = self.initialized_conn() else {
            return;
        };
        let (mask, pos) = {
            let st = self.0.state.borrow();
            (st.button_mask, st.last_pos)
        };
        let Some((x, y)) = pos else { return };
        let Some((fx, fy)) = self.to_fb(x, y) else {
            return;
        };
        let mut bits = vec![];
        if dy < 0.0 {
            bits.push(1u8 << 3);
        } else if dy > 0.0 {
            bits.push(1u8 << 4);
        }
        if dx < 0.0 {
            bits.push(1u8 << 5);
        } else if dx > 0.0 {
            bits.push(1u8 << 6);
        }
        let absolute = self.0.state.borrow().absolute;
        let (px, py) = if absolute { (fx, fy) } else { (0x7FFF, 0x7FFF) };
        for bit in bits {
            conn.pointer_event(mask | bit, px, py);
            conn.pointer_event(mask, px, py);
        }
    }

    /// `key_event`. Returns whether the key was consumed.
    fn key(&self, press: bool, keyval: u32, keycode: u32) -> bool {
        let Some(conn) = self.initialized_conn() else {
            return false;
        };
        let keyval = if keyval == gdk::Key::ISO_Left_Tab.into_glib() {
            gdk::Key::Tab.into_glib()
        } else {
            keyval
        };
        let scancode = keymap::gdk_keycode_to_rfb(keycode);
        let fired = {
            let mut st = self.0.state.borrow_mut();
            // Release handling always runs: GTK can merge repeats into
            // press+press+release, which servers dislike.
            if let Some(pos) = st.down_keys.iter().position(|(c, _)| *c == keycode) {
                let (_, down_keyval) = st.down_keys.remove(pos);
                conn.key_event(false, down_keyval, scancode);
            }
            if press && st.down_keys.len() < 16 {
                st.down_keys.push((keycode, keyval));
                conn.key_event(true, keyval, scancode);
            }
            st.grab_seq.check(press, keyval)
        };
        if fired {
            let (grabbed, absolute) = {
                let st = self.0.state.borrow();
                (st.in_pointer_grab, st.absolute)
            };
            if grabbed {
                self.pointer_ungrab();
            } else if !absolute {
                self.pointer_grab();
            }
        }
        true
    }

    /// `release_keys`: key-ups for anything still held.
    fn release_keys(&self) {
        let Some(conn) = self.initialized_conn() else {
            return;
        };
        let down = std::mem::take(&mut self.0.state.borrow_mut().down_keys);
        for (keycode, keyval) in down {
            conn.key_event(false, keyval, keymap::gdk_keycode_to_rfb(keycode));
        }
    }

    fn keyboard_grab(&self) {
        {
            let mut st = self.0.state.borrow_mut();
            if st.in_keyboard_grab {
                return;
            }
            st.in_keyboard_grab = true;
        }
        self.emit(DisplayEvent::KeyboardGrab);
    }

    fn keyboard_ungrab(&self) {
        {
            let mut st = self.0.state.borrow_mut();
            if !st.in_keyboard_grab {
                return;
            }
            st.in_keyboard_grab = false;
        }
        self.emit(DisplayEvent::KeyboardUngrab);
    }

    fn pointer_grab(&self) {
        self.0.state.borrow_mut().in_pointer_grab = true;
        self.emit(DisplayEvent::PointerGrab);
    }

    fn pointer_ungrab(&self) {
        self.0.state.borrow_mut().in_pointer_grab = false;
        self.emit(DisplayEvent::PointerUngrab);
    }
}

/// `get_scancode_from_keyval`.
fn scancode_from_keyval(widget: &impl IsA<gtk::Widget>, keyval: u32) -> u16 {
    display::keycode_from_keyval(widget, keyval)
        .map(keymap::gdk_keycode_to_rfb)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_type_preference() {
        assert_eq!(
            choose_auth_type(&[gvnc::AUTH_NONE as i32, gvnc::AUTH_VNC as i32]),
            Some(gvnc::AUTH_VNC)
        );
        assert_eq!(
            choose_auth_type(&[gvnc::AUTH_NONE as i32, gvnc::AUTH_VENCRYPT as i32]),
            Some(gvnc::AUTH_VENCRYPT)
        );
        assert_eq!(choose_auth_type(&[5]), None);
        assert_eq!(
            choose_auth_subtype(
                gvnc::AUTH_VENCRYPT,
                &[
                    gvnc::AUTH_VENCRYPT_TLSNONE as i32,
                    gvnc::AUTH_VENCRYPT_X509VNC as i32
                ]
            ),
            Some(gvnc::AUTH_VENCRYPT_X509VNC)
        );
        // Plain cleartext is refused.
        assert_eq!(choose_auth_subtype(gvnc::AUTH_VENCRYPT, &[256]), None);
        assert_eq!(
            choose_auth_subtype(gvnc::AUTH_TLS, &[gvnc::AUTH_TLS as i32, 1]),
            Some(gvnc::AUTH_NONE)
        );
        assert_eq!(choose_auth_subtype(gvnc::AUTH_VNC, &[1]), None);
    }

    #[test]
    fn grab_sequence_fires_on_release() {
        let mut seq = GrabSeq::default();
        seq.set(vec![65507, 65513]);
        assert!(!seq.check(true, 65507));
        assert!(!seq.check(true, 65513));
        assert!(seq.check(false, 65513));
        // Another key in between resets it.
        assert!(!seq.check(true, 65507));
        assert!(!seq.check(true, 97));
        assert!(!seq.check(true, 65513));
        assert!(!seq.check(false, 65513));
    }
}
