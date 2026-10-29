//! `device/addstorage.py` (`vmmAddStorage`): the new disk's storage
//! (a fresh image in the default pool, or a path to pick or create),
//! and the advanced options box the details disk page shares.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use crate::connection::VmmConnection;
use crate::devpages::SimpleCombo;

/// `_EDIT_*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StorageEdit {
    Cache,
    Discard,
    Readonly,
    Shareable,
    Removable,
    Serial,
}

/// `get_values()`: the advanced options the user changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StorageValues {
    pub cache: Option<Option<String>>,
    pub discard: Option<Option<String>>,
    pub readonly: Option<bool>,
    pub shareable: Option<bool>,
    pub removable: Option<bool>,
    pub serial: Option<String>,
}

/// `pretty_storage`: "Unknown GiB" for -1.
fn pretty_storage(size: f64) -> String {
    if size < 0.0 {
        return "Unknown GiB".into();
    }
    format!("{size:.1} GiB")
}

fn mode_combo(modes: &[&str]) -> SimpleCombo {
    let mut values = vec![(None, "Hypervisor default".to_string())];
    values.extend(modes.iter().map(|m| (Some(m.to_string()), m.to_string())));
    let combo = SimpleCombo::unsorted(values, str::to_string);
    combo.set(None);
    combo
}

type Callback = Box<dyn Fn()>;
type BrowseCallback = Box<dyn Fn(&gtk::Entry)>;

pub struct AddStorage {
    /// `storage-box`.
    pub top_box: gtk::Box,
    /// `storage-advanced-box`.
    pub advanced_top_box: gtk::Box,
    conn: RefCell<Option<VmmConnection>>,
    storage_create: gtk::CheckButton,
    storage_create_box: gtk::Box,
    storage_size: gtk::SpinButton,
    phys_hd_label: gtk::Label,
    storage_select: gtk::CheckButton,
    storage_browse_box: gtk::Box,
    storage_entry: gtk::Entry,
    storage_advanced: gtk::Expander,
    cache: SimpleCombo,
    discard: SimpleCombo,
    readonly: gtk::CheckButton,
    shareable: gtk::CheckButton,
    removable: gtk::CheckButton,
    removable_label: gtk::Label,
    serial: gtk::Entry,
    active_edits: RefCell<Vec<StorageEdit>>,
    /// Set while `set_dev` fills the widgets, so they aren't edits.
    quiet: Cell<bool>,
    changed: RefCell<Option<Callback>>,
    browse_clicked: RefCell<Option<BrowseCallback>>,
}

