Round 24 step a2: plaza stand with ENGINE_FIXED_SYNC=sets, main 69c48190, Tobias's PC, release, Ultra, window as the exe opens it (client 1600x900),
real time. ENGINE_FIXED_SYNC=sets ENGINE_PROFILE=1 showcase.exe --fly --profile --quality ultra; showcase.goto plaza 5 s after the world was
built (23:48:39 UTC), stay 75 s. Started after 5095 s with no input. Twice a second: engine.status (frame, fixed_tick,
fps) and engine.stats "render." and "offframe."; every 5 s of the stand engine.stats "render.cpu." (dumps/).
Steady window 25-75 s after arriving (round 23 step 1's). The numbers and the A/B against a1: ../README.txt and
../a-plaza-ab.txt.
Files: analysis.txt (ab24.py: every profile in the window with systems busy, cores, one at a time / no system /
several at once, FixedUpdate ms and runs, each schedule's ordering floor and the FixedUpdate chain; the render keys;
fps and fixed steps per 5 s; the render.cpu.* dumps as a table), dumps/ (stats_dump_plaza_NNNs.json), frames.csv,
timeline.txt, nvidia_dmon.txt, plaza.jpg, logs.zip (stderr.log with every profile and the parallelism report,
engine-profile.txt, sampled_stats.jsonl, stats_*.json).
The flag is read once (crates/core/src/app.rs set_sync_points); the parallelism report at frame 30 shows it took
effect: FixedUpdate 185 systems, 9 exclusive, 4 bevy_ecs::apply_deferred (plain: 200, 24, 19).
