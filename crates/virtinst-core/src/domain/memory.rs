//! `<currentMemory>` — **not** from an upstream `domain/memory.py`;
//! there is no such file (checked both the directory listing and a
//! grep for `CurrentMemory` across `virtinst/` — nothing). Found while
//! wiring `Guest` together: `currentMemory` is really a flat
//! `is_int=True` property on `guest.py`'s own `Guest` class
//! (`_currentMemory`/`memory`/`maxMemory`/`maxMemorySlots`, now
//! [`crate::guest::Guest::current_memory`] and its siblings), same as
//! `name`/`uuid`/`description`/`title` — not a nested child object
//! upstream ever builds.
//!
//! This struct earns its keep anyway, for a narrower reason: it's the
//! suite's only proof that `attribute` and `text` can bind to two
//! different fields on the *same* element (no `path` needed for
//! either), a real libvirt shape (`<currentMemory unit="KiB">N</currentMemory>`)
//! that upstream itself just doesn't bother modeling the unit half of.
//! Not wired into `Guest` — `Guest::current_memory` reads the same
//! text content directly and ignores the unit, matching upstream
//! exactly.

use virtinst_xml::XmlBound;

/// ```xml
/// <currentMemory unit="KiB">1048576</currentMemory>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "currentMemory")]
pub struct CurrentMemory {
    #[xml(attribute = "unit")]
    pub unit: Option<String>,

    #[xml(text)]
    pub value: Option<String>,
}
