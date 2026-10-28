//! `virt-install`: upstream's `virtinst/virtinstall.py`, on the parser
//! engine. Every XML option runs through the same parsers as virt-xml,
//! the guest gets `Guest.set_defaults` (`virtinst_cli::guest`), and the
//! install media/bootdev handling is the `Installer`
//! (`virtinst_cli::installer`).
//!
//! `--unattended` and `--cloud-init` are the next step of the port;
//! they fail with a clear message for now.

use clap::{ArgAction, Parser};
use virtinst_cli::cli::{self, fail, print_stderr, print_stdout};
use virtinst_cli::guest::{self, GuestOpts};
use virtinst_cli::installer::{Installer, InstallerArgs, Started};
use virtinst_cli::parser::{self, run_parser, ParserDef};
use virtinst_cli::parsers;
use virtinst_core::capabilities::caps::Capabilities;
use virtinst_core::connection::Connection;
use virtinst_core::devices::disk as dd;
use virtinst_core::guest::GUEST;
use virtinst_core::osinfo::OsDb;
use virtinst_core::xmlbuilder::{Value, XmlObject};

const INSTALL_METHODS: &str =
    "--location URL, --cdrom CD/ISO, --pxe, --import, --boot hd|cdrom|...";

#[derive(Parser)]
#[command(
    name = "virt-install",
    override_usage = "virt-install OPTIONS",
    about = "Create a new virtual machine from specified install media.",
    after_help = "Use '--option=?' or '--option help' to see available suboptions\n\
                  See man page for examples and full option syntax.",
    disable_version_flag = true,
    args_override_self = true
)]
struct Args {
    /// show program's version number and exit
    #[arg(long = "version")]
    _version: bool,

    /// Connect to hypervisor with libvirt URI
    #[arg(long, value_name = "URI")]
    connect: Option<String>,

    // General Options
    /// Name of the guest instance
    #[arg(short, long, help_heading = "General Options")]
    name: Option<String>,

    /// Configure guest memory allocation. Ex:
    /// --memory 1024 (in MiB)
    /// --memory memory=1024,currentMemory=512
    #[arg(long, help_heading = "General Options", verbatim_doc_comment)]
    memory: Vec<String>,

    #[arg(short = 'r', long = "ram", hide = true)]
    oldmemory: Option<String>,

    /// Number of vCPUs to configure for your guest. Ex:
    /// --vcpus 5
    /// --vcpus 5,maxvcpus=10,cpuset=1-4,6,8
    /// --vcpus sockets=2,cores=4,threads=2
    #[arg(long, help_heading = "General Options", verbatim_doc_comment)]
    vcpus: Vec<String>,

    /// CPU model and features. Ex:
    /// --cpu coreduo,+x2apic
    /// --cpu host-passthrough
    /// --cpu host-model
    #[arg(long, help_heading = "General Options", verbatim_doc_comment)]
    cpu: Vec<String>,

    #[arg(long, hide = true)]
    check_cpu: bool,

    #[arg(long, hide = true)]
    cpuset: Option<String>,

    /// Configure guest metadata. Ex:
    /// --metadata name=foo,title="My pretty title",uuid=...
    /// --metadata description="My nice long description"
    #[arg(long, help_heading = "General Options", verbatim_doc_comment)]
    metadata: Vec<String>,

    /// Perform raw XML XPath options on the final XML. Example:
    /// --xml ./cpu/@mode=host-passthrough
    /// --xml ./devices/disk[2]/serial=new-serial
    /// --xml xpath.delete=./clock
    #[arg(long, help_heading = "General Options", verbatim_doc_comment)]
    xml: Vec<String>,

    #[arg(short = 'u', long, hide = true)]
    uuid: Option<String>,

    #[arg(long, hide = true)]
    description: Option<String>,

    // Installation Method Options
    #[arg(short = 'c', hide = true)]
    cdrom_short: Option<String>,

    /// CD-ROM installation media
    #[arg(long, help_heading = "Installation Method Options")]
    cdrom: Option<String>,

    /// Distro install URL, eg. https://host/path. See man page for
    /// specific distro examples.
    #[arg(short, long, help_heading = "Installation Method Options")]
    location: Option<String>,

    /// Boot from the network using the PXE protocol
    #[arg(long, help_heading = "Installation Method Options")]
    pxe: bool,

    /// Build guest around an existing disk image
    #[arg(long = "import", help_heading = "Installation Method Options")]
    import_install: bool,

    #[arg(long, hide = true)]
    livecd: bool,

    /// Additional arguments to pass to the install kernel booted from
    /// --location
    #[arg(short = 'x', long, help_heading = "Installation Method Options")]
    extra_args: Vec<String>,

    /// Add given file to root of initrd from --location
    #[arg(long, help_heading = "Installation Method Options")]
    initrd_inject: Vec<String>,

    /// Perform an unattended installation
    #[arg(long, num_args = 0..=1, default_missing_value = "1", help_heading = "Installation Method Options")]
    unattended: Option<String>,

    /// Specify fine grained install options
    #[arg(long, help_heading = "Installation Method Options")]
    install: Option<String>,

