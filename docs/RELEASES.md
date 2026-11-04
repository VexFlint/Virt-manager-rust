# Plano de releases (GitHub Releases)

Status: **em andamento** (R1 a R7 feitos; `v0.9.0` saiu em 2026-11-04). Nenhum item começa sem o Vex escolher.
Reserve o item como no `docs/ROADMAP.md` (seção 2) antes de começar.

Modelo visual: a página de release do
[RustDesk 1.5.0](https://github.com/rustdesk/rustdesk/releases/tag/1.5.0):
notas curtas no topo e uma lista de pacotes por sistema embaixo.

## 1. O que publicar

Binários: `virt-manager` (GUI), `virt-install`, `virt-clone`, `virt-xml`.

| Formato | Distros alvo | Como gerar | Prioridade |
|---|---|---|---|
| `.deb` | Ubuntu 24.04, Debian 13 | `cargo-deb`, compilado em container da própria distro | 1 |
| `.rpm` | Fedora (2 últimas versões) | `cargo-generate-rpm` em container `fedora` | 1 |
| `.pkg.tar.zst` + AUR | Arch e derivadas | `PKGBUILD` + `makepkg` em container `archlinux` | 2 |
| `.tar.gz` | qualquer distro (manual) | `make install`-like, `DESTDIR` | 1 |
| Flatpak | opcional | precisa de acesso a `/run/libvirt` (sai do sandbox) | 3 |
| AppImage | não recomendado | GTK4 + libvirt + gvnc/spice/vte embutidos dão trabalho e quebram fácil | - |
| Windows `.msi`/`.zip` | Fase 2 | só depois do backend Windows (ROADMAP, Fase 2) | depois |

Por que compilar em container de cada distro: o app linka dinamicamente
GTK4, libvirt, libosinfo, gvnc, spice e vte. Um binário de Ubuntu não serve
para Fedora; cada pacote precisa ser compilado contra as libs da distro.

**Decidido (Vex, 2026-10-29): o pacote substitui o virt-manager Python.**
Nossos binários e o schema GSettings (`org.virt-manager.virt-manager.gschema.xml`)
têm o mesmo nome dos pacotes `virt-manager`/`virtinst` da distro. O pacote
`virt-manager-rust` declara `Conflicts`/`Provides`/`Replaces: virt-manager, virtinst`
(no Arch: `conflicts`/`provides`) e instala nos mesmos caminhos. É o 1:1 de verdade:
quem instala o nosso remove o Python, e as configurações continuam as mesmas.

## 2. Versionamento

- SemVer com tag `vX.Y.Z`. A versão vem do `Cargo.toml` da raiz
  (`[workspace.package] version`); o workflow falha se a tag não bater.
- Primeira release: `v0.9.0`, marcada como *pre-release*. `v1.0.0` quando o
  1:1 estiver validado em uso real.
- Cada nota diz a base do upstream: "baseado no virt-manager 5.1.0".
- Tags `vX.Y.Z-rc.N` geram release *pre-release* automaticamente.

## 3. Workflow de CI (`.github/workflows/release.yml`)

Gatilho: `push` de tag `v*` (e `workflow_dispatch` para testar sem publicar).

1. `check`: tag == versão do `Cargo.toml`; roda os mesmos passos do `ci.yml`.
2. `build` (matriz por container): `ubuntu:24.04`, `debian:trixie`,
   `fedora:latest`, `archlinux:latest`. Cada job instala as deps, compila em
   `--release` e gera o pacote.
3. `smoke`: instala cada pacote num container limpo da mesma distro e roda
   `virt-install --version`, `virt-xml --help`, `virt-clone --help` e
   `glib-compile-schemas --strict` no schema instalado.
4. `publish`: junta tudo, gera `SHA256SUMS`, cria a atestação de origem
   (`actions/attest-build-provenance`, verificável com `gh attestation verify`)
   e cria a release como **rascunho** com o modelo da seção 4. O Vex (ou o
   Claude, se autorizado) revisa e publica.

Notas automáticas: `.github/release.yml` com categorias por label
(GUI, CLI, Correções, Infra) para o "Gerar notas" do GitHub.

## 4. Modelo das notas de release

Bilíngue: inglês primeiro, português num bloco `<details>` logo depois da
tabela (gerado por `packaging/release-notes.sh`).

```markdown
## virt-manager-rust vX.Y.Z

1:1 port of virt-manager 5.1.0 to Rust + GTK4.

### Highlights
- (3 to 5 lines, what users will notice)

### Install
| System | File | Command |
|---|---|---|
| Ubuntu 24.04 | `virt-manager-rust_X.Y.Z_ubuntu24.04_amd64.deb` | `sudo apt install ./file.deb` |
| Debian 13 | `virt-manager-rust_X.Y.Z_debian13_amd64.deb` | `sudo apt install ./file.deb` |
| Fedora | `virt-manager-rust-X.Y.Z-1.fcNN.x86_64.rpm` | `sudo dnf install ./file.rpm` |
| Arch | AUR `virt-manager-rust` or `.pkg.tar.zst` | `sudo pacman -U file.pkg.tar.zst` |
| Other | `virt-manager-rust-X.Y.Z-x86_64-linux.tar.gz` | see `INSTALL` in the tarball |
| Windows | coming later (phase 2) | |

Verify: `sha256sum -c SHA256SUMS` and
`gh attestation verify file --repo VexFlint/Virt-manager-rust`.

<details>
<summary>Português (Brasil)</summary>

Porte 1:1 do virt-manager 5.1.0 para Rust + GTK4.
**Destaques** (os mesmos de cima, em português), como instalar e verificar.
</details>

### What's Changed
(gerado pelo GitHub a partir dos PRs)

**Full Changelog:** vA.B.C...vX.Y.Z
```

Nomes de arquivo sempre com versão, distro e arquitetura, como no RustDesk.

## 5. PRs (em ordem)

Complexidade: P = pequeno, M = médio, G = grande.

| # | PR | Tam. |
|---|---|---|
| R1 | ✅ Arquivos de instalação: `.desktop`, metainfo AppStream, ícones hicolor, schema, layout `/usr` (copiados do upstream `data/`; `packaging/install.sh`) | M |
| R2 | ✅ Versão: `--version` nos 4 binários (imprime `5.1.0`, como o upstream), `CHANGELOG.md`, `.github/release.yml` (categorias por label: GUI, CLI, bug, Infra) | P |
| R3 | ✅ `release.yml`: gatilho na tag, checagem de versão, tarball, `SHA256SUMS`, atestação, release rascunho com o modelo | M |
| R4 | ✅ `.deb` Ubuntu 24.04 + Debian 13 (`dpkg-deb` + `dpkg-shlibdeps` sobre a árvore do `install.sh`, sem `cargo-deb`) + smoke test | M |
| R5 | ✅ `.rpm` Fedora (`rpmbuild` sobre a árvore do `install.sh`) + smoke test | M |
| R6 | ✅ Arch: `PKGBUILD` no repo + `.pkg.tar.zst` na release (AUR depois, precisa de chave SSH em secret) | M |
| R7 | ✅ Primeira release `v0.9.0` (pre-release) | P |
| R8 | Flatpak (opcional, manifesto + permissões de libvirt) | G |
| W1 | Windows `.msi` (Fase 2, depois do F2-1 do ROADMAP) | G |

Mínimo para a primeira release: R1, R2, R3, R4 e R7 (Ubuntu/Debian + tarball).
R5 e R6 podem entrar na `v0.9.1`.
