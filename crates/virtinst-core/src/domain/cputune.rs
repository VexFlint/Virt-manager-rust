//! `<cputune>` — upstream's `domain/cputune.py`. The largest struct
//! tree so far by file count (10 upstream classes), but every field is
//! a flat attribute or a plain `#[xml(list)]` — no predicates, no
//! parent-axis paths, nothing beyond shapes already proven elsewhere.
//! Grouped the same four ways upstream's own file comments do: CPU
//! pinning, scheduling, cache & memory tunables, and the top-level
//! resource-quota scalars.

use virtinst_xml::XmlBound;

// --- CPU pinning ---

/// `<vcpupin>` — upstream's `_VCPUPin`.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "vcpupin")]
pub struct CputuneVcpuPin {
    #[xml(attribute = "vcpu")]
    pub vcpu: Option<u32>,

    #[xml(attribute = "cpuset")]
    pub cpuset: Option<String>,
}

/// `<iothreadpin>` — upstream's `_IOThreadPin`.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "iothreadpin")]
pub struct CputuneIothreadPin {
    #[xml(attribute = "iothread")]
    pub iothread: Option<u32>,

    #[xml(attribute = "cpuset")]
    pub cpuset: Option<String>,
}

// --- Scheduling ---

/// `<vcpusched>` — upstream's `_VCPUSched`.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "vcpusched")]
pub struct CputuneVcpuSched {
    #[xml(attribute = "vcpus")]
    pub vcpus: Option<String>,

    #[xml(attribute = "scheduler")]
    pub scheduler: Option<String>,

    #[xml(attribute = "priority")]
    pub priority: Option<u32>,
}

/// `<iothreadsched>` — upstream's `_IOThreadSched`.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "iothreadsched")]
pub struct CputuneIothreadSched {
    #[xml(attribute = "iothreads")]
    pub iothreads: Option<String>,

    #[xml(attribute = "scheduler")]
    pub scheduler: Option<String>,

    #[xml(attribute = "priority")]
    pub priority: Option<u32>,
}

// --- Cache & memory tunables ---

/// `<cachetune><cache>` — upstream's `_CacheTuneCache`.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "cache")]
pub struct CachetuneCache {
    #[xml(attribute = "id")]
    pub id: Option<u32>,

    #[xml(attribute = "level")]
    pub level: Option<u32>,

    #[xml(attribute = "type")]
    pub cache_type: Option<String>,

    #[xml(attribute = "size")]
    pub size: Option<u32>,

    #[xml(attribute = "unit")]
    pub unit: Option<String>,
}

/// `<cachetune><monitor>` — upstream's `_CacheTuneMonitor`.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "monitor")]
pub struct CachetuneMonitor {
    #[xml(attribute = "level")]
    pub level: Option<u32>,

    #[xml(attribute = "vcpus")]
    pub vcpus: Option<String>,
}

/// `<cachetune>` — upstream's `_CacheTune`.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "cachetune")]
pub struct Cachetune {
    #[xml(attribute = "vcpus")]
    pub vcpus: Option<String>,

    #[xml(list)]
    pub caches: Vec<CachetuneCache>,

    #[xml(list)]
    pub monitors: Vec<CachetuneMonitor>,
}

/// `<memorytune><node>` — upstream's `_MemoryTuneNode`.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "node")]
pub struct MemorytuneNode {
    #[xml(attribute = "id")]
    pub id: Option<u32>,

    #[xml(attribute = "bandwidth")]
    pub bandwidth: Option<u32>,
}

/// `<memorytune>` — upstream's `_MemoryTune`.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "memorytune")]
pub struct Memorytune {
    #[xml(attribute = "vcpus")]
    pub vcpus: Option<String>,

    #[xml(list)]
    pub nodes: Vec<MemorytuneNode>,
}

// --- Actual <cputune> domain ---

