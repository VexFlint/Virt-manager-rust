//! `install/unattended.py`: libosinfo install scripts for `--unattended`.

use std::collections::BTreeMap;
use std::io::Write;

use super::cloudinit::get_password;
use super::urlfetcher::{direct_fetcher, named_tempfile};
use crate::osinfo::installscript::{injection_method, InstallConfig, InstallScript, ScriptTarget};
use crate::osinfo::{OsDb, OsInfo, OsMedia, OsTree};
use crate::progress::Meter;

fn is_user_login_safe(login: &str) -> bool {
    login != "root"
}

/// `getpass.getuser()`: the login env vars, then the passwd entry.
fn getuser() -> Option<String> {
    for var in ["LOGNAME", "USER", "LNAME", "USERNAME"] {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    // SAFETY: getpwuid returns NULL or a pointer to static storage.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() {
            return None;
        }
        Some(
            std::ffi::CStr::from_ptr((*pw).pw_name)
                .to_string_lossy()
                .into_owned(),
        )
    }
}

/// `pwd.getpwnam(name).pw_gecos`.
fn gecos(name: &str) -> String {
    let Ok(c) = std::ffi::CString::new(name) else {
        return String::new();
    };
    // SAFETY: getpwnam returns NULL or a pointer to static storage.
    unsafe {
        let pw = libc::getpwnam(c.as_ptr());
        if pw.is_null() || (*pw).pw_gecos.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr((*pw).pw_gecos)
            .to_string_lossy()
            .into_owned()
    }
}

/// `_login_from_hostuser`.
fn login_from_hostuser() -> (Option<String>, Option<String>) {
    let Some(hostuser) = getuser() else {
        return (None, None);
    };
    if !is_user_login_safe(&hostuser) {
        return (None, None);
    }
    let realname = gecos(&hostuser);
    (Some(hostuser), Some(realname))
}

/// `get_timezone`: the zone `/etc/localtime` links to.
fn get_timezone() -> Option<String> {
    let linkpath = std::fs::canonicalize("/etc/localtime").ok()?;
    let linkpath = linkpath.to_string_lossy();
    let mut tokens = linkpath.split("zoneinfo/");
    tokens.next();
    tokens.next().map(str::to_string)
}

/// `locale.getlocale()[0]`: Python sets LC_CTYPE from the environment
/// at startup and coerces the C locale to C.UTF-8.
fn get_language() -> Option<String> {
    // SAFETY: setlocale with an empty string reads the environment; the
    // returned pointer is valid until the next setlocale call.
    let name = unsafe {
        let p = libc::setlocale(libc::LC_CTYPE, c"".as_ptr());
        if p.is_null() {
            None
        } else {
            Some(std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned())
        }
    };
    let name = match name.as_deref() {
        None | Some("C") | Some("POSIX") => "C.UTF-8".to_string(),
        Some(n) => n.to_string(),
    };
    let code = name.split('@').next().unwrap_or_default();
    if let Some((lang, _)) = code.split_once('.') {
        return Some(lang.to_string());
    }
    (code != "C").then(|| code.to_string())
}

/// `UnattendedData`.
#[derive(Clone, Debug, Default)]
pub struct UnattendedData {
    pub profile: Option<String>,
    pub admin_password_file: Option<String>,
    pub user_login: Option<String>,
    pub user_password_file: Option<String>,
    pub product_key: Option<String>,
    pub reg_login: Option<String>,
}

impl UnattendedData {
    pub fn get_user_password(&self) -> Result<Option<String>, String> {
        self.user_password_file
            .as_deref()
            .map(get_password)
            .transpose()
    }

    pub fn get_admin_password(&self) -> Result<Option<String>, String> {
        self.admin_password_file
            .as_deref()
            .map(get_password)
            .transpose()
    }
}

