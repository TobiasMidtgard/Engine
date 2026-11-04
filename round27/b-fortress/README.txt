Round 27 step b: fortress stand, main ced07d57, Tobias's PC, release, Ultra, window as the exe opens it (client
1600x900), real time, plain (no ENGINE_EXECUTOR). ENGINE_PROFILE=1 showcase.exe --fly --profile --quality ultra;
showcase.goto fortress 5 s after the world was built (07:46:45.25 UTC), stay 60 s; started after 33,782 s with no input.
engine.status and engine.stats "render." and "offframe." twice a second (no render.cpu. dumps). 20 s after arriving
(07:47:05.41) engine.trace 60 frames; the log's "written" line at 07:47:08.36.
Files: traces/ (trace_fortress.json.gz; trace_fortress_inner.txt = inner27.py: the spans inside
engine_destruction::build_system and engine_script::run_scripts, totals and per frame; trace_fortress_summary.txt =
scripts/trace-summary.py --frames --worst 3; trace_fortress_frames.txt = trace26.py), frames.csv, timeline.txt,
nvidia_dmon.txt, fortress.jpg (after the stand), logs.zip (stderr.log with every profile, engine-profile.txt,
sampled_stats.jsonl, stats_*.json).
inner27.py counts a span as inside a system when it starts during one of the system's runs, on any thread but the
render thread's (so pool work the system hands out counts; its "other spans" can be time overlaps on other threads, e.g.
smoke on the async pool).
