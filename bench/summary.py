#!/usr/bin/env python3
# Gera a tabela em Markdown a partir dos JSON de bench/run.sh.
# Uso: summary.py DIRETORIO
import json
import os
import statistics
import sys

d = sys.argv[1]


# hyperfine 1.x grava "times"/"memory_usage_byte"; o 2.x, "measurements".
LEGACY = {"time_wall_clock": "times", "memory_peak_resident": "memory_usage_byte"}


def series(result, key):
    if "measurements" in result:
        return [m[key]["value"] for m in result["measurements"]]
    return result[LEGACY[key]]


def iqr(v):
    q = statistics.quantiles(v, n=4)
    return q[2] - q[0]


print("| Measure | Python | Rust | Rust / Python |")
print("|---|---|---|---|")

cli = {r["name"]: r for r in json.load(open(os.path.join(d, "cli.json")))["results"]}
for tool in ("virt-install", "virt-xml", "virt-clone"):
    for key, label, scale, unit in (
            ("time_wall_clock", "time", 1000, "ms"),
            ("memory_peak_resident", "peak RSS", 1 / 2**20, "MiB")):
        vals = []
        for impl in ("python", "rust"):
            m = series(cli[f"{impl}-{tool}"], key)
            vals.append((statistics.median(m) * scale, iqr(m) * scale))
        (p, pi), (r, ri) = vals
        print(f"| `{tool}` {label} | {p:.0f} ± {pi:.0f} {unit} "
              f"| {r:.0f} ± {ri:.0f} {unit} | {r / p:.2f} |")

gui = {n: json.load(open(os.path.join(d, f"gui-{n}.json")))["summary"]
       for n in ("python", "rust", "rust-cairo")}
for key, label, scale, unit, fmt in (
        ("startup_s", "GUI: main window shown", 1000, "ms", ".0f"),
        ("rss_mib", "GUI: idle RSS", 1, "MiB", ".0f"),
        ("idle_cpu_pct", "GUI: idle CPU", 1, "%", ".2f")):
    cell = {}
    for n, s in gui.items():
        v = s[key]
        cell[n] = (v["median"] * scale, (v["q3"] - v["q1"]) * scale)
    (p, pi), (r, ri) = cell["python"], cell["rust"]
    # /proc conta CPU em ticks de 10 ms: em 10 s, a resolução é 0,1%.
    ratio = "n/a" if key == "idle_cpu_pct" else f"{r / p:.2f}"
    print(f"| {label} | {p:{fmt}} ± {pi:{fmt}} {unit} "
          f"| {r:{fmt}} ± {ri:{fmt}} {unit} | {ratio} |")
    if key == "rss_mib":
        c, ci = cell["rust-cairo"]
        print(f"| GUI: idle RSS, `GSK_RENDERER=cairo` | | "
              f"{c:.0f} ± {ci:.0f} MiB | {c / p:.2f} |")

print("\nValues are median ± interquartile range. Idle CPU resolution is 0.1%.")
