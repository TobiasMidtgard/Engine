Round 29 steps 3 and 4: plaza stand, then the lake arrival from it, main 94ab0ecc, Tobias's PC, release, Ultra, window as
the exe opens it (client 1600x900), real time, plain. ENGINE_PROFILE=1 showcase.exe --fly --profile --quality ultra;
started after 43,693 s with no input.
Step 3: showcase.goto plaza 5 s after the world was built (10:31:56.71 UTC), stay 75 s, as round 26 a / round 27 a1:
engine.status and engine.stats "render." and "offframe." twice a second, engine.stats "render.cpu." every 5 s (dumps/),
engine.trace 120 frames right after the 40 s dump (10:32:36.84; written 10:32:39.26). Then the plaza picture.
Step 4: engine.trace {"frames": 20} to trace_lake_arrival.json, and the moment it was answered showcase.goto lake
(answered 10:33:13.27; the trace records from the end of the frame that asked it, and the bridge runs a request at a
frame's start, so the goto's frame is the trace's frame 0); then engine.stats {"prefix": "render.gi"} back to back on the
same connection, 20 answers on frames 3487-3506, one a frame (arrival_stats.jsonl: wall time and answer). Then 15 s at
the lake, stats and a picture.
Files: analysis.txt (ab28.py: the plaza stand's profiles, render keys, fps per 5 s, the dumps), arrival.txt (arrive28.py:
the 20 answers with every render.gi.* key, each one's wall interval, and the arrival trace per frame), arrival_stats.jsonl,
traces/ (trace_plaza.json.gz, trace_lake_arrival.json.gz; *_summary.txt = scripts/trace-summary.py --frames --worst 3;
*_frames.txt = trace26.py; trace_lake_arrival_schedules.txt = sched26.py per frame; trace_plaza_vs_r28.txt = diff27.py
against round 28's plaza trace), dumps/, frames.csv, timeline.txt, nvidia_dmon.txt, jpg/ (plaza, lake), logs.zip.
trace_lake_arrival_inner.txt = inner29.py: render_system, ripple_system, shore_wetness_system and run_scripts with their spans, per frame. Against rounds 28 and 27: ../plaza-r29-r28-r27.txt.
