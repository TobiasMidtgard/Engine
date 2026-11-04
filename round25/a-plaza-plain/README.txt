Round 25 step a: plaza stand, main e11f16aa (origin/main at the start; has 25c1e855), Tobias's PC, release, Ultra,
window as the exe opens it (client 1600x900), real time, plain (no ENGINE_FIXED_SYNC). ENGINE_PROFILE=1 showcase.exe
--fly --profile --quality ultra; showcase.goto plaza 5 s after the world was built (05:13:32 UTC), stay 75 s. Started
after 24,586 s with no input. Twice a second: engine.status (frame, fixed_tick, fps) and engine.stats "render." and
"offframe."; every 5 s of the stand engine.stats "render.cpu." (dumps/). Steady window 25-75 s after arriving (round 23
step 1's and round 24 a1's); 30-75 s in brackets in ../README.txt.
Files: analysis.txt (ab25.py: every profile in the window with systems busy, cores, one at a time / no system / several
at once, FixedUpdate ms and runs, each schedule's ordering floor and the FixedUpdate chain; the render keys with
render.prepare_shared_ms; fps and fixed steps per 5 s; the render.cpu.* dumps as a table; the render.cpu.prepare rows),
dumps/ (stats_dump_plaza_NNNs.json), frames.csv, timeline.txt, nvidia_dmon.txt, plaza.jpg, logs.zip (stderr.log with
every profile and the parallelism report, engine-profile.txt, sampled_stats.jsonl, stats_*.json).
Against round 24 a1 (the same script on both): ../a-plaza-r24-r25.txt.