impl AddStorage {
    pub fn new(conn: Option<VmmConnection>) -> Rc<AddStorage> {
        let mnemonic = |text: &str| {
            let l = gtk::Label::with_mnemonic(text);
            l.set_xalign(0.0);
            l
        };

        // storage-box
        let storage_create = gtk::CheckButton::new();
        storage_create.set_child(Some(&mnemonic(
            "C_reate a disk image for the virtual machine",
        )));
        let adjustment = gtk::Adjustment::new(20.0, 0.0, 1_000_000.0, 0.1, 10.0, 0.0);
        let storage_size = gtk::SpinButton::new(Some(&adjustment), 1.0, 1);
        storage_size.update_property(&[gtk::accessible::Property::Label("storage-size")]);
        let gib = gtk::Label::with_mnemonic("_GiB");
        gib.set_mnemonic_widget(Some(&storage_size));
        let size_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        size_box.append(&storage_size);
        size_box.append(&gib);
        let phys_hd_label = gtk::Label::new(Some("Free Space"));
        phys_hd_label.set_xalign(0.0);
        let create_vbox = gtk::Box::new(gtk::Orientation::Vertical, 6);
        create_vbox.set_margin_start(22);
        create_vbox.append(&size_box);
        create_vbox.append(&phys_hd_label);
        let storage_create_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        storage_create_box.append(&storage_create);
        storage_create_box.append(&create_vbox);

        let storage_select = gtk::CheckButton::new();
        storage_select.set_child(Some(&mnemonic("_Select or create custom storage")));
        storage_select.set_group(Some(&storage_create));
        let storage_browse = gtk::Button::with_mnemonic("_Manage...");
        let storage_entry = gtk::Entry::new();
        storage_entry.set_hexpand(true);
        storage_entry.update_property(&[gtk::accessible::Property::Label("storage-entry")]);
        let storage_browse_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        storage_browse_box.set_margin_start(22);
        storage_browse_box.append(&storage_browse);
        storage_browse_box.append(&storage_entry);
        storage_browse_box.set_sensitive(false);

        let top_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        top_box.append(&storage_create_box);
        top_box.append(&storage_select);
        top_box.append(&storage_browse_box);

        // storage-advanced-box
        let cache = mode_combo(&virtinst_core::devices::disk::CACHE_MODES);
        let discard = mode_combo(&virtinst_core::devices::disk::DISCARD_MODES);
        let readonly = gtk::CheckButton::new();
        let shareable = gtk::CheckButton::new();
        let removable = gtk::CheckButton::new();
        let serial = gtk::Entry::new();
        let grid = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(6)
            .margin_start(12)
            .margin_top(6)
            .build();
        let mut row = 0;
        let mut add = |text: &str, w: &gtk::Widget| {
            let l = gtk::Label::with_mnemonic(text);
            l.set_halign(gtk::Align::End);
            l.set_mnemonic_widget(Some(w));
            grid.attach(&l, 0, row, 1, 1);
            grid.attach(w, 1, row, 1, 1);
            row += 1;
            l
        };
        add("R_eadonly:", readonly.upcast_ref());
        add("Sharea_ble:", shareable.upcast_ref());
        let removable_label = add("Removab_le:", removable.upcast_ref());
        add("Seria_l:", serial.upcast_ref());
        add("Cac_he mode:", cache.widget.upcast_ref());
        add("Discard mod_e:", discard.widget.upcast_ref());
        let storage_advanced = gtk::Expander::with_mnemonic("Advanced _options");
        storage_advanced.set_child(Some(&grid));
        let advanced_top_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        advanced_top_box.append(&storage_advanced);

        let this = Rc::new(AddStorage {
            top_box,
            advanced_top_box,
            conn: RefCell::new(conn),
            storage_create,
            storage_create_box,
            storage_size,
            phys_hd_label,
            storage_select,
            storage_browse_box,
            storage_entry,
            storage_advanced,
            cache,
            discard,
            readonly,
            shareable,
            removable,
            removable_label,
            serial,
            active_edits: RefCell::default(),
            quiet: Cell::new(false),
            changed: RefCell::default(),
            browse_clicked: RefCell::default(),
        });

        let edit = |e: StorageEdit| {
            let weak = Rc::downgrade(&this);
            move || {
                if let Some(this) = weak.upgrade() {
                    this.change_cb(e);
                }
            }
        };
        let cb = edit(StorageEdit::Cache);
        this.cache.widget.connect_selected_notify(move |_| cb());
        let cb = edit(StorageEdit::Discard);
        this.discard.widget.connect_selected_notify(move |_| cb());
        let cb = edit(StorageEdit::Readonly);
        this.readonly.connect_toggled(move |_| cb());
        let cb = edit(StorageEdit::Shareable);
        this.shareable.connect_toggled(move |_| cb());
        let cb = edit(StorageEdit::Removable);
        this.removable.connect_toggled(move |_| cb());
        let cb = edit(StorageEdit::Serial);
        this.serial.connect_changed(move |_| cb());

        let weak = Rc::downgrade(&this);
        storage_browse.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                if let Some(cb) = &*this.browse_clicked.borrow() {
                    cb(&this.storage_entry);
                }
            }
        });
        let weak = Rc::downgrade(&this);
        this.storage_select.connect_toggled(move |src| {
            if let Some(this) = weak.upgrade() {
                this.storage_browse_box.set_sensitive(src.is_active());
            }
        });
        this.storage_create.set_active(true);
        this
    }

    /// The `changed` signal.
    pub fn connect_changed(&self, cb: impl Fn() + 'static) {
        self.changed.replace(Some(Box::new(cb)));
    }

    /// The `browse-clicked` signal, with the entry to fill.
    pub fn connect_browse_clicked(&self, cb: impl Fn(&gtk::Entry) + 'static) {
        self.browse_clicked.replace(Some(Box::new(cb)));
    }

    /// `_change_cb`.
    fn change_cb(&self, edit: StorageEdit) {
        if self.quiet.get() {
            return;
        }
        let mut edits = self.active_edits.borrow_mut();
        if !edits.contains(&edit) {
            edits.push(edit);
        }
        drop(edits);
        if let Some(cb) = &*self.changed.borrow() {
            cb();
        }
    }

    /// `_update_host_space`: the default pool's free space, fetched on
    /// a worker.
    fn update_host_space(&self) {
        let label = self.phys_hd_label.clone();
        let set = |label: &gtk::Label, size: f64| {
            let text = format!("{} available in the default location", pretty_storage(size));
            label.set_markup(&format!("<span>{}</span>", glib::markup_escape_text(&text)));
        };
        set(&label, -1.0);
        let Some(conn) = self.conn.borrow().clone() else {
            return;
        };
        glib::spawn_future_local(async move {
            if let Some(avail) = conn.default_pool_available().await {
                set(&label, avail as f64 / 1024.0 / 1024.0 / 1024.0);
            }
        });
    }

    pub fn set_conn(&self, conn: Option<VmmConnection>) {
        self.conn.replace(conn);
    }

    /// `storage-size`, in GiB.
    pub fn set_size(&self, size: f64) {
        self.storage_size.set_value(size);
    }

    /// `check_path_search`: when the emulator may not be able to search
    /// the directories of `path`, offer to fix them (unless told not to
    /// ask about them again), then list the ones that couldn't be.
    /// `parent` is the window whose `err` upstream uses (`src`).
    pub async fn check_path_search(
        parent: Option<&gtk::Window>,
        conn: &VmmConnection,
        path: Option<&str>,
    ) {
        use crate::error;
        use virtinst_core::devices::disk;

        let config = crate::engine::get().config();
        let skip_paths = config.get_perms_fix_ignore();
        let Some(handle) = conn.handle() else {
            return;
        };
        // `conn.get_backend()`'s URI. The test suite's `fakeuri`
        // stands in for it, as in virtinst's `Connection::open`.
        let uri = std::env::var("VIRTINST_TEST_FAKE_URI").unwrap_or_else(|_| conn.get_uri());
        let uri = virtinst_core::connection::Uri::parse(&uri);
        let search_path = path.map(str::to_string);
        let searchdata = gio::spawn_blocking(move || {
            disk::check_path_search(handle.connect(), &uri, search_path.as_deref())
        })
        .await
        .unwrap_or_default();

        let broken_paths: Vec<String> = searchdata
            .fixlist
            .iter()
            .filter(|p| !skip_paths.contains(p))
            .cloned()
            .collect();

        if broken_paths.is_empty() {
            return;
        }

        glib::g_debug!(
            "virt-manager",
            "No search access for dirs: {}",
            crate::addhardware::py_list(&broken_paths)
        );
        let (resp, chkres) = error::warn_chkbox(
            parent,
            &format!(
                "The emulator may not have search permissions for the path '{}'.",
                path.unwrap_or("None")
            ),
            Some("Do you want to correct this now?"),
            "Don't ask about these directories again.",
            error::Buttons::YesNo,
        )
        .await;

        if chkres {
            config.add_perms_fix_ignore(&broken_paths);
        }
        if !resp {
            return;
        }

        glib::g_debug!("virt-manager", "Attempting to correct permission issues.");
        let errors = gio::spawn_blocking(move || disk::fix_path_search(&searchdata))
            .await
            .unwrap_or_default();
        if errors.is_empty() {
            return;
        }

        let errmsg = "Errors were encountered changing permissions for the following directories:";
        let mut details = String::new();
        for (p, error) in &errors {
            if broken_paths.contains(p) {
                details += &format!("{p} : {error}\n");
            }
        }
        details += "\nIt is very likely the VM will fail to start up.";

        glib::g_debug!("virt-manager", "Permission errors:\n{details}");

        let (_ignore, chkres) = error::err_chkbox(
            parent,
            errmsg,
            Some(&details),
            "Don't ask about these directories again.",
            error::Buttons::Ok,
        )
        .await;

        if chkres {
            let paths: Vec<String> = errors.into_iter().map(|(p, _)| p).collect();
            config.add_perms_fix_ignore(&paths);
        }
    }

    /// `reset_state`.
    pub fn reset_state(&self) {
        self.update_host_space();
        self.quiet.set(true);
        self.storage_create.set_active(true);
        self.storage_size.set_value(20.0);
        self.storage_entry.set_text("");
        self.storage_create_box.set_sensitive(true);
        self.cache.widget.set_selected(0);
        self.discard.widget.set_selected(0);
        self.serial.set_text("");
        self.storage_advanced.set_expanded(false);
        self.readonly.set_active(false);
        self.shareable.set_active(false);
        self.removable.set_active(false);
        self.removable.set_visible(false);
        self.removable_label.set_visible(false);
        self.quiet.set(false);
        self.active_edits.borrow_mut().clear();
    }

    /// `storage-create-box`'s sensitivity: only disks and LUNs get a
    /// new image; the others must pick a path.
    pub fn set_allow_create(&self, allow_create: bool) {
        self.storage_create_box.set_sensitive(allow_create);
        if !allow_create {
            self.storage_select.set_active(true);
        }
    }

    /// `is_default_storage`.
    pub fn is_default_storage(&self) -> bool {
        self.storage_create.is_active()
    }

    /// The custom path, stripped.
    pub fn entry_path(&self) -> String {
        self.storage_entry.text().trim().to_string()
    }

    /// The size to create, in GiB.
    pub fn size(&self) -> f64 {
        self.storage_size.update();
        self.storage_size.value()
    }

    /// `set_disk_bus`: removable only applies to USB.
    pub fn set_disk_bus(&self, bus: Option<&str>) {
        let show = bus == Some("usb");
        self.removable.set_visible(show);
        self.removable_label.set_visible(show);
    }

    /// `set_dev`, from the disk's XML values.
    #[allow(clippy::too_many_arguments)]
    pub fn set_dev(
        &self,
        bus: Option<&str>,
        cache: Option<&str>,
        discard: Option<&str>,
        serial: &str,
        readonly: bool,
        is_cdrom: bool,
        shareable: bool,
        removable: bool,
    ) {
        self.quiet.set(true);
        self.set_disk_bus(bus);
        self.cache.set(cache);
        self.discard.set(discard);
        self.serial.set_text(serial);
        self.readonly.set_active(readonly);
        self.readonly.set_sensitive(!is_cdrom);
        self.shareable.set_active(shareable);
        self.removable.set_active(removable);
        self.quiet.set(false);
        // This comes last
        self.active_edits.borrow_mut().clear();
    }

    /// `get_values`.
    pub fn get_values(&self) -> StorageValues {
        let mut ret = StorageValues::default();
        for e in self.active_edits.borrow().iter() {
            match e {
                StorageEdit::Cache => ret.cache = Some(self.cache.get()),
                StorageEdit::Discard => ret.discard = Some(self.discard.get()),
                StorageEdit::Readonly => ret.readonly = Some(self.readonly.is_active()),
                StorageEdit::Shareable => ret.shareable = Some(self.shareable.is_active()),
                StorageEdit::Removable => ret.removable = Some(self.removable.is_active()),
                StorageEdit::Serial => ret.serial = Some(self.serial.text().to_string()),
            }
        }
        if !self.removable.is_visible() {
            ret.removable = None;
        }
        ret
    }
}

