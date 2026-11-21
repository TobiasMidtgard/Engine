#!/usr/bin/env python3
"""The economy rig: plays the scripted player over several seeds with no picture and checks
the town economy's targets (engine-design/archipelago-economy, "Targets").

    python3 games/archipelago/tools/economy_rig.py --seeds 10 --minutes 360 --out rig/today
    python3 games/archipelago/tools/economy_rig.py --check rig/today     # only re-check

Writes <out>/seed_N/{timeline.json,events.log}, <out>/targets.json and <out>/report.md.
Standard library only, so it runs anywhere the game builds.
"""
import argparse
import json
import os
import statistics
import subprocess
import sys
import time

BIN_DEFAULT = os.path.expanduser(
    os.environ.get("CARGO_TARGET_DIR", "target") + "/release/archipelago"
)


def play(binary, seed, minutes, speed, out, extra=()):
    os.makedirs(out, exist_ok=True)
    cmd = [binary, "--headless", "--size", "320x180", "--play", str(minutes),
           "--play-speed", str(speed), "--seed", str(seed), "--play-out", out, *extra]
    with open(os.path.join(out, "run.log"), "w") as log:
        return subprocess.Popen(cmd, stdout=log, stderr=subprocess.STDOUT)


def minutes_of(row):
    if "minute" in row:
        return row["minute"]
    h, m, s = (int(x) for x in row["clock"].split(":"))
    return h * 60 + m + s / 60


def first(rows, pred):
    for r in rows:
        if pred(r):
            return minutes_of(r)
    return None


def judge(rows):
    """One seed's numbers for each target that a timeline can show."""
    if not rows:
        return {}
    t0 = minutes_of(rows[0])
    coins = [r["coins"] for r in rows]
    # Balance positive and staying so for ten minutes.
    turned = None
    for i, r in enumerate(rows):
        if r["balance_per_min"] > 0 and all(x["balance_per_min"] > 0 for x in rows[i:i + 10]):
            turned = minutes_of(r) - t0
            break
    last = rows[-1]
    # A game with rivals: the player's company only (the timeline's totals count everyone).
    rivals = len(last.get("companies", [])) > 1
    if rivals:
        lab = first(rows, lambda r: r["companies"][0]["tiers"][1] > 0)
        bur = first(rows, lambda r: r["companies"][0]["tiers"][2] > 0)
        # (Timelines from before the fourth and fifth tiers have three entries.)
        mas = first(rows, lambda r: len(r["companies"][0]["tiers"]) > 3 and r["companies"][0]["tiers"][3] > 0)
        pat = first(rows, lambda r: len(r["companies"][0]["tiers"]) > 4 and r["companies"][0]["tiers"][4] > 0)
        # The first time any rival company has Masters or Patricians.
        rmas = first(rows, lambda r: any(len(c["tiers"]) > 3 and c["tiers"][3] > 0 for c in r["companies"][1:]))
        rpat = first(rows, lambda r: any(len(c["tiers"]) > 4 and c["tiers"][4] > 0 for c in r["companies"][1:]))
        people = last["companies"][0]["people"]
    else:
        lab = first(rows, lambda r: r["houses"][1] > 0)
        bur = first(rows, lambda r: r["houses"][2] > 0)
        mas = first(rows, lambda r: len(r["houses"]) > 3 and r["houses"][3] > 0)
        pat = first(rows, lambda r: len(r["houses"]) > 4 and r["houses"][4] > 0)
        rmas = rpat = None
        people = sum(sum(i["people"]) for i in last["islands"])
    houses = sum(last["houses"])
    happy = None
    if "happy_houses" in last:
        happy = last["happy_houses"] / max(houses, 1)
    elif houses:
        happy = last["content_houses"] / houses
    return {
        "minutes": minutes_of(last) - t0,
        "lowest_coins": min(coins),
        "balance_positive_at": turned,
        "first_labourers_at": None if lab is None else lab - t0,
        "first_burghers_at": None if bur is None else bur - t0,
        "first_masters_at": None if mas is None else mas - t0,
        "first_patricians_at": None if pat is None else pat - t0,
        "rival_masters_at": None if rmas is None else rmas - t0,
        "rival_patricians_at": None if rpat is None else rpat - t0,
        "people_at_end": round(people),
        "islands_at_end": last["companies"][0]["islands"] if rivals else last.get("settled", len(last["islands"])),
        # Where the player's company finished by people (1 = first), in a game with rivals.
        "rank_at_end": 1 + sum(c["people"] > people for c in last["companies"][1:]) if rivals else None,
        "happy_share_at_end": happy,
        "waiting_without_reason": last.get("waiting_without_reason"),
    }


