//! `vmmFSDetails` (`device/fsdetails.py`): a filesystem passthrough's
//! type, driver, source, target and readonly flag, shared by the
//! details page and (later) Add Hardware.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use virtinst_core::devices::filesystem::{
    FsEdits, DRIVER_LOOP, DRIVER_NBD, TYPE_BLOCK, TYPE_FILE, TYPE_MOUNT, TYPE_RAM, TYPE_TEMPLATE,
};
use virtinst_xml::{Document, Element};

use crate::connection::VmmConnection;
use crate::devpages::{Grid, GridRow, SimpleCombo};

/// `_EDIT_FS_*`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FsEdit {
    Type,
    Driver,
    Source,
    RamSource,
    Readonly,
    Target,
    Format,
}

/// `simple_store_set`'s rows: the value is its own label.
fn simple_rows(values: &[Option<&str>]) -> Vec<(Option<String>, String)> {
    values
        .iter()
        .map(|v| {
            let label = v.unwrap_or("Hypervisor default").to_string();
            (v.map(str::to_string), label)
        })
        .collect()
}

pub struct FsDetails {
    pub top_box: gtk::Grid,
    is_qemu: bool,
    fstype: SimpleCombo,
    fstype_row: GridRow,
    driver: SimpleCombo,
    driver_row: GridRow,
    driver_warn: gtk::Label,
    driver_warn_row: GridRow,
    format: SimpleCombo,
    format_row: GridRow,
    source: gtk::Entry,
    source_row: GridRow,
    source_browse: gtk::Button,
    ram_source: gtk::SpinButton,
    ram_source_row: GridRow,
    target: gtk::Entry,
    readonly: gtk::CheckButton,
    have_shared_mem: Cell<bool>,
    active_edits: RefCell<Vec<FsEdit>>,
    changed: RefCell<Option<Box<dyn Fn()>>>,
    /// Set while the driver rows are rebuilt, which isn't an edit.
    quiet: Cell<bool>,
}

impl FsDetails {
    /// `_init_ui`. Off the container drivers the driver rows need
    /// domcaps: `set_virtiofs_supported` fills them in.
    pub fn new(conn: Option<&VmmConnection>) -> Rc<FsDetails> {
        let container = conn.is_some_and(VmmConnection::is_container_only);
        let is_qemu = conn.is_some_and(|c| c.is_qemu() || c.is_test());
        let show_readonly = conn.is_some_and(|c| c.is_qemu() || c.is_test() || c.is_lxc());

        let types: &[Option<&str>] = if container {
            &[
                Some(TYPE_MOUNT),
                Some(TYPE_FILE),
                Some(TYPE_BLOCK),
                Some(TYPE_RAM),
            ]
        } else {
            &[Some(TYPE_MOUNT)]
        };
        let fstype = SimpleCombo::unsorted(simple_rows(types), str::to_string);
        let driver = SimpleCombo::unsorted(
            if container {
                simple_rows(&[Some(DRIVER_LOOP), Some(DRIVER_NBD), None])
            } else {
                vec![(None, "virtio-9p".into())]
            },
            str::to_string,
        );
        let format =
            SimpleCombo::unsorted(simple_rows(&[Some("raw"), Some("qcow2")]), str::to_string);

        let warn_icon = gtk::Image::from_icon_name("dialog-warning");
        let driver_warn = gtk::Label::new(None);
        driver_warn.set_wrap(true);
        driver_warn.set_max_width_chars(40);
        driver_warn.set_xalign(0.0);
        let warn_box = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        warn_box.append(&warn_icon);
        warn_box.append(&driver_warn);

        let source = gtk::Entry::new();
        source.set_hexpand(true);
        let source_browse = gtk::Button::with_mnemonic("_Browse...");
        let source_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        source_box.append(&source);
        source_box.append(&source_browse);

        let ram_source = gtk::SpinButton::with_range(0.0, 18446744073709.0, 100.0);
        ram_source.set_increments(100.0, 1000.0);
        ram_source.set_numeric(true);
        let ram_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        ram_box.append(&ram_source);
        ram_box.append(&gtk::Label::new(Some("MiB")));

        let target = gtk::Entry::new();
        target.set_hexpand(true);
        let readonly = gtk::CheckButton::with_mnemonic("E_xport filesystem as readonly mount");

        let mut g = Grid::new();
        let fstype_row = g.row("_Type:", &fstype.widget);
        let driver_row = g.row("_Driver:", &driver.widget);
        let driver_warn_row = g.row("", &warn_box);
        let format_row = g.row("_Format:", &format.widget);
        let source_row = g.row("_Source path:", &source_box);
        source_row.set_mnemonic(&source);
        let ram_source_row = g.row("_Usage:", &ram_box);
        ram_source_row.set_mnemonic(&ram_source);
        g.row("Ta_rget path:", &target);
        let readonly_row = g.row("", &readonly);
        readonly_row.set_visible(show_readonly);

        let this = Rc::new(FsDetails {
            top_box: g.into_grid(),
            is_qemu,
            fstype,
            fstype_row,
            driver,
            driver_row,
            driver_warn,
            driver_warn_row,
            format,
            format_row,
            source,
            source_row,
            source_browse,
            ram_source,
            ram_source_row,
            target,
            readonly,
            have_shared_mem: Cell::new(true),
            active_edits: RefCell::default(),
            changed: RefCell::default(),
            quiet: Cell::new(false),
        });
        this.connect_signals();
        this.reset_state();
        this
    }

