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
pub mod installscript;

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

    /// `--os-variant`-shaped lookup: tries `id` as a full libosinfo ID
    /// first (`http://fedoraproject.org/fedora/40`), falling back to a
    /// linear scan for a matching *primary* short-id (`fedora40`) —
    /// the form users actually type on the CLI, upstream's own
    /// `_OSDB.lookup_os` (short-id path) vs. `lookup_os_by_full_id`
    /// split. Doesn't model an OS's full `short_id_list` (aliases
    /// beyond the primary one) — `OsInfo::short_id` only exposes the
    /// primary — a stated gap, not silently dropped, since libosinfo's
    /// `_list` variant is a separate query this crate doesn't bind
    /// yet.
    ///
    /// ponytail: the short-id fallback walks every entry in the db
    /// (`all_os()`'s full ref/unref pass) on every miss -- O(n) per
    /// lookup, fine for `virt-install`'s one call per run, quadratic
    /// if a future caller (e.g. a GUI OS-picker) calls this in a loop.
    /// Upstream avoids it with `Libosinfo.Filter` + `PRODUCT_PROP_SHORT_ID`;
    /// switch to that if this ever shows up in a loop.
    /// `_OSDB.lookup_os`: the first entry with `key` among its
    /// short-ids (aliases included).
    pub fn lookup_os(&self, key: &str) -> Option<OsInfo> {
        self.all_os()
            .into_iter()
            .find(|o| o.short_id_list().iter().any(|s| s == key))
    }

    pub fn os_by_id_or_short_id(&self, id: &str) -> Option<OsInfo> {
        self.os(id)
            .or_else(|| self.all_os().into_iter().find(|o| o.short_id() == id))
    }
}

/// `_OsMedia`: a libosinfo media object.
pub struct OsMedia {
    ptr: *mut ffi::OsinfoMedia,
    /// The media only holds a weak ref to its OS: keep it alive, like
    /// upstream's process-wide OSDB does.
    os: Option<OsInfo>,
}

impl OsMedia {
    pub fn get_kernel_path(&self) -> Option<String> {
        let p = unsafe { ffi::osinfo_media_get_kernel_path(self.ptr) };
        (!p.is_null()).then(|| unsafe { c_str_to_string(p) })
    }

    pub fn get_initrd_path(&self) -> Option<String> {
        let p = unsafe { ffi::osinfo_media_get_initrd_path(self.ptr) };
        (!p.is_null()).then(|| unsafe { c_str_to_string(p) })
    }

    /// `is_netinst`.
    pub fn is_netinst(&self) -> bool {
        unsafe {
            let list = ffi::osinfo_media_get_os_variants(self.ptr) as *mut ffi::OsinfoList;
            if list.is_null() {
                return false;
            }
            let mut ret = false;
            for i in 0..ffi::osinfo_list_get_length(list) {
                let v = ffi::osinfo_list_get_nth(list, i) as *mut ffi::OsinfoEntity;
                if !v.is_null() && c_str_to_string(ffi::osinfo_entity_get_id(v)).contains("netinst")
                {
                    ret = true;
                }
            }
            gobject_sys::g_object_unref(list as *mut gobject_sys::GObject);
            ret
        }
    }
}

impl Drop for OsMedia {
    fn drop(&mut self) {
        unsafe { gobject_sys::g_object_unref(self.ptr as *mut gobject_sys::GObject) };
    }
}

/// `_OsTree`: a libosinfo tree object.
pub struct OsTree {
    ptr: *mut ffi::OsinfoTree,
    /// Same as `OsMedia::os`.
    os: Option<OsInfo>,
}

impl Drop for OsTree {
    fn drop(&mut self) {
        unsafe { gobject_sys::g_object_unref(self.ptr as *mut gobject_sys::GObject) };
    }
}

impl OsDb {
    /// Upstream's `guess_os_by_iso`: the OS libosinfo identifies from
    /// an install ISO's volume descriptors.
    pub fn guess_os_by_iso(&self, location: &str) -> Option<OsInfo> {
        self.guess_media_by_iso(location).map(|(os, _)| os)
    }

    /// `guess_os_by_iso`, with the media object.
    pub fn guess_media_by_iso(&self, location: &str) -> Option<(OsInfo, OsMedia)> {
        let c_loc = CString::new(location).ok()?;
        unsafe {
            let mut error: *mut glib_sys::GError = ptr::null_mut();
            // transfer-ownership="full"
            let media =
                ffi::osinfo_media_create_from_location(c_loc.as_ptr(), ptr::null_mut(), &mut error);
            if let Some(e) = take_glib_error(error) {
                log::debug!("Error creating libosinfo media object: {e}");
                return None;
            }
            if media.is_null() {
                return None;
            }
            let os = if ffi::osinfo_db_identify_media(self.ptr, media) != 0 {
                // transfer-ownership="full"
                ffi::osinfo_media_get_os(media)
            } else {
                ptr::null_mut()
            };
            if os.is_null() {
                let _ = OsMedia {
                    ptr: media,
                    os: None,
                };
                return None;
            }
            let os = OsInfo { ptr: os };
            let media = OsMedia {
                ptr: media,
                os: Some(os.clone()),
            };
            Some((os, media))
        }
    }

    /// Upstream's `guess_os_by_tree`: the OS of a network install tree
    /// (a URL, or a local path).
    pub fn guess_os_by_tree(&self, location: &str) -> Option<OsInfo> {
        self.guess_tree(location, false).map(|(os, _)| os)
    }

