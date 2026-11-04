"""Round 24 step a: the plaza stand, plain against ENGINE_FIXED_SYNC=sets.

python -I ab24.py <run dir A> [<run dir B>] [--steady lo hi]

Per run, over the steady window (lo-hi s after AT plaza, default 25-75):
- every 5 s frame profile (stderr.log) reporting inside it: frames, ms a frame, fps, worst; systems busy, cores;
  one system at a time / no system / several at once; FixedUpdate ms a frame and runs a frame and its ordering
  floor (with the chain); the other schedules' ms a frame;
- the render thread keys (sampled_stats.jsonl, engine.stats "render." twice a second): mean / median / p90 / max;
- fps and fixed steps a frame (frames.csv, engine.status twice a second) over the window and per 5 s;
- the render.cpu.* dumps (stats_dump_plaza_NNNs.json, every 5 s of the stand): one column per dump.
With two runs, an A/B table of the steady means at the end.
"""
import csv, glob, json, os, re, statistics, sys
from datetime import datetime, timedelta

args = sys.argv[1:]
lo, hi = 25.0, 75.0
if "--steady" in args:
    i = args.index("--steady")
    lo, hi = float(args[i + 1]), float(args[i + 2])
    del args[i:i + 3]
TS = re.compile(r"^(20\d\d-\d\d-\d\dT[\d:.]+)Z")
KEYS = ["render.wait_ms", "render.extract_ms", "render.extract_wait_ms", "render.record_ms",
        "render.record_total_ms", "render.record_cpu_ms", "render.cpu_ms"]


def iso(s):
    s = s.rstrip("Z")
    if "." in s:
        a, b = s.split(".")
        s = a + "." + b[:6]
    return datetime.fromisoformat(s)


def summ(vals):
    vals = sorted(v for v in vals if v is not None)
    if not vals:
        return None
    return {"mean": statistics.mean(vals), "median": statistics.median(vals),
            "p90": vals[min(len(vals) - 1, int(len(vals) * 0.9))], "min": vals[0], "max": vals[-1], "n": len(vals)}


def fmt(s, f="{:.2f}"):
    if not s:
        return "n/a"
    return f"mean {f.format(s['mean'])}, median {f.format(s['median'])}, p90 {f.format(s['p90'])}, " \
           f"min {f.format(s['min'])}, max {f.format(s['max'])} (n {s['n']})"


