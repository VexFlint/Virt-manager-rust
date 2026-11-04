#!/bin/sh
# Monta o .rpm (docs/RELEASES.md, R5) a partir da árvore do
# packaging/install.sh, com rpmbuild. Os Requires de bibliotecas saem do
# gerador automático do rpm; os caches (schemas, ícones, .desktop) são
# atualizados pelos gatilhos de arquivo do glib2, gtk e desktop-file-utils.
#
# Uso (dentro do container do Fedora, depois do 'cargo build --release'):
#   sh packaging/rpm/build.sh <versão> <pasta de saída>
set -eu

version=$1
out=$2
root=$(cd "$(dirname "$0")/../.." && pwd)
top=$(mktemp -d)
trap 'rm -rf "$top"' EXIT

DESTDIR=$top/SOURCES/pkg sh "$root/packaging/install.sh"
install -m 0644 "$root/LICENSE" "$root/CHANGELOG.md" "$top/SOURCES/"
# O rpm não aceita '-' na versão: 0.9.0-rc.1 vira 0.9.0~rc.1.
rpmbuild -bb --define "_topdir $top" --define "pkgversion $(echo "$version" | tr - '~')" \
    "$root/packaging/rpm/virt-manager-rust.spec" >/dev/null
mkdir -p "$out"
for rpm in "$top"/RPMS/*/*.rpm; do
    cp "$rpm" "$out/"
    echo "$out/$(basename "$rpm")"
done
