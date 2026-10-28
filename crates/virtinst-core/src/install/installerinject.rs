//! `install/installerinject.py`: putting files into an initrd (cpio)
//! or into a generated ISO.

use std::process::{Command, Stdio};

use super::urlfetcher::{basename, mkdtemp, named_tempfile, path_join};
use crate::diskbackend::oserror_str;

/// `_run_initrd_commands`: append `tempdir`'s files to `initrd` as a
/// gzipped newc cpio archive.
fn run_initrd_commands(initrd: &str, tempdir: &str) -> Result<(), String> {
    log::debug!("Appending to the initrd.");
    let out = std::fs::OpenOptions::new()
        .append(true)
        .open(initrd)
        .map_err(|e| oserror_str(&e, Some(initrd)))?;
    let mut find = Command::new("find")
        .args([".", "-print0"])
        .current_dir(tempdir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| oserror_str(&e, Some("find")))?;
    let mut cpio = Command::new("cpio")
        .args([
            "--create",
            "--null",
            "--quiet",
            "--format=newc",
            "--owner=0:0",
        ])
        .current_dir(tempdir)
        .stdin(find.stdout.take().expect("piped"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| oserror_str(&e, Some("cpio")))?;
    let gzip = Command::new("gzip")
        .stdin(cpio.stdout.take().expect("piped"))
        .stdout(out)
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| oserror_str(&e, Some("gzip")))?;
    let gzip = gzip.wait_with_output().map_err(|e| e.to_string())?;
    let cpio = cpio.wait_with_output().map_err(|e| e.to_string())?;
    let find = find.wait_with_output().map_err(|e| e.to_string())?;
    for (name, o) in [("find", &find), ("cpio", &cpio), ("gzip", &gzip)] {
        if !o.stderr.is_empty() {
            log::debug!("{name} stderr={}", String::from_utf8_lossy(&o.stderr));
        }
    }
    if !(cpio.status.success() && find.status.success() && gzip.status.success()) {
        return Err("Failed to inject files into initrd".into());
    }
    Ok(())
}

/// `_run_iso_commands`: build `iso` from `tempdir`.
fn run_iso_commands(iso: &str, tempdir: &str, cloudinit: bool) -> Result<(), String> {
    // These three programs all behave similarly for our needs, and
    // different distros only have some available. xorriso is apparently
    // the actively maintained variant that should be available everywhere
    // and without any license issues. Some more info here:
    // https://wiki.debian.org/genisoimage
    let programs = ["xorrisofs", "genisoimage", "mkisofs"];
    let in_path = |p: &str| {
        std::env::var("PATH")
            .unwrap_or_default()
            .split(':')
            .any(|d| std::path::Path::new(&path_join(d, p)).is_file())
    };
    let program = programs
        .iter()
        .find(|p| in_path(p))
        .unwrap_or(&programs[programs.len() - 1]);
    let mut cmd: Vec<&str> = vec!["-o", iso, "-J", "-input-charset", "utf8", "-rational-rock"];
    if cloudinit {
        cmd.extend(["-V", "cidata"]);
    }
    cmd.push(tempdir);
    log::debug!("Running iso build command: {program} {:?}", cmd);
    let output = Command::new(program)
        .args(&cmd)
        .output()
        .map_err(|e| oserror_str(&e, Some(program)))?;
    let mut text = output.stdout;
    text.extend(output.stderr);
    log::debug!("cmd output: {}", String::from_utf8_lossy(&text));
    if !output.status.success() {
        return Err(format!(
            "Command '{program}' returned non-zero exit status {}.",
            output.status.code().unwrap_or(-1)
        ));
    }
    Ok(())
}

/// `_perform_generic_injections`: copy each `(src, dst)` into a temp
/// dir under `scratchdir`, then run `cb` on it.
fn perform_generic_injections(
    injections: &[(String, String)],
    scratchdir: &str,
    media: &str,
    cb: impl FnOnce(&str) -> Result<(), String>,
) -> Result<(), String> {
    if injections.is_empty() {
        return Ok(());
    }
    use std::os::unix::fs::PermissionsExt;
    let tempdir = mkdtemp(scratchdir)?;
    let res = (|| {
        std::fs::set_permissions(&tempdir, std::fs::Permissions::from_mode(0o775))
            .map_err(|e| oserror_str(&e, Some(&tempdir)))?;
        for (src, dst) in injections {
            log::debug!("Injecting src={src} dst={dst} into media={media}");
            std::fs::copy(src, path_join(&tempdir, dst)).map_err(|e| oserror_str(&e, Some(src)))?;
        }
        cb(&tempdir)
    })();
    let _ = std::fs::remove_dir_all(&tempdir);
    res
}

/// An injection: `(src, dst)`, or a bare path whose basename is `dst`.
pub fn injection_pair(filename: &str) -> (String, String) {
    (filename.to_string(), basename(filename).to_string())
}

/// `perform_initrd_injections`: insert files into the root directory of
/// the initial ram disk.
pub fn perform_initrd_injections(
    initrd: &str,
    injections: &[(String, String)],
    scratchdir: &str,
) -> Result<(), String> {
    perform_generic_injections(injections, scratchdir, initrd, |tempdir| {
        run_initrd_commands(initrd, tempdir)
    })
}

/// `perform_cdrom_injections`: insert files into the root directory of a
/// generated cdrom, and return the ISO's path.
pub fn perform_cdrom_injections(
    injections: &[(String, String)],
    scratchdir: &str,
    cloudinit: bool,
) -> Result<String, String> {
    let iso_suffix = if cloudinit {
        "-cloudinit.iso"
    } else {
        "-unattended.iso"
    };
    let (_, iso) = named_tempfile(scratchdir, "virtinst-", iso_suffix)?;
    let res = perform_generic_injections(injections, scratchdir, &iso, |tempdir| {
        run_iso_commands(&iso, tempdir, cloudinit)
    });
    if let Err(e) = res {
        let _ = std::fs::remove_file(&iso);
        return Err(e);
    }
    Ok(iso)
}
