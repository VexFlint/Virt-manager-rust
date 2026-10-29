//! Upstream's `virtManager/details/snapshots.py` + `ui/snapshots.ui` +
//! `ui/snapshotsnew.ui`: the VM window's Snapshots page
//! (`vmmSnapshotPage`) and its "Create snapshot" dialog
//! (`vmmSnapshotNew`), plus the libvirt half of
//! `virtManager/object/domain.py` they drive (`vmmDomainSnapshot`,
//! `list_snapshots`, `get_current_snapshot`, `create_snapshot`,
//! `revert_to_snapshot`, `snapshots_supported`).
//!
//! The snapshot list is a `GtkListBox` with one rich row per snapshot
//! (ticket 11): state icon, name and state, and the "current" emblem,
//! with a separator row between the internal and external snapshots.
//! Every libvirt call runs on a worker thread (ticket 09); the
//! `<domainsnapshot>` XML is `virtinst_core::snapshot`.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use virt::connect::Connect;
use virt::domain::Domain;
use virt::domain_snapshot::DomainSnapshot as VirtSnapshot;
use virt::error::ErrorNumber;
use virt::sys;
use virtinst_core::snapshot::{self, DomainSnapshot};

use crate::asyncjob;
use crate::devpages::EntryCombo;
use crate::domain::{DomainAction, VmmDomain};
use crate::error;
use crate::libvirtenummap::{self as enummap, DomainState};

// ---------------------------------------------------------------------
// The libvirt half: `vmmDomainSnapshot` and `vmmDomain`'s snapshot API.
// ---------------------------------------------------------------------

/// `vmmDomainSnapshot`: one libvirt snapshot, as read off the domain.
/// The XML is replaced in place by [`Self::ensure_latest_xml`], like
/// upstream's cached `get_xmlobj()`.
#[derive(Debug)]
pub struct VmmDomainSnapshot {
    name: String,
    current: bool,
    /// `getXMLDesc(0)`, which a redefine starts from.
    xml: RefCell<String>,
    xmlobj: RefCell<DomainSnapshot>,
}

impl VmmDomainSnapshot {
    /// Blocking: reads one snapshot.
    fn fetch(snap: &VirtSnapshot) -> Result<VmmDomainSnapshot, virt::error::Error> {
        let xml = snap.get_xml_desc(0)?;
        Ok(VmmDomainSnapshot {
            name: snap.get_name()?,
            current: snap.is_current(0)?,
            xmlobj: RefCell::new(DomainSnapshot::parse(&xml).unwrap_or_default()),
            xml: RefCell::new(xml),
        })
    }

    pub fn get_name(&self) -> &str {
        &self.name
    }

    pub fn get_xmlobj(&self) -> std::cell::Ref<'_, DomainSnapshot> {
        self.xmlobj.borrow()
    }

    /// `ensure_latest_xml`, with the XML a worker re-read.
    fn ensure_latest_xml(&self, xml: String) {
        self.xmlobj
            .replace(DomainSnapshot::parse(&xml).unwrap_or_default());
        self.xml.replace(xml);
    }

    /// `_state_str_to_int`.
    fn state(&self) -> DomainState {
        state_str_to_int(self.get_xmlobj().state.as_deref())
    }

    pub fn run_status(&self) -> &'static str {
        enummap::pretty_run_status(self.state(), false)
    }

    pub fn run_status_icon_name(&self) -> &'static str {
        enummap::run_status_icon_name(self.state())
    }

    /// Captured state is a running domain.
    pub fn is_running(&self) -> bool {
        self.state() == sys::VIR_DOMAIN_RUNNING
    }

    /// Captured state contains run state in addition to disk state.
    pub fn has_run_state(&self) -> bool {
        matches!(
            self.state(),
            sys::VIR_DOMAIN_RUNNING | sys::VIR_DOMAIN_PAUSED
        )
    }

    pub fn is_current(&self) -> bool {
        self.current
    }

    pub fn is_external(&self) -> bool {
        self.get_xmlobj().is_external()
    }
}

/// `vmmDomainSnapshot._state_str_to_int`: libvirt's `<state>` text as
/// a domain state, shut off when unknown.
fn state_str_to_int(state: Option<&str>) -> DomainState {
    match state {
        Some("nostate") => sys::VIR_DOMAIN_NOSTATE,
        Some("running") => sys::VIR_DOMAIN_RUNNING,
        Some("blocked") => sys::VIR_DOMAIN_BLOCKED,
        Some("paused") => sys::VIR_DOMAIN_PAUSED,
        Some("shutdown") => sys::VIR_DOMAIN_SHUTDOWN,
        Some("shutoff") => sys::VIR_DOMAIN_SHUTOFF,
        Some("crashed") => sys::VIR_DOMAIN_CRASHED,
        Some("pmsuspended") => sys::VIR_DOMAIN_PMSUSPENDED,
        _ => sys::VIR_DOMAIN_SHUTOFF,
    }
}

/// Blocking: `vm.list_snapshots()` without the cache.
fn list_snapshots(dom: &Domain) -> Result<Vec<VmmDomainSnapshot>, virt::error::Error> {
    dom.list_all_snapshots(0)?
        .iter()
        .map(VmmDomainSnapshot::fetch)
        .collect()
}

/// Blocking: `get_current_snapshot`.
fn get_current_snapshot(dom: &Domain) -> Result<Option<VmmDomainSnapshot>, virt::error::Error> {
    let has = unsafe { sys::virDomainHasCurrentSnapshot(dom.as_ptr(), 0) };
    if has < 0 {
        return Err(virt::error::Error::last_error());
    }
    if has == 0 {
        return Ok(None);
    }
    let rawsnap = VirtSnapshot::current(dom, 0)?;
    VmmDomainSnapshot::fetch(&rawsnap).map(Some)
}

/// Blocking: `create_snapshot(xml, redefine, diskOnly)`.
fn create_snapshot(
    dom: &Domain,
    xml: &str,
    redefine: bool,
    disk_only: bool,
) -> Result<(), virt::error::Error> {
    let mut flags = 0;
    if redefine {
        flags |= sys::VIR_DOMAIN_SNAPSHOT_CREATE_REDEFINE;
    } else {
        if disk_only {
            flags |= sys::VIR_DOMAIN_SNAPSHOT_CREATE_DISK_ONLY;
        }
        eprintln!("Creating snapshot flags={flags} xml=\n{xml}");
    }
    let obj = VirtSnapshot::create_xml(dom, xml, flags)?;
    eprintln!(
        "returned new snapshot XML:\n{}",
        obj.get_xml_desc(0).unwrap_or_default()
    );
    Ok(())
}

/// Blocking: `vmmDomainSnapshot.delete`.
fn delete_snapshot(dom: &Domain, name: &str) -> Result<(), virt::error::Error> {
    VirtSnapshot::lookup_by_name(dom, name, 0)?.delete(0)
}

/// Blocking: `revert_to_snapshot`, minus the time setting.
fn revert_to_snapshot(dom: &Domain, name: &str) -> Result<(), virt::error::Error> {
    VirtSnapshot::lookup_by_name(dom, name, 0)?.revert(0)
}

/// `(error, details)` for a job, from libvirt's error.
fn job_err(e: virt::error::Error) -> asyncjob::JobError {
    let msg = e.message().to_string();
    (msg.clone(), msg)
}

/// Runs `f` on `vm`'s domain, looked up by UUID, on a worker thread.
async fn with_domain<T: Send + 'static>(
    vm: &VmmDomain,
    f: impl FnOnce(&Connect, &Domain) -> Result<T, virt::error::Error> + Send + 'static,
) -> Result<T, String> {
    if vm.is_virtinst() {
        return Err("The virtual machine isn't defined yet".into());
    }
    let handle = vm.handle()?;
    let uuid = vm.get_uuid();
    gio::spawn_blocking(move || {
        let conn = handle.connect();
        let dom = Domain::lookup_by_uuid_string(conn, &uuid)?;
        f(conn, &dom)
    })
    .await
    .map_err(|_| "Worker thread panicked".to_string())?
    .map_err(|e| e.message().to_string())
}

/// `vm.snapshots_supported()`: why snapshots can't be used, if they
/// can't. Like upstream's support check, only a "not supported" answer
/// to `listAllSnapshots` counts; a `vmmDomainVirtinst` always passes.
pub async fn snapshots_supported(vm: &VmmDomain) -> Option<String> {
    if vm.is_virtinst() {
        return None;
    }
    let supported = with_domain(vm, |_, dom| {
        Ok(match dom.list_all_snapshots(0) {
            Err(e) => !matches!(e.code(), ErrorNumber::NoSupport | ErrorNumber::Rpc),
            Ok(_) => true,
        })
    })
    .await
    .unwrap_or(true);
    (!supported).then(|| "Libvirt connection does not support snapshots.".to_string())
}

const CHANNEL_NAME_QEMUGA: &str = "org.qemu.guest_agent.0";

/// `_get_agent`'s channel in a domain XML: its `<target>` element.
fn agent_target(doc: &virtinst_xml::Document) -> Option<virtinst_xml::Element> {
    let root = doc.root_element()?;
    virtinst_xml::resolve_path(doc, root, &["devices"])?
        .find_all(doc, "channel")
        .into_iter()
        .filter(|c| c.attribute(doc, "type") == Some("unix"))
        .filter_map(|c| c.find(doc, "target"))
        .find(|t| t.attribute(doc, "name") == Some(CHANNEL_NAME_QEMUGA))
}

/// `has_agent`.
fn has_agent(vm: &VmmDomain) -> bool {
    vm.xmlobj().is_some_and(|x| agent_target(&x.doc).is_some())
}