/// What the storage page's worker found: `vmmAddStorage.build_device`
/// up to the disk's XML.
pub(crate) struct DiskPlan {
    pub path: Option<String>,
    pub props: Option<virtinst_core::devices::disk::PathProps>,
    pub vol_install: Option<virtinst_core::storage::VolInstall>,
}

/// What [`plan_disk`] needs from the page.
pub(crate) struct DiskRequest {
    pub vmname: String,
    pub is_default_storage: bool,
    pub entry_path: String,
    pub size: f64,
    pub fmt: Option<String>,
    pub collide_paths: Vec<String>,
    pub is_session: bool,
    pub is_local: bool,
}

/// Blocking `get_default_path`: a free image name for `vmname` in the
/// default pool, `None` without one.
pub(crate) fn get_default_path(
    conn: &virt::connect::Connect,
    vmname: &str,
    fmt: Option<&str>,
    collide_paths: &[String],
    is_session: bool,
) -> Result<Option<String>, String> {
    use virtinst_core::storage;
    let Some(pool) = storage::lookup_default_pool(conn, is_session) else {
        return Ok(None);
    };
    let suffix = storage::file_extension_for_format(fmt);
    let suffix = if suffix.is_empty() {
        ".img".into()
    } else {
        suffix
    };
    let name = storage::find_free_name(&pool, vmname, &suffix, collide_paths)?;
    let target = pool.xml.target_path.clone().unwrap_or_default();
    Ok(Some(format!("{}/{name}", target.trim_end_matches('/'))))
}

