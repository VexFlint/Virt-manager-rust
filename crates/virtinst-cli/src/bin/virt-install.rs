//! `virt-install` — reuses `InstallMethod` (ticket 08) as-is.
//! `--unattended`/`--cloud-init` thread through as extra fields on
//! relevant variants; `--reinstall` is its own CLI-only operation with
//! no wizard equivalent, closer in shape to `virt-xml --edit` than to
//! Create VM (ticket 16).
//!
//! Six `InstallMethod` shapes are wired up so far:
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
//! - `--container --filesystem <dir>` — `InstallMethod::ContainerOs {
//!   source: dir, bootstrap: None }`. `ContainerApp` (`--boot init=...`)
//!   isn't built — needs optstring parsing `--boot` doesn't do, see
//!   `container.rs`'s header. `bootstrap: Some(_)` (fetch/populate the
//!   directory from a registry first) isn't built either — needs real
//!   network fetching, same shape of gap as `Url`.
//!
//! The domain XML for all six is built via direct DOM construction
//! (`virtinst_core::install::{build_import_document, build_iso_document,
//! build_iso_two_phase_documents, build_manual_document,
//! build_manual_two_phase_documents, build_container_os_document}` — see
//! the amendment on ticket 08 for why `Guest::to_xml_string()` can't do
//! this). Every other `InstallMethod` variant, `--unattended`/
//! `--cloud-init`, and `--reinstall` are undeclared rather than stubbed.
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
//!
//! `hypervisor_type`/`arch`/`os_type`/`machine`/`emulator` all come from
//! one real `Capabilities::fetch` + `guest_lookup` call (ticket 20 +
//! ticket 21 slice (a)) — `_set_default_machine`'s fallback-only branch
//! and `set_capabilities_defaults`'s plain `emulator` thread, not the
//! full `set_defaults()` pipeline. `--container` deliberately excepted
//! from `os_type`/`machine`/`emulator` (see `build_container_os_
//! document`'s doc comment). `graphics`/`clock`'s base trio/hostdev/
//! CPU mode defaults are all built (ticket 21 slices (c)–(f)); `--os-
//! variant` (ticket 04's real `libosinfo` bindings) now feeds `clock`'s
//! `offset` attribute too. `loader`/UEFI selection, the q35-preferred
//! machine branch, PM defaults, and the 9-file per-driver device-
//! default matrix remain unbuilt — scoped in `.scratch/rust-conversion/
//! issues/21-set-defaults-scope.md`; `loader`/UEFI selection and the
//! per-driver disk-bus/video-model defaults need more `osinfo::OsInfo`
//! query methods than `clock_offset()` alone (device-list and
//! firmware-list queries, not built yet — see ticket 04's own doc
//! comment for the minimal slice that exists today).

use clap::Parser;
use virtinst_core::capabilities::{Capabilities, DomainCapabilities};
use virtinst_core::connection::{Connection, Domain};
use virtinst_core::install::{
    build_container_os_document, build_import_document, build_iso_document,
    build_iso_two_phase_documents, build_manual_document, build_manual_two_phase_documents,
    default_disk_bus, default_firmware, default_nic_model, prefers_uefi, GuestCapsDefaults,
};
use virtinst_core::osinfo::OsDb;

#[derive(Parser)]
#[command(name = "virt-install", disable_version_flag = true)]
struct Args {
    /// show program's version number and exit
    #[arg(long = "version")]
    _version: bool,

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

    /// Build a container guest. Upstream's `--container` is a general
    /// os-type flag (`--hvm`/`--paravirt`'s sibling), not itself an
    /// install method — combined with `--filesystem` here
    /// (`InstallMethod::ContainerOs`); combined with `--boot init=...`
    /// upstream (`ContainerApp`, not built, see this binary's own doc
    /// comment).
    #[arg(long)]
    container: bool,

    /// Existing directory to use as the container's root filesystem
    /// (`InstallMethod::ContainerOs::source`). Required with
    /// `--container`; ignored otherwise. Upstream's `--filesystem` is a
    /// full `source=...,target=...` optstring (`ParserFilesystem`);
    /// this is a bare path only, same posture as `--disk`.
    #[arg(long)]
    filesystem: Option<String>,

    /// Name for the new guest.
    #[arg(short, long)]
    name: String,

    /// Memory to allocate, in MiB.
    #[arg(long)]
    memory: u64,

