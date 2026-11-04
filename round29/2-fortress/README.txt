Round 29 step 2: fortress stand, main 94ab0ecc, Tobias's PC, release, Ultra, window as the exe opens it (client
1600x900), real time, plain. ENGINE_PROFILE=1 showcase.exe --fly --profile --quality ultra; showcase.goto fortress 5 s
after the world was built (10:29:56.78 UTC), stay 60 s; started after 43,566 s with no input. engine.status and
engine.stats "render." and "offframe." twice a second. 20.1 s after arriving (10:30:16.89) engine.trace 60 frames; the
log's "written" line at 10:30:18.94.
Files: analysis.txt (ab28.py --place fortress --steady 25 60: every profile, render keys, fps per 5 s), traces/
(trace_fortress.json.gz; trace_fortress_summary.txt = scripts/trace-summary.py --frames --worst 3; trace_fortress_inner.txt
= inner29.py, the spans inside build_system, rebuild_world_model, ripple_system and render's prepare water, totals and per frame; trace_fortress_frames.txt =
trace26.py; trace_fortress_vs_r28.txt = diff27.py against round 28's fortress trace, with every water system), frames.csv, timeline.txt,
nvidia_dmon.txt, fortress.jpg (after the stand), logs.zip (stderr.log with every profile, engine-profile.txt,
sampled_stats.jsonl, stats_*.json). Against rounds 28 and 27: ../fortress-r29-r28-r27.txt.
