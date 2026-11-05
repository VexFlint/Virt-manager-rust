#!/bin/sh
# Compara este port com o virt-manager Python, só em qemu:///session.
# Uso: PY_SRC=~/virt-manager PY=python3 sh bench/run.sh [SAIDA]
#   PY_SRC  checkout do virt-manager upstream (roda direto do código)
#   PY      python com gi, libvirt e libxml2 (padrão: python3)
#   RS_BIN  binários Rust (padrão: target/release)
#   RUNS    execuções de cada medida (padrão: 20 CLI, 10 GUI)
# Precisa de hyperfine, xprop, virsh e uma sessão gráfica.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
: "${PY_SRC:?defina PY_SRC com o checkout do virt-manager upstream}"
py=${PY:-python3}
rs=${RS_BIN:-$root/target/release}
cli_runs=${RUNS:-20}
gui_runs=${GUI_RUNS:-10}
out=${1:-$root/bench/results/$(date +%F)}
uri=qemu:///session
dom=bench-src
mkdir -p "$out"

# Ambiente, para a medida poder ser reproduzida e comparada.
{
    echo "date: $(date -Is)"
    echo "commit: $(git -C "$root" rev-parse HEAD)"
    echo "upstream: $(git -C "$PY_SRC" rev-parse HEAD)"
    echo "cpu: $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2 | sed 's/^ //')"
    echo "kernel: $(uname -r)"
    echo "python: $($py --version 2>&1)"
    echo "libvirt: $(pkg-config --modversion libvirt)"
    echo "gtk4: $(pkg-config --modversion gtk4)"
    echo "gtk3: $(pkg-config --modversion gtk+-3.0)"
    echo "libosinfo: $(pkg-config --modversion libosinfo-1.0)"
    echo "osinfo-db: $(ls /usr/share/osinfo/VERSION >/dev/null 2>&1 && cat /usr/share/osinfo/VERSION || echo ?)"
    echo "hyperfine: $(hyperfine --version)"
    echo "session: ${XDG_CURRENT_DESKTOP:-?} ${XDG_SESSION_TYPE:-?} (GUI medida via Xwayland)"
} > "$out/env.txt"

# Wrappers: o upstream roda do checkout, sem instalar nada.
pybin=$(mktemp -d)
trap 'rm -rf "$pybin"; virsh -c $uri undefine $dom >/dev/null 2>&1 || true' EXIT
for t in virt-install virt-xml virt-clone virt-manager; do
    printf '#!/bin/sh\nexec %s %s/%s "$@"\n' "$py" "$PY_SRC" "$t" > "$pybin/$t"
    chmod +x "$pybin/$t"
done

# Domínio desligado, sem disco, para virt-xml e virt-clone lerem.
"$rs/virt-install" --connect $uri --name $dom --memory 512 --disk none \
    --import --osinfo linux2022 --print-xml > "$pybin/dom.xml"
virsh -c $uri define "$pybin/dom.xml" >/dev/null

# CLI: só comandos que não mudam nada (--print-xml / --print-diff).
inst="--connect $uri --name bench-new --memory 512 --disk none --import --osinfo linux2022 --print-xml"
xml="--connect $uri $dom --edit --memory 768 --print-diff"
clone="--connect $uri --original $dom --auto-clone --print-xml"
hyperfine -N -w 3 -r "$cli_runs" --export-json "$out/cli.json" \
    -n rust-virt-install "$rs/virt-install $inst" \
    -n python-virt-install "$pybin/virt-install $inst" \
    -n rust-virt-xml "$rs/virt-xml $xml" \
    -n python-virt-xml "$pybin/virt-xml $xml" \
    -n rust-virt-clone "$rs/virt-clone $clone" \
    -n python-virt-clone "$pybin/virt-clone $clone"

# GUI: o padrão de cada um e, no Rust, o renderizador cairo do GTK4 para
# separar quanto do RSS vem do renderizador GL.
"$py" "$root/bench/gui.py" "$gui_runs" "$out/gui-python.json" -- "$pybin/virt-manager"
"$py" "$root/bench/gui.py" "$gui_runs" "$out/gui-rust.json" -- "$rs/virt-manager"
GSK_RENDERER=cairo "$py" "$root/bench/gui.py" "$gui_runs" "$out/gui-rust-cairo.json" -- "$rs/virt-manager"

"$py" "$root/bench/summary.py" "$out" | tee "$out/summary.md"
