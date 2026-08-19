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
