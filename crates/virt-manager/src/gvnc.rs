//! Hand-written bindings to libgvnc-1.0, gtk-vnc's GTK-free protocol
//! core (ticket 14: gtk-vnc's `VncDisplay` widget is GTK3-only, so the
//! GTK4 display widget in `vncdisplay.rs` drives a `VncConnection`
//! directly, the same way `VncDisplay` does internally).
//!
//! Only the calls `vncdisplay.c` makes are declared. `VncConnection`
//! is a plain GObject, so its signals are connected through
//! [`glib::Object`].

use std::ffi::{c_char, c_int, c_uint, CString};

use gtk::glib;
use gtk::glib::gobject_ffi;
use gtk::glib::prelude::*;
use gtk::glib::translate::*;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VncPixelFormat {
    pub bits_per_pixel: u8,
    pub depth: u8,
    pub byte_order: u16,
    pub true_color_flag: u8,
    pub red_max: u16,
    pub green_max: u16,
    pub blue_max: u16,
    pub red_shift: u8,
    pub green_shift: u8,
    pub blue_shift: u8,
}

const G_LITTLE_ENDIAN: u16 = 1234;
const G_BIG_ENDIAN: u16 = 4321;

impl VncPixelFormat {
    /// `vnc_cairo_framebuffer_new`'s local format: cairo's RGB24, i.e.
    /// native-endian 0x00RRGGBB.
    pub fn local() -> VncPixelFormat {
        VncPixelFormat {
            bits_per_pixel: 32,
            depth: 24,
            byte_order: native_byte_order(),
            true_color_flag: 1,
            red_max: 255,
            green_max: 255,
            blue_max: 255,
            red_shift: 16,
            green_shift: 8,
            blue_shift: 0,
        }
    }
}

pub fn native_byte_order() -> u16 {
    if cfg!(target_endian = "little") {
        G_LITTLE_ENDIAN
    } else {
        G_BIG_ENDIAN
    }
}

// VncConnectionEncoding
pub const ENCODING_RAW: i32 = 0;
pub const ENCODING_COPY_RECT: i32 = 1;
pub const ENCODING_RRE: i32 = 2;
pub const ENCODING_HEXTILE: i32 = 5;
pub const ENCODING_TIGHT: i32 = 7;
pub const ENCODING_ZRLE: i32 = 16;
pub const ENCODING_DESKTOP_RESIZE: i32 = -223;
pub const ENCODING_LAST_RECT: i32 = -224;
pub const ENCODING_WMVI: i32 = 0x574D5669;
pub const ENCODING_RICH_CURSOR: i32 = -239;
pub const ENCODING_XCURSOR: i32 = -240;
pub const ENCODING_POINTER_CHANGE: i32 = -257;
pub const ENCODING_EXT_KEY_EVENT: i32 = -258;
pub const ENCODING_AUDIO: i32 = -259;
pub const ENCODING_LED_STATE: i32 = -261;
pub const ENCODING_DESKTOP_NAME: i32 = -307;
pub const ENCODING_EXTENDED_DESKTOP_RESIZE: i32 = -308;
pub const ENCODING_XVP: i32 = -309;
pub const ENCODING_ALPHA_CURSOR: i32 = -314;

// VncConnectionAuth
pub const AUTH_NONE: u32 = 1;
pub const AUTH_VNC: u32 = 2;
pub const AUTH_TLS: u32 = 18;
pub const AUTH_VENCRYPT: u32 = 19;
pub const AUTH_SASL: u32 = 20;
pub const AUTH_ARD: u32 = 30;
pub const AUTH_MSLOGONII: u32 = 0x71;
pub const AUTH_MSLOGON: u32 = 0xfffffffa;

// VncConnectionAuthVencrypt
pub const AUTH_VENCRYPT_TLSNONE: u32 = 257;
pub const AUTH_VENCRYPT_TLSVNC: u32 = 258;
pub const AUTH_VENCRYPT_TLSPLAIN: u32 = 259;
pub const AUTH_VENCRYPT_X509NONE: u32 = 260;
pub const AUTH_VENCRYPT_X509VNC: u32 = 261;
pub const AUTH_VENCRYPT_X509PLAIN: u32 = 262;
pub const AUTH_VENCRYPT_X509SASL: u32 = 263;
pub const AUTH_VENCRYPT_TLSSASL: u32 = 264;

