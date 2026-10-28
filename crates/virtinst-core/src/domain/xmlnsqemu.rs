//! `<qemu:commandline>` — upstream's `domain/xmlnsqemu.py`, the
//! `xmlns:qemu` foreign-namespace passthrough ticket 06's whole DOM
//! approach exists to preserve untouched. This struct is the one
//! *modeled* corner of that namespace (`virt-install --qemu-commandline`
//! writes it deliberately); everything else under `xmlns:qemu` stays
//! untouched passthrough, never parsed at all.
//!
//! **Real gap, narrow and non-blocking:** `edit_xml::Element::find`
//! matches on the *stripped* local name (confirmed empirically — see
//! `element.rs`'s own `name()` vs `full_name()` split), so tags here
//! are `"commandline"`/`"arg"`/`"env"`, not `"qemu:commandline"`/
//! `"qemu:arg"`/`"qemu:env"`. Reads correctly (nothing else in real
//! libvirt XML uses those bare local names), but doesn't disambiguate
//! by namespace — a document with some *other* foreign
//! `<x:commandline>` sibling would collide. Not a problem in practice
//! (no such sibling exists in the wild) and this struct's `#[xml(list)]`
//! fields are read-only anyway (same as every other list field in this
//! crate — see `devices::list`'s doc comment), so write-side
//! prefix-loss on `Element::new` never gets exercised either.

use virtinst_xml::XmlBound;

/// A single `<qemu:arg>` — upstream's `_XMLNSQemuArg`.
///
/// ```xml
/// <qemu:arg value="-device"/>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "arg")]
pub struct XmlnsQemuArg {
    #[xml(attribute = "value")]
    pub value: Option<String>,
}

/// A single `<qemu:env>` — upstream's `_XMLNSQemuEnv`.
///
/// ```xml
/// <qemu:env name="DISPLAY" value=":0"/>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "env")]
pub struct XmlnsQemuEnv {
    #[xml(attribute = "name")]
    pub name: Option<String>,

    #[xml(attribute = "value")]
    pub value: Option<String>,
}

/// ```xml
/// <qemu:commandline xmlns:qemu="http://libvirt.org/schemas/domain/qemu/1.0">
///   <qemu:arg value="-device"/>
///   <qemu:env name="DISPLAY" value=":0"/>
/// </qemu:commandline>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "commandline")]
pub struct DomainXmlnsQemu {
    #[xml(list)]
    pub args: Vec<XmlnsQemuArg>,

    #[xml(list)]
    pub envs: Vec<XmlnsQemuEnv>,
}

/// `_XMLNSQemuArg`, for the CLI parsers.
pub static XMLNS_QEMU_ARG: crate::xmlbuilder::XmlClass = {
    use crate::xmlbuilder::{prop, Kind, XmlClass};
    XmlClass {
        xml_name: "qemu:arg",
        props: &[prop("value", "./@value", Kind::Str)],
        children: &[],
        prop_order: &[],
    }
};

/// `_XMLNSQemuEnv`, for the CLI parsers.
pub static XMLNS_QEMU_ENV: crate::xmlbuilder::XmlClass = {
    use crate::xmlbuilder::{prop, Kind, XmlClass};
    XmlClass {
        xml_name: "qemu:env",
        props: &[
            prop("name", "./@name", Kind::Str),
            prop("value", "./@value", Kind::Str),
        ],
        children: &[],
        prop_order: &[],
    }
};

/// `DomainXMLNSQemu`, for the CLI parsers.
pub static DOMAIN_XMLNS_QEMU: crate::xmlbuilder::XmlClass = {
    use crate::xmlbuilder::{child, XmlClass};
    XmlClass {
        xml_name: "qemu:commandline",
        props: &[],
        children: &[
            child("args", &XMLNS_QEMU_ARG, ".", false),
            child("envs", &XMLNS_QEMU_ENV, ".", false),
        ],
        prop_order: &["args", "envs"],
    }
};