    /// Reinstall existing VM. Only install options are applied, all
    /// other VM configuration options are ignored.
    #[arg(
        long,
        value_name = "DOMAIN",
        help_heading = "Installation Method Options"
    )]
    reinstall: Option<String>,

    /// Perform a cloud image installation, configuring cloud-init
    #[arg(long, num_args = 0..=1, default_missing_value = "1", help_heading = "Installation Method Options")]
    cloud_init: Option<String>,

    #[arg(long, hide = true)]
    test_media_detection: Option<String>,

    #[arg(long, hide = true)]
    test_stub_command: bool,

    /// Configure guest boot settings. Ex:
    /// --boot hd,cdrom,menu=on
    /// --boot init=/sbin/init (for containers)
    #[arg(
        long,
        help_heading = "Installation Method Options",
        verbatim_doc_comment
    )]
    boot: Vec<String>,

    /// Enable user namespace for LXC container. Ex:
    /// --idmap uid.start=0,uid.target=1000,uid.count=10
    #[arg(
        long,
        help_heading = "Installation Method Options",
        verbatim_doc_comment
    )]
    idmap: Vec<String>,

    #[arg(long, hide = true)]
    init: Option<String>,

    // OS options
    /// The OS being installed in the guest.
    /// This is used for deciding optimal defaults like VirtIO.
    /// Example values: fedora29, rhel7.0, win10, ...
    /// Use '--osinfo list' to see a full list.
    #[arg(
        long,
        visible_alias = "os-variant",
        help_heading = "OS options",
        verbatim_doc_comment
    )]
    osinfo: Option<String>,

    #[arg(long = "os-type", hide = true)]
    old_os_type: Option<String>,

    // Device Options
    /// Specify storage with various options. Ex.
    /// --disk size=10 (new 10GiB image in default location)
    /// --disk /my/existing/disk,cache=none
    /// --disk device=cdrom,bus=scsi
    /// --disk=?
    #[arg(long, help_heading = "Device Options", verbatim_doc_comment)]
    disk: Vec<String>,

    /// Configure a guest network interface. Ex:
    /// --network bridge=mybr0
    /// --network network=my_libvirt_virtual_net
    /// --network network=mynet,model=virtio,mac=00:11...
    /// --network none
    /// --network help
    #[arg(
        short = 'w',
        long,
        help_heading = "Device Options",
        verbatim_doc_comment
    )]
    network: Vec<String>,

    /// Configure guest display settings. Ex:
    /// --graphics spice
    /// --graphics vnc,port=5901,listen=0.0.0.0
    /// --graphics none
    #[arg(long, help_heading = "Device Options", verbatim_doc_comment)]
    graphics: Vec<String>,

    /// Configure a guest controller device. Ex:
    /// --controller type=usb,model=qemu-xhci
    /// --controller type=scsi,model=virtio-scsi
    #[arg(long, help_heading = "Device Options", verbatim_doc_comment)]
    controller: Vec<String>,

    /// Configure a guest input device. Ex:
    /// --input tablet
    /// --input keyboard,bus=usb
    #[arg(long, help_heading = "Device Options", verbatim_doc_comment)]
    input: Vec<String>,

    /// Configure a guest serial device
    #[arg(long, help_heading = "Device Options")]
    serial: Vec<String>,

    /// Configure a guest parallel device
    #[arg(long, help_heading = "Device Options")]
    parallel: Vec<String>,

    /// Configure a guest communication channel
    #[arg(long, help_heading = "Device Options")]
    channel: Vec<String>,

    /// Configure a text console connection between the guest and host
    #[arg(long, help_heading = "Device Options")]
    console: Vec<String>,

    /// Configure physical USB/PCI/etc host devices to be shared with the
    /// guest
    #[arg(long, alias = "host-device", help_heading = "Device Options")]
    hostdev: Vec<String>,

    /// Pass host directory to the guest. Ex:
    /// --filesystem /my/source/dir,/dir/in/guest
    /// --filesystem template_name,/,type=template
    #[arg(long, help_heading = "Device Options", verbatim_doc_comment)]
    filesystem: Vec<String>,

    /// Configure guest sound device emulation
    #[arg(
        long,
        alias = "soundhw",
        num_args = 0..=1,
        default_missing_value = "default",
        action = ArgAction::Append,
        help_heading = "Device Options"
    )]
    sound: Vec<String>,

    /// Configure host audio backend for sound devices
    #[arg(long, help_heading = "Device Options")]
    audio: Vec<String>,

    /// Configure a guest watchdog device
    #[arg(long, help_heading = "Device Options")]
    watchdog: Vec<String>,

    /// Configure guest video hardware.
    #[arg(long, help_heading = "Device Options")]
    video: Vec<String>,

    /// Configure a guest smartcard device. Ex:
    /// --smartcard mode=passthrough
    #[arg(long, help_heading = "Device Options", verbatim_doc_comment)]
    smartcard: Vec<String>,

    /// Configure a guest redirection device. Ex:
    /// --redirdev usb,type=tcp,server=192.168.1.1:4000
    #[arg(long, help_heading = "Device Options", verbatim_doc_comment)]
    redirdev: Vec<String>,

    /// Configure a guest memballoon device. Ex:
    /// --memballoon model=virtio
    #[arg(long, help_heading = "Device Options", verbatim_doc_comment)]
    memballoon: Vec<String>,

    /// Configure a guest TPM device. Ex:
    /// --tpm /dev/tpm
    #[arg(long, help_heading = "Device Options", verbatim_doc_comment)]
    tpm: Vec<String>,

    /// Configure a guest RNG device. Ex:
    /// --rng /dev/urandom
    #[arg(long, help_heading = "Device Options", verbatim_doc_comment)]
    rng: Vec<String>,

    /// Configure a guest panic device. Ex:
    /// --panic default
    #[arg(long, help_heading = "Device Options", verbatim_doc_comment)]
    panic: Vec<String>,

    /// Configure a guest shared memory device. Ex:
    /// --shmem name=shmem0
    #[arg(long, help_heading = "Device Options", verbatim_doc_comment)]
    shmem: Vec<String>,

    /// Configure a guest memory device. Ex:
    /// --memdev dimm,target.size=1024
    #[arg(long, help_heading = "Device Options", verbatim_doc_comment)]
    memdev: Vec<String>,

    /// Configure guest vsock sockets. Ex:
    /// --vsock cid.auto=yes
    /// --vsock cid.address=7
    #[arg(long, help_heading = "Device Options", verbatim_doc_comment)]
    vsock: Vec<String>,

    /// Configure an IOMMU device. Ex:
    /// --iommu model=intel,driver.aw_bits=48
    #[arg(long, help_heading = "Device Options", verbatim_doc_comment)]
    iommu: Vec<String>,

    /// Configure a nvram storage device.
    /// It's for guest kernel to record oops/panic logs. Ex:
    /// --pstore backend=acpi-erst,size=8
    #[arg(long, help_heading = "Device Options", verbatim_doc_comment)]
    pstore: Vec<String>,

    // Deprecated device options
    #[arg(short = 'f', long = "file", hide = true)]
    file_paths: Vec<String>,

    #[arg(short = 's', long = "file-size", hide = true)]
    disksize: Vec<f64>,

    #[arg(long, hide = true)]
    nonsparse: bool,

    #[arg(long, hide = true)]
    nodisks: bool,

    #[arg(long, hide = true)]
    nonetworks: bool,

    #[arg(short = 'b', long, hide = true)]
    bridge: Vec<String>,

    #[arg(short = 'm', long, hide = true)]
    mac: Vec<String>,

    #[arg(long, hide = true)]
    vnc: bool,

    #[arg(long, hide = true)]
    vncport: Option<i64>,

    #[arg(long, hide = true)]
    vnclisten: Option<String>,

    #[arg(short = 'k', long, hide = true)]
    keymap: Option<String>,

    #[arg(long, hide = true)]
    sdl: bool,

    #[arg(long, hide = true)]
    nographics: bool,

    // Guest Configuration Options
    /// Set domain <iothreads> and <iothreadids> configuration.
    #[arg(long, help_heading = "Guest Configuration Options")]
    iothreads: Vec<String>,

    /// Set domain seclabel configuration.
    #[arg(
        long,
        visible_alias = "security",
        help_heading = "Guest Configuration Options"
    )]
    seclabel: Vec<String>,

    /// Set guest to perform the S390 cryptographic key management
    /// operations.
    #[arg(long, help_heading = "Guest Configuration Options")]
    keywrap: Vec<String>,

    /// Tune CPU parameters for the domain process.
    #[arg(long, help_heading = "Guest Configuration Options")]
    cputune: Vec<String>,

    /// Tune NUMA policy for the domain process.
    #[arg(long, help_heading = "Guest Configuration Options")]
    numatune: Vec<String>,

    /// Tune memory policy for the domain process.
    #[arg(long, help_heading = "Guest Configuration Options")]
    memtune: Vec<String>,

    /// Tune blkio policy for the domain process.
    #[arg(long, help_heading = "Guest Configuration Options")]
    blkiotune: Vec<String>,

    /// Set memory backing policy for the domain process. Ex:
    /// --memorybacking hugepages=on
    #[arg(
        long,
        help_heading = "Guest Configuration Options",
        verbatim_doc_comment
    )]
    memorybacking: Vec<String>,

    /// Set domain <features> XML. Ex:
    /// --features acpi=off
    /// --features apic=on,apic.eoi=on
    #[arg(
        long,
        help_heading = "Guest Configuration Options",
        verbatim_doc_comment
    )]
    features: Vec<String>,

    /// Set domain <clock> XML. Ex:
    /// --clock offset=localtime,rtc_tickpolicy=catchup
    #[arg(
        long,
        help_heading = "Guest Configuration Options",
        verbatim_doc_comment
    )]
    clock: Vec<String>,

    /// Configure VM power management features
    #[arg(long, help_heading = "Guest Configuration Options")]
    pm: Vec<String>,

    /// Configure VM lifecycle management policy
    #[arg(long, help_heading = "Guest Configuration Options")]
    events: Vec<String>,

    /// Configure VM resource partitioning (cgroups)
    #[arg(long, help_heading = "Guest Configuration Options")]
    resource: Vec<String>,

    /// Configure SMBIOS System Information. Ex:
    /// --sysinfo host
    /// --sysinfo bios.vendor=MyVendor,bios.version=1.2.3,...
    #[arg(
        long,
        help_heading = "Guest Configuration Options",
        verbatim_doc_comment
    )]
    sysinfo: Vec<String>,

    /// Pass arguments directly to the QEMU emulator. Ex:
    /// --qemu-commandline='-display gtk,gl=on'
    /// --qemu-commandline env=DISPLAY=:0.1
    #[arg(
        long,
        help_heading = "Guest Configuration Options",
        verbatim_doc_comment
    )]
    qemu_commandline: Vec<String>,

    /// Configure VM launch security (e.g. SEV memory encryption). Ex:
    /// --launchSecurity sev
    #[arg(
        long = "launchSecurity",
        visible_alias = "launchsecurity",
        help_heading = "Guest Configuration Options",
        verbatim_doc_comment
    )]
    launch_security: Vec<String>,

    // Virtualization Platform Options
    /// This guest should be a fully virtualized guest
    #[arg(short = 'v', long, help_heading = "Virtualization Platform Options", conflicts_with_all = ["paravirt", "container"])]
    hvm: bool,

    /// This guest should be a paravirtualized guest
    #[arg(
        short = 'p',
        long,
        help_heading = "Virtualization Platform Options",
        conflicts_with = "container"
    )]
    paravirt: bool,

    /// This guest should be a container guest
    #[arg(long, help_heading = "Virtualization Platform Options")]
    container: bool,

    /// Hypervisor name to use (kvm, qemu, xen, ...)
    #[arg(long, help_heading = "Virtualization Platform Options")]
    virt_type: Option<String>,

    /// The CPU architecture to simulate
    #[arg(long, help_heading = "Virtualization Platform Options")]
    arch: Option<String>,

    /// The machine type to emulate
    #[arg(long, help_heading = "Virtualization Platform Options")]
    machine: Option<String>,

    #[arg(long, hide = true)]
    accelerate: bool,

    #[arg(long, hide = true)]
    noapic: bool,

    #[arg(long, hide = true)]
    noacpi: bool,

    // Miscellaneous Options
    /// Have domain autostart on host boot up.
    #[arg(long, help_heading = "Miscellaneous Options")]
    autostart: bool,

    /// Create a transient domain.
    #[arg(long, help_heading = "Miscellaneous Options")]
    transient: bool,

    /// Force power off the domain when the console viewer is closed.
    #[arg(long, help_heading = "Miscellaneous Options")]
    destroy_on_exit: bool,

    /// Minutes to wait for install to complete.
    #[arg(
        long,
        num_args = 0..=1,
        default_missing_value = "-1",
        allow_negative_numbers = true,
        help_heading = "Miscellaneous Options"
    )]
    wait: Option<i64>,

    #[arg(long, hide = true)]
    prompt: bool,

    #[arg(long, hide = true)]
    force: bool,

    /// Configure guest console auto connect. Example:
    /// --autoconsole text
    /// --autoconsole graphical
    /// --autoconsole none
    #[arg(long, help_heading = "Miscellaneous Options", verbatim_doc_comment)]
    autoconsole: Option<String>,

    /// Don't automatically try to connect to the guest console
    #[arg(long, help_heading = "Miscellaneous Options")]
    noautoconsole: bool,

    /// Don't boot guest after completing install.
    #[arg(long, help_heading = "Miscellaneous Options")]
    noreboot: bool,

    /// Print the generated domain XML rather than create the guest.
    #[arg(
        long = "print-xml",
        value_name = "XMLONLY",
        num_args = 0..=1,
        default_missing_value = "all",
        help_heading = "Miscellaneous Options"
    )]
    xmlonly: Option<String>,

    #[arg(long = "print-step", hide = true)]
    xmlstep: Option<String>,

    /// Run through install process, but do not create devices or define
    /// the guest.
    #[arg(long = "dry-run", help_heading = "Miscellaneous Options")]
    dry: bool,

    /// Enable or disable validation checks. Example:
    /// --check path_in_use=off
    /// --check all=off
    #[arg(long, help_heading = "Miscellaneous Options", verbatim_doc_comment)]
    check: Vec<String>,

    /// Suppress non-error output
    #[arg(short, long, help_heading = "Miscellaneous Options")]
    quiet: bool,

    /// Print debugging information
    #[arg(short, long, help_heading = "Miscellaneous Options")]
    debug: bool,
}

