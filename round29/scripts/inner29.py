"""Round 29: inside a system or span of an engine.trace capture (the container, matched on the end of its name), the
inner spans named on the command line that start during one of its runs: ms a frame, runs a frame, ms a run, max; the
container's own row first, then the named spans (matched on the end of their names), then any other span found inside.
A container on a render thread counts only spans on its own thread; any other container counts its own thread's spans
and, on the other threads but the render threads', spans of its own crate (so the pool work a system hands out
counts, but not the systems beside it; the render threads overlap the main thread's next frame). Per frame too with --frames (the container's runs summed, each span summed).
Round 27's inner27.py took systems only.

python -I inner29.py <trace> <container> [<span> ...] [--frames]
"""
import bisect, gzip, json, sys
from collections import defaultdict

args = sys.argv[1:]
per_frame = "--frames" in args
args = [a for a in args if a != "--frames"]
path, container, want = args[0], args[1], args[2:]
ev = json.load((gzip.open if path.endswith(".gz") else open)(path, "rt", encoding="utf-8"))["traceEvents"]
names = {e["tid"]: e["args"]["name"] for e in ev if e.get("ph") == "M"}
marks = sorted(e["ts"] for e in ev if e.get("cat") == "frame")
n = len(marks)
X = [e for e in ev if e.get("ph") == "X" and 0 <= e["ts"] < marks[-1]]
runs = sorted((e for e in X if e["cat"] in ("system", "span") and e["name"].endswith(container)), key=lambda e: e["ts"])
on_render = lambda tid: str(names.get(tid, "")).startswith("render")
by_tid = defaultdict(list)
for r in runs:
    by_tid[r["tid"]].append(r)
starts_any = [r["ts"] for r in runs]


def owner(e):
    """The container run this span starts in, under the thread rule above."""
    cands = by_tid.get(e["tid"], [])
    i = bisect.bisect_right([c["ts"] for c in cands], e["ts"]) - 1
    if i >= 0 and cands[i] is not e and e["ts"] < cands[i]["ts"] + cands[i]["dur"]:
        return cands[i]
    if on_render(e["tid"]):
        return None
    i = bisect.bisect_right(starts_any, e["ts"]) - 1
    while i >= 0:
        r = runs[i]
        # On another thread a system's span must come from its own crate (engine_water::x -> "engine_water: ..."),
        # so the systems running beside it don't count.
        same_crate = e["name"].startswith(r["name"].split("::")[0].split(":")[0] + ":")
        if r is not e and not on_render(r["tid"]) and same_crate and r["ts"] <= e["ts"] < r["ts"] + r["dur"]:
            return r
        if r["ts"] + 200000 < e["ts"]:
            break
        i -= 1
    return None


def frame(e):
    return bisect.bisect_right(marks, e["ts"])


rows = defaultdict(list)
run_ids = {id(r) for r in runs}
for e in X:
    if e["cat"] == "span" and id(e) not in run_ids and owner(e) is not None:
        rows[e["name"]].append((frame(e), e["dur"] / 1000, names.get(e["tid"], e["tid"])))
own = [(frame(r), r["dur"] / 1000, names.get(r["tid"], r["tid"])) for r in runs]
label = runs[0]["name"] if runs else container


def line(lbl, xs):
    if not xs:
        return f"  {lbl:52s}  (not in the trace)"
    ms = [x[1] for x in xs]
    threads = sorted({str(x[2]) for x in xs})
    th = ", ".join(threads[:3]) + (f" +{len(threads) - 3}" if len(threads) > 3 else "")
    return (f"  {lbl:52s} {sum(ms) / n:7.3f} ms a frame, {len(ms) / n:5.2f} runs a frame, "
            f"{sum(ms) / len(ms):6.3f} a run, max {max(ms):6.2f}  ({th})")


print(f"{path.replace(chr(92), '/').split('/')[-1]}: {n} frames; inside {label} (spans starting during one of its runs)")
print(line(label, own))
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
    cols = [label] + want
    print("\nper frame (ms; the container's runs summed, each span summed; runs in brackets):")
    print("frame  " + "  ".join(f"{c[-24:]:>30s}" for c in cols))
    for f in range(n):
        cells = []
        xs = [x for x in own if x[0] == f]
        cells.append(f"{sum(x[1] for x in xs):24.2f} ({len(xs):2d})")
        for w in want:
            ys = [x for k in rows if k.endswith(w) for x in rows[k] if x[0] == f]
            cells.append(f"{sum(x[1] for x in ys):24.2f} ({len(ys):2d})")
        print(f"{f:5d}  " + "  ".join(cells))
