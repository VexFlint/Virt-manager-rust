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
    /// `./capability/uuid`, for `mdev` devices.
    pub uuid: Option<String>,
    /// `./capability/type/@id`, for `mdev` devices.
    pub type_id: Option<String>,
    /// `_capability_type`: `./capability/capability/@type`.
    pub capability_type: Option<String>,
    /// `./capability/host`, for `scsi_host` devices.
    pub host: Option<String>,
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
            uuid: cap_text("uuid"),
            type_id: cap_attr("type", "id"),
            capability_type: cap
                .and_then(|c| c.find(&doc, "capability"))
                .and_then(|c| c.attribute(&doc, "type"))
                .map(str::to_string),
            host: cap_text("host"),
        })
    }

    pub fn is_pci_bridge(&self) -> bool {
        self.capability_type.as_deref() == Some("pci-bridge")
    }

    pub fn is_usb_linux_root_hub(&self) -> bool {
        self.vendor_id.as_deref() == Some("0x1d6b")
            && matches!(
                self.product_id.as_deref(),
                Some("0x0001" | "0x0002" | "0x0003")
            )
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

/// `_compare_int`'s `_intify`: `None` for a missing or bad value.
fn intify(val: Option<&str>) -> Option<i64> {
    let val = val?.trim();
    if let Some(pos) = val.find("0x") {
        i64::from_str_radix(&val[pos + 2..], 16).ok()
    } else {
        val.parse().ok()
    }
}

/// `_compare_int`: a value the hostdev doesn't set matches anything.
fn compare_int(nodedev_val: Option<&str>, hostdev_val: Option<&str>) -> bool {
    let hostdev_val = intify(hostdev_val);
    hostdev_val.is_none() || intify(nodedev_val) == hostdev_val
}

/// `uuid.UUID(val)`, as 32 lowercase hex digits.
fn normalize_uuid(val: &str) -> Option<String> {
    let val = val.trim();
    let val = val.strip_prefix("urn:uuid:").unwrap_or(val);
    let hex: String = val
        .chars()
        .filter(|c| !matches!(c, '-' | '{' | '}'))
        .collect::<String>()
        .to_lowercase();
    (hex.len() == 32 && hex.chars().all(|c| c.is_ascii_hexdigit())).then_some(hex)
}

impl NodeDevice {
    /// `get_mdev_uuid`: libvirt 7.3.0 added `<uuid>`; before that the
    /// name is `mdev_$UUID_WITH_UNDERSCORES`.
    pub fn get_mdev_uuid(&self) -> String {
        match &self.uuid {
            Some(uuid) => uuid.clone(),
            None => self.name.get(5..).unwrap_or_default().replace('_', "-"),
        }
    }

    /// `compare_to_hostdev`: whether `hostdev` (a `<hostdev>`) passes
    /// this device through.
    pub fn compare_to_hostdev(&self, doc: &Document, hostdev: Element) -> bool {
        let hattr = |path: &[&str], name: &str| {
            virtinst_xml::resolve_path(doc, hostdev, path)
                .and_then(|e| e.attribute(doc, name))
                .map(str::to_string)
        };
        let addr = |name: &str| hattr(&["source", "address"], name);
        let htype = hostdev.attribute(doc, "type");
        match self.device_type.as_deref() {
            Some("pci") => {
                htype == Some("pci")
                    && compare_int(self.domain.as_deref(), addr("domain").as_deref())
                    && compare_int(self.bus.as_deref(), addr("bus").as_deref())
                    && compare_int(self.slot.as_deref(), addr("slot").as_deref())
                    && compare_int(self.function.as_deref(), addr("function").as_deref())
            }
            Some("usb_device") => {
                htype == Some("usb")
                    && compare_int(
                        self.product_id.as_deref(),
                        hattr(&["source", "product"], "id").as_deref(),
                    )
                    && compare_int(
                        self.vendor_id.as_deref(),
                        hattr(&["source", "vendor"], "id").as_deref(),
                    )
                    && compare_int(self.bus.as_deref(), addr("bus").as_deref())
                    && compare_int(self.device.as_deref(), addr("device").as_deref())
            }
            // Upstream's `_compare_uuid` returns a truthy -1 for a value
            // it can't parse; a bad UUID matches nothing here.
            Some("mdev") => {
                htype == Some("mdev")
                    && match (
                        normalize_uuid(&self.get_mdev_uuid()),
                        addr("uuid").as_deref().and_then(normalize_uuid),
                    ) {
                        (Some(a), Some(b)) => a == b,
                        _ => false,
                    }
            }
            _ => false,
        }
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
        assert!(usb.is_usb_linux_root_hub() && !pci.is_pci_bridge());
        let bridge = NodeDevice::parse(
            "<device><name>pci_0000_00_1c_0</name><capability type='pci'>\
             <capability type='pci-bridge'/></capability></device>",
        )
        .unwrap();
        assert!(bridge.is_pci_bridge());
        let mdev = NodeDevice::parse(
            "<device><name>mdev_x</name><capability type='mdev'>\
             <type id='nvidia-63'/><uuid>4b20d080-1b54-4048-85b3-a6a62d165c01</uuid>\
             </capability></device>",
        )
        .unwrap();
        assert_eq!(mdev.type_id.as_deref(), Some("nvidia-63"));
    }

    fn hostdev(xml: &str) -> (Document, Element) {
        let doc = virtinst_xml::parse_libvirt_xml(xml).unwrap();
        let el = doc.root_element().unwrap();
        (doc, el)
    }

    #[test]
    fn compare_to_hostdev() {
        let pci = NodeDevice::parse(PCI).unwrap();
        let usb = NodeDevice::parse(USB).unwrap();
        let (doc, el) = hostdev(
            "<hostdev type='pci'><source><address domain='0x0000' bus='0x00' \
             slot='0x02' function='0x0'/></source></hostdev>",
        );
        assert!(pci.compare_to_hostdev(&doc, el));
        assert!(!usb.compare_to_hostdev(&doc, el));
        let (doc, el) = hostdev(
            "<hostdev type='pci'><source><address domain='0x0000' bus='0x00' \
             slot='0x03' function='0x0'/></source></hostdev>",
        );
        assert!(!pci.compare_to_hostdev(&doc, el));
        // vendor/product only: the unset bus/device match anything
        let (doc, el) = hostdev(
            "<hostdev type='usb'><source><vendor id='0x1d6b'/>\
             <product id='0x0001'/></source></hostdev>",
        );
        assert!(usb.compare_to_hostdev(&doc, el));
        let mdev = NodeDevice::parse(
            "<device><name>mdev_4b20d080_1b54_4048_85b3_a6a62d165c01</name>\
             <capability type='mdev'/></device>",
        )
        .unwrap();
        let (doc, el) = hostdev(
            "<hostdev type='mdev'><source>\
             <address uuid='4B20D080-1B54-4048-85B3-A6A62D165C01'/></source></hostdev>",
        );
        assert!(mdev.compare_to_hostdev(&doc, el));
    }
}
