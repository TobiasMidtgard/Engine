"""Round 23 step 2: per route leg, the worst frames with that frame's offframe.* and render CPU rows.

python -I route23.py <run dir>

Sources: stderr.log (leg lines), png/route/route.csv (one row a frame), fast.jsonl (engine.stats
render.cpu.* back to back, about once a frame: frame, frame_ms, every render CPU row) and
fast_offframe.jsonl (engine.stats offframe.* back to back on its own connection).
route.csv row i is frame base + i; base is found as in round 22 (route22.py): the offset at which the
rows' terrain_ms equal the render.cpu sampler's.
"""
import csv, json, re, statistics, sys
from collections import defaultdict
from datetime import datetime

d = sys.argv[1]
L = open(f"{d}/stderr.log", encoding="utf-8", errors="replace").read().splitlines()
TS = re.compile(r"^(20\d\d-\d\d-\d\dT[\d:.]+)Z")


def load(path):
    out = {}
    try:
        f = open(path, encoding="utf-8")
    except FileNotFoundError:
        return out
    for line in f:
        t, j = line.split(" ", 1)
        try:
            r = json.loads(j)["result"]
        except (json.JSONDecodeError, KeyError):
            continue
        out.setdefault(r["frame"], r["metrics"])
    return out


rows = list(csv.DictReader(open(f"{d}/png/route/route.csv")))
cpu = load(f"{d}/fast.jsonl")
off = load(f"{d}/fast_offframe.jsonl")


def route_terrain(m):
    return sum(v for k, v in m.items() if k.startswith("render.cpu.") and "terrain" in k[11:]
               and not k[11:].startswith("terrain:"))


best = (-1, 0)
for base in range(min(cpu) - 60, min(cpu) + 60):
    hits = sum(1 for i, r in enumerate(rows) if base + i in cpu and r["terrain_ms"] not in ("", "NaN")
               and abs(float(r["terrain_ms"]) - route_terrain(cpu[base + i])) < 0.006)
    if hits > best[0]:
        best = (hits, base)
base = best[1]
n_cpu = sum(1 for i in range(len(rows)) if base + i in cpu)
n_off = sum(1 for i in range(len(rows)) if base + i in off)
print(f"route.csv: {len(rows)} rows; row i = frame {base} + i (terrain_ms equal in {best[0]} of the {n_cpu} rows the "
      f"render.cpu sampler answered for); the offframe sampler answered for {n_off} of the rows")
for i, r in enumerate(rows):
    r["_frame"] = base + i


def offs(fr):
    # The frame's own offframe answer, else the nearest within one frame (marked).
    for dfr, mark in ((0, ""), (-1, " (frame before)"), (1, " (frame after)")):
        if fr + dfr in off:
            m = off[fr + dfr]
            return (f"offframe pending {m.get('offframe.pending', 0):g}, building_ms {m.get('offframe.building_ms', 0):.2f}, "
                    f"apply_ms {m.get('offframe.apply_ms', 0):.2f}{mark}")
    return "offframe: no answer for this frame"


def slowest(fr, n=5):
    m = cpu.get(fr)
    if not m:
        return "render.cpu: no answer for this frame"
    top = sorted(((k[len("render.cpu."):], v) for k, v in m.items() if k.startswith("render.cpu.")
                  and not k.startswith("render.cpu.terrain: ")), key=lambda x: -x[1])[:n]
    return ", ".join(f"{k} {v:.1f}" for k, v in top)


legs = []
for r in rows:
    if not legs or legs[-1][0] != r["leg"]:
        legs.append((r["leg"], []))
    legs[-1][1].append(r)
print("\nleg | frames | mean ms | median | max ms | over 33 | over 50 | RAM max MB | offframe pending max | building_ms max | apply_ms max | frames with apply_ms > 1")
for name, lr in legs:
    fm = [float(r["frame_ms"]) for r in lr]
    om = [off[r["_frame"]] for r in lr if r["_frame"] in off]
    mx = lambda k: max((m.get(k, 0) for m in om), default=float("nan"))
    print(f"{name} | {len(lr)} | {statistics.mean(fm):.1f} | {statistics.median(fm):.1f} | {max(fm):.1f} | "
          f"{sum(1 for v in fm if v > 33)} | {sum(1 for v in fm if v > 50)} | {max(float(r['ram_mb']) for r in lr):.0f} | "
          f"{mx('offframe.pending'):g} | {mx('offframe.building_ms'):.2f} | {mx('offframe.apply_ms'):.2f} | "
          f"{sum(1 for m in om if m.get('offframe.apply_ms', 0) > 1)}")
print("\nWorst 6 frames of each leg (route.csv frame_ms, time, m up; the frame's offframe.*; its 5 slowest render CPU rows):")
for name, lr in legs:
    print(f"== {name}")
    for r in sorted(lr, key=lambda r: -float(r["frame_ms"]))[:6]:
        fr = r["_frame"]
        print(f"  frame {fr}, {r['t_s']} s, {float(r['altitude_m']):.0f} m: {float(r['frame_ms']):.1f} ms, render CPU "
              f"{float(r['render_cpu_ms']):.1f}; {offs(fr)}; {slowest(fr)}")
print("\nWorst 25 frames of the run:")
for r in sorted(rows, key=lambda r: -float(r["frame_ms"]))[:25]:
    fr = r["_frame"]
    print(f"  frame {fr}, {r['t_s']} s, {r['leg']}, {float(r['altitude_m']):.0f} m: {float(r['frame_ms']):.1f} ms, render CPU "
          f"{float(r['render_cpu_ms']):.1f}; {offs(fr)}; {slowest(fr, 4)}")
big = sorted(((fr, m) for fr, m in off.items() if m.get("offframe.apply_ms", 0) > 1), key=lambda x: -x[1]["offframe.apply_ms"])
row_of = {r["_frame"]: r for r in rows}
print(f"\nFrames with offframe.apply_ms > 1 ms ({len(big)}; top 20 by apply_ms, with the frame's route.csv row):")
for fr, m in big[:20]:
    r = row_of.get(fr)
    where = f"{r['leg']}, {float(r['altitude_m']):.0f} m, {float(r['frame_ms']):.1f} ms" if r else "not a route frame"
    print(f"  frame {fr}: pending {m.get('offframe.pending', 0):g}, building_ms {m.get('offframe.building_ms', 0):.2f}, "
          f"apply_ms {m['offframe.apply_ms']:.2f} ({where})")