// VncConnectionCredential
pub const CREDENTIAL_PASSWORD: i32 = 0;
pub const CREDENTIAL_USERNAME: i32 = 1;
pub const CREDENTIAL_CLIENTNAME: i32 = 2;

#[link(name = "gvnc-1.0")]
extern "C" {
    fn vnc_connection_new() -> *mut gobject_ffi::GObject;
    fn vnc_connection_open_fd(conn: *mut gobject_ffi::GObject, fd: c_int) -> glib::ffi::gboolean;
    fn vnc_connection_open_host(
        conn: *mut gobject_ffi::GObject,
        host: *const c_char,
        port: *const c_char,
    ) -> glib::ffi::gboolean;
    fn vnc_connection_is_open(conn: *mut gobject_ffi::GObject) -> glib::ffi::gboolean;
    fn vnc_connection_is_initialized(conn: *mut gobject_ffi::GObject) -> glib::ffi::gboolean;
    fn vnc_connection_shutdown(conn: *mut gobject_ffi::GObject);
    fn vnc_connection_set_auth_type(
        conn: *mut gobject_ffi::GObject,
        type_: c_uint,
    ) -> glib::ffi::gboolean;
    fn vnc_connection_set_auth_subtype(
        conn: *mut gobject_ffi::GObject,
        type_: c_uint,
    ) -> glib::ffi::gboolean;
    fn vnc_connection_set_credential(
        conn: *mut gobject_ffi::GObject,
        type_: c_int,
        data: *const c_char,
    ) -> glib::ffi::gboolean;
    fn vnc_connection_pointer_event(
        conn: *mut gobject_ffi::GObject,
        button_mask: u8,
        x: u16,
        y: u16,
    ) -> glib::ffi::gboolean;
    fn vnc_connection_key_event(
        conn: *mut gobject_ffi::GObject,
        down: glib::ffi::gboolean,
        key: u32,
        scancode: u16,
    ) -> glib::ffi::gboolean;
    fn vnc_connection_framebuffer_update_request(
        conn: *mut gobject_ffi::GObject,
        incremental: glib::ffi::gboolean,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
    ) -> glib::ffi::gboolean;
    fn vnc_connection_set_encodings(
        conn: *mut gobject_ffi::GObject,
        n: c_int,
        encodings: *const i32,
    ) -> glib::ffi::gboolean;
    fn vnc_connection_set_pixel_format(
        conn: *mut gobject_ffi::GObject,
        fmt: *const VncPixelFormat,
    ) -> glib::ffi::gboolean;
    fn vnc_connection_get_pixel_format(conn: *mut gobject_ffi::GObject) -> *const VncPixelFormat;
    fn vnc_connection_set_framebuffer(
        conn: *mut gobject_ffi::GObject,
        fb: *mut gobject_ffi::GObject,
    ) -> glib::ffi::gboolean;
    fn vnc_connection_get_width(conn: *mut gobject_ffi::GObject) -> c_int;
    fn vnc_connection_get_height(conn: *mut gobject_ffi::GObject) -> c_int;
    fn vnc_connection_get_abs_pointer(conn: *mut gobject_ffi::GObject) -> glib::ffi::gboolean;
    fn vnc_connection_set_size(conn: *mut gobject_ffi::GObject, w: c_uint, h: c_uint) -> c_int;

    fn vnc_base_framebuffer_new(
        buffer: *mut u8,
        width: u16,
        height: u16,
        rowstride: c_int,
        local: *const VncPixelFormat,
        remote: *const VncPixelFormat,
    ) -> *mut gobject_ffi::GObject;

    fn vnc_cursor_get_data(cursor: *mut gobject_ffi::GObject) -> *const u8;
    fn vnc_cursor_get_hotx(cursor: *mut gobject_ffi::GObject) -> u16;
    fn vnc_cursor_get_hoty(cursor: *mut gobject_ffi::GObject) -> u16;
    fn vnc_cursor_get_width(cursor: *mut gobject_ffi::GObject) -> u16;
    fn vnc_cursor_get_height(cursor: *mut gobject_ffi::GObject) -> u16;

    fn vnc_util_set_debug(enabled: glib::ffi::gboolean);
}