    /// Number of virtual CPUs. Defaulting to a flat `1` when absent
    /// isn't upstream's real behavior — `--vcpus` is optional there
    /// too, but falls back to `osinfo`'s recommended-resources lookup
    /// (`get_recommended_ncpus`, a query this crate's `osinfo::OsInfo`
    /// doesn't have yet even though the underlying bindings now exist —
    /// ticket 04's own doc comment scopes what's built so far), not a
    /// flat `1`.
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

    /// The OS being installed, as a libosinfo short-id (`fedora25`) or
    /// full id (`http://fedoraproject.org/fedora/25`) — feeds the
    /// `OsInfo`-gated defaults ticket 21 scoped and left blocked until
    /// ticket 04 built real `libosinfo` bindings: `clock`'s `offset`
    /// attribute, disk/cdrom `bus` selection (`default_disk_bus`), and
    /// NIC `model` selection (`default_nic_model`) now all consult it.
    /// UEFI firmware selection is still unbuilt regardless of this
    /// flag, see `osinfo::OsInfo`'s own doc comment for which query
    /// methods exist. Omitted entirely (not a `"generic"` sentinel
    /// value the way upstream's CLI accepts) means the same "no OS
    /// selected" defaults upstream's own synthetic generic entry
    /// produces — see `push_default_clock`'s doc comment for why
    /// that's equivalent, not a separate code path. Note: the clock-
    /// offset, disk-bus, and NIC-model defaults this flag feeds are all
    /// themselves gated on `is_qemu` (`supports_virtio*`'s own gate,
    /// upstream-faithful — see `domain_skeleton::default_disk_bus`'s
    /// doc comment), so on a non-qemu `--connect` (e.g. `test:///`,
    /// `lxc:///`) this flag has no observable effect on any of them.
    #[arg(long)]
    os_variant: Option<String>,
}

