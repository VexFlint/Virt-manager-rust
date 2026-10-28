//! Boot and security options: `--boot`, `--idmap`, `--seclabel`,
//! `--keywrap` and `--launchSecurity` (`ParserBoot`, `ParserIdmap`,
//! `ParserSeclabel`, `ParserKeyWrap` and `ParserLaunchSecurity`).

use std::sync::OnceLock;

use virtinst_core::capabilities::caps::Capabilities;
use virtinst_core::capabilities::domcaps::DomainCapabilities;
use virtinst_core::domain::os::{self as dos, BOOT_DEVICES};
use virtinst_core::domain::{idmap, keywrap, launch_security, seclabel};
use virtinst_core::xmlbuilder::{Value, XmlObject};

use crate::parser::{
    conn_support, connect, fail, ArgDef, ArgInst, Error, OptDict, ParseCtx, ParserDef,
};

fn noset_cb(
    _ctx: &mut ParseCtx,
    _inst: &mut XmlObject,
    _val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    Ok(())
}

fn str_val(val: Option<Value>) -> Option<Value> {
    val.map(|v| Value::Str(v.to_xml_string()))
}

/// `ParserBoot._parse`'s back compat: `--boot uefi` with no value.
fn boot_pre_parse(_optstr: &str, o: &mut OptDict) -> Result<(), Error> {
    // Back compat to allow uefi to have no cli value specified
    if matches!(o.get("uefi"), Some(None)) {
        o.insert("uefi".into(), Some("on".into()));
    }
    Ok(())
}

/// `ParserBoot._convert_boot_order`.
fn convert_boot_order(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    o: &mut OptDict,
) -> Result<(), Error> {
    // Build boot order
    let mut boot_order: Vec<String> = vec![];
    let keys: Vec<String> = o.0.iter().map(|(k, _)| k.clone()).collect();
    for cliname in keys {
        if !BOOT_DEVICES.contains(&cliname.as_str()) {
            continue;
        }
        o.remove(&cliname);
        if !boot_order.contains(&cliname) {
            boot_order.push(cliname);
        }
    }
    if !boot_order.is_empty() {
        dos::set_bootorder(inst, &boot_order)?;
    }
    Ok(())
}

/// `guest.lookup_domcaps()` with `os` the guest's (taken out) `<os>`.
fn lookup_domcaps(guest: &XmlObject, os: &XmlObject) -> DomainCapabilities {
    os.sync();
    let info = super::guest_info(guest);
    connect()
        .and_then(|c| {
            DomainCapabilities::fetch_from(
                &c,
                None,
                os.get_str("arch").as_deref(),
                os.get_str("machine").as_deref(),
                info.domain_type.as_deref(),
            )
            .map_err(|e| log::debug!("Error fetching domcapabilities XML: {e}"))
            .ok()
        })
        .unwrap_or_else(|| {
            DomainCapabilities::parse("<domainCapabilities/>").expect("empty domcaps parses")
        })
}

/// `Guest.set_uefi_path`.
fn set_uefi_path(guest: &mut XmlObject, os: &mut XmlObject, path: String) -> Result<(), Error> {
    os.set("loader_ro", Some(Value::Bool(true)))?;
    os.set("loader_type", Some(Value::Str("pflash".into())))?;
    let secboot = path.contains("secboot");
    os.set("loader", Some(Value::Str(path)))?;

    // If the firmware name contains "secboot" it is probably build
    // with SMM feature required so we need to enable that feature,
    // otherwise the firmware may fail to load.  True secure boot is
    // currently supported only on x86 architecture and with q35 with
    // SMM feature enabled so change the machine to q35 as well.
    // To actually enforce the secure boot for the guest if Secure Boot
    // Mode is configured we need to enable loader secure feature.
    if dos::is_x86(os) && secboot {
        guest
            .child_mut("features")
            .set("smm", Some(Value::Bool(true)))?;
        os.set("loader_secure", Some(Value::Bool(true)))?;
        if !dos::is_q35(os) {
            log::warn!(
                "Changing machine type from '{}' to 'q35' which is required for UEFI secure boot.",
                os.get_str("machine").unwrap_or_else(|| "None".into())
            );
            os.set("machine", Some(Value::Str("q35".into())))?;
        }
    }
    Ok(())
}

