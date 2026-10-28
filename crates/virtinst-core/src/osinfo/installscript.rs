//! The libosinfo install-script, install-config and device-driver
//! objects that `install/unattended.py` drives through
//! `gi.repository.Libosinfo`.

use std::ffi::CString;
use std::ptr;
use std::sync::Mutex;

use super::{c_str_to_string, ffi, OsInfo, OsMedia, OsTree};

/// `Guest._extra_drivers`: the devices `add_extra_drivers()` registered,
/// as `(id, class, name)`. Upstream keeps them on the guest and passes
/// them to every `supports_virtio*()` check; virt-install only ever has
/// one guest per process, so they live here.
static EXTRA_DEVICES: Mutex<Vec<ExtraDevice>> = Mutex::new(Vec::new());

/// One device of a pre-installable driver.
#[derive(Clone, Debug)]
pub struct ExtraDevice {
    pub id: String,
    pub class: String,
    pub name: String,
}

/// `Guest.add_extra_drivers`.
pub fn add_extra_drivers(devices: Vec<ExtraDevice>) {
    *EXTRA_DEVICES.lock().unwrap() = devices;
}

pub(super) fn extra_devices() -> Vec<ExtraDevice> {
    EXTRA_DEVICES.lock().unwrap().clone()
}

/// Walks an `OsinfoList` and hands each element to `f`.
unsafe fn for_each_entity(list: *mut ffi::OsinfoList, mut f: impl FnMut(*mut ffi::OsinfoEntity)) {
    if list.is_null() {
        return;
    }
    for i in 0..ffi::osinfo_list_get_length(list) {
        let e = ffi::osinfo_list_get_nth(list, i);
        if !e.is_null() {
            f(e);
        }
    }
}

fn opt_str(p: *const std::os::raw::c_char) -> Option<String> {
    (!p.is_null()).then(|| unsafe { c_str_to_string(p) })
}

fn cstring(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap()
}

/// `Libosinfo.InstallScript`.
pub struct InstallScript {
    ptr: *mut ffi::OsinfoInstallScript,
}

impl Drop for InstallScript {
    fn drop(&mut self) {
        unsafe { gobject_sys::g_object_unref(self.ptr as *mut gobject_sys::GObject) };
    }
}

/// `osinfo_*_get_install_script_list` (transfer full): refs each script
/// and drops the list.
unsafe fn script_list(list: *mut ffi::OsinfoInstallScriptList) -> Vec<InstallScript> {
    let mut ret = vec![];
    let list = list as *mut ffi::OsinfoList;
    for_each_entity(list, |e| {
        gobject_sys::g_object_ref(e as *mut gobject_sys::GObject);
        ret.push(InstallScript {
            ptr: e as *mut ffi::OsinfoInstallScript,
        });
    });
    if !list.is_null() {
        gobject_sys::g_object_unref(list as *mut gobject_sys::GObject);
    }
    ret
}

/// `INJECTION_METHOD_*`: the `namestr` names upstream maps.
pub fn injection_method(namestr: &str) -> u32 {
    match namestr {
        "initrd" => {
            ffi::OsinfoInstallScriptInjectionMethod_OSINFO_INSTALL_SCRIPT_INJECTION_METHOD_INITRD
        }
        _ => ffi::OsinfoInstallScriptInjectionMethod_OSINFO_INSTALL_SCRIPT_INJECTION_METHOD_CDROM,
    }
}

/// The three things a script can generate for.
#[derive(Clone)]
pub enum ScriptTarget {
    Media(OsMedia),
    Tree(OsTree),
    Os(OsInfo),
}

impl Clone for OsMedia {
    fn clone(&self) -> Self {
        unsafe { gobject_sys::g_object_ref(self.ptr as *mut gobject_sys::GObject) };
        OsMedia {
            ptr: self.ptr,
            os: self.os.clone(),
        }
    }
}

impl Clone for OsTree {
    fn clone(&self) -> Self {
        unsafe { gobject_sys::g_object_ref(self.ptr as *mut gobject_sys::GObject) };
        OsTree {
            ptr: self.ptr,
            os: self.os.clone(),
        }
    }
}