fn main() {
    virtinst_cli::cli::handle_version_flag();
    let args = Args::parse();

    let conn = Connection::open(&args.connect).unwrap_or_else(|e| {
        eprintln!("virt-install: failed to connect to {}: {e}", args.connect);
        std::process::exit(1);
    });

    // Upstream's `guest.set_capabilities_defaults()`, via `conn.caps.
    // guest_lookup()` — ticket 20's real hypervisor-type/arch/os_type
    // selection algorithm (`capabilities.py`'s `_guestForOSType`/
    // `_bestDomainType` priority chain: `kvm > xen > hvf > qemu`), not
    // `getDomainCapabilities(None,None,None,None)`'s incidental
    // default-param behavior — the two happened to agree for every
    // fixture this crate's tests currently exercise (confirmed directly:
    // `test:///default`'s own capture resolves to `hypervisor_type=
    // "test"`, `arch="i686"` either way), but only `guest_lookup` is the
    // real algorithm. No `--arch`/`--os-variant`/`--virt-type`/
    // `--machine` CLI surface exists yet to override
    // os_type/arch/type/machine, so every argument below is `None` —
    // same shape as upstream's own no-flags-given case (a freshly
    // constructed `Guest` starts with all four unset).
    //
    // `DomainCapabilities` itself isn't fetched here at all anymore:
    // this binary's only past use of it was `.domain`/`.arch`, both now
    // sourced from `capsinfo` instead. Upstream still fetches domcaps
    // separately (`DomainCapabilities.build_from_guest`, keyed off the
    // now-resolved emulator/arch/machine/type) for UEFI firmware lookup
    // and device-support queries — real future work once this crate
    // ports either, not dropped by oversight.
    let host_caps = Capabilities::fetch(&conn).unwrap_or_else(|e| {
        eprintln!("virt-install: failed to fetch host capabilities: {e}");
        std::process::exit(1);
    });
    let capsinfo = host_caps
        .guest_lookup(None, None, None, None)
        .unwrap_or_else(|e| {
            eprintln!("virt-install: {e}");
            std::process::exit(1);
        });
    let domain_type = capsinfo.hypervisor_type.unwrap_or_else(|| {
        eprintln!("virt-install: connection didn't report a default domain type");
        std::process::exit(1);
    });
    let arch = capsinfo.arch.unwrap_or_else(|| {
        eprintln!("virt-install: connection didn't report a default architecture");
        std::process::exit(1);
    });
    // `set_capabilities_defaults`: `self.os.os_type = capsinfo.os_type`
    // — threaded into every install method below except `--container`,
    // which hardcodes `"exe"` itself (`build_container_os_document`'s
    // own doc comment: `DomainOs.is_container()`'s own check, not
    // something `guest_lookup` would report — no driver reachable from
    // this crate advertises an `exe` guest type in its host capabilities
    // XML, so filtering on it here would just fail the lookup). Resolved
    // lazily, not eagerly: `<guest><os_type>` is a required element in
    // every real driver's capabilities XML, so this can't actually fail
    // in practice, but `--container` never touches the value at all —
    // no reason to make that path's success depend on a field it
    // doesn't use.
    let require_os_type = || -> String {
        capsinfo.os_type.clone().unwrap_or_else(|| {
            eprintln!("virt-install: connection didn't report a default os_type");
            std::process::exit(1);
        })
    };
    // ticket 21 slice (a): `machine` is `_set_default_machine`'s
    // fallback-only branch (`capsinfo.machines and capsinfo.machines[0]
    // or None`) — the q35-preferred branch above it needs `OsInfo` plus
    // an unbuilt hypervisor-version support-check mechanism, both out
    // of scope. `emulator` is `set_capabilities_defaults`'s plain
    // `self.emulator = capsinfo.emulator` thread. Both `None` when the
    // driver doesn't report one (`test:///default`'s own real machine
    // list is empty) — `build_domain_skeleton` omits the attribute/
    // element entirely rather than emitting an empty one. Neither is
    // threaded into `--container` below, deliberately — see `build_
    // container_os_document`'s doc comment for why there's no correct
    // value to give it.
    let machine = capsinfo.machines.first().cloned();
    let emulator = capsinfo.emulator.clone();
    // A real bug, found and fixed: the default NIC's OUI used to be
    // derived from `domain_type == "qemu"` — a literal string match on
    // `hypervisor_type`, which is `"kvm"` on a real KVM-accelerated
    // `qemu:///system` (confirmed against this machine's own real
    // capture, `domcaps.rs`'s `QEMU_DOMCAPS_EXCERPT`), not `"qemu"`.
    // That's not what upstream's `interface.py:30` keys on either — it
    // checks `conn.is_qemu()`, the connection's URI scheme, which is
    // what `Uri::is_qemu()` gives here. Unlike `os_type`/`machine`/
    // `emulator` above, this *does* apply to `--container` too: it's
    // not derived from `capsinfo` at all, so there's no "wrong guest"
    // hazard to avoid.
    let is_qemu = conn.uri().is_qemu();
    // `default_disk_bus`'s `is_bhyve` parameter — same URI-scheme-check
    // family as `is_qemu` above (`Uri::is_bhyve()`), not derived from
    // `domain_type` for the exact reason `is_qemu` isn't either (see
    // the comment just above): `domain_type` is the capsinfo-reported
    // hypervisor string, not a URI scheme.
    let is_bhyve = conn.uri().is_bhyve();
    // `prefers_uefi`'s `is_test` parameter — same URI-scheme-check
    // family, same reasoning as `is_bhyve` just above.
    let is_test = conn.uri().is_test();

    // Ticket 21 slice (c): `graphics` default type
    // (`DeviceGraphics._default_type`, `graphics.py:181`) —
    // `DomainCapabilities::fetch` dropped entirely from this binary
    // under ticket 20 piece (b) (its only past use, `.domain`/`.arch`,
    // moved to `capsinfo`); brought back here for its real purpose,
    // querying the `<devices><graphics>` block. Falls back to
    // `DomainCapabilities::default()` on fetch failure, matching
    // upstream's own `build_from_params`: "if not supported, just use
    // a stub object" rather than treating an old/unresponsive driver
    // as a hard error over a defaulting decision.
    let domcaps = DomainCapabilities::fetch(
        &conn,
        emulator.as_deref(),
        Some(&arch),
        machine.as_deref(),
        Some(&domain_type),
    )
    .unwrap_or_default();
    let graphics_type =
        domcaps.default_graphics_type(conn.uri(), host_caps.host.cpu.arch.as_deref());

    // Ticket 21 slice (f) piece 2: `cpu` mode defaulting
    // (`DomainCpu.set_defaults`, `domain/cpu.py:512`) — resolved once,
    // here, same shape as `graphics_type` above rather than `clock`'s
    // self-gating: the real decision needs `domcaps` plus the host's
    // own reported CPU (from `<capabilities>`, not domcaps), neither of
    // which the builders thread through today. `host_arch`/`host_cpu_
    // model` both come from the same `host_caps` `graphics_type` above
    // already fetched, no new capability query. `arch` here is the
    // *guest* arch (`capsinfo.arch`) — deliberately the other half of
    // the same `guest_arch`-vs-`host_arch` split `_set_cpu_x86_kvm_
    // default`'s `guest.os.arch != self.conn.caps.host.cpu.arch` check
    // makes, not the same value twice under different names.
    let cpu_mode = domcaps.default_cpu_mode(
        conn.uri(),
        &domain_type,
        &arch,
        host_caps.host.cpu.arch.as_deref().unwrap_or(""),
        host_caps.host.cpu.model.as_deref(),
    );

    // Ticket 04's real libosinfo bindings, first real consumer:
    // `--os-variant` resolves to an `OsInfo` only when given (upstream
    // always has one — a synthetic "generic" entry when no flag is
    // passed — but `clock_offset()`'s outcome for that entry is
    // identical to `None`'s fallback below, so there's nothing a
    // synthetic object here would add; see `push_default_clock`'s doc
    // comment). Accepts both the short-id and full-id forms — the
    // short one (`fedora25`) is what users actually type.
    let os_info = args.os_variant.as_deref().map(|variant| {
        let db = OsDb::load_default().unwrap_or_else(|e| {
            eprintln!("virt-install: failed to load osinfo-db: {e}");
            std::process::exit(1);
        });
        db.os_by_id_or_short_id(variant).unwrap_or_else(|| {
            eprintln!("virt-install: unknown --os-variant '{variant}'");
            std::process::exit(1);
        })
    });
    let clock_offset = os_info.as_ref().map_or("utc", |os| os.clock_offset());
    // `default_disk_bus`'s `supports_virtiodisk`/`supports_virtioscsi`
    // inputs — `false` with no `--os-variant` given, exactly matching
    // upstream's own synthetic "generic" `OsInfo` entry (no devices
    // declared), same reasoning `clock_offset`'s `None` fallback above
    // already documents.
    let supports_virtiodisk = os_info.as_ref().is_some_and(|os| os.supports_virtiodisk());
    let supports_virtioscsi = os_info.as_ref().is_some_and(|os| os.supports_virtioscsi());
    // `default_nic_model`'s own inputs, same fallback reasoning as
    // above: `false`/empty `Vec` with no `--os-variant` given, matching
    // upstream's synthetic "generic" entry (no devices declared).
    let supports_virtionet = os_info.as_ref().is_some_and(|os| os.supports_virtionet());
    let supported_netmodels = os_info
        .as_ref()
        .map(|os| os.supported_netmodels())
        .unwrap_or_default();
    // `prefers_uefi`'s `OsInfo`-derived input — `false` with no
    // `--os-variant` given, matching upstream's synthetic "generic"
    // entry (no `<firmware>` declarations at all, same `default`-
    // fallback branch `requires_firmware_efi`'s own doc comment names).
    let requires_firmware_efi = os_info
        .as_ref()
        .is_some_and(|os| os.requires_firmware_efi(&arch));
    // `default_firmware`'s other input — the modern libvirt "firmware
    // auto-select" capability, real per-driver data from `domcaps`
    // (already fetched above for `graphics_type`/`cpu_mode`).
    let supports_firmware_efi = domcaps.supports_firmware_efi();

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
        let os_type = require_os_type();
        let caps = GuestCapsDefaults {
            domain_type: &domain_type,
            os_type: &os_type,
            arch: &arch,
            machine: machine.as_deref(),
            emulator: emulator.as_deref(),
            is_qemu,
        };
        let disk_bus = default_disk_bus(
            caps,
            false,
            is_bhyve,
            supports_virtiodisk,
            supports_virtioscsi,
        );
        let nic_model = default_nic_model(caps, supports_virtionet, &supported_netmodels);
        let firmware = default_firmware(
            prefers_uefi(caps, is_test, is_bhyve, requires_firmware_efi),
            supports_firmware_efi,
        );
        let xml = write(build_import_document(
            &args.name,
            memory_kib,
            args.vcpus,
            &disk,
            caps,
            graphics_type,
            cpu_mode.as_ref(),
            clock_offset,
            disk_bus,
            nic_model,
            firmware,
        ));
        (xml.clone(), xml)
    } else if let Some(media) = args.cdrom {
        let os_type = require_os_type();
        let caps = GuestCapsDefaults {
            domain_type: &domain_type,
            os_type: &os_type,
            arch: &arch,
            machine: machine.as_deref(),
            emulator: emulator.as_deref(),
            is_qemu,
        };
        let cdrom_bus = default_disk_bus(
            caps,
            true,
            is_bhyve,
            supports_virtiodisk,
            supports_virtioscsi,
        );
        let nic_model = default_nic_model(caps, supports_virtionet, &supported_netmodels);
        let firmware = default_firmware(
            prefers_uefi(caps, is_test, is_bhyve, requires_firmware_efi),
            supports_firmware_efi,
        );
        if args.livecd {
            let xml = write(build_iso_document(
                &args.name,
                memory_kib,
                args.vcpus,
                &media,
                caps,
                graphics_type,
                cpu_mode.as_ref(),
                clock_offset,
                cdrom_bus,
                nic_model,
                firmware,
            ));
            (xml.clone(), xml)
        } else {
            let disk = args.disk.unwrap_or_else(|| {
                eprintln!("virt-install: --cdrom without --livecd requires --disk");
                std::process::exit(1);
            });
            let disk_bus = default_disk_bus(
                caps,
                false,
                is_bhyve,
                supports_virtiodisk,
                supports_virtioscsi,
            );
            let (initial, final_doc) = build_iso_two_phase_documents(
                &args.name,
                memory_kib,
                args.vcpus,
                &media,
                &disk,
                caps,
                graphics_type,
                cpu_mode.as_ref(),
                clock_offset,
                cdrom_bus,
                disk_bus,
                nic_model,
                firmware,
            );
            (write(initial), write(final_doc))
        }
    } else if args.pxe {
        let os_type = require_os_type();
        let caps = GuestCapsDefaults {
            domain_type: &domain_type,
            os_type: &os_type,
            arch: &arch,
            machine: machine.as_deref(),
            emulator: emulator.as_deref(),
            is_qemu,
        };
        let disk_bus = default_disk_bus(
            caps,
            false,
            is_bhyve,
            supports_virtiodisk,
            supports_virtioscsi,
        );
        let nic_model = default_nic_model(caps, supports_virtionet, &supported_netmodels);
        let firmware = default_firmware(
            prefers_uefi(caps, is_test, is_bhyve, requires_firmware_efi),
            supports_firmware_efi,
        );
        let (initial, final_doc) = build_manual_two_phase_documents(
            &args.name,
            memory_kib,
            args.vcpus,
            "network",
            args.disk.as_deref(),
            caps,
            graphics_type,
            cpu_mode.as_ref(),
            clock_offset,
            disk_bus,
            nic_model,
            firmware,
        );
        (write(initial), write(final_doc))
    } else if let Some(boot_dev) = args.boot {
        let os_type = require_os_type();
        let caps = GuestCapsDefaults {
            domain_type: &domain_type,
            os_type: &os_type,
            arch: &arch,
            machine: machine.as_deref(),
            emulator: emulator.as_deref(),
            is_qemu,
        };
        let disk_bus = default_disk_bus(
            caps,
            false,
            is_bhyve,
            supports_virtiodisk,
            supports_virtioscsi,
        );
        let nic_model = default_nic_model(caps, supports_virtionet, &supported_netmodels);
        let firmware = default_firmware(
            prefers_uefi(caps, is_test, is_bhyve, requires_firmware_efi),
            supports_firmware_efi,
        );
        let xml = write(build_manual_document(
            &args.name,
            memory_kib,
            args.vcpus,
            &boot_dev,
            args.disk.as_deref(),
            caps,
            graphics_type,
            cpu_mode.as_ref(),
            clock_offset,
            disk_bus,
            nic_model,
            firmware,
        ));
        (xml.clone(), xml)
    } else if args.container {
        let filesystem = args.filesystem.unwrap_or_else(|| {
            eprintln!("virt-install: --container requires --filesystem");
            std::process::exit(1);
        });
        let xml = write(build_container_os_document(
            &args.name,
            memory_kib,
            args.vcpus,
            &filesystem,
            &domain_type,
            &arch,
            is_qemu,
        ));
        (xml.clone(), xml)
    } else {
        eprintln!(
            "virt-install: specify --import, --cdrom, --pxe, --boot, or --container \
             (only methods implemented so far)"
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
