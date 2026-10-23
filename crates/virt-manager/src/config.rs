//! Upstream's `config.py` (`vmmConfig`) — GSettings-backed, against the
//! very same `org.virt-manager.virt-manager` schema (copied verbatim into
//! `data/`). Sharing the schema id and paths means this app reads and
//! writes the same dconf keys Python virt-manager does: the saved
//! connection list, window size, confirmation toggles all carry over.
//!
//! Schema lookup order:
//! 1. The system schema source — present when either this app or
//!    upstream virt-manager is installed.
//! 2. The copy `build.rs` compiled into `$OUT_DIR/schemas` — so
//!    `cargo run` works with nothing installed.
//!
//! `VIRT_MANAGER_TEST_MEMORY_SETTINGS` swaps the dconf backend for an
//! in-memory one, so tests never touch the developer's real settings.
//!
//! Only the keys the ported screens use are wrapped so far; each new
//! screen adds its own accessors here, named after upstream's.

use std::cell::RefCell;
use std::collections::HashMap;

use gtk::gio;
use gtk::gio::prelude::*;
use gtk::glib;

use crate::statsmanager::PollFlags;

const SCHEMA_ID: &str = "org.virt-manager.virt-manager";

/// `BuildConfig.default_graphics` (meson's `default-graphics` option).
pub const DEFAULT_GRAPHICS_FROM_CONFIG: &str = "spice";
/// `default_storage_format_from_config`.
pub const DEFAULT_STORAGE_FORMAT_FROM_CONFIG: &str = "qcow2";
/// `default_console_resizeguest`.
pub const DEFAULT_CONSOLE_RESIZEGUEST: i32 = 0;

/// virtinst's `DomainCpu.SPECIAL_MODES`.
pub const CPU_SPECIAL_MODES: &[&str] = &[
    "host-model-only",
    "hv-default",
    "host-copy",
    "host-model",
    "host-passthrough",
    "maximum",
    "clear",
    "default",
];

/// The manager's stats columns (upstream's `COL_GUEST_CPU` … `COL_NETWORK`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatsColumn {
    GuestCpu,
    HostCpu,
    Memory,
    Disk,
    Network,
}

impl StatsColumn {
    pub const ALL: [StatsColumn; 5] = [
        StatsColumn::GuestCpu,
        StatsColumn::HostCpu,
        StatsColumn::Memory,
        StatsColumn::Disk,
        StatsColumn::Network,
    ];

    fn vmlist_key(self) -> &'static str {
        match self {
            StatsColumn::GuestCpu => "cpu-usage",
            StatsColumn::HostCpu => "host-cpu-usage",
            StatsColumn::Memory => "memory-usage",
            StatsColumn::Disk => "disk-usage",
            StatsColumn::Network => "network-traffic",
        }
    }

    fn poll_key(self) -> &'static str {
        match self {
            StatsColumn::GuestCpu | StatsColumn::HostCpu => "enable-cpu-poll",
            StatsColumn::Memory => "enable-memory-poll",
            StatsColumn::Disk => "enable-disk-poll",
            StatsColumn::Network => "enable-net-poll",
        }
    }
}

pub struct Config {
    root: gio::Settings,
    connections: gio::Settings,
    vmlist_fields: gio::Settings,
    stats: gio::Settings,
    console: gio::Settings,
    new_vm: gio::Settings,
    details: gio::Settings,
    confirm: gio::Settings,
    paths: gio::Settings,
    source: gio::SettingsSchemaSource,
    backend: Option<gio::SettingsBackend>,
    /// Per-VM relocatable `.vm` schemas, keyed by UUID.
    pervm: RefCell<HashMap<String, gio::Settings>>,
}

/// `CONSOLE_SCALE_NEVER/FULLSCREEN/ALWAYS`.
pub const CONSOLE_SCALE_NEVER: i32 = 0;
pub const CONSOLE_SCALE_FULLSCREEN: i32 = 1;
pub const CONSOLE_SCALE_ALWAYS: i32 = 2;

