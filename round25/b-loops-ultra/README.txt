Round 25 step b: loops x1 at Ultra on main e11f16aa, Tobias's PC (RTX 5080, 32 threads, 64 GB), release, window as the
exe opens it (client 1600x900), round 24 step b's method. Started right after step a (no input since; 24,723 s idle);
the game exited by itself after the round, code 0.

PHYSICS_REPORT=240 RUST_LOG=info showcase.exe --loops overview,plaza,market,forest,lake,fortress,field,orbit:1 --profile --quality ultra
Per stop: the last 20 s of its 25 s dwell (tables.txt: fps / 1% low from engine.status twice a second; worst frame from
the 5 s frame profiles; render_system from the last profile in the window; GPU ms from the last render perf line; GPU
busy from nvidia-smi). A picture and a render + terrain engine.stats snapshot per stop at 20 s (jpg/, logs.zip).
New against round 24's step b (it polled engine.status only):
- engine.stats "water." and "render.extra_views" twice a second through the whole run (sampled_stats.jsonl in logs.zip;
  water.ripple_rebuilds per stop in npc_water.txt);
- engine.stats "render.extra_views" back to back (one answer a frame) from the forest picture (20 s into the forest's
  dwell) to 4 s after the lake arrival (fast.jsonl in logs.zip; lake_arrival.txt). So the forest's window has ~4.5 s of
  it at its end; the lake's window (5-25 s after arriving) has none.

Files: tables.txt (every column), fixed_update.txt (fixed25.py: per stop and profile FixedUpdate ms a frame, runs, ms a
run, ordering floor and chain, Render / Update / PostUpdate; the stop's mean in the tables' window), npc_water.txt
(loops25.py: per stop and 5 s profile, think_and_act, ripple_system, press_fallen_crowns, flow_bed_system,
step_beside_physics and the inner spans find path, nav chunk, ripples: rebuild / depths / streams / swash / speeds /
step, as ms a frame / max ms of a run / runs a frame (count = runs x frames); the worst single runs over the whole run;
the nav chunk counts; water.ripple_rebuilds per stop and per 5 s), lake_arrival.txt (lake25.py: the back-to-back
render.extra_views answers around the arrival with each frame's interval, the renderer's log lines, the profile covering
it), hitches.txt (every profile with a frame over 80 ms and the runs over 25 ms in it), passes.txt (per-pass GPU),
extras.txt (physics_simulate per profile, per-stop terrain steps), physics_reports.txt, summary.json, frames.csv,
timeline.txt, nvidia_dmon.txt, jpg/, logs.zip (stderr.log with every profile, engine-profile.txt, sampled_stats.jsonl,
fast.jsonl, stats JSON per stop).
The fps table against round 24, FixedUpdate, the fortress water and the lake arrival: ../README.txt, b).
