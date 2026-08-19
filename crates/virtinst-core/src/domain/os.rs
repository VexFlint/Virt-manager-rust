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
