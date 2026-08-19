//! One typed struct per domain-XML subsystem — upstream's `domain/*.py`
//! (18 files: cpu, clock, os, features, seclabel, sysinfo, vcpus,
//! xmlnsqemu foreign-namespace preservation, etc.).
//!
//! Bound to XPaths via the derive macro from ticket 06, read/written
//! through the `edit-xml`-backed order-preserving DOM. `xmlnsqemu.py`'s
//! foreign-namespace preservation is exactly what makes the DOM
//! approach necessary in the first place (ticket 03's acceptance bar).

pub mod clock;
pub use clock::Clock;

pub mod memory;
pub use memory::CurrentMemory;

pub mod pm;
pub use pm::DomainPm;

pub mod memtune;
pub use memtune::DomainMemtune;

pub mod resource;
pub use resource::DomainResource;

pub mod vcpus;
pub use vcpus::{DomainVcpu, DomainVcpus};

pub mod keywrap;
pub use keywrap::{DomainKeyWrap, KeyWrapCipher};

pub mod idmap;
pub use idmap::DomainIdmap;

pub mod blkiotune;
pub use blkiotune::{BlkiotuneDevice, DomainBlkiotune};

pub mod numatune;
pub use numatune::{DomainNumatune, NumatuneMemnode};

pub mod xmlnsqemu;
pub use xmlnsqemu::{DomainXmlnsQemu, XmlnsQemuArg, XmlnsQemuEnv};

pub mod memorybacking;
pub use memorybacking::{DomainMemoryBacking, HugepagesPage};

pub mod launch_security;
pub use launch_security::DomainLaunchSecurity;

pub mod seclabel;
pub use seclabel::DomainSeclabel;

pub mod sysinfo;
pub use sysinfo::{DomainSysinfo, SysinfoEntry};

pub mod cputune;
pub use cputune::{
    Cachetune, CachetuneCache, CachetuneMonitor, CputuneIothreadPin, CputuneIothreadSched,
    CputuneVcpuPin, CputuneVcpuSched, DomainCputune, Memorytune, MemorytuneNode,
};

pub mod cpu;
pub use cpu::{
    CpuFeature, CpuNumaBandwidth, CpuNumaCell, CpuNumaCellCache, CpuNumaCellSibling,
    CpuNumaLatency, DomainCpu,
};

pub mod os;
pub use os::{DomainOs, OsBootDevice, OsFirmwareFeature, OsInitArg, OsInitEnv};

// TODO: features.py — needs the already-added Option<OnOff> for most
// of its ~40 fields but is otherwise flat, same shape as everything
// else in domain/.
