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

use gtk::gio;
use gtk::gio::prelude::*;

use crate::statsmanager::PollFlags;

const SCHEMA_ID: &str = "org.virt-manager.virt-manager";

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
    confirm: gio::Settings,
}

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
            confirm: open("org.virt-manager.virt-manager.confirm"),
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