/// `_make_installconfig`.
fn make_installconfig(
    script: &OSInstallScript,
    osobj: &OsInfo,
    data: &UnattendedData,
    arch: &str,
    hostname: &str,
    url: Option<&str>,
) -> Result<InstallConfig, String> {
    let config = InstallConfig::new();
    let osname = osobj.short_id();

    let (login, realname) = match &data.user_login {
        Some(l) if !l.is_empty() => (Some(l.clone()), Some(l.clone())),
        _ => login_from_hostuser(),
    };
    if let Some(login) = login {
        let login = login.to_lowercase();
        if !is_user_login_safe(&login) {
            return Err(format!("{osname} cannot use '{login}' as user-login."));
        }
        config.set_user_login(&login);
        config.set_user_realname(&realname.unwrap_or_default());
    }

    let user_password = data.get_user_password()?.filter(|p| !p.is_empty());
    if script.script.requires_user_password() && user_password.is_none() {
        return Err(format!("{osname} requires the user-password to be set."));
    }
    config.set_user_password(user_password.as_deref().unwrap_or(""));

    let admin_password = data.get_admin_password()?.filter(|p| !p.is_empty());
    if script.script.requires_admin_password() && admin_password.is_none() {
        return Err(format!("{osname} requires the admin-password to be set."));
    }
    config.set_admin_password(admin_password.as_deref().unwrap_or(""));

    let tgt = if osobj.is_windows() {
        "C"
    } else if osobj.supports_virtiodisk() {
        "/dev/vda"
    } else {
        "/dev/sda"
    };
    config.set_target_disk(tgt);
    config.set_hardware_arch(arch);

    // Some installations will bail if the Computer's name contains one of
    // the following characters: "[{|}~[\\]^':; <=>?@!\"#$%`()+/.,*&]".
    let hostname: String = hostname
        .chars()
        .map(|c| {
            if "{|}~[]^':; <=>?@!\"#$%`()+/.,*&".contains(c) {
                '-'
            } else {
                c
            }
        })
        .collect();
    config.set_hostname(&hostname);

    if let Some(tz) = get_timezone() {
        config.set_l10n_timezone(&tz);
    }
    if let Some(lang) = get_language() {
        config.set_l10n_language(&lang);
        config.set_l10n_keyboard(&lang);
    }
    if let Some(url) = url {
        config.set_installation_url(url);
    }
    if let Some(r) = data.reg_login.as_deref().filter(|r| !r.is_empty()) {
        config.set_reg_login(r);
    }
    if let Some(k) = data.product_key.as_deref().filter(|k| !k.is_empty()) {
        config.set_reg_product_key(k);
    }

    let none = |v: Option<String>| v.unwrap_or_else(|| "None".into());
    log::debug!("InstallScriptConfig created with the following params:");
    log::debug!("username: {}", none(config.get_user_login()));
    log::debug!("realname: {}", none(config.get_user_realname()));
    log::debug!("target disk: {}", none(config.get_target_disk()));
    log::debug!("hardware arch: {}", none(config.get_hardware_arch()));
    log::debug!("hostname: {}", none(config.get_hostname()));
    log::debug!("timezone: {}", none(config.get_l10n_timezone()));
    log::debug!("language: {}", none(config.get_l10n_language()));
    log::debug!("keyboard: {}", none(config.get_l10n_keyboard()));
    log::debug!("url: {}", none(config.get_installation_url()));
    log::debug!("reg-login {}", none(config.get_reg_login()));
    log::debug!("product-key: {}", none(config.get_reg_product_key()));
    Ok(config)
}

/// `OSInstallScript`.
pub struct OSInstallScript {
    script: InstallScript,
    osname: String,
    target: ScriptTarget,
    config: Option<InstallConfig>,
}

impl OSInstallScript {
    /// `have_new_libosinfo`: win7's scripts know the cdrom injection.
    pub fn have_new_libosinfo() -> bool {
        let Some(win7) = OsDb::load_default()
            .ok()
            .and_then(|db| db.lookup_os("win7"))
        else {
            return false;
        };
        let cdrom = injection_method("cdrom");
        win7.get_install_script_list()
            .iter()
            .any(|s| s.get_injection_methods() & cdrom != 0)
    }

