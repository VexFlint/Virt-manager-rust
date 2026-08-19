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
//! This slice covers `--build-xml --watchdog <optstr>` and
//! `--edit [SELECTOR] --watchdog <optstr>` against a real domain.
//! `--add-device`/`--remove-device` and every device besides
//! `--watchdog` aren't built yet — see `virtinst_cli`'s doc comment for
//! why `--watchdog` specifically. Only the flags this slice actually
//! implements are declared; an unimplemented flag that parses is worse
//! than an absent one.
//!
//! `--edit`'s output contract is also deliberately narrower than
//! upstream for now: this always defines the change (upstream's
//! default when a domain is given and no `--print-xml`/`--no-define`
//! is passed) — the `--print-xml`/`--print-diff`/confirm-prompt paths
//! aren't ported yet.

use clap::Parser;
use virtinst_cli::{apply_watchdog_optstr, select_watchdog_elements, watchdog_from_optstr};
use virtinst_core::connection::{Connection, Domain};
use virtinst_xml::to_xml_string;

#[derive(Parser)]
#[command(name = "virt-xml")]
struct Args {
    /// Domain name — required for --edit, unused (and not required)
    /// for --build-xml.
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

    /// Watchdog device options, e.g. "model=itco,action=none".
    #[arg(long)]
    watchdog: Option<String>,
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

    eprintln!("virt-xml: only --build-xml and --edit are implemented so far");
    std::process::exit(1);
}

fn run_build_xml(args: &Args) {
    let Some(optstr) = &args.watchdog else {
        eprintln!(
            "virt-xml --build-xml: no device option given (only --watchdog is implemented so far)"
        );
        std::process::exit(1);
    };

    let dev = watchdog_from_optstr(optstr);
    let xml = to_xml_string(&dev).expect("a freshly built DeviceWatchdog always serializes");
    println!("{xml}");
}

fn run_edit(args: &Args, selector: &str) {
    let Some(optstr) = &args.watchdog else {
        eprintln!(
            "virt-xml --edit: no device option given (only --watchdog is implemented so far)"
        );
        std::process::exit(1);
    };
    let Some(domain_name) = &args.domain else {
        eprintln!("virt-xml --edit: a domain name is required");
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

    let result = domain.edit_inactive_xml(&conn, |doc, root| {
        let els = select_watchdog_elements(doc, root, selector)?;
        for el in els {
            apply_watchdog_optstr(doc, el, optstr);
        }
        Ok(())
    });

    match result {
        Ok(()) => println!("Domain '{domain_name}' defined successfully."),
        Err(e) => {
            eprintln!("virt-xml: {e}");
            std::process::exit(1);
        }
    }
}
