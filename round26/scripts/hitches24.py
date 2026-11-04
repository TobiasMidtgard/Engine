"""Round 24: every 5 s frame profile of a --loops run whose worst frame is over 80 ms, with the stop it falls in and the
systems and inner spans that had a single run over 25 ms (slowest systems / inside systems lists).

python -I hitches24.py <run dir> > hitches.txt
"""
import re, sys
from datetime import datetime, timedelta

d = sys.argv[1]
L = open(f"{d}/stderr.log", encoding="utf-8", errors="replace").read().splitlines()
TS = re.compile(r"^(20\d\d-\d\d-\d\dT[\d:.]+)Z")
loops = []
for l in L:
    m = re.search(r"showcase::loops: loop (\d+)/\d+ at (\w+):", l)
    if m:
        loops.append((datetime.fromisoformat(TS.match(l).group(1)[:26]), m.group(2)))


def stop(t):
    for e, k in loops:
        if e - timedelta(seconds=25) < t <= e + timedelta(seconds=0.5):
            return k
    return "start"


for i, l in enumerate(L):
    if "engine_core::profile: frame profile" not in l:
        continue
    t = datetime.fromisoformat(TS.match(l).group(1)[:26])
    b = []
    for j in range(i + 1, len(L)):
        if TS.match(L[j]):
            break
        b.append(L[j])
    w = float(re.search(r"worst ([\d.]+) ms", b[0]).group(1))
    if w < 80:
        continue
    big, sec = [], None
    for s in b:
        if s.startswith("slowest systems"):
            sec = "s"; continue
        if s.startswith("ran alone"):
            sec = "a"; continue
        if s.startswith("inside systems"):
            sec = "i"; continue
        if s.startswith("ordering floor"):
            sec = None
        if sec in ("s", "i"):
            m = re.match(r"^(?:  in )?(.+?)\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)\s*$", s)
            if m and float(m.group(3)) > 25:
                big.append(f"{'in ' if sec == 'i' else ''}{m.group(1).replace('engine_', '')} {float(m.group(3)):.1f}")
    print(f"{t:%H:%M:%S} {stop(t):9s} worst {w:7.1f}: " + ", ".join(big))
