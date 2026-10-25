//! Upstream's `virtManager/object/domain.py` (`vmmDomain`), the slice the
//! manager window needs: identity, run state, and the lifecycle actions
//! from `vmmenu.py`. XML-derived detail (devices, stats vectors, …)
//! comes with the details window and stats ports.
//!
//! A `VmmDomain` holds the last [`DomainSnapshot`] its connection's
//! tick polled, and emits `state-changed` when a new poll differs —
//! upstream's signal name, so later windows connect the same way.
//! Lifecycle actions look the domain up by UUID on a worker thread
//! (ticket 09) and ask the connection for a priority tick afterwards,
//! same as upstream's post-action `self._update_status()`.

use std::cell::{Cell, Ref, RefCell};
use std::collections::HashSet;
use std::sync::OnceLock;

use gtk::gio;
use gtk::gio::prelude::*;
use gtk::glib;
use gtk::glib::subclass::prelude::*;
use gtk::glib::subclass::Signal;
use virt::connect::Connect;
use virt::domain::Domain;
use virt::sys;

use crate::connection::VmmConnection;
use crate::libvirtenummap::{self as enummap, DomainState};
use crate::statsmanager::{self, PollFlags, RawSample, StatsList};
use std::rc::Rc;
use virtinst_core::capabilities::domcaps::DomainCapabilities;
use virtinst_core::Guest;
use virtinst_xml::XmlBound;

/// One poll's worth of what the UI shows about a domain.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DomainSnapshot {
    pub uuid: String,
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub id: Option<u32>,
    pub state: DomainState,
    pub reason: i32,
    pub has_managed_save: bool,
}

impl DomainSnapshot {
    /// Blocking: reads one domain's state. Runs on the tick thread.
    pub fn fetch(dom: &Domain) -> Result<DomainSnapshot, virt::error::Error> {
        let metadata = |kind| {
            dom.get_metadata(kind as i32, None, 0)
                .ok()
                .filter(|s| !s.is_empty())
        };
        let (state, reason) = dom.get_state()?;
        Ok(DomainSnapshot {
            uuid: dom.get_uuid_string()?,
            name: dom.get_name()?,
            title: metadata(sys::VIR_DOMAIN_METADATA_TITLE),
            description: metadata(sys::VIR_DOMAIN_METADATA_DESCRIPTION),
            id: dom.get_id(),
            state,
            reason,
            // Not every driver implements managed save; upstream gates
            // this on `managedsave_supported` and treats it as False.
            has_managed_save: dom.has_managed_save(0).unwrap_or(false),
        })
    }

    /// Blocking: every domain on the connection, active and inactive.
    pub fn fetch_all(conn: &Connect) -> Result<Vec<DomainSnapshot>, virt::error::Error> {
        conn.list_all_domains(0)?
            .iter()
            .map(DomainSnapshot::fetch)
            .collect()
    }

    /// Blocking: one tick's poll — every domain's snapshot plus its
    /// stats sample. `mem_period_set` lists the UUIDs whose memory
    /// stats period was already set.
    pub fn fetch_all_with_stats(
        conn: &Connect,
        flags: PollFlags,
        is_lxc: bool,
        mem_period_set: &HashSet<String>,
    ) -> Result<Vec<(DomainSnapshot, RawSample)>, virt::error::Error> {
        conn.list_all_domains(0)?
            .iter()
            .map(|dom| {
                let snap = DomainSnapshot::fetch(dom)?;
                let set_period = !mem_period_set.contains(&snap.uuid);
                let raw = statsmanager::sample(dom, flags, is_lxc, set_period);
                Ok((snap, raw))
            })
            .collect()
    }
}

/// The domain's XML, parsed: upstream's `vm.get_xmlobj()`. `xml` is
/// kept for re-parsing into an editable DOM (ticket 06), `guest` is the
/// typed read view, `doc` the raw tree for the bits `Guest` doesn't
/// bind yet (`<emulator>`, every device's attributes for the hw list).
pub struct DomainXml {
    pub xml: String,
    pub doc: virtinst_xml::Document,
    pub guest: Guest,
}

impl DomainXml {
    pub fn parse(xml: String) -> Result<DomainXml, String> {
        let doc = virtinst_xml::parse_libvirt_xml(&xml).map_err(|e| e.to_string())?;
        let root = doc.root_element().ok_or("Empty domain XML")?;
        let guest = Guest::from_element(&doc, root);
        Ok(DomainXml { xml, doc, guest })
    }

    pub fn root(&self) -> virtinst_xml::Element {
        self.doc.root_element().expect("parsed with a root")
    }

    /// `./devices/emulator`.
    pub fn emulator(&self) -> Option<String> {
        virtinst_xml::resolve_path(&self.doc, self.root(), &["devices", "emulator"])
            .map(|e| e.text_content(&self.doc))
            .filter(|s| !s.is_empty())
    }
}

