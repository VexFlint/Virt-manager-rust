//! `<filesystem>` — upstream's `devices/filesystem.py`. The struct that
//! prompted adding `#[xml(present)]`: `readonly = XMLProperty("./readonly", is_bool=True)`
//! is a child element's presence, same shape as `DeviceDisk`'s
//! `read_only`/`shareable`/`transient` (now fixed to use it too).

use virtinst_xml::XmlBound;

/// ```xml
/// <filesystem type="mount" accessmode="mapped">
///   <source dir="/host/share"/>
///   <target dir="/mnt/share"/>
///   <readonly/>
/// </filesystem>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "filesystem")]
pub struct DeviceFilesystem {
    #[xml(attribute = "type")]
    pub filesystem_type: Option<String>,

    #[xml(attribute = "accessmode")]
    pub accessmode: Option<String>,

    #[xml(path = "source", attribute = "dir")]
    pub source_dir: Option<String>,

    #[xml(path = "target", attribute = "dir")]
    pub target_dir: Option<String>,

    #[xml(path = "readonly", present)]
    pub readonly: bool,
}

// Editing an existing `<filesystem>` in place, for the details page.

use virtinst_xml::{Document, Element};

pub const TYPE_MOUNT: &str = "mount";
pub const TYPE_TEMPLATE: &str = "template";
pub const TYPE_FILE: &str = "file";
pub const TYPE_BLOCK: &str = "block";
pub const TYPE_RAM: &str = "ram";
pub const MODE_MAPPED: &str = "mapped";
pub const DRIVER_LOOP: &str = "loop";
pub const DRIVER_NBD: &str = "nbd";

/// `_type_to_source_prop`: the `<source>` attribute that holds the
/// source for a filesystem type.
pub fn source_attr(fstype: Option<&str>) -> &'static str {
    match fstype {
        Some(TYPE_TEMPLATE) => "name",
        Some(TYPE_FILE) => "file",
        Some(TYPE_BLOCK) => "dev",
        Some(TYPE_RAM) => "usage",
        _ => "dir",
    }
}

/// The `source` property.
pub fn source(doc: &Document, fs: Element) -> Option<String> {
    super::edit::attr(doc, fs, &["source"], source_attr(fs.attribute(doc, "type")))
}

/// `default_accessmode`: virtiofs leaves it to libvirt, 9p uses
/// `mapped` (passthrough only works well for qemu running as root).
pub fn default_accessmode(driver_type: Option<&str>) -> Option<&'static str> {
    (driver_type != Some("virtiofs")).then_some(MODE_MAPPED)
}

/// What `vmmFSDetails._set_values` changes; each field is `Some` only
/// when edited.
#[derive(Debug, Clone, Default)]
pub struct FsEdits {
    pub fstype: Option<Option<String>>,
    /// A path, or for `ram` the usage in MiB.
    pub source: Option<String>,
    pub target: Option<String>,
    pub readonly: Option<bool>,
    pub driver: Option<Option<String>>,
    pub format: Option<Option<String>>,
}

