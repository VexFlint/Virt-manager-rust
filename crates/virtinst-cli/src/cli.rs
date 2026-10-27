//! The shared CLI plumbing from upstream's `cli.py`: logging
//! (`setupLogging`), `fail`/`print_stdout`, the `--check` validation
//! switches (`ParserCheck`, `_optional_fail`), the libvirt connection
//! with interactive auth (`getConnection`), and the disk/MAC sanity
//! checks shared by the tools (`validate_disk`, `validate_mac`).

use std::io::Write;
use std::sync::Mutex;

use virtinst_core::connection::Connection;
use virtinst_core::diskbackend::oserror_str;

/// `BuildConfig.version`: the upstream virt-manager release these tools
/// are a port of, which is what upstream's `--version` prints.
pub const VERSION: &str = "5.1.0";

/// The `--version` argparse adds in `setupParser`: print just the
/// version and exit, wherever the flag is and whatever else is on the
/// command line, before any other parsing.
pub fn handle_version_flag() {
    let args = std::env::args().skip(1).take_while(|a| a != "--");
    if args.into_iter().any(|a| a == "--version") {
        println!("{VERSION}");
        std::process::exit(0);
    }
}

/// `_GlobalState`.
struct GlobalState {
    quiet: bool,
    all_checks: Option<bool>,
    validation_checks: Vec<(String, bool)>,
}

static GLOBAL: Mutex<GlobalState> = Mutex::new(GlobalState {
    quiet: false,
    all_checks: None,
    validation_checks: Vec::new(),
});

fn global() -> std::sync::MutexGuard<'static, GlobalState> {
    GLOBAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// `get_validation_check`: every check defaults to on.
fn get_validation_check(checkname: &str) -> bool {
    let g = global();
    if let Some(all) = g.all_checks {
        return all;
    }
    g.validation_checks
        .iter()
        .rev()
        .find(|(k, _)| k == checkname)
        .map_or(true, |(_, v)| *v)
}

pub fn is_quiet() -> bool {
    global().quiet
}

/// The process's logger: upstream's stream handler on stderr (warnings
/// and errors as `LEVEL    message`, everything with `--debug`) plus the
/// rotating debug log file in the app cache dir.
struct Logger {
    appname: String,
    debug_stdout: bool,
    stream_level: log::LevelFilter,
    logfile: Mutex<Option<std::fs::File>>,
}

fn level_name(level: log::Level) -> &'static str {
    match level {
        log::Level::Error => "ERROR",
        log::Level::Warn => "WARNING",
        log::Level::Info => "INFO",
        log::Level::Debug | log::Level::Trace => "DEBUG",
    }
}

/// `%a, %d %b %Y %H:%M:%S` in local time.
fn asctime() -> String {
    // SAFETY: localtime_r and strftime write into our own buffers.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        let mut buf = [0u8; 64];
        let fmt = b"%a, %d %b %Y %H:%M:%S\0";
        let n = libc::strftime(buf.as_mut_ptr().cast(), buf.len(), fmt.as_ptr().cast(), &tm);
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }
}

impl Logger {
    /// `fileFormat`.
    fn file_line(&self, record: &log::Record) -> String {
        let module = record
            .file()
            .and_then(|f| f.rsplit('/').next())
            .map(|f| f.trim_end_matches(".rs"))
            .unwrap_or("?");
        format!(
            "[{} {} {}] {} ({}:{}) {}",
            asctime(),
            self.appname,
            std::process::id(),
            level_name(record.level()),
            module,
            record.line().unwrap_or(0),
            record.args()
        )
    }
}

impl log::Log for Logger {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        let fileline = self.file_line(record);
        if let Some(f) = self
            .logfile
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            let _ = writeln!(f, "{fileline}");
        }
        if record.level() <= self.stream_level {
            let line = if self.debug_stdout {
                fileline
            } else {
                // `streamErrorFormat`
                format!("{:<8} {}", level_name(record.level()), record.args())
            };
            let _ = writeln!(std::io::stderr(), "{line}");
        }
    }

    fn flush(&self) {}
}

