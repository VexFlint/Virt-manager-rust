//! Shared scaffolding for the direct-DOM domain builders
//! (`import::build_import_document`, `iso::build_iso_document`,
//! `manual::build_manual_document`) — extracted once a third caller
//! needed the identical `<name>`/`<uuid>`/`<memory>`/`<currentMemory>`/
//! `<vcpu>`/`<os><type arch>` skeleton, per this project's "wait for a
//! second/real consumer" rule (same rule `#[xml(nested)]` and
//! `connect_and_lookup`/`report` in `virtinst-cli` were built under —
//! two identical copies stayed duplicated, a third got extracted).
//!
//! Per the ticket 08 amendment: direct `edit_xml::Document`/`Element`
//! construction, not `Guest::to_xml_string()` — see any of the three
//! callers' doc comments for the full rationale.
//!
//! Also home to the fixed-default device pushers shared across builders
//! once a third caller needed them (`push_disk`,
//! `push_default_nic`) and the two-phase-only elements
//! (`push_on_reboot_destroy`).

use virtinst_xml::{Document, Element};

/// The `capsinfo`/URI-derived defaults shared by every non-container
/// document builder — upstream's `set_capabilities_defaults`'s
/// `domain_type`/`os_type`/`arch`/`machine`/`emulator` cluster, plus
/// `is_qemu` (URI-scheme-derived, not `capsinfo`-derived, but resolved
/// at the same `virt-install.rs` call site and needed by every one of
/// these same callers for `push_default_nic`, then — as of ticket 21
/// slice (c) — `push_default_graphics` too).
///
/// Collapsed from 6 individual parameters into one struct once a 3rd
/// field (`graphics_type`, threaded separately, see slice (c) below)
/// was about to join `machine`/`emulator`/`is_qemu` as yet another
/// mechanical addition to every affected builder's already-`#[allow(
/// clippy::too_many_arguments)]` signature — the third such threading
/// in a row (`.scratch/rust-conversion/issues/21-set-defaults-scope.md`
/// has the first two: `machine`/`emulator`, then `is_qemu`), each
/// paying the same "add a param, fix N call sites" tax. Pure refactor,
/// no behavior change — every existing test's assertions are untouched,
/// only call sites' argument lists changed shape.
///
/// Deliberately **not** used by `--container`: it only ever needs 3 of
/// these 6 fields (`domain_type`/`arch`/`is_qemu`), and forcing the
/// same struct on it would either silently ignore 3 fields or invite
/// exactly the "wrong guest" hazard `build_container_os_document`'s own
/// doc comment already warns against for `os_type`/`machine`/
/// `emulator` (no driver this crate reaches ever resolves a real
/// `capsinfo` for an `exe` guest, so there's no correct value to give
/// those fields, only a wrong one borrowed from the `hvm`/`xen` guest a
/// plain, unfiltered `guest_lookup` would actually return).
#[derive(Debug, Clone, Copy)]
pub struct GuestCapsDefaults<'a> {
    pub domain_type: &'a str,
    pub os_type: &'a str,
    pub arch: &'a str,
    pub machine: Option<&'a str>,
    pub emulator: Option<&'a str>,
    pub is_qemu: bool,
}

/// Builds `<domain type=domain_type><name/><uuid/><memory/>
/// <currentMemory/><vcpu/><os><type arch=arch machine=machine>os_type
/// </type></os><devices><emulator>emulator</emulator></devices></domain>`
/// and returns the open document plus its `<os>` and `<devices>`
/// elements — callers push their own `<boot>` entries onto `os` (bootdev
/// handling differs enough per install method, see `Iso::live`/`Manual`,
/// that it isn't shared here — container guests skip `<boot>` entirely,
/// see `container.rs`) and their own devices onto the returned `devices`
/// element (already attached to `root`, same "push into an already-
/// attached element" pattern `os_el` has always used for `<boot>`).
///
/// `caps.os_type` is `<os><type>`'s text content — `"hvm"` for every
/// install method except containers (`"exe"`, `DomainOs.is_container()`'s
/// check, hardcoded by `container.rs` directly rather than threaded
/// through here). `caps.arch` is still always passed through from
/// `DomainCapabilities` even for containers — confirmed directly in
/// `guest.py`'s `set_capabilities_defaults`: `self.os.arch = capsinfo.
/// arch` runs unconditionally, no container exception, so this isn't
/// guessed.
///
/// `caps.machine`/`caps.emulator` are ticket 21 slice (a): `capsinfo.
/// machines.first()`/`capsinfo.emulator` threaded through when the
/// caller has them, `None` when it doesn't. Neither attribute/element
/// is emitted when absent — `machine` omitted from `<type>` (matches
/// upstream's own `_set_default_machine`, whose *fallback-only* branch
/// this ports: `capsinfo.machines and capsinfo.machines[0] or None`;
/// the q35-preferred branch above it needs `OsInfo` plus an unbuilt
/// hypervisor-version support-check mechanism, both out of scope per
/// ticket 21), `<emulator>` omitted from `<devices>` entirely rather
/// than emitted empty.
///
/// `caps.is_qemu` isn't used by this function at all (only `caps.
/// domain_type`/`os_type`/`arch`/`machine`/`emulator` are) — it rides
/// along in the same struct because every caller needs it a few lines
/// later for `push_default_nic`/`push_default_graphics`, and passing
/// two separate params where callers already have one bundle would be
/// the pointless split, not this.
///
/// `uuid` is caller-supplied rather than generated here — required
/// because `virt-install`'s default sequence calls `Domain::create_new`
/// then `Domain::define_new`, and for a two-phase install (`Iso { live:
/// false }`) those two calls need two *different* XML documents that
/// still describe the same domain, i.e. the same UUID stamped into both.
/// A single `generate_uuid()` inside this function couldn't do that — the
/// caller generates one and passes it to both `build_domain_skeleton`
/// calls (see `iso::build_iso_two_phase_documents`). Callers that only
/// ever build one document (`import`, `manual`, single-phase `iso`) just
/// generate one `generate_uuid()` and pass it through same as before.
/// Without a fixed UUID at all, each libvirt call invents its own and
/// the second call fails outright ("domain already exists with uuid
/// ..." — hit this for real, see `import.rs`'s doc comment).
pub(super) fn build_domain_skeleton(
    name: &str,
    memory_kib: u64,
    vcpus: u32,
    uuid: &str,
    caps: &GuestCapsDefaults,
) -> (Document, Element, Element, Element) {
    let mut doc = Document::new();
    let root = Element::new(&mut doc, "domain");
    root.set_attribute(&mut doc, "type", caps.domain_type);
    doc.push_root_node(root)
        .expect("a freshly-created detached element always attaches cleanly");

    push_text_child(&mut doc, root, "name", name);
    push_text_child(&mut doc, root, "uuid", uuid);

    let mem_el = Element::new_with_text(&mut doc, "memory", &memory_kib.to_string());
    mem_el.set_attribute(&mut doc, "unit", "KiB");
    root.push_child(&mut doc, mem_el)
        .expect("a freshly-created detached element always attaches cleanly");

    let curmem_el = Element::new_with_text(&mut doc, "currentMemory", &memory_kib.to_string());
    curmem_el.set_attribute(&mut doc, "unit", "KiB");
    root.push_child(&mut doc, curmem_el)
        .expect("a freshly-created detached element always attaches cleanly");

    push_text_child(&mut doc, root, "vcpu", &vcpus.to_string());

    let os_el = Element::new(&mut doc, "os");
    let type_el = Element::new_with_text(&mut doc, "type", caps.os_type);
    type_el.set_attribute(&mut doc, "arch", caps.arch);
    if let Some(m) = caps.machine {
        type_el.set_attribute(&mut doc, "machine", m);
    }
    os_el
        .push_child(&mut doc, type_el)
        .expect("a freshly-created detached element always attaches cleanly");
    root.push_child(&mut doc, os_el)
        .expect("a freshly-created detached element always attaches cleanly");

    let devices_el = Element::new(&mut doc, "devices");
    if let Some(e) = caps.emulator {
        push_text_child(&mut doc, devices_el, "emulator", e);
    }
    root.push_child(&mut doc, devices_el)
        .expect("a freshly-created detached element always attaches cleanly");

    (doc, root, os_el, devices_el)
}

