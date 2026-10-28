//! `<domainCapabilities>` — upstream's `domcapabilities.py`. A
//! lightweight deserialize-only path, deliberately separate from
//! ticket 06's order-preserving derive-macro DOM (ticket 17's call):
//! this is fetched fresh per-connection, never edited or serialized
//! back, so none of the derive macro's `write_to`/round-trip-
//! preservation machinery earns its keep here. Still reuses
//! `edit_xml`'s parser (already a dependency) for the actual walk —
//! just hand-written `from_element` functions, no `#[derive(XmlBound)]`,
//! no `TAG` contract.
//!
//! **Scoped to what ticket 17 actually needs this for** — the
//! per-driver device-default chains (disk bus, video model, etc.) —
//! not the whole 523-line upstream file. Built: the generic
//! `<enum>`/`<value>` reader (`_CapsBlock`/`_Enum` upstream),
//! `DomainCapabilities`' own scalars, and its `devices` block (the 9
//! `_CapsBlock`-shaped device-capability sections upstream's own
//! `_Devices` class binds — `hostdev`/`disk`/`video`/`graphics`/`tpm`/
//! `filesystem`/`redirdev`/`channel`/`panic`). Real libvirt output
//! (checked against a live `qemu:///system` on this machine) also has
//! `crypto`/`interface`/`console` blocks newer than upstream's own
//! class — not modeled, matching upstream's own gap rather than
//! exceeding it.
//!
//! **`os` built as of the UEFI-firmware-defaults slice, `features`
//! still not built**: `_OS` upstream layers a nested `loader` sub-
//! block plus a plain `firmware`/`firmwareFeatures`/`varstore` set of
//! `_CapsBlock`s on top of its own `supported` flag — only the plain
//! top-level `supported`/`<enum name="firmware">` shape this crate's
//! one real caller (`supports_firmware_efi`) needs was modeled, reusing
//! `CapsBlock` as-is rather than a dedicated `Os` struct — `<loader>`'s
//! own nested shape (needed for the JSON-firmware-descriptor loader-
//! path fallback, a distinct subsystem, see `domain_skeleton::
//! prefers_uefi`'s doc comment) stays unbuilt, same "close the gap
//! when it's real" pattern. `_Features` (`_SEV`, etc.) is unrelated and
//! still not built — added when ticket 17's per-driver chains actually
//! need one.
//!
//! **`cpu` built as of ticket 21 slice (f)**: `_CPU` is a list of
//! `<mode>`s (`Cpu`/`CpuMode` below), each carrying its own `_CapsBlock`
//! shape (`supported`/`enums`, reused as plain fields rather than
//! inheritance — Rust has no `_CapsBlock` base class to extend) plus a
//! list of `<model>` children (`CpuModel`). Real libvirt output on this
//! machine's own `qemu:///session` capture also has `<vendor>`/
//! `<maxphysaddr>`/`<feature>` elements directly under `<mode
//! name='host-model'>`, and `vendor`/`canonical` attributes on `custom`
//! mode's `<model>` entries — none bound here, matching upstream's own
//! `_CPUMode`/`_CPUModel` classes exactly rather than exceeding them
//! (same posture as the `crypto`/`interface`/`console` gap already
//! noted above).

use crate::connection::Connection;
use virt::error::Error as LibvirtError;
use virtinst_xml::{parse_libvirt_xml, Document, Element, Result as XmlResult};

/// Either the libvirt call or the XML parse can fail, for different
/// reasons — same small bridging-enum shape `connection::domain`'s
/// `DomainError` already established for the same situation. `Io`
/// added for ticket 20's test-mode injection (`VIRTINST_TEST_CAPS_
/// FILE`/`VIRTINST_TEST_DOMCAPS_FILE`) — reading a caller-supplied
/// fixture path can fail (typo, missing file) same as any other file
/// read, and should surface as a normal `Err` rather than panicking.
#[derive(Debug)]
pub enum CapabilitiesError {
    Libvirt(LibvirtError),
    Xml(virtinst_xml::EditXMLError),
    Io(std::io::Error),
}

impl std::fmt::Display for CapabilitiesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CapabilitiesError::Libvirt(e) => write!(f, "{e}"),
            CapabilitiesError::Xml(e) => write!(f, "{e}"),
            CapabilitiesError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CapabilitiesError {}

impl From<LibvirtError> for CapabilitiesError {
    fn from(e: LibvirtError) -> Self {
        CapabilitiesError::Libvirt(e)
    }
}

impl From<virtinst_xml::EditXMLError> for CapabilitiesError {
    fn from(e: virtinst_xml::EditXMLError) -> Self {
        CapabilitiesError::Xml(e)
    }
}

impl From<std::io::Error> for CapabilitiesError {
    fn from(e: std::io::Error) -> Self {
        CapabilitiesError::Io(e)
    }
}

/// `<enum name="...">` — a named list of allowed values, e.g.
/// `<enum name="bus"><value>ide</value><value>virtio</value></enum>`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapsEnum {
    pub name: Option<String>,
    pub values: Vec<String>,
}

impl CapsEnum {
    pub fn has_value(&self, val: &str) -> bool {
        self.values.iter().any(|v| v == val)
    }

    fn from_element(doc: &Document, el: Element) -> CapsEnum {
        CapsEnum {
            name: el.attribute(doc, "name").map(str::to_string),
            values: read_values(doc, el),
        }
    }
}

/// One `supported="yes"/"no"` capability block — upstream's
/// `_CapsBlock`. `values` are direct `<value>` children (distinct from
/// each `<enum>`'s own values) — real in upstream's `<loader>` block, a
/// default path sitting alongside its `type`/`readonly`/`secure` enums.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapsBlock {
    pub supported: Option<bool>,
    pub values: Vec<String>,
    pub enums: Vec<CapsEnum>,
}

impl CapsBlock {
    pub fn has_enum(&self, name: &str) -> bool {
        self.get_enum(name).is_some()
    }

    pub fn get_enum(&self, name: &str) -> Option<&CapsEnum> {
        self.enums.iter().find(|e| e.name.as_deref() == Some(name))
    }

    /// Absent block (the tag doesn't exist at all in this driver's
    /// output) reads the same as `CapsBlock::default()` — `supported`
    /// stays `None`, same "missing reads as absent" rule every field
    /// kind in this crate already follows.
    fn from_element(doc: &Document, el: Element) -> CapsBlock {
        CapsBlock {
            supported: el.attribute(doc, "supported").map(|s| s == "yes"),
            values: read_values(doc, el),
            enums: el
                .find_all(doc, "enum")
                .into_iter()
                .map(|e| CapsEnum::from_element(doc, e))
                .collect(),
        }
    }

    fn from_optional_child(doc: &Document, parent: Element, tag: &str) -> CapsBlock {
        match parent.find(doc, tag) {
            Some(el) => CapsBlock::from_element(doc, el),
            None => CapsBlock::default(),
        }
    }
}

fn read_values(doc: &Document, el: Element) -> Vec<String> {
    el.find_all(doc, "value")
        .into_iter()
        .map(|v| v.text_content(doc))
        .collect()
}

/// `<domainCapabilities><devices>` — upstream's `_Devices`. One
/// `CapsBlock` per device type upstream actually models.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DomainCapabilitiesDevices {
    pub hostdev: CapsBlock,
    pub disk: CapsBlock,
    pub video: CapsBlock,
    pub graphics: CapsBlock,
    pub tpm: CapsBlock,
    pub filesystem: CapsBlock,
    pub redirdev: CapsBlock,
    pub channel: CapsBlock,
    pub panic: CapsBlock,
}

impl DomainCapabilitiesDevices {
    fn from_element(doc: &Document, el: Element) -> DomainCapabilitiesDevices {
        DomainCapabilitiesDevices {
            hostdev: CapsBlock::from_optional_child(doc, el, "hostdev"),
            disk: CapsBlock::from_optional_child(doc, el, "disk"),
            video: CapsBlock::from_optional_child(doc, el, "video"),
            graphics: CapsBlock::from_optional_child(doc, el, "graphics"),
            tpm: CapsBlock::from_optional_child(doc, el, "tpm"),
            filesystem: CapsBlock::from_optional_child(doc, el, "filesystem"),
            redirdev: CapsBlock::from_optional_child(doc, el, "redirdev"),
            channel: CapsBlock::from_optional_child(doc, el, "channel"),
            panic: CapsBlock::from_optional_child(doc, el, "panic"),
        }
    }
}

/// `<model>` under a `<cpu><mode>` — upstream's `_CPUModel`. `model` is
/// the element's own text content (`XMLProperty(".")`), not an
/// attribute — the one case this struct needs `text_content` rather
/// than `attribute` for its primary value, same shape `net_interface`
/// already needed in `devices/hostdev.rs`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CpuModel {
    pub model: Option<String>,
    pub usable: Option<String>,
    pub fallback: Option<String>,
}

