"""Round 27 step b: inside one system of an engine.trace capture, every inner span that starts during one of its runs
(any thread but the render thread's, so work it hands to the pool counts too; the render thread overlaps the main
thread's next frame in time): ms a frame, runs a frame, ms a run, max; the system's own row
first, then the spans named on the command line (matched on the end of the span's name), then any other span found
inside it. Per frame too with --frames.

python -I inner27.py <trace> <system> [<span> ...] [--frames]
"""
import bisect, gzip, json, sys
from collections import defaultdict

args = sys.argv[1:]
per_frame = "--frames" in args
args = [a for a in args if a != "--frames"]
path, system, want = args[0], args[1], args[2:]
ev = json.load((gzip.open if path.endswith(".gz") else open)(path, "rt", encoding="utf-8"))["traceEvents"]
names = {e["tid"]: e["args"]["name"] for e in ev if e.get("ph") == "M"}
marks = sorted(e["ts"] for e in ev if e.get("cat") == "frame")
n = len(marks)
X = [e for e in ev if e.get("ph") == "X" and 0 <= e["ts"] < marks[-1]]
runs = sorted((e for e in X if e["cat"] == "system" and e["name"] == system), key=lambda e: e["ts"])
starts = [r["ts"] for r in runs]


def owner(e):
    i = bisect.bisect_right(starts, e["ts"]) - 1
    return runs[i] if i >= 0 and e["ts"] < runs[i]["ts"] + runs[i]["dur"] else None


def frame(e):
    return bisect.bisect_right(marks, e["ts"])


rows = defaultdict(list)  # span name -> [(frame, ms, thread)]
for e in X:
    if e["cat"] == "span" and owner(e) is not None and not str(names.get(e["tid"], "")).startswith("render"):
        rows[e["name"]].append((frame(e), e["dur"] / 1000, names.get(e["tid"], e["tid"])))
sysrow = [(frame(r), r["dur"] / 1000, names.get(r["tid"], r["tid"])) for r in runs]


def line(label, xs):
    if not xs:
        return f"  {label:48s}  (not in the trace)"
    ms = [x[1] for x in xs]
    threads = sorted({x[2] for x in xs})
    th = ", ".join(threads[:3]) + (f" +{len(threads) - 3}" if len(threads) > 3 else "")
    return (f"  {label:48s} {sum(ms) / n:7.3f} ms a frame, {len(ms) / n:5.2f} runs a frame, "
            f"{sum(ms) / len(ms):6.3f} a run, max {max(ms):6.2f}  ({th})")


print(f"{path.replace(chr(92), '/').split('/')[-1]}: {n} frames; inside {system} (spans starting during one of its runs)")
print(line(system, sysrow))
named = []
for w in want:
    keys = [k for k in rows if k.endswith(w)]
    named += keys
    print(line(keys[0] if len(keys) == 1 else w, [x for k in keys for x in rows[k]]))
others = sorted((k for k in rows if k not in named), key=lambda k: -sum(x[1] for x in rows[k]))
if others:
    print("  other spans inside it:")
    for k in others[:12]:
        print("  " + line(k, rows[k]))
if per_frame:
    cols = [system] + want
    print("\nper frame (ms; the system's runs summed, each span summed):")
    print("frame  " + "  ".join(f"{c[-22:]:>22s}" for c in cols))
    for f in range(n):
        vals = [sum(x[1] for x in sysrow if x[0] == f)]
        for w in want:
            vals.append(sum(x[1] for k in rows if k.endswith(w) for x in rows[k] if x[0] == f))
        print(f"{f:5d}  " + "  ".join(f"{v:22.2f}" for v in vals))