fn schema_source() -> Option<gio::SettingsSchemaSource> {
    if let Some(src) = gio::SettingsSchemaSource::default() {
        if src.lookup(SCHEMA_ID, true).is_some() {
            return Some(src);
        }
    }
    let dir = concat!(env!("OUT_DIR"), "/schemas");
    gio::SettingsSchemaSource::from_directory(
        dir,
        gio::SettingsSchemaSource::default().as_ref(),
        false,
    )
    .ok()
}

impl Config {
    pub fn new() -> Config {
        let source = schema_source().expect("virt-manager GSettings schema not found");
        let backend = std::env::var_os("VIRT_MANAGER_TEST_MEMORY_SETTINGS")
            .map(|_| gio::functions::memory_settings_backend_new());
        let open = |id: &str| {
            let schema = source
                .lookup(id, true)
                .unwrap_or_else(|| panic!("schema {id} missing"));
            gio::Settings::new_full(&schema, backend.as_ref(), None::<&str>)
        };
        Config {
            root: open(SCHEMA_ID),
            connections: open("org.virt-manager.virt-manager.connections"),
            vmlist_fields: open("org.virt-manager.virt-manager.vmlist-fields"),
            stats: open("org.virt-manager.virt-manager.stats"),
            console: open("org.virt-manager.virt-manager.console"),
            new_vm: open("org.virt-manager.virt-manager.new-vm"),
            details: open("org.virt-manager.virt-manager.details"),
            confirm: open("org.virt-manager.virt-manager.confirm"),
            paths: open("org.virt-manager.virt-manager.paths"),
            source: source.clone(),
            backend: backend.clone(),
            pervm: RefCell::default(),
        }
    }

    // Manager window size
    pub fn get_manager_window_size(&self) -> (i32, i32) {
        (
            self.root.int("manager-window-width"),
            self.root.int("manager-window-height"),
        )
    }

    pub fn set_manager_window_size(&self, w: i32, h: i32) {
        let _ = self.root.set_int("manager-window-width", w);
        let _ = self.root.set_int("manager-window-height", h);
    }

    /// `make_vm_settings`: the `.vm` schema relocated to
    /// `/org/virt-manager/virt-manager/vms/<uuid without dashes>/`,
    /// the same path upstream uses.
    pub fn vm_settings(&self, uuid: &str) -> gio::Settings {
        self.pervm
            .borrow_mut()
            .entry(uuid.to_string())
            .or_insert_with(|| {
                let schema = self
                    .source
                    .lookup(&format!("{SCHEMA_ID}.vm"), true)
                    .expect("vm schema missing");
                let path = format!(
                    "/org/virt-manager/virt-manager/vms/{}/",
                    uuid.replace('-', "")
                );
                gio::Settings::new_full(&schema, self.backend.as_ref(), Some(&path))
            })
            .clone()
    }

    // Show VM details toolbar
    pub fn get_details_show_toolbar(&self) -> bool {
        self.details.boolean("show-toolbar")
    }
    pub fn set_details_show_toolbar(&self, val: bool) {
        let _ = self.details.set_boolean("show-toolbar", val);
    }

    // System tray visibility
    pub fn get_view_system_tray(&self) -> bool {
        self.root.boolean("system-tray")
    }
    pub fn set_view_system_tray(&self, val: bool) {
        let _ = self.root.set_boolean("system-tray", val);
    }

    // XML editor enabled
    pub fn get_xmleditor_enabled(&self) -> bool {
        self.root.boolean("xmleditor-enabled")
    }
    pub fn set_xmleditor_enabled(&self, val: bool) {
        let _ = self.root.set_boolean("xmleditor-enabled", val);
    }

    // Libguestfs VM inspection
    pub fn get_libguestfs_inspect_vms(&self) -> bool {
        self.root.boolean("enable-libguestfs-vm-inspection")
    }
    pub fn set_libguestfs_inspect_vms(&self, val: bool) {
        let _ = self
            .root
            .set_boolean("enable-libguestfs-vm-inspection", val);
    }