pub(super) fn push_text_child(doc: &mut Document, parent: Element, tag: &str, text: &str) {
    let el = Element::new_with_text(doc, tag, text);
    parent
        .push_child(doc, el)
        .expect("a freshly-created detached element always attaches cleanly");
}

/// Pushes `<boot dev=dev/>` onto `os` — shared by every caller that
/// needs to add one boot entry (`Manual` may add a second, see
/// `manual::build_manual_document`).
pub(super) fn push_boot_dev(doc: &mut Document, os: Element, dev: &str) {
    let boot_el = Element::new(doc, "boot");
    boot_el.set_attribute(doc, "dev", dev);
    os.push_child(doc, boot_el)
        .expect("a freshly-created detached element always attaches cleanly");
}

/// Pushes `<on_reboot>destroy</on_reboot>` onto `root` — upstream's
/// `_alter_bootconfig`'s very first line (`guest.on_reboot = "destroy"`),
/// applied *only* to the initial (`createXML`) document in a two-phase
/// install. This is the actual mechanism that makes two-phase installs
/// work: libvirt's own default is `restart`, so without this override
/// the transient instance would reboot straight back into the install
/// media/bootdev forever instead of yielding to the already-`defineXML`-
/// ed final config once the install's reboot happens. Missing entirely
/// from the first two-phase commit (`iso.rs`'s `Iso { live: false }`) —
/// no test caught it because `test:///` never actually boots anything,
/// so nothing here exercises the behavior this element controls, only
/// its presence in the XML.
pub(super) fn push_on_reboot_destroy(doc: &mut Document, root: Element) {
    push_text_child(doc, root, "on_reboot", "destroy");
}

/// `<disk type="file" device="disk"><source file=.../><target dev=.../
/// bus=bus/></disk>`, pushed onto `devices_el` — the persistent disk
/// shape shared by `import.rs`, `iso.rs`'s two-phase case, and
/// `manual.rs`. `bus` is the caller's already-resolved answer
/// (`default_disk_bus`, or a literal in tests that don't care about the
/// decision tree) — same "resolve once, pass the answer in" pattern
/// `clock_offset`/`graphics_type` already use, not computed here.
/// `target dev`'s prefix follows `bus` via `target_prefix_for_bus`
/// (`"vda"` for `bus="virtio"`, `"hda"` for `bus="ide"`, etc.) — this
/// function used to hardcode both `bus="virtio"` and `dev="vda"`
/// unconditionally; renamed from `push_virtio_disk` once bus stopped
/// being a foregone conclusion. Moved here once `iso.rs` and
/// `manual.rs` became the second and third copies.
pub(super) fn push_disk(doc: &mut Document, devices_el: Element, path: &str, bus: &str) {
    let disk_el = Element::new(doc, "disk");
    disk_el.set_attribute(doc, "type", "file");
    disk_el.set_attribute(doc, "device", "disk");
    let source_el = Element::new(doc, "source");
    source_el.set_attribute(doc, "file", path);
    disk_el
        .push_child(doc, source_el)
        .expect("a freshly-created detached element always attaches cleanly");
    let target_el = Element::new(doc, "target");
    target_el.set_attribute(doc, "dev", format!("{}a", target_prefix_for_bus(bus)));
    target_el.set_attribute(doc, "bus", bus);
    disk_el
        .push_child(doc, target_el)
        .expect("a freshly-created detached element always attaches cleanly");
    devices_el
        .push_child(doc, disk_el)
        .expect("a freshly-created detached element always attaches cleanly");
}

/// Upstream's `domain/os.py`'s arch/`os_type`/`machine` predicates,
/// ported as free functions over the plain strings `GuestCapsDefaults`
/// already carries — matching how `push_default_clock` already treats
/// `os_type`/`arch` as plain parameters rather than a wrapping
/// `Guest`/`DomainOs` object. Only the ones a real caller below
/// actually needs are ported; upstream's `is_ppc64` (without the
/// `pseries` machine check)/`is_riscv` (without the `virt` machine
/// check) have no caller here and are omitted rather than spuriously
/// added.
fn is_hvm(os_type: &str) -> bool {
    os_type == "hvm"
}
fn is_xenpv(os_type: &str) -> bool {
    matches!(os_type, "xen" | "linux" | "xenpvh")
}
fn is_x86(arch: &str) -> bool {
    arch == "x86_64" || arch == "i686"
}
fn is_q35(arch: &str, machine: Option<&str>) -> bool {
    is_x86(arch) && machine.is_some_and(|m| m.contains("q35"))
}
fn is_riscv_virt(arch: &str, machine: Option<&str>) -> bool {
    matches!(arch, "riscv64" | "riscv32") && machine.is_some_and(|m| m.starts_with("virt"))
}
fn is_pseries(arch: &str, machine: Option<&str>) -> bool {
    matches!(arch, "ppc64" | "ppc64le") && machine.is_some_and(|m| m.starts_with("pseries"))
}
fn is_s390x(arch: &str) -> bool {
    arch == "s390x"
}
fn is_loongarch64(arch: &str) -> bool {
    arch == "loongarch64"
}
fn is_arm_machvirt(arch: &str, machine: Option<&str>) -> bool {
    matches!(arch, "armv7l" | "aarch64") && machine.is_some_and(|m| m.starts_with("virt"))
}