impl FsEdits {
    /// `define_filesystem` with `update_device`'s new device.
    pub fn apply(&self, doc: &mut Document, fs: Element) {
        use super::edit::set_attr;
        if let Some(fstype) = &self.fstype {
            // `_set_type`: the source moves to the new type's attribute
            let old_attr = source_attr(fs.attribute(doc, "type"));
            let old_source = source(doc, fs);
            set_attr(doc, fs, "", "type", fstype.as_deref());
            let new_attr = source_attr(fstype.as_deref());
            if new_attr != old_attr {
                set_attr(doc, fs, "source", old_attr, None);
                set_attr(doc, fs, "source", new_attr, old_source.as_deref());
            }
        }
        if let Some(src) = &self.source {
            let fstype = fs.attribute(doc, "type").map(str::to_string);
            set_attr(doc, fs, "source", source_attr(fstype.as_deref()), Some(src));
            if fstype.as_deref() == Some(TYPE_RAM) {
                set_attr(doc, fs, "source", "units", Some("MiB"));
            }
        }
        if let Some(target) = &self.target {
            set_attr(doc, fs, "target", "dir", Some(target));
        }
        if let Some(readonly) = self.readonly {
            let cur = fs.find(doc, "readonly");
            match (readonly, cur) {
                (true, None) => {
                    virtinst_xml::resolve_or_create_path(doc, fs, &["readonly"]);
                }
                (false, Some(el)) => {
                    let _ = el.detach(doc);
                }
                _ => {}
            }
        }
        if let Some(driver) = &self.driver {
            let orig = super::edit::attr(doc, fs, &["driver"], "type");
            set_attr(doc, fs, "driver", "type", driver.as_deref());
            if orig.as_deref() == Some("virtiofs") || driver.as_deref() == Some("virtiofs") {
                // virtiofs needs a different accessmode
                set_attr(
                    doc,
                    fs,
                    "",
                    "accessmode",
                    default_accessmode(driver.as_deref()),
                );
            }
        }
        if let Some(format) = &self.format {
            set_attr(doc, fs, "driver", "format", format.as_deref());
        }
    }
}

// The `XMLBuilder` port of `DeviceFilesystem`, for the CLI parsers.
// `type`, `source` and `target` are Python properties: set them
// through the functions below.

use crate::xmlbuilder::{prop, Kind, Value, XmlClass, XmlObject};

/// `DeviceFilesystem`.
pub static DEVICE_FILESYSTEM: XmlClass = XmlClass {
    xml_name: "filesystem",
    props: &[
        prop("_type_prop", "./@type", Kind::Str),
        prop("accessmode", "./@accessmode", Kind::Str),
        prop("model", "./@model", Kind::Str),
        prop("multidevs", "./@multidevs", Kind::Str),
        prop("fmode", "./@fmode", Kind::Str),
        prop("dmode", "./@dmode", Kind::Str),
        prop("readonly", "./readonly", Kind::Bool),
        prop("space_hard_limit", "./space_hard_limit", Kind::Str),
        prop("space_soft_limit", "./space_soft_limit", Kind::Str),
        prop("driver_wrpolicy", "./driver/@wrpolicy", Kind::Str),
        prop("driver_type", "./driver/@type", Kind::Str),
        prop("driver_format", "./driver/@format", Kind::Str),
        prop("driver_queue", "./driver/@queue", Kind::Str),
        prop("driver_name", "./driver/@name", Kind::Str),
        prop("target_dir", "./target/@dir", Kind::Str),
        prop("source_dir", "./source/@dir", Kind::Str),
        prop("source_name", "./source/@name", Kind::Str),
        prop("source_file", "./source/@file", Kind::Str),
        prop("source_dev", "./source/@dev", Kind::Str),
        prop("source_usage", "./source/@usage", Kind::Str),
        prop("source_units", "./source/@units", Kind::Str),
        prop("source_pool", "./source/@pool", Kind::Str),
        prop("source_volume", "./source/@volume", Kind::Str),
        prop("source_socket", "./source/@socket", Kind::Str),
        prop("binary_path", "./binary/@path", Kind::Str),
        prop("binary_xattr", "./binary/@xattr", Kind::OnOff),
        prop("binary_cache_mode", "./binary/cache/@mode", Kind::Str),
        prop("binary_lock_posix", "./binary/lock/@posix", Kind::OnOff),
        prop("binary_lock_flock", "./binary/lock/@flock", Kind::OnOff),
        prop("binary_sandbox_mode", "./binary/sandbox/@mode", Kind::Str),
    ],
    children: crate::device_children!(),
    prop_order: crate::device_prop_order!("_type_prop", "accessmode", "fmode", "dmode"),
};

/// `_type_to_source_prop`.
fn type_to_source_prop(dev: &XmlObject) -> &'static str {
    match dev.get_str("_type_prop").as_deref() {
        Some(TYPE_TEMPLATE) => "source_name",
        Some(TYPE_FILE) => "source_file",
        Some("block") => "source_dev",
        Some("ram") => "source_usage",
        _ => "source_dir",
    }
}

