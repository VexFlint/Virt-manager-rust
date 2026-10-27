//! `virt-clone` — upstream's `virtclone.py`, option for option, over
//! `virtinst_core::cloner::Cloner` (the port of `cloner.py`).
//!
//! Option prefixes work like argparse's (`--auto` is `--auto-clone`,
//! `--preserve` is `--preserve-data`), and so does repeating a
//! single-value option (the last one wins). `--prompt`/`--force` are the
//! same hidden back-compat flags upstream keeps.

use clap::Parser;
use virtinst_cli::cli::{self, fail, print_stdout, DiskCheck};
use virtinst_core::cloner::Cloner;

const ABOUT: &str = "Duplicate a virtual machine, changing all the unique host side configuration \
like MAC address, name, etc. \n\n\
The VM contents are NOT altered: virt-clone does not change anything _inside_ the guest OS, it \
only duplicates disks and does host side changes. So things like changing passwords, changing \
static IP address, etc are outside the scope of this tool. For these types of changes, please \
see virt-sysprep(1).";

#[derive(Parser)]
#[command(
    name = "virt-clone",
    version,
    override_usage = "virt-clone --original [NAME] ...",
    about = ABOUT,
    after_help = "See man page for examples and full option syntax.",
    infer_long_args = true,
    args_override_self = true
)]
struct Args {
    /// Connect to hypervisor with libvirt URI
    #[arg(long, value_name = "URI")]
    connect: Option<String>,

    /// Name of the original guest to clone.
    #[arg(short = 'o', long = "original", help_heading = "General Options")]
    src_name: Option<String>,

    /// XML file to use as the original guest.
    #[arg(long, help_heading = "General Options")]
    original_xml: Option<String>,

    /// Auto generate clone name and storage paths from the original guest configuration.
    #[arg(long, help_heading = "General Options")]
    auto_clone: bool,

    /// Name for the new guest
    #[arg(short = 'n', long = "name", help_heading = "General Options")]
    new_name: Option<String>,

    #[arg(short = 'u', long = "uuid", hide = true)]
    new_uuid: Option<String>,

    /// use btrfs COW lightweight copy
    #[arg(long, help_heading = "General Options")]
    reflink: bool,

    /// New file to use as the disk image for the new guest
    #[arg(short = 'f', long = "file", help_heading = "Storage Configuration")]
    new_diskfile: Vec<String>,

    /// Force to copy devices (eg, if 'hdc' is a readonly cdrom device, --force-copy=hdc)
    #[arg(long = "force-copy", help_heading = "Storage Configuration")]
    target: Vec<String>,

    /// Skip copy of the device target. (eg, if 'vda' is a disk you don't want to copy and use
    /// the same path in the new VM, use --skip-copy=vda)
    #[arg(long, help_heading = "Storage Configuration")]
    skip_copy: Vec<String>,

    /// Do not use a sparse file for the clone's disk image
    #[arg(long, help_heading = "Storage Configuration")]
    nonsparse: bool,

    /// Do not clone storage contents to specified file paths, their contents will be left
    /// untouched. This requires specifying existing paths for every cloneable disk image.
    #[arg(long = "preserve-data", help_heading = "Storage Configuration")]
    preserve: bool,

    /// New file to use as storage for nvram VARS
    #[arg(long = "nvram", help_heading = "Storage Configuration")]
    new_nvram: Option<String>,

    /// New fixed MAC address for the clone guest. Default is a randomly generated MAC
    #[arg(short = 'm', long = "mac", help_heading = "Networking Configuration")]
    new_mac: Vec<String>,

    /// Just used for clone tests.
    #[arg(long = "__test-nodry", hide = true)]
    test_nodry: bool,

    #[arg(long, hide = true)]
    prompt: bool,

    #[arg(long, hide = true)]
    force: bool,

    /// Don't check name collision, overwrite any guest with the same name.
    #[arg(long, help_heading = "Miscellaneous Options")]
    replace: bool,

