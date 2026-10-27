//! `<serial>`/`<parallel>`/`<console>`/`<channel>` — upstream's
//! `devices/char.py`'s `_DeviceChar` family. Grounded in the real
//! `XMLProperty` declarations, read carefully rather than at a glance:
//! `_DeviceChar` itself only has `type` and `target/*` as
//! straightforward self/nested attributes; `host`/`service`/`path`/
//! `mode`/`tls` all live on a single nested `<source>` child
//! (`CharSource`, `is_single=True` — one `<source>`, not a list), also
//! a plain forward path.
//!
//! **Real gap, narrow and non-blocking:** `CharSource` also declares
//! `connect_host`/`bind_host`/`protocol`/`log_file` via `..` (parent
//! axis) and `[@mode='connect']` (attribute-predicate) XPath —
//! `./../source[@mode='connect']/@host`. Neither parent traversal nor
//! predicate filtering exists in this crate's `path` resolver (plain
//! forward `find()` by tag name only). Not modeled here — but these are
//! upstream's own *redundant convenience duplicates* for
//! cross-referencing a sibling `<source>` when more than one exists
//! (e.g. separate bind/connect sources for TCP); `source_mode`/
//! `source_host`/`source_service` below already reach the same data
//! for this struct's own single `<source>`, so nothing is actually
//! unreachable, just the shortcut spelling.
//!
//! Upstream shares this exact field set across four tags via
//! `_DeviceChar` (`DeviceConsole`="console", `DeviceSerial`="serial",
//! `DeviceParallel`="parallel", `DeviceChannel`="channel") — no Rust
//! inheritance to port it through (ticket 06 already rejected porting
//! upstream's dynamic class machinery), so each is its own
//! `#[xml(tag = "...")]` struct, fields spelled out per struct rather
//! than factored through a macro (matches this crate's existing
//! one-struct-per-tag convention; four already-tiny structs don't earn
//! a generator).
//!
//! `DeviceSerial` was built first as the representative case (see
//! `addhardware.py`'s `hw-list`: "Serial"/"Parallel"/"Console"/"Channel"
//! are four separate user-facing rows, all landing on `PAGE_CHAR` and
//! dispatched to one of these four classes by `_get_char_class()` — so
//! ticket 12's `NewDevice::Char` variant needs all four, not `DeviceSerial`
//! alone, to avoid silently mislabeling three of the four rows).

use virtinst_xml::XmlBound;

/// ```xml
/// <serial type="pty">
///   <target type="isa-serial" port="0">
///     <model name="isa-serial"/>
///   </target>
/// </serial>
/// ```
/// or
/// ```xml
/// <serial type="tcp">
///   <source mode="bind" host="127.0.0.1" service="2445"/>
///   <target type="isa-serial" port="0"/>
/// </serial>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "serial")]
pub struct DeviceSerial {
    #[xml(attribute = "type")]
    pub char_type: Option<String>,

    #[xml(path = "source", attribute = "mode")]
    pub source_mode: Option<String>,

    #[xml(path = "source", attribute = "host")]
    pub source_host: Option<String>,

    #[xml(path = "source", attribute = "service")]
    pub source_service: Option<u32>,

    #[xml(path = "source", attribute = "path")]
    pub source_path: Option<String>,

    #[xml(path = "source", attribute = "tls")]
    pub source_tls: Option<bool>,

    #[xml(path = "target", attribute = "type")]
    pub target_type: Option<String>,

    #[xml(path = "target", attribute = "port")]
    pub target_port: Option<u32>,

    #[xml(path = "target/model", attribute = "name")]
    pub target_model_name: Option<String>,
}

/// Same field set as [`DeviceSerial`], `<parallel>` tag — upstream's
/// `DeviceParallel`, the "Parallel" `hw-list` row.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "parallel")]
pub struct DeviceParallel {
    #[xml(attribute = "type")]
    pub char_type: Option<String>,

    #[xml(path = "source", attribute = "mode")]
    pub source_mode: Option<String>,

    #[xml(path = "source", attribute = "host")]
    pub source_host: Option<String>,

    #[xml(path = "source", attribute = "service")]
    pub source_service: Option<u32>,

    #[xml(path = "source", attribute = "path")]
    pub source_path: Option<String>,

    #[xml(path = "source", attribute = "tls")]
    pub source_tls: Option<bool>,

