Round 26 step a: plaza stand, main d3cf57ad (origin/main at the start; has b15e1ccf engine.trace and Water #587
ab985f0f; not Physics #588, still open), Tobias's PC, release, Ultra, window as the exe opens it (client 1600x900), real
time. ENGINE_PROFILE=1 showcase.exe --fly --profile --quality ultra; showcase.goto plaza 5 s after the world was built
(06:31:50.85 UTC), stay 75 s. Started after 29,282 s with no input. As round 25 a: twice a second engine.status and
engine.stats "render." and "offframe."; every 5 s of the stand engine.stats "render.cpu." (dumps/). New: right after the
40 s dump (40.1 s after arriving, 06:32:30.97 UTC) engine.trace {"path": ".../trace_plaza.json", "frames": 120}; the log's
"trace of 120 frames written" line came at 06:32:33.34. Steady window 25-75 s after arriving; 30-75 s in brackets.
Files: analysis.txt (ab25.py, as round 25), spanmax.txt (spanmax26.py: every profile's rows for think_and_act, "choose
position" and "prepare character", with the seconds after arriving), traces/ (trace_plaza.json.gz, 13.6 MB raw -> 1.7 MB;
trace_plaza_summary.txt = scripts/trace-summary.py --frames --worst 3; trace_plaza_frames.txt = trace26.py, one line
per frame with its longest systems and inner spans), dumps/, frames.csv, timeline.txt, nvidia_dmon.txt, plaza.jpg,
logs.zip (stderr.log with every profile, engine-profile.txt, sampled_stats.jsonl, stats_*.json).
Against round 25 a (ab25.py on both runs; "r25a" there is round 25's a_plaza_plain): ../a-plaza-r25-r26.txt and
../a-plaza-r25-r26-30-75s.txt.
