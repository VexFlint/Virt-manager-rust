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
//! Not modeled on this struct: `set_special_mode`/the security-
//! mitigation helpers (`_add_security_features` etc.) — business logic
//! over the bound fields, ticket 17 territory, same as everywhere
//! else. **`set_defaults`'s actual logic is built, but lives in
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
