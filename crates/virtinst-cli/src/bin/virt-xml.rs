//! `virt-xml` — `--edit`/`--add-device`/`--remove-device`/`--build-xml`
//! (the last works standalone, no connection needed). `--edit` is the
//! first real end-to-end consumer of ticket 06's order-preserving DOM —
//! the exact scenario it was built for: locate an existing element and
//! mutate only the attributes named on the command line, proven (see
//! `virtinst_cli`'s test suite) to leave both the untouched attributes
//! on that same element *and* a completely foreign `<qemu:commandline>`
//! block elsewhere in the document intact. That's ticket 03's bar at
//! the content level — not literally byte-for-byte, since `edit_xml`'s
//! own attribute storage (a `HashMap`) doesn't guarantee stable
//! serialization order across process runs; see
//! `watchdog_build_xml_output_matches_expected_shape`'s doc comment in
//! `virtinst-cli` for where that was found.
//!
//! This slice covers `--build-xml`/`--edit`/`--add-device`/
//! `--remove-device`, for `--watchdog`/`--vsock` — see `virtinst_cli`'s
//! doc comment for why those two devices specifically, and why their
//! optstring mapping is still hand-written rather than shared. Only the
//! flags this slice actually implements are declared; an unimplemented
//! flag that parses is worse than an absent one.
//!
//! `--add-device`/`--edit`'s output contract is also deliberately
//! narrower than upstream for now: this always defines the change
//! (upstream's default when a domain is given and no
//! `--print-xml`/`--no-define` is passed), with no `--update` (live
//! hotplug alongside the persistent define) support yet — upstream
//! itself only hotplugs when `--update` is explicitly passed, so a
//! running domain being edited/added-to here still behaves like
//! upstream's own default, just without the opt-in extra.

use clap::Parser;
use virtinst_cli::{
    apply_vsock_optstr, apply_watchdog_optstr, remove_elements, select_elements, vsock_from_optstr,
    watchdog_from_optstr,
};
use virtinst_core::connection::{Connection, Domain};
use virtinst_xml::to_xml_string;

#[derive(Parser)]
#[command(name = "virt-xml")]
struct Args {
    /// Domain name — required for --edit/--add-device/--remove-device,
    /// unused (and not required) for --build-xml.
    domain: Option<String>,

    /// Connect to a non-default hypervisor.
    #[arg(short, long, default_value = "qemu:///system")]
    connect: String,

    /// Just build the requested XML block and print it. No domain
    /// connection required.
    #[arg(long)]
    build_xml: bool,

    /// Edit the specified XML block. Bare `--edit` means the first
    /// object (upstream's index-1 default); `--edit N`/`--edit -N`
    /// selects by position; `--edit all` selects every object.
    #[arg(long, num_args = 0..=1, default_missing_value = "1")]
    edit: Option<String>,

    /// Add the device described by --watchdog/--vsock.
    #[arg(long)]
    add_device: bool,

    /// Remove the device(s) --watchdog/--vsock selects (index/`-N`/`all`,
    /// same selector syntax as --edit).
    #[arg(long)]
    remove_device: bool,

    /// Watchdog device options for --build-xml/--edit/--add-device
    /// (e.g. "model=itco,action=none"), or the selector for
    /// --remove-device (e.g. "1", "all").
    #[arg(long)]
    watchdog: Option<String>,

    /// Vsock device options for --build-xml/--edit/--add-device (e.g.
    /// "cid.auto=no,cid.address=3"), or the selector for
    /// --remove-device (e.g. "1", "all").
    #[arg(long)]
    vsock: Option<String>,
}

fn main() {
    let args = Args::parse();

    if args.build_xml {
        run_build_xml(&args);
        return;
    }
    if let Some(selector) = args.edit.clone() {
        run_edit(&args, &selector);
        return;
    }
    if args.add_device {
        run_add_device(&args);
        return;
    }
    if args.remove_device {
        run_remove_device(&args);
        return;
    }

    eprintln!(
        "virt-xml: only --build-xml, --edit, --add-device, and --remove-device are implemented so far"
    );
    std::process::exit(1);
}

