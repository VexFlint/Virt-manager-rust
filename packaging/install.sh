#!/bin/sh
# Instala o virt-manager-rust no layout /usr do upstream (virt-manager
# v5.1.0, meson.build, data/meson.build, data/icons/meson.build,
# po/meson.build e man/meson.build), para os pacotes .deb/.rpm/Arch e o
# tarball (docs/RELEASES.md, R1).
#
# Uso:
#   cargo build --release --workspace
#   sudo sh packaging/install.sh
#   DESTDIR=/tmp/raiz sh packaging/install.sh      # pacote: não roda os caches
#
# Variáveis: PREFIX (padrão /usr), DESTDIR (padrão vazio), BINDIR com os
# binários compilados (padrão target/release). Precisa do msgfmt (gettext).
#
# Os ícones que não são "apps" o upstream põe em /usr/share/virt-manager/icons;
# aqui eles já vão embutidos no binário (GResource), então só os ícones do
# app vão para o tema hicolor.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
prefix=${PREFIX:-/usr}
destdir=${DESTDIR:-}
bindir=${BINDIR:-$root/target/release}
gui_data=$root/crates/virt-manager/data
cli_data=$root/crates/virtinst-cli/data
datadir=$destdir$prefix/share

# install -D cria as pastas que faltam.
inst() {
    install -D -m "$1" "$2" "$3"
}

# Binários (meson.build, bin_data).
for tool in virt-manager virt-install virt-clone virt-xml; do
    if [ ! -x "$bindir/$tool" ]; then
        echo "$bindir/$tool não existe: rode 'cargo build --release --workspace'" >&2
        exit 1
    fi
    inst 0755 "$bindir/$tool" "$destdir$prefix/bin/$tool"
done

# .desktop e metainfo (po/meson.build).
inst 0644 "$gui_data/virt-manager.desktop" "$datadir/applications/virt-manager.desktop"
inst 0644 "$gui_data/virt-manager.appdata.xml" "$datadir/metainfo/virt-manager.appdata.xml"

# Schema GSettings (data/meson.build).
inst 0644 "$gui_data/org.virt-manager.virt-manager.gschema.xml" \
    "$datadir/glib-2.0/schemas/org.virt-manager.virt-manager.gschema.xml"

# Ícone do app no tema hicolor (data/icons/meson.build, categoria apps).
for size in 16x16 22x22 24x24 32x32 48x48 256x256; do
    inst 0644 "$gui_data/icons/$size/apps/virt-manager.png" \
        "$datadir/icons/hicolor/$size/apps/virt-manager.png"
done

# Man pages (man/meson.build).
mandir=$(mktemp -d)
trap 'rm -rf "$mandir"' EXIT
sh "$root/man/build.sh" "$mandir" >/dev/null
for page in "$mandir"/*.1; do
    inst 0644 "$page" "$datadir/man/man1/$(basename "$page")"
done

# Traduções (po/meson.build): po/<língua>.po de cada língua do po/LINGUAS
# vira /usr/share/locale/<língua>/LC_MESSAGES/virt-manager-rust.mo, que é
# onde o build de release procura (virtinst_core::i18n::localedir).
for lang in $(grep -v '^#' "$root/po/LINGUAS"); do
    mo=$datadir/locale/$lang/LC_MESSAGES/virt-manager-rust.mo
    mkdir -p "$(dirname "$mo")"
    msgfmt --check -o "$mo" "$root/po/$lang.po"
    chmod 0644 "$mo"
done

# Autocompletar (data/meson.build instala o de bash; zsh e fish são nossos).
for tool in virt-install virt-clone virt-xml; do
    inst 0644 "$cli_data/bash-completion/$tool" "$datadir/bash-completion/completions/$tool"
    inst 0644 "$cli_data/zsh-completion/_$tool" "$datadir/zsh/site-functions/_$tool"
    inst 0644 "$cli_data/fish-completion/$tool.fish" "$datadir/fish/vendor_completions.d/$tool.fish"
done

# gnome.post_install do meson.build: só numa instalação direta. Num pacote
# (DESTDIR) quem roda isso são os gatilhos da distro.
if [ -z "$destdir" ]; then
    if command -v glib-compile-schemas >/dev/null; then
        glib-compile-schemas "$datadir/glib-2.0/schemas"
    fi
    if command -v gtk-update-icon-cache >/dev/null; then
        gtk-update-icon-cache -q -t -f "$datadir/icons/hicolor" || true
    fi
    if command -v update-desktop-database >/dev/null; then
        update-desktop-database -q "$datadir/applications" || true
    fi
fi