    /// `guess_os_by_tree`, with the tree object. `in_testsuite` skips
    /// non-file URLs like upstream's mock network tests do.
    pub fn guess_tree(&self, location: &str, in_testsuite: bool) -> Option<(OsInfo, OsTree)> {
        let location = if location.starts_with('/') {
            format!("file://{location}")
        } else {
            location.to_string()
        };
        if in_testsuite && !location.starts_with("file:") {
            // We have mock network tests, but we don't want to pass the
            // fake URL to libosinfo because it slows down the testcase
            return None;
        }
        let c_loc = CString::new(location.clone()).ok()?;
        unsafe {
            let mut error: *mut glib_sys::GError = ptr::null_mut();
            // transfer-ownership="full"
            let tree =
                ffi::osinfo_tree_create_from_location(c_loc.as_ptr(), ptr::null_mut(), &mut error);
            if let Some(e) = take_glib_error(error) {
                log::debug!("Error creating libosinfo tree object for location={location} : {e}");
                return None;
            }
            if tree.is_null() {
                return None;
            }
            let os = if ffi::osinfo_db_identify_tree(self.ptr, tree) != 0 {
                // transfer-ownership="full"
                ffi::osinfo_tree_get_os(tree)
            } else {
                ptr::null_mut()
            };
            if os.is_null() {
                let _ = OsTree {
                    ptr: tree,
                    os: None,
                };
                return None;
            }
            let os = OsInfo { ptr: os };
            let tree = OsTree {
                ptr: tree,
                os: Some(os.clone()),
            };
            Some((os, tree))
        }
    }
}

/// One `<resources>` entry: `-1` is libosinfo's "not set".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResourceValues {
    ram: i64,
    n_cpus: i64,
    storage: i64,
}

/// Upstream's `_OsResources`: the minimum and recommended resources,
/// by arch (`all` for any arch).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OsResources {
    minimum: Vec<(String, ResourceValues)>,
    recommended: Vec<(String, ResourceValues)>,
}

impl OsResources {
    fn get_key(
        resources: &[(String, ResourceValues)],
        key: fn(&ResourceValues) -> i64,
        arch: &str,
    ) -> Option<i64> {
        for checkarch in [arch, "all"] {
            // A later entry for the same arch wins, like the dict.
            let val = resources
                .iter()
                .rev()
                .find(|(a, _)| a == checkarch)
                .map_or(-1, |(_, v)| key(v));
            if val != -1 {
                return Some(val);
            }
        }
        None
    }

    fn get_minimum_key(&self, key: fn(&ResourceValues) -> i64, arch: &str) -> Option<i64> {
        Self::get_key(&self.minimum, key, arch).filter(|v| *v > 0)
    }

    fn get_recommended_key(&self, key: fn(&ResourceValues) -> i64, arch: &str) -> Option<i64> {
        if let Some(v) = Self::get_key(&self.recommended, key, arch).filter(|v| *v > 0) {
            return Some(v);
        }
        // If we are looking for a recommended value, but the OS DB
        // only has minimum resources tracked, double the minimum value
        // as an approximation at a 'recommended' value
        self.get_minimum_key(key, arch).map(|v| v * 2)
    }

    pub fn get_minimum_ram(&self, arch: &str) -> Option<i64> {
        self.get_minimum_key(|r| r.ram, arch)
    }

    /// Bytes.
    pub fn get_recommended_ram(&self, arch: &str) -> Option<i64> {
        self.get_recommended_key(|r| r.ram, arch)
    }

    pub fn get_recommended_ncpus(&self, arch: &str) -> Option<i64> {
        self.get_recommended_key(|r| r.n_cpus, arch)
    }

    /// Bytes.
    pub fn get_recommended_storage(&self, arch: &str) -> Option<i64> {
        self.get_recommended_key(|r| r.storage, arch)
    }
}

/// # Safety
/// `list` must be null or an owned `OsinfoResourcesList`; it's freed.
unsafe fn take_resources_list(
    list: *mut ffi::OsinfoResourcesList,
) -> Vec<(String, ResourceValues)> {
    if list.is_null() {
        return vec![];
    }
    let l = list as *mut ffi::OsinfoList;
    let mut ret = vec![];
    for i in 0..ffi::osinfo_list_get_length(l) {
        let r = ffi::osinfo_list_get_nth(l, i) as *mut ffi::OsinfoResources;
        if r.is_null() {
            continue;
        }
        ret.push((
            c_str_to_string(ffi::osinfo_resources_get_architecture(r)),
            ResourceValues {
                ram: ffi::osinfo_resources_get_ram(r),
                n_cpus: ffi::osinfo_resources_get_n_cpus(r) as i64,
                storage: ffi::osinfo_resources_get_storage(r),
            },
        ));
    }
    gobject_sys::g_object_unref(list as *mut gobject_sys::GObject);
    ret
}

impl OsDb {
    /// `list_os(sortkey="label")` plus vmmOSList's "generic entries
    /// last" ordering: every entry, the `generic` pseudo-OS included,
    /// natural-sorted by label with numbers sorted in reverse (so the
    /// newest release of a distro comes first).
    pub fn list_os_variants(&self) -> Vec<OsVariant> {
        let mut list: Vec<OsVariant> = self.all_os().iter().map(OsVariant::from_os).collect();
        list.push(OsVariant::generic());
        list.sort_by_cached_key(|o| alphanum_key(&o.label));
        list.sort_by_key(OsVariant::always_show);
        list
    }
}

