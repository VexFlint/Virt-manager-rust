//! libvirt-glib's `gvir_event_register()` (`libvirt-glib-event.c`):
//! libvirt's event loop hooks implemented on the GLib main loop, so
//! stream callbacks (the serial console), keepalives and the like are
//! dispatched by GTK's main context. Upstream's `virt-manager` calls
//! `LibvirtGLib.event_register()` at startup; [`event_register`] is the
//! same, called from `main` before any connection opens.
//!
//! libvirt may call these hooks from any thread (our blocking calls
//! run on worker threads), so the bookkeeping sits behind a mutex and
//! sources are attached to the default main context explicitly. As in
//! libvirt-glib, a removed handle or timeout has its free callback run
//! from an idle source, never from inside the remove call.

use std::ffi::{c_int, c_void};
use std::sync::{Mutex, Once};
use std::time::Duration;

use gtk::glib;
use gtk::glib::ffi as glib_ffi;
use gtk::glib::translate::from_glib_full;
use virt::sys;

/// A libvirt `opaque` pointer, handed back to libvirt untouched.
#[derive(Clone, Copy)]
struct Opaque(*mut c_void);
// SAFETY: libvirt owns what it points to and expects it back on
// whichever thread the loop dispatches from.
unsafe impl Send for Opaque {}

impl Opaque {
    fn get(self) -> *mut c_void {
        self.0
    }
}

struct Handle {
    watch: c_int,
    fd: c_int,
    events: c_int,
    source: Option<glib::Source>,
    cb: sys::virEventHandleCallback,
    opaque: Opaque,
    ff: sys::virFreeCallback,
}

struct Timeout {
    timer: c_int,
    interval: c_int,
    source: Option<glib::Source>,
    cb: sys::virEventTimeoutCallback,
    opaque: Opaque,
    ff: sys::virFreeCallback,
}

struct State {
    next_watch: c_int,
    handles: Vec<Handle>,
    next_timer: c_int,
    timeouts: Vec<Timeout>,
}

