//! `NewDevice` — ticket 12's Add Hardware wizard enum. 17 mutually
//! exclusive variants, one per `addhardware.py` `hw-list` *page*
//! (`PAGE_DISK`..`PAGE_VSOCK`), compile-time exhaustive over the real
//! XML-bound structs in this module — not the `DiskConfig`/
//! `ControllerConfig` placeholder names ticket 12's resolution text
//! used. Those names predate ticket 06's derive-macro structs; there
//! was nothing for them to refer to yet when 12 was resolved. The
//! shape ticket 12 decided (17-variant enum-of-structs, controller
//! dependency folded inside its variant) is unchanged, only the type
//! names are now real.
//!
//! **`Char` is one variant wrapping [`CharDevice`], not `DeviceSerial`
//! alone.** `addhardware.py`'s `hw-list` has four separate user-facing
//! rows — "Serial"/"Parallel"/"Console"/"Channel" — that all land on
//! `PAGE_CHAR` and get dispatched to one of four distinct device
//! classes by `_get_char_class()` (keyed off which row was selected,
//! not a combo on the page itself). Collapsing straight to `DeviceSerial`
//! would silently mislabel the other three rows' devices. One
//! `NewDevice::Char` variant still matches the page count ticket 12
//! counted (17), it just needs an inner selector to stay lossless.
//!
//! **Controller dependency stays inside `Disk`.** Upstream's
//! `vmm_controller` attribute — a new disk implicitly needing a new
//! SCSI controller — is the one companion-controller case ticket 12
//! actually traced. Not added speculatively to other variants; nothing
//! else in `addhardware.py` showed the same pattern.
//!
//! **Not this module's job:** the `attach_device()`-then-`add_device()`
//! live+persistent sequencing ticket 12 also resolved lives in
//! `connection` (ticket 15), against a real libvirt connection — still
//! a stub. Per-driver default resolution (`dev.set_defaults(...)`,
//! ticket 17) is a separate not-yet-built pipeline stage. This module
//! is the data model the wizard fills in, nothing downstream of it.

use virtinst_xml::XmlBound;

use super::{
    DeviceChannel, DeviceConsole, DeviceController, DeviceDisk, DeviceFilesystem, DeviceGraphics,
    DeviceHostdev, DeviceInput, DeviceNetwork, DevicePanic, DeviceParallel, DeviceRedirdev,
    DeviceRng, DeviceSerial, DeviceSmartcard, DeviceSound, DeviceTpm, DeviceVideo, DeviceVsock,
    DeviceWatchdog,
};

/// The device class actually selected by which of the four `hw-list`
/// char rows the user picked — upstream's `_get_char_class()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CharDevice {
    Serial(DeviceSerial),
    Parallel(DeviceParallel),
    Console(DeviceConsole),
    Channel(DeviceChannel),
}

impl CharDevice {
    /// The XML tag this variant's wrapped struct binds to.
    pub fn tag(&self) -> &'static str {
        match self {
            CharDevice::Serial(_) => DeviceSerial::TAG,
            CharDevice::Parallel(_) => DeviceParallel::TAG,
            CharDevice::Console(_) => DeviceConsole::TAG,
            CharDevice::Channel(_) => DeviceChannel::TAG,
        }
    }
}

/// One device-type selection from the Add Hardware wizard's `hw-list`,
/// per ticket 12.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewDevice {
    /// `vmm_controller`: an implicit companion controller (e.g. a new
    /// SCSI controller for a new disk) attaches/persists before the
    /// disk itself when present.
    Disk {
        device: DeviceDisk,
        companion_controller: Option<DeviceController>,
    },
    Controller(DeviceController),
    Network(DeviceNetwork),
    Input(DeviceInput),
    Graphics(DeviceGraphics),
    Sound(DeviceSound),
    Hostdev(DeviceHostdev),
    Char(CharDevice),
    Video(DeviceVideo),
    Watchdog(DeviceWatchdog),
    Filesystem(DeviceFilesystem),
    Smartcard(DeviceSmartcard),
    Redirdev(DeviceRedirdev),
    Tpm(DeviceTpm),
    Rng(DeviceRng),
    Panic(DevicePanic),
    Vsock(DeviceVsock),
}