/// `Guest.enable_uefi`, with `os` the guest's (taken out) `<os>`.
pub fn enable_uefi(guest: &mut XmlObject, os: &mut XmlObject) -> Result<(), Error> {
    let domcaps = lookup_domcaps(guest, os);
    if domcaps.supports_firmware_efi() {
        return os.set("firmware", Some(Value::Str("efi".into())));
    }

    // _lookup_default_uefi_path
    let arch = os.get_str("arch").unwrap_or_else(|| "None".into());
    if !domcaps.supports_uefi_loader() {
        return Err("Libvirt version does not support UEFI.".into());
    }
    if !domcaps.arch_can_uefi() {
        return Err(format!("Don't know how to setup UEFI for arch '{arch}'"));
    }
    let path = domcaps
        .find_uefi_path_for_arch(conn_support().is_bhyve)
        .ok_or_else(|| format!("Did not find any UEFI binary path for arch '{arch}'"))?;
    log::debug!("Setting default UEFI path={path}");
    set_uefi_path(guest, os, path)
}

/// `Guest.disable_uefi`.
fn disable_uefi(os: &mut XmlObject) -> Result<(), Error> {
    for name in [
        "firmware",
        "loader",
        "loader_ro",
        "loader_type",
        "loader_secure",
        "nvram",
        "nvram_template",
    ] {
        os.set(name, None)?;
    }
    while !os.children("firmware_features").is_empty() {
        os.remove_child("firmware_features", 0);
    }

    // Force remove any properties we don't know about
    for xpath in ["./firmware", "./nvram", "./loader"] {
        os.force_remove_path(xpath);
    }
    Ok(())
}

/// `ParserBoot.set_uefi_cb`.
fn set_uefi_cb(
    ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    let val = match val {
        None => None,
        Some(v) => {
            let s = v.to_xml_string();
            Some(
                crate::cli::raw_on_off_convert(&s)
                    .ok_or_else(|| fail("uefi must be 'yes' or 'no'"))?,
            )
        }
    };

    if !ctx.editing {
        // From virt-install, we just set this flag, and set_defaults()
        // will fill in everything for us, otherwise we have a circular
        // dep on determining arch/machine info
        ctx.stash.insert(
            "uefi_requested".into(),
            val.map(|v| if v { "on" } else { "off" }.to_string()),
        );
        Ok(())
    } else if val != Some(true) {
        disable_uefi(inst)
    } else {
        enable_uefi(ctx.guest, inst)
    }
}

/// `Guest.refresh_machine_type`.
fn refresh_machine_type(ctx: &mut ParseCtx, os: &mut XmlObject) -> Result<(), Error> {
    // We need to unset the machine type first, so we can perform
    // a successful capsinfo lookup, otherwise we will error when qemu
    // has deprecated and removed the old machine type
    let original_machine_type = os.get_str("machine").unwrap_or_default();
    os.set("machine", None)?;

    let caps = connect()
        .ok_or("no connection")
        .and_then(|c| Capabilities::fetch_from(&c).map_err(|_| "no connection"))?;
    let machines = caps.guest_lookup_machines(
        os.get_str("os_type").as_deref(),
        os.get_str("arch").as_deref(),
        ctx.guest.get_str("type").as_deref(),
        None,
    )?;
    let canonical_names = machines
        .iter()
        .filter(|m| m.canonical.is_some())
        .filter_map(|m| m.name.clone());

    for machine_alias in canonical_names {
        let prefix = match machine_alias.as_str() {
            "pc" => "pc-i440fx-".to_string(),
            "q35" => "pc-q35-".to_string(),
            // Example: pseries-X, virt-X, s390-ccw-virtio-X
            _ => format!("{machine_alias}-"),
        };
        if original_machine_type.starts_with(&prefix) {
            return os.set("machine", Some(Value::Str(machine_alias)));
        }
    }
    Err(format!(
        "Don't know how to refresh machine type '{original_machine_type}'"
    ))
}

/// `ParserBoot.refresh_machine_type_cb`.
fn refresh_machine_type_cb(
    ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    if val == Some(Value::Bool(true)) {
        refresh_machine_type(ctx, inst)?;
    }
    Ok(())
}

