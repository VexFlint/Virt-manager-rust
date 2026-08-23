//! Wraps `bindgen`-bound `libosinfo` (ticket 04) — upstream's
//! `osdict.py`. Pure GObject C API, no GTK in the link chain;
//! `osinfo-db` is a separately-versioned, distro-installed data
//! package with its own update tooling, not something this crate
//! vendors.
//!
//! **`bindgen`, not `gir`** — the module doc comment used to say
//! "gir-bound" (an assumption that crept in from tickets 09/14's
//! unrelated libvirt-glib/console decisions, never actually checked
//! against ticket 04's own research). Reasserted ticket 04's original
//! conclusion after an empirical comparison on this machine: `gir`'s
//! idiomatic-wrapper generation leaves most of the functions that
//! actually matter here (`Loader::process_default_path`/
//! `process_system_path`, `Db::os_list()`, `Db::guess_os_from_media()`,
//! `Db::identify_media()`) as commented-out stubs — its heuristics
//! don't auto-translate `(void, GError**)`-shaped functions or
//! libosinfo's custom `OsinfoList`-family container types, which is
//! most of this API's actual surface. Plain `bindgen` gets every
//! function unstubbed with a `build.rs` + `pkg-config`, no `Gir.toml`
//! override-authoring detour. See ticket 04's file for the raw numbers.
//!
//! **GObject memory management, hand-rolled**: no `glib`/`gtk-rs`
//! idiomatic-wrapper crate sits between this module and the raw FFI
//! (that's exactly what ticket 04's bindgen-over-gir call means —
//! `glib`'s own `wrapper!`/`ObjectExt` machinery is a `gir`-ecosystem
//! convention this crate isn't pulling in for one library). Every
//! wrapper type here manages its own `g_object_ref`/`g_object_unref`
//! pair directly. Confirmed safe by inspecting the generated FFI
//! itself, not assumed: every `Osinfo*` struct's first field is its
//! immediate parent type (`OsinfoOs { parent_instance: OsinfoProduct,
//! .. }`, `#[repr(C)]`), the standard GObject inheritance-by-embedding
//! layout — so a `*mut OsinfoOs as *mut OsinfoProduct as *mut
//! OsinfoEntity as *mut GObject` cast chain is exactly what libosinfo's
//! own C macros (`OSINFO_ENTITY(os)` etc.) do, not an assumption this
//! binding introduces.
//!
//! **Transfer-ownership per call site, checked against the real `.gir`
//! annotations, not inferred from C naming conventions.** The `.gir`
//! file `bindgen` doesn't consume (`/usr/share/gir-1.0/Libosinfo-1.0.gir`)
//! is strictly better ground truth here than the headers bindgen reads
//! from, since C has no ownership-transfer syntax at all — a
//! `transfer-ownership="none"`/`"full"` attribute on each `<return-
//! value>` is metadata the `.gir` carries that bindgen's raw FFI
//! output simply can't. A first draft of this module inferred
//! ownership from each function's name instead (`_get_` implies
//! borrowed) and got one of the four calls below backwards
//! (`osinfo_db_get_os_list` is actually `transfer-ownership="full"`
//! despite the `get_` prefix) — caught on review before it shipped as
//! a use-after-free that a short-lived test process would never have
//! reproduced. Every `g_object_ref`/`_unref` call site below cites
//! which annotation it's honoring.
//!
//! Not `Send`/`Sync` — GObject reference counts aren't atomic by
//! default and libosinfo doesn't document its objects as thread-safe,
//! so these types stay single-threaded until a real cross-thread
//! caller (this crate has none yet) forces the question.

mod ffi;

use std::ffi::{CStr, CString};
use std::fmt;
use std::ptr;

