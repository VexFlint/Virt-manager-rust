//! `device/mediacombo.py`: the CD/floppy media picker. An entry for a
//! path, with a drop-down of recently used ISOs and the host's optical
//! or floppy drives.
//!
//! GTK4 has no combo box with an entry that isn't deprecated, so this
//! is an entry plus a menu button, the way the CPU model picker is.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use crate::connection::VmmConnection;

/// `[path, label, has_media, device key]`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MediaRow {
    path: String,
    label: String,
    has_media: bool,
    key: String,
}

impl MediaRow {
    fn iso(path: &str) -> MediaRow {
        MediaRow {
            path: path.into(),
            label: path.into(),
            has_media: true,
            key: path.into(),
        }
    }
}

/// A host drive from the node device list.
#[derive(Debug, Clone)]
struct Drive {
    drive_type: String,
    row: MediaRow,
}

/// `_make_nodedev_rows`' filter over one nodedev's XML: storage
/// devices whose drive type is `cdrom` or `floppy`.
fn parse_drive(xml: &str) -> Option<Drive> {
    let doc = virtinst_xml::parse_libvirt_xml(xml).ok()?;
    let root = doc.root_element()?;
    let name = root.find(&doc, "name")?.text_content(&doc);
    let cap = root
        .find_all(&doc, "capability")
        .into_iter()
        .find(|c| c.attribute(&doc, "type") == Some("storage"))?;
    let text =
        |path: &[&str]| virtinst_xml::resolve_path(&doc, cap, path).map(|e| e.text_content(&doc));
    let drive_type = text(&["drive_type"])?;
    if !matches!(drive_type.as_str(), "cdrom" | "floppy") {
        return None;
    }
    let block = text(&["block"]).unwrap_or_default();
    let removable = cap
        .find_all(&doc, "capability")
        .into_iter()
        .find(|c| c.attribute(&doc, "type") == Some("removable"));
    let sub = |name: &str| {
        removable
            .and_then(|r| r.find(&doc, name))
            .map(|e| e.text_content(&doc))
    };
    let media_available = sub("media_available").is_some_and(|v| v.trim() == "1");
    let mut media_label = sub("media_label")
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| "Media Unknown".into());
    if !media_available {
        media_label = "No media detected".into();
    }
    Some(Drive {
        drive_type,
        row: MediaRow {
            path: block.clone(),
            label: format!("{media_label} ({block})"),
            has_media: media_available,
            key: name,
        },
    })
}

type ChangedCb = Box<dyn Fn()>;

pub struct MediaCombo {
    pub top_box: gtk::Box,
    pub entry: gtk::Entry,
    list: gtk::ListBox,
    popover: gtk::Popover,
    conn: RefCell<Option<VmmConnection>>,
    /// The rows shown, `None` for the separator.
    rows: RefCell<Vec<Option<MediaRow>>>,
    /// The row the entry shows, until the text is edited.
    active: Cell<Option<usize>>,
    /// Set while the entry is filled in from a row.
    setting: Cell<bool>,
    drives: RefCell<Option<Vec<Drive>>>,
    is_floppy: Cell<bool>,
    changed: RefCell<Option<ChangedCb>>,
}

impl MediaCombo {
    pub fn new(conn: Option<VmmConnection>) -> Rc<MediaCombo> {
        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some("No media selected"));
        entry.set_hexpand(true);
        entry.update_property(&[gtk::accessible::Property::Label("media-entry")]);
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        let scroll = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .propagate_natural_width(true)
            .max_content_height(400)
            .build();
        let popover = gtk::Popover::new();
        popover.set_child(Some(&scroll));
        let button = gtk::MenuButton::new();
        button.set_popover(Some(&popover));
        button.update_property(&[gtk::accessible::Property::Label("media-combo")]);
        let combo = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        combo.add_css_class("linked");
        combo.set_hexpand(true);
        combo.append(&entry);
        combo.append(&button);
        // Like a combo's drop-down, the list is as wide as the combo.
        let c = combo.clone();
        popover.connect_show(move |p| p.set_size_request(c.width(), -1));
        let top_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        top_box.set_hexpand(true);
        top_box.append(&combo);

