//! `<cpu>` — upstream's `domain/cpu.py`, the largest domain-subsystem
//! file (531 lines, 10 upstream classes). Every field bound here is a
//! flat attribute or a plain `#[xml(list)]`, same shapes as everywhere
//! else — the size is from breadth (topology/cache/features/NUMA
//! cells/interconnects/maxphysaddr), not new macro capability.
//!
//! **`topology`/`cache`/`maxphysaddr` are `is_single=True` in
//! upstream** — exactly one `<topology>`/`<cache>`/`<maxphysaddr>`
//! child, not a list. This macro has no "nested single struct" field
//! kind (only flat self/path attributes or `Vec<T>` lists) — same gap
//! `char.rs`'s `CharSource` and `smartcard.rs`'s `source_*` fields
//! already worked around by flattening the nested struct's fields
//! directly onto the parent with a `path` prefix, rather than a nested
//! `topology: CpuTopology` field. Same move here: `topology_sockets`,
//! `cache_level`, `maxphysaddr_mode`, etc.
//!
//! **`<cpu><cache>` vs `<cpu><numa><cell><cache>` are two unrelated
//! elements upstream names identically** (`_CPUCache` vs
//! `_NUMACellCache`) — kept as two distinct Rust types
//! (`cache_level`/`cache_mode` flattened fields on `DomainCpu` itself,
//! vs `CpuNumaCellCache` nested inside `CpuNumaCell`); no collision
//! since each only ever resolves relative to its own parent element.
//!
//! **NUMA nesting, grounded in the real `XMLChildProperty` calls, not
//! the class doc-comments** (which say `<cell><interconnects>`, but the
//! actual declarations on `DomainCpu` say otherwise): `cells` is
//! `relative_xpath="./numa"` (cells are direct children of `<numa>`);
//! `latencies`/`bandwidths` are both `relative_xpath="./numa/interconnects"`
//! — siblings of `<cell>` under `<numa>`, not nested inside a cell.
//! Only `siblings` (`_NUMACellSibling`, the distance table) actually
//! nests under an individual cell's own `<distances>`.
//!
//! `set_special_mode`/`set_model` and the security-mitigation helpers
//! (`_add_security_features` etc.) are the `edit_*` functions at the
//! end of this file, editing the document directly (the details
//! window's CPU page uses them). **`set_defaults`'s actual logic is built, but lives in
//! `capabilities::domcaps::DomainCapabilities::default_cpu_mode`
//! instead of here** (ticket 21 slice (f)) — this crate builds domain
//! XML through direct `edit_xml::Document`/`Element` construction
//! (`install/domain_skeleton.rs`'s `push_default_cpu`), not through
//! `DomainCpu`'s bound-struct setters, so there's no `self.mode =` on
//! `DomainCpu` for a `set_defaults` here to mutate. A future consumer
//! of this struct (e.g. a `virt-xml --cpu` editor) should look there
//! first rather than re-porting `domain/cpu.py`'s defaulting chain a
//! second time onto this type.
//! `match` is a Rust keyword; the field is `cpu_match`, bound to the
//! `match` attribute by string, same as every other field's name
//! already differing freely from its XML attribute name.

use virtinst_xml::{OnOff, XmlBound};

/// `<cpu><numa><cell><distances><sibling>` — upstream's
/// `_NUMACellSibling`.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "sibling")]
pub struct CpuNumaCellSibling {
    #[xml(attribute = "id")]
    pub id: Option<u32>,

    #[xml(attribute = "value")]
    pub value: Option<u32>,
}

/// `<cpu><numa><cell><cache>` — upstream's `_NUMACellCache`. A
/// different element from `DomainCpu`'s own flattened `cache_*`
/// fields (`<cpu><cache>`) despite the identical upstream class-naming
/// convention — see this module's doc comment.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "cache")]
pub struct CpuNumaCellCache {
    #[xml(attribute = "level")]
    pub level: Option<u32>,

    #[xml(attribute = "associativity")]
    pub associativity: Option<String>,

    #[xml(attribute = "policy")]
    pub policy: Option<String>,

    #[xml(path = "size", attribute = "value")]
    pub size_value: Option<u32>,

    #[xml(path = "size", attribute = "unit")]
    pub size_unit: Option<String>,

    #[xml(path = "line", attribute = "value")]
    pub line_value: Option<u32>,

