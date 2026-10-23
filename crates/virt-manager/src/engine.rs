//! Upstream's `virtManager/engine.py` (`vmmEngine`): application
//! lifecycle, the stats/poll tick, connection autostart, first-run
//! default-connection probing, and the CLI's `--connect` handling.
//!
//! One engine per process, reachable from anywhere on the GTK thread
//! via [`get`] — upstream's `vmmEngine.get_instance()`.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::VecDeque;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use crate::config::Config;
use crate::connection::{ConnectError, VmmConnection};
use crate::connmanager::VmmConnectionManager;
use crate::error;
use crate::manager::VmmManager;

/// Distinct from upstream's `org.virt-manager.virt-manager` so a running
/// Python virt-manager doesn't swallow this app's activation over D-Bus.
/// Settings and icons still use upstream's ids/paths.
pub const APP_ID: &str = "org.virt-manager.virt-manager-rust";
const RESOURCE_BASE: &str = "/org/virt-manager/virt-manager";

thread_local! {
    static ENGINE: OnceCell<Rc<Engine>> = const { OnceCell::new() };
}

/// The process-wide engine. Panics if called before [`run`] set it up.
pub fn get() -> Rc<Engine> {
    ENGINE.with(|e| e.get().cloned().expect("engine not started"))
}

pub struct Engine {
    app: gtk::Application,
    config: Rc<Config>,
    connmanager: VmmConnectionManager,
    manager: OnceCell<VmmManager>,
    timer: RefCell<Option<glib::SourceId>>,
    window_count: Cell<i32>,
    exiting: Cell<bool>,
}

/// What `virt-manager`'s command line asked for.
#[derive(Clone, Debug, Default)]
pub struct StartupOptions {
    pub uri: Option<String>,
}

/// `createconn._default_uri`: probe the host for a hypervisor.
pub fn default_uri() -> Option<&'static str> {
    let exists = |p: &str| Path::new(p).exists();
    if exists("/var/lib/xen") && (exists("/dev/xen/evtchn") || exists("/proc/xen")) {
        return Some("xen:///");
    }
    let qemu_system = std::fs::read_dir("/usr/bin")
        .map(|d| {
            d.flatten().any(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with("qemu-system-"))
            })
        })
        .unwrap_or(false);
    if exists("/usr/bin/qemu")
        || exists("/usr/bin/qemu-kvm")
        || exists("/usr/bin/kvm")
        || exists("/usr/libexec/qemu-kvm")
        || qemu_system
    {
        return Some("qemu:///system");
    }
    if exists("/usr/lib/libvirt/libvirt_lxc") || exists("/usr/lib64/libvirt/libvirt_lxc") {
        return Some("lxc:///");
    }
    None
}

/// `connectauth.setup_first_uri`.
pub fn setup_first_uri(detected_uri: Option<&str>) -> Option<String> {
    if detected_uri.is_some() {
        return None;
    }
    Some(
        "Could not detect a default hypervisor. Make \
         sure the appropriate QEMU/KVM virtualization and libvirt \
         packages are installed to manage virtualization \
         on this host.\n\n\
         A virtualization connection can be manually added via File->Add Connection"
            .to_string(),
    )
}

/// Builds the application and runs the GTK main loop.
pub fn run(opts: StartupOptions) -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .resource_base_path(RESOURCE_BASE)
        .build();
    let opts = Rc::new(RefCell::new(Some(opts)));
    app.connect_activate(move |app| {
        if let Some(opts) = opts.borrow_mut().take() {
            Engine::startup(app, opts);
        } else {
            get().get_manager().show();
        }
    });
    // Our own argv was already parsed by clap; don't let GApplication
    // reject flags it doesn't know.
    app.run_with_args::<&str>(&[])
}

impl Engine {
    fn startup(app: &gtk::Application, opts: StartupOptions) {
        gio::resources_register_include!("virt-manager.gresource")
            .expect("bundled resources register");
        gtk::Window::set_default_icon_name("virt-manager");
        // Upstream registers a libvirt error handler that only logs at
        // debug level; without one libvirt prints every expected error
        // (e.g. "metadata not found" on each poll) to stderr.
        virt::error::clear_error_callback();

        let config = Rc::new(Config::new());
        let connmanager = VmmConnectionManager::new(config.clone());
        let engine = Rc::new(Engine {
            app: app.clone(),
            config,
            connmanager,
            manager: OnceCell::new(),
            timer: RefCell::new(None),
            window_count: Cell::new(0),
            exiting: Cell::new(false),
        });
        ENGINE.with(|e| {
            let _ = e.set(engine.clone());
        });
        engine.default_startup(opts.uri.is_some());
        engine.handle_cli_command(opts.uri);
    }

    pub fn config(&self) -> Rc<Config> {
        self.config.clone()
    }

