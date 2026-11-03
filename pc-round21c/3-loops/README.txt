Round 21c step 3: re-measure main 145dd3a6 on Tobias's PC (RTX 5080, 32 threads, 64 GB), release build, window as the exe
opens it (client 1600x900, not maximized). Same method as round 21b step 4 (main aac391d4), 2 rounds instead of 5.

showcase.exe --loops overview,plaza,market,forest,lake,fortress,field,orbit:2 --profile --quality ultra|high
Per stop: the last 20 s of its 25 s dwell.
- fps and frame ms: exact, from engine.status polled twice a second (frame counter and real elapsed time).
- 1% low: from those 0.5 s samples (each frame in a sample gets the sample's mean), so it is smoothed.
- worst frame: the worst of the engine's own 5 s frame profiles inside the window (exact).
- render_system, systems busy / one at a time, cores, ran alone: the last frame profile in the window.
- GPU ms: the last render perf line in the window; GPU busy: nvidia-smi dmon sm% over the window.
  Ultra round 2 orbit has no GPU busy: nvidia-smi had not flushed its file when it was stopped (fixed for High on).
Round 1 took one screenshot plus one render and one terrain engine.stats snapshot per stop at 20 s (overview's in
round 2): "screenshot save" costs that frame 60-130 ms of render steps, so round 1's worst frames include it.
Round 2 has no screenshots or stats calls except overview's.

Dwell clock: the tool can't start it after the first frames' loading. loops.rs starts each 25 s dwell (DWELL_S, fixed)
at the jump, and the first jump happens before frame 1 (13 s at Ultra, the renderer's start) and the loading frames
after it, so overview round 1 is mostly startup (Ultra: frames running from 11 s before its loop line; High: 8 s).
Read round 2 for overview.

softbody yard: "made 27 materials (62.0 MB of textures) in 0.32 s" in both runs (21b: 7.4 s).
=== ULTRA (main 145dd3a6), round 1 of 2 (last 20 s of each 25 s dwell; * = first stop, its dwell started before the first frames' loading)
stop | fps | 1% low (0.5 s samples) | worst frame ms (5 s profiles) | frame ms | systems busy / one at a time ms | render_system ms (max) | GPU ms | GPU busy % | cores busy | top 5 ran alone (ms/frame)
overview* | 4.4 | 4.0 | 3,891.1 | 226 | 1242.4 / 706.1 | 284.37 (max 1568.38) | 28.3 | 6 | 1.27 | render::render_system 284.37, npc::build_stand_ins 171.70, water::ripple_system 47.64, physics::physics_sync 30.55, water::shelter_system 27.52
plaza | 28.2 | 17.7 | 161.4 | 35 | 30.8 / 23.9 | 9.44 (max 19.12) | 26.3 | 77 | 1.03 | render::render_system 9.44, script::run_scripts 1.16, physics::physics_simulate 1.07, softbody::step_bodies 0.92, softbody::step_beside_physics 0.91
market | 26.6 | 14.4 | 276.3 | 38 | 34.5 / 23.7 | 8.09 (max 17.47) | 31.4 | 83 | 0.92 | render::render_system 8.09, softbody::step_beside_physics 2.17, softbody::step_bodies 1.00, script::run_scripts 0.99, water::boat_air_system 0.90
forest | 28.8 | 16.1 | 203.5 | 35 | 33.5 / 24.8 | 9.77 (max 17.62) | 29.1 | 84 | 1.10 | render::render_system 9.77, physics::physics_simulate 2.39, script::run_scripts 1.25, water::boat_air_system 0.92, ai_bridge::bridge_frame_start_system 0.88
lake | 22.5 | 16.4 | 151.2 | 44 | 51.9 / 35.3 | 20.16 (max 23.81) | 27.0 | 90 | 1.19 | render::render_system 20.16, softbody::step_bodies 1.40, script::run_scripts 1.34, water::boat_air_system 1.22, physics::physics_simulate 0.99
fortress | 18.7 | 14.0 | 172.6 | 54 | 74.6 / 48.2 | 15.91 (max 19.55) | 28.9 | 55 | 1.22 | render::render_system 15.91, destruction::build_system 7.45, water::flow_bed_system 2.89, script::run_scripts 1.42, water::boat_air_system 1.32
field | 30.8 | 24.4 | 189.3 | 33 | 34.5 / 26.6 | 11.51 (max 17.95) | 28.9 | 92 | 1.06 | render::render_system 11.51, script::run_scripts 1.33, physics::physics_simulate 1.03, water::boat_air_system 0.99, ai_bridge::bridge_frame_start_system 0.86
orbit | 54.1 | 44.6 | 73.8 | 18 | 18.4 / 15.9 | 8.34 (max 12.49) | 13.5 | 79 | 0.99 | render::render_system 8.34, script::run_scripts 1.08, water::boat_air_system 0.59, core::propagate_transforms_system 0.32, core::frame_diagnostics_system 0.30
=== ULTRA (main 145dd3a6), round 2 of 2 (last 20 s of each 25 s dwell)
stop | fps | 1% low (0.5 s samples) | worst frame ms (5 s profiles) | frame ms | systems busy / one at a time ms | render_system ms (max) | GPU ms | GPU busy % | cores busy | top 5 ran alone (ms/frame)
overview | 25.1 | 17.8 | 239.5 | 40 | 40.4 / 36.1 | 23.11 (max 31.09) | 38.6 | 100 | 1.00 | render::render_system 23.11, ai_bridge::bridge_frame_start_system 1.65, script::run_scripts 1.39, water::boat_air_system 0.84, physics::physics_simulate 0.78
plaza | 35.6 | 30.0 | 76.9 | 28 | 28.9 / 23.0 | 11.10 (max 17.39) | 24.4 | 88 | 1.04 | render::render_system 11.10, physics::physics_simulate 1.41, script::run_scripts 1.21, water::boat_air_system 0.82, softbody::gather_colliders 0.42
market | 29.2 | 22.6 | 80.5 | 34 | 39.5 / 27.1 | 10.46 (max 22.39) | 31.1 | 91 | 1.12 | render::render_system 10.46, softbody::step_beside_physics 2.35, script::run_scripts 1.31, water::boat_air_system 1.03, softbody::gather_colliders 0.62
forest | 26.2 | 15.8 | 87.3 | 38 | 54.1 / 39.6 | 10.59 (max 16.55) | 28.0 | 78 | 1.10 | render::render_system 10.59, physics::physics_simulate 5.80, physics::physics_sync 2.70, water::flow_bed_system 2.60, npc::rebuild_world_model 2.00
lake | 21.2 | 16.3 | 94.5 | 47 | 63.6 / 44.1 | 24.89 (max 28.87) | 26.9 | 86 | 1.39 | render::render_system 24.89, physics::physics_simulate 2.45, softbody::step_bodies 1.88, script::run_scripts 1.52, water::boat_air_system 1.48
fortress | 15.1 | 11.8 | 120.9 | 66 | 81.3 / 52.2 | 17.63 (max 21.02) | 29.9 | 50 | 1.27 | render::render_system 17.63, destruction::build_system 8.75, water::flow_bed_system 3.39, smoke::step 2.84, script::run_scripts 1.42
field | 28.6 | 24.4 | 86.3 | 35 | 36.6 / 28.3 | 13.49 (max 18.63) | 29.3 | 84 | 1.07 | render::render_system 13.49, script::run_scripts 1.38, water::boat_air_system 1.03, physics::physics_simulate 0.91, smoke::step 0.79
orbit | 48.4 | 42.8 | 69.6 | 21 | 19.9 / 17.2 | 9.27 (max 13.86) | 13.6 | n/a | 0.99 | render::render_system 9.27, script::run_scripts 1.05, water::boat_air_system 0.63, smoke::step 0.39, core::propagate_transforms_system 0.32
=== HIGH (main 145dd3a6), round 1 of 2 (last 20 s of each 25 s dwell; * = first stop, its dwell started before the first frames' loading)
stop | fps | 1% low (0.5 s samples) | worst frame ms (5 s profiles) | frame ms | systems busy / one at a time ms | render_system ms (max) | GPU ms | GPU busy % | cores busy | top 5 ran alone (ms/frame)
overview* | 17.5 | 4.0 | 1,582.1 | 57 | 34.8 / 26.8 | 12.39 (max 31.83) | 36.5 | 43 | 0.91 | render::render_system 12.39, softbody::step_beside_physics 4.25, physics::physics_simulate 1.18, script::run_scripts 1.07, water::boat_air_system 0.73
plaza | 37.4 | 27.5 | 177.5 | 27 | 26.2 / 21.6 | 9.58 (max 20.03) | 25.4 | 91 | 1.01 | render::render_system 9.58, physics::physics_simulate 1.64, script::run_scripts 1.29, water::boat_air_system 0.80, ai_bridge::bridge_frame_start_system 0.67
market | 32.3 | 24.5 | 211.6 | 31 | 32.8 / 25.9 | 12.64 (max 21.12) | 29.4 | 100 | 1.06 | render::render_system 12.64, physics::physics_simulate 1.67, script::run_scripts 1.32, ai_bridge::bridge_frame_start_system 1.00, water::boat_air_system 0.93
forest | 35.7 | 25.7 | 184.5 | 28 | 29.5 / 24.0 | 13.07 (max 19.25) | 26.6 | 100 | 1.03 | render::render_system 13.07, physics::physics_simulate 1.48, script::run_scripts 1.10, water::boat_air_system 0.82, ai_bridge::bridge_frame_start_system 0.81
lake | 24.3 | 19.6 | 146.7 | 41 | 44.2 / 35.7 | 20.36 (max 25.19) | 26.3 | 93 | 1.09 | render::render_system 20.36, physics::physics_simulate 2.68, script::run_scripts 1.37, softbody::step_bodies 1.16, water::boat_air_system 1.14
fortress | 23.5 | 14.6 | 144.7 | 43 | 41.0 / 27.0 | 12.31 (max 18.62) | 29.1 | 68 | 1.18 | render::render_system 12.31, script::run_scripts 1.24, destruction::build_system 0.90, physics::physics_simulate 0.89, water::boat_air_system 0.77
field | 34.1 | 26.1 | 131.4 | 29 | 31.7 / 24.6 | 11.89 (max 21.17) | 27.9 | 100 | 1.07 | render::render_system 11.89, physics::physics_simulate 1.84, script::run_scripts 1.32, water::boat_air_system 0.91, ai_bridge::bridge_frame_start_system 0.49
orbit | 54.8 | 45.6 | 69.9 | 18 | 23.2 / 20.3 | 11.07 (max 14.87) | 13.5 | 79 | 1.18 | render::render_system 11.07, script::run_scripts 1.34, water::boat_air_system 0.73, movement::step_movers 0.51, core::propagate_transforms_system 0.39
=== HIGH (main 145dd3a6), round 2 of 2 (last 20 s of each 25 s dwell)
stop | fps | 1% low (0.5 s samples) | worst frame ms (5 s profiles) | frame ms | systems busy / one at a time ms | render_system ms (max) | GPU ms | GPU busy % | cores busy | top 5 ran alone (ms/frame)
overview | 26.1 | 17.5 | 241.9 | 38 | 38.0 / 33.9 | 21.09 (max 28.92) | 36.7 | 100 | 0.99 | render::render_system 21.09, ai_bridge::bridge_frame_start_system 1.51, script::run_scripts 1.36, physics::physics_simulate 1.09, water::boat_air_system 0.80
plaza | 39.8 | 36.7 | 88.0 | 25 | 30.5 / 24.9 | 10.46 (max 18.08) | 23.8 | 99 | 1.21 | render::render_system 10.46, physics::physics_simulate 2.09, script::run_scripts 1.58, water::boat_air_system 0.93, weather::ground_system 0.50
market | 33.1 | 29.2 | 356.7 | 30 | 39.7 / 31.8 | 14.83 (max 27.01) | 28.9 | 100 | 1.24 | render::render_system 14.83, physics::physics_simulate 1.94, script::run_scripts 1.59, water::shelter_system 1.53, water::boat_air_system 1.06
forest | 36.7 | 32.6 | 79.5 | 27 | 28.2 / 22.4 | 10.19 (max 35.59) | 25.7 | 100 | 1.05 | render::render_system 10.19, physics::physics_simulate 2.12, script::run_scripts 1.31, water::boat_air_system 0.80, physics::physics_sync 0.44
lake | 24.2 | 20.4 | 92.2 | 41 | 44.1 / 34.9 | 20.58 (max 27.32) | 26.3 | 92 | 1.08 | render::render_system 20.58, physics::physics_simulate 2.52, script::run_scripts 1.27, water::boat_air_system 1.11, softbody::step_bodies 1.06
fortress | 29.8 | 19.0 | 80.7 | 34 | 38.4 / 24.8 | 9.59 (max 15.95) | 28.0 | 91 | 1.20 | render::render_system 9.59, water::flow_bed_system 2.05, script::run_scripts 1.34, smoke::step 0.88, water::boat_air_system 0.77
field | 34.7 | 31.8 | 74.9 | 29 | 36.2 / 28.9 | 15.76 (max 22.73) | 27.7 | 100 | 1.25 | render::render_system 15.76, script::run_scripts 1.50, physics::physics_simulate 1.06, water::boat_air_system 1.01, movement::step_movers 0.58
orbit | 61.7 | 56.1 | 67.0 | 16 | 12.7 / 10.9 | 5.21 (max 12.06) | 13.6 | 89 | 0.80 | render::render_system 5.21, script::run_scripts 0.89, water::boat_air_system 0.39, movement::step_movers 0.38, core::frame_diagnostics_system 0.25

=== Round 2 against 21b (aac391d4, rounds 2-5 medians): fps
stop | Ultra 21b | Ultra 21c r2 | High 21b | High 21c r2
overview | 14.9 | 25.1 | 15.0 | 26.1
plaza | 11.2 | 35.6 | 12.6 | 39.8
market | 10.6 | 29.2 | 12.9 | 33.1
forest | 10.3 | 26.2 | 10.7 | 36.7
lake | 9.8 | 21.2 | 10.5 | 24.2
fortress | 8.8 | 15.1 | 11.2 | 29.8
field | 11.7 | 28.6 | 13.5 | 34.7
orbit | 16.9 | 48.4 | 17.4 | 61.7
render_system went from 40-56 ms a frame (21b) to 8-25 ms. Most stops are now GPU-bound: GPU 24-31 ms a frame
(37-39 ms at overview, 13.5 at orbit) with the GPU 77-100% busy, and the GPU time is nearly the same at High as at Ultra.
Ultra fortress is the CPU-bound exception: systems busy 75-81 ms, destruction::build_system 7.5-8.8 ms ran alone,
GPU 50-55% busy. Lake's render_system (20-25 ms vs 8-13 elsewhere) carries the extra view (below).

=== Extras-- physics_simulate in each 5 s frame profile inside the stop's window (ms/frame, max ms, runs/frame)
round 1 overview : 1.35 / 2.18 / 3.2 | 3.94 / 8.72 / 1.2
round 1 plaza    : 2.38 / 2.57 / 2.2 | 2.85 / 3.22 / 2.3 | 2.22 / 2.23 / 1.7
round 1 market   : 3.40 / 2.98 / 2.2 | 3.93 / 2.58 / 2.6 | 2.57 / 2.38 / 1.8
round 1 forest   : 5.80 / 5.57 / 2.3 | 4.17 / 6.72 / 1.9 | 3.31 / 4.23 / 1.8
round 1 lake     : 5.07 / 2.95 / 2.7 | 4.91 / 3.42 / 2.6 | 4.97 / 3.32 / 2.6
round 1 fortress : 3.71 / 2.52 / 3.0 | 4.83 / 3.38 / 3.1 | 5.33 / 2.95 / 3.5
round 1 field    : 2.24 / 2.03 / 1.9 | 1.91 / 1.77 / 2.0 | 2.30 / 2.39 / 1.9
round 1 orbit    : 0.18 / 0.60 / 1.1 | 0.19 / 0.45 / 1.1 | 0.18 / 0.44 / 1.1
round 2 overview : 1.23 / 1.10 / 2.4 | 1.25 / 1.16 / 2.4 | 1.24 / 1.28 / 2.4
round 2 plaza    : 2.20 / 1.96 / 1.7 | 2.08 / 2.37 / 1.6 | 2.14 / 2.35 / 1.7
round 2 market   : 2.48 / 2.30 / 2.0 | 3.01 / 2.74 / 2.1 | 2.83 / 4.57 / 2.1
round 2 forest   : 2.48 / 2.56 / 1.9 | 5.26 / 20.42 / 2.7 | 7.46 / 6.67 / 2.8
round 2 lake     : 5.50 / 3.87 / 2.7 | 5.74 / 3.49 / 2.7 | 6.90 / 3.14 / 3.3
round 2 fortress : 5.91 / 2.82 / 3.7 | 6.14 / 3.18 / 3.7 | 5.56 / 3.60 / 3.7
round 2 field    : 2.34 / 2.20 / 2.2 | 2.37 / 1.86 / 2.0 | 2.26 / 1.70 / 2.1
round 2 orbit    : 0.23 / 0.54 / 1.5 | 0.16 / 1.02 / 1.0 | 0.19 / 0.52 / 1.2
-- softbody yard
engine_softbody::demo::kit: softbody yard: made 27 materials (62.0 MB of textures) in 0.32 s
-- per stop (engine.stats at 20 s into round 1's dwell; overview's in round 2)
plaza    : prepare terrain 2.15 ms (inside: buffers 0.02, maps 0.12, patches 0.99); GPU scene: terrain 5.35 ms; extra views  (0.00 ms CPU, 0.00 ms GPU); patches drawn 700.0, resident 1593.0, building 0.0, missing 0.0
market   : prepare terrain 0.68 ms (inside: buffers 0.02, maps 0.01, patches 0.86); GPU scene: terrain 9.73 ms; extra views  (0.00 ms CPU, 0.00 ms GPU); patches drawn 725.0, resident 1601.0, building 0.0, missing 0.0
forest   : prepare terrain 0.98 ms (inside: buffers 0.02, maps 0.06, patches 0.76); GPU scene: terrain 8.80 ms; extra views  (0.00 ms CPU, 0.00 ms GPU); patches drawn 702.0, resident 1577.0, building 0.0, missing 0.0
lake     : prepare terrain 0.55 ms (inside: buffers 0.02, maps 0.01, patches 0.71); GPU scene: terrain 12.09 ms; extra views 1.0 (10.85 ms CPU, 11.37 ms GPU); patches drawn 701.0, resident 1600.0, building 0.0, missing 0.0
fortress : prepare terrain 0.62 ms (inside: buffers 0.02, patches 0.51); GPU scene: terrain 11.10 ms; extra views 1.0 (10.98 ms CPU, 11.40 ms GPU); patches drawn 713.0, resident 1630.0, building 0.0, missing 0.0
field    : prepare terrain 0.55 ms (inside: buffers 0.02, maps 1.09, patches 0.50); GPU scene: terrain 8.75 ms; extra views 1.0 (10.98 ms CPU, 11.40 ms GPU); patches drawn 721.0, resident 1616.0, building 0.0, missing 0.0
orbit    : prepare terrain 0.45 ms (inside: buffers 0.02, maps 0.01, patches 0.39); GPU scene: terrain 9.26 ms; extra views 1.0 (10.98 ms CPU, 11.40 ms GPU); patches drawn 698.0, resident 1185.0, building 0.0, missing 0.0
overview : prepare terrain 0.58 ms (inside: buffers 0.02, far looks 0.01, maps 0.01, patches 0.57); GPU scene: terrain 21.39 ms; extra views 1.0 (10.98 ms CPU, 11.40 ms GPU); patches drawn 699.0, resident 1651.0, building 0.0, missing 0.0
=== HIGH
-- physics_simulate in each 5 s frame profile inside the stop's window (ms/frame, max ms, runs/frame)
round 1 overview : 11.02 / 37.50 / 1.2 | 2.34 / 16.81 / 2.6 | 3.72 / 6.37 / 2.0
round 1 plaza    : 2.03 / 1.92 / 1.7 | 2.15 / 2.68 / 1.5 | 2.09 / 2.36 / 1.6
round 1 market   : 2.53 / 2.16 / 1.8 | 2.88 / 2.53 / 1.8 | 2.71 / 5.03 / 1.8
round 1 forest   : 2.58 / 3.92 / 2.0 | 1.92 / 3.16 / 1.4 | 2.02 / 2.12 / 1.7
round 1 lake     : 4.05 / 2.77 / 2.5 | 3.87 / 2.92 / 2.4 | 3.97 / 2.30 / 2.5
round 1 fortress : 11.10 / 7.72 / 3.3 | 5.69 / 7.06 / 2.6 | 5.32 / 7.24 / 2.4 | 3.50 / 3.12 / 2.1
round 1 field    : 2.81 / 3.15 / 1.7 | 2.94 / 4.99 / 1.7 | 3.01 / 2.86 / 1.8 | 2.97 / 3.29 / 1.8
round 1 orbit    : 0.16 / 0.48 / 1.0 | 0.16 / 0.60 / 1.1 | 0.16 / 0.46 / 1.1 | 0.20 / 0.61 / 1.4
round 2 overview : 1.94 / 3.21 / 2.3 | 1.40 / 2.33 / 2.3 | 1.78 / 1.79 / 2.3 | 1.55 / 1.55 / 2.2
round 2 plaza    : 2.18 / 2.37 / 1.5 | 2.54 / 2.67 / 1.8 | 1.85 / 2.79 / 1.2 | 2.64 / 2.50 / 1.8
round 2 market   : 2.59 / 2.30 / 1.8 | 2.80 / 3.09 / 1.8 | 2.66 / 2.56 / 1.8 | 3.11 / 2.99 / 2.2
round 2 forest   : 2.29 / 2.39 / 1.6 | 2.37 / 2.57 / 1.6 | 2.67 / 3.09 / 1.6
round 2 lake     : 4.16 / 2.37 / 2.6 | 4.17 / 2.16 / 2.5 | 3.94 / 2.49 / 2.4
round 2 fortress : 2.17 / 2.57 / 1.8 | 2.70 / 5.37 / 2.1 | 2.54 / 6.54 / 1.9
round 2 field    : 1.56 / 1.78 / 1.8 | 1.68 / 1.99 / 1.7 | 1.99 / 1.71 / 2.1
round 2 orbit    : 0.15 / 0.33 / 1.0 | 0.20 / 0.64 / 1.2 | 0.12 / 0.38 / 0.8
-- softbody yard
engine_softbody::demo::kit: softbody yard: made 27 materials (62.0 MB of textures) in 0.32 s
-- per stop (engine.stats at 20 s into round 1's dwell; overview's in round 2)
plaza    : prepare terrain 2.14 ms (inside: buffers 0.02, maps 0.06, patches 0.98); GPU scene: terrain 5.15 ms; extra views  (0.00 ms CPU, 0.00 ms GPU); patches drawn 720.0, resident 1609.0, building 0.0, missing 0.0
market   : prepare terrain 0.65 ms (inside: buffers 0.03, maps 1.09, patches 0.55); GPU scene: terrain 9.85 ms; extra views  (0.00 ms CPU, 0.00 ms GPU); patches drawn 725.0, resident 1585.0, building 0.0, missing 0.0
forest   : prepare terrain 1.11 ms (inside: buffers 0.02, maps 0.01, patches 0.99); GPU scene: terrain 8.57 ms; extra views  (0.00 ms CPU, 0.00 ms GPU); patches drawn 702.0, resident 1563.0, building 0.0, missing 0.0
lake     : prepare terrain 0.54 ms (inside: buffers 0.02, maps 0.01, patches 0.74); GPU scene: terrain 11.97 ms; extra views 1.0 (10.83 ms CPU, 10.40 ms GPU); patches drawn 704.0, resident 1580.0, building 0.0, missing 0.0
fortress : prepare terrain 0.54 ms (inside: buffers 0.02, maps 0.01, patches 0.49); GPU scene: terrain 11.25 ms; extra views 1.0 (13.47 ms CPU, 10.20 ms GPU); patches drawn 713.0, resident 1610.0, building 0.0, missing 0.0
field    : prepare terrain 0.55 ms (inside: buffers 0.02, maps 0.01, patches 0.47); GPU scene: terrain 8.81 ms; extra views 1.0 (13.47 ms CPU, 10.20 ms GPU); patches drawn 721.0, resident 1596.0, building 0.0, missing 0.0
orbit    : prepare terrain 0.42 ms (inside: buffers 0.03, patches 0.41); GPU scene: terrain 9.43 ms; extra views 1.0 (13.47 ms CPU, 10.20 ms GPU); patches drawn 698.0, resident 1185.0, building 0.0, missing 0.0
overview : prepare terrain 2.17 ms (inside: buffers 0.02, maps 0.01, patches 1.02); GPU scene: terrain 20.57 ms; extra views 1.0 (13.47 ms CPU, 10.20 ms GPU); patches drawn 702.0, resident 1631.0, building 0.0, missing 0.0

Notes on the extras:
- forest physics_simulate (was ~11 ms a frame ran alone in 21b, 10.45 Ultra / 10.88 High): now 3.3-5.8 ms a frame at
  Ultra round 1 and 2.5-7.5 in round 2 (one 5 s profile had a 20.4 ms step), 1.9-2.7 ms at High; about 2 steps a frame.
  Ran alone in the last profile: Ultra 2.39 (r1) / 5.80 (r2), High 1.48 / 2.12.
- render.extra_views_ms / _gpu_ms at the lake: Ultra 10.85 ms CPU, 11.37 ms GPU; High 10.83 ms CPU, 10.40 ms GPU
  (one extra view). The values at fortress, field, orbit and overview are stale: identical at every stop after the lake
  (Ultra 10.98 / 11.40, High 13.47 / 10.20). renderer.rs render_extra_views returns early when no extra view is active,
  before it sets render.extra_views*, so the last value sticks (render.extra_views also stays 1).
- render.cpu.terrain: "prepare terrain" 0.4-2.2 ms per stop (inside it "patches" 0.4-1.0 ms, "maps" up to 1.1 ms),
  terrain patches building 0 and missing 0 at every stop at 20 s.
