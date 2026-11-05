**English** · [Português (Brasil)](README.pt-BR.md)

> **Disclaimer:** this program was rebuilt with AI, mainly as a test and proof
> of concept of how well a frontier model (Claude, by Anthropic) and its
> harness can migrate a Python program to Rust. We have the utmost respect for
> the developers of the [original virt-manager](https://github.com/virt-manager/virt-manager)
> and their effort in building the project.

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
| `virt-clone` | Working binary. Full upstream option set: `--auto-clone`, disk copying (`--file`, `--force-copy`, `--skip-copy`, `--nonsparse`, `--reflink`), `--preserve-data`, `--nvram`, `--mac`, `--replace`, `--print-xml`. |
| `virt-xml` | Working binary. Every upstream device and guest option on the parser engine, `--build-xml`/`--edit`/`--add-device`/`--remove-device`/`--update`/`--print-diff`, `--convert-to-q35`/`--convert-to-vnc`, `--osinfo`. Checked byte for byte against upstream's own `virt-xml` (1300+ harness cases). |
| `virt-manager` (GUI) | GTK4, same layout and texts as upstream. Main window (connections, VM list, sparkline columns), Add Connection with password/keyring login, Preferences, About, system tray icon (StatusNotifierItem). New VM (local media, URL, import, manual, "Customize before install"), Delete, Clone, Migrate. VM window: details for every hardware type, Add Hardware, the Performance page, the XML tab, Snapshots, the VNC/SPICE consoles (needs libgvnc-1.0 and libspice-client-glib-2.0) and serial consoles (needs vte-2.91-gtk4). Connection Details: overview, virtual networks (and Create Network), storage pools/volumes. `--show-*` startup flags. Shares upstream's GSettings and keyring entries, so saved connections and console passwords carry over. |

Known differences from upstream: no libguestfs inspection (the Applications
list stays hidden, as upstream does when inspection is unavailable); the tray
icon needs a StatusNotifierItem host (GTK4 has no `GtkStatusIcon`, so there is
no XEmbed fallback).

Run any binary with `--help` for its exact current flag set — the help text
says what's implemented and what upstream does differently, per flag.

## Performance

Measured against the Python virt-manager (upstream `bac5379`) on the same
machine, `qemu:///session` only, with [`bench/run.sh`](bench/run.sh). Raw data
and the full environment (CPU, kernel, library versions, commit) are in
[`bench/results/2026-11-04/`](bench/results/2026-11-04/). Values are median ±
interquartile range: 20 runs per CLI command (3 warm-up runs), 10 GUI launches.

| Measure | Python | Rust | Rust / Python |
|---|---|---|---|
| `virt-install --import --print-xml` | 529 ± 3 ms | 349 ± 4 ms | 0.66 |
| `virt-install` peak RSS | 124 MiB | 84 MiB | 0.68 |
| `virt-xml --edit --print-diff` | 273 ± 3 ms | 14 ± 0 ms | 0.05 |
| `virt-xml` peak RSS | 74 MiB | 27 MiB | 0.36 |
| `virt-clone --auto-clone --print-xml` | 277 ± 2 ms | 13 ± 0 ms | 0.05 |
| `virt-clone` peak RSS | 76 MiB | 27 MiB | 0.35 |
| GUI: launch to main window shown | 615 ± 9 ms | 255 ± 4 ms | 0.41 |
| GUI: RSS 5 s after launch | 126 MiB | 159 MiB | 1.26 |
| GUI: same, with `GSK_RENDERER=cairo` | | 128 MiB | 1.02 |
| GUI: idle CPU over 10 s | 0.1% | 0.0% | below the 0.1% resolution |

**Gains:** every CLI command is faster and uses less memory; the GUI shows its
main window 2.4× sooner.

**Losses:** the GUI uses 33 MiB (26%) more memory at idle. With GTK4's cairo
renderer it drops to 128 MiB, within 2 MiB of Python, so the extra memory is
the default (GPU) renderer's.

Not measured yet: console (VNC/SPICE) performance, large VM lists, remote
connections and native Wayland (the GUI was measured through Xwayland, which
the script needs to find the window).

### Ideas to build and keep

- Run `bench/run.sh` before every release and commit the results, so each
  version is compared against the previous one.
- Run the CLI part in CI against `test:///default`, to catch regressions
  like the 3.4 s `virt-install` fixed in #117 (every osinfo lookup reloaded
  the whole database).
- Reduce the GUI's idle memory: measure what the default renderer allocates and
  whether it can be trimmed without switching renderers.
- Measure what is still missing above: consoles, a list with hundreds of VMs,
  remote `qemu+ssh` connections and native Wayland.

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

## Install

Ready-made packages (Ubuntu 24.04, Debian 13, Fedora, Arch and a generic
tarball) are on the [Releases page](https://github.com/VexFlint/Virt-manager-rust/releases).
On Ubuntu 24.04 (`noble`) or Debian 13 (use `trixie` instead of `noble`),
from our signed apt repository, which also brings updates with `apt upgrade`:

```sh
sudo curl -fsSLo /etc/apt/keyrings/virt-manager-rust.gpg https://vexflint.github.io/Virt-manager-rust/virt-manager-rust.gpg
echo "deb [signed-by=/etc/apt/keyrings/virt-manager-rust.gpg] https://vexflint.github.io/Virt-manager-rust noble main" | sudo tee /etc/apt/sources.list.d/virt-manager-rust.list
sudo apt update && sudo apt install virt-manager-rust
```

On Arch, from the AUR: `yay -S virt-manager-rust-bin` (prebuilt) or
`yay -S virt-manager-rust` (builds from source).
To build and install from source:

`packaging/install.sh` installs everything in upstream's `/usr` layout
(binaries, `.desktop`, AppStream metainfo, hicolor icon, GSettings schema,
man pages, shell completion). It replaces a distro `virt-manager`/`virtinst`
install, since the paths and the schema are the same:

```sh
cargo build --release --workspace
sudo sh packaging/install.sh            # PREFIX=/usr by default
DESTDIR=/tmp/root sh packaging/install.sh  # staging, for packages
```

## Shell completion

`virt-install`, `virt-clone` and `virt-xml` complete options and their
sub-options (`--disk source.<TAB>`) in bash, zsh and fish, through the same
protocol as upstream's argcomplete. The scripts are in
`crates/virtinst-cli/data/`:

| Shell | Install to |
|-------|------------|
| bash | `bash-completion/*` → `/usr/share/bash-completion/completions/` |
| zsh | `zsh-completion/_*` → a directory in `$fpath` (also needs the bash files above) |
| fish | `fish-completion/*.fish` → `~/.config/fish/completions/` |

## Man pages

`man/` holds upstream's own man pages (`man/*.rst`, unchanged). Build them
with `man/build.sh` (needs `rst2man`, from `python3-docutils`); the pages
land in `target/man/` and install to `/usr/share/man/man1/`.

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
The project rules and planning docs (`AGENTS.md`, `docs/ROADMAP.md`,
`docs/RELEASES.md`) are written in Portuguese, the maintainers' language.

The program uses upstream's own strings and gettext catalogs (`po/`), so the
GUI and CLI are translated like the Python virt-manager. Brazilian Portuguese
(`pt_BR`) is included, and Preferences > General has a language selector
("System default" or any bundled language, applied on restart). A new
language only needs its `.po` file.

## License

GPL-2.0-or-later, matching upstream. See [`LICENSE`](LICENSE).
