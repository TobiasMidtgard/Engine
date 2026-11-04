"""Round 25 step b: per stop of a --loops run, each 5 s frame profile's FixedUpdate ms a frame (runs a frame), ms a
run, its ordering floor and chain, and the other schedules' ms; then the stop's mean over the profiles reporting in its
last 20 s (the tables' window).

python -I fixed25.py <run dir> > fixed_update.txt
"""
import re, statistics, sys
from datetime import datetime, timedelta

d = sys.argv[1]
L = open(f"{d}/stderr.log", encoding="utf-8", errors="replace").read().splitlines()
TS = re.compile(r"^(20\d\d-\d\d-\d\dT[\d:.]+)Z")


def ts(s):
    t = TS.match(s).group(1)
    if "." in t:
        a, b = t.split(".")
        t = a + "." + b[:6]
    return datetime.fromisoformat(t)


loops = [(ts(l), m.group(2)) for l in L for m in [re.search(r"showcase::loops: loop (\d+)/\d+ at (\w+):", l)] if m]
profs = []
for i, l in enumerate(L):
    if "engine_core::profile: frame profile" not in l:
        continue
    p = {"t": ts(l), "sched": {}, "floor": {}, "chain": {}}
    sec, last = None, None
    for j in range(i + 1, len(L)):
        s = L[j]
        if TS.match(s):
            break
        m = re.match(r"(\d+) frames in ([\d.]+) s: ([\d.]+) ms per frame \(([\d.]+) fps\), worst ([\d.]+) ms", s)
        if m:
            p["frames"], p["ms"], p["worst"] = int(m.group(1)), float(m.group(3)), float(m.group(5))
        if s.startswith("schedule ") and "ms/frame" in s:
            sec = "sched"; continue
        if s.startswith("slowest systems") or s.startswith("ran alone") or s.startswith("inside systems"):
            sec = None; continue
        if s.startswith("ordering floor"):
            sec = "floor"; continue
        if sec == "sched":
            m = re.match(r"^(\w+)\s+([\d.]+)\s+([\d.]+)\s*$", s)
            if m:
                p["sched"][m.group(1)] = (float(m.group(2)), float(m.group(3)))
        elif sec == "floor":
            m = re.match(r"^(\w+)\s+([\d.]+) of\s+([\d.]+) \((\d+) systems", s)
            if m:
                last = m.group(1)
                p["floor"][last] = (float(m.group(2)), float(m.group(3)), int(m.group(4)))
            m = re.match(r"^\s+chain: (.+)$", s)
            if m and last:
                p["chain"][last] = m.group(1)
    profs.append(p)

for end, key in loops:
    start = end - timedelta(seconds=25)
    print(f"== {key} ({start:%H:%M:%S}-{end:%H:%M:%S} UTC; * = reports in the last 20 s, the tables' window)")
    win = []
    for p in profs:
        if start < p["t"] <= end + timedelta(seconds=0.5):
            fu = p["sched"].get("FixedUpdate", (0, 0))
            fl = p["floor"].get("FixedUpdate", (None, None, None))
            star = "*" if p["t"] > end - timedelta(seconds=20) + timedelta(seconds=4.5) else " "
            if star == "*":
                win.append(p)
            print(f" {star}{p['t']:%H:%M:%S} {p.get('frames')} frames, {p.get('ms')} ms a frame, worst {p.get('worst')}: "
                  f"FixedUpdate {fu[0]:.2f} ms a frame (x{fu[1]}, {fu[0] / fu[1] if fu[1] else 0:.2f} a run), floor {fl[0]} "
                  f"({fl[2]} systems); Render {p['sched'].get('Render', ('-',))[0]}, Update {p['sched'].get('Update', ('-',))[0]}, "
                  f"PostUpdate {p['sched'].get('PostUpdate', ('-',))[0]}")
            print(f"     chain: {p['chain'].get('FixedUpdate', '')[:400]}")
    if win:
        def mean(f):
            v = [f(p) for p in win if f(p) is not None]
            return f"{statistics.mean(v):.2f} ({min(v):.2f}-{max(v):.2f})" if v else "n/a"
        print(f"  mean of {len(win)}: FixedUpdate {mean(lambda p: p['sched'].get('FixedUpdate', (None,))[0])} ms a frame, "
              f"runs {mean(lambda p: p['sched'].get('FixedUpdate', (None, None))[1])}, "
              f"floor {mean(lambda p: p['floor'].get('FixedUpdate', (None,))[0])}, "
              f"floor a run {mean(lambda p: p['floor']['FixedUpdate'][0] / p['sched']['FixedUpdate'][1] if 'FixedUpdate' in p['floor'] and p['sched'].get('FixedUpdate', (0, 0))[1] else None)}")