/// Every action below needs exactly one device flag; this picks whichever
/// of `--watchdog`/`--vsock` was given (fixed priority, not argv order —
/// `--watchdog` wins if both were) and exits with the "nothing
/// implemented for this action" message the caller supplies if neither
/// was.
fn device_optstr<'a>(args: &'a Args, action: &str) -> (&'static str, &'a str) {
    if let Some(optstr) = &args.watchdog {
        return ("watchdog", optstr);
    }
    if let Some(optstr) = &args.vsock {
        return ("vsock", optstr);
    }
    eprintln!("virt-xml {action}: no device option given (--watchdog/--vsock implemented so far)");
    std::process::exit(1);
}

/// Opens `--connect`'s URI and looks up `domain` — the boilerplate every
/// action but `--build-xml` needs. Exits the process on any failure
/// (bad URI, no such domain, no domain name given at all) rather than
/// threading a `Result` through `main`, matching this binary's existing
/// error-reporting style.
fn connect_and_lookup(args: &Args) -> (Connection, Domain, String) {
    let Some(domain_name) = &args.domain else {
        eprintln!("virt-xml: a domain name is required");
        std::process::exit(1);
    };

    let conn = Connection::open(&args.connect).unwrap_or_else(|e| {
        eprintln!("virt-xml: failed to connect to {}: {e}", args.connect);
        std::process::exit(1);
    });
    let domain = Domain::lookup_by_name(&conn, domain_name).unwrap_or_else(|e| {
        eprintln!("virt-xml: failed to find domain '{domain_name}': {e}");
        std::process::exit(1);
    });
    (conn, domain, domain_name.clone())
}

fn run_build_xml(args: &Args) {
    let (device, optstr) = device_optstr(args, "--build-xml");

    let xml = match device {
        "watchdog" => to_xml_string(&watchdog_from_optstr(optstr))
            .expect("a freshly built DeviceWatchdog always serializes"),
        "vsock" => to_xml_string(&vsock_from_optstr(optstr))
            .expect("a freshly built DeviceVsock always serializes"),
        _ => unreachable!(),
    };
    println!("{xml}");
}

fn run_edit(args: &Args, selector: &str) {
    let (device, optstr) = device_optstr(args, "--edit");
    let (conn, domain, domain_name) = connect_and_lookup(args);

    let result = domain.edit_inactive_xml(&conn, |doc, root| {
        let els = select_elements(doc, root, device, selector)?;
        for el in els {
            match device {
                "watchdog" => apply_watchdog_optstr(doc, el, optstr),
                "vsock" => apply_vsock_optstr(doc, el, optstr),
                _ => unreachable!(),
            }
        }
        Ok(())
    });
    report(&domain_name, result);
}

fn run_add_device(args: &Args) {
    let (device, optstr) = device_optstr(args, "--add-device");
    let (conn, domain, domain_name) = connect_and_lookup(args);

    let result = match device {
        "watchdog" => domain.add_device(&conn, &watchdog_from_optstr(optstr)),
        "vsock" => domain.add_device(&conn, &vsock_from_optstr(optstr)),
        _ => unreachable!(),
    };
    report(&domain_name, result);
}

fn run_remove_device(args: &Args) {
    let (device, selector) = device_optstr(args, "--remove-device");
    let (conn, domain, domain_name) = connect_and_lookup(args);

    let result = domain.edit_inactive_xml(&conn, |doc, root| {
        remove_elements(doc, root, device, selector)?;
        Ok(())
    });
    report(&domain_name, result);
}

/// Both actions above end the same way: define succeeded or it didn't.
fn report<E: std::fmt::Display>(domain_name: &str, result: Result<(), E>) {
    match result {
        Ok(()) => println!("Domain '{domain_name}' defined successfully."),
        Err(e) => {
            eprintln!("virt-xml: {e}");
            std::process::exit(1);
        }
    }
}
