//! `virt-xml` — upstream's `virtxml.py`: one XML action (`--edit`,
//! `--add-device`, `--remove-device`, `--build-xml`) with one XML option
//! (`--watchdog ...`) against a domain, its XML on stdin, or nothing
//! (`--build-xml`), then define it, hotplug it (`--update`), start it,
//! or only print the result (`--print-diff`, `--print-xml`).
//!
//! Every XML option goes through its ported `VirtCLIParser`
//! (`virtinst_cli::parsers`).

use std::io::{BufRead, IsTerminal, Write};

use clap::Parser;
use virt::domain::Domain;
use virtinst_cli::cli::{self, fail, print_stdout};
use virtinst_cli::parser::{
    find_objects_to_edit, lookup_prop, parse_edit, parse_guest_edit, parse_new, parse_scalar_edit,
    ParserDef,
};
use virtinst_cli::parsers;
use virtinst_cli::xmlutil;
use virtinst_core::connection::Connection;
use virtinst_core::guest::GUEST;
use virtinst_core::xmlbuilder::XmlObject;

#[derive(Parser)]
#[command(
    name = "virt-xml",
    disable_version_flag = true,
    override_usage = "virt-xml [options]",
    about = "Edit libvirt XML using command line options.",
    after_help = "Use '--option=?' or '--option help' to see available suboptions\n\
                  See man page for examples and full option syntax.",
    infer_long_args = true,
    args_override_self = true,
    // `--edit -1`: argparse takes a negative number as a value.
    allow_negative_numbers = true
)]
struct Args {
    /// show program's version number and exit
    #[arg(long = "version")]
    _version: bool,

    /// Connect to hypervisor with libvirt URI
    #[arg(short = 'c', long, value_name = "URI")]
    connect: Option<String>,

    /// Domain name, id, or uuid
    domain: Option<String>,

    /// Edit VM XML. Examples:
    /// --edit --disk ...     (edit first disk device)
    /// --edit 2 --disk ...   (edit second disk device)
    /// --edit all --disk ... (edit all disk devices)
    /// --edit target=hda --disk ... (edit disk 'hda')
    #[arg(long, num_args = 0..=1, help_heading = "XML actions", verbatim_doc_comment)]
    edit: Option<Option<String>>,

    /// Remove specified device. Examples:
    /// --remove-device --disk 1 (remove first disk)
    /// --remove-device --disk all (remove all disks)
    /// --remove-device --disk /some/path
    #[arg(long, help_heading = "XML actions", verbatim_doc_comment)]
    remove_device: bool,

    /// Add specified device. Example:
    /// --add-device --disk ...
    #[arg(long, help_heading = "XML actions", verbatim_doc_comment)]
    add_device: bool,

    /// Output built device XML. Domain is optional but recommended to ensure optimal defaults.
    #[arg(long, help_heading = "XML actions")]
    build_xml: bool,

    /// Apply changes to the running VM.
    /// With --add-device, this is a hotplug operation.
    /// With --remove-device, this is a hotunplug operation.
    /// With --edit, this is an update device operation.
    #[arg(long, help_heading = "Output options", verbatim_doc_comment)]
    update: bool,

    /// Force defining the domain. Only required if a --print option was specified.
    #[arg(long, help_heading = "Output options", conflicts_with = "no_define")]
    define: bool,

    /// Force not defining the domain.
    #[arg(long, help_heading = "Output options")]
    no_define: bool,

    /// Start the domain.
    #[arg(long, help_heading = "Output options")]
    start: bool,

    /// Only print the requested change, in diff format
    #[arg(long, help_heading = "Output options")]
    print_diff: bool,

    /// Only print the requested change, in full XML format
    #[arg(long, help_heading = "Output options")]
    print_xml: bool,

    /// Require confirmation before saving any results.
    #[arg(long, help_heading = "Output options")]
    confirm: bool,

    /// Configure a guest disk device.
    #[arg(long, help_heading = "XML options")]
    disk: Vec<String>,