/// `_async_set_time` (`_vmmDomainSetTimeThread`): set the guest clock
/// to the host's on a separate thread, for qemu (once its guest agent
/// is online, waiting up to 30s) and the test driver. Blocking callers
/// only; it returns at once.
fn async_set_time(
    handle: std::sync::Arc<crate::connection::ConnHandle>,
    uuid: String,
    is_qemu: bool,
    is_test: bool,
    has_agent: bool,
) {
    // Only run the API for qemu and test drivers, they are the only ones
    // that support it. This will save spamming logs with error output.
    if !is_qemu && !is_test {
        return;
    }
    // For qemu, only run the API if the VM has the qemu guest agent in
    // the XML.
    if is_qemu && !has_agent {
        return;
    }
    eprintln!("Starting time setting thread");
    std::thread::spawn(move || {
        let Ok(dom) = Domain::lookup_by_uuid_string(handle.connect(), &uuid) else {
            return;
        };
        // `agent_ready`: the test driver doesn't report 'connected'
        // state so hack it there.
        let agent_ready = || {
            is_test
                || dom
                    .get_xml_desc(0)
                    .ok()
                    .and_then(|xml| virtinst_xml::parse_libvirt_xml(&xml).ok())
                    .is_some_and(|doc| {
                        agent_target(&doc)
                            .is_some_and(|t| t.attribute(&doc, "state") == Some("connected"))
                    })
        };
        if is_qemu {
            // `_wait_for_agent`
            let (maxwait, sleep) = (30.0, 0.5);
            let mut waited = 0.0;
            while waited < maxwait && !agent_ready() {
                if waited == 0.0 {
                    eprintln!("Waiting for qemu guest agent to come online...");
                }
                std::thread::sleep(std::time::Duration::from_secs_f64(sleep));
                waited += sleep;
            }
            if !agent_ready() {
                eprintln!("Giving up on qemu guest agent for time sync");
                return;
            }
        }
        // `set_time`
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        match dom.set_time(now.as_secs() as i64, now.subsec_nanos() as i32, 0) {
            Ok(_) => eprintln!("Successfully set guest time"),
            Err(e) => eprintln!("Failed to set time: {}", e.message()),
        }
    });
}

// ---------------------------------------------------------------------
// Screenshots, kept in the VM's cache dir.
// ---------------------------------------------------------------------

/// `mimemap`.
const MIMEMAP: [(&str, &str); 2] = [("image/x-portable-pixmap", "ppm"), ("image/png", "png")];

/// `_mime_to_ext(val)`.
fn mime_to_ext(mime: &str) -> Option<&'static str> {
    let ret = MIMEMAP.iter().find(|(m, _)| *m == mime).map(|(_, e)| *e);
    if ret.is_none() {
        eprintln!("Don't know how to convert mime={mime} to extension");
    }
    ret
}

/// `_mime_to_ext(val, reverse=True)`.
fn ext_to_mime(ext: &str) -> Option<&'static str> {
    let ret = MIMEMAP.iter().find(|(_, e)| *e == ext).map(|(m, _)| *m);
    if ret.is_none() {
        eprintln!("Don't know how to convert extension={ext} to mime");
    }
    ret
}

/// `vm.get_cache_dir()`: `conn.get_cache_dir()` (the app cache dir
/// plus the URI) plus the VM's UUID, created if missing. The same
/// place upstream keeps its snapshot screenshots.
fn vm_cache_dir(vm: &VmmDomain) -> PathBuf {
    use std::os::unix::fs::DirBuilderExt;
    let uri = vm.conn().map(|c| c.get_uri()).unwrap_or_default();
    // `get_app_cache_dir`
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| glib::home_dir().join(".cache"));
    let ret = base
        .join("virt-manager")
        .join(uri.replace('/', "_"))
        .join(vm.get_uuid());
    let _ = std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o755)
        .create(&ret);
    ret
}

/// `<cachedir>/snap-screenshot-<name>`, which gets the image's
/// extension.
fn screenshot_basename(cache_dir: &Path, name: &str) -> PathBuf {
    cache_dir.join(format!("snap-screenshot-{name}"))
}

fn with_ext(base: &Path, ext: &str) -> PathBuf {
    let mut p = base.as_os_str().to_owned();
    p.push(format!(".{ext}"));
    PathBuf::from(p)
}

/// The screenshot half of `_do_create_snapshot`: drop any old
/// screenshot under this name, then save the new one. Errors are only
/// logged.
fn save_screenshot(cache_dir: &Path, name: &str, screenshot: Option<(String, Vec<u8>)>) {
    let basesn = screenshot_basename(cache_dir, name);
    // Remove any pre-existing screenshots so we don't show stale data
    for (_, ext) in MIMEMAP {
        let p = with_ext(&basesn, ext);
        if p.exists() {
            if let Err(e) = std::fs::remove_file(&p) {
                eprintln!("Error saving screenshot: {e}");
                return;
            }
        }
    }
    let Some((mime, sndata)) = screenshot else {
        return;
    };
    let Some(ext) = mime_to_ext(&mime) else {
        return;
    };
    let filename = with_ext(&basesn, ext);
    eprintln!("Writing screenshot to {}", filename.display());
    if let Err(e) = std::fs::write(&filename, sndata) {
        eprintln!("Error saving screenshot: {e}");
    }
}

/// `_read_screenshot_file`'s `glob(basename + ".*")[0]`.
fn find_screenshot_file(cache_dir: &Path, name: &str) -> Option<PathBuf> {
    let prefix = format!("snap-screenshot-{name}.");
    let mut files: Vec<PathBuf> = std::fs::read_dir(cache_dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
        .map(|e| e.path())
        .collect();
    files.sort();
    files.into_iter().next()
}

/// `_make_screenshot_pixbuf`'s `_scale`: the larger side at most 450.
fn screenshot_scale(width: i32, height: i32) -> (i32, i32) {
    const MAXSIZE: i32 = 450;
    let scale = |big: i32, small: i32| {
        if big <= MAXSIZE {
            return (big, small);
        }
        let factor = MAXSIZE as f64 / big as f64;
        (MAXSIZE, (factor * small as f64) as i32)
    };
    if width > height {
        scale(width, height)
    } else {
        let (h, w) = scale(height, width);
        (w, h)
    }
}

/// `_make_screenshot_pixbuf`.
fn make_screenshot_pixbuf(mime: &str, sdata: &[u8]) -> Option<gtk::gdk_pixbuf::Pixbuf> {
    let loader = gtk::gdk_pixbuf::PixbufLoader::with_mime_type(mime).ok()?;
    loader.write(sdata).ok()?;
    loader.close().ok()?;
    let pixbuf = loader.pixbuf()?;
    let (width, height) = screenshot_scale(pixbuf.width(), pixbuf.height());
    pixbuf.scale_simple(width, height, gtk::gdk_pixbuf::InterpType::Bilinear)
}

/// Blocking: `vmmSnapshotNew._take_screenshot`, `(mime, data)`.
fn take_screenshot(conn: &Connect, dom: &Domain) -> Result<(String, Vec<u8>), virt::error::Error> {
    let stream = virt::stream::Stream::new(conn, 0)?;
    let mime = dom.screenshot(&stream, 0, 0)?;
    let mut ret = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let got = stream.recv(&mut buf)?;
        if got == 0 {
            break;
        }
        ret.extend_from_slice(&buf[..got]);
    }
    let _ = stream.finish();
    Ok((mime, ret))
}

// ---------------------------------------------------------------------
// The New Snapshot dialog's rules, without the widgets.
// ---------------------------------------------------------------------

/// `_get_mode()`'s two answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SnapshotMode {
    Internal,
    External,
}

impl SnapshotMode {
    /// `_get_current_mode` for the current snapshot.
    fn of(snap: &VmmDomainSnapshot) -> SnapshotMode {
        if snap.is_external() {
            SnapshotMode::External
        } else {
            SnapshotMode::Internal
        }
    }
}

/// `_reset_snapshot_mode`: external when the connection can do it,
/// unless the current snapshot is an internal one.
fn default_mode(external_supported: bool, current: Option<SnapshotMode>) -> SnapshotMode {
    if external_supported && current != Some(SnapshotMode::Internal) {
        SnapshotMode::External
    } else {
        SnapshotMode::Internal
    }
}

/// `_set_memory_path_visibility`: the memory state row is for external
/// snapshots of a running VM.
fn memory_path_visible(mode: SnapshotMode, vm_active: bool) -> bool {
    mode == SnapshotMode::External && vm_active
}

/// `_create_new_snapshot`'s `diskOnly`: an external snapshot of a
/// stopped VM has no memory to save.
fn disk_only(mode: SnapshotMode, vm_active: bool) -> bool {
    !vm_active && mode == SnapshotMode::External
}

/// `_reset_state`'s default name: `snapshot1`, `snapshot2`, ...
fn default_snapshot_name(collides: impl Fn(&str) -> bool) -> String {
    virtinst_core::cloner::generate_name("snapshot", collides, "", 1, "", true)
}

/// `_populate_memory_path`'s choices: `<vm>-mem.<snapshot>` next to
/// each disk, without repeats.
fn memory_paths(vm_name: &str, snap_name: &str, disk_paths: &[String]) -> Vec<String> {
    let memname = format!("{vm_name}-mem.{snap_name}");
    let mut paths: Vec<String> = Vec::new();
    for diskpath in disk_paths.iter().filter(|p| !p.is_empty()) {
        let p = Path::new(diskpath);
        let newpath = p
            .parent()
            .unwrap_or(p)
            .join(&memname)
            .to_string_lossy()
            .into_owned();
        if !paths.contains(&newpath) {
            paths.push(newpath);
        }
    }
    paths
}

