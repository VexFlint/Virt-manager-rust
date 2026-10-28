//! `<os>` — upstream's `domain/os.py`, boot-device/firmware/direct-
//! kernel-boot/container-init configuration. Four small child types
//! plus a wide flat field set on `DomainOs` itself; no new macro
//! capability needed — everything here is a self/path attribute, a
//! self/path text field, or a plain `#[xml(list)]`.
//!
//! **Not modeled**, all business logic over the bound fields rather
//! than XML binding:
//! - The dozen `is_hvm()`/`is_x86()`/`is_arm64()`/... predicate
//!   methods — pure computation over `os_type`/`arch`/`machine`, no
//!   XML of their own. `NewDevice`/wizard code that needs these ports
//!   them as free functions or methods on `Guest` once something
//!   actually calls them, same as every other upstream method this
//!   crate leaves opaque until needed.
//! - `bootorder`, upstream's computed get/set `property` pairing
//!   `remove_child`/`add_new` calls over `bootdevs` — `bootdevs` itself
//!   (the real list of `<boot dev="..."/>` elements) is bound below;
//!   the ordering helper is a convenience wrapper this crate doesn't
//!   need to duplicate to have the same underlying data.
//! - `do_abspath=True` on `nvram`/`kernel`/`initrd`/`dtb`/`acpi_tb` —
//!   upstream resolves relative paths against the connection's URI on
//!   *write*; doesn't change what a *read* value means, so the field
//!   binds as a plain string here, same as every path-shaped field
//!   elsewhere in this crate. Path resolution is connection-layer
//!   logic (ticket 15), not something the typed struct does itself.
//! - `set_defaults` (container `init` defaulting) — ticket 17
//!   territory, same as everywhere else.

use virtinst_xml::XmlBound;

/// `<os><boot dev="..."/>` — upstream's `_BootDevice`.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "boot")]
pub struct OsBootDevice {
    #[xml(attribute = "dev")]
    pub dev: Option<String>,
}

/// `<os><firmware><feature enabled="yes" name="..."/></firmware>` —
/// upstream's `_FirmwareFeature`.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "feature")]
pub struct OsFirmwareFeature {
    #[xml(attribute = "enabled")]
    pub enabled: Option<bool>,

    #[xml(attribute = "name")]
    pub name: Option<String>,
}

/// `<os><initarg>value</initarg></os>` (container direct-boot argv) —
/// upstream's `_InitArg`, `val = XMLProperty(".")` (self text, same
/// shape `smartcard.rs`'s `Certificate` already proved).
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "initarg")]
pub struct OsInitArg {
    #[xml(text)]
    pub val: Option<String>,
}

/// `<os><initenv name="...">value</initenv></os>` (container env vars)
/// — upstream's `_InitEnv`.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "initenv")]
pub struct OsInitEnv {
    #[xml(attribute = "name")]
    pub name: Option<String>,

    #[xml(text)]
    pub value: Option<String>,
}

/// ```xml
/// <os firmware="efi">
///   <type arch="x86_64" machine="q35">hvm</type>
///   <loader readonly="yes" type="pflash" secure="no">/path/to/OVMF_CODE.fd</loader>
///   <nvram template="/path/to/OVMF_VARS.fd"/>
///   <boot dev="hd"/>
///   <boot dev="cdrom"/>
///   <bootmenu enable="yes" timeout="3000"/>
///   <smbios mode="sysinfo"/>
/// </os>
/// ```
/// or, direct kernel boot:
/// ```xml
/// <os>
///   <type>hvm</type>
///   <kernel>/boot/vmlinuz</kernel>
///   <initrd>/boot/initrd.img</initrd>
///   <cmdline>console=ttyS0</cmdline>
/// </os>
/// ```
/// or, container init:
/// ```xml
/// <os>
///   <type>exe</type>
///   <init>/bin/sh</init>
///   <initarg>-c</initarg>
///   <initenv name="TERM">xterm</initenv>
/// </os>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "os")]
pub struct DomainOs {
    // Shared/generic boot options
    #[xml(path = "type", text)]
    pub os_type: Option<String>,

    #[xml(path = "type", attribute = "arch")]
    pub arch: Option<String>,

    #[xml(path = "type", attribute = "machine")]
    pub machine: Option<String>,