/// `_make_xmlobj_to_define`'s result: the inactive XML as an editable
/// document, with its typed view. Edits go into `guest` (or straight
/// into `doc` for the nested parts `Guest` keeps read-only), and
/// [`VmmDomain::redefine_xmlobj`] writes them back.
pub struct XmlToDefine {
    pub doc: virtinst_xml::Document,
    pub guest: Guest,
    origxml: String,
}

impl XmlToDefine {
    fn parse(xml: String) -> Result<XmlToDefine, String> {
        let doc = virtinst_xml::parse_libvirt_xml(&xml).map_err(|e| e.to_string())?;
        let root = doc.root_element().ok_or("Empty domain XML")?;
        let guest = Guest::from_element(&doc, root);
        Ok(XmlToDefine {
            doc,
            guest,
            origxml: xml,
        })
    }

    pub fn root(&self) -> virtinst_xml::Element {
        self.doc.root_element().expect("parsed with a root")
    }

    /// `get_xml()`: the document with `guest`'s edits written back.
    pub fn get_xml(&mut self) -> Result<String, String> {
        let root = self.root();
        self.guest.write_to(&mut self.doc, root);
        // An unset text field writes nothing; `guest` was read from
        // `doc`, so `None` here means the element was cleared.
        for (tag, unset) in [
            ("title", self.guest.title.is_none()),
            ("description", self.guest.description.is_none()),
        ] {
            if unset {
                virtinst_xml::remove_path_if_present(&mut self.doc, root, &[tag])
                    .map_err(|e| e.to_string())?;
            }
        }
        self.doc.write_str().map_err(|e| e.to_string())
    }

    /// `./os`, written back from an edited copy of `guest.os`.
    pub fn write_os(&mut self) {
        let root = self.root();
        let os = virtinst_xml::resolve_or_create_path(&mut self.doc, root, &["os"]);
        self.guest.os.write_to(&mut self.doc, os);
    }

    /// `is_uefi`.
    pub fn is_uefi(&self) -> bool {
        let os = &self.guest.os;
        if os.loader.is_some() && os.loader_type.as_deref() == Some("pflash") {
            return true;
        }
        os.firmware.as_deref() == Some("efi")
    }
}

/// `Guest.validate_name` (with `XMLBuilder.validate_generic_name`),
/// minus the collision check, which needs the connection.
pub fn validate_generic_name(name_label: &str, val: &str) -> Result<(), String> {
    if val.is_empty() {
        return Err(format!("A name must be specified for the {name_label}"));
    }
    for c in ['/'] {
        if val.contains(c) {
            return Err(format!(
                "{name_label} name '{val}' can not contain '{c}' character."
            ));
        }
    }
    Ok(())
}

/// A storage volume or local file that `_copy_nvram_file` made or
/// will remove.
enum NvramFile {
    Vol(virt::storage_vol::StorageVol),
    Local(String),
}

impl NvramFile {
    fn path(&self) -> String {
        match self {
            NvramFile::Vol(v) => v.get_path().unwrap_or_default(),
            NvramFile::Local(p) => p.clone(),
        }
    }

    fn delete(&self) -> Result<(), String> {
        match self {
            NvramFile::Vol(v) => v.delete(0).map_err(|e| e.message().to_string()),
            NvramFile::Local(p) => std::fs::remove_file(p).map_err(|e| e.to_string()),
        }
    }
}

/// `_copy_nvram_file`'s copy, blocking: libvirt has no API to rename a
/// storage volume, so the NVRAM is cloned next to the old one under
/// the new name (`Cloner.build_clone_disk` + `build_storage`). A path
/// outside any storage pool is copied directly when the connection is
/// local. Returns `(new, old)`, or `None` when the old file can't be
/// found.
fn copy_nvram_file(
    conn: &Connect,
    old_path: &str,
    new_name: &str,
    is_local: bool,
) -> Result<Option<(NvramFile, NvramFile)>, String> {
    use virt::storage_pool::StoragePool;
    use virt::storage_vol::StorageVol;

    let old = std::path::Path::new(old_path);
    let ext = old
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_else(|| ".fd".into());
    let base = std::path::Path::new(new_name)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| new_name.to_string());
    let new_file = format!("{base}_VARS{ext}");
    let dir = old.parent().unwrap_or(std::path::Path::new("/"));
    let new_path = dir.join(&new_file).to_string_lossy().into_owned();

    if let Ok(vol) = StorageVol::lookup_by_path(conn, old_path) {
        let pool = StoragePool::lookup_by_volume(&vol).map_err(|e| e.message().to_string())?;
        let info = vol.get_info().map_err(|e| e.message().to_string())?;
        let format = vol
            .get_xml_desc(0)
            .ok()
            .and_then(|xml| volume_format(&xml))
            .map(|f| format!("<format type='{f}'/>"))
            .unwrap_or_default();
        let xml = format!(
            "<volume><name>{}</name><capacity>{}</capacity>\
             <target>{format}</target></volume>",
            glib::markup_escape_text(&new_file),
            info.capacity
        );
        let newvol = StorageVol::create_xml_from(&pool, &xml, &vol, 0)
            .map_err(|e| e.message().to_string())?;
        return Ok(Some((NvramFile::Vol(newvol), NvramFile::Vol(vol))));
    }
    if is_local && old.exists() {
        std::fs::copy(old, &new_path).map_err(|e| e.to_string())?;
        return Ok(Some((
            NvramFile::Local(new_path),
            NvramFile::Local(old_path.to_string()),
        )));
    }
    eprintln!(
        "old_nvram_path={old_path} but it doesn't appear to exist. \
         skipping rename nvram duplication"
    );
    Ok(None)
}