/// `_validate_new_snapshot`'s snapshot, before the error dialog.
fn build_new_snapshot(
    name: &str,
    desc: &str,
    mode: SnapshotMode,
    vm_active: bool,
    mempath: Option<String>,
) -> Result<DomainSnapshot, String> {
    let mut newsnap = DomainSnapshot {
        name: Some(name.to_string()),
        description: (!desc.is_empty()).then(|| desc.to_string()),
        ..Default::default()
    };
    if mode == SnapshotMode::External && vm_active {
        newsnap.memory_type = Some("external".into());
        newsnap.memory_file = mempath;
    }
    newsnap.get_xml()?;
    crate::domain::validate_generic_name("Snapshot", name)?;
    Ok(newsnap)
}

/// `_populate_snapshot_list`'s row label markup and sort key.
fn list_row_label(name: &str, state: &str, external: bool) -> (String, String) {
    let name_esc = glib::markup_escape_text(name);
    let state_esc = glib::markup_escape_text(state);
    if external {
        (
            format!("{name_esc}\n<span size='small'>VM State: {state_esc} (External)</span>"),
            format!("3{name}"),
        )
    } else {
        (
            format!("{name_esc}\n<span size='small'>VM State: {state_esc}</span>"),
            format!("1{name}"),
        )
    }
}

/// `model.set_sort_column_id(4, Gtk.SortType.ASCENDING)`: GtkListStore
/// compares the sort keys with `g_utf8_collate`, in the user's locale
/// (`alpha` before `Backup`), not byte by byte.
fn sort_by_sortname<T>(entries: &mut [(String, T)]) {
    entries.sort_by_cached_key(|(sortname, _)| glib::CollationKey::from(sortname));
}

/// `_set_snapshot_state`'s "Snapshot Mode:" text for an external
/// snapshot. Upstream's `is_disk` is a list of per-disk booleans, so
/// any `<disk>` at all counts as an external disk; kept as is.
fn external_mode_label(xmlobj: &DomainSnapshot) -> &'static str {
    let is_mem = xmlobj.memory_type.as_deref() == Some("external");
    let is_disk = !xmlobj.disks.is_empty();
    if is_mem && is_disk {
        "External disk and memory"
    } else if is_mem {
        "External memory only"
    } else {
        "External disk only"
    }
}

/// `str(datetime.datetime.fromtimestamp(creationTime))`.
fn format_timestamp(t: u32) -> String {
    glib::DateTime::from_unix_local(t as i64)
        .and_then(|d| d.format("%Y-%m-%d %H:%M:%S"))
        .map(|s| s.to_string())
        .unwrap_or_default()
}

/// The disk paths of the VM's XML, for the memory path choices.
fn vm_disk_paths(vm: &VmmDomain) -> Vec<String> {
    let Some(x) = vm.xmlobj() else {
        return vec![];
    };
    let doc = &x.doc;
    virtinst_xml::resolve_path(doc, x.root(), &["devices"])
        .map(|d| d.find_all(doc, "disk"))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|d| virtinst_core::devices::disk::source_path(doc, d))
        .collect()
}

/// `vm.xmlobj.devices.graphics`, non-empty.
fn vm_has_graphics(vm: &VmmDomain) -> bool {
    vm.xmlobj().is_some_and(|x| {
        virtinst_xml::resolve_path(&x.doc, x.root(), &["devices", "graphics"]).is_some()
    })
}

fn grid_label(text: &str) -> gtk::Label {
    let l = gtk::Label::with_mnemonic(text);
    l.set_halign(gtk::Align::Start);
    l
}

