//! `--disk` (`ParserDisk`).
//!
//! Storage creation (`size=`, `pool=`, `format=`, `backing_store=`, and
//! new paths that don't exist yet) is not ported yet: those options
//! fail explicitly instead of defining a guest whose disk is missing.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

use virtinst_core::devices::disk as dd;
use virtinst_core::diskbackend::StorageBackend;
use virtinst_core::xmlbuilder::{Value, XmlObject};

use super::charsrc::add_device_seclabel_args;
use crate::parser::{
    add_boot_order_arg, add_common_device_args, conn_support, connect, fail, ArgDef, ArgInst,
    Error, ParseCtx, ParserDef,
};

const STORAGE_NOT_PORTED: &str = "Storage creation for --disk is not implemented yet";

fn noset_cb(
    _ctx: &mut ParseCtx,
    _inst: &mut XmlObject,
    _val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    Ok(())
}

fn conn() -> Result<virt::connect::Connect, Error> {
    connect().ok_or_else(|| "No libvirt connection".to_string())
}

thread_local! {
    /// The disk's `_storage_backend` once this parse looked its path
    /// up (`set_source_path`, `set_vol_object`). `None` is upstream's
    /// `StorageBackendStub`: the source path the disk was parsed with.
    static BACKEND: RefCell<Option<Rc<StorageBackend>>> = const { RefCell::new(None) };
}

/// The storage the last parsed `--disk` looked up, for its
/// `set_defaults` (`None`: a stub).
pub fn backend() -> Option<Rc<StorageBackend>> {
    BACKEND.with(|b| b.borrow().clone())
}

fn set_backend(b: Option<StorageBackend>) {
    BACKEND.with(|cur| *cur.borrow_mut() = b.map(Rc::new));
}

/// `get_source_path()`. A stub knows the path the disk was parsed
/// with (none for a new disk); without one, the `<source>` is resolved
/// into the disk's backend (`_resolve_storage_backend`).
fn source_path(inst: &XmlObject, editing: bool) -> Result<Option<String>, Error> {
    if let Some(b) = backend() {
        return Ok(b.get_path());
    }
    if editing {
        if let Some(p) = dd::xmlpath(inst) {
            return Ok(Some(p));
        }
    }
    let b = dd::resolve_backend(inst, &conn()?, conn_support().is_remote);
    let path = b.get_path();
    set_backend(Some(b));
    Ok(path)
}

/// `ParserDisk.set_path_cb`.
fn set_path_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    let path = val.map(|v| v.to_xml_string()).filter(|p| !p.is_empty());
    let b = dd::set_source_path_obj(inst, path.as_deref(), &conn()?, conn_support().is_remote)?;
    set_backend(Some(b));
    Ok(())
}

/// `ParserDisk.path_lookup_cb`.
fn path_lookup_cb(
    _ctx: &ParseCtx,
    inst: &XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<bool, Error> {
    // Lookups run on parsed disks: a stub's path, else the resolved one.
    let path = match dd::xmlpath(inst) {
        Some(p) => Some(p),
        None => dd::resolve_backend(inst, &conn()?, conn_support().is_remote).get_path(),
    };
    Ok(path == val.map(|v| v.to_xml_string()))
}

/// The `type=` option: `inst.type = disktype`, set up front.
fn set_type_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    match val.map(|v| v.to_xml_string()).filter(|t| !t.is_empty()) {
        Some(t) => inst.set("_xmltype", Some(Value::Str(t))),
        None => Ok(()),
    }
}