/// Upstream's `guest.py`'s `_supports_virtio` — the `conn.is_qemu()`
/// gate first (non-qemu connections never get virtio, regardless of
/// what the OS supports — upstream's own comment doesn't explain why,
/// it just guards the whole method on it), then the four arches that
/// *only* ever expose virtio controllers (no legacy IDE/e1000/etc.
/// hardware exists for them at all), which short-circuit to `true`
/// regardless of `os_support` before it's ever consulted. Everything
/// else defers to the real per-OS `os_support` value (`OsInfo::
/// supports_virtio{disk,scsi,net}()`, ticket 04's second caller).
///
/// `is_bhyve` isn't one of the four short-circuit arches upstream
/// checks here — it's a separate, later branch in `_default_bus`
/// itself (`self.conn.is_bhyve()`, a URI-scheme check like `is_qemu`,
/// not an arch/machine check), kept out of this function to match
/// upstream's own structure.
fn supports_virtio(arch: &str, machine: Option<&str>, is_qemu: bool, os_support: bool) -> bool {
    if !is_qemu {
        return false;
    }
    if is_riscv_virt(arch, machine)
        || is_s390x(arch)
        || is_pseries(arch, machine)
        || is_loongarch64(arch)
    {
        return true;
    }
    os_support
}

/// Upstream's `devices/disk.py`'s `_default_bus`, ported for the two
/// device shapes this crate's builders actually construct
/// (`device="disk"` and `device="cdrom"` — no floppy device exists
/// anywhere in this crate, so upstream's `is_floppy()` branch has no
/// analog here and is omitted rather than modeled unreachable).
///
/// **Wired as of the commit that added this line** — every builder's
/// `disk_bus`/`cdrom_bus` parameter is now the caller's already-resolved
/// answer from this function (`virt-install.rs`'s own call sites), not a
/// hardcoded `"virtio"`/`"ide"` constant. Unit-tested in isolation first
/// (this file's own `bus_selection_tests`, against synthetic
/// `GuestCapsDefaults`) before any caller wired it, specifically so a
/// test failure at either stage could only be the tree or the wiring —
/// never both at once, see this crate's git history for that split.
///
/// Every builder function's own unit tests still pass a literal `bus`
/// string (`"virtio"`, `"ide"`) rather than routing through this
/// function — they're proving each builder places `bus`/`dev`/`readonly`
/// correctly for whatever value it's given, not re-testing the decision
/// tree itself (already covered here). Only `virt-install.rs`'s real
/// call sites compute a bus through this function, so this is the one
/// place in this crate where `!is_qemu` genuinely does now flip the
/// real, observable default (`bus="virtio"` → `bus="ide"` for a
/// `--os-variant`-less, non-qemu-connection invocation) — upstream-
/// faithful (`_supports_virtio` gates the whole thing on
/// `conn.is_qemu()` first too), not a regression.
///
/// `is_bhyve` is a plain `bool` parameter here, not derived from
/// `caps.domain_type` — `domain_type` is the capsinfo-reported
/// hypervisor string (`"kvm"` on a real KVM host, not the literal
/// `"qemu"`), the exact conflation `push_default_nic`'s own doc
/// comment already documents as a real, previously-shipped bug for
/// `is_qemu`. The real caller-side value is `Uri::is_bhyve()` (a
/// URI-scheme check, same family as `Uri::is_qemu()` — both already
/// exist on `connection::uri::Uri`), threaded through from
/// `virt-install.rs` the same way `is_qemu` already is.
///
/// Real libvirt is lenient about a `bus`/target-`dev`-prefix mismatch
/// (confirmed against `test:///`: `<target dev='hda' bus='sata'/>`
/// defines cleanly, libvirt even synthesizes the matching `<controller
/// type='sata'>` for it) — `target_prefix_for_bus` below exists for
/// output fidelity with upstream's own `generate_target`, not because
/// a mismatch would be rejected.
///
// ponytail: four consecutive `bool`s -- correct today (all 6 call
// sites in `virt-install.rs` are checked against this signature), but
// nothing stops a future call site from passing them in the wrong
// order and still compiling. A params struct would be over-building
// for one function; if a second real caller (not just a test) shows
// up, that's the trigger to add one.
pub fn default_disk_bus(
    caps: GuestCapsDefaults,
    is_cdrom: bool,
    is_bhyve: bool,
    supports_virtiodisk: bool,
    supports_virtioscsi: bool,
) -> &'static str {
    if is_xenpv(caps.os_type) {
        return "xen";
    }
    if !is_hvm(caps.os_type) {
        return "ide";
    }
    if !is_cdrom && supports_virtio(caps.arch, caps.machine, caps.is_qemu, supports_virtiodisk) {
        return "virtio";
    }
    if is_q35(caps.arch, caps.machine) {
        return "sata";
    }
    if is_bhyve {
        return "sata";
    }
    if is_x86(caps.arch) {
        return "ide";
    }
    if is_cdrom && supports_virtio(caps.arch, caps.machine, caps.is_qemu, supports_virtioscsi) {
        return "scsi";
    }
    if caps.machine.is_some_and(|m| m.starts_with("vexpress")) {
        return "sd";
    }
    "usb"
}

