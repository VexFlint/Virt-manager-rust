#!/usr/bin/env python3
# Mede a GUI: tempo até a janela principal mapear, RSS e CPU ociosa.
# Uso: gui.py RUNS SAIDA.json -- COMANDO [ARGS...]
# Roda o comando com --no-fork --connect qemu:///session, sob Xwayland
# (GDK_BACKEND=x11) para achar a janela pelo _NET_WM_PID com xprop.
import json
import os
import re
import statistics
import subprocess
import sys
import time

IDLE_WAIT = 5    # segundos entre a janela aparecer e ler o RSS
IDLE_SPAN = 10   # segundos de amostragem da CPU ociosa

runs, out = int(sys.argv[1]), sys.argv[2]
cmd = sys.argv[sys.argv.index("--") + 1:]
env = dict(os.environ, GDK_BACKEND="x11", GSETTINGS_BACKEND="memory")
tick = os.sysconf("SC_CLK_TCK")


def windows():
    o = subprocess.run(["xprop", "-root", "_NET_CLIENT_LIST"],
                       capture_output=True, text=True).stdout
    return re.findall(r"0x[0-9a-f]+", o)


def window_pid(w):
    o = subprocess.run(["xprop", "-id", w, "_NET_WM_PID"],
                       capture_output=True, text=True).stdout
    m = re.search(r"= (\d+)", o)
    return int(m.group(1)) if m else None


def cpu_ticks(pid):
    f = open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()
    return int(f[11]) + int(f[12])


def rss_mib(pid):
    for line in open(f"/proc/{pid}/status"):
        if line.startswith("VmRSS"):
            return int(line.split()[1]) / 1024


samples = []
for i in range(runs):
    before = set(windows())
    t0 = time.perf_counter()
    p = subprocess.Popen(cmd + ["--no-fork", "--connect", "qemu:///session"],
                         env=env, stdout=subprocess.DEVNULL,
                         stderr=subprocess.DEVNULL)
    startup = None
    while time.perf_counter() - t0 < 30:
        if any(w not in before and window_pid(w) == p.pid for w in windows()):
            startup = time.perf_counter() - t0
            break
        time.sleep(0.005)
    if startup is None:
        p.kill()
        sys.exit(f"run {i}: a janela não apareceu em 30 s")
    time.sleep(IDLE_WAIT)
    rss = rss_mib(p.pid)
    c0 = cpu_ticks(p.pid)
    time.sleep(IDLE_SPAN)
    cpu = (cpu_ticks(p.pid) - c0) / tick / IDLE_SPAN * 100
    p.terminate()
    p.wait(10)
    samples.append({"startup_s": startup, "rss_mib": rss, "idle_cpu_pct": cpu})
    print(f"run {i}: {startup * 1000:.0f} ms, {rss:.1f} MiB, {cpu:.2f}% CPU",
          file=sys.stderr)


def stats(key):
    v = [s[key] for s in samples]
    q = statistics.quantiles(v, n=4) if len(v) > 1 else [v[0]] * 3
    return {"median": statistics.median(v), "q1": q[0], "q3": q[2],
            "min": min(v), "max": max(v)}


json.dump({"command": cmd, "runs": runs, "samples": samples,
           "summary": {k: stats(k) for k in samples[0]}},
          open(out, "w"), indent=2)
