//! `delete.py`: the Delete Virtual Machine dialog (`vmmDeleteDialog`)
//! and the Remove Disk Device dialog (`vmmDeleteStorage`), both of
//! which can also delete the storage files behind the disks.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use virt::connect::Connect;
use virtinst_xml::{Document, Element};

use crate::domain::VmmDomain;

/// `_DiskData`: what decides whether a path is deleted by default.
#[derive(Clone, Debug, Default)]
pub struct DiskData {
    label: String,
    path: Option<String>,
    ro: bool,
    shared: bool,
    is_media: bool,
}

impl DiskData {
    /// `_DiskData.from_disk`.
    pub fn from_disk(doc: &Document, disk: Element) -> DiskData {
        let device = disk.attribute(doc, "device");
        DiskData {
            label: virtinst_xml::resolve_path(doc, disk, &["target"])
                .and_then(|t| t.attribute(doc, "dev"))
                .unwrap_or_default()
                .to_string(),
            path: virtinst_core::devices::disk::source_path(doc, disk),
            ro: disk.find(doc, "readonly").is_some(),
            shared: disk.find(doc, "shareable").is_some(),
            is_media: matches!(device, Some("cdrom" | "floppy")),
        }
    }
}

/// `_build_diskdata_for_vm`: every disk, plus the direct kernel boot
/// files.
fn build_diskdata_for_vm(vm: &VmmDomain) -> Vec<DiskData> {
    let Some(x) = vm.xmlobj() else {
        return vec![];
    };
    let doc = &x.doc;
    let root = x.root();
    let mut ret: Vec<DiskData> = virtinst_xml::resolve_path(doc, root, &["devices"])
        .map(|d| d.find_all(doc, "disk"))
        .unwrap_or_default()
        .into_iter()
        .map(|d| DiskData::from_disk(doc, d))
        .collect();
    for label in ["kernel", "initrd", "dtb"] {
        ret.push(DiskData {
            label: label.into(),
            path: virtinst_xml::resolve_path(doc, root, &["os", label])
                .map(|e| e.text_content(doc))
                .filter(|p| !p.is_empty()),
            ro: true,
            shared: false,
            is_media: true,
        });
    }
    ret
}

/// One row of the storage list.
struct StorageRow {
    confirm: Cell<bool>,
    cant_delete: bool,
    path: String,
    target: String,
    info: Option<String>,
}

/// `access(2)`, which std doesn't expose.
fn access_ok(path: &std::path::Path, mode: i32) -> bool {
    extern "C" {
        fn access(path: *const std::ffi::c_char, mode: i32) -> i32;
    }
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    unsafe { access(c.as_ptr(), mode) == 0 }
}
const W_OK: i32 = 2;

/// `_can_delete`: whether the path can be deleted at all, and why not.
fn can_delete(
    vol: Option<&virt::storage_vol::StorageVol>,
    is_remote: bool,
    path: &str,
) -> Option<&'static str> {
    if let Some(vol) = vol {
        // Managed storage
        let pool_type = virt::storage_pool::StoragePool::lookup_by_volume(vol)
            .ok()
            .and_then(|p| p.get_xml_desc(0).ok())
            .and_then(|xml| {
                let doc = virtinst_xml::parse_libvirt_xml(&xml).ok()?;
                let root = doc.root_element()?;
                root.attribute(&doc, "type").map(str::to_string)
            });
        return match pool_type.as_deref() {
            Some("iscsi") => Some("Cannot delete iSCSI share."),
            Some("scsi") => Some("Cannot delete SCSI device."),
            _ => None,
        };
    }
    let p = std::path::Path::new(path);
    if is_remote {
        Some("Cannot delete unmanaged remote storage.")
    } else if !p.exists() {
        Some("Path does not exist.")
    } else if !access_ok(p.parent().unwrap_or(p), W_OK) {
        Some("No write access to parent directory.")
    } else if std::fs::metadata(p).is_ok_and(|m| {
        use std::os::unix::fs::FileTypeExt;
        m.file_type().is_block_device()
    }) {
        Some("Cannot delete unmanaged block device.")
    } else {
        None
    }
}

