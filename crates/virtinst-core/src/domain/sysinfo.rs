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

/// `_SysinfoEntry`, for the CLI parsers.
pub static SYSINFO_ENTRY: crate::xmlbuilder::XmlClass = {
    use crate::xmlbuilder::{prop, Kind, XmlClass};
    XmlClass {
        xml_name: "entry",
        props: &[
            prop("name", "./@name", Kind::Str),
            prop("value", ".", Kind::Str),
            prop("file", "./@file", Kind::Str),
        ],
        children: &[],
        prop_order: &[],
    }
};

/// `_SysinfoOemString`: its own class, so a new object finds its list.
pub static SYSINFO_OEM_STRING: crate::xmlbuilder::XmlClass = {
    use crate::xmlbuilder::{prop, Kind, XmlClass};
    XmlClass {
        xml_name: "entry",
        props: &[
            prop("name", "./@name", Kind::Str),
            prop("value", ".", Kind::Str),
            prop("file", "./@file", Kind::Str),
        ],
        children: &[],
        prop_order: &[],
    }
};

/// `DomainSysinfo`, for the CLI parsers.
pub static DOMAIN_SYSINFO: crate::xmlbuilder::XmlClass = {
    use crate::xmlbuilder::{child, prop, Kind, XmlClass};
    XmlClass {
        xml_name: "sysinfo",
        props: &[
            prop("type", "./@type", Kind::Str),
            prop("bios_date", "./bios/entry[@name='date']", Kind::Str),
            prop("bios_vendor", "./bios/entry[@name='vendor']", Kind::Str),
            prop("bios_version", "./bios/entry[@name='version']", Kind::Str),
            prop("bios_release", "./bios/entry[@name='release']", Kind::Str),
            prop("system_uuid", "./system/entry[@name='uuid']", Kind::Str),
            prop(
                "system_manufacturer",
                "./system/entry[@name='manufacturer']",
                Kind::Str,
            ),
            prop(
                "system_product",
                "./system/entry[@name='product']",
                Kind::Str,
            ),
            prop(
                "system_version",
                "./system/entry[@name='version']",
                Kind::Str,
            ),
            prop("system_serial", "./system/entry[@name='serial']", Kind::Str),
            prop("system_sku", "./system/entry[@name='sku']", Kind::Str),
            prop("system_family", "./system/entry[@name='family']", Kind::Str),
            prop(
                "baseBoard_manufacturer",
                "./baseBoard/entry[@name='manufacturer']",
                Kind::Str,
            ),
            prop(
                "baseBoard_product",
                "./baseBoard/entry[@name='product']",
                Kind::Str,
            ),
            prop(
                "baseBoard_version",
                "./baseBoard/entry[@name='version']",
                Kind::Str,
            ),
            prop(
                "baseBoard_serial",
                "./baseBoard/entry[@name='serial']",
                Kind::Str,
            ),
            prop(
                "baseBoard_asset",
                "./baseBoard/entry[@name='asset']",
                Kind::Str,
            ),
            prop(
                "baseBoard_location",
                "./baseBoard/entry[@name='location']",
                Kind::Str,
            ),
            prop(
                "chassis_manufacturer",
                "./chassis/entry[@name='manufacturer']",
                Kind::Str,
            ),
            prop(
                "chassis_version",
                "./chassis/entry[@name='version']",
                Kind::Str,
            ),
            prop(
                "chassis_serial",
                "./chassis/entry[@name='serial']",
                Kind::Str,
            ),
            prop("chassis_asset", "./chassis/entry[@name='asset']", Kind::Str),
            prop("chassis_sku", "./chassis/entry[@name='sku']", Kind::Str),
        ],
        children: &[
            child("oemStrings", &SYSINFO_OEM_STRING, "./oemStrings", false),
            child("entries", &SYSINFO_ENTRY, ".", false),
        ],
        prop_order: &[
            "type",
            "bios_vendor",
            "bios_version",
            "bios_date",
            "bios_release",
            "system_manufacturer",
            "system_product",
            "system_version",
            "system_serial",
            "system_uuid",
            "system_sku",
            "system_family",
            "baseBoard_manufacturer",
            "baseBoard_product",
            "baseBoard_version",
            "baseBoard_serial",
            "baseBoard_asset",
            "baseBoard_location",
            "chassis_manufacturer",
            "chassis_version",
            "chassis_serial",
            "chassis_asset",
            "chassis_sku",
            "oemStrings",
        ],
    }
};
