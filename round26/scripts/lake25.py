"""Round 25 step b: the lake arrival in a --loops run (the jump at the forest's "loop" line).

python -I lake25.py <run dir> > lake_arrival.txt

- fast.jsonl: engine.stats "render.extra_views" back to back (about one answer a frame) from the forest picture to
  4 s after the arrival: per answer the frame, frame_ms, render.extra_views, render.extra_views_ms (CPU), _gpu_ms,
  render.extra_views_new_ms, _new_compiled and every render.extra_views_new.<part>;
- the log lines around the arrival: "a new extra view's state" (renderer), "extra views:" (the first extra view's CPU
  steps), anything WARN;
- the 5 s frame profile covering the arrival: its head, render_system and the systems / inner spans with a run over 10 ms.
"""
import json, re, sys
from datetime import datetime, timedelta

d = sys.argv[1]
TS = re.compile(r"^(20\d\d-\d\d-\d\dT[\d:.]+)Z")


def iso(s):
    s = s.rstrip("Z")
    if "." in s:
        a, b = s.split(".")
        s = a + "." + b[:6]
    return datetime.fromisoformat(s)


L = open(f"{d}/stderr.log", encoding="utf-8", errors="replace").read().splitlines()
arr = None
for l in L:
    if re.search(r"showcase::loops: loop \d+/\d+ at forest:", l):
        arr = iso(TS.match(l).group(1))
        break
print(f"lake arrival (the forest's loop line): {arr:%H:%M:%S.%f} UTC")

print("\n-- fast.jsonl around the arrival (t = s after the loop line; ms; new.* parts only when they change)")
rows = []
try:
    for line in open(f"{d}/fast.jsonl", encoding="utf-8"):
        t, j = line.split(" ", 1)
        try:
            r = json.loads(j)["result"]
        except (json.JSONDecodeError, KeyError):
            continue
        rows.append((iso(t), r))
except FileNotFoundError:
    print("   (no fast.jsonl)")
last_new, last_frame, prev = None, None, None
print("frame_ms is the engine's smoothed frame time; interval = wall time since the previous answer / frames since it "
      "(one answer a frame, so about each frame's own time)")
print("t s     frame    frame_ms  interval  views  views_ms  views_gpu_ms  new_ms  new_compiled  new parts")
for t, r in rows:
    m = r.get("metrics", {})
    if r.get("frame") == last_frame:
        continue
    last_frame = r.get("frame")
    new = {k[len("render.extra_views_new."):]: v for k, v in m.items() if k.startswith("render.extra_views_new.")}
    newtxt = ""
    key = (m.get("render.extra_views_new_ms"), m.get("render.extra_views_new_compiled"), tuple(sorted(new.items())))
    if key != last_new:
        newtxt = ", ".join(f"{k} {v:.1f}" for k, v in sorted(new.items(), key=lambda x: -x[1]))
        last_new = key
    dt = (t - arr).total_seconds()
    iv = (t - prev[0]).total_seconds() * 1000 / max(1, r.get("frame") - prev[1]) if prev else float("nan")
    prev = (t, r.get("frame"))
    if -1.5 <= dt <= 4.5:
        def f(k, fmt="{:.2f}"):
            v = m.get(k)
            return fmt.format(v) if v is not None else "-"
        print(f"{dt:6.2f} {r.get('frame'):8d} {r.get('frame_ms', 0):9.1f}  {iv:8.1f}  {f('render.extra_views', '{:.0f}'):>5s}  "
              f"{f('render.extra_views_ms'):>8s}  {f('render.extra_views_gpu_ms'):>12s}  {f('render.extra_views_new_ms'):>6s}  "
              f"{f('render.extra_views_new_compiled', '{:.0f}'):>12s}  {newtxt}")
big = sorted(((r.get("frame_ms", 0), (t - arr).total_seconds(), r.get("frame")) for t, r in rows), reverse=True)[:8]
print("slowest answers' frame_ms (ms, s after the loop line, frame): " + "; ".join(f"{a:.1f} ({b:.2f} s, {c})" for a, b, c in big))
if rows:
    m = rows[-1][1].get("metrics", {})
    print("last answer's keys: " + ", ".join(f"{k} {v:.2f}" for k, v in sorted(m.items())))

print("\n-- log lines from 1 s before to 6 s after the arrival (renderer / extra views / WARN / hitch-like)")
for i, l in enumerate(L):
    m = TS.match(l)
    if not m:
        continue
    t = iso(m.group(1))
    if arr - timedelta(seconds=1) <= t <= arr + timedelta(seconds=6):
        if any(s in l for s in ("extra view", "WARN", "renderer:", "pipeline", "showcase::loops")):
            print(f"{(t - arr).total_seconds():6.2f} s  {l[l.index('Z ') + 2:][:600]}")

print("\n-- the 5 s frame profile covering the arrival")
for i, l in enumerate(L):
    if "engine_core::profile: frame profile" not in l:
        continue
    t = iso(TS.match(l).group(1))
    if not (arr < t <= arr + timedelta(seconds=5.5)):
        continue
    b = []
    for j in range(i + 1, len(L)):
        if TS.match(L[j]):
            break
        b.append(L[j])
    print(f"{t:%H:%M:%S} ({(t - arr).total_seconds():.2f} s after the arrival): " + " | ".join(b[:4]))
    sec = None
    for s in b:
        if s.startswith("slowest systems"):
            sec = "s"; continue
        if s.startswith("ran alone"):
            sec = None; continue
        if s.startswith("inside systems"):
            sec = "i"; continue
        if s.startswith("ordering floor"):
            sec = None
        if s.startswith("schedule ") or s.startswith("FixedUpdate ") or s.startswith("Render ") or s.startswith("Update ") or s.startswith("PostUpdate "):
            if sec is None and re.match(r"^\w+\s+[\d.]+\s+[\d.]+\s*$", s):
                print("   schedule " + re.sub(r"\s+", " ", s))
        if sec:
            m = re.match(r"^(?:  in )?(.+?)\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)\s*$", s)
            if m and (float(m.group(3)) > 10 or "render_system" in m.group(1)):
                print(f"   {'in ' if sec == 'i' else ''}{m.group(1)}: {m.group(2)} ms a frame, max {m.group(3)}, {m.group(4)} runs a frame")
    break
