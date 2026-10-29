# virt-manager, in Rust

A from-scratch Rust rewrite of [virt-manager](https://github.com/virt-manager/virt-manager)
(the GTK/Python libvirt VM management app) — full feature parity, same GUI
layout, full libvirt driver parity (QEMU/KVM, Xen, LXC, bhyve), Linux only.

**Status: feature-complete port of virt-manager v5.1.0, not yet battle-tested.**
Every upstream window and the three CLI tools are ported; see
[Status](#status) for the few things that differ and what hasn't been
exercised against real hosts yet.

## Why

Same GUI, but Rust throughout — aiming for something more sleek and fast than
the Python original, without changing how it looks or where anything is.

## Status

| Piece | State |
|---|---|
| `virtinst-core` | Library. All device/domain/network/snapshot XML types, the connection layer (real `virt`/libvirt bindings), install-method logic, clone logic (including disk copying), disk path permission fixups. |
| `virt-install` | Working binary. Every upstream option on the 1:1 parser engine, all install methods (`--location` with distro detection, `--cdrom`, `--pxe`, `--import`, `--install` kernel/initrd, `--cloud-init`, `--unattended`, `--reinstall`). |
| `virt-clone` | Working binary. Full upstream option set: `--auto-clone`, disk copying (`--file`, `--force-copy`, `--skip-copy`, `--nonsparse`, `--reflink`), `--preserve-data`, `--nvram`, `--mac`, `--replace`, `--print-xml`, bash completion. |
| `virt-xml` | Working binary. Every upstream device and guest option on the parser engine, `--build-xml`/`--edit`/`--add-device`/`--remove-device`/`--update`/`--print-diff`, `--convert-to-q35`/`--convert-to-vnc`, `--osinfo`. Checked byte for byte against upstream's own `virt-xml` (1300+ harness cases). |
| `virt-manager` (GUI) | GTK4, same layout and texts as upstream. Main window (connections, VM list, sparkline columns), Add Connection with password/keyring login, Preferences, About, system tray icon (StatusNotifierItem). New VM (local media, URL, import, manual, "Customize before install"), Delete, Clone, Migrate. VM window: details for every hardware type, Add Hardware, the Performance page, the XML tab, Snapshots, the VNC/SPICE consoles (needs libgvnc-1.0 and libspice-client-glib-2.0) and serial consoles (needs vte-2.91-gtk4). Connection Details: overview, virtual networks (and Create Network), storage pools/volumes. `--show-*` startup flags. Shares upstream's GSettings and keyring entries, so saved connections and console passwords carry over. |

Known differences from upstream: no libguestfs inspection (the Applications
list stays hidden, as upstream does when inspection is unavailable); the tray
icon needs a StatusNotifierItem host (GTK4 has no `GtkStatusIcon`, so there is
no XEmbed fallback).

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

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md) (build, tests, PRs),
[`AGENTS.md`](AGENTS.md) (project rules, also read by AI agents) and
[`docs/ROADMAP.md`](docs/ROADMAP.md) (what's left and how to claim an item).

## License

GPL-2.0-or-later, matching upstream. See [`LICENSE`](LICENSE).
