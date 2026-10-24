//! Hand-written bindings for the parts of libspice-client-glib that the
//! SPICE viewer uses. spice-gtk's own widget library is GTK3-only, so
//! like gtk-vnc's we only take the protocol library (which needs no
//! GTK at all) and rebuild the widget on top of it in GTK4, in
//! [`crate::spicedisplay`].
//!
//! Sessions and channels are plain `glib::Object`s here: their
//! properties and signals go through the GObject API, so only the
//! methods need declaring.

use std::ffi::{c_int, c_uint, c_void, CStr};

use gtk::glib;
use gtk::glib::ffi::{gboolean, GError, GList, GType};
use gtk::glib::gobject_ffi;
use gtk::glib::translate::*;
use gtk::prelude::*;

/// `SpiceChannelEvent`
pub const CHANNEL_CLOSED: i32 = 11;
pub const CHANNEL_ERROR_AUTH: i32 = 23;
/// The `SPICE_CHANNEL_ERROR_*` values all sit from 20 up.
pub const CHANNEL_ERROR_FIRST: i32 = 20;
pub const CHANNEL_OPENED: i32 = 10;

/// `SpiceMouseMode`
pub const MOUSE_MODE_SERVER: i32 = 1;
pub const MOUSE_MODE_CLIENT: i32 = 2;

/// `SpiceMouseButton`
pub const MOUSE_BUTTON_LEFT: i32 = 1;
pub const MOUSE_BUTTON_MIDDLE: i32 = 2;
pub const MOUSE_BUTTON_RIGHT: i32 = 3;
pub const MOUSE_BUTTON_UP: i32 = 4;
pub const MOUSE_BUTTON_DOWN: i32 = 5;
pub const MOUSE_BUTTON_SIDE: i32 = 6;
pub const MOUSE_BUTTON_EXTRA: i32 = 7;

/// `SpiceMouseButtonMask`
pub const MOUSE_BUTTON_MASK_LEFT: i32 = 1 << 0;
pub const MOUSE_BUTTON_MASK_MIDDLE: i32 = 1 << 1;
pub const MOUSE_BUTTON_MASK_RIGHT: i32 = 1 << 2;
pub const MOUSE_BUTTON_MASK_SIDE: i32 = 1 << 3;
pub const MOUSE_BUTTON_MASK_EXTRA: i32 = 1 << 4;

/// `SPICE_DISPLAY_CAP_MONITORS_CONFIG`
pub const DISPLAY_CAP_MONITORS_CONFIG: u32 = 1;

/// `SpiceKeyboardModifierFlags`
pub const KEY_LOCK_SCROLL: u32 = 1 << 0;
pub const KEY_LOCK_NUM: u32 = 1 << 1;
pub const KEY_LOCK_CAPS: u32 = 1 << 2;

/// `SpiceSurfaceFmt`
pub const SURFACE_FMT_16_555: i32 = 16;
pub const SURFACE_FMT_16_565: i32 = 80;

#[repr(C)]
struct SpiceDisplayPrimary {
    format: c_int,
    width: c_int,
    height: c_int,
    stride: c_int,
    shmid: c_int,
    data: *mut u8,
    marked: gboolean,
}

#[repr(C)]
struct SpiceCursorShape {
    type_: c_int,
    width: u16,
    height: u16,
    hot_spot_x: u16,
    hot_spot_y: u16,
    data: *mut c_void,
}

#[repr(C)]
struct SpiceDisplayMonitorConfig {
    id: c_uint,
    surface_id: c_uint,
    x: c_uint,
    y: c_uint,
    width: c_uint,
    height: c_uint,
}

