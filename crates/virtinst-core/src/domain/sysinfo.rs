//! `<sysinfo>` — upstream's `domain/sysinfo.py`. Partial, deliberately:
//! all ~25 of upstream's `bios_*`/`system_*`/`baseBoard_*`/`chassis_*`
//! fields are bound via XPath **attribute predicates**
//! (`./bios/entry[@name='date']`) — filtering children by a sibling
//! attribute's value, not just walking a plain tag-name chain. This
//! crate's `resolve_path` is a plain forward `find()` by tag name only
//! (the same gap `char.rs`'s `CharSource` doc already flagged for
//! parent-axis paths); predicate filtering doesn't exist either, so
//! none of those ~25 fields are representable without extending the
//! resolver — a real gap, left for when something actually needs
//! sysinfo's BIOS/system/chassis data, not spent speculatively here.
//!
//! What *is* representable and modeled: `type` (a plain attribute) and
//! both `XMLChildProperty` lists — `_SysinfoEntry`/`_SysinfoOemString`
//! bind `value = XMLProperty(".")`, self text content with no
//! predicate involved, same shape `smartcard.rs`'s `Certificate`
//! already proved.

use virtinst_xml::XmlBound;

/// A single `<entry>` — upstream's `_SysinfoEntry` (and
/// `_SysinfoOemString`, an identical subclass reused for `<oemStrings>`
/// entries — no distinct fields to justify a second Rust type).
///
/// ```xml
/// <entry name="manufacturer">Bochs</entry>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "entry")]
pub struct SysinfoEntry {
    #[xml(attribute = "name")]
    pub name: Option<String>,

    #[xml(text)]
    pub value: Option<String>,

    #[xml(attribute = "file")]
    pub file: Option<String>,
}

/// ```xml
/// <sysinfo type="smbios">
///   <oemStrings>
///     <entry>str1</entry>
///   </oemStrings>
///   <bios>
///     <entry name="vendor">Seabios</entry>
///   </bios>
/// </sysinfo>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "sysinfo")]
pub struct DomainSysinfo {
    #[xml(attribute = "type")]
    pub sysinfo_type: Option<String>,

    #[xml(path = "oemStrings", list)]
    pub oem_strings: Vec<SysinfoEntry>,

    #[xml(list)]
    pub entries: Vec<SysinfoEntry>,
}
