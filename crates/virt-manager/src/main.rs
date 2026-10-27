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
mod config;
mod connection;
mod connevents;
mod connmanager;
mod console;
mod createconn;
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
mod hoststorage;
mod keymap;
mod libvirtenummap;
mod libvirtglib;
mod manager;
mod manager_row;
mod mediacombo;
mod netlist;
mod oslist;
mod preferences;
mod serialcon;
mod spice;
mod spicedisplay;
mod spicegtksession;
mod sshtunnels;
mod statsmanager;
mod storagebrowse;
mod tpmdetails;
mod usbdevicewidget;
mod viewers;
mod vmmenu;
mod vmwindow;
mod vncdisplay;
mod vsockdetails;
mod xmleditor;

use clap::Parser;

/// Upstream's argparse options that the ported windows can honor. The
/// `--show-*` window launchers arrive with the windows they open.
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
}

fn main() -> gtk::glib::ExitCode {
    let cli = Cli::parse();
    // Forking into the background isn't ported: the process always stays
    // in the foreground, which is what `--no-fork`/`--debug` ask for.
    let _ = (cli.debug, cli.no_fork, cli.fork);
    // LibvirtGLib.event_register()
    libvirtglib::event_register();
    engine::run(engine::StartupOptions { uri: cli.uri })
}