    fn new(
        script: InstallScript,
        osobj: &OsInfo,
        media: Option<&OsMedia>,
        tree: Option<&OsTree>,
    ) -> Result<Self, String> {
        if !Self::have_new_libosinfo() {
            return Err("libosinfo or osinfo-db is too old to support unattended installs.".into());
        }
        let target = match (media, tree) {
            (Some(m), _) => ScriptTarget::Media(m.clone()),
            (None, Some(t)) => ScriptTarget::Tree(t.clone()),
            _ => ScriptTarget::Os(osobj.clone()),
        };
        Ok(OSInstallScript {
            script,
            osname: osobj.short_id(),
            target,
            config: None,
        })
    }

    pub fn get_expected_filename(&self) -> String {
        self.script.get_expected_filename()
    }

    fn set_preferred_injection_method(&self, namestr: &str) -> Result<(), String> {
        log::debug!("Using '{namestr}' injection method");
        let method = injection_method(namestr);
        if method & self.script.get_injection_methods() == 0 {
            return Err(format!(
                "OS '{}' does not support required injection method '{namestr}'",
                self.osname
            ));
        }
        self.script.set_preferred_injection_method(method);
        Ok(())
    }

    fn set_installation_source(&self, namestr: &str) {
        log::debug!("Using '{namestr}' installation source");
        self.script.set_installation_source(namestr);
    }

    fn config(&self) -> &InstallConfig {
        self.config.as_ref().expect("set_config runs first")
    }

    pub fn generate(&self) -> Result<String, String> {
        self.script.generate(&self.target, self.config())
    }

    pub fn generate_cmdline(&self) -> Option<String> {
        self.script.generate_cmdline(&self.target, self.config())
    }

    fn generate_debug(&self) -> Result<String, String> {
        let config = self.config();
        let user = config.get_user_password();
        let admin = config.get_admin_password();
        config.set_user_password("[SCRUBBLED]");
        config.set_admin_password("[SCRUBBLED]");
        let ret = self.generate();
        config.set_user_password(&user.unwrap_or_default());
        config.set_admin_password(&admin.unwrap_or_default());
        ret
    }

    /// `write`: the script in a new tempfile; returns its path.
    pub fn write(&self) -> Result<String, String> {
        let tmpdir = std::env::temp_dir();
        let (mut f, scriptpath) =
            named_tempfile(&tmpdir.to_string_lossy(), "virtinst-unattended-script", "")?;
        let content = self.generate()?;
        f.write_all(content.as_bytes())
            .map_err(|e| crate::diskbackend::oserror_str(&e, Some(&scriptpath)))?;
        let debug_content = self.generate_debug()?;
        log::debug!("Generated unattended script: {scriptpath}");
        log::debug!("Generated script contents:\n{debug_content}");
        Ok(scriptpath)
    }
}

/// `_find_default_profile`.
fn find_default_profile(profile_names: &[String]) -> String {
    if profile_names.iter().any(|p| p == "desktop") {
        return "desktop".into();
    }
    profile_names[0].clone()
}

/// `_lookup_rawscripts`.
fn lookup_rawscripts(
    osinfo: &OsInfo,
    profile: Option<&str>,
    os_media: Option<&OsMedia>,
) -> Result<Vec<InstallScript>, String> {
    let osname = osinfo.short_id();
    let mut script_list = vec![];
    if let Some(media) = os_media {
        if !media.supports_installer_script() {
            // This is a specific annotation for media like livecds that
            // don't support unattended installs
            return Err(format!(
                "OS '{osname}' media does not support unattended installation"
            ));
        }
        script_list = media.get_install_script_list();
    }
    if script_list.is_empty() {
        script_list = osinfo.get_install_script_list();
    }
    if script_list.is_empty() {
        return Err(format!(
            "OS '{osname}' does not support unattended installation."
        ));
    }

    // `_make_scriptmap`
    let mut script_map: BTreeMap<String, Vec<InstallScript>> = BTreeMap::new();
    for s in script_list {
        script_map
            .entry(s.get_profile().unwrap_or_else(|| "None".into()))
            .or_default()
            .push(s);
    }
    let profile_names: Vec<String> = script_map.keys().cloned().collect();
    let (profile, rawscripts) = match profile.filter(|p| !p.is_empty()) {
        Some(p) => match script_map.remove(p) {
            Some(r) => (p.to_string(), r),
            None => {
                return Err(format!(
                    "OS '{osname}' does not support unattended installation for the '{p}' \
                     profile. Available profiles: {}",
                    profile_names.join(", ")
                ))
            }
        },
        None => {
            let p = find_default_profile(&profile_names);
            log::warn!("Using unattended profile '{p}'");
            let r = script_map.remove(&p).unwrap_or_default();
            (p, r)
        }
    };

    let ids: Vec<String> = rawscripts.iter().map(|s| s.get_id()).collect();
    log::debug!(
        "Install scripts found for profile '{profile}': {}",
        ids.join(", ")
    );
    Ok(rawscripts)
}

