//! `virt-install` — reuses `InstallMethod` (ticket 08) as-is.
//! `--unattended`/`--cloud-init` thread through as extra fields on
//! relevant variants; `--reinstall` is its own CLI-only operation with
//! no wizard equivalent, closer in shape to `virt-xml --edit` than to
//! Create VM (ticket 16).
//!
//! This slice is `--import` only — upstream's simplest install method:
//! no media fetch, no OS detection, no `Installer`-class media/kernel
//! wiring, an existing disk becomes the VM's disk as-is
//! (`InstallMethod::Import`). The domain XML is built via
//! `virtinst_core::install::build_import_document` (direct DOM
//! construction — see the amendment on ticket 08 for why `Guest::
//! to_xml_string()` can't do this). Every other `InstallMethod`
//! variant, `--unattended`/`--cloud-init`, and `--reinstall` are
//! undeclared rather than stubbed.
//!
//! Matches upstream's actual default sequence
//! (`Installer._create_guest`, `doboot=True`, not `--transient`): two
//! separate libvirt calls against the *same* XML — `createXML` (start
//! it running now) then `defineXML` (persist it) — not one call with
//! both effects.

use clap::Parser;
use virtinst_core::capabilities::DomainCapabilities;
use virtinst_core::connection::{Connection, Domain};
use virtinst_core::install::build_import_document;

#[derive(Parser)]
#[command(name = "virt-install")]
struct Args {
    /// Build a guest around an existing disk image — the only install
    /// method implemented so far.
    #[arg(long)]
    import: bool,

    /// Name for the new guest.
    #[arg(short, long)]
    name: String,

    /// Memory to allocate, in MiB.
    #[arg(long)]
    memory: u64,

    /// Number of virtual CPUs. Defaulting to a flat `1` when absent
    /// isn't upstream's real behavior — `--vcpus` is optional there
    /// too, but falls back to `osinfo`'s recommended-resources lookup,
    /// not a flat `1` (see `build_import_document`'s doc comment:
    /// `OsInfo` isn't built yet).
    #[arg(long, default_value_t = 1)]
    vcpus: u32,

    /// Path to the existing disk image to import. Upstream's `--disk`
    /// is a full `path=...,bus=...` optstring (`ParserDisk`); this is
    /// a bare path only — the equivalent of upstream's positional
    /// `--disk /path/to/img` form (`remove_first="path"`), not the
    /// general option-string syntax.
    #[arg(long)]
    disk: String,

    /// Connect to a non-default hypervisor.
    #[arg(short, long, default_value = "qemu:///system")]
    connect: String,
}

fn main() {
    let args = Args::parse();

    if !args.import {
        eprintln!("virt-install: only --import is implemented so far");
        std::process::exit(1);
    }

    let conn = Connection::open(&args.connect).unwrap_or_else(|e| {
        eprintln!("virt-install: failed to connect to {}: {e}", args.connect);
        std::process::exit(1);
    });

    // Upstream's `guest.set_capabilities_defaults()` — the connection's
    // own default domain type/arch, not guessed (`test:///`'s is
    // `i686`, a real `qemu:///system`'s is `x86_64`).
    let caps = DomainCapabilities::fetch(&conn, None, None, None, None).unwrap_or_else(|e| {
        eprintln!("virt-install: failed to fetch domain capabilities: {e}");
        std::process::exit(1);
    });
    let domain_type = caps.domain.unwrap_or_else(|| {
        eprintln!("virt-install: connection didn't report a default domain type");
        std::process::exit(1);
    });
    let arch = caps.arch.unwrap_or_else(|| {
        eprintln!("virt-install: connection didn't report a default architecture");
        std::process::exit(1);
    });

    let memory_kib = args.memory.checked_mul(1024).unwrap_or_else(|| {
        eprintln!("virt-install: --memory {} MiB overflows", args.memory);
        std::process::exit(1);
    });
    let doc = build_import_document(
        &args.name,
        memory_kib,
        args.vcpus,
        &args.disk,
        &domain_type,
        &arch,
    );
    let xml = doc.write_str().unwrap_or_else(|e| {
        eprintln!("virt-install: failed to serialize the guest's XML: {e}");
        std::process::exit(1);
    });

    println!("\nStarting install...");

    if let Err(e) = Domain::create_new(&conn, &xml) {
        eprintln!("virt-install: failed to start the domain: {e}");
        std::process::exit(1);
    }
    if let Err(e) = Domain::define_new(&conn, &xml) {
        eprintln!("virt-install: failed to persist the domain: {e}");
        std::process::exit(1);
    }

    println!("Domain creation completed.");
    println!(
        "You can restart your domain by running:\n  virsh --connect {} start {}",
        args.connect, args.name
    );
}