    /// Configure a guest network interface. Ex:
    /// --network bridge=mybr0
    /// --network network=my_libvirt_virtual_net
    /// --network network=mynet,model=virtio,mac=00:11...
    /// --network none
    /// --network help
    #[arg(short = 'w', long, help_heading = "XML options", verbatim_doc_comment)]
    network: Vec<String>,

    /// Configure guest display settings. Ex:
    /// --graphics spice
    /// --graphics vnc,port=5901,listen=0.0.0.0
    /// --graphics none
    #[arg(long, help_heading = "XML options", verbatim_doc_comment)]
    graphics: Vec<String>,

    /// Configure guest metadata. Ex:
    /// --metadata name=foo,title="My pretty title",uuid=...
    /// --metadata description="My nice long description"
    #[arg(long, help_heading = "XML options", verbatim_doc_comment)]
    metadata: Vec<String>,

    /// Configure guest memory allocation. Ex:
    /// --memory 1024 (in MiB)
    /// --memory memory=1024,currentMemory=512
    #[arg(long, help_heading = "XML options", verbatim_doc_comment)]
    memory: Vec<String>,

    /// Perform raw XML XPath options on the final XML. Example:
    /// --xml ./cpu/@mode=host-passthrough
    /// --xml ./devices/disk[2]/serial=new-serial
    /// --xml xpath.delete=./clock
    #[arg(long, help_heading = "XML options", verbatim_doc_comment)]
    xml: Vec<String>,

    /// Set domain <iothreads> and <iothreadids> configuration.
    #[arg(long, help_heading = "XML options")]
    iothreads: Vec<String>,

    /// Tune NUMA policy for the domain process.
    #[arg(long, help_heading = "XML options")]
    numatune: Vec<String>,

    /// Tune memory policy for the domain process.
    #[arg(long, help_heading = "XML options")]
    memtune: Vec<String>,

    /// Tune blkio policy for the domain process.
    #[arg(long, help_heading = "XML options")]
    blkiotune: Vec<String>,

    /// Set memory backing policy for the domain process. Ex:
    /// --memorybacking hugepages=on
    #[arg(long, help_heading = "XML options", verbatim_doc_comment)]
    memorybacking: Vec<String>,

    /// Set domain <features> XML. Ex:
    /// --features acpi=off
    /// --features apic=on,apic.eoi=on
    #[arg(long, help_heading = "XML options", verbatim_doc_comment)]
    features: Vec<String>,

    /// Set domain <clock> XML. Ex:
    /// --clock offset=localtime,rtc_tickpolicy=catchup
    #[arg(long, help_heading = "XML options", verbatim_doc_comment)]
    clock: Vec<String>,

    /// Configure VM power management features
    #[arg(long, help_heading = "XML options")]
    pm: Vec<String>,

    /// Configure VM lifecycle management policy
    #[arg(long, help_heading = "XML options")]
    events: Vec<String>,

    /// Configure VM resource partitioning (cgroups)
    #[arg(long, help_heading = "XML options")]
    resource: Vec<String>,

    /// Configure SMBIOS System Information. Ex:
    /// --sysinfo host
    /// --sysinfo bios.vendor=MyVendor,bios.version=1.2.3,...
    #[arg(long, help_heading = "XML options", verbatim_doc_comment)]
    sysinfo: Vec<String>,

    /// Pass arguments directly to the QEMU emulator. Ex:
    /// --qemu-commandline='-display gtk,gl=on'
    /// --qemu-commandline env=DISPLAY=:0.1
    #[arg(long, help_heading = "XML options", verbatim_doc_comment)]
    qemu_commandline: Vec<String>,

    /// Configure a guest controller device. Ex:
    /// --controller type=usb,model=qemu-xhci
    /// --controller type=scsi,model=virtio-scsi
    #[arg(long, help_heading = "XML options", verbatim_doc_comment)]
    controller: Vec<String>,

    /// Configure a guest input device. Ex:
    /// --input tablet
    /// --input keyboard,bus=usb
    #[arg(long, help_heading = "XML options", verbatim_doc_comment)]
    input: Vec<String>,

    /// Configure a guest serial device
    #[arg(long, help_heading = "XML options")]
    serial: Vec<String>,