    pub fn connmanager(&self) -> VmmConnectionManager {
        self.connmanager.clone()
    }

    pub fn get_manager(&self) -> VmmManager {
        self.manager
            .get_or_init(|| VmmManager::new(&self.app))
            .clone()
    }

    fn default_startup(&self, have_cliuri: bool) {
        // Build the manager first so it sees every conn-added.
        let manager = self.get_manager();
        self.config
            .on_stats_update_interval_changed(|| get().schedule_timer());
        self.schedule_timer();
        self.tick();

        glib::idle_add_local_once(|| get().autostart_conns());

        if self.config.get_conn_uris().is_empty() && !have_cliuri {
            // Only add default if no connections are currently known
            manager.set_startup_error("Checking for virtualization packages...");
            glib::timeout_add_local_once(Duration::from_millis(1000), || get().add_default_conn());
        }
    }

    fn add_default_conn(&self) {
        let detected_uri = default_uri();
        if let Some(msg) = setup_first_uri(detected_uri) {
            self.get_manager().set_startup_error(&msg);
            return;
        }
        let uri = detected_uri.expect("checked above");
        let conn = self.connmanager.add_conn(uri);
        self.connmanager.set_autoconnect(&conn, true);
        conn.open(|err| {
            if let Some(err) = err {
                get().handle_conn_error(err);
            }
        });
    }

    /// Opens autoconnect URIs one after another, so polkit/ssh-askpass
    /// prompts don't pile up. Errors are ignored, as upstream does.
    fn autostart_conns(&self) {
        let queue: VecDeque<VmmConnection> = self
            .connmanager
            .conns()
            .into_iter()
            .filter(|c| self.connmanager.get_autoconnect(c))
            .collect();
        fn next(mut queue: VecDeque<VmmConnection>) {
            let Some(conn) = queue.pop_front() else {
                return;
            };
            if get().exiting.get() {
                return;
            }
            if !conn.is_disconnected() {
                next(queue);
                return;
            }
            conn.open(move |err| {
                if let Some(err) = err {
                    eprintln!("Autostart connection error: {}", err.details);
                }
                next(queue);
            });
        }
        next(queue);
    }

    pub fn schedule_timer(&self) {
        if let Some(id) = self.timer.take() {
            id.remove();
        }
        let interval = self.config.get_stats_update_interval();
        let id = glib::timeout_add_seconds_local(interval, || {
            get().tick();
            glib::ControlFlow::Continue
        });
        self.timer.replace(Some(id));
    }

    fn tick(&self) {
        for conn in self.connmanager.conns() {
            conn.tick();
        }
    }

    fn handle_cli_command(&self, uri: Option<String>) {
        let Some(uri) = uri else {
            self.get_manager().show();
            return;
        };
        let conn_is_new = self.connmanager.get_conn(&uri).is_none();
        let conn = self.connmanager.add_conn(&uri);
        if conn.is_active() {
            self.launch_manager(&uri);
            return;
        }
        conn.open(move |err| {
            let engine = get();
            match err {
                Some(err) => {
                    if conn_is_new {
                        engine.connmanager.remove_conn(&uri);
                    }
                    engine.handle_conn_error(err);
                }
                None => engine.launch_manager(&uri),
            }
        });
    }

    fn launch_manager(&self, uri: &str) {
        let manager = self.get_manager();
        manager.set_initial_selection(uri);
        manager.show();
    }

    fn handle_conn_error(&self, err: ConnectError) {
        let parent = self.manager.get().map(|m| m.window());
        glib::spawn_future_local(async move {
            let parent = parent.filter(|w| w.is_visible());
            error::show_err(parent.as_ref(), &err.msg, Some(&err.details), &err.title).await;
            get().exit_app_if_no_windows();
        });
    }

    pub fn increment_window_counter(&self) {
        self.window_count.set(self.window_count.get() + 1);
    }

    pub fn decrement_window_counter(&self) {
        self.window_count.set(self.window_count.get() - 1);
        self.exit_app_if_no_windows();
    }

    fn exit_app_if_no_windows(&self) {
        if !self.exiting.get() && self.window_count.get() <= 0 {
            self.exit_app();
        }
    }

    pub fn exit_app(&self) {
        if self.exiting.replace(true) {
            return;
        }
        glib::idle_add_local_once(|| {
            let engine = get();
            engine.connmanager.cleanup();
            if let Some(manager) = engine.manager.get() {
                manager.save_window_size();
            }
            engine.app.quit();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_uri_message_only_without_a_detected_uri() {
        assert!(setup_first_uri(Some("qemu:///system")).is_none());
        let msg = setup_first_uri(None).unwrap();
        assert!(msg.starts_with("Could not detect a default hypervisor."));
        assert!(msg.ends_with("via File->Add Connection"));
    }
}
