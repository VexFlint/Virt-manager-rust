# Plano de releases (GitHub Releases)

Status: **plano, nada implementado**. Nenhum item começa sem o Vex escolher.
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

```markdown
## virt-manager-rust vX.Y.Z

Porte 1:1 do virt-manager 5.1.0 para Rust + GTK4.

### Destaques
- (3 a 5 linhas, o que o usuário percebe)

### Instalar
| Sistema | Arquivo | Comando |
|---|---|---|
| Ubuntu 24.04 | `virt-manager-rust_X.Y.Z_ubuntu24.04_amd64.deb` | `sudo apt install ./arquivo.deb` |
| Debian 13 | `virt-manager-rust_X.Y.Z_debian13_amd64.deb` | `sudo apt install ./arquivo.deb` |
| Fedora | `virt-manager-rust-X.Y.Z-1.fcNN.x86_64.rpm` | `sudo dnf install ./arquivo.rpm` |
| Arch | AUR `virt-manager-rust` ou `.pkg.tar.zst` | `sudo pacman -U arquivo.pkg.tar.zst` |
| Outras | `virt-manager-rust-X.Y.Z-x86_64-linux.tar.gz` | ver `INSTALL` no tarball |
| Windows | em breve (Fase 2) | |

Verificar: `sha256sum -c SHA256SUMS` e
`gh attestation verify arquivo --repo VexFlint/Virt-manager-rust`.

### O que mudou
(gerado pelo GitHub a partir dos PRs)

**Changelog completo:** vA.B.C...vX.Y.Z
```

Nomes de arquivo sempre com versão, distro e arquitetura, como no RustDesk.

## 5. PRs (em ordem)

Complexidade: P = pequeno, M = médio, G = grande.

| # | PR | Tam. |
|---|---|---|
| R1 | Arquivos de instalação: `.desktop`, metainfo AppStream, ícones hicolor, schema, layout `/usr` (copiados do upstream `data/`) | M |
| R2 | Versão: `--version` nos 4 binários, `CHANGELOG.md`, `.github/release.yml` (categorias) | P |
| R3 | `release.yml`: gatilho na tag, checagem de versão, tarball, `SHA256SUMS`, atestação, release rascunho com o modelo | M |
| R4 | `.deb` Ubuntu 24.04 + Debian 13 (`cargo-deb`) + smoke test | M |
| R5 | `.rpm` Fedora (`cargo-generate-rpm`) + smoke test | M |
| R6 | Arch: `PKGBUILD` no repo + `.pkg.tar.zst` na release (AUR depois, precisa de chave SSH em secret) | M |
| R7 | Primeira release `v0.9.0` (pre-release) | P |
| R8 | Flatpak (opcional, manifesto + permissões de libvirt) | G |
| W1 | Windows `.msi` (Fase 2, depois do F2-1 do ROADMAP) | G |

Mínimo para a primeira release: R1, R2, R3, R4 e R7 (Ubuntu/Debian + tarball).
R5 e R6 podem entrar na `v0.9.1`.
