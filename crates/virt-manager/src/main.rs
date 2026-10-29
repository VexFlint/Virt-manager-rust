//! GTK4 desktop GUI entry point — upstream's `virt-manager` script
//! (`virtManager/virtmanager.py`).
//!
//! Screen tickets resolved so far (`.scratch/rust-conversion/issues/`):
//! 08 (Create VM wizard), 10 (VM details window), 11 (Snapshot
//! management), 12 (Add Hardware wizard), 13 (VM Manager main window),
//! 14 (console-viewer specifics). Remaining screens: see the map's
//! "Not yet specified" section — ticket 07's inventory has the full list.
//!
//! Ported so far: the manager window (ticket 13) and what it needs —
//! engine, connection manager, connection/domain objects, the error
//! dialogs, the VM action menus, Add Connection, Preferences, the About
//! dialog, and the VM window's first slice (ticket 10).

mod about;
mod addhardware;
mod addstorage;
mod asyncjob;
mod clone;
mod config;
mod connectauth;
mod connection;
mod connevents;
mod connmanager;
mod console;
mod createconn;
mod createnet;
mod createpool;
mod createvm;
mod createvol;
mod delete;
mod details;
mod devpages;
mod display;
mod domain;
mod engine;
mod error;
mod fsdetails;
mod gfxdetails;
mod graphwidgets;
mod gvnc;
mod host;
mod hostnets;
mod hoststorage;
mod keymap;
mod keyring;
mod libvirtenummap;
mod libvirtglib;
mod manager;
mod manager_row;
mod mediacombo;
mod migrate;
mod netlist;
mod oslist;
mod preferences;
mod serialcon;
mod snapshots;
mod spice;
mod spicedisplay;
mod spicegtksession;
mod sshtunnels;
mod statsmanager;
mod storagebrowse;
mod systray;
mod tpmdetails;
mod usbdevicewidget;
mod viewers;
mod vmmenu;
mod vmwindow;
mod vncdisplay;
mod vsockdetails;
mod xmleditor;

use clap::Parser;

/// Upstream's argparse options (`parse_commandline`), minus the hidden
/// `--trace-libvirt` and `--test-options`.
#[derive(Parser, Debug)]
#[command(name = "virt-manager", version, about = "Virtual Machine Manager")]
struct Cli {
    /// Connect to hypervisor at URI
    #[arg(short = 'c', long = "connect", value_name = "URI")]
    uri: Option<String>,

    /// Print debug output to stdout (implies --no-fork)
    #[arg(long)]
    debug: bool,

    /// Don't fork into background on startup
    #[arg(long)]
    no_fork: bool,

    /// Force fork into background on startup (this is the default)
    #[arg(long, conflicts_with = "no_fork")]
    fork: bool,

    /// Show 'New VM' wizard
    #[arg(long)]
    show_domain_creator: bool,

    /// Show domain details window
    #[arg(long, value_name = "NAME|ID|UUID")]
    show_domain_editor: Option<String>,

    /// Show domain performance window
    #[arg(long, value_name = "NAME|ID|UUID")]
    show_domain_performance: Option<String>,

    /// Show domain graphical console window
    #[arg(long, value_name = "NAME|ID|UUID")]
    show_domain_console: Option<String>,

    /// Show domain delete window
    #[arg(long, value_name = "NAME|ID|UUID")]
    show_domain_delete: Option<String>,

    /// Show connection details window
    #[arg(long)]
    show_host_summary: bool,

    /// Launch virt-manager only in system tray
    #[arg(long)]
    show_systray: bool,
}

impl Cli {
    /// `main()`'s if/elif chain: the first `--show-*` option in
    /// upstream's order picks the window, and a `--show-domain-*` one
    /// the VM. An empty `NAME|ID|UUID` is false to Python, so skipped.
    fn show_window(&self) -> (Option<engine::ShowWindow>, Option<String>) {
        use engine::ShowWindow as W;
        let domain = |opt: &Option<String>| opt.clone().filter(|d| !d.is_empty());
        if self.show_domain_creator {
            (Some(W::DomainCreator), None)
        } else if self.show_host_summary {
            (Some(W::HostSummary), None)
        } else if let Some(d) = domain(&self.show_domain_editor) {
            (Some(W::DomainEditor), Some(d))
        } else if let Some(d) = domain(&self.show_domain_performance) {
            (Some(W::DomainPerformance), Some(d))
        } else if let Some(d) = domain(&self.show_domain_console) {
            (Some(W::DomainConsole), Some(d))
        } else if let Some(d) = domain(&self.show_domain_delete) {
            (Some(W::DomainDelete), Some(d))
        } else if self.show_systray {
            (Some(W::SystemTray), None)
        } else {
            (None, None)
        }
    }

    /// `engine.start()`'s arguments, or upstream's error for a window
    /// that needs `--connect`.
    fn startup_options(self) -> Result<engine::StartupOptions, &'static str> {
        let (show_window, domain) = self.show_window();
        let needs_uri = show_window.is_some_and(|w| w != engine::ShowWindow::SystemTray);
        if needs_uri && self.uri.is_none() {
            return Err("can't use --show-* options without --connect (except --show-systray)");
        }
        Ok(engine::StartupOptions {
            uri: self.uri,
            show_window,
            domain,
            skip_autostart: needs_uri,
        })
    }
}

