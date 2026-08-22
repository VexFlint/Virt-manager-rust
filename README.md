# virt-manager, in Rust

A from-scratch Rust rewrite of [virt-manager](https://github.com/virt-manager/virt-manager)
(the GTK/Python libvirt VM management app) — full feature parity, same GUI
layout, full libvirt driver parity (QEMU/KVM, Xen, LXC, bhyve), Linux only.

**Status: early. No GUI yet. Three CLI tools exist with partial flag
coverage.** This is not yet a usable replacement for upstream virt-manager —
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
| `virt-xml` | Working binary. `--build-xml`/`--edit`/`--add-device`/`--remove-device`, wired up for two device options (`--watchdog`, `--vsock`) as the pattern to extend. |
| `virt-manager` (GUI) | **Not started.** `fn main() {}`. |

Run any binary with `--help` for its exact current flag set — the help text
says what's implemented and what upstream does differently, per flag.

## Building

```sh
cargo build --workspace
cargo test --workspace
```

Needs a Rust toolchain (stable) and a libvirt dev environment (`libvirt-dev`
or equivalent) to link the `virt` crate.

## Design decisions

Every architectural decision — GUI toolkit, XML modeling approach, async
model, crate boundaries, license, distribution — was worked through and
recorded before writing code, not assumed. See
[`.scratch/rust-conversion/map.md`](.scratch/rust-conversion/map.md) and its
linked tickets in [`.scratch/rust-conversion/issues/`](.scratch/rust-conversion/issues/)
for the full reasoning behind each one.

## License

GPL-2.0-or-later, matching upstream. See [`LICENSE`](LICENSE).
