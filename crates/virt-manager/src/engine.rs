//! Upstream's `virtManager/engine.py` (`vmmEngine`): application
//! lifecycle, the stats/poll tick, connection autostart, first-run
//! default-connection probing, and the CLI's `--connect` and `--show-*`
//! window launchers.
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
use crate::domain::VmmDomain;
use crate::error;
use crate::manager::VmmManager;
use crate::systray::VmmSystray;

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

/// The `CLI_SHOW_*` windows besides the manager (`CLI_SHOW_MANAGER`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShowWindow {
    /// `CLI_SHOW_DOMAIN_CREATOR`
    DomainCreator,
    /// `CLI_SHOW_DOMAIN_EDITOR`
    DomainEditor,
    /// `CLI_SHOW_DOMAIN_PERFORMANCE`
    DomainPerformance,
    /// `CLI_SHOW_DOMAIN_CONSOLE`
    DomainConsole,
    /// `CLI_SHOW_DOMAIN_DELETE`
    DomainDelete,
    /// `CLI_SHOW_HOST_SUMMARY`
    HostSummary,
    /// `CLI_SHOW_SYSTEM_TRAY`
    SystemTray,
}

impl ShowWindow {
    /// The `CLI_SHOW_*` string sent in `cli_command`.
    fn cli_str(self) -> &'static str {
        match self {
            ShowWindow::DomainCreator => "creator",
            ShowWindow::DomainEditor => "editor",
            ShowWindow::DomainPerformance => "performance",
            ShowWindow::DomainConsole => "console",
            ShowWindow::DomainDelete => "delete",
            ShowWindow::HostSummary => "summary",
            ShowWindow::SystemTray => "systray",
        }
    }

    /// Back from a `cli_command` string; `CLI_SHOW_MANAGER` ("manager")
    /// and "" are `None`.
    fn from_cli_str(s: &str) -> Option<ShowWindow> {
        [
            ShowWindow::DomainCreator,
            ShowWindow::DomainEditor,
            ShowWindow::DomainPerformance,
            ShowWindow::DomainConsole,
            ShowWindow::DomainDelete,
            ShowWindow::HostSummary,
            ShowWindow::SystemTray,
        ]
        .into_iter()
        .find(|w| w.cli_str() == s)
    }
}

/// `engine.start()`'s `(sss)` `cli_command` payload.
fn cli_command_data(opts: &StartupOptions) -> (String, String, String) {
    let show = match opts.show_window {
        Some(w) => w.cli_str(),
        None if opts.uri.is_some() => "manager",
        None => "",
    };
    (
        opts.uri.clone().unwrap_or_default(),
        show.to_string(),
        opts.domain.clone().unwrap_or_default(),
    )
}

/// What `virt-manager`'s command line asked for: `engine.start()`'s
/// arguments.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StartupOptions {
    pub uri: Option<String>,
    /// The `--show-*` window (`CLI_SHOW_*`); `None` is the manager
    /// (`CLI_SHOW_MANAGER`).
    pub show_window: Option<ShowWindow>,
    /// The `NAME|ID|UUID` of a `--show-domain-*` option.
    pub domain: Option<String>,
    pub skip_autostart: bool,
}

/// `_find_vm_by_cli_str`'s test: a string of digits is an ID, anything
/// else a name or UUID.
fn vm_matches_cli_str(clistr: &str, id: Option<u32>, name: &str, uuid: &str) -> bool {
    if !clistr.is_empty() && clistr.bytes().all(|b| b.is_ascii_digit()) {
        // `int(clistr)` only ever equals `get_id()`; an inactive VM's
        // ID is -1, and a number too big for an ID matches nothing.
        return id.is_some_and(|id| clistr.parse::<u64>() == Ok(u64::from(id)));
    }
    clistr == name || clistr == uuid
}