/// Upstream's `devices/disk.py`'s `get_target_prefix` — the
/// bus-to-target-device-name-prefix table `generate_target` uses to
/// name a disk's `<target dev=.../>` consistently with its `bus`.
/// Ported for fidelity (see `default_disk_bus`'s doc comment on why
/// libvirt itself doesn't require this), not modeling upstream's
/// per-prefix numbering-limit table (`nummap`) since every caller here
/// builds exactly one disk of each device kind — there's never a
/// second one to number past `a`.
pub(super) fn target_prefix_for_bus(bus: &str) -> &'static str {
    match bus {
        "virtio" => "vd",
        "xen" => "xvd",
        "fdc" => "fd",
        "ide" => "hd",
        // sata, scsi, usb, sd
        _ => "sd",
    }
}

/// Upstream's `devices/interface.py`'s `DeviceInterface.default_model`,
/// ported. `None` means upstream's own `None` branches (non-`hvm`, or
/// non-x86 with neither virtio nor q35 available) — omit the
/// `<model>` element entirely in that case, matching `self.model =
/// self.default_model(guest)` leaving `self.model` unset when the
/// right-hand side is `None`, not a fallback to some other constant.
///
/// `supports_virtionet`/`supported_netmodels` are the caller's already-
/// fetched `OsInfo` answers (`OsInfo::supports_virtionet()`/
/// `supported_netmodels()`, ticket 04) — same "resolve once, pass the
/// answer in" shape `default_disk_bus` uses, and the same `false`/
/// empty-`Vec` fallback with no `--os-variant` given (upstream's
/// synthetic "generic" entry declares no devices at all, so both
/// resolve the same way whether or not a real `OsInfo` exists).
/// `supports_virtio` (this file's own helper, same one `default_disk_
/// bus` uses) applies the `is_qemu` gate and the four virtio-only-arch
/// overrides before `supports_virtionet` is ever consulted, exactly
/// like upstream's `guest.supports_virtionet()` wrapping `self.osinfo.
/// supports_virtionet(..)` in `_supports_virtio`.
///
/// **Wired** — `push_default_nic` below takes the caller's resolved
/// `Option<&str>` from this function instead of a hardcoded
/// `"virtio"` constant, same shape as `default_disk_bus`'s own
/// wiring.
pub fn default_nic_model(
    caps: GuestCapsDefaults,
    supports_virtionet: bool,
    supported_netmodels: &[String],
) -> Option<&'static str> {
    if !is_hvm(caps.os_type) {
        return None;
    }
    if supports_virtio(caps.arch, caps.machine, caps.is_qemu, supports_virtionet) {
        return Some("virtio");
    }
    if is_q35(caps.arch, caps.machine) {
        return Some("e1000e");
    }
    if !is_x86(caps.arch) {
        return None;
    }
    const PREFS: [&str; 4] = ["e1000", "rtl8139", "ne2k_pci", "pcnet"];
    for pref in PREFS {
        if supported_netmodels.iter().any(|m| m == pref) {
            return Some(pref);
        }
    }
    Some("e1000")
}

/// Upstream's `guest.py`'s `prefers_uefi()`, ported. Two disjoint
/// cases, matching upstream's own `if`/`else` exactly rather than one
/// merged condition:
/// - x86 on a qemu or test connection: defers entirely to `OsInfo`
///   (`requires_firmware_efi`) — upstream's own comment: "if OS has
///   dropped support for bios, we have no choice but to use EFI; for
///   other OS still prefer BIOS since it's faster and doesn't break
///   QEMU internal snapshots."
/// - everything else: a fixed set of arch/machine combinations that
///   only ever boot through UEFI in practice (arm/riscv/loongarch64
///   `machvirt`-style machines, and bhyve), `OsInfo` not consulted at
///   all — matches upstream exactly, not a simplification.
///
/// `is_test`/`is_bhyve` are plain `bool` parameters, same reasoning as
/// `default_disk_bus`'s own `is_bhyve` — both are `Uri`-scheme checks
/// (`Uri::is_test()`/`Uri::is_bhyve()`, already exist), not derivable
/// from `caps.domain_type` without the same conflation `push_default_
/// nic`'s doc comment documents as a real, previously-shipped bug for
/// `is_qemu`.
///
/// **Built and unit-tested here, not yet wired to a real caller** —
/// no builder in this module emits `<os firmware=...>` at all yet.
/// Wiring this needs `DomainCapabilities::supports_firmware_efi()`
/// (ticket 04/this slice's second real caller) threaded through
/// alongside the `OsInfo`-derived `bool` this function takes, the same
/// "resolve once in `virt-install.rs`, pass the answer in" shape
/// `default_disk_bus`/`default_nic_model` both went through — kept as
/// its own future slice per this session's "one slice, verified, per
/// turn" discipline. Even once wired, this crate can only ever reach
/// the fast path (`supports_firmware_efi() == true` → `<os
/// firmware="efi">`, no `<loader>`/`<nvram>` needed) — the loader-path
/// fallback (`_lookup_default_uefi_path()`, a real filesystem scan
/// over JSON firmware-descriptor files) is a distinct subsystem this
/// crate doesn't model, not a corner cut inside this function.
pub fn prefers_uefi(
    caps: GuestCapsDefaults,
    is_test: bool,
    is_bhyve: bool,
    requires_firmware_efi: bool,
) -> bool {
    if is_x86(caps.arch) && (caps.is_qemu || is_test) {
        requires_firmware_efi
    } else {
        is_arm_machvirt(caps.arch, caps.machine)
            || is_riscv_virt(caps.arch, caps.machine)
            || is_loongarch64(caps.arch)
            || is_bhyve
    }
}

/// `<graphics type=graphics_type port="-1" .../>` (`tlsPort="-1"
/// autoport="yes"` added only for `spice`) — ticket 21 slice (c):
/// upstream's `_add_default_graphics` + `DeviceGraphics.set_defaults`'s
/// port/tlsPort/autoport resolution (`graphics.py`'s `_get_default_port`/
/// `_get_default_tlsport`/`_get_default_autoport`), restricted to the one
/// case every builder here actually reaches: no caller ever adds a
/// `<listen>` element, so `_listen_need_port()` is always `True` and the
/// full listen-aware branching collapses to a fixed shape per type.
/// `graphics_type` (`"spice"`/`"vnc"`) is caller-resolved via
/// `DomainCapabilities::default_graphics_type` — this function just
/// materializes the XML for whichever type won, it doesn't decide.
///
/// Not called by `--container` at all: upstream's own guard
/// (`self.os.is_container() and not self.conn.is_vz()`) skips the whole
/// device unconditionally on every driver this crate can reach
/// (`is_vz()` is never true here), matching `build_container_os_
/// document`'s existing container-only omissions for `os_type`/
/// `machine`/`emulator`.
pub(super) fn push_default_graphics(doc: &mut Document, devices_el: Element, graphics_type: &str) {
    let graphics_el = Element::new(doc, "graphics");
    graphics_el.set_attribute(doc, "type", graphics_type);
    graphics_el.set_attribute(doc, "port", "-1");
    if graphics_type == "spice" {
        graphics_el.set_attribute(doc, "tlsPort", "-1");
        graphics_el.set_attribute(doc, "autoport", "yes");
    }
    devices_el
        .push_child(doc, graphics_el)
        .expect("a freshly-created detached element always attaches cleanly");
}

