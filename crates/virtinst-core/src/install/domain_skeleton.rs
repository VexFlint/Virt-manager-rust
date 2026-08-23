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
//! once a third caller needed them (`push_virtio_disk`,
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

/// `<disk type="file" device="disk"><source file=.../><target dev="vda"
/// bus="virtio"/></disk>`, pushed onto `devices_el` — the fixed-defaults
/// persistent disk shape shared by `import.rs`, `iso.rs`'s two-phase
/// case, and `manual.rs` (real driver/OS-aware bus selection isn't
/// ported, see `import.rs`'s doc comment for the gap). Moved here once
/// `iso.rs` and `manual.rs` became the second and third copies.
pub(super) fn push_virtio_disk(doc: &mut Document, devices_el: Element, path: &str) {
    let disk_el = Element::new(doc, "disk");
    disk_el.set_attribute(doc, "type", "file");
    disk_el.set_attribute(doc, "device", "disk");
    let source_el = Element::new(doc, "source");
    source_el.set_attribute(doc, "file", path);
    disk_el
        .push_child(doc, source_el)
        .expect("a freshly-created detached element always attaches cleanly");
    let target_el = Element::new(doc, "target");
    target_el.set_attribute(doc, "dev", "vda");
    target_el.set_attribute(doc, "bus", "virtio");
    disk_el
        .push_child(doc, target_el)
        .expect("a freshly-created detached element always attaches cleanly");
    devices_el
        .push_child(doc, disk_el)
        .expect("a freshly-created detached element always attaches cleanly");
}

/// `<interface type="network"><source network="default"/><mac
/// address=mac/><model type="virtio"/></interface>`, pushed onto
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
/// Model is a fixed `virtio`
/// default (`DeviceInterface.default_model`: `hvm` + `guest.
/// supports_virtionet()`, the common qemu/kvm x86 case) — the non-virtio
/// fallback chain (`e1000`/`rtl8139`/`ne2k_pci`/`pcnet`, chosen via
/// `guest.osinfo.supported_netmodels()`) needs `OsInfo`, unmodeled, same
/// posture as this crate's disk-bus default.
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

pub(super) fn push_default_nic(doc: &mut Document, devices_el: Element, mac: &str) {
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
    let model_el = Element::new(doc, "model");
    model_el.set_attribute(doc, "type", "virtio");
    iface_el
        .push_child(doc, model_el)
        .expect("a freshly-created detached element always attaches cleanly");
    devices_el
        .push_child(doc, iface_el)
        .expect("a freshly-created detached element always attaches cleanly");
}
