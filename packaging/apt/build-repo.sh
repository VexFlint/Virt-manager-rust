#!/bin/sh
# Monta o repositório apt assinado do GitHub Pages (docs/RELEASES.md, R10).
#
#   sh packaging/apt/build-repo.sh DEBS OUT
#
# DEBS tem os .deb da release (virt-manager-rust_X.Y.Z_<distro>_amd64.deb).
# Cada distro vira uma suíte (ubuntu24.04 -> noble, debian13 -> trixie) com
# o componente main. A chave secreta GPG já deve estar importada; usa a
# primeira (ou a de GPG_KEY_ID). Saída em OUT:
#   pool/main/*.deb, dists/<suíte>/{Release,InRelease,Release.gpg,main/...},
#   virt-manager-rust.gpg (chave pública, para /etc/apt/keyrings) e index.html.
set -eu

debs=$1
out=$2
key=${GPG_KEY_ID:-$(gpg --list-secret-keys --with-colons | awk -F: '/^fpr/ {print $10; exit}')}
test -n "$key"

rm -rf "$out"
mkdir -p "$out/pool/main"
cd "$out"
out=$(pwd)

suites=
for deb in "$debs"/virt-manager-rust_*_amd64.deb; do
    distro=$(basename "$deb" | sed 's/^virt-manager-rust_[^_]*_\(.*\)_amd64\.deb$/\1/')
    case $distro in
        ubuntu24.04) suite=noble; origin=Ubuntu ;;
        debian13) suite=trixie; origin=Debian ;;
        *) echo "distro desconhecida: $distro" >&2; exit 1 ;;
    esac
    cp "$deb" pool/main/
    bin=dists/$suite/main/binary-amd64
    mkdir -p "$bin"
    # Só o .deb desta distro entra no Packages da suíte.
    apt-ftparchive packages "pool/main/$(basename "$deb")" > "$bin/Packages"
    gzip -9kn "$bin/Packages"
    apt-ftparchive \
        -o APT::FTPArchive::Release::Origin=virt-manager-rust \
        -o APT::FTPArchive::Release::Label=virt-manager-rust \
        -o APT::FTPArchive::Release::Suite="$suite" \
        -o APT::FTPArchive::Release::Codename="$suite" \
        -o APT::FTPArchive::Release::Architectures=amd64 \
        -o APT::FTPArchive::Release::Components=main \
        -o APT::FTPArchive::Release::Description="virt-manager-rust for $origin $suite" \
        release "dists/$suite" > "$out/Release.tmp"
    # Gerado fora de dists/ para o Release não se listar.
    mv "$out/Release.tmp" "dists/$suite/Release"
    gpg --batch --yes --local-user "$key" --clearsign -o "dists/$suite/InRelease" "dists/$suite/Release"
    gpg --batch --yes --local-user "$key" -abs -o "dists/$suite/Release.gpg" "dists/$suite/Release"
    suites="$suites $suite"
done
test -n "$suites"

gpg --batch --export "$key" > virt-manager-rust.gpg

url=https://vexflint.github.io/Virt-manager-rust
cat > index.html <<HTML
<!doctype html>
<meta charset="utf-8">
<title>virt-manager-rust apt repository</title>
<h1>virt-manager-rust apt repository</h1>
<p>Suites:$suites. Install (Ubuntu 24.04 = noble, Debian 13 = trixie):</p>
<pre>sudo curl -fsSLo /etc/apt/keyrings/virt-manager-rust.gpg $url/virt-manager-rust.gpg
echo "deb [signed-by=/etc/apt/keyrings/virt-manager-rust.gpg] $url noble main" | sudo tee /etc/apt/sources.list.d/virt-manager-rust.list
sudo apt update && sudo apt install virt-manager-rust</pre>
<p><a href="https://github.com/VexFlint/Virt-manager-rust">github.com/VexFlint/Virt-manager-rust</a></p>
HTML
