//! `oslist.py`: the searchable OS picker shared by the details page and
//! (later) the create wizard. A search entry pops up a filtered list of
//! every libosinfo OS, hiding end-of-life ones unless asked.
//!
//! GTK4 popovers that don't autohide never take keyboard focus, so the
//! Up/Down keys move the list selection from the entry itself instead
//! of handing focus to the list the way the GTK3 version does.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use virtinst_core::osinfo::{OsDb, OsVariant};

thread_local! {
    static ALL_OS: OnceCell<Rc<Vec<OsVariant>>> = const { OnceCell::new() };
}

/// `OSDB.list_os(sortkey="label")`, generic entries last, loaded once.
pub fn all_os() -> Rc<Vec<OsVariant>> {
    ALL_OS.with(|c| {
        c.get_or_init(|| {
            Rc::new(match OsDb::load_default() {
                Ok(db) => db.list_os_variants(),
                Err(e) => {
                    eprintln!("Error loading the libosinfo database: {e}");
                    vec![OsVariant::generic()]
                }
            })
        })
        .clone()
    })
}

/// The OS for a libosinfo full id, falling back to `generic` like
/// `Guest.osinfo` does.
pub fn lookup_os_by_full_id(full_id: Option<&str>) -> OsVariant {
    full_id
        .and_then(|id| {
            all_os()
                .iter()
                .find(|o| o.full_id.as_deref() == Some(id))
                .cloned()
        })
        .unwrap_or_else(OsVariant::generic)
}

type SelectedCb = Box<dyn Fn(Option<&OsVariant>)>;

pub struct OsList {
    pub search_entry: gtk::SearchEntry,
    popover: gtk::Popover,
    scroll: gtk::ScrolledWindow,
    list: gtk::ListBox,
    include_eol: gtk::CheckButton,
    all_os: Rc<Vec<OsVariant>>,
    filter_name: Rc<RefCell<String>>,
    filter_eol: Rc<Cell<bool>>,
    selected_os: RefCell<Option<OsVariant>>,
    os_selected: RefCell<Option<SelectedCb>>,
}

impl OsList {
    pub fn new() -> Rc<OsList> {
        let search_entry = gtk::SearchEntry::new();
        search_entry.set_placeholder_text(Some("Type to start searching..."));
        search_entry.set_hexpand(true);
        search_entry.update_property(&[gtk::accessible::Property::Label("os-name")]);

        let eol_warn = gtk::Label::new(None);
        eol_warn.set_markup(&format!(
            "<small>{}</small>",
            glib::markup_escape_text(
                "Can't find the operating system you are looking for?\n\
                 Try selecting a similar distro or version, or use one of the 'Generic' options."
            )
        ));
        eol_warn.set_halign(gtk::Align::Start);
        eol_warn.set_xalign(0.0);
        eol_warn.set_wrap(true);
        eol_warn.set_max_width_chars(40);
        eol_warn.set_hexpand(true);
        let info = gtk::Image::from_icon_name("dialog-information");
        info.set_icon_size(gtk::IconSize::Large);
        info.set_valign(gtk::Align::Start);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.append(&info);
        header.append(&eol_warn);

        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        list.set_activate_on_single_click(true);
        list.set_show_separators(true);
        list.update_property(&[gtk::accessible::Property::Label("os-list")]);
        let scroll = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Always)
            .has_frame(true)
            .vexpand(true)
            .build();

        let include_eol = gtk::CheckButton::with_label("Include end of life operating systems");
        include_eol.set_halign(gtk::Align::Start);
        include_eol.update_property(&[gtk::accessible::Property::Label("include-eol")]);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
        content.append(&header);
        content.append(&scroll);
        content.append(&include_eol);
        let popover = gtk::Popover::new();
        popover.set_child(Some(&content));
        popover.set_autohide(false);
        popover.set_position(gtk::PositionType::Bottom);
        popover.set_parent(&search_entry);
        let p = popover.clone();
        search_entry.connect_destroy(move |_| p.unparent());