def run(d):
    L = open(f"{d}/stderr.log", encoding="utf-8", errors="replace").read().splitlines()
    at = None
    for line in open(f"{d}/timeline.txt", encoding="utf-8"):
        m = re.match(r"^(\d\d:\d\d:\d\d\.\d+)Z\s+\S+\s+AT plaza:", line)
        if m:
            at = m.group(1)
    started = next((l for l in open(f"{d}/timeline.txt", encoding="utf-8") if "started:" in l), "").strip()
    day = TS.match(L[0]).group(1)[:10]
    at = iso(f"{day}T{at}")
    w0, w1 = at + timedelta(seconds=lo), at + timedelta(seconds=hi)
    out = {"dir": os.path.basename(d.rstrip("/\\")), "started": started, "at": at}
    print(f"===== {out['dir']}: {started[started.find('started:'):]}")
    print(f"AT plaza {at:%H:%M:%S.%f} UTC; steady window {lo:g}-{hi:g} s after it ({w0:%H:%M:%S}-{w1:%H:%M:%S} UTC)")

    # Frame profiles reporting inside the window: each covers the 5 s before it.
    profs = []
    for i, l in enumerate(L):
        if "engine_core::profile: frame profile" not in l:
            continue
        t = iso(TS.match(l).group(1))
        b = []
        for j in range(i + 1, len(L)):
            if TS.match(L[j]):
                break
            b.append(L[j])
        p = {"t": t, "block": b}
        m = re.match(r"(\d+) frames in ([\d.]+) s: ([\d.]+) ms per frame \(([\d.]+) fps\), worst ([\d.]+) ms", b[0])
        if m:
            p.update(frames=int(m.group(1)), secs=float(m.group(2)), frame_ms=float(m.group(3)),
                     fps=float(m.group(4)), worst=float(m.group(5)))
        sec = None
        p["sched"] = {}
        p["floor"] = {}
        p["chain"] = {}
        p["sys"] = {}
        p["inner"] = {}
        last_floor = None
        for s in b:
            m = re.match(r"systems busy ([\d.]+) ms per frame: ([\d.]+) of (\d+) cores", s)
            if m:
                p["busy"], p["cores"], p["threads"] = float(m.group(1)), float(m.group(2)), int(m.group(3))
            m = re.match(r"one system at a time ([\d.]+) ms per frame, no system ([\d.]+) ms, several at once ([\d.]+) ms", s)
            if m:
                p["alone"], p["idle"], p["several"] = map(float, m.groups())
            if s.startswith("systems ran on:"):
                p["ran_on"] = s[len("systems ran on: "):]
            if s.startswith("schedule ") and "ms/frame" in s:
                sec = "sched"; continue
            if s.startswith("slowest systems"):
                sec = "sys"; continue
            if s.startswith("ran alone"):
                sec = "alone"; continue
            if s.startswith("inside systems"):
                sec = "inner"; continue
            if s.startswith("ordering floor"):
                sec = "floor"; continue
            if sec == "sched":
                m = re.match(r"^(\w+)\s+([\d.]+)\s+([\d.]+)\s*$", s)
                if m:
                    p["sched"][m.group(1)] = (float(m.group(2)), float(m.group(3)))
            elif sec == "sys":
                m = re.match(r"^(\S+)\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)\s*$", s)
                if m:
                    p["sys"][m.group(1)] = tuple(map(float, m.groups()[1:]))
            elif sec == "inner":
                m = re.match(r"^  in (.+?)\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)\s*$", s)
                if m:
                    p["inner"][m.group(1)] = tuple(map(float, m.groups()[1:]))
            elif sec == "floor":
                m = re.match(r"^(\w+)\s+([\d.]+) of\s+([\d.]+) \((\d+) systems", s)
                if m:
                    last_floor = m.group(1)
                    p["floor"][last_floor] = (float(m.group(2)), float(m.group(3)), int(m.group(4)))
                m = re.match(r"^\s+chain: (.+)$", s)
                if m and last_floor:
                    p["chain"][last_floor] = m.group(1)
        profs.append(p)
    pw = [p for p in profs if w0 + timedelta(seconds=4.5) < p["t"] <= w1 + timedelta(seconds=0.5)]
    out["profs"] = pw
    print(f"\n{len(pw)} frame profiles report inside the window (each covers the 5 s before it):")
    print("time UTC | frames | ms/frame | fps | worst | systems busy ms | cores | one at a time | no system | "
          "several at once | FixedUpdate ms/frame (runs) | FixedUpdate floor | Render | Update | PostUpdate")
    for p in pw:
        fu = p["sched"].get("FixedUpdate", (None, None))
        fl = p["floor"].get("FixedUpdate", (None, None, None))
        print(f"{p['t']:%H:%M:%S} | {p.get('frames')} | {p.get('frame_ms')} | {p.get('fps')} | {p.get('worst')} | "
              f"{p.get('busy')} | {p.get('cores')} | {p.get('alone')} | {p.get('idle')} | {p.get('several')} | "
              f"{fu[0]} (x{fu[1]}) | {fl[0]} of {fl[1]} ({fl[2]} systems) | {p['sched'].get('Render', ('',))[0]} | "
              f"{p['sched'].get('Update', ('',))[0]} | {p['sched'].get('PostUpdate', ('',))[0]}")
    print("\nEach profile's ordering floors (schedule: floor of schedule ms) and the FixedUpdate chain:")
    for p in pw:
        print(f"-- {p['t']:%H:%M:%S}: " + ", ".join(f"{k} {v[0]:.2f} of {v[1]:.2f}" for k, v in p["floor"].items()))
        print(f"   systems ran on: {p.get('ran_on', '')}")
        print(f"   FixedUpdate chain: {p['chain'].get('FixedUpdate', '')}")
    print("\nHitches in the window's profiles (worst frame over 60 ms): the systems and inner spans with a run over 30 ms:")
    for p in pw:
        if (p.get("worst") or 0) > 60:
            big = [f"{k} {v[1]:.1f}" for k, v in p["sys"].items() if v[1] > 30] + \
                  [f"in {k} {v[1]:.1f}" for k, v in p["inner"].items() if v[1] > 30]
            print(f"  {p['t']:%H:%M:%S} worst {p['worst']} ms: " + (", ".join(big) or "(none listed over 30 ms)"))

    # The render thread keys.
    rows = []
    for line in open(f"{d}/sampled_stats.jsonl", encoding="utf-8"):
        t, j = line.split(" ", 1)
        try:
            r = json.loads(j)["result"]
        except (json.JSONDecodeError, KeyError):
            continue
        rows.append((iso(t), r))
    rend = [(t, r) for t, r in rows if "render.wait_ms" in r["metrics"] and w0 <= t <= w1]
    out["render"] = {k: summ([r["metrics"].get(k) for t, r in rend]) for k in KEYS}
    out["render"]["frame_ms"] = summ([r["frame_ms"] for t, r in rend])
    print(f"\nRender thread keys, {len(rend)} engine.stats \"render.\" samples in the window:")
    for k in ["frame_ms"] + KEYS:
        print(f"  {k:24s} {fmt(out['render'][k])}")
    rt = summ([r["metrics"]["render.record_total_ms"] / r["frame_ms"] for t, r in rend
               if r["metrics"].get("render.record_total_ms") is not None and r["frame_ms"] > 0])
    print(f"  record_total_ms / frame_ms {fmt(rt)}")

    # fps and fixed steps a frame from engine.status.
    st = []
    for r in csv.DictReader(open(f"{d}/frames.csv")):
        if r["fixed_tick"]:
            st.append((iso(r["wall_utc"]), int(r["frame"]), float(r["elapsed_real"]), int(r["fixed_tick"])))
    ss = [s for s in st if w0 <= s[0] <= w1]
    if len(ss) >= 2:
        a, b = ss[0], ss[-1]
        out["fps"] = (b[1] - a[1]) / (b[2] - a[2])
        out["steps"] = (b[3] - a[3]) / (b[1] - a[1])
        print(f"\nengine.status over the window: {out['fps']:.2f} fps, {out['steps']:.3f} fixed steps a frame "
              f"({b[1] - a[1]} frames, {b[3] - a[3]} fixed steps, {b[2] - a[2]:.1f} s)")
        print("per 5 s after AT plaza (fps, fixed steps a frame):")
        k0 = 0.0
        while k0 < 75:
            g = [s for s in st if k0 <= (s[0] - at).total_seconds() <= k0 + 5]
            if len(g) >= 2:
                print(f"  {k0:3.0f}-{k0 + 5:3.0f} s  {(g[-1][1] - g[0][1]) / max(1e-3, g[-1][2] - g[0][2]):5.1f} fps  "
                      f"{(g[-1][3] - g[0][3]) / max(1, g[-1][1] - g[0][1]):.2f} steps/frame")
            k0 += 5

    # The render.cpu.* dumps.
    dumps = sorted(glob.glob(f"{d}/stats_dump_plaza_*s.json"))
    cols, allk = [], {}
    for f in dumps:
        s = re.search(r"_(\d+)s\.json$", f).group(1)
        try:
            r = json.loads(open(f, encoding="utf-8-sig").read())["result"]
        except (json.JSONDecodeError, KeyError):
            continue
        cols.append((int(s), r))
        for k, v in r["metrics"].items():
            allk.setdefault(k, []).append(v)
    out["dumps"] = cols
    if cols:
        order = sorted(allk, key=lambda k: -statistics.mean(allk[k]))
        print(f"\nrender.cpu.* every 5 s of the stand ({len(cols)} dumps; s after AT plaza; frame; ms, sorted by mean; "
              f"each dump is one frame's latest step times):")
        print(f"{'key':44s} " + " ".join(f"{s:>6d}" for s, _ in cols) + "   mean")
        print(f"{'(frame)':44s} " + " ".join(f"{r['frame'] % 1000000:>6d}" for _, r in cols))
        for k in order:
            print(f"{k[len('render.cpu.'):][:44]:44s} " + " ".join(
                f"{r['metrics'].get(k, float('nan')):6.2f}" for _, r in cols) + f" {statistics.mean(allk[k]):6.2f}")
    print()
    return out