    #[xml(path = "line", attribute = "unit")]
    pub line_unit: Option<String>,
}

/// `<cpu><numa><cell>` — upstream's `_NUMACell`.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "cell")]
pub struct CpuNumaCell {
    #[xml(attribute = "id")]
    pub id: Option<u32>,

    #[xml(attribute = "cpus")]
    pub cpus: Option<String>,

    #[xml(attribute = "memory")]
    pub memory: Option<u32>,

    #[xml(attribute = "unit")]
    pub unit: Option<String>,

    #[xml(attribute = "memAccess")]
    pub mem_access: Option<String>,

    #[xml(attribute = "discard")]
    pub discard: Option<bool>,

    #[xml(path = "distances", list)]
    pub siblings: Vec<CpuNumaCellSibling>,

    #[xml(list)]
    pub caches: Vec<CpuNumaCellCache>,
}

/// `<cpu><numa><interconnects><latency>` — upstream's `_NUMALatency`.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "latency")]
pub struct CpuNumaLatency {
    #[xml(attribute = "initiator")]
    pub initiator: Option<u32>,

    #[xml(attribute = "target")]
    pub target: Option<u32>,

    #[xml(attribute = "cache")]
    pub cache: Option<u32>,

    #[xml(attribute = "type")]
    pub latency_type: Option<String>,

    #[xml(attribute = "value")]
    pub value: Option<u32>,

    #[xml(attribute = "unit")]
    pub unit: Option<String>,
}

/// `<cpu><numa><interconnects><bandwidth>` — upstream's
/// `_NUMABandwidth`. Same field shape as [`CpuNumaLatency`]; kept as a
/// distinct type since upstream does too (and libvirt tolerates a
/// `cache=` attribute on `<bandwidth>` only by upstream's own
/// admission it's not actually documented there).
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "bandwidth")]
pub struct CpuNumaBandwidth {
    #[xml(attribute = "initiator")]
    pub initiator: Option<u32>,

    #[xml(attribute = "target")]
    pub target: Option<u32>,

    #[xml(attribute = "cache")]
    pub cache: Option<u32>,

    #[xml(attribute = "type")]
    pub bandwidth_type: Option<String>,

    #[xml(attribute = "value")]
    pub value: Option<u32>,

    #[xml(attribute = "unit")]
    pub unit: Option<String>,
}

/// `<cpu><feature>` — upstream's `_CPUFeature`.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "feature")]
pub struct CpuFeature {
    #[xml(attribute = "name")]
    pub name: Option<String>,

    #[xml(attribute = "policy")]
    pub policy: Option<String>,
}

/// ```xml
/// <cpu mode="host-model" match="exact" migratable="on">
///   <model fallback="allow">Skylake-Client</model>
///   <topology sockets="1" dies="1" cores="4" threads="2"/>
///   <cache level="3" mode="emulate"/>
///   <feature policy="require" name="vmx"/>
///   <numa>
///     <cell id="0" cpus="0-7" memory="4194304" unit="KiB"/>
///     <interconnects>
///       <latency initiator="0" target="0" type="access" value="10"/>
///     </interconnects>
///   </numa>
/// </cpu>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "cpu")]
pub struct DomainCpu {
    #[xml(attribute = "mode")]
    pub mode: Option<String>,

    #[xml(attribute = "match")]
    pub cpu_match: Option<String>,

    #[xml(attribute = "check")]
    pub check: Option<String>,

    #[xml(attribute = "migratable")]
    pub migratable: Option<OnOff>,

    #[xml(path = "model", text)]
    pub model: Option<String>,

    #[xml(path = "model", attribute = "fallback")]
    pub model_fallback: Option<String>,

    #[xml(path = "model", attribute = "vendor_id")]
    pub model_vendor_id: Option<String>,

    #[xml(path = "vendor", text)]
    pub vendor: Option<String>,

    // <topology> is is_single=True upstream — flattened, see module doc.
    #[xml(path = "topology", attribute = "sockets")]
    pub topology_sockets: Option<u32>,

    #[xml(path = "topology", attribute = "dies")]
    pub topology_dies: Option<u32>,

    #[xml(path = "topology", attribute = "cores")]
    pub topology_cores: Option<u32>,

    #[xml(path = "topology", attribute = "threads")]
    pub topology_threads: Option<u32>,

