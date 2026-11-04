Round 24 step b: loops x1 at Ultra on main 69c48190, Tobias's PC (RTX 5080, 32 threads, 64 GB), release, window as the
exe opens it (client 1600x900), round 23 step 3a's method with one round. Started right after step a2 (no input since;
5230 s idle), the game exited by itself after the round, code 0.

PHYSICS_REPORT=240 RUST_LOG=info showcase.exe --loops overview,plaza,market,forest,lake,fortress,field,orbit:1 --profile --quality ultra
Per stop: the last 20 s of its 25 s dwell (tables.txt: fps / 1% low from engine.status twice a second; worst frame from
the 5 s frame profiles; render_system from the last profile in the window; GPU ms from the last render perf line; GPU
busy from nvidia-smi). A picture and a render + terrain engine.stats snapshot per stop at 20 s (jpg/, logs.zip).
The fps table against round 23, the NPC, nav chunk and lake findings: ../README.txt, b).

Files: tables.txt (every column), npc_water.txt (loops24.py: per stop and 5 s profile, engine_npc::think_and_act, water
ripple_system, flow_bed_system, softbody step_beside_physics, and the inner spans find path, nav chunk, ripples: step,
ripples: rebuild, as ms a frame / max ms of a run / runs a frame (count = runs x frames); then the worst single runs over
the whole run and the nav chunk counts), hitches.txt (hitches24.py: every profile with a frame over 80 ms and the runs
over 25 ms in it), passes.txt (per-pass GPU), extras.txt (physics_simulate per profile, per-stop terrain steps),
physics_reports.txt, summary.json, frames.csv, timeline.txt, nvidia_dmon.txt, jpg/, logs.zip (stderr.log with every
profile, engine-profile.txt, stats JSON per stop).