TARGETS = [
    ("1 Never broke", "lowest_coins", lambda v: v is not None and v > 0, "above 0"),
    ("2 Money recovers", "balance_positive_at", lambda v: v is not None and v <= 25, "by 0:25"),
    ("3 Steady climb", "first_labourers_at", lambda v: v is not None and 8 <= v <= 15, "0:08 to 0:15"),
    ("4 Burghers", "first_burghers_at", lambda v: v is not None and 60 <= v <= 90, "1:00 to 1:30"),
    ("4b Masters", "first_masters_at", lambda v: v is not None, "reached"),
    ("4c Patricians", "first_patricians_at", lambda v: v is not None, "reached"),
    ("4d Rivals reach Masters", "rival_masters_at", lambda v: v is not None, "reached"),
    ("4e Rivals reach Patricians", "rival_patricians_at", lambda v: v is not None, "reached"),
    ("5a People", "people_at_end", lambda v: v is not None and v >= 4000, "4,000 or more"),
    ("5b Islands", "islands_at_end", lambda v: v is not None and v >= 4, "4 or more"),
    ("6 Happy towns", "happy_share_at_end", lambda v: v is not None and v >= 0.7, "70 % or more"),
    ("7 No upgrade lock", "waiting_without_reason", lambda v: v == 0, "none"),
    ("9 Place among the houses", "rank_at_end", lambda v: v is None or v == 1, "first (a third of games against Admirals)"),
]


def fmt(key, v):
    if v is None:
        return "never" if key.endswith("_at") else "-"
    if key.endswith("_at"):
        return f"{int(v) // 60}:{int(v) % 60:02d}"
    if key == "happy_share_at_end":
        return f"{v * 100:.0f} %"
    return f"{v:,.0f}" if isinstance(v, (int, float)) else str(v)


def check(out):
    seeds = sorted((d for d in os.listdir(out) if d.startswith("seed_")), key=lambda d: int(d[5:]))
    results = {}
    for d in seeds:
        path = os.path.join(out, d, "timeline.json")
        if os.path.exists(path):
            with open(path) as f:
                results[d] = judge(json.load(f))
    report = {"seeds": results, "targets": []}
    lines = ["| Target | Goal | " + " | ".join(s.replace("seed_", "s") for s in results) + " | Pass |",
             "| --- | --- | " + " | ".join("---" for _ in results) + " | --- |"]
    for name, key, ok, goal in TARGETS:
        vals = [results[s].get(key) for s in results]
        passed = all(ok(v) for v in vals) if vals else False
        report["targets"].append({"target": name, "key": key, "goal": goal, "values": vals, "pass": passed})
        lines.append(f"| {name} | {goal} | " + " | ".join(fmt(key, v) for v in vals) + f" | {'yes' if passed else 'no'} |")
    people = [results[s]["people_at_end"] for s in results if results[s].get("people_at_end") is not None]
    if people:
        med = statistics.median(people)
        spread_ok = med > 0 and all(abs(p - med) <= 0.25 * med for p in people)
        report["targets"].append({"target": "8 The same game twice", "key": "people_at_end", "goal": "within 25 % of the median", "values": people, "pass": spread_ok, "median": med})
        lines.append(f"| 8 The same game twice | within 25 % of the median ({med:,.0f}) | " + " | ".join(f"{p:,.0f}" for p in people) + f" | {'yes' if spread_ok else 'no'} |")
    with open(os.path.join(out, "targets.json"), "w") as f:
        json.dump(report, f, indent=2)
    with open(os.path.join(out, "report.md"), "w") as f:
        f.write("\n".join(lines) + "\n")
    print("\n".join(lines))
    return report


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--seeds", type=int, default=10)
    ap.add_argument("--first-seed", type=int, default=1)
    ap.add_argument("--minutes", type=float, default=360)
    ap.add_argument("--speed", type=float, default=16)
    ap.add_argument("--parallel", type=int, default=2)
    ap.add_argument("--bin", default=BIN_DEFAULT)
    ap.add_argument("--out", default="rig/latest")
    ap.add_argument("--check", metavar="DIR", help="only check an earlier run")
    ap.add_argument("--game", default="",
                    help='more game flags, e.g. "--play-level admiral --rivals 3 --rival-level admiral"')
    a = ap.parse_args()
    if a.check:
        check(a.check)
        return
    seeds = list(range(a.first_seed, a.first_seed + a.seeds))
    running = []
    started = time.time()
    while seeds or running:
        while seeds and len(running) < a.parallel:
            s = seeds.pop(0)
            running.append((s, play(a.bin, s, a.minutes, a.speed, os.path.join(a.out, f"seed_{s}"), a.game.split())))
        for s, p in list(running):
            if p.poll() is not None:
                running.remove((s, p))
                print(f"seed {s} done ({p.returncode}) after {time.time() - started:.0f} s", file=sys.stderr)
        time.sleep(1)
    check(a.out)


if __name__ == "__main__":
    main()