    // <cpu><cache> is also is_single=True — flattened, distinct from
    // CpuNumaCellCache (<cpu><numa><cell><cache>) above.
    #[xml(path = "cache", attribute = "level")]
    pub cache_level: Option<u32>,

    #[xml(path = "cache", attribute = "mode")]
    pub cache_mode: Option<String>,

    // <maxphysaddr> is is_single=True — flattened.
    #[xml(path = "maxphysaddr", attribute = "mode")]
    pub maxphysaddr_mode: Option<String>,

    #[xml(path = "maxphysaddr", attribute = "bits")]
    pub maxphysaddr_bits: Option<u32>,

    #[xml(list)]
    pub features: Vec<CpuFeature>,

    #[xml(path = "numa", list)]
    pub cells: Vec<CpuNumaCell>,

    #[xml(path = "numa/interconnects", list)]
    pub latencies: Vec<CpuNumaLatency>,

    #[xml(path = "numa/interconnects", list)]
    pub bandwidths: Vec<CpuNumaBandwidth>,
}

// Upstream `DomainCpu`'s editing half: the special modes, `set_model`
// with its security-feature handling, and topology. These edit a
// domain document in place, under its `<cpu>` (created on demand,
// dropped again once empty, as upstream's XML API does), because the
// bound struct above only ever writes values, never removes them.

use virtinst_xml::{Document, Element};

pub const SPECIAL_MODE_HOST_MODEL_ONLY: &str = "host-model-only";
pub const SPECIAL_MODE_HV_DEFAULT: &str = "hv-default";
pub const SPECIAL_MODE_HOST_COPY: &str = "host-copy";
pub const SPECIAL_MODE_HOST_MODEL: &str = "host-model";
pub const SPECIAL_MODE_HOST_PASSTHROUGH: &str = "host-passthrough";
pub const SPECIAL_MODE_MAXIMUM: &str = "maximum";
pub const SPECIAL_MODE_CLEAR: &str = "clear";
pub const SPECIAL_MODE_APP_DEFAULT: &str = "default";
pub const SPECIAL_MODES: [&str; 8] = [
    SPECIAL_MODE_HOST_MODEL_ONLY,
    SPECIAL_MODE_HV_DEFAULT,
    SPECIAL_MODE_HOST_COPY,
    SPECIAL_MODE_HOST_MODEL,
    SPECIAL_MODE_HOST_PASSTHROUGH,
    SPECIAL_MODE_MAXIMUM,
    SPECIAL_MODE_CLEAR,
    SPECIAL_MODE_APP_DEFAULT,
];

/// What the edits need from the connection: the resolved
/// `_get_app_default_mode`, `conn.caps.host.cpu.model`, and
/// `domcaps.get_cpu_security_features()`.
#[derive(Debug, Clone, Default)]
pub struct CpuEditContext {
    pub app_default_mode: String,
    pub host_cpu_model: Option<String>,
    pub security_features: Vec<String>,
}

impl DomainCpu {
    /// `has_topology`.
    pub fn has_topology(&self) -> bool {
        self.topology_sockets.is_some()
            || self.topology_dies.is_some()
            || self.topology_cores.is_some()
            || self.topology_threads.is_some()
    }

    /// `check_security_features`: the value of the (not stored in the
    /// XML) `secure` flag for this CPU.
    pub fn check_security_features(&self, features: &[String]) -> bool {
        if features.is_empty() {
            return false;
        }
        let mut guest_features: Vec<&str> = self
            .features
            .iter()
            .filter(|f| f.policy.as_deref() == Some("require"))
            .filter_map(|f| f.name.as_deref())
            .collect();
        if let Some(model) = &self.model {
            if model.ends_with("IBRS") {
                guest_features.push("spec-ctrl");
            }
            if model.ends_with("IBPB") {
                guest_features.push("ibpb");
            }
        }
        features
            .iter()
            .all(|f| guest_features.contains(&f.as_str()))
    }
}

fn cpu_element(doc: &mut Document, domain: Element) -> Element {
    virtinst_xml::resolve_or_create_path(doc, domain, &["cpu"])
}

fn set_attr(doc: &mut Document, el: Element, name: &str, val: Option<&str>) {
    match val {
        Some(v) => el.set_attribute(doc, name, v),
        None => {
            el.mut_attributes(doc).remove(name);
        }
    }
}