#[link(name = "spice-client-glib-2.0")]
extern "C" {
    fn spice_session_new() -> *mut gobject_ffi::GObject;
    fn spice_session_connect(session: *mut gobject_ffi::GObject) -> gboolean;
    fn spice_session_open_fd(session: *mut gobject_ffi::GObject, fd: c_int) -> gboolean;
    fn spice_session_disconnect(session: *mut gobject_ffi::GObject);
    fn spice_session_get_channels(session: *mut gobject_ffi::GObject) -> *mut GList;

    fn spice_channel_connect(channel: *mut gobject_ffi::GObject) -> gboolean;
    fn spice_channel_open_fd(channel: *mut gobject_ffi::GObject, fd: c_int) -> gboolean;
    fn spice_channel_test_capability(channel: *mut gobject_ffi::GObject, cap: u32) -> gboolean;
    fn spice_channel_get_error(channel: *mut gobject_ffi::GObject) -> *const GError;
    fn spice_channel_event_get_type() -> GType;

    fn spice_main_channel_get_type() -> GType;
    fn spice_display_channel_get_type() -> GType;
    fn spice_inputs_channel_get_type() -> GType;
    fn spice_cursor_channel_get_type() -> GType;
    fn spice_playback_channel_get_type() -> GType;
    fn spice_record_channel_get_type() -> GType;

    fn spice_main_channel_update_display(
        channel: *mut gobject_ffi::GObject,
        id: c_int,
        x: c_int,
        y: c_int,
        width: c_int,
        height: c_int,
        update: gboolean,
    );
    fn spice_main_channel_update_display_mm(
        channel: *mut gobject_ffi::GObject,
        id: c_int,
        width_mm: c_int,
        height_mm: c_int,
        update: gboolean,
    );
    fn spice_main_channel_update_display_enabled(
        channel: *mut gobject_ffi::GObject,
        id: c_int,
        enabled: gboolean,
        update: gboolean,
    );

    fn spice_display_channel_get_primary(
        channel: *mut gobject_ffi::GObject,
        surface_id: u32,
        primary: *mut SpiceDisplayPrimary,
    ) -> gboolean;

    fn spice_inputs_channel_motion(
        channel: *mut gobject_ffi::GObject,
        dx: c_int,
        dy: c_int,
        button_state: c_int,
    );
    fn spice_inputs_channel_position(
        channel: *mut gobject_ffi::GObject,
        x: c_int,
        y: c_int,
        display: c_int,
        button_state: c_int,
    );
    fn spice_inputs_channel_button_press(
        channel: *mut gobject_ffi::GObject,
        button: c_int,
        button_state: c_int,
    );
    fn spice_inputs_channel_button_release(
        channel: *mut gobject_ffi::GObject,
        button: c_int,
        button_state: c_int,
    );
    fn spice_inputs_channel_key_press(channel: *mut gobject_ffi::GObject, scancode: c_uint);
    fn spice_inputs_channel_key_release(channel: *mut gobject_ffi::GObject, scancode: c_uint);
    fn spice_inputs_channel_set_key_locks(channel: *mut gobject_ffi::GObject, locks: c_uint);
    fn spice_inputs_channel_key_press_and_release(
        channel: *mut gobject_ffi::GObject,
        scancode: c_uint,
    );

    fn spice_audio_get(
        session: *mut gobject_ffi::GObject,
        context: *mut glib::ffi::GMainContext,
    ) -> *mut gobject_ffi::GObject;
}

fn ptr(obj: &glib::Object) -> *mut gobject_ffi::GObject {
    obj.as_ptr()
}

/// `SpiceSession`
#[derive(Clone)]
pub struct Session(glib::Object);

impl Session {
    pub fn new() -> Session {
        Session(unsafe { from_glib_full(spice_session_new()) })
    }

    pub fn object(&self) -> &glib::Object {
        &self.0
    }

    pub fn connect(&self) -> bool {
        unsafe { spice_session_connect(ptr(&self.0)) != 0 }
    }

    pub fn open_fd(&self, fd: i32) -> bool {
        unsafe { spice_session_open_fd(ptr(&self.0), fd) != 0 }
    }

    pub fn disconnect(&self) {
        unsafe { spice_session_disconnect(ptr(&self.0)) }
    }

    pub fn channels(&self) -> Vec<glib::Object> {
        unsafe {
            let list = spice_session_get_channels(ptr(&self.0));
            FromGlibPtrContainer::from_glib_container(list)
        }
    }

