//! `virt-install` — reuses `InstallMethod` (ticket 08) as-is.
//! `--unattended`/`--cloud-init` thread through as extra fields on
//! relevant variants; `--reinstall` is its own CLI-only operation with
//! no wizard equivalent, closer in shape to `virt-xml --edit` than to
//! Create VM (ticket 16).
//!
//! Five `InstallMethod` shapes are wired up so far:
//! - `--import` — upstream's simplest install method: no media fetch, no
//!   OS detection, no `Installer`-class media/kernel wiring, an existing
//!   disk becomes the VM's disk as-is (`InstallMethod::Import`).
//! - `--cdrom --livecd` — `InstallMethod::Iso { live: true, .. }`: boot
//!   straight into CD media with no persistent disk.
//! - `--cdrom --disk <path>` (no `--livecd`) — `InstallMethod::Iso {
//!   live: false, .. }`: install from CD onto a new persistent disk, the
//!   two-phase case (`iso::build_iso_two_phase_documents`) — `createXML`
//!   gets a *different* document than `defineXML`.
//! - `--boot <dev>` — `InstallMethod::Manual { boot_dev: Some(dev) }`,
//!   the single-phase case (`options.boot_was_set` forces
//!   `no_install = True` upstream when `--boot` is the only install
//!   method given).
//! - `--pxe` (optionally with `--disk <path>`) — also `InstallMethod::
//!   Manual { boot_dev: Some("network") }`, but always two-phase
//!   (`manual::build_manual_two_phase_documents`) regardless of whether
//!   a disk was given — `requires_postboot_xml_changes()` only checks
//!   `install_bootdev` truthiness, not disk presence; with no disk the
//!   two phases just end up identical (see `manual.rs`'s header for the
//!   exact derivation of both cases).
//!
//! The domain XML for all five is built via direct DOM construction
//! (`virtinst_core::install::{build_import_document, build_iso_document,
//! build_iso_two_phase_documents, build_manual_document,
//! build_manual_two_phase_documents}` — see the amendment on ticket 08
//! for why `Guest::to_xml_string()` can't do this). Every other
//! `InstallMethod` variant, `--unattended`/`--cloud-init`, and
//! `--reinstall` are undeclared rather than stubbed.
//!
//! Matches upstream's actual default sequence (`Installer._create_guest`,
//! `doboot=True`, not `--transient`): two separate libvirt calls,
//! `createXML` (start it running now) then `defineXML` (persist it) —
//! not one call with both effects. For the single-phase methods these
//! two calls get identical XML; for `Iso { live: false }` they don't
//! (see above) — **this was never a missing primitive at the
//! `Domain::create_new`/`define_new` layer**, both always took their own
//! independent `xml: &str`. Two prior commits' doc comments claimed
//! otherwise ("both are single-shot against one XML string") — that was
//! wrong, caught while grounding this one; the actual gap was builder-
//! side (nothing produced a document *pair* sharing a UUID).

use clap::Parser;
use virtinst_core::capabilities::DomainCapabilities;
use virtinst_core::connection::{Connection, Domain};
use virtinst_core::install::{
    build_import_document, build_iso_document, build_iso_two_phase_documents,
    build_manual_document, build_manual_two_phase_documents,
};

#[derive(Parser)]
#[command(name = "virt-install")]
struct Args {
    /// Build a guest around an existing disk image (`InstallMethod::
    /// Import`).
    #[arg(long)]
    import: bool,

    /// Path to install/boot media (`InstallMethod::Iso`). Combined with
    /// `--livecd`: boot straight into the media, no persistent disk.
    /// Combined with `--disk`: install onto that disk, two-phase
    /// (`live: false`). One of the two is required alongside `--cdrom`.
    #[arg(long)]
    cdrom: Option<String>,

    /// Boot straight into `--cdrom`'s media with no persistent disk
    /// attached at all (`InstallMethod::Iso { live: true, .. }`).
    #[arg(long)]
    livecd: bool,