impl InstallScript {
    pub fn get_id(&self) -> String {
        unsafe {
            c_str_to_string(ffi::osinfo_entity_get_id(
                self.ptr as *mut ffi::OsinfoEntity,
            ))
        }
    }

    pub fn get_profile(&self) -> Option<String> {
        opt_str(unsafe { ffi::osinfo_install_script_get_profile(self.ptr) })
    }

    pub fn get_expected_filename(&self) -> String {
        opt_str(unsafe { ffi::osinfo_install_script_get_expected_filename(self.ptr) })
            .unwrap_or_default()
    }

    pub fn get_injection_methods(&self) -> u32 {
        unsafe { ffi::osinfo_install_script_get_injection_methods(self.ptr) }
    }

    pub fn set_preferred_injection_method(&self, method: u32) {
        unsafe { ffi::osinfo_install_script_set_preferred_injection_method(self.ptr, method) }
    }

    /// `set_installation_source`: "media" or "network".
    pub fn set_installation_source(&self, namestr: &str) {
        let src = if namestr == "media" {
            ffi::OsinfoInstallScriptInstallationSource_OSINFO_INSTALL_SCRIPT_INSTALLATION_SOURCE_MEDIA
        } else {
            ffi::OsinfoInstallScriptInstallationSource_OSINFO_INSTALL_SCRIPT_INSTALLATION_SOURCE_NETWORK
        };
        unsafe { ffi::osinfo_install_script_set_installation_source(self.ptr, src) }
    }

    /// `_requires_param`.
    fn requires_param(&self, name: &[u8]) -> bool {
        unsafe {
            let p = ffi::osinfo_install_script_get_config_param(
                self.ptr,
                name.as_ptr() as *const std::os::raw::c_char,
            );
            if p.is_null() {
                return false;
            }
            let ret = ffi::osinfo_install_config_param_is_optional(p) == 0;
            gobject_sys::g_object_unref(p as *mut gobject_sys::GObject);
            ret
        }
    }

    pub fn requires_user_password(&self) -> bool {
        self.requires_param(ffi::OSINFO_INSTALL_CONFIG_PROP_USER_PASSWORD)
    }

    pub fn requires_admin_password(&self) -> bool {
        self.requires_param(ffi::OSINFO_INSTALL_CONFIG_PROP_ADMIN_PASSWORD)
    }

    /// `generate_for_media` / `generate_for_tree` / `generate`.
    pub fn generate(
        &self,
        target: &ScriptTarget,
        config: &InstallConfig,
    ) -> Result<String, String> {
        unsafe {
            let mut err: *mut glib_sys::GError = ptr::null_mut();
            let out = match target {
                ScriptTarget::Media(m) => ffi::osinfo_install_script_generate_for_media(
                    self.ptr,
                    m.ptr,
                    config.ptr,
                    ptr::null_mut(),
                    &mut err,
                ),
                ScriptTarget::Tree(t) => ffi::osinfo_install_script_generate_for_tree(
                    self.ptr,
                    t.ptr,
                    config.ptr,
                    ptr::null_mut(),
                    &mut err,
                ),
                ScriptTarget::Os(o) => ffi::osinfo_install_script_generate(
                    self.ptr,
                    o.ptr,
                    config.ptr,
                    ptr::null_mut(),
                    &mut err,
                ),
            };
            take_gstring(out, err)
        }
    }

    /// `generate_command_line*`.
    pub fn generate_cmdline(
        &self,
        target: &ScriptTarget,
        config: &InstallConfig,
    ) -> Option<String> {
        unsafe {
            let out = match target {
                ScriptTarget::Media(m) => {
                    ffi::osinfo_install_script_generate_command_line_for_media(
                        self.ptr, m.ptr, config.ptr,
                    )
                }
                ScriptTarget::Tree(t) => ffi::osinfo_install_script_generate_command_line_for_tree(
                    self.ptr, t.ptr, config.ptr,
                ),
                ScriptTarget::Os(o) => {
                    ffi::osinfo_install_script_generate_command_line(self.ptr, o.ptr, config.ptr)
                }
            };
            take_gstring(out, ptr::null_mut()).ok()
        }
    }
}

