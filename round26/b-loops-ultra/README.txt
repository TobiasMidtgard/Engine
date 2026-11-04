Round 26 step b: loops x1 at Ultra on main d3cf57ad, Tobias's PC (RTX 5080, 32 threads, 64 GB), release, window as the
exe opens it (client 1600x900), round 25 step b's method. Started right after step a (no input since; 29,422 s idle);
the game exited by itself after the round, code 0.

PHYSICS_REPORT=240 RUST_LOG=info showcase.exe --loops overview,plaza,market,forest,lake,fortress,field,orbit:1 --profile --quality ultra
As round 25 b: per stop the last 20 s of its 25 s dwell in tables.txt; a picture and a render + terrain stats snapshot
per stop at 20 s; engine.stats "water." and "render.extra_views" twice a second; engine.stats "render.extra_views" back
to back (one answer a frame) from the forest picture to 4 s after the lake arrival (fast.jsonl, lake_arrival.txt).
New: engine.trace (one at a time, each after the log's "written" line of the one before):
- 60 frames 10 s after arriving at plaza, market, lake and fortress (10 s after the stop before's loop line, by that
  line's own log time): trace_plaza / _market / _lake / _fortress.json.gz;
- 30 frames armed 24.96 s after the market's loop line (log time), i.e. just before the forest's loop line and the jump
  to the lake: asked at 06:35:52.149, the forest's line logged at 06:35:52.205, so the trace holds the forest's loop
  frame (its frame 1) and the jump frame (frame 2; both run showcase::run, the --loops stepper) and 27 frames after:
  trace_lake_arrival.json.gz. The 5 s profiles covering the traces show no slowdown from the recording.
Loops lines were read from the log's tail every 50 ms (round 25: the whole log every 200 ms); each picture still came
20.2 s after its stop's arrival line, as in round 25.

Files: as round 25 (tables.txt, fixed_update.txt, npc_water.txt, lake_arrival.txt, hitches.txt, passes.txt,
extras.txt, physics_reports.txt, summary.json, frames.csv, timeline.txt, nvidia_dmon.txt, jpg/, logs.zip), plus
spanmax.txt (spanmax26.py: every profile's think_and_act, "choose position" and "prepare character" rows and the max of
a run per stop) and traces/: each trace .json.gz (6.6-8.6 MB raw, 0.6-1.1 MB gz), its trace-summary.py --frames --worst
3 text (_summary.txt) and its trace26.py per-frame text (_frames.txt); lake_arrival_vs_steady.txt (sched26.py per frame
of the arrival trace: FixedUpdate / Render / Update / PostUpdate runs and ms, shore_wetness, ripple_system,
physics_simulate; top26.py: FixedUpdate's systems and spans in the arrival's frames 5-20 against the lake trace 10 s
later); fortress_schedules.txt (the same for the fortress trace, with Update).
top26.py leaves out the render thread's spans (they overlap the main thread's next frame in time); trace-summary.py's
own tables count every thread.
The fps table against round 25, FixedUpdate, the fortress water and the lake arrival: ../README.txt, b).