/// `<target><format type=...>` of a storage volume's XML.
fn volume_format(xml: &str) -> Option<String> {
    let doc = virtinst_xml::parse_libvirt_xml(xml).ok()?;
    let root = doc.root_element()?;
    let format = virtinst_xml::resolve_path(&doc, root, &["target", "format"])?;
    format.attribute(&doc, "type").map(str::to_string)
}

/// `vmmLibvirtObject.define_name` + `vmmConnection.rename_object`:
/// undefine the old name (keeping NVRAM and TPM state) and define the
/// new XML, putting the old one back if that fails; autostart carries
/// over. Blocking.
fn define_name(
    conn: &Connect,
    dom: &Domain,
    newname: &str,
    has_nvram: bool,
    has_tpm_state: bool,
) -> Result<(), String> {
    let msg = |e: virt::error::Error| e.message().to_string();
    let oldautostart = dom.get_autostart().unwrap_or(false);
    let origxml = dom
        .get_xml_desc(sys::VIR_DOMAIN_XML_INACTIVE | sys::VIR_DOMAIN_XML_SECURE)
        .map_err(msg)?;
    let mut xmlobj = XmlToDefine::parse(origxml.clone())?;
    if xmlobj.guest.name.as_deref() == Some(newname) {
        return Ok(());
    }
    eprintln!(
        "Changing domain name from {} to {newname}",
        xmlobj.guest.name.as_deref().unwrap_or("")
    );
    xmlobj.guest.name = Some(newname.to_string());
    let newxml = xmlobj.get_xml()?;

    // `delete(force=False)`
    let mut flags = 0;
    if has_nvram {
        flags |= sys::VIR_DOMAIN_UNDEFINE_KEEP_NVRAM;
    }
    // `domain_undefine_keep_tpm`: libvirt 8.9.0
    if has_tpm_state && conn.get_lib_version().unwrap_or(0) >= 8_009_000 {
        flags |= sys::VIR_DOMAIN_UNDEFINE_KEEP_TPM;
    }
    if let Err(e) = dom.undefine_flags(flags) {
        eprintln!("libvirt undefineFlags failed, falling back to old style: {e}");
        dom.undefine().map_err(msg)?;
    }

    let newdom = match Domain::define_xml(conn, &newxml) {
        Ok(d) => d,
        Err(renameerr) => {
            eprintln!("Error defining new name domain XML: {renameerr}");
            return match Domain::define_xml(conn, &origxml) {
                Ok(_) => Err(renameerr.message().to_string()),
                Err(fixerr) => Err(format!(
                    "domain rename failed. Attempting to recover also failed.\n\n\
                     Original error: {}\n\nRecover error: {}",
                    renameerr.message(),
                    fixerr.message()
                )),
            };
        }
    };
    if oldautostart {
        newdom.set_autostart(true).map_err(msg)?;
    }
    Ok(())
}

/// `vmmConnection.pretty_hv`: `<domain type>` + `<os><type>` to a label.
pub fn pretty_hv(gtype: &str, domtype: &str) -> String {
    let gtype = gtype.to_lowercase();
    let domtype = domtype.to_lowercase();
    match (domtype.as_str(), gtype.as_str()) {
        ("xen", "xen") => "xen (paravirt)".into(),
        ("xen", "hvm") => "xen (fullvirt)".into(),
        ("test", "xen") => "test (xen)".into(),
        ("test", "hvm") => "test (hvm)".into(),
        ("qemu", _) => "QEMU TCG".into(),
        ("kvm", _) => "KVM".into(),
        ("hvf", _) => "Hypervisor.framework".into(),
        _ => domtype,
    }
}

/// The lifecycle calls `VMActionUI` makes, as data so they can cross to
/// the worker thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomainAction {
    Startup,
    Suspend,
    Resume,
    Shutdown,
    Reboot,
    Reset,
    Destroy,
    Save,
    /// `vm.remove_saved_image()`: drop a managed-save image.
    RemoveSavedImage,
}

