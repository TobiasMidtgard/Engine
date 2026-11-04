Round 27 step a3: plaza stand, main ced07d57, Tobias's PC, release, Ultra, window as the exe opens it (client
1600x900), real time. ENGINE_EXECUTOR=single:Update,PostUpdate,First,Last,PreUpdate,FixedPreUpdate,FixedPostUpdate (FixedUpdate, Extract and Render on the multi-threaded executor).
ENGINE_PROFILE=1 showcase.exe --fly --profile --quality ultra; showcase.goto plaza 5 s after the world was built
(07:44:30.63 UTC), stay 75 s; started after 33,647 s with no input. As round 26 a: twice a second engine.status and
engine.stats "render." and "offframe."; every 5 s of the stand engine.stats "render.cpu." (dumps/); right after the 40 s
dump engine.trace 120 frames (asked 07:45:10.77, the log's "written" line at 07:45:13.40 UTC).
Files: analysis.txt (ab27.py on this run alone: every profile in the 25-75 s window, render keys, fps per 5 s, the
dumps), traces/ (trace_plaza.json.gz; trace_plaza_summary.txt = scripts/trace-summary.py --frames --worst 3;
trace_plaza_frames.txt = trace26.py), dumps/, frames.csv, timeline.txt, nvidia_dmon.txt, plaza.jpg, logs.zip
(stderr.log with every profile, engine-profile.txt, sampled_stats.jsonl, stats_*.json).
The three runs side by side: ../a-plaza-a1-a2-a3.txt (25-75 s) and ../a-plaza-a1-a2-a3-30-75s.txt; per schedule the
systems that changed most against a1 in the traces: ../a-plaza-schedule-diffs.txt.