/// `_do_we_default`: whether to delete by default, and why not.
fn do_we_default(
    conn: &Connect,
    vm_name: &str,
    has_vol: bool,
    diskdata: &DiskData,
    path: &str,
) -> Option<String> {
    let mut info = vec![];
    if diskdata.ro {
        info.push("Storage is read-only.".to_string());
    } else if !has_vol && !access_ok(std::path::Path::new(path), W_OK) {
        info.push("No write access to path.".to_string());
    }
    if diskdata.shared {
        info.push("Storage is marked as shareable.".to_string());
    }
    if info.is_empty() && diskdata.is_media {
        info.push("Storage is a media device.".to_string());
    }
    match crate::domain::path_in_use_by(conn, path) {
        Ok(mut names) if names.len() > 1 => {
            names.retain(|n| n != vm_name);
            info.push(format!(
                "Storage is in use by the following virtual machines\n- {}",
                names.join("\n- ")
            ));
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!("Failed checking disk conflict: {e}");
            info.push("Failed to check disk usage conflict.".to_string());
        }
    }
    (!info.is_empty()).then(|| info.join("\n"))
}

/// `_populate_storage_list`'s rows, blocking.
fn build_storage_rows(
    conn: &Connect,
    is_remote: bool,
    vm_name: &str,
    diskdatas: &[DiskData],
) -> Vec<(bool, bool, String, String, Option<String>)> {
    diskdatas
        .iter()
        .filter_map(|d| {
            let path = d.path.clone()?;
            let vol = virt::storage_vol::StorageVol::lookup_by_path(conn, &path).ok();
            let delinfo = can_delete(vol.as_ref(), is_remote, &path);
            let (default, definfo) = match delinfo {
                None => {
                    let definfo = do_we_default(conn, vm_name, vol.is_some(), d, &path);
                    (definfo.is_none(), definfo)
                }
                Some(_) => (false, None),
            };
            let info = match delinfo {
                Some(msg) => Some(msg.to_string()),
                None => definfo,
            };
            Some((default, delinfo.is_some(), path, d.label.clone(), info))
        })
        .collect()
}

/// `remove_devobj_internal`: drop the device from the config, then try
/// to hot-unplug it. `false` when the device wasn't removed from the
/// running VM either.
pub async fn remove_devobj_internal(
    parent: Option<&gtk::Window>,
    vm: &VmmDomain,
    id: &virtinst_core::devices::DeviceId,
    deleting_storage: bool,
) -> bool {
    let devxml = vm
        .xmlobj()
        .and_then(|x| {
            let doc = virtinst_xml::parse_libvirt_xml(&x.xml).ok()?;
            let root = doc.root_element()?;
            let dev = virtinst_core::devices::find_device(&doc, root, id)?;
            virtinst_xml::element_to_string(doc, dev).ok()
        })
        .unwrap_or_default();
    glib::g_debug!("virt-manager", "Removing device: {devxml}");

    // Define the change
    if let Err(e) = vm.remove_device(id).await {
        crate::error::show_err(parent, &format!("Error Removing Device: {e}"), None, "").await;
        return false;
    }

    // Try to hot remove
    let Err(e) = vm.detach_device(devxml).await else {
        return true;
    };
    glib::g_debug!("virt-manager", "Device could not be hotUNplugged: {e}");
    let mut msg = "This change will take effect after the next guest shutdown.".to_string();
    if deleting_storage {
        msg += " Storage will not be deleted.";
    }
    crate::error::show_info(
        parent,
        "Device could not be removed from the running machine",
        Some(&format!("{msg}\n\n{e}")),
        None,
        crate::error::Buttons::Ok,
    )
    .await;
    false
}