impl DomainAction {
    fn run(self, dom: &Domain) -> Result<(), virt::error::Error> {
        match self {
            DomainAction::Startup => dom.create().map(drop),
            DomainAction::Suspend => dom.suspend().map(drop),
            DomainAction::Resume => dom.resume().map(drop),
            DomainAction::Shutdown => dom.shutdown().map(drop),
            DomainAction::Reboot => dom.reboot(0),
            DomainAction::Reset => dom.reset().map(drop),
            DomainAction::Destroy => dom.destroy(),
            DomainAction::Save => dom.managed_save(0).map(drop),
            DomainAction::RemoveSavedImage => dom.managed_save_remove(0).map(drop),
        }
    }
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct VmmDomain {
        pub snapshot: RefCell<DomainSnapshot>,
        pub conn: glib::WeakRef<VmmConnection>,
        pub stats: RefCell<StatsList>,
        pub mem_period_set: Cell<bool>,
        pub xmlobj: RefCell<Option<Rc<DomainXml>>>,
        pub domcaps: RefCell<Option<Rc<DomainCapabilities>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for VmmDomain {
        const NAME: &'static str = "VmmDomain";
        type Type = super::VmmDomain;
    }

    impl ObjectImpl for VmmDomain {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    Signal::builder("state-changed").build(),
                    Signal::builder("resources-sampled").build(),
                ]
            })
        }
    }
}

glib::wrapper! {
    pub struct VmmDomain(ObjectSubclass<imp::VmmDomain>);
}

impl VmmDomain {
    pub fn new(conn: &VmmConnection, snapshot: DomainSnapshot) -> VmmDomain {
        let obj: VmmDomain = glib::Object::new();
        obj.imp().conn.set(Some(conn));
        obj.imp().snapshot.replace(snapshot);
        obj
    }

    /// Applies a fresh poll; emits `state-changed` only on a real change.
    pub fn update(&self, snapshot: DomainSnapshot) {
        if *self.imp().snapshot.borrow() == snapshot {
            return;
        }
        self.imp().snapshot.replace(snapshot);
        self.emit_by_name::<()>("state-changed", &[]);
    }

    /// `statsmanager.refresh_vm_stats` with this tick's raw sample, then
    /// upstream's `resources-sampled` signal.
    pub fn apply_sample(&self, raw: &RawSample, flags: PollFlags, hostcpus: u32) {
        self.imp()
            .stats
            .borrow_mut()
            .append_sample(raw, flags, hostcpus);
        self.imp().mem_period_set.set(
            flags.mem && !matches!(raw.state, sys::VIR_DOMAIN_SHUTOFF | sys::VIR_DOMAIN_CRASHED),
        );
        self.emit_by_name::<()>("resources-sampled", &[]);
    }

    pub fn mem_period_set(&self) -> bool {
        self.imp().mem_period_set.get()
    }