/// `alphanum_key`: lowercase text runs, with digit runs negated so
/// higher numbers sort first.
pub fn alphanum_key(val: &str) -> Vec<Result<i64, String>> {
    let mut out = Vec::new();
    let mut rest = val;
    while !rest.is_empty() {
        let digit = rest.starts_with(|c: char| c.is_ascii_digit());
        let end = rest
            .find(|c: char| c.is_ascii_digit() != digit)
            .unwrap_or(rest.len());
        let (run, tail) = rest.split_at(end);
        out.push(match run.parse::<i64>() {
            Ok(n) if digit => Ok(-n),
            _ => Err(run.to_lowercase()),
        });
        rest = tail;
    }
    out
}

/// Days since 1970-01-01 for a `YYYY-MM-DD` date.
fn days_from_ymd(date: &str) -> Option<i64> {
    let mut it = date.splitn(3, '-').map(|p| p.parse::<i64>().ok());
    let (y, m, d) = (it.next()??, it.next()??, it.next()??);
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146097 + doe - 719468)
}

fn today_days() -> i64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    (secs / 86400) as i64
}

/// `_OsVariant._get_eol`: past its EOL date, or (with no EOL date and
/// not a rolling release) released more than ten years ago.
fn compute_eol(eol: &str, release: &str, release_status: &str, today: i64) -> bool {
    if let Some(eol) = days_from_ymd(eol) {
        return today >= eol;
    }
    if release_status == "rolling" {
        return false;
    }
    days_from_ymd(release).is_some_and(|rel| today > rel + 365 * 10)
}

/// One row of the OS picker: upstream's `_OsVariant` reduced to what
/// the GUI reads, detached from libosinfo so it can be cached. The
/// `generic` pseudo-OS has no `full_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OsVariant {
    /// Primary short-id (`fedora40`).
    pub name: String,
    /// Display name (`Fedora 40`).
    pub label: String,
    pub full_id: Option<String>,
    pub eol: bool,
}

impl OsVariant {
    fn from_os(os: &OsInfo) -> OsVariant {
        let product = os.ptr as *mut ffi::OsinfoProduct;
        let (eol, release, status) = unsafe {
            (
                c_str_to_string(ffi::osinfo_product_get_eol_date_string(product)),
                c_str_to_string(ffi::osinfo_product_get_release_date_string(product)),
                c_str_to_string(ffi::osinfo_entity_get_param_value(
                    os.ptr as *mut ffi::OsinfoEntity,
                    ffi::OSINFO_OS_PROP_RELEASE_STATUS.as_ptr() as *const _,
                )),
            )
        };
        OsVariant {
            name: os.short_id(),
            label: os.name(),
            full_id: Some(os.id()).filter(|s| !s.is_empty()),
            eol: compute_eol(&eol, &release, &status, today_days()),
        }
    }

    /// `_OSDB._os_generic`.
    pub fn generic() -> OsVariant {
        OsVariant {
            name: "generic".into(),
            label: "Generic or unknown OS. Usage is not recommended.".into(),
            full_id: None,
            eol: false,
        }
    }

    pub fn is_generic(&self) -> bool {
        self.name == "generic"
    }

    /// `is_linux_generic`: `linux2022` and friends.
    pub fn is_linux_generic(&self) -> bool {
        self.name
            .strip_prefix("linux")
            .is_some_and(|y| y.len() >= 4 && y[..4].bytes().all(|b| b.is_ascii_digit()))
    }