/// `ParserDisk._parse`, before the options are applied.
fn disk_parse(ctx: &mut ParseCtx) -> Result<Option<Vec<XmlObject>>, Error> {
    set_backend(None);
    // These are essentially aliases for new style options, but we still
    // want to advertise them in --disk=help output because they are
    // historically commonly used. This should rarely, if ever, be extended
    let o = &mut ctx.optdict;
    if let Some(v) = o.remove("bus") {
        o.insert("target.bus".into(), v);
    }
    if let Some(v) = o.remove("cache") {
        o.insert("driver.cache".into(), v);
    }

    if ctx.optstr == "none" {
        return Ok(Some(vec![]));
    }

    // Popped here, used once the options are applied.
    for key in [
        "backing_store",
        "backing_format",
        "pool",
        "vol",
        "size",
        "format",
        "sparse",
    ] {
        if let Some(v) = ctx.optdict.remove(key) {
            ctx.stash.insert(key.into(), v);
        }
    }
    // `parse_size`, `_on_off_convert("sparse")`: checked in pop order.
    if let Some(Some(s)) = ctx.stash.get("size") {
        if s.trim().parse::<f64>().is_err() {
            return Err(fail(format!(
                "Improper value for 'size': could not convert string to float: '{s}'"
            )));
        }
    }
    if let Some(Some(s)) = ctx.stash.get("sparse") {
        if crate::cli::raw_on_off_convert(s).is_none() {
            return Err(fail("sparse must be 'yes' or 'no'"));
        }
    }
    // `convert_perms`
    match ctx.optdict.remove("perms").flatten().as_deref() {
        None | Some("rw") => {}
        Some("ro") => {
            ctx.optdict.insert("readonly".into(), Some("on".into()));
        }
        Some("sh") => {
            ctx.optdict.insert("shareable".into(), Some("on".into()));
        }
        Some(val) => return Err(fail(format!("Unknown 'perms' value '{val}'"))),
    }
    if let Some(Some(v)) = ctx.stash.get("vol") {
        if !v.is_empty() && v.matches('/').count() != 1 {
            return Err("Storage volume must be specified as vol=poolname/volname".into());
        }
    }
    Ok(None)
}

/// `ParserDisk._parse`, once the options are applied.
fn disk_post_parse(ctx: &mut ParseCtx, inst: &mut XmlObject) -> Result<(), Error> {
    let stash = |k: &str| ctx.stash.get(k).cloned().flatten();
    let backing_store = stash("backing_store").filter(|s| !s.is_empty());
    let mut poolname = stash("pool").filter(|s| !s.is_empty());
    let mut volname = stash("vol").filter(|s| !s.is_empty());
    let size = stash("size").and_then(|s| s.trim().parse::<f64>().ok());
    let fmt = stash("format").filter(|s| !s.is_empty());
    let sparse = stash("sparse").map_or(true, |s| crate::cli::raw_on_off_convert(&s) == Some(true));

    if let Some((p, v)) = volname.clone().as_deref().and_then(|v| v.split_once('/')) {
        log::debug!("Parsed --disk volume as: pool={p} vol={v}");
        poolname = Some(p.to_string());
        volname = Some(v.to_string());
    }

    let has_size = size.is_some_and(|s| s != 0.0);
    if has_size
        && volname.is_none()
        && poolname.is_none()
        && source_path(inst, ctx.editing)?.is_none()
        && dd::disk_type(inst, None) == "file"
    {
        // Saw something like --disk size=X, have it imply pool=default
        poolname = Some("default".into());
    }

    // Generate and fill in the disk source info
    let conn = conn()?;
    let mut pool = None;
    if let Some(name) = &poolname {
        if name == "default" {
            // `StoragePool.build_default_pool` creates the pool when
            // it's missing: storage creation.
            return Err(STORAGE_NOT_PORTED.into());
        }
        let p = virt::storage_pool::StoragePool::lookup_by_name(&conn, name)
            .map_err(|e| e.message().to_string())?;
        // `StoragePool.ensure_pool_is_running`
        if !p.is_active().unwrap_or(true) {
            log::debug!("starting pool={name}");
            p.create(0).map_err(|e| e.message().to_string())?;
        }
        pool = Some(p);
    }

    if let (Some(name), Some(p)) = (&volname, pool.take()) {
        let vol = virt::storage_vol::StorageVol::lookup_by_name(&p, name)
            .map_err(|e| e.message().to_string())?;
        // `set_vol_object`
        let backend = StorageBackend::new(
            None,
            Some(vol),
            virtinst_core::diskbackend::pool_of(&p),
            conn_support().is_remote,
        );
        dd::set_xmlpath(inst, backend.get_path(), &backend)?;
        set_backend(Some(backend));
    }

    let wants_creation = backend().is_some_and(|b| !b.exists());
    if (pool.is_some() || wants_creation)
        && (fmt.is_some() || has_size || sparse || backing_store.is_some())
    {
        // `DeviceDisk.build_vol_install` + `set_vol_install`.
        return Err(STORAGE_NOT_PORTED.into());
    }

    if !ctx.editing {
        validate(inst)?;
    }
    Ok(())
}