/// Bridges the one failure mode a libosinfo call can hit — a `GError`
/// out-param, GLib's own error-reporting convention. Same small
/// bridging-enum shape `capabilities::domcaps`'s `CapabilitiesError`
/// already established for a two/three-way version of the same idea;
/// `libosinfo` only has the one failure mode so far, but the enum
/// shape (over a bare `String`) leaves room for a second without a
/// breaking API change, matching that precedent's own reasoning.
#[derive(Debug)]
pub enum OsInfoError {
    Glib(String),
}

impl fmt::Display for OsInfoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OsInfoError::Glib(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for OsInfoError {}

/// Extracts and frees a `GError` produced via the `(.., GError**)`
/// out-param convention libosinfo (and GLib generally) uses — every
/// caller passes `&mut error` and checks this right after the call,
/// matching upstream's own `try/except GLib.Error` pattern at each
/// call site rather than a single centralized handler (libosinfo's own
/// C API has no such handler to centralize into).
///
/// # Safety
/// `err` must be either null or a valid, owned `*mut GError` (i.e. one
/// this function's caller has exclusive ownership of and hasn't freed
/// already) — every call site here passes the out-param a libosinfo
/// FFI call just populated, matching that contract.
unsafe fn take_glib_error(err: *mut glib_sys::GError) -> Option<OsInfoError> {
    if err.is_null() {
        return None;
    }
    let c_msg = (*err).message;
    let msg = if c_msg.is_null() {
        "libosinfo reported an error with no message".to_string()
    } else {
        CStr::from_ptr(c_msg).to_string_lossy().into_owned()
    };
    glib_sys::g_error_free(err);
    Some(OsInfoError::Glib(msg))
}

/// # Safety
/// `ptr` must be either null or a valid pointer to a NUL-terminated C
/// string that outlives this call — every call site here reads a
/// `*const gchar` a libosinfo getter just returned, matching that
/// contract (none of them are used past the point the owning object
/// could have freed them).
unsafe fn c_str_to_string(ptr: *const std::os::raw::c_char) -> String {
    if ptr.is_null() {
        String::new()
    } else {
        CStr::from_ptr(ptr).to_string_lossy().into_owned()
    }
}

/// The loaded OS database — upstream's `osdict.py`'s module-level
/// `OSDB` singleton. Construction runs libosinfo's real init sequence
/// (ticket 04's "tiny init sequence" finding, `osinfo_loader_new()` →
/// `osinfo_loader_process_default_path()` → `osinfo_loader_get_db()`).
/// The loader itself isn't a public type here — pure plumbing to reach
/// the `Db`, matching upstream's own shape (nothing outside this init
/// sequence ever touches the loader again once it has one).
pub struct OsDb {
    ptr: *mut ffi::OsinfoDb,
}

impl OsDb {
    /// Loads the OS database from the standard system/local/user paths
    /// (`osinfo_loader_process_default_path`) — the actual OS
    /// definitions live in `osinfo-db`, a separately-versioned distro
    /// package (ticket 04's own finding), not vendored by this crate.
    pub fn load_default() -> Result<OsDb, OsInfoError> {
        unsafe {
            let loader = ffi::osinfo_loader_new();
            assert!(!loader.is_null(), "osinfo_loader_new returned NULL");

            let mut error: *mut glib_sys::GError = ptr::null_mut();
            ffi::osinfo_loader_process_default_path(loader, &mut error);
            if let Some(e) = take_glib_error(error) {
                gobject_sys::g_object_unref(loader as *mut gobject_sys::GObject);
                return Err(e);
            }

            let db = ffi::osinfo_loader_get_db(loader);
            assert!(
                !db.is_null(),
                "osinfo_loader_get_db returned NULL after a successful load"
            );
            // osinfo_loader_get_db is transfer-ownership="none" per
            // the real .gir annotation (checked directly -- C headers
            // carry no ownership-transfer information at all, so this
            // can't be read off the function signature bindgen saw;
            // inferring it from the function's name alone is exactly
            // the mistake that would have gone uncaught here) -- it
            // returns a borrowed reference the loader still owns, so
            // take our own ref before the loader itself gets dropped.
            gobject_sys::g_object_ref(db as *mut gobject_sys::GObject);
            gobject_sys::g_object_unref(loader as *mut gobject_sys::GObject);

            Ok(OsDb { ptr: db })
        }
    }

    /// A single OS entry by its full libosinfo ID (e.g.
    /// `"http://fedoraproject.org/fedora/40"`), or `None` if unknown —
    /// upstream's `OSDB.get_os(id)`.
    pub fn os(&self, id: &str) -> Option<OsInfo> {
        let c_id = CString::new(id).ok()?;
        unsafe {
            let os = ffi::osinfo_db_get_os(self.ptr, c_id.as_ptr());
            if os.is_null() {
                return None;
            }
            // transfer-ownership="none" per the real .gir annotation
            // (checked directly, same as osinfo_loader_get_db above) —
            // borrowed, take our own ref.
            gobject_sys::g_object_ref(os as *mut gobject_sys::GObject);
            Some(OsInfo { ptr: os })
        }
    }

    /// Every OS entry the database knows about — upstream's rough
    /// equivalent of iterating `OSDB.get_all_os()`. `OsinfoList`'s C
    /// API is a generic index-by-position container
    /// (`osinfo_list_get_length`/`osinfo_list_get_nth`), the base
    /// class every `Osinfo*List` type shares — not modeled as its own
    /// Rust type here since no other caller needs list iteration
    /// beyond this one method yet.
    pub fn all_os(&self) -> Vec<OsInfo> {
        unsafe {
            let list = ffi::osinfo_db_get_os_list(self.ptr);
            if list.is_null() {
                return Vec::new();
            }
            let list = list as *mut ffi::OsinfoList;
            let len = ffi::osinfo_list_get_length(list);
            let mut result = Vec::with_capacity(len.max(0) as usize);
            for i in 0..len {
                // osinfo_list_get_nth is transfer-ownership="none" per
                // the real .gir annotation (checked directly) —
                // borrowed, take our own ref.
                let entity = ffi::osinfo_list_get_nth(list, i);
                if !entity.is_null() {
                    gobject_sys::g_object_ref(entity as *mut gobject_sys::GObject);
                    result.push(OsInfo {
                        ptr: entity as *mut ffi::OsinfoOs,
                    });
                }
            }
            // osinfo_db_get_os_list is transfer-ownership="full" per
            // the real .gir annotation (checked directly, not inferred
            // from the `get_` prefix — that inference would have been
            // wrong in the other direction for osinfo_loader_get_db/
            // osinfo_db_get_os above, so it isn't trustworthy here
            // either without checking): a fresh, owned list container,
            // distinct from the OsinfoOs entries it holds (each
            // borrowed, refed individually above) — release our
            // ownership of the container itself once we're done
            // walking it.
            gobject_sys::g_object_unref(list as *mut gobject_sys::GObject);
            result
        }
    }
}

impl Drop for OsDb {
    fn drop(&mut self) {
        unsafe { gobject_sys::g_object_unref(self.ptr as *mut gobject_sys::GObject) };
    }
}

/// One OS entry — upstream's `osdict.py` wrapper, kept intentionally
/// minimal (id/short_id/name) until a real consumer (`--os-variant` on
/// the CLI, the per-driver device-default chains ticket 17 catalogued)
/// needs more of libosinfo's actual data (device lists, install
/// scripts, firmware requirements, …).
pub struct OsInfo {
    ptr: *mut ffi::OsinfoOs,
}

impl OsInfo {
    /// The full libosinfo ID, e.g.
    /// `"http://fedoraproject.org/fedora/40"` — `OsinfoEntity`'s own
    /// `id` property, shared by every entity type in the database, not
    /// just OS entries.
    pub fn id(&self) -> String {
        unsafe {
            c_str_to_string(ffi::osinfo_entity_get_id(
                self.ptr as *mut ffi::OsinfoEntity,
            ))
        }
    }

    /// The short, stable identifier (e.g. `"fedora40"`) — upstream's
    /// `--os-variant` value.
    pub fn short_id(&self) -> String {
        unsafe {
            c_str_to_string(ffi::osinfo_product_get_short_id(
                self.ptr as *mut ffi::OsinfoProduct,
            ))
        }
    }

    /// Human-readable display name, e.g. `"Fedora 40"`.
    pub fn name(&self) -> String {
        unsafe {
            c_str_to_string(ffi::osinfo_product_get_name(
                self.ptr as *mut ffi::OsinfoProduct,
            ))
        }
    }
}

impl Clone for OsInfo {
    fn clone(&self) -> Self {
        unsafe { gobject_sys::g_object_ref(self.ptr as *mut gobject_sys::GObject) };
        OsInfo { ptr: self.ptr }
    }
}

impl Drop for OsInfo {
    fn drop(&mut self) {
        unsafe { gobject_sys::g_object_unref(self.ptr as *mut gobject_sys::GObject) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real end-to-end proof against this machine's actual installed
    /// `osinfo-db`, not a fixture — there's nothing to fake here the
    /// way `test:///` stands in for a real libvirt driver: libosinfo
    /// has no equivalent injection seam, and osinfo-db is real,
    /// system-installed XML data (`/usr/share/osinfo`, ticket 04).
    ///
    /// Asserts on a *property* of a specific, decade-old, permanently-
    /// archived OS entry (Fedora 25, EOL for years) rather than a raw
    /// count of anything — per ticket 04's own finding, osinfo-db is
    /// an independently-versioned distro package that will keep
    /// growing new entries under this test over time; an old release
    /// entry never gets removed, only ever added to.
    #[test]
    fn loads_the_real_system_db_and_finds_a_known_stable_os_entry() {
        let db = OsDb::load_default().expect("osinfo-db is installed on this machine");

        let fedora25 = db
            .os("http://fedoraproject.org/fedora/25")
            .expect("Fedora 25 is a real, permanently-archived osinfo-db entry");
        assert_eq!(fedora25.id(), "http://fedoraproject.org/fedora/25");
        assert_eq!(fedora25.short_id(), "fedora25");
        assert_eq!(fedora25.name(), "Fedora 25");

        assert!(
            db.os("this-id-does-not-exist-anywhere").is_none(),
            "an unknown id must read as absent, not panic or return garbage"
        );
    }

    /// `all_os()`'s list-iteration path — proven separately from the
    /// single-lookup path above since it exercises the generic
    /// `OsinfoList` accessors (`osinfo_list_get_length`/`_get_nth`),
    /// not `osinfo_db_get_os`.
    #[test]
    fn all_os_includes_the_same_known_entry_the_direct_lookup_finds() {
        let db = OsDb::load_default().expect("osinfo-db is installed on this machine");

        let all = db.all_os();
        // A real distro osinfo-db package has hundreds of entries;
        // asserting merely "not empty" (rather than an exact count)
        // is the stable-under-package-updates version of that check.
        assert!(
            all.len() > 10,
            "a real osinfo-db install has far more than 10 entries, got {}",
            all.len()
        );

        assert!(
            all.iter()
                .any(|os| os.id() == "http://fedoraproject.org/fedora/25"),
            "the same entry the direct os() lookup finds must show up in the full list too"
        );
    }

    /// `OsInfo::clone()`'s own refcounting -- proves a cloned handle
    /// keeps working (and doesn't double-free) after the original is
    /// dropped, the actual risk a hand-rolled `g_object_ref`/`_unref`
    /// pair carries that a derived `Clone` wouldn't.
    #[test]
    fn cloned_os_info_outlives_the_original() {
        let db = OsDb::load_default().expect("osinfo-db is installed on this machine");
        let original = db
            .os("http://fedoraproject.org/fedora/25")
            .expect("real entry");
        let cloned = original.clone();
        drop(original);
        assert_eq!(cloned.short_id(), "fedora25");
    }
}