impl CpuModel {
    fn from_element(doc: &Document, el: Element) -> CpuModel {
        CpuModel {
            model: Some(el.text_content(doc)),
            usable: el.attribute(doc, "usable").map(str::to_string),
            fallback: el.attribute(doc, "fallback").map(str::to_string),
        }
    }
}

/// `<mode>` under `<domainCapabilities><cpu>` — upstream's `_CPUMode`,
/// which extends `_CapsBlock` (`supported`/`values`/`enums`) rather
/// than composing it; Rust has no base class to extend, so
/// `supported`/`enums` are plain fields here instead (`values` omitted
/// — no real `<cpu><mode>` block uses direct `<value>` children the
/// way `<loader>` does, unlike every other `CapsBlock`-shaped block in
/// this file).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CpuMode {
    pub name: Option<String>,
    pub supported: Option<bool>,
    pub enums: Vec<CapsEnum>,
    pub models: Vec<CpuModel>,
}

impl CpuMode {
    pub fn get_enum(&self, name: &str) -> Option<&CapsEnum> {
        self.enums.iter().find(|e| e.name.as_deref() == Some(name))
    }

    pub fn get_model(&self, name: &str) -> Option<&CpuModel> {
        self.models
            .iter()
            .find(|m| m.model.as_deref() == Some(name))
    }

    fn from_element(doc: &Document, el: Element) -> CpuMode {
        CpuMode {
            name: el.attribute(doc, "name").map(str::to_string),
            supported: el.attribute(doc, "supported").map(|s| s == "yes"),
            enums: el
                .find_all(doc, "enum")
                .into_iter()
                .map(|e| CapsEnum::from_element(doc, e))
                .collect(),
            models: el
                .find_all(doc, "model")
                .into_iter()
                .map(|m| CpuModel::from_element(doc, m))
                .collect(),
        }
    }
}

/// `<domainCapabilities><cpu>` — upstream's `_CPU`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cpu {
    pub modes: Vec<CpuMode>,
}

impl Cpu {
    pub fn get_mode(&self, name: &str) -> Option<&CpuMode> {
        self.modes.iter().find(|m| m.name.as_deref() == Some(name))
    }

    fn from_element(doc: &Document, el: Element) -> Cpu {
        Cpu {
            modes: el
                .find_all(doc, "mode")
                .into_iter()
                .map(|m| CpuMode::from_element(doc, m))
                .collect(),
        }
    }
}

/// ```xml
/// <domainCapabilities>
///   <path>/usr/bin/qemu-system-x86_64</path>
///   <domain>kvm</domain>
///   <machine>pc-i440fx-resolute</machine>
///   <arch>x86_64</arch>
///   <os supported="yes">
///     <enum name="firmware"><value>efi</value></enum>
///   </os>
///   <devices>
///     <disk supported="yes">
///       <enum name="bus"><value>ide</value><value>virtio</value></enum>
///     </disk>
///   </devices>
/// </domainCapabilities>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DomainCapabilities {
    pub path: Option<String>,
    pub domain: Option<String>,
    pub machine: Option<String>,
    pub arch: Option<String>,
    /// `<domainCapabilities><os>` — a sibling of `<devices>`, not
    /// nested inside it (unlike `disk`/`graphics`/etc.), but the exact
    /// same `supported="yes"` + `<enum>` shape `CapsBlock` already
    /// models, so no new struct was needed, just a field at the right
    /// level.
    pub os: CapsBlock,
    /// `<os><loader>`.
    pub loader: CapsBlock,
    pub devices: DomainCapabilitiesDevices,
    pub cpu: Cpu,
    /// `<memoryBacking>` — upstream's `_MemoryBacking`.
    pub memorybacking: CapsBlock,
    /// `<features><hyperv>`.
    pub hyperv: CapsBlock,
    /// `_convert_mode_to_cpu` for the supported `host-model` mode: a
    /// `<cpu>` document to expand with `baselineHypervisorCPU`, kept
    /// at parse time since the mode's raw children aren't modeled.
    pub host_model_cpu_xml: Option<String>,
    /// `<features><sev supported=...>` — upstream's `_SEV.supported`.
    pub sev_supported: bool,
    /// `<features><sev><maxESGuests>`.
    pub sev_max_es_guests: Option<String>,
}

/// `<domainCapabilities><features><sev>`.
fn sev(doc: &Document, root: Element) -> Option<Element> {
    root.find(doc, "features")?.find(doc, "sev")
}

impl DomainCapabilities {
    /// `supports_sev_launch_security`: False if either libvirt doesn't
    /// advertise support for SEV at all (< libvirt-4.5.0) or if it
    /// explicitly advertises it as unsupported on the platform
    pub fn supports_sev_launch_security(&self, check_es: bool) -> bool {
        if check_es {
            return self.sev_supported
                && self
                    .sev_max_es_guests
                    .as_deref()
                    .is_some_and(|v| !v.is_empty());
        }
        self.sev_supported
    }

    /// Parses already-fetched `domainCapabilities` XML — split from
    /// [`fetch`](DomainCapabilities::fetch) so a captured real fixture
    /// can be tested without a live connection.
    pub fn parse(xml: &str) -> XmlResult<DomainCapabilities> {
        let doc = parse_libvirt_xml(xml)?;
        let root = doc
            .root_element()
            .expect("non-empty domainCapabilities XML always has a root element");

        Ok(DomainCapabilities {
            path: root.find(&doc, "path").map(|e| e.text_content(&doc)),
            domain: root.find(&doc, "domain").map(|e| e.text_content(&doc)),
            machine: root.find(&doc, "machine").map(|e| e.text_content(&doc)),
            arch: root.find(&doc, "arch").map(|e| e.text_content(&doc)),
            os: CapsBlock::from_optional_child(&doc, root, "os"),
            loader: match root.find(&doc, "os") {
                Some(el) => CapsBlock::from_optional_child(&doc, el, "loader"),
                None => CapsBlock::default(),
            },
            devices: match root.find(&doc, "devices") {
                Some(el) => DomainCapabilitiesDevices::from_element(&doc, el),
                None => DomainCapabilitiesDevices::default(),
            },
            cpu: match root.find(&doc, "cpu") {
                Some(el) => Cpu::from_element(&doc, el),
                None => Cpu::default(),
            },
            memorybacking: CapsBlock::from_optional_child(&doc, root, "memoryBacking"),
            hyperv: match root.find(&doc, "features") {
                Some(el) => CapsBlock::from_optional_child(&doc, el, "hyperv"),
                None => CapsBlock::default(),
            },
            host_model_cpu_xml: host_model_cpu_xml(&doc, root),
            sev_supported: sev(&doc, root)
                .and_then(|e| e.attribute(&doc, "supported"))
                .is_some_and(|v| v == "yes"),
            sev_max_es_guests: sev(&doc, root)
                .and_then(|e| e.find(&doc, "maxESGuests"))
                .map(|e| e.text_content(&doc)),
        })
    }

    /// Fetches and parses in one call — upstream's
    /// `DomainCapabilities.build_from_params`, minus the
    /// `conn.support.conn_domain_capabilities()` pre-check (this
    /// crate's `connection::Connection` doesn't have a support-cache
    /// yet — see `connection.rs`'s doc comment for why; a call against
    /// a driver too old to support this just surfaces as a normal
    /// `Err` here instead).
    ///
    /// Checks `VIRTINST_TEST_DOMCAPS_FILE` first (ticket 20's test-mode
    /// injection, same precedent as [`Capabilities::fetch`](super::caps::Capabilities::fetch)'s
    /// `VIRTINST_TEST_CAPS_FILE` — see `caps.rs`'s doc comment for the
    /// full rationale) — reads and parses that file instead of making
    /// the real libvirt call when set. Every one of this function's
    /// existing callers (`import.rs`/`container.rs`/`iso.rs`/
    /// `manual.rs`) needs no changes at all; the gate lives here once.
    pub fn fetch(
        conn: &Connection,
        emulator: Option<&str>,
        arch: Option<&str>,
        machine: Option<&str>,
        virttype: Option<&str>,
    ) -> Result<DomainCapabilities, CapabilitiesError> {
        DomainCapabilities::fetch_from(conn.as_connect(), emulator, arch, machine, virttype)
    }

    /// [`DomainCapabilities::fetch`] on a bare libvirt connection.
    pub fn fetch_from(
        conn: &virt::connect::Connect,
        emulator: Option<&str>,
        arch: Option<&str>,
        machine: Option<&str>,
        virttype: Option<&str>,
    ) -> Result<DomainCapabilities, CapabilitiesError> {
        if let Some(path) = std::env::var_os("VIRTINST_TEST_DOMCAPS_FILE") {
            let xml = std::fs::read_to_string(&path)?;
            return Ok(DomainCapabilities::parse(&xml)?);
        }
        let xml = conn.get_domain_capabilities(emulator, arch, machine, virttype, 0)?;
        Ok(DomainCapabilities::parse(&xml)?)
    }

