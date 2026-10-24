//! Hand-written bindings for the parts of libspice-client-glib that the
//! SPICE viewer uses. spice-gtk's own widget library is GTK3-only, so
//! like gtk-vnc's we only take the protocol library (which needs no
//! GTK at all) and rebuild the widget on top of it in GTK4, in
//! [`crate::spicedisplay`].
//!
//! Sessions and channels are plain `glib::Object`s here: their
//! properties and signals go through the GObject API, so only the
//! methods need declaring.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr};
use std::ptr::NonNull;

use gtk::gio;
use gtk::glib;
use gtk::glib::ffi::{gboolean, GError, GList, GPtrArray, GType};
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

/// `VD_AGENT_CLIPBOARD_SELECTION_*`
pub const CLIPBOARD_SELECTION_CLIPBOARD: u32 = 0;
pub const CLIPBOARD_SELECTION_PRIMARY: u32 = 1;
/// `VD_AGENT_CLIPBOARD_SELECTION_SECONDARY + 1`
pub const CLIPBOARD_LAST: usize = 3;

/// `VD_AGENT_CLIPBOARD_*` data types
pub const CLIPBOARD_NONE: u32 = 0;
pub const CLIPBOARD_UTF8_TEXT: u32 = 1;
pub const CLIPBOARD_IMAGE_PNG: u32 = 2;
pub const CLIPBOARD_IMAGE_BMP: u32 = 3;
pub const CLIPBOARD_IMAGE_TIFF: u32 = 4;
pub const CLIPBOARD_IMAGE_JPG: u32 = 5;
pub const CLIPBOARD_FILE_LIST: u32 = 6;