/// `VirtinstConnection.get_app_cache_dir`.
fn app_cache_dir() -> String {
    let base = std::env::var("XDG_CACHE_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}/.cache", std::env::var("HOME").unwrap_or_default()));
    format!("{base}/virt-manager")
}

/// `RotatingFileHandler(logfile, "ae", 1024 * 1024, 5)`: rotate a full
/// log before appending.
fn open_logfile(appname: &str) -> Result<std::fs::File, String> {
    use std::os::unix::fs::DirBuilderExt;
    let dir = app_cache_dir();
    if !std::path::Path::new(&dir).exists() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o751)
            .create(&dir)
            .map_err(|e| {
                format!(
                    "Could not create directory {dir}: {}",
                    oserror_str(&e, Some(&dir))
                )
            })?;
    }
    let logfile = format!("{dir}/{appname}.log");
    if std::fs::metadata(&logfile).is_ok_and(|m| m.len() >= 1024 * 1024) {
        for i in (1..5).rev() {
            let _ = std::fs::rename(format!("{logfile}.{i}"), format!("{logfile}.{}", i + 1));
        }
        let _ = std::fs::rename(&logfile, format!("{logfile}.1"));
    }
    std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&logfile)
        .map_err(|e| {
            format!(
                "No write access to logfile {logfile}: {}",
                oserror_str(&e, Some(&logfile))
            )
        })
}

/// `setupLogging`: errors and warnings on stderr (only errors when
/// `quiet`), everything when `debug_stdout`, and a debug log file.
pub fn setup_logging(appname: &str, debug_stdout: bool, quiet: bool) {
    {
        let mut g = global();
        g.quiet = quiet;
        g.all_checks = None;
        g.validation_checks.clear();
    }
    let in_testsuite = std::env::var_os("VIRTINST_TEST_SUITE").is_some();
    let logfile = if in_testsuite {
        None
    } else {
        match open_logfile(appname) {
            Ok(f) => Some(f),
            Err(e) => {
                eprintln!("WARNING  Error setting up logfile: {e}");
                None
            }
        }
    };
    let stream_level = if debug_stdout {
        log::LevelFilter::Debug
    } else if quiet {
        log::LevelFilter::Error
    } else {
        log::LevelFilter::Warn
    };
    let logger = Logger {
        appname: appname.into(),
        debug_stdout,
        stream_level,
        logfile: Mutex::new(logfile),
    };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(log::LevelFilter::Debug);
    }
    let argv: Vec<String> = std::env::args().collect();
    log::debug!(
        "Version {} launched with command line: {}",
        VERSION,
        argv.join(" ")
    );
}

/// `fail`: log the error and exit 1.
pub fn fail(msg: impl std::fmt::Display) -> ! {
    log::error!("{msg}");
    std::process::exit(1);
}

/// `print_stdout`: printed unless `--quiet`, or always with `do_force`.
pub fn print_stdout(msg: &str, do_force: bool) {
    log::debug!("{msg}");
    if do_force || !is_quiet() {
        println!("{msg}");
    }
}

/// `print_stderr`.
pub fn print_stderr(msg: &str) {
    log::debug!("{msg}");
    eprintln!("{msg}");
}

/// `set_prompt`.
pub fn set_prompt(prompt: bool) {
    // Set whether we allow prompts, or fail if a prompt pops up
    if prompt {
        log::warn!("--prompt mode is no longer supported.");
    }
}

/// `convert_old_force`: `--force` is `--check all=off`.
pub fn convert_old_force(force: bool, check: &mut Vec<String>) {
    if force && check.is_empty() {
        check.push("all=off".into());
    }
}

