#!/usr/bin/env python3
"""Summarize an ENGINE_TRACE capture: per frame, how many threads ran systems, and what the
other cores waited on.

    python3 scripts/trace-summary.py engine-trace.json [--frames] [--worst N]

For each frame (between two `frame` marks) it splits the wall time into moments when no thread
ran a system ("idle": sync points, executor hand-offs, work outside systems), exactly one did
("alone": the serial part, Amdahl's law) and several did, then lists over all frames:
- the systems that ran alone longest, with the schedule they ran in;
- the idle gaps, named by the system that ended before and the one that started after;
- each schedule's wall time and its average number of busy threads.
`--frames` adds one line per frame; `--worst N` prints the N slowest frames system by system.
Accepts `.json` or `.json.gz`.
"""

import argparse
import gzip
import json
from collections import defaultdict


def load(path):
    opener = gzip.open if path.endswith(".gz") else open
    with opener(path, "rt", encoding="utf-8") as f:
        return json.load(f)["traceEvents"]


def intervals(events, cat):
    return [e for e in events if e.get("ph") == "X" and e.get("cat") == cat]


def schedule_at(schedules, t):
    """The innermost schedule span covering time t (schedules sorted by start)."""
    best = None
    for s in schedules:
        if s["ts"] > t:
            break
        if s["ts"] <= t < s["ts"] + s["dur"]:
            if best is None or s["dur"] <= best["dur"]:
                best = s
    return best["name"] if best else "-"


def sweep(systems, start, end):
    """Moments in [start, end) with the threads running a system and the innermost one each."""
    points = []
    for e in systems:
        a, b = max(e["ts"], start), min(e["ts"] + e["dur"], end)
        if b <= a:
            continue
        points.append((a, 1, e))
        points.append((b, 0, e))
    points.sort(key=lambda p: (p[0], p[1]))
    open_by_thread = defaultdict(list)
    t = start
    last_ended = None
    for at, is_start, e in points:
        if at > t:
            busy = {tid: stack[-1] for tid, stack in open_by_thread.items() if stack}
            yield t, at, busy, last_ended
        t = max(t, at)
        stack = open_by_thread[e["tid"]]
        if is_start:
            stack.append(e)
        else:
            if e in stack:
                stack.remove(e)
            last_ended = e
    if end > t:
        yield t, end, {}, last_ended


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("trace")
    ap.add_argument("--frames", action="store_true", help="one line per frame")
    ap.add_argument("--worst", type=int, default=0, help="detail the N slowest frames")
    ap.add_argument("--rows", type=int, default=25)
    args = ap.parse_args()

    events = load(args.trace)
    names = {e["tid"]: e["args"]["name"] for e in events if e.get("ph") == "M"}
    marks = sorted(e["ts"] for e in events if e.get("cat") == "frame")
    systems = sorted(intervals(events, "system"), key=lambda e: e["ts"])
    schedules = sorted(intervals(events, "schedule"), key=lambda e: e["ts"])
    if len(marks) < 1:
        raise SystemExit("no frame marks in the trace")
    frames = list(zip([0.0] + marks[:-1], marks))

    alone = defaultdict(float)  # (schedule, system) -> us
    gaps = defaultdict(float)  # (schedule, before, after) -> us
    sched_wall = defaultdict(float)
    sched_busy = defaultdict(float)  # thread-us inside each schedule
    thread_busy = defaultdict(float)
    totals = {"idle": 0.0, "alone": 0.0, "several": 0.0}
    per_frame = []
    pending_gap = None

    for i, (a, b) in enumerate(frames):
        f = {"idle": 0.0, "alone": 0.0, "several": 0.0, "wall": b - a}
        for t0, t1, busy, last in sweep(systems, a, b):
            span = t1 - t0
            sched = schedule_at(schedules, t0)
            sched_wall[sched] += span
            sched_busy[sched] += span * len(busy)
            for tid in busy:
                thread_busy[tid] += span
            if not busy:
                f["idle"] += span
                pending_gap = (sched, last["name"] if last else "frame start", t0, span)
                gaps_key = (sched, pending_gap[1])
                gaps[gaps_key + ("?",)] += span
            else:
                if pending_gap is not None:
                    # Name the gap by what started after it.
                    sched_g, before, _, g = pending_gap
                    gaps[(sched_g, before, "?")] -= g
                    after = next(iter(busy.values()))["name"]
                    gaps[(sched_g, before, after)] += g
                    pending_gap = None
                if len(busy) == 1:
                    f["alone"] += span
                    alone[(sched, next(iter(busy.values()))["name"])] += span
                else:
                    f["several"] += span
        for k in totals:
            totals[k] += f[k]
        per_frame.append(f)

    n = len(frames)
    wall = sum(f["wall"] for f in per_frame)
    ms = lambda us: us / 1000.0 / n
    print(f"{n} frames, {wall / n / 1000:.2f} ms a frame")
    print(
        f"  idle {ms(totals['idle']):.2f} ms, alone {ms(totals['alone']):.2f} ms, "
        f"several {ms(totals['several']):.2f} ms; "
        f"{sum(thread_busy.values()) / max(wall, 1):.2f} threads busy on average"
    )

    print("\nschedules (ms a frame, threads busy on average):")
    for s, w in sorted(sched_wall.items(), key=lambda kv: -kv[1])[: args.rows]:
        print(f"  {ms(w):7.2f}  {sched_busy[s] / max(w, 1):4.2f}  {s}")

    print("\nran alone (ms a frame):")
    for (s, name), us in sorted(alone.items(), key=lambda kv: -kv[1])[: args.rows]:
        print(f"  {ms(us):7.3f}  {s:16} {name}")

    print("\nidle gaps (ms a frame): after -> before")
    for (s, before, after), us in sorted(gaps.items(), key=lambda kv: -kv[1])[: args.rows]:
        if us > 0.5:
            print(f"  {ms(us):7.3f}  {s:16} {before} -> {after}")

    print("\nthreads (ms of systems a frame):")
    for tid, us in sorted(thread_busy.items(), key=lambda kv: -kv[1]):
        print(f"  {ms(us):7.2f}  {names.get(tid, tid)}")

    if args.frames:
        print("\nframes: wall idle alone several (ms)")
        for i, f in enumerate(per_frame):
            print(
                f"  {i:4} {f['wall'] / 1000:7.2f} {f['idle'] / 1000:6.2f} "
                f"{f['alone'] / 1000:6.2f} {f['several'] / 1000:6.2f}"
            )

    for i in sorted(range(n), key=lambda i: -per_frame[i]["wall"])[: args.worst]:
        a, b = frames[i]
        print(f"\nframe {i}: {(b - a) / 1000:.2f} ms (us from frame start, thread, ms)")
        for e in systems:
            if a <= e["ts"] < b and e["dur"] >= 100:
                print(
                    f"  {(e['ts'] - a):9.0f} {names.get(e['tid'], e['tid'])[:14]:14} "
                    f"{e['dur'] / 1000:7.2f}  {e['name']}"
                )


if __name__ == "__main__":
    main()