    // Saved connections
    pub fn get_conn_uris(&self) -> Vec<String> {
        strv(&self.connections, "uris")
    }

    pub fn add_conn_uri(&self, uri: &str) {
        let mut uris = self.get_conn_uris();
        if !uris.iter().any(|u| u == uri) {
            uris = insert_like_upstream(uris, uri);
            set_strv(&self.connections, "uris", &uris);
        }
    }

    pub fn remove_conn_uri(&self, uri: &str) {
        let mut uris = self.get_conn_uris();
        if let Some(idx) = uris.iter().position(|u| u == uri) {
            uris.remove(idx);
            set_strv(&self.connections, "uris", &uris);
        }
        if self.get_conn_autoconnect(uri) {
            self.set_conn_autoconnect(uri, false);
        }
    }

    pub fn get_conn_autoconnect(&self, uri: &str) -> bool {
        strv(&self.connections, "autoconnect")
            .iter()
            .any(|u| u == uri)
    }

    pub fn set_conn_autoconnect(&self, uri: &str, val: bool) {
        let mut uris = strv(&self.connections, "autoconnect");
        let present = uris.iter().any(|u| u == uri);
        if !val && present {
            uris.retain(|u| u != uri);
        } else if val && !present {
            uris.push(uri.to_string());
        }
        set_strv(&self.connections, "autoconnect", &uris);
    }

    // Stats
    pub fn get_stats_update_interval(&self) -> u32 {
        self.stats.int("update-interval").max(1) as u32
    }

    pub fn set_stats_update_interval(&self, val: i32) {
        let _ = self.stats.set_int("update-interval", val);
    }

    pub fn on_stats_update_interval_changed(&self, cb: impl Fn() + 'static) {
        self.stats
            .connect_changed(Some("update-interval"), move |_, _| cb());
    }

    // VM list stats columns (View > Graph)
    pub fn is_vmlist_visible(&self, col: StatsColumn) -> bool {
        self.vmlist_fields.boolean(col.vmlist_key())
    }

    pub fn set_vmlist_visible(&self, col: StatsColumn, val: bool) {
        let _ = self.vmlist_fields.set_boolean(col.vmlist_key(), val);
    }

    pub fn on_vmlist_visible_changed(&self, cb: impl Fn() + 'static) {
        self.vmlist_fields.connect_changed(None, move |_, _| cb());
    }

    /// `get_stats_enable_{cpu,memory,disk,net}_poll`; both CPU columns
    /// share the CPU poll.
    pub fn get_stats_enable_poll(&self, col: StatsColumn) -> bool {
        self.stats.boolean(col.poll_key())
    }

    pub fn set_stats_enable_poll(&self, col: StatsColumn, val: bool) {
        let _ = self.stats.set_boolean(col.poll_key(), val);
    }

    pub fn on_stats_enable_poll_changed(&self, cb: impl Fn() + 'static) {
        self.stats.connect_changed(None, move |_, key| {
            if key.starts_with("enable-") {
                cb()
            }
        });
    }

    pub fn get_poll_flags(&self) -> PollFlags {
        PollFlags {
            cpu: self.get_stats_enable_poll(StatsColumn::GuestCpu),
            mem: self.get_stats_enable_poll(StatsColumn::Memory),
            disk: self.get_stats_enable_poll(StatsColumn::Disk),
            net: self.get_stats_enable_poll(StatsColumn::Network),
        }
    }

    // Graphical console
    pub fn get_console_scaling(&self) -> i32 {
        self.console.int("scaling")
    }
    pub fn set_console_scaling(&self, val: i32) {
        let _ = self.console.set_int("scaling", val);
    }