/// Turns a `transfer full` string (or its GError) into a Result.
unsafe fn take_gstring(
    out: *mut std::os::raw::c_char,
    err: *mut glib_sys::GError,
) -> Result<String, String> {
    if !err.is_null() {
        let msg = c_str_to_string((*err).message);
        glib_sys::g_error_free(err);
        if !out.is_null() {
            glib_sys::g_free(out as *mut _);
        }
        return Err(msg);
    }
    if out.is_null() {
        return Err("libosinfo returned no content".into());
    }
    let s = c_str_to_string(out);
    glib_sys::g_free(out as *mut _);
    Ok(s)
}

/// `Libosinfo.InstallConfig`.
pub struct InstallConfig {
    ptr: *mut ffi::OsinfoInstallConfig,
}

impl Drop for InstallConfig {
    fn drop(&mut self) {
        unsafe { gobject_sys::g_object_unref(self.ptr as *mut gobject_sys::GObject) };
    }
}

impl Default for InstallConfig {
    fn default() -> Self {
        Self::new()
    }
}

macro_rules! config_str {
    ($set:ident, $get:ident, $ffiset:ident, $ffiget:ident) => {
        pub fn $set(&self, v: &str) {
            let c = cstring(v);
            unsafe { ffi::$ffiset(self.ptr, c.as_ptr()) }
        }
        pub fn $get(&self) -> Option<String> {
            opt_str(unsafe { ffi::$ffiget(self.ptr) })
        }
    };
}

impl InstallConfig {
    /// `Libosinfo.InstallConfig()`: a bare `g_object_new`, which still
    /// gets libosinfo's own l10n defaults.
    pub fn new() -> Self {
        let ptr = unsafe {
            gobject_sys::g_object_new_with_properties(
                ffi::osinfo_install_config_get_type(),
                0,
                ptr::null_mut(),
                ptr::null(),
            )
        };
        InstallConfig {
            ptr: ptr as *mut ffi::OsinfoInstallConfig,
        }
    }

    config_str!(
        set_user_login,
        get_user_login,
        osinfo_install_config_set_user_login,
        osinfo_install_config_get_user_login
    );
    config_str!(
        set_user_realname,
        get_user_realname,
        osinfo_install_config_set_user_realname,
        osinfo_install_config_get_user_realname
    );
    config_str!(
        set_user_password,
        get_user_password,
        osinfo_install_config_set_user_password,
        osinfo_install_config_get_user_password
    );
    config_str!(
        set_admin_password,
        get_admin_password,
        osinfo_install_config_set_admin_password,
        osinfo_install_config_get_admin_password
    );
    config_str!(
        set_target_disk,
        get_target_disk,
        osinfo_install_config_set_target_disk,
        osinfo_install_config_get_target_disk
    );
    config_str!(
        set_hardware_arch,
        get_hardware_arch,
        osinfo_install_config_set_hardware_arch,
        osinfo_install_config_get_hardware_arch
    );
    config_str!(
        set_hostname,
        get_hostname,
        osinfo_install_config_set_hostname,
        osinfo_install_config_get_hostname
    );
    config_str!(
        set_l10n_timezone,
        get_l10n_timezone,
        osinfo_install_config_set_l10n_timezone,
        osinfo_install_config_get_l10n_timezone
    );
    config_str!(
        set_l10n_language,
        get_l10n_language,
        osinfo_install_config_set_l10n_language,
        osinfo_install_config_get_l10n_language
    );
    config_str!(
        set_l10n_keyboard,
        get_l10n_keyboard,
        osinfo_install_config_set_l10n_keyboard,
        osinfo_install_config_get_l10n_keyboard
    );
    config_str!(
        set_installation_url,
        get_installation_url,
        osinfo_install_config_set_installation_url,
        osinfo_install_config_get_installation_url
    );
    config_str!(
        set_reg_login,
        get_reg_login,
        osinfo_install_config_set_reg_login,
        osinfo_install_config_get_reg_login
    );
    config_str!(
        set_reg_product_key,
        get_reg_product_key,
        osinfo_install_config_set_reg_product_key,
        osinfo_install_config_get_reg_product_key
    );
}

