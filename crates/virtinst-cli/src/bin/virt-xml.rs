//! `virt-xml` — `--edit`/`--add-device`/`--remove-device`/`--build-xml`
//! (the last works standalone, no connection needed). `--edit` is the
//! first real end-to-end consumer of ticket 06's order-preserving DOM —
//! the exact scenario it was built for (ticket 03's acceptance bar:
//! byte-exact preservation of untouched XML, including foreign
//! `xmlns:qemu` elements).
//!
//! This first slice is deliberately just `--build-xml --watchdog
//! <optstr>` (upstream's own docs call `--build-xml` the one action
//! that needs no domain and no connection — `action_build_xml` in
//! `virtxml.py` is `cli.run_parser(guest, parserclass, parservalue)`
//! then `print(unindent_device_xml(dev.get_xml()))`, one `println!` of
//! `to_xml_string` here). `--edit`/`--add-device`/`--remove-device`
//! all need a domain connection and the DOM-preservation path and
//! aren't built yet — see this file's module doc above for what they
//! are. Only the flags this slice actually implements are declared;
//! an unimplemented flag that parses is worse than an absent one.

use clap::Parser;
use virtinst_cli::watchdog_from_optstr;
use virtinst_xml::to_xml_string;

#[derive(Parser)]
#[command(name = "virt-xml")]
struct Args {
    /// Just build the requested XML block and print it.
    #[arg(long)]
    build_xml: bool,

    /// Watchdog device options, e.g. "model=itco,action=none".
    #[arg(long)]
    watchdog: Option<String>,
}

fn main() {
    let args = Args::parse();

    if !args.build_xml {
        eprintln!("virt-xml: only --build-xml is implemented so far");
        std::process::exit(1);
    }

    let Some(optstr) = args.watchdog else {
        eprintln!(
            "virt-xml --build-xml: no device option given (only --watchdog is implemented so far)"
        );
        std::process::exit(1);
    };

    let dev = watchdog_from_optstr(&optstr);
    let xml = to_xml_string(&dev).expect("a freshly built DeviceWatchdog always serializes");
    println!("{xml}");
}