/// `_raw_on_off_convert`.
pub fn raw_on_off_convert(s: &str) -> Option<bool> {
    match s.to_lowercase().as_str() {
        "y" | "yes" | "1" | "true" | "t" | "on" => Some(true),
        "n" | "no" | "0" | "false" | "f" | "off" => Some(false),
        _ => None,
    }
}

/// `parse_check`: `--check path_in_use=off,all=off`.
pub fn parse_check(checks: &[String]) {
    const NAMES: [&str; 5] = [
        "path_in_use",
        "disk_size",
        "path_exists",
        "mac_in_use",
        "all",
    ];
    for optstr in checks {
        let mut leftover = vec![];
        for (key, val) in crate::parse_optstr_tuples(optstr) {
            if !NAMES.contains(&key.as_str()) {
                leftover.push(key);
                continue;
            }
            let Some(val) = val else {
                continue;
            };
            let Some(val) = raw_on_off_convert(&val) else {
                fail(format!("{key} must be 'yes' or 'no'"));
            };
            let mut g = global();
            if key == "all" {
                g.all_checks = Some(val);
            } else {
                g.validation_checks.push((key, val));
            }
        }
        if !leftover.is_empty() {
            let list: Vec<String> = leftover.iter().map(|k| format!("'{k}'")).collect();
            fail(format!("Unknown --check options: [{}]", list.join(", ")));
        }
    }
}

/// `_optional_fail`: fail unless `--check <checkname>=off`.
pub fn optional_fail(msg: &str, checkname: &str, warn_on_skip: bool) {
    if get_validation_check(checkname) {
        fail(format!(
            "{msg} (Use --check {checkname}=off or --check all=off to override)"
        ));
    }
    log::debug!("Skipping --check {checkname} error condition '{msg}'");
    if warn_on_skip {
        log::warn!("{msg}");
    }
}

/// `_openauth_cb`: answer libvirt's credential prompts on the
/// terminal. The `&mut Vec` is `virt`'s `ConnectAuthCallback` type.
#[allow(clippy::ptr_arg)]
fn openauth_cb(creds: &mut Vec<virt::connect::ConnectCredential>) {
    for cred in creds.iter_mut() {
        let noecho = cred.typed == virt::sys::VIR_CRED_PASSPHRASE as i32
            || cred.typed == virt::sys::VIR_CRED_NOECHOPROMPT as i32;
        if cred.prompt.is_empty() {
            log::error!("No prompt for auth credtype={}", cred.typed);
            return;
        }
        log::debug!("openauth_cb prompt={}", cred.prompt);
        let prompt = format!("{}: ", cred.prompt);
        let res = if noecho {
            read_password(&prompt)
        } else {
            print!("{prompt}");
            let _ = std::io::stdout().flush();
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            line.trim_end_matches(['\n', '\r']).to_string()
        };
        cred.result = Some(res);
    }
}

/// `getpass.getpass`: read a line from the terminal without echo.
fn read_password(prompt: &str) -> String {
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    // SAFETY: plain termios calls on stdin, restored before returning.
    let saved = unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(0, &mut t) == 0 {
            let old = t;
            t.c_lflag &= !libc::ECHO;
            libc::tcsetattr(0, libc::TCSANOW, &t);
            Some(old)
        } else {
            None
        }
    };
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    if let Some(old) = saved {
        // SAFETY: restores the attributes read above.
        unsafe {
            libc::tcsetattr(0, libc::TCSANOW, &old);
        }
        eprintln!();
    }
    line.trim_end_matches(['\n', '\r']).to_string()
}

/// `getConnection`.
pub fn get_connection(uri: Option<&str>) -> Connection {
    // `virtinst/__init__.py`'s registerErrorHandler: errors are
    // reported by whoever gets them, not printed by libvirt.
    virt::error::clear_error_callback();
    log::debug!("Requesting libvirt URI {}", uri.unwrap_or("default"));
    match Connection::open_auth(uri, openauth_cb) {
        Ok(conn) => {
            log::debug!(
                "Received libvirt URI {}",
                conn.real_uri().unwrap_or_default()
            );
            conn
        }
        Err(e) => fail(e.message()),
    }
}