impl Args {
    /// `os_type`, from `--hvm`/`--paravirt`/`--container`.
    fn os_type(&self) -> Option<&'static str> {
        if self.hvm {
            Some("hvm")
        } else if self.paravirt {
            Some("xen")
        } else if self.container {
            Some("exe")
        } else {
            None
        }
    }

    /// `options.autoconsole`: `--noautoconsole` is `none`.
    fn autoconsole(&self) -> String {
        if self.noautoconsole {
            return "none".into();
        }
        self.autoconsole.clone().unwrap_or_else(|| "default".into())
    }
}

/// Upstream's `VIRT_PARSERS`, in virt-install's registration order.
fn virt_parsers(args: &Args) -> Vec<(&'static ParserDef, Vec<String>)> {
    let engine =
        |name: &str, v: &Vec<String>| (parsers::by_name(name).expect("ported parser"), v.clone());
    vec![
        engine("memory", &args.memory),
        engine("cpu", &args.cpu),
        engine("vcpus", &args.vcpus),
        engine("metadata", &args.metadata),
        engine("xml", &args.xml),
        engine("boot", &args.boot),
        engine("idmap", &args.idmap),
        engine("disk", &args.disk),
        engine("network", &args.network),
        engine("graphics", &args.graphics),
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
        engine("iothreads", &args.iothreads),
        engine("seclabel", &args.seclabel),
        engine("keywrap", &args.keywrap),
        engine("cputune", &args.cputune),
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
        engine("launchSecurity", &args.launch_security),
    ]
}

fn fail_conflicting(opt1: &str, opt2: &str) -> ! {
    fail(format!("Cannot use {opt1} and {opt2} at the same time"))
}

// Back compat option conversion

fn check_cdrom_option_error(args: &mut Args) {
    if args.cdrom_short.is_some() && args.cdrom.is_some() {
        fail_conflicting("-c", "--cdrom");
    }
    if let Some(c) = args.cdrom_short.take() {
        if c.contains("://") {
            fail(
                "-c specified with what looks like a libvirt URI. Did you mean to use --connect? \
                 If not, use --cdrom instead",
            );
        }
        args.cdrom = Some(c);
    }
}

fn convert_old_init(args: &mut Args) {
    let Some(init) = args.init.take() else {
        return;
    };
    if args.boot.is_empty() {
        args.boot.push(String::new());
    }
    let last = args.boot.last_mut().expect("just made non empty");
    last.push_str(&format!(",init={init}"));
    log::debug!("Converted old --init to --boot {last}");
}

/// Python's `%s` of a float.
fn py_float_str(f: f64) -> String {
    if f.fract() == 0.0 && f.abs() < 1e16 {
        format!("{f:.1}")
    } else {
        format!("{f}")
    }
}

fn convert_old_disks(args: &mut Args) {
    if args.nodisks
        && (!args.file_paths.is_empty() || !args.disk.is_empty() || !args.disksize.is_empty())
    {
        fail("Cannot specify storage and use --nodisks");
    }
    if (!args.file_paths.is_empty() || !args.disksize.is_empty() || args.nonsparse)
        && !args.disk.is_empty()
    {
        fail(
            "Cannot mix --file, --nonsparse, or --file-size with --disk options. Use --disk \
             PATH[,size=SIZE][,sparse=yes|no]",
        );
    }
    if args.disk.is_empty() {
        if args.nodisks {
            args.disk = vec!["none".into()];
        } else {
            // `_do_convert_old_disks`
            let n = args.file_paths.len().max(args.disksize.len());
            let mut opts = vec![];
            for idx in 0..n {
                let mut optstr = String::new();
                if let Some(path) = args.file_paths.get(idx) {
                    optstr += &format!("path={path}");
                }
                if let Some(size) = args.disksize.get(idx) {
                    if !optstr.is_empty() {
                        optstr += ",";
                    }
                    optstr += &format!("size={}", py_float_str(*size));
                }
                if args.nonsparse {
                    if !optstr.is_empty() {
                        optstr += ",";
                    }
                    optstr += "sparse=no";
                }
                log::debug!("Converted to new style: --disk {optstr}");
                opts.push(optstr);
            }
            args.disk = opts;
        }
    }
    log::debug!("Distilled --disk options: {}", py_list(&args.disk));
}

/// Python's `str()` of a list of strings.
fn py_list(items: &[String]) -> String {
    let items: Vec<String> = items.iter().map(|s| py_repr(s)).collect();
    format!("[{}]", items.join(", "))
}

/// Python's `repr()` of a string.
fn py_repr(s: &str) -> String {
    if s.contains('\'') && !s.contains('"') {
        format!("\"{s}\"")
    } else {
        format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
    }
}

fn convert_old_os_options(args: &mut Args) {
    if args.old_os_type.take().is_none() {
        return;
    }
    log::warn!("--os-type is deprecated and does nothing. Please stop using it.");
}

fn convert_old_memory(args: &mut Args) {
    if !args.memory.is_empty() {
        return;
    }
    if let Some(old) = args.oldmemory.take() {
        let n: i64 = old
            .trim()
            .parse()
            .unwrap_or_else(|_| fail(format!("argument -r/--ram: invalid int value: '{old}'")));
        args.memory = vec![n.to_string()];
    }
}

fn convert_old_cpuset(args: &mut Args) {
    let Some(cpuset) = args.cpuset.take() else {
        return;
    };
    args.vcpus.push(format!(",cpuset={cpuset}"));
    log::debug!(
        "Generated compat cpuset: --vcpus {}",
        args.vcpus.last().expect("just pushed")
    );
}

fn convert_old_networks(args: &mut Args) {
    if args.nonetworks {
        args.network = vec!["none".into()];
    }
    let macs = std::mem::take(&mut args.mac);
    let mut networks = std::mem::take(&mut args.network);
    let bridges = std::mem::take(&mut args.bridge);
    if !bridges.is_empty() && !networks.is_empty() {
        fail_conflicting("--bridge", "--network");
    }
    if !bridges.is_empty() {
        // Convert old --bridges to --networks
        networks = bridges.iter().map(|b| format!("bridge:{b}")).collect();
    }
    // If a plain mac is specified, have it imply a default network
    let n = networks.len().max(macs.len()).max(1);
    let mut out = vec![];
    for idx in 0..n {
        let mut net = networks
            .get(idx)
            .cloned()
            .unwrap_or_else(|| "default".into());
        if let Some(mac) = macs.get(idx) {
            net += &format!(",mac={mac}");
        }
        // Handle old format of bridge:foo instead of bridge=foo
        for prefix in ["network", "bridge"] {
            if net.starts_with(&format!("{prefix}:")) {
                net = net.replace(&format!("{prefix}:"), &format!("{prefix}="));
            }
        }
        out.push(net);
    }
    log::debug!("Distilled --network options: {}", py_list(&out));
    args.network = out;
}

fn convert_old_graphics(args: &mut Args) {
    let vnc = args.vnc;
    let sdl = args.sdl;
    let nographics = args.nographics;
    let graphics = !args.graphics.is_empty();
    if graphics
        && (vnc
            || sdl
            || args.keymap.is_some()
            || args.vncport.is_some()
            || args.vnclisten.is_some())
    {
        fail("Cannot mix --graphics and old style graphical options");
    }
    let optnum = [vnc, nographics, sdl, graphics]
        .iter()
        .filter(|b| **b)
        .count();
    if optnum > 1 {
        fail("Can't specify more than one of VNC, SDL, --graphics or --nographics");
    }
    if graphics || optnum == 0 {
        return;
    }
    // Build a --graphics command line from old style opts
    let mut optstr = if vnc {
        "vnc"
    } else if sdl {
        "sdl"
    } else {
        "none"
    }
    .to_string();
    if let Some(l) = &args.vnclisten {
        optstr += &format!(",listen={l}");
    }
    if let Some(p) = args.vncport {
        optstr += &format!(",port={p}");
    }
    if let Some(k) = &args.keymap {
        optstr += &format!(",keymap={k}");
    }
    log::debug!("--graphics compat generated: {optstr}");
    args.graphics = vec![optstr];
}

fn convert_old_features(args: &mut Args) {
    if !args.features.is_empty() {
        return;
    }
    let mut opts = vec![];
    if args.noacpi {
        opts.push("acpi=off");
    }
    if args.noapic {
        opts.push("apic=off");
    }
    if !opts.is_empty() {
        args.features = vec![opts.join(",")];
    }
}

fn convert_wait_zero(args: &mut Args) {
    // Historical back compat, --wait 0 is identical to --noautoconsole
    if args.wait == Some(0) {
        log::warn!("Treating --wait 0 as --noautoconsole");
        args.noautoconsole = true;
        args.autoconsole = Some("none".into());
        args.wait = None;
    }
}

/// `set_test_stub_options`: basic options that will let virt-install
/// succeed, for testing new command line additions.
fn set_test_stub_options(args: &mut Args) {
    if !args.test_stub_command {
        return;
    }
    args.import_install = true;
    if args.connect.is_none() {
        args.connect = Some("test:///default".into());
    }
    if args.name.is_none() {
        args.name = Some("test-stub-command".into());
    }
    if args.memory.is_empty() {
        args.memory = vec!["256".into()];
    }
    if args.disk.is_empty() {
        args.disk = vec!["none".into()];
    }
    if args.graphics.is_empty() {
        args.graphics = vec!["none".into()];
    }
    if args.osinfo.is_none() {
        args.osinfo = Some("fedora27".into());
    }
}

// --osinfo parsing