/// `_find_vm_by_cli_str`: the first VM, in list order, that the string
/// names.
fn find_vm_by_cli_str(vms: &[VmmDomain], clistr: &str) -> Option<VmmDomain> {
    vms.iter()
        .find(|vm| vm_matches_cli_str(clistr, vm.get_id(), &vm.get_name(), &vm.get_uuid()))
        .cloned()
}

/// Upstream lists a connection's VMs (`_populate_initial_state`) before
/// `open-completed`; here that first poll is still on its worker when
/// the open finishes, so wait for it to land (or for the connection to
/// drop).
async fn wait_for_vm_list(conn: &VmmConnection) {
    if !conn.is_ticking() {
        return;
    }
    let (tx, rx) = futures_channel::oneshot::channel::<()>();
    let tx = Rc::new(RefCell::new(Some(tx)));
    let ids: Vec<glib::SignalHandlerId> = ["resources-sampled", "state-changed"]
        .into_iter()
        .map(|signal| {
            let tx = tx.clone();
            conn.connect_local(signal, false, move |_| {
                if let Some(tx) = tx.borrow_mut().take() {
                    let _ = tx.send(());
                }
                None
            })
        })
        .collect();
    let _ = rx.await;
    for id in ids {
        conn.disconnect(id);
    }
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

/// The details of `_show_systray_from_cli`'s error. Upstream's carry
/// the `RuntimeError` traceback; ours say why the icon wasn't embedded,
/// as the tray's D-Bus side reported it.
fn systray_error_details(msg: &str) -> String {
    let reason = VmmSystray::get_instance()
        .embed_error()
        .unwrap_or_else(|| "The system tray icon was not embedded within 5 seconds.".into());
    format!("{msg}\n\n{reason}")
}

/// Builds the application and runs the GTK main loop.
pub fn run(opts: StartupOptions) -> glib::ExitCode {
    // Before the Application exists: GTK can load the icon theme during
    // its own startup, and a theme loaded before the icons are registered
    // never sees them (every bundled icon draws as image-missing).
    gio::resources_register_include!("virt-manager.gresource").expect("bundled resources register");
    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .resource_base_path(RESOURCE_BASE)
        .build();
    // `cli_command`: how a second `virt-manager` hands its --connect and
    // --show-* options to the instance already running (`engine.start()`).
    let action =
        gio::SimpleAction::new("cli_command", Some(glib::VariantTy::new("(sss)").unwrap()));
    action.connect_activate(|_, data| {
        let Some((uri, show, domain)) = data.and_then(|d| d.get::<(String, String, String)>())
        else {
            return;
        };
        let nonempty = |s: String| (!s.is_empty()).then_some(s);
        get().handle_cli_command(
            nonempty(uri),
            ShowWindow::from_cli_str(&show),
            nonempty(domain),
        );
    });
    app.add_action(&action);
    if app.register(gio::Cancellable::NONE).is_ok() && app.is_remote() {
        // Connected to the remote app instance.
        app.activate_action("cli_command", Some(&cli_command_data(&opts).to_variant()));
        return glib::ExitCode::SUCCESS;
    }
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
        gtk::Window::set_default_icon_name("virt-manager");
        // Upstream registers a libvirt error handler that only logs at
        // debug level; without one libvirt prints every expected error
        // (e.g. "metadata not found" on each poll) to stderr.
        virt::error::clear_error_callback();

        let config = Rc::new(Config::new());
        crate::config::install_css();
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
        engine.default_startup(opts.uri.is_some(), opts.skip_autostart);
        engine.handle_cli_command(opts.uri, opts.show_window, opts.domain);
    }

    pub fn app(&self) -> gtk::Application {
        self.app.clone()
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

    fn default_startup(&self, have_cliuri: bool, skip_autostart: bool) {
        VmmSystray::get_instance();
        // Build the manager first so it sees every conn-added.
        let manager = self.get_manager();
        self.config
            .on_stats_update_interval_changed(|| get().schedule_timer());
        self.schedule_timer();
        self.tick();

        if !skip_autostart {
            glib::idle_add_local_once(|| get().autostart_conns());
        }

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

    /// `_handle_cli_command`.
    fn handle_cli_command(
        &self,
        uri: Option<String>,
        show_window: Option<ShowWindow>,
        domain: Option<String>,
    ) {
        if show_window == Some(ShowWindow::SystemTray) {
            // Launching only in the system tray
            self.show_systray_from_cli();
            if uri.is_none() {
                return;
            }
        }
        let Some(uri) = uri else {
            self.get_manager().show();
            return;
        };
        let conn_is_new = self.connmanager.get_conn(&uri).is_none();
        let conn = self.connmanager.add_conn(&uri);
        if conn.is_active() {
            self.launch_cli_window(&uri, show_window, domain.as_deref());
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
                None => engine.launch_cli_window(&uri, show_window, domain.as_deref()),
            }
        });
    }

    /// `_cli_show_vm_helper`: the VM's window on the requested page,
    /// plus the delete dialog for `--show-domain-delete`. A VM the
    /// connection doesn't have is a startup error (`_show_startup_error`).
    fn cli_show_vm_helper(&self, uri: &str, clistr: &str, page: ShowWindow) {
        let Some(conn) = self.connmanager.get_conn(uri) else {
            return;
        };
        let (uri, clistr) = (uri.to_string(), clistr.to_string());
        glib::spawn_future_local(async move {
            wait_for_vm_list(&conn).await;
            let Some(vm) = find_vm_by_cli_str(&conn.list_vms(), &clistr) else {
                // Upstream's details are the traceback, which ends with
                // this same RuntimeError text.
                let msg = format!("{uri} does not have VM '{clistr}'");
                get().show_startup_error(&msg, &msg);
                return;
            };
            let Some(details) = crate::vmwindow::VmmVMWindow::get_instance(&vm) else {
                return;
            };
            match page {
                ShowWindow::DomainPerformance => details.activate_performance_page(),
                ShowWindow::DomainEditor | ShowWindow::DomainDelete => {
                    details.activate_config_page()
                }
                ShowWindow::DomainConsole => details.activate_console_page(),
                _ => {}
            }
            details.show();
            if page == ShowWindow::DomainDelete {
                crate::delete::DeleteDialog::show(
                    Some(&details.window()),
                    &vm,
                    crate::delete::DeleteKind::Vm,
                );
            }
        });
    }

    /// `_launch_cli_window`. The tray was already handled by
    /// [`Self::show_systray_from_cli`].
    fn launch_cli_window(&self, uri: &str, show_window: Option<ShowWindow>, clistr: Option<&str>) {
        match show_window {
            None => self.launch_manager(uri),
            Some(ShowWindow::DomainCreator) => {
                crate::createvm::VmmCreateVM::show_instance(None, Some(uri.to_string()))
            }
            Some(ShowWindow::HostSummary) => {
                if let Some(conn) = self.connmanager.get_conn(uri) {
                    crate::host::VmmHost::show_instance(&conn);
                }
            }
            Some(
                page @ (ShowWindow::DomainEditor
                | ShowWindow::DomainPerformance
                | ShowWindow::DomainConsole
                | ShowWindow::DomainDelete),
            ) => self.cli_show_vm_helper(uri, clistr.unwrap_or_default(), page),
            // Handled elsewhere
            Some(ShowWindow::SystemTray) => {}
        }
    }

    fn launch_manager(&self, uri: &str) {
        let manager = self.get_manager();
        manager.set_initial_selection(uri);
        manager.show();
    }

    /// `_systray_is_embedded`: the tray icon isn't a window, and even
    /// when shown it may not be embedded in a tray area.
    fn systray_is_embedded(&self) -> bool {
        VmmSystray::get_instance().is_embedded()
    }

    /// `_show_systray_from_cli`: force the tray icon on, then give it
    /// five seconds to get embedded before failing like a startup
    /// routine.
    fn show_systray_from_cli(&self) {
        VmmSystray::get_instance().show_from_cli();
        fn check(count: u32) {
            let count = count - 1;
            let engine = get();
            if engine.systray_is_embedded() {
                return;
            }
            if count == 0 {
                let msg = "systray did not show up";
                engine.show_startup_error(msg, &systray_error_details(msg));
                return;
            }
            glib::timeout_add_local_once(Duration::from_millis(1000), move || check(count));
        }
        glib::timeout_add_local_once(Duration::from_millis(1000), || check(5));
    }

    /// `_show_startup_error`: the error with its details, then exit if
    /// nothing is left open.
    fn show_startup_error(&self, msg: &str, details: &str) {
        let msg = msg.to_string();
        let details = details.to_string();
        glib::spawn_future_local(async move {
            error::show_err(None, &msg, Some(&details), "").await;
            get().exit_app_if_no_windows();
        });
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

    /// `_can_exit`.
    fn can_exit(&self) -> bool {
        self.window_count.get() <= 0 && !self.systray_is_embedded()
    }

    fn exit_app_if_no_windows(&self) {
        if !self.exiting.get() && self.can_exit() {
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
            // app-closing
            VmmSystray::get_instance().cleanup();
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

    #[test]
    fn cli_command_data_round_trips_like_engine_start() {
        let opts = |uri: Option<&str>, show, domain: Option<&str>| StartupOptions {
            uri: uri.map(str::to_string),
            show_window: show,
            domain: domain.map(str::to_string),
            skip_autostart: false,
        };
        // `if uri and not show_window: show_window = CLI_SHOW_MANAGER`
        assert_eq!(
            cli_command_data(&opts(Some("test:///default"), None, None)),
            ("test:///default".into(), "manager".into(), String::new())
        );
        assert_eq!(
            cli_command_data(&opts(None, None, None)),
            Default::default()
        );
        assert_eq!(
            cli_command_data(&opts(
                Some("test:///default"),
                Some(ShowWindow::DomainConsole),
                Some("test")
            )),
            ("test:///default".into(), "console".into(), "test".into())
        );
        for w in [
            ShowWindow::DomainCreator,
            ShowWindow::DomainEditor,
            ShowWindow::DomainPerformance,
            ShowWindow::DomainConsole,
            ShowWindow::DomainDelete,
            ShowWindow::HostSummary,
            ShowWindow::SystemTray,
        ] {
            assert_eq!(ShowWindow::from_cli_str(w.cli_str()), Some(w));
        }
        assert_eq!(ShowWindow::from_cli_str("manager"), None);
        assert_eq!(ShowWindow::from_cli_str(""), None);
    }

    #[test]
    fn cli_str_finds_vm_by_id_name_or_uuid() {
        const UUID: &str = "6695eb01-f6a4-8304-79aa-97f2502e193f";
        // name and UUID
        assert!(vm_matches_cli_str("test", Some(1), "test", UUID));
        assert!(vm_matches_cli_str(UUID, Some(1), "test", UUID));
        assert!(!vm_matches_cli_str("tes", Some(1), "test", UUID));
        assert!(!vm_matches_cli_str("TEST", Some(1), "test", UUID));
        // digits are an ID: `int("01") == 1`
        assert!(vm_matches_cli_str("1", Some(1), "test", UUID));
        assert!(vm_matches_cli_str("01", Some(1), "test", UUID));
        assert!(!vm_matches_cli_str("2", Some(1), "test", UUID));
        // an inactive VM has no ID, and digits never match a name
        assert!(!vm_matches_cli_str("1", None, "test", UUID));
        assert!(!vm_matches_cli_str("42", None, "42", UUID));
        assert!(!vm_matches_cli_str(
            "99999999999999999999999",
            Some(1),
            "x",
            UUID
        ));
        // not all digits: a name again
        assert!(vm_matches_cli_str("-1", None, "-1", UUID));
        assert!(vm_matches_cli_str("1a", Some(1), "1a", UUID));
    }
}
