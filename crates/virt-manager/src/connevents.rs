//! `vmmConnection._add_conn_events`: libvirt's domain, network and
//! storage pool event callbacks. The events arrive through the
//! GLib-backed loop in `libvirtglib`, and each one is handed to the
//! connection on the main context: domain events to
//! [`VmmConnection::domain_event`], which re-reads the domain like
//! upstream's `recache_from_event_loop`, network and pool events to
//! [`VmmConnection::object_event`].

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

/// Which network or pool callback fired: upstream's
/// `_network_lifecycle_event`, `_storage_pool_lifecycle_event` and
/// `_storage_pool_refresh_event`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectEvent {
    NetworkLifecycle,
    PoolLifecycle,
    PoolRefresh,
}

/// The callback ids [`register`] got, per event family: each family
/// has its own deregister call.
#[derive(Debug, Default)]
pub struct EventIds {
    domain: Vec<c_int>,
    network: Vec<c_int>,
    pool: Vec<c_int>,
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

/// Common body of the network and pool callbacks: the object's name,
/// passed to the connection on the main context.
unsafe fn dispatch_object(name: *const c_char, opaque: *mut c_void, event: ObjectEvent) {
    if name.is_null() {
        return;
    }
    let weak = (*(opaque as *const Opaque)).clone();
    let name = CStr::from_ptr(name).to_string_lossy().into_owned();
    glib::MainContext::default().invoke(move || {
        if let Some(conn) = weak.upgrade() {
            conn.object_event(event, &name);
        }
    });
}

/// `_network_lifecycle_event`.
unsafe extern "C" fn network_lifecycle_cb(
    _conn: sys::virConnectPtr,
    net: sys::virNetworkPtr,
    _event: c_int,
    _detail: c_int,
    opaque: *mut c_void,
) {
    dispatch_object(
        sys::virNetworkGetName(net),
        opaque,
        ObjectEvent::NetworkLifecycle,
    );
}

/// `_storage_pool_lifecycle_event`.
unsafe extern "C" fn pool_lifecycle_cb(
    _conn: sys::virConnectPtr,
    pool: sys::virStoragePoolPtr,
    _event: c_int,
    _detail: c_int,
    opaque: *mut c_void,
) {
    dispatch_object(
        sys::virStoragePoolGetName(pool),
        opaque,
        ObjectEvent::PoolLifecycle,
    );
}

/// `_storage_pool_refresh_event`.
unsafe extern "C" fn pool_refresh_cb(
    _conn: sys::virConnectPtr,
    pool: sys::virStoragePoolPtr,
    opaque: *mut c_void,
) {
    dispatch_object(
        sys::virStoragePoolGetName(pool),
        opaque,
        ObjectEvent::PoolRefresh,
    );
}

/// Blocking: registers the domain, network and storage pool callbacks
/// on `conn`, returning their ids for [`deregister`].
pub fn register(conn: &Connect, weak: &Opaque) -> EventIds {
    EventIds {
        domain: register_domain(conn, weak),
        network: register_network(conn, weak),
        pool: register_pool(conn, weak),
    }
}

/// Hands libvirt a boxed weak reference for one callback, and takes it
/// back when registering failed: `f` gets the opaque pointer and the
/// free function, and returns the callback id or a negative error.
fn register_one(
    weak: &Opaque,
    f: impl FnOnce(*mut c_void, sys::virFreeCallback) -> c_int,
) -> Option<c_int> {
    let opaque = Box::into_raw(Box::new(weak.clone())) as *mut c_void;
    let id = f(opaque, Some(free_cb));
    if id < 0 {
        // SAFETY: on failure libvirt never took `opaque`.
        unsafe { free_cb(opaque) };
        None
    } else {
        Some(id)
    }
}

/// The network part of `_add_conn_events`: the lifecycle event, as
/// `using_network_events`. Without it the list is only fetched when the
/// Virtual Networks tab asks.
fn register_network(conn: &Connect, weak: &Opaque) -> Vec<c_int> {
    // SAFETY: libvirt calls the callback with the lifecycle event's
    // arguments, the signature `network_lifecycle_cb` has.
    let cb: sys::virConnectNetworkEventGenericCallback = Some(unsafe {
        std::mem::transmute::<
            *const (),
            unsafe extern "C" fn(sys::virConnectPtr, sys::virNetworkPtr, *mut c_void),
        >(network_lifecycle_cb as *const ())
    });
    let id = register_one(weak, |opaque, free| {
        // SAFETY: `opaque` is owned by libvirt from here on and freed
        // by `free`.
        unsafe {
            sys::virConnectNetworkEventRegisterAny(
                conn.as_ptr(),
                std::ptr::null_mut(),
                sys::VIR_NETWORK_EVENT_ID_LIFECYCLE as c_int,
                cb,
                opaque,
                free,
            )
        }
    });
    id.into_iter().collect()
}

/// The storage pool part of `_add_conn_events`: the lifecycle and
/// refresh events, as `using_storage_pool_events`.
fn register_pool(conn: &Connect, weak: &Opaque) -> Vec<c_int> {
    // SAFETY: as in `register_network`, for each event's own signature.
    let lifecycle: sys::virConnectStoragePoolEventGenericCallback = Some(unsafe {
        std::mem::transmute::<
            *const (),
            unsafe extern "C" fn(sys::virConnectPtr, sys::virStoragePoolPtr, *mut c_void),
        >(pool_lifecycle_cb as *const ())
    });
    let events: [(c_uint, sys::virConnectStoragePoolEventGenericCallback); 2] = [
        (sys::VIR_STORAGE_POOL_EVENT_ID_LIFECYCLE, lifecycle),
        (
            sys::VIR_STORAGE_POOL_EVENT_ID_REFRESH,
            Some(pool_refresh_cb),
        ),
    ];
    let mut ids = vec![];
    for (eventid, cb) in events {
        let id = register_one(weak, |opaque, free| {
            // SAFETY: as in `register_network`.
            unsafe {
                sys::virConnectStoragePoolEventRegisterAny(
                    conn.as_ptr(),
                    std::ptr::null_mut(),
                    eventid as c_int,
                    cb,
                    opaque,
                    free,
                )
            }
        });
        match id {
            Some(id) => ids.push(id),
            // Upstream gives up on pool events as a whole
            None => {
                deregister_pool(conn, &ids);
                return vec![];
            }
        }
    }
    ids
}

/// Blocking: registers the domain callbacks on `conn`, returning their
/// ids. Without the lifecycle event nothing else is registered and the
/// engine's tick does all the work, as upstream's
/// `using_domain_events = False`.
fn register_domain(conn: &Connect, weak: &Opaque) -> Vec<c_int> {
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
pub fn deregister(conn: &Connect, ids: &EventIds) {
    for &id in &ids.domain {
        // SAFETY: `id` came from `register` on this connection.
        unsafe { sys::virConnectDomainEventDeregisterAny(conn.as_ptr(), id) };
    }
    for &id in &ids.network {
        // SAFETY: as above.
        unsafe { sys::virConnectNetworkEventDeregisterAny(conn.as_ptr(), id) };
    }
    deregister_pool(conn, &ids.pool);
}

fn deregister_pool(conn: &Connect, ids: &[c_int]) {
    for &id in ids {
        // SAFETY: `id` came from `register_pool` on this connection.
        unsafe { sys::virConnectStoragePoolEventDeregisterAny(conn.as_ptr(), id) };
    }
}