/// `bind_escape_key_close`.
fn bind_escape_key_close(win: &gtk::Window, close: impl Fn() + 'static) {
    let esc = gtk::EventControllerKey::new();
    esc.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape {
            close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    win.add_controller(esc);
}

// ---------------------------------------------------------------------
// `vmmSnapshotNew`
// ---------------------------------------------------------------------

/// The `snapshot-created` handler: the new snapshot's name.
type CreatedCallback = Box<dyn Fn(&str)>;

pub struct NewInner {
    vm: VmmDomain,
    window: gtk::Window,
    name: gtk::Entry,
    description: gtk::TextView,
    status_icon: gtk::Image,
    status_text: gtk::Label,
    mode_external: gtk::CheckButton,
    mode_internal: gtk::CheckButton,
    memory_label: gtk::Label,
    memory_box: gtk::Box,
    memory_auto: gtk::CheckButton,
    memory_path: Rc<EntryCombo>,
    ok: gtk::Button,
    /// `_init_snapshot_mode` ran: it needs the host capabilities.
    mode_initialized: Cell<bool>,
    /// Set while Finish is being handled, so it runs once.
    creating: Cell<bool>,
    snapshot_created: RefCell<Option<CreatedCallback>>,
}

/// `vmmSnapshotNew`: the "Create snapshot" dialog.
#[derive(Clone)]
pub struct SnapshotNew(Rc<NewInner>);

impl std::ops::Deref for SnapshotNew {
    type Target = NewInner;
    fn deref(&self) -> &NewInner {
        &self.0
    }
}

impl SnapshotNew {
    pub fn new(vm: &VmmDomain) -> SnapshotNew {
        let window = gtk::Window::builder()
            .title("Create snapshot")
            .hide_on_close(true)
            .build();

        // header
        let image = gtk::Image::from_icon_name("document-new");
        image.set_pixel_size(48);
        let title = gtk::Label::new(None);
        title.set_markup("<span size='large'>Create snapshot</span>");
        title.set_halign(gtk::Align::Start);
        title.set_hexpand(true);
        title.add_css_class("vmm-header-text");
        let hbox77 = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        hbox77.set_margin_start(6);
        hbox77.set_margin_end(6);
        hbox77.set_margin_top(6);
        hbox77.set_margin_bottom(6);
        hbox77.append(&image);
        hbox77.append(&title);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        header.add_css_class("vmm-header");
        header.append(&hbox77);

        // snapshot-new-box
        let name = gtk::Entry::new();
        let name_label = grid_label("_Name:");
        name_label.set_mnemonic_widget(Some(&name));

        let status_icon = gtk::Image::new();
        let status_text = gtk::Label::new(Some("Shut down"));
        status_text.set_halign(gtk::Align::Start);
        let status_box = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        status_box.append(&status_icon);
        status_box.append(&status_text);

        let mode_external = gtk::CheckButton::with_mnemonic("_external");
        let mode_internal = gtk::CheckButton::with_mnemonic("_internal");
        mode_internal.set_group(Some(&mode_external));
        // Both are  in the .ui; the group keeps the last one.
        mode_external.set_active(true);
        mode_internal.set_active(true);
        let mode_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        mode_box.set_homogeneous(true);
        mode_box.set_halign(gtk::Align::Start);
        mode_box.append(&mode_external);
        mode_box.append(&mode_internal);
        let mode_label = grid_label("Snapshot _Mode:");

        let memory_auto = gtk::CheckButton::with_mnemonic("_auto");
        // `_init_memory_path`
        let memory_path = EntryCombo::new(vec![]);
        memory_path.top.set_hexpand(true);
        memory_path.top.set_halign(gtk::Align::Fill);
        memory_path.entry.set_hexpand(true);
        let memory_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        memory_box.append(&memory_auto);
        memory_box.append(&memory_path.top);
        let memory_label = grid_label("Memory _State:");

        // `_init_ui`: a fresh buffer
        let description = gtk::TextView::new();
        let desc_scroll = gtk::ScrolledWindow::builder()
            .child(&description)
            .has_frame(true)
            .width_request(300)
            .height_request(125)
            .build();
        let desc_label = grid_label("_Description:");
        desc_label.set_valign(gtk::Align::Start);
        desc_label.set_mnemonic_widget(Some(&description));

        let grid = gtk::Grid::new();
        grid.set_row_spacing(6);
        grid.set_column_spacing(6);
        grid.attach(&name_label, 0, 0, 1, 1);
        grid.attach(&name, 1, 0, 1, 1);
        grid.attach(&grid_label("Status:"), 0, 1, 1, 1);
        grid.attach(&status_box, 1, 1, 1, 1);
        grid.attach(&mode_label, 0, 2, 1, 1);
        grid.attach(&mode_box, 1, 2, 1, 1);
        grid.attach(&memory_label, 0, 3, 1, 1);
        grid.attach(&memory_box, 1, 3, 1, 1);
        grid.attach(&desc_label, 0, 4, 1, 1);
        grid.attach(&desc_scroll, 1, 4, 1, 1);
        let box_align = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        box_align.set_vexpand(true);
        box_align.append(&grid);

        let cancel = gtk::Button::with_mnemonic("_Cancel");
        let ok = gtk::Button::with_mnemonic("_Finish");
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.set_halign(gtk::Align::End);
        buttons.append(&cancel);
        buttons.append(&ok);

        let box3 = gtk::Box::new(gtk::Orientation::Vertical, 18);
        box3.set_margin_start(12);
        box3.set_margin_end(12);
        box3.set_margin_top(12);
        box3.set_margin_bottom(12);
        box3.set_vexpand(true);
        box3.append(&box_align);
        box3.append(&buttons);
        let box1 = gtk::Box::new(gtk::Orientation::Vertical, 6);
        box1.append(&header);
        box1.append(&box3);
        window.set_child(Some(&box1));

        let this = SnapshotNew(Rc::new(NewInner {
            vm: vm.clone(),
            window,
            name,
            description,
            status_icon,
            status_text,
            mode_external,
            mode_internal,
            memory_label,
            memory_box,
            memory_auto,
            memory_path,
            ok,
            mode_initialized: Cell::new(false),
            creating: Cell::new(false),
            snapshot_created: RefCell::default(),
        }));

        let cb = this.with(SnapshotNew::close);
        cancel.connect_clicked(move |_| cb());
        let cb = this.with(SnapshotNew::name_changed);
        this.name.connect_changed(move |_| cb());
        let cb = this.with(|t| t.spawn(|t| async move { t.ok_clicked().await }));
        this.name.connect_activate(move |_| cb());
        let cb = this.with(|t| t.spawn(|t| async move { t.ok_clicked().await }));
        this.ok.connect_clicked(move |_| cb());
        let cb = this.with(SnapshotNew::set_memory_path_visibility);
        this.mode_external.connect_toggled(move |_| cb());
        let cb = this.with(SnapshotNew::memory_toggled);
        this.memory_auto.connect_toggled(move |_| cb());
        let cb = this.with(SnapshotNew::close);
        bind_escape_key_close(&this.window, cb);
        this
    }

    fn weak(&self) -> Weak<NewInner> {
        Rc::downgrade(&self.0)
    }

    fn with(&self, f: impl Fn(&SnapshotNew) + 'static) -> impl Fn() + 'static {
        let weak = self.weak();
        move || {
            if let Some(inner) = weak.upgrade() {
                f(&SnapshotNew(inner));
            }
        }
    }

    fn spawn<F: std::future::Future<Output = ()> + 'static>(
        &self,
        f: impl FnOnce(SnapshotNew) -> F + 'static,
    ) {
        let this = self.clone();
        glib::spawn_future_local(async move { f(this).await });
    }

    /// The `snapshot-created` signal.
    pub fn connect_snapshot_created(&self, cb: impl Fn(&str) + 'static) {
        *self.snapshot_created.borrow_mut() = Some(Box::new(cb));
    }

    /// `show`.
    pub async fn show(&self, parent: &gtk::Window) {
        eprintln!("Showing new snapshot wizard");
        if !self.mode_initialized.get() {
            self.init_snapshot_mode().await;
            self.mode_initialized.set(true);
        }
        self.reset_state().await;
        // `topwin.resize(1, 1)`
        self.window.set_default_size(1, 1);
        self.window.set_transient_for(Some(parent));
        self.window.present();
        self.name.grab_focus();
    }

    /// `close`.
    pub fn close(&self) {
        eprintln!("Closing new snapshot wizard");
        self.window.set_visible(false);
    }

    /// `_cleanup`.
    pub fn cleanup(&self) {
        self.window.destroy();
    }

    /// `_init_snapshot_mode`: external snapshots need the host's
    /// `<externalSnapshot/>` guest feature.
    async fn init_snapshot_mode(&self) {
        let x = self.vm.xmlobj();
        let supported = match (self.vm.conn(), x) {
            (Some(conn), Some(x)) => conn.fetch_caps().await.is_ok_and(|caps| {
                let g = &x.guest;
                caps.guest_lookup_guest(
                    g.os.os_type.as_deref(),
                    g.os.arch.as_deref(),
                    g.domain_type.as_deref(),
                    g.os.machine.as_deref(),
                )
                .is_ok_and(|guest| guest.supports_external_snapshot())
            }),
            _ => false,
        };
        if !supported {
            self.mode_external.set_sensitive(false);
            self.mode_external.set_tooltip_text(Some(
                "external snapshots not supported with this libvirt connection",
            ));
        }
    }

    /// `_reset_state`, with its libvirt lookups on a worker.
    async fn reset_state(&self) {
        let (default_name, current_mode) = with_domain(&self.vm, |_, dom| {
            let name = default_snapshot_name(|n| VirtSnapshot::lookup_by_name(dom, n, 0).is_ok());
            let current = get_current_snapshot(dom)
                .ok()
                .flatten()
                .map(|s| SnapshotMode::of(&s));
            Ok((name, current))
        })
        .await
        .unwrap_or_else(|_| ("snapshot1".to_string(), None));

        self.name.set_text(&default_name);
        self.name_changed();
        self.description.buffer().set_text("");
        self.status_text.set_text(self.vm.run_status());
        self.status_icon
            .set_icon_name(Some(self.vm.run_status_icon_name()));
        self.reset_snapshot_mode(current_mode);
        self.reset_snapshot_memory_path();
    }

    /// `_reset_snapshot_mode`.
    fn reset_snapshot_mode(&self, current_mode: Option<SnapshotMode>) {
        match default_mode(self.mode_external.is_sensitive(), current_mode) {
            SnapshotMode::External => self.mode_external.set_active(true),
            SnapshotMode::Internal => self.mode_internal.set_active(true),
        }
    }

    /// `_reset_snapshot_memory_path`.
    fn reset_snapshot_memory_path(&self) {
        self.set_memory_path_visibility();
        self.memory_auto.set_active(true);
    }

    /// `_set_memory_path_visibility`: `uiutil.set_grid_row_visible`.
    fn set_memory_path_visibility(&self) {
        let visible = memory_path_visible(self.get_mode(), self.vm.is_active());
        self.memory_label.set_visible(visible);
        self.memory_box.set_visible(visible);
    }

    /// `_populate_memory_path`: only while "auto" picks the path.
    fn populate_memory_path(&self) {
        if self.memory_path.top.is_sensitive() {
            return;
        }
        let paths = memory_paths(
            &self.vm.get_name(),
            &self.name.text(),
            &vm_disk_paths(&self.vm),
        );
        self.memory_path
            .set_rows(paths.into_iter().map(|p| (Some(p.clone()), p)).collect());
        self.memory_path.set_active(0);
    }

    /// `_get_mode`.
    fn get_mode(&self) -> SnapshotMode {
        if self.mode_external.is_active() {
            SnapshotMode::External
        } else {
            SnapshotMode::Internal
        }
    }

    /// `_name_changed_cb`.
    fn name_changed(&self) {
        self.ok.set_sensitive(!self.name.text().is_empty());
        self.populate_memory_path();
    }

    /// `_memory_toggled_cb`.
    fn memory_toggled(&self) {
        self.memory_path
            .top
            .set_sensitive(!self.memory_auto.is_active());
        self.populate_memory_path();
    }

    /// `_ok_clicked_cb`. Upstream blocks the main loop until the job
    /// starts; here a second Enter or Finish meanwhile is ignored.
    async fn ok_clicked(&self) {
        if self.creating.replace(true) {
            return;
        }
        self.ok_clicked_inner().await;
        self.creating.set(false);
    }

    /// The rest of `_ok_clicked_cb`: warn before mixing internal and
    /// external snapshots.
    async fn ok_clicked_inner(&self) {
        let current_mode = with_domain(&self.vm, |_, dom| get_current_snapshot(dom))
            .await
            .ok()
            .flatten()
            .map(|s| SnapshotMode::of(&s));
        if current_mode.is_some_and(|m| m != self.get_mode()) {
            let result = error::yes_no(
                Some(&self.window),
                "Mixing external and internal snapshots for the same VM is not \
                 recommended. Are you sure you want to continue?",
                None,
            )
            .await;
            if !result {
                return;
            }
        }
        self.create_new_snapshot().await;
    }

    /// `_validate_new_snapshot`.
    async fn validate_new_snapshot(&self) -> Option<DomainSnapshot> {
        let buf = self.description.buffer();
        let desc = buf.text(&buf.start_iter(), &buf.end_iter(), true);
        let mode = self.get_mode();
        let vm_active = self.vm.is_active();
        let mempath = (mode == SnapshotMode::External && vm_active)
            .then(|| self.memory_path.get())
            .flatten();
        match build_new_snapshot(&self.name.text(), &desc, mode, vm_active, mempath) {
            Ok(snap) => Some(snap),
            Err(e) => {
                error::val_err(
                    Some(&self.window),
                    &format!("Error validating snapshot: {e}"),
                )
                .await;
                None
            }
        }
    }

    /// `_get_screenshot`: a running VM's display, as `(mime, data)`.
    async fn get_screenshot(&self) -> Option<(String, Vec<u8>)> {
        if !self.vm.is_active() {
            eprintln!("Skipping screenshot since VM is not active");
            return None;
        }
        if !vm_has_graphics(&self.vm) {
            eprintln!("Skipping screenshot since VM has no graphics");
            return None;
        }
        let res = with_domain(&self.vm, |conn, dom| {
            // Perform two screenshots, because qemu + qxl has a bug where
            // screenshot generally only shows the data from the previous
            // screenshot request:
            // https://bugs.launchpad.net/qemu/+bug/1314293
            take_screenshot(conn, dom)?;
            take_screenshot(conn, dom)
        })
        .await;
        let (mime, sdata) = match res {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Error taking screenshot: {e}");
                return None;
            }
        };
        mime_to_ext(&mime)?;
        Some((mime, sdata))
    }

    /// `_create_new_snapshot` + `_do_create_snapshot` +
    /// `_new_finish_cb`.
    async fn create_new_snapshot(&self) {
        let Some(snap) = self.validate_new_snapshot().await else {
            return;
        };
        let Ok(xml) = snap.get_xml() else {
            return;
        };
        let name = snap.name.clone().unwrap_or_default();
        let screenshot = self.get_screenshot().await;
        let disk_only = disk_only(self.get_mode(), self.vm.is_active());
        let Ok(handle) = self.vm.handle() else {
            return;
        };
        let uuid = self.vm.get_uuid();
        let cache_dir = vm_cache_dir(&self.vm);

        self.window.set_cursor_from_name(Some("progress"));
        let jobname = name.clone();
        let err = asyncjob::run(
            Some(&self.window),
            "Creating snapshot",
            "Creating virtual machine snapshot",
            true,
            move |_job| {
                let dom =
                    Domain::lookup_by_uuid_string(handle.connect(), &uuid).map_err(job_err)?;
                create_snapshot(&dom, &xml, false, disk_only).map_err(job_err)?;
                save_screenshot(&cache_dir, &jobname, screenshot);
                Ok(())
            },
        )
        .await;
        // `_new_finish_cb`
        self.window.set_cursor(None);
        if let Some((error, details)) = err {
            error::show_err(
                Some(&self.window),
                &format!("Error creating snapshot: {error}"),
                Some(&details),
                "",
            )
            .await;
            return;
        }
        if let Some(cb) = self.snapshot_created.borrow().as_ref() {
            cb(&name);
        }
        self.close();
    }
}

