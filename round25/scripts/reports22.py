"""Every engine_physics::report block of a --loops run with the stop it falls in -> physics_reports.txt."""
import re, sys
d = sys.argv[1]
L = open(f"{d}/stderr.log", encoding="utf-8", errors="replace").read().splitlines()
TS = re.compile(r"^(20\d\d-\d\d-\d\dT[\d:.]+)Z")
loops = [(m.group(1), m.group(2), m.group(3)) for l in L for m in [re.match(r"^(\S+)Z .*showcase::loops: loop (\d+)/\d+ at (\w+):", l)] if m]
def stop_at(t):
    for lt, r, k in loops:
        if lt >= t:
            return f"round {r} {k}"
    return "after the loops"
out = []
for i, l in enumerate(L):
    if "engine_physics::report" in l:
        t = TS.match(l).group(1)
        out.append(f"== {t} ({stop_at(t)}) " + l[l.index("physics report"):])
        j = i + 1
        while j < len(L) and not TS.match(L[j]):
            out.append("   " + L[j]); j += 1
open(f"{d}/physics_reports.txt", "w", encoding="utf-8").write("\n".join(out) + "\n")