/// `<clock><timer name="rtc" tickpolicy="catchup"/><timer name="pit"
/// tickpolicy="delay"/><timer name="hpet" present="no"/></clock>` —
/// ticket 21 slice (d): upstream's `DomainClock.set_defaults`
/// (`clock.py:41`), the x86+qemu-only base timer trio (`-no-hpet
/// -no-kvm-pit-reinjection -rtc driftfix=slew`, upstream's own comment
/// on what this maps to at the qemu command-line level).
///
/// Self-gating, exactly like upstream's own three early-returns, so
/// this is called unconditionally from every builder including
/// `--container` (upstream calls `self.clock.set_defaults(self)`
/// unconditionally too, from `guest.py`'s `set_defaults()`, relying on
/// the same gate) rather than needing a "not container" exception the
/// way `push_default_graphics` does:
/// - `os_type != "hvm"` (upstream's `guest.os.is_hvm()`) — containers
///   (`os_type = "exe"`) get no `<clock>` at all, matches without a
///   separate check at any call site.
/// - `arch` not x86 (upstream's `is_x86()`: `x86_64`/`i686` only).
/// - `!is_qemu` (upstream's `conn.is_qemu()`).
///
/// `offset` (upstream's `osinfo.get_clock()`) was blocked on `OsInfo`
/// not existing at all — now that `osinfo::OsInfo::clock_offset()`
/// does (ticket 04, then wired here), always emitted rather than
/// omitted, matching upstream's real shape: upstream's `guest.osinfo`
/// is *never* absent, even with no `--os-variant` given — it falls
/// back to a synthetic "generic" entry with no family set, which
/// resolves to `"utc"` through the exact same `get_clock()` logic
/// `clock_offset()` ports (not a special-cased default coded
/// separately). Callers pass `None` for that same "no OS selected"
/// case rather than constructing a synthetic generic `OsInfo` — the
/// outcome is identical (no family info → `"utc"`), so there's nothing
/// a synthetic object would add. This *is* a real, deliberate output
/// change from this function's previous version, which omitted
/// `offset` and let libvirt's own hypervisor-specific default
/// (`utc` for qemu) stand implicitly — every domain this crate builds
/// now gets an explicit `offset="utc"` (or `"localtime"` for a
/// Windows/Solaris `--os-variant`) instead. The optional `hypervclock`
/// timer (`guest.hyperv_supported()` — needs `is_windows()` **and** a
/// hv-version support-check mechanism this crate still doesn't have,
/// `conn.support.conn_hyperv_clock()`) stays unbuilt regardless:
/// `is_windows()` existing doesn't make the *other* half of that `and`
/// reachable too.
pub(super) fn push_default_clock(
    doc: &mut Document,
    root: Element,
    os_type: &str,
    arch: &str,
    is_qemu: bool,
    offset: &str,
) {
    if os_type != "hvm" || (arch != "x86_64" && arch != "i686") || !is_qemu {
        return;
    }

    let clock_el = Element::new(doc, "clock");
    clock_el.set_attribute(doc, "offset", offset);

    let rtc_el = Element::new(doc, "timer");
    rtc_el.set_attribute(doc, "name", "rtc");
    rtc_el.set_attribute(doc, "tickpolicy", "catchup");
    clock_el
        .push_child(doc, rtc_el)
        .expect("a freshly-created detached element always attaches cleanly");

    let pit_el = Element::new(doc, "timer");
    pit_el.set_attribute(doc, "name", "pit");
    pit_el.set_attribute(doc, "tickpolicy", "delay");
    clock_el
        .push_child(doc, pit_el)
        .expect("a freshly-created detached element always attaches cleanly");

    let hpet_el = Element::new(doc, "timer");
    hpet_el.set_attribute(doc, "name", "hpet");
    hpet_el.set_attribute(doc, "present", "no");
    clock_el
        .push_child(doc, hpet_el)
        .expect("a freshly-created detached element always attaches cleanly");

    root.push_child(doc, clock_el)
        .expect("a freshly-created detached element always attaches cleanly");
}

/// `<cpu mode="host-passthrough"/>` (or `host-model`/`maximum`/a
/// `custom` model) — ticket 21 slice (f) piece 2. Unlike `push_default_
/// clock`, this doesn't self-gate on primitives already in hand: the
/// real decision (host-passthrough vs. host-model vs. custom vs.
/// nothing at all) needs `DomainCapabilities` plus the host's own
/// reported CPU model from `<capabilities>`, neither of which this
/// module threads through today. Same shape as `push_default_graphics`
/// instead — the caller resolves via `DomainCapabilities::default_cpu_
/// mode` and passes the already-decided [`DefaultCpuMode`], or `None`
/// for "don't emit `<cpu>` at all" (a real, non-error outcome: a xen
/// connection, cross-arch emulation, or old-enough domcaps with no
/// safe default to offer). This function just materializes the XML for
/// whichever outcome won, it doesn't decide.
///
/// Called unconditionally from every builder including `--container`
/// (same as `push_default_clock`, unlike `push_default_graphics`):
/// `default_cpu_mode` self-gates on `domain_type`/arch the same way
/// `push_default_clock` gates on `os_type`/arch/`is_qemu`, so a
/// container's `domain_type` (`"lxc"`, never `"kvm"`/`"qemu"`) already
/// resolves to `None` with no separate container check needed at any
/// call site.
pub(super) fn push_default_cpu(
    doc: &mut Document,
    root: Element,
    resolved: Option<&crate::capabilities::DefaultCpuMode>,
) {
    use crate::capabilities::DefaultCpuMode;

    let Some(resolved) = resolved else {
        return;
    };

    let cpu_el = Element::new(doc, "cpu");
    match resolved {
        DefaultCpuMode::Mode(mode) => {
            cpu_el.set_attribute(doc, "mode", *mode);
        }
        DefaultCpuMode::Custom { model } => {
            cpu_el.set_attribute(doc, "mode", "custom");
            cpu_el.set_attribute(doc, "match", "exact");
            if let Some(model) = model {
                push_text_child(doc, cpu_el, "model", model);
            }
        }
    }
    root.push_child(doc, cpu_el)
        .expect("a freshly-created detached element always attaches cleanly");
}

