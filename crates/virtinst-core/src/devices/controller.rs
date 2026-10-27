//! `<controller>` — upstream's `devices/controller.py`. Grounded in the
//! real `XMLProperty` declarations first: most of this device is
//! `is_int=True` attributes (`index`, `vectors`, `ports`, plus several
//! nested under `driver`/`target`), the first real exercise of
//! `Option<u32>` binding.
//!
//! This is the type ticket 12 named directly — a new `DeviceDisk`
//! needing virtio-scsi implicitly wants a companion `DeviceController`,
//! folded directly into `NewDevice::Disk`'s `companion_controller`
//! field, exactly as ticket 12 resolved (see `devices::new_device`).
//!
//! `target/@hotplug` uses libvirt's `on`/`off` boolean spelling, not
//! `yes`/`no` — `Option<OnOff>` (`virtinst_xml`), added once
//! `domain::DomainKeyWrap` made it a real gap rather than a
//! speculative one.

use virtinst_xml::{OnOff, XmlBound};

/// ```xml
/// <controller type="scsi" index="0" model="virtio-scsi">
///   <driver queues="4"/>
///   <target chassis="0" hotplug="on"/>
/// </controller>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "controller")]
pub struct DeviceController {
    #[xml(attribute = "type")]
    pub controller_type: Option<String>,

    #[xml(attribute = "model")]
    pub model: Option<String>,

    #[xml(attribute = "index")]
    pub index: Option<u32>,

    #[xml(path = "driver", attribute = "queues")]
    pub driver_queues: Option<u32>,

    #[xml(path = "target", attribute = "chassis")]
    pub target_chassis: Option<u32>,

    #[xml(path = "target", attribute = "hotplug")]
    pub target_hotplug: Option<OnOff>,
}

// Editing controllers in place, for the details page.

use virtinst_xml::{Document, Element, Node};

/// `get_usb2_controllers`: an EHCI controller and its three UHCI
/// companions, as `(model, master startport)`.
const USB2_CONTROLLERS: [(&str, Option<&str>); 4] = [
    ("ich9-ehci1", None),
    ("ich9-uhci1", Some("0")),
    ("ich9-uhci2", Some("2")),
    ("ich9-uhci3", Some("4")),
];

fn new_usb_controller(doc: &mut Document, model: Option<&str>) -> Element {
    let ctrl = Element::new(doc, "controller");
    ctrl.set_attribute(doc, "type", "usb");
    if let Some(model) = model {
        ctrl.set_attribute(doc, "model", model);
    }
    ctrl
}

/// `define_controller`'s USB model change: every USB controller is
/// replaced by the set for the new model. `usb3` is `qemu-xhci` with
/// 15 ports (`get_usb3_controller`, on a libvirt with
/// `conn_qemu_xhci` and `conn_usb3_ports`); `ich9-ehci1` brings its
/// UHCI companions.
pub fn set_usb_model(doc: &mut Document, devices: Element, model: Option<&str>) {
    for ctrl in devices.find_all(doc, "controller") {
        if ctrl.attribute(doc, "type") == Some("usb") {
            let _ = ctrl.detach(doc);
        }
    }
    let mut new = vec![];
    match model {
        Some("ich9-ehci1") => {
            for (m, startport) in USB2_CONTROLLERS {
                let ctrl = new_usb_controller(doc, Some(m));
                if let Some(port) = startport {
                    let master = Element::new(doc, "master");
                    master.set_attribute(doc, "startport", port);
                    ctrl.push_child(doc, master)
                        .expect("a fresh element always attaches");
                }
                new.push(ctrl);
            }
        }
        Some("usb3") => {
            let ctrl = new_usb_controller(doc, Some("qemu-xhci"));
            ctrl.set_attribute(doc, "ports", "15");
            new.push(ctrl);
        }
        other => new.push(new_usb_controller(doc, other)),
    }
    // `add_device`: after the last controller, else at the end.
    let children = devices.children(doc);
    let start = children
        .iter()
        .rposition(|n| n.as_element().is_some_and(|e| e.name(doc) == "controller"))
        .map_or(children.len(), |i| i + 1);
    for (pos, ctrl) in (start..).zip(new) {
        devices
            .insert_child(doc, pos, Node::Element(ctrl))
            .expect("a fresh element always attaches");
    }
}

/// `define_controller` for other controllers: the model, and a fresh
/// guest address.
pub fn set_model(doc: &mut Document, ctrl: Element, model: Option<&str>) {
    super::edit::set_attr(doc, ctrl, "", "model", model);
    super::edit::clear_address(doc, ctrl);
}

/// `populate_controller_model_combo`'s rows: `(model, label)`.
pub fn model_rows(ctype: &str) -> Vec<(Option<&'static str>, &'static str)> {
    let mut rows = match ctype {
        "usb" => vec![(Some("usb3"), "USB 3"), (Some("ich9-ehci1"), "USB 2")],
        "scsi" => vec![(Some("virtio-scsi"), "VirtIO SCSI")],
        _ => vec![],
    };
    rows.push((None, "Hypervisor default"));
    rows
}

