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

    #[xml(path = "vcpu", text)]
    pub vcpus: Option<u32>,

    #[xml(path = "vcpu", attribute = "current")]
    pub vcpu_current: Option<u32>,

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

// Upstream `Guest`'s boot order helpers and the `define_boot` edits of
// `<os>`, over a domain document. Device ids are upstream's
// `get_xml_id()`: `./devices/<tag>[<n>]`, n counted per tag from 1.

/// `get_bootable_devices(exclude_redirdev=True)`'s xml ids, each with
/// its element: every disk, then every interface, then every hostdev.
fn bootable_devices(
    doc: &virtinst_xml::Document,
    root: virtinst_xml::Element,
) -> Vec<(String, virtinst_xml::Element)> {
    let Some(devices) = root.find(doc, "devices") else {
        return vec![];
    };
    ["disk", "interface", "hostdev"]
        .into_iter()
        .flat_map(|tag| {
            devices
                .find_all(doc, tag)
                .into_iter()
                .enumerate()
                .map(move |(i, el)| (format!("./devices/{tag}[{}]", i + 1), el))
        })
        .collect()
}

/// The xml ids of `get_bootable_devices(exclude_redirdev=True)`.
/// `Guest.generate_name`: a free VM name from the OS short id, like
/// `fedora40`, `fedora40-2`, or `vm1` for the generic OS.
pub fn generate_name(
    os_name: &str,
    is_container: bool,
    arch: &str,
    host_arch: Option<&str>,
    collides: impl Fn(&str) -> bool,
) -> String {
    let mut force_num = false;
    let mut basename = os_name
        .strip_suffix("-unknown")
        .unwrap_or(os_name)
        .to_string();
    if os_name == "generic" {
        force_num = true;
        basename = if is_container { "container" } else { "vm" }.into();
    }
    if Some(arch) != host_arch {
        let pretty = if arch == "armv7l" { "arm" } else { arch };
        basename = format!("{basename}-{pretty}");
        force_num = false;
    }
    // `generatename.generate_name(start_num=force_num and 1 or 2,
    // sep=not force_num and "-" or "")`
    let (start, sep) = if force_num { (1, "") } else { (2, "-") };
    (!force_num)
        .then(|| basename.clone())
        .into_iter()
        .chain((start..start + 100_000).map(|i| format!("{basename}{sep}{i}")))
        .find(|n| !collides(n))
        .unwrap_or(basename)
}