    /// Configure a guest parallel device
    #[arg(long, help_heading = "XML options")]
    parallel: Vec<String>,

    /// Configure a guest communication channel
    #[arg(long, help_heading = "XML options")]
    channel: Vec<String>,

    /// Configure a text console connection between the guest and host
    #[arg(long, help_heading = "XML options")]
    console: Vec<String>,

    /// Configure physical USB/PCI/etc host devices to be shared with the guest
    #[arg(long, alias = "host-device", help_heading = "XML options")]
    hostdev: Vec<String>,

    /// Configure a guest filesystem device.
    #[arg(long, help_heading = "XML options")]
    filesystem: Vec<String>,

    /// Configure a guest smartcard device.
    #[arg(long, help_heading = "XML options")]
    smartcard: Vec<String>,

    /// Configure a guest TPM device.
    #[arg(long, help_heading = "XML options")]
    tpm: Vec<String>,

    /// Configure guest sound device emulation
    #[arg(long, help_heading = "XML options")]
    sound: Vec<String>,

    /// Configure host audio backend for sound devices
    #[arg(long, help_heading = "XML options")]
    audio: Vec<String>,

    /// Configure guest video hardware.
    #[arg(long, help_heading = "XML options")]
    video: Vec<String>,

    /// Configure a guest redirection device.
    #[arg(long, help_heading = "XML options")]
    redirdev: Vec<String>,

    /// Configure a guest RNG device. Ex:
    /// --rng /dev/urandom
    #[arg(long, help_heading = "XML options", verbatim_doc_comment)]
    rng: Vec<String>,

    /// Configure a guest watchdog device.
    #[arg(long, help_heading = "XML options")]
    watchdog: Vec<String>,

    /// Configure a guest memballoon device.
    #[arg(long, help_heading = "XML options")]
    memballoon: Vec<String>,

    /// Configure a guest panic device.
    #[arg(long, help_heading = "XML options")]
    panic: Vec<String>,

    /// Configure a guest shared memory device. Ex:
    /// --shmem name=shmem0
    #[arg(long, help_heading = "XML options", verbatim_doc_comment)]
    shmem: Vec<String>,

    /// Configure a guest memory device. Ex:
    /// --memdev dimm,target.size=1024
    #[arg(long, help_heading = "XML options", verbatim_doc_comment)]
    memdev: Vec<String>,

    /// Configure guest vsock sockets.
    #[arg(long, help_heading = "XML options")]
    vsock: Vec<String>,

    /// Configure an IOMMU device. Ex:
    /// --iommu model=intel,driver.aw_bits=48
    #[arg(long, help_heading = "XML options", verbatim_doc_comment)]
    iommu: Vec<String>,

    /// Configure a nvram storage device.
    /// It's for guest kernel to record oops/panic logs. Ex:
    /// --pstore backend=acpi-erst,size=8
    #[arg(long, help_heading = "XML options", verbatim_doc_comment)]
    pstore: Vec<String>,

    /// Suppress non-error output
    #[arg(short, long, help_heading = "Miscellaneous Options")]
    quiet: bool,

    /// Print debugging information
    #[arg(short, long, help_heading = "Miscellaneous Options")]
    debug: bool,
}

