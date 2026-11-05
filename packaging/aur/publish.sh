#!/bin/sh
# Gera (e opcionalmente publica) os pacotes do AUR a partir dos PKGBUILDs
# do repo (docs/RELEASES.md, R9). Roda num container archlinux como root.
#
#   sh packaging/aur/publish.sh VERSÃO DIR [--push]
#
# Para cada pacote (virt-manager-rust, que compila do fonte, e
# virt-manager-rust-bin, que extrai o .pkg.tar.zst da release), escreve em
# DIR/<pacote>/ o PKGBUILD com o pkgver e os sha256sums da release VERSÃO e
# o .SRCINFO. A release da tag precisa já existir no GitHub.
# Com --push, clona ssh://aur@aur.archlinux.org/<pacote>.git, copia os dois
# arquivos e empurra (a chave SSH já deve estar no ~/.ssh do usuário).
set -eu

version=$1
out=$2
push=${3:-}
here=$(cd "$(dirname "$0")" && pwd)
pkgver=$(echo "$version" | tr - _)

id builder >/dev/null 2>&1 || useradd -m builder
mkdir -p "$out"

for pkg in virt-manager-rust virt-manager-rust-bin; do
    case $pkg in
        virt-manager-rust) src=$here/../arch/PKGBUILD ;;
        *) src=$here/$pkg/PKGBUILD ;;
    esac
    dir=$out/$pkg
    rm -rf "$dir"
    mkdir -p "$dir"
    sed -e "s/^pkgver=.*/pkgver=$pkgver/" -e "s/^pkgrel=.*/pkgrel=1/" "$src" > "$dir/PKGBUILD"
    chown -R builder "$dir"
    # updpkgsums baixa os fontes da release e grava os sha256 reais.
    su builder -c "cd '$dir' && updpkgsums && makepkg --verifysource && makepkg --printsrcinfo > .SRCINFO"
    if grep -q "sha256sums = SKIP" "$dir/.SRCINFO"; then
        echo "erro: $pkg ficou com sha256sums=SKIP" >&2
        exit 1
    fi
    rm -f "$dir"/*.tar.gz "$dir"/*.pkg.tar.zst
    cat "$dir/.SRCINFO"

    if [ "$push" = --push ]; then
        repo=$(mktemp -d)
        git clone "ssh://aur@aur.archlinux.org/$pkg.git" "$repo"
        cp "$dir/PKGBUILD" "$dir/.SRCINFO" "$repo/"
        git -C "$repo" add PKGBUILD .SRCINFO
        if git -C "$repo" diff --cached --quiet; then
            echo "$pkg: nada mudou no AUR"
        else
            git -C "$repo" commit -m "Atualiza para $version"
            git -C "$repo" push origin HEAD:master
        fi
    fi
done
