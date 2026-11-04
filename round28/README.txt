Round 28 on Tobias's PC (RTX 5080, 32 threads, 64 GB), release, Ultra, window 1600x900 as the exe opens it, real time,
every showcase run with ENGINE_PROFILE=1 and --profile, plain (no ENGINE_EXECUTOR). Both runs on main 6da6a069
("Destruction: a builder's aim marches the ground by height only"; has cb045bd7 "spans inside a builder's plan" and
Rendering #592 26aedf55 "mesh scene sync visits only what changed"; NOT Physics #591 "load-time collider syncs off the
frame, shared shapes, collider audit", which is still open, so no PHYSICS_REPORT run). No asset manifest changes since
round 27's ced07d57, so no fetch-assets. Build at below-normal priority with nothing else running, 08:21:51-08:24:54
UTC, 3 m 02 s (build.txt). Each game run started only after 5+ min with no input (36,104 and 36,224 s), one at a time,
and ended with taskkill /IM showcase.exe /F. Input only through the game's flags, environment and bridge. Runs
08:25-08:30 UTC, 2026-11-04.
2-fortress           --fly, showcase.goto fortress, 60 s stand, a 60-frame engine.trace 20 s after arriving
3-4-plaza-lake       --fly, showcase.goto plaza, 75 s stand (render.cpu. dumps every 5 s, a 120-frame trace 40 s in),
                     then from the plaza: engine.trace 20 frames, and the moment it was answered showcase.goto lake (the
                     bridge runs requests at a frame's start and a trace records from the end of the frame that asked,
                     so the goto's frame is the trace's frame 0), then engine.stats "render.gi" back to back, 20 answers
                     on 20 consecutive frames (arrival_stats.jsonl), then 15 s at the lake and a picture
fortress-r28-r27.txt ab28.py on this fortress stand and round 27's (25-60 s after arriving)
plaza-r28-r27-r26.txt ab28.py on this plaza stand, round 27 a1 and round 26 a (25-75 s)
scripts              run28.ps1 = run27.ps1 plus the "arrive-<place>:secs" stop; ab28.py = ab27.py with --place;
                     arrive28.py (the arrival's answers and trace frames); the rest are round 26-27's
Traces are gzip'd Chrome traces (scripts/trace-summary.py reads .json.gz), 0.4-1.8 MB; trace-summary.py --frames --worst
3 output is beside each (traces/*_summary.txt).

Summary
2) Fortress 60 s stand (25-60 s after arriving; round 27 b, the same stand on ced07d57, in brackets):
   fps 31.3 (21.4); frame 32.35 ms (47.30); fixed steps a frame 1.91 (2.78)
   FixedUpdate 14.85 ms a frame (21.30), 1.93 runs (2.81), 7.66 a run (7.54), floor 8.01 (11.97)
   Update 6.31 ms a frame (13.05), floor 3.36 (9.96); Render 6.26 (7.70); PostUpdate 3.36 (3.67)
   one system at a time 22.43 ms (35.26), no system 2.65 (3.30), several at once 7.30 (8.74); cores 1.24 (1.22)
   per 5 s: 13.4, 12.3, 15.4, 18.3, 29.6, 31.9, 28.5, 34.2, 33.0, 27.5, 29.9, 32.9 fps
   Trace (60 frames from 20.2 s; 37.64 ms a frame, 26.6 fps; round 27's 47.86): no system 3.10, one system 28.12,
   several 6.41; 1.17 threads busy. FixedUpdate 19.68 ms a frame (2.22 runs), Render 7.01, Update 5.42, PostUpdate 3.93.
   Ran alone (ms a frame): render_system 6.94 (r27 10.24), npc rebuild_world_model 1.98 (2.33), think_and_act 1.57
   (1.71), run_scripts 1.36 (1.44), destruction simulate_debris 1.25 (1.48), smoke::step 1.04 (1.11), build_system 1.03
   (3.64), physics_sync 0.78, terrain update_colliders 0.60, puddle_system 0.55, apply_hits 0.53, step_movers 0.51.
   Inside engine_destruction::build_system (traces/trace_fortress_inner.txt; spans starting during its runs):
     build_system                    1.04 ms a frame, 1 run, max 2.51      (round 27: 3.65, max 6.51)
     build: builders                 1.03, max 2.49                        (3.64, max 6.50)
     build: plan                     0.97 ms a frame, 2 runs, 0.49 a run, max 0.91   (3.43, 2 runs, 1.71 a run)
     build: aim at ground and rock   0.96 ms a frame, 2.08 runs, 0.46 a run, max 0.87  (new span)
     build: snap and support         0.022 ms a frame, 1.08 runs, max 0.04  (new)
     build: occupied                 0.029 ms a frame, 1.08 runs, max 0.06  (new)
     build: pieces near 0.008, ghosts and support 0.001 (as round 27)
     Per frame (the file's table) build: plan sums to 0.72-1.69 ms (median 0.83, p90 1.56); nearly all of it is the aim.
   So the builders cost 2.6 ms a frame less, and render_system 3.3 ms less (the only render change since round 27 is
   #592); shorter frames then need fewer fixed runs (2.80 -> 2.22 a frame in the traces, 2.81 -> 1.93 in the profiles;
   a run costs about the same, 8.50 -> 8.86 ms in the traces), which is most of FixedUpdate's drop.
3) Plaza 75 s stand, 25-75 s after arriving (round 27 a1 / round 26 a, the same stand, in brackets):
   fps 49.9 (50.7 / 51.2); frame 20.00 ms (19.46 / 19.30)
   one system at a time 14.06 ms (13.83 / 13.57), no system 1.94 (1.82 / 1.84), several at once 3.99 (3.81 / 3.87)
   FixedUpdate 7.85 ms a frame (7.22 / 7.06) at 1.20 runs (1.18 / 1.16), 6.51 a run (6.13 / 6.11)
   Render 4.25 (4.61 / 4.64), render_system 4.21 (4.57 / 4.59), render.extract_ms 4.15 (4.65 / 4.64), render.cpu_ms
   7.41 (7.62 / 7.68)
   render meshes: sync: 1.26 ms (1.83 / 1.95), the mean of the 11 render.cpu. dumps at 25-75 s (0.76-1.86 over the
   stand); in the traces 1.21 ms a frame, median 1.17, max 2.51 (round 27 1.90, round 26 1.88). #592 takes ~0.7 ms off
   it; prepare meshes 2.81 (3.13 / 3.27).
   Trace (120 frames from 40.2 s): 20.54 ms a frame (48.7 fps; round 27 a1 19.00), no system 2.05, one system 15.94,
   several 2.56; 1.07 threads busy. Ran alone: render_system 4.39, run_scripts 1.21, smoke::step 1.10, step_bodies
   0.64, step_movers 0.63, think_and_act 0.50, physics_simulate 0.45.
   So Render is 0.36 ms shorter, but FixedUpdate costs 0.6 ms a frame more than in round 27 a1, and fps is 0.8 lower.
   In the traces the growth is smoke::step +0.43 ms a frame, step_movers +0.40, think_and_act +0.26 (fire step_field
   -0.25) (traces/trace_plaza_vs_r27.txt). The same systems grew in round 27's single-executor runs, so smoke and the
   NPCs evidently vary from run to run at the plaza; round 27's reading that the executor made them slower is weaker
   than I wrote.
4) Lake arrival from the plaza (3-4-plaza-lake/arrival.txt; traces/trace_lake_arrival*):
   frame ms from the goto's frame (trace): 45, 99, 78, 78, 70, 66, 65, 67, 72, 95, 72, 70, 71, 72, 73, 60, 60, 55, 44,
   43 (mean 67.8, 1355 ms over 20 frames). The engine.stats answers' wall intervals match frame for frame (99.1, 78.0,
   77.5, 69.5, ...); their frame_ms is the engine's smoothed time (19.4 -> 49.6). Round 26's arrival from the forest:
   54, 98, 71, 76, 78, 77, 81, 70, 65, 70, 74, 76, 73, 80, 76, 66, 64, 65, 68, 55.
   render.gi per answer (answer j is read at the start of the frame after trace frame j; its values match trace frame
   j-1's spans, e.g. extract_ms 4.79 / probe gi 4.80 in frame 0, 9.06 / 9.08 in frame 1):
     j   extract_ms build_ms read instances meshes built_triangles building full_gather rebuilt changed
     0   1.55       1.07     77   332       121    0               0        0           0       3524   (before the jump)
     1   4.79       4.06     49   10        131    7986            4        1           1       3542
     2   9.06       7.71     214  111       142    7914            4        0           1       4164
     3   5.03       2.86     139  120       151    6086            4        0           1       5051
     4   5.87       1.97     139  124       155    0               4        0           1       3970
     5   5.19       2.22     142  129       160    864             4        0           1       4354
     6   3.26       1.28     142  133       164    0               4        0           1       4202
     7   3.59       1.94     142  137       168    0               4        0           1       3213
     8   3.41       0.68     142  141       172    0               0        0           1       2768
     9   2.06       1.24     142  141       172    0               0        0           0       2713
     10  8.46       7.66     142  141       172    0               0        0           0       3189
     11  2.06       1.35     142  139       172    0               0        0           1       2732
     12-19 1.58-3.53 1.05-2.77 140 139      172    0               0        0           0       2331-4460
   (gathered 1 from answer 1 on; probes 16384, probes_updated 2048 and on 1 throughout; gpu_ms_per_kray 0.011 -> 0.002.)
   So: one full gather on the first frame after the jump, 4 meshes building for 7 frames (~22,000 triangles built in the
   first 3), instances 10 -> 141 and meshes 131 -> 172 over 8 frames; extract 4.8-9.1 ms in the first 5 frames, 2-3.6 to
   frame 13, ~1.6 after; one more 8.5 / 7.7 ms extract / build in answer 10 without a rebuild. "changed" is 2,300-5,100
   on every frame, rebuilt or not.
   The rest of those frames is as in round 26 (traces/trace_lake_arrival_schedules.txt): frame 0 (45 ms) render_system
   29.3 (prepare space bodies 6.5 for the new mirror view); frame 1 (99) two fixed runs, ripple_system 19.0 + 8.6
   (ripples: rebuild 18.7, of it swash: ground 13.0), vfx tick_effects 9.5; frame 9 (95) run_scripts 16.0, of it
   "scripts: draw and profile" 12.9. Frames 2-17 are fixed-step catch-up, 3-4 FixedUpdate runs a frame (3.12 on average)
   at ~12 ms a run; per run there: softbody step_beside_physics 3.36 (6.6 max; round 26 2.22), shore_wetness 2.10
   (round 26 2.19), physics_simulate 1.65 (2.44), ripple_system 1.61 (1.67), smoke::step 0.89, step_movers 0.83.
   render_system is 29 -> 11 ms over the 20 frames, of it "probe gi" 4.8, 9.1, 5.0, 5.9, 5.2, then 2-3.6, then ~1.6.