    /// Upstream's `supports_graphics_spice` (`domcapabilities.py:464`) —
    /// the first `supports_*` query method ported for this struct, since
    /// ticket 21 slice (c) is its first real caller. This struct has no
    /// connection reference of its own (unlike upstream's `DomainCapabilities`,
    /// which keeps `self.conn` from construction) — `fetch`'s only real
    /// use for a connection is making the libvirt call itself (see its
    /// own doc comment) — but `Uri::parse` is a pure, connection-free
    /// parser (no libvirt call, no I/O), so taking `&Uri` directly costs
    /// nothing and closes off a real risk a first draft of this function
    /// had: two same-typed `bool`s (`is_qemu`/`is_test`) are exactly the
    /// "transposition compiles, no test catches it" shape the NIC-OUI
    /// bug (see `push_default_nic`'s doc comment) already hit once for
    /// real. `host_arch` is the *host's* CPU arch (upstream's `self.conn
    /// .caps.host.cpu.arch`) — deliberately not this struct's own `arch`
    /// field, which is the *domain's* target arch instead — plain
    /// `Capabilities` has no `Uri`-shaped wrapper to fold it into, so it
    /// stays a separate parameter.
    pub fn supports_graphics_spice(
        &self,
        uri: &crate::connection::Uri,
        host_arch: Option<&str>,
    ) -> bool {
        if self.devices.graphics.supported != Some(true) {
            // domcaps is too old, or the driver doesn't advertise
            // graphics support at all. Use the pre-domcaps heuristic
            // upstream still carries for that case.
            if !uri.is_qemu() && !uri.is_test() {
                return false;
            }
            return matches!(host_arch, Some("i686") | Some("x86_64"));
        }
        self.devices
            .graphics
            .get_enum("type")
            .is_some_and(|e| e.has_value("spice"))
    }

    /// Upstream's `domcapabilities.py`'s `supports_firmware_efi` — the
    /// modern libvirt "firmware auto-select" capability
    /// (`<os supported="yes"><enum name="firmware"><value>efi</value>
    /// </enum></os>`), confirmed against this machine's own real
    /// `qemu:///session` capture (`REAL_QEMU_OS_FIRMWARE_EXCERPT`
    /// below). When `true`, `<os firmware="efi">` alone is enough to
    /// get a UEFI guest — no `<loader>`/`<nvram>` path resolution
    /// needed at all, upstream's own fast path in `enable_uefi()`.
    /// When `false` (older libvirt, or a driver that never gained this
    /// capability), upstream falls back to `_lookup_default_uefi_path()`
    /// — a real filesystem scan over JSON firmware-descriptor files
    /// this crate doesn't model (a distinct subsystem, not a query
    /// method away like `default_disk_bus`/`default_nic_model` were),
    /// so this crate can only ever enable UEFI through this fast path.
    pub fn supports_firmware_efi(&self) -> bool {
        self.os
            .get_enum("firmware")
            .is_some_and(|e| e.has_value("efi"))
    }

