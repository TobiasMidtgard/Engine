"""Round 29b step 2: the lake arrival, frame by frame (round 28's arrive28.py, plus arrival_stats2.jsonl: the answers of a
second connection asked the same frames, here engine.stats "render.cpu.meshes", every key per answer).
Round 28 step 4: the lake arrival, frame by frame.

python -I arrive28.py <run dir> > arrival.txt

- arrival_stats.jsonl: the engine.stats "render.gi" answers asked back to back right after showcase.goto (one a frame;
  the bridge runs a request at a frame's start, so answer j, read at the start of the (j+1)th frame after the goto's,
  holds what the frames up to the jth after it left): per answer the wall time, the engine's frame number, frame_ms
  (the engine's smoothed frame time), the wall interval since the answer before over the frames between them, and every
  render.gi.* key;
- trace_lake_arrival.json(.gz): engine.trace answered just before the goto, so its first frame is the goto's: per frame
  its wall ms, the main thread's systems ms, the longest systems and the longest inner spans, and the render spans
  whose name mentions gi or probe.
"""
import bisect, glob, gzip, json, os, re, statistics, sys
from datetime import datetime

d = sys.argv[1]


def iso(s):
    s = s.rstrip("Z")
    if "." in s:
        a, b = s.split(".")
        s = a + "." + b[:6]
    return datetime.fromisoformat(s)


rows = []
for line in open(f"{d}/arrival_stats.jsonl", encoding="utf-8-sig"):
    line = line.strip()
    if not line:
        continue
    t, j = line.split(" ", 1)
    try:
        r = json.loads(j)["result"]
    except (json.JSONDecodeError, KeyError):
        continue
    rows.append((iso(t), r))
keys = sorted({k for _, r in rows for k in r["metrics"]})
short = [k[len("render.gi."):] for k in keys]
print(f"== {len(rows)} engine.stats \"render.gi\" answers after the goto (j = answer; frame = the engine's frame number;"
      " interval = wall ms since the answer before / frames between)")
print(f"{'j':>2} {'wall UTC':12s} {'frame':>7s} {'frame_ms':>8s} {'interval':>8s}  " + "  ".join(f"{s:>15s}" for s in short))
prev = None
for j, (t, r) in enumerate(rows):
    iv = ""
    if prev and r["frame"] > prev[1]["frame"]:
        iv = f"{(t - prev[0]).total_seconds() * 1000 / (r['frame'] - prev[1]['frame']):8.1f}"
    m = r["metrics"]
    vals = "  ".join(f"{'-':>15s}" if k not in m else f"{int(m[k]):15d}" if float(m[k]).is_integer() else f"{m[k]:15.3f}"
                     for k in keys)
    print(f"{j:2d} {t:%H:%M:%S.%f}"[:-3] + f" {r['frame']:7d} {r['frame_ms']:8.2f} {iv:>8s}  {vals}")
    prev = (t, r)
if rows:
    fr = [r["frame"] for _, r in rows]
    print(f"frames {fr[0]}-{fr[-1]}: {len(set(fr))} distinct of {len(fr)} answers"
          + ("" if len(set(fr)) == len(fr) else " (some answers came in the same frame)"))

rows2 = []
try:
    for line in open(f"{d}/arrival_stats2.jsonl", encoding="utf-8-sig"):
        line = line.strip()
        if not line:
            continue
        t2, j2 = line.split(" ", 1)
        try:
            rows2.append((iso(t2), json.loads(j2)["result"]))
        except (json.JSONDecodeError, KeyError):
            pass
except FileNotFoundError:
    pass
if rows2:
    keys2 = sorted({k for _, r in rows2 for k in r["metrics"]})
    print(f"\n== {len(rows2)} answers on the second connection, asked the same frames (engine frame; ms)")
    print(f"{'j':>2} {'frame':>7s}  " + "  ".join(f"{k.split('.')[-1][-18:]:>18s}" for k in keys2))
    for j, (t2, r) in enumerate(rows2):
        print(f"{j:2d} {r['frame']:7d}  " + "  ".join(f"{r['metrics'].get(k, float('nan')):18.3f}" for k in keys2))
    same = sum(1 for (a, r1), (b, r2) in zip(rows, rows2) if r1["frame"] == r2["frame"])
    print(f"answers in the same frame on both connections: {same} of {min(len(rows), len(rows2))}")

tr = (glob.glob(f"{d}/trace_lake_arrival.json") + glob.glob(f"{d}/trace_lake_arrival.json.gz"))
if tr:
    path = tr[0]
    ev = json.load((gzip.open if path.endswith(".gz") else open)(path, "rt", encoding="utf-8"))["traceEvents"]
    names = {e["tid"]: e["args"]["name"] for e in ev if e.get("ph") == "M"}
    marks = sorted(e["ts"] for e in ev if e.get("cat") == "frame")
    X = [e for e in ev if e.get("ph") == "X" and 0 <= e["ts"] < marks[-1]]
    sched = {}
    for e in X:
        if e["cat"] == "schedule":
            sched[e["tid"]] = sched.get(e["tid"], 0) + e["dur"]
    main = max(sched, key=sched.get)
    by = {}
    for e in X:
        by.setdefault(bisect.bisect_right(marks, e["ts"]), []).append(e)
    walls = [(b - a) / 1000 for a, b in zip([0.0] + marks[:-1], marks)]
    print(f"\n== {os.path.basename(path)}: {len(marks)} frames (frame 0 is the goto's); wall ms, main-thread systems ms, "
          "longest systems (ms, thread) | longest inner spans | gi / probe spans")
    for i, w in enumerate(walls):
        es = by.get(i, [])
        sy = sorted((e for e in es if e["cat"] == "system"), key=lambda e: -e["dur"])[:4]
        sp = sorted((e for e in es if e["cat"] == "span"), key=lambda e: -e["dur"])[:4]
        gi = [e for e in es if e["cat"] == "span" and re.search(r"\bgi\b|probe", e["name"])]
        mm = sum(e["dur"] for e in es if e["cat"] == "system" and e["tid"] == main) / 1000
        print(f"{i:3d} {w:7.2f} {mm:7.2f}  "
              + ", ".join(f"{e['name']} {e['dur'] / 1000:.1f} ({names.get(e['tid'], e['tid'])[:10]})" for e in sy)
              + " | " + ", ".join(f"{e['name']} {e['dur'] / 1000:.1f}" for e in sp)
              + " | " + (", ".join(f"{e['name']} {e['dur'] / 1000:.2f}" for e in gi) or "-"))
    print(f"frame ms: {', '.join(f'{w:.0f}' for w in walls)}")
    print(f"  mean {statistics.mean(walls):.1f}, max {max(walls):.1f}, sum {sum(walls):.0f} ms over {len(walls)} frames")
