#!/usr/bin/env python3
"""Which shared data keeps the frame serial: joins an ENGINE_TRACE capture with the schedule
audit's per-system access.

    cargo run -p schedule-audit -- --json > access.json
    python3 scripts/trace-blockers.py engine-trace.json access.json [--rows N]

Whenever exactly one thread runs a system (the "alone" time of trace-summary.py), the systems
that start within a moment of it ending were waiting for it. Each such wait is put down to
what the two share: a component or resource one writes and the other reads or writes, an
exclusive system, or, when they share nothing, the schedule's ordering (`.after`, sets,
chains). The alone time is split evenly over the systems that were waiting, then summed:
- per piece of data: the alone time it is to blame for (where an outbox, a `Without` filter or
  a split would let the systems overlap);
- per pair: the system that ran alone, the one that waited, and what they share.
Takes the trace from the build the access was dumped from (system names must match).
"""

import argparse
import gzip
import json
from collections import defaultdict

# A system that starts this soon (us) after one that ran alone ends was waiting for it.
HANDOFF_US = 40.0


def load(path):
    opener = gzip.open if path.endswith(".gz") else open
    with opener(path, "rt", encoding="utf-8") as f:
        return json.load(f)["traceEvents"]


def conflict(a, b, b_name):
    """What a and b share that keeps them apart (a list), or None if nothing does."""
    if b_name.endswith("(commands)"):
        return ["(commands applied)"]
    if a is None or b is None:
        # Run conditions, piped systems, the game's own systems: not in the audit's app.
        return ["(not in the audit)"]
    if a["exclusive"] or b["exclusive"]:
        return ["(exclusive)"]
    if a["unbounded"] or b["unbounded"]:
        return ["(whole-world access)"]
    aw, bw = set(a["writes"]), set(b["writes"])
    ar, br = set(a["reads"]), set(b["reads"])
    shared = (aw & (bw | br)) | (bw & ar)
    return sorted(shared) or None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("trace")
    ap.add_argument("access")
    ap.add_argument("--rows", type=int, default=30)
    args = ap.parse_args()

    events = load(args.trace)
    access = {}
    for sched in json.load(open(args.access)):
        label = sched["schedule"]
        for s in sched["systems"]:
            access[(label, s["name"])] = s
    marks = sorted(e["ts"] for e in events if e.get("cat") == "frame")
    n = max(1, len(marks))
    systems = sorted(
        (e for e in events if e.get("ph") == "X" and e.get("cat") == "system"),
        key=lambda e: e["ts"],
    )
    schedules = sorted(
        (e for e in events if e.get("ph") == "X" and e.get("cat") == "schedule"),
        key=lambda e: e["ts"],
    )

    def schedule_of(e):
        best = None
        for s in schedules:
            if s["ts"] > e["ts"]:
                break
            if s["ts"] <= e["ts"] < s["ts"] + s["dur"] and (best is None or s["dur"] <= best["dur"]):
                best = s
        return best["name"] if best else "-"

    sched = [schedule_of(e) for e in systems]
    # Alone time per system run: the parts of its span when no other thread ran a system.
    points = []
    for i, e in enumerate(systems):
        points.append((e["ts"], 1, i))
        points.append((e["ts"] + e["dur"], 0, i))
    points.sort()
    running = set()
    alone_us = defaultdict(float)
    t = None
    for at, is_start, i in points:
        if t is not None and len(running) == 1 and at > t:
            alone_us[next(iter(running))] += at - t
        t = at
        if is_start:
            running.add(i)
        else:
            running.discard(i)

    by_data = defaultdict(float)
    by_pair = defaultdict(float)
    starts = [e["ts"] for e in systems]
    import bisect

    for i, us in alone_us.items():
        if us < 1:
            continue
        a = systems[i]
        end = a["ts"] + a["dur"]
        lo = bisect.bisect_left(starts, end - 1)
        hi = bisect.bisect_right(starts, end + HANDOFF_US)
        waiting = [j for j in range(lo, hi) if j != i and sched[j] == sched[i]]
        if not waiting:
            by_data["(nothing started after it)"] += us
            by_pair[(sched[i], a["name"], "-", "(nothing started after it)")] += us
            continue
        share = us / len(waiting)
        sa = access.get((sched[i], a["name"]))
        for j in waiting:
            b = systems[j]
            sb = access.get((sched[j], b["name"]))
            what = conflict(sa, sb, b["name"])
            if what is None:
                what = ["(ordering)"]
            for d in what:
                by_data[f"{sched[i]:14} {d}"] += share / len(what)
            by_pair[(sched[i], a["name"], b["name"], ", ".join(what))] += share

    ms = lambda us: us / 1000.0 / n
    print(f"{n} frames; alone time put down to what the waiting systems share (ms a frame)\n")
    print("by data:")
    for d, us in sorted(by_data.items(), key=lambda kv: -kv[1])[: args.rows]:
        print(f"  {ms(us):7.3f}  {d}")
    print("\nby pair (ran alone -> started after it: what they share):")
    for (s, a, b, what), us in sorted(by_pair.items(), key=lambda kv: -kv[1])[: args.rows]:
        print(f"  {ms(us):7.3f}  {s:14} {a} -> {b}: {what}")


if __name__ == "__main__":
    main()