    #[xml(path = "loader", text)]
    pub loader: Option<String>,

    #[xml(path = "loader", attribute = "readonly")]
    pub loader_ro: Option<bool>,

    #[xml(path = "loader", attribute = "type")]
    pub loader_type: Option<String>,

    #[xml(path = "loader", attribute = "secure")]
    pub loader_secure: Option<bool>,

    #[xml(path = "loader", attribute = "stateless")]
    pub loader_stateless: Option<bool>,

    // BIOS bootloader options
    #[xml(list)]
    pub bootdevs: Vec<OsBootDevice>,

    #[xml(attribute = "firmware")]
    pub firmware: Option<String>,

    #[xml(path = "firmware", list)]
    pub firmware_features: Vec<OsFirmwareFeature>,

    #[xml(path = "nvram", text)]
    pub nvram: Option<String>,

    #[xml(path = "nvram", attribute = "template")]
    pub nvram_template: Option<String>,

    #[xml(path = "nvram", attribute = "templateFormat")]
    pub nvram_template_format: Option<String>,

    #[xml(path = "bootmenu", attribute = "enable")]
    pub bootmenu_enable: Option<bool>,

    #[xml(path = "bootmenu", attribute = "timeout")]
    pub bootmenu_timeout: Option<u32>,

    #[xml(path = "bios", attribute = "useserial")]
    pub bios_useserial: Option<bool>,

    #[xml(path = "bios", attribute = "rebootTimeout")]
    pub bios_reboot_timeout: Option<u32>,

    #[xml(path = "smbios", attribute = "mode")]
    pub smbios_mode: Option<String>,

    // Direct kernel boot options
    #[xml(path = "kernel", text)]
    pub kernel: Option<String>,

    #[xml(path = "initrd", text)]
    pub initrd: Option<String>,

    #[xml(path = "cmdline", text)]
    pub kernel_args: Option<String>,

    #[xml(path = "dtb", text)]
    pub dtb: Option<String>,

    #[xml(path = "acpi/table", text)]
    pub acpi_tb: Option<String>,

    #[xml(path = "acpi/table", attribute = "type")]
    pub acpi_tb_type: Option<String>,

    // Container boot options
    #[xml(path = "init", text)]
    pub init: Option<String>,

    #[xml(list)]
    pub initargs: Vec<OsInitArg>,

    #[xml(list)]
    pub initenvs: Vec<OsInitEnv>,

    #[xml(path = "initdir", text)]
    pub initdir: Option<String>,

    #[xml(path = "inituser", text)]
    pub inituser: Option<String>,

    #[xml(path = "initgroup", text)]
    pub initgroup: Option<String>,
}

use crate::xmlbuilder::{child, prop, Kind, XmlClass, XmlObject};

/// `_InitArg`.
pub static INIT_ARG: XmlClass = XmlClass {
    xml_name: "initarg",
    props: &[prop("val", ".", Kind::Str)],
    children: &[],
    prop_order: &[],
};

/// `_InitEnv`.
pub static INIT_ENV: XmlClass = XmlClass {
    xml_name: "initenv",
    props: &[
        prop("name", "./@name", Kind::Str),
        prop("value", ".", Kind::Str),
    ],
    children: &[],
    prop_order: &[],
};

/// `_BootDevice`.
pub static BOOT_DEVICE: XmlClass = XmlClass {
    xml_name: "boot",
    props: &[prop("dev", "./@dev", Kind::Str)],
    children: &[],
    prop_order: &[],
};

/// `_FirmwareFeature`.
pub static FIRMWARE_FEATURE: XmlClass = XmlClass {
    xml_name: "feature",
    props: &[
        prop("enabled", "./@enabled", Kind::YesNo),
        prop("name", "./@name", Kind::Str),
    ],
    children: &[],
    prop_order: &["enabled", "name"],
};

/// `DomainOs.BOOT_DEVICES`.
pub const BOOT_DEVICES: [&str; 4] = ["hd", "cdrom", "fd", "network"];