/// `<interface type="network"><source network="default"/><mac
/// address=mac/>[<model type=model/>]</interface>`, pushed onto
/// `devices_el` — upstream's own CLI default, not a `--pxe`-specific
/// fix: `virtinstall.py`'s `convert_old_networks` synthesizes a single
/// `"default"`-network entry whenever both `--network` and
/// `--nonetworks` are absent, applied unconditionally before any
/// install-method-specific logic runs. Every document this crate builds
/// gets one, matching that. `--pxe` is just the one install method
/// where omitting it makes the resulting domain outright inert (can't
/// network-boot at all) rather than merely missing connectivity
/// afterward — that's what prompted grounding this, but the fix is
/// general.
///
/// `mac` is caller-supplied for the same reason `uuid` is: a two-phase
/// pair's initial and final documents must describe the *same* device,
/// not two independently-randomized MACs. Reuses `crate::cloner::
/// generate_mac` (`52:54:00`/`00:16:3E` OUI by driver) rather than
/// duplicating it — the exact same function this crate's `virt-clone`
/// support already uses for the same reason.
///
/// **A real bug, found and fixed, not a hypothetical one**: every
/// caller used to derive `generate_mac`'s `is_qemu` from `domain_type
/// == "qemu"` (the hypervisor-type string threaded through from
/// `guest_lookup`/`DomainCapabilities`) instead of `cloner.rs`'s own
/// correct `Uri::is_qemu()` (the connection's URI scheme). Those two
/// disagree on the single most common real invocation this crate
/// builds: `--connect qemu:///system` (the default) with KVM available
/// resolves `hypervisor_type` to `"kvm"`, not the literal string
/// `"qemu"` (confirmed against this machine's own real capture —
/// `domcaps.rs`'s `QEMU_DOMCAPS_EXCERPT`: `<domain>kvm</domain>`) — so
/// every real, non-`test:///` domain this crate ever built got the
/// Xen OUI (`00:16:3E`) instead of qemu's (`52:54:00`). Not reachable
/// through `test:///default` at all (its own domain type genuinely is
/// the literal string `"test"`, coincidentally never triggering the
/// bug in this crate's own test suite) — an earlier version of this
/// comment claimed the divergence was "not reachable through this
/// crate's current CLI surface," which was false the whole time:
/// `--connect` has defaulted to `qemu:///system` since `virt-install`'s
/// first commit. Fixed by threading a real `is_qemu: bool` (`conn.
/// uri().is_qemu()`, computed once in `virt-install.rs`) through every
/// caller instead of inferring it from `domain_type`.
///
/// `model` is the caller's already-resolved `default_nic_model`
/// answer — `None` omits `<model>` entirely, matching upstream's own
/// `self.model = None` (non-`hvm`, or non-x86 with neither virtio nor
/// q35 available). Used to be a hardcoded `model type="virtio"`
/// constant; wired the same way `push_disk`'s `bus` parameter was.
pub(super) fn push_default_nic(
    doc: &mut Document,
    devices_el: Element,
    mac: &str,
    model: Option<&str>,
) {
    let iface_el = Element::new(doc, "interface");
    iface_el.set_attribute(doc, "type", "network");
    let source_el = Element::new(doc, "source");
    source_el.set_attribute(doc, "network", "default");
    iface_el
        .push_child(doc, source_el)
        .expect("a freshly-created detached element always attaches cleanly");
    let mac_el = Element::new(doc, "mac");
    mac_el.set_attribute(doc, "address", mac);
    iface_el
        .push_child(doc, mac_el)
        .expect("a freshly-created detached element always attaches cleanly");
    if let Some(model) = model {
        let model_el = Element::new(doc, "model");
        model_el.set_attribute(doc, "type", model);
        iface_el
            .push_child(doc, model_el)
            .expect("a freshly-created detached element always attaches cleanly");
    }
    devices_el
        .push_child(doc, iface_el)
        .expect("a freshly-created detached element always attaches cleanly");
}

#[cfg(test)]
mod default_selection_tests {
    use super::*;

    /// The common case this crate actually targets: a real qemu/KVM
    /// x86_64 `pc-*` guest with an OS that reports virtio support —
    /// `push_disk`'s previously-hardcoded output, now reached via the
    /// decision tree instead of a fixed constant.
    #[test]
    fn qemu_x86_pc_with_virtio_support_picks_virtio_for_disk() {
        let caps = GuestCapsDefaults {
            domain_type: "kvm",
            os_type: "hvm",
            arch: "x86_64",
            machine: Some("pc-q35-8.2"),
            emulator: None,
            is_qemu: true,
        };
        // machine contains "q35" here on purpose -- proves the
        // virtio-disk branch is checked *before* the is_q35 branch,
        // matching upstream's own ordering (virtio wins over q35/sata
        // whenever the OS actually supports it).
        assert_eq!(default_disk_bus(caps, false, false, true, true), "virtio");
    }

    /// Same guest, but the OS doesn't declare virtio-disk support
    /// (e.g. a pre-virtio `--os-variant`) and the machine really is
    /// q35 — falls through to `sata`, not `ide`, matching upstream's
    /// branch order (q35 checked before the unconditional x86 `ide`).
    #[test]
    fn q35_without_virtio_support_picks_sata_for_disk() {
        let caps = GuestCapsDefaults {
            domain_type: "kvm",
            os_type: "hvm",
            arch: "x86_64",
            machine: Some("pc-q35-8.2"),
            emulator: None,
            is_qemu: true,
        };
        assert_eq!(default_disk_bus(caps, false, false, false, false), "sata");
    }

