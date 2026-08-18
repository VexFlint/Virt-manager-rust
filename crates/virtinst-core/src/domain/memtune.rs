//! `<memtune>` — upstream's `domain/memtune.py`. Four flat `is_int=True`
//! text fields, all direct children of `<memtune>` itself — no nested
//! elements, no new macro capability needed (same `text` + `Option<u32>`
//! combination `Guest::memory` already exercises).

use virtinst_xml::XmlBound;

/// ```xml
/// <memtune>
///   <hard_limit>1048576</hard_limit>
///   <soft_limit>1048576</soft_limit>
///   <swap_hard_limit>2097152</swap_hard_limit>
///   <min_guarantee>524288</min_guarantee>
/// </memtune>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "memtune")]
pub struct DomainMemtune {
    #[xml(path = "hard_limit", text)]
    pub hard_limit: Option<u32>,

    #[xml(path = "soft_limit", text)]
    pub soft_limit: Option<u32>,

    #[xml(path = "swap_hard_limit", text)]
    pub swap_hard_limit: Option<u32>,

    #[xml(path = "min_guarantee", text)]
    pub min_guarantee: Option<u32>,
}
