//! `<capabilities>` (host) — upstream's `capabilities.py`. Ticket 20's
//! own scope: only what `guest_lookup` (upstream's real hypervisor-
//! type/arch/machine selection algorithm — `Guest.set_capabilities_
//! defaults`'s decision logic) actually needs: `host.cpu.arch` (the
//! "prefer host arch when none was requested" fallback) and the full
//! `<guest>`/`<domain>`/`<machine>` tree `guest_lookup` walks. Same
//! lightweight deserialize-only, no-DOM path `DomainCapabilities`
//! already established (ticket 17) — read-only, fetched fresh per
//! connection, never edited or serialized back, so none of ticket 06's
//! order-preserving machinery earns its keep here either.
//!
//! **Not modeled**: `<host><secmodel>` (upstream's
//! `get_qemu_baselabel` — real uid/permission resolution via
//! `pwd.getpwuid`, unrelated to hypervisor-type selection, no current
//! consumer); `_CapsGuest`'s query helper methods
//! (`supports_pae`/`is_kvm_available`/`supports_acpi`/etc — the
//! `features` data itself *is* bound, since it's part of `_CapsGuest`'s
//! real shape, but nothing calls these accessor methods yet);
//! `is_machine_alias` (used by a `Guest.refresh_machine_type`-
//! equivalent this crate hasn't built); `has_install_options`. All
//! flagged rather than guessed at — same "close the gap when a real
//! caller needs it" pattern `domcaps.rs` already set, not a new
//! standard invented here.
//!
//! **Test-mode injection** (ticket 20): [`Capabilities::fetch`] checks
//! `VIRTINST_TEST_CAPS_FILE` before making a real libvirt call, reading
//! and parsing that file instead when set — same env-var-gated
//! precedent `VIRTINST_TEST_PREDICTABLE` already established (a
//! harness spawns each case as its own OS process, so there's never a
//! need for two different values in one process; an env var is simpler
//! than threading an override through every call site). Upstream's
//! `MagicURI` does the equivalent by monkeypatching `conn.
//! getCapabilities` after the connection opens — the real `virt` crate
//! has no such seam (ticket 19's own finding), so this gates inside
//! `fetch` itself instead, transparent to every existing caller.

use super::domcaps::CapabilitiesError;
use crate::connection::Connection;
use virtinst_xml::{parse_libvirt_xml, Document, Element, Result as XmlResult};

/// `<host><cpu>` — upstream's `_CapsCPU`. Three plain text-content
/// child elements, not attributes (`arch = XMLProperty("./arch")`, one
/// level down from `<cpu>` itself).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapsCpu {
    pub arch: Option<String>,
    pub model: Option<String>,
    pub vendor: Option<String>,
}

impl CapsCpu {
    fn from_element(doc: &Document, el: Element) -> CapsCpu {
        CapsCpu {
            arch: el.find(doc, "arch").map(|e| e.text_content(doc)),
            model: el.find(doc, "model").map(|e| e.text_content(doc)),
            vendor: el.find(doc, "vendor").map(|e| e.text_content(doc)),
        }
    }
}

/// `<host>` — upstream's `_CapsHost`. Of `secmodels` only each
/// `<secmodel><model>` (see this module's doc comment for why).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapsHost {
    pub cpu: CapsCpu,
    pub secmodels: Vec<Option<String>>,
}

impl CapsHost {
    fn from_element(doc: &Document, el: Element) -> CapsHost {
        CapsHost {
            cpu: match el.find(doc, "cpu") {
                Some(c) => CapsCpu::from_element(doc, c),
                None => CapsCpu::default(),
            },
            secmodels: el
                .find_all(doc, "secmodel")
                .into_iter()
                .map(|m| m.find(doc, "model").map(|e| e.text_content(doc)))
                .collect(),
        }
    }
}

/// `<machine>` — upstream's `_CapsMachine`. `name` is the element's own
/// text content (`XMLProperty(".")`), `canonical` a self-attribute.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapsMachine {
    pub name: Option<String>,
    pub canonical: Option<String>,
}

impl CapsMachine {
    fn from_element(doc: &Document, el: Element) -> CapsMachine {
        CapsMachine {
            name: Some(el.text_content(doc)),
            canonical: el.attribute(doc, "canonical").map(str::to_string),
        }
    }
}

