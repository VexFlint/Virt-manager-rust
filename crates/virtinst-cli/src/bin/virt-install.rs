//! `virt-install` — reuses `InstallMethod` (ticket 08) as-is.
//! `--unattended`/`--cloud-init` thread through as extra fields on
//! relevant variants; `--reinstall` is its own CLI-only operation with
//! no wizard equivalent, closer in shape to `virt-xml --edit` than to
//! Create VM (ticket 16).
//!
//! Three `InstallMethod` variants are wired up so far:
//! - `--import` — upstream's simplest install method: no media fetch, no
//!   OS detection, no `Installer`-class media/kernel wiring, an existing
//!   disk becomes the VM's disk as-is (`InstallMethod::Import`).
//! - `--cdrom --livecd` — `InstallMethod::Iso { live: true, .. }`: boot
//!   straight into CD media with no persistent disk. Plain `--cdrom`
//!   (install onto a new persistent disk, `live: false`) isn't
//!   implemented yet — it needs the two-phase `createXML`(boot=cdrom)/
//!   `defineXML`(boot=hd) dance documented on `iso::build_iso_document`,
//!   which nothing in this crate does yet.
//! - `--boot <dev>` — `InstallMethod::Manual { boot_dev: Some(dev) }`,
//!   the single-phase case (`options.boot_was_set` forces
//!   `no_install = True` upstream when `--boot` is the only install
//!   method given). `--pxe` isn't implemented — it needs the same
//!   two-phase dance as `--cdrom` without `--livecd`, see
//!   `manual::build_manual_document`'s doc comment.
//!
//! The domain XML for all three is built via direct DOM construction
//! (`virtinst_core::install::{build_import_document, build_iso_document,
//! build_manual_document}` — see the amendment on ticket 08 for why
//! `Guest::to_xml_string()` can't do this). Every other `InstallMethod`
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
use virtinst_core::install::{build_import_document, build_iso_document, build_manual_document};

#[derive(Parser)]
#[command(name = "virt-install")]
struct Args {
    /// Build a guest around an existing disk image (`InstallMethod::
    /// Import`).
    #[arg(long)]
    import: bool,

    /// Path to install/boot media (`InstallMethod::Iso`). Only
    /// meaningful combined with `--livecd` so far — plain `--cdrom`
    /// (install onto a new persistent disk) needs the two-phase
    /// boot-then-redefine dance `iso::build_iso_document` doesn't do
    /// yet.
    #[arg(long)]
    cdrom: Option<String>,

    /// Boot straight into `--cdrom`'s media with no persistent disk
    /// attached at all (`InstallMethod::Iso { live: true, .. }`).
    /// Required alongside `--cdrom` for now, see its doc comment.
    #[arg(long)]
    livecd: bool,

    /// Force a specific boot device rather than fetching/attaching
    /// install media (`InstallMethod::Manual { boot_dev: Some(dev) }`).
    /// Upstream's `--boot` also configures firmware/loader/machine type;
    /// this is narrower — boot-device selection only.
    #[arg(long)]
    boot: Option<String>,

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

    /// Path to a disk image. Required with `--import`; optional with
    /// `--boot` (a disk isn't mandatory there — e.g. a pure `--boot
    /// network` appliance); ignored otherwise. Upstream's `--disk` is a
    /// full `path=...,bus=...` optstring (`ParserDisk`); this is a bare
    /// path only — the equivalent of upstream's positional `--disk
    /// /path/to/img` form (`remove_first="path"`), not the general
    /// option-string syntax.
    #[arg(long)]
    disk: Option<String>,

    /// Connect to a non-default hypervisor.
    #[arg(short, long, default_value = "qemu:///system")]
    connect: String,
}

fn main() {
    let args = Args::parse();

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

    let doc = if args.import {
        let disk = args.disk.unwrap_or_else(|| {
            eprintln!("virt-install: --import requires --disk");
            std::process::exit(1);
        });
        build_import_document(
            &args.name,
            memory_kib,
            args.vcpus,
            &disk,
            &domain_type,
            &arch,
        )
    } else if let Some(media) = args.cdrom {
        if !args.livecd {
            eprintln!(
                "virt-install: --cdrom requires --livecd for now (installing onto a new \
                 persistent disk isn't implemented yet)"
            );
            std::process::exit(1);
        }
        build_iso_document(
            &args.name,
            memory_kib,
            args.vcpus,
            &media,
            &domain_type,
            &arch,
        )
    } else if let Some(boot_dev) = args.boot {
        build_manual_document(
            &args.name,
            memory_kib,
            args.vcpus,
            &boot_dev,
            args.disk.as_deref(),
            &domain_type,
            &arch,
        )
    } else {
        eprintln!(
            "virt-install: specify --import, --cdrom --livecd, or --boot (only methods \
             implemented so far)"
        );
        std::process::exit(1);
    };

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