/// `ParserBoot.set_initargs_cb`.
fn set_initargs_cb(
    _ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    let val = val.map(|v| v.to_xml_string()).unwrap_or_default();
    dos::set_initargs_string(inst, &val)
}

/// The `ParserBoot` callbacks that set a plain guest property.
macro_rules! guest_prop_cb {
    ($name:ident, $prop:literal) => {
        fn $name(
            ctx: &mut ParseCtx,
            _inst: &mut XmlObject,
            val: Option<Value>,
            _arg: &ArgInst,
        ) -> Result<(), Error> {
            ctx.guest.set($prop, str_val(val))
        }
    };
}
guest_prop_cb!(set_bootloader_cb, "bootloader");
guest_prop_cb!(set_bootloader_args_cb, "bootloader_args");
guest_prop_cb!(set_domain_type_cb, "type");
guest_prop_cb!(set_emulator_cb, "emulator");

/// `ParserBoot`.
pub fn boot() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new("boot", Some("os"), Some(&dos::DOMAIN_OS));
        def.pre_parse = Some(boot_pre_parse);
        def.inst_pre_parse = Some(convert_boot_order);

        // This is simply so the boot options are advertised with --boot help,
        // actual processing is handled by _parse
        for dev in BOOT_DEVICES {
            def.arg(ArgDef::cb_only(dev, noset_cb).no_lookup());
        }

        def.arg(
            ArgDef::cb_only("refresh-machine-type", refresh_machine_type_cb)
                .no_lookup()
                .onoff(),
        );

        // UEFI depends on these bits, so set them first
        def.arg(ArgDef::new("os_type", "os_type"));
        def.arg(ArgDef::new("arch", "arch"));
        def.arg(ArgDef::new("machine", "machine"));
        def.arg(ArgDef::cb_only("bootloader", set_bootloader_cb).no_lookup());
        def.arg(ArgDef::cb_only("bootloader_args", set_bootloader_args_cb).no_lookup());
        def.arg(ArgDef::cb_only("domain_type", set_domain_type_cb).no_lookup());
        def.arg(ArgDef::cb_only("emulator", set_emulator_cb).no_lookup());
        def.arg(ArgDef::cb_only("uefi", set_uefi_cb).no_lookup());

        // Common/Shared boot options
        def.arg(ArgDef::new("loader", "loader"));
        def.arg(ArgDef::new("loader.readonly", "loader_ro").onoff());
        def.arg(ArgDef::new("loader.type", "loader_type"));
        def.arg(ArgDef::new("loader.secure", "loader_secure").onoff());
        def.arg(ArgDef::new("loader.stateless", "loader_stateless").onoff());

        // Guest-Based bootloader options
        def.arg(ArgDef::new("firmware", "firmware"));
        let ff = ("feature", "firmware_features");
        def.arg(
            ArgDef::new("firmware.feature[0-9]*.enabled", "enabled")
                .find_inst(ff.0, ff.1)
                .onoff(),
        );
        def.arg(ArgDef::new("firmware.feature[0-9]*.name", "name").find_inst(ff.0, ff.1));
        def.arg(ArgDef::new("nvram", "nvram"));
        def.arg(ArgDef::new("nvram.template", "nvram_template"));
        def.arg(ArgDef::new("boot[0-9]*.dev", "dev").find_inst("boot", "bootdevs"));
        def.arg(ArgDef::new("bootmenu.enable", "bootmenu_enable").onoff());
        def.arg(ArgDef::new("bootmenu.timeout", "bootmenu_timeout"));
        def.arg(ArgDef::new("bios.useserial", "bios_useserial").onoff());
        def.arg(ArgDef::new("bios.rebootTimeout", "bios_rebootTimeout"));
        def.arg(ArgDef::new("smbios.mode", "smbios_mode"));
        def.arg(ArgDef::new("nvram.templateFormat", "nvram_templateFormat"));

        // Direct kernel boot options
        def.arg(ArgDef::new("kernel", "kernel"));
        def.arg(ArgDef::new("initrd", "initrd"));
        def.arg(ArgDef::new("cmdline", "kernel_args").can_comma());
        def.arg(ArgDef::new("dtb", "dtb"));
        def.arg(ArgDef::new("acpi.table", "acpi_tb"));
        def.arg(ArgDef::new("acpi.table.type", "acpi_tb_type"));

        // Container boot options
        def.arg(ArgDef::new("init", "init"));
        def.arg(ArgDef::new("initargs", "initargs").cb(set_initargs_cb));
        def.arg(ArgDef::new("initarg[0-9]*", "val").find_inst("initarg", "initargs"));
        def.arg(ArgDef::new("initenv[0-9]*", "value").find_inst("initenv", "initenvs"));
        def.arg(ArgDef::new("initenv[0-9]*.name", "name").find_inst("initenv", "initenvs"));
        def.arg(ArgDef::new("initdir", "initdir"));
        def.arg(ArgDef::new("inituser", "inituser"));
        def.arg(ArgDef::new("initgroup", "initgroup"));

        def.alias("bios.rebootTimeout", &["rebootTimeout"]);
        def.alias("bios.useserial", &["useserial"]);
        def.alias("bootmenu.enable", &["menu"]);
        def.alias("cmdline", &["extra_args", "kernel_args"]);
        def.alias("loader.readonly", &["loader_ro"]);
        def.alias("loader.type", &["loader_type"]);
        def.alias("loader.secure", &["loader_secure"]);
        def.alias("nvram.template", &["nvram_template"]);
        def.alias("smbios.mode", &["smbios_mode"]);
        def
    })
}