/// Blocking: `get_default_path`, `set_source_path` (`manage_path`) and,
/// for storage that doesn't exist yet (`wants_storage_creation`),
/// `build_vol_install`. The volume is validated later, by
/// [`validate_disk`].
pub(crate) fn plan_disk(
    conn: &virt::connect::Connect,
    req: &DiskRequest,
) -> Result<DiskPlan, String> {
    use virtinst_core::storage;
    let path = if req.is_default_storage {
        get_default_path(
            conn,
            &req.vmname,
            req.fmt.as_deref(),
            &req.collide_paths,
            req.is_session,
        )?
    } else {
        Some(req.entry_path.clone()).filter(|p| !p.is_empty())
    };
    let Some(path) = path else {
        return Ok(DiskPlan {
            path: None,
            props: None,
            vol_install: None,
        });
    };
    let path = path.strip_prefix("file://").unwrap_or(&path).to_string();
    let managed = storage::manage_path(conn, &path)?;
    if let Some(vol) = &managed.vol {
        return Ok(DiskPlan {
            path: Some(managed.path),
            props: Some(storage::vol_path_props(vol)),
            vol_install: None,
        });
    }
    // `StorageBackend.exists()`: on a remote connection, a path we
    // don't auto-manage (`/dev/sdX`) counts as existing
    let backend = virtinst_core::diskbackend::StorageBackend::new(
        Some(managed.path.clone()),
        None,
        managed.pool,
        !req.is_local,
    );
    if backend.exists() {
        let props = virtinst_core::devices::disk::PathProps {
            dev_type: backend.get_dev_type(),
            format: backend.get_driver_type(),
        };
        return Ok(DiskPlan {
            path: Some(managed.path),
            props: Some(props),
            vol_install: None,
        });
    }
    // `wants_storage_creation`: if the user changed the default disk
    // format to raw, assume they want to maximize performance, so fully
    // allocate the disk image. Otherwise use sparse
    let sparse = req.fmt.as_deref() != Some("raw");
    let volname = managed.path.rsplit('/').next().unwrap_or_default();
    let pool = backend.pool.as_ref().map(|p| &p.xml);
    let mut vol = storage::VolInstall::new(volname, pool, req.size, sparse, None)?;
    if vol.supports_format() {
        glib::g_debug!(
            "virt-manager",
            "Using default prefs format={:?} for path={}",
            req.fmt,
            managed.path
        );
        vol.format = req.fmt.clone().or(vol.format);
    }
    Ok(DiskPlan {
        path: Some(managed.path),
        props: Some(vol.path_props()),
        vol_install: Some(vol),
    })
}