    /// Print the generated domain XML rather than create the guest.
    #[arg(long = "print-xml", help_heading = "Miscellaneous Options")]
    xmlonly: bool,

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

/// `_process_src`.
fn process_src(args: &Args) -> (Option<String>, Option<String>) {
    let src_xml = args.original_xml.as_ref().map(|path| {
        std::fs::read_to_string(path).unwrap_or_else(|e| {
            fail(format!(
                "[Errno {}] {e}: '{path}'",
                e.raw_os_error().unwrap_or(0)
            ))
        })
    });
    if src_xml.is_none() && args.src_name.is_none() {
        fail("An original machine name is required, use '--original src_name' and try again.");
    }
    (args.src_name.clone(), src_xml)
}

/// `_process_macs`.
fn process_macs(args: &Args, cloner: &mut Cloner) {
    let mut new_macs = args.new_mac.clone();
    if new_macs.is_empty() || new_macs[0] == "RANDOM" {
        return;
    }
    for mac in &new_macs {
        cli::validate_mac(cloner.conn(), mac);
    }
    for idx in 0..cloner.interface_count() {
        if new_macs.is_empty() {
            fail("pop from empty list");
        }
        let mac = new_macs.remove(0);
        cloner.set_interface_mac(idx, &mac);
    }
}

/// `_process_disks`.
fn process_disks(args: &Args, cloner: &mut Cloner) {
    let mut newpaths = args.new_diskfile.clone();
    for idx in cloner.nonshare_indexes() {
        let origpath = cloner.diskinfos()[idx].get_source_path();
        let mut newpath = None;
        if !newpaths.is_empty() {
            newpath = Some(newpaths.remove(0));
        } else if args.auto_clone {
            break;
        }
        if origpath.is_none() {
            newpath = None;
        }
        cloner.set_disk_new_path(idx, newpath.as_deref(), !args.nonsparse);
        if let Err(e) = cloner.diskinfos()[idx].raise_error() {
            fail(e);
        }
    }
}

/// `_validate_disks`: extra CLI validation for specified disks.
fn validate_disks(cloner: &Cloner) {
    for diskinfo in cloner.diskinfos() {
        if let Err(e) = diskinfo.raise_error() {
            fail(e);
        }
        let Some(new_disk) = &diskinfo.new_disk else {
            continue;
        };
        let warn_overwrite = !diskinfo.is_preserve_requested();
        let check = DiskCheck {
            path: new_disk.get_source_path(),
            shareable: false,
            read_only: false,
            size_conflict: new_disk.is_size_conflict(),
        };
        cli::validate_disk(cloner.conn(), &check, warn_overwrite);
    }
}

fn main() {
    let mut args = Args::parse();

    let quiet = args.quiet || args.xmlonly;
    cli::setup_logging("virt-clone", args.debug, quiet);

    cli::convert_old_force(args.force, &mut args.check);
    cli::parse_check(&args.check);
    cli::set_prompt(args.prompt);
    let conn = cli::get_connection(args.connect.as_deref());

    if args.new_diskfile.is_empty() && !args.auto_clone {
        fail("Either --auto-clone or --file is required, use '--auto-clone or --file' and try again.");
    }

    let (src_name, src_xml) = process_src(&args);
    let mut cloner =
        Cloner::new(&conn, src_name.as_deref(), src_xml.as_deref()).unwrap_or_else(|e| fail(e));

    cloner.set_replace(args.replace);
    cloner.set_reflink(args.reflink);
    cloner.set_sparse(!args.nonsparse);

    if let Some(uuid) = &args.new_uuid {
        cloner.set_clone_uuid(uuid);
    }
    if let Some(nvram) = &args.new_nvram {
        cloner.set_nvram_path(nvram);
    }

    for diskinfo in cloner.diskinfos_mut() {
        let target = diskinfo.target.clone().unwrap_or_default();
        if args.target.contains(&target) {
            diskinfo.set_clone_requested();
        }
        if args.skip_copy.contains(&target) {
            diskinfo.set_share_requested();
        }
    }

    if args.preserve {
        for idx in cloner.nonshare_indexes() {
            cloner.diskinfos_mut()[idx].set_preserve_requested();
        }
        if let Some(nvram) = cloner.nvram_diskinfo_mut() {
            nvram.set_preserve_requested();
        }
    }

    if let Some(name) = &args.new_name {
        cloner.set_clone_name(name);
    } else if !args.auto_clone {
        fail("A name is required for the new virtual machine, use '--name NEW_VM_NAME' to specify one.");
    }

    process_macs(&args, &mut cloner);
    process_disks(&args, &mut cloner);

    if let Err(e) = cloner.prepare() {
        fail(e);
    }

    validate_disks(&cloner);

    let mut run = true;
    if args.xmlonly {
        run = args.test_nodry;
        print_stdout(&cloner.new_xml(), true);
    }
    if run {
        if let Err(e) = cloner.start_duplicate(&mut cli::get_meter()) {
            fail(e);
        }
        print_stdout("", false);
        print_stdout(
            &format!("Clone '{}' created successfully.", cloner.new_name()),
            false,
        );
    }
}