pub fn bootable_device_ids(
    doc: &virtinst_xml::Document,
    root: virtinst_xml::Element,
) -> Vec<String> {
    bootable_devices(doc, root)
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

fn old_boot_order(doc: &virtinst_xml::Document, root: virtinst_xml::Element) -> Vec<String> {
    let Some(os) = root.find(doc, "os") else {
        return vec![];
    };
    os.find_all(doc, "boot")
        .into_iter()
        .filter_map(|b| b.attribute(doc, "dev").map(str::to_string))
        .collect()
}

/// `_convert_old_boot_order`: `<os><boot dev=...>` as device ids.
fn convert_old_boot_order(
    doc: &virtinst_xml::Document,
    root: virtinst_xml::Element,
) -> Vec<String> {
    let devs = bootable_devices(doc, root);
    let first = |pred: &dyn Fn(&str, virtinst_xml::Element) -> bool| {
        devs.iter()
            .find(|(id, el)| pred(id, *el))
            .map(|(id, _)| id.clone())
    };
    let disk_device =
        |el: virtinst_xml::Element| el.attribute(doc, "device").unwrap_or("disk").to_string();
    let is_disk = |id: &str| id.starts_with("./devices/disk[");
    let cdrom = first(&|id, el| is_disk(id) && disk_device(el) == "cdrom");
    let floppy = first(&|id, el| is_disk(id) && disk_device(el) == "floppy");
    let disk =
        first(&|id, el| is_disk(id) && !matches!(disk_device(el).as_str(), "cdrom" | "floppy"));
    let net = first(&|id, _| id.starts_with("./devices/interface["));
    old_boot_order(doc, root)
        .iter()
        .filter_map(|b| match b.as_str() {
            "network" => net.clone(),
            "hd" => disk.clone(),
            "cdrom" => cdrom.clone(),
            "fd" => floppy.clone(),
            _ => None,
        })
        .collect()
}

/// `get_boot_order(legacy=...)`.
pub fn get_boot_order(
    doc: &virtinst_xml::Document,
    root: virtinst_xml::Element,
    legacy: bool,
) -> Vec<String> {
    if legacy {
        return old_boot_order(doc, root);
    }
    let mut order: Vec<(String, u32)> = bootable_devices(doc, root)
        .into_iter()
        .filter_map(|(id, el)| {
            let order = el
                .find(doc, "boot")?
                .attribute(doc, "order")?
                .parse()
                .ok()?;
            Some((id, order))
        })
        .collect();
    if order.is_empty() {
        // No devices individually marked bootable, convert traditional
        // boot XML to fine grained
        return convert_old_boot_order(doc, root);
    }
    order.sort_by_key(|(_, o)| *o);
    order.into_iter().map(|(id, _)| id).collect()
}

/// `set_boot_order(boot_order, legacy=...)`.
pub fn set_boot_order(
    doc: &mut virtinst_xml::Document,
    root: virtinst_xml::Element,
    boot_order: &[String],
    legacy: bool,
) {
    let os = virtinst_xml::resolve_or_create_path(doc, root, &["os"]);
    // Unset the traditional boot order
    for b in os.find_all(doc, "boot") {
        let _ = b.detach(doc);
    }
    if legacy {
        for dev in boot_order {
            let b = virtinst_xml::Element::new(doc, "boot");
            b.set_attribute(doc, "dev", dev.as_str());
            os.push_child(doc, b)
                .expect("a fresh element always attaches");
        }
        return;
    }
    // Unset device boot order
    if let Some(devices) = root.find(doc, "devices") {
        for dev in devices.child_elements(doc) {
            if let Some(boot) = dev.find(doc, "boot") {
                boot.mut_attributes(doc).remove("order");
                if boot.attributes(doc).is_empty() && boot.child_elements(doc).is_empty() {
                    let _ = boot.detach(doc);
                }
            }
        }
    }
    let devs = bootable_devices(doc, root);
    for (idx, id) in boot_order.iter().enumerate() {
        if let Some((_, el)) = devs.iter().find(|(i, _)| i == id) {
            let boot = virtinst_xml::resolve_or_create_path(doc, *el, &["boot"]);
            boot.set_attribute(doc, "order", (idx + 1).to_string());
        }
    }
}

/// Sets (or, for `None`, removes) a text child of `<os>`, such as
/// `kernel`, `initrd`, `dtb`, `cmdline` or `init`.
pub fn set_os_text(
    doc: &mut virtinst_xml::Document,
    root: virtinst_xml::Element,
    tag: &str,
    val: Option<&str>,
) {
    let os = virtinst_xml::resolve_or_create_path(doc, root, &["os"]);
    match val {
        Some(v) => virtinst_xml::resolve_or_create_path(doc, os, &[tag]).set_text_content(doc, v),
        None => virtinst_xml::remove_path_if_present(doc, os, &[tag])
            .expect("removing a plain child element cannot fail"),
    }
}

/// `os.bootmenu_enable = val`.
pub fn set_bootmenu(doc: &mut virtinst_xml::Document, root: virtinst_xml::Element, val: bool) {
    let os = virtinst_xml::resolve_or_create_path(doc, root, &["os"]);
    let menu = virtinst_xml::resolve_or_create_path(doc, os, &["bootmenu"]);
    menu.set_attribute(doc, "enable", if val { "yes" } else { "no" });
}

/// `os.set_initargs_string`: the arguments split shell-style.
pub fn set_initargs_string(
    doc: &mut virtinst_xml::Document,
    root: virtinst_xml::Element,
    argstring: &str,
) -> Result<(), String> {
    let args = shlex::split(argstring).ok_or("No closing quotation")?;
    let os = virtinst_xml::resolve_or_create_path(doc, root, &["os"]);
    for a in os.find_all(doc, "initarg") {
        let _ = a.detach(doc);
    }
    for val in args {
        let a = virtinst_xml::Element::new_with_text(doc, "initarg", &val);
        os.push_child(doc, a)
            .expect("a fresh element always attaches");
    }
    Ok(())
}

const LIBOSINFO_NS: &str = "http://libosinfo.org/xmlns/libvirt/domain/1.0";

/// `_set_os_obj`'s `_metadata.libosinfo.os_id = obj.full_id`: `None`
/// (the `generic` pseudo-OS) drops the libosinfo metadata, along with
/// the `<metadata>` element if nothing else is left in it.
pub fn set_os_id(
    doc: &mut virtinst_xml::Document,
    root: virtinst_xml::Element,
    os_id: Option<&str>,
) {
    let Some(os_id) = os_id else {
        if let Some(metadata) = root.find(doc, "metadata") {
            if let Some(lo) = metadata.find(doc, "libosinfo") {
                let _ = lo.detach(doc);
            }
            if !metadata.has_children_ignore_whitespace(doc) {
                let _ = metadata.detach(doc);
            }
        }
        return;
    };
    let metadata = virtinst_xml::resolve_or_create_path(doc, root, &["metadata"]);
    let lo = match metadata.find(doc, "libosinfo") {
        Some(lo) => lo,
        None => {
            let lo = virtinst_xml::Element::new(doc, "libosinfo:libosinfo");
            lo.set_namespace_decl(doc, "libosinfo", LIBOSINFO_NS);
            metadata
                .push_child(doc, lo)
                .expect("a fresh element always attaches");
            lo
        }
    };
    let os = match lo.find(doc, "os") {
        Some(os) => os,
        None => {
            let os = virtinst_xml::Element::new(doc, "libosinfo:os");
            lo.push_child(doc, os)
                .expect("a fresh element always attaches");
            os
        }
    };
    os.set_attribute(doc, "id", os_id);
}

/// `vmmDomain.get_init`'s args: shell-quoted and space-joined.
pub fn initargs_string(os: &DomainOs) -> String {
    os.initargs
        .iter()
        .map(|a| {
            let v = a.val.as_deref().unwrap_or("");
            shlex::try_quote(v)
                .map(|q| q.into_owned())
                .unwrap_or_else(|_| v.to_string())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------
// `Guest` and `_DomainDevices` as `XMLBuilder` classes, for the CLI
// parsers (`virtinst-cli`'s port of `VirtCLIParser`). Device classes
// join `DOMAIN_DEVICES` as their parsers get ported.
// ---------------------------------------------------------------------

use crate::xmlbuilder::{child, prop, Kind, XmlClass};

/// `_DomainDevices`.
pub static DOMAIN_DEVICES: XmlClass = XmlClass {
    xml_name: "devices",
    props: &[],
    children: &[
        child(
            "watchdog",
            &crate::devices::watchdog::DEVICE_WATCHDOG,
            ".",
            false,
        ),
        child(
            "memballoon",
            &crate::devices::memballoon::DEVICE_MEMBALLOON,
            ".",
            false,
        ),
        child("panic", &crate::devices::panic::DEVICE_PANIC, ".", false),
    ],
    prop_order: &[
        "disk",
        "controller",
        "filesystem",
        "interface",
        "smartcard",
        "serial",
        "parallel",
        "console",
        "channel",
        "input",
        "tpm",
        "graphics",
        "sound",
        "audio",
        "video",
        "hostdev",
        "redirdev",
        "watchdog",
        "memballoon",
        "rng",
        "panic",
        "shmem",
        "memory",
        "vsock",
        "iommu",
        "pstore",
    ],
};

/// `Guest`.
pub static GUEST: XmlClass = XmlClass {
    xml_name: "domain",
    props: &[
        prop("type", "./@type", Kind::Str),
        prop("name", "./name", Kind::Str),
        prop("uuid", "./uuid", Kind::Str),
    ],
    children: &[child("devices", &DOMAIN_DEVICES, ".", true)],
    prop_order: &["type", "name", "uuid"],
};

#[cfg(test)]
mod boot_tests {
    #[test]
    fn generate_name_like_upstream() {
        let none = |_: &str| false;
        let x86 = Some("x86_64");
        assert_eq!(
            generate_name("fedora40", false, "x86_64", x86, none),
            "fedora40"
        );
        assert_eq!(
            generate_name("fedora40", false, "x86_64", x86, |n| n == "fedora40"),
            "fedora40-2"
        );
        assert_eq!(generate_name("generic", false, "x86_64", x86, none), "vm1");
        assert_eq!(
            generate_name("generic", true, "x86_64", x86, none),
            "container1"
        );
        assert_eq!(
            generate_name("generic", false, "armv7l", x86, none),
            "vm-arm"
        );
        assert_eq!(
            generate_name("win10-unknown", false, "x86_64", x86, none),
            "win10"
        );
    }

    use super::*;

    const XML: &str = "<domain><os><type>hvm</type><boot dev='cdrom'/><boot dev='hd'/></os>\
        <devices><disk device='disk'/><disk device='cdrom'/><interface type='network'/>\
        <redirdev bus='usb'><boot order='9'/></redirdev></devices></domain>";

    fn parse(xml: &str) -> (virtinst_xml::Document, virtinst_xml::Element) {
        let doc = virtinst_xml::parse_libvirt_xml(xml).unwrap();
        let root = doc.root_element().unwrap();
        (doc, root)
    }

    #[test]
    fn old_boot_order_converts_to_device_ids() {
        let (doc, root) = parse(XML);
        assert_eq!(get_boot_order(&doc, root, true), ["cdrom", "hd"]);
        assert_eq!(
            get_boot_order(&doc, root, false),
            ["./devices/disk[2]", "./devices/disk[1]"]
        );
        assert_eq!(
            bootable_device_ids(&doc, root),
            [
                "./devices/disk[1]",
                "./devices/disk[2]",
                "./devices/interface[1]"
            ]
        );
    }

    #[test]
    fn device_boot_order_round_trip() {
        let (mut doc, root) = parse(XML);
        let order = vec![
            "./devices/interface[1]".to_string(),
            "./devices/disk[1]".to_string(),
        ];
        set_boot_order(&mut doc, root, &order, false);
        assert_eq!(get_boot_order(&doc, root, false), order);
        let x = doc.write_str().unwrap();
        assert!(
            !x.contains("dev=\"cdrom\"") && !x.contains("order=\"9\""),
            "{x}"
        );
        set_boot_order(&mut doc, root, &["network".into()], true);
        assert_eq!(get_boot_order(&doc, root, true), ["network"]);
    }

    #[test]
    fn os_edits() {
        let (mut doc, root) =
            parse("<domain><os><type>exe</type><init>/bin/sh</init></os></domain>");
        set_os_text(&mut doc, root, "kernel", Some("/vmlinuz"));
        set_os_text(&mut doc, root, "init", None);
        set_bootmenu(&mut doc, root, true);
        set_initargs_string(&mut doc, root, "-c 'echo hi'").unwrap();
        assert!(set_initargs_string(&mut doc, root, "'open").is_err());
        let guest = Guest::from_element(&doc, root);
        assert_eq!(guest.os.kernel.as_deref(), Some("/vmlinuz"));
        assert_eq!(guest.os.init, None);
        assert_eq!(guest.os.bootmenu_enable, Some(true));
        assert_eq!(initargs_string(&guest.os), "-c 'echo hi'");
    }

    #[test]
    fn os_id_round_trips() {
        let (mut doc, root) = parse("<domain><name>x</name></domain>");
        set_os_id(&mut doc, root, Some("http://fedoraproject.org/fedora/40"));
        set_os_id(&mut doc, root, Some("http://fedoraproject.org/fedora/41"));
        let xml = doc.write_str().unwrap();
        assert!(
            xml.contains("xmlns:libosinfo=\"http://libosinfo.org/xmlns/libvirt/domain/1.0\""),
            "{xml}"
        );
        assert!(
            xml.contains("<libosinfo:os id=\"http://fedoraproject.org/fedora/41\"/>"),
            "{xml}"
        );
        let reparsed = virtinst_xml::parse_libvirt_xml(&xml).unwrap();
        let r = reparsed.root_element().unwrap();
        let md = crate::domain::metadata::DomainMetadata::from_element(
            &reparsed,
            r.find(&reparsed, "metadata").unwrap(),
        );
        assert_eq!(
            md.libosinfo.os_id.as_deref(),
            Some("http://fedoraproject.org/fedora/41")
        );
        assert_eq!(Guest::from_element(&reparsed, r).metadata, md);
        set_os_id(&mut doc, root, None);
        assert!(!doc.write_str().unwrap().contains("metadata"));
    }
}