    /// The i440fx (non-q35) x86 case without virtio support — the
    /// long-standing IDE default, both for the persistent disk and
    /// the cdrom (upstream's own comment: "x86 long time default has
    /// been IDE CDROM").
    #[test]
    fn plain_x86_without_virtio_support_picks_ide_for_disk_and_cdrom() {
        let caps = GuestCapsDefaults {
            domain_type: "kvm",
            os_type: "hvm",
            arch: "x86_64",
            machine: Some("pc-i440fx-8.2"),
            emulator: None,
            is_qemu: true,
        };
        assert_eq!(default_disk_bus(caps, false, false, false, false), "ide");
        assert_eq!(default_disk_bus(caps, true, false, false, false), "ide");
    }

    /// A non-x86, non-q35 cdrom with virtio-scsi support gets `scsi` —
    /// the branch upstream's own comment says only matters off x86,
    /// since `is_x86()` returns `ide` unconditionally before it's ever
    /// reached on x86. `riscv64`/`virt` also happens to be one of the
    /// four virtio-only arches, so `supports_virtio` short-circuits to
    /// `true` regardless of the `os_support` argument passed in.
    #[test]
    fn non_x86_cdrom_with_virtioscsi_support_picks_scsi() {
        let caps = GuestCapsDefaults {
            domain_type: "kvm",
            os_type: "hvm",
            arch: "riscv64",
            machine: Some("virt"),
            emulator: None,
            is_qemu: true,
        };
        assert_eq!(default_disk_bus(caps, true, false, false, false), "scsi");
    }

    /// `!is_qemu` overrides every other signal, including an OS that
    /// genuinely supports virtio — matches upstream's `_supports_virtio`
    /// guarding the whole method on `conn.is_qemu()` first. This is the
    /// exact behavior this function's own doc comment flags as the
    /// large, deliberate diff wiring it in will cause: every
    /// `test:///`-based existing test builds through a connection where
    /// this is `false`.
    #[test]
    fn non_qemu_connection_never_gets_virtio_even_if_the_os_supports_it() {
        let caps = GuestCapsDefaults {
            domain_type: "test",
            os_type: "hvm",
            arch: "x86_64",
            machine: None,
            emulator: None,
            is_qemu: false,
        };
        assert_eq!(default_disk_bus(caps, false, false, true, true), "ide");
    }

    /// `is_xenpv`/`!is_hvm` both short-circuit before virtio (or
    /// anything else) is ever considered — the disk device on a
    /// paravirtualized Xen guest, or any non-`hvm` `os_type`, never
    /// gets a virtio bus regardless of what the OS or arch would
    /// otherwise support.
    #[test]
    fn xenpv_and_non_hvm_short_circuit_before_virtio() {
        let xenpv = GuestCapsDefaults {
            domain_type: "xen",
            os_type: "linux",
            arch: "x86_64",
            machine: None,
            emulator: None,
            is_qemu: false,
        };
        assert_eq!(default_disk_bus(xenpv, false, false, true, true), "xen");

        // "exe" (a container os_type) is neither "hvm" nor one of the
        // xenpv-family os_types -- picked specifically so this
        // exercises the `!is_hvm` branch, not `is_xenpv` again.
        let non_hvm = GuestCapsDefaults {
            domain_type: "lxc",
            os_type: "exe",
            arch: "x86_64",
            machine: None,
            emulator: None,
            is_qemu: false,
        };
        assert_eq!(default_disk_bus(non_hvm, false, false, true, true), "ide");
    }

    /// `is_bhyve` is a plain argument here (not derived from
    /// `domain_type`, see this module's doc comment on why) — proven
    /// by setting it `true` on an otherwise-plain-x86-without-q35 caps
    /// value and confirming `sata` wins over the `is_x86` → `ide`
    /// branch that would otherwise apply.
    #[test]
    fn is_bhyve_flag_picks_sata_ahead_of_the_plain_x86_ide_default() {
        let caps = GuestCapsDefaults {
            domain_type: "bhyve",
            os_type: "hvm",
            arch: "x86_64",
            machine: Some("pc-i440fx-8.2"),
            emulator: None,
            is_qemu: false,
        };
        assert_eq!(default_disk_bus(caps, false, true, false, false), "sata");
    }

    #[test]
    fn target_prefix_matches_upstreams_bus_to_prefix_table() {
        assert_eq!(target_prefix_for_bus("virtio"), "vd");
        assert_eq!(target_prefix_for_bus("xen"), "xvd");
        assert_eq!(target_prefix_for_bus("fdc"), "fd");
        assert_eq!(target_prefix_for_bus("ide"), "hd");
        assert_eq!(target_prefix_for_bus("sata"), "sd");
        assert_eq!(target_prefix_for_bus("scsi"), "sd");
        assert_eq!(target_prefix_for_bus("usb"), "sd");
    }

    /// The common qemu/KVM x86 case with an OS that reports virtio-net
    /// support — the `virtio` branch wins even on a q35 machine,
    /// mirroring `default_disk_bus`'s own "virtio checked before q35"
    /// ordering proof.
    #[test]
    fn qemu_x86_q35_with_virtionet_support_picks_virtio() {
        let caps = GuestCapsDefaults {
            domain_type: "kvm",
            os_type: "hvm",
            arch: "x86_64",
            machine: Some("pc-q35-8.2"),
            emulator: None,
            is_qemu: true,
        };
        assert_eq!(default_nic_model(caps, true, &[]), Some("virtio"));
    }

    /// q35 without virtio-net support falls to `e1000e`, not the
    /// `prefs` fallback chain — upstream checks `is_q35()` before
    /// `is_x86()`'s fallback-chain branch, so q35 never reaches the
    /// `prefs` loop at all regardless of what `supported_netmodels`
    /// contains.
    #[test]
    fn q35_without_virtionet_support_picks_e1000e_regardless_of_supported_netmodels() {
        let caps = GuestCapsDefaults {
            domain_type: "kvm",
            os_type: "hvm",
            arch: "x86_64",
            machine: Some("pc-q35-8.2"),
            emulator: None,
            is_qemu: true,
        };
        let netmodels = vec!["rtl8139".to_string()];
        assert_eq!(default_nic_model(caps, false, &netmodels), Some("e1000e"));
    }

