Round 29b steps 2 and 3: plaza stand, then the lake arrival from it, main 4c164e62, Tobias's PC, release, Ultra, client
1600x900, real time, plain. ENGINE_PROFILE=1 showcase.exe --fly --profile --quality ultra; started after 46,048 s with no
input. Step 3: showcase.goto plaza (11:11:18.74 UTC), stay 75 s; engine.status and engine.stats "render." and
"offframe." twice a second, engine.stats "render.cpu." every 5 s (dumps/), engine.trace 120 frames right after the 40 s
dump (11:11:58.83; written 11:12:01.18). Step 2: engine.trace 20 frames, and the moment it was answered showcase.goto
lake (answered 11:12:35.35; the goto's frame is the trace's frame 0); then each frame engine.stats "render.gi" on the
main connection (arrival_stats.jsonl) and, written right after it, engine.stats "render.cpu.meshes" on a second
connection (arrival_stats2.jsonl): 20 pairs on frames 3488-3507, each pair answered in the same frame. Then 15 s at the
lake and a picture.
Files: analysis.txt (ab28.py on the plaza stand), arrival.txt (arrive29b.py: both connections' answers, every key, and
the arrival trace per frame), arrival_stats.jsonl, arrival_stats2.jsonl, traces/ (trace_plaza.json.gz,
trace_lake_arrival.json.gz; *_summary.txt = scripts/trace-summary.py --frames --worst 3; *_frames.txt = trace26.py;
trace_lake_arrival_inner.txt = inner29.py: ripple_system, run_scripts and render_system with their spans, per frame;
trace_lake_arrival_schedules.txt = sched26.py per frame and top26.py against round 29), dumps/, frames.csv,
timeline.txt, nvidia_dmon.txt, jpg/, logs.zip. Against rounds 29 and 28: ../plaza-r29b-r29-r28.txt.