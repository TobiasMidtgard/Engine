"""Round 29b step 4: per stand of a stops run (AT <key> to END <key> in timeline.txt), each 5 s render profile's GPU
passes block ("GPU passes N ms/frame", 5 s averages): the total and every row whose name has "grass" or "shadow" in it,
then the mean of those rows over the profiles reporting in the stand's window; and fps over the window (frames.csv).
With --loops <dir>, the same for a --loops run's market and field stops (their last 20 s before the loop line).

python -I grass29b.py <run dir> [--from S] [--loops <loops run dir>]
  --from S: the window starts S s after the stand's AT line (default 5; profiles reporting more than 4.5 s after it)
"""
import csv, re, statistics, sys
from datetime import datetime, timedelta

args = sys.argv[1:]
frm = 5.0
if "--from" in args:
    i = args.index("--from"); frm = float(args[i + 1]); del args[i:i + 2]
loops_dir = None
if "--loops" in args:
    i = args.index("--loops"); loops_dir = args[i + 1]; del args[i:i + 2]
TS = re.compile(r"^(20\d\d-\d\d-\d\dT[\d:.]+)Z")


def t(s):
    return datetime.fromisoformat(s[:26])


def blocks(d):
    """(time, total, {pass: ms}) for every GPU passes block in stderr.log."""
    L = open(f"{d}/stderr.log", encoding="utf-8", errors="replace").read().splitlines()
    out, last, cur = [], None, None
    for l in L:
        m = TS.match(l)
        if m:
            last = t(m.group(1)); cur = None
            continue
        g = re.match(r"^GPU passes\s+([\d.]+)", l)
        if g:
            cur = (last, float(g.group(1)), {}); out.append(cur)
            continue
        if cur is not None:
            r = re.match(r"^\s{2}(\S.*?)\s{2,}([\d.]+)\s*$", l)
            if r:
                cur[2][r.group(1)] = float(r.group(2))
            elif l.strip() and not l.startswith("  "):
                cur = None
    return out


def fps(d, a, b):
    st = [(t(r["wall_utc"]), int(r["frame"]), float(r["elapsed_real"])) for r in csv.DictReader(open(f"{d}/frames.csv"))]
    g = [s for s in st if a <= s[0] <= b]
    return (g[-1][1] - g[0][1]) / (g[-1][2] - g[0][2]) if len(g) >= 2 else float("nan")


def report(d, windows):
    bs = blocks(d)
    for key, a, b in windows:
        pw = [x for x in bs if a + timedelta(seconds=4.5) < x[0] <= b + timedelta(seconds=0.5)]
        rows = sorted({k for x in pw for k in x[2] if "grass" in k or "shadow" in k})
        print(f"== {key}: {a:%H:%M:%S}-{b:%H:%M:%S} UTC, {fps(d, a, b):.1f} fps (engine.status), {len(pw)} render profiles")
        print(f"   {'profile UTC':12s} {'GPU total':>9s}  " + "  ".join(f"{r:>22s}" for r in rows))
        for x in pw:
            print(f"   {x[0]:%H:%M:%S}     {x[1]:9.2f}  " + "  ".join(f"{x[2].get(r, float('nan')):22.2f}" for r in rows))
        if pw:
            print(f"   {'mean':12s} {statistics.mean(x[1] for x in pw):9.2f}  "
                  + "  ".join(f"{statistics.mean(x[2].get(r, 0) for x in pw):22.2f}" for r in rows))
            g = [statistics.mean(sum(v for k, v in x[2].items() if 'grass' in k) for x in pw)]
            print(f"   every grass row summed (mean): {g[0]:.2f} ms")


L = open(f"{args[0]}/timeline.txt", encoding="utf-8-sig").read().splitlines()
day = None
for l in open(f"{args[0]}/stderr.log", encoding="utf-8", errors="replace"):
    m = TS.match(l)
    if m:
        day = m.group(1)[:10]; break
ats = []
for l in L:
    m = re.match(r"^(\d\d:\d\d:\d\d\.\d+)Z\s+\S+\s+(AT|END) (\w+)", l)
    if m:
        ats.append((m.group(2), m.group(3), datetime.fromisoformat(f"{day}T{m.group(1)[:12]}")))
win = []
for i, (kind, key, at) in enumerate(ats):
    if kind == "AT":
        end = next((x[2] for x in ats[i + 1:] if x[0] == "END" and x[1] == key), None)
        if end:
            win.append((key, at + timedelta(seconds=frm), end))
print(f"# {args[0]}: stands from {frm:g} s after arriving to END")
report(args[0], win)
if loops_dir:
    LL = open(f"{loops_dir}/stderr.log", encoding="utf-8", errors="replace").read().splitlines()
    lines = [(t(m.group(1)), m.group(2)) for l in LL for m in [re.match(r"^(\S+)Z .*showcase::loops: loop \d+/\d+ at (\w+):", l)] if m]
    w = [(f"{k} (loops)", tt - timedelta(seconds=20), tt) for tt, k in lines if k in ("market", "field")]
    print(f"\n# {loops_dir}: --loops stops, the last 20 s before each loop line")
    report(loops_dir, w)