    /// The raw key: -1 means "system default"
    /// ([`DEFAULT_CONSOLE_RESIZEGUEST`]).
    pub fn get_console_resizeguest_raw(&self) -> i32 {
        self.console.int("resize-guest")
    }
    /// `get_console_resizeguest`: -1 resolved to the default.
    pub fn get_console_resizeguest(&self) -> i32 {
        match self.get_console_resizeguest_raw() {
            -1 => DEFAULT_CONSOLE_RESIZEGUEST,
            v => v,
        }
    }
    pub fn set_console_resizeguest(&self, val: i32) {
        let _ = self.console.set_int("resize-guest", val);
    }

    pub fn get_auto_usbredir(&self) -> bool {
        self.console.boolean("auto-redirect")
    }
    pub fn set_auto_usbredir(&self, val: bool) {
        let _ = self.console.set_boolean("auto-redirect", val);
    }

    pub fn get_console_autoconnect(&self) -> bool {
        self.console.boolean("autoconnect")
    }
    pub fn set_console_autoconnect(&self, val: bool) {
        let _ = self.console.set_boolean("autoconnect", val);
    }

    /// Comma separated GDK keyvals; Left Control + Left Alt when unset.
    pub fn get_keys_combination(&self) -> String {
        let ret = self.console.string("grab-keys");
        if ret.is_empty() {
            return "65507,65513".to_string();
        }
        ret.to_string()
    }
    pub fn set_keys_combination(&self, keyvals: &[u32]) {
        let val: Vec<String> = keyvals.iter().map(u32::to_string).collect();
        let _ = self.console.set_string("grab-keys", &val.join(","));
    }
    pub fn on_keys_combination_changed(
        &self,
        cb: impl Fn() + 'static,
    ) -> (gio::Settings, glib::SignalHandlerId) {
        let id = self
            .console
            .connect_changed(Some("grab-keys"), move |_, _| cb());
        (self.console.clone(), id)
    }

    // Default directories
    /// `get_default_directory`: `/paths/<key>-default`.
    pub fn get_default_directory(&self, key: &str) -> String {
        self.paths.string(&format!("{key}-default")).to_string()
    }
    pub fn set_default_directory(&self, key: &str, folder: &str) {
        if folder.is_empty() || folder.starts_with("/dev") {
            return;
        }
        let _ = self.paths.set_string(&format!("{key}-default"), folder);
    }

    // New VM defaults
    /// `get_graphics_type(raw=True)`.
    pub fn get_graphics_type_raw(&self) -> String {
        let ret = self.new_vm.string("graphics-type").to_string();
        if ["system", "vnc", "spice"].contains(&ret.as_str()) {
            ret
        } else {
            "system".to_string()
        }
    }
    pub fn set_graphics_type(&self, val: &str) {
        let _ = self.new_vm.set_string("graphics-type", &val.to_lowercase());
    }

    /// `get_default_storage_format(raw=True)`.
    pub fn get_default_storage_format_raw(&self) -> String {
        let ret = self.new_vm.string("storage-format").to_string();
        if ["default", "raw", "qcow2"].contains(&ret.as_str()) {
            ret
        } else {
            "default".to_string()
        }
    }
    pub fn set_storage_format(&self, val: &str) {
        let _ = self
            .new_vm
            .set_string("storage-format", &val.to_lowercase());
    }

    pub fn get_default_cpu_setting(&self) -> String {
        let ret = self.new_vm.string("cpu-default").to_string();
        if CPU_SPECIAL_MODES.contains(&ret.as_str()) {
            ret
        } else {
            "default".to_string()
        }
    }
    pub fn set_default_cpu_setting(&self, val: &str) {
        let _ = self.new_vm.set_string("cpu-default", &val.to_lowercase());
    }

    pub fn get_default_firmware_setting(&self) -> String {
        let ret = self.new_vm.string("firmware").to_string();
        if ["default", "uefi"].contains(&ret.as_str()) {
            ret
        } else {
            "default".to_string()
        }
    }
    pub fn set_firmware_setting(&self, val: &str) {
        let _ = self.new_vm.set_string("firmware", &val.to_lowercase());
    }