/// ```xml
/// <cputune>
///   <shares>2048</shares>
///   <vcpupin vcpu="0" cpuset="0-3"/>
///   <emulatorpin cpuset="0-3"/>
///   <vcpusched vcpus="0-1" scheduler="fifo" priority="1"/>
///   <cachetune vcpus="0-3">
///     <cache id="0" level="3" type="both" size="3" unit="MiB"/>
///   </cachetune>
/// </cputune>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "cputune")]
pub struct DomainCputune {
    #[xml(path = "shares", text)]
    pub shares: Option<u32>,

    #[xml(path = "period", text)]
    pub period: Option<u32>,

    #[xml(path = "quota", text)]
    pub quota: Option<u32>,

    #[xml(path = "global_period", text)]
    pub global_period: Option<u32>,

    #[xml(path = "global_quota", text)]
    pub global_quota: Option<u32>,

    #[xml(path = "emulator_period", text)]
    pub emulator_period: Option<u32>,

    #[xml(path = "emulator_quota", text)]
    pub emulator_quota: Option<u32>,

    #[xml(path = "iothread_period", text)]
    pub iothread_period: Option<u32>,

    #[xml(path = "iothread_quota", text)]
    pub iothread_quota: Option<u32>,

    #[xml(list)]
    pub vcpupins: Vec<CputuneVcpuPin>,

    #[xml(path = "emulatorpin", attribute = "cpuset")]
    pub emulatorpin_cpuset: Option<String>,

    #[xml(list)]
    pub iothreadpins: Vec<CputuneIothreadPin>,

    #[xml(path = "emulatorsched", attribute = "scheduler")]
    pub emulatorsched_scheduler: Option<String>,

    #[xml(path = "emulatorsched", attribute = "priority")]
    pub emulatorsched_priority: Option<u32>,

    #[xml(list)]
    pub vcpuscheds: Vec<CputuneVcpuSched>,

    #[xml(list)]
    pub iothreadscheds: Vec<CputuneIothreadSched>,

    #[xml(list)]
    pub cachetunes: Vec<Cachetune>,

    #[xml(list)]
    pub memorytunes: Vec<Memorytune>,
}

// ---------------------------------------------------------------------
// `DomainCputune` as an XML backed object, for the CLI parsers.
// ---------------------------------------------------------------------

use crate::xmlbuilder::{child, prop, Kind, XmlClass};

/// `_VCPUPin`.
pub static VCPU_PIN: XmlClass = XmlClass {
    xml_name: "vcpupin",
    props: &[
        prop("vcpu", "./@vcpu", Kind::Int),
        prop("cpuset", "./@cpuset", Kind::Str),
    ],
    children: &[],
    prop_order: &["vcpu", "cpuset"],
};

/// `_IOThreadPin`.
pub static IOTHREAD_PIN: XmlClass = XmlClass {
    xml_name: "iothreadpin",
    props: &[
        prop("iothread", "./@iothread", Kind::Int),
        prop("cpuset", "./@cpuset", Kind::Str),
    ],
    children: &[],
    prop_order: &["iothread", "cpuset"],
};

/// `_VCPUSched`.
pub static VCPU_SCHED: XmlClass = XmlClass {
    xml_name: "vcpusched",
    props: &[
        prop("vcpus", "./@vcpus", Kind::Str),
        prop("scheduler", "./@scheduler", Kind::Str),
        prop("priority", "./@priority", Kind::Int),
    ],
    children: &[],
    prop_order: &["vcpus", "scheduler", "priority"],
};

/// `_IOThreadSched`.
pub static IOTHREAD_SCHED: XmlClass = XmlClass {
    xml_name: "iothreadsched",
    props: &[
        prop("iothreads", "./@iothreads", Kind::Str),
        prop("scheduler", "./@scheduler", Kind::Str),
        prop("priority", "./@priority", Kind::Int),
    ],
    children: &[],
    prop_order: &["iothreads", "scheduler", "priority"],
};