/// `OSInfoData`.
struct OsInfoData {
    name: Option<String>,
    id: Option<String>,
    detect: bool,
    require_from_cli: Option<bool>,
    require_on: bool,
}

/// `parse_osinfo`.
fn parse_osinfo(optstr: Option<&str>) -> OsInfoData {
    let mut data = OsInfoData {
        name: None,
        id: None,
        detect: false,
        require_from_cli: None,
        require_on: false,
    };
    match optstr {
        Some(o) if o.contains('=') => {
            for (k, v) in parser::parse_optstr_tuples(o) {
                let v = v.unwrap_or_default();
                let onoff = || {
                    cli::raw_on_off_convert(&v).unwrap_or_else(|| {
                        fail(format!("Error: --osinfo {o}: {k} must be 'yes' or 'no'"))
                    })
                };
                match k.as_str() {
                    "name" | "short-id" => data.name = Some(v),
                    "id" => data.id = Some(v),
                    "detect" => data.detect = onoff(),
                    "require" => data.require_from_cli = Some(onoff()),
                    _ => fail(format!("Unknown --osinfo options: ['{k}']")),
                }
            }
        }
        // `set_compat_str`
        None | Some("auto") => data.detect = true,
        Some("none") => data.name = Some("generic".into()),
        Some(o) if o.contains("://") => data.id = Some(o.into()),
        Some(o) => data.name = Some(o.into()),
    }

    // `validate`. Upstream reports a lookup error with the parser's
    // "Error: --osinfo ..." prefix only for the key=value form.
    let prefix = match optstr {
        Some(o) if o.contains('=') => format!("Error: --osinfo {o}: "),
        _ => String::new(),
    };
    let db = || OsDb::load_default().unwrap_or_else(|e| fail(e));
    if let Some(id) = &data.id {
        let os = db()
            .os(id)
            .unwrap_or_else(|| fail(format!("{prefix}Unknown libosinfo ID '{id}'")));
        data.name = Some(os.short_id());
    } else if let Some(name) = &data.name {
        if name != "generic" {
            let os = db().lookup_os(name).unwrap_or_else(|| {
                fail(format!(
                    "{prefix}Unknown OS name '{name}'. See `--osinfo list` for valid values."
                ))
            });
            data.name = Some(os.short_id());
        }
    }
    match data.require_from_cli {
        Some(false) => {
            if data.name.is_none() {
                log::debug!("Converting `require=off` to fallback `name=generic`");
                data.name = Some("generic".into());
            }
        }
        Some(true) => data.require_on = true,
        None => {}
    }
    data
}

/// `--osinfo list`'s natural sort key: numbers sorted in reverse.
fn alphanum_key(val: &str) -> Vec<(bool, String, i64)> {
    let mut out = vec![];
    let mut text = String::new();
    let mut rest = val;
    while let Some(start) = rest.find(|c: char| c.is_ascii_digit()) {
        text.push_str(&rest[..start]);
        out.push((false, std::mem::take(&mut text).to_lowercase(), 0));
        let tail = &rest[start..];
        let end = tail
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(tail.len());
        let n: i64 = tail[..end].parse().unwrap_or(0);
        out.push((true, String::new(), -n));
        rest = &tail[end..];
    }
    out.push((false, rest.to_lowercase(), 0));
    out
}

/// `check_osinfo_list`.
fn check_osinfo_list(args: &Args) -> bool {
    if args.osinfo.as_deref() != Some("list") {
        return false;
    }
    let db = OsDb::load_default().unwrap_or_else(|e| fail(e));
    let mut list: Vec<(String, Vec<String>)> = db
        .all_os()
        .iter()
        .map(|o| {
            let mut names = o.short_id_list();
            let name = names[0].clone();
            names.sort();
            names.dedup();
            (name, names)
        })
        .collect();
    list.push(("generic".into(), vec!["generic".into()]));
    list.sort_by_cached_key(|(name, _)| alphanum_key(name));
    for (_, names) in list {
        println!("{}", names.join(", "));
    }
    println!();
    println!("You can see additional information with:\n\n  osinfo-query os\n");
    true
}

// --install parsing

/// `InstallData`.
#[derive(Default)]
struct InstallData {
    bootdev: Option<String>,
    kernel: Option<String>,
    initrd: Option<String>,
    kernel_args: Option<String>,
    kernel_args_overwrite: Option<bool>,
    os: Option<String>,
    is_set: bool,
    no_install: Option<bool>,
}

/// `parse_install`.
fn parse_install(optstr: Option<&str>) -> InstallData {
    let mut data = InstallData {
        is_set: optstr.is_some_and(|o| !o.is_empty()),
        ..InstallData::default()
    };
    let Some(optstr) = optstr.filter(|o| !o.is_empty()) else {
        return data;
    };
    let mut first = true;
    for (k, v) in parser::parse_optstr_tuples(optstr) {
        // remove_first = "os"
        let (k, v) = match (first, v) {
            (true, None) => ("os".to_string(), Some(k)),
            (_, v) => (k, v),
        };
        first = false;
        let v = v.unwrap_or_default();
        let onoff = || {
            cli::raw_on_off_convert(&v).unwrap_or_else(|| {
                fail(format!(
                    "Error: --install {optstr}: {k} must be 'yes' or 'no'"
                ))
            })
        };
        match k.as_str() {
            "bootdev" => data.bootdev = Some(v),
            "kernel" => data.kernel = Some(v),
            "initrd" => data.initrd = Some(v),
            "kernel_args" => data.kernel_args = Some(v),
            "kernel_args_overwrite" => data.kernel_args_overwrite = Some(onoff()),
            "os" => data.os = Some(v),
            "no_install" => data.no_install = Some(onoff()),
            _ => fail(format!("Unknown --install options: ['{k}']")),
        }
    }
    data
}

/// The connection's URI as upstream's `conn.uri` reports it.
fn conn_uri(conn: &Connection) -> String {
    std::env::var("VIRTINST_TEST_FAKE_URI")
        .ok()
        .or_else(|| conn.real_uri().ok())
        .unwrap_or_default()
}

fn virsh_start_cmd(guest: &XmlObject, conn: &Connection) -> String {
    format!(
        "virsh --connect {} start {}",
        conn_uri(conn),
        guest.get_str("name").unwrap_or_default()
    )
}

fn install_fail(guest: &XmlObject, conn: &Connection) -> ! {
    print_stderr(&format!(
        "Domain installation does not appear to have been successful.\n\
         If it was, you can restart your domain by running:\n  {}\n\
         otherwise, please restart your installation.",
        virsh_start_cmd(guest, conn)
    ));
    std::process::exit(1);
}

/// Everything virt-install keeps about the guest besides its XML.
struct Built {
    guest: XmlObject,
    opts: GuestOpts,
    installer: Installer,
    /// Volumes to create, per disk (`build_storage`).
    vol_installs: Vec<virtinst_core::storage::VolInstall>,
}

/// `memory_specified`.
fn memory_specified(guest: &XmlObject) -> bool {
    let set = |n: &str| matches!(guest.get(n), Some(Value::Int(v)) if v != 0);
    set("memory") || set("_currentMemory") || !guest.child("cpu").children("cells").is_empty()
}

/// `storage_specified`.
fn storage_specified(args: &Args, guest: &XmlObject) -> bool {
    if is_container(guest) {
        return true;
    }
    !args.disk.is_empty() || !args.filesystem.is_empty()
}

fn is_container(guest: &XmlObject) -> bool {
    guest.child("os").get_str("os_type").as_deref() == Some("exe")
}

fn is_hvm(guest: &XmlObject) -> bool {
    guest.child("os").get_str("os_type").as_deref() == Some("hvm")
}

/// `validate_required_options`. Doesn't error right away if nothing is
/// specified, aggregates the errors to help first time users get it
/// right.
fn validate_required_options(args: &Args, guest: &XmlObject, installer: &Installer) {
    let mut msg = String::new();
    if args.reinstall.is_none() {
        if !memory_specified(guest) {
            msg += "\n--memory amount in MiB is required";
        }
        if !storage_specified(args, guest) {
            msg += "\n--disk storage must be specified (override with --disk none)";
        }
    }
    if !is_container(guest) && !installer.options_specified() {
        msg += &format!("\nAn install method must be specified\n({INSTALL_METHODS})");
    }
    if !msg.is_empty() {
        fail(msg);
    }
}

/// `show_console_warnings`.
fn show_console_warnings(installer: &Installer, autoconsole: &AutoconsoleData) {
    if installer.cdrom().is_none() || !autoconsole.is_text() {
        return;
    }
    log::warn!(
        "CDROM media does not print to the text console by default, so you likely will not see \
         text install output. You might want to use --location. See the man page for examples \
         of using --location with CDROM media"
    );
}

/// `_show_memory_warnings`.
fn show_memory_warnings(guest: &XmlObject) {
    let cur = match guest.get("_currentMemory") {
        Some(Value::Int(v)) if v != 0 => v,
        _ => return,
    };
    let rammb = cur / 1024;
    let os = guest::osinfo_obj(guest);
    let arch = guest.child("os").get_str("arch").unwrap_or_default();
    let minram = os
        .as_ref()
        .and_then(|o| o.get_recommended_resources().get_minimum_ram(&arch))
        .unwrap_or(0);
    if minram != 0 {
        if minram / 1024 > cur {
            log::warn!(
                "Requested memory {rammb} MiB is less than the recommended {} MiB for OS {}",
                minram / (1024 * 1024),
                guest::osinfo_name(guest)
            );
        }
    } else if rammb < 17 {
        log::warn!(
            "Requested memory {rammb} MiB is abnormally low. Were you trying to specify GiB?"
        );
    }
}