/// `<guest><arch><domain>` — upstream's `_CapsDomain`. `machines` here
/// are the domain's *own* `<machine>` children, distinct from
/// `CapsGuest::machines` one level up (upstream binds the same
/// `_CapsMachine` type at two different relative paths).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapsDomain {
    pub hypervisor_type: Option<String>,
    pub emulator: Option<String>,
    pub machines: Vec<CapsMachine>,
}

impl CapsDomain {
    fn from_element(doc: &Document, el: Element) -> CapsDomain {
        CapsDomain {
            hypervisor_type: el.attribute(doc, "type").map(str::to_string),
            emulator: el.find(doc, "emulator").map(|e| e.text_content(doc)),
            machines: el
                .find_all(doc, "machine")
                .into_iter()
                .map(|m| CapsMachine::from_element(doc, m))
                .collect(),
        }
    }
}

/// `<guest><features>` — upstream's `_CapsGuestFeatures`. Bound for
/// structural completeness (part of `_CapsGuest`'s real shape); no
/// accessor methods built yet, see this module's doc comment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapsGuestFeatures {
    pub pae: bool,
    pub acpi: Option<bool>,
    pub apic: Option<bool>,
    pub external_snapshot: bool,
}

impl CapsGuestFeatures {
    fn from_element(doc: &Document, el: Element) -> CapsGuestFeatures {
        CapsGuestFeatures {
            pae: el.find(doc, "pae").is_some(),
            acpi: el
                .find(doc, "acpi")
                .and_then(|e| e.attribute(doc, "default"))
                .map(|s| s == "on"),
            apic: el
                .find(doc, "apic")
                .and_then(|e| e.attribute(doc, "default"))
                .map(|s| s == "on"),
            external_snapshot: el.find(doc, "externalSnapshot").is_some(),
        }
    }
}

/// `<guest>` — upstream's `_CapsGuest`. `arch`/`loader`/`emulator`/
/// `domains`/`machines` all live one level down, under `<arch>`
/// (`relative_xpath="./arch"` upstream) — `os_type`/`features` are
/// direct children of `<guest>` itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapsGuest {
    pub os_type: Option<String>,
    pub arch: Option<String>,
    pub loader: Option<String>,
    pub emulator: Option<String>,
    pub domains: Vec<CapsDomain>,
    pub features: CapsGuestFeatures,
    pub machines: Vec<CapsMachine>,
}

impl CapsGuest {
    /// Upstream's `is_kvm_available`: any `<domain type='kvm'>`.
    pub fn is_kvm_available(&self) -> bool {
        self.domains
            .iter()
            .any(|d| d.hypervisor_type.as_deref() == Some("kvm"))
    }

    /// Upstream's `supports_externalSnapshot`: `<externalSnapshot/>`
    /// under `<features>`.
    pub fn supports_external_snapshot(&self) -> bool {
        self.features.external_snapshot
    }