/// Upstream's `VIRT_PARSERS` order for the options this binary has.
fn xml_options(args: &Args) -> Vec<(&'static ParserDef, &Vec<String>)> {
    let engine = |name: &str, v| (parsers::by_name(name).expect("ported parser"), v);
    vec![
        engine("disk", &args.disk),
        engine("network", &args.network),
        engine("graphics", &args.graphics),
        engine("metadata", &args.metadata),
        engine("memory", &args.memory),
        engine("xml", &args.xml),
        engine("iothreads", &args.iothreads),
        engine("numatune", &args.numatune),
        engine("memtune", &args.memtune),
        engine("blkiotune", &args.blkiotune),
        engine("memorybacking", &args.memorybacking),
        engine("features", &args.features),
        engine("clock", &args.clock),
        engine("pm", &args.pm),
        engine("events", &args.events),
        engine("resource", &args.resource),
        engine("sysinfo", &args.sysinfo),
        engine("qemu_commandline", &args.qemu_commandline),
        engine("controller", &args.controller),
        engine("input", &args.input),
        engine("serial", &args.serial),
        engine("parallel", &args.parallel),
        engine("channel", &args.channel),
        engine("console", &args.console),
        engine("hostdev", &args.hostdev),
        engine("filesystem", &args.filesystem),
        engine("sound", &args.sound),
        engine("audio", &args.audio),
        engine("watchdog", &args.watchdog),
        engine("video", &args.video),
        engine("smartcard", &args.smartcard),
        engine("redirdev", &args.redirdev),
        engine("memballoon", &args.memballoon),
        engine("tpm", &args.tpm),
        engine("rng", &args.rng),
        engine("panic", &args.panic),
        engine("shmem", &args.shmem),
        engine("memdev", &args.memdev),
        engine("vsock", &args.vsock),
        engine("iommu", &args.iommu),
        engine("pstore", &args.pstore),
    ]
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActionName {
    Edit,
    AddDevice,
    RemoveDevice,
    BuildXml,
}

impl ActionName {
    fn as_str(&self) -> &'static str {
        match self {
            ActionName::Edit => "edit",
            ActionName::AddDevice => "add-device",
            ActionName::RemoveDevice => "remove-device",
            ActionName::BuildXml => "build-xml",
        }
    }
}

/// `Action`: one XML action with one XML option.
struct Action {
    name: ActionName,
    selector: Option<String>,
    change: &'static ParserDef,
    values: Vec<String>,
}