/// `supports_pxe`: false if we are pretty sure the config doesn't
/// support PXE.
fn supports_pxe(guest: &XmlObject, conn: &Connection) -> bool {
    for nic in guest.child("devices").children("interface") {
        let ntype = nic.get_str("type").unwrap_or_default();
        if ntype == "user" {
            continue;
        }
        if ntype != "network" {
            return true;
        }
        let source = nic.get_str("network").unwrap_or_default();
        return match virt::network::Network::lookup_by_name(conn.as_connect(), &source)
            .and_then(|n| n.get_xml_desc(0))
        {
            Ok(xml) => can_pxe(&xml),
            Err(_) => {
                log::debug!("Error checking if PXE supported");
                true
            }
        };
    }
    false
}

/// `Network.can_pxe`.
fn can_pxe(xml: &str) -> bool {
    let Ok(doc) = virtinst_xml::parse_libvirt_xml(xml) else {
        return true;
    };
    let Some(root) = doc.root_element() else {
        return true;
    };
    if let Some(mode) = root
        .find(&doc, "forward")
        .and_then(|f| f.attribute(&doc, "mode"))
    {
        if !mode.is_empty() && mode != "nat" {
            return true;
        }
    }
    root.find_all(&doc, "ip").into_iter().any(|ip| {
        ip.find(&doc, "dhcp")
            .and_then(|d| d.find(&doc, "bootp"))
            .and_then(|b| b.attribute(&doc, "file"))
            .is_some_and(|f| !f.is_empty())
    })
}

/// Python's `shlex.quote`.
fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    let safe = |c: char| c.is_alphanumeric() || c == '_' || "@%+=:,./-".contains(c);
    if s.chars().all(safe) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

/// `show_guest_warnings`.
fn show_guest_warnings(args: &Args, guest: &XmlObject, conn: &Connection) {
    if args.pxe && !supports_pxe(guest, conn) {
        log::warn!("The guest's network configuration may not support PXE");
    }
    // HVM is really the only case where OS impacts what we set for
    // defaults, so far.
    if guest::osinfo_name(guest) == "generic" && is_hvm(guest) {
        log::warn!(
            "Using --osinfo generic, VM performance may suffer. Specify an accurate OS for \
             optimal results."
        );
    }
    show_memory_warnings(guest);
}

/// `do_test_media_detection`.
fn do_test_media_detection(args: &Args, conn: &Connection, caps: &Capabilities) {
    let url = args.test_media_detection.clone();
    let mut guest = XmlObject::new(&GUEST);
    if let Some(arch) = args.arch.clone().filter(|a| !a.is_empty()) {
        guest
            .set_path("os.arch", Some(Value::Str(arch)))
            .unwrap_or_else(|e| fail(e));
    }
    if let Some(t) = args.os_type() {
        guest
            .set_path("os.os_type", Some(Value::Str(t.into())))
            .unwrap_or_else(|e| fail(e));
    }
    let support = parser::conn_support();
    guest::set_capabilities_defaults(&mut guest, caps, &support).unwrap_or_else(|e| fail(e));

    let mut installer = Installer::new(
        conn,
        InstallerArgs {
            location: url,
            ..InstallerArgs::default()
        },
    )
    .unwrap_or_else(|e| fail(e));
    let ret = installer
        .detect_distro(&guest, conn)
        .unwrap_or_else(|e| fail(e));
    print_stdout(ret.as_deref().unwrap_or("None"), true);
}

/// `get_location_for_os`.
fn get_location_for_os(guest: &XmlObject, osname: &str, profile: Option<&str>) -> String {
    let db = OsDb::load_default().unwrap_or_else(|e| fail(e));
    let osinfo = db.lookup_os(osname).unwrap_or_else(|| {
        fail(format!(
            "Unknown OS name '{osname}'. See `--osinfo list` for valid values."
        ))
    });
    let arch = guest.child("os").get_str("arch").unwrap_or_default();
    let location = osinfo
        .get_location(&arch, profile)
        .unwrap_or_else(|e| fail(e));
    print_stdout(&format!("Using {osname} --location {location}"), false);
    location
}

/// `parse_location`: (location, kernel, initrd).
fn parse_location(optstr: &str) -> (Option<String>, Option<String>, Option<String>) {
    let mut def = ParserDef::new("location", None, None);
    def.remove_first = &["location"];
    for name in ["location", "kernel", "initrd"] {
        def.arg(parser::ArgDef::new(name, name).can_comma());
    }
    let mut ret = (None, None, None);
    let mut leftover = vec![];
    for (k, v) in def.optdict(optstr).0 {
        match k.as_str() {
            "location" => ret.0 = v,
            "kernel" => ret.1 = v,
            "initrd" => ret.2 = v,
            _ => leftover.push(format!("'{k}'")),
        }
    }
    if !leftover.is_empty() {
        fail(format!(
            "Unknown --location options: [{}]",
            leftover.join(", ")
        ));
    }
    ret
}

/// `build_installer`.
fn build_installer(
    args: &Args,
    conn: &Connection,
    guest: &XmlObject,
    installdata: &InstallData,
) -> Installer {
    let mut a = InstallerArgs {
        is_reinstall: args.reinstall.is_some(),
        install_bootdev: installdata.bootdev.clone(),
        install_kernel: installdata.kernel.clone(),
        install_initrd: installdata.initrd.clone(),
        no_install: installdata.no_install.unwrap_or(false),
        ..InstallerArgs::default()
    };
    let mut extra_args = args.extra_args.clone();
    if let Some(ka) = installdata.kernel_args.clone().filter(|k| !k.is_empty()) {
        if installdata.kernel_args_overwrite == Some(true) {
            a.install_kernel_args = Some(ka);
        } else {
            extra_args = vec![ka];
        }
    }

    if args.unattended.is_some() && args.cloud_init.is_some() {
        fail_conflicting("--unattended", "--cloud-init");
    }
    let not_yet = |opt: &str| -> ! {
        fail(format!(
            "{opt} is not implemented yet in this port of virt-install"
        ))
    };
    // `parse_unattended`/`parse_cloud_init` return nothing for "none"
    let given = |o: &Option<String>| o.as_deref().is_some_and(|v| v != "none");
    if given(&args.unattended) {
        not_yet("--unattended");
    }

    if let Some(os) = &installdata.os {
        a.location = Some(get_location_for_os(guest, os, None));
    } else if let Some(loc) = &args.location {
        (a.location, a.location_kernel, a.location_initrd) = parse_location(loc);
    } else if let Some(c) = &args.cdrom {
        a.cdrom = Some(c.clone());
        if args.livecd {
            a.no_install = true;
        }
    } else if args.pxe {
        a.install_bootdev = Some("network".into());
    } else if installdata.is_set {
    } else if args.xmlonly.is_some() || args.import_install || !args.boot.is_empty() {
        a.no_install = true;
    }

    let mut installer = Installer::new(conn, a).unwrap_or_else(|e| fail(e));
    if !extra_args.is_empty() {
        installer
            .set_extra_args(extra_args)
            .unwrap_or_else(|e| fail(e));
    }
    if !args.initrd_inject.is_empty() {
        installer
            .set_initrd_injections(args.initrd_inject.clone())
            .unwrap_or_else(|e| fail(e));
    }
    if args.autostart {
        installer.autostart = true;
    }
    if given(&args.cloud_init) {
        not_yet("--cloud-init");
    }
    installer
}

/// `set_cli_default_name`.
fn set_cli_default_name(guest: &mut XmlObject, conn: &Connection, caps: &Capabilities) {
    if guest.get_str("name").is_some_and(|n| !n.is_empty()) {
        return;
    }
    let name = guest::generate_name(guest, conn, caps);
    print_stdout(&format!("Using default --name {name}"), false);
    guest
        .set("name", Some(Value::Str(name)))
        .unwrap_or_else(|e| fail(e));
}

/// `set_cli_defaults`.
fn set_cli_defaults(args: &mut Args, b: &mut Built) {
    let guest = &mut b.guest;
    if is_container(guest) {
        if !memory_specified(guest) {
            let mbram = 1024;
            // LXC doesn't even do anything with memory settings, but libvirt
            // XML requires it anyways. Fill in 64 MiB
            print_stdout(&format!("Using container default --memory {mbram}"), false);
            virtinst_core::guest::set_current_memory(guest, Some(mbram * 1024))
                .unwrap_or_else(|e| fail(e));
        }
        return;
    }

    let Some(os) = guest::osinfo_obj(guest) else {
        return;
    };
    let osname = os.short_id();
    let arch = guest.child("os").get_str("arch").unwrap_or_default();
    let res = os.get_recommended_resources();
    let storage = res.get_recommended_storage(&arch);
    let ram = res.get_recommended_ram(&arch);
    let ncpus = res.get_recommended_ncpus(&arch);

    if let Some(ram) = ram {
        if !memory_specified(guest) {
            let mb = ram as f64 / (1024.0 * 1024.0);
            let mbram = py_float_str(mb);
            let mbram = mbram.trim_end_matches('0').trim_end_matches('.');
            print_stdout(&format!("Using {osname} default --memory {mbram}"), false);
            virtinst_core::guest::set_current_memory(guest, Some(ram / 1024))
                .unwrap_or_else(|e| fail(e));
        }
    }

    if let Some(ncpus) = ncpus {
        // We need to do this upfront, so we don't incorrectly set guest.vcpus
        guest::sync_vcpus_topology(guest, ncpus).unwrap_or_else(|e| fail(e));
    }

    if let Some(storage) = storage {
        if !storage_specified(args, guest) {
            let diskstr = format!("size={}", storage / (1024 * 1024 * 1024));
            print_stdout(&format!("Using {osname} default --disk {diskstr}"), false);
            args.disk = vec![diskstr.clone()];
            parse_disk(b, &diskstr);
        }
    }
}

