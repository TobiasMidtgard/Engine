Step 4: re-measure main (aac391d4) on Tobias's PC (RTX 5080, 32 threads, 64 GB), release build, window as the exe
opens it (client 1600x900, not maximized).

Loops: showcase.exe --loops overview,plaza,market,forest,lake,fortress,field,orbit --profile --quality ultra|high
(no :rounds, so the loop's default 5 rounds; round 21 ran :1). Per stop: the last 20 s of its 25 s dwell.
- fps and frame ms: exact, from engine.status polled twice a second (frame counter and real elapsed time).
- 1% low: from those 0.5 s samples (each frame in a sample gets the sample's mean), so it is smoothed - not
  per-frame like round 21's (which polled every frame and cost 7-14 ms a frame in bridge_frame_start_system).
- worst frame: the worst of the engine's own 5 s frame profiles inside the window (exact).
- render_system, systems busy / one at a time, cores, ran alone: the last frame profile in the window.
- GPU ms: the last render perf line in the window; GPU busy: nvidia-smi dmon sm% over the window.
Round 1 took one screenshot and one engine.stats snapshot per stop at 20 s: that frame's
bridge_frame_start_system took 70-98 ms (the 2 Hz status polls alone don't show), so round 1's worst frames
include it; rounds 2-5 have no screenshots or stats calls (except High round 2's overview picture).
overview* in round 1: on main the first frames spend ~10.7 s in engine::stream_system (schedule Last) loading
27 cells at once (paddock alone 3.9 s), so overview's first dwell is mostly startup. It also trips the
engine's watchdog ("frame 1 has not finished for 20 s"). Its Ultra picture is missing for that reason; the
High run took it in round 2.

GI: showcase.exe --fly --profile --quality ultra, showcase.goto plaza, window 5-25 s after the jump, picture
after the window; GI default vs ENGINE_PROBE_GI=off.

plaza --fly, ENGINE_PROBE_GI default: fps 10,5, 1% low (0.5 s samples) 6,2, worst 142,1 ms, frame 95,2 ms, systems busy / one at a time 96.9 / 79.4 ms, render_system 48.21 (max 54.46), GPU 26.5 ms, GPU busy 28%, cores 1.08; top 5 ran alone: render::render_system 48.21, softbody::step_bodies 2.89, water::boat_air_system 2.40, smoke::step 2.36, movement::step_movers 2.25
plaza --fly, ENGINE_PROBE_GI =off: fps 29,6, 1% low (0.5 s samples) 11,4, worst 80,2 ms, frame 33,8 ms, systems busy / one at a time 28.7 / 21.3 ms, render_system 7.75 (max 10.77), GPU 24.1 ms, GPU busy 74%, cores 1.08; top 5 ran alone: render::render_system 7.75, script::run_scripts 1.26, softbody::step_bodies 0.91, water::boat_air_system 0.75, smoke::step 0.71

=== ULTRA (main aac391d4), round 1 (last 20 s of each 25 s dwell; * = first stop after the world built)
stop | fps | 1% low (0.5 s samples) | worst frame ms (5 s profiles) | frame ms | systems busy / one at a time ms | render_system ms (max) | GPU ms | GPU busy % | cores busy | top 5 ran alone (ms/frame)
overview* | 7.2 | 4.0 | 13,042.8 | 138 | 335.5 / 215.9 | 49.12 (max 88.15) | 21.9 | 6 | 1.06 | render::render_system 49.12, smoke::run_demos 40.06, water::ripple_system 24.75, physics::physics_sync 23.30, physics::physics_simulate 12.32
plaza | 11.4 | 7.3 | 170.0 | 88 | 94.4 / 79.2 | 51.86 (max 84.64) | 26.6 | 33 | 1.03 | render::render_system 51.86, physics::physics_simulate 4.59, softbody::step_beside_physics 2.81, water::boat_air_system 2.44, script::run_scripts 1.43
market | 10.9 | 9.1 | 187.9 | 92 | 103.2 / 77.3 | 45.21 (max 50.51) | 34.3 | 42 | 1.11 | render::render_system 45.21, softbody::step_beside_physics 5.61, physics::physics_simulate 2.34, water::boat_air_system 2.31, script::run_scripts 1.55
forest | 10.5 | 9.3 | 216.3 | 95 | 102.2 / 79.6 | 42.99 (max 51.23) | 28.5 | 30 | 1.08 | render::render_system 42.99, physics::physics_simulate 10.45, physics::physics_sync 2.73, water::boat_air_system 2.40, npc::rebuild_world_model 2.15
lake | 10.1 | 8.6 | 179.3 | 99 | 119.8 / 85.3 | 55.74 (max 59.57) | 26.0 | 35 | 1.21 | render::render_system 55.74, physics::physics_simulate 4.86, softbody::step_bodies 3.42, water::boat_air_system 2.61, script::run_scripts 1.67
fortress | 10.0 | 8.8 | 186.2 | 100 | 118.7 / 82.2 | 46.74 (max 52.52) | 28.9 | 30 | 1.20 | render::render_system 46.74, physics::physics_simulate 5.22, destruction::build_system 3.58, water::flow_bed_system 2.66, physics::physics_sync 2.21
field | 12.3 | 9.2 | 165.1 | 81 | 87.0 / 70.9 | 43.05 (max 48.94) | 28.1 | 38 | 1.07 | render::render_system 43.05, physics::physics_simulate 4.26, water::boat_air_system 2.21, script::run_scripts 1.56, ai_bridge::bridge_frame_start_system 1.24
orbit | 17.3 | 14.8 | 110.3 | 58 | 59.6 / 54.6 | 39.72 (max 42.80) | 13.7 | 26 | 1.01 | render::render_system 39.72, water::boat_air_system 1.67, smoke::step 1.40, script::run_scripts 1.26, movement::step_movers 0.80
=== ULTRA (main aac391d4), rounds 2-5 per stop (median over the rounds; worst = the worst of them)
stop | rounds | fps median (min-max) | 1% low median | worst frame ms | frame ms median | render_system ms median | GPU ms median | GPU busy % median | cores median
overview | 4 | 14.9 (14.4-15.4) | 10.0 | 119.2 | 67 | 43.5 | 37.4 | 53 | 1.01
plaza | 4 | 11.2 (10.9-11.6) | 9.0 | 346.7 | 89 | 47.1 | 25.1 | 29 | 1.05
market | 4 | 10.6 (10.6-10.7) | 9.5 | 183.9 | 94 | 45.8 | 30.9 | 34 | 1.14
forest | 4 | 10.3 (10.1-10.8) | 8.9 | 383.4 | 97 | 43.9 | 28.1 | 30 | 1.06
lake | 4 | 9.8 (9.8-10.1) | 8.8 | 150.7 | 102 | 55.5 | 26.4 | 36 | 1.17
fortress | 4 | 8.8 (8.5-9.6) | 7.4 | 175.0 | 114 | 45.6 | 28.9 | 26 | 1.16
field | 4 | 11.7 (11.5-11.9) | 9.7 | 153.4 | 85 | 43.8 | 28.3 | 34 | 1.07
orbit | 4 | 16.9 (16.8-17.3) | 14.7 | 115.6 | 59 | 39.8 | 13.7 | 25 | 1.00
=== HIGH (main aac391d4), round 1 (last 20 s of each 25 s dwell; * = first stop after the world built)
stop | fps | 1% low (0.5 s samples) | worst frame ms (5 s profiles) | frame ms | systems busy / one at a time ms | render_system ms (max) | GPU ms | GPU busy % | cores busy | top 5 ran alone (ms/frame)
overview* | 7.7 | 4.0 | 13,006.5 | 130 | 369.2 / 217.8 | 49.19 (max 94.20) | 10.4 | 6 | 1.11 | render::render_system 49.19, smoke::run_demos 44.04, water::ripple_system 30.77, physics::physics_sync 24.75, physics::physics_simulate 10.04
plaza | 12.9 | 8.8 | 167.3 | 77 | 80.5 / 72.8 | 51.83 (max 79.04) | 24.9 | 34 | 1.01 | render::render_system 51.83, physics::physics_simulate 2.91, water::boat_air_system 2.23, script::run_scripts 1.67, softbody::step_beside_physics 1.65
market | 14.1 | 9.9 | 178.3 | 71 | 74.8 / 62.9 | 44.70 (max 52.01) | 32.5 | 48 | 1.05 | render::render_system 44.70, water::boat_air_system 1.98, physics::physics_simulate 1.98, script::run_scripts 1.37, softbody::gather_colliders 0.86
forest | 12.3 | 10.1 | 159.0 | 81 | 81.1 / 70.8 | 42.89 (max 46.86) | 26.2 | 33 | 1.05 | render::render_system 42.89, physics::physics_simulate 10.88, water::boat_air_system 2.17, script::run_scripts 1.59, physics::physics_sync 1.03
lake | 11.4 | 8.8 | 178.0 | 88 | 93.8 / 79.4 | 55.35 (max 64.97) | 25.4 | 37 | 1.08 | render::render_system 55.35, physics::physics_simulate 4.55, softbody::step_bodies 2.45, water::boat_air_system 2.39, ai_bridge::bridge_frame_start_system 1.41
fortress | 11.0 | 9.2 | 165.2 | 91 | 109.7 / 75.5 | 45.25 (max 50.46) | 28.1 | 33 | 1.21 | render::render_system 45.25, physics::physics_simulate 2.93, npc::rebuild_world_model 2.71, physics::physics_sync 2.22, water::boat_air_system 1.84
field | 13.7 | 11.1 | 170.5 | 73 | 78.1 / 65.6 | 44.44 (max 49.63) | 27.9 | 47 | 1.06 | render::render_system 44.44, physics::physics_simulate 3.21, water::boat_air_system 2.06, script::run_scripts 1.52, ai_bridge::bridge_frame_start_system 1.25
orbit | 17.7 | 15.7 | 108.2 | 56 | 56.0 / 51.8 | 39.68 (max 44.20) | 13.6 | 24 | 0.98 | render::render_system 39.68, water::boat_air_system 1.58, script::run_scripts 1.18, ai_bridge::bridge_frame_start_system 0.56, movement::step_movers 0.49
=== HIGH (main aac391d4), rounds 2-5 per stop (median over the rounds; worst = the worst of them)
stop | rounds | fps median (min-max) | 1% low median | worst frame ms | frame ms median | render_system ms median | GPU ms median | GPU busy % median | cores median
overview | 4 | 15.0 (14.6-15.6) | 9.8 | 168.1 | 67 | 43.4 | 35.6 | 55 | 1.01
plaza | 4 | 12.6 (12.3-13.1) | 9.4 | 133.6 | 79 | 47.2 | 23.6 | 32 | 1.01
market | 4 | 12.9 (12.6-13.8) | 11.0 | 133.1 | 77 | 44.6 | 29.0 | 40 | 1.04
forest | 4 | 10.7 (9.6-11.3) | 9.3 | 170.7 | 93 | 43.6 | 26.4 | 29 | 1.04
lake | 4 | 10.5 (10.1-10.8) | 9.0 | 152.4 | 96 | 54.7 | 25.8 | 37 | 1.06
fortress | 4 | 11.2 (10.1-11.4) | 9.4 | 152.9 | 90 | 45.1 | 28.2 | 32 | 1.16
field | 4 | 13.5 (12.8-13.8) | 11.6 | 129.6 | 74 | 44.2 | 27.9 | 39 | 1.06
orbit | 4 | 17.4 (17.1-17.6) | 15.4 | 112.3 | 57 | 40.2 | 13.5 | 25 | 1.00