    /// oslist.py's `_always_show`.
    pub fn always_show(&self) -> bool {
        self.is_generic() || self.is_linux_generic()
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

    /// Every short-id of this entry (`short_id_list`), primary first:
    /// aliases like `fedora-unknown` included.
    pub fn short_id_list(&self) -> Vec<String> {
        let mut ret = vec![];
        unsafe {
            // (transfer container) (element-type utf8): the strings
            // belong to the entity, only the list is ours.
            let list = ffi::osinfo_product_get_short_id_list(self.ptr as *mut ffi::OsinfoProduct);
            let mut node = list;
            while !node.is_null() {
                ret.push(c_str_to_string((*node).data as *const std::os::raw::c_char));
                node = (*node).next;
            }
            glib_sys::g_list_free(list);
        }
        if ret.is_empty() {
            ret.push(self.short_id());
        }
        ret
    }

    /// `_get_generic_location`/`get_location`: the install tree URL for
    /// `arch`, preferring the tree whose variant matches `profile`.
    pub fn get_location(&self, arch: &str, profile: Option<&str>) -> Result<String, String> {
        let profile = match profile {
            Some("jeos") => "Server",
            Some("desktop") => "Workstation",
            None | Some("") => "Everything",
            Some(p) => p,
        };
        // (url, matches profile) of each tree for `arch`.
        let mut trees: Vec<(String, bool)> = vec![];
        let mut have_trees = false;
        unsafe {
            let list = ffi::osinfo_os_get_tree_list(self.ptr) as *mut ffi::OsinfoList;
            if !list.is_null() {
                for i in 0..ffi::osinfo_list_get_length(list) {
                    let tree = ffi::osinfo_list_get_nth(list, i) as *mut ffi::OsinfoTree;
                    if tree.is_null() {
                        continue;
                    }
                    have_trees = true;
                    if c_str_to_string(ffi::osinfo_tree_get_architecture(tree)) != arch {
                        continue;
                    }
                    let url = c_str_to_string(ffi::osinfo_tree_get_url(tree));
                    let variants = ffi::osinfo_tree_get_os_variants(tree) as *mut ffi::OsinfoList;
                    let mut matched = false;
                    if !variants.is_null() {
                        for j in 0..ffi::osinfo_list_get_length(variants) {
                            let v =
                                ffi::osinfo_list_get_nth(variants, j) as *mut ffi::OsinfoOsVariant;
                            if !v.is_null()
                                && c_str_to_string(ffi::osinfo_os_variant_get_name(v))
                                    .contains(profile)
                            {
                                matched = true;
                            }
                        }
                        gobject_sys::g_object_unref(variants as *mut gobject_sys::GObject);
                    }
                    trees.push((url, matched));
                }
                gobject_sys::g_object_unref(list as *mut gobject_sys::GObject);
            }
        }
        if !have_trees {
            return Err(format!(
                "OS '{}' does not have a URL location",
                self.short_id()
            ));
        }
        // The first tree with a matching variant, else the last tree
        // for the architecture.
        if let Some((url, _)) = trees.iter().find(|(_, m)| *m).or(trees.last()) {
            return Ok(url.clone());
        }
        Err(format!(
            "OS '{}' does not have a URL location for the architecture '{arch}'",
            self.short_id()
        ))
    }

    /// Human-readable display name, e.g. `"Fedora 40"`.
    pub fn name(&self) -> String {
        unsafe {
            c_str_to_string(ffi::osinfo_product_get_name(
                self.ptr as *mut ffi::OsinfoProduct,
            ))
        }
    }

    /// The generic OS family this entry belongs to, e.g. `"linux"`/
    /// `"winnt"`/`"solaris"`/`"freebsd"` — upstream's `osdict.py`'s
    /// `_OsVariant._family` (`self._os.get_family()`). Empty when
    /// libosinfo doesn't report one for this entry (the real getter is
    /// `osinfo_os_get_family`, an `OsinfoOs`-level property — not
    /// `OsinfoProduct`, despite `id`/`short_id`/`name` above all being
    /// `OsinfoEntity`/`OsinfoProduct`-level; confirmed against the
    /// generated bindings rather than assumed to follow the same
    /// pattern).
    pub fn family(&self) -> String {
        unsafe { c_str_to_string(ffi::osinfo_os_get_family(self.ptr)) }
    }

    /// Upstream's `osdict.py`'s `is_windows()` — `_family in ["win9x",
    /// "winnt", "win16"]`, ported literally rather than generalized
    /// (upstream doesn't generalize it either).
    pub fn is_windows(&self) -> bool {
        matches!(self.family().as_str(), "win9x" | "winnt" | "win16")
    }

    /// Upstream's `osdict.py`'s `get_clock()` — `"localtime"` for
    /// Windows or Solaris guests, `"utc"` otherwise. Pure logic over
    /// `family()`/`is_windows()`, no separate libosinfo call: this is
    /// exactly the piece ticket 21 flagged as blocked outright
    /// (`clock.py`'s `offset` field, "blocked — no fallback branch at
    /// all") — now unblocked now that `is_windows()`/`family()` exist,
    /// closing that specific gap. The optional `hypervclock` timer
    /// (`hyperv_supported()`) stays blocked regardless: it needs
    /// `is_windows()` *and* `conn.support.conn_hyperv_clock()`, a
    /// hypervisor-version support-check mechanism this crate still
    /// doesn't have (the same class of gap that blocked slice (b)'s
    /// q35-preferred machine branch) — `is_windows()` existing doesn't
    /// make that second half reachable too.
    pub fn clock_offset(&self) -> &'static str {
        if self.is_windows() || self.family() == "solaris" {
            "localtime"
        } else {
            "utc"
        }
    }

    /// `osinfo_os_get_all_devices(self, NULL)` — every device this OS
    /// (and, per libosinfo's own doc comment on this call vs. plain
    /// `get_devices`, every OS it derives from or clones) declares
    /// support for, with no property filter. Upstream's `osdict.py`
    /// builds an `OsinfoFilter` for the `devids`-only case anyway
    /// (`_device_filter`) but never actually calls
    /// `osinfo_filter_add_constraint` with anything before iterating —
    /// it filters client-side by walking every device and checking
    /// `dev.get_id()` — so passing `NULL` here and matching client-side
    /// too is the same behavior via the same libosinfo call, not a
    /// simplification of it.
    ///
    /// `osinfo_os_get_all_devices` is `transfer-ownership="full"`
    /// returning an `OsinfoDeviceList*` (checked directly against the
    /// real `.gir`, not inferred from the `get_` prefix — see this
    /// module's doc comment on why that inference isn't trustworthy).
    /// `OsinfoDeviceList`'s parent is `OsinfoList` (confirmed against
    /// the generated struct layout), so it walks with the exact same
    /// `osinfo_list_get_length`/`osinfo_list_get_nth` pair `all_os()`
    /// already uses on `OsinfoDb`'s os list, each element
    /// `transfer-ownership="none"` per `.gir`.
    fn has_device_id(&self, device_id: &str) -> bool {
        unsafe {
            let list = ffi::osinfo_os_get_all_devices(self.ptr, std::ptr::null_mut());
            if list.is_null() {
                return false;
            }
            let list = list as *mut ffi::OsinfoList;
            let len = ffi::osinfo_list_get_length(list);
            let mut found = false;
            for i in 0..len {
                let entity = ffi::osinfo_list_get_nth(list, i);
                if !entity.is_null()
                    && c_str_to_string(ffi::osinfo_entity_get_id(entity)) == device_id
                {
                    found = true;
                    break;
                }
            }
            gobject_sys::g_object_unref(list as *mut gobject_sys::GObject);
            found
        }
    }