// ---------------------------------------------------------------------
// `vmmSnapshotPage`
// ---------------------------------------------------------------------

/// The `notebook` pages of `snapshot-notebook`.
const PAGE_DETAILS: &str = "details";
const PAGE_ERROR: &str = "error";

pub struct PageInner {
    vm: VmmDomain,
    /// The VM window: `topwin`, every dialog's parent.
    topwin: gtk::Window,
    /// `snapshot-top-box`.
    top_box: gtk::Box,
    list: gtk::ListBox,
    /// The snapshot rows in list order (separator excluded).
    rows: RefCell<Vec<(gtk::ListBoxRow, Rc<VmmDomainSnapshot>)>>,
    notebook: gtk::Stack,
    error_label: gtk::Label,
    title: gtk::Label,
    is_current: gtk::Box,
    timestamp: gtk::Label,
    status_icon: gtk::Image,
    status_text: gtk::Label,
    mode_label: gtk::Label,
    mode: gtk::Label,
    description: gtk::TextView,
    screenshot: gtk::Picture,
    screenshot_label: gtk::Label,
    add: gtk::Button,
    start: gtk::Button,
    delete: gtk::Button,
    apply: gtk::Button,
    menu: gtk::PopoverMenu,
    /// `vm._snapshot_list`: `None` until fetched, and after a refresh.
    snapshot_list: RefCell<Option<Vec<Rc<VmmDomainSnapshot>>>>,
    initial_populate: Cell<bool>,
    unapplied_changes: Cell<bool>,
    snapshot_new: RefCell<Option<SnapshotNew>>,
    /// The snapshot the details show, which unapplied changes belong to.
    shown: RefCell<Option<Rc<VmmDomainSnapshot>>>,
    /// Set while the list is refilled, so it isn't a selection change.
    quiet: Cell<bool>,
    /// Bumped per list fetch, so a stale one is dropped.
    generation: Cell<u64>,
    populating: Cell<bool>,
}

/// `vmmSnapshotPage`: the VM window's Snapshots page.
#[derive(Clone)]
pub struct VmmSnapshotPage(Rc<PageInner>);

impl std::ops::Deref for VmmSnapshotPage {
    type Target = PageInner;
    fn deref(&self) -> &PageInner {
        &self.0
    }
}

fn page_button(icon: &str, tooltip: &str, accessible: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.set_tooltip_text(Some(tooltip));
    b.update_property(&[gtk::accessible::Property::Label(accessible)]);
    b
}

impl VmmSnapshotPage {
    pub fn new(vm: &VmmDomain, topwin: &gtk::Window) -> VmmSnapshotPage {
        // snapshot-list, in scrolledwindow7
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Multiple);
        list.set_activate_on_single_click(false);
        // `col.set_min_width(150)`
        list.set_size_request(150, -1);
        list.update_property(&[gtk::accessible::Property::Label("snapshot-list")]);
        let list_scroll = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .has_frame(true)
            .build();

        // sgrid1
        let title = gtk::Label::new(None);
        title.set_halign(gtk::Align::Start);
        title.set_markup("<b>snapshot 'foo'</b>");
        let current_icon = gtk::Image::from_icon_name("dialog-information");
        current_icon.set_pixel_size(16);
        let current_label = gtk::Label::new(None);
        current_label.set_halign(gtk::Align::Start);
        current_label
            .set_markup("<small><i>This was the most recently applied snapshot.</i></small>");
        let is_current = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        is_current.append(&current_icon);
        is_current.append(&current_label);
        let timestamp = gtk::Label::new(Some("label"));
        timestamp.set_halign(gtk::Align::Start);
        let status_icon = gtk::Image::from_icon_name("image-missing");
        let status_text = gtk::Label::new(Some("Shut down"));
        status_text.set_halign(gtk::Align::Start);
        let status_box = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        status_box.append(&status_icon);
        status_box.append(&status_text);
        let mode_label = grid_label("Snapshot Mode:");
        let mode = gtk::Label::new(Some("label"));
        mode.set_halign(gtk::Align::Start);
        let description = gtk::TextView::new();
        description.set_left_margin(2);
        description.set_right_margin(2);
        description.update_property(&[gtk::accessible::Property::Label("snapshot-description")]);
        let desc_scroll = gtk::ScrolledWindow::builder()
            .child(&description)
            .has_frame(true)
            .height_request(80)
            .hexpand(true)
            .build();
        let desc_label = grid_label("Description:");
        desc_label.set_valign(gtk::Align::Start);
        let screenshot = gtk::Picture::new();
        screenshot.set_can_shrink(false);
        screenshot.set_halign(gtk::Align::Start);
        screenshot.set_valign(gtk::Align::Start);
        let screenshot_label = gtk::Label::new(Some("No screenshot available"));
        let box5 = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        box5.append(&screenshot);
        box5.append(&screenshot_label);
        let screenshot_title = grid_label("Screenshot:");
        screenshot_title.set_valign(gtk::Align::Start);

        let grid = gtk::Grid::new();
        grid.set_row_spacing(6);
        grid.set_column_spacing(6);
        grid.attach(&title, 0, 0, 2, 1);
        grid.attach(&is_current, 0, 1, 2, 1);
        grid.attach(&grid_label("Timestamp:"), 0, 2, 1, 1);
        grid.attach(&timestamp, 1, 2, 1, 1);
        grid.attach(&grid_label("VM State:"), 0, 3, 1, 1);
        grid.attach(&status_box, 1, 3, 1, 1);
        grid.attach(&mode_label, 0, 4, 1, 1);
        grid.attach(&mode, 1, 4, 1, 1);
        grid.attach(&desc_label, 0, 5, 1, 1);
        grid.attach(&desc_scroll, 1, 5, 1, 1);
        grid.attach(&screenshot_title, 0, 6, 1, 1);
        grid.attach(&box5, 1, 6, 1, 1);
        let sbox3 = gtk::Box::new(gtk::Orientation::Vertical, 12);
        sbox3.append(&grid);

        // snapshot-notebook, tabs hidden: the details or an error
        let error_label = gtk::Label::new(Some("error label"));
        error_label.update_property(&[gtk::accessible::Property::Label("snapshot-error-label")]);
        let notebook = gtk::Stack::new();
        notebook.set_margin_start(12);
        notebook.add_named(&sbox3, Some(PAGE_DETAILS));
        notebook.add_named(&error_label, Some(PAGE_ERROR));

        let pane = gtk::Paned::new(gtk::Orientation::Horizontal);
        pane.set_start_child(Some(&list_scroll));
        pane.set_end_child(Some(&notebook));
        pane.set_resize_start_child(false);
        pane.set_shrink_start_child(false);
        pane.set_resize_end_child(true);
        pane.set_shrink_end_child(true);
        pane.set_position(200);
        pane.set_vexpand(true);

        // box2: the snapshot buttons, then Apply
        let add = page_button("list-add", "Create new snapshot", "snapshot-add");
        let start = page_button(
            "media-playback-start",
            "Run selected snapshot",
            "snapshot-start",
        );
        let refresh = page_button("view-refresh", "Refresh snapshot list", "snapshot-refresh");
        let delete = page_button("edit-delete", "Delete selected snapshot", "snapshot-delete");
        let hbox10 = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        hbox10.set_hexpand(true);
        for b in [&add, &start, &refresh, &delete] {
            hbox10.append(b);
        }
        let apply = gtk::Button::with_mnemonic("_Apply");
        apply.set_tooltip_text(Some("Save updated snapshot metadata"));
        apply.update_property(&[gtk::accessible::Property::Label("snapshot-apply")]);
        let buttonbox1 = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttonbox1.set_halign(gtk::Align::End);
        buttonbox1.append(&apply);
        let box2 = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        box2.append(&hbox10);
        box2.append(&buttonbox1);

        let top_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        top_box.set_hexpand(true);
        top_box.set_vexpand(true);
        top_box.set_margin_start(12);
        top_box.set_margin_end(12);
        top_box.set_margin_top(12);
        top_box.set_margin_bottom(12);
        top_box.append(&pane);
        top_box.append(&box2);

        // Snapshot popup menu. Not parented to the list itself, which
        // treats every child as a row.
        let menu_model = gio::Menu::new();
        menu_model.append(Some("_Start snapshot"), Some("snapshot.start"));
        menu_model.append(Some("_Delete snapshot"), Some("snapshot.delete"));
        let menu = gtk::PopoverMenu::from_model(Some(&menu_model));
        menu.set_parent(&top_box);
        menu.set_has_arrow(false);
        menu.set_halign(gtk::Align::Start);