    // Confirmation dialogs
    pub fn get_confirm_poweroff(&self) -> bool {
        self.confirm.boolean("poweroff")
    }
    pub fn set_confirm_poweroff(&self, val: bool) {
        let _ = self.confirm.set_boolean("poweroff", val);
    }
    pub fn get_confirm_forcepoweroff(&self) -> bool {
        self.confirm.boolean("forcepoweroff")
    }
    pub fn set_confirm_forcepoweroff(&self, val: bool) {
        let _ = self.confirm.set_boolean("forcepoweroff", val);
    }
    pub fn get_confirm_pause(&self) -> bool {
        self.confirm.boolean("pause")
    }
    pub fn set_confirm_pause(&self, val: bool) {
        let _ = self.confirm.set_boolean("pause", val);
    }
    pub fn get_confirm_removedev(&self) -> bool {
        self.confirm.boolean("removedev")
    }
    pub fn set_confirm_removedev(&self, val: bool) {
        let _ = self.confirm.set_boolean("removedev", val);
    }
    pub fn get_confirm_unapplied(&self) -> bool {
        self.confirm.boolean("unapplied-dev")
    }
    pub fn set_confirm_unapplied(&self, val: bool) {
        let _ = self.confirm.set_boolean("unapplied-dev", val);
    }
    pub fn get_confirm_delstorage(&self) -> bool {
        self.confirm.boolean("delete-storage")
    }
    pub fn set_confirm_delstorage(&self, val: bool) {
        let _ = self.confirm.set_boolean("delete-storage", val);
    }
}

fn strv(settings: &gio::Settings, key: &str) -> Vec<String> {
    settings.strv(key).iter().map(|s| s.to_string()).collect()
}

fn set_strv(settings: &gio::Settings, key: &str, val: &[String]) {
    let refs: Vec<&str> = val.iter().map(String::as_str).collect();
    let _ = settings.set_strv(key, refs);
}

/// Upstream's `add_conn_uri` does `uris.insert(len(uris) - 1, uri)`:
/// a new URI lands *before* the last saved one (or is appended when the
/// list is empty, since Python's `insert(-1, x)` on `[]` appends).
/// Kept as-is so both apps agree on the list's order.
fn insert_like_upstream(mut uris: Vec<String>, uri: &str) -> Vec<String> {
    let idx = uris.len().saturating_sub(1);
    uris.insert(idx, uri.to_string());
    uris
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_matches_python_list_insert_minus_one() {
        assert_eq!(insert_like_upstream(vec![], "a"), vec!["a"]);
        assert_eq!(insert_like_upstream(vec!["a".into()], "b"), vec!["b", "a"]);
        assert_eq!(
            insert_like_upstream(vec!["a".into(), "b".into()], "c"),
            vec!["a", "c", "b"]
        );
    }

    #[test]
    fn conn_uris_round_trip_through_memory_backend() {
        std::env::set_var("VIRT_MANAGER_TEST_MEMORY_SETTINGS", "1");
        let config = Config::new();
        assert!(config.get_conn_uris().is_empty());
        config.add_conn_uri("test:///default");
        config.set_conn_autoconnect("test:///default", true);
        assert_eq!(config.get_conn_uris(), vec!["test:///default"]);
        assert!(config.get_conn_autoconnect("test:///default"));
        config.remove_conn_uri("test:///default");
        assert!(config.get_conn_uris().is_empty());
        assert!(!config.get_conn_autoconnect("test:///default"));
        assert_eq!(config.get_stats_update_interval(), 3);
        assert!(!config.get_confirm_poweroff());
        assert!(config.get_confirm_forcepoweroff());
        assert!(config.is_vmlist_visible(StatsColumn::GuestCpu));
        assert!(!config.is_vmlist_visible(StatsColumn::Disk));
        assert_eq!(
            config.get_poll_flags(),
            PollFlags {
                cpu: true,
                mem: false,
                disk: false,
                net: false
            }
        );
    }
}
