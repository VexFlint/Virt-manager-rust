//! `storagebrowse.py` (`vmmStorageBrowser`): picks a storage volume,
//! or a local file with "Browse Local", for a disk, media or
//! filesystem path.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::connection::VmmConnection;
use crate::engine;
use crate::hoststorage::HostStorage;

/// `REASON_*`, or `None` for a plain volume chooser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowseReason {
    Image,
    IsoMedia,
    FloppyMedia,
    Fs,
}

/// How the local file chooser picks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DialogType {
    Open,
    /// `Gtk.FileChooserAction.SAVE`: a new file name can be typed.
    Save,
    SelectFolder,
}

/// `_BrowseReasonMetadata`.
struct BrowseReasonMetadata {
    storage_title: &'static str,
    local_title: Option<&'static str>,
    gsettings_key: Option<&'static str>,
    dialog_type: DialogType,
    choose_label: Option<&'static str>,
}

impl BrowseReasonMetadata {
    fn new(reason: Option<BrowseReason>) -> BrowseReasonMetadata {
        let mut data = BrowseReasonMetadata {
            storage_title: "",
            local_title: None,
            gsettings_key: None,
            dialog_type: DialogType::Open,
            choose_label: None,
        };
        match reason {
            Some(BrowseReason::Image) => {
                data.local_title = Some("Locate existing storage");
                data.storage_title = "Locate or create storage volume";
                data.dialog_type = DialogType::Save;
                data.choose_label = Some("_Open");
                data.gsettings_key = Some("image");
            }
            Some(BrowseReason::IsoMedia) => {
                data.local_title = Some("Locate ISO media");
                data.storage_title = "Locate ISO media volume";
                data.gsettings_key = Some("media");
            }
            Some(BrowseReason::FloppyMedia) => {
                data.local_title = Some("Locate floppy media");
                data.storage_title = "Locate floppy media volume";
                data.gsettings_key = Some("media");
            }
            Some(BrowseReason::Fs) => {
                data.local_title = Some("Locate directory volume");
                data.storage_title = "Locate directory volume";
                data.dialog_type = DialogType::SelectFolder;
            }
            // `vol-add` (`enable_create`) waits on the volume wizard
            None => {
                data.storage_title = "Choose Storage Volume";
            }
        }
        data
    }
}

type FinishCallback = Box<dyn Fn(&str)>;

pub struct Inner {
    conn: VmmConnection,
    window: gtk::Window,
    storagelist: HostStorage,
    first_run: Cell<bool>,
    finish_cb: RefCell<Option<FinishCallback>>,
    browse_reason: Rc<Cell<Option<BrowseReason>>>,
}

#[derive(Clone)]
pub struct StorageBrowser(Rc<Inner>);

impl StorageBrowser {
    pub fn new(conn: &VmmConnection) -> StorageBrowser {
        let browse_reason: Rc<Cell<Option<BrowseReason>>> = Rc::default();
        // `_vol_sensitive_cb`
        let reason = browse_reason.clone();
        let storagelist = HostStorage::new(
            conn,
            Some(Box::new(move |fmt| {
                reason.get() != Some(BrowseReason::Fs) || fmt == "dir"
            })),
        );

        let window = gtk::Window::builder()
            .title("title")
            .default_width(750)
            .default_height(500)
            .destroy_with_parent(true)
            .hide_on_close(true)
            .build();
        window.update_property(&[gtk::accessible::Property::Label("vmm-storage-browser")]);
        let align = gtk::Box::new(gtk::Orientation::Vertical, 0);
        align.set_margin_start(6);
        align.set_margin_end(6);
        align.set_margin_top(6);
        align.set_margin_bottom(6);
        align.append(&storagelist.top_box);
        window.set_child(Some(&align));

        let this = StorageBrowser(Rc::new(Inner {
            conn: conn.clone(),
            window,
            storagelist,
            first_run: Cell::new(false),
            finish_cb: RefCell::default(),
            browse_reason,
        }));
        this.init_ui();
        this
    }