fn main() -> gtk::glib::ExitCode {
    let cli = Cli::parse();
    // Forking into the background isn't ported: the process always stays
    // in the foreground, which is what `--no-fork`/`--debug` ask for.
    let _ = (cli.debug, cli.no_fork, cli.fork);
    // Force SSH to use askpass if a password is required,
    // rather than possibly prompting on a terminal the user isn't looking at.
    if std::env::var_os("SSH_ASKPASS_REQUIRE").is_none() {
        std::env::set_var("SSH_ASKPASS_REQUIRE", "force");
    }
    let opts = match cli.startup_options() {
        Ok(opts) => opts,
        Err(msg) => {
            eprintln!("{msg}");
            return gtk::glib::ExitCode::FAILURE;
        }
    };
    // LibvirtGLib.event_register()
    libvirtglib::event_register();
    engine::run(opts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use engine::ShowWindow as W;

    fn opts(args: &[&str]) -> Result<engine::StartupOptions, &'static str> {
        let argv = std::iter::once("virt-manager").chain(args.iter().copied());
        Cli::try_parse_from(argv).unwrap().startup_options()
    }

    fn window(args: &[&str]) -> (Option<W>, Option<String>) {
        let o = opts(args).unwrap();
        (o.show_window, o.domain)
    }

    const C: &str = "test:///default";

    #[test]
    fn show_flags_map_to_windows() {
        assert_eq!(window(&["-c", C]), (None, None));
        assert_eq!(
            window(&["-c", C, "--show-domain-creator"]),
            (Some(W::DomainCreator), None)
        );
        for (flag, w) in [
            ("--show-domain-editor", W::DomainEditor),
            ("--show-domain-performance", W::DomainPerformance),
            ("--show-domain-console", W::DomainConsole),
            ("--show-domain-delete", W::DomainDelete),
        ] {
            assert_eq!(
                window(&["-c", C, flag, "test"]),
                (Some(w), Some("test".into()))
            );
        }
        assert_eq!(
            window(&["-c", C, "--show-host-summary"]),
            (Some(W::HostSummary), None)
        );
        assert_eq!(window(&["--show-systray"]), (Some(W::SystemTray), None));
    }

    #[test]
    fn show_flags_follow_upstream_elif_order() {
        let both = |a: &[&str]| window(&[&["-c", C][..], a].concat()).0;
        assert_eq!(
            both(&["--show-domain-editor", "x", "--show-domain-creator"]),
            Some(W::DomainCreator)
        );
        assert_eq!(
            both(&["--show-domain-editor", "x", "--show-host-summary"]),
            Some(W::HostSummary)
        );
        assert_eq!(
            both(&["--show-domain-delete", "x", "--show-domain-editor", "y"]),
            Some(W::DomainEditor)
        );
        assert_eq!(
            both(&[
                "--show-domain-console",
                "x",
                "--show-domain-performance",
                "y"
            ]),
            Some(W::DomainPerformance)
        );
        assert_eq!(
            both(&["--show-systray", "--show-domain-console", "x"]),
            Some(W::DomainConsole)
        );
        // `--show-domain-editor ""` is false in Python
        assert_eq!(
            window(&[
                "-c",
                C,
                "--show-domain-editor",
                "",
                "--show-domain-delete",
                "x"
            ]),
            (Some(W::DomainDelete), Some("x".into()))
        );
    }

    #[test]
    fn show_flags_need_connect_except_systray() {
        let err = "can't use --show-* options without --connect (except --show-systray)";
        assert_eq!(opts(&["--show-domain-creator"]), Err(err));
        assert_eq!(opts(&["--show-domain-editor", "test"]), Err(err));
        assert_eq!(opts(&["--show-domain-performance", "1"]), Err(err));
        assert_eq!(opts(&["--show-domain-console", "test"]), Err(err));
        assert_eq!(opts(&["--show-domain-delete", "test"]), Err(err));
        assert_eq!(opts(&["--show-host-summary"]), Err(err));
        assert!(opts(&["--show-systray"]).is_ok());
        assert!(opts(&[]).is_ok());
    }

    #[test]
    fn skip_autostart_for_windows_but_not_the_tray() {
        assert!(!opts(&["-c", C]).unwrap().skip_autostart);
        assert!(!opts(&["--show-systray"]).unwrap().skip_autostart);
        assert!(!opts(&["-c", C, "--show-systray"]).unwrap().skip_autostart);
        assert!(
            opts(&["-c", C, "--show-host-summary"])
                .unwrap()
                .skip_autostart
        );
        assert!(
            opts(&["-c", C, "--show-domain-creator"])
                .unwrap()
                .skip_autostart
        );
        assert!(
            opts(&["-c", C, "--show-domain-console", "test"])
                .unwrap()
                .skip_autostart
        );
    }

    #[test]
    fn show_flags_help_and_metavars_match_upstream() {
        let cmd = Cli::command();
        let arg = |id: &str| cmd.get_arguments().find(|a| a.get_id() == id).unwrap();
        for (id, help, metavar) in [
            ("show_domain_creator", "Show 'New VM' wizard", None),
            (
                "show_domain_editor",
                "Show domain details window",
                Some("NAME|ID|UUID"),
            ),
            (
                "show_domain_performance",
                "Show domain performance window",
                Some("NAME|ID|UUID"),
            ),
            (
                "show_domain_console",
                "Show domain graphical console window",
                Some("NAME|ID|UUID"),
            ),
            (
                "show_domain_delete",
                "Show domain delete window",
                Some("NAME|ID|UUID"),
            ),
            ("show_host_summary", "Show connection details window", None),
            (
                "show_systray",
                "Launch virt-manager only in system tray",
                None,
            ),
        ] {
            let a = arg(id);
            assert_eq!(a.get_help().unwrap().to_string(), help);
            assert_eq!(a.get_long(), Some(id.replace('_', "-").as_str()));
            match metavar {
                Some(metavar) => {
                    let names = a.get_value_names().unwrap();
                    assert_eq!(
                        names.iter().map(|n| n.as_str()).collect::<Vec<_>>(),
                        [metavar]
                    );
                }
                // store_true
                None => assert!(!a.get_action().takes_values()),
            }
        }
    }
}