    /// Plain (non-q35) x86 without virtio-net support walks `prefs` in
    /// order and returns the first one `supported_netmodels` actually
    /// contains — not the first entry in `supported_netmodels` itself.
    #[test]
    fn plain_x86_without_virtionet_support_walks_prefs_in_order() {
        let caps = GuestCapsDefaults {
            domain_type: "kvm",
            os_type: "hvm",
            arch: "x86_64",
            machine: Some("pc-i440fx-8.2"),
            emulator: None,
            is_qemu: true,
        };
        // rtl8139 comes first in this list but e1000 is earlier in
        // `PREFS` -- proves the fixed preference order wins, not
        // input order.
        let netmodels = vec!["rtl8139".to_string(), "e1000".to_string()];
        assert_eq!(default_nic_model(caps, false, &netmodels), Some("e1000"));
    }

    /// None of `PREFS` present in `supported_netmodels` (e.g. no
    /// `--os-variant` given, so it's empty) still returns `e1000` —
    /// upstream's own unconditional fallback after the loop, not a
    /// `None`.
    #[test]
    fn plain_x86_with_no_matching_netmodels_still_falls_back_to_e1000() {
        let caps = GuestCapsDefaults {
            domain_type: "kvm",
            os_type: "hvm",
            arch: "x86_64",
            machine: Some("pc-i440fx-8.2"),
            emulator: None,
            is_qemu: true,
        };
        assert_eq!(default_nic_model(caps, false, &[]), Some("e1000"));
    }

    /// Non-x86, non-virtio-capable arch with no q35 gets `None` — no
    /// `<model>` element at all, matching upstream's `self.model =
    /// None` rather than a guessed constant.
    #[test]
    fn non_x86_without_virtio_support_and_no_q35_gets_no_model() {
        let caps = GuestCapsDefaults {
            domain_type: "kvm",
            os_type: "hvm",
            arch: "aarch64",
            machine: Some("virt"),
            emulator: None,
            is_qemu: true,
        };
        assert_eq!(default_nic_model(caps, false, &[]), None);
    }

    /// `!is_hvm` short-circuits before virtio/q35/x86 are ever
    /// considered, same as `default_disk_bus`'s own `!is_hvm` branch.
    #[test]
    fn non_hvm_gets_no_model_regardless_of_virtio_support() {
        let caps = GuestCapsDefaults {
            domain_type: "lxc",
            os_type: "exe",
            arch: "x86_64",
            machine: None,
            emulator: None,
            is_qemu: true,
        };
        assert_eq!(default_nic_model(caps, true, &[]), None);
    }

    /// `!is_qemu` overrides a genuinely virtio-capable OS on x86,
    /// falling to the `prefs` chain instead — same `supports_virtio`
    /// gate `default_disk_bus`'s own `non_qemu_connection_never_gets_
    /// virtio_even_if_the_os_supports_it` test proves.
    #[test]
    fn non_qemu_connection_never_gets_virtio_for_nic_model_either() {
        let caps = GuestCapsDefaults {
            domain_type: "test",
            os_type: "hvm",
            arch: "x86_64",
            machine: None,
            emulator: None,
            is_qemu: false,
        };
        assert_eq!(default_nic_model(caps, true, &[]), Some("e1000"));
    }

    /// x86+qemu defers entirely to `OsInfo` -- `true`/`false` both
    /// pass straight through, no arch/machine override in play.
    #[test]
    fn x86_qemu_defers_entirely_to_requires_firmware_efi() {
        let caps = GuestCapsDefaults {
            domain_type: "kvm",
            os_type: "hvm",
            arch: "x86_64",
            machine: None,
            emulator: None,
            is_qemu: true,
        };
        assert!(prefers_uefi(caps, false, false, true));
        assert!(!prefers_uefi(caps, false, false, false));
    }

    /// x86+`test:///` behaves the same as x86+qemu -- upstream's own
    /// `or` explicitly includes `conn.is_test()`, not just qemu.
    #[test]
    fn x86_test_driver_also_defers_to_requires_firmware_efi() {
        let caps = GuestCapsDefaults {
            domain_type: "test",
            os_type: "hvm",
            arch: "x86_64",
            machine: None,
            emulator: None,
            is_qemu: false,
        };
        assert!(prefers_uefi(caps, true, false, true));
    }

    /// x86 on neither qemu nor test (e.g. a real Xen connection) never
    /// consults `OsInfo` at all -- falls to the second branch, which
    /// has no x86 case, so `false` regardless of `requires_firmware_efi`.
    #[test]
    fn x86_on_neither_qemu_nor_test_never_consults_osinfo() {
        let caps = GuestCapsDefaults {
            domain_type: "xen",
            os_type: "hvm",
            arch: "x86_64",
            machine: None,
            emulator: None,
            is_qemu: false,
        };
        assert!(
            !prefers_uefi(caps, false, false, true),
            "requires_firmware_efi=true must not leak through on a non-qemu/test x86 connection"
        );
    }

    /// arm `machvirt` always prefers UEFI, `OsInfo` never consulted --
    /// proven by passing `requires_firmware_efi=false` and still
    /// getting `true`.
    #[test]
    fn arm_machvirt_always_prefers_uefi_regardless_of_osinfo() {
        let caps = GuestCapsDefaults {
            domain_type: "kvm",
            os_type: "hvm",
            arch: "aarch64",
            machine: Some("virt-8.2"),
            emulator: None,
            is_qemu: true,
        };
        assert!(prefers_uefi(caps, false, false, false));
    }

    /// `is_bhyve` wins in the second branch the same way it does for
    /// `default_disk_bus` -- a plain URI-scheme signal, not derived
    /// from `caps.domain_type`.
    #[test]
    fn is_bhyve_prefers_uefi_regardless_of_arch_or_osinfo() {
        let caps = GuestCapsDefaults {
            domain_type: "bhyve",
            os_type: "hvm",
            arch: "x86_64",
            machine: None,
            emulator: None,
            is_qemu: false,
        };
        // x86 without qemu/test falls to the second branch, where
        // is_bhyve applies.
        assert!(prefers_uefi(caps, false, true, false));
    }

    /// A plain x86 `pc-i440fx` qemu guest with no EFI requirement is
    /// the common case this crate actually targets -- `false`.
    #[test]
    fn plain_x86_qemu_guest_with_no_efi_requirement_does_not_prefer_uefi() {
        let caps = GuestCapsDefaults {
            domain_type: "kvm",
            os_type: "hvm",
            arch: "x86_64",
            machine: Some("pc-i440fx-8.2"),
            emulator: None,
            is_qemu: true,
        };
        assert!(!prefers_uefi(caps, false, false, false));
    }
}