/// Which dialog this is.
pub enum DeleteKind {
    /// `vmmDeleteDialog`.
    Vm,
    /// `vmmDeleteStorage` for the disk with this id and data.
    Disk {
        id: virtinst_core::devices::DeviceId,
        disk: DiskData,
    },
}

struct Inner {
    win: gtk::Window,
    vm: VmmDomain,
    kind: DeleteKind,
    remove_storage: gtk::CheckButton,
    storage_scroll: gtk::ScrolledWindow,
    store: gio::ListStore,
    removed_handler: RefCell<Option<glib::SignalHandlerId>>,
}

#[derive(Clone)]
pub struct DeleteDialog(Rc<Inner>);

impl DeleteDialog {
    /// `show`.
    pub fn show(parent: Option<&gtk::Window>, vm: &VmmDomain, kind: DeleteKind) {
        let title = match &kind {
            DeleteKind::Vm => "Delete Virtual Machine",
            DeleteKind::Disk { .. } => "Remove Disk Device",
        };
        let win = gtk::Window::builder()
            .title(title)
            .width_request(450)
            .height_request(350)
            .build();
        win.set_transient_for(parent);

        let icon = gtk::Image::from_icon_name("vm_delete_wizard");
        icon.set_pixel_size(48);
        let header_label = gtk::Label::new(None);
        header_label.set_xalign(0.0);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        header.set_margin_start(6);
        header.set_margin_end(6);
        header.set_margin_top(6);
        header.set_margin_bottom(6);
        header.append(&icon);
        header.append(&header_label);

        let warn_icon = gtk::Image::from_icon_name("dialog-warning");
        let warn_label = gtk::Label::new(None);
        warn_label.set_markup(
            "<small>This VM is currently running and will be forced off before being deleted</small>",
        );
        warn_label.set_xalign(0.0);
        let warn_box = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        warn_box.append(&warn_icon);
        warn_box.append(&warn_label);

        let remove_storage = gtk::CheckButton::with_mnemonic("Delete _associated storage files");
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let list = gtk::ColumnView::new(Some(gtk::NoSelection::new(Some(store.clone()))));
        build_columns(&list);
        let storage_scroll = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .has_frame(true)
            .vexpand(true)
            .build();

        let grid = gtk::Box::new(gtk::Orientation::Vertical, 6);
        grid.append(&warn_box);
        grid.append(&remove_storage);
        grid.append(&storage_scroll);
        grid.set_vexpand(true);

        let cancel = gtk::Button::with_mnemonic("_Cancel");
        let ok = gtk::Button::with_mnemonic("_Delete");
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.set_halign(gtk::Align::End);
        buttons.append(&cancel);
        buttons.append(&ok);

        let body = gtk::Box::new(gtk::Orientation::Vertical, 18);
        body.set_margin_start(12);
        body.set_margin_end(12);
        body.set_margin_top(12);
        body.set_margin_bottom(12);
        body.append(&grid);
        body.append(&buttons);
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 6);
        vbox.append(&header);
        vbox.append(&body);
        win.set_child(Some(&vbox));

        // `_reset_state`
        let text = match &kind {
            DeleteKind::Vm => format!("Delete '{}'", vm.get_name()),
            DeleteKind::Disk { disk, .. } => format!("Remove disk device '{}'", disk.label),
        };
        header_label.set_markup(&format!(
            "<span size='large'>{}</span>",
            glib::markup_escape_text(&text)
        ));
        let vm_active = matches!(kind, DeleteKind::Vm) && vm.is_active();
        warn_box.set_visible(vm_active);
        // Enable storage removal by default
        remove_storage.set_active(matches!(kind, DeleteKind::Vm));
        storage_scroll.set_visible(remove_storage.is_active());