// `DeviceController` as an `XMLBuilder` class, for the CLI parsers.

use crate::xmlbuilder::{prop, Kind, XmlClass, XmlObject};

/// `DeviceController`.
pub static DEVICE_CONTROLLER: XmlClass = XmlClass {
    xml_name: "controller",
    props: &[
        prop("type", "./@type", Kind::Str),
        prop("model", "./@model", Kind::Str),
        prop("vectors", "./@vectors", Kind::Int),
        prop("ports", "./@ports", Kind::Int),
        prop("maxGrantFrames", "./@maxGrantFrames", Kind::Int),
        prop("index", "./@index", Kind::Int),
        prop("driver_iothread", "./driver/@iothread", Kind::Int),
        prop("driver_queues", "./driver/@queues", Kind::Int),
        prop("master_startport", "./master/@startport", Kind::Int),
        prop("target_chassisNr", "./target/@chassisNr", Kind::Int),
        prop("target_chassis", "./target/@chassis", Kind::Int),
        prop("target_port", "./target/@port", Kind::Int),
        prop("target_hotplug", "./target/@hotplug", Kind::OnOff),
        prop("target_busNr", "./target/@busNr", Kind::Int),
        prop("target_index", "./target/@index", Kind::Int),
        prop("target_node", "./target/node", Kind::Int),
        prop("target_memReserve", "./target/@memReserve", Kind::Int),
    ],
    children: crate::device_children!(),
    prop_order: crate::device_prop_order!(
        "type",
        "index",
        "model",
        "master_startport",
        "driver_queues",
        "maxGrantFrames",
    ),
};

/// `DeviceController.get_usb2_controllers`.
pub fn get_usb2_controllers() -> Vec<XmlObject> {
    USB2_CONTROLLERS
        .iter()
        .map(|(model, startport)| {
            let mut ctrl = XmlObject::new(&DEVICE_CONTROLLER);
            let _ = ctrl.set("type", Some("usb".into()));
            let _ = ctrl.set("model", Some((*model).into()));
            if let Some(port) = startport {
                let _ = ctrl.set("master_startport", Some((*port).into()));
            }
            ctrl
        })
        .collect()
}

/// `DeviceController.get_usb3_controller`. `qemu_xhci` and
/// `usb3_ports` are `conn.support.conn_qemu_xhci()` and
/// `conn_usb3_ports()`.
pub fn get_usb3_controller(qemu_xhci: bool, usb3_ports: bool) -> XmlObject {
    let mut ctrl = XmlObject::new(&DEVICE_CONTROLLER);
    let _ = ctrl.set("type", Some("usb".into()));
    let _ = ctrl.set("model", Some("nec-xhci".into()));
    if qemu_xhci {
        let _ = ctrl.set("model", Some("qemu-xhci".into()));
    }
    if usb3_ports {
        // 15 is the max ports qemu supports, might as well
        // Add as many as possible
        let _ = ctrl.set("ports", Some("15".into()));
    }
    ctrl
}

#[cfg(test)]
mod edit_tests {
    use super::*;

    #[test]
    fn usb_model_swap() {
        let xml = "<domain><devices><disk/>\
            <controller type='usb' index='0' model='qemu-xhci'/>\
            <controller type='sata' index='0'/><interface/></devices></domain>";
        let mut doc = virtinst_xml::parse_libvirt_xml(xml).unwrap();
        let devices = doc.root_element().unwrap().find(&doc, "devices").unwrap();
        set_usb_model(&mut doc, devices, Some("ich9-ehci1"));
        let names: Vec<String> = devices
            .child_elements(&doc)
            .iter()
            .map(|e| {
                format!(
                    "{}:{}",
                    e.name(&doc),
                    e.attribute(&doc, "model").unwrap_or_default()
                )
            })
            .collect();
        assert_eq!(
            names,
            [
                "disk:",
                "controller:",
                "controller:ich9-ehci1",
                "controller:ich9-uhci1",
                "controller:ich9-uhci2",
                "controller:ich9-uhci3",
                "interface:"
            ]
        );
        set_usb_model(&mut doc, devices, Some("usb3"));
        let out = doc.write_str().unwrap();
        assert!(out.contains("model=\"qemu-xhci\""), "{out}");
        assert!(out.contains("ports=\"15\""), "{out}");
        assert!(!out.contains("uhci"), "{out}");
    }

    #[test]
    fn scsi_model() {
        let xml =
            "<controller type='scsi' index='0' model='lsilogic'><address type='pci'/></controller>";
        let mut doc = virtinst_xml::parse_libvirt_xml(xml).unwrap();
        let ctrl = doc.root_element().unwrap();
        set_model(&mut doc, ctrl, Some("virtio-scsi"));
        let out = doc.write_str().unwrap();
        assert!(
            out.contains("virtio-scsi") && !out.contains("<address"),
            "{out}"
        );
        set_model(&mut doc, ctrl, None);
        assert_eq!(ctrl.attribute(&doc, "model"), None);
    }
}