fn remove_child(doc: &mut Document, el: Element, tag: &str) {
    virtinst_xml::remove_path_if_present(doc, el, &[tag])
        .expect("removing a plain child element cannot fail");
}

/// Drops `el` once nothing is left in it.
fn remove_if_empty(doc: &mut Document, el: Element) {
    if el.attributes(doc).is_empty() && el.child_elements(doc).is_empty() {
        let _ = el.detach(doc);
    }
}

/// `DomainCpu.clear()`: no `<cpu>` at all.
pub fn edit_clear(doc: &mut Document, domain: Element) {
    remove_child(doc, domain, "cpu");
}

/// `topology.clear()`.
pub fn edit_clear_topology(doc: &mut Document, domain: Element) {
    if let Some(cpu) = domain.find(doc, "cpu") {
        remove_child(doc, cpu, "topology");
        remove_if_empty(doc, cpu);
    }
}

/// `topology.sockets/cores/threads = ...`.
pub fn edit_set_topology(
    doc: &mut Document,
    domain: Element,
    sockets: u32,
    cores: u32,
    threads: u32,
) {
    let cpu = cpu_element(doc, domain);
    let top = virtinst_xml::resolve_or_create_path(doc, cpu, &["topology"]);
    top.set_attribute(doc, "sockets", sockets.to_string());
    top.set_attribute(doc, "cores", cores.to_string());
    top.set_attribute(doc, "threads", threads.to_string());
}

fn add_security_features(doc: &mut Document, cpu: Element, ctx: &CpuEditContext) {
    for feature in &ctx.security_features {
        let exists = cpu
            .find_all(doc, "feature")
            .iter()
            .any(|f| f.attribute(doc, "name") == Some(feature));
        if !exists {
            let f = Element::new(doc, "feature");
            f.set_attribute(doc, "policy", "require");
            f.set_attribute(doc, "name", feature.as_str());
            cpu.push_child(doc, f)
                .expect("a fresh element always attaches");
        }
    }
}

fn remove_security_features(doc: &mut Document, cpu: Element, ctx: &CpuEditContext) {
    for feature in &ctx.security_features {
        let found = cpu.find_all(doc, "feature").into_iter().find(|f| {
            f.attribute(doc, "name") == Some(feature)
                && f.attribute(doc, "policy") == Some("require")
        });
        if let Some(f) = found {
            let _ = f.detach(doc);
        }
    }
}

/// `set_model`.
pub fn edit_set_model(
    doc: &mut Document,
    domain: Element,
    ctx: &CpuEditContext,
    val: Option<&str>,
    secure: bool,
) {
    let val = val.filter(|v| !v.is_empty());
    let cpu = cpu_element(doc, domain);
    set_attr(doc, cpu, "migratable", None);
    if val.is_some() {
        cpu.set_attribute(doc, "mode", "custom");
        if cpu.attribute(doc, "match").is_none() {
            cpu.set_attribute(doc, "match", "exact");
        }
        if secure {
            add_security_features(doc, cpu, ctx);
        } else {
            remove_security_features(doc, cpu, ctx);
        }
    }
    match val {
        Some(v) => {
            let model = virtinst_xml::resolve_or_create_path(doc, cpu, &["model"]);
            model.set_text_content(doc, v);
        }
        None => remove_child(doc, cpu, "model"),
    }
    remove_if_empty(doc, cpu);
}

/// `set_special_mode`.
pub fn edit_set_special_mode(
    doc: &mut Document,
    domain: Element,
    ctx: &CpuEditContext,
    val: &str,
    secure: bool,
) -> Result<(), String> {
    let val = if val == SPECIAL_MODE_APP_DEFAULT {
        ctx.app_default_mode.as_str()
    } else {
        val
    };
    match val {
        SPECIAL_MODE_HOST_MODEL | SPECIAL_MODE_HOST_PASSTHROUGH | SPECIAL_MODE_MAXIMUM => {
            let cpu = cpu_element(doc, domain);
            for tag in ["model", "vendor"] {
                remove_child(doc, cpu, tag);
            }
            for attr in ["migratable", "check"] {
                set_attr(doc, cpu, attr, None);
            }
            for f in cpu.find_all(doc, "feature") {
                let _ = f.detach(doc);
            }
            cpu.set_attribute(doc, "mode", val);
        }
        SPECIAL_MODE_HV_DEFAULT | SPECIAL_MODE_CLEAR => edit_clear(doc, domain),
        SPECIAL_MODE_HOST_MODEL_ONLY | SPECIAL_MODE_HOST_COPY => {
            if let Some(model) = &ctx.host_cpu_model {
                edit_clear(doc, domain);
                edit_set_model(doc, domain, ctx, Some(model), secure);
            }
        }
        _ => return Err(format!("unknown special cpu mode '{val}'")),
    }
    Ok(())
}