    pub fn stats(&self) -> Ref<'_, StatsList> {
        self.imp().stats.borrow()
    }

    pub fn conn(&self) -> Option<VmmConnection> {
        self.imp().conn.upgrade()
    }

    fn snap<T>(&self, f: impl FnOnce(&DomainSnapshot) -> T) -> T {
        f(&self.imp().snapshot.borrow())
    }

    pub fn get_uuid(&self) -> String {
        self.snap(|s| s.uuid.clone())
    }
    pub fn get_name(&self) -> String {
        self.snap(|s| s.name.clone())
    }
    pub fn get_id(&self) -> Option<u32> {
        self.snap(|s| s.id)
    }
    pub fn get_name_or_title(&self) -> String {
        self.snap(|s| s.title.clone().unwrap_or_else(|| s.name.clone()))
    }
    pub fn get_title(&self) -> Option<String> {
        self.snap(|s| s.title.clone())
    }
    pub fn get_description(&self) -> String {
        self.snap(|s| s.description.clone().unwrap_or_default())
    }
    pub fn status(&self) -> DomainState {
        self.snap(|s| s.state)
    }
    pub fn has_managed_save(&self) -> bool {
        self.snap(|s| s.has_managed_save)
    }

    pub fn run_status(&self) -> &'static str {
        enummap::pretty_run_status(self.status(), self.has_managed_save())
    }
    /// `run_status_reason`.
    pub fn run_status_reason(&self) -> Option<&'static str> {
        self.snap(|s| enummap::pretty_status_reason(s.state, s.reason))
    }
    pub fn is_crashed(&self) -> bool {
        enummap::is_crashed(self.status())
    }
    pub fn run_status_icon_name(&self) -> &'static str {
        enummap::run_status_icon_name(self.status())
    }
    pub fn is_active(&self) -> bool {
        enummap::is_active(self.status())
    }
    pub fn is_runable(&self) -> bool {
        enummap::is_runable(self.status())
    }
    pub fn is_paused(&self) -> bool {
        enummap::is_paused(self.status())
    }
    pub fn is_pauseable(&self) -> bool {
        enummap::is_pauseable(self.status())
    }
    pub fn is_unpauseable(&self) -> bool {
        enummap::is_unpauseable(self.status())
    }
    pub fn is_stoppable(&self) -> bool {
        enummap::is_stoppable(self.status())
    }
    pub fn is_destroyable(&self) -> bool {
        enummap::is_destroyable(self.status())
    }
    pub fn is_cloneable(&self) -> bool {
        enummap::is_cloneable(self.status())
    }

    /// The last fetched XML; `None` until [`Self::refresh_xml`] ran once.
    pub fn xmlobj(&self) -> Option<Rc<DomainXml>> {
        self.imp().xmlobj.borrow().clone()
    }

    /// Re-reads the domain XML on a worker (upstream refreshes it on
    /// libvirt's XML-changed events; without libvirt-glib this happens
    /// when the VM window shows or the state changes). Returns whether
    /// it changed.
    pub async fn refresh_xml(&self) -> Result<bool, String> {
        let conn = self.conn().ok_or("Connection closed")?;
        let handle = conn.handle().ok_or("Not connected")?;
        let uuid = self.get_uuid();
        let xml = gio::spawn_blocking(move || {
            let dom = Domain::lookup_by_uuid_string(handle.connect(), &uuid)?;
            dom.get_xml_desc(0)
        })
        .await
        .map_err(|_| "Worker thread panicked".to_string())?
        .map_err(|e| e.message().to_string())?;
        if self.xmlobj().is_some_and(|x| x.xml == xml) {
            return Ok(false);
        }
        let parsed = DomainXml::parse(xml)?;
        self.imp().xmlobj.replace(Some(Rc::new(parsed)));
        Ok(true)
    }

    /// `get_domain_capabilities`, fetched once per domain from the
    /// emulator/arch/machine/type in its XML.
    pub async fn get_domain_capabilities(&self) -> Option<Rc<DomainCapabilities>> {
        if let Some(caps) = self.imp().domcaps.borrow().clone() {
            return Some(caps);
        }
        let x = self.xmlobj()?;
        let handle = self.conn()?.handle()?;
        let emulator = x.emulator();
        let arch = x.guest.os.arch.clone();
        let machine = x.guest.os.machine.clone();
        let virttype = x.guest.domain_type.clone();
        let xml = gio::spawn_blocking(move || {
            handle.connect().get_domain_capabilities(
                emulator.as_deref(),
                arch.as_deref(),
                machine.as_deref(),
                virttype.as_deref(),
                0,
            )
        })
        .await
        .ok()?
        .ok()?;
        let caps = Rc::new(DomainCapabilities::parse(&xml).ok()?);
        self.imp().domcaps.replace(Some(caps.clone()));
        Some(caps)
    }

    /// `get_pretty_hv_type`.
    pub fn emit_state_changed(&self) {
        self.emit_by_name::<()>("state-changed", &[]);
    }

    fn handle(&self) -> Result<std::sync::Arc<crate::connection::ConnHandle>, String> {
        let conn = self.conn().ok_or("Connection closed")?;
        conn.handle().ok_or_else(|| "Not connected".to_string())
    }

    /// Runs `f` on the domain, looked up by UUID, on a worker thread.
    async fn with_domain<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connect, &Domain) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let handle = self.handle()?;
        let uuid = self.get_uuid();
        gio::spawn_blocking(move || {
            let conn = handle.connect();
            let dom =
                Domain::lookup_by_uuid_string(conn, &uuid).map_err(|e| e.message().to_string())?;
            f(conn, &dom)
        })
        .await
        .map_err(|_| "Worker thread panicked".to_string())?
    }

    /// `_make_xmlobj_to_define`: the inactive XML, for editing.
    pub async fn make_xmlobj_to_define(&self) -> Result<XmlToDefine, String> {
        let xml = self
            .with_domain(|_, dom| {
                dom.get_xml_desc(sys::VIR_DOMAIN_XML_INACTIVE | sys::VIR_DOMAIN_XML_SECURE)
                    .map_err(|e| e.message().to_string())
            })
            .await?;
        XmlToDefine::parse(xml)
    }

    /// `_redefine_xmlobj` / `_redefine_xml_internal`: define the edited
    /// XML, then pick it up (there are no libvirt events to do it).
    pub async fn redefine_xmlobj(&self, mut xmlobj: XmlToDefine) -> Result<(), String> {
        let newxml = xmlobj.get_xml()?;
        if newxml == xmlobj.origxml {
            eprintln!(
                "Redefine requested for {}, but XML didn't change!",
                self.get_name()
            );
        }
        let handle = self.handle()?;
        gio::spawn_blocking(move || {
            Domain::define_xml(handle.connect(), &newxml)
                .map(|_| ())
                .map_err(|e| e.message().to_string())
        })
        .await
        .map_err(|_| "Worker thread panicked".to_string())??;
        self.ensure_latest_xml().await;
        Ok(())
    }

    /// `ensure_latest_xml` + `idle_emit("state-changed")`.
    async fn ensure_latest_xml(&self) {
        let _ = self.refresh_xml().await;
        self.emit_state_changed();
        if let Some(conn) = self.conn() {
            conn.tick();
        }
    }

    /// `define_overview`: `None` leaves a field alone, `Some(None)`
    /// clears it.
    pub async fn define_overview(
        &self,
        description: Option<Option<String>>,
        title: Option<Option<String>>,
        nvram: Option<String>,
    ) -> Result<(), String> {
        let mut guest = self.make_xmlobj_to_define().await?;
        if let Some(description) = description {
            guest.guest.description = description.filter(|s| !s.is_empty());
        }
        if let Some(title) = title {
            guest.guest.title = title.filter(|s| !s.is_empty());
        }
        if let Some(nvram) = nvram {
            guest.guest.os.nvram = Some(nvram);
            guest.write_os();
        }
        self.redefine_xmlobj(guest).await
    }

    /// `hotplug(description=..., title=...)`: the live metadata.
    pub async fn hotplug_metadata(
        &self,
        description: Option<String>,
        title: Option<String>,
    ) -> Result<(), String> {
        if !self.is_active() {
            return Ok(());
        }
        self.with_domain(move |_, dom| {
            let flags = sys::VIR_DOMAIN_AFFECT_LIVE | sys::VIR_DOMAIN_AFFECT_CONFIG;
            let set = |kind: u32, val: &str| {
                dom.set_metadata(kind as i32, Some(val), None, None, flags)
                    .map(|_| ())
                    .map_err(|e| e.message().to_string())
            };
            if let Some(d) = &description {
                set(sys::VIR_DOMAIN_METADATA_DESCRIPTION, d)?;
            }
            if let Some(t) = &title {
                set(sys::VIR_DOMAIN_METADATA_TITLE, t)?;
            }
            Ok(())
        })
        .await?;
        self.ensure_latest_xml().await;
        Ok(())
    }

    /// `rename_domain`: validate, carry the NVRAM file over to the new
    /// name, then `define_name`.
    pub async fn rename_domain(&self, new_name: &str) -> Result<(), String> {
        if new_name == self.get_name() {
            return Ok(());
        }
        validate_generic_name("Guest", new_name)?;
        let name = new_name.to_string();
        self.with_domain(move |conn, _| match Domain::lookup_by_name(conn, &name) {
            Ok(_) => Err(format!("Guest name '{name}' is already in use.")),
            Err(_) => Ok(()),
        })
        .await?;

        let xmlobj = self.make_xmlobj_to_define().await?;
        let has_nvram = xmlobj.is_uefi() || xmlobj.guest.os.nvram.is_some();
        let has_tpm_state = xmlobj
            .guest
            .devices
            .tpms
            .iter()
            .any(|t| t.backend_type.as_deref() == Some("emulator"));
        let uri = self.conn().map(|c| c.get_uri()).unwrap_or_default();
        let parsed = virtinst_core::connection::Uri::parse(&uri);
        let is_local = !parsed.is_remote();
        let old_nvram_path = xmlobj.guest.os.nvram.clone().unwrap_or_else(|| {
            // Probably using firmware=efi which doesn't put nvram
            // path in the XML on older libvirt. Build the implied path
            let root = if parsed.path == "/session" {
                std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| {
                    glib::home_dir()
                        .join(".local/share/libvirt")
                        .to_string_lossy()
                        .into_owned()
                })
            } else {
                "/var/lib/libvirt".to_string()
            };
            let driver = parsed
                .scheme
                .split('+')
                .next()
                .unwrap_or("qemu")
                .to_string();
            format!("{root}/{driver}/nvram/{}_VARS.fd", self.get_name())
        });

        let name = new_name.to_string();
        let renamed = self
            .with_domain(move |conn, dom| {
                let copied = if has_nvram {
                    copy_nvram_file(conn, &old_nvram_path, &name, is_local)?
                } else {
                    None
                };
                let res = define_name(conn, dom, &name, has_nvram, has_tpm_state);
                if let Err(e) = res {
                    if let Some((new, _)) = &copied {
                        if let Err(warn) = new.delete() {
                            eprintln!("rename failed and new nvram was not removed: '{warn}'");
                        }
                    }
                    return Err(e);
                }
                let Some((new, old)) = copied else {
                    return Ok(None);
                };
                if let Err(warn) = old.delete() {
                    eprintln!("old nvram file was not removed: '{warn}'");
                }
                Ok(Some(new.path()))
            })
            .await;
        self.ensure_latest_xml().await;
        if let Some(new_path) = renamed? {
            self.define_overview(None, None, Some(new_path)).await?;
        }
        Ok(())
    }

    pub fn get_pretty_hv_type(&self) -> String {
        let Some(x) = self.xmlobj() else {
            return String::new();
        };
        let gtype = x.guest.os.os_type.clone().unwrap_or_default();
        let domtype = x.guest.domain_type.clone().unwrap_or_default();
        pretty_hv(&gtype, &domtype)
    }

    // Per-VM console settings: -1 means "use the global preference".
    fn pervm(&self) -> gio::Settings {
        crate::engine::get().config().vm_settings(&self.get_uuid())
    }

    pub fn get_console_scaling(&self) -> i32 {
        match self.pervm().int("scaling") {
            -1 => crate::engine::get().config().get_console_scaling(),
            v => v,
        }
    }
    pub fn set_console_scaling(&self, val: i32) {
        let _ = self.pervm().set_int("scaling", val);
    }
    pub fn on_console_scaling_changed(&self, cb: impl Fn() + 'static) -> glib::SignalHandlerId {
        self.pervm()
            .connect_changed(Some("scaling"), move |_, _| cb())
    }

    pub fn get_console_resizeguest(&self) -> i32 {
        match self.pervm().int("resize-guest") {
            -1 => crate::engine::get().config().get_console_resizeguest(),
            v => v,
        }
    }
    pub fn set_console_resizeguest(&self, val: i32) {
        let _ = self.pervm().set_int("resize-guest", val);
    }
    pub fn on_console_resizeguest_changed(&self, cb: impl Fn() + 'static) -> glib::SignalHandlerId {
        self.pervm()
            .connect_changed(Some("resize-guest"), move |_, _| cb())
    }

    pub fn get_console_autoconnect(&self) -> bool {
        match self.pervm().int("autoconnect") {
            -1 => crate::engine::get().config().get_console_autoconnect(),
            v => v != 0,
        }
    }
    pub fn set_console_autoconnect(&self, val: bool) {
        let _ = self.pervm().set_int("autoconnect", val as i32);
    }
    /// Upstream listens on `/resize-guest` here, a copy/paste slip; this
    /// listens on the key it's named after.
    pub fn on_console_autoconnect_changed(&self, cb: impl Fn() + 'static) -> glib::SignalHandlerId {
        self.pervm()
            .connect_changed(Some("autoconnect"), move |_, _| cb())
    }

    pub fn get_details_window_size(&self) -> (i32, i32) {
        self.pervm().get::<(i32, i32)>("vm-window-size")
    }
    pub fn set_details_window_size(&self, w: i32, h: i32) {
        let _ = self.pervm().set("vm-window-size", (w, h));
    }

    /// Runs `action` on a worker thread, then schedules a priority tick
    /// so the list reflects the new state without waiting a full
    /// interval. The error string is libvirt's message, for
    /// `asyncjob::simple_async_noshow`'s dialog.
    pub async fn run_action(&self, action: DomainAction) -> Result<(), String> {
        let conn = self.conn().ok_or("Connection closed")?;
        let handle = conn.handle().ok_or("Not connected")?;
        let uuid = self.get_uuid();
        let res = gio::spawn_blocking(move || {
            let dom = Domain::lookup_by_uuid_string(handle.connect(), &uuid)?;
            action.run(&dom)
        })
        .await
        .map_err(|_| "Worker thread panicked".to_string())?;
        conn.tick();
        res.map_err(|e| e.message().to_string())
    }

    /// `open_graphics_fd`: a connected socket to the `idx`th graphical
    /// console (`virDomainOpenGraphicsFD`), which the caller owns.
    /// `open_console`: a new non-blocking stream on the named console
    /// device (the first one when `None`). The caller owns the stream.
    pub fn open_console(&self, devname: Option<&str>) -> Result<sys::virStreamPtr, String> {
        let conn = self.conn().ok_or("Connection closed")?;
        let handle = conn.handle().ok_or("Not connected")?;
        let dom = Domain::lookup_by_uuid_string(handle.connect(), &self.get_uuid())
            .map_err(|e| e.message().to_string())?;
        let name = devname.and_then(|n| std::ffi::CString::new(n).ok());
        let last_error = || virt::error::Error::last_error().message().to_string();
        unsafe {
            let stream = sys::virStreamNew(handle.connect().as_ptr(), sys::VIR_STREAM_NONBLOCK);
            if stream.is_null() {
                return Err(last_error());
            }
            let ret = sys::virDomainOpenConsole(
                dom.as_ptr(),
                name.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()),
                stream,
                0,
            );
            if ret < 0 {
                let err = last_error();
                sys::virStreamFree(stream);
                return Err(err);
            }
            Ok(stream)
        }
    }

    pub async fn open_graphics_fd(&self, idx: u32) -> Result<i32, String> {
        let conn = self.conn().ok_or("Connection closed")?;
        let handle = conn.handle().ok_or("Not connected")?;
        let uuid = self.get_uuid();
        gio::spawn_blocking(move || {
            let dom = Domain::lookup_by_uuid_string(handle.connect(), &uuid)?;
            dom.open_graphics_fd(idx, 0)
        })
        .await
        .map_err(|_| "Worker thread panicked".to_string())?
        .map(|fd| fd as i32)
        .map_err(|e| e.message().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `test:///default` is one shared in-process driver instance, so
    /// tests that change its domains' state must not overlap.
    static TEST_DRIVER: Mutex<()> = Mutex::new(());

    #[test]
    fn validate_generic_name_like_upstream() {
        assert!(validate_generic_name("Guest", "ok-name").is_ok());
        assert_eq!(
            validate_generic_name("Guest", "").unwrap_err(),
            "A name must be specified for the Guest"
        );
        assert_eq!(
            validate_generic_name("Guest", "a/b").unwrap_err(),
            "Guest name 'a/b' can not contain '/' character."
        );
    }

    #[test]
    fn xml_to_define_edits_round_trip() {
        let xml = "<domain type='test'><name>x</name><title>t</title>\
                   <os><type>hvm</type><loader type='pflash'>/l.fd</loader></os></domain>";
        let mut x = XmlToDefine::parse(xml.to_string()).unwrap();
        assert!(x.is_uefi());
        x.guest.title = None;
        x.guest.description = Some("d".into());
        x.guest.os.nvram = Some("/n.fd".into());
        x.write_os();
        let out = x.get_xml().unwrap();
        assert!(!out.contains("<title>"), "{out}");
        assert!(out.contains("<description>d</description>"), "{out}");
        assert!(out.contains("<nvram>/n.fd</nvram>"), "{out}");
        assert!(out.contains("pflash"), "{out}");
    }

    #[test]
    fn local_nvram_copy_uses_the_new_name() {
        let dir = std::env::temp_dir().join(format!("vmm-nvram-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let old = dir.join("old_VARS.fd");
        std::fs::write(&old, b"vars").unwrap();
        let mut conn = Connect::open(Some("test:///default")).unwrap();
        let (new, oldf) = copy_nvram_file(&conn, old.to_str().unwrap(), "new", true)
            .unwrap()
            .expect("copied");
        assert_eq!(new.path(), dir.join("new_VARS.fd").to_str().unwrap());
        assert_eq!(std::fs::read(new.path()).unwrap(), b"vars");
        oldf.delete().unwrap();
        assert!(!old.exists());
        new.delete().unwrap();
        let _ = std::fs::remove_dir(&dir);
        conn.close().unwrap();
    }

    #[test]
    fn define_name_renames_and_keeps_autostart() {
        let _guard = TEST_DRIVER.lock().unwrap_or_else(|e| e.into_inner());
        let mut conn = Connect::open(Some("test:///default")).unwrap();
        let xml = "<domain type='test'><name>vmm-rename-a</name><memory>1024</memory>\
                   <os><type>hvm</type></os></domain>";
        let dom = Domain::define_xml(&conn, xml).unwrap();
        dom.set_autostart(true).unwrap();
        define_name(&conn, &dom, "vmm-rename-b", false, false).unwrap();
        assert!(Domain::lookup_by_name(&conn, "vmm-rename-a").is_err());
        let renamed = Domain::lookup_by_name(&conn, "vmm-rename-b").unwrap();
        assert!(renamed.get_autostart().unwrap());
        renamed.undefine().unwrap();
        conn.close().unwrap();
    }

    /// libvirt's built-in `test:///default` driver ships one running
    /// domain called `test`.
    #[test]
    fn fetch_all_against_the_builtin_test_driver() {
        let _guard = TEST_DRIVER.lock().unwrap_or_else(|e| e.into_inner());
        let mut conn = Connect::open(Some("test:///default")).expect("test driver opens");
        let snaps = DomainSnapshot::fetch_all(&conn).expect("listing works");
        let test = snaps
            .iter()
            .find(|s| s.name == "test")
            .expect("default test domain");
        assert_eq!(test.state, sys::VIR_DOMAIN_RUNNING);
        assert!(test.id.is_some());
        assert_eq!(test.uuid.len(), 36);
        conn.close().unwrap();
    }

    #[test]
    fn actions_change_state_on_the_test_driver() {
        let _guard = TEST_DRIVER.lock().unwrap_or_else(|e| e.into_inner());
        let mut conn = Connect::open(Some("test:///default")).expect("test driver opens");
        let dom = Domain::lookup_by_name(&conn, "test").unwrap();
        DomainAction::Suspend.run(&dom).unwrap();
        assert_eq!(dom.get_state().unwrap().0, sys::VIR_DOMAIN_PAUSED);
        DomainAction::Resume.run(&dom).unwrap();
        DomainAction::Destroy.run(&dom).unwrap();
        assert_eq!(dom.get_state().unwrap().0, sys::VIR_DOMAIN_SHUTOFF);
        DomainAction::Startup.run(&dom).unwrap();
        assert_eq!(dom.get_state().unwrap().0, sys::VIR_DOMAIN_RUNNING);
        drop(dom);
        conn.close().unwrap();
    }
}