/// `ParserDisk(optstr, guest=guest).parse(None)`, keeping the disk's
/// storage for its `set_defaults` and `build_storage`.
fn parse_disk(b: &mut Built, optstr: &str) {
    let def = parsers::by_name("disk").expect("ported parser");
    let idxs = run_parser(def, &mut b.guest, optstr).unwrap_or_else(|e| fail(e));
    for idx in idxs {
        while b.opts.disk_backends.len() < idx {
            b.opts.disk_backends.push(None);
        }
        b.opts.disk_backends.insert(idx, parsers::disk::backend());
        if let Some(vi) = parsers::disk::vol_install() {
            b.vol_installs.push(vi);
        }
    }
}

/// `cli.run_all_parsers`.
fn run_all_parsers(args: &Args, b: &mut Built) {
    for (def, values) in virt_parsers(args) {
        for optstr in &values {
            if def.cli_arg_name == "disk" {
                parse_disk(b, optstr);
                continue;
            }
            run_parser(def, &mut b.guest, optstr).unwrap_or_else(|e| fail(e));
        }
    }
}

/// `installer_detect_distro`.
fn installer_detect_distro(
    guest: &mut XmlObject,
    installer: &mut Installer,
    osdata: &OsInfoData,
    conn: &Connection,
) {
    let mut os_set = false;
    // OS name has to be set firstly whenever --osinfo is passed,
    // otherwise it won't be respected when the installer creates the
    // Distro Store.
    let mut fallback_name = None;
    if let Some(name) = &osdata.name {
        fallback_name = Some(name.clone());
        os_set = true;
        guest::set_os_name(guest, name).unwrap_or_else(|e| fail(e));
    }

    // This also validates the install location
    let autodistro = installer
        .detect_distro(guest, conn)
        .unwrap_or_else(|e| fail(format!("Error validating install location: {e}")));
    if osdata.detect {
        if let Some(d) = autodistro {
            os_set = true;
            guest::set_os_name(guest, &d).unwrap_or_else(|e| fail(e));
        } else if let Some(name) = fallback_name {
            log::warn!(
                "Failed to detect osinfo OS name from install media, using fallback name \
                 '{name}'.\nPlease file a bug against virt-install if you expected the detection \
                 to succeed."
            );
        }
    }

    let msg = "--osinfo/--os-variant OS name is required, but no value was\nset or detected.";
    if os_set {
        return;
    }
    if osdata.require_on {
        fail(msg);
    }
    if !is_hvm(guest) {
        return;
    }

    let mut fail_msg = format!("{msg}\n\n");
    fail_msg += "This is now a fatal error. Specifying an OS name is required\n\
                 for modern, performant, and secure virtual machine defaults.\n";
    if installer.location().is_some() || installer.cdrom().is_some() {
        fail_msg += "\nIf you expected virt-install to detect an OS name from the\n\
                     install media, you can set a fallback OS name with:\n\
                     \n  --osinfo detect=on,name=OSNAME\n";
    }
    fail_msg += "\nYou can see a full list of possible OS name values with:\n\
                 \n   virt-install --osinfo list\n";

    let db = OsDb::load_default().unwrap_or_else(|e| fail(e));
    let mut generic_linux: Vec<(String, String)> = db
        .all_os()
        .iter()
        .map(|o| o.short_id())
        .filter(|n| {
            n.strip_prefix("linux")
                .is_some_and(|y| y.len() >= 4 && y[..4].bytes().all(|b| b.is_ascii_digit()))
        })
        .map(|n| (n.clone(), n))
        .collect();
    generic_linux.sort_by_cached_key(|(n, _)| alphanum_key(n));
    if !generic_linux.is_empty() {
        let names: Vec<String> = generic_linux.into_iter().map(|(n, _)| n).collect();
        fail_msg += &format!(
            "\nIf your Linux distro is not listed, try one of generic values\nsuch as: {}\n",
            names.join(", ")
        );
    }

    let envkey = "VIRTINSTALL_OSINFO_DISABLE_REQUIRE";
    fail_msg += &format!(
        "\nIf you just need to get the old behavior back, you can use:\n\
         \n  --osinfo detect=on,require=off\n\
         \nOr export {envkey}=1\n"
    );
    let fail_msg = format!("\n{fail_msg}");
    if std::env::var_os(envkey).is_some() {
        log::warn!("{fail_msg}");
        log::warn!("{envkey} set. Skipping fatal error.");
    } else {
        fail(fail_msg);
    }
}

/// `set_explicit_guest_options`.
fn set_explicit_guest_options(args: &mut Args, guest: &mut XmlObject) {
    let set = |guest: &mut XmlObject, path: &str, val: Option<String>| {
        if let Some(v) = val.filter(|v| !v.is_empty()) {
            guest
                .set_path(path, Some(Value::Str(v)))
                .unwrap_or_else(|e| fail(e));
        }
    };
    set(guest, "name", args.name.take());
    set(guest, "uuid", args.uuid.take());
    set(guest, "description", args.description.take());
    set(guest, "os.os_type", args.os_type().map(str::to_string));
    set(guest, "type", args.virt_type.take());
    set(guest, "os.arch", args.arch.take());
    set(guest, "os.machine", args.machine.take());
}

/// What the parsers need from the guest's connection before they run.
fn setup_parser_conn(args: &Args, guest: &XmlObject, conn: &Connection, caps: &Capabilities) {
    let info = parsers::guest_info(guest);
    let domcaps = parsers::lookup_domcaps(&info, Some(conn));
    parsers::graphics::set_domcaps(domcaps.clone());
    parsers::cpu::set_cpu_conn(domcaps, caps.host.cpu.model.clone());
    if !args.hostdev.is_empty() || !args.network.is_empty() || !args.graphics.is_empty() {
        parser::set_nodedevs(cli::fetch_all_nodedevs(conn));
    }
}

/// `_build_options_guest`.
fn build_options_guest(args: &mut Args, conn: &Connection, caps: &Capabilities, b: &mut Built) {
    // Fill in guest from the command line content
    set_explicit_guest_options(args, &mut b.guest);

    // We do these two parser bit early, since Installer setup will
    // depend on them, but delay the rest to later, since things like
    // disk naming can depend on Installer operations
    for (name, values) in [("boot", &args.boot), ("metadata", &args.metadata)] {
        let def = parsers::by_name(name).expect("ported parser");
        for optstr in values {
            run_parser(def, &mut b.guest, optstr).unwrap_or_else(|e| fail(e));
        }
    }

    // Call set_capabilities_defaults explicitly here rather than depend
    // on set_defaults calling it. Installer setup needs filled in values.
    // However we want to do it after run_all_parsers to ensure
    // we are operating on any arch/os/type values passed in with --boot
    let support = parser::conn_support();
    guest::set_capabilities_defaults(&mut b.guest, caps, &support).unwrap_or_else(|e| fail(e));
    setup_parser_conn(args, &b.guest, conn, caps);
}

/// The new disks' storage checks (`cli.validate_disk`).
fn validate_disks(b: &Built, conn: &Connection) {
    let vconn = conn.as_connect();
    let disks = b.guest.child("devices").children("disk");
    for (idx, disk) in disks.iter().enumerate() {
        let backend = b.opts.disk_backends.get(idx).cloned().flatten();
        let path = backend
            .as_ref()
            .and_then(|be| be.get_path())
            .or_else(|| dd::xmlpath(disk));
        let size_conflict = backend
            .as_ref()
            .and_then(|be| be.vol_install())
            .map_or((false, None), |vi| vi.is_size_conflict(vconn));
        let check = cli::DiskCheck {
            path: path.as_deref(),
            shareable: disk
                .get_str("shareable")
                .is_some_and(|v| v == "True" || v == "yes")
                || disk.get("shareable") == Some(Value::Bool(true)),
            read_only: disk.get("read_only") == Some(Value::Bool(true)),
            size_conflict,
        };
        cli::validate_disk(conn, &check, false);
    }
    for net in b.guest.child("devices").children("interface") {
        if let Some(mac) = net.get_str("macaddr") {
            cli::validate_mac(conn, &mac);
        }
    }
}

/// `build_guest_instance`.
fn build_guest_instance(args: &mut Args, conn: &Connection, caps: &Capabilities) -> Built {
    let installdata = parse_install(args.install.as_deref());
    let osdata = parse_osinfo(args.osinfo.as_deref().or(installdata.os.as_deref()));

    let mut b = Built {
        guest: XmlObject::new(&GUEST),
        opts: GuestOpts::default(),
        installer: build_installer_placeholder(),
        vol_installs: vec![],
    };
    if let Some(domstr) = args.reinstall.clone() {
        (_, b.guest, _) = cli::get_domain_and_guest(conn, &domstr);
    } else {
        build_options_guest(args, conn, caps, &mut b);
    }

    b.installer = build_installer(args, conn, &b.guest, &installdata);

    // Set guest osname, from commandline or detected from media
    guest::set_os_name(&mut b.guest, "generic").unwrap_or_else(|e| fail(e));
    installer_detect_distro(&mut b.guest, &mut b.installer, &osdata, conn);

    if args.reinstall.is_none() {
        // We want to fill in --name before we do disk parsing, since
        // default disk paths are generated based on VM name
        set_cli_default_name(&mut b.guest, conn, caps);
        run_all_parsers(args, &mut b);
        set_cli_defaults(args, &mut b);
    }

    b.opts = GuestOpts {
        disk_backends: std::mem::take(&mut b.opts.disk_backends),
        ..GuestOpts::from_flags(&parser::guest_flags())
    };
    b.installer
        .set_install_defaults(&mut b.guest, &mut b.opts, conn)
        .unwrap_or_else(|e| fail(e));
    for path in b
        .installer
        .get_search_paths(conn)
        .unwrap_or_else(|e| fail(e))
    {
        cli::check_path_search(conn, &path);
    }

    if args.reinstall.is_none() {
        // cli specific disk validation
        validate_disks(&b, conn);
    }

    validate_required_options(args, &b.guest, &b.installer);
    show_guest_warnings(args, &b.guest, conn);
    b
}

