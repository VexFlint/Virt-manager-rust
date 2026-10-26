//! `Guest.remove_device`: drop a device from the domain XML, plus the
//! devices that only make sense alongside it.

use virtinst_xml::{Document, Element};

fn child_attr(doc: &Document, el: Element, child: &str, name: &str) -> Option<String> {
    el.find(doc, child)
        .and_then(|c| c.attribute(doc, name))
        .map(str::to_string)
}

/// `DeviceConsole.get_console_duplicate`: the first `<console>`, when
/// it is the libvirt-made copy of `serial`.
pub fn get_console_duplicate(doc: &Document, devices: Element, serial: Element) -> Option<Element> {
    if serial.name(doc) != "serial" {
        return None;
    }
    let console = devices.find(doc, "console")?;
    let target_type = child_attr(doc, console, "target", "type");
    (console.attribute(doc, "type") == serial.attribute(doc, "type")
        && target_type.as_deref().map_or(true, |t| t == "serial"))
    .then_some(console)
}

/// `_force_remove_spice_devices`: spice audio, spicevmc channels and
/// USB redirection, and spiceport serials/consoles.
pub fn force_remove_spice_devices(doc: &mut Document, devices: Element) {
    let doomed: Vec<Element> = devices
        .child_elements(doc)
        .into_iter()
        .filter(|el| {
            let typ = el.attribute(doc, "type");
            match el.name(doc) {
                "audio" => typ == Some("spice"),
                "channel" | "redirdev" => typ == Some("spicevmc"),
                "serial" | "console" => typ == Some("spiceport"),
                _ => false,
            }
        })
        .collect();
    for el in doomed {
        let _ = el.detach(doc);
    }
}

/// `Guest.remove_device`: removes `dev`, its duplicate `<console>`,
/// and the spice helpers once the last spice display is gone.
pub fn remove_device(doc: &mut Document, devices: Element, dev: Element) {
    let condup = get_console_duplicate(doc, devices, dev);
    let is_graphics = dev.name(doc) == "graphics";
    let _ = dev.detach(doc);
    if let Some(console) = condup {
        let _ = console.detach(doc);
    }
    let has_spice = devices
        .find_all(doc, "graphics")
        .iter()
        .any(|g| g.attribute(doc, "type") == Some("spice"));
    if is_graphics && !has_spice {
        force_remove_spice_devices(doc, devices);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(doc: &Document, devices: Element) -> Vec<String> {
        devices
            .child_elements(doc)
            .iter()
            .map(|e| {
                format!(
                    "{}:{}",
                    e.name(doc),
                    e.attribute(doc, "type").unwrap_or_default()
                )
            })
            .collect()
    }

    #[test]
    fn serial_takes_its_console() {
        let xml = "<devices><serial type='pty'/><console type='pty'>\
            <target type='serial'/></console><input type='tablet'/></devices>";
        let mut doc = virtinst_xml::parse_libvirt_xml(xml).unwrap();
        let devices = doc.root_element().unwrap();
        let serial = devices.find(&doc, "serial").unwrap();
        remove_device(&mut doc, devices, serial);
        assert_eq!(names(&doc, devices), ["input:tablet"]);
    }

    #[test]
    fn last_spice_display_takes_spice_helpers() {
        let xml = "<devices><graphics type='spice'/><channel type='spicevmc'/>\
            <channel type='unix'/><redirdev type='spicevmc'/><audio type='spice'/>\
            <audio type='none'/></devices>";
        let mut doc = virtinst_xml::parse_libvirt_xml(xml).unwrap();
        let devices = doc.root_element().unwrap();
        let gfx = devices.find(&doc, "graphics").unwrap();
        remove_device(&mut doc, devices, gfx);
        assert_eq!(names(&doc, devices), ["channel:unix", "audio:none"]);
    }
}
