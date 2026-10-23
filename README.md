# virt-manager, in Rust

A from-scratch Rust rewrite of [virt-manager](https://github.com/virt-manager/virt-manager)
(the GTK/Python libvirt VM management app) — full feature parity, same GUI
layout, full libvirt driver parity (QEMU/KVM, Xen, LXC, bhyve), Linux only.

**Status: early. The GUI has its main window; the other windows are still
being ported. Three CLI tools exist with partial flag coverage.** This is not yet a usable replacement for upstream virt-manager —
see [Status](#status) below before expecting it to do what the real
virt-manager does.

## Why

Same GUI, but Rust throughout — aiming for something more sleek and fast than
the Python original, without changing how it looks or where anything is.

## Status

| Piece | State |
|---|---|
| `virtinst-core` | Library. All device/domain XML types, the connection layer (real `virt`/libvirt bindings), install-method logic, clone logic. |
| `virt-install` | Working binary. `--import`, `--cdrom`/`--livecd`, `--boot`, `--cdrom --disk` (two-phase), `--pxe`, `--container --filesystem`. Most of upstream's flags aren't implemented yet. |
| `virt-clone` | Working binary. `--preserve-data` mode only (rewrites XML — name/UUID/MACs/disk paths — no disk-content copying). |
| `virt-xml` | Working binary. `--build-xml`/`--edit`/`--add-device`/`--remove-device`/`--print-xml`, wired up for eleven device options (`--watchdog`, `--vsock`, `--disk` — a deliberately partial slice of upstream's real 40+-field `--disk`, `--memballoon`, `--sound`, `--video`, `--controller`, `--tpm`, `--smartcard`, `--redirdev`, `--filesystem`) as the pattern to extend. `--edit`/`--remove-device` also support upstream's field-match selector form (e.g. `--remove-device --sound ich6`), not just index/`all`. |
| `virt-manager` (GUI) | **Main window only.** GTK4. Connection list with VMs, Add Connection dialog, connect/disconnect/remove connection, Run/Pause/Resume/Shut Down/Reboot/Force Reset/Force Off/Save, confirmation and error dialogs, About. Shares upstream's GSettings, so saved connections carry over. Menu entries for windows not yet ported (New VM, details, console, preferences, …) are shown but disabled. |

Run any binary with `--help` for its exact current flag set — the help text
says what's implemented and what upstream does differently, per flag.

## Building

```sh
cargo build --workspace
cargo test --workspace
```

Needs a Rust toolchain (stable), a libvirt dev environment (`libvirt-dev`
or equivalent) to link the `virt` crate, `libosinfo-1.0-dev` + `libclang-dev`
for `virtinst-core`, and `libgtk-4-dev` (GTK 4.14+) for the GUI.

Try the GUI against libvirt's built-in test driver, no daemon needed:

```sh
cargo run -p virt-manager -- --connect test:///default
```

## Design decisions

Every architectural decision — GUI toolkit, XML modeling approach, async
model, crate boundaries, license, distribution — was worked through and
recorded before writing code, not assumed. See
[`.scratch/rust-conversion/map.md`](.scratch/rust-conversion/map.md) and its
linked tickets in [`.scratch/rust-conversion/issues/`](.scratch/rust-conversion/issues/)
for the full reasoning behind each one.

## License

GPL-2.0-or-later, matching upstream. See [`LICENSE`](LICENSE).