/// `check_xmlopt_collision`.
fn check_xmlopt_collision(args: &Args) -> (&'static ParserDef, Vec<String>) {
    let collisions: Vec<(&'static ParserDef, &Vec<String>)> = xml_options(args)
        .into_iter()
        .filter(|(_, v)| !v.is_empty())
        .collect();
    if collisions.is_empty() {
        fail("No change specified.");
    }
    if collisions.len() != 1 {
        let names: Vec<String> = collisions
            .iter()
            .map(|(c, _)| format!("'{}'", c.cli_flag_name()))
            .collect();
        fail(format!(
            "Only one change operation may be specified (conflicting options [{}])",
            names.join(", ")
        ));
    }
    let (change, values) = collisions.into_iter().next().unwrap();
    (change, values.clone())
}

/// `check_action_collision`.
fn check_action_collision(args: &Args) -> (ActionName, Option<String>) {
    let mut collisions = vec![];
    if let Some(sel) = &args.edit {
        collisions.push((ActionName::Edit, sel.clone()));
    }
    if args.add_device {
        collisions.push((ActionName::AddDevice, None));
    }
    if args.remove_device {
        collisions.push((ActionName::RemoveDevice, None));
    }
    if args.build_xml {
        collisions.push((ActionName::BuildXml, None));
    }
    if collisions.is_empty() {
        fail("One of --edit, --add-device, --remove-device, --build-xml must be specified.");
    }
    if collisions.len() > 1 {
        let names: Vec<String> = collisions
            .iter()
            .map(|(a, _)| format!("--{}", a.as_str()))
            .collect();
        fail(format!("Conflicting options {}", names.join(", ")));
    }
    collisions.into_iter().next().unwrap()
}

/// `validate_action`.
fn validate_action(action: &Action, args: &Args) {
    let def = action.change;
    if def.guest_propname.is_none() && action.name == ActionName::BuildXml {
        fail(format!(
            "--build-xml not supported for {}",
            def.cli_flag_name()
        ));
    }
    if action.name == ActionName::BuildXml
        && def.guest_propname.is_some_and(|p| GUEST.prop(p).is_some())
    {
        // Upstream builds a whole default guest here (`Guest.set_defaults`),
        // which comes with the virt-install port.
        fail(format!(
            "--build-xml {} is not implemented yet",
            def.cli_flag_name()
        ));
    }
    if !prop_is_list(def) {
        if action.name == ActionName::RemoveDevice {
            fail(format!(
                "Cannot use --remove-device with {}",
                def.cli_flag_name()
            ));
        }
        if action.name == ActionName::AddDevice {
            fail(format!(
                "Cannot use --add-device with {}",
                def.cli_flag_name()
            ));
        }
    }
    if args.update && def.guest_propname.is_none() {
        fail(format!(
            "Don't know how to --update for {}",
            def.cli_flag_name()
        ));
    }
}

/// `parserclass.prop_is_list(stub_guest)`.
fn prop_is_list(def: &ParserDef) -> bool {
    let Some(propname) = def.guest_propname else {
        return false;
    };
    let mut class = &GUEST;
    let mut is_list = false;
    for piece in propname.split('.') {
        match class.child(piece) {
            Some(cp) => {
                is_list = !cp.is_single;
                class = cp.class;
            }
            None => return false,
        }
    }
    is_list
}

/// `parse_action`.
fn parse_action(args: &Args) -> Action {
    let (change, values) = check_xmlopt_collision(args);
    let (name, selector) = check_action_collision(args);
    let action = Action {
        name,
        selector,
        change,
        values,
    };
    validate_action(&action, args);
    action
}

/// One device an action touched: its XML, for `--update` and
/// `--confirm`.
struct TouchedDev {
    xml: String,
    /// A device still in the guest, whose XML is read (`dev.get_xml()`)
    /// only after the guest's: like upstream, pending sets are written
    /// once, by the guest's `get_xml`.
    pending: Option<(&'static ParserDef, usize)>,
}

/// Fill in the XML of the devices still in `guest`.
fn read_dev_xml(devs: &mut [TouchedDev], guest: &mut XmlObject) {
    for dev in devs {
        if let Some((def, idx)) = dev.pending.take() {
            dev.xml = lookup_prop(guest, def)[idx].get_xml();
        }
    }
}

/// `_find_objects_to_edit` + `run_parser` for each, and the other
/// actions, on `guest`.
fn perform_action(action: &Action, guest: &mut XmlObject, conn: &Connection) -> Vec<TouchedDev> {
    perform_engine(action, action.change, guest, conn)
}

fn perform_engine(
    action: &Action,
    def: &'static ParserDef,
    guest: &mut XmlObject,
    conn: &Connection,
) -> Vec<TouchedDev> {
    let dev_xml = |guest: &mut XmlObject, idx: usize| TouchedDev {
        xml: lookup_prop(guest, def)[idx].get_xml(),
        pending: None,
    };
    let pending = |idx: usize| TouchedDev {
        xml: String::new(),
        pending: Some((def, idx)),
    };
    match action.name {
        ActionName::Edit if def.guest_propname.is_none() => {
            // `action_edit`: the guest itself is the object.
            if let Some(sel) = action.selector.as_deref() {
                if sel != "1" && sel != "all" {
                    fail(format!(
                        "'--edit {sel}' doesn't make sense with {}, just use empty '--edit'",
                        def.cli_flag_name()
                    ));
                }
            }
            for optstr in &action.values {
                parse_guest_edit(def, guest, optstr).unwrap_or_else(|e| fail(e));
            }
            vec![]
        }
        ActionName::Edit if def.guest_propname.is_some_and(|p| guest.has_prop(p)) => {
            let idxs = find_objects_to_edit(def, guest, "edit", action.selector.as_deref())
                .unwrap_or_else(|e| fail(e));
            for _ in idxs {
                for optstr in &action.values {
                    parse_scalar_edit(def, guest, optstr).unwrap_or_else(|e| fail(e));
                }
            }
            vec![]
        }
        ActionName::Edit => {
            let idxs = find_objects_to_edit(def, guest, "edit", action.selector.as_deref())
                .unwrap_or_else(|e| fail(e));
            let mut devs = vec![];
            for idx in idxs {
                for optstr in &action.values {
                    parse_edit(def, guest, idx, optstr).unwrap_or_else(|e| fail(e));
                }
                devs.push(pending(idx));
            }
            devs
        }
        ActionName::AddDevice | ActionName::BuildXml => {
            let mut devs = vec![];
            for optstr in &action.values {
                for idx in parse_new(def, guest, optstr).unwrap_or_else(|e| fail(e)) {
                    parsers::set_defaults(def, guest, idx, Some(conn)).unwrap_or_else(|e| fail(e));
                    devs.push(pending(idx));
                }
            }
            devs
        }
        ActionName::RemoveDevice => {
            let selector = action.values.last().expect("one value at least");
            let mut idxs = find_objects_to_edit(def, guest, "remove-device", Some(selector))
                .unwrap_or_else(|e| fail(e));
            let devs = idxs.iter().map(|i| dev_xml(guest, *i)).collect();
            idxs.sort_unstable();
            let list = def
                .guest_propname
                .and_then(|p| p.rsplit('.').next())
                .expect("guest backed parser");
            for idx in idxs.into_iter().rev() {
                // `guest.remove_device`: and `_remove_duplicate_console`,
                // `_remove_spice_devices`.
                let devices = guest.child_mut("devices");
                let removed = devices.remove_child(list, idx);
                if virtinst_core::devices::char::console_duplicate(
                    &removed,
                    devices.children("console"),
                ) {
                    devices.remove_child("console", 0);
                }
                parsers::graphics::remove_spice_devices(devices, &removed);
            }
            devs
        }
    }
}

/// `prompt_yes_or_no`.
fn prompt_yes_or_no(msg: &str) -> bool {
    let stdin = std::io::stdin();
    loop {
        print!("{msg} (y/n): ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
            return false;
        }
        match line.trim().to_lowercase().as_str() {
            "y" | "yes" => return true,
            "n" | "no" => return false,
            _ => print_stdout("Please enter 'yes' or 'no'.", false),
        }
    }
}

/// The guest's `<name>`.
fn guest_name(guest: &XmlObject) -> String {
    guest.get_str("name").unwrap_or_default()
}

/// `prepare_changes`: the action on a copy of `orig`, the diff or XML
/// printed if asked for.
fn prepare_changes(
    orig: &XmlObject,
    args: &Args,
    action: &Action,
    conn: &Connection,
) -> (Vec<TouchedDev>, XmlObject) {
    let origxml = orig.get_document_xml();
    let mut xmlobj = XmlObject::parse(&GUEST, &origxml).unwrap_or_else(|e| fail(e));
    let mut devs = perform_action(action, &mut xmlobj, conn);
    let newxml = xmlobj.get_document_xml();
    read_dev_xml(&mut devs, &mut xmlobj);
    let diff = xmlutil::diff(&origxml, &newxml, "Original XML", "Altered XML");
    if diff.is_empty() {
        log::debug!("No XML diff, didn't generate any change.");
        log::warn!("No XML diff was generated. The requested changes will have no effect.");
    } else {
        log::debug!("XML diff:\n{diff}");
    }
    if args.print_diff {
        if !diff.is_empty() {
            print_stdout(&diff, false);
        }
    } else if args.print_xml {
        print_stdout(&newxml, false);
    }
    (devs, xmlobj)
}

/// `update_changes`: hotplug, hotunplug or update each device.
fn update_changes(domain: &Domain, devs: &[TouchedDev], action: &Action, confirm: bool) {
    let (confirm_msg, success, failure) = match action.name {
        ActionName::AddDevice => (
            "Hotplug this device to the guest",
            "Device hotplug successful.",
            "Error attempting device hotplug",
        ),
        ActionName::RemoveDevice => (
            "Hotunplug this device from the guest",
            "Device hotunplug successful.",
            "Error attempting device hotunplug",
        ),
        ActionName::Edit => (
            "Update this device for the guest",
            "Device update successful.",
            "Error attempting device update",
        ),
        ActionName::BuildXml => unreachable!("--build-xml returns before"),
    };
    let name = domain.get_name().unwrap_or_default();
    for dev in devs {
        if confirm && !prompt_yes_or_no(&format!("{}\n\n{confirm_msg} '{name}'?", dev.xml)) {
            continue;
        }
        let flags = virt::sys::VIR_DOMAIN_AFFECT_LIVE;
        let res = match action.name {
            ActionName::AddDevice => domain.attach_device_flags(&dev.xml, flags),
            ActionName::RemoveDevice => domain.detach_device_flags(&dev.xml, flags),
            _ => domain.update_device_flags(&dev.xml, flags),
        };
        if let Err(e) = res {
            if std::env::var_os("VIRTXML_TESTSUITE_UPDATE_IGNORE_FAIL").is_none() {
                fail(format!("{failure}: {}", e.message()));
            }
        }
        print_stdout(success, false);
        if confirm {
            print_stdout("", false);
        }
    }
}

/// `get_xmldesc`.
fn get_xmldesc(domain: &Domain, inactive: bool) -> String {
    let mut flags = virt::sys::VIR_DOMAIN_XML_SECURE;
    if inactive {
        flags |= virt::sys::VIR_DOMAIN_XML_INACTIVE;
    }
    domain
        .get_xml_desc(flags)
        .unwrap_or_else(|e| fail(e.message()))
}

/// `get_domain_and_guest`: by name, else by ID or UUID.
fn get_domain_and_guest(conn: &Connection, domstr: &str) -> (Domain, XmlObject, Option<XmlObject>) {
    let isint = domstr.parse::<u32>().is_ok();
    let isuuid = {
        let parts: Vec<&str> = domstr.split('-').collect();
        let lens = [8, 4, 4, 4, 12];
        parts.len() == 5
            && parts
                .iter()
                .zip(lens)
                .all(|(p, l)| p.len() == l && p.chars().all(|c| c.is_ascii_hexdigit()))
    };
    let lookup = Domain::lookup_by_name(conn.as_connect(), domstr).or_else(|e| {
        if isint {
            Domain::lookup_by_id(conn.as_connect(), domstr.parse().unwrap())
        } else if isuuid {
            Domain::lookup_by_uuid_string(conn.as_connect(), domstr)
        } else {
            Err(e)
        }
    });
    let domain = lookup
        .unwrap_or_else(|e| fail(format!("Could not find domain '{domstr}': {}", e.message())));
    let state = domain.get_state().unwrap_or_else(|e| fail(e.message())).0;
    let parse = |xml: &str| XmlObject::parse(&GUEST, xml).unwrap_or_else(|e| fail(e));
    let mut inactive = parse(&get_xmldesc(&domain, false));
    let mut active = None;
    if state != virt::sys::VIR_DOMAIN_SHUTOFF {
        active = Some(inactive);
        inactive = parse(&get_xmldesc(&domain, true));
    }
    (domain, inactive, active)
}

/// `defined_xml_is_unchanged`.
fn defined_xml_is_unchanged(domain: &Domain, original_xml: &str) -> bool {
    let rawxml = get_xmldesc(domain, true);
    match XmlObject::parse(&GUEST, &rawxml) {
        Ok(g) => g.get_document_xml() == original_xml,
        Err(_) => false,
    }
}

fn main() {
    cli::handle_version_flag();
    let mut args = Args::parse();

    let quiet =
        args.quiet && !(args.confirm || args.print_xml || args.print_diff || args.build_xml);
    cli::setup_logging("virt-xml", args.debug, quiet);

    // `check_option_introspection`: `--disk=?`/`--disk help`.
    let mut introspected = false;
    for (def, values) in xml_options(&args) {
        if values.iter().any(|v| v == "?" || v == "help") {
            print!("{}", def.introspection());
            introspected = true;
        }
    }
    if introspected {
        return;
    }

    let mut stdinxml = None;
    if args.domain.is_none() && !args.build_xml {
        let stdin = std::io::stdin();
        if !stdin.is_terminal() {
            if args.confirm {
                fail("Can't use --confirm with stdin input.");
            }
            if args.update {
                fail("Can't use --update with stdin input.");
            }
            let mut xml = String::new();
            let _ = std::io::Read::read_to_string(&mut stdin.lock(), &mut xml);
            stdinxml = Some(xml);
        } else {
            fail("A domain must be specified");
        }
    }

    // Default to --define, unless:
    //  --no-define explicitly specified
    //  --print-* option is used
    //  XML input came from stdin
    let mut define = if args.define {
        Some(true)
    } else if args.no_define {
        Some(false)
    } else {
        None
    };
    if !args.print_xml && !args.print_diff {
        if stdinxml.is_some() {
            if define != Some(true) {
                args.print_xml = true;
            }
        } else if define.is_none() {
            define = Some(true);
        }
    }
    let define = define == Some(true);
    if args.confirm && !args.print_xml {
        args.print_diff = true;
    }

    let conn = cli::get_connection(args.connect.as_deref());
    virtinst_cli::parser::set_conn_support(cli::conn_support(&conn));
    virtinst_cli::parser::set_connect(Some(conn.as_connect().clone()));
    if !args.hostdev.is_empty() || !args.network.is_empty() || !args.graphics.is_empty() {
        virtinst_cli::parser::set_nodedevs(cli::fetch_all_nodedevs(&conn));
    }
    let action = parse_action(&args);

    let (domain, inactive, active) = match &args.domain {
        Some(d) => {
            let (dom, inactive, active) = get_domain_and_guest(&conn, d);
            (Some(dom), inactive, active)
        }
        None => {
            // `Guest(conn, parsexml="")` builds an empty guest.
            let xml = stdinxml.clone().unwrap_or_default();
            let guest = if xml.is_empty() {
                XmlObject::new(&GUEST)
            } else {
                XmlObject::parse(&GUEST, &xml).unwrap_or_else(|e| fail(e))
            };
            (None, guest, None)
        }
    };
    let vm_is_running = active.is_some();
    if !args.graphics.is_empty() {
        // `guest.lookup_domcaps()`, for the spice devices `--graphics`
        // edits add.
        let info = parsers::guest_info(&inactive);
        parsers::graphics::set_domcaps(parsers::lookup_domcaps(&info, Some(&conn)));
    }

    if action.name == ActionName::BuildXml {
        let mut guest = inactive;
        let mut devs = perform_engine(&action, action.change, &mut guest, &conn);
        read_dev_xml(&mut devs, &mut guest);
        for dev in devs {
            print_stdout(&xmlutil::unindent_device_xml(&dev.xml), true);
        }
        return;
    }

    let mut performed_update = false;
    if args.update {
        if args.start {
            fail("Cannot use --update and --start at the same time");
        }
        match (&active, &domain) {
            (Some(active), Some(domain)) => {
                let (devs, _) = prepare_changes(active, &args, &action, &conn);
                update_changes(domain, &devs, &action, args.confirm);
                performed_update = true;
            }
            _ => log::warn!("The VM is not running, --update is inapplicable."),
        }
        if !define {
            // --update and --no-define passed, so we are done
            return;
        }
    }

    let original_xml = inactive.get_document_xml();
    let (_devs, xmlobj) = prepare_changes(&inactive, &args, &action, &conn);
    let name = guest_name(&xmlobj);
    if !define {
        if args.start {
            if args.confirm && !prompt_yes_or_no(&format!("Start '{name}' with the changed XML?")) {
                return;
            }
            match Domain::create_xml(conn.as_connect(), &xmlobj.get_document_xml(), 0) {
                Ok(_) => print_stdout(&format!("Domain '{name}' started successfully."), false),
                Err(e) => fail(format!("Failed starting domain '{name}': {}", e.message())),
            }
        }
        return;
    }

    // `define_changes`
    if args.confirm && !prompt_yes_or_no(&format!("Define '{name}' with the changed XML?")) {
        return;
    }
    let dom = Domain::define_xml(conn.as_connect(), &xmlobj.get_document_xml())
        .unwrap_or_else(|e| fail(e.message()));
    print_stdout(&format!("Domain '{name}' defined successfully."), false);

    if args.start {
        if let Err(e) = dom.create() {
            fail(format!(
                "Failed starting domain '{}': {}",
                guest_name(&inactive),
                e.message()
            ));
        }
        print_stdout(
            &format!("Domain '{}' started successfully.", guest_name(&inactive)),
            false,
        );
    } else if vm_is_running && !performed_update {
        print_stdout(
            "Changes will take effect after the domain is fully powered off.",
            false,
        );
    } else if defined_xml_is_unchanged(&dom, &original_xml) {
        log::warn!(
            "XML did not change after domain define. You may have changed a value that libvirt \
             is setting by default."
        );
    }
}