    /// `_device_filter(devids, extra_devs=guest._extra_drivers)`: like
    /// `has_device_id`, plus the devices `add_extra_drivers` registered.
    fn has_device_id_or_extra(&self, device_id: &str) -> bool {
        self.has_device_id(device_id)
            || installscript::extra_devices()
                .iter()
                .any(|d| d.id == device_id)
    }

    /// Upstream's `osdict.py`'s `supports_virtiodisk()` — the exact two
    /// PCI device ids it hardcodes (`virtio-block` and `virtio1.0-block`),
    /// ported literally. `extra_devs` (`Guest._extra_drivers`, set by
    /// `--unattended` for Windows) lives in `installscript::
    /// add_extra_drivers`; the checks guest.py passes it to use
    /// `has_device_id_or_extra`.
    pub fn supports_virtiodisk(&self) -> bool {
        self.has_device_id_or_extra("http://pcisig.com/pci/1af4/1001")
            || self.has_device_id_or_extra("http://pcisig.com/pci/1af4/1042")
    }

    /// Upstream's `osdict.py`'s `supports_virtioscsi()` — same two-id
    /// shape as `supports_virtiodisk()` above.
    pub fn supports_virtioscsi(&self) -> bool {
        self.has_device_id_or_extra("http://pcisig.com/pci/1af4/1004")
            || self.has_device_id_or_extra("http://pcisig.com/pci/1af4/1048")
    }

    /// Upstream's `osdict.py`'s `supports_virtiogpu()`: virtio1.0-gpu.
    pub fn supports_virtiogpu(&self) -> bool {
        self.has_device_id("http://pcisig.com/pci/1af4/1050")
    }

    /// Upstream's `osdict.py`'s `supports_virtionet()` — same shape.
    pub fn supports_virtionet(&self) -> bool {
        self.has_device_id_or_extra("http://pcisig.com/pci/1af4/1000")
            || self.has_device_id_or_extra("http://pcisig.com/pci/1af4/1041")
    }

    /// Upstream's `osdict.py`'s `supports_virtiorng()`.
    pub fn supports_virtiorng(&self) -> bool {
        self.has_device_id_or_extra("http://pcisig.com/pci/1af4/1005")
            || self.has_device_id_or_extra("http://pcisig.com/pci/1af4/1044")
    }

    /// Upstream's `osdict.py`'s `supports_virtioballoon()`.
    pub fn supports_virtioballoon(&self) -> bool {
        self.has_device_id_or_extra("http://pcisig.com/pci/1af4/1002")
            || self.has_device_id_or_extra("http://pcisig.com/pci/1af4/1045")
    }

    /// Upstream's `osdict.py`'s `supports_virtioserial()`, without its
    /// stopgap for osinfo-db's pre-2018 RHEL 6 data.
    pub fn supports_virtioserial(&self) -> bool {
        self.has_device_id_or_extra("http://pcisig.com/pci/1af4/1003")
            || self.has_device_id_or_extra("http://pcisig.com/pci/1af4/1043")
    }

    /// Upstream's `osdict.py`'s `supports_virtioinput()`: virtio1.0-input.
    pub fn supports_virtioinput(&self) -> bool {
        self.has_device_id_or_extra("http://pcisig.com/pci/1af4/1052")
    }

    /// Upstream's `osdict.py`'s `supports_usb3()`: qemu-xhci.
    pub fn supports_usb3(&self) -> bool {
        self.has_device_id("http://pcisig.com/pci/1b36/0004")
    }

    /// Upstream's `osdict.py`'s `supports_chipset_q35()`: q35, and
    /// virtio1.0 too when the OS knows virtio-net at all.
    pub fn supports_chipset_q35(&self) -> bool {
        let virtionet = self.has_device_id("http://pcisig.com/pci/1af4/1000")
            || self.has_device_id("http://pcisig.com/pci/1af4/1041");
        if virtionet && !self.has_device_id("http://pcisig.com/pci/1af4/1041") {
            return false;
        }
        self.has_device_id("http://qemu.org/chipset/x86/q35")
    }