        let diskdatas = match &kind {
            DeleteKind::Vm => None,
            DeleteKind::Disk { disk, .. } => Some(vec![disk.clone()]),
        };
        let this = DeleteDialog(Rc::new(Inner {
            win,
            vm: vm.clone(),
            kind,
            remove_storage,
            storage_scroll,
            store,
            removed_handler: RefCell::default(),
        }));
        this.populate_storage_list(diskdatas);

        let i = &this.0;
        let scroll = i.storage_scroll.clone();
        i.remove_storage
            .connect_toggled(move |b| scroll.set_visible(b.is_active()));
        let weak = Rc::downgrade(&this.0);
        cancel.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                DeleteDialog(inner).close();
            }
        });
        let weak = Rc::downgrade(&this.0);
        ok.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                let this = DeleteDialog(inner);
                glib::spawn_future_local(async move { this.finish().await });
            }
        });
        let keep = this.clone();
        i.win.connect_close_request(move |_| {
            keep.disconnect_vm();
            glib::Propagation::Proceed
        });
        let esc = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(&this.0);
        esc.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                if let Some(inner) = weak.upgrade() {
                    DeleteDialog(inner).close();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        i.win.add_controller(esc);
        if let Some(conn) = vm.conn() {
            let weak = Rc::downgrade(&this.0);
            let id = conn.connect_local("vm-removed", false, move |args| {
                let removed = args[1].get::<VmmDomain>().ok();
                if let Some(inner) = weak.upgrade() {
                    if removed.as_ref() == Some(&inner.vm) {
                        DeleteDialog(inner).close();
                    }
                }
                None
            });
            i.removed_handler.replace(Some(id));
        }
        cancel.grab_focus();
        i.win.present();
    }

    fn disconnect_vm(&self) {
        if let (Some(id), Some(conn)) = (self.0.removed_handler.take(), self.0.vm.conn()) {
            conn.disconnect(id);
        }
    }

    fn close(&self) {
        self.disconnect_vm();
        self.0.win.destroy();
    }

    /// `_populate_storage_list`, with the checks on a worker thread.
    /// `None` lists the VM's own storage, from freshly read XML.
    fn populate_storage_list(&self, diskdatas: Option<Vec<DiskData>>) {
        let Some(conn) = self.0.vm.conn() else {
            return;
        };
        let Ok(handle) = self.0.vm.handle() else {
            return;
        };
        let is_remote = conn.is_remote();
        let vm_name = self.0.vm.get_name();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let diskdatas = match diskdatas {
                Some(d) => d,
                None => {
                    let _ = this.0.vm.refresh_xml().await;
                    build_diskdata_for_vm(&this.0.vm)
                }
            };
            let rows = gio::spawn_blocking(move || {
                build_storage_rows(handle.connect(), is_remote, &vm_name, &diskdatas)
            })
            .await
            .unwrap_or_default();
            this.0.store.remove_all();
            for (confirm, cant_delete, path, target, info) in rows {
                this.0
                    .store
                    .append(&glib::BoxedAnyObject::new(Rc::new(StorageRow {
                        confirm: Cell::new(confirm),
                        cant_delete,
                        path,
                        target,
                        info,
                    })));
            }
        });
    }

    /// `_get_paths_to_delete`.
    fn paths_to_delete(&self) -> Vec<String> {
        if !self.0.remove_storage.is_active() {
            return vec![];
        }
        (0..self.0.store.n_items())
            .filter_map(|n| self.0.store.item(n))
            .filter_map(|o| o.downcast::<glib::BoxedAnyObject>().ok())
            .filter_map(|o| {
                let row = o.borrow::<Rc<StorageRow>>();
                (!row.cant_delete && row.confirm.get()).then(|| row.path.clone())
            })
            .collect()
    }

    /// `_finish`.
    async fn finish(&self) {
        let i = &self.0;
        let parent = Some(i.win.clone());
        let paths = self.paths_to_delete();
        if !paths.is_empty() {
            let config = crate::engine::get().config();
            let ok = crate::error::chkbox_helper(
                parent.as_ref(),
                || config.get_confirm_delstorage(),
                |v| config.set_confirm_delstorage(v),
                "Are you sure you want to delete the storage?",
                Some(&format!(
                    "The following paths will be deleted:\n\n{}",
                    paths.join("\n")
                )),
            )
            .await;
            if !ok {
                return;
            }
        }
        i.win.set_cursor_from_name(Some("progress"));

        // `_remove_device`
        if let DeleteKind::Disk { id, .. } = &i.kind {
            if !remove_devobj_internal(parent.as_ref(), &i.vm, id, !paths.is_empty()).await {
                // Don't delete storage if device removal failed
                self.close();
                return;
            }
        }

        self.run_delete(paths).await;
    }

    /// The `vmmAsyncJob` half of `_finish`: `_async_delete`.
    async fn run_delete(&self, paths: Vec<String>) {
        let i = &self.0;
        let Ok(handle) = i.vm.handle() else {
            self.close();
            return;
        };
        let name = i.vm.get_name();
        let label = match &i.kind {
            DeleteKind::Vm => None,
            DeleteKind::Disk { disk, .. } => Some(disk.label.clone()),
        };
        let title = match (&label, paths.is_empty()) {
            (None, false) => format!(
                "Deleting virtual machine '{name}' and selected storage (this may take a while)"
            ),
            (None, true) => format!("Deleting virtual machine '{name}'"),
            (Some(l), false) => {
                format!("Removing disk device '{l}' and selected storage (this may take a while)")
            }
            (Some(l), true) => format!("Removing disk device '{l}'"),
        };
        let is_vm = label.is_none();
        let uuid = i.vm.get_uuid();
        // `has_nvram`
        let has_nvram = i.vm.xmlobj().is_some_and(|x| {
            let os = &x.guest.os;
            (os.loader.is_some() && os.loader_type.as_deref() == Some("pflash"))
                || os.firmware.as_deref() == Some("efi")
                || os.nvram.is_some()
        });
        let err = crate::asyncjob::run(
            Some(&i.win),
            &title,
            &title,
            true,
            move |job| {
                let conn = handle.connect();
                let res = (|| -> Result<Vec<(String, String)>, String> {
                    let dom = virt::domain::Domain::lookup_by_uuid_string(conn, &uuid)
                        .map_err(|e| e.message().to_string());
                    // `_destroy_vm`
                    if is_vm {
                        let dom = dom.as_ref().map_err(Clone::clone)?;
                        if dom.is_active().unwrap_or(false) {
                            eprintln!("Forcing VM '{name}' power off.");
                            dom.destroy().map_err(|e| e.message().to_string())?;
                        }
                    }
                    // `_async_delete_paths`
                    let mut meter = job.meter();
                    let mut storage_errors = vec![];
                    for path in &paths {
                        eprintln!("Deleting path: {path}");
                        meter.start(&format!("Deleting path '{path}'"), None);
                        if let Err(e) = delete_path(conn, path) {
                            storage_errors.push((e, String::new()));
                        }
                        meter.end();
                    }
                    // `_delete_vm`
                    if is_vm {
                        let dom = dom.as_ref().map_err(Clone::clone)?;
                        if dom.is_persistent().unwrap_or(false) {
                            eprintln!("Removing VM '{name}'");
                            crate::domain::delete_domain(dom, has_nvram)?;
                        }
                    }
                    Ok(storage_errors)
                })();

                let (errdata, storage_errors) = match res {
                    Ok(errs) => (None, errs),
                    Err(e) => (
                        Some(format!("Error deleting virtual machine '{name}': {e}")),
                        vec![],
                    ),
                };
                if errdata.is_none() && storage_errors.is_empty() {
                    return Ok(());
                }
                let storage_errstr: String = storage_errors
                    .iter()
                    .map(|(e, d)| format!("{e}\n{d}\n"))
                    .collect();
                Err(match errdata {
                    Some(error) => (
                        error.clone(),
                        format!(
                            "{error}\n\nAdditionally, there were errors removing certain storage devices: \n{storage_errstr}"
                        ),
                    ),
                    None => (
                        "Errors encountered while removing certain storage devices.".into(),
                        storage_errstr,
                    ),
                })
            },
        )
        .await;
        if let Some(conn) = i.vm.conn() {
            conn.tick();
        }
        // `_delete_finished_cb`
        i.win.set_cursor(None);
        if let Some((error, details)) = err {
            crate::error::show_err(Some(&i.win), &error, Some(&details), "").await;
        }
        self.close();
    }
}