    /// Upstream's `DeviceGraphics._default_type` (`graphics.py:181`).
    /// `DEFAULT_GRAPHICS_TYPE` stands in for `BuildConfig.default_graphics`
    /// (`meson_options.txt`'s `default-graphics` option, shipped default
    /// `"spice"`) — this crate has no equivalent build-time config, so a
    /// fixed constant matching upstream's own shipped default.
    pub fn default_graphics_type(
        &self,
        uri: &crate::connection::Uri,
        host_arch: Option<&str>,
    ) -> &'static str {
        const DEFAULT_GRAPHICS_TYPE: &str = "spice";
        if uri.is_xen() {
            // Xen domcaps can advertise spice support, but upstream has
            // historically not defaulted to it for xen — forced to vnc
            // unconditionally.
            return "vnc";
        }
        if DEFAULT_GRAPHICS_TYPE == "spice" && !self.supports_graphics_spice(uri, host_arch) {
            return "vnc";
        }
        DEFAULT_GRAPHICS_TYPE
    }

    /// Ticket 21 slice (f): upstream's `supports_safe_host_model`
    /// (`domapabilities.py:374`). Uses `.first()` rather than upstream's
    /// bare `m.models[0]` — a real, deliberate divergence: upstream's
    /// Python would raise `IndexError` on an advertised-but-modelless
    /// `host-model` block, whereas this returns `false` instead of
    /// panicking. Confirmed against this machine's own real
    /// `qemu:///session` capture that a real `host-model` block always
    /// has exactly one `<model>` child, so this is a defensive
    /// divergence for malformed input, not a behavior change on any
    /// real driver output.
    pub fn supports_safe_host_model(&self) -> bool {
        let Some(mode) = self.cpu.get_mode("host-model") else {
            return false;
        };
        mode.supported == Some(true)
            && mode
                .models
                .first()
                .is_some_and(|m| m.fallback.as_deref() == Some("forbid"))
    }

    /// Upstream's `supports_safe_host_passthrough`
    /// (`domcapabilities.py:381`).
    pub fn supports_safe_host_passthrough(&self) -> bool {
        let Some(mode) = self.cpu.get_mode("host-passthrough") else {
            return false;
        };
        mode.supported == Some(true)
            && mode
                .get_enum("hostPassthroughMigratable")
                .is_some_and(|e| e.has_value("on"))
    }

    /// Upstream's `supports_uefi_loader`.
    pub fn supports_uefi_loader(&self) -> bool {
        self.loader
            .get_enum("readonly")
            .is_some_and(|e| e.has_value("yes"))
    }

    /// Upstream's `arch_can_uefi`.
    pub fn arch_can_uefi(&self) -> bool {
        matches!(
            self.arch.as_deref(),
            Some("i686" | "x86_64" | "aarch64" | "armv7l" | "riscv64" | "loongarch64")
        )
    }

    /// Upstream's `find_uefi_path_for_arch`: the first loader path
    /// matching the arch's `_uefi_arch_patterns`, in pattern order.
    pub fn find_uefi_path_for_arch(&self, is_bhyve: bool) -> Option<String> {
        if !self.arch_can_uefi() {
            return None;
        }
        let files = &self.loader.values;
        if is_bhyve {
            return files
                .iter()
                .find(|f| f.contains("BHYVE_UEFI.fd"))
                .or(files.first())
                .cloned();
        }
        // The `re.match(".*X")` patterns, as substring checks.
        let after = |p: &str, pre: &str, post: &str| {
            p.find(pre)
                .is_some_and(|i| p[i + pre.len()..].contains(post))
        };
        type Pat = Box<dyn Fn(&str) -> bool>;
        let patterns: Vec<Pat> = match self.arch.as_deref() {
            Some("i686") => vec![
                Box::new(move |p| after(p, "edk2-i386-", ".fd")),
                Box::new(|p| p.contains("ovmf-ia32")),
            ],
            Some("x86_64") => vec![
                Box::new(move |p| after(p, "edk2-x86_64-", ".fd")),
                Box::new(|p| p.contains("OVMF_CODE.fd")),
                Box::new(move |p| after(p, "ovmf-x64/OVMF", ".fd")),
                Box::new(|p| p.contains("ovmf-x86_64-")),
                Box::new(|p| p.contains("ovmf")),
                Box::new(|p| p.contains("OVMF")),
            ],
            Some("aarch64") => vec![
                Box::new(|p| p.contains("AAVMF_CODE.fd")),
                Box::new(|p| p.contains("aarch64/QEMU_EFI")),
                Box::new(|p| p.contains("aarch64")),
                Box::new(|p| p.contains("edk2-aarch64-code.fd")),
            ],
            Some("armv7l") => vec![
                Box::new(|p| p.contains("AAVMF32_CODE.fd")),
                Box::new(|p| p.contains("arm/QEMU_EFI")),
                Box::new(|p| p.contains("edk2-arm-code.fd")),
            ],
            Some("riscv64") => vec![
                Box::new(|p| p.contains("RISCV_VIRT_CODE.")),
                Box::new(|p| p.contains("riscv64")),
            ],
            Some("loongarch64") => vec![
                Box::new(|p| p.contains("loongarch64/QEMU_CODE.")),
                Box::new(|p| p.contains("loongarch64")),
            ],
            _ => vec![],
        };
        patterns
            .iter()
            .find_map(|pat| files.iter().find(|f| pat(f)))
            .cloned()
    }

    /// Upstream's `supports_tpm_emulator`.
    pub fn supports_tpm_emulator(&self) -> bool {
        let values = |name: &str| {
            self.devices
                .tpm
                .get_enum(name)
                .map(|e| e.values.clone())
                .unwrap_or_default()
        };
        let models = values("model");
        if self.arch.as_deref() == Some("armv7l") && models == ["tpm-tis"] {
            // libvirt can advertise armv7l tpm-tis, then rejects it
            return false;
        }
        !models.is_empty() && values("backendModel").iter().any(|b| b == "emulator")
    }

    /// Upstream's `supports_channel_spicevmc`: only an explicit "no"
    /// from libvirt counts.
    pub fn supports_channel_spicevmc(&self) -> bool {
        let channel = &self.devices.channel;
        channel.supported.is_none()
            || channel
                .get_enum("type")
                .is_some_and(|e| e.has_value("spicevmc"))
    }

    /// Upstream's `supports_redirdev_usb`, same shape.
    pub fn supports_redirdev_usb(&self) -> bool {
        let redirdev = &self.devices.redirdev;
        redirdev.supported.is_none() || redirdev.get_enum("bus").is_some_and(|e| e.has_value("usb"))
    }

    /// Upstream's `supported_hyperv_features`.
    pub fn supported_hyperv_features(&self) -> Vec<String> {
        if self.hyperv.supported != Some(true) {
            return vec![];
        }
        self.hyperv
            .get_enum("features")
            .map(|e| e.values.clone())
            .unwrap_or_default()
    }

    /// Upstream's `get_cpu_models`: the usable custom models.
    pub fn get_cpu_models(&self) -> Vec<String> {
        self.cpu
            .modes
            .iter()
            .filter(|m| m.name.as_deref() == Some("custom") && m.supported == Some(true))
            .flat_map(|m| &m.models)
            .filter(|m| m.usable.as_deref() != Some("no"))
            .filter_map(|m| m.model.clone())
            .collect()
    }

    /// Upstream's `supports_filesystem_virtiofs`.
    pub fn supports_filesystem_virtiofs(&self) -> bool {
        self.devices
            .filesystem
            .get_enum("driverType")
            .is_some_and(|e| e.has_value("virtiofs"))
    }

    /// Upstream's `supports_memorybacking_memfd`.
    pub fn supports_memorybacking_memfd(&self) -> bool {
        self.memorybacking
            .get_enum("sourceType")
            .is_some_and(|e| e.has_value("memfd"))
    }

    /// `DomainCpu._get_app_default_mode` (with
    /// `_should_use_maximum_cpu_mode`), for a guest of `domain_type`
    /// on `arch`/`machine`.
    pub fn app_default_cpu_mode(
        &self,
        domain_type: Option<&str>,
        arch: Option<&str>,
        machine: Option<&str>,
    ) -> &'static str {
        let arch = arch.unwrap_or("");
        let machine = machine.unwrap_or("");
        let virt = machine.starts_with("virt");
        let arch_ok = matches!(arch, "x86_64" | "i686" | "loongarch64")
            || (matches!(arch, "armv7l" | "aarch64") && virt)
            || (matches!(arch, "riscv64" | "riscv32") && virt);
        if self.supports_maximum_cpu_mode() && domain_type == Some("qemu") && arch_ok {
            return "maximum";
        }
        if self.supports_safe_host_passthrough() {
            return "host-passthrough";
        }
        if self.supports_safe_host_model() {
            return "host-model";
        }
        "host-model-only"
    }

    /// Upstream's `supports_maximum_cpu_mode` (`domcapabilities.py:391`).
    pub fn supports_maximum_cpu_mode(&self) -> bool {
        self.cpu
            .get_mode("maximum")
            .is_some_and(|m| m.supported == Some(true))
    }

    /// Ticket 21 slice (f) piece 2: upstream's `_get_app_default_mode`/
    /// `set_special_mode`/`set_model` chain (`domain/cpu.py`),
    /// collapsed into "what would `DomainCpu.set_defaults()` put in
    /// the `<cpu>` element" — this crate builds domain XML directly
    /// (`domain_skeleton.rs`) rather than through `DomainCpu`'s bound-
    /// struct setters, so there's no `self.mode =`/`self.model =` to
    /// mutate; a resolved [`DefaultCpuMode`] (or `None` for "don't
    /// emit `<cpu>` at all") is the whole result.
    ///
    /// Scope, traced from upstream's real branches rather than assumed:
    /// - Top gate: `uri.is_test() || uri.is_qemu()` — upstream's
    ///   `self.conn.is_test() or self.conn.is_qemu()`. Anything else
    ///   (a real xen/vz connection) never gets a `<cpu>` default here.
    /// - `guest.os.is_arm_machvirt()`/`is_riscv_virt()`/
    ///   `is_loongarch64()` guest-type branches: **not modeled**, same
    ///   as everywhere else `is_x86()`-adjacent logic has come up in
    ///   this crate — no ARM/RISC-V/LoongArch guest support exists yet
    ///   to ground it against.
    /// - x86 + `domain_type == "kvm"`: upstream's `_set_cpu_x86_kvm_
    ///   default`. Requires `guest_arch == host_arch` (cross-arch
    ///   emulation gets no host-based default at all). Prefers
    ///   `host-passthrough`, then `host-model`, then falls back to a
    ///   custom model pinned to the host's own reported CPU (from
    ///   `<capabilities>`, not domcaps) — see [`DefaultCpuMode::Custom`]
    ///   for that branch's own wrinkles. `_should_use_maximum_cpu_mode`
    ///   cannot fire inside this branch: it requires `guest.type ==
    ///   "qemu"`, which this branch's own `domain_type == "kvm"` guard
    ///   already rules out — confirmed by reading the source, not
    ///   assumed, since it's what let this branch skip straight to
    ///   host-passthrough/host-model/custom with no maximum-mode check
    ///   at all.
    /// - x86 + `domain_type == "qemu"` (TCG, not KVM-accelerated):
    ///   upstream's `_should_use_maximum_cpu_mode` on the `else`
    ///   branch — `maximum` mode if domcaps advertises it, otherwise no
    ///   default at all (nothing better to offer on old libvirt/qemu).
    ///
    /// **A deliberate superset, not a straight port**: the custom-model
    /// fallback always validates the model against domcaps before
    /// emitting it (see [`DefaultCpuMode::Custom`]'s doc comment).
    /// Upstream only does this when `guest.x86_cpu_default ==
    /// HOST_MODEL_ONLY` — for `virt-install` specifically,
    /// `x86_cpu_default` is pinned to `APP_DEFAULT` (`guest.py:273`,
    /// never reassigned on that path) and `set_special_mode` only
    /// resolves `APP_DEFAULT` to an actual mode *after* the validation
    /// check already ran on the unresolved sentinel, so `_validate_
    /// default_host_model_only` is dead code on `virt-install`'s own
    /// CLI path — it only fires via the GUI's CPU-preference setting
    /// (`createvm.py:1390`) or a test override. Validating unconditionally
    /// here is deliberate, not an oversight: this crate has no `x86_cpu_
    /// default`-equivalent switch to gate it on, a 1:1 GUI port is this
    /// project's stated endpoint (where this exact behavior *is* live),
    /// and skipping validation to match virt-install's dead branch
    /// would just mean re-adding it later. A future `--cpu`/GUI CPU-
    /// preference flag is where the two paths' difference becomes
    /// observable.
    ///
    /// **Not modeled, stated rather than silently dropped**: the
    /// CPU-security-feature enrichment upstream applies on the custom-
    /// model path (`_add_security_features`/`get_cpu_security_
    /// features`), which needs `_get_expanded_cpu` — a live libvirt
    /// round-trip to expand a CPU model into its real feature list, a
    /// kind of capability query this crate doesn't have. Only matters
    /// on the `host-model-only` fallback, itself unreachable on any
    /// host (including the one this was grounded against) where
    /// host-passthrough or host-model is safely supported.
    pub fn default_cpu_mode(
        &self,
        uri: &crate::connection::Uri,
        domain_type: &str,
        guest_arch: &str,
        host_arch: &str,
        host_cpu_model: Option<&str>,
    ) -> Option<DefaultCpuMode> {
        if !uri.is_test() && !uri.is_qemu() {
            return None;
        }

        let is_x86 = guest_arch == "x86_64" || guest_arch == "i686";

        if is_x86 && domain_type == "kvm" {
            if guest_arch != host_arch {
                return None;
            }
            if self.supports_safe_host_passthrough() {
                return Some(DefaultCpuMode::Mode("host-passthrough"));
            }
            if self.supports_safe_host_model() {
                return Some(DefaultCpuMode::Mode("host-model"));
            }
            let model = host_cpu_model?;
            // Upstream's `_validate_default_host_model_only` logic,
            // applied unconditionally here rather than gated on
            // `x86_cpu_default == HOST_MODEL_ONLY` (dead on virt-
            // install's own CLI path, live on the GUI's CPU-preference
            // path — see this function's own doc comment for why
            // that's a deliberate superset, not a straight port). The
            // model `<capabilities>` reports as the host CPU may not
            // actually be usable per domcaps; if so, strip just the
            // model name and leave `mode="custom"` (with `match=
            // "exact"`) standing with no `<model>` child — a genuinely
            // odd emitted shape, but the one upstream's own GUI path
            // produces, replicated faithfully rather than tidied up.
            let usable = self
                .cpu
                .get_mode("custom")
                .and_then(|m| m.get_model(model))
                .and_then(|m| m.usable.as_deref());
            let model = if usable == Some("no") {
                None
            } else {
                Some(model.to_string())
            };
            return Some(DefaultCpuMode::Custom { model });
        }

        if domain_type == "qemu" && is_x86 && self.supports_maximum_cpu_mode() {
            return Some(DefaultCpuMode::Mode("maximum"));
        }

        None
    }
}