        let this = Rc::new(OsList {
            search_entry,
            popover,
            scroll,
            list,
            include_eol,
            all_os: all_os(),
            filter_name: Rc::default(),
            filter_eol: Rc::new(Cell::new(true)),
            selected_os: RefCell::default(),
            os_selected: RefCell::default(),
        });
        this.init_state();
        this.connect_signals();
        this
    }

    /// `_init_state`.
    fn init_state(&self) {
        for os in self.all_os.iter() {
            let label = gtk::Label::new(Some(&format!("{} ({})", os.label, os.name)));
            label.set_xalign(0.0);
            label.set_margin_top(3);
            label.set_margin_bottom(3);
            label.set_margin_start(3);
            self.list.append(&label);
        }
        let all_os = self.all_os.clone();
        let filter_name = self.filter_name.clone();
        let filter_eol = self.filter_eol.clone();
        self.list.set_filter_func(move |row| {
            let Some(os) = all_os.get(row.index() as usize) else {
                return false;
            };
            filter_os(os, &filter_name.borrow(), filter_eol.get())
        });
    }

    fn connect_signals(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.include_eol.connect_toggled(move |src| {
            if let Some(this) = weak.upgrade() {
                this.filter_eol.set(!src.is_active());
                this.refilter();
            }
        });
        let weak = Rc::downgrade(self);
        self.search_entry.connect_activate(move |_| {
            if let Some(this) = weak.upgrade() {
                this.entry_activate();
            }
        });
        let weak = Rc::downgrade(self);
        self.search_entry.connect_search_changed(move |src| {
            if let Some(this) = weak.upgrade() {
                this.search_changed(src);
            }
        });
        let weak = Rc::downgrade(self);
        self.search_entry.connect_stop_search(move |_| {
            if let Some(this) = weak.upgrade() {
                this.stop_search();
            }
        });
        let weak = Rc::downgrade(self);
        self.list.connect_row_activated(move |list, row| {
            if let Some(this) = weak.upgrade() {
                list.select_row(Some(row));
                this.sync_os_selection();
            }
        });
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(this) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let step = match key {
                gtk::gdk::Key::Down => 1,
                gtk::gdk::Key::Up => -1,
                _ => return glib::Propagation::Proceed,
            };
            if !this.popover.is_visible() {
                this.show_popover();
            } else {
                this.move_selection(step);
            }
            glib::Propagation::Stop
        });
        self.search_entry.add_controller(keys);
    }

    /// Calls `cb` with the picked OS: the `os-selected` signal.
    pub fn connect_os_selected(&self, cb: impl Fn(Option<&OsVariant>) + 'static) {
        self.os_selected.replace(Some(Box::new(cb)));
    }

    // Private helpers

    fn visible_rows(&self) -> Vec<gtk::ListBoxRow> {
        let mut rows = vec![];
        let mut idx = 0;
        while let Some(row) = self.list.row_at_index(idx) {
            if row.is_child_visible() {
                rows.push(row);
            }
            idx += 1;
        }
        rows
    }

    fn set_default_selection(&self) {
        if !self.popover.is_visible() {
            return;
        }
        if let Some(first) = self.visible_rows().first() {
            self.list.select_row(Some(first));
        }
    }

    fn move_selection(&self, step: i32) {
        let rows = self.visible_rows();
        let cur = self
            .list
            .selected_row()
            .and_then(|sel| rows.iter().position(|r| *r == sel));
        let next = match cur {
            Some(i) => (i as i32 + step).clamp(0, rows.len() as i32 - 1) as usize,
            None => 0,
        };
        if let Some(row) = rows.get(next) {
            self.list.select_row(Some(row));
            let adj = self.scroll.vadjustment();
            if let Some(bounds) = row.compute_bounds(&self.list) {
                let (y, h) = (bounds.y() as f64, bounds.height() as f64);
                if y < adj.value() {
                    adj.set_value(y);
                } else if y + h > adj.value() + adj.page_size() {
                    adj.set_value(y + h - adj.page_size());
                }
            }
        }
    }

    fn refilter(&self) {
        self.list.unselect_all();
        self.list.invalidate_filter();
        self.set_default_selection();
    }

    fn filter_by_name(&self, partial_name: &str) {
        self.filter_name.replace(partial_name.to_lowercase());
        self.refilter();
    }

    fn clear_filter(&self) {
        self.filter_by_name("");
        self.scroll.vadjustment().set_value(0.0);
    }

    fn sync_os_selection(&self) {
        let selected = self
            .list
            .selected_row()
            .and_then(|row| self.all_os.get(row.index() as usize).cloned());
        if let Some(os) = &selected {
            self.search_entry.set_text(&os.label);
        }
        self.selected_os.replace(selected.clone());
        self.popover.popdown();
        if let Some(cb) = &*self.os_selected.borrow() {
            cb(selected.as_ref());
        }
    }

    fn show_popover(&self) {
        // Match the width to the search entry's.
        self.popover
            .set_size_request(self.search_entry.width().max(300), 350);
        self.popover.popup();
        self.set_default_selection();
    }

    // UI callbacks

    fn entry_activate(&self) {
        if !self.popover.is_visible() {
            return;
        }
        let os = self
            .list
            .selected_row()
            .and_then(|row| self.all_os.get(row.index() as usize).cloned());
        if let Some(os) = os {
            self.select_os(&os);
        }
    }

    fn search_changed(&self, src: &gtk::SearchEntry) {
        let text = src.text();
        let searchname = text.trim();
        let selected_label = self.selected_os.borrow().as_ref().map(|o| o.label.clone());
        if !src.is_sensitive()
            || searchname.is_empty()
            || selected_label.as_deref() == Some(searchname)
        {
            self.popover.popdown();
            self.clear_filter();
            return;
        }
        self.filter_by_name(searchname);
        self.show_popover();
    }

    /// Escape: back to the selected OS's label.
    fn stop_search(&self) {
        let label = self.selected_os.borrow().as_ref().map(|o| o.label.clone());
        self.search_entry.set_text(label.as_deref().unwrap_or(""));
        self.popover.popdown();
    }

    // Public APIs

    pub fn select_os(&self, vmosobj: &OsVariant) {
        self.clear_filter();
        if vmosobj.eol && !self.include_eol.is_active() {
            self.include_eol.set_active(true);
        }
        let Some(idx) = self.all_os.iter().position(|o| o.name == vmosobj.name) else {
            return;
        };
        if let Some(row) = self.list.row_at_index(idx as i32) {
            self.list.select_row(Some(&row));
            self.sync_os_selection();
        }
    }

    pub fn get_selected_os(&self) -> Option<OsVariant> {
        self.selected_os.borrow().clone()
    }
}