/// `_async_delete_path`: a managed volume through libvirt, anything
/// else straight off the disk.
fn delete_path(conn: &Connect, path: &str) -> Result<(), String> {
    match virt::storage_vol::StorageVol::lookup_by_path(conn, path) {
        Ok(vol) => vol.delete(0).map_err(|e| e.message().to_string()),
        Err(_) => {
            eprintln!("Path '{path}' is not managed. Deleting locally");
            std::fs::remove_file(path).map_err(|e| e.to_string())
        }
    }
}

/// `_prepare_storage_list`: checkbox, Storage Path, Target, and a
/// warning icon; each row's tooltip says why it isn't deleted by
/// default.
fn build_columns(list: &gtk::ColumnView) {
    fn column(
        list: &gtk::ColumnView,
        title: Option<&str>,
        expand: bool,
        setup: impl Fn() -> gtk::Widget + 'static,
        bind: impl Fn(&gtk::Widget, &Rc<StorageRow>) + 'static,
    ) {
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
            item.set_child(Some(&setup()));
        });
        factory.connect_bind(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
            let (Some(child), Some(obj)) = (
                item.child(),
                item.item().and_downcast::<glib::BoxedAnyObject>(),
            ) else {
                return;
            };
            let row = obj.borrow::<Rc<StorageRow>>().clone();
            child.set_tooltip_text(row.info.as_deref());
            bind(&child, &row);
        });
        let col = gtk::ColumnViewColumn::new(title, Some(factory));
        col.set_expand(expand);
        list.append_column(&col);
    }

    column(
        list,
        None,
        false,
        || gtk::CheckButton::new().upcast(),
        |w, row| {
            let chk = w.downcast_ref::<gtk::CheckButton>().expect("a check");
            chk.set_inconsistent(row.cant_delete);
            chk.set_sensitive(!row.cant_delete);
            chk.set_active(row.confirm.get());
            let row = row.clone();
            // rebinding replaces the handler from the last row
            if let Some(old) = unsafe { chk.steal_data::<glib::SignalHandlerId>("toggled-id") } {
                chk.disconnect(old);
            }
            let id = chk.connect_toggled(move |c| row.confirm.set(c.is_active()));
            unsafe { chk.set_data("toggled-id", id) };
        },
    );
    column(
        list,
        Some("Storage Path"),
        true,
        || {
            let l = gtk::Label::new(None);
            l.set_xalign(0.0);
            l.set_width_chars(50);
            l.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            l.upcast()
        },
        |w, row| {
            w.downcast_ref::<gtk::Label>()
                .expect("a label")
                .set_text(&row.path)
        },
    );
    column(
        list,
        Some("Target"),
        false,
        || {
            let l = gtk::Label::new(None);
            l.set_xalign(0.0);
            l.upcast()
        },
        |w, row| {
            w.downcast_ref::<gtk::Label>()
                .expect("a label")
                .set_text(&row.target)
        },
    );
    column(
        list,
        None,
        false,
        || gtk::Image::from_icon_name("dialog-warning").upcast(),
        |w, row| w.set_visible(row.info.is_some()),
    );
}