/// `VncConnection`.
#[derive(Clone, Debug)]
pub struct Connection(glib::Object);

impl Connection {
    pub fn new() -> Connection {
        // SAFETY: vnc_connection_new returns a new full reference.
        Connection(unsafe { from_glib_full(vnc_connection_new()) })
    }

    pub fn object(&self) -> &glib::Object {
        &self.0
    }

    fn ptr(&self) -> *mut gobject_ffi::GObject {
        self.0.as_ptr()
    }

    pub fn open_fd(&self, fd: i32) -> bool {
        unsafe { vnc_connection_open_fd(self.ptr(), fd) != 0 }
    }

    pub fn open_host(&self, host: &str, port: &str) -> bool {
        let host = CString::new(host).unwrap_or_default();
        let port = CString::new(port).unwrap_or_default();
        unsafe { vnc_connection_open_host(self.ptr(), host.as_ptr(), port.as_ptr()) != 0 }
    }

    pub fn is_open(&self) -> bool {
        unsafe { vnc_connection_is_open(self.ptr()) != 0 }
    }

    pub fn is_initialized(&self) -> bool {
        unsafe { vnc_connection_is_initialized(self.ptr()) != 0 }
    }

    pub fn shutdown(&self) {
        unsafe { vnc_connection_shutdown(self.ptr()) }
    }

    pub fn set_auth_type(&self, t: u32) -> bool {
        unsafe { vnc_connection_set_auth_type(self.ptr(), t) != 0 }
    }

    pub fn set_auth_subtype(&self, t: u32) -> bool {
        unsafe { vnc_connection_set_auth_subtype(self.ptr(), t) != 0 }
    }

    pub fn set_credential(&self, cred: i32, data: &str) -> bool {
        let data = CString::new(data).unwrap_or_default();
        unsafe { vnc_connection_set_credential(self.ptr(), cred, data.as_ptr()) != 0 }
    }

    pub fn pointer_event(&self, mask: u8, x: u16, y: u16) -> bool {
        unsafe { vnc_connection_pointer_event(self.ptr(), mask, x, y) != 0 }
    }

    pub fn key_event(&self, down: bool, keyval: u32, scancode: u16) -> bool {
        unsafe { vnc_connection_key_event(self.ptr(), down.into_glib(), keyval, scancode) != 0 }
    }

    pub fn framebuffer_update_request(&self, incremental: bool) -> bool {
        let (w, h) = self.size();
        unsafe {
            vnc_connection_framebuffer_update_request(
                self.ptr(),
                incremental.into_glib(),
                0,
                0,
                w as u16,
                h as u16,
            ) != 0
        }
    }

    pub fn set_encodings(&self, encodings: &[i32]) -> bool {
        unsafe {
            vnc_connection_set_encodings(self.ptr(), encodings.len() as c_int, encodings.as_ptr())
                != 0
        }
    }

    pub fn set_pixel_format(&self, fmt: &VncPixelFormat) -> bool {
        unsafe { vnc_connection_set_pixel_format(self.ptr(), fmt) != 0 }
    }

    pub fn pixel_format(&self) -> VncPixelFormat {
        unsafe { *vnc_connection_get_pixel_format(self.ptr()) }
    }

    pub fn set_framebuffer(&self, fb: &Framebuffer) -> bool {
        unsafe { vnc_connection_set_framebuffer(self.ptr(), fb.obj.as_ptr()) != 0 }
    }

    pub fn size(&self) -> (i32, i32) {
        unsafe {
            (
                vnc_connection_get_width(self.ptr()),
                vnc_connection_get_height(self.ptr()),
            )
        }
    }

    pub fn abs_pointer(&self) -> bool {
        unsafe { vnc_connection_get_abs_pointer(self.ptr()) != 0 }
    }

    pub fn set_size(&self, w: u32, h: u32) -> i32 {
        unsafe { vnc_connection_set_size(self.ptr(), w, h) }
    }
}

