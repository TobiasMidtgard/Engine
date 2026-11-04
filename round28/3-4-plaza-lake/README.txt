Round 28 steps 3 and 4: plaza stand, then the lake arrival from it, main 6da6a069, Tobias's PC, release, Ultra, window as
the exe opens it (client 1600x900), real time, plain. ENGINE_PROFILE=1 showcase.exe --fly --profile --quality ultra;
started after 36,224 s with no input.
Step 3: showcase.goto plaza 5 s after the world was built (08:27:27.78 UTC), stay 75 s, as round 26 a / round 27 a1:
engine.status and engine.stats "render." and "offframe." twice a second, engine.stats "render.cpu." every 5 s (dumps/),
engine.trace 120 frames right after the 40 s dump (08:28:08.04; written 08:28:10.57). Then the plaza picture.
Step 4: engine.trace {"frames": 20} to trace_lake_arrival.json, and the moment it was answered showcase.goto lake
(answered 08:28:44.36; the trace records from the end of the frame that asked it, and the bridge runs a request at a
frame's start, so the goto's frame is the trace's frame 0); then engine.stats {"prefix": "render.gi"} back to back on the
same connection, 20 answers on frames 3351-3370, one a frame (arrival_stats.jsonl: wall time and answer). Then 15 s at
the lake, stats and a picture.
Files: analysis.txt (ab28.py: the plaza stand's profiles, render keys, fps per 5 s, the dumps), arrival.txt (arrive28.py:
the 20 answers with every render.gi.* key, each one's wall interval, and the arrival trace per frame), arrival_stats.jsonl,
traces/ (trace_plaza.json.gz, trace_lake_arrival.json.gz; *_summary.txt = scripts/trace-summary.py --frames --worst 3;
*_frames.txt = trace26.py; trace_lake_arrival_schedules.txt = sched26.py per frame; trace_plaza_vs_r27.txt = diff27.py
against round 27 a1's plaza trace), dumps/, frames.csv, timeline.txt, nvidia_dmon.txt, jpg/ (plaza, lake), logs.zip.
Against round 27 a1 and round 26 a: ../plaza-r28-r27-r26.txt.