    /// `spice_audio_get(session, NULL)`; the session keeps it.
    pub fn audio(&self) -> Option<glib::Object> {
        unsafe { from_glib_none(spice_audio_get(ptr(&self.0), std::ptr::null_mut())) }
    }
}

/// The kinds of `SpiceChannel` the viewer cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelKind {
    Main,
    Display,
    Inputs,
    Cursor,
    Playback,
    Record,
    Other,
}

pub fn channel_kind(channel: &glib::Object) -> ChannelKind {
    let t = channel.type_();
    let is = |f: unsafe extern "C" fn() -> GType| t.is_a(unsafe { from_glib(f()) });
    if is(spice_main_channel_get_type) {
        ChannelKind::Main
    } else if is(spice_display_channel_get_type) {
        ChannelKind::Display
    } else if is(spice_inputs_channel_get_type) {
        ChannelKind::Inputs
    } else if is(spice_cursor_channel_get_type) {
        ChannelKind::Cursor
    } else if is(spice_playback_channel_get_type) {
        ChannelKind::Playback
    } else if is(spice_record_channel_get_type) {
        ChannelKind::Record
    } else {
        ChannelKind::Other
    }
}

pub fn channel_connect(channel: &glib::Object) -> bool {
    unsafe { spice_channel_connect(ptr(channel)) != 0 }
}

pub fn channel_open_fd(channel: &glib::Object, fd: i32) -> bool {
    unsafe { spice_channel_open_fd(ptr(channel), fd) != 0 }
}

pub fn channel_test_capability(channel: &glib::Object, cap: u32) -> bool {
    unsafe { spice_channel_test_capability(ptr(channel), cap) != 0 }
}

/// `spice_channel_get_error()->message`
pub fn channel_error(channel: &glib::Object) -> Option<String> {
    unsafe {
        let err = spice_channel_get_error(ptr(channel));
        if err.is_null() || (*err).message.is_null() {
            return None;
        }
        Some(
            CStr::from_ptr((*err).message)
                .to_string_lossy()
                .into_owned(),
        )
    }
}

/// A `SpiceChannelEvent` value's nick, e.g. `error-connect`.
pub fn channel_event_nick(event: i32) -> String {
    let t: glib::Type = unsafe { from_glib(spice_channel_event_get_type()) };
    glib::EnumClass::with_type(t)
        .and_then(|c| c.value(event).map(|v| v.nick().to_string()))
        .unwrap_or_else(|| event.to_string())
}

/// An enum signal argument (`channel-event`'s `SpiceChannelEvent`).
pub fn value_enum(value: &glib::Value) -> i32 {
    unsafe { gobject_ffi::g_value_get_enum(value.to_glib_none().0) }
}

pub fn main_update_display(main: &glib::Object, id: i32, x: i32, y: i32, w: i32, h: i32) {
    unsafe { spice_main_channel_update_display(ptr(main), id, x, y, w, h, true.into_glib()) }
}

pub fn main_update_display_mm(main: &glib::Object, id: i32, width_mm: i32, height_mm: i32) {
    unsafe {
        spice_main_channel_update_display_mm(ptr(main), id, width_mm, height_mm, true.into_glib())
    }
}

pub fn main_update_display_enabled(main: &glib::Object, id: i32, enabled: bool) {
    unsafe {
        spice_main_channel_update_display_enabled(
            ptr(main),
            id,
            enabled.into_glib(),
            true.into_glib(),
        )
    }
}

/// `SpiceDisplayPrimary`, minus the pixels' pointer kept apart.
#[derive(Clone, Copy, Debug)]
pub struct Primary {
    pub format: i32,
    pub width: i32,
    pub height: i32,
    pub stride: i32,
    pub data: *const u8,
    pub marked: bool,
}