    /// The domcaps half of `_init_ui`'s driver rows; the selected
    /// driver stays selected.
    pub fn set_virtiofs_supported(&self, virtiofs: bool) {
        let mut rows = vec![];
        if virtiofs {
            rows.push((Some("virtiofs".to_string()), "virtiofs".to_string()));
        }
        rows.push((None, "virtio-9p".to_string()));
        let cur = self.driver.get();
        self.quiet.set(true);
        self.driver.set_values(rows);
        self.driver.set(cur.as_deref());
        self.quiet.set(false);
    }

    /// `vm.has_shared_mem()`, for the virtiofs warning.
    pub fn set_have_shared_mem(&self, have: bool) {
        self.have_shared_mem.set(have);
        self.sync_ui();
    }

    pub fn connect_changed(&self, cb: impl Fn() + 'static) {
        self.changed.replace(Some(Box::new(cb)));
    }

    fn connect_signals(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let edit = move |e: FsEdit| {
            let weak = weak.clone();
            move || {
                if let Some(this) = weak.upgrade() {
                    this.change_cb(e);
                }
            }
        };
        let cb = edit(FsEdit::Type);
        self.fstype.widget.connect_selected_notify(move |_| cb());
        let cb = edit(FsEdit::Driver);
        self.driver.widget.connect_selected_notify(move |_| cb());
        let cb = edit(FsEdit::Readonly);
        self.readonly.connect_toggled(move |_| cb());
        let cb = edit(FsEdit::Format);
        self.format.widget.connect_selected_notify(move |_| cb());
        let cb = edit(FsEdit::Source);
        self.source.connect_changed(move |_| cb());
        let cb = edit(FsEdit::RamSource);
        self.ram_source.connect_value_changed(move |_| cb());
        let cb = edit(FsEdit::Target);
        self.target.connect_changed(move |_| cb());

        // `_browse_fs_source_cb`. The storage browser isn't ported
        // yet, so this offers the local folder chooser.
        let entry = self.source.clone();
        self.source_browse.connect_clicked(move |b| {
            let dialog = gtk::FileDialog::builder()
                .title("Locate directory volume")
                .modal(true)
                .build();
            let parent = b.root().and_downcast::<gtk::Window>();
            let entry = entry.clone();
            dialog.select_folder(parent.as_ref(), gtk::gio::Cancellable::NONE, move |res| {
                if let Some(path) = res.ok().and_then(|f| f.path()) {
                    entry.set_text(&path.to_string_lossy());
                }
            });
        });
    }