/// The guest facts `prepare_install_scripts` reads.
pub struct ScriptGuest<'a> {
    pub osinfo: &'a OsInfo,
    pub arch: &'a str,
    pub name: &'a str,
}

/// `prepare_install_scripts`.
pub fn prepare_install_scripts(
    guest: &ScriptGuest,
    data: &UnattendedData,
    url: Option<&str>,
    os_media: Option<&OsMedia>,
    os_tree: Option<&OsTree>,
    injection_method: &str,
) -> Result<Vec<OSInstallScript>, String> {
    let rawscripts = lookup_rawscripts(guest.osinfo, data.profile.as_deref(), os_media)?;
    let mut scripts = vec![];
    for rawscript in rawscripts {
        let mut script = OSInstallScript::new(rawscript, guest.osinfo, os_media, os_tree)?;
        script.set_preferred_injection_method(injection_method)?;
        script.set_installation_source(if os_media.is_some() {
            "media"
        } else {
            "network"
        });
        let config = make_installconfig(&script, guest.osinfo, data, guest.arch, guest.name, url)?;
        script.config = Some(config);
        scripts.push(script);
    }
    Ok(scripts)
}

/// `download_drivers`: (local file, file name) pairs.
pub fn download_drivers(
    locations: &[String],
    scratchdir: &str,
    meter: &mut dyn Meter,
) -> Result<Vec<(String, String)>, String> {
    let mut fetcher = direct_fetcher(scratchdir);
    let mut drivers: Vec<(String, String)> = vec![];
    for location in locations {
        let filename = location.rsplit_once('/').map_or("", |(_, f)| f).to_string();
        match fetcher.acquire_file(location, None, meter) {
            Ok(driver) => drivers.push((driver, filename)),
            Err(e) => {
                for (f, _) in &drivers {
                    let _ = std::fs::remove_file(f);
                }
                return Err(e);
            }
        }
    }
    Ok(drivers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_prefers_desktop() {
        assert_eq!(
            find_default_profile(&["desktop".into(), "jeos".into()]),
            "desktop"
        );
        assert_eq!(find_default_profile(&["jeos".into()]), "jeos");
    }

    #[test]
    fn root_login_is_unsafe() {
        assert!(!is_user_login_safe("root"));
        assert!(is_user_login_safe("foobar"));
    }

    #[test]
    fn fedora_scripts_by_profile() {
        let db = OsDb::load_default().unwrap();
        let os = db.lookup_os("fedora26").unwrap();
        let err = lookup_rawscripts(&os, Some("FRIBBER"), None).err().unwrap();
        assert!(err.contains("Available profiles: desktop, jeos"), "{err}");
        assert!(!lookup_rawscripts(&os, Some("jeos"), None)
            .unwrap()
            .is_empty());
    }
}

#[cfg(test)]
mod generate_tests {
    use super::*;

    #[test]
    fn fedora_kickstart_generates() {
        let db = OsDb::load_default().unwrap();
        let os = db.lookup_os("fedora26").unwrap();
        let data = UnattendedData {
            profile: Some("desktop".into()),
            user_login: Some("foobar".into()),
            ..UnattendedData::default()
        };
        let guest = ScriptGuest {
            osinfo: &os,
            arch: "x86_64",
            name: "f26",
        };
        let scripts = prepare_install_scripts(&guest, &data, None, None, None, "initrd").unwrap();
        let content = scripts[0].generate().unwrap();
        assert!(content.contains("foobar"), "{content}");
    }
}
