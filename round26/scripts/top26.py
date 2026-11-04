"""Round 26: the systems (and inner spans) with the most time in frames lo..hi of an engine.trace capture, inside one
schedule: ms a frame (all threads but the render thread, whose work overlaps the main thread's next frame), runs a
frame, ms a run, max.

python -I top26.py <trace> <schedule> <lo> <hi> [rows]
"""
import bisect, gzip, json, sys
from collections import defaultdict
path, sched, lo, hi = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4]); rows = int(sys.argv[5]) if len(sys.argv) > 5 else 20
ev = json.load((gzip.open if path.endswith(".gz") else open)(path, "rt", encoding="utf-8"))["traceEvents"]
names = {e["tid"]: e["args"]["name"] for e in ev if e.get("ph") == "M"}
marks = sorted(e["ts"] for e in ev if e.get("cat") == "frame")
frames = list(zip([0.0] + marks[:-1], marks))
a, b = frames[lo][0], frames[hi][1]
sch = sorted((e for e in ev if e.get("ph") == "X" and e["cat"] == "schedule" and e["name"] == sched and a <= e["ts"] < b), key=lambda e: e["ts"])
starts = [s["ts"] for s in sch]
def inside(e):
    i = bisect.bisect_right(starts, e["ts"]) - 1
    return i >= 0 and e["ts"] < sch[i]["ts"] + sch[i]["dur"]
n = hi - lo + 1
for cat in ("system", "span"):
    t = defaultdict(lambda: [0, 0.0, 0.0])
    for e in ev:
        if e.get("ph") == "X" and e["cat"] == cat and a <= e["ts"] < b and inside(e) and not names.get(e["tid"], "").startswith("render"):
            r = t[e["name"]]; r[0] += 1; r[1] += e["dur"] / 1000; r[2] = max(r[2], e["dur"] / 1000)
    print(f"{cat}s in {sched}, frames {lo}-{hi} ({len(sch) / n:.2f} {sched} runs a frame): ms a frame / runs a frame / ms a run / max")
    for k, (c, ms, mx) in sorted(t.items(), key=lambda kv: -kv[1][1])[:rows]:
        print(f"  {ms / n:7.2f} {c / n:5.2f} {ms / c:6.2f} {mx:6.2f}  {k}")
