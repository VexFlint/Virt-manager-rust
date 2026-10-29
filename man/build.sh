#!/bin/sh
# Gera as man pages (seção 1) a partir dos .rst copiados do upstream
# (virt-manager v5.1.0, man/), como o man/meson.build de lá:
# `rst2man --strict`. Uso: man/build.sh [pasta de saída] (padrão:
# target/man). Precisa do docutils (pacote python3-docutils).
set -eu
here=$(dirname "$0")
out=${1:-target/man}
rst2man=$(command -v rst2man || command -v rst2man.py || true)
if [ -z "$rst2man" ]; then
    echo "rst2man não encontrado: instale o python3-docutils" >&2
    exit 1
fi
mkdir -p "$out"
for rst in "$here"/*.rst; do
    page="$out/$(basename "$rst" .rst).1"
    "$rst2man" --strict "$rst" "$page"
    echo "$page"
done