    /// Upstream's `osdict.py`'s `supported_netmodels()` — `self.
    /// _device_filter(cls="net")`: every declared device whose
    /// `get_class()` is `"net"`, by display name (`get_name()`, e.g.
    /// `"e1000"`), not id. Confirmed against real `osinfo-db` XML
    /// (`/usr/share/osinfo/device/pcisig.com/pci-1af4-1000.d/*.xml`)
    /// that `<class>` is a plain literal string (`"net"`), not a
    /// hierarchical/prefixed value — upstream's `re.match(cls, ...)`
    /// (a prefix match, not equality) degrades to plain equality here
    /// for that reason, not as an unchecked simplification.
    ///
    /// `osinfo_device_get_class`/`osinfo_device_get_name` are both
    /// `transfer-ownership="none"` per the real `.gir` (checked
    /// directly) — borrowed C strings, safe to read without taking a
    /// ref. `OsinfoDevice`'s first field is `OsinfoEntity` (confirmed
    /// against the generated struct layout, same inheritance-by-
    /// embedding cast already established elsewhere in this module),
    /// so `osinfo_list_get_nth`'s `*mut OsinfoEntity` is a safe cast to
    /// `*mut OsinfoDevice` here — this device list has no other entity
    /// type in it.
    ///
    /// Upstream doesn't pass `extra_devs` here.
    pub fn supported_netmodels(&self) -> Vec<String> {
        unsafe {
            let list = ffi::osinfo_os_get_all_devices(self.ptr, std::ptr::null_mut());
            if list.is_null() {
                return Vec::new();
            }
            let raw_list = list as *mut ffi::OsinfoList;
            let len = ffi::osinfo_list_get_length(raw_list);
            let mut result = Vec::new();
            for i in 0..len {
                let entity = ffi::osinfo_list_get_nth(raw_list, i);
                if entity.is_null() {
                    continue;
                }
                let dev = entity as *mut ffi::OsinfoDevice;
                if c_str_to_string(ffi::osinfo_device_get_class(dev)) == "net" {
                    result.push(c_str_to_string(ffi::osinfo_device_get_name(dev)));
                }
            }
            gobject_sys::g_object_unref(list as *mut gobject_sys::GObject);
            result
        }
    }

    /// Upstream's `osdict.py`'s `_get_firmware_list()` — every declared
    /// `(architecture, firmware_type, is_supported)` triple, ported
    /// without upstream's `hasattr(self._os, "get_complete_firmware_
    /// list")` guard: that guard exists because upstream supports
    /// arbitrarily old installed `libosinfo` versions at runtime, while
    /// this crate's binding is generated at build time against whatever
    /// `libosinfo` headers are actually installed (ticket 04) — the
    /// function is either present in the generated bindings or this
    /// crate wouldn't compile, so there's no "maybe absent at runtime"
    /// case to guard against here the way upstream's dynamically-typed
    /// `hasattr` check does.
    ///
    /// `osinfo_os_get_complete_firmware_list` is `transfer-ownership=
    /// "full"` returning an `OsinfoFirmwareList*` (checked directly
    /// against the real `.gir`); `OsinfoFirmwareList`'s parent is
    /// `OsinfoList` (confirmed against the generated struct layout,
    /// same as `OsinfoDeviceList` above), so it walks with the same
    /// `osinfo_list_get_length`/`osinfo_list_get_nth` pair.
    /// `osinfo_firmware_get_architecture`/`get_firmware_type`/
    /// `is_supported` are all `transfer-ownership="none"` (the first
    /// two) or a plain `gboolean` (the third) per the real `.gir` —
    /// borrowed, safe to read without taking a ref. `OsinfoFirmware`'s
    /// first field is `OsinfoEntity` (confirmed against the generated
    /// struct layout), the same inheritance-by-embedding cast already
    /// established elsewhere in this module.
    fn firmware_list(&self) -> Vec<(String, String, bool)> {
        unsafe {
            let list = ffi::osinfo_os_get_complete_firmware_list(self.ptr, std::ptr::null_mut());
            if list.is_null() {
                return Vec::new();
            }
            let raw_list = list as *mut ffi::OsinfoList;
            let len = ffi::osinfo_list_get_length(raw_list);
            let mut result = Vec::with_capacity(len.max(0) as usize);
            for i in 0..len {
                let entity = ffi::osinfo_list_get_nth(raw_list, i);
                if entity.is_null() {
                    continue;
                }
                let fw = entity as *mut ffi::OsinfoFirmware;
                result.push((
                    c_str_to_string(ffi::osinfo_firmware_get_architecture(fw)),
                    c_str_to_string(ffi::osinfo_firmware_get_firmware_type(fw)),
                    ffi::osinfo_firmware_is_supported(fw) != 0,
                ));
            }
            gobject_sys::g_object_unref(list as *mut gobject_sys::GObject);
            result
        }
    }

    /// Upstream's `osdict.py`'s `_supports_firmware_type` — the first
    /// declared firmware entry matching both `arch` and `name`
    /// (`"efi"`/`"bios"`) wins; `default` when no entry for this
    /// `(arch, name)` pair exists at all (an absent `<firmware>`
    /// declaration means "no opinion", not "unsupported").
    fn supports_firmware_type(&self, name: &str, arch: &str, default: bool) -> bool {
        for (fw_arch, fw_type, is_supported) in self.firmware_list() {
            if fw_arch != arch {
                continue;
            }
            if fw_type == name {
                return is_supported;
            }
        }
        default
    }

    /// Upstream's `osdict.py`'s `requires_firmware_efi(arch)` — `efi`
    /// supported and `bios` explicitly *not* supported for this arch.
    /// Ported literally, minus upstream's outer `try/except Exception`
    /// (a defensive catch-all around dynamically-typed libosinfo calls
    /// that could raise for reasons Python can't statically rule out —
    /// this binding's calls can't raise at all, only return `false`/
    /// empty data, so there's nothing here for a `try` to catch).
    ///
    /// Verified against a real, permanently-relevant `osinfo-db` fact:
    /// Windows 11 requires UEFI (`<firmware arch="x86_64" type="efi"/>`
    /// present, `<firmware arch="x86_64" type="bios" supported="false"/>`
    /// explicit) — one of the few real hardware-requirement facts
    /// unlikely to ever change in osinfo-db, unlike an EOL-date-based
    /// fixture that stays stable only by being permanently archived.
    pub fn requires_firmware_efi(&self, arch: &str) -> bool {
        let supports_efi = self.supports_firmware_type("efi", arch, false);
        let supports_bios = self.supports_firmware_type("bios", arch, true);
        supports_efi && !supports_bios
    }

