**English** · [Português (Brasil)](CHANGELOG.pt-BR.md)

# Changelog

Changes in each virt-manager-rust release. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow
[SemVer](https://semver.org/). The detailed list of PRs for each version is
on its GitHub release page.

The package version comes from the root `Cargo.toml`. The `--version` of all
four programs prints the upstream version the port is based on (`5.1.0`),
just like the Python virt-manager; the package version shows in "About".

## [Unreleased]

### Added
- `bench/run.sh`: reproducible benchmark against the Python virt-manager
  (CLI time and memory, GUI startup, idle memory and CPU), with results in
  `bench/results/` and a Performance section in the README.

### Fixed
- `virt-install` loads the libosinfo database once per run instead of at
  every lookup: `--print-xml` for an imported guest went from 3.4 s to 0.35 s.
- Details, NIC page: the NIC icon (`network-idle`) is bundled, so it no
  longer shows as a missing image on icon themes without it (Breeze,
  Adwaita 46+); "MAC address:", "active" and the network warnings are
  translated, as upstream.
- Consoles over `qemu+ssh`: the SSH tunnel no longer fails with
  `sh: 1: Syntax error: "fi" unexpected (expecting "then")`; the remote
  `nc` script is built byte for byte like upstream.

## [1.0.0] - 2026-11-04

Based on virt-manager 5.1.0. First stable release.

### Added
- Translations: GUI and CLI use gettext with upstream's own catalogs
  (`po/`), so the texts match the Python virt-manager. Brazilian Portuguese
  (`pt_BR`) is included; a new language only needs its `.po` file.
- Language selector in Preferences > General ("System default" or any
  bundled language). Applies after restarting virt-manager.
- Documentation in English and Portuguese (`*.pt-BR.md`).
- Packages ship the `.mo` translation files.

### Fixed
- Flaky clone tests in CI (MAC address generation).
- rpm package now lists the translation files.

## [0.9.0] - 2026-11-04

Based on virt-manager 5.1.0.

### Added
- `virt-manager` (GTK4 GUI): main window, VM details, VNC, SPICE (with GL)
  and serial consoles, New VM, host details, clone, migrate, snapshots,
  preferences, tray icon and keyring.
- `virt-install`, `virt-xml` and `virt-clone` with upstream's options,
  checked byte for byte against upstream in the CI harness.
- Man pages and bash completion.
- Install files in upstream's `/usr` layout (`packaging/install.sh`).
- `virt-manager --version` prints only the upstream version, like argparse.
