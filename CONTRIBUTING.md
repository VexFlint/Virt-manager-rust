**English** · [Português (Brasil)](CONTRIBUTING.pt-BR.md)

# Contributing

Quick guide for humans. The full rules (for people and for AIs) are in
[`AGENTS.md`](AGENTS.md); what's left to do is in
[`docs/ROADMAP.md`](docs/ROADMAP.md). Both are written in Portuguese, the
maintainers' language. Commit messages and PR titles are in Portuguese too.

## The goal in one sentence

A 1:1 copy of [virt-manager v5.1.0](https://github.com/virt-manager/virt-manager/tree/v5.1.0)
in Rust: same screens, texts, menus and behavior. Modern libraries yes, new
UX no.

## 1. Set up the machine (Ubuntu 24.04)

```sh
sudo apt-get update
sudo apt-get install -y libvirt-dev libosinfo-1.0-dev libclang-dev pkg-config \
  libxml2-utils libgtk-4-dev libgvnc-1.0-dev libspice-client-glib-2.0-dev \
  libvte-2.91-gtk4-dev libgtksourceview-5-dev python3-libvirt python3-libxml2 python3-requests \
  cpio xorriso genisoimage python3-docutils
```

Plus stable Rust with `rustfmt` and `clippy` (`rustup component add rustfmt clippy`).

## 2. Build and run

```sh
cargo build --workspace
cargo run -p virt-manager -- --connect test:///default   # GUI, no daemon
cargo run -p virtinst-cli --bin virt-install -- --help    # CLI
```

## 3. Test (same as CI)

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
cargo test --workspace --no-fail-fast

# harness: compares our CLI with upstream's, byte for byte
git clone --depth 1 --branch v5.1.0 https://github.com/virt-manager/virt-manager /tmp/virt-manager-upstream
UPSTREAM_REF=/tmp/virt-manager-upstream bash .scratch/rust-conversion/harness/run.sh
```

## 4. Open a PR

1. Pick a **free** item in [`docs/ROADMAP.md`](docs/ROADMAP.md) and claim it
   (draft PR or issue with `[#N]` in the title). See section 2 there.
2. New branch from `master`:
   `git fetch origin master && git checkout -b my-branch origin/master`.
3. One small PR per screen (or piece of a screen). If it touches a screen,
   put screenshots in the description.
4. Run step 3 locally, push and open the PR.
5. When the **`build-test`** check is green and there is no blocking review,
   **squash merge**. Mark the item as ✅ in the roadmap in the same PR.

`master` is protected: changes only land through PRs, with `build-test`
green, no force push.

## Where things are

- `crates/virtinst-core`: logic without GUI (XML, defaults, storage,
  install), mirrors upstream's `virtinst/`.
- `crates/virtinst-cli`: `virt-install`, `virt-xml`, `virt-clone` and the
  option parsers.
- `crates/virtinst-xml` and `crates/virtinst-xml-derive`: XML modeling base
  (traits and the `XmlBound` derive).
- `crates/virt-manager`: the GTK4 GUI; each `.rs` is named after the
  equivalent upstream module (`createvm.rs` ↔ `virtManager/createvm.py`).
- `.scratch/rust-conversion/`: architecture decisions (`map.md`, `issues/`),
  the harness and the CLI handoff.
