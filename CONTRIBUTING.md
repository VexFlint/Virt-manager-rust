# Como contribuir

Guia rápido para humanos. As regras completas (para pessoas e para IAs) estão
no [`AGENTS.md`](AGENTS.md); o que falta fazer está no
[`docs/ROADMAP.md`](docs/ROADMAP.md).

## O objetivo em uma frase

Uma cópia 1:1 do [virt-manager v5.1.0](https://github.com/virt-manager/virt-manager/tree/v5.1.0)
em Rust: mesmas telas, textos, menus e comportamento. Bibliotecas modernas
sim, UX nova não.

## 1. Preparar a máquina (Ubuntu 24.04)

```sh
sudo apt-get update
sudo apt-get install -y libvirt-dev libosinfo-1.0-dev libclang-dev pkg-config \
  libxml2-utils libgtk-4-dev libgvnc-1.0-dev libspice-client-glib-2.0-dev \
  libvte-2.91-gtk4-dev libgtksourceview-5-dev python3-libvirt python3-libxml2 python3-requests \
  cpio xorriso genisoimage
```

E o Rust estável com `rustfmt` e `clippy` (`rustup component add rustfmt clippy`).

## 2. Compilar e rodar

```sh
cargo build --workspace
cargo run -p virt-manager -- --connect test:///default   # GUI sem daemon
cargo run -p virtinst-cli --bin virt-install -- --help    # CLI
```

## 3. Testar (o mesmo que a CI)

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
cargo test --workspace --no-fail-fast

# harness: compara nossa CLI com a do upstream, byte a byte
git clone --depth 1 --branch v5.1.0 https://github.com/virt-manager/virt-manager /tmp/virt-manager-upstream
UPSTREAM_REF=/tmp/virt-manager-upstream bash .scratch/rust-conversion/harness/run.sh
```

## 4. Abrir um PR

1. Escolha um item **livre** no [`docs/ROADMAP.md`](docs/ROADMAP.md) e
   reserve-o (PR draft ou issue com `[#N]` no título). Veja a seção 2 de lá.
2. Branch nova a partir do `master`:
   `git fetch origin master && git checkout -b minha-branch origin/master`.
3. Um PR pequeno por tela (ou pedaço de tela). Se mexer em tela, coloque
   prints na descrição.
4. Rode o passo 3 localmente, faça push e abra o PR.
5. Quando o check **`build-test`** ficar verde e não houver revisão
   bloqueante, faça **squash merge**. Marque o item como ✅ no roadmap no
   mesmo PR.

O `master` é protegido: só entra por PR, com `build-test` verde, sem force
push.

## Onde está cada coisa

- `crates/virtinst-core`: lógica sem GUI (XML, defaults, storage, instalação),
  espelha o `virtinst/` do upstream.
- `crates/virtinst-cli`: `virt-install`, `virt-xml`, `virt-clone` e os parsers
  das opções.
- `crates/virtinst-xml` e `crates/virtinst-xml-derive`: base de modelagem
  do XML (traits e o derive `XmlBound`).
- `crates/virt-manager`: a GUI em GTK4; cada `.rs` tem o nome do módulo
  equivalente do upstream (`createvm.rs` ↔ `virtManager/createvm.py`).
- `.scratch/rust-conversion/`: decisões de arquitetura (`map.md`, `issues/`),
  o harness e o handoff da CLI.