static STATE: Mutex<State> = Mutex::new(State {
    next_watch: 1,
    handles: Vec::new(),
    next_timer: 1,
    timeouts: Vec::new(),
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn events_to_condition(events: c_int) -> glib_ffi::GIOCondition {
    let events = events as u32;
    let mut cond = 0;
    if events & sys::VIR_EVENT_HANDLE_READABLE != 0 {
        cond |= glib_ffi::G_IO_IN;
    }
    if events & sys::VIR_EVENT_HANDLE_WRITABLE != 0 {
        cond |= glib_ffi::G_IO_OUT;
    }
    cond
}

fn condition_to_events(cond: glib_ffi::GIOCondition) -> c_int {
    let mut events = 0;
    if cond & glib_ffi::G_IO_IN != 0 {
        events |= sys::VIR_EVENT_HANDLE_READABLE;
    }
    if cond & glib_ffi::G_IO_OUT != 0 {
        events |= sys::VIR_EVENT_HANDLE_WRITABLE;
    }
    if cond & glib_ffi::G_IO_HUP != 0 {
        events |= sys::VIR_EVENT_HANDLE_HANGUP;
    }
    if cond & glib_ffi::G_IO_ERR != 0 {
        events |= sys::VIR_EVENT_HANDLE_ERROR;
    }
    events as c_int
}

/// What a handle's watch carries to its dispatch.
struct WatchData {
    watch: c_int,
    fd: c_int,
    cb: sys::virEventHandleCallback,
    opaque: Opaque,
}

unsafe extern "C" fn watch_dispatch(
    _channel: *mut glib_ffi::GIOChannel,
    cond: glib_ffi::GIOCondition,
    data: glib_ffi::gpointer,
) -> glib_ffi::gboolean {
    let d = &*(data as *const WatchData);
    if let Some(cb) = d.cb {
        cb(d.watch, d.fd, condition_to_events(cond), d.opaque.0);
    }
    glib_ffi::GTRUE
}

unsafe extern "C" fn watch_data_free(data: glib_ffi::gpointer) {
    drop(Box::from_raw(data as *mut WatchData));
}

/// `g_io_add_watch` on the default context, as a `glib::Source`.
fn add_watch(h: &Handle) -> glib::Source {
    let data = Box::new(WatchData {
        watch: h.watch,
        fd: h.fd,
        cb: h.cb,
        opaque: h.opaque,
    });
    unsafe {
        let channel = glib_ffi::g_io_channel_unix_new(h.fd);
        let source = glib_ffi::g_io_create_watch(channel, events_to_condition(h.events));
        glib_ffi::g_io_channel_unref(channel);
        // SAFETY: an IO watch calls its callback as a GIOFunc.
        let func: glib_ffi::GSourceFunc = Some(std::mem::transmute::<
            unsafe extern "C" fn(
                *mut glib_ffi::GIOChannel,
                glib_ffi::GIOCondition,
                glib_ffi::gpointer,
            ) -> glib_ffi::gboolean,
            unsafe extern "C" fn(glib_ffi::gpointer) -> glib_ffi::gboolean,
        >(watch_dispatch));
        glib_ffi::g_source_set_callback(
            source,
            func,
            Box::into_raw(data) as glib_ffi::gpointer,
            Some(watch_data_free),
        );
        glib_ffi::g_source_attach(source, std::ptr::null_mut());
        from_glib_full(source)
    }
}

fn add_timeout_source(t: &Timeout) -> glib::Source {
    let (timer, cb, opaque) = (t.timer, t.cb, t.opaque);
    let source = glib::timeout_source_new(
        Duration::from_millis(t.interval as u64),
        None,
        glib::Priority::DEFAULT,
        move || {
            if let Some(cb) = cb {
                // SAFETY: libvirt's callback, with its own opaque.
                unsafe { cb(timer, opaque.get()) };
            }
            glib::ControlFlow::Continue
        },
    );
    source.attach(None);
    source
}

/// Runs a free callback later, from the main loop.
fn free_later(ff: sys::virFreeCallback, opaque: Opaque) {
    let Some(ff) = ff else { return };
    let source = glib::idle_source_new(None, glib::Priority::DEFAULT, move || {
        // SAFETY: libvirt's free callback, with its own opaque.
        unsafe { ff(opaque.get()) };
        glib::ControlFlow::Break
    });
    source.attach(None);
}

unsafe extern "C" fn add_handle(
    fd: c_int,
    events: c_int,
    cb: sys::virEventHandleCallback,
    opaque: *mut c_void,
    ff: sys::virFreeCallback,
) -> c_int {
    let mut st = state();
    let watch = st.next_watch;
    st.next_watch += 1;
    let mut h = Handle {
        watch,
        fd,
        events,
        source: None,
        cb,
        opaque: Opaque(opaque),
        ff,
    };
    if events != 0 {
        h.source = Some(add_watch(&h));
    }
    st.handles.push(h);
    watch
}

unsafe extern "C" fn update_handle(watch: c_int, events: c_int) {
    let mut st = state();
    let Some(h) = st.handles.iter_mut().find(|h| h.watch == watch) else {
        return;
    };
    if events != 0 {
        if h.events == events && h.source.is_some() {
            return;
        }
        if let Some(s) = h.source.take() {
            s.destroy();
        }
        h.events = events;
        h.source = Some(add_watch(h));
    } else {
        if let Some(s) = h.source.take() {
            s.destroy();
        }
        h.events = 0;
    }
}

unsafe extern "C" fn remove_handle(watch: c_int) -> c_int {
    let mut st = state();
    let Some(pos) = st.handles.iter().position(|h| h.watch == watch) else {
        return -1;
    };
    let h = st.handles.remove(pos);
    drop(st);
    if let Some(s) = h.source {
        s.destroy();
    }
    free_later(h.ff, h.opaque);
    0
}

unsafe extern "C" fn add_timeout(
    interval: c_int,
    cb: sys::virEventTimeoutCallback,
    opaque: *mut c_void,
    ff: sys::virFreeCallback,
) -> c_int {
    let mut st = state();
    let timer = st.next_timer;
    st.next_timer += 1;
    let mut t = Timeout {
        timer,
        interval,
        source: None,
        cb,
        opaque: Opaque(opaque),
        ff,
    };
    if interval >= 0 {
        t.source = Some(add_timeout_source(&t));
    }
    st.timeouts.push(t);
    timer
}

unsafe extern "C" fn update_timeout(timer: c_int, interval: c_int) {
    let mut st = state();
    let Some(t) = st.timeouts.iter_mut().find(|t| t.timer == timer) else {
        return;
    };
    if interval >= 0 {
        if let Some(s) = t.source.take() {
            s.destroy();
        }
        t.interval = interval;
        t.source = Some(add_timeout_source(t));
    } else {
        if let Some(s) = t.source.take() {
            s.destroy();
        }
        t.interval = -1;
    }
}

unsafe extern "C" fn remove_timeout(timer: c_int) -> c_int {
    let mut st = state();
    let Some(pos) = st.timeouts.iter().position(|t| t.timer == timer) else {
        return -1;
    };
    let t = st.timeouts.remove(pos);
    drop(st);
    if let Some(s) = t.source {
        s.destroy();
    }
    free_later(t.ff, t.opaque);
    0
}

/// `gvir_event_register`: once per process.
pub fn event_register() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        sys::virEventRegisterImpl(
            Some(add_handle),
            Some(update_handle),
            Some(remove_handle),
            Some(add_timeout),
            Some(update_timeout),
            Some(remove_timeout),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn condition_mapping() {
        let r = sys::VIR_EVENT_HANDLE_READABLE as c_int;
        let w = sys::VIR_EVENT_HANDLE_WRITABLE as c_int;
        assert_eq!(
            events_to_condition(r | w),
            glib_ffi::G_IO_IN | glib_ffi::G_IO_OUT
        );
        assert_eq!(
            condition_to_events(glib_ffi::G_IO_IN | glib_ffi::G_IO_HUP),
            r | sys::VIR_EVENT_HANDLE_HANGUP as c_int
        );
        assert_eq!(
            condition_to_events(glib_ffi::G_IO_ERR),
            sys::VIR_EVENT_HANDLE_ERROR as c_int
        );
    }
}