/// `validate_mac`: a MAC in use by another VM fails unless
/// `--check mac_in_use=off`.
pub fn validate_mac(conn: &Connection, macaddr: &str) {
    if let Err(e) = virtinst_core::cloner::check_mac_in_use(conn, macaddr) {
        optional_fail(&e, "mac_in_use", true);
    }
}

/// What `validate_disk` needs to know about a new disk.
pub struct DiskCheck<'a> {
    pub path: Option<&'a str>,
    pub shareable: bool,
    pub read_only: bool,
    /// `is_size_conflict()`: `(fatal, message)`.
    pub size_conflict: (bool, Option<String>),
}

/// `validate_disk`: overwriting an existing path, a path other VMs
/// use, not enough space, and paths the qemu user can't reach.
pub fn validate_disk(conn: &Connection, dev: &DiskCheck, warn_overwrite: bool) {
    use virtinst_core::storage;
    let path = dev.path;
    let vconn = conn.as_connect();
    let remote = conn.uri().is_remote();

    // Prompt if disk file already exists and preserve mode is not used
    if warn_overwrite {
        if let Some(p) = path {
            if storage::path_definitely_exists(vconn, p, remote) {
                optional_fail(
                    &format!("This will overwrite the existing path '{p}'"),
                    "path_exists",
                    true,
                );
            }
        }
    }

    // Check if disk is inuse by another guest
    if let Some(p) = path {
        let volmap = storage::get_volmap(vconn);
        let vms = virtinst_core::cloner::all_domain_xmls(conn);
        let names = storage::path_in_use_by(p, &volmap, &vms, dev.shareable, dev.read_only);
        if !names.is_empty() {
            let names: Vec<String> = names.iter().map(|n| format!("'{n}'")).collect();
            optional_fail(
                &format!(
                    "Disk {p} is already in use by other guests [{}].",
                    names.join(", ")
                ),
                "path_in_use",
                true,
            );
        }
    }

    // Check if specified size exceeds available storage
    if let (false, Some(msg)) = &dev.size_conflict {
        optional_fail(msg, "disk_size", false);
    }

    // `check_path_search`
    let search = virtinst_core::diskbackend::check_path_search(vconn, conn.uri(), path);
    if !search.fixlist.is_empty() {
        let dirs: Vec<String> = search.fixlist.iter().map(|d| format!("'{d}'")).collect();
        log::warn!(
            "{} may not be accessible by the hypervisor. You will need to grant the '{}' user \
             search permissions for the following directories: [{}]",
            path.unwrap_or_default(),
            search.user.unwrap_or_default(),
            dirs.join(", ")
        );
    }
}

/// `get_meter`: the text meter, silent with `--quiet`.
pub fn get_meter() -> virtinst_core::progress::TextMeter {
    let quiet = is_quiet() || std::env::var_os("VIRTINST_TEST_SUITE").is_some();
    virtinst_core::progress::make_meter(quiet)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_off_values_match_upstream() {
        for v in ["y", "YES", "1", "true", "t", "on"] {
            assert_eq!(raw_on_off_convert(v), Some(true));
        }
        for v in ["n", "no", "0", "False", "f", "off"] {
            assert_eq!(raw_on_off_convert(v), Some(false));
        }
        assert_eq!(raw_on_off_convert("maybe"), None);
    }

    #[test]
    fn force_turns_into_check_all_off() {
        let mut check = vec![];
        convert_old_force(true, &mut check);
        assert_eq!(check, ["all=off"]);
        let mut check = vec!["path_in_use=off".to_string()];
        convert_old_force(true, &mut check);
        assert_eq!(check, ["path_in_use=off"]);
    }
}