/// `DeviceFilesystem.source`'s getter.
pub fn get_source(dev: &XmlObject) -> Option<Value> {
    dev.get(type_to_source_prop(dev))
}

/// `DeviceFilesystem.source`'s setter.
pub fn set_source(dev: &mut XmlObject, val: Option<Value>) -> Result<(), String> {
    dev.set(type_to_source_prop(dev), val)
}

/// `DeviceFilesystem.type`'s setter: the source value moves to the
/// new type's source attribute.
pub fn set_type(dev: &mut XmlObject, val: Option<Value>) -> Result<(), String> {
    // Get type/value of the attribute of "source" property
    let old_source_type = type_to_source_prop(dev);
    let old_source_value = get_source(dev);

    // Update "type" property
    dev.set("_type_prop", val)?;

    // If the attribute type of 'source' property has changed
    // restore the value
    if old_source_type != type_to_source_prop(dev) {
        set_source(dev, old_source_value)?;
    }
    Ok(())
}

/// `DeviceFilesystem.set_defaults`. `qemu_lxc_or_test` is
/// `conn.is_qemu() or conn.is_lxc() or conn.is_test()`.
pub fn set_defaults(dev: &mut XmlObject, qemu_lxc_or_test: bool) -> Result<(), String> {
    if !qemu_lxc_or_test {
        return Ok(());
    }

    // type=mount is the libvirt default. But hardcode it since other
    // bits like validation depend on it
    if dev.get("_type_prop").is_none() {
        set_type(dev, Some(TYPE_MOUNT.into()))?;
    }

    if dev.get("accessmode").is_none() {
        // `default_accessmode`
        if dev.get_str("driver_type").as_deref() != Some("virtiofs") {
            // libvirt qemu defaults to accessmode=passthrough, but that
            // really only works well for qemu running as root, which is
            // not the common case. so use mode=mapped
            dev.set("accessmode", Some("mapped".into()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod edit_tests {
    use super::*;

    fn apply(xml: &str, edits: FsEdits) -> String {
        let mut doc = virtinst_xml::parse_libvirt_xml(xml).unwrap();
        let fs = doc.root_element().unwrap();
        edits.apply(&mut doc, fs);
        doc.write_str().unwrap()
    }

    #[test]
    fn virtiofs_switch() {
        let out = apply(
            "<filesystem type='mount' accessmode='mapped'><source dir='/srv'/>\
             <target dir='share'/></filesystem>",
            FsEdits {
                driver: Some(Some("virtiofs".into())),
                readonly: Some(true),
                target: Some("data".into()),
                ..Default::default()
            },
        );
        assert!(out.contains("<driver type=\"virtiofs\""), "{out}");
        assert!(!out.contains("accessmode"), "{out}");
        assert!(out.contains("<readonly/>"), "{out}");
        assert!(out.contains("<target dir=\"data\""), "{out}");
        let out = apply(
            &out,
            FsEdits {
                driver: Some(None),
                readonly: Some(false),
                ..Default::default()
            },
        );
        assert!(out.contains("accessmode=\"mapped\""), "{out}");
        assert!(
            !out.contains("<readonly") && !out.contains("<driver"),
            "{out}"
        );
    }

    #[test]
    fn type_moves_source() {
        let out = apply(
            "<filesystem type='mount'><source dir='/img'/></filesystem>",
            FsEdits {
                fstype: Some(Some(TYPE_FILE.into())),
                ..Default::default()
            },
        );
        assert!(out.contains("<source file=\"/img\""), "{out}");
        let out = apply(
            "<filesystem type='ram'><source usage='1024' units='KiB'/></filesystem>",
            FsEdits {
                source: Some("2".into()),
                ..Default::default()
            },
        );
        assert!(
            out.contains("usage=\"2\"") && out.contains("units=\"MiB\""),
            "{out}"
        );
    }
}