        let this = Rc::new(MediaCombo {
            top_box,
            entry,
            list,
            popover,
            conn: RefCell::new(conn),
            rows: RefCell::default(),
            active: Cell::new(None),
            setting: Cell::new(false),
            drives: RefCell::default(),
            is_floppy: Cell::new(false),
            changed: RefCell::default(),
        });
        let weak = Rc::downgrade(&this);
        this.entry.connect_changed(move |_| {
            if let Some(this) = weak.upgrade() {
                if !this.setting.get() {
                    this.active.set(None);
                }
                if let Some(cb) = &*this.changed.borrow() {
                    cb();
                }
            }
        });
        this.entry.connect_icon_press(|entry, _| entry.set_text(""));
        let weak = Rc::downgrade(&this);
        this.list.connect_row_activated(move |_, row| {
            if let Some(this) = weak.upgrade() {
                this.set_active(row.index() as usize);
                this.popover.popdown();
            }
        });
        this
    }

    /// Calls `cb` whenever the entry changes: the `changed` signal.
    pub fn connect_changed(&self, cb: impl Fn() + 'static) {
        self.changed.replace(Some(Box::new(cb)));
    }

    fn set_active(&self, idx: usize) {
        let label = match self.rows.borrow().get(idx) {
            Some(Some(row)) => row.label.clone(),
            _ => return,
        };
        self.setting.set(true);
        self.entry.set_text(&label);
        self.setting.set(false);
        self.active.set(Some(idx));
        self.entry.set_position(-1);
    }

    /// `_init_rows`' node device half, fetched once on a worker.
    fn init_drives(self: &Rc<Self>) {
        if self.drives.borrow().is_some() {
            return;
        }
        self.drives.replace(Some(vec![]));
        let Some(handle) = self.conn.borrow().as_ref().and_then(|c| c.handle()) else {
            return;
        };
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let res = gio::spawn_blocking(move || {
                let devs = handle
                    .connect()
                    .list_all_node_devices(virt::sys::VIR_CONNECT_LIST_NODE_DEVICES_CAP_STORAGE)
                    .unwrap_or_default();
                devs.iter()
                    .filter_map(|d| d.get_xml_desc(0).ok())
                    .filter_map(|xml| parse_drive(&xml))
                    .collect::<Vec<_>>()
            })
            .await;
            let (Some(this), Ok(drives)) = (weak.upgrade(), res) else {
                return;
            };
            let found = !drives.is_empty();
            this.drives.replace(Some(drives));
            if found {
                this.rebuild_rows();
            }
        });
    }

    fn rebuild_rows(&self) {
        let media_type = if self.is_floppy.get() {
            "floppy"
        } else {
            "cdrom"
        };
        let mut rows: Vec<Option<MediaRow>> = crate::engine::get()
            .config()
            .get_iso_paths()
            .iter()
            .map(|p| Some(MediaRow::iso(p)))
            .collect();
        let drive_rows: Vec<_> = self
            .drives
            .borrow()
            .iter()
            .flatten()
            .filter(|d| d.drive_type == media_type)
            .map(|d| Some(d.row.clone()))
            .collect();
        if !rows.is_empty() && !drive_rows.is_empty() {
            rows.push(None);
        }
        rows.extend(drive_rows);

        let active = self
            .active
            .get()
            .and_then(|i| self.rows.borrow().get(i).cloned().flatten());
        self.list.remove_all();
        for row in &rows {
            let child: gtk::Widget = match row {
                Some(r) => {
                    let l = gtk::Label::new(Some(&r.label));
                    l.set_xalign(0.0);
                    l.set_margin_top(3);
                    l.set_margin_bottom(3);
                    l.set_margin_start(3);
                    l.upcast()
                }
                None => gtk::Separator::new(gtk::Orientation::Horizontal).upcast(),
            };
            let lbrow = gtk::ListBoxRow::builder().child(&child).build();
            lbrow.set_activatable(row.is_some());
            self.list.append(&lbrow);
        }
        self.active
            .set(active.and_then(|a| rows.iter().position(|r| r.as_ref() == Some(&a))));
        self.rows.replace(rows);
    }

    pub fn set_conn(&self, conn: Option<VmmConnection>) {
        self.conn.replace(conn);
        self.drives.replace(None);
    }

    /// `reset_state`.
    pub fn reset_state(self: &Rc<Self>, is_floppy: bool) {
        self.is_floppy.set(is_floppy);
        self.init_drives();
        self.entry.set_text("");
        self.active.set(None);
        self.rebuild_rows();
    }

    /// `get_path`: the picked row's path, else the typed text. Typed
    /// paths that aren't host devices are remembered for next time.
    pub fn get_path(&self, store_media: bool) -> String {
        let ret = match self
            .active
            .get()
            .and_then(|i| self.rows.borrow().get(i).cloned().flatten())
        {
            Some(row) => row.path,
            None => self.entry.text().trim().to_string(),
        };
        if store_media && !ret.starts_with("/dev") {
            crate::engine::get().config().add_iso_path(&ret);
        }
        ret
    }

    /// `set_path`.
    pub fn set_path(&self, path: &str) {
        let idx = self
            .rows
            .borrow()
            .iter()
            .position(|r| r.as_ref().is_some_and(|r| r.path == path));
        match idx {
            Some(idx) => self.set_active(idx),
            None => {
                self.entry.set_text(path);
                self.entry.set_position(-1);
            }
        }
    }

    pub fn set_mnemonic_label(&self, label: &gtk::Label) {
        label.set_mnemonic_widget(Some(&self.entry));
    }

    pub fn show_clear_icon(&self) {
        let pos = gtk::EntryIconPosition::Secondary;
        self.entry
            .set_icon_from_icon_name(pos, Some("edit-clear-symbolic"));
        self.entry.set_icon_activatable(pos, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_optical_drives_only() {
        let cd = "<device><name>block_sr0</name><capability type='storage'>\
            <block>/dev/sr0</block><drive_type>cdrom</drive_type>\
            <capability type='removable'><media_available>1</media_available>\
            <media_label>Fedora-41</media_label></capability></capability></device>";
        let d = parse_drive(cd).unwrap();
        assert_eq!(d.drive_type, "cdrom");
        assert_eq!(d.row.label, "Fedora-41 (/dev/sr0)");
        assert!(d.row.has_media);
        assert_eq!(d.row.key, "block_sr0");

        let empty = cd.replace("<media_available>1", "<media_available>0");
        assert_eq!(
            parse_drive(&empty).unwrap().row.label,
            "No media detected (/dev/sr0)"
        );
        let disk = cd.replace("cdrom", "disk");
        assert!(parse_drive(&disk).is_none());
    }
}
