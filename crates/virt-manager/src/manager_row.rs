//! One row of the manager's VM list — upstream's `Gtk.TreeStore` row
//! (`ROW_HANDLE`, `ROW_SORT_KEY`, `ROW_MARKUP`, `ROW_STATUS_ICON`,
//! `ROW_HINT`, `ROW_IS_CONN`, `ROW_IS_VM`, `ROW_COLOR`) as a GObject, so
//! ticket 13's `GtkTreeListModel` can hold it and the row widgets can
//! bind to its properties. Connection rows own a child store of VM rows.

use std::cell::RefCell;

use gtk::gio;
use gtk::glib;
use gtk::glib::prelude::*;
use gtk::glib::subclass::prelude::*;

use crate::connection::VmmConnection;
use crate::domain::VmmDomain;

#[derive(Clone, Debug, PartialEq)]
pub enum RowHandle {
    Conn(VmmConnection),
    Vm(VmmDomain),
}

mod imp {
    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::ManagerRow)]
    pub struct ManagerRow {
        #[property(get, set)]
        pub markup: RefCell<String>,
        #[property(get, set)]
        pub status_icon: RefCell<String>,
        #[property(get, set)]
        pub hint: RefCell<String>,
        #[property(get, set)]
        pub is_vm: std::cell::Cell<bool>,
        /// `ROW_COLOR`'s job: disconnected connections render greyed.
        #[property(get, set)]
        pub sensitive: std::cell::Cell<bool>,
        pub sort_key: RefCell<String>,
        pub handle: RefCell<Option<RowHandle>>,
        pub children: RefCell<Option<gio::ListStore>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ManagerRow {
        const NAME: &'static str = "VmmManagerRow";
        type Type = super::ManagerRow;
    }

    #[glib::derived_properties]
    impl ObjectImpl for ManagerRow {}
}

glib::wrapper! {
    pub struct ManagerRow(ObjectSubclass<imp::ManagerRow>);
}

impl ManagerRow {
    pub fn new(handle: RowHandle) -> ManagerRow {
        let obj: ManagerRow = glib::Object::new();
        if matches!(handle, RowHandle::Conn(_)) {
            obj.imp()
                .children
                .replace(Some(gio::ListStore::new::<ManagerRow>()));
        } else {
            obj.set_is_vm(true);
        }
        obj.imp().handle.replace(Some(handle));
        obj
    }

    pub fn handle(&self) -> RowHandle {
        self.imp().handle.borrow().clone().expect("set in new()")
    }

    pub fn conn(&self) -> Option<VmmConnection> {
        match self.handle() {
            RowHandle::Conn(c) => Some(c),
            RowHandle::Vm(_) => None,
        }
    }

    pub fn vm(&self) -> Option<VmmDomain> {
        match self.handle() {
            RowHandle::Vm(v) => Some(v),
            RowHandle::Conn(_) => None,
        }
    }

    pub fn children(&self) -> Option<gio::ListStore> {
        self.imp().children.borrow().clone()
    }

    pub fn sort_key(&self) -> String {
        self.imp().sort_key.borrow().clone()
    }

    pub fn set_sort_key(&self, key: &str) {
        self.imp().sort_key.replace(key.to_string());
    }
}
