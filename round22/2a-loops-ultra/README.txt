Round 22 step 2a: loops at Ultra on main 5de4f33e, Tobias's PC (RTX 5080, 32 threads, 64 GB), release, window as the
exe opens it (client 1600x900), 21d's method (2 rounds), with PHYSICS_REPORT=240 RUST_LOG=info.

PHYSICS_REPORT=240 RUST_LOG=info showcase.exe --loops overview,plaza,market,forest,lake,fortress,field,orbit:2 --profile --quality ultra
Per stop: the last 20 s of its 25 s dwell. fps / frame ms / 1% low from engine.status twice a second; worst frame from the
5 s frame profiles; render_system etc. from the last frame profile in the window; GPU ms from the last render perf line;
GPU busy from nvidia-smi. Round 1 took a picture and a render + terrain engine.stats snapshot per stop at 20 s. The game
exited by itself after round 2, code 0.

stop     | round 1 fps / 1% low | round 2 fps / 1% low | render_system ms r1 / r2 | GPU ms r1 / r2 | GPU busy % r1 / r2 | 21d r1 / r2 fps
overview | 37.9* / 28.2 | 43.7 / 40.7 | 12.34 / 10.72 | 24.8 / 21.7 |  98 / 100 | 37.7 / 43.0
plaza    | 41.6 / 35.6  | 40.5 / 20.3 |  8.50 /  8.63 | 18.0 / 18.0 |  80 /  76 | 36.0 / 39.8
market   | 28.8 / 13.6  | 29.1 / 25.7 |  9.32 /  9.97 | 21.7 / 22.0 |  61 /  66 | 26.8 / 20.6
forest   | 31.8 / 17.3  | 30.2 / 18.7 |  8.89 / 10.28 | 18.9 / 18.7 |  66 /  57 | 34.4 / 17.4
lake     | 22.4 / 17.6  | 24.3 / 16.9 | 18.47 / 18.86 | 18.5 / 18.5 |  57 /  58 | 25.8 / 16.5
fortress | 22.6 / 14.2  | 17.1 / 12.5 | 12.97 / 13.84 | 19.2 / 20.0 |  47 /  35 | 21.5 / 14.3
field    | 40.3 / 32.6  | 38.5 / 33.6 |  8.08 /  8.72 | 20.5 / 23.4 |  88 /  94 | 40.4 / 38.7
orbit    | 64.1 / 58.2  | 64.6 / 58.9 |  6.02 /  6.19 |  7.6 /  7.6 |  56 /  54 | 64.7 / 63.6
(* overview round 1 includes the start.) tables.txt has every column (systems busy, cores, ran alone).

Does round 2 hold round 1 from market to fortress? Yes at market (28.8 -> 29.1), forest (31.8 -> 30.2) and lake (22.4 ->
24.3); fortress drops 22.6 -> 17.1. 21d dropped at all four (26.8/34.4/25.8/21.5 -> 20.6/17.4/16.5/14.3).
- physics_simulate (extras.txt, each 5 s profile in the window, ms a frame): round 2 market 2.6-2.9, forest 1.8-4.8 (one
  profile max 19.88 a run), lake 3.8-4.0, fortress 4.4-4.8; round 1 2.5-2.7, 3.1-5.9, 7.5-8.2, 3.9-4.4. 21d round 2:
  18.7-36.3.
- The physics reports (physics_reports.txt, every 240 steps, ~4 s) agree: round 2 market 2.1-4.0 ms a step, forest
  1.4-3.1, lake 2.0-2.3 (6.0 for the first), fortress 2.4-3.3; Rapier's CCD sweeps 0.13-0.34 ms a step there (round 1:
  0.24-1.25, the lake's highest).
- Fortress round 2 is the CPU at 3.4 fixed steps a frame: systems busy 70.8 ms a frame on 1.22 cores (round 1 67.8 at
  3.2 steps), FixedUpdate 24.9 (22.4), destruction::build_system 8.93 a frame (7.73), smoke::step 4.01 (2.91),
  water::flow_bed_system 3.99 (3.05); physics_simulate 4.46 (4.40). GPU 35% busy.
The same loops at render scale 0.7 right after (2b) did lose round 2 at forest, lake and fortress, with physics:
see 2b-render-scale-0.7/README.txt.

Per-pass GPU (passes.txt; the last render profile in each window, 5 s averages, ms). Round 2:
stop     | total | scene | scene: terrain | scene: meshes | prepass | shadows | weather clouds | probe gi | world refl. | transparent | bounce light | aerial persp.
overview | 21.73 | 13.80 | 7.67 | 5.33 | 2.10 | 0.83 | 1.93 | 0.03 | 0.02 | 0.39 | 0.62 | 0.34
plaza    | 18.02 |  6.48 | 1.79 | 3.91 | 2.22 | 1.75 | 2.29 | 1.28 | 0.58 | 0.26 | 0.61 | 0.38
market   | 21.97 |  9.41 | 3.23 | 3.21 | 3.01 | 1.90 | 2.17 | 0.76 | 1.33 | 0.30 | 0.56 | 0.41
forest   | 18.74 |  6.89 | 2.30 | 4.10 | 2.73 | 2.78 | 2.18 | 0.76 | 0.16 | 0.12 | 0.58 | 0.40
lake     | 18.47 |  8.67 | 6.36 | 1.85 | 1.19 | 1.36 | 2.22 | 0.09 | 0.19 | 1.66 | 0.61 | 0.41
fortress | 19.96 |  8.86 | 4.01 | 3.60 | 2.24 | 1.58 | 1.92 | 0.12 | 0.11 | 1.80 | 0.62 | 0.39
field    | 23.41 |  8.34 | 2.93 | 4.61 | 1.93 | 1.60 | 2.21 | 2.33 | 2.60 | 1.55 | 0.54 | 0.41
orbit    |  7.63 |  4.57 | 4.21 | 0.01 | 0.24 | 0.45 | 0.46 | 0.03 | 0.02 | 0.59 | 0.32 | 0.03
Against 21d round 2: within ~0.5 ms at the ground stops except orbit 11.85 -> 7.63 (scene: terrain 8.40 -> 4.21, terrain
5de4f33e "detail from orbit stops at what the screen can show") and field 21.43 -> 23.41 (world reflections 0.08 ->
2.60 in that one profile; round 1 field had 0.02). Other rows over 0.3 ms (sorted lists in passes.txt): scene: grass
2.57 at the market, 1.04 at the fortress; prepass: meshes 1.0-2.6; compute meshes 2.80 at overview round 1 only (the
start); occlusion culling 0.06-0.16.
Lake extra view (round 1 snapshot, extras.txt): render.extra_views 1, 10.28 ms CPU, 4.95 ms GPU (21d: 8.94 / 5.35).

Files: tables.txt, passes.txt, extras.txt (physics_simulate per profile, terrain/extra-view snapshots),
physics_reports.txt (all 97 engine_physics::report blocks, each with the stop it falls in), summary.json, frames.csv,
timeline.txt, nvidia_dmon.txt, jpg/ (round 1 stops), logs.zip (stderr with every profile, engine-profile.txt, stats
JSON per stop).
