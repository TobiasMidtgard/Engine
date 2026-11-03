"""Round 23 step 3 extras for a --loops run: per stop window (its 25 s dwell, ending at its "loop" line),
each 5 s frame profile's rows for chosen systems, and the physics reports' summaries and awake lists.

python -I loops23.py <run dir> > systems_physics.txt

Systems: water::flow_bed_system, water::ripple_system (asked at the lake), foliage::stream_far_woods
(asked at field and plaza), physics_simulate; each as printed: ms a frame, max ms of a run, runs a frame.
A system missing from a profile is not among its slowest listed systems.
"""
import re, sys
from collections import Counter
from datetime import datetime, timedelta

d = sys.argv[1]
L = open(f"{d}/stderr.log", encoding="utf-8", errors="replace").read().splitlines()
TS = re.compile(r"^(20\d\d-\d\d-\d\dT[\d:.]+)Z")
SYS = ["engine_water::flow_bed_system", "engine_water::ripple_system", "engine_foliage::stream_far_woods",
       "engine_physics::physics_simulate"]


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
profs = [(ts(l), block(i)) for i, l in enumerate(L) if "engine_core::profile: frame profile" in l]
reports = [(ts(l), l[l.index("physics report"):], block(i)) for i, l in enumerate(L) if "engine_physics::report" in l]


def row(b, name):
    for s in b:
        m = re.match(rf"^{re.escape(name)}\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)\s*$", s)
        if m:
            return f"{m.group(1)} / max {m.group(2)} / {m.group(3)} runs"
    return "-"


allmax = Counter()
for end, rnd, key in loops:
    start = end - timedelta(seconds=25)
    print(f"== round {rnd} {key} ({start:%H:%M:%S}-{end:%H:%M:%S} UTC)")
    for t, b in profs:
        if start < t <= end + timedelta(seconds=0.5):
            head = re.sub(r"\s{2,}", " ", b[0]).strip() if b else ""
            print(f"  profile {t:%H:%M:%S} ({head[:90]})")
            for s in SYS:
                print(f"    {s.split('::')[1]:22s} {row(b, s)}")
    for t, head, b in reports:
        if start < t <= end:
            m = re.match(r"physics report: ([\d.]+) ms per step over (\d+) steps, (\d+) regions", head)
            regions = []
            for s in b:
                r = re.match(r"\s+region (\d+): ([\d.]+) ms per step.*?(\d+) bodies \((\d+) awake", s)
                if r:
                    regions.append([f"region {r.group(1)} {r.group(2)} ms, {r.group(4)} of {r.group(3)} bodies awake", ""])
                elif "awake:" in s and regions:
                    # The region's own detail line: "...; awake: Name (speed, at x y z) Name (...)".
                    names = re.findall(r"(?:^|\) )([^()]+?) \([\d.]+ m/s", s[s.index("awake:") + 6:].strip())
                    regions[-1][1] = ", ".join(n.strip() for n in names)
            print(f"  physics report {t:%H:%M:%S}: " + (f"{m.group(1)} ms a step over {m.group(2)} steps, {m.group(3)} regions"
                                                         if m else head[:120]))
            for x, a in regions:
                print(f"    {x}" + (f"; awake: {a[:300]}" if a else ""))
for t, b in profs:
    for s in b:
        m = re.match(r"^engine_foliage::stream_far_woods\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)\s*$", s)
        if m:
            allmax[f"{t:%H:%M:%S}"] = float(m.group(2))
if allmax:
    print("\nstream_far_woods: the 10 highest max-ms-of-a-run over every profile of the run: "
          + ", ".join(f"{k} {v:.2f}" for k, v in allmax.most_common(10)))
else:
    print("\nstream_far_woods: in no profile's listed systems")
