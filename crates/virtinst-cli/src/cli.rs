//! The shared CLI plumbing from upstream's `cli.py`: logging
//! (`setupLogging`), `fail`/`print_stdout`, the `--check` validation
//! switches (`ParserCheck`, `_optional_fail`), the libvirt connection
//! with interactive auth (`getConnection`), and the disk/MAC sanity
//! checks shared by the tools (`validate_disk`, `validate_mac`), and
//! bash completion (`autocomplete`, argcomplete's protocol).

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

/// Parses the command line like argparse does for the upstream tools:
/// unambiguous prefixes of long options are accepted (`--dry` for
/// `--dry-run`) and errors use argparse's `usage:`/`prog: error:` format
/// with exit code 2.
pub fn argparse_parse<T: clap::Parser + clap::CommandFactory>(prog: &str, usage: &str) -> T {
    let cmd = T::command();
    let longs: Vec<String> = cmd
        .get_arguments()
        .flat_map(|a| {
            a.get_long()
                .into_iter()
                .chain(a.get_all_aliases().unwrap_or_default())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .chain(["help".to_string(), "version".to_string()])
        .collect();
    let argparse_fail = |msg: String| -> ! {
        eprintln!("usage: {prog} {usage}");
        eprintln!("{prog}: error: {msg}");
        std::process::exit(2);
    };

    let mut argv: Vec<String> = Vec::new();
    let mut rest = false;
    for arg in std::env::args() {
        if rest || argv.is_empty() || !arg.starts_with("--") || arg == "--" {
            rest |= arg == "--";
            argv.push(arg);
            continue;
        }
        let (name, value) = match arg[2..].split_once('=') {
            Some((n, v)) => (n.to_string(), Some(v.to_string())),
            None => (arg[2..].to_string(), None),
        };
        let full = if longs.contains(&name) {
            name
        } else {
            let matches: Vec<&String> = longs.iter().filter(|l| l.starts_with(&name)).collect();
            match matches.as_slice() {
                [one] => one.to_string(),
                [] => name,
                many => {
                    let list: Vec<String> = many.iter().map(|m| format!("--{m}")).collect();
                    argparse_fail(format!(
                        "ambiguous option: --{name} could match {}",
                        list.join(", ")
                    ))
                }
            }
        };
        argv.push(match value {
            Some(v) => format!("--{full}={v}"),
            None => format!("--{full}"),
        });
    }

    T::try_parse_from(argv).unwrap_or_else(|e| {
        use clap::error::{ContextKind, ContextValue, ErrorKind};
        match e.kind() {
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => e.exit(),
            ErrorKind::UnknownArgument => {
                let arg = match e.get(ContextKind::InvalidArg) {
                    Some(ContextValue::String(s)) => s.clone(),
                    _ => String::new(),
                };
                argparse_fail(format!("unrecognized arguments: {arg}"))
            }
            _ => {
                let msg = e.to_string();
                let first = msg.lines().next().unwrap_or_default();
                argparse_fail(first.trim_start_matches("error: ").to_string())
            }
        }
    })
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

/// `ParserCheck`'s sub-options.
pub const CHECK_OPTIONS: [&str; 5] = [
    "path_in_use",
    "disk_size",
    "path_exists",
    "mac_in_use",
    "all",
];

/// `parse_check`: `--check path_in_use=off,all=off`.
pub fn parse_check(checks: &[String]) {
    for optstr in checks {
        let mut leftover = vec![];
        for (key, val) in crate::parser::parse_optstr_tuples(optstr) {
            if !CHECK_OPTIONS.contains(&key.as_str()) {
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
    let open_uri = uri.map(|u| match u.strip_prefix(MAGIC_URI_PREFIX) {
        Some(magic) => apply_magic_uri(magic).unwrap_or_else(|e| fail(e)),
        None => u.to_string(),
    });
    match Connection::open_auth(open_uri.as_deref(), openauth_cb) {
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

/// `MagicURI.VIRTINST_URI_MAGIC_PREFIX`.
const MAGIC_URI_PREFIX: &str = "__virtinst_test__";

/// `MagicURI`: upstream's test-suite URIs, `__virtinst_test__` then an
/// openable URI and comma separated options (`predictable`,
/// `fakeuri=`, `connver=`, `libver=`, `caps=`, `domcaps=`). Each
/// option becomes the `VIRTINST_TEST_*` variable the libvirt wrappers
/// already read; returns the URI to really open.
fn apply_magic_uri(uri: &str) -> Result<String, String> {
    let (open_uri, opts) = uri.split_once(',').unwrap_or((uri, ""));
    let mut opts: Vec<(String, Option<String>)> = crate::parser::parse_optstr_tuples(opts);
    let mut pop = |field: &str| {
        let idx = opts.iter().position(|(k, _)| k == field)?;
        Some(opts.remove(idx).1)
    };
    let predictable = pop("predictable").is_some();
    let fakeuri = pop("fakeuri").flatten();
    let capsfile = pop("caps").flatten();
    let domcapsfile = pop("domcaps").flatten();
    let mut conn_version = pop("connver").flatten();
    if conn_version.is_none() && fakeuri.is_some() {
        conn_version = Some("10000000000".into());
    }
    let libvirt_version = pop("libver").flatten();
    if !opts.is_empty() {
        let opts: Vec<String> = opts
            .iter()
            .map(|(k, v)| match v {
                Some(v) => format!("'{k}': '{v}'"),
                None => format!("'{k}': None"),
            })
            .collect();
        return Err(format!(
            "MagicURI has unhandled opts={{{}}}",
            opts.join(", ")
        ));
    }
    let vars = [
        (
            "VIRTINST_TEST_PREDICTABLE",
            predictable.then(|| "1".to_string()),
        ),
        ("VIRTINST_TEST_FAKE_URI", fakeuri),
        ("VIRTINST_TEST_CAPS_FILE", capsfile),
        (
            "VIRTINST_TEST_DOMCAPS_NOSUPPORT",
            domcapsfile.is_none().then(|| "1".to_string()),
        ),
        ("VIRTINST_TEST_DOMCAPS_FILE", domcapsfile),
        ("VIRTINST_TEST_CONN_VERSION", conn_version),
        ("VIRTINST_TEST_LIBVIRT_VERSION", libvirt_version),
    ];
    for (name, val) in vars {
        if let Some(val) = val {
            // Only ever set here, before any other thread runs.
            std::env::set_var(name, val);
        }
    }
    Ok(open_uri.to_string())
}

/// `conn.local_libvirt_version()`, faked by a magic URI's `libver=`.
fn libvirt_version(conn: &Connection) -> u64 {
    std::env::var("VIRTINST_TEST_LIBVIRT_VERSION")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| conn.as_connect().get_lib_version().unwrap_or(0) as u64)
}

/// `conn.conn_version()`, faked by a magic URI's `connver=`.
fn conn_version(conn: &Connection) -> u64 {
    std::env::var("VIRTINST_TEST_CONN_VERSION")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| conn.as_connect().get_hyp_version().unwrap_or(0) as u64)
}

/// The `conn.support` checks the parsers read: `conn_usb3_ports`
/// (libvirt 1.3.5) and `conn_qemu_xhci` (libvirt 3.3.0, qemu 2.9.0).
pub fn conn_support(conn: &Connection) -> crate::parser::ConnSupport {
    let libvirt = libvirt_version(conn);
    let hv = conn_version(conn);
    crate::parser::ConnSupport {
        usb3_ports: libvirt >= 1_003_005,
        qemu_xhci: conn.uri().is_qemu() && libvirt >= 3_003_000 && hv >= 2_009_000,
        is_xen: conn.uri().is_xen(),
        is_test: conn.uri().is_test(),
        // `is_privileged`: `/embed` needs root, anything else but
        // `/session` is privileged.
        is_qemu_unprivileged: conn.uri().is_qemu()
            && match conn.uri().path.as_str() {
                "/session" => true,
                // SAFETY: getuid has no preconditions.
                "/embed" => (unsafe { libc::getuid() }) != 0,
                _ => false,
            },
        is_remote: conn.uri().is_remote(),
        vmport: libvirt >= 1_002_016
            && (conn.uri().is_test() || (conn.uri().is_qemu() && hv >= 2_002_000)),
        graphics_listen_none: libvirt >= 2_000_000,
        is_qemu: conn.uri().is_qemu(),
        is_bhyve: conn.uri().is_bhyve(),
        disk_driver_name_qemu: conn.uri().is_qemu()
            || (conn.uri().is_xen() && libvirt >= 1_001_000 && hv >= 4_002_000),
        default_qcow2: conn.uri().is_test() || (conn.uri().is_qemu() && hv >= 1_002_000),
        is_session: conn.uri().path == "/session",
    }
}

/// `conn.fetch_all_nodedevs()`.
pub fn fetch_all_nodedevs(conn: &Connection) -> Vec<virtinst_core::nodedev::NodeDevice> {
    let devs = conn
        .as_connect()
        .list_all_node_devices(0)
        .unwrap_or_default();
    devs.iter()
        .filter_map(|d| d.get_xml_desc(0).ok())
        .filter_map(|xml| virtinst_core::nodedev::NodeDevice::parse(&xml))
        .collect()
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
        let names = storage::path_in_use_by_conn(vconn, p, dev.shareable, dev.read_only)
            .unwrap_or_default();
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

    check_path_search_opt(conn, path);
}

/// `check_path_search`: warn when the hypervisor user can't reach
/// `path`.
pub fn check_path_search(conn: &Connection, path: &str) {
    check_path_search_opt(conn, Some(path));
}

fn check_path_search_opt(conn: &Connection, path: Option<&str>) {
    let search = virtinst_core::diskbackend::check_path_search(conn.as_connect(), conn.uri(), path);
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

// ---------------------------------------------------------------------
// Bash completion helpers: upstream's `autocomplete`, which hands the
// parser to argcomplete. The same protocol, so upstream's own
// completion script (`data/bash-completion.sh.in`, shipped here as
// `crates/virtinst-cli/data/bash-completion/`) drives these tools: the
// script runs the tool with `_ARGCOMPLETE=1` and the line in
// `COMP_LINE`/`COMP_POINT`, and reads the completions back from fd 8.
// ---------------------------------------------------------------------

/// Upstream's `_virtparser_completer`: the sub-options of a parser
/// option (`name=`), minus the ones already typed in `prefix`.
fn virtparser_completer(names: &[&str], prefix: &str) -> Vec<String> {
    let mut names = names.to_vec();
    names.sort_unstable();
    let mut sub_options: Vec<String> = names.iter().map(|n| format!("{n}=")).collect();
    for option in prefix.split(',') {
        if let Some(pos) = option.find('=').filter(|p| *p > 0) {
            sub_options.retain(|s| *s != option[..=pos]);
        }
    }
    sub_options
}

/// Upstream's `_completer_validator`: `--disk path=foo,de<TAB>` offers
/// the sub-options starting with `de`.
fn completer_validator(suboption: &str, current_input: &str) -> bool {
    if current_input.is_empty() {
        return true;
    }
    let current_option = current_input.rsplit(',').next().unwrap_or_default();
    suboption.starts_with(current_option)
}

/// argcomplete's `FilesCompleter`: the files and directories (with a
/// trailing `/`) that start with `prefix`.
fn files_completer(prefix: &str) -> Vec<String> {
    let (dir, base) = match prefix.rfind('/') {
        Some(i) => (&prefix[..=i], &prefix[i + 1..]),
        None => ("", prefix),
    };
    let read = if dir.is_empty() { "." } else { dir };
    let mut ret: Vec<String> = std::fs::read_dir(read)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            if !name.starts_with(base) || (name.starts_with('.') && !base.starts_with('.')) {
                return None;
            }
            let slash = if e.path().is_dir() { "/" } else { "" };
            Some(format!("{dir}{name}{slash}"))
        })
        .collect();
    ret.sort();
    ret
}

/// Splits the line up to the cursor into the finished words and the
/// word being completed (argcomplete's `split_line`): shell quoting,
/// and an unterminated quote is still a word.
fn split_line(line: &str) -> (Vec<String>, String) {
    let mut words = vec![];
    let mut cur = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) if c == q => quote = None,
            Some('"') if c == '\\' => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            Some(_) => cur.push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                in_word = true;
            }
            None if c == '\\' => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
                in_word = true;
            }
            None if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            None => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    (words, cur)
}

/// The completions for the line up to the cursor: options that start
/// with the word, and the value of the option before it (its parser's
/// sub-options from `parsers`, else files).
pub fn get_completions(
    cmd: &clap::Command,
    parsers: &[(&str, &[&str])],
    line: &str,
) -> Vec<String> {
    let (words, prefix) = split_line(line);
    let visible: Vec<&clap::Arg> = cmd
        .get_arguments()
        .filter(|a| !a.is_hide_set() && !a.is_positional())
        .collect();
    let option_strings = |a: &clap::Arg| -> Vec<String> {
        let mut v = vec![];
        if let Some(s) = a.get_short() {
            v.push(format!("-{s}"));
        }
        if let Some(l) = a.get_long() {
            v.push(format!("--{l}"));
        }
        v
    };
    // The option whose value is being typed: the previous word, with
    // argparse's prefix matching, or `--opt=` in this word.
    let find = |word: &str| -> Option<&clap::Arg> {
        let all: Vec<&clap::Arg> = cmd.get_arguments().collect();
        if let Some(a) = all
            .iter()
            .find(|a| option_strings(a).iter().any(|o| o == word))
        {
            return Some(a);
        }
        let long = word.strip_prefix("--")?;
        let matches: Vec<&&clap::Arg> = all
            .iter()
            .filter(|a| a.get_long().is_some_and(|l| l.starts_with(long)))
            .collect();
        (matches.len() == 1).then(|| *matches[0])
    };
    let takes_value = |a: &clap::Arg| a.get_action().takes_values();

    let (active, value_prefix) = match prefix.split_once('=') {
        Some((opt, val)) if prefix.starts_with("--") => (find(opt), val.to_string()),
        _ => (
            words
                .last()
                .filter(|w| w.starts_with('-') && !w.contains('='))
                .and_then(|w| find(w))
                .filter(|a| takes_value(a)),
            prefix.clone(),
        ),
    };

    let mut completions = vec![];
    if !(prefix.starts_with("--") && prefix.contains('=')) {
        for a in &visible {
            for o in option_strings(a) {
                if o.starts_with(&prefix) {
                    completions.push(o);
                }
            }
        }
    }
    if let Some(a) = active {
        let flags = option_strings(a);
        let parser = parsers
            .iter()
            .find(|(flag, _)| flags.iter().any(|f| f == flag));
        let values = match parser {
            Some((_, names)) => virtparser_completer(names, &value_prefix)
                .into_iter()
                .filter(|s| completer_validator(s, &value_prefix))
                .collect(),
            None => files_completer(&value_prefix),
        };
        completions.extend(values);
    }
    let mut seen = std::collections::HashSet::new();
    completions.retain(|c| seen.insert(c.clone()));
    completions
}

/// `autocomplete(parser)`: when bash's completion script runs the tool
/// (`_ARGCOMPLETE` set), print the completions for `COMP_LINE` and
/// exit; otherwise do nothing. `parsers` are the options with
/// sub-options (`--check` and friends) and their names.
pub fn autocomplete(cmd: clap::Command, parsers: &[(&str, &[&str])]) {
    if std::env::var_os("_ARGCOMPLETE").is_none() {
        return;
    }
    let mut cmd = cmd;
    cmd.build();
    let line = std::env::var("COMP_LINE").unwrap_or_default();
    let point = std::env::var("COMP_POINT")
        .ok()
        .and_then(|p| p.parse::<usize>().ok())
        .unwrap_or(line.len());
    // COMP_POINT counts bytes.
    let mut end = point.min(line.len());
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    let mut completions = get_completions(&cmd, parsers, &line[..end]);

    // argcomplete's append_space: bash adds the space itself unless the
    // script couldn't turn `nospace` off.
    let append_space = std::env::var("_ARGCOMPLETE_SUPPRESS_SPACE").as_deref() != Ok("1");
    if append_space && completions.len() == 1 && !completions[0].ends_with(['=', '/', ':']) {
        completions[0].push(' ');
    }
    let ifs = std::env::var("IFS").unwrap_or_else(|_| "\u{b}".into());
    let out = completions.join(&ifs);

    if std::env::var_os("VIRTINST_TEST_SUITE").is_some() {
        println!("{out}");
    } else {
        use std::os::fd::FromRawFd;
        // SAFETY: fd 8 is the script's stdout (`8>&1`); if it isn't
        // open the write just fails.
        let mut f = unsafe { std::fs::File::from_raw_fd(8) };
        let _ = f.write_all(out.as_bytes());
    }
    std::process::exit(0);
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

    fn clone_like_cmd() -> clap::Command {
        clap::Command::new("virt-clone")
            .arg(
                clap::Arg::new("preserve")
                    .long("preserve-data")
                    .action(clap::ArgAction::SetTrue),
            )
            .arg(clap::Arg::new("orig").short('o').long("original"))
            .arg(
                clap::Arg::new("check")
                    .long("check")
                    .action(clap::ArgAction::Append),
            )
            .arg(
                clap::Arg::new("nodry")
                    .long("__test-nodry")
                    .hide(true)
                    .action(clap::ArgAction::SetTrue),
            )
    }

    /// Upstream's `_add_argcomplete_cmd` cases that apply here.
    #[test]
    fn completes_options_and_check_suboptions_like_upstream() {
        let mut cmd = clone_like_cmd();
        cmd.build();
        let parsers: &[(&str, &[&str])] = &[("--check", &CHECK_OPTIONS)];
        let c = |line: &str| get_completions(&cmd, parsers, line);
        // "virt-clone --preserve" -> "--preserve-data"
        assert_eq!(c("virt-clone --preserve"), ["--preserve-data"]);
        // "virt-install --check d" -> "disk_size"
        assert_eq!(c("virt-clone --check d"), ["disk_size="]);
        // "virt-install --disk " lists every sub-option (options too,
        // argcomplete's always_complete_options).
        let all = c("virt-clone --check ");
        for want in [
            "all=",
            "disk_size=",
            "mac_in_use=",
            "path_exists=",
            "path_in_use=",
        ] {
            assert!(all.contains(&want.to_string()), "{all:?}");
        }
        assert!(all.contains(&"--preserve-data".to_string()));
        assert_eq!(c("virt-clone --check path_in_use=off,pa"), ["path_exists="]);
        assert_eq!(c("virt-clone --chec all=off,d"), ["disk_size="]);
        assert_eq!(c("virt-clone --check=m"), ["mac_in_use="]);
        // Hidden options are never offered.
        assert!(c("virt-clone --__").is_empty());
    }

    #[test]
    fn splits_the_line_like_a_shell() {
        assert_eq!(
            split_line("virt-clone --file 'a b' --name \"x"),
            (
                vec![
                    "virt-clone".into(),
                    "--file".into(),
                    "a b".into(),
                    "--name".into()
                ],
                "x".to_string()
            )
        );
        assert_eq!(
            split_line("virt-clone --check "),
            (vec!["virt-clone".into(), "--check".into()], String::new())
        );
    }
}