/// `_CacheTuneCache`.
pub static CACHETUNE_CACHE: XmlClass = XmlClass {
    xml_name: "cache",
    props: &[
        prop("id", "./@id", Kind::Int),
        prop("level", "./@level", Kind::Int),
        prop("type", "./@type", Kind::Str),
        prop("size", "./@size", Kind::Int),
        prop("unit", "./@unit", Kind::Str),
    ],
    children: &[],
    prop_order: &["id", "level", "type", "size", "unit"],
};

/// `_CacheTuneMonitor`.
pub static CACHETUNE_MONITOR: XmlClass = XmlClass {
    xml_name: "monitor",
    props: &[
        prop("level", "./@level", Kind::Int),
        prop("vcpus", "./@vcpus", Kind::Str),
    ],
    children: &[],
    prop_order: &["level", "vcpus"],
};

/// `_CacheTune`.
pub static CACHETUNE: XmlClass = XmlClass {
    xml_name: "cachetune",
    props: &[prop("vcpus", "./@vcpus", Kind::Str)],
    children: &[
        child("caches", &CACHETUNE_CACHE, ".", false),
        child("monitors", &CACHETUNE_MONITOR, ".", false),
    ],
    prop_order: &["vcpus", "caches"],
};

/// `_MemoryTuneNode`.
pub static MEMORYTUNE_NODE: XmlClass = XmlClass {
    xml_name: "node",
    props: &[
        prop("id", "./@id", Kind::Int),
        prop("bandwidth", "./@bandwidth", Kind::Int),
    ],
    children: &[],
    prop_order: &["id", "bandwidth"],
};

/// `_MemoryTune`.
pub static MEMORYTUNE: XmlClass = XmlClass {
    xml_name: "memorytune",
    props: &[prop("vcpus", "./@vcpus", Kind::Str)],
    children: &[child("nodes", &MEMORYTUNE_NODE, ".", false)],
    prop_order: &["vcpus", "nodes"],
};

/// `DomainCputune`.
pub static DOMAIN_CPUTUNE: XmlClass = XmlClass {
    xml_name: "cputune",
    props: &[
        prop("shares", "./shares", Kind::Int),
        prop("period", "./period", Kind::Int),
        prop("quota", "./quota", Kind::Int),
        prop("global_period", "./global_period", Kind::Int),
        prop("global_quota", "./global_quota", Kind::Int),
        prop("emulator_period", "./emulator_period", Kind::Int),
        prop("emulator_quota", "./emulator_quota", Kind::Int),
        prop("iothread_period", "./iothread_period", Kind::Int),
        prop("iothread_quota", "./iothread_quota", Kind::Int),
        prop("emulatorpin_cpuset", "./emulatorpin/@cpuset", Kind::Str),
        prop(
            "emulatorsched_scheduler",
            "./emulatorsched/@scheduler",
            Kind::Str,
        ),
        prop(
            "emulatorsched_priority",
            "./emulatorsched/@priority",
            Kind::Int,
        ),
    ],
    children: &[
        child("vcpupins", &VCPU_PIN, ".", false),
        child("iothreadpins", &IOTHREAD_PIN, ".", false),
        child("vcpuscheds", &VCPU_SCHED, ".", false),
        child("iothreadscheds", &IOTHREAD_SCHED, ".", false),
        child("cachetunes", &CACHETUNE, ".", false),
        child("memorytunes", &MEMORYTUNE, ".", false),
    ],
    prop_order: &[
        "shares",
        "period",
        "quota",
        "global_period",
        "global_quota",
        "emulator_period",
        "emulator_quota",
        "iothread_period",
        "iothread_quota",
        "vcpupins",
        "emulatorpin_cpuset",
        "iothreadpins",
        "emulatorsched_scheduler",
        "emulatorsched_priority",
        "vcpuscheds",
        "iothreadscheds",
        "cachetunes",
        "memorytunes",
    ],
};
