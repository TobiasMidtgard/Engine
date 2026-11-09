#!/usr/bin/env python3
"""Add up `engine-hitches.txt` (the profiler's slow-frame log, see crates/core/src/profile.rs).

    scripts/hitch-summary.py engine-hitches.txt [--from SECONDS] [--to SECONDS] [--top N]

Each slow frame is blamed on one thing: the longest system that ran on the main thread in it,
and inside that system the innermost span that still covers most of it (a system that spent
its frame inside "stream: step" is listed as that). A frame whose main-thread spans don't add
up to half of it is blamed on "(outside the main thread's spans)": the main thread waited, for
the render thread, a lock or the display. A frame no one system holds but which ran three or
more fixed steps is "(fixed-step catch-up)".

Prints how many frames each cause held, the time they took in all and the worst one, so the
list reads as "what to fix first". `--from`/`--to` keep only frames in that span of seconds
(to leave out the loading screen).
"""

import argparse
import collections
import re
import sys

LINE = re.compile(
    r"^frame (\d+) at ([\d.]+) s: ([\d.]+) ms"
    r"(?:, (\d+) fixed steps, (\d+) ms dropped)? \|(.*)\|(.*)$"
)


def rows(text):
    out = []
    for part in text.split(";"):
        part = part.strip()
        if not part:
            continue
        name, _, ms = part.rpartition(" ")
        try:
            out.append((name, float(ms)))
        except ValueError:
            pass
    return out


def blame(frame_ms, main, steps):
    """The span to blame, and the system it ran in."""
    systems = [(n, ms) for n, ms in main if not n.startswith(("in ", "schedule "))]
    inner = [(n, ms) for n, ms in main if n.startswith("in ")]
    if not systems or systems[0][1] < 0.25 * frame_ms:
        # No one system holds the frame. Several fixed steps in it: the simulation catching
        # up after a slow frame, each step at its usual cost.
        if steps >= 3:
            return f"(fixed-step catch-up, {min(steps, 4)}{'+' if steps > 4 else ''} steps)", ""
        covered = sum(ms for n, ms in main if n.startswith("schedule "))
        if covered < 0.5 * frame_ms:
            return "(outside the main thread's spans)", ""
        return "(many small systems)", ""
    system, system_ms = systems[0]
    # The innermost span that still holds most of the system's time: spans are listed longest
    # first, so the last one over 60% of it is the deepest.
    deepest = None
    for name, ms in inner:
        if 0.6 * system_ms <= ms <= system_ms * 1.02:
            deepest = name
    return (deepest[3:] if deepest else system), system


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("file")
    ap.add_argument("--from", dest="start", type=float, default=0.0)
    ap.add_argument("--to", dest="end", type=float, default=float("inf"))
    ap.add_argument("--top", type=int, default=25)
    ap.add_argument("--over", type=float, default=0.0, help="only frames longer than this, ms")
    ap.add_argument("--list", action="store_true", help="also list every frame with its blame")
    args = ap.parse_args()

    by = collections.defaultdict(lambda: [0, 0.0, 0.0, ""])
    frames = []
    for line in open(args.file, encoding="utf-8", errors="replace"):
        m = LINE.match(line.rstrip("\n"))
        if not m:
            continue
        frame, at, ms = int(m.group(1)), float(m.group(2)), float(m.group(3))
        if not (args.start <= at <= args.end) or ms <= args.over:
            continue
        what, system = blame(ms, rows(m.group(6)), int(m.group(4) or 0))
        entry = by[what]
        entry[0] += 1
        entry[1] += ms
        entry[2] = max(entry[2], ms)
        entry[3] = system
        frames.append((frame, at, ms, what))

    if not frames:
        print("no slow frames in that span")
        return 0
    total = sum(f[2] for f in frames)
    print(f"{len(frames)} slow frames, {total:.0f} ms in all, worst {max(f[2] for f in frames):.1f} ms")
    for bound in (33.0, 50.0, 100.0, 250.0):
        print(f"  over {bound:.0f} ms: {sum(1 for f in frames if f[2] > bound)}")
    print(f"\n{'frames':>6} {'ms in all':>10} {'worst':>8}  held by")
    for what, (n, ms, worst, system) in sorted(by.items(), key=lambda kv: -kv[1][1])[: args.top]:
        where = f"  [{system}]" if system and system != what else ""
        print(f"{n:>6} {ms:>10.0f} {worst:>8.1f}  {what}{where}")
    if args.list:
        print()
        for frame, at, ms, what in frames:
            print(f"frame {frame} at {at:.2f} s: {ms:.1f} ms: {what}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
