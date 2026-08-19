//! The root domain-XML aggregate type — upstream's `guest.py`.
//!
//! Aggregates [`crate::devices`] and [`crate::domain`], bound to their
//! XPaths via the derive macro decided in ticket 06. Two distinct
//! construction paths, not one:
//!
//! - **Fresh creation** (Create VM wizard, ticket 08; `virt-install`,
//!   ticket 16): build the struct directly and serialize fresh. No
//!   `edit-xml` DOM involved — there's no existing document to preserve.
//! - **Editing an existing domain** (VM details window, ticket 10;
//!   `virt-xml --edit`, ticket 16 — the first ticket to walk this path
//!   end to end): parse through the order-preserving `edit-xml` DOM,
//!   mutate via the typed struct's XPath binding, serialize back. This
//!   is the path that has to satisfy ticket 03's acceptance bar —
//!   byte-exact preservation of untouched XML, including foreign
//!   `xmlns:qemu` elements.
//!
//! `description`/`title`/`name`/`uuid`/`domain_type`/`memory`/
//! `max_memory`/`max_memory_slots`/`current_memory` are `Guest`'s own
//! flat scalars — real upstream `Guest` properties (`./name`, `./uuid`,
//! `./@type`, `./memory`, `./maxMemory`, `./maxMemory/@slots`,
//! `./currentMemory`), not delegated to any nested struct. See
//! [`crate::domain::memory`]'s doc comment for why `current_memory`
//! (unlike everything below) doesn't go through `CurrentMemory` — that
//! struct isn't what upstream actually uses here.
//!
//! Everything else is a `#[xml(nested)]` (or, for the two upstream
//! fields that are genuinely repeatable — `seclabels`/`sysinfo` —
//! `#[xml(list)]`) field wired straight to the real
//! `XMLChildProperty` calls in `guest.py`, grounded in reading them
//! directly rather than assumed from each struct's own module. `nested`
//! fields are **read-only**, same as `list` — see
//! `virtinst_xml::nested_read`'s docs for why `Guest::write_to` never
//! invents an empty `<clock/>`/`<cpu/>`/etc. on a document that never
//! had one. Mutating any of these goes through `find()`-ing the
//! specific element and calling that struct's own `write_to` directly,
//! exactly how every leaf struct here already gets edited (ticket 10's
//! VM details window, ticket 16's `virt-xml --edit`).
//!
//! `xmlns_qemu` inherits `domain::xmlnsqemu`'s documented
//! stripped-local-name gap (matches on `"commandline"`, not
//! `"qemu:commandline"`) — noted there, not re-litigated here.

use crate::devices::DeviceList;
use crate::domain::{
    Clock, DomainBlkiotune, DomainCpu, DomainCputune, DomainFeatures, DomainIdmap, DomainKeyWrap,
    DomainLaunchSecurity, DomainMemoryBacking, DomainMemtune, DomainMetadata, DomainNumatune,
    DomainOs, DomainPm, DomainResource, DomainSeclabel, DomainSysinfo, DomainVcpus,
    DomainXmlnsQemu,
};
use virtinst_xml::XmlBound;

#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "domain")]
pub struct Guest {
    #[xml(attribute = "type")]
    pub domain_type: Option<String>,

    #[xml(path = "name", text)]
    pub name: Option<String>,

    #[xml(path = "uuid", text)]
    pub uuid: Option<String>,

    #[xml(path = "description", text)]
    pub description: Option<String>,

    #[xml(path = "title", text)]
    pub title: Option<String>,

    #[xml(path = "currentMemory", text)]
    pub current_memory: Option<u32>,

    #[xml(path = "memory", text)]
    pub memory: Option<u32>,

    #[xml(path = "maxMemory", text)]
    pub max_memory: Option<u32>,

    #[xml(path = "maxMemory", attribute = "slots")]
    pub max_memory_slots: Option<u32>,

    #[xml(nested)]
    pub devices: DeviceList,

    #[xml(nested)]
    pub vcpulist: DomainVcpus,

    #[xml(list)]
    pub seclabels: Vec<DomainSeclabel>,

    #[xml(nested)]
    pub keywrap: DomainKeyWrap,

    #[xml(nested)]
    pub os: DomainOs,

    #[xml(nested)]
    pub features: DomainFeatures,

    #[xml(nested)]
    pub clock: Clock,

    #[xml(nested)]
    pub cpu: DomainCpu,

    #[xml(nested)]
    pub cputune: DomainCputune,

    #[xml(nested)]
    pub numatune: DomainNumatune,

    #[xml(nested)]
    pub pm: DomainPm,

    #[xml(nested)]
    pub blkiotune: DomainBlkiotune,

    #[xml(nested)]
    pub memtune: DomainMemtune,

    #[xml(nested)]
    pub memory_backing: DomainMemoryBacking,

    #[xml(nested)]
    pub idmap: DomainIdmap,

    #[xml(nested)]
    pub resource: DomainResource,

    #[xml(list)]
    pub sysinfo: Vec<DomainSysinfo>,

    #[xml(nested)]
    pub launch_security: DomainLaunchSecurity,

    #[xml(nested)]
    pub metadata: DomainMetadata,

    #[xml(nested)]
    pub xmlns_qemu: DomainXmlnsQemu,
}