/// `_filter_os_cb`.
fn filter_os(os: &OsVariant, filter_name: &str, filter_eol: bool) -> bool {
    if filter_eol && os.eol {
        return false;
    }
    if os.always_show() {
        return true;
    }
    filter_name.is_empty()
        || os.label.to_lowercase().contains(filter_name)
        || os.name.to_lowercase().contains(filter_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(name: &str, label: &str, eol: bool) -> OsVariant {
        OsVariant {
            name: name.into(),
            label: label.into(),
            full_id: None,
            eol,
        }
    }

    #[test]
    fn filter_matches_label_or_name_and_hides_eol() {
        let f40 = os("fedora40", "Fedora Linux 40", false);
        let f25 = os("fedora25", "Fedora 25", true);
        assert!(filter_os(&f40, "", true));
        assert!(filter_os(&f40, "linux 4", true));
        assert!(filter_os(&f40, "fedora40", true));
        assert!(!filter_os(&f40, "ubuntu", true));
        assert!(!filter_os(&f25, "fedora", true));
        assert!(filter_os(&f25, "fedora", false));
        assert!(filter_os(&OsVariant::generic(), "ubuntu", true));
        assert!(filter_os(
            &os("linux2022", "Generic Linux 2022", false),
            "win",
            true
        ));
    }
}
