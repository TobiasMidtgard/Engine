Round 28 step 2: fortress stand, main 6da6a069, Tobias's PC, release, Ultra, window as the exe opens it (client
1600x900), real time, plain. ENGINE_PROFILE=1 showcase.exe --fly --profile --quality ultra; showcase.goto fortress 5 s
after the world was built (08:25:28.50 UTC), stay 60 s; started after 36,104 s with no input. engine.status and
engine.stats "render." and "offframe." twice a second. 20.2 s after arriving (08:25:48.70) engine.trace 60 frames; the
log's "written" line at 08:25:51.03.
Files: analysis.txt (ab28.py --place fortress --steady 25 60: every profile, render keys, fps per 5 s), traces/
(trace_fortress.json.gz; trace_fortress_summary.txt = scripts/trace-summary.py --frames --worst 3; trace_fortress_inner.txt
= inner27.py, the spans inside engine_destruction::build_system, totals and per frame; trace_fortress_frames.txt =
trace26.py; trace_fortress_vs_r27.txt = diff27.py against round 27's fortress trace), frames.csv, timeline.txt,
nvidia_dmon.txt, fortress.jpg (after the stand), logs.zip (stderr.log with every profile, engine-profile.txt,
sampled_stats.jsonl, stats_*.json). Against round 27's stand: ../fortress-r28-r27.txt.