/// One pre-installable driver: `get_location()`, `get_files()` and the
/// devices it brings.
struct Driver {
    location: String,
    files: Vec<String>,
    devices: Vec<ExtraDevice>,
}

impl OsMedia {
    pub fn supports_installer_script(&self) -> bool {
        unsafe { ffi::osinfo_media_supports_installer_script(self.ptr) != 0 }
    }

    pub fn get_install_script_list(&self) -> Vec<InstallScript> {
        unsafe { script_list(ffi::osinfo_media_get_install_script_list(self.ptr)) }
    }
}

impl OsInfo {
    pub fn get_install_script_list(&self) -> Vec<InstallScript> {
        unsafe { script_list(ffi::osinfo_os_get_install_script_list(self.ptr)) }
    }

    /// `_get_pre_installable_drivers`.
    fn pre_installable_drivers(&self, arch: &str) -> Vec<Driver> {
        let mut ret = vec![];
        unsafe {
            let list = ffi::osinfo_os_get_device_drivers(self.ptr) as *mut ffi::OsinfoList;
            for_each_entity(list, |e| {
                let d = e as *mut ffi::OsinfoDeviceDriver;
                let darch = opt_str(ffi::osinfo_device_driver_get_architecture(d));
                if arch != "all" && darch.as_deref() != Some(arch) {
                    return;
                }
                if ffi::osinfo_device_driver_get_pre_installable(d) == 0 {
                    return;
                }
                let mut files = vec![];
                let glist = ffi::osinfo_device_driver_get_files(d);
                let mut node = glist;
                while !node.is_null() {
                    files.push(c_str_to_string((*node).data as *const _));
                    node = (*node).next;
                }
                if !glist.is_null() {
                    glib_sys::g_list_free(glist);
                }
                let mut devices = vec![];
                let devlist = ffi::osinfo_device_driver_get_devices(d) as *mut ffi::OsinfoList;
                for_each_entity(devlist, |de| {
                    let dev = de as *mut ffi::OsinfoDevice;
                    devices.push(ExtraDevice {
                        id: c_str_to_string(ffi::osinfo_entity_get_id(de)),
                        class: opt_str(ffi::osinfo_device_get_class(dev)).unwrap_or_default(),
                        name: opt_str(ffi::osinfo_device_get_name(dev)).unwrap_or_default(),
                    });
                });
                ret.push(Driver {
                    location: opt_str(ffi::osinfo_device_driver_get_location(d))
                        .unwrap_or_default(),
                    files,
                    devices,
                });
            });
        }
        ret
    }

    /// `get_pre_installable_drivers_location`.
    pub fn get_pre_installable_drivers_location(&self, arch: &str) -> Vec<String> {
        let mut ret = vec![];
        for d in self.pre_installable_drivers(arch) {
            for f in d.files {
                // `os.path.join`
                if f.starts_with('/') {
                    ret.push(f);
                } else if d.location.is_empty() || d.location.ends_with('/') {
                    ret.push(format!("{}{f}", d.location));
                } else {
                    ret.push(format!("{}/{f}", d.location));
                }
            }
        }
        ret
    }

    /// `get_pre_installable_devices`.
    pub fn get_pre_installable_devices(&self, arch: &str) -> Vec<ExtraDevice> {
        self.pre_installable_drivers(arch)
            .into_iter()
            .flat_map(|d| d.devices)
            .collect()
    }

    /// `supports_unattended_drivers`.
    pub fn supports_unattended_drivers(&self, arch: &str) -> bool {
        !self.pre_installable_drivers(arch).is_empty()
    }
}