        let this = VmmSnapshotPage(Rc::new(PageInner {
            vm: vm.clone(),
            topwin: topwin.clone(),
            top_box,
            list,
            rows: RefCell::default(),
            notebook,
            error_label,
            title,
            is_current,
            timestamp,
            status_icon,
            status_text,
            mode_label,
            mode,
            description,
            screenshot,
            screenshot_label,
            add,
            start,
            delete,
            apply,
            menu,
            snapshot_list: RefCell::default(),
            initial_populate: Cell::new(false),
            unapplied_changes: Cell::new(false),
            snapshot_new: RefCell::default(),
            shown: RefCell::default(),
            quiet: Cell::new(false),
            generation: Cell::new(0),
            populating: Cell::new(false),
        }));
        this.connect_signals(&refresh);
        // `selection.emit("changed")`
        this.snapshot_selected();
        this
    }

    fn weak(&self) -> Weak<PageInner> {
        Rc::downgrade(&self.0)
    }

    fn with(&self, f: impl Fn(&VmmSnapshotPage) + 'static) -> impl Fn() + 'static {
        let weak = self.weak();
        move || {
            if let Some(inner) = weak.upgrade() {
                f(&VmmSnapshotPage(inner));
            }
        }
    }

    fn spawn<F: std::future::Future<Output = ()> + 'static>(
        &self,
        f: impl FnOnce(VmmSnapshotPage) -> F + 'static,
    ) {
        let this = self.clone();
        glib::spawn_future_local(async move { f(this).await });
    }

    fn connect_signals(&self, refresh: &gtk::Button) {
        let cb = self.with(|t| t.spawn(|t| async move { t.on_add_clicked().await }));
        self.add.connect_clicked(move |_| cb());
        let cb = self.with(|t| t.spawn(|t| async move { t.on_delete_clicked().await }));
        self.delete.connect_clicked(move |_| cb());
        let cb = self.with(|t| t.spawn(|t| async move { t.on_start_clicked().await }));
        self.start.connect_clicked(move |_| cb());
        let cb = self.with(|t| t.spawn(|t| async move { t.on_apply_clicked().await }));
        self.apply.connect_clicked(move |_| cb());
        let cb = self.with(|t| t.refresh_snapshots(None));
        refresh.connect_clicked(move |_| cb());
        let cb = self.with(VmmSnapshotPage::snapshot_selected);
        self.list.connect_selected_rows_changed(move |_| cb());
        let cb = self.with(|t| t.spawn(|t| async move { t.on_start_clicked().await }));
        self.list.connect_row_activated(move |_, _| cb());
        let cb = self.with(VmmSnapshotPage::description_changed);
        self.description.buffer().connect_changed(move |_| cb());

        let actions = gio::SimpleActionGroup::new();
        let start = gio::SimpleAction::new("start", None);
        let cb = self.with(|t| t.spawn(|t| async move { t.on_start_clicked().await }));
        start.connect_activate(move |_, _| cb());
        actions.add_action(&start);
        let delete = gio::SimpleAction::new("delete", None);
        let cb = self.with(|t| t.spawn(|t| async move { t.on_delete_clicked().await }));
        delete.connect_activate(move |_, _| cb());
        actions.add_action(&delete);
        self.top_box.insert_action_group("snapshot", Some(&actions));

        // `on_snapshot_list_button_press_event`
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_SECONDARY);
        let weak = self.weak();
        click.connect_pressed(move |_, _, x, y| {
            if let Some(inner) = weak.upgrade() {
                VmmSnapshotPage(inner).popup_snapshot_menu(x, y);
            }
        });
        self.list.add_controller(click);
    }

    pub fn top_box(&self) -> &gtk::Box {
        &self.top_box
    }

    /// `_popup_snapshot_menu`: a right click selects the row under the
    /// pointer, as the tree view's own handler did, and pops up the menu.
    fn popup_snapshot_menu(&self, x: f64, y: f64) {
        if let Some(row) = self.list.row_at_y(y as i32) {
            if row.is_selectable() && !row.is_selected() {
                self.list.unselect_all();
                self.list.select_row(Some(&row));
            }
        }
        let point = self
            .list
            .compute_point(
                &self.top_box,
                &gtk::graphene::Point::new(x as f32, y as f32),
            )
            .unwrap_or_else(|| gtk::graphene::Point::new(x as f32, y as f32));
        self.menu.set_pointing_to(Some(&gdk::Rectangle::new(
            point.x() as i32,
            point.y() as i32,
            1,
            1,
        )));
        self.menu.popup();
    }

    ///////////////////////
    // Functional bits   //
    ///////////////////////

    /// `_get_selected_snapshots`.
    fn get_selected_snapshots(&self) -> Vec<Rc<VmmDomainSnapshot>> {
        self.rows
            .borrow()
            .iter()
            .filter(|(row, _)| row.is_selected())
            .map(|(_, snap)| snap.clone())
            .collect()
    }

    /// `vm.list_snapshots()`, cached until the next refresh.
    async fn list_snapshots(&self) -> Result<Vec<Rc<VmmDomainSnapshot>>, String> {
        if let Some(list) = self.snapshot_list.borrow().clone() {
            return Ok(list);
        }
        let gen = self.generation.get();
        let list: Vec<Rc<VmmDomainSnapshot>> = with_domain(&self.vm, |_, dom| list_snapshots(dom))
            .await?
            .into_iter()
            .map(Rc::new)
            .collect();
        if gen == self.generation.get() {
            self.snapshot_list.replace(Some(list.clone()));
        }
        Ok(list)
    }

    /// `_refresh_snapshots`.
    fn refresh_snapshots(&self, select_name: Option<String>) {
        // `vm.refresh_snapshots()`
        self.snapshot_list.replace(None);
        self.spawn(|t| async move { t.populate_snapshot_list(select_name).await });
    }

    /// `vmwindow_refresh_vm_state`.
    pub fn vmwindow_refresh_vm_state(&self) {
        if !self.initial_populate.get() && !self.populating.get() {
            self.spawn(|t| async move { t.populate_snapshot_list(None).await });
        }
    }

    /// `_set_error_page`.
    fn set_error_page(&self, msg: &str) {
        self.set_snapshot_state(None);
        self.notebook.set_visible_child_name(PAGE_ERROR);
        self.error_label.set_text(msg);
    }

    /// `_populate_snapshot_list`.
    async fn populate_snapshot_list(&self, select_name: Option<String>) {
        let cursnaps: Vec<String> = self
            .get_selected_snapshots()
            .iter()
            .map(|s| s.get_name().to_string())
            .collect();
        let gen = self.generation.get() + 1;
        self.generation.set(gen);
        self.populating.set(true);
        let snapshots = self.list_snapshots().await;
        if gen != self.generation.get() {
            // A newer refresh took over.
            return;
        }
        self.populating.set(false);

        // `model.clear()`, which drops the selection and any unapplied
        // edit with it.
        self.quiet.set(true);
        self.list.remove_all();
        self.rows.borrow_mut().clear();
        self.unapplied_changes.set(false);
        self.shown.replace(None);
        self.quiet.set(false);

        let snapshots = match snapshots {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{e}");
                self.set_error_page(&format!("Error refreshing snapshot list: {e}"));
                return;
            }
        };

        let mut has_external = false;
        let mut has_internal = false;
        let mut entries: Vec<(String, Option<Rc<VmmDomainSnapshot>>)> = Vec::new();
        for snap in &snapshots {
            let external = snap.is_external();
            if external {
                has_external = true;
            } else {
                has_internal = true;
            }
            let (_, sortname) = list_row_label(snap.get_name(), snap.run_status(), external);
            entries.push((sortname, Some(snap.clone())));
        }
        if has_internal && has_external {
            entries.push(("2".into(), None));
        }
        sort_by_sortname(&mut entries);

        self.quiet.set(true);
        for (_, snap) in entries {
            match snap {
                Some(snap) => {
                    let row = build_list_row(&snap);
                    self.list.append(&row);
                    self.rows.borrow_mut().push((row, snap));
                }
                None => {
                    // `set_row_separator_func`
                    let row = gtk::ListBoxRow::new();
                    row.set_child(Some(&gtk::Separator::new(gtk::Orientation::Horizontal)));
                    row.set_selectable(false);
                    row.set_activatable(false);
                    row.set_focusable(false);
                    self.list.append(&row);
                }
            }
        }

        // `check_selection`
        for (row, snap) in self.rows.borrow().iter() {
            let select = match &select_name {
                Some(n) => snap.get_name() == n,
                None => cursnaps.iter().any(|n| n == snap.get_name()),
            };
            if select {
                self.list.select_row(Some(row));
            }
        }
        self.quiet.set(false);

        self.initial_populate.set(true);
        self.snapshot_selected();
    }

    /// `_read_screenshot_file`.
    fn read_screenshot_file(&self, name: &str) -> Option<gtk::gdk_pixbuf::Pixbuf> {
        if name.is_empty() {
            return None;
        }
        let cache_dir = vm_cache_dir(&self.vm);
        let filename = find_screenshot_file(&cache_dir, name)?;
        let ext = filename.extension()?.to_string_lossy().into_owned();
        let mime = ext_to_mime(&ext)?;
        let data = std::fs::read(&filename).ok()?;
        make_screenshot_pixbuf(mime, &data)
    }

    /// `_set_snapshot_state`.
    fn set_snapshot_state(&self, snap: Option<&Rc<VmmDomainSnapshot>>) {
        self.notebook.set_visible_child_name(PAGE_DETAILS);

        let xmlobj = snap.map(|s| s.get_xmlobj().clone());
        let name = xmlobj
            .as_ref()
            .and_then(|x| x.name.clone())
            .unwrap_or_default();
        let desc = xmlobj
            .as_ref()
            .and_then(|x| x.description.clone())
            .unwrap_or_default();
        let state = snap.map(|s| s.run_status()).unwrap_or_default();
        let icon = snap.map(|s| s.run_status_icon_name());
        let is_external = snap.is_some_and(|s| s.is_external());
        let is_current = snap.is_some_and(|s| s.is_current());

        let timestamp = xmlobj
            .as_ref()
            .map(|x| format_timestamp(x.creation_time.unwrap_or(0)))
            .unwrap_or_default();

        let title = if name.is_empty() {
            String::new()
        } else {
            format!("<b>Snapshot '{}':</b>", glib::markup_escape_text(&name))
        };

        // `uiutil.set_grid_row_visible(snapshot-is-current)`
        self.is_current.set_visible(is_current);
        self.title.set_markup(&title);
        self.timestamp.set_text(&timestamp);
        // The details now belong to `snap`, before the buffer's
        // `changed` compares against it.
        self.shown.replace(snap.cloned());
        self.description.buffer().set_text(&desc);

        self.status_text.set_text(state);
        if let Some(icon) = icon {
            self.status_icon.set_icon_name(Some(icon));
        }

        self.mode_label.set_visible(is_external);
        self.mode.set_visible(is_external);
        if let Some(x) = xmlobj.as_ref().filter(|_| is_external) {
            self.mode.set_text(external_mode_label(x));
        }

        let sn = self.read_screenshot_file(&name);
        self.screenshot.set_visible(sn.is_some());
        self.screenshot_label.set_visible(sn.is_none());
        if let Some(sn) = sn {
            #[allow(deprecated)]
            let texture = gdk::Texture::for_pixbuf(&sn);
            self.screenshot.set_paintable(Some(&texture));
        }

        self.add.set_sensitive(true);
        self.delete.set_sensitive(snap.is_some());
        self.start.set_sensitive(snap.is_some());
        self.apply.set_sensitive(false);
        self.unapplied_changes.set(false);
    }

    fn description_text(&self) -> String {
        let buf = self.description.buffer();
        buf.text(&buf.start_iter(), &buf.end_iter(), true)
            .to_string()
    }

    /// `_apply`: redefine the snapshot with the edited description.
    async fn apply(&self, snap: &Rc<VmmDomainSnapshot>, desc: &str) -> Result<(), String> {
        let origxml = snap.xml.borrow().clone();
        let (origxml, newxml) = snapshot::set_description(&origxml, desc)?;
        // `log_redefine_xml_diff`
        if newxml == origxml {
            eprintln!(
                "Redefine requested for snapshot '{}', but XML didn't change!",
                snap.get_name()
            );
            return Ok(());
        }
        let name = snap.get_name().to_string();
        eprintln!("Redefining snapshot '{name}' with XML:\n{newxml}");
        let latest = with_domain(&self.vm, move |_, dom| {
            create_snapshot(dom, &newxml, true, false)?;
            // `snap.ensure_latest_xml()`
            VirtSnapshot::lookup_by_name(dom, &name, 0)?.get_xml_desc(0)
        })
        .await?;
        snap.ensure_latest_xml(latest);
        Ok(())
    }

    /// `_snapshot_selected`, plus `_confirm_changes`: leaving a
    /// snapshot with an unapplied description offers to apply it.
    fn snapshot_selected(&self) {
        if self.quiet.get() {
            return;
        }
        let snaps = self.get_selected_snapshots();

        let prev = self.shown.borrow().clone();
        if let Some(prev) = prev.filter(|_| self.unapplied_changes.get()) {
            if !snaps.iter().any(|s| Rc::ptr_eq(s, &prev)) {
                let desc = self.description_text();
                self.spawn(|t| async move {
                    if error::confirm_unapplied_changes(Some(&t.topwin)).await {
                        if let Err(e) = t.apply(&prev, &desc).await {
                            eprintln!("Error applying snapshot changes: {e}");
                        }
                    }
                });
            }
        }

        if snaps.is_empty() {
            self.set_error_page("No snapshot selected.");
            return;
        }
        if snaps.len() > 1 {
            self.set_error_page("Multiple snapshots selected.");
            self.start.set_sensitive(false);
            self.apply.set_sensitive(false);
            self.delete.set_sensitive(true);
            return;
        }
        self.set_snapshot_state(Some(&snaps[0]));
    }

    //////////////////
    // Listeners    //
    //////////////////

    /// `_cleanup`.
    pub fn cleanup(&self) {
        if self.menu.parent().is_some() {
            self.menu.unparent();
        }
        if let Some(new) = self.snapshot_new.take() {
            new.cleanup();
        }
    }

    /// `_description_changed`.
    fn description_changed(&self) {
        let snaps = self.get_selected_snapshots();
        let desc = self.description_text();
        if snaps.len() == 1 && snaps[0].get_xmlobj().description.as_deref() != Some(desc.as_str()) {
            self.unapplied_changes.set(true);
        }
        self.apply.set_sensitive(true);
    }

    /// `_on_apply_clicked`.
    async fn on_apply_clicked(&self) {
        let snaps = self.get_selected_snapshots();
        if snaps.len() == 1 {
            let desc = self.description_text();
            if let Err(e) = self.apply(&snaps[0], &desc).await {
                eprintln!("Error applying snapshot changes: {e}");
                return;
            }
        }
        self.refresh_snapshots(None);
    }

    /// `_on_add_clicked`.
    async fn on_add_clicked(&self) {
        if self.snapshot_new.borrow().is_none() {
            let new = SnapshotNew::new(&self.vm);
            // `_snapshot_created_cb`
            let weak = self.weak();
            new.connect_snapshot_created(move |newname| {
                if let Some(inner) = weak.upgrade() {
                    VmmSnapshotPage(inner).refresh_snapshots(Some(newname.to_string()));
                }
            });
            self.snapshot_new.replace(Some(new));
        }
        if self.vm.has_managed_save() {
            let result = error::ok_cancel(
                Some(&self.topwin),
                "Saved memory state will not be part of the snapshot",
                Some(
                    "The domain is currently saved. Due to technical \
                     limitations that saved memory state will not become part \
                     of the snapshot. Running it later will be the same as \
                     having forced the system off mid-flight. It is \
                     recommended to snapshot either the running or shut down \
                     system instead.",
                ),
            )
            .await;
            if !result {
                return;
            }
        }
        let new = self.snapshot_new.borrow().clone();
        if let Some(new) = new {
            new.show(&self.topwin).await;
        }
    }

    /// `vmmAsyncJob.simple_async` for one call on the domain, with
    /// upstream's `errorintro + ": " + error` dialog.
    async fn simple_async(
        &self,
        title: &str,
        text: &str,
        errorintro: &str,
        work: impl FnOnce(&Connect, &Domain) -> Result<(), virt::error::Error> + Send + 'static,
    ) {
        let Ok(handle) = self.vm.handle() else {
            return;
        };
        let uuid = self.vm.get_uuid();
        let err = asyncjob::run(Some(&self.topwin), title, text, true, move |_job| {
            let conn = handle.connect();
            let dom = Domain::lookup_by_uuid_string(conn, &uuid).map_err(job_err)?;
            work(conn, &dom).map_err(job_err)
        })
        .await;
        if let Some((error, details)) = err {
            error::show_err(
                Some(&self.topwin),
                &format!("{errorintro}: {error}"),
                Some(&details),
                "",
            )
            .await;
        }
    }

    /// `_on_start_clicked`: revert to the selected snapshot.
    async fn on_start_clicked(&self) {
        let snaps = self.get_selected_snapshots();
        if snaps.len() != 1 {
            return;
        }
        let snap = snaps[0].clone();
        let name = snap.get_name().to_string();

        let msg = if self.vm.is_active() {
            format!(
                "Are you sure you want to run the snapshot '{name}'? \
                 All the disk changes since the last snapshot was created \
                 will be discarded."
            )
        } else {
            format!(
                "Are you sure you want to run the snapshot '{name}'? \
                 All the disk and configuration changes since the last \
                 snapshot was created will be discarded."
            )
        };
        if !error::yes_no(Some(&self.topwin), &msg, None).await {
            return;
        }

        if self.vm.has_managed_save() && !snap.has_run_state() {
            let result = error::ok_cancel(
                Some(&self.topwin),
                "Saved state will be removed to avoid filesystem corruption",
                Some(&format!(
                    "Snapshot '{name}' contains only disk and no memory state. \
                     Restoring the snapshot would leave the existing saved state \
                     in place, effectively switching a disk underneath a running \
                     system. Running the domain afterwards would likely result in \
                     extensive filesystem corruption. Therefore the saved state \
                     will be removed before restoring the snapshot."
                )),
            )
            .await;
            if !result {
                return;
            }
            if let Err(e) = self.vm.run_action(DomainAction::RemoveSavedImage).await {
                eprintln!("Error removing saved image: {e}");
                return;
            }
        }

        eprintln!("Running snapshot '{name}'");
        // `revert_to_snapshot`: no use trying to set the guest time if
        // it is going to be switched off after reverting to the snapshot
        let will_be_running = snap.is_running();
        let conn = self.vm.conn();
        let handle = self.vm.handle().ok();
        let is_qemu = conn.as_ref().is_some_and(|c| c.is_qemu());
        let is_test = conn.as_ref().is_some_and(|c| c.is_test());
        let has_agent = has_agent(&self.vm);
        let uuid = self.vm.get_uuid();
        let revert_name = name.clone();
        self.simple_async(
            "Running snapshot",
            &format!("Running snapshot '{name}'"),
            &format!("Error running snapshot '{name}'"),
            move |_, dom| {
                revert_to_snapshot(dom, &revert_name)?;
                // looking at the domain state after revert will always
                // come back as paused, so look at the snapshot state
                // instead
                if let Some(handle) = handle.filter(|_| will_be_running) {
                    async_set_time(handle, uuid, is_qemu, is_test, has_agent);
                }
                Ok(())
            },
        )
        .await;
        // `lifecycle_action`: a priority tick picks up the new state.
        if let Some(conn) = conn {
            conn.tick();
        }
        self.refresh_snapshots(None);
    }

    /// `_on_delete_clicked`: one job per selected snapshot.
    async fn on_delete_clicked(&self) {
        let snaps = self.get_selected_snapshots();
        if snaps.is_empty() {
            return;
        }
        let result = error::yes_no(
            Some(&self.topwin),
            "Are you sure you want to permanently delete the selected snapshots?",
            None,
        )
        .await;
        if !result {
            return;
        }
        for snap in snaps {
            let name = snap.get_name().to_string();
            eprintln!("Deleting snapshot '{name}'");
            let delname = name.clone();
            self.simple_async(
                "Deleting snapshot",
                &format!("Deleting snapshot '{name}'"),
                &format!("Error deleting snapshot '{name}'"),
                move |_, dom| delete_snapshot(dom, &delname),
            )
            .await;
            self.refresh_snapshots(None);
        }
    }
}