    /// Force a specific boot device rather than fetching/attaching
    /// install media (`InstallMethod::Manual { boot_dev: Some(dev) }`).
    /// Upstream's `--boot` also configures firmware/loader/machine type;
    /// this is narrower — boot-device selection only.
    #[arg(long)]
    boot: Option<String>,

    /// Boot from the network (`InstallMethod::Manual { boot_dev:
    /// Some("network") }`), always two-phase — see this binary's own doc
    /// comment. `--disk` is optional here (a pure network-boot appliance
    /// is a real invocation), unlike plain `--cdrom`.
    #[arg(long)]
    pxe: bool,

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

    /// Path to a disk image. Required with `--import`; required with
    /// `--cdrom` unless `--livecd` is also given; optional with `--boot`
    /// (a disk isn't mandatory there — e.g. a pure `--boot network`
    /// appliance); ignored otherwise. Upstream's `--disk` is a full
    /// `path=...,bus=...` optstring (`ParserDisk`); this is a bare path
    /// only — the equivalent of upstream's positional `--disk
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

    let write = |doc: virtinst_xml::Document| {
        doc.write_str().unwrap_or_else(|e| {
            eprintln!("virt-install: failed to serialize the guest's XML: {e}");
            std::process::exit(1);
        })
    };

    // (initial, final) — identical XML for every single-phase method;
    // genuinely different XML for `Iso { live: false }`. See this
    // binary's own doc comment for why this was never a `Domain::
    // create_new`/`define_new` limitation, only a builder-side gap.
    let (initial_xml, final_xml) = if args.import {
        let disk = args.disk.unwrap_or_else(|| {
            eprintln!("virt-install: --import requires --disk");
            std::process::exit(1);
        });
        let xml = write(build_import_document(
            &args.name,
            memory_kib,
            args.vcpus,
            &disk,
            &domain_type,
            &arch,
        ));
        (xml.clone(), xml)
    } else if let Some(media) = args.cdrom {
        if args.livecd {
            let xml = write(build_iso_document(
                &args.name,
                memory_kib,
                args.vcpus,
                &media,
                &domain_type,
                &arch,
            ));
            (xml.clone(), xml)
        } else {
            let disk = args.disk.unwrap_or_else(|| {
                eprintln!("virt-install: --cdrom without --livecd requires --disk");
                std::process::exit(1);
            });
            let (initial, final_doc) = build_iso_two_phase_documents(
                &args.name,
                memory_kib,
                args.vcpus,
                &media,
                &disk,
                &domain_type,
                &arch,
            );
            (write(initial), write(final_doc))
        }
    } else if args.pxe {
        let (initial, final_doc) = build_manual_two_phase_documents(
            &args.name,
            memory_kib,
            args.vcpus,
            "network",
            args.disk.as_deref(),
            &domain_type,
            &arch,
        );
        (write(initial), write(final_doc))
    } else if let Some(boot_dev) = args.boot {
        let xml = write(build_manual_document(
            &args.name,
            memory_kib,
            args.vcpus,
            &boot_dev,
            args.disk.as_deref(),
            &domain_type,
            &arch,
        ));
        (xml.clone(), xml)
    } else {
        eprintln!(
            "virt-install: specify --import, --cdrom, --pxe, or --boot (only methods \
             implemented so far)"
        );
        std::process::exit(1);
    };

    println!("\nStarting install...");

    if let Err(e) = Domain::create_new(&conn, &initial_xml) {
        eprintln!("virt-install: failed to start the domain: {e}");
        std::process::exit(1);
    }
    if let Err(e) = Domain::define_new(&conn, &final_xml) {
        eprintln!("virt-install: failed to persist the domain: {e}");
        std::process::exit(1);
    }

    println!("Domain creation completed.");
    println!(
        "You can restart your domain by running:\n  virsh --connect {} start {}",
        args.connect, args.name
    );
}
