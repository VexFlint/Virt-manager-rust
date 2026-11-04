#!/bin/sh
# Topo das notas da release (docs/RELEASES.md, seção 4), com os nomes dos
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

Porte 1:1 do virt-manager 5.1.0 para Rust + GTK4. Substitui os pacotes
\`virt-manager\`/\`virtinst\` da distro (mesmos caminhos e mesmas configurações).

### Destaques
- (editar antes de publicar: 3 a 5 linhas, o que o usuário percebe)

### Instalar
| Sistema | Arquivo | Comando |
|---|---|---|
NOTES
row "Ubuntu 24.04" "*_ubuntu24.04_*.deb" '`sudo apt install ./arquivo.deb`'
row "Debian 13" "*_debian13_*.deb" '`sudo apt install ./arquivo.deb`'
row "Fedora" "*.rpm" '`sudo dnf install ./arquivo.rpm`'
row "Arch" "*.pkg.tar.zst" '`sudo pacman -U arquivo.pkg.tar.zst`'
row "Outras" "*-x86_64-linux.tar.gz" 'ver `INSTALL` no tarball'
cat <<NOTES
| Windows | em breve (Fase 2) | |

Verificar: \`sha256sum -c SHA256SUMS\` e
\`gh attestation verify arquivo --repo $repo\`.

NOTES