impl NewDevice {
    /// The XML tag the selected device ultimately serializes as —
    /// `Disk`'s companion controller (if any) is a separate element,
    /// not reflected here.
    pub fn tag(&self) -> &'static str {
        match self {
            NewDevice::Disk { .. } => DeviceDisk::TAG,
            NewDevice::Controller(_) => DeviceController::TAG,
            NewDevice::Network(_) => DeviceNetwork::TAG,
            NewDevice::Input(_) => DeviceInput::TAG,
            NewDevice::Graphics(_) => DeviceGraphics::TAG,
            NewDevice::Sound(_) => DeviceSound::TAG,
            NewDevice::Hostdev(_) => DeviceHostdev::TAG,
            NewDevice::Char(c) => c.tag(),
            NewDevice::Video(_) => DeviceVideo::TAG,
            NewDevice::Watchdog(_) => DeviceWatchdog::TAG,
            NewDevice::Filesystem(_) => DeviceFilesystem::TAG,
            NewDevice::Smartcard(_) => DeviceSmartcard::TAG,
            NewDevice::Redirdev(_) => DeviceRedirdev::TAG,
            NewDevice::Tpm(_) => DeviceTpm::TAG,
            NewDevice::Rng(_) => DeviceRng::TAG,
            NewDevice::Panic(_) => DevicePanic::TAG,
            NewDevice::Vsock(_) => DeviceVsock::TAG,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cheap round-trip check this enum actually has: every arm
    /// (including all four `CharDevice` sub-variants) reports the tag
    /// its wrapped struct really binds to. Catches a mis-wired variant
    /// silently emitting the wrong element — the exact failure mode
    /// collapsing `Char` straight to `DeviceSerial` would have caused.
    #[test]
    fn every_variant_tag_matches_its_wrapped_struct() {
        assert_eq!(
            NewDevice::Disk {
                device: DeviceDisk::default(),
                companion_controller: None,
            }
            .tag(),
            "disk"
        );
        assert_eq!(
            NewDevice::Controller(DeviceController::default()).tag(),
            "controller"
        );
        assert_eq!(
            NewDevice::Network(DeviceNetwork::default()).tag(),
            "interface"
        );
        assert_eq!(NewDevice::Input(DeviceInput::default()).tag(), "input");
        assert_eq!(
            NewDevice::Graphics(DeviceGraphics::default()).tag(),
            "graphics"
        );
        assert_eq!(NewDevice::Sound(DeviceSound::default()).tag(), "sound");
        assert_eq!(
            NewDevice::Hostdev(DeviceHostdev::default()).tag(),
            "hostdev"
        );
        assert_eq!(NewDevice::Video(DeviceVideo::default()).tag(), "video");
        assert_eq!(
            NewDevice::Watchdog(DeviceWatchdog::default()).tag(),
            "watchdog"
        );
        assert_eq!(
            NewDevice::Filesystem(DeviceFilesystem::default()).tag(),
            "filesystem"
        );
        assert_eq!(
            NewDevice::Smartcard(DeviceSmartcard::default()).tag(),
            "smartcard"
        );
        assert_eq!(
            NewDevice::Redirdev(DeviceRedirdev::default()).tag(),
            "redirdev"
        );
        assert_eq!(NewDevice::Tpm(DeviceTpm::default()).tag(), "tpm");
        assert_eq!(NewDevice::Rng(DeviceRng::default()).tag(), "rng");
        assert_eq!(NewDevice::Panic(DevicePanic::default()).tag(), "panic");
        assert_eq!(NewDevice::Vsock(DeviceVsock::default()).tag(), "vsock");

        assert_eq!(
            NewDevice::Char(CharDevice::Serial(DeviceSerial::default())).tag(),
            "serial"
        );
        assert_eq!(
            NewDevice::Char(CharDevice::Parallel(DeviceParallel::default())).tag(),
            "parallel"
        );
        assert_eq!(
            NewDevice::Char(CharDevice::Console(DeviceConsole::default())).tag(),
            "console"
        );
        assert_eq!(
            NewDevice::Char(CharDevice::Channel(DeviceChannel::default())).tag(),
            "channel"
        );
    }

    #[test]
    fn disk_companion_controller_is_independent_of_disk_tag() {
        // The controller travels with the variant but isn't what
        // `.tag()` reports - guards against someone later collapsing
        // the two together.
        let dev = NewDevice::Disk {
            device: DeviceDisk::default(),
            companion_controller: Some(DeviceController::default()),
        };
        assert_eq!(dev.tag(), "disk");
        match dev {
            NewDevice::Disk {
                companion_controller,
                ..
            } => assert!(companion_controller.is_some()),
            _ => unreachable!(),
        }
    }
}
