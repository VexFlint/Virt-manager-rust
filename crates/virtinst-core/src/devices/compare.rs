//! `Device.compare_device` and `Guest.find_device`: finding the same
//! device again in another copy of the domain XML (the inactive config,
//! or a newer live XML), where it may sit at another position or look
//! slightly different.

use virtinst_xml::{Document, Element};

use super::edit::attr;

/// One of `compare_device`'s `devprops`.
enum Prop {
    /// `./path/@name`; an empty path is the device element itself.
    Attr(&'static [&'static str], &'static str),
    /// `xmlindex`: the position among same-tag siblings.
    XmlIndex,
}

use Prop::{Attr, XmlIndex};

/// `devprops`, by device tag. `disk` and `interface` have their own
/// lookups in `disk.rs` and `network.rs`.
fn devprops(tag: &str) -> &'static [Prop] {
    match tag {
        "disk" => &[Attr(&["target"], "dev"), Attr(&["target"], "bus")],
        "interface" => &[Attr(&["mac"], "address"), XmlIndex],
        "input" => &[Attr(&[], "bus"), Attr(&[], "type"), XmlIndex],
        "sound" => &[Attr(&[], "model"), XmlIndex],
        "audio" => &[Attr(&[], "type"), Attr(&[], "id")],
        "video" => &[Attr(&["model"], "type"), XmlIndex],
        "watchdog" => &[Attr(&[], "model"), XmlIndex],
        "hostdev" => &[
            Attr(&[], "type"),
            Attr(&[], "managed"),
            XmlIndex,
            Attr(&["source", "product"], "id"),
            Attr(&["source", "vendor"], "id"),
            Attr(&["source", "address"], "function"),
            Attr(&["source", "address"], "domain"),
            Attr(&["source", "address"], "slot"),
        ],
        "serial" | "parallel" => &[Attr(&[], "type"), Attr(&["target"], "port")],
        "console" => &[
            Attr(&[], "type"),
            Attr(&["target"], "type"),
            Attr(&["target"], "port"),
        ],
        "graphics" => &[Attr(&[], "type"), XmlIndex],
        "controller" => &[Attr(&[], "type"), Attr(&[], "index")],
        "channel" => &[Attr(&[], "type"), Attr(&["target"], "name")],
        "filesystem" => &[Attr(&["target"], "dir"), XmlIndex],
        "smartcard" => &[Attr(&[], "mode"), XmlIndex],
        "redirdev" => &[Attr(&[], "bus"), Attr(&[], "type"), XmlIndex],
        "tpm" => &[Attr(&["backend"], "type"), XmlIndex],
        "rng" => &[Attr(&["backend"], "model"), XmlIndex],
        "panic" => &[Attr(&[], "model"), XmlIndex],
        "shmem" => &[Attr(&[], "name"), XmlIndex],
        "vsock" | "memballoon" | "iommu" => &[Attr(&[], "model"), XmlIndex],
        _ => &[],
    }
}

/// What `compare_device` looks at for one device, taken from the XML
/// the device was shown from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceId {
    pub tag: String,
    pub idx: usize,
    vals: Vec<Option<String>>,
}

fn prop_values(doc: &Document, el: Element, tag: &str) -> Vec<Option<String>> {
    devprops(tag)
        .iter()
        .map(|p| match p {
            Attr(path, name) => attr(doc, el, path, name),
            XmlIndex => None,
        })
        .collect()
}

/// The [`DeviceId`] of `el`, the `idx`th `<tag>` under `<devices>`.
pub fn device_id(doc: &Document, el: Element, tag: &str, idx: usize) -> DeviceId {
    DeviceId {
        tag: tag.to_string(),
        idx,
        vals: prop_values(doc, el, tag),
    }
}

/// `find_device`: the first `<tag>` in `root`'s `<devices>` that
/// `compare_device` matches against `id`.
pub fn find_device(doc: &Document, root: Element, id: &DeviceId) -> Option<Element> {
    let props = devprops(&id.tag);
    if props.is_empty() {
        return None;
    }
    let devices = root.find(doc, "devices")?;
    devices
        .find_all(doc, &id.tag)
        .into_iter()
        .enumerate()
        .find(|(idx, el)| {
            let vals = prop_values(doc, *el, &id.tag);
            props
                .iter()
                .zip(vals.iter().zip(&id.vals))
                .all(|(p, (new, orig))| match p {
                    XmlIndex => *idx == id.idx,
                    Attr(..) => new == orig,
                })
        })
        .map(|(_, el)| el)
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = "<domain><devices>\
        <sound model='ich9'/><sound model='ac97'/>\
        <video><model type='virtio'/></video>\
        <controller type='usb' index='0'/><controller type='pci' index='0'/>\
        <serial type='pty'><target port='0'/></serial>\
        </devices></domain>";

    fn nth(doc: &Document, tag: &str, idx: usize) -> Element {
        let devices = doc.root_element().unwrap().find(doc, "devices").unwrap();
        devices.find_all(doc, tag)[idx]
    }

    #[test]
    fn finds_by_props_and_index() {
        let doc = virtinst_xml::parse_libvirt_xml(XML).unwrap();
        let root = doc.root_element().unwrap();
        let ac97 = nth(&doc, "sound", 1);
        let id = device_id(&doc, ac97, "sound", 1);
        assert_eq!(find_device(&doc, root, &id), Some(ac97));

        // Same model at another position doesn't match.
        let moved = DeviceId {
            idx: 0,
            ..id.clone()
        };
        assert_eq!(find_device(&doc, root, &moved), None);

        // Controllers match by type and index, not position.
        let pci = nth(&doc, "controller", 1);
        let id = device_id(&doc, pci, "controller", 7);
        assert_eq!(find_device(&doc, root, &id), Some(pci));

        let serial = nth(&doc, "serial", 0);
        let id = device_id(&doc, serial, "serial", 0);
        assert_eq!(find_device(&doc, root, &id), Some(serial));
    }

    #[test]
    fn other_xml_copy() {
        let doc = virtinst_xml::parse_libvirt_xml(XML).unwrap();
        let id = device_id(&doc, nth(&doc, "video", 0), "video", 0);
        let other = XML.replace("type='virtio'", "type='qxl'");
        let doc2 = virtinst_xml::parse_libvirt_xml(&other).unwrap();
        assert_eq!(find_device(&doc2, doc2.root_element().unwrap(), &id), None);
    }
}
