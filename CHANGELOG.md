**English** · [Português (Brasil)](CHANGELOG.pt-BR.md)

# Changelog

Changes in each virt-manager-rust release. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow
[SemVer](https://semver.org/). The detailed list of PRs for each version is
on its GitHub release page.

The package version comes from the root `Cargo.toml`. The `--version` of all
four programs prints the upstream version the port is based on (`5.1.0`),
just like the Python virt-manager; the package version shows in "About".

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