/// `_convert_mode_to_cpu` for the supported `host-model` mode.
fn host_model_cpu_xml(doc: &Document, root: Element) -> Option<String> {
    let mode = root
        .find(doc, "cpu")?
        .find_all(doc, "mode")
        .into_iter()
        .find(|m| {
            m.attribute(doc, "name") == Some("host-model")
                && m.attribute(doc, "supported") == Some("yes")
        })?;
    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    let mut xml = String::from("<cpu>");
    for child in mode.child_elements(doc) {
        let mut attrs: Vec<_> = child.attributes(doc).iter().collect();
        attrs.sort();
        xml.push('<');
        xml.push_str(child.name(doc));
        for (k, v) in attrs {
            xml.push_str(&format!(" {k}=\"{}\"", esc(v)));
        }
        xml.push_str(&format!(
            ">{}</{}>",
            esc(&child.text_content(doc)),
            child.name(doc)
        ));
    }
    let arch = root
        .find(doc, "arch")
        .map(|e| e.text_content(doc))
        .unwrap_or_default();
    xml.push_str(&format!("<arch>{}</arch></cpu>", esc(&arch)));
    Some(xml)
}

/// `_get_expanded_cpu`: `baselineHypervisorCPU` with
/// `EXPAND_FEATURES`, falling back to the older `baselineCPU`.
/// Blocking.
pub fn baseline_hypervisor_cpu(
    conn: &virt::connect::Connect,
    (emulator, arch, machine, virttype): (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ),
    cpu_xml: &str,
) -> Option<String> {
    use std::ffi::{CStr, CString};
    use std::os::raw::{c_char, c_void};
    extern "C" {
        fn free(p: *mut c_void);
    }
    let cstr = |s: Option<String>| s.and_then(|s| CString::new(s).ok());
    let (emulator, arch, machine, virttype) =
        (cstr(emulator), cstr(arch), cstr(machine), cstr(virttype));
    let ptr = |s: &Option<CString>| s.as_ref().map_or(std::ptr::null(), |s| s.as_ptr());
    let xml = CString::new(cpu_xml).ok()?;
    let mut xmls: [*const c_char; 1] = [xml.as_ptr()];
    // SAFETY: every pointer is a live NUL-terminated string or null,
    // which libvirt accepts for the optional parameters; the result
    // is a malloc'd string we own.
    let ret = unsafe {
        virt::sys::virConnectBaselineHypervisorCPU(
            conn.as_ptr(),
            ptr(&emulator),
            ptr(&arch),
            ptr(&machine),
            ptr(&virttype),
            xmls.as_mut_ptr(),
            1,
            virt::sys::VIR_CONNECT_BASELINE_CPU_EXPAND_FEATURES,
        )
    };
    if !ret.is_null() {
        // SAFETY: non-null result from libvirt, freed right after.
        let out = unsafe { CStr::from_ptr(ret) }
            .to_string_lossy()
            .into_owned();
        unsafe { free(ret as *mut c_void) };
        return Some(out);
    }
    conn.baseline_cpu(
        &[cpu_xml],
        virt::sys::VIR_CONNECT_BASELINE_CPU_EXPAND_FEATURES,
    )
    .ok()
}

impl DomainCapabilities {
    /// `get_cpu_security_features` (`_lookup_cpu_security_features`):
    /// the security features of the expanded host-model CPU. Blocking.
    pub fn lookup_cpu_security_features(&self, conn: &virt::connect::Connect) -> Vec<String> {
        let mut ret = vec![];
        if let Some(cpu_xml) = &self.host_model_cpu_xml {
            log::debug!("Generated CPU XML for security flag baseline:\n{cpu_xml}");
            let params = (
                self.path.clone(),
                self.arch.clone(),
                self.machine.clone(),
                self.domain.clone(),
            );
            match baseline_hypervisor_cpu(conn, params, cpu_xml) {
                Some(xml) => ret = cpu_security_features(&xml),
                None => log::warn!("Failed to get expanded CPU XML"),
            }
        }
        log::debug!("Found host-model security features: {ret:?}");
        ret
    }
}

/// `_lookup_cpu_security_features`'s filter over the expanded CPU.
pub fn cpu_security_features(expanded_xml: &str) -> Vec<String> {
    const SEC_FEATURES: [&str; 5] = ["spec-ctrl", "ssbd", "ibpb", "virt-ssbd", "md-clear"];
    let Ok(doc) = parse_libvirt_xml(expanded_xml) else {
        return vec![];
    };
    let Some(root) = doc.root_element() else {
        return vec![];
    };
    root.find_all(&doc, "feature")
        .into_iter()
        .filter_map(|f| f.attribute(&doc, "name"))
        .filter(|n| SEC_FEATURES.contains(n))
        .map(str::to_string)
        .collect()
}

