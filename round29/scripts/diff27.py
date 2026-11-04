"""Round 27 step a: per schedule, each system's ms a frame in two engine.trace captures (A, B) and the difference,
the largest differences first; then the schedule's wall ms a frame in each.

python -I diff27.py <trace A> <trace B> <schedule> [rows]
"""
import bisect, gzip, json, sys
from collections import defaultdict


def load(path, sched):
    ev = json.load((gzip.open if path.endswith(".gz") else open)(path, "rt", encoding="utf-8"))["traceEvents"]
    names = {e["tid"]: e["args"]["name"] for e in ev if e.get("ph") == "M"}
    marks = sorted(e["ts"] for e in ev if e.get("cat") == "frame")
    X = [e for e in ev if e.get("ph") == "X" and 0 <= e["ts"] < marks[-1]]
    sch = sorted((e for e in X if e["cat"] == "schedule" and e["name"] == sched), key=lambda e: e["ts"])
    st = [s["ts"] for s in sch]
    out = defaultdict(float)
    for e in X:
        if e["cat"] != "system" or str(names.get(e["tid"], "")).startswith("render"):
            continue
        i = bisect.bisect_right(st, e["ts"]) - 1
        if i >= 0 and e["ts"] < sch[i]["ts"] + sch[i]["dur"]:
            out[e["name"]] += e["dur"] / 1000
    n = len(marks)
    return {k: v / n for k, v in out.items()}, sum(s["dur"] for s in sch) / 1000 / n, len(sch) / n


a, b, sched = sys.argv[1], sys.argv[2], sys.argv[3]
rows = int(sys.argv[4]) if len(sys.argv) > 4 else 15
A, wa, ra = load(a, sched)
B, wb, rb = load(b, sched)
print(f"{sched}: wall {wa:.2f} ms a frame ({ra:.2f} runs) in A, {wb:.2f} ({rb:.2f} runs) in B; systems' sum "
      f"{sum(A.values()):.2f} / {sum(B.values()):.2f}")
print("   A ms    B ms    B-A  system (ms a frame)")
for k in sorted(set(A) | set(B), key=lambda k: -abs(B.get(k, 0) - A.get(k, 0)))[:rows]:
    print(f"  {A.get(k, 0):6.3f}  {B.get(k, 0):6.3f}  {B.get(k, 0) - A.get(k, 0):+6.3f}  {k}")
