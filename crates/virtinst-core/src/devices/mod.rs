//! One typed struct per device type — upstream's `devices/*.py` (20 files).
//!
//! Two enums assemble these for different flows, both compile-time
//! exhaustive over the same per-device structs here:
//! - [`NewDevice`](crate::devices) (ticket 12, Add Hardware wizard):
//!   17 mutually-exclusive variants, one per device type.
//! - `InstallMethod`'s device-adjacent fields (ticket 08, Create VM).
//!
//! Per-driver default resolution (ticket 17) lives as a method on each
//! type here, taking [`crate::capabilities`] — a direct-ported ordered
//! decision chain per field (upstream's `set_defaults`/`_default_bus`
//! shape), not a driver-strategy trait. Only ~9 of these files actually
//! branch on driver family upstream; this isn't a pervasive concern.

pub mod disk;
pub use disk::DeviceDisk;

pub mod network;
pub use network::DeviceNetwork;

pub mod graphics;
pub use graphics::DeviceGraphics;

pub mod controller;
pub use controller::DeviceController;

pub mod input;
pub use input::DeviceInput;

pub mod sound;
pub use sound::DeviceSound;

pub mod filesystem;
pub use filesystem::DeviceFilesystem;

pub mod hostdev;
pub use hostdev::DeviceHostdev;

pub mod char;
pub use char::{DeviceChannel, DeviceConsole, DeviceParallel, DeviceSerial};

pub mod tpm;
pub use tpm::DeviceTpm;

pub mod rng;
pub use rng::DeviceRng;

pub mod panic;
pub use panic::DevicePanic;

pub mod vsock;
pub use vsock::DeviceVsock;

pub mod watchdog;
pub use watchdog::DeviceWatchdog;

pub mod smartcard;
pub use smartcard::{Certificate, DeviceSmartcard};

pub mod redirdev;
pub use redirdev::DeviceRedirdev;

pub mod video;
pub use video::DeviceVideo;

pub mod list;
pub use list::DeviceList;

pub mod new_device;
pub use new_device::{CharDevice, NewDevice};

// All 17 of ticket 12's NewDevice variants now have real XML bindings,
// and the enum itself is built (`new_device.rs`) — including all four
// of Char's tags, needed once addhardware.py's hw-list showed Char
// covers four distinct user-facing rows, not one. Still ahead:
// per-driver default-resolution methods (ticket 17), and the CLI's
// derive-macro-generated option-string parsers (ticket 16).
