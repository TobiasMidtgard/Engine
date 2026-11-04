"""Round 26: for each 5 s frame profile in a run's stderr.log, the rows naming any of the given systems or inner spans
(ms a frame / max of a run / runs a frame, as the profile prints them), with the stop the profile falls in (a --loops
run's "loop" lines) or the seconds after "AT <place>" (a stops run's timeline). Then the max of a run per stop.

python -I spanmax26.py <run dir> <name> [<name> ...]
"""
import re, sys
from datetime import datetime

d = sys.argv[1]; names = sys.argv[2:]
L = open(f"{d}/stderr.log", encoding="utf-8", errors="replace").read().splitlines()
TS = re.compile(r"^(20\d\d-\d\d-\d\dT[\d:.]+)Z")


def t(s):
    return datetime.fromisoformat(s[:26])


loops = [(t(m.group(1)), m.group(2)) for l in L for m in [re.match(r"^(\S+)Z .*showcase::loops: loop \d+/\d+ at (\w+):", l)] if m]
at = None
try:
    for l in open(f"{d}/timeline.txt", encoding="utf-8-sig"):
        m = re.match(r"^(\d\d:\d\d:\d\d\.\d+)Z\s+\S+\s+AT (\w+):", l)
        if m and not loops:
            at = (m.group(1), m.group(2))
except FileNotFoundError:
    pass


def where(ts):
    if loops:
        for lt, k in loops:
            if lt >= ts:
                return k
        return "after"
    if at:
        a = datetime.fromisoformat(ts.date().isoformat() + "T" + at[0][:12])
        return f"{(ts - a).total_seconds():5.1f} s after AT {at[1]}"
    return ""


best = {}
cur = None
for l in L:
    m = TS.match(l)
    if m:
        cur = t(m.group(1)) if "frame profile" in l else None
        continue
    if cur is None:
        continue
    for n in names:
        if n in l:
            nums = re.findall(r"\d+\.\d+|\d+", l.split(n, 1)[1])
            w = where(cur)
            print(f"{cur:%H:%M:%S} {w:24s} {l.strip()}")
            if len(nums) >= 3:
                v = float(nums[-2])
                key = (w if loops else "", n)
                if key not in best or v > best[key][0]:
                    best[key] = (v, f"{cur:%H:%M:%S} {w}")
print("\nmax of a run (per stop for a loops run):")
for (w, n), (v, ts) in sorted(best.items()):
    print(f"  {w:10s} {n:40s} {v:7.1f} ({ts})")
