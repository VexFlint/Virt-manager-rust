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
//! `description`/`title` were the first real fields; `name`/`uuid`/
//! `domain_type`/`memory`/`max_memory`/`max_memory_slots` round out
//! Guest's own flat scalars (upstream's `./name`, `./uuid`, `./@type`,
//! `./memory`, `./maxMemory`, `./maxMemory/@slots` — all plain
//! self/child bindings, no new macro capability needed: `text` reuses
//! the same `XmlAttrValue` conversion as `attribute`, so `Option<u32>`
//! text fields already worked, just hadn't been exercised yet).
//!
//! Still TODO: `devices: DeviceList` and the domain-subsystem structs
//! (`Clock`, `CurrentMemory`, and the other ~16 files under
//! `domain/*.py` — cpu, os, features, seclabel, etc.) as fields here,
//! once each gets its own struct.

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

    #[xml(path = "memory", text)]
    pub memory: Option<u32>,

    #[xml(path = "maxMemory", text)]
    pub max_memory: Option<u32>,

    #[xml(path = "maxMemory", attribute = "slots")]
    pub max_memory_slots: Option<u32>,
}