/// `ParserIdmap`.
pub fn idmap() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new("idmap", Some("idmap"), Some(&idmap::DOMAIN_IDMAP));
        for (cli, prop) in [
            ("uid.start", "uid_start"),
            ("uid.target", "uid_target"),
            ("uid.count", "uid_count"),
            ("gid.start", "gid_start"),
            ("gid.target", "gid_target"),
            ("gid.count", "gid_count"),
        ] {
            def.arg(ArgDef::new(cli, prop));
        }
        for (cli, alias) in [
            ("uid.start", "uid_start"),
            ("uid.target", "uid_target"),
            ("uid.count", "uid_count"),
            ("gid.start", "gid_start"),
            ("gid.target", "gid_target"),
            ("gid.count", "gid_count"),
        ] {
            def.alias(cli, &[alias]);
        }
        def
    })
}

/// `ParserSeclabel`.
pub fn seclabel() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new(
            "seclabel",
            Some("seclabels"),
            Some(&seclabel::DOMAIN_SECLABEL),
        );
        def.arg(ArgDef::new("type", "type"));
        def.arg(ArgDef::new("model", "model"));
        def.arg(ArgDef::new("relabel", "relabel").onoff());
        def.arg(ArgDef::new("label", "label").can_comma());
        def.arg(ArgDef::new("baselabel", "baselabel").can_comma());
        def
    })
}

/// `ParserKeyWrap`.
pub fn keywrap() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new("keywrap", Some("keywrap"), Some(&keywrap::DOMAIN_KEYWRAP));
        def.arg(
            ArgDef::new("cipher[0-9]*.name", "name")
                .can_comma()
                .find_inst("cipher", "cipher"),
        );
        def.arg(
            ArgDef::new("cipher[0-9]*.state", "state")
                .can_comma()
                .find_inst("cipher", "cipher"),
        );
        def
    })
}

/// `ParserLaunchSecurity`.
pub fn launch_security() -> &'static ParserDef {
    static DEF: OnceLock<ParserDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ParserDef::new(
            "launchSecurity",
            Some("launchSecurity"),
            Some(&launch_security::DOMAIN_LAUNCH_SECURITY),
        );
        def.remove_first = &["type"];
        for name in [
            "type",
            "cbitpos",
            "reducedPhysBits",
            "policy",
            "session",
            "dhCert",
            "guestVisibleWorkarounds",
            "idBlock",
            "idAuth",
            "hostData",
        ] {
            def.arg(ArgDef::new(name, name));
        }
        for name in ["kernelHashes", "authorKey", "vcek"] {
            def.arg(ArgDef::new(name, name).onoff());
        }
        for name in ["mrConfigId", "mrOwner", "mrOwnerConfig"] {
            def.arg(ArgDef::new(name, name));
        }
        def.arg(ArgDef::new("quoteGenerationService", "quoteGenerationService").onoff());
        def.arg(ArgDef::new(
            "quoteGenerationSocket",
            "quoteGenerationSocket",
        ));
        def
    })
}