    /// `codename` (empty when unset).
    pub fn codename(&self) -> String {
        unsafe {
            c_str_to_string(ffi::osinfo_product_get_codename(
                self.ptr as *mut ffi::OsinfoProduct,
            ))
        }
    }

    /// `distro` (empty when unset).
    pub fn distro(&self) -> String {
        unsafe { c_str_to_string(ffi::osinfo_os_get_distro(self.ptr)) }
    }

    /// `get_kernel_url_arg`: the kernel argument name the distro's
    /// installer uses to reference a network source.
    pub fn get_kernel_url_arg(&self) -> Option<String> {
        // Let's ask the OS for its kernel argument for the source
        let osarg = unsafe { ffi::osinfo_os_get_kernel_url_argument(self.ptr) };
        if !osarg.is_null() {
            return Some(unsafe { c_str_to_string(osarg) });
        }
        let distro = self.distro();
        // SUSE distros
        if ["caasp", "sle", "sled", "sles", "opensuse"].contains(&distro.as_str()) {
            return Some("install".into());
        }
        if !["centos", "rhel", "fedora"].contains(&distro.as_str()) {
            return None;
        }
        // Default for RH distros, in case libosinfo data isn't complete
        Some("inst.repo".into())
    }

    /// `get_network_install_required_ram` (bytes).
    pub fn get_network_install_required_ram(&self, arch: &str) -> Option<i64> {
        let list =
            unsafe { take_resources_list(ffi::osinfo_os_get_network_install_resources(self.ptr)) };
        list.into_iter()
            .find(|(a, _)| a == arch || a == "all")
            .map(|(_, r)| r.ram)
    }