/// `spice_display_channel_get_primary(channel, 0)`. The pixels belong
/// to the channel, which only writes them from its coroutine on the
/// main loop, and stay valid until `display-primary-destroy`.
pub fn display_primary(display: &glib::Object) -> Option<Primary> {
    unsafe {
        let mut p = std::mem::zeroed::<SpiceDisplayPrimary>();
        if spice_display_channel_get_primary(ptr(display), 0, &mut p) == 0 || p.data.is_null() {
            return None;
        }
        Some(Primary {
            format: p.format,
            width: p.width,
            height: p.height,
            stride: p.stride,
            data: p.data,
            marked: p.marked != 0,
        })
    }
}

/// One `SpiceDisplayMonitorConfig` of the display channel's
/// `monitors` property: `(id, surface_id, x, y, width, height)`.
pub type MonitorConfig = (u32, u32, i32, i32, i32, i32);

/// The display channel's `monitors` (a `GArray`), `None` when unset.
pub fn display_monitors(display: &glib::Object) -> Option<Vec<MonitorConfig>> {
    let value = display.property_value("monitors");
    unsafe {
        let arr =
            gobject_ffi::g_value_get_boxed(value.to_glib_none().0) as *const glib::ffi::GArray;
        if arr.is_null() {
            return None;
        }
        let data = (*arr).data as *const SpiceDisplayMonitorConfig;
        Some(
            (0..(*arr).len as usize)
                .map(|i| {
                    let c = &*data.add(i);
                    (
                        c.id,
                        c.surface_id,
                        c.x as i32,
                        c.y as i32,
                        c.width as i32,
                        c.height as i32,
                    )
                })
                .collect(),
        )
    }
}

pub fn inputs_motion(inputs: &glib::Object, dx: i32, dy: i32, state: i32) {
    unsafe { spice_inputs_channel_motion(ptr(inputs), dx, dy, state) }
}

pub fn inputs_position(inputs: &glib::Object, x: i32, y: i32, display: i32, state: i32) {
    unsafe { spice_inputs_channel_position(ptr(inputs), x, y, display, state) }
}

pub fn inputs_button_press(inputs: &glib::Object, button: i32, state: i32) {
    unsafe { spice_inputs_channel_button_press(ptr(inputs), button, state) }
}

pub fn inputs_button_release(inputs: &glib::Object, button: i32, state: i32) {
    unsafe { spice_inputs_channel_button_release(ptr(inputs), button, state) }
}

pub fn inputs_key_press(inputs: &glib::Object, scancode: u32) {
    unsafe { spice_inputs_channel_key_press(ptr(inputs), scancode) }
}

pub fn inputs_key_release(inputs: &glib::Object, scancode: u32) {
    unsafe { spice_inputs_channel_key_release(ptr(inputs), scancode) }
}

pub fn inputs_key_press_and_release(inputs: &glib::Object, scancode: u32) {
    unsafe { spice_inputs_channel_key_press_and_release(ptr(inputs), scancode) }
}

pub fn inputs_set_key_locks(inputs: &glib::Object, locks: u32) {
    unsafe { spice_inputs_channel_set_key_locks(ptr(inputs), locks) }
}

/// The cursor channel's `cursor` property: RGBA pixels (spice-gtk
/// already swaps them from the wire's BGRA), size and hotspot.
pub struct CursorShape {
    pub data: Vec<u8>,
    pub width: i32,
    pub height: i32,
    pub hotx: i32,
    pub hoty: i32,
}

pub fn cursor_shape(cursor: &glib::Object) -> Option<CursorShape> {
    let value = cursor.property_value("cursor");
    unsafe {
        let shape =
            gobject_ffi::g_value_get_boxed(value.to_glib_none().0) as *const SpiceCursorShape;
        if shape.is_null() || (*shape).data.is_null() {
            return None;
        }
        let s = &*shape;
        let len = s.width as usize * s.height as usize * 4;
        Some(CursorShape {
            data: std::slice::from_raw_parts(s.data as *const u8, len).to_vec(),
            width: s.width as i32,
            height: s.height as i32,
            hotx: s.hot_spot_x as i32,
            hoty: s.hot_spot_y as i32,
        })
    }
}
