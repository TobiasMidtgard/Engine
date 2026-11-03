"""Step 2b: per-pass GPU ms at render scale 1.0 (s2_loops) and 0.7 (s2_loops_scale07), round 2, from passes21d's rows."""
import re, sys
def rows(path):
    out = {}
    for line in open(path, encoding="utf-8-sig"):
        m = re.match(r"^round (\d) (\w+)\s+\S+ GPU total ([\d.]+)\s+render CPU ([\d.]+)\s+\|\s+(.*)$", line.rstrip())
        if not m:
            continue
        vals = dict((k.strip(), float(v)) for k, v in re.findall(r"([a-z][a-z :-]*?) ([\d.]+)(?=\s{2}|$)", m.group(5)))
        vals["GPU total"] = float(m.group(3))
        out[(int(m.group(1)), m.group(2))] = vals
    return out
a, b = rows(sys.argv[1]), rows(sys.argv[2])
rnd = int(sys.argv[3]) if len(sys.argv) > 3 else 2
stops = [k for (r, k) in a if r == rnd]
passes = ["GPU total", "scene", "scene: terrain", "scene: meshes", "prepass", "shadows", "compute weather clouds", "transparent",
          "probe gi", "world reflections", "bounce light", "volumetric fog", "aerial perspective", "ambient occlusion", "reflections",
          "sky occlusion", "environment", "anti-aliasing", "bloom"]
print(f"round {rnd}, ms at scale 1.0 -> 0.7 (ratio); a per-pixel pass would go to ~0.49")
print("pass | " + " | ".join(stops) + " | all stops summed")
for p in passes:
    cells = []; s1 = s2 = 0
    for k in stops:
        x, y = a[(rnd, k)].get(p, 0), b[(rnd, k)].get(p, 0)
        s1 += x; s2 += y
        cells.append(f"{x:.2f}->{y:.2f}" + (f" ({y / x:.2f})" if x >= 0.05 else ""))
    print(f"{p} | " + " | ".join(cells) + f" | {s1:.2f}->{s2:.2f}" + (f" ({s2 / s1:.2f})" if s1 > 0 else ""))