    /// `_sync_ui`.
    fn sync_ui(&self) {
        let fstype = self.fstype.get();
        let fsdriver = self.driver.get();

        let show_ram_source = fstype.as_deref() == Some(TYPE_RAM);
        self.ram_source_row.set_visible(show_ram_source);
        self.source_row.set_visible(!show_ram_source);

        self.format_row
            .set_visible(fsdriver.as_deref() == Some(DRIVER_NBD));

        let show_driver_combo = self.is_qemu || fstype.as_deref() == Some(TYPE_FILE);
        let source_text = if fstype.as_deref() == Some(TYPE_TEMPLATE) {
            "Te_mplate:"
        } else {
            "_Source path:"
        };
        self.source_row.label.set_text_with_mnemonic(source_text);
        self.fstype_row.set_visible(!self.is_qemu);
        self.driver_row.set_visible(show_driver_combo);

        let show_shared_mem_warn =
            fsdriver.as_deref() == Some("virtiofs") && !self.have_shared_mem.get();
        self.driver_warn_row.set_visible(show_shared_mem_warn);
        if show_shared_mem_warn {
            self.driver_warn.set_markup(&format!(
                "<small>{}</small>",
                gtk::glib::markup_escape_text(
                    "You may need to 'Enable shared memory' on the 'Memory' screen."
                )
            ));
        }
    }

    /// `reset_state`.
    fn reset_state(&self) {
        self.fstype.widget.set_selected(0);
        self.driver.widget.set_selected(0);
        self.format.widget.set_selected(0);
        self.source.set_text("");
        self.target.set_text("");
        self.readonly.set_active(false);
        self.sync_ui();
        self.active_edits.borrow_mut().clear();
    }

    /// `set_dev`.
    pub fn set_dev(&self, doc: &Document, fs: Element) {
        let attr = |path: &[&str], name: &str| {
            virtinst_xml::resolve_path(doc, fs, path)
                .and_then(|e| e.attribute(doc, name))
                .map(str::to_string)
        };
        self.reset_state();

        let fstype = attr(&[], "type");
        self.fstype.set(fstype.as_deref());
        self.driver.set(attr(&["driver"], "type").as_deref());
        self.format.set(attr(&["driver"], "format").as_deref());

        let source = virtinst_core::devices::filesystem::source(doc, fs);
        if fstype.as_deref() != Some(TYPE_RAM) {
            self.source.set_text(source.as_deref().unwrap_or_default());
        } else {
            let usage: u64 = source.and_then(|s| s.parse().ok()).unwrap_or(0);
            self.ram_source.set_value((usage / 1024) as f64);
        }
        self.target
            .set_text(attr(&["target"], "dir").as_deref().unwrap_or_default());
        self.readonly.set_active(fs.find(doc, "readonly").is_some());
        self.sync_ui();

        self.active_edits.borrow_mut().clear();
    }

    /// `_set_values`: what `update_device` changes.
    pub fn get_values(&self) -> FsEdits {
        let edits = self.active_edits.borrow();
        let edited = |e| edits.contains(&e);
        let fstype = self.fstype.get();
        self.ram_source.update();
        let usage = self.ram_source.value() as u64;
        let format = if self.format_row.is_visible() {
            self.format.get()
        } else {
            None
        };
        let driver = if self.driver_row.is_visible() {
            self.driver.get()
        } else {
            None
        };
        let source = if fstype.as_deref() == Some(TYPE_RAM) {
            usage.to_string()
        } else {
            self.source.text().to_string()
        };
        FsEdits {
            fstype: edited(FsEdit::Type).then_some(fstype),
            source: (edited(FsEdit::RamSource) || edited(FsEdit::Source)).then_some(source),
            target: edited(FsEdit::Target).then(|| self.target.text().to_string()),
            readonly: edited(FsEdit::Readonly).then(|| self.readonly.is_active()),
            driver: edited(FsEdit::Driver).then_some(driver),
            format: edited(FsEdit::Format).then_some(format),
        }
    }

    /// `_change_cb`.
    fn change_cb(&self, edit: FsEdit) {
        if self.quiet.get() {
            return;
        }
        self.sync_ui();
        {
            let mut edits = self.active_edits.borrow_mut();
            if !edits.contains(&edit) {
                edits.push(edit);
            }
        }
        if let Some(cb) = &*self.changed.borrow() {
            cb();
        }
    }
}