    /// `_init_ui`.
    fn init_ui(&self) {
        let i = &self.0;
        let weak = Rc::downgrade(&self.0);
        let with = move |f: fn(&StorageBrowser)| {
            let weak = weak.clone();
            move || {
                if let Some(inner) = weak.upgrade() {
                    f(&StorageBrowser(inner));
                }
            }
        };
        let cb = with(|t| {
            let t = t.clone();
            glib::spawn_future_local(async move { t.browse_local().await });
        });
        i.storagelist.connect_browse_clicked(cb);
        let weak = Rc::downgrade(&self.0);
        i.storagelist.connect_volume_chosen(move |vol| {
            if let Some(inner) = weak.upgrade() {
                eprintln!("Chosen volume: {}", vol.xml.target_path);
                StorageBrowser(inner).finish(&vol.xml.target_path);
            }
        });
        i.storagelist
            .connect_cancel_clicked(with(StorageBrowser::close));

        let esc = gtk::EventControllerKey::new();
        let cb = with(StorageBrowser::close);
        esc.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                cb();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        i.window.add_controller(esc);
        let cb = with(|t| t.0.storagelist.close());
        i.window.connect_close_request(move |_| {
            cb();
            glib::Propagation::Proceed
        });

        let sl = &i.storagelist;
        let is_remote = i.conn.is_remote();
        sl.browse_local.set_sensitive(!is_remote);
        sl.browse_local.set_tooltip_text(
            is_remote.then_some("Cannot use local storage on remote connection."),
        );

        for w in [
            sl.pool_autostart_label.upcast_ref::<gtk::Widget>(),
            sl.pool_autostart.upcast_ref(),
            sl.pool_name_label.upcast_ref(),
            sl.pool_name_entry.upcast_ref(),
            sl.pool_state_label.upcast_ref(),
            sl.pool_state_box.upcast_ref(),
        ] {
            w.set_visible(false);
        }
        sl.browse_local.set_visible(true);
        sl.browse_cancel.set_visible(true);
        sl.choose_volume.set_visible(true);
        sl.choose_volume.set_sensitive(false);
        sl.pool_apply.set_visible(false);

        self.set_browse_reason(i.browse_reason.get());
    }

    /// `show`: the first time, with the default pool selected.
    pub fn show(&self, parent: &gtk::Window) {
        eprintln!("Showing storage browser");
        let i = &self.0;
        if !i.first_run.get() {
            i.first_run.set(true);
            let this = self.clone();
            glib::spawn_future_local(async move {
                let Some(handle) = this.0.conn.handle() else {
                    return;
                };
                let session = this.0.conn.is_qemu_unprivileged();
                let name = gio::spawn_blocking(move || {
                    virtinst_core::storage::lookup_default_pool(handle.connect(), session)
                        .map(|p| p.xml.name)
                })
                .await
                .ok()
                .flatten();
                if let Some(name) = name {
                    this.0.storagelist.select_pool(&name);
                }
            });
        }
        i.window.set_transient_for(Some(parent));
        i.window.present();
        i.storagelist.refresh_page();
    }

    pub fn close(&self) {
        if self.0.window.is_visible() {
            eprintln!("Closing storage browser");
            self.0.window.set_visible(false);
        }
        self.0.storagelist.close();
    }

    pub fn set_finish_cb(&self, cb: impl Fn(&str) + 'static) {
        *self.0.finish_cb.borrow_mut() = Some(Box::new(cb));
    }

    pub fn set_browse_reason(&self, reason: Option<BrowseReason>) {
        self.0.browse_reason.set(reason);
        let data = BrowseReasonMetadata::new(reason);
        self.0.window.set_title(Some(data.storage_title));
    }

    /// `_browse_local`.
    async fn browse_local(&self) {
        let data = BrowseReasonMetadata::new(self.0.browse_reason.get());
        let config = engine::get().config();
        let start_folder = data
            .gsettings_key
            .map(|k| config.get_default_directory(k))
            .filter(|f| !f.is_empty());

        let dialog = gtk::FileDialog::builder()
            .title(data.local_title.unwrap_or_default())
            .modal(true)
            .build();
        if let Some(label) = data.choose_label {
            dialog.set_accept_label(Some(label));
        }
        if let Some(folder) = &start_folder {
            dialog.set_initial_folder(Some(&gio::File::for_path(folder)));
        }
        let parent = Some(&self.0.window);
        let res = match data.dialog_type {
            DialogType::Open => dialog.open_future(parent).await,
            DialogType::Save => dialog.save_future(parent).await,
            DialogType::SelectFolder => dialog.select_folder_future(parent).await,
        };
        let Some(path) = res.ok().and_then(|f| f.path()) else {
            return;
        };
        let filename = path.to_string_lossy().to_string();
        eprintln!("Browse local chose path={filename}");

        if let Some(key) = data.gsettings_key {
            if let Some(dir) = path.parent().and_then(|d| d.to_str()) {
                config.set_default_directory(key, dir);
            }
        }
        self.finish(&filename);
    }

    fn finish(&self, path: &str) {
        if let Some(cb) = self.0.finish_cb.borrow().as_ref() {
            cb(path);
        }
        self.close();
    }
}

/// `_browse_file`'s shared shape: reuse (or make) the caller's
/// browser, point it at `cb`, and show it over `parent`.
pub fn browse(
    cache: &RefCell<Option<StorageBrowser>>,
    conn: &VmmConnection,
    parent: &gtk::Window,
    reason: BrowseReason,
    cb: impl Fn(&str) + 'static,
) {
    let browser = cache
        .borrow_mut()
        .get_or_insert_with(|| StorageBrowser::new(conn))
        .clone();
    browser.set_finish_cb(cb);
    browser.set_browse_reason(Some(reason));
    browser.show(parent);
}
