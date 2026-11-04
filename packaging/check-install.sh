#!/bin/sh
# Confere o layout que packaging/install.sh gera (a CI roda isto).
# Uso: BINDIR=target/debug sh packaging/check-install.sh
# Precisa de desktop-file-utils, appstream e libglib2.0-bin.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
gui_data=$root/crates/virt-manager/data
upstream=${UPSTREAM_REF:-}

# .desktop e metainfo são cópias 1:1 do upstream (sem as traduções do po/).
if [ -n "$upstream" ]; then
    cmp "$upstream/data/virt-manager.desktop.in" "$gui_data/virt-manager.desktop"
    cmp "$upstream/data/virt-manager.appdata.xml.in" "$gui_data/virt-manager.appdata.xml"
    cmp "$upstream/data/org.virt-manager.virt-manager.gschema.xml" \
        "$gui_data/org.virt-manager.virt-manager.gschema.xml"
fi

dest=$(mktemp -d)
trap 'rm -rf "$dest"' EXIT
DESTDIR=$dest PREFIX=/usr sh "$root/packaging/install.sh"
share=$dest/usr/share

for f in \
    usr/bin/virt-manager usr/bin/virt-install usr/bin/virt-clone usr/bin/virt-xml \
    usr/share/applications/virt-manager.desktop \
    usr/share/metainfo/virt-manager.appdata.xml \
    usr/share/glib-2.0/schemas/org.virt-manager.virt-manager.gschema.xml \
    usr/share/icons/hicolor/256x256/apps/virt-manager.png \
    usr/share/man/man1/virt-manager.1 usr/share/man/man1/virt-install.1 \
    usr/share/man/man1/virt-clone.1 usr/share/man/man1/virt-xml.1 \
    usr/share/bash-completion/completions/virt-install \
    usr/share/zsh/site-functions/_virt-xml \
    usr/share/fish/vendor_completions.d/virt-clone.fish \
    usr/share/locale/pt_BR/LC_MESSAGES/virt-manager-rust.mo; do
    if [ ! -e "$dest/$f" ]; then
        echo "faltou instalar /$f" >&2
        exit 1
    fi
done

desktop-file-validate "$share/applications/virt-manager.desktop"
# O id "virt-manager.desktop" não é rDNS; é o do upstream e fica assim (1:1).
appstreamcli validate --no-net --override=cid-desktopapp-is-not-rdns=info \
    "$share/metainfo/virt-manager.appdata.xml"
glib-compile-schemas --strict --dry-run "$share/glib-2.0/schemas"

# Os binários instalados rodam.
# --version imprime a versão do upstream (5.1.0), como o argparse dele.
for tool in virt-manager virt-install virt-clone virt-xml; do
    test "$("$dest/usr/bin/$tool" --version)" = 5.1.0
done
"$dest/usr/bin/virt-xml" --help >/dev/null
# O .mo instalado traduz (o binário de release procura em /usr/share/locale;
# aqui ele está no DESTDIR, então aponta a pasta à mão).
if locale -a 2>/dev/null | grep -qi '^pt_BR.utf-\?8$'; then
    out=$(VIRT_MANAGER_LOCALEDIR=$share/locale LC_ALL=pt_BR.UTF-8 LANGUAGE= \
        "$dest/usr/bin/virt-install" --help)
    case $out in
    *"Criar uma nova máquina virtual"*) ;;
    *) echo "a tradução pt_BR instalada não funcionou" >&2; exit 1 ;;
    esac
fi
"$dest/usr/bin/virt-clone" --help >/dev/null

echo "layout de instalação OK"
