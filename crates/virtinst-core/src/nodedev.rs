//! `<device>` node device XML — upstream's `virtinst/nodedev.py`, plus
//! the `pretty_name` from `virtManager/object/nodedev.py`, which the
//! details pages show.

use virtinst_xml::{Document, Element};

/// The parts of a node device the UI reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeDevice {
    pub name: String,
    pub parent: Option<String>,
    /// `./capability/@type`: `pci`, `usb_device`, `net`, `drm`, ...
    pub device_type: Option<String>,
    pub interface: Option<String>,
    pub domain: Option<String>,
    pub bus: Option<String>,
    pub slot: Option<String>,
    pub function: Option<String>,
    pub device: Option<String>,
    pub product_name: Option<String>,
    pub vendor_name: Option<String>,
    pub product_id: Option<String>,
    pub vendor_id: Option<String>,
    /// `./capability/type`, for `drm` devices.
    pub drm_type: Option<String>,
    /// `<devnode>` paths, in document order.
    pub devnodes: Vec<String>,
}

fn text(doc: &Document, el: Element, path: &[&str]) -> Option<String> {
    virtinst_xml::resolve_path(doc, el, path).map(|e| e.text_content(doc))
}

impl NodeDevice {
    pub fn parse(xml: &str) -> Option<NodeDevice> {
        let doc = virtinst_xml::parse_libvirt_xml(xml).ok()?;
        let root = doc.root_element()?;
        let cap = root.find(&doc, "capability");
        let cap_text = |name: &str| cap.and_then(|c| text(&doc, c, &[name]));
        let cap_attr = |name: &str, attr: &str| {
            cap.and_then(|c| c.find(&doc, name))
                .and_then(|e| e.attribute(&doc, attr))
                .map(str::to_string)
        };
        Some(NodeDevice {
            name: text(&doc, root, &["name"])?,
            parent: text(&doc, root, &["parent"]),
            device_type: cap
                .and_then(|c| c.attribute(&doc, "type"))
                .map(str::to_string),
            interface: cap_text("interface"),
            domain: cap_text("domain"),
            bus: cap_text("bus"),
            slot: cap_text("slot"),
            function: cap_text("function"),
            device: cap_text("device"),
            product_name: cap_text("product"),
            vendor_name: cap_text("vendor"),
            product_id: cap_attr("product", "id"),
            vendor_id: cap_attr("vendor", "id"),
            drm_type: cap_text("type"),
            devnodes: root
                .find_all(&doc, "devnode")
                .into_iter()
                .map(|d| d.text_content(&doc))
                .collect(),
        })
    }

    pub fn is_drm_render(&self) -> bool {
        self.device_type.as_deref() == Some("drm") && self.drm_type.as_deref() == Some("render")
    }

    /// `get_devnode(parent="by-path")`: the `/dev/dri/by-path/...` node
    /// if there is one, else the first.
    pub fn get_devnode(&self) -> Option<&str> {
        self.devnodes
            .iter()
            .find(|p| {
                let parts: Vec<&str> = p.split('/').collect();
                parts.len() > 2 && parts[parts.len() - 2] == "by-path"
            })
            .or(self.devnodes.first())
            .map(String::as_str)
    }
}

/// Python's `int(x)` over libvirt's decimal or `0x` hex strings.
fn parse_int(val: Option<&str>) -> u64 {
    let val = val.unwrap_or_default().trim();
    match val.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16).unwrap_or(0),
        None => val.parse().unwrap_or(0),
    }
}

/// `_usb_pretty_name`.
fn usb_pretty_name(dev: &NodeDevice) -> String {
    let pick = |name: &Option<String>, id: &Option<String>| {
        let name = name.as_deref().unwrap_or_default().trim();
        if name.is_empty() {
            id.clone().unwrap_or_default()
        } else {
            name.to_string()
        }
    };
    let product = pick(&dev.product_name, &dev.product_id);
    let vendor = pick(&dev.vendor_name, &dev.vendor_id);
    let busstr = format!(
        "{:03}:{:03}",
        parse_int(dev.bus.as_deref()),
        parse_int(dev.device.as_deref())
    );
    format!("{busstr} {vendor} {product}")
}

/// `vmmNodeDevice.pretty_name`; `all` is the host's node devices, for
/// a DRM device's parent.
pub fn pretty_name(dev: &NodeDevice, all: &[NodeDevice]) -> String {
    match dev.device_type.as_deref() {
        Some("net") => format!("Interface {}", dev.interface.as_deref().unwrap_or_default()),
        Some("pci") => format!(
            "{:04X}:{:02X}:{:02X}:{:X} {} {}",
            parse_int(dev.domain.as_deref()),
            parse_int(dev.bus.as_deref()),
            parse_int(dev.slot.as_deref()),
            parse_int(dev.function.as_deref()),
            dev.vendor_name.as_deref().unwrap_or("None"),
            dev.product_name.as_deref().unwrap_or("None"),
        ),
        Some("usb_device") => usb_pretty_name(dev),
        Some("drm") => {
            let parent = dev.parent.clone().unwrap_or_default();
            // Apparently we can't depend on a successful parent lookup.
            let pretty_parent = all
                .iter()
                .find(|d| d.name == parent)
                .map(|p| pretty_name(p, all))
                .unwrap_or(parent);
            format!(
                "{pretty_parent} ({})",
                dev.drm_type.as_deref().unwrap_or("None")
            )
        }
        _ => dev.name.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PCI: &str = "<device><name>pci_0000_00_02_0</name><parent>computer</parent>\
        <capability type='pci'><domain>0</domain><bus>0</bus><slot>2</slot>\
        <function>0</function><product id='0x5917'>UHD Graphics 620</product>\
        <vendor id='0x8086'>Intel Corporation</vendor></capability></device>";
    const DRM: &str = "<device><name>drm_renderD128</name>\
        <parent>pci_0000_00_02_0</parent>\
        <devnode type='dev'>/dev/dri/renderD128</devnode>\
        <devnode type='link'>/dev/dri/by-path/pci-0000:00:02.0-render</devnode>\
        <capability type='drm'><type>render</type></capability></device>";
    const USB: &str = "<device><name>usb_1_2</name><capability type='usb_device'>\
        <bus>1</bus><device>2</device><product id='0x0001'/>\
        <vendor id='0x1d6b'>Linux Foundation</vendor></capability></device>";

    #[test]
    fn pretty_names() {
        let pci = NodeDevice::parse(PCI).unwrap();
        let drm = NodeDevice::parse(DRM).unwrap();
        let all = vec![pci.clone(), drm.clone()];
        assert_eq!(
            pretty_name(&pci, &all),
            "0000:00:02:0 Intel Corporation UHD Graphics 620"
        );
        assert!(drm.is_drm_render());
        assert_eq!(
            drm.get_devnode(),
            Some("/dev/dri/by-path/pci-0000:00:02.0-render")
        );
        assert_eq!(
            pretty_name(&drm, &all),
            "0000:00:02:0 Intel Corporation UHD Graphics 620 (render)"
        );
        assert_eq!(pretty_name(&drm, &[]), "pci_0000_00_02_0 (render)");
        let usb = NodeDevice::parse(USB).unwrap();
        assert_eq!(pretty_name(&usb, &[]), "001:002 Linux Foundation 0x0001");
    }
}