fn build_installer_placeholder() -> Installer {
    Installer::empty()
}

// Install process helpers

/// `_AutoconsoleData`.
struct AutoconsoleData {
    autoconsole: String,
    is_default: bool,
}

impl AutoconsoleData {
    fn new(autoconsole: &str, guest: &XmlObject) -> AutoconsoleData {
        if !["none", "default", "text", "graphical"].contains(&autoconsole) {
            fail(format!("Unknown autoconsole type '{autoconsole}'"));
        }
        let is_default = autoconsole == "default";
        let mut autoconsole = autoconsole.to_string();
        if is_default {
            autoconsole = default_autoconsole_type(guest)
                .unwrap_or("none")
                .to_string();
        }
        AutoconsoleData {
            autoconsole,
            is_default,
        }
    }

    fn is_text(&self) -> bool {
        self.autoconsole == "text"
    }

    fn is_graphical(&self) -> bool {
        self.autoconsole == "graphical"
    }

    fn has_console_cb(&self) -> bool {
        self.is_text() || self.is_graphical()
    }
}

fn in_testsuite() -> bool {
    std::env::var_os("VIRTINST_TEST_SUITE").is_some()
}

/// `_determine_default_autoconsole_type`.
fn default_autoconsole_type(guest: &XmlObject) -> Option<&'static str> {
    let gdevs = guest.child("devices").children("graphics");
    let Some(g) = gdevs.first() else {
        return Some("text");
    };
    let gtype = g.get_str("_type").unwrap_or_default();
    if !["default", "vnc", "spice"].contains(&gtype.as_str()) {
        log::debug!("No viewer to launch for graphics type '{gtype}'");
        return None;
    }
    let has_display = std::env::var("DISPLAY").is_ok_and(|d| !d.is_empty())
        || std::env::var_os("DISPLAY_WAYLAND").is_some();
    if !has_display && !in_testsuite() {
        log::warn!("No display detected. Not running virt-viewer.");
        return None;
    }
    let has_viewer = std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("virt-viewer").is_file()));
    if !has_viewer && !in_testsuite() {
        log::warn!(
            "Unable to connect to graphical console: virt-viewer not installed. Please install \
             the 'virt-viewer' package."
        );
        return None;
    }
    Some("graphical")
}

/// `_run_console`: print the command, then run it in a child process.
fn run_console(message: &str, args: Vec<String>) -> Option<std::process::Child> {
    log::debug!("Running: {}", args.join(" "));
    let argstr: Vec<String> = args.iter().map(|a| shell_quote(a)).collect();
    print_stdout(&message.replace("%(command)s", &argstr.join(" ")), false);
    let args = if in_testsuite() {
        vec!["/bin/test".to_string()]
    } else {
        args
    };
    match std::process::Command::new(&args[0])
        .args(&args[1..])
        .spawn()
    {
        Ok(c) => Some(c),
        Err(e) => {
            println!("Error launching {args:?}: {e}");
            None
        }
    }
}

/// `_gfx_console` / `_txt_console`.
fn launch_console(
    autoconsole: &AutoconsoleData,
    guest: &XmlObject,
    conn: &Connection,
) -> Option<std::process::Child> {
    let name = guest.get_str("name").unwrap_or_default();
    let uri = conn_uri(conn);
    if autoconsole.is_graphical() {
        let mut args = vec![
            "virt-viewer".to_string(),
            "--connect".into(),
            uri,
            "--wait".into(),
            name,
        ];
        // Currently virt-viewer needs attaching to the local display while
        // spice gl is enabled or listen type none is used.
        let info = parsers::guest_info(guest);
        let has_listen_none = guest.child("devices").children("graphics").iter().any(|g| {
            g.children("listens")
                .first()
                .and_then(|l| l.get_str("type"))
                .as_deref()
                == Some("none")
        });
        if info.has_gl || has_listen_none {
            args.push("--attach".into());
        }
        return run_console("Running graphical console command: %(command)s", args);
    }
    if autoconsole.is_text() {
        let args = vec![
            "virsh".to_string(),
            "--connect".into(),
            uri,
            "console".into(),
            name,
        ];
        return run_console("Running text console command: %(command)s", args);
    }
    None
}

/// `WaitHandler`: the `--wait` option sleeping and time tracking.
struct WaitHandler {
    wait_is_requested: bool,
    wait_mins: i64,
    start: std::time::Instant,
}

impl WaitHandler {
    fn new(wait: Option<i64>) -> WaitHandler {
        WaitHandler {
            wait_is_requested: wait.is_some(),
            wait_mins: wait.unwrap_or(0),
            start: std::time::Instant::now(),
        }
    }

    /// If --wait specified, we don't want the default behavior of
    /// waiting for virt-viewer to exit, we want to launch it, then
    /// manually count down time for ourselves
    fn wait_for_console_to_exit(&self) -> bool {
        !self.wait_is_requested
    }

    fn wait_forever(&self) -> bool {
        self.wait_mins < 0
    }

    fn get_time_string(&self) -> String {
        if self.wait_forever() {
            return "Waiting for the installation to complete.".into();
        }
        let unit = if self.wait_mins == 1 {
            "minute"
        } else {
            "minutes"
        };
        format!(
            "Waiting {} {unit} for the installation to complete.",
            self.wait_mins
        )
    }

    /// Sleeps 1 second, then returns whether the wait time expired.
    fn wait(&self) -> bool {
        sleep(1);
        if self.wait_forever() {
            return in_testsuite();
        }
        self.start.elapsed().as_secs() as i64 >= self.wait_mins * 60 || in_testsuite()
    }
}

fn sleep(secs: u64) {
    if !in_testsuite() {
        std::thread::sleep(std::time::Duration::from_secs(secs));
    }
}

/// `_InstalledDomain`: the domain after the initial install creation.
struct InstalledDomain {
    domain: virt::domain::Domain,
    transient: bool,
    destroy_on_exit: bool,
}

impl InstalledDomain {
    fn handle_destroy_on_exit(&self) {
        if self.destroy_on_exit && self.domain.is_active().unwrap_or(false) {
            log::debug!("console exited and destroy_on_exit passed, destroying");
            let _ = self.domain.destroy();
        }
    }

    fn domain_was_destroyed(&self) -> bool {
        match self.domain.get_state() {
            Ok((state, reason)) => {
                state == virt::sys::VIR_DOMAIN_SHUTOFF
                    && [
                        virt::sys::VIR_DOMAIN_SHUTOFF_DESTROYED as i32,
                        virt::sys::VIR_DOMAIN_SHUTOFF_SAVED as i32,
                    ]
                    .contains(&reason)
            }
            Err(_) => {
                log::debug!("Error checking VM shutdown reason");
                false
            }
        }
    }

    fn check_inactive(&self) -> bool {
        match self.domain.get_info() {
            Ok(info) => {
                if info.state == virt::sys::VIR_DOMAIN_CRASHED {
                    fail("Domain has crashed.");
                }
                !self.domain.is_active().unwrap_or(false)
            }
            Err(e) => {
                if e.code() != virt::error::ErrorNumber::NoDomain {
                    fail(e.message());
                }
                if self.transient {
                    log::debug!("transient VM shutdown and disappeared.");
                    return true;
                }
                fail(format!("VM disappeared unexpectedly: {}", e.message()));
            }
        }
    }
}

/// `_connect_console`.
fn connect_console(
    guest: &XmlObject,
    conn: &Connection,
    instdomain: &InstalledDomain,
    autoconsole: &AutoconsoleData,
    wait: bool,
) {
    let Some(mut child) = launch_console(autoconsole, guest, conn) else {
        return;
    };
    if !wait {
        return;
    }
    // If we connected the console, wait for it to finish
    match child.wait() {
        Ok(status) if !status.success() => log::warn!("Console command returned failure."),
        Ok(_) => {}
        Err(e) => log::debug!("waitpid error: {e}"),
    }
    instdomain.handle_destroy_on_exit();
}

/// `_wait_for_domain`: make sure domain ends up in expected state, and
/// wait if for install to complete if requested.
fn wait_for_domain(
    installer: &Installer,
    instdomain: &InstalledDomain,
    autoconsole: &AutoconsoleData,
    waithandler: &WaitHandler,
) {
    if instdomain.check_inactive() {
        return;
    }
    if autoconsole.has_console_cb() {
        // We are trying to detect if the VM shutdown, or the user
        // just closed the console and the VM is still running. In the
        // the former case, libvirt may not have caught up yet with the
        // VM having exited, so wait a bit and check again
        sleep(2);
        if instdomain.check_inactive() {
            return;
        }
    }

    // If we reach here, the VM still appears to be running.
    let mut msg = "\nDomain is still running. Installation may be in progress.".to_string();
    if !waithandler.wait_is_requested {
        // User either:
        //   used --noautoconsole
        //   killed console and guest is still running
        if !installer.requires_postboot_xml_changes() {
            return;
        }
        msg += "\nYou can reconnect to the console to complete the installation process.";
        print_stdout(&msg, false);
        std::process::exit(0);
    }

    print_stdout(&msg, false);
    print_stdout(&waithandler.get_time_string(), false);

    // Wait loop
    loop {
        if instdomain.check_inactive() {
            print_stdout("Domain has shutdown. Continuing.", false);
            break;
        }
        if waithandler.wait() {
            print_stdout(
                "Installation has exceeded specified time limit. Exiting application.",
                false,
            );
            std::process::exit(1);
        }
    }
}

