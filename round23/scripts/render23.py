"""Round 23 step 1: the plaza stand's render thread keys, twice a second, and the steady state.

python -I render23.py <run dir> [steady_from_s steady_to_s]

Sources: sampled_stats.jsonl (engine.stats "render." and "offframe." in turn, twice a second: each answer
has its frame, fps and frame_ms), frames.csv (engine.status twice a second: frame, real time, fixed_tick,
fps), timeline.txt (AT plaza).
"""
import csv, json, re, statistics, sys
from datetime import datetime

d = sys.argv[1]
lo, hi = (float(sys.argv[2]), float(sys.argv[3])) if len(sys.argv) > 3 else (25.0, 75.0)
KEYS = ["render.wait_ms", "render.extract_ms", "render.extract_wait_ms", "render.record_ms",
        "render.record_total_ms", "render.record_cpu_ms", "render.cpu_ms"]
MORE = ["render.finish_ms", "render.submit_ms", "render.present_ms", "render.screenshot_ms",
        "render.record_other_ms"]
OFF = ["offframe.pending", "offframe.building_ms", "offframe.apply_ms"]


def iso(s):
    s = s.rstrip("Z")
    if "." in s:
        a, b = s.split(".")
        s = a + "." + b[:6]
    return datetime.fromisoformat(s)


day = None
at = None
for line in open(f"{d}/timeline.txt", encoding="utf-8"):
    m = re.match(r"^(\d\d:\d\d:\d\d\.\d+)Z\s+\S+\s+AT plaza:", line)
    if m:
        at = m.group(1)
rows = []
for line in open(f"{d}/sampled_stats.jsonl", encoding="utf-8"):
    t, j = line.split(" ", 1)
    try:
        r = json.loads(j)["result"]
    except (json.JSONDecodeError, KeyError):
        continue
    rows.append((iso(t), r))
day = rows[0][0].strftime("%Y-%m-%d")
at = datetime.fromisoformat(f"{day}T{at}") if at else rows[0][0]
status = []
for r in csv.DictReader(open(f"{d}/frames.csv")):
    if r["fixed_tick"]:
        status.append((iso(r["wall_utc"]), int(r["frame"]), float(r["elapsed_real"]), int(r["fixed_tick"]),
                       float(r["fps"]) if r.get("fps") else None))


def steps_and_fps(t):
    # Fixed steps a frame and fps over the 1 s of engine.status samples ending at t.
    win = [s for s in status if 0 <= (t - s[0]).total_seconds() <= 1.0]
    if len(win) < 2:
        return None, None, None
    a, b = win[0], win[-1]
    df = b[1] - a[1]
    return ((b[3] - a[3]) / df if df else None), (df / (b[2] - a[2]) if b[2] > a[2] else None), b[4]


rend = [(t, r) for t, r in rows if "render.wait_ms" in r["metrics"]]
off = [(t, r) for t, r in rows if any(k in r["metrics"] for k in OFF)]
print(f"AT plaza {at:%H:%M:%S.%f} UTC; {len(rend)} render.* samples, {len(off)} offframe.* samples, "
      f"{len(status)} engine.status samples")
print("\nEvery render.* sample (s after AT plaza; frame; engine.stats fps and frame_ms (Diagnostics, smoothed);"
      " the render thread keys; fixed steps a frame and fps from engine.status over the second before):")
hdr = ["s", "frame", "fps", "frame_ms"] + [k.replace("render.", "") for k in KEYS + MORE] + ["fixed/frame", "status fps"]
print(" | ".join(hdr))
for t, r in rend:
    m = r["metrics"]
    st, fps, _ = steps_and_fps(t)
    vals = [f"{(t - at).total_seconds():6.1f}", str(r["frame"]), f"{r['fps']:.1f}", f"{r['frame_ms']:.1f}"]
    vals += [f"{m.get(k, float('nan')):.2f}" for k in KEYS + MORE]
    vals += [f"{st:.2f}" if st is not None else "", f"{fps:.1f}" if fps is not None else ""]
    print(" | ".join(vals))


def summ(vals):
    vals = sorted(v for v in vals if v is not None)
    if not vals:
        return "n/a"
    p90 = vals[min(len(vals) - 1, int(len(vals) * 0.9))]
    return f"mean {statistics.mean(vals):.2f}, median {statistics.median(vals):.2f}, p90 {p90:.2f}, max {vals[-1]:.2f}"


print(f"\nSteady state, {lo:g}-{hi:g} s after AT plaza:")
sr = [(t, r) for t, r in rend if lo <= (t - at).total_seconds() <= hi]
print(f"  {len(sr)} render.* samples")
for k in ["frame_ms"] + KEYS + MORE:
    v = [r[k] if k == "frame_ms" else r["metrics"].get(k) for t, r in sr]
    print(f"  {k:28s} {summ(v)}")
fr = [r["frame_ms"] for t, r in sr]
for k in ("render.wait_ms", "render.record_total_ms", "render.record_cpu_ms", "render.cpu_ms"):
    ratio = [r["metrics"][k] / r["frame_ms"] for t, r in sr if k in r["metrics"] and r["frame_ms"] > 0]
    print(f"  {k} / frame_ms: {summ(ratio)}")
ss = [s for s in status if lo <= (s[0] - at).total_seconds() <= hi]
if len(ss) >= 2:
    a, b = ss[0], ss[-1]
    print(f"  engine.status over the span: {(b[1] - a[1]) / (b[2] - a[2]):.1f} fps, "
          f"{(b[3] - a[3]) / (b[1] - a[1]):.2f} fixed steps a frame ({b[1] - a[1]} frames, {b[2] - a[2]:.1f} s)")
so = [(t, r) for t, r in off if lo <= (t - at).total_seconds() <= hi]
for k in OFF:
    print(f"  {k:28s} {summ([r['metrics'].get(k) for t, r in so])}  ({len(so)} samples)")
print("\noffframe.* samples (s after AT plaza: pending, building_ms, apply_ms), nonzero ones:")
nz = [(t, r) for t, r in off if any(r["metrics"].get(k, 0) for k in OFF)]
for t, r in nz[:80]:
    m = r["metrics"]
    print(f"  {(t - at).total_seconds():6.1f}  frame {r['frame']}: " + ", ".join(f"{k.split('.')[1]} {m.get(k, 0):g}" for k in OFF))
print(f"  ({len(nz)} of {len(off)} samples nonzero)")
