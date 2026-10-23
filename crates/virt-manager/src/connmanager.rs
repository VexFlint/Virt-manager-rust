//! Upstream's `virtManager/connmanager.py` (`vmmConnectionManager`): the
//! set of known connections, mirrored to the `connections/uris` setting.
//! Signals, named as upstream's: `conn-added(VmmConnection)`,
//! `conn-removed(uri)`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

use gtk::glib;
use gtk::glib::prelude::*;
use gtk::glib::subclass::prelude::*;
use gtk::glib::subclass::Signal;

use crate::config::Config;
use crate::connection::VmmConnection;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct VmmConnectionManager {
        pub conns: RefCell<Vec<VmmConnection>>,
        pub config: RefCell<Option<Rc<Config>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for VmmConnectionManager {
        const NAME: &'static str = "VmmConnectionManager";
        type Type = super::VmmConnectionManager;
    }

    impl ObjectImpl for VmmConnectionManager {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    Signal::builder("conn-added")
                        .param_types([VmmConnection::static_type()])
                        .build(),
                    Signal::builder("conn-removed")
                        .param_types([String::static_type()])
                        .build(),
                ]
            })
        }
    }
}

glib::wrapper! {
    pub struct VmmConnectionManager(ObjectSubclass<imp::VmmConnectionManager>);
}

impl VmmConnectionManager {
    /// Loads the saved URIs, like upstream's `__init__`.
    pub fn new(config: Rc<Config>) -> VmmConnectionManager {
        let obj: VmmConnectionManager = glib::Object::new();
        for uri in config.get_conn_uris() {
            obj.imp().conns.borrow_mut().push(VmmConnection::new(&uri));
        }
        obj.imp().config.replace(Some(config));
        obj
    }

    fn config(&self) -> Rc<Config> {
        self.imp().config.borrow().clone().expect("set in new()")
    }

    pub fn conns(&self) -> Vec<VmmConnection> {
        self.imp().conns.borrow().clone()
    }

    pub fn get_conn(&self, uri: &str) -> Option<VmmConnection> {
        self.conns().into_iter().find(|c| c.get_uri() == uri)
    }

    pub fn add_conn(&self, uri: &str) -> VmmConnection {
        if let Some(conn) = self.get_conn(uri) {
            return conn;
        }
        let conn = VmmConnection::new(uri);
        self.imp().conns.borrow_mut().push(conn.clone());
        self.config().add_conn_uri(uri);
        self.emit_by_name::<()>("conn-added", &[&conn]);
        conn
    }

    pub fn remove_conn(&self, uri: &str) {
        let Some(conn) = self.get_conn(uri) else {
            return;
        };
        conn.close();
        self.imp().conns.borrow_mut().retain(|c| c != &conn);
        self.config().remove_conn_uri(uri);
        self.emit_by_name::<()>("conn-removed", &[&uri.to_string()]);
    }

    pub fn get_autoconnect(&self, conn: &VmmConnection) -> bool {
        self.config().get_conn_autoconnect(&conn.get_uri())
    }

    pub fn set_autoconnect(&self, conn: &VmmConnection, val: bool) {
        self.config().set_conn_autoconnect(&conn.get_uri(), val);
    }

    pub fn cleanup(&self) {
        for conn in self.conns() {
            conn.close();
        }
    }
}
