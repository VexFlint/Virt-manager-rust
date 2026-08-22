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
//! `--remove-device`, for `--watchdog`/`--vsock`/`--disk`/
//! `--memballoon`/`--sound`/`--video` — see `virtinst_cli`'s doc comment
//! for why those devices specifically (`--disk` is upstream's largest
//! and most irregular parser; only a partial slice of it is ported —
//! see `disk_from_optstr`'s own doc comment for exactly which fields),
//! and why their optstring mapping is still hand-written rather than
//! shared. Only the flags this slice actually implements are declared;
//! an unimplemented flag that parses is worse than an absent one.
//! `--memballoon` also can't be exercised through `--add-device` against
//! a domain that already has one — the real driver rejects a second
//! `<memballoon>` outright (see `virtinst_cli`'s
//! `remove_then_restore_memballoon_against_a_real_domain` test); that's
//! a real hardware constraint, not a gap here.
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
//! `--print-xml`, on all three of `--edit`/`--add-device`/
//! `--remove-device` — upstream's `virtxml.py`'s own doc comment on the
//! function backing it says it best: "Perform requested XML edits
//! locally, but don't submit them to libvirt." One `inactive_xml()`
//! read, the same mutation each defining path already applies
//! ([`apply_edit`]/[`list_add`]/[`remove_elements_by_selector`]) run against that
//! parsed copy instead, printed — the real redefine call never happens.
//! Real, not incidental: this is the only way to inspect an action's
//! effect against libvirt's `test://` driver from outside the process
//! that made it — each `test://` connection is its own throwaway
//! in-memory copy of its seed XML, a second `virsh`/process never sees
//! what a first one defined (checked directly, not assumed; this is
//! also upstream's own actual testing architecture, not a workaround —
//! `tests/test_cli.py`'s compare mechanism checks captured stdout via
//! `--print-diff`/`--print-xml`, never a live re-query of the
//! connection, even for its own `--add-device`/`--remove-device`
//! fixtures). `--print-diff` isn't built — needs a real diff algorithm
//! this crate doesn't have a use for yet beyond this one flag.

