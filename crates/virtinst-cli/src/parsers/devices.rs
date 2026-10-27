//! `--controller`, `--input`, `--sound`, `--audio` and `--video`:
//! `ParserController`, `ParserInput`, `ParserSound`, `ParserAudio` and
//! `ParserVideo`.

use std::sync::OnceLock;

use virtinst_core::devices::{audio, controller, input, sound, video};
use virtinst_core::xmlbuilder::{Value, XmlObject};

use crate::parser::{
    add_common_device_args, conn_support, ArgDef, ArgInst, Error, ParseCtx, ParserDef,
};

/// `ParserController._parse`.
fn controller_parse(ctx: &mut ParseCtx) -> Result<Option<Vec<XmlObject>>, Error> {
    if ctx.optstr == "usb2" {
        return Ok(Some(controller::get_usb2_controllers()));
    } else if ctx.optstr == "usb3" {
        let support = conn_support();
        return Ok(Some(vec![controller::get_usb3_controller(
            support.qemu_xhci,
            support.usb3_ports,
        )]));
    }
    if let Some(val) = ctx.optdict.remove("num_pcie_root_ports") {
        // `self.guest.num_pcie_root_ports = int(...)`: only virt-install's
        // q35 setup reads it.
        let val = val.unwrap_or_default();
        val.trim()
            .parse::<i64>()
            .map_err(|_| format!("invalid literal for int() with base 10: '{val}'"))?;
        return Ok(Some(vec![]));
    }
    Ok(None)
}

/// `ParserController.set_address_cb`.
fn controller_address_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    // Convenience option for address= PCI parsing. This pattern should
    // not be extended IMO, make users manually specify the address
    // fields they need
    let addrstr = val.map(|v| v.to_xml_string()).unwrap_or_default();
    let colons = addrstr.matches(':').count();
    if (colons == 1 || colons == 2) && addrstr.contains('.') {
        inst.set_path("address.type", Some("pci".into()))?;
        let (rest, function) = addrstr.split_once('.').expect("checked above");
        inst.set_path("address.function", Some(function.into()))?;
        let (rest, slot) = rest.rsplit_once(':').unwrap_or(("", rest));
        inst.set_path("address.slot", Some(slot.into()))?;
        inst.set_path("address.domain", Some("0".into()))?;
        if let Some((domain, bus)) = rest.split_once(':') {
            inst.set_path("address.domain", Some(domain.into()))?;
            inst.set_path("address.bus", Some(bus.into()))?;
        }
        return Ok(());
    }
    Err(format!("Expected PCI format string for '{addrstr}'"))
}

/// `VirtCLIParser.noset_cb`.
fn noset_cb(
    _ctx: &mut ParseCtx,
    _inst: &mut XmlObject,
    _val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    Ok(())
}

/// `ParserController`.
pub fn controller() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new(
            "controller",
            Some("devices.controller"),
            Some(&controller::DEVICE_CONTROLLER),
        );
        def.remove_first = &["type"];
        def.custom_parse = Some(controller_parse);
        add_common_device_args(&mut def, false, true);
        for (cli, prop) in [
            ("type", "type"),
            ("model", "model"),
            ("index", "index"),
            ("maxGrantFrames", "maxGrantFrames"),
            ("vectors", "vectors"),
            ("master.startport", "master_startport"),
            ("driver.iothread", "driver_iothread"),
            ("driver.queues", "driver_queues"),
            ("target.chassisNr", "target_chassisNr"),
            ("target.chassis", "target_chassis"),
            ("target.port", "target_port"),
            ("target.hotplug", "target_hotplug"),
            ("target.busNr", "target_busNr"),
            ("target.index", "target_index"),
            ("target.node", "target_node"),
            ("target.memReserve", "target_memReserve"),
        ] {
            def.arg(ArgDef::new(cli, prop));
        }
        def.arg(ArgDef::cb_only("address", controller_address_cb));
        def.arg(ArgDef::cb_only("num_pcie_root_ports", noset_cb));
        def.alias("master.startport", &["master"]);
        def.alias("driver.queues", &["driver_queues"]);
        def
    })
}

/// `ParserInput._parse` and `ParserSound._parse`: `none` only turns
/// off the default device.
fn none_skips_default(ctx: &mut ParseCtx) -> Result<Option<Vec<XmlObject>>, Error> {
    if ctx.optstr == "none" {
        // `self.guest.skip_default_input/sound = True`: only
        // virt-install's default devices read it.
        return Ok(Some(vec![]));
    }
    Ok(None)
}

/// `ParserInput`.
pub fn input() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new("input", Some("devices.input"), Some(&input::DEVICE_INPUT));
        def.remove_first = &["type"];
        def.stub_none = false;
        def.custom_parse = Some(none_skips_default);
        add_common_device_args(&mut def, false, true);
        def.arg(ArgDef::new("type", "type").ignore_default());
        def.arg(ArgDef::new("bus", "bus").ignore_default());
        def.arg(ArgDef::new("model", "model"));
        def.arg(ArgDef::new("source.evdev", "source_evdev"));
        def.arg(ArgDef::new("source.dev", "source_dev"));
        def.arg(ArgDef::new("source.repeat", "source_repeat").onoff());
        def.arg(ArgDef::new("source.grab", "source_grab"));
        def.arg(ArgDef::new("source.grabToggle", "source_grabToggle"));
        def
    })
}

/// `ParserSound`.
pub fn sound() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new("sound", Some("devices.sound"), Some(&sound::DEVICE_SOUND));
        def.remove_first = &["model"];
        def.stub_none = false;
        def.custom_parse = Some(none_skips_default);
        add_common_device_args(&mut def, false, false);
        def.arg(ArgDef::new("model", "model").ignore_default());
        def.arg(ArgDef::new("audio.id", "audio_id"));
        def.arg(ArgDef::new("codec[0-9]*.type", "type").find_inst("codec", "codecs"));
        def.arg(ArgDef::new("multichannel", "multichannel"));
        def.arg(ArgDef::new("streams", "streams"));
        def
    })
}

/// `ParserAudio`.
pub fn audio() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new("audio", Some("devices.audio"), Some(&audio::DEVICE_AUDIO));
        def.arg(ArgDef::new("type", "type"));
        def.arg(ArgDef::new("id", "id"));
        def
    })
}

/// `DeviceVideo.model`'s setter, for `model.type`.
fn video_model_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    video::set_model_prop(inst, val)
}

/// `ParserVideo`.
pub fn video() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new("video", Some("devices.video"), Some(&video::DEVICE_VIDEO));
        def.remove_first = &["model.type"];
        add_common_device_args(&mut def, false, true);
        def.arg(
            ArgDef::new("model.type", "_model")
                .cb(video_model_cb)
                .ignore_default(),
        );
        def.arg(ArgDef::new("model.acceleration.accel3d", "accel3d").onoff());
        def.arg(ArgDef::new("model.heads", "heads"));
        def.arg(ArgDef::new("model.ram", "ram"));
        def.arg(ArgDef::new("model.vram", "vram"));
        def.arg(ArgDef::new("model.vram64", "vram64"));
        def.arg(ArgDef::new("model.vgamem", "vgamem"));
        def.arg(ArgDef::new("model.blob", "blob"));
        def.alias("model.type", &["model"]);
        def.alias("model.heads", &["heads"]);
        def.alias("model.ram", &["ram"]);
        def.alias("model.vram", &["vram"]);
        def.alias("model.vram64", &["vram64"]);
        def.alias("model.vgamem", &["vgamem"]);
        def.alias("model.acceleration.accel3d", &["accel3d"]);
        def
    })
}