/// One snapshot row: the state icon, the name and state markup, and
/// the "current" emblem; the description is the tooltip.
fn build_list_row(snap: &VmmDomainSnapshot) -> gtk::ListBoxRow {
    let (label, _) = list_row_label(snap.get_name(), snap.run_status(), snap.is_external());
    let icon = gtk::Image::from_icon_name(snap.run_status_icon_name());
    icon.set_pixel_size(24);
    let text = gtk::Label::new(None);
    text.set_markup(&label);
    text.set_xalign(0.0);
    text.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let current = gtk::Image::from_icon_name("emblem-default");
    current.set_pixel_size(16);
    current.set_halign(gtk::Align::Start);
    current.set_visible(snap.is_current());
    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    hbox.set_margin_start(2);
    hbox.set_margin_end(2);
    hbox.set_margin_top(2);
    hbox.set_margin_bottom(2);
    hbox.append(&icon);
    hbox.append(&text);
    hbox.append(&current);
    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&hbox));
    // `slist.set_tooltip_column(2)`: the tree view sets it as markup.
    if let Some(desc) = snap.get_xmlobj().description.as_deref() {
        row.set_tooltip_markup(Some(desc));
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_state_strings_like_upstream() {
        assert_eq!(state_str_to_int(Some("running")), sys::VIR_DOMAIN_RUNNING);
        assert_eq!(
            state_str_to_int(Some("pmsuspended")),
            sys::VIR_DOMAIN_PMSUSPENDED
        );
        assert_eq!(
            state_str_to_int(Some("disk-snapshot")),
            sys::VIR_DOMAIN_SHUTOFF
        );
        assert_eq!(state_str_to_int(None), sys::VIR_DOMAIN_SHUTOFF);
        assert_eq!(
            enummap::pretty_run_status(state_str_to_int(Some("paused")), false),
            "Paused"
        );
    }

    #[test]
    fn default_name_counts_from_one() {
        assert_eq!(default_snapshot_name(|_| false), "snapshot1");
        let taken = ["snapshot1", "snapshot2"];
        assert_eq!(default_snapshot_name(|n| taken.contains(&n)), "snapshot3");
        // Never the bare base name, even when it's free.
        assert_eq!(default_snapshot_name(|n| n == "snapshot1"), "snapshot2");
    }

    /// `_reset_snapshot_mode`, `_set_memory_path_visibility` and the
    /// `diskOnly` flag.
    #[test]
    fn mode_rules_like_upstream() {
        use SnapshotMode::*;
        assert_eq!(default_mode(true, None), External);
        assert_eq!(default_mode(true, Some(External)), External);
        assert_eq!(default_mode(true, Some(Internal)), Internal);
        assert_eq!(default_mode(false, None), Internal);
        assert_eq!(default_mode(false, Some(External)), Internal);

        assert!(memory_path_visible(External, true));
        assert!(!memory_path_visible(External, false));
        assert!(!memory_path_visible(Internal, true));

        assert!(disk_only(External, false));
        assert!(!disk_only(External, true));
        assert!(!disk_only(Internal, false));
    }

    #[test]
    fn memory_paths_sit_next_to_each_disk() {
        let disks = vec![
            "/var/lib/libvirt/images/vm.qcow2".to_string(),
            "/var/lib/libvirt/images/data.qcow2".to_string(),
            "/mnt/other/disk.img".to_string(),
            String::new(),
            "relative.img".to_string(),
        ];
        assert_eq!(
            memory_paths("vm", "snapshot1", &disks),
            vec![
                "/var/lib/libvirt/images/vm-mem.snapshot1",
                "/mnt/other/vm-mem.snapshot1",
                "vm-mem.snapshot1",
            ]
        );
        assert!(memory_paths("vm", "s", &[]).is_empty());
    }

    #[test]
    fn new_snapshot_validation_like_upstream() {
        use SnapshotMode::*;
        let s = build_new_snapshot("snapshot1", "", Internal, true, None).unwrap();
        assert_eq!(s.description, None);
        assert_eq!(s.memory_type, None);
        // Memory goes to a file only for an external snapshot of a
        // running VM.
        let s = build_new_snapshot("s", "d", External, true, Some("/m".into())).unwrap();
        assert_eq!(s.description.as_deref(), Some("d"));
        assert_eq!(s.memory_type.as_deref(), Some("external"));
        assert_eq!(s.memory_file.as_deref(), Some("/m"));
        let s = build_new_snapshot("s", "", External, false, Some("/m".into())).unwrap();
        assert_eq!(s.memory_type, None);
        assert_eq!(
            build_new_snapshot("", "", Internal, false, None).unwrap_err(),
            "A name must be specified for the Snapshot"
        );
        assert_eq!(
            build_new_snapshot("a/b", "", Internal, false, None).unwrap_err(),
            "Snapshot name 'a/b' can not contain '/' character."
        );
    }

    #[test]
    fn list_rows_sort_internal_then_separator_then_external() {
        let (label, sort) = list_row_label("a<b", "Running", false);
        assert_eq!(label, "a&lt;b\n<span size='small'>VM State: Running</span>");
        assert_eq!(sort, "1a<b");
        let (label, sort) = list_row_label("z", "Shutoff", true);
        assert_eq!(
            label,
            "z\n<span size='small'>VM State: Shutoff (External)</span>"
        );
        assert_eq!(sort, "3z");
        let mut entries: Vec<(String, ())> = ["3a", "2", "1z", "1b"]
            .into_iter()
            .map(|k| (k.to_string(), ()))
            .collect();
        sort_by_sortname(&mut entries);
        let keys: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["1b", "1z", "2", "3a"]);
    }

    #[test]
    fn external_mode_labels() {
        let mut x = DomainSnapshot {
            memory_type: Some("external".into()),
            ..Default::default()
        };
        assert_eq!(external_mode_label(&x), "External memory only");
        x.disks.push(Default::default());
        assert_eq!(external_mode_label(&x), "External disk and memory");
        x.memory_type = Some("no".into());
        assert_eq!(external_mode_label(&x), "External disk only");
    }

    #[test]
    fn screenshots_scale_to_450_and_keep_their_file_name() {
        assert_eq!(screenshot_scale(1024, 768), (450, 337));
        assert_eq!(screenshot_scale(600, 800), (337, 450));
        assert_eq!(screenshot_scale(300, 200), (300, 200));
        assert_eq!(mime_to_ext("image/png"), Some("png"));
        assert_eq!(mime_to_ext("image/x-portable-pixmap"), Some("ppm"));
        assert_eq!(mime_to_ext("image/jpeg"), None);
        assert_eq!(ext_to_mime("ppm"), Some("image/x-portable-pixmap"));

        let dir = std::env::temp_dir().join(format!("vmm-snapshots-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A dot in the name isn't taken for the extension.
        save_screenshot(&dir, "v1.0", Some(("image/png".into(), b"png".to_vec())));
        let file = find_screenshot_file(&dir, "v1.0").unwrap();
        assert_eq!(file.file_name().unwrap(), "snap-screenshot-v1.0.png");
        // A new snapshot under the same name drops the old image.
        save_screenshot(&dir, "v1.0", None);
        assert!(find_screenshot_file(&dir, "v1.0").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The libvirt calls the page makes, against the test driver, on a
    /// domain of its own.
    #[test]
    fn create_list_revert_delete_on_the_test_driver() {
        let mut conn = Connect::open(Some("test:///default")).unwrap();
        let xml = "<domain type='test'><name>vmm-snapshots-test</name>\
                   <memory>1024</memory><os><type>hvm</type></os></domain>";
        let dom = Domain::define_xml(&conn, xml).unwrap();
        dom.create().unwrap();
        assert!(get_current_snapshot(&dom).unwrap().is_none());

        let name = default_snapshot_name(|n| VirtSnapshot::lookup_by_name(&dom, n, 0).is_ok());
        assert_eq!(name, "snapshot1");
        let snap = build_new_snapshot(&name, "first", SnapshotMode::Internal, true, None).unwrap();
        create_snapshot(&dom, &snap.get_xml().unwrap(), false, false).unwrap();
        let name2 = default_snapshot_name(|n| VirtSnapshot::lookup_by_name(&dom, n, 0).is_ok());
        assert_eq!(name2, "snapshot2");

        let list = list_snapshots(&dom).unwrap();
        assert_eq!(list.len(), 1);
        let s = &list[0];
        assert_eq!(s.get_name(), "snapshot1");
        assert!(s.is_current());
        assert!(!s.is_external());
        assert!(s.is_running() && s.has_run_state());
        assert_eq!(s.run_status(), "Running");
        assert_eq!(s.get_xmlobj().description.as_deref(), Some("first"));
        let current = get_current_snapshot(&dom).unwrap().unwrap();
        assert_eq!(SnapshotMode::of(&current), SnapshotMode::Internal);

        // Apply: redefine with a new description.
        let (_, newxml) = snapshot::set_description(&s.xml.borrow(), "second").unwrap();
        create_snapshot(&dom, &newxml, true, false).unwrap();
        let latest = VirtSnapshot::lookup_by_name(&dom, "snapshot1", 0)
            .unwrap()
            .get_xml_desc(0)
            .unwrap();
        s.ensure_latest_xml(latest);
        assert_eq!(s.get_xmlobj().description.as_deref(), Some("second"));

        dom.destroy().unwrap();
        revert_to_snapshot(&dom, "snapshot1").unwrap();
        assert_eq!(dom.get_state().unwrap().0, sys::VIR_DOMAIN_RUNNING);

        delete_snapshot(&dom, "snapshot1").unwrap();
        assert!(list_snapshots(&dom).unwrap().is_empty());
        dom.destroy().unwrap();
        dom.undefine().unwrap();
        conn.close().unwrap();
    }
}