use clap::Parser;
use virtinst_cli::{
    apply_disk_optstr, apply_memballoon_optstr, apply_sound_optstr, apply_video_optstr,
    apply_vsock_optstr, apply_watchdog_optstr, disk_from_optstr, memballoon_from_optstr,
    remove_elements_by_selector, select_elements_by_selector, sound_from_optstr, video_from_optstr,
    vsock_from_optstr, watchdog_from_optstr,
};
use virtinst_core::connection::{Connection, Domain};
use virtinst_xml::{list_add, parse_libvirt_xml, to_xml_string, Document, Element};

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

    /// Print the resulting XML instead of defining it — the action is
    /// applied to a local, throwaway copy; libvirt never sees it. Works
    /// with --edit/--add-device/--remove-device.
    #[arg(long)]
    print_xml: bool,

    /// Add the device described by --watchdog/--vsock/--disk.
    #[arg(long)]
    add_device: bool,

    /// Remove the device(s) --watchdog/--vsock/--disk selects (index/`-N`/`all`,
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

    /// Disk device options for --build-xml/--edit/--add-device (e.g.
    /// "/path/to/img,bus=virtio,readonly=on"), or the selector for
    /// --remove-device (e.g. "1", "all"). Partial slice of upstream's
    /// real --disk — see disk_from_optstr's doc comment for exactly
    /// which fields.
    #[arg(long)]
    disk: Option<String>,

    /// Memballoon device options for --build-xml/--edit/--add-device
    /// (e.g. "model=none" or bare "none"), or the selector for
    /// --remove-device (e.g. "1", "all").
    #[arg(long)]
    memballoon: Option<String>,

    /// Sound device options for --build-xml/--edit/--add-device (e.g.
    /// "pcspk" or "model=ich9"), or the selector for --remove-device
    /// (e.g. "1", "all").
    #[arg(long)]
    sound: Option<String>,

    /// Video device options for --build-xml/--edit/--add-device (e.g.
    /// "cirrus" or "model.type=virtio,model.heads=2"), or the selector
    /// for --remove-device (e.g. "1", "all").
    #[arg(long)]
    video: Option<String>,
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
/// of `--watchdog`/`--vsock`/`--disk`/`--memballoon` was given (fixed
/// priority, not argv order — `--watchdog` wins if more than one were)
/// and exits with the "nothing implemented for this action" message the
/// caller supplies if none was.
fn device_optstr<'a>(args: &'a Args, action: &str) -> (&'static str, &'a str) {
    if let Some(optstr) = &args.watchdog {
        return ("watchdog", optstr);
    }
    if let Some(optstr) = &args.vsock {
        return ("vsock", optstr);
    }
    if let Some(optstr) = &args.disk {
        return ("disk", optstr);
    }
    if let Some(optstr) = &args.memballoon {
        return ("memballoon", optstr);
    }
    if let Some(optstr) = &args.sound {
        return ("sound", optstr);
    }
    if let Some(optstr) = &args.video {
        return ("video", optstr);
    }
    eprintln!(
        "virt-xml {action}: no device option given (--watchdog/--vsock/--disk/--memballoon/--sound/--video implemented so far)"
    );
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
        "disk" => {
            let dev = disk_from_optstr(optstr).unwrap_or_else(exit_on_err);
            to_xml_string(&dev).expect("a freshly built DeviceDisk always serializes")
        }
        "memballoon" => {
            let dev = memballoon_from_optstr(optstr).unwrap_or_else(exit_on_err);
            to_xml_string(&dev).expect("a freshly built DeviceMemballoon always serializes")
        }
        "sound" => {
            let dev = sound_from_optstr(optstr).unwrap_or_else(exit_on_err);
            to_xml_string(&dev).expect("a freshly built DeviceSound always serializes")
        }
        "video" => {
            let dev = video_from_optstr(optstr).unwrap_or_else(exit_on_err);
            to_xml_string(&dev).expect("a freshly built DeviceVideo always serializes")
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

/// The `inactive_xml()`-fetch-then-parse step every `--print-xml` path
/// needs before it can mutate a local copy — factored out once a third
/// caller ([`run_print_xml_add_device`]/[`run_print_xml_remove_device`],
/// alongside [`run_print_xml_edit`]) needed the exact same two steps.
fn fetch_and_parse_inactive(domain: &virtinst_core::connection::Domain) -> (Document, Element) {
    let xml = domain.inactive_xml().unwrap_or_else(|e| {
        eprintln!("virt-xml: failed to fetch the domain's XML: {e}");
        std::process::exit(1);
    });
    let doc = parse_libvirt_xml(&xml).unwrap_or_else(|e| {
        eprintln!("virt-xml: failed to parse the domain's XML: {e}");
        std::process::exit(1);
    });
    let root = doc
        .root_element()
        .expect("a parsed domain document has a root element");
    (doc, root)
}

/// Every `--print-xml` path ends the same way: serialize and print the
/// locally-mutated copy — nothing was ever defined.
fn print_doc(doc: &Document) {
    let printed = doc
        .write_str()
        .expect("an edited domain document always serializes");
    println!("{printed}");
}

/// The `--print-xml` path: one `inactive_xml()` read, the edit applied
/// to that parsed copy exactly like [`run_edit`]'s defining path (same
/// [`apply_edit`]), printed — `edit_inactive_xml`'s actual redefine call
/// never happens. See this file's own doc comment for why this exists.
fn run_print_xml_edit(args: &Args, device: &str, optstr: &str, selector: &str) {
    let (_conn, domain, _domain_name) = connect_and_lookup(args);
    let (mut doc, root) = fetch_and_parse_inactive(&domain);

    if let Err(e) = apply_edit(&mut doc, root, device, optstr, selector) {
        eprintln!("virt-xml: {e}");
        std::process::exit(1);
    }
    print_doc(&doc);
}

/// [`run_print_xml_edit`]'s `--add-device` counterpart — [`list_add`]
/// straight onto the parsed copy, same primitive
/// [`virtinst_core::connection::Domain::add_device`]'s defining path
/// uses internally.
fn run_print_xml_add_device(args: &Args, device: &str, optstr: &str) {
    let (_conn, domain, _domain_name) = connect_and_lookup(args);
    let (mut doc, root) = fetch_and_parse_inactive(&domain);

    match device {
        "watchdog" => {
            let dev = watchdog_from_optstr(optstr).unwrap_or_else(exit_on_err);
            list_add(&mut doc, root, &["devices"], &dev);
        }
        "vsock" => {
            let dev = vsock_from_optstr(optstr).unwrap_or_else(exit_on_err);
            list_add(&mut doc, root, &["devices"], &dev);
        }
        "disk" => {
            let dev = disk_from_optstr(optstr).unwrap_or_else(exit_on_err);
            list_add(&mut doc, root, &["devices"], &dev);
        }
        "memballoon" => {
            let dev = memballoon_from_optstr(optstr).unwrap_or_else(exit_on_err);
            list_add(&mut doc, root, &["devices"], &dev);
        }
        "sound" => {
            let dev = sound_from_optstr(optstr).unwrap_or_else(exit_on_err);
            list_add(&mut doc, root, &["devices"], &dev);
        }
        "video" => {
            let dev = video_from_optstr(optstr).unwrap_or_else(exit_on_err);
            list_add(&mut doc, root, &["devices"], &dev);
        }
        _ => unreachable!(),
    }
    print_doc(&doc);
}

/// [`run_print_xml_edit`]'s `--remove-device` counterpart — same
/// [`remove_elements_by_selector`] the defining path uses, applied to the parsed
/// copy instead of a document `edit_inactive_xml` will redefine.
fn run_print_xml_remove_device(args: &Args, device: &str, selector: &str) {
    let (_conn, domain, _domain_name) = connect_and_lookup(args);
    let (mut doc, root) = fetch_and_parse_inactive(&domain);

    if let Err(e) = remove_elements_by_selector(&mut doc, root, device, "remove-device", selector) {
        eprintln!("virt-xml: {e}");
        std::process::exit(1);
    }
    print_doc(&doc);
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
    let els = select_elements_by_selector(doc, root, device, "edit", selector)?;
    for el in els {
        match device {
            "watchdog" => apply_watchdog_optstr(doc, el, optstr)?,
            "vsock" => apply_vsock_optstr(doc, el, optstr)?,
            "disk" => apply_disk_optstr(doc, el, optstr)?,
            "memballoon" => apply_memballoon_optstr(doc, el, optstr)?,
            "sound" => apply_sound_optstr(doc, el, optstr)?,
            "video" => apply_video_optstr(doc, el, optstr)?,
            _ => unreachable!(),
        }
    }
    Ok(())
}

fn run_add_device(args: &Args) {
    let (device, optstr) = device_optstr(args, "--add-device");

    if args.print_xml {
        run_print_xml_add_device(args, device, optstr);
        return;
    }

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
        "disk" => domain.add_device(&conn, &disk_from_optstr(optstr).unwrap_or_else(exit_on_err)),
        "memballoon" => domain.add_device(
            &conn,
            &memballoon_from_optstr(optstr).unwrap_or_else(exit_on_err),
        ),
        "sound" => domain.add_device(
            &conn,
            &sound_from_optstr(optstr).unwrap_or_else(exit_on_err),
        ),
        "video" => domain.add_device(
            &conn,
            &video_from_optstr(optstr).unwrap_or_else(exit_on_err),
        ),
        _ => unreachable!(),
    };
    report(&domain_name, result);
}

fn run_remove_device(args: &Args) {
    let (device, selector) = device_optstr(args, "--remove-device");

    if args.print_xml {
        run_print_xml_remove_device(args, device, selector);
        return;
    }

    let (conn, domain, domain_name) = connect_and_lookup(args);
    let result = domain.edit_inactive_xml(&conn, |doc, root| {
        remove_elements_by_selector(doc, root, device, "remove-device", selector)?;
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