#[cfg(test)]
mod edit_tests {
    use super::*;

    fn doc(xml: &str) -> (Document, Element) {
        let doc = virtinst_xml::parse_libvirt_xml(xml).unwrap();
        let root = doc.root_element().unwrap();
        (doc, root)
    }

    fn out(doc: &Document) -> String {
        doc.write_str().unwrap()
    }

    fn ctx() -> CpuEditContext {
        CpuEditContext {
            app_default_mode: SPECIAL_MODE_HOST_PASSTHROUGH.into(),
            host_cpu_model: Some("Skylake-Client".into()),
            security_features: vec!["ssbd".into(), "md-clear".into()],
        }
    }

    #[test]
    fn host_passthrough_strips_the_custom_model() {
        let (mut d, root) = doc("<domain><cpu mode='custom' match='exact' check='partial'>\
             <model fallback='allow'>Haswell</model><feature policy='require' name='ssbd'/>\
             <topology sockets='1' cores='2' threads='1'/></cpu></domain>");
        edit_set_special_mode(&mut d, root, &ctx(), SPECIAL_MODE_APP_DEFAULT, true).unwrap();
        let x = out(&d);
        assert!(x.contains("mode=\"host-passthrough\""), "{x}");
        assert!(
            !x.contains("model") && !x.contains("feature") && !x.contains("check"),
            "{x}"
        );
        assert!(x.contains("<topology"), "{x}");
    }

    #[test]
    fn clear_and_hv_default_drop_the_cpu() {
        let (mut d, root) = doc("<domain><cpu mode='host-model'/></domain>");
        edit_set_special_mode(&mut d, root, &ctx(), SPECIAL_MODE_HV_DEFAULT, true).unwrap();
        assert!(!out(&d).contains("cpu"));
        assert!(edit_set_special_mode(&mut d, root, &ctx(), "bogus", true).is_err());
    }

    #[test]
    fn set_model_adds_and_removes_security_features() {
        let (mut d, root) = doc("<domain><name>x</name></domain>");
        edit_set_model(&mut d, root, &ctx(), Some("Haswell"), true);
        let x = out(&d);
        assert!(
            x.contains("mode=\"custom\"") && x.contains("match=\"exact\""),
            "{x}"
        );
        assert!(x.contains("<model>Haswell</model>"), "{x}");
        assert_eq!(x.matches("policy=\"require\"").count(), 2, "{x}");

        let guest = crate::guest::Guest::from_element(&d, root);
        assert!(guest.cpu.check_security_features(&ctx().security_features));

        edit_set_model(&mut d, root, &ctx(), Some("Haswell"), false);
        let x = out(&d);
        assert!(!x.contains("feature"), "{x}");
        let guest = crate::guest::Guest::from_element(&d, root);
        assert!(!guest.cpu.check_security_features(&ctx().security_features));
    }

    #[test]
    fn host_model_only_pins_the_host_model() {
        let (mut d, root) = doc("<domain><cpu mode='maximum'/></domain>");
        edit_set_special_mode(&mut d, root, &ctx(), SPECIAL_MODE_HOST_MODEL_ONLY, false).unwrap();
        let x = out(&d);
        assert!(x.contains("<model>Skylake-Client</model>"), "{x}");
        assert!(!x.contains("maximum"), "{x}");
    }

    #[test]
    fn topology_set_and_clear() {
        let (mut d, root) = doc("<domain/>");
        edit_set_topology(&mut d, root, 2, 3, 1);
        let guest = crate::guest::Guest::from_element(&d, root);
        assert!(guest.cpu.has_topology());
        assert_eq!(guest.cpu.topology_cores, Some(3));
        edit_clear_topology(&mut d, root);
        assert!(!out(&d).contains("cpu"), "{}", out(&d));
    }
}