    #[xml(path = "target", attribute = "type")]
    pub target_type: Option<String>,

    #[xml(path = "target", attribute = "port")]
    pub target_port: Option<u32>,

    #[xml(path = "target/model", attribute = "name")]
    pub target_model_name: Option<String>,
}

/// Same field set as [`DeviceSerial`], `<console>` tag — upstream's
/// `DeviceConsole`, the "Console" `hw-list` row (the one row of the
/// four that's always enabled — no `is_hvm()` gate upstream).
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "console")]
pub struct DeviceConsole {
    #[xml(attribute = "type")]
    pub char_type: Option<String>,

    #[xml(path = "source", attribute = "mode")]
    pub source_mode: Option<String>,

    #[xml(path = "source", attribute = "host")]
    pub source_host: Option<String>,

    #[xml(path = "source", attribute = "service")]
    pub source_service: Option<u32>,

    #[xml(path = "source", attribute = "path")]
    pub source_path: Option<String>,

    #[xml(path = "source", attribute = "tls")]
    pub source_tls: Option<bool>,

    #[xml(path = "target", attribute = "type")]
    pub target_type: Option<String>,

    #[xml(path = "target", attribute = "port")]
    pub target_port: Option<u32>,

    #[xml(path = "target/model", attribute = "name")]
    pub target_model_name: Option<String>,
}

/// Same field set as [`DeviceSerial`], `<channel>` tag — upstream's
/// `DeviceChannel`, the "Channel" `hw-list` row (virtio-serial channels,
/// e.g. `org.qemu.guest_agent.0`; `char-target-name`/`char-target-type`
/// in the wizard UI are a separate concern from this field set).
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "channel")]
pub struct DeviceChannel {
    #[xml(attribute = "type")]
    pub char_type: Option<String>,

    #[xml(path = "source", attribute = "mode")]
    pub source_mode: Option<String>,

    #[xml(path = "source", attribute = "host")]
    pub source_host: Option<String>,

    #[xml(path = "source", attribute = "service")]
    pub source_service: Option<u32>,

    #[xml(path = "source", attribute = "path")]
    pub source_path: Option<String>,

    #[xml(path = "source", attribute = "tls")]
    pub source_tls: Option<bool>,

    #[xml(path = "target", attribute = "type")]
    pub target_type: Option<String>,

    #[xml(path = "target", attribute = "port")]
    pub target_port: Option<u32>,

    #[xml(path = "target/model", attribute = "name")]
    pub target_model_name: Option<String>,
}

// `CharSource` as an `XMLBuilder` class, for the CLI parsers.

use crate::xmlbuilder::{child, prop, Kind, XmlClass, XmlObject};

/// `CharSource`.
pub static CHAR_SOURCE: XmlClass = XmlClass {
    xml_name: "source",
    props: &[
        prop("host", "./@host", Kind::Str),
        prop("service", "./@service", Kind::Int),
        prop("path", "./@path", Kind::Str),
        prop("channel", "./@channel", Kind::Str),
        prop("master", "./@master", Kind::Str),
        prop("slave", "./@slave", Kind::Str),
        prop("mode", "./@mode", Kind::Str),
        prop("tls", "./@tls", Kind::YesNo),
        prop("clipboard_copypaste", "./clipboard/@copypaste", Kind::YesNo),
        prop("mouse_mode", "./mouse/@mode", Kind::Str),
        prop("protocol", "./../protocol/@type", Kind::Str),
        prop("log_file", "./../log/@file", Kind::Str),
        prop("log_append", "./../log/@append", Kind::OnOff),
        prop(
            "connect_host",
            "./../source[@mode='connect']/@host",
            Kind::Str,
        ),
        prop(
            "connect_service",
            "./../source[@mode='connect']/@service",
            Kind::Int,
        ),
        prop("bind_host", "./../source[@mode='bind']/@host", Kind::Str),
        prop(
            "bind_service",
            "./../source[@mode='bind']/@service",
            Kind::Int,
        ),
    ],
    children: &[child(
        "seclabels",
        &crate::xmlbuilder::DEVICE_SECLABEL,
        ".",
        false,
    )],
    prop_order: &[
        "bind_host",
        "bind_service",
        "mode",
        "connect_host",
        "connect_service",
        "path",
        "channel",
    ],
};