runs = [run(d) for d in args]


def pm(vals):
    vals = [v for v in vals if v is not None]
    if not vals:
        return "n/a"
    return f"{statistics.mean(vals):.2f} ({min(vals):.2f}-{max(vals):.2f})"


if len(runs) == 2:
    print(f"===== A/B, {lo:g}-{hi:g} s after AT plaza: means over the profiles reporting in it (range in brackets); "
          f"{len(runs[0]['profs'])} / {len(runs[1]['profs'])} profiles")
    rowsab = [
        ("frame ms (profiles)", lambda r: pm([p.get("frame_ms") for p in r["profs"]])),
        ("fps (engine.status)", lambda r: f"{r.get('fps', float('nan')):.2f}"),
        ("fixed steps a frame", lambda r: f"{r.get('steps', float('nan')):.3f}"),
        ("FixedUpdate ms/frame", lambda r: pm([p["sched"].get("FixedUpdate", (None,))[0] for p in r["profs"]])),
        ("FixedUpdate runs/frame", lambda r: pm([p["sched"].get("FixedUpdate", (None, None))[1] for p in r["profs"]])),
        ("FixedUpdate ms a run", lambda r: pm([p["sched"]["FixedUpdate"][0] / p["sched"]["FixedUpdate"][1]
                                                for p in r["profs"] if p["sched"].get("FixedUpdate", (0, 0))[1]])),
        ("FixedUpdate ordering floor", lambda r: pm([p["floor"].get("FixedUpdate", (None,))[0] for p in r["profs"]])),
        ("FixedUpdate floor a run", lambda r: pm([p["floor"]["FixedUpdate"][0] / p["sched"]["FixedUpdate"][1]
                                                  for p in r["profs"] if "FixedUpdate" in p["floor"]
                                                  and p["sched"].get("FixedUpdate", (0, 0))[1]])),
        ("FixedUpdate systems (floor line)", lambda r: pm([p["floor"].get("FixedUpdate", (None, None, None))[2] for p in r["profs"]])),
        ("Render ms/frame", lambda r: pm([p["sched"].get("Render", (None,))[0] for p in r["profs"]])),
        ("Update ms/frame", lambda r: pm([p["sched"].get("Update", (None,))[0] for p in r["profs"]])),
        ("PostUpdate ms/frame", lambda r: pm([p["sched"].get("PostUpdate", (None,))[0] for p in r["profs"]])),
        ("systems busy ms/frame", lambda r: pm([p.get("busy") for p in r["profs"]])),
        ("cores busy", lambda r: pm([p.get("cores") for p in r["profs"]])),
        ("one system at a time ms", lambda r: pm([p.get("alone") for p in r["profs"]])),
        ("no system ms", lambda r: pm([p.get("idle") for p in r["profs"]])),
        ("several at once ms", lambda r: pm([p.get("several") for p in r["profs"]])),
        ("worst frame ms", lambda r: pm([p.get("worst") for p in r["profs"]])),
    ] + [(k.replace("render.", "render.") + " (samples)", (lambda k: lambda r: (lambda s: f"{s['mean']:.2f} (p90 {s['p90']:.2f}, max {s['max']:.2f})" if s else "n/a")(r["render"][k]))(k))
         for k in ["render.wait_ms", "render.extract_ms", "render.record_total_ms", "render.record_cpu_ms", "render.cpu_ms"]]
    w = max(len(n) for n, _ in rowsab)
    print(f"{'':{w}s} | {runs[0]['dir']:28s} | {runs[1]['dir']}")
    for n, f in rowsab:
        print(f"{n:{w}s} | {f(runs[0]):28s} | {f(runs[1])}")
    # render.cpu.* means of the dumps, both runs.
    ka = {}
    for i, r in enumerate(runs):
        for s, x in r["dumps"]:
            if not lo <= s <= hi:
                continue
            for k, v in x["metrics"].items():
                ka.setdefault(k, ([], []))[i].append(v)
    if ka:
        print(f"\nrender.cpu.* mean of the 5 s dumps at {lo:g}-{hi:g} s (ms), A / B (steps at 0.05 ms or more in either; every step in"
              " each run's analysis.txt):")
        for k in sorted(ka, key=lambda k: -(statistics.mean(ka[k][0]) if ka[k][0] else 0)):
            a, b = ka[k]
            if max(statistics.mean(a) if a else 0, statistics.mean(b) if b else 0) < 0.05:
                continue
            print(f"  {k[len('render.cpu.'):][:50]:50s} {statistics.mean(a) if a else float('nan'):6.2f} / "
                  f"{statistics.mean(b) if b else float('nan'):6.2f}")
