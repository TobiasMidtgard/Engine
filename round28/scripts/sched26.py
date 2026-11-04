"""Round 26: per frame of an engine.trace capture, each schedule's runs and ms (main schedules), and the systems
named on the command line (ms a frame, runs, max run).

python -I sched26.py <trace> [system ...]
"""
import bisect, gzip, json, sys
from collections import defaultdict
path = sys.argv[1]; want = sys.argv[2:]
ev = json.load((gzip.open if path.endswith(".gz") else open)(path, "rt", encoding="utf-8"))["traceEvents"]
marks = sorted(e["ts"] for e in ev if e.get("cat") == "frame")
S = ["FixedUpdate", "Render", "Update", "PostUpdate"]
rows = defaultdict(lambda: defaultdict(lambda: [0, 0.0, 0.0]))
for e in ev:
    if e.get("ph") != "X" or not 0 <= e["ts"] < marks[-1]:
        continue
    k = e["name"] if (e["cat"] == "schedule" and e["name"] in S) or e["name"] in want else None
    if k:
        r = rows[bisect.bisect_right(marks, e["ts"])][k]; r[0] += 1; r[1] += e["dur"] / 1000; r[2] = max(r[2], e["dur"] / 1000)
walls = [(b - a) / 1000 for a, b in zip([0.0] + marks[:-1], marks)]
cols = S + want
print("frame  wall | " + " | ".join(f"{c} runs ms (max)" for c in cols))
tot = defaultdict(lambda: [0, 0.0, 0.0])
for i, w in enumerate(walls):
    cells = []
    for c in cols:
        n, ms, mx = rows[i][c]; t = tot[c]; t[0] += n; t[1] += ms; t[2] = max(t[2], mx)
        cells.append(f"{n:2} {ms:6.2f} ({mx:5.2f})")
    print(f"{i:5} {w:6.1f} | " + " | ".join(cells))
n = len(walls)
print("a frame  | " + " | ".join(f"{tot[c][0] / n:4.2f} runs {tot[c][1] / n:6.2f} ms, {tot[c][1] / max(1, tot[c][0]):.2f} a run, max {tot[c][2]:.2f}" for c in cols))