/// `VD_AGENT_CAP_*`
pub const AGENT_CAP_CLIPBOARD_BY_DEMAND: u32 = 5;
pub const AGENT_CAP_GUEST_LINEEND_CRLF: u32 = 9;
pub const AGENT_CAP_CLIPBOARD_NO_RELEASE_ON_REGRAB: u32 = 16;

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

    fn spice_session_get_read_only(session: *mut gobject_ffi::GObject) -> gboolean;

    fn spice_main_channel_clipboard_selection_grab(
        channel: *mut gobject_ffi::GObject,
        selection: c_uint,
        types: *mut u32,
        ntypes: c_int,
    );
    fn spice_main_channel_clipboard_selection_release(
        channel: *mut gobject_ffi::GObject,
        selection: c_uint,
    );
    fn spice_main_channel_clipboard_selection_notify(
        channel: *mut gobject_ffi::GObject,
        selection: c_uint,
        type_: u32,
        data: *const u8,
        size: usize,
    );
    fn spice_main_channel_clipboard_selection_request(
        channel: *mut gobject_ffi::GObject,
        selection: c_uint,
        type_: u32,
    );
    fn spice_main_channel_agent_test_capability(
        channel: *mut gobject_ffi::GObject,
        cap: u32,
    ) -> gboolean;

    fn spice_usb_device_get_type() -> GType;
    fn spice_usb_device_get_description(device: *mut c_void, format: *const c_char) -> *mut c_char;
    fn spice_usb_device_manager_get(
        session: *mut gobject_ffi::GObject,
        err: *mut *mut GError,
    ) -> *mut gobject_ffi::GObject;
    fn spice_usb_device_manager_get_devices(manager: *mut gobject_ffi::GObject) -> *mut GPtrArray;
    fn spice_usb_device_manager_is_device_connected(
        manager: *mut gobject_ffi::GObject,
        device: *mut c_void,
    ) -> gboolean;
    fn spice_usb_device_manager_connect_device_async(
        manager: *mut gobject_ffi::GObject,
        device: *mut c_void,
        cancellable: *mut gio::ffi::GCancellable,
        callback: gio::ffi::GAsyncReadyCallback,
        user_data: glib::ffi::gpointer,
    );
    fn spice_usb_device_manager_disconnect_device_async(
        manager: *mut gobject_ffi::GObject,
        device: *mut c_void,
        cancellable: *mut gio::ffi::GCancellable,
        callback: gio::ffi::GAsyncReadyCallback,
        user_data: glib::ffi::gpointer,
    );
    fn spice_usb_device_manager_connect_device_finish(
        manager: *mut gobject_ffi::GObject,
        res: *mut gio::ffi::GAsyncResult,
        err: *mut *mut GError,
    ) -> gboolean;
    fn spice_usb_device_manager_disconnect_device_finish(
        manager: *mut gobject_ffi::GObject,
        res: *mut gio::ffi::GAsyncResult,
        err: *mut *mut GError,
    ) -> gboolean;
    fn spice_usb_device_manager_can_redirect_device(
        manager: *mut gobject_ffi::GObject,
        device: *mut c_void,
        err: *mut *mut GError,
    ) -> gboolean;
    fn spice_usb_device_manager_is_redirecting(manager: *mut gobject_ffi::GObject) -> gboolean;
    fn spice_usb_device_manager_create_shared_cd_device(
        manager: *mut gobject_ffi::GObject,
        filename: *mut c_char,
        err: *mut *mut GError,
    ) -> gboolean;
    fn spice_usb_device_manager_is_device_shared_cd(
        manager: *mut gobject_ffi::GObject,
        device: *mut c_void,
    ) -> gboolean;

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

    /// Wraps a session object, e.g. one kept weakly.
    pub fn from_object(obj: glib::Object) -> Session {
        Session(obj)
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

    pub fn read_only(&self) -> bool {
        unsafe { spice_session_get_read_only(ptr(&self.0)) != 0 }
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

pub fn main_clipboard_selection_grab(main: &glib::Object, selection: u32, types: &[u32]) {
    let mut types = types.to_vec();
    unsafe {
        spice_main_channel_clipboard_selection_grab(
            ptr(main),
            selection,
            types.as_mut_ptr(),
            types.len() as c_int,
        )
    }
}

pub fn main_clipboard_selection_release(main: &glib::Object, selection: u32) {
    unsafe { spice_main_channel_clipboard_selection_release(ptr(main), selection) }
}

/// `data` of `None` tells the agent the request failed.
pub fn main_clipboard_selection_notify(
    main: &glib::Object,
    selection: u32,
    type_: u32,
    data: Option<&[u8]>,
) {
    let (p, len) = data.map_or((std::ptr::null(), 0), |d| (d.as_ptr(), d.len()));
    unsafe { spice_main_channel_clipboard_selection_notify(ptr(main), selection, type_, p, len) }
}

pub fn main_clipboard_selection_request(main: &glib::Object, selection: u32, type_: u32) {
    unsafe { spice_main_channel_clipboard_selection_request(ptr(main), selection, type_) }
}

pub fn main_agent_test_capability(main: &glib::Object, cap: u32) -> bool {
    unsafe { spice_main_channel_agent_test_capability(ptr(main), cap) != 0 }
}

/// A `G_TYPE_POINTER` signal argument.
pub fn value_pointer(value: &glib::Value) -> *const c_void {
    unsafe { gobject_ffi::g_value_get_pointer(value.to_glib_none().0) }
}

/// A `SpiceUsbDevice`: a refcounted boxed type, compared by identity
/// as spice-gtk's widget does.
pub struct UsbDevice(NonNull<c_void>);

fn usb_device_type() -> GType {
    unsafe { spice_usb_device_get_type() }
}

impl UsbDevice {
    /// Takes a new reference to `device`.
    unsafe fn from_ptr(device: *const c_void) -> Option<UsbDevice> {
        let copy = gobject_ffi::g_boxed_copy(usb_device_type(), device as glib::ffi::gconstpointer);
        NonNull::new(copy).map(UsbDevice)
    }

    /// A `SpiceUsbDevice` signal argument.
    pub fn from_value(value: &glib::Value) -> Option<UsbDevice> {
        unsafe { Self::from_ptr(gobject_ffi::g_value_get_boxed(value.to_glib_none().0)) }
    }

    fn as_ptr(&self) -> *mut c_void {
        self.0.as_ptr()
    }

    /// `spice_usb_device_get_description(device, format)`
    pub fn description(&self, format: Option<&str>) -> String {
        unsafe {
            from_glib_full(spice_usb_device_get_description(
                self.as_ptr(),
                format.to_glib_none().0,
            ))
        }
    }
}

impl Clone for UsbDevice {
    fn clone(&self) -> Self {
        unsafe { Self::from_ptr(self.as_ptr()) }.expect("g_boxed_copy")
    }
}

impl Drop for UsbDevice {
    fn drop(&mut self) {
        unsafe { gobject_ffi::g_boxed_free(usb_device_type(), self.as_ptr()) }
    }
}

impl PartialEq for UsbDevice {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

/// `SpiceUsbDeviceManager`, one per session.
#[derive(Clone)]
pub struct UsbDeviceManager(glib::Object);

/// What a `GAsyncReadyCallback` boxes up.
type UsbDone = Box<dyn FnOnce(Result<(), glib::Error>)>;

unsafe extern "C" fn usb_connect_done(
    source: *mut gobject_ffi::GObject,
    res: *mut gio::ffi::GAsyncResult,
    data: glib::ffi::gpointer,
) {
    let f = Box::from_raw(data as *mut UsbDone);
    let mut err = std::ptr::null_mut();
    spice_usb_device_manager_connect_device_finish(source, res, &mut err);
    f(if err.is_null() {
        Ok(())
    } else {
        Err(from_glib_full(err))
    });
}

unsafe extern "C" fn usb_disconnect_done(
    source: *mut gobject_ffi::GObject,
    res: *mut gio::ffi::GAsyncResult,
    data: glib::ffi::gpointer,
) {
    let f = Box::from_raw(data as *mut UsbDone);
    let mut err = std::ptr::null_mut();
    spice_usb_device_manager_disconnect_device_finish(source, res, &mut err);
    f(if err.is_null() {
        Ok(())
    } else {
        Err(from_glib_full(err))
    });
}

impl UsbDeviceManager {
    /// `spice_usb_device_manager_get`: fails when the library was
    /// built without USB redirection or libusb can't start.
    pub fn get(session: &Session) -> Result<UsbDeviceManager, glib::Error> {
        unsafe {
            let mut err = std::ptr::null_mut();
            let m = spice_usb_device_manager_get(ptr(&session.0), &mut err);
            if !err.is_null() {
                return Err(from_glib_full(err));
            }
            if m.is_null() {
                return Err(glib::Error::new(
                    glib::FileError::Failed,
                    "USB redirection is not available",
                ));
            }
            Ok(UsbDeviceManager(from_glib_none(m)))
        }
    }

    pub fn object(&self) -> &glib::Object {
        &self.0
    }

    pub fn devices(&self) -> Vec<UsbDevice> {
        unsafe {
            let arr = spice_usb_device_manager_get_devices(ptr(&self.0));
            if arr.is_null() {
                return vec![];
            }
            let pdata = (*arr).pdata;
            let devices = (0..(*arr).len as usize)
                .filter_map(|i| UsbDevice::from_ptr(*pdata.add(i)))
                .collect();
            glib::ffi::g_ptr_array_unref(arr);
            devices
        }
    }

    pub fn is_device_connected(&self, device: &UsbDevice) -> bool {
        unsafe { spice_usb_device_manager_is_device_connected(ptr(&self.0), device.as_ptr()) != 0 }
    }

    pub fn connect_device_async(
        &self,
        device: &UsbDevice,
        done: impl FnOnce(Result<(), glib::Error>) + 'static,
    ) {
        let data: Box<UsbDone> = Box::new(Box::new(done));
        unsafe {
            spice_usb_device_manager_connect_device_async(
                ptr(&self.0),
                device.as_ptr(),
                std::ptr::null_mut(),
                Some(usb_connect_done),
                Box::into_raw(data) as glib::ffi::gpointer,
            )
        }
    }

    pub fn disconnect_device_async(
        &self,
        device: &UsbDevice,
        done: impl FnOnce(Result<(), glib::Error>) + 'static,
    ) {
        let data: Box<UsbDone> = Box::new(Box::new(done));
        unsafe {
            spice_usb_device_manager_disconnect_device_async(
                ptr(&self.0),
                device.as_ptr(),
                std::ptr::null_mut(),
                Some(usb_disconnect_done),
                Box::into_raw(data) as glib::ffi::gpointer,
            )
        }
    }

    pub fn can_redirect_device(&self, device: &UsbDevice) -> Result<(), glib::Error> {
        unsafe {
            let mut err = std::ptr::null_mut();
            if spice_usb_device_manager_can_redirect_device(ptr(&self.0), device.as_ptr(), &mut err)
                != 0
            {
                return Ok(());
            }
            Err(if err.is_null() {
                glib::Error::new(glib::FileError::Failed, "unknown error")
            } else {
                from_glib_full(err)
            })
        }
    }

    pub fn is_redirecting(&self) -> bool {
        unsafe { spice_usb_device_manager_is_redirecting(ptr(&self.0)) != 0 }
    }

    /// `Ok(false)` stands for a failure that set no error.
    pub fn create_shared_cd_device(&self, filename: &str) -> Result<bool, glib::Error> {
        unsafe {
            let mut err = std::ptr::null_mut();
            let name: glib::GString = filename.into();
            let ok = spice_usb_device_manager_create_shared_cd_device(
                ptr(&self.0),
                name.as_ptr() as *mut c_char,
                &mut err,
            );
            if !err.is_null() {
                return Err(from_glib_full(err));
            }
            Ok(ok != 0)
        }
    }

    pub fn is_device_shared_cd(&self, device: &UsbDevice) -> bool {
        unsafe { spice_usb_device_manager_is_device_shared_cd(ptr(&self.0), device.as_ptr()) != 0 }
    }
}
