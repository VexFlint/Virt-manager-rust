#!/bin/sh
# Monta o tarball genérico da release (docs/RELEASES.md, R3):
# virt-manager-rust-<versão>-x86_64-linux.tar.gz com a árvore usr/ do
# packaging/install.sh e o INSTALL.
#
# Uso: sh packaging/tarball/build.sh <versão> <pasta de saída>
# (depois de 'cargo build --release --workspace').
set -eu

version=$1
out=$2
root=$(cd "$(dirname "$0")/../.." && pwd)
name=virt-manager-rust-$version-x86_64-linux
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

DESTDIR=$stage/$name sh "$root/packaging/install.sh"
install -m 0644 "$root/packaging/tarball/INSTALL" "$stage/$name/INSTALL"
install -m 0644 "$root/LICENSE" "$stage/$name/LICENSE"
install -m 0644 "$root/CHANGELOG.md" "$stage/$name/CHANGELOG.md"
mkdir -p "$out"
tar -C "$stage" --owner=0 --group=0 --sort=name -czf "$out/$name.tar.gz" "$name"
echo "$out/$name.tar.gz"