/// `_process_domain`: the pieces of the install process after the
/// initial VM startup.
#[allow(clippy::too_many_arguments)]
fn process_domain(
    domain: virt::domain::Domain,
    guest: &XmlObject,
    conn: &Connection,
    installer: &Installer,
    waithandler: &WaitHandler,
    autoconsole: &AutoconsoleData,
    args: &Args,
) -> virt::domain::Domain {
    let instdomain = InstalledDomain {
        domain,
        transient: args.transient,
        destroy_on_exit: args.destroy_on_exit,
    };
    connect_console(
        guest,
        conn,
        &instdomain,
        autoconsole,
        waithandler.wait_for_console_to_exit(),
    );

    // Trigger specific behavior checking if user destroyed the domain
    if std::env::var_os("VIRTINST_TESTSUITE_HACK_DESTROY").is_some() {
        let _ = instdomain.domain.destroy();
    }
    wait_for_domain(installer, &instdomain, autoconsole, waithandler);
    print_stdout("Domain creation completed.", false);

    let domain = &instdomain.domain;
    if args.transient || domain.is_active().unwrap_or(false) {
        return instdomain.domain;
    }
    if args.noreboot || !installer.requires_postboot_xml_changes() {
        print_stdout(
            &format!(
                "You can restart your domain by running:\n  {}",
                virsh_start_cmd(guest, conn)
            ),
            false,
        );
        return instdomain.domain;
    }
    if instdomain.domain_was_destroyed() && !args.destroy_on_exit {
        print_stdout("User stopped the VM. Not rebooting.", false);
        return instdomain.domain;
    }
    print_stdout("Restarting guest.", false);
    if let Err(e) = domain.create() {
        fail(e.message());
    }
    connect_console(guest, conn, &instdomain, autoconsole, true);
    instdomain.domain
}

/// `_destroy_on_exit`.
fn destroy_on_exit(domain: &virt::domain::Domain) {
    if domain.is_active().unwrap_or(false) {
        let _ = domain.destroy();
    }
}

/// `build_storage` for every new disk.
fn storage_builder<'a>(
    b: &'a [virtinst_core::storage::VolInstall],
    conn: &'a Connection,
) -> impl Fn() -> Result<(), String> + 'a {
    move || {
        for vi in b {
            parsers::disk::build_storage(vi, conn.as_connect())?;
        }
        Ok(())
    }
}

/// `start_install`.
fn start_install(b: &mut Built, args: &mut Args, conn: &Connection) {
    let autoconsole = AutoconsoleData::new(&args.autoconsole(), &b.guest);
    show_console_warnings(&b.installer, &autoconsole);
    // `_set_default_wait`
    if args.wait.is_none() && !autoconsole.has_console_cb() && autoconsole.is_default {
        // If there isn't any console to actually connect up,
        // default to --wait -1 to get similarish behavior
        log::warn!("No console to launch for the guest, defaulting to --wait -1");
        args.wait = Some(-1);
    }
    let waithandler = WaitHandler::new(args.wait);

    // we've got everything -- try to start the install
    print_stdout("\nStarting install...", false);

    let vols = std::mem::take(&mut b.vol_installs);
    let build = storage_builder(&vols, conn);
    let res = b.installer.start_install(
        &mut b.guest,
        &mut b.opts,
        conn,
        false,
        false,
        !args.noreboot,
        args.transient,
        &build,
    );
    let domain = match res {
        Ok(Started::Domain(d)) => d,
        Ok(Started::Xml(..)) => unreachable!("not asked for XML"),
        Err(e) => {
            cleanup_created_disks(&vols, conn);
            log::error!("{e}");
            install_fail(&b.guest, conn);
        }
    };
    let domain = process_domain(
        domain,
        &b.guest,
        conn,
        &b.installer,
        &waithandler,
        &autoconsole,
        args,
    );
    if in_testsuite() && args.destroy_on_exit {
        // Helps with unit testing
        destroy_on_exit(&domain);
    }
}

/// `Installer.cleanup_created_disks`: remove any disks we created as
/// part of the install.
fn cleanup_created_disks(vols: &[virtinst_core::storage::VolInstall], conn: &Connection) {
    for vi in vols {
        let Ok(pool) =
            virt::storage_pool::StoragePool::lookup_by_name(conn.as_connect(), &vi.pool_name)
        else {
            continue;
        };
        let Ok(vol) = virt::storage_vol::StorageVol::lookup_by_name(&pool, &vi.name) else {
            continue;
        };
        log::debug!("Removing created disk vol_object={}", vi.name);
        if let Err(e) = vol.delete(0) {
            log::debug!("Failed to remove disk '{}'", vi.name);
            log::error!("Failed to remove disk '{}': {}", vi.name, e.message());
        }
    }
}

/// `xml_to_print`.
fn xml_to_print(b: &mut Built, args: &Args, conn: &Connection) -> Option<String> {
    let vols = std::mem::take(&mut b.vol_installs);
    let build = storage_builder(&vols, conn);
    let res = b
        .installer
        .start_install(
            &mut b.guest,
            &mut b.opts,
            conn,
            args.dry,
            true,
            true,
            false,
            &build,
        )
        .unwrap_or_else(|e| fail(e));
    let Started::Xml(start_xml, final_xml) = res else {
        unreachable!("asked for XML");
    };
    let (start_xml, final_xml) = match start_xml {
        Some(s) => (s, Some(final_xml)),
        None => (final_xml, None),
    };

    if args.dry && args.xmlonly.is_none() {
        print_stdout("Dry run completed successfully", false);
        return None;
    }
    let xmlonly = args.xmlonly.as_deref().unwrap_or("all");
    if !["1", "2", "all"].contains(&xmlonly) {
        fail(format!(
            "Unknown XML step request '{xmlonly}', must be 1, 2, or all"
        ));
    }
    if xmlonly == "1" {
        return Some(start_xml);
    }
    if xmlonly == "2" {
        return Some(
            final_xml.unwrap_or_else(|| fail("Requested installation does not have XML step 2")),
        );
    }
    // "all" case
    Some(start_xml + &final_xml.unwrap_or_default())
}

fn main() {
    cli::handle_version_flag();
    let mut args: Args = cli::argparse_parse("virt-install", "OPTIONS");

    // `convert_old_printxml`
    if let Some(step) = args.xmlstep.take() {
        args.xmlonly = Some(step);
    }
    let quiet = args.xmlonly.is_some() || args.test_media_detection.is_some() || args.quiet;
    cli::setup_logging("virt-install", args.debug, quiet);

    // `check_option_introspection`: `--disk=?`/`--disk help`.
    let mut introspected = false;
    for (def, values) in virt_parsers(&args) {
        if values.iter().any(|v| v == "?" || v == "help") {
            print!("{}", def.introspection());
            introspected = true;
        }
    }
    // The rest of `_get_completer_parsers`, which have no ParserDef here.
    let extra: [(&str, Vec<&String>, &[&str]); 6] = [
        (
            "check",
            args.check.iter().collect(),
            &[
                "all",
                "disk_size",
                "mac_in_use",
                "path_exists",
                "path_in_use",
            ],
        ),
        (
            "location",
            args.location.iter().collect(),
            &["initrd", "kernel", "location"],
        ),
        (
            "unattended",
            args.unattended.iter().collect(),
            &[
                "admin-password-file",
                "product-key",
                "profile",
                "reg-login",
                "user-login",
                "user-password-file",
            ],
        ),
        (
            "install",
            args.install.iter().collect(),
            &[
                "bootdev",
                "initrd",
                "kernel",
                "kernel_args",
                "kernel_args_overwrite",
                "no_install",
                "os",
            ],
        ),
        (
            "cloud-init",
            args.cloud_init.iter().collect(),
            &[
                "clouduser-ssh-key",
                "disable",
                "meta-data",
                "network-config",
                "root-password-file",
                "root-password-generate",
                "root-ssh-key",
                "user-data",
            ],
        ),
        (
            "osinfo",
            args.osinfo.iter().collect(),
            &["detect", "id", "name", "require", "short-id"],
        ),
    ];
    for (flag, values, names) in extra {
        if values.iter().any(|v| *v == "?" || *v == "help") {
            println!("--{flag} options:");
            for n in names {
                println!("  {n}");
            }
            println!();
            introspected = true;
        }
    }
    if introspected {
        return;
    }
    if check_osinfo_list(&args) {
        return;
    }

    check_cdrom_option_error(&mut args);
    cli::convert_old_force(args.force, &mut args.check);
    cli::parse_check(&args.check);
    cli::set_prompt(args.prompt);
    convert_old_memory(&mut args);
    convert_old_networks(&mut args);
    convert_old_graphics(&mut args);
    convert_old_disks(&mut args);
    convert_old_features(&mut args);
    convert_old_cpuset(&mut args);
    convert_old_init(&mut args);
    convert_wait_zero(&mut args);
    set_test_stub_options(&mut args);
    convert_old_os_options(&mut args);

    let conn = cli::get_connection(args.connect.as_deref());
    parser::set_conn_support(cli::conn_support(&conn));
    parser::defer_xml_action_checks();
    parser::set_connect(Some(conn.as_connect().clone()));

    let caps = Capabilities::fetch(&conn).unwrap_or_else(|e| fail(e));
    if args.test_media_detection.is_some() {
        do_test_media_detection(&args, &conn, &caps);
        return;
    }

    let mut b = build_guest_instance(&mut args, &conn, &caps);
    if args.xmlonly.is_some() || args.dry {
        if let Some(xml) = xml_to_print(&mut b, &args, &conn) {
            print_stdout(&xml, true);
        }
    } else {
        start_install(&mut b, &mut args, &conn);
    }
}
