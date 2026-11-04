"""Round 26: one line per frame of an engine.trace capture (.json or .json.gz), beside scripts/trace-summary.py.

python -I trace26.py <trace> [--top N] [--mark SYSTEM]

Per frame (between two `frame` marks): its wall ms, the main thread's systems ms (the thread that ran the most
schedule time), the longest systems of the frame (any thread, with the thread) and the longest inner spans; frames
that ran SYSTEM (default showcase::run, the --loops stepper: it runs on a stop's "loop" frame and on the jump frame
after it) are marked J. Then the frame ms stats, and with a J frame, the frames from the last J frame on (the jump).
"""
import bisect, gzip, json, statistics, sys
from collections import defaultdict

args = sys.argv[1:]
top = 4
mark = "showcase::run"
if "--top" in args:
    i = args.index("--top"); top = int(args[i + 1]); del args[i:i + 2]
if "--mark" in args:
    i = args.index("--mark"); mark = args[i + 1]; del args[i:i + 2]
path = args[0]
opener = gzip.open if path.endswith(".gz") else open
with opener(path, "rt", encoding="utf-8") as f:
    ev = json.load(f)["traceEvents"]
names = {e["tid"]: e["args"]["name"] for e in ev if e.get("ph") == "M"}
marks = sorted(e["ts"] for e in ev if e.get("cat") == "frame")
X = [e for e in ev if e.get("ph") == "X"]
frames = list(zip([0.0] + marks[:-1], marks))
sched_by_tid = defaultdict(float)
for e in X:
    if e["cat"] == "schedule":
        sched_by_tid[e["tid"]] += e["dur"]
main_tid = max(sched_by_tid, key=sched_by_tid.get) if sched_by_tid else None


by_frame = defaultdict(lambda: ([], []))  # frame -> (systems, inner spans) starting in it
for e in X:
    if e["cat"] in ("system", "span") and 0.0 <= e["ts"] < (marks[-1] if marks else 0):
        by_frame[bisect.bisect_right(marks, e["ts"])][0 if e["cat"] == "system" else 1].append(e)

rows = []
for i, (a, b) in enumerate(frames):
    sys_, spans = by_frame[i]
    main_ms = sum(e["dur"] for e in sys_ if e["tid"] == main_tid) / 1000
    j = any(e["name"] == mark for e in sys_)
    longest = sorted(sys_, key=lambda e: -e["dur"])[:top]
    inner = sorted(spans, key=lambda e: -e["dur"])[:top]
    rows.append((i, (b - a) / 1000, main_ms, j, longest, inner))

print(f"{path.replace(chr(92), '/').split('/')[-1]}: {len(frames)} frames, {len(X)} spans, "
      f"main thread {names.get(main_tid, main_tid)}")
print("frame  wall ms  main-thread systems ms  J  longest systems (ms, thread) | longest inner spans (ms)")
for i, wall, main_ms, j, longest, inner in rows:
    ls = ", ".join(f"{e['name']} {e['dur'] / 1000:.1f} ({names.get(e['tid'], e['tid'])[:12]})" for e in longest)
    ins = ", ".join(f"{e['name']} {e['dur'] / 1000:.1f}" for e in inner)
    print(f"{i:5} {wall:8.2f} {main_ms:8.2f}               {'J' if j else ' '}  {ls} | {ins}")
walls = [r[1] for r in rows]
if walls:
    s = sorted(walls)
    print(f"\nframe ms: mean {statistics.mean(walls):.2f}, median {statistics.median(walls):.2f}, "
          f"p90 {s[min(len(s) - 1, int(0.9 * len(s)))]:.2f}, max {max(walls):.2f}, min {min(walls):.2f}; "
          f"{1000 * len(walls) / max(sum(walls), 1e-9):.1f} fps over the trace")
js = [r[0] for r in rows if r[3]]
if js:
    k = js[-1]
    after = walls[k:]
    print(f"{mark} ran in frame(s) {js}; from frame {k} (the jump if it is the second): "
          + ", ".join(f"{w:.0f}" for w in after))
    first20 = after[:20]
    print(f"  first {len(first20)} frames from it: {min(first20):.1f}-{max(first20):.1f} ms, "
          f"sum {sum(first20):.0f} ms, mean {statistics.mean(first20):.1f}")
