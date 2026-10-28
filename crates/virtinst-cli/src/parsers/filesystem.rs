//! `--filesystem` and `--vsock`: `ParserFilesystem` and `ParserVsock`.

use std::sync::OnceLock;

use virtinst_core::devices::{filesystem, vsock};
use virtinst_core::xmlbuilder::{Value, XmlObject};

use crate::parser::{add_common_device_args, ArgDef, ArgInst, Error, ParseCtx, ParserDef};

/// `DeviceFilesystem.type`'s setter.
fn set_type_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    filesystem::set_type(inst, val)
}

/// `DeviceFilesystem.source`'s setter.
fn set_source_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    filesystem::set_source(inst, val)
}

/// `DeviceFilesystem.source`, compared the way `lookup_param` does.
fn source_lookup_cb(
    _ctx: &ParseCtx,
    inst: &XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<bool, Error> {
    let cur = filesystem::get_source(inst).map(|v| v.to_xml_string());
    Ok(cur == val.map(|v| v.to_xml_string()))
}

/// `ParserFilesystem`.
pub fn filesystem() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new(
            "filesystem",
            Some("devices.filesystem"),
            Some(&filesystem::DEVICE_FILESYSTEM),
        );
        def.remove_first = &["source", "target"];
        add_common_device_args(&mut def, false, true);

        def.arg(ArgDef::new("type", "_type_prop").cb(set_type_cb));
        def.arg(ArgDef::new("accessmode", "accessmode"));
        def.arg(ArgDef::new("model", "model"));
        def.arg(ArgDef::new("multidevs", "multidevs"));
        def.arg(ArgDef::new("readonly", "readonly").onoff());
        def.arg(ArgDef::new("space_hard_limit", "space_hard_limit"));
        def.arg(ArgDef::new("space_soft_limit", "space_soft_limit"));
        def.arg(ArgDef::new("fmode", "fmode"));
        def.arg(ArgDef::new("dmode", "dmode"));

        def.arg(
            ArgDef::new("source", "source_dir")
                .cb(set_source_cb)
                .lookup(source_lookup_cb),
        );
        def.arg(ArgDef::new("target", "target_dir"));

        for (cli, prop) in [
            ("source.file", "source_file"),
            ("source.dir", "source_dir"),
            ("source.dev", "source_dev"),
            ("source.name", "source_name"),
            ("source.pool", "source_pool"),
            ("source.volume", "source_volume"),
            ("source.units", "source_units"),
            ("source.usage", "source_usage"),
            ("source.socket", "source_socket"),
            ("target.dir", "target_dir"),
            ("binary.path", "binary_path"),
        ] {
            def.arg(ArgDef::new(cli, prop));
        }
        def.arg(ArgDef::new("binary.xattr", "binary_xattr").onoff());
        def.arg(ArgDef::new("binary.cache.mode", "binary_cache_mode"));
        def.arg(ArgDef::new("binary.lock.posix", "binary_lock_posix").onoff());
        def.arg(ArgDef::new("binary.lock.flock", "binary_lock_flock").onoff());
        def.arg(ArgDef::new("binary.sandbox.mode", "binary_sandbox_mode"));

        for (cli, prop) in [
            ("driver.format", "driver_format"),
            ("driver.name", "driver_name"),
            ("driver.queue", "driver_queue"),
            ("driver.type", "driver_type"),
            ("driver.wrpolicy", "driver_wrpolicy"),
        ] {
            def.arg(ArgDef::new(cli, prop));
        }

        def.alias("accessmode", &["mode"]);
        def.post_parse = Some(validate_post_parse);
        def
    })
}

/// `DeviceFilesystem.validate`, which `parse` runs on each new object.
fn validate_post_parse(ctx: &mut ParseCtx, inst: &mut XmlObject) -> Result<(), Error> {
    if ctx.editing {
        return Ok(());
    }
    let Some(target) = inst.get_str("target_dir").filter(|t| !t.is_empty()) else {
        return Ok(());
    };
    // `validate_target`: in case of qemu for default fs type (mount)
    // target is not actually a directory, it is merely a arbitrary
    // string tag that is exported to the guest as a hint for where to
    // mount
    let cs = crate::parser::conn_support();
    let typ = inst.get_str("_type_prop");
    if (cs.is_qemu || cs.is_test) && typ.as_deref().map_or(true, |t| t == filesystem::TYPE_MOUNT) {
        return Ok(());
    }
    if !target.starts_with('/') {
        return Err(format!(
            "Filesystem target '{target}' must be an absolute path"
        ));
    }
    Ok(())
}

/// `ParserVsock`.
pub fn vsock() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new("vsock", Some("devices.vsock"), Some(&vsock::DEVICE_VSOCK));
        def.remove_first = &["model"];
        def.stub_none = false;
        add_common_device_args(&mut def, false, true);

        def.arg(ArgDef::new("model", "model").ignore_default());
        def.arg(ArgDef::new("cid.auto", "auto_cid").onoff());
        def.arg(ArgDef::new("cid.address", "cid"));

        def.alias("cid.auto", &["auto_cid"]);
        def.alias("cid.address", &["cid"]);
        def
    })
}