/// A `VncBaseFramebuffer` over our own 32bpp xRGB buffer (what
/// `VncCairoFramebuffer` uses, cairo's RGB24). The buffer is attached
/// to the framebuffer object, so the pixels live as long as the
/// connection holds the framebuffer, even past our own teardown.
#[derive(Clone)]
pub struct Framebuffer {
    obj: glib::Object,
    data: *const u8,
    width: i32,
    height: i32,
    stride: usize,
    remote: VncPixelFormat,
}

impl Framebuffer {
    pub fn new(width: i32, height: i32, remote: &VncPixelFormat) -> Option<Framebuffer> {
        if width <= 0 || height <= 0 {
            return None;
        }
        let stride = width as usize * 4;
        let mut buf = vec![0u8; stride * height as usize].into_boxed_slice();
        let data = buf.as_mut_ptr();
        let local = VncPixelFormat::local();
        let obj: glib::Object = unsafe {
            from_glib_full(vnc_base_framebuffer_new(
                data,
                width as u16,
                height as u16,
                stride as c_int,
                &local,
                remote,
            ))
        };
        // SAFETY: the box moves into the object without reallocating,
        // so `data` stays valid for the object's lifetime.
        unsafe { obj.set_data("vmm-buffer", buf) };
        Some(Framebuffer {
            obj,
            data,
            width,
            height,
            stride,
            remote: *remote,
        })
    }

    /// The pixels as they are now. gvnc only writes them from its
    /// coroutine on the main loop, never while this borrow is held.
    pub fn pixels(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.data, self.stride * self.height as usize) }
    }

    pub fn stride(&self) -> usize {
        self.stride
    }

    /// A snapshot of the framebuffer as a GDK texture.
    pub fn texture(&self) -> gtk::gdk::Texture {
        let format = if cfg!(target_endian = "little") {
            gtk::gdk::MemoryFormat::B8g8r8x8
        } else {
            gtk::gdk::MemoryFormat::X8r8g8b8
        };
        let bytes = glib::Bytes::from(self.pixels());
        gtk::gdk::MemoryTexture::new(self.width, self.height, format, &bytes, self.stride).upcast()
    }

    pub fn remote(&self) -> VncPixelFormat {
        self.remote
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }
}

/// `VncCursor` read out of a `vnc-cursor-changed` argument.
pub struct Cursor {
    pub data: Vec<u8>,
    pub width: i32,
    pub height: i32,
    pub hotx: i32,
    pub hoty: i32,
}

impl Cursor {
    pub fn from_object(obj: &glib::Object) -> Cursor {
        let p = obj.as_ptr();
        unsafe {
            let width = vnc_cursor_get_width(p) as i32;
            let height = vnc_cursor_get_height(p) as i32;
            let len = (width * height * 4) as usize;
            let data_ptr = vnc_cursor_get_data(p);
            let data = if data_ptr.is_null() {
                vec![0; len]
            } else {
                std::slice::from_raw_parts(data_ptr, len).to_vec()
            };
            Cursor {
                data,
                width,
                height,
                hotx: vnc_cursor_get_hotx(p) as i32,
                hoty: vnc_cursor_get_hoty(p) as i32,
            }
        }
    }
}

/// The enum values in a `GValueArray` signal argument
/// (`vnc-auth-credential`, `vnc-auth-choose-type`, ...).
pub fn value_array_enums(value: &glib::Value) -> Vec<i32> {
    unsafe {
        let arr = gobject_ffi::g_value_get_boxed(value.to_glib_none().0)
            as *const gobject_ffi::GValueArray;
        if arr.is_null() {
            return vec![];
        }
        (0..(*arr).n_values as usize)
            .map(|i| gobject_ffi::g_value_get_enum((*arr).values.add(i)))
            .collect()
    }
}

/// `GTK_VNC_DEBUG`: libgvnc only reads the `--gtk-vnc-debug` option
/// itself, so mirror the env var gtk-vnc's display honours.
pub fn init_debug() {
    if std::env::var_os("GTK_VNC_DEBUG").is_some() {
        unsafe { vnc_util_set_debug(true.into_glib()) }
    }
}

/// A string signal argument; `NULL` reads as empty.
pub fn value_str(value: &glib::Value) -> String {
    value
        .get::<Option<String>>()
        .ok()
        .flatten()
        .unwrap_or_default()
}
