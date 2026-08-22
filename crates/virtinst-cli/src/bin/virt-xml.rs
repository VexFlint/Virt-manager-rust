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
//! narrower than upstream for now: absent `--print-xml`, this always
//! defines the change (upstream's default when a domain is given and
//! no `--print-xml`/`--no-define` is passed), with no `--update` (live
//! hotplug alongside the persistent define) support yet — upstream
//! itself only hotplugs when `--update` is explicitly passed, so a
//! running domain being edited/added-to here still behaves like
//! upstream's own default, just without the opt-in extra.
//!
//! `--print-xml` on `--edit`, the one case built so far — upstream's
//! `virtxml.py`'s own doc comment on the function backing it says it
//! best: "Perform requested XML edits locally, but don't submit them to
//! libvirt." One `inactive_xml()` read, the edit applied to that parsed
//! copy same as the defining path, printed, `edit_inactive_xml`'s
//! actual redefine call never happens. Real, not incidental: this is
//! the only way to inspect an edit's effect against libvirt's `test://`
//! driver from outside the process that made it — each `test://`
//! connection is its own throwaway in-memory copy of its seed XML, a
//! second `virsh`/process never sees what a first one defined (checked
//! directly, not assumed). `--print-diff` isn't built — needs a real
//! diff algorithm this crate doesn't have a use for yet beyond this one
//! flag.

use clap::Parser;
use virtinst_cli::{
    apply_vsock_optstr, apply_watchdog_optstr, remove_elements, select_elements, vsock_from_optstr,
    watchdog_from_optstr,
};
use virtinst_core::connection::{Connection, Domain};
use virtinst_xml::{parse_libvirt_xml, to_xml_string};

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

    /// Print the edited XML instead of defining it — the edit is
    /// applied to a local, throwaway copy; libvirt never sees it.
    /// `--edit` only so far.
    #[arg(long)]
    print_xml: bool,

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
        "watchdog" => {
            let dev = watchdog_from_optstr(optstr).unwrap_or_else(exit_on_err);
            to_xml_string(&dev).expect("a freshly built DeviceWatchdog always serializes")
        }
        "vsock" => {
            let dev = vsock_from_optstr(optstr).unwrap_or_else(exit_on_err);
            to_xml_string(&dev).expect("a freshly built DeviceVsock always serializes")
        }
        _ => unreachable!(),
    };
    println!("{xml}");
}

/// Shared exit path for a device optstring's own parse errors (e.g.
/// `--vsock cid.auto=bogus`) — distinct from [`report`], which handles
/// the libvirt-define outcome after a successful parse.
fn exit_on_err<T>(e: String) -> T {
    eprintln!("virt-xml: {e}");
    std::process::exit(1);
}

fn run_edit(args: &Args, selector: &str) {
    let (device, optstr) = device_optstr(args, "--edit");

    if args.print_xml {
        run_print_xml_edit(args, device, optstr, selector);
        return;
    }

    let (conn, domain, domain_name) = connect_and_lookup(args);
    let result = domain.edit_inactive_xml(&conn, |doc, root| {
        apply_edit(doc, root, device, optstr, selector)
    });
    report(&domain_name, result);
}

/// The `--print-xml` path: one `inactive_xml()` read, the edit applied
/// to that parsed copy exactly like [`run_edit`]'s defining path (same
/// [`apply_edit`]), printed — `edit_inactive_xml`'s actual redefine call
/// never happens. See this file's own doc comment for why this exists.
fn run_print_xml_edit(args: &Args, device: &str, optstr: &str, selector: &str) {
    let (_conn, domain, _domain_name) = connect_and_lookup(args);
    let xml = domain.inactive_xml().unwrap_or_else(|e| {
        eprintln!("virt-xml: failed to fetch the domain's XML: {e}");
        std::process::exit(1);
    });
    let mut doc = parse_libvirt_xml(&xml).unwrap_or_else(|e| {
        eprintln!("virt-xml: failed to parse the domain's XML: {e}");
        std::process::exit(1);
    });
    let root = doc
        .root_element()
        .expect("a parsed domain document has a root element");

    if let Err(e) = apply_edit(&mut doc, root, device, optstr, selector) {
        eprintln!("virt-xml: {e}");
        std::process::exit(1);
    }

    let printed = doc
        .write_str()
        .expect("an edited domain document always serializes");
    println!("{printed}");
}

/// The actual edit, shared between the defining path
/// ([`Domain::edit_inactive_xml`]'s closure) and the local-only
/// `--print-xml` path — both need the exact same select-then-apply
/// sequence against an already-parsed `Document`, just with a different
/// caller deciding what happens to the result afterward.
fn apply_edit(
    doc: &mut virtinst_xml::Document,
    root: virtinst_xml::Element,
    device: &str,
    optstr: &str,
    selector: &str,
) -> Result<(), String> {
    let els = select_elements(doc, root, device, selector)?;
    for el in els {
        match device {
            "watchdog" => apply_watchdog_optstr(doc, el, optstr)?,
            "vsock" => apply_vsock_optstr(doc, el, optstr)?,
            _ => unreachable!(),
        }
    }
    Ok(())
}

fn run_add_device(args: &Args) {
    let (device, optstr) = device_optstr(args, "--add-device");
    let (conn, domain, domain_name) = connect_and_lookup(args);

    let result = match device {
        "watchdog" => domain.add_device(
            &conn,
            &watchdog_from_optstr(optstr).unwrap_or_else(exit_on_err),
        ),
        "vsock" => domain.add_device(
            &conn,
            &vsock_from_optstr(optstr).unwrap_or_else(exit_on_err),
        ),
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