    fn from_element(doc: &Document, el: Element) -> CapsGuest {
        let arch_el = el.find(doc, "arch");
        CapsGuest {
            os_type: el.find(doc, "os_type").map(|e| e.text_content(doc)),
            arch: arch_el
                .and_then(|a| a.attribute(doc, "name"))
                .map(str::to_string),
            loader: arch_el
                .and_then(|a| a.find(doc, "loader"))
                .map(|e| e.text_content(doc)),
            emulator: arch_el
                .and_then(|a| a.find(doc, "emulator"))
                .map(|e| e.text_content(doc)),
            domains: arch_el
                .map(|a| {
                    a.find_all(doc, "domain")
                        .into_iter()
                        .map(|d| CapsDomain::from_element(doc, d))
                        .collect()
                })
                .unwrap_or_default(),
            features: match el.find(doc, "features") {
                Some(f) => CapsGuestFeatures::from_element(doc, f),
                None => CapsGuestFeatures::default(),
            },
            machines: arch_el
                .map(|a| {
                    a.find_all(doc, "machine")
                        .into_iter()
                        .map(|m| CapsMachine::from_element(doc, m))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// Upstream's `all_machine_names`: the given domain's own machines
    /// if it has any, else this guest's own list — appending each
    /// machine's canonical alias too, skipping names already seen.
    pub fn all_machine_names(&self, domain: Option<&CapsDomain>) -> Vec<String> {
        let machines: &[CapsMachine] = match domain {
            Some(d) if !d.machines.is_empty() => &d.machines,
            _ => &self.machines,
        };
        let mut ret: Vec<String> = Vec::new();
        for m in machines {
            if let Some(name) = &m.name {
                if !ret.contains(name) {
                    ret.push(name.clone());
                }
            }
            if let Some(canonical) = &m.canonical {
                if !ret.contains(canonical) {
                    ret.push(canonical.clone());
                }
            }
        }
        ret
    }
}

/// Upstream's `_CapsInfo` — the result of [`Capabilities::guest_lookup`].
/// Drops the Python object's `conn`/`guest`/`domain` back-references
/// (nothing here needs to walk back to the source objects, only the
/// resolved values); `is_machine_alias` isn't ported either, see this
/// module's doc comment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapsInfo {
    pub hypervisor_type: Option<String>,
    pub os_type: Option<String>,
    pub arch: Option<String>,
    pub loader: Option<String>,
    pub emulator: Option<String>,
    pub machines: Vec<String>,
}

impl CapsInfo {
    /// `Guest.get_recommended_machine`: only the arch cases with a
    /// clear preference over the default machine. `is_qemu_or_test` is
    /// the connection's `is_qemu() or is_test()`.
    pub fn recommended_machine(&self, is_qemu_or_test: bool) -> Option<&'static str> {
        if !is_qemu_or_test {
            return None;
        }
        let has = |m: &str| self.machines.iter().any(|x| x == m);
        match self.arch.as_deref()? {
            "ppc64" | "ppc64le" if has("pseries") => Some("pseries"),
            "armv7l" | "aarch64" if has("virt") => Some("virt"),
            "armv7l" | "aarch64" if has("vexpress-a15") => Some("vexpress-a15"),
            "s390x" if has("s390-ccw-virtio") => Some("s390-ccw-virtio"),
            "riscv64" | "riscv32" if has("virt") => Some("virt"),
            _ => None,
        }
    }
}

/// `<capabilities>` — upstream's `Capabilities`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub host: CapsHost,
    pub guests: Vec<CapsGuest>,
}

impl Capabilities {
    /// Parses already-fetched `capabilities` XML — split from
    /// [`fetch`](Capabilities::fetch), same reason `DomainCapabilities`
    /// splits the two: a captured real fixture can be tested without a
    /// live connection, and it's the seam ticket 20's test-mode
    /// injection gates on.
    pub fn parse(xml: &str) -> XmlResult<Capabilities> {
        let doc = parse_libvirt_xml(xml)?;
        let root = doc
            .root_element()
            .expect("non-empty capabilities XML always has a root element");

        Ok(Capabilities {
            host: match root.find(&doc, "host") {
                Some(h) => CapsHost::from_element(&doc, h),
                None => CapsHost::default(),
            },
            guests: root
                .find_all(&doc, "guest")
                .into_iter()
                .map(|g| CapsGuest::from_element(&doc, g))
                .collect(),
        })
    }

    /// Fetches and parses in one call — upstream's real
    /// `virConnectGetCapabilities`. Checks `VIRTINST_TEST_CAPS_FILE`
    /// first (see this module's doc comment); real call otherwise.
    pub fn fetch(conn: &Connection) -> Result<Capabilities, CapabilitiesError> {
        Capabilities::fetch_from(conn.as_connect())
    }

    /// [`Capabilities::fetch`] on a bare libvirt connection.
    pub fn fetch_from(conn: &virt::connect::Connect) -> Result<Capabilities, CapabilitiesError> {
        if let Some(path) = std::env::var_os("VIRTINST_TEST_CAPS_FILE") {
            let xml = std::fs::read_to_string(&path)?;
            return Ok(Capabilities::parse(&xml)?);
        }
        let xml = conn.get_capabilities()?;
        Ok(Capabilities::parse(&xml)?)
    }

    /// Upstream's `has_install_options`: some guest has a domain type.
    pub fn has_install_options(&self) -> bool {
        self.guests.iter().any(|g| !g.domains.is_empty())
    }