/// `DomainOs`.
pub static DOMAIN_OS: XmlClass = XmlClass {
    xml_name: "os",
    props: &[
        // Shared/Generic boot options
        prop("os_type", "./type", Kind::Str),
        prop("arch", "./type/@arch", Kind::Str),
        prop("machine", "./type/@machine", Kind::Str),
        prop("loader", "./loader", Kind::Str),
        prop("loader_ro", "./loader/@readonly", Kind::YesNo),
        prop("loader_type", "./loader/@type", Kind::Str),
        prop("loader_secure", "./loader/@secure", Kind::YesNo),
        prop("loader_stateless", "./loader/@stateless", Kind::YesNo),
        // BIOS bootloader options
        prop("firmware", "./@firmware", Kind::Str),
        prop("nvram", "./nvram", Kind::AbsPath),
        prop("nvram_template", "./nvram/@template", Kind::Str),
        prop("bootmenu_enable", "./bootmenu/@enable", Kind::YesNo),
        prop("bootmenu_timeout", "./bootmenu/@timeout", Kind::Int),
        prop("bios_useserial", "./bios/@useserial", Kind::YesNo),
        prop("bios_rebootTimeout", "./bios/@rebootTimeout", Kind::Int),
        prop("smbios_mode", "./smbios/@mode", Kind::Str),
        prop("nvram_templateFormat", "./nvram/@templateFormat", Kind::Str),
        // Direct kernel boot options
        prop("kernel", "./kernel", Kind::AbsPath),
        prop("initrd", "./initrd", Kind::AbsPath),
        prop("kernel_args", "./cmdline", Kind::Str),
        prop("dtb", "./dtb", Kind::AbsPath),
        prop("acpi_tb", "./acpi/table", Kind::AbsPath),
        prop("acpi_tb_type", "./acpi/table/@type", Kind::Str),
        // Container boot options
        prop("init", "./init", Kind::Str),
        prop("initdir", "./initdir", Kind::Str),
        prop("inituser", "./inituser", Kind::Str),
        prop("initgroup", "./initgroup", Kind::Str),
    ],
    children: &[
        child("bootdevs", &BOOT_DEVICE, ".", false),
        child("firmware_features", &FIRMWARE_FEATURE, "./firmware", false),
        child("initargs", &INIT_ARG, ".", false),
        child("initenvs", &INIT_ENV, ".", false),
    ],
    prop_order: &[
        "firmware",
        "os_type",
        "arch",
        "machine",
        "firmware_features",
        "loader",
        "loader_ro",
        "loader_secure",
        "loader_type",
        "loader_stateless",
        "nvram",
        "nvram_template",
        "init",
        "initargs",
        "initenvs",
        "initdir",
        "inituser",
        "initgroup",
        "kernel",
        "initrd",
        "kernel_args",
        "dtb",
        "acpi_tb",
        "acpi_tb_type",
        "bootdevs",
        "bootmenu_enable",
        "bootmenu_timeout",
        "bios_useserial",
        "bios_rebootTimeout",
        "smbios_mode",
    ],
};

/// `DomainOs.bootorder`'s setter.
pub fn set_bootorder(os: &mut XmlObject, newdevs: &[String]) -> Result<(), String> {
    while !os.children("bootdevs").is_empty() {
        let n = os.children("bootdevs").len();
        os.remove_child("bootdevs", n - 1);
    }
    for d in newdevs {
        let mut dev = XmlObject::new(&BOOT_DEVICE);
        dev.set("dev", Some(crate::xmlbuilder::Value::Str(d.clone())))?;
        os.add_child(dev);
    }
    Ok(())
}

/// `DomainOs.set_initargs_string`.
pub fn set_initargs_string(os: &mut XmlObject, argstring: &str) -> Result<(), String> {
    while !os.children("initargs").is_empty() {
        let n = os.children("initargs").len();
        os.remove_child("initargs", n - 1);
    }
    let vals = shlex::split(argstring).ok_or("No closing quotation")?;
    for val in vals {
        let mut obj = XmlObject::new(&INIT_ARG);
        obj.set("val", Some(crate::xmlbuilder::Value::Str(val)))?;
        os.add_child(obj);
    }
    Ok(())
}

/// `DomainOs.is_x86`.
pub fn is_x86(os: &XmlObject) -> bool {
    matches!(os.get_str("arch").as_deref(), Some("x86_64" | "i686"))
}

/// `DomainOs.is_q35`.
pub fn is_q35(os: &XmlObject) -> bool {
    is_x86(os) && os.get_str("machine").is_some_and(|m| m.contains("q35"))
}
