#!/bin/sh
# Topo das notas da release (docs/RELEASES.md, seção 4), em inglês com
# a versão em português num <details>, com os nomes dos
# arquivos que estão em <pasta>. O "O que mudou" vem do --generate-notes do
# GitHub, por isso termina aqui.
#
# Uso: sh packaging/release-notes.sh <versão> <pasta com os pacotes>
set -eu

v=$1
dist=$2
repo=${GITHUB_REPOSITORY:-VexFlint/Virt-manager-rust}

# Uma linha da tabela por arquivo que existe (glob sem casamento é ignorado).
row() {
    for f in "$dist"/$2; do
        [ -e "$f" ] && echo "| $1 | \`$(basename "$f")\` | $3 |"
    done
    return 0
}

cat <<NOTES
## virt-manager-rust $v

1:1 port of virt-manager 5.1.0 to Rust + GTK4. Replaces the distro's
\`virt-manager\`/\`virtinst\` packages (same paths and same settings).

### Highlights
- (edit before publishing: 3 to 5 lines, what users will notice)

### Install
| System | File | Command |
|---|---|---|
NOTES
row "Ubuntu 24.04" "*_ubuntu24.04_*.deb" '`sudo apt install ./file.deb`'
row "Debian 13" "*_debian13_*.deb" '`sudo apt install ./file.deb`'
row "Fedora" "*.rpm" '`sudo dnf install ./file.rpm`'
row "Arch" "*.pkg.tar.zst" '`sudo pacman -U file.pkg.tar.zst`'
row "Other" "*-x86_64-linux.tar.gz" 'see `INSTALL` in the tarball'
cat <<NOTES
| Windows | coming later (phase 2) | |

Verify: \`sha256sum -c SHA256SUMS\` and
\`gh attestation verify file --repo $repo\`.

<details>
<summary>Português (Brasil)</summary>

Porte 1:1 do virt-manager 5.1.0 para Rust + GTK4. Substitui os pacotes
\`virt-manager\`/\`virtinst\` da distro (mesmos caminhos e mesmas configurações).

**Destaques**
- (editar antes de publicar: os mesmos destaques acima, em português)

**Instalar:** baixe o arquivo do seu sistema na tabela acima e rode o
comando dela. Em outras distros, use o tarball e siga o \`INSTALL\`.
Windows: em breve (Fase 2).

Verificar: \`sha256sum -c SHA256SUMS\` e
\`gh attestation verify arquivo --repo $repo\`.

</details>

NOTES