    /// Upstream's `get_recommended_resources`.
    pub fn get_recommended_resources(&self) -> OsResources {
        unsafe {
            // Both transfer-ownership="full".
            OsResources {
                minimum: take_resources_list(ffi::osinfo_os_get_minimum_resources(self.ptr)),
                recommended: take_resources_list(ffi::osinfo_os_get_recommended_resources(
                    self.ptr,
                )),
            }
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

    /// `family()`/`is_windows()`/`clock_offset()` against three real
    /// entries spanning every branch `clock_offset()` has: a Linux
    /// entry (family `linux`, not Windows/Solaris -> utc), a Windows
    /// entry (family `winnt` -> localtime), and a Solaris entry
    /// (family `solaris`, not Windows but still localtime). Windows
    /// 10 and Solaris 11 are, like Fedora 25, real archived osinfo-db
    /// entries stable enough to anchor a test on.
    #[test]
    fn clock_offset_matches_upstream_for_linux_windows_and_solaris() {
        let db = OsDb::load_default().expect("osinfo-db is installed on this machine");

        let fedora = db
            .os("http://fedoraproject.org/fedora/25")
            .expect("real entry");
        assert_eq!(fedora.family(), "linux");
        assert!(!fedora.is_windows());
        assert_eq!(fedora.clock_offset(), "utc");

        let windows = db.os("http://microsoft.com/win/10").expect("real entry");
        assert_eq!(windows.family(), "winnt");
        assert!(windows.is_windows());
        assert_eq!(windows.clock_offset(), "localtime");

        let solaris = db.os("http://oracle.com/solaris/11").expect("real entry");
        assert_eq!(solaris.family(), "solaris");
        assert!(!solaris.is_windows(), "solaris is not windows");
        assert_eq!(
            solaris.clock_offset(),
            "localtime",
            "solaris gets localtime too, despite not being windows"
        );
    }

    /// `os_by_id_or_short_id`'s two paths -- the form users actually
    /// type on the CLI (`--os-variant fedora25`) is the short-id one,
    /// not the full libosinfo URL.
    #[test]
    fn os_by_id_or_short_id_resolves_both_forms_to_the_same_entry() {
        let db = OsDb::load_default().expect("osinfo-db is installed on this machine");

        let by_full_id = db
            .os_by_id_or_short_id("http://fedoraproject.org/fedora/25")
            .expect("full id resolves");
        let by_short_id = db
            .os_by_id_or_short_id("fedora25")
            .expect("short id resolves");
        assert_eq!(by_full_id.id(), by_short_id.id());

        assert!(db.os_by_id_or_short_id("not-a-real-os-variant").is_none());
    }

    /// Real device-list lookup against this machine's actual `osinfo-db`
    /// (same no-fixture posture as the rest of this module): a modern
    /// entry that declares virtio devices vs. a real pre-virtio entry
    /// (Windows XP, EOL 2014, predates virtio entirely) that doesn't —
    /// proves both the true and the false branch of `has_device_id`,
    /// not just the happy path.
    #[test]
    fn virtio_support_matches_upstream_device_ids() {
        let db = OsDb::load_default().expect("osinfo-db is installed on this machine");

        let fedora = db
            .os("http://fedoraproject.org/fedora/25")
            .expect("real entry");
        assert!(fedora.supports_virtiodisk());
        assert!(fedora.supports_virtioscsi());
        assert!(fedora.supports_virtionet());

        let winxp = db
            .os_by_id_or_short_id("winxp")
            .expect("winxp ships in every osinfo-db release");
        assert!(!winxp.supports_virtiodisk());
        assert!(!winxp.supports_virtioscsi());
        assert!(!winxp.supports_virtionet());
    }

    /// Real device-class-filter proof against the same two entries:
    /// Fedora 25's declared net devices include `virtio-net` (proving
    /// the `"net"`-class filter, not just id lookups); Windows XP (its
    /// own osinfo-db entry declares exactly one net device,
    /// `http://pcisig.com/pci/8086/100e`) resolves to `["e1000"]` —
    /// the real device upstream's own fallback chain in `interface.py`
    /// is built to find on a pre-virtio OS.
    #[test]
    fn supported_netmodels_matches_real_osinfo_db_device_classes() {
        let db = OsDb::load_default().expect("osinfo-db is installed on this machine");

        let fedora = db
            .os("http://fedoraproject.org/fedora/25")
            .expect("real entry");
        assert!(
            fedora
                .supported_netmodels()
                .iter()
                .any(|m| m == "virtio-net"),
            "fedora25 declares virtio-net among its net-class devices"
        );

        let winxp = db
            .os_by_id_or_short_id("winxp")
            .expect("winxp ships in every osinfo-db release");
        assert_eq!(
            winxp.supported_netmodels(),
            vec!["e1000".to_string()],
            "winxp's osinfo-db entry declares exactly one net device, e1000"
        );
    }

    /// Real firmware-requirement proof against this machine's actual
    /// `osinfo-db`: Windows 11 (`<firmware arch="x86_64" type="efi"/>`
    /// present, `<firmware arch="x86_64" type="bios" supported="false"/>`
    /// explicit) genuinely requires UEFI — a real hardware fact, not a
    /// stand-in, and one unlikely to ever change in osinfo-db unlike a
    /// permanently-archived EOL fixture. Fedora 25 (no `<firmware>`
    /// declarations at all) proves the `default` fallback branch: no
    /// declared opinion on either firmware type reads as "efi not
    /// supported, bios supported" (upstream's own `_supports_firmware_
    /// type` defaults), not "unknown" or a panic.
    #[test]
    fn requires_firmware_efi_matches_real_osinfo_db_firmware_declarations() {
        let db = OsDb::load_default().expect("osinfo-db is installed on this machine");

        let win11 = db
            .os_by_id_or_short_id("win11")
            .expect("win11 ships in every recent osinfo-db release");
        assert!(
            win11.requires_firmware_efi("x86_64"),
            "windows 11 genuinely requires UEFI on x86_64"
        );

        let fedora = db
            .os("http://fedoraproject.org/fedora/25")
            .expect("real entry");
        assert!(
            !fedora.requires_firmware_efi("x86_64"),
            "fedora25 declares no firmware requirements at all -- defaults to false"
        );
    }

    #[test]
    fn alphanum_key_sorts_newest_first() {
        let mut v = vec!["Fedora 9", "Fedora 40", "fedora 41", "Alpine"];
        v.sort_by_key(|s| alphanum_key(s));
        assert_eq!(v, ["Alpine", "fedora 41", "Fedora 40", "Fedora 9"]);
    }

    #[test]
    fn eol_rules() {
        let today = days_from_ymd("2026-10-26").unwrap();
        assert!(compute_eol("2020-01-01", "", "", today));
        assert!(!compute_eol("2030-01-01", "2000-01-01", "", today));
        assert!(!compute_eol("", "2000-01-01", "rolling", today));
        assert!(compute_eol("", "2010-01-01", "", today));
        assert!(!compute_eol("", "2020-01-01", "", today));
        assert!(!compute_eol("", "", "", today));
        assert_eq!(days_from_ymd("1970-01-01"), Some(0));
    }

    #[test]
    fn list_os_variants_puts_generic_last() {
        let db = OsDb::load_default().expect("osinfo-db loads");
        let list = db.list_os_variants();
        assert!(list.last().unwrap().always_show());
        assert!(list.iter().any(|o| o.is_generic()));
        let f25 = list.iter().find(|o| o.name == "fedora25").unwrap();
        assert!(f25.eol);
        assert_eq!(
            f25.full_id.as_deref(),
            Some("http://fedoraproject.org/fedora/25")
        );
    }

    #[test]
    fn recommended_resources_fall_back_to_double_the_minimum() {
        let v = |ram, n_cpus, storage| ResourceValues {
            ram,
            n_cpus,
            storage,
        };
        let res = OsResources {
            minimum: vec![("all".into(), v(1 << 30, 1, -1))],
            recommended: vec![("x86_64".into(), v(-1, 2, 20 << 30))],
        };
        assert_eq!(res.get_recommended_ram("x86_64"), Some(2 << 30));
        assert_eq!(res.get_recommended_ncpus("x86_64"), Some(2));
        assert_eq!(res.get_recommended_ncpus("aarch64"), Some(2));
        assert_eq!(res.get_recommended_storage("x86_64"), Some(20 << 30));
        assert_eq!(res.get_recommended_storage("aarch64"), None);
        assert_eq!(res.get_minimum_ram("x86_64"), Some(1 << 30));
    }

    #[test]
    fn real_db_has_resources_and_rejects_bogus_media() {
        let db = OsDb::load_default().expect("osinfo-db is installed on this machine");
        let fedora25 = db.os("http://fedoraproject.org/fedora/25").unwrap();
        let res = fedora25.get_recommended_resources();
        assert!(res.get_recommended_ram("x86_64").unwrap() >= 1 << 30);
        assert!(db.guess_os_by_iso("/nonexistent.iso").is_none());
        assert!(db.guess_os_by_tree("/nonexistent").is_none());
    }
}
