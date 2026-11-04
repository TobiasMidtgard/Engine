"""Round 24 step b for a --loops run: per stop window (its 25 s dwell, ending at its "loop" line), each 5 s frame
profile's rows for the NPC's think_and_act and its inner "find path" / "nav chunk" spans, water's ripple_system
and flow_bed_system; then the worst single runs over the whole run, and the nav chunk counts.

python -I loops24.py <run dir> > npc_water.txt

Rows as printed by the profile: ms a frame / max ms of a run / runs a frame; "count" = runs a frame x the
profile's frames. A system or span missing from a profile is not among its listed rows (40 slowest systems,
40 heaviest inner spans), so it was small there.
"""
import re, sys
from datetime import timedelta, datetime

d = sys.argv[1]
L = open(f"{d}/stderr.log", encoding="utf-8", errors="replace").read().splitlines()
TS = re.compile(r"^(20\d\d-\d\d-\d\dT[\d:.]+)Z")
SYS = ["engine_npc::think_and_act", "engine_water::ripple_system", "engine_water::flow_bed_system",
       "engine_softbody::step_beside_physics"]
INNER = ["engine_npc: find path", "engine_npc: nav chunk", "engine_water: ripples: step",
         "engine_water: ripples: rebuild"]


def ts(s):
    m = TS.match(s)
    if not m:
        return None
    t = m.group(1)
    if "." in t:
        a, b = t.split(".")
        t = a + "." + b[:6]
    return datetime.fromisoformat(t)


def block(i):
    b = []
    for j in range(i + 1, len(L)):
        if TS.match(L[j]):
            break
        b.append(L[j])
    return b


loops = []
for line in L:
    m = re.search(r"showcase::loops: loop (\d+)/\d+ at (\w+):", line)
    if m:
        loops.append((ts(line), int(m.group(1)), m.group(2)))
profs = []
for i, l in enumerate(L):
    if "engine_core::profile: frame profile" not in l:
        continue
    b = block(i)
    m = re.match(r"(\d+) frames in ([\d.]+) s: ([\d.]+) ms per frame \(([\d.]+) fps\), worst ([\d.]+) ms", b[0] if b else "")
    rows = {}
    for s in b:
        m2 = re.match(r"^(?:  in )?(.+?)\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)\s*$", s)
        if m2 and (m2.group(1) in SYS or m2.group(1) in INNER):
            rows[m2.group(1)] = tuple(map(float, m2.groups()[1:]))
    profs.append({"t": ts(l), "frames": int(m.group(1)) if m else 0, "worst": float(m.group(5)) if m else None,
                  "ms": float(m.group(3)) if m else None, "rows": rows})


def stop_of(t):
    # The stop whose 25 s dwell holds the profile's end (the window it covers is the 5 s before t).
    for end, rnd, key in loops:
        if end - timedelta(seconds=25) < t <= end + timedelta(seconds=0.5):
            return f"r{rnd} {key}"
    return "start" if loops and t < loops[0][0] else "after"


def row(p, name):
    r = p["rows"].get(name)
    if not r:
        return "-"
    return f"{r[0]:.2f} / max {r[1]:.2f} / {r[2]:.1f} runs (count {r[2] * p['frames']:.0f})"


for end, rnd, key in loops:
    start = end - timedelta(seconds=25)
    print(f"== round {rnd} {key} ({start:%H:%M:%S}-{end:%H:%M:%S} UTC)")
    for p in profs:
        if start < p["t"] <= end + timedelta(seconds=0.5):
            print(f"  profile {p['t']:%H:%M:%S} ({p['frames']} frames, {p['ms']} ms a frame, worst {p['worst']} ms)")
            for s in SYS:
                print(f"    {s.split('::')[1]:22s} {row(p, s)}")
            for s in INNER:
                print(f"    in {s.split(': ', 1)[1]:19s} {row(p, s)}")

print("\n== worst single runs over every profile of the run (max ms of a run; profile time; stop; that profile's worst frame)")
for name in SYS[:2] + INNER[:3]:
    xs = sorted(((p["rows"][name][1], p) for p in profs if name in p["rows"]), key=lambda x: -x[0])
    print(f"{name}: " + ("; ".join(f"{v:.1f} ({p['t']:%H:%M:%S}, {stop_of(p['t'])}, worst frame {p['worst']})"
                                   for v, p in xs[:10]) or "in no profile's list"))
nc = [(p, p["rows"]["engine_npc: nav chunk"]) for p in profs if "engine_npc: nav chunk" in p["rows"]]
if nc:
    tot = sum(r[2] * p["frames"] for p, r in nc)
    print(f"\nnav chunk: listed in {len(nc)} of {len(profs)} profiles, about {tot:.0f} chunks built over them; "
          f"max single chunk {max(r[1] for p, r in nc):.2f} ms")
    print("per profile it is listed in (time, stop, count, max ms, ms a frame):")
    for p, r in nc:
        print(f"  {p['t']:%H:%M:%S} {stop_of(p['t']):12s} count {r[2] * p['frames']:5.0f}  max {r[1]:6.2f}  {r[0]:.2f} ms a frame")
else:
    print("\nnav chunk: in no profile's inner list")
fp = [(p, p["rows"]["engine_npc: find path"]) for p in profs if "engine_npc: find path" in p["rows"]]
print(f"find path: listed in {len(fp)} of {len(profs)} profiles" +
      (f", about {sum(r[2] * p['frames'] for p, r in fp):.0f} runs over them" if fp else ""))
