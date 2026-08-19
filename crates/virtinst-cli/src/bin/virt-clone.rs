//! `virt-clone` — thin wrapper over `virtinst_core::cloner`.
//! `--preserve-data` only: the source domain's disk paths are rewritten
//! to caller-given paths whose content is assumed already correct, not
//! copied — see `cloner`'s module doc for why that's a real upstream
//! mode, not a crippled slice, and what else isn't ported yet
//! (`--auto-clone`, NVRAM, MAC/UUID collision checking against the
//! connection's other domains).
//!
//! Upstream requires the source domain be shut off before cloning it
//! (`cloner.py`'s `_init_src`) — this does too, same real safety guard
//! against cloning a disk that's actively being written to.

use clap::Parser;
use virtinst_core::cloner::build_clone_document;
use virtinst_core::connection::{Connection, Domain};

#[derive(Parser)]
#[command(name = "virt-clone")]
struct Args {
    /// Name of the original guest to clone.
    #[arg(short = 'o', long = "original")]
    original: String,

    /// Name for the new guest.
    #[arg(short = 'n', long = "name")]
    name: String,

    /// New file to use as the disk image for the new guest — one per
    /// non-shareable disk on the source, in the order they appear in
    /// its XML. The path's content is not touched (`--preserve-data`
    /// is this binary's only mode); it must already contain what the
    /// new guest should boot from.
    #[arg(short = 'f', long = "file")]
    file: Vec<String>,

    /// Connect to a non-default hypervisor.
    #[arg(short, long, default_value = "qemu:///system")]
    connect: String,
}

fn main() {
    let args = Args::parse();

    if args.file.is_empty() {
        eprintln!(
            "virt-clone: --file is required (one per disk to clone; --auto-clone isn't implemented yet)"
        );
        std::process::exit(1);
    }

    let conn = Connection::open(&args.connect).unwrap_or_else(|e| {
        eprintln!("virt-clone: failed to connect to {}: {e}", args.connect);
        std::process::exit(1);
    });
    let source = Domain::lookup_by_name(&conn, &args.original).unwrap_or_else(|e| {
        eprintln!("virt-clone: failed to find domain '{}': {e}", args.original);
        std::process::exit(1);
    });

    match source.is_active() {
        Ok(true) => {
            eprintln!("virt-clone: Domain to clone must be shutoff.");
            std::process::exit(1);
        }
        Ok(false) => {}
        Err(e) => {
            eprintln!("virt-clone: failed to query '{}': {e}", args.original);
            std::process::exit(1);
        }
    }

    let src_xml = source.inactive_xml().unwrap_or_else(|e| {
        eprintln!("virt-clone: failed to fetch '{}''s XML: {e}", args.original);
        std::process::exit(1);
    });

    let is_qemu = conn.uri().is_qemu();
    let doc = build_clone_document(&src_xml, &args.name, is_qemu, &args.file).unwrap_or_else(|e| {
        eprintln!("virt-clone: {e}");
        std::process::exit(1);
    });
    let new_xml = doc.write_str().unwrap_or_else(|e| {
        eprintln!("virt-clone: failed to serialize the clone's XML: {e}");
        std::process::exit(1);
    });

    match Domain::define_new(&conn, &new_xml) {
        Ok(_) => println!("Clone '{}' created successfully.", args.name),
        Err(e) => {
            eprintln!("virt-clone: {e}");
            std::process::exit(1);
        }
    }
}