/// `DeviceDisk.validate`.
fn validate(inst: &XmlObject) -> Result<(), Error> {
    let path = source_path(inst, false)?;
    let b = backend();
    let Some(path) = path else {
        let device = dd::disk_device(inst);
        let can_be_empty = device == "floppy"
            || device == "cdrom"
            || !matches!(
                dd::disk_type(inst, b.as_deref()).as_str(),
                "file" | "block" | "dir" | "volume" | "network"
            );
        if !can_be_empty {
            return Err(format!("Device type '{device}' requires a path"));
        }
        return Ok(());
    };
    // A stub always exists; storage creation was refused above.
    if b.is_some_and(|b| !b.exists()) {
        return Err(format!(
            "Must specify storage creation parameters for non-existent path '{path}'."
        ));
    }
    Ok(())
}

/// `ParserDisk`.
pub fn disk() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new("disk", Some("devices.disk"), Some(&dd::DEVICE_DISK));
        def.remove_first = &["path"];
        def.stub_none = false;
        def.custom_parse = Some(disk_parse);
        def.post_parse = Some(disk_post_parse);
        add_common_device_args(&mut def, false, false);
        add_boot_order_arg(&mut def);
        def.arg(ArgDef::new("boot.loadparm", "boot.loadparm"));
        def.arg(ArgDef::new("driver.ats", "virtio_driver.ats").onoff());
        def.arg(ArgDef::new("driver.iommu", "virtio_driver.iommu").onoff());
        def.arg(ArgDef::new("driver.packed", "virtio_driver.packed").onoff());
        def.arg(ArgDef::new("driver.page_per_vq", "virtio_driver.page_per_vq").onoff());

        // These are all handled specially in _parse
        for name in [
            "backing_store",
            "backing_format",
            "pool",
            "vol",
            "size",
            "format",
            "sparse",
        ] {
            def.arg(ArgDef::cb_only(name, noset_cb));
        }
        def.arg(ArgDef::cb_only("type", set_type_cb));

        // These are handled in _add_advertised_aliases
        def.arg(ArgDef::new("bus", "bus").cb(noset_cb));
        def.arg(ArgDef::new("cache", "driver_cache").cb(noset_cb));

        // More standard XML props
        for (cli, prop) in [
            ("source.dir", "source.dir"),
            ("source.file", "source.file"),
            ("source.dev", "source.dev"),
            ("source.pool", "source.pool"),
            ("source.volume", "source.volume"),
            ("source.name", "source.name"),
            ("source.protocol", "source.protocol"),
            ("source.startupPolicy", "startup_policy"),
            // type=nvme source props
            ("source.type", "source.type"),
            ("source.namespace", "source.namespace"),
        ] {
            def.arg(ArgDef::new(cli, prop));
        }
        def.arg(ArgDef::new("source.managed", "source.managed").onoff());
        for (cli, prop) in [
            ("source.address.domain", "source.address.domain"),
            ("source.address.bus", "source.address.bus"),
            ("source.address.slot", "source.address.slot"),
            ("source.address.function", "source.address.function"),
        ] {
            def.arg(ArgDef::new(cli, prop));
        }

        // `host_find_inst_cb` searches for `hosts(\d+)`, so the index
        // in `source.host1.name` is never seen: always the first host.
        for (cli, prop) in [
            ("source.host[0-9]*.name", "name"),
            ("source.host[0-9]*.port", "port"),
            ("source.host[0-9]*.socket", "socket"),
            ("source.host[0-9]*.transport", "transport"),
        ] {
            def.arg(ArgDef::new(cli, prop).find_inst("hosts", "source.hosts"));
        }

        add_device_seclabel_args(&mut def, "seclabels", "");

        def.arg(
            ArgDef {
                propname: None,
                cb: Some(set_path_cb),
                ..ArgDef::new("path", "")
            }
            .lookup(path_lookup_cb),
        );
        for (cli, prop) in [
            ("device", "_device"),
            ("snapshot", "snapshot_policy"),
            ("sgio", "sgio"),
            ("rawio", "rawio"),
            ("serial", "serial"),
            ("wwn", "wwn"),
        ] {
            def.arg(ArgDef::new(cli, prop));
        }
        for (cli, prop) in [
            ("readonly", "read_only"),
            ("shareable", "shareable"),
            ("transient", "transient"),
            ("transient.shareBacking", "transient_shareBacking"),
        ] {
            def.arg(ArgDef::new(cli, prop).onoff());
        }

        def.arg(ArgDef::new("target.bus", "bus"));
        def.arg(ArgDef::new("target.removable", "removable").onoff());
        def.arg(ArgDef::new("target.dev", "target"));
        def.arg(ArgDef::new("target.rotation_rate", "rotation_rate"));

        for (cli, prop) in [
            ("driver.cache", "driver_cache"),
            ("driver.discard", "driver_discard"),
            ("driver.detect_zeroes", "driver_detect_zeroes"),
            ("driver.name", "driver_name"),
            ("driver.type", "driver_type"),
        ] {
            def.arg(ArgDef::new(cli, prop));
        }
        def.arg(ArgDef::new("driver.copy_on_read", "driver_copy_on_read").onoff());
        for (cli, prop) in [
            ("driver.io", "driver_io"),
            ("driver.iothread", "driver_iothread"),
            ("driver.queues", "driver_queues"),
            ("driver.error_policy", "error_policy"),
        ] {
            def.arg(ArgDef::new(cli, prop));
        }
        def.arg(ArgDef::new("driver.discard_no_unref", "driver_discard_no_unref").onoff());
        for (cli, prop) in [
            ("driver.queue_size", "driver_queue_size"),
            (
                "driver.metadata_cache.max_size",
                "driver_metadata_cache_max_size",
            ),
            (
                "driver.metadata_cache.max_size.unit",
                "driver_metadata_cache_max_size_unit",
            ),
            ("iotune.read_bytes_sec", "iotune_rbs"),
            ("iotune.write_bytes_sec", "iotune_wbs"),
            ("iotune.total_bytes_sec", "iotune_tbs"),
            ("iotune.read_iops_sec", "iotune_ris"),
            ("iotune.write_iops_sec", "iotune_wis"),
            ("iotune.total_iops_sec", "iotune_tis"),
            ("blockio.logical_block_size", "logical_block_size"),
            ("blockio.physical_block_size", "physical_block_size"),
            ("blockio.discard_granularity", "discard_granularity"),
            ("geometry.cyls", "geometry_cyls"),
            ("geometry.heads", "geometry_heads"),
            ("geometry.secs", "geometry_secs"),
            ("geometry.trans", "geometry_trans"),
            ("source.reservations.managed", "reservations_managed"),
            (
                "source.reservations.source.type",
                "reservations_source_type",
            ),
            (
                "source.reservations.source.path",
                "reservations_source_path",
            ),
            (
                "source.reservations.source.mode",
                "reservations_source_mode",
            ),
        ] {
            def.arg(ArgDef::new(cli, prop));
        }

        for (cli, alias) in [
            ("blockio.logical_block_size", "logical_block_size"),
            ("blockio.physical_block_size", "physical_block_size"),
            ("iotune.read_bytes_sec", "read_bytes_sec"),
            ("iotune.write_bytes_sec", "write_bytes_sec"),
            ("iotune.total_bytes_sec", "total_bytes_sec"),
            ("iotune.read_iops_sec", "read_iops_sec"),
            ("iotune.write_iops_sec", "write_iops_sec"),
            ("iotune.total_iops_sec", "total_iops_sec"),
            ("source.pool", "source_pool"),
            ("source.volume", "source_volume"),
            ("source.name", "source_name"),
            ("source.protocol", "source_protocol"),
            ("source.host[0-9]*.name", "source_host_name"),
            ("source.host[0-9]*.port", "source_host_port"),
            ("source.host[0-9]*.socket", "source_host_socket"),
            ("source.host[0-9]*.transport", "source_host_transport"),
            ("source.startupPolicy", "startup_policy"),
            ("source.seclabel[0-9]*.model", "seclabel[0-9]*.model"),
            ("source.seclabel[0-9]*.relabel", "seclabel[0-9]*.relabel"),
            ("source.seclabel[0-9]*.label", "seclabel[0-9]*.label"),
            ("source.reservations.managed", "reservations.managed"),
            (
                "source.reservations.source.type",
                "reservations.source.type",
            ),
            (
                "source.reservations.source.path",
                "reservations.source.path",
            ),
            (
                "source.reservations.source.mode",
                "reservations.source.mode",
            ),
            ("snapshot", "snapshot_policy"),
            ("target.dev", "target"),
            ("target.removable", "removable"),
            ("target.rotation_rate", "rotation_rate"),
            ("driver.discard", "discard"),
            ("driver.detect_zeroes", "detect_zeroes"),
            ("driver.error_policy", "error_policy"),
            ("driver.io", "io"),
            ("driver.name", "driver_name"),
            ("driver.type", "driver_type"),
        ] {
            def.alias(cli, &[alias]);
        }
        def
    })
}