/// Resolved default `<cpu>` mode — see [`DomainCapabilities::
/// default_cpu_mode`]'s doc comment for the full trace of which
/// upstream branch produces which variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefaultCpuMode {
    /// `<cpu mode="MODE"/>` — `host-passthrough`, `host-model`, or
    /// `maximum`, no other attributes or children.
    Mode(&'static str),
    /// `<cpu mode="custom" match="exact">` with an optional `<model>`
    /// child — upstream's host-model-only fallback. `model` is `None`
    /// when domcaps considers the host's own reported CPU model unsafe
    /// to pin; `mode="custom"`/`match="exact"` are emitted either way.
    /// See `default_cpu_mode`'s doc comment: this validate-then-maybe-
    /// strip step is upstream's GUI-path behavior (`x86_cpu_default ==
    /// HOST_MODEL_ONLY`), applied unconditionally here since this crate
    /// has no equivalent switch — not something `virt-install`'s own
    /// CLI path reaches upstream, but real behavior a 1:1 GUI port will
    /// need.
    Custom { model: Option<String> },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TEST_DOMAIN_LOCK;

    #[test]
    fn cpu_models_memfd_and_host_model_xml() {
        let caps = DomainCapabilities::parse(
            "<domainCapabilities><arch>x86_64</arch>\
             <memoryBacking supported='yes'><enum name='sourceType'><value>memfd</value></enum></memoryBacking>\
             <cpu><mode name='host-passthrough' supported='yes'/>\
             <mode name='host-model' supported='yes'><model fallback='forbid'>Skylake</model>\
             <vendor>Intel</vendor><feature policy='require' name='ssbd'/></mode>\
             <mode name='custom' supported='yes'><model usable='yes'>Haswell</model>\
             <model usable='no'>EPYC</model><model usable='unknown'>qemu64</model></mode>\
             <mode name='maximum' supported='yes'/></cpu>\
             <devices><filesystem supported='yes'><enum name='driverType'><value>virtiofs</value></enum></filesystem></devices>\
             </domainCapabilities>",
        )
        .unwrap();
        assert_eq!(caps.get_cpu_models(), ["Haswell", "qemu64"]);
        assert!(caps.supports_memorybacking_memfd());
        assert!(caps.supports_filesystem_virtiofs());
        let xml = caps.host_model_cpu_xml.as_deref().unwrap();
        assert_eq!(
            xml,
            "<cpu><model fallback=\"forbid\">Skylake</model><vendor>Intel</vendor>\
             <feature name=\"ssbd\" policy=\"require\"></feature><arch>x86_64</arch></cpu>"
        );
        assert_eq!(
            caps.app_default_cpu_mode(Some("qemu"), Some("x86_64"), Some("pc-q35-9.0")),
            "maximum"
        );
        assert_eq!(
            caps.app_default_cpu_mode(Some("kvm"), Some("x86_64"), None),
            "host-model"
        );
        assert_eq!(
            cpu_security_features(
                "<cpu><feature policy='require' name='ssbd'/><feature policy='require' name='vmx'/>\
                 <feature policy='disable' name='md-clear'/></cpu>"
            ),
            ["ssbd", "md-clear"]
        );
    }

    /// Real libvirt call against libvirt's builtin `test:///` driver —
    /// same no-daemon guarantee `connection.rs`/`domain.rs`'s own tests
    /// rely on. Sparse output (the test driver reports almost nothing
    /// supported), but proves the fetch-then-parse pipeline actually
    /// works end to end against a live connection, not just a fixture.
    /// Holds `TEST_DOMAIN_LOCK`: asserts specific `domain`/`arch`
    /// values a concurrently-running `VIRTINST_TEST_DOMCAPS_FILE`
    /// injection test below would otherwise override — see
    /// `TEST_DOMAIN_LOCK`'s own doc comment for the confirmed-for-real
    /// failure this guards against.
    #[test]
    fn fetch_against_the_builtin_test_driver() {
        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        let caps = DomainCapabilities::fetch(&conn, None, None, None, None)
            .expect("test driver answers domcapabilities");
        assert_eq!(caps.domain.as_deref(), Some("test"));
        assert_eq!(caps.arch.as_deref(), Some("i686"));
    }

    /// Ticket 20's test-mode injection: `VIRTINST_TEST_DOMCAPS_FILE`
    /// read directly instead of calling libvirt, mirroring
    /// `capabilities::caps`'s identical test for `Capabilities::fetch`.
    #[test]
    fn fetch_reads_from_env_var_file_instead_of_calling_libvirt() {
        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let path = std::env::temp_dir().join("virtinst_test_domcaps_fetch.xml");
        std::fs::write(&path, QEMU_DOMCAPS_EXCERPT).expect("writes fixture file");
        std::env::set_var("VIRTINST_TEST_DOMCAPS_FILE", &path);

        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        let caps = DomainCapabilities::fetch(&conn, None, None, None, None)
            .expect("reads the injected file, not libvirt");

        std::env::remove_var("VIRTINST_TEST_DOMCAPS_FILE");
        std::fs::remove_file(&path).ok();

        // test:///default's own domcaps report domain="test"/arch="i686"
        // (see the test right above) -- getting kvm/x86_64 back instead
        // proves the injected file was really used, not a live call.
        assert_eq!(caps.domain.as_deref(), Some("kvm"));
        assert_eq!(caps.arch.as_deref(), Some("x86_64"));
    }

    #[test]
    fn fetch_surfaces_a_missing_env_var_file_as_a_real_error_not_a_panic() {
        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(
            "VIRTINST_TEST_DOMCAPS_FILE",
            "/nonexistent/path/does-not-exist.xml",
        );

        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        let result = DomainCapabilities::fetch(&conn, None, None, None, None);

        std::env::remove_var("VIRTINST_TEST_DOMCAPS_FILE");

        assert!(matches!(result, Err(CapabilitiesError::Io(_))));
    }

    /// A real `qemu:///system` domcapabilities excerpt captured on this
    /// machine (`virsh -c qemu:///system domcapabilities`), not
    /// invented — exercises the shapes `test:///`'s own sparse output
    /// never does: multi-value enums, a block with direct `<value>`
    /// children alongside its enums, and an empty self-closed `<enum/>`.
    const QEMU_DOMCAPS_EXCERPT: &str = r#"<domainCapabilities>
  <path>/usr/bin/qemu-system-x86_64</path>
  <domain>kvm</domain>
  <machine>pc-i440fx-resolute</machine>
  <arch>x86_64</arch>
  <devices>
    <disk supported='yes'>
      <enum name='diskDevice'>
        <value>disk</value>
        <value>cdrom</value>
        <value>floppy</value>
        <value>lun</value>
      </enum>
      <enum name='bus'>
        <value>ide</value>
        <value>fdc</value>
        <value>scsi</value>
        <value>virtio</value>
        <value>usb</value>
        <value>sata</value>
        <value>nvme</value>
      </enum>
    </disk>
    <graphics supported='yes'>
      <enum name='type'>
        <value>sdl</value>
        <value>vnc</value>
        <value>spice</value>
      </enum>
    </graphics>
    <hostdev supported='yes'>
      <enum name='subsysType'>
        <value>usb</value>
        <value>pci</value>
        <value>scsi</value>
      </enum>
      <enum name='capsType'/>
    </hostdev>
  </devices>
</domainCapabilities>"#;

    /// A real `<os>` block, from this machine's own
    /// `virsh -c qemu:///session domcapabilities` capture (`qemu:///
    /// system` polkit rationale same as `REAL_QEMU_CPU_EXCERPT`'s own
    /// comment below) — the modern firmware auto-select capability
    /// (`<enum name='firmware'><value>efi</value></enum>`) genuinely
    /// present on this host's real QEMU/libvirt install, not a
    /// hand-constructed shape guessed from upstream's own source.
    const REAL_QEMU_OS_FIRMWARE_EXCERPT: &str = r#"<domainCapabilities>
  <os supported='yes'>
    <enum name='firmware'>
      <value>efi</value>
    </enum>
    <firmwareFeatures supported='yes'>
      <enum name='secureBoot'>
        <value>no</value>
      </enum>
      <enum name='enrolledKeys'>
        <value>no</value>
      </enum>
    </firmwareFeatures>
    <loader supported='yes'>
      <value>/usr/share/edk2/x64/OVMF_CODE.4m.fd</value>
      <enum name='type'>
        <value>rom</value>
        <value>pflash</value>
      </enum>
      <enum name='readonly'>
        <value>yes</value>
        <value>no</value>
      </enum>
      <enum name='secure'>
        <value>no</value>
      </enum>
    </loader>
    <varstore supported='no'/>
  </os>
</domainCapabilities>"#;

    /// A real `<cpu>` block, trimmed from this machine's own
    /// `virsh -c qemu:///session domcapabilities` capture (`custom`'s
    /// full model list runs to hundreds of entries — kept two real
    /// ones, one `usable='yes'` and one `usable='no'`, enough to
    /// exercise the shape without pasting the whole thing). Not
    /// `qemu:///system`'s capture — this session had no polkit
    /// authorization for `qemu:///system` directly, `qemu:///session`
    /// reports the identical `<cpu>` shape on this same host/QEMU/
    /// libvirt install.
    const REAL_QEMU_CPU_EXCERPT: &str = r#"<domainCapabilities>
  <cpu>
    <mode name='host-passthrough' supported='yes'>
      <enum name='hostPassthroughMigratable'>
        <value>on</value>
        <value>off</value>
      </enum>
    </mode>
    <mode name='maximum' supported='yes'>
      <enum name='maximumMigratable'>
        <value>on</value>
        <value>off</value>
      </enum>
    </mode>
    <mode name='host-model' supported='yes'>
      <model fallback='forbid'>Skylake-Client-IBRS</model>
      <vendor>Intel</vendor>
    </mode>
    <mode name='custom' supported='yes'>
      <model usable='yes' vendor='unknown' canonical='486-v1'>486</model>
      <model usable='no' vendor='Intel' canonical='Broadwell-v1'>Broadwell</model>
    </mode>
  </cpu>
</domainCapabilities>"#;

    #[test]
    fn parse_reads_scalars_and_nested_enum_values() {
        let caps = DomainCapabilities::parse(QEMU_DOMCAPS_EXCERPT).expect("parses");
        assert_eq!(caps.path.as_deref(), Some("/usr/bin/qemu-system-x86_64"));
        assert_eq!(caps.domain.as_deref(), Some("kvm"));
        assert_eq!(caps.machine.as_deref(), Some("pc-i440fx-resolute"));
        assert_eq!(caps.arch.as_deref(), Some("x86_64"));

        assert_eq!(caps.devices.disk.supported, Some(true));
        let bus = caps.devices.disk.get_enum("bus").expect("bus enum");
        assert!(bus.has_value("virtio"));
        assert!(bus.has_value("nvme"));
        assert!(!bus.has_value("made-up-bus"));
        assert_eq!(bus.values.len(), 7);

        assert!(caps.devices.graphics.has_enum("type"));
        assert!(!caps.devices.graphics.has_enum("nonexistent"));
    }

    #[test]
    fn parse_handles_a_self_closed_empty_enum() {
        let caps = DomainCapabilities::parse(QEMU_DOMCAPS_EXCERPT).expect("parses");
        let caps_type = caps
            .devices
            .hostdev
            .get_enum("capsType")
            .expect("capsType enum exists even though it has no values");
        assert!(caps_type.values.is_empty());
    }

    #[test]
    fn parse_leaves_unmodeled_and_absent_device_blocks_as_default() {
        let caps = DomainCapabilities::parse(QEMU_DOMCAPS_EXCERPT).expect("parses");
        // video/tpm/filesystem/redirdev/channel/panic are all modeled
        // fields, but this excerpt's <devices> doesn't include them.
        assert_eq!(caps.devices.video, CapsBlock::default());
        assert_eq!(caps.devices.tpm.supported, None);
    }

    /// A real driver that advertises graphics support but only offers
    /// `vnc`, not `spice` — the branch `QEMU_DOMCAPS_EXCERPT` alone can't
    /// exercise (it offers all three types).
    const VNC_ONLY_DOMCAPS_EXCERPT: &str = r#"<domainCapabilities>
  <devices>
    <graphics supported='yes'>
      <enum name='type'>
        <value>vnc</value>
      </enum>
    </graphics>
  </devices>
</domainCapabilities>"#;

    #[test]
    fn supports_graphics_spice_reads_the_advertised_enum_when_present() {
        let caps = DomainCapabilities::parse(QEMU_DOMCAPS_EXCERPT).expect("parses");
        // The advertised enum settles it regardless of the URI-scheme/
        // arch fallback inputs -- pass a URI/arch that would fail the
        // fallback heuristic to prove they're never consulted.
        assert!(caps.supports_graphics_spice(&crate::connection::Uri::parse("xen:///"), None));
    }

    #[test]
    fn supports_graphics_spice_is_false_when_the_advertised_enum_lacks_spice() {
        let caps = DomainCapabilities::parse(VNC_ONLY_DOMCAPS_EXCERPT).expect("parses");
        // Same reasoning in reverse: pass a URI/arch that would succeed
        // the fallback heuristic, to prove the advertised (negative)
        // answer wins.
        assert!(!caps.supports_graphics_spice(
            &crate::connection::Uri::parse("qemu:///system"),
            Some("x86_64")
        ));
    }

    #[test]
    fn supports_graphics_spice_falls_back_to_the_pre_domcaps_heuristic_when_unadvertised() {
        use crate::connection::Uri;

        // No <graphics> block at all -- `supported` stays `None`, same
        // as talking to a libvirt too old to advertise it.
        let caps = DomainCapabilities::default();
        assert!(
            caps.supports_graphics_spice(&Uri::parse("qemu:///system"), Some("x86_64")),
            "qemu + x86_64 must fall back to true"
        );
        assert!(
            caps.supports_graphics_spice(&Uri::parse("test:///default"), Some("i686")),
            "the test driver + i686 must fall back to true too"
        );
        assert!(
            !caps.supports_graphics_spice(&Uri::parse("xen:///"), Some("x86_64")),
            "neither qemu nor test -- e.g. a real Xen connection -- must fall back to false"
        );
        assert!(
            !caps.supports_graphics_spice(&Uri::parse("qemu:///system"), Some("aarch64")),
            "qemu on a non-x86 host arch must fall back to false"
        );
    }

    #[test]
    fn supports_firmware_efi_reads_the_advertised_enum() {
        let caps = DomainCapabilities::parse(REAL_QEMU_OS_FIRMWARE_EXCERPT).expect("parses");
        assert!(caps.supports_firmware_efi());
    }

    #[test]
    fn supports_firmware_efi_is_false_when_the_os_block_is_absent() {
        // No <os> block at all -- same as talking to a libvirt too old
        // to advertise the modern firmware auto-select capability.
        // No fallback heuristic here (unlike supports_graphics_spice):
        // upstream's own enable_uefi() falls to a real loader-path
        // filesystem scan in this case, not a guessed bool.
        let caps = DomainCapabilities::default();
        assert!(!caps.supports_firmware_efi());
    }

    #[test]
    fn default_graphics_type_forces_vnc_for_xen_even_when_spice_is_supported() {
        let caps = DomainCapabilities::parse(QEMU_DOMCAPS_EXCERPT).expect("parses");
        assert_eq!(
            caps.default_graphics_type(&crate::connection::Uri::parse("xen:///"), None),
            "vnc",
            "xen must be forced to vnc regardless of what domcaps advertises"
        );
    }

    #[test]
    fn default_graphics_type_stays_spice_when_the_hv_supports_it() {
        let caps = DomainCapabilities::parse(QEMU_DOMCAPS_EXCERPT).expect("parses");
        assert_eq!(
            caps.default_graphics_type(
                &crate::connection::Uri::parse("qemu:///system"),
                Some("x86_64")
            ),
            "spice"
        );
    }

    #[test]
    fn default_graphics_type_downgrades_to_vnc_when_the_hv_lacks_spice_support() {
        let caps = DomainCapabilities::parse(VNC_ONLY_DOMCAPS_EXCERPT).expect("parses");
        assert_eq!(
            caps.default_graphics_type(
                &crate::connection::Uri::parse("qemu:///system"),
                Some("x86_64")
            ),
            "vnc"
        );
    }

    /// The actual regression test for the transposition risk an advisor
    /// review flagged: every test above passes a `Uri` built directly
    /// from a literal scheme string, which proves `default_graphics_
    /// type` honors `Uri::is_xen()`/`is_qemu()`/`is_test()` correctly,
    /// but not that a real call site can't transpose *which* argument
    /// carries which meaning. This is the one case where transposition
    /// would actually be observable: a **xen** URI specifically, not a
    /// qemu one -- with a qemu URI, `is_xen()`/`is_test()` are both
    /// `false` either way, so swapping them changes nothing. With xen,
    /// swapping `is_xen`/`is_qemu`'s old bool-parameter order would have
    /// silently let a real xen connection fall through to the spice
    /// branch instead of being forced to vnc. `VIRTINST_TEST_DOMCAPS_
    /// FILE` here advertises spice support explicitly (same shape as
    /// `QEMU_DOMCAPS_EXCERPT`) specifically so a wrong answer would be
    /// `"spice"`, not an already-correct `"vnc"` for an unrelated reason
    /// (e.g. spice just not being advertised).
    #[test]
    fn default_graphics_type_forces_vnc_for_a_real_xen_uri_even_when_domcaps_advertises_spice() {
        use crate::connection::Uri;

        let fixture = r#"<domainCapabilities>
  <devices>
    <graphics supported='yes'>
      <enum name='type'>
        <value>vnc</value>
        <value>spice</value>
      </enum>
    </graphics>
  </devices>
</domainCapabilities>"#;
        let caps = DomainCapabilities::parse(fixture).expect("parses");
        let uri = Uri::parse("xen+ssh://root@example.com/");

        assert!(uri.is_xen(), "sanity check on the fixture URI itself");
        assert!(
            caps.supports_graphics_spice(&uri, Some("x86_64")),
            "sanity check: domcaps really does advertise spice here"
        );
        assert_eq!(
            caps.default_graphics_type(&uri, Some("x86_64")),
            "vnc",
            "a real xen URI must be forced to vnc even though domcaps advertises spice support"
        );
    }

    #[test]
    fn parse_reads_the_cpu_block_and_its_nested_models() {
        let caps = DomainCapabilities::parse(REAL_QEMU_CPU_EXCERPT).expect("parses");

        let passthrough = caps.cpu.get_mode("host-passthrough").expect("mode exists");
        assert_eq!(passthrough.supported, Some(true));
        assert!(passthrough
            .get_enum("hostPassthroughMigratable")
            .expect("enum exists")
            .has_value("on"));

        let maximum = caps.cpu.get_mode("maximum").expect("mode exists");
        assert_eq!(maximum.supported, Some(true));

        let host_model = caps.cpu.get_mode("host-model").expect("mode exists");
        assert_eq!(host_model.models.len(), 1);
        let model = host_model
            .get_model("Skylake-Client-IBRS")
            .expect("model exists");
        assert_eq!(model.fallback.as_deref(), Some("forbid"));

        let custom = caps.cpu.get_mode("custom").expect("mode exists");
        assert_eq!(custom.models.len(), 2);
        assert_eq!(
            custom.get_model("486").unwrap().usable.as_deref(),
            Some("yes")
        );
        assert_eq!(
            custom.get_model("Broadwell").unwrap().usable.as_deref(),
            Some("no")
        );

        assert!(
            caps.cpu.get_mode("nonexistent").is_none(),
            "a mode name that isn't in the fixture must read as absent, not panic"
        );
    }

    #[test]
    fn cpu_block_absent_reads_as_default_not_a_parse_error() {
        let caps = DomainCapabilities::parse("<domainCapabilities/>").expect("parses");
        assert_eq!(caps.cpu, Cpu::default());
        assert!(caps.cpu.get_mode("host-passthrough").is_none());
    }

    #[test]
    fn supports_safe_host_passthrough_reads_the_real_fixture() {
        let caps = DomainCapabilities::parse(REAL_QEMU_CPU_EXCERPT).expect("parses");
        assert!(caps.supports_safe_host_passthrough());
    }

    #[test]
    fn supports_safe_host_passthrough_is_false_when_the_mode_is_absent() {
        let caps = DomainCapabilities::default();
        assert!(!caps.supports_safe_host_passthrough());
    }

    #[test]
    fn supports_safe_host_passthrough_is_false_when_migratable_lacks_on() {
        let fixture = r#"<domainCapabilities>
  <cpu>
    <mode name='host-passthrough' supported='yes'>
      <enum name='hostPassthroughMigratable'>
        <value>off</value>
      </enum>
    </mode>
  </cpu>
</domainCapabilities>"#;
        let caps = DomainCapabilities::parse(fixture).expect("parses");
        assert!(!caps.supports_safe_host_passthrough());
    }

    #[test]
    fn supports_maximum_cpu_mode_reads_the_real_fixture() {
        let caps = DomainCapabilities::parse(REAL_QEMU_CPU_EXCERPT).expect("parses");
        assert!(caps.supports_maximum_cpu_mode());
    }

    #[test]
    fn supports_maximum_cpu_mode_is_false_when_the_mode_is_absent() {
        let caps = DomainCapabilities::default();
        assert!(!caps.supports_maximum_cpu_mode());
    }

    #[test]
    fn supports_safe_host_model_reads_the_real_fixture() {
        let caps = DomainCapabilities::parse(REAL_QEMU_CPU_EXCERPT).expect("parses");
        assert!(caps.supports_safe_host_model());
    }

    #[test]
    fn supports_safe_host_model_is_false_when_the_mode_is_absent() {
        let caps = DomainCapabilities::default();
        assert!(!caps.supports_safe_host_model());
    }

    /// The `.first()`-not-`[0]` divergence from upstream's bare
    /// `m.models[0]`, documented on `supports_safe_host_model` itself:
    /// an advertised `host-model` block with no `<model>` child at all
    /// must read as unsupported, not panic.
    #[test]
    fn supports_safe_host_model_is_false_when_advertised_with_no_model_child() {
        let fixture = r#"<domainCapabilities>
  <cpu>
    <mode name='host-model' supported='yes'/>
  </cpu>
</domainCapabilities>"#;
        let caps = DomainCapabilities::parse(fixture).expect("parses");
        assert!(!caps.supports_safe_host_model());
    }

    #[test]
    fn supports_safe_host_model_is_false_when_fallback_is_not_forbid() {
        let fixture = r#"<domainCapabilities>
  <cpu>
    <mode name='host-model' supported='yes'>
      <model fallback='allow'>Skylake-Client-IBRS</model>
    </mode>
  </cpu>
</domainCapabilities>"#;
        let caps = DomainCapabilities::parse(fixture).expect("parses");
        assert!(!caps.supports_safe_host_model());
    }

    // -- default_cpu_mode (ticket 21 slice (f) piece 2) --

    #[test]
    fn default_cpu_mode_prefers_host_passthrough_when_safe() {
        use crate::connection::Uri;
        // REAL_QEMU_CPU_EXCERPT advertises both host-passthrough and
        // host-model as safe -- host-passthrough must win, matching
        // upstream's preference order.
        let caps = DomainCapabilities::parse(REAL_QEMU_CPU_EXCERPT).expect("parses");
        assert_eq!(
            caps.default_cpu_mode(
                &Uri::parse("qemu:///system"),
                "kvm",
                "x86_64",
                "x86_64",
                Some("Skylake-Client-IBRS"),
            ),
            Some(DefaultCpuMode::Mode("host-passthrough"))
        );
    }

    #[test]
    fn default_cpu_mode_falls_back_to_host_model_when_passthrough_is_unsafe() {
        use crate::connection::Uri;
        let fixture = r#"<domainCapabilities>
  <cpu>
    <mode name='host-model' supported='yes'>
      <model fallback='forbid'>Skylake-Client-IBRS</model>
    </mode>
  </cpu>
</domainCapabilities>"#;
        let caps = DomainCapabilities::parse(fixture).expect("parses");
        assert_eq!(
            caps.default_cpu_mode(
                &Uri::parse("qemu:///system"),
                "kvm",
                "x86_64",
                "x86_64",
                Some("Skylake-Client-IBRS"),
            ),
            Some(DefaultCpuMode::Mode("host-model"))
        );
    }

    #[test]
    fn default_cpu_mode_falls_back_to_a_custom_model_when_neither_is_safe() {
        use crate::connection::Uri;
        // REAL_QEMU_CPU_EXCERPT's `custom` mode lists "486" as
        // `usable='yes'` -- an empty <cpu> otherwise means neither
        // host-passthrough nor host-model is even advertised.
        let fixture = r#"<domainCapabilities>
  <cpu>
    <mode name='custom' supported='yes'>
      <model usable='yes' vendor='unknown' canonical='486-v1'>486</model>
    </mode>
  </cpu>
</domainCapabilities>"#;
        let caps = DomainCapabilities::parse(fixture).expect("parses");
        assert_eq!(
            caps.default_cpu_mode(
                &Uri::parse("qemu:///system"),
                "kvm",
                "x86_64",
                "x86_64",
                Some("486"),
            ),
            Some(DefaultCpuMode::Custom {
                model: Some("486".to_string())
            })
        );
    }

    /// Upstream's `_validate_default_host_model_only` logic (applied
    /// unconditionally here, see `default_cpu_mode`'s doc comment for
    /// why): the host's own reported CPU model (from `<capabilities>`)
    /// may be marked `usable='no'` in domcaps' `custom` mode list.
    /// `mode="custom"` still gets emitted (via `Custom { model: None }`)
    /// -- not tidied away, matching the shape upstream's own GUI path
    /// produces.
    #[test]
    fn default_cpu_mode_strips_an_unusable_host_model_but_keeps_custom_mode() {
        use crate::connection::Uri;
        let fixture = r#"<domainCapabilities>
  <cpu>
    <mode name='custom' supported='yes'>
      <model usable='no' vendor='Intel' canonical='Broadwell-v1'>Broadwell</model>
    </mode>
  </cpu>
</domainCapabilities>"#;
        let caps = DomainCapabilities::parse(fixture).expect("parses");
        assert_eq!(
            caps.default_cpu_mode(
                &Uri::parse("qemu:///system"),
                "kvm",
                "x86_64",
                "x86_64",
                Some("Broadwell"),
            ),
            Some(DefaultCpuMode::Custom { model: None })
        );
    }

    #[test]
    fn default_cpu_mode_is_none_when_no_host_cpu_model_is_known_at_all() {
        use crate::connection::Uri;
        let caps = DomainCapabilities::default();
        assert_eq!(
            caps.default_cpu_mode(
                &Uri::parse("qemu:///system"),
                "kvm",
                "x86_64",
                "x86_64",
                None
            ),
            None,
            "nothing better to offer -- upstream leaves <cpu> unset entirely here"
        );
    }

    #[test]
    fn default_cpu_mode_is_none_for_cross_arch_emulation() {
        use crate::connection::Uri;
        // Guest arch doesn't match the host's -- no host-based default
        // makes sense, regardless of what domcaps advertises.
        let caps = DomainCapabilities::parse(REAL_QEMU_CPU_EXCERPT).expect("parses");
        assert_eq!(
            caps.default_cpu_mode(
                &Uri::parse("qemu:///system"),
                "kvm",
                "aarch64",
                "x86_64",
                Some("Skylake-Client-IBRS"),
            ),
            None
        );
    }

    #[test]
    fn default_cpu_mode_uses_maximum_for_tcg_qemu_when_advertised() {
        use crate::connection::Uri;
        // domain_type = "qemu" (TCG, not KVM-accelerated) -- the
        // host-passthrough/host-model/custom chain never runs; this is
        // the one branch `_should_use_maximum_cpu_mode` can actually
        // reach on its own, independent of `_get_app_default_mode`.
        let caps = DomainCapabilities::parse(REAL_QEMU_CPU_EXCERPT).expect("parses");
        assert_eq!(
            caps.default_cpu_mode(
                &Uri::parse("qemu:///system"),
                "qemu",
                "x86_64",
                "x86_64",
                Some("Skylake-Client-IBRS"),
            ),
            Some(DefaultCpuMode::Mode("maximum"))
        );
    }

    #[test]
    fn default_cpu_mode_is_none_for_tcg_qemu_when_maximum_is_not_advertised() {
        use crate::connection::Uri;
        let fixture = r#"<domainCapabilities>
  <cpu>
    <mode name='host-passthrough' supported='yes'>
      <enum name='hostPassthroughMigratable'>
        <value>on</value>
      </enum>
    </mode>
  </cpu>
</domainCapabilities>"#;
        let caps = DomainCapabilities::parse(fixture).expect("parses");
        assert_eq!(
            caps.default_cpu_mode(
                &Uri::parse("qemu:///system"),
                "qemu",
                "x86_64",
                "x86_64",
                Some("Skylake-Client-IBRS"),
            ),
            None,
            "host-passthrough being safe is irrelevant on the TCG branch -- it's never consulted"
        );
    }

    #[test]
    fn default_cpu_mode_is_none_for_a_real_xen_connection() {
        use crate::connection::Uri;
        // Upstream's top gate: `conn.is_test() or conn.is_qemu()` --
        // xen (and vz) never get a <cpu> default here at all.
        let caps = DomainCapabilities::parse(REAL_QEMU_CPU_EXCERPT).expect("parses");
        assert_eq!(
            caps.default_cpu_mode(
                &Uri::parse("xen+ssh://root@example.com/"),
                "xen",
                "x86_64",
                "x86_64",
                Some("Skylake-Client-IBRS"),
            ),
            None
        );
    }
}