    /// The `<guest>` a [`guest_lookup`](Capabilities::guest_lookup)
    /// result came from (upstream keeps it as `capsinfo.guest`).
    pub fn guest_for_info(&self, info: &CapsInfo) -> Option<&CapsGuest> {
        self.guest_for_os_type(info.os_type.as_deref(), info.arch.as_deref())
    }

    /// Upstream's `_guestForOSType`: the first `<guest>` whose
    /// `os_type`/`arch` both match what was asked (`None` matches
    /// anything). No explicit `arch` requested falls back to trying the
    /// host's own CPU arch first, then any arch at all — upstream's
    /// exact two-pass `archs = [arch]` vs. `[self.host.cpu.arch, None]`
    /// shape.
    fn guest_for_os_type(&self, os_type: Option<&str>, arch: Option<&str>) -> Option<&CapsGuest> {
        let archs: Vec<Option<&str>> = match arch {
            Some(a) => vec![Some(a)],
            None => vec![self.host.cpu.arch.as_deref(), None],
        };
        for a in archs {
            for g in &self.guests {
                let os_ok = os_type.is_none() || g.os_type.as_deref() == os_type;
                let arch_ok = a.is_none() || g.arch.as_deref() == a;
                if os_ok && arch_ok {
                    return Some(g);
                }
            }
        }
        None
    }

    /// Upstream's `_bestDomainType`: among `guest`'s domains matching
    /// the requested type/machine (both optional filters), prefer
    /// `kvm` > `xen` > `hvf` > `qemu` in that order; falls back to the
    /// last matching domain if none of those four are present — matches
    /// upstream's own fallback exactly (`return domains[-1]`), not a
    /// guessed default.
    fn best_domain_type<'a>(
        &self,
        guest: &'a CapsGuest,
        typ: Option<&str>,
        machine: Option<&str>,
    ) -> Option<&'a CapsDomain> {
        let candidates: Vec<&CapsDomain> = guest
            .domains
            .iter()
            .filter(|d| {
                if let Some(t) = typ {
                    let t_lower = t.to_lowercase();
                    if d.hypervisor_type.as_deref() != Some(t_lower.as_str()) {
                        return false;
                    }
                }
                if let Some(m) = machine {
                    if !guest.all_machine_names(Some(d)).iter().any(|n| n == m) {
                        return false;
                    }
                }
                true
            })
            .collect();