/// `_set_host_helper`: `host:port` into a host and a port property.
fn set_host_helper(
    obj: &mut XmlObject,
    hostparam: &str,
    portparam: &str,
    val: Option<&str>,
) -> Result<(), String> {
    let (host, port) = val
        .unwrap_or_default()
        .split_once(':')
        .map_or_else(|| (val.unwrap_or_default(), ""), |(h, p)| (h, p));
    let mut host = (!host.is_empty()).then(|| host.to_string());
    let port = (!port.is_empty()).then(|| port.to_string());
    if port.is_some() && host.is_none() {
        host = Some("127.0.0.1".into());
    }
    obj.set(hostparam, host.map(Into::into))?;
    if let Some(port) = port {
        obj.set(portparam, Some(port.into()))?;
    }
    Ok(())
}

/// `CharSource.set_friendly_host`.
pub fn set_friendly_host(src: &mut XmlObject, val: Option<&str>) -> Result<(), String> {
    set_host_helper(src, "host", "service", val)
}

/// `CharSource.set_friendly_bind`.
pub fn set_friendly_bind(src: &mut XmlObject, val: Option<&str>) -> Result<(), String> {
    set_host_helper(src, "bind_host", "bind_service", val)
}

/// `CharSource.set_friendly_connect`.
pub fn set_friendly_connect(src: &mut XmlObject, val: Option<&str>) -> Result<(), String> {
    set_host_helper(src, "connect_host", "connect_service", val)
}

/// One `_DeviceChar` subclass: its own `XML_NAME`, the shared
/// properties.
macro_rules! char_class {
    ($name:expr) => {
        XmlClass {
            xml_name: $name,
            props: &[
                prop("type", "./@type", Kind::Str),
                prop("target_address", "./target/@address", Kind::Str),
                prop("target_port", "./target/@port", Kind::Int),
                prop("target_type", "./target/@type", Kind::Str),
                prop("target_name", "./target/@name", Kind::Str),
                prop("target_state", "./target/@state", Kind::Str),
                prop("target_model_name", "./target/model/@name", Kind::Str),
            ],
            children: crate::device_children!(child("source", &CHAR_SOURCE, ".", true)),
            prop_order: crate::device_prop_order!(
                "type",
                "source",
                "target_type",
                "target_name",
                "target_state"
            ),
        }
    };
}

/// `DeviceConsole`.
pub static DEVICE_CONSOLE: XmlClass = char_class!("console");
/// `DeviceSerial`.
pub static DEVICE_SERIAL: XmlClass = char_class!("serial");
/// `DeviceParallel`.
pub static DEVICE_PARALLEL: XmlClass = char_class!("parallel");
/// `DeviceChannel`.
pub static DEVICE_CHANNEL: XmlClass = char_class!("channel");

/// `_DeviceChar.set_friendly_target`.
pub fn set_friendly_target(dev: &mut XmlObject, val: Option<&str>) -> Result<(), String> {
    set_host_helper(dev, "target_address", "target_port", val)
}

/// `_DeviceChar.set_defaults`.
pub fn set_defaults(dev: &mut XmlObject) -> Result<(), String> {
    let empty = |v: Option<String>| v.map_or(true, |s| s.is_empty());
    let typ = dev.get_str("type");
    if empty(dev.child("source").get_str("mode")) && matches!(typ.as_deref(), Some("unix" | "tcp"))
    {
        dev.child_mut("source").set("mode", Some("bind".into()))?;
    }
    if empty(dev.get_str("target_type")) && dev.class.xml_name == "channel" {
        dev.set("target_type", Some("virtio".into()))?;
    }
    if empty(dev.get_str("target_name"))
        && matches!(typ.as_deref(), Some("spicevmc" | "qemu-vdagent"))
    {
        dev.set("target_name", Some(super::build::CHANNEL_NAME_SPICE.into()))?;
    }
    Ok(())
}

/// `DeviceConsole.get_console_duplicate`: the guest's first console,
/// when it mirrors the passed serial device.
pub fn console_duplicate(serial: &XmlObject, consoles: &[XmlObject]) -> bool {
    if serial.class.xml_name != "serial" {
        return false;
    }
    let Some(console) = consoles.first() else {
        return false;
    };
    let target_type = console.get_str("target_type");
    console.get_str("type") == serial.get_str("type")
        && matches!(target_type.as_deref(), None | Some("serial"))
}
