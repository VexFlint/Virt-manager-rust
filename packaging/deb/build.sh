#!/bin/sh
# Monta o .deb (docs/RELEASES.md, R4) a partir da árvore do
# packaging/install.sh, com dpkg-deb. As dependências de bibliotecas saem
# do dpkg-shlibdeps, então cada distro gera as suas. Os gatilhos de
# arquivo do glib, do hicolor-icon-theme e do desktop-file-utils atualizam
# os caches, por isso não há scripts de mantenedor.
#
# Uso (dentro do container da distro, depois do 'cargo build --release'):
#   sh packaging/deb/build.sh <versão> <distro, ex. ubuntu24.04> <pasta de saída>
set -eu

version=$1
distro=$2
out=$3
root=$(cd "$(dirname "$0")/../.." && pwd)
arch=$(dpkg --print-architecture)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
pkg=$work/pkg

DESTDIR=$pkg sh "$root/packaging/install.sh"
gzip -9n "$pkg"/usr/share/man/man1/*.1
install -D -m 0644 "$root/LICENSE" "$pkg/usr/share/doc/virt-manager-rust/copyright"
gzip -9nc "$root/CHANGELOG.md" > "$pkg/usr/share/doc/virt-manager-rust/changelog.gz"

# dpkg-shlibdeps procura um debian/control na pasta atual.
mkdir -p "$work/debian"
printf 'Source: virt-manager-rust\n\nPackage: virt-manager-rust\nArchitecture: any\n' \
    > "$work/debian/control"
shlibs=$(cd "$work" && dpkg-shlibdeps -O -e pkg/usr/bin/virt-manager -e pkg/usr/bin/virt-install \
    -e pkg/usr/bin/virt-clone -e pkg/usr/bin/virt-xml | sed -n 's/^shlibs:Depends=//p')

mkdir -p "$pkg/DEBIAN"
cat > "$pkg/DEBIAN/control" <<CONTROL
Package: virt-manager-rust
Version: $version
Architecture: $arch
Maintainer: Vex <vexflint@gmail.com>
Installed-Size: $(du -sk "$pkg/usr" | cut -f1)
Depends: $shlibs, gsettings-desktop-schemas, hicolor-icon-theme
Recommends: libvirt-daemon-system, qemu-system-x86, osinfo-db, xorriso
Suggests: libvirt-clients
Conflicts: virt-manager, virtinst
Replaces: virt-manager, virtinst
Provides: virt-manager, virtinst
Section: admin
Priority: optional
Homepage: https://github.com/VexFlint/Virt-manager-rust
Description: desktop application for managing virtual machines (Rust port)
 Port of virt-manager 5.1.0 to Rust and GTK4, with the same screens and
 behaviour. Includes the virt-manager GUI and the virt-install, virt-clone
 and virt-xml command line tools. Replaces the virt-manager and virtinst
 packages: same paths, same GSettings schema.
CONTROL

mkdir -p "$out"
deb=$out/virt-manager-rust_${version}_${distro}_$arch.deb
dpkg-deb --root-owner-group -Zxz --build "$pkg" "$deb" >/dev/null
echo "$deb"