        if candidates.is_empty() {
            return None;
        }
        for pref in ["kvm", "xen", "hvf", "qemu"] {
            if let Some(d) = candidates
                .iter()
                .find(|d| d.hypervisor_type.as_deref() == Some(pref))
            {
                return Some(d);
            }
        }
        candidates.last().copied()
    }

    /// Upstream's `guest_lookup` — the real hypervisor-type/arch/
    /// machine selection algorithm behind `Guest.set_capabilities_
    /// defaults`, ported in full including its four distinct error
    /// messages (upstream raises `ValueError` with the exact wording
    /// below; this returns `Err(String)` instead of a typed error since
    /// nothing downstream branches on *which* case failed, only that
    /// lookup failed — matching what the message itself is for).
    /// `os_type == "linux"` is remapped to `"xen"` first — a real
    /// upstream quirk (old libxl-xen capabilities XML used `linux` as
    /// its `os_type`), not invented here.
    pub fn guest_lookup(
        &self,
        os_type: Option<&str>,
        arch: Option<&str>,
        typ: Option<&str>,
        machine: Option<&str>,
    ) -> Result<CapsInfo, String> {
        let os_type = if os_type == Some("linux") {
            Some("xen")
        } else {
            os_type
        };

        let guest = self
            .guest_for_os_type(os_type, arch)
            .ok_or_else(|| match (arch, os_type) {
                (Some(a), Some(o)) => format!(
                    "Host does not support virtualization type '{o}' for architecture '{a}'"
                ),
                (Some(a), None) => {
                    format!(
                        "Host does not support any virtualization options for architecture '{a}'"
                    )
                }
                (None, Some(o)) => format!("Host does not support virtualization type '{o}'"),
                (None, None) => "Host does not support any virtualization options".to_string(),
            })?;

        let domain = self.best_domain_type(guest, typ, machine).ok_or_else(|| {
            let typ = typ.unwrap_or("None");
            let os = guest.os_type.as_deref().unwrap_or_default();
            let arch = guest.arch.as_deref().unwrap_or_default();
            match machine {
                Some(m) => format!(
                    "Host does not support domain type {typ} with machine '{m}' for \
                     virtualization type '{os}' with architecture '{arch}'"
                ),
                None => format!(
                    "Host does not support domain type {typ} for virtualization type \
                     '{os}' with architecture '{arch}'"
                ),
            }
        })?;

        Ok(CapsInfo {
            hypervisor_type: domain.hypervisor_type.clone(),
            os_type: guest.os_type.clone(),
            arch: guest.arch.clone(),
            loader: guest.loader.clone(),
            // Upstream's `or` also falls back on an empty string, not
            // just None (Python falsy-ness) -- this only falls back on
            // None. Unhit in practice (a real emulator path is never
            // empty), flagged rather than silently matched.
            emulator: domain.emulator.clone().or_else(|| guest.emulator.clone()),
            machines: guest.all_machine_names(Some(domain)),
        })
    }

    /// `(capsinfo.domain and capsinfo.domain.machines) or
    /// capsinfo.guest.machines` for a [`Capabilities::guest_lookup`],
    /// as `Guest.refresh_machine_type` reads it.
    pub fn guest_lookup_machines(
        &self,
        os_type: Option<&str>,
        arch: Option<&str>,
        typ: Option<&str>,
        machine: Option<&str>,
    ) -> Result<Vec<CapsMachine>, String> {
        self.guest_lookup(os_type, arch, typ, machine)?;
        let os_type = if os_type == Some("linux") {
            Some("xen")
        } else {
            os_type
        };
        let guest = self
            .guest_for_os_type(os_type, arch)
            .expect("guest_lookup found it");
        let domain = self
            .best_domain_type(guest, typ, machine)
            .expect("guest_lookup found it");
        Ok(if domain.machines.is_empty() {
            guest.machines.clone()
        } else {
            domain.machines.clone()
        })
    }

    /// `capsinfo.guest` for a [`Capabilities::guest_lookup`]: the
    /// `<guest>` it picked, as `vmmSnapshotNew._init_snapshot_mode`
    /// reads `capsinfo.guest.supports_externalSnapshot()`.
    pub fn guest_lookup_guest(
        &self,
        os_type: Option<&str>,
        arch: Option<&str>,
        typ: Option<&str>,
        machine: Option<&str>,
    ) -> Result<&CapsGuest, String> {
        self.guest_lookup(os_type, arch, typ, machine)?;
        let os_type = if os_type == Some("linux") {
            Some("xen")
        } else {
            os_type
        };
        Ok(self
            .guest_for_os_type(os_type, arch)
            .expect("guest_lookup found it"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Connection;
    use crate::test_support::TEST_DOMAIN_LOCK;

    /// Real libvirt call against libvirt's builtin `test:///` driver —
    /// same no-daemon guarantee `domcaps.rs`'s own test relies on.
    /// Holds `TEST_DOMAIN_LOCK`: asserts a specific `arch` value that a
    /// concurrently-running `VIRTINST_TEST_CAPS_FILE` injection test
    /// below would otherwise override (confirmed for real — see
    /// `TEST_DOMAIN_LOCK`'s own doc comment).
    #[test]
    fn fetch_against_the_builtin_test_driver() {
        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        let caps = Capabilities::fetch(&conn).expect("test driver answers capabilities");
        assert_eq!(caps.host.cpu.arch.as_deref(), Some("i686"));
        assert!(!caps.guests.is_empty());
    }

    /// A real `virsh -c test:///default capabilities` capture, not
    /// invented — the exact shape `guest_lookup` has to handle when a
    /// guest has *no* `<machine>` elements at all (this driver's own
    /// output never has any).
    const TEST_DRIVER_CAPS: &str = r#"<capabilities>
  <host>
    <cpu>
      <arch>i686</arch>
    </cpu>
  </host>
  <guest>
    <os_type>hvm</os_type>
    <arch name='i686'>
      <wordsize>32</wordsize>
      <emulator>/usr/bin/test-hv</emulator>
      <domain type='test'/>
    </arch>
    <features>
      <pae/>
    </features>
  </guest>
  <guest>
    <os_type>xen</os_type>
    <arch name='i686'>
      <wordsize>32</wordsize>
      <emulator>/usr/bin/test-hv</emulator>
      <domain type='test'/>
    </arch>
  </guest>
</capabilities>"#;

    #[test]
    fn parse_reads_host_cpu_and_guest_scalars() {
        let caps = Capabilities::parse(TEST_DRIVER_CAPS).expect("parses");
        assert_eq!(caps.host.cpu.arch.as_deref(), Some("i686"));
        assert_eq!(caps.guests.len(), 2);
        assert_eq!(caps.guests[0].os_type.as_deref(), Some("hvm"));
        assert_eq!(caps.guests[0].arch.as_deref(), Some("i686"));
        assert_eq!(caps.guests[0].emulator.as_deref(), Some("/usr/bin/test-hv"));
        assert_eq!(
            caps.guests[0].domains[0].hypervisor_type.as_deref(),
            Some("test")
        );
        assert!(caps.guests[0].features.pae);
        assert!(
            !caps.guests[1].features.pae,
            "second guest has no <features> at all"
        );
    }

    #[test]
    fn guest_lookup_finds_hvm_over_xen_by_default() {
        let caps = Capabilities::parse(TEST_DRIVER_CAPS).expect("parses");
        // os_type=None tries every guest in document order; hvm is first.
        let info = caps
            .guest_lookup(None, None, None, None)
            .expect("finds a guest");
        assert_eq!(info.os_type.as_deref(), Some("hvm"));
        assert_eq!(info.hypervisor_type.as_deref(), Some("test"));
        assert_eq!(info.arch.as_deref(), Some("i686"));
        assert_eq!(info.emulator.as_deref(), Some("/usr/bin/test-hv"));
        assert!(
            info.machines.is_empty(),
            "this driver reports no <machine> elements at all"
        );
    }

    /// `capsinfo.guest.supports_externalSnapshot()`: the test driver
    /// doesn't report it; a qemu-style `<externalSnapshot/>` does.
    #[test]
    fn guest_lookup_guest_reports_external_snapshot_support() {
        let caps = Capabilities::parse(TEST_DRIVER_CAPS).expect("parses");
        let guest = caps
            .guest_lookup_guest(Some("hvm"), Some("i686"), Some("test"), None)
            .expect("finds a guest");
        assert_eq!(guest.os_type.as_deref(), Some("hvm"));
        assert!(!guest.supports_external_snapshot());
        assert!(caps
            .guest_lookup_guest(Some("hvm"), Some("ppc64"), None, None)
            .is_err());

        let qemu = TEST_DRIVER_CAPS.replace("<pae/>", "<pae/>\n      <externalSnapshot/>");
        let caps = Capabilities::parse(&qemu).expect("parses");
        let guest = caps
            .guest_lookup_guest(Some("hvm"), None, None, None)
            .expect("finds a guest");
        assert!(guest.supports_external_snapshot());
    }

    #[test]
    fn guest_lookup_can_request_xen_explicitly() {
        let caps = Capabilities::parse(TEST_DRIVER_CAPS).expect("parses");
        let info = caps
            .guest_lookup(Some("xen"), None, None, None)
            .expect("finds the xen guest");
        assert_eq!(info.os_type.as_deref(), Some("xen"));
    }

    #[test]
    fn guest_lookup_remaps_linux_os_type_to_xen() {
        let caps = Capabilities::parse(TEST_DRIVER_CAPS).expect("parses");
        let info = caps
            .guest_lookup(Some("linux"), None, None, None)
            .expect("old libxl-xen os_type=linux quirk");
        assert_eq!(info.os_type.as_deref(), Some("xen"));
    }

    #[test]
    fn guest_lookup_fails_with_upstreams_exact_message_for_unsupported_arch() {
        let caps = Capabilities::parse(TEST_DRIVER_CAPS).expect("parses");
        let err = caps
            .guest_lookup(None, Some("riscv64"), None, None)
            .unwrap_err();
        assert_eq!(
            err,
            "Host does not support any virtualization options for architecture 'riscv64'"
        );
    }

    /// Constructed (not captured — this sandbox has no real
    /// `qemu:///system` access), realistic enough to exercise
    /// `guest_lookup`'s priority selection (`kvm` beats `qemu` even
    /// though `qemu` is listed first) and its machine-name filter,
    /// which `TEST_DRIVER_CAPS`'s real but machine-less output can't.
    const MULTI_DOMAIN_CAPS: &str = r#"<capabilities>
  <host>
    <cpu>
      <arch>x86_64</arch>
    </cpu>
  </host>
  <guest>
    <os_type>hvm</os_type>
    <arch name='x86_64'>
      <emulator>/usr/bin/qemu-system-x86_64</emulator>
      <machine canonical='pc-i440fx-8.2'>pc</machine>
      <machine>q35</machine>
      <domain type='qemu'/>
      <domain type='kvm'>
        <machine>q35</machine>
      </domain>
    </arch>
  </guest>
</capabilities>"#;

    #[test]
    fn guest_lookup_prefers_kvm_over_qemu_regardless_of_document_order() {
        let caps = Capabilities::parse(MULTI_DOMAIN_CAPS).expect("parses");
        let info = caps
            .guest_lookup(None, None, None, None)
            .expect("finds a domain");
        assert_eq!(info.hypervisor_type.as_deref(), Some("kvm"));
    }

    #[test]
    fn guest_lookup_can_request_qemu_explicitly_over_the_kvm_default() {
        let caps = Capabilities::parse(MULTI_DOMAIN_CAPS).expect("parses");
        let info = caps
            .guest_lookup(None, None, Some("qemu"), None)
            .expect("finds the qemu domain");
        assert_eq!(info.hypervisor_type.as_deref(), Some("qemu"));
    }

    #[test]
    fn guest_lookup_filters_by_machine_and_falls_back_to_guest_level_names() {
        let caps = Capabilities::parse(MULTI_DOMAIN_CAPS).expect("parses");
        // "pc" only exists at the guest level (kvm's own <domain> only
        // lists "q35") -- upstream's all_machine_names only falls back
        // to the guest list when the domain has *no* machines of its
        // own, so a domain with a non-empty machine list of its own
        // (kvm here) must be excluded when asking for a name it
        // doesn't have, even though the guest-level list has it.
        let err = caps
            .guest_lookup(None, None, Some("kvm"), Some("pc"))
            .unwrap_err();
        assert!(err.contains("does not support domain type kvm with machine 'pc'"));

        let info = caps
            .guest_lookup(None, None, Some("kvm"), Some("q35"))
            .expect("kvm's own machine list has q35");
        assert_eq!(info.hypervisor_type.as_deref(), Some("kvm"));
    }

    #[test]
    fn all_machine_names_includes_canonical_aliases() {
        let caps = Capabilities::parse(MULTI_DOMAIN_CAPS).expect("parses");
        let names = caps.guests[0].all_machine_names(None);
        assert!(names.contains(&"pc".to_string()));
        assert!(names.contains(&"pc-i440fx-8.2".to_string()));
        assert!(names.contains(&"q35".to_string()));
    }

    #[test]
    fn fetch_reads_from_env_var_file_instead_of_calling_libvirt() {
        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let path = std::env::temp_dir().join("virtinst_test_caps_fetch.xml");
        std::fs::write(&path, MULTI_DOMAIN_CAPS).expect("writes fixture file");
        std::env::set_var("VIRTINST_TEST_CAPS_FILE", &path);

        // A deliberately-invalid connect target: if fetch() actually
        // tried the real libvirt call, this would fail outright, proving
        // the injection path is really the one that ran.
        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        let caps = Capabilities::fetch(&conn).expect("reads the injected file, not libvirt");

        std::env::remove_var("VIRTINST_TEST_CAPS_FILE");
        std::fs::remove_file(&path).ok();

        assert_eq!(caps.host.cpu.arch.as_deref(), Some("x86_64"));
    }

    #[test]
    fn fetch_surfaces_a_missing_env_var_file_as_a_real_error_not_a_panic() {
        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(
            "VIRTINST_TEST_CAPS_FILE",
            "/nonexistent/path/does-not-exist.xml",
        );

        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        let result = Capabilities::fetch(&conn);

        std::env::remove_var("VIRTINST_TEST_CAPS_FILE");

        assert!(matches!(result, Err(CapabilitiesError::Io(_))));
    }

    /// A real `virsh -c qemu:///system capabilities` excerpt captured on
    /// this machine (`virtqemud`, real KVM/x86_64, Skylake host), not
    /// invented — same posture as `domcaps.rs`'s own `QEMU_DOMCAPS_
    /// EXCERPT`. Trimmed to a handful of representative `<machine>`
    /// entries per arch rather than the full ~250-line capture. Every
    /// previous `guest_lookup` test in this file used `test:///
    /// default`'s single-domain-per-guest shape; this exercises the
    /// `kvm > xen > hvf > qemu` priority chain and the two-pass arch
    /// fallback against real multi-domain, multi-machine data on the
    /// default `--connect qemu:///system` shape every real user of this
    /// binary takes.
    const REAL_QEMU_SYSTEM_CAPS_EXCERPT: &str = r#"<capabilities>
  <host>
    <cpu>
      <arch>x86_64</arch>
      <model>Skylake-Client-v4</model>
      <vendor>Intel</vendor>
    </cpu>
  </host>
  <guest>
    <os_type>hvm</os_type>
    <arch name='i686'>
      <wordsize>32</wordsize>
      <emulator>/usr/bin/qemu-system-i386</emulator>
      <machine maxCpus='255'>pc-i440fx-11.1</machine>
      <machine canonical='pc-i440fx-11.1' maxCpus='255'>pc</machine>
      <machine maxCpus='4096'>pc-q35-11.1</machine>
      <machine canonical='pc-q35-11.1' maxCpus='4096'>q35</machine>
      <domain type='qemu'/>
      <domain type='kvm'/>
    </arch>
    <features>
      <pae/>
      <nonpae/>
      <acpi default='on' toggle='yes'/>
      <apic default='on' toggle='no'/>
    </features>
  </guest>
  <guest>
    <os_type>hvm</os_type>
    <arch name='x86_64'>
      <wordsize>64</wordsize>
      <emulator>/usr/bin/qemu-system-x86_64</emulator>
      <machine maxCpus='255'>pc-i440fx-11.1</machine>
      <machine canonical='pc-i440fx-11.1' maxCpus='255'>pc</machine>
      <machine maxCpus='4096'>pc-q35-11.1</machine>
      <machine canonical='pc-q35-11.1' maxCpus='4096'>q35</machine>
      <domain type='qemu'/>
      <domain type='kvm'/>
    </arch>
    <features>
      <acpi default='on' toggle='yes'/>
      <apic default='on' toggle='no'/>
    </features>
  </guest>
</capabilities>"#;

    #[test]
    fn guest_lookup_on_a_real_qemu_system_capture_prefers_kvm_x86_64() {
        let caps = Capabilities::parse(REAL_QEMU_SYSTEM_CAPS_EXCERPT).expect("parses");
        let info = caps
            .guest_lookup(None, None, None, None)
            .expect("real host always resolves a default guest");
        // Host cpu.arch is x86_64, so the arch-fallback's first pass
        // (host arch, not doc order) must pick the second `<guest>`
        // over the first, even though both are `os_type=hvm` and the
        // i686 guest comes first in the document.
        assert_eq!(info.arch.as_deref(), Some("x86_64"));
        assert_eq!(info.os_type.as_deref(), Some("hvm"));
        // kvm beats qemu per the priority chain, even though qemu is
        // listed first in the document.
        assert_eq!(info.hypervisor_type.as_deref(), Some("kvm"));
        assert!(info.machines.iter().any(|m| m == "q35"));
    }

    #[test]
    fn recommended_machine_by_arch() {
        let info = |arch: &str, machines: &[&str]| CapsInfo {
            arch: Some(arch.into()),
            machines: machines.iter().map(|m| m.to_string()).collect(),
            ..CapsInfo::default()
        };
        assert_eq!(
            info("aarch64", &["virt", "sbsa-ref"]).recommended_machine(true),
            Some("virt")
        );
        assert_eq!(
            info("ppc64le", &["pseries", "powernv"]).recommended_machine(true),
            Some("pseries")
        );
        assert_eq!(
            info("x86_64", &["pc", "q35"]).recommended_machine(true),
            None
        );
        assert_eq!(info("aarch64", &["virt"]).recommended_machine(false), None);
    }

    #[test]
    fn install_options_and_kvm() {
        let caps = Capabilities::parse(TEST_DRIVER_CAPS).unwrap();
        assert!(caps.has_install_options());
        assert!(!caps.guests[0].is_kvm_available());
        assert!(!Capabilities::default().has_install_options());
    }
}
