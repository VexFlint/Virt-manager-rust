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
//! **Not yet built**: `os`/`cpu`/`features` — each a different,
//! non-`_CapsBlock` shape upstream layers on top (`_OS` adds a nested
//! `loader` sub-block, `_CPU` is a list of `<mode>`s with their own
//! `<model>` children including full CPU feature lists, `_Features`
//! mixes plain `_CapsBlock`s with a distinct `_SEV` shape) — added
//! when ticket 17's per-driver chains actually need one, same "close
//! the gap when it's real, not speculatively" pattern as everywhere
//! else in this crate.

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

/// ```xml
/// <domainCapabilities>
///   <path>/usr/bin/qemu-system-x86_64</path>
///   <domain>kvm</domain>
///   <machine>pc-i440fx-resolute</machine>
///   <arch>x86_64</arch>
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
    pub devices: DomainCapabilitiesDevices,
}

impl DomainCapabilities {
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
            devices: match root.find(&doc, "devices") {
                Some(el) => DomainCapabilitiesDevices::from_element(&doc, el),
                None => DomainCapabilitiesDevices::default(),
            },
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
        if let Some(path) = std::env::var_os("VIRTINST_TEST_DOMCAPS_FILE") {
            let xml = std::fs::read_to_string(&path)?;
            return Ok(DomainCapabilities::parse(&xml)?);
        }
        let xml = conn
            .as_connect()
            .get_domain_capabilities(emulator, arch, machine, virttype, 0)?;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TEST_DOMAIN_LOCK;

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
}