/// Blocking `DeviceDisk.validate`, past `validate_device`'s empty path
/// check: the volume to create is validated (its name and size,
/// `ManagedStorageCreator.validate`); a path from the XML tab
/// (`set_backend_for_existing_path`) has to exist; an empty disk whose
/// `<source pool= volume=>` didn't resolve fails with that error.
pub(crate) fn validate_disk(
    conn: &virt::connect::Connect,
    vol_install: Option<&virtinst_core::storage::VolInstall>,
    existing_path: Option<&str>,
    source_volume_err: Option<&str>,
    remote: bool,
) -> Result<(), String> {
    if let Some(vol) = vol_install {
        return vol.validate(conn);
    }
    if let Some(path) = existing_path {
        let backend =
            virtinst_core::diskbackend::StorageBackend::for_path(conn, Some(path), remote)?;
        if !backend.exists() {
            return Err(format!(
                "Must specify storage creation parameters for non-existent path '{path}'."
            ));
        }
        return Ok(());
    }
    match source_volume_err {
        Some(e) => Err(e.to_string()),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_space() {
        assert_eq!(pretty_storage(-1.0), "Unknown GiB");
        assert_eq!(pretty_storage(12.345), "12.3 GiB");
    }

    /// `StorageBackend.exists()` and `DeviceDisk.validate` against the
    /// test driver, with `/dev` paths so no pool gets auto-created.
    #[test]
    fn plan_and_validate_disk() {
        let mut conn = virt::connect::Connect::open(Some("test:///default")).unwrap();
        let req = |path: &str, is_local: bool| DiskRequest {
            vmname: "vm".into(),
            is_default_storage: false,
            entry_path: path.into(),
            size: 1.0,
            fmt: Some("qcow2".into()),
            collide_paths: vec![],
            is_session: false,
            is_local,
        };
        // Remote: a device path we don't auto-manage is taken as is
        let plan = plan_disk(&conn, &req("/dev/sdzz", false)).unwrap();
        assert_eq!(plan.path.as_deref(), Some("/dev/sdzz"));
        assert!(plan.vol_install.is_none());
        assert_eq!(plan.props.unwrap().dev_type, "block");
        // Local: it has to exist, or be in a pool
        assert_eq!(
            plan_disk(&conn, &req("/dev/sdzz", true)).err().unwrap(),
            "Don't know how to create storage for path 'sdzz'. Use libvirt APIs to \
             manage the parent directory as a pool first."
        );
        assert!(plan_disk(&conn, &req("/dev/null", true))
            .unwrap()
            .vol_install
            .is_none());

        assert!(validate_disk(&conn, None, Some("/dev/null"), None, false).is_ok());
        assert!(validate_disk(&conn, None, Some("/dev/sdzz"), None, true).is_ok());
        assert_eq!(
            validate_disk(&conn, None, Some("/dev/sdzz"), None, false).unwrap_err(),
            "Must specify storage creation parameters for non-existent path '/dev/sdzz'."
        );
        assert_eq!(
            validate_disk(&conn, None, None, Some("no pool"), false).unwrap_err(),
            "no pool"
        );
        assert!(validate_disk(&conn, None, None, None, false).is_ok());
        conn.close().unwrap();
    }
}
