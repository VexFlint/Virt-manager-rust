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
