//! `vmmConnection._add_conn_events`: libvirt's domain event callbacks.
//! The events arrive through the GLib-backed loop in `libvirtglib`, and
//! each one is handed to [`VmmConnection::domain_event`] on the main
//! context, which re-reads the domain like upstream's
//! `recache_from_event_loop`.

use std::ffi::{c_char, c_int, c_uint, c_ulonglong, c_void, CStr};

use gtk::glib;
use virt::connect::Connect;
use virt::sys;

use crate::connection::VmmConnection;

/// Which callback fired: upstream's `_domain_lifecycle_event`,
/// `_domain_agent_lifecycle_event` and `_domain_xml_misc_event`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomainEvent {
    Lifecycle,
    AgentLifecycle,
    XmlMisc,
}

type Opaque = glib::SendWeakRef<VmmConnection>;

/// Common body of every callback: the domain's UUID, passed to the
/// connection on the main context (libvirt may dispatch from a worker
/// thread mid-call).
unsafe fn dispatch(dom: sys::virDomainPtr, opaque: *mut c_void, event: DomainEvent) {
    let weak = (*(opaque as *const Opaque)).clone();
    let mut buf = [0 as c_char; sys::VIR_UUID_STRING_BUFLEN as usize];
    if sys::virDomainGetUUIDString(dom, buf.as_mut_ptr()) != 0 {
        return;
    }
    let uuid = CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned();
    glib::MainContext::default().invoke(move || {
        if let Some(conn) = weak.upgrade() {
            conn.domain_event(&uuid, event);
        }
    });
}

unsafe extern "C" fn lifecycle_cb(
    _conn: sys::virConnectPtr,
    dom: sys::virDomainPtr,
    _event: c_int,
    _detail: c_int,
    opaque: *mut c_void,
) -> c_int {
    dispatch(dom, opaque, DomainEvent::Lifecycle);
    0
}

unsafe extern "C" fn agent_lifecycle_cb(
    _conn: sys::virConnectPtr,
    dom: sys::virDomainPtr,
    _state: c_int,
    _reason: c_int,
    opaque: *mut c_void,
) {
    dispatch(dom, opaque, DomainEvent::AgentLifecycle);
}

unsafe extern "C" fn balloon_change_cb(
    _conn: sys::virConnectPtr,
    dom: sys::virDomainPtr,
    _actual: c_ulonglong,
    opaque: *mut c_void,
) {
    dispatch(dom, opaque, DomainEvent::XmlMisc);
}

unsafe extern "C" fn tray_change_cb(
    _conn: sys::virConnectPtr,
    dom: sys::virDomainPtr,
    _dev_alias: *const c_char,
    _reason: c_int,
    opaque: *mut c_void,
) {
    dispatch(dom, opaque, DomainEvent::XmlMisc);
}

/// `DEVICE_ADDED` and `DEVICE_REMOVED` share a signature.
unsafe extern "C" fn device_cb(
    _conn: sys::virConnectPtr,
    dom: sys::virDomainPtr,
    _dev_alias: *const c_char,
    opaque: *mut c_void,
) {
    dispatch(dom, opaque, DomainEvent::XmlMisc);
}

unsafe extern "C" fn metadata_change_cb(
    _conn: sys::virConnectPtr,
    dom: sys::virDomainPtr,
    _type: c_int,
    _nsuri: *const c_char,
    opaque: *mut c_void,
) {
    dispatch(dom, opaque, DomainEvent::XmlMisc);
}

unsafe extern "C" fn free_cb(opaque: *mut c_void) {
    drop(Box::from_raw(opaque as *mut Opaque));
}

/// Blocking: registers the domain callbacks on `conn`, returning their
/// ids for [`deregister`]. Without the lifecycle event nothing else is
/// registered and the engine's tick does all the work, as upstream's
/// `using_domain_events = False`.
pub fn register(conn: &Connect, weak: &Opaque) -> Vec<c_int> {
    // `VIR_DOMAIN_EVENT_CALLBACK()`: libvirt calls each with the
    // signature its event id documents.
    macro_rules! generic {
        ($f:expr) => {
            // SAFETY: only the pointer's type changes; libvirt calls it
            // with the arguments of the event it was registered for.
            Some(unsafe {
                std::mem::transmute::<
                    *const (),
                    unsafe extern "C" fn(sys::virConnectPtr, sys::virDomainPtr, *mut c_void),
                >($f as *const ())
            })
        };
    }
    let reg = |eventid: c_uint, cb: sys::virConnectDomainEventGenericCallback| {
        let opaque = Box::into_raw(Box::new(weak.clone())) as *mut c_void;
        // SAFETY: `opaque` is owned by libvirt from here on and freed
        // by `free_cb`; on failure it was never taken.
        let id = unsafe {
            sys::virConnectDomainEventRegisterAny(
                conn.as_ptr(),
                std::ptr::null_mut(),
                eventid as c_int,
                cb,
                opaque,
                Some(free_cb),
            )
        };
        if id < 0 {
            unsafe { free_cb(opaque) };
            None
        } else {
            Some(id)
        }
    };

    let mut ids = Vec::new();
    let Some(id) = reg(sys::VIR_DOMAIN_EVENT_ID_LIFECYCLE, generic!(lifecycle_cb)) else {
        return ids;
    };
    ids.push(id);
    let xml_events: [(c_uint, sys::virConnectDomainEventGenericCallback); 6] = [
        (
            sys::VIR_DOMAIN_EVENT_ID_BALLOON_CHANGE,
            generic!(balloon_change_cb),
        ),
        (
            sys::VIR_DOMAIN_EVENT_ID_TRAY_CHANGE,
            generic!(tray_change_cb),
        ),
        (sys::VIR_DOMAIN_EVENT_ID_DEVICE_REMOVED, generic!(device_cb)),
        (sys::VIR_DOMAIN_EVENT_ID_DEVICE_ADDED, generic!(device_cb)),
        (
            sys::VIR_DOMAIN_EVENT_ID_AGENT_LIFECYCLE,
            generic!(agent_lifecycle_cb),
        ),
        (
            sys::VIR_DOMAIN_EVENT_ID_METADATA_CHANGE,
            generic!(metadata_change_cb),
        ),
    ];
    ids.extend(xml_events.into_iter().filter_map(|(id, cb)| reg(id, cb)));
    ids
}

/// Blocking: drops the callbacks [`register`] added. They hold a
/// reference on the connection, so this comes before closing it.
pub fn deregister(conn: &Connect, ids: &[c_int]) {
    for &id in ids {
        // SAFETY: `id` came from `register` on this connection.
        unsafe { sys::virConnectDomainEventDeregisterAny(conn.as_ptr(), id) };
    }
}
