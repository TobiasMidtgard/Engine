Round 29 on Tobias's PC (RTX 5080, 32 threads, 64 GB), release, Ultra, window 1600x900 as the exe opens it, real time,
every showcase run with ENGINE_PROFILE=1 and --profile, plain. Both runs on main 94ab0ecc ("Water: a fuller waterfall
... (#594)"; since round 28's 6da6a069 it has 5b196710 destruction ground march, 4e149fc5 NPC tactical world updates
once per 0.2 s, f4c68b50 unchanged transforms not rewritten, 0946baf5 script draw/debugger/profiler spans, 0c6c0d6d
shore ground read on workers + lake mirror reuses the falls (#593), bde8c704 far lakes stepped only for a boat in view).
NOT in it: Physics #591 (still open) and Rendering round 17 (no render/round17 branch or PR on GitHub), so there are no
render.gi.unchanged / render.gi.shifted keys. No asset manifest changes since 6da6a069, so no fetch-assets. Build at
below-normal priority with nothing else running, 10:26:22-10:29:21 UTC, 2 m 58 s (build.txt). Each game run started only
after 5+ min with no input (43,566 and 43,693 s), one at a time, and ended with taskkill /IM showcase.exe /F. Input
only through the game's flags, environment and bridge. Runs 10:29-10:34 UTC, 2026-11-04.
2-fortress, 3-4-plaza-lake   round 28's two runs, the same scripts (the fortress trace asked 20.1 s after arriving;
                     the plaza trace 40.1 s in; the lake goto answered 10:33:13.27 and the 20 render.gi answers came on
                     frames 3487-3506, one a frame)
fortress-r29-r28-r27.txt, plaza-r29-r28-r27.txt   ab28.py on this round's stand and rounds 28 and 27's (same dir names
                     for r29 and r28: the first column is this round)
traces/*_inner.txt   inner29.py: the spans inside a system or span (ms a frame, runs, ms a run, max) and per frame;
                     *_vs_r28.txt diff27.py against round 28's traces; *_schedules.txt sched26.py per frame
scripts              round 28's, plus inner29.py (inner27.py that also takes a span as the container, keeps a render
                     thread's container to its own thread, and counts other threads' spans only from the container's
                     crate)

Summary
2) Fortress 60 s stand, 25-60 s after arriving (round 28 / round 27 in brackets):
   fps 36.7 (31.3 / 21.4); frame 27.82 ms (32.35 / 47.30); fixed steps a frame 1.63 (1.91 / 2.78)
   FixedUpdate 12.96 ms a frame (14.85 / 21.30), 1.67 runs (1.93 / 2.81), 7.75 a run (7.66 / 7.54), floor 7.09
   Update 4.12 (6.31 / 13.05), floor 1.59 (3.36 / 9.96); Render 5.91 (6.26 / 7.70); PostUpdate 3.26 (3.36 / 3.67)
   one system at a time 19.60 ms (22.43), no system 2.45 (2.65), several at once 5.73 (7.30); cores 1.17 (1.24)
   per 5 s: 8.7, 15.3, 15.4, 24.9, 31.3, 41.3, 37.9, 36.6, 36.4, 33.5, 35.6, 39.7 fps
   Trace (60 frames from 20.1 s): 33.23 ms a frame (30.1 fps; round 28 37.64); no system 3.17, one system 25.12,
   several 4.94; 1.10 threads busy. FixedUpdate 16.57 ms a frame (1.97 runs, 8.43 a run), Render 6.54, Update 4.66,
   PostUpdate 3.80.
   Ran alone (ms a frame): render_system 6.49 (r28 6.94), physics_simulate 1.75 (0.47), run_scripts 1.42 (1.36),
   think_and_act 1.12 (1.57), simulate_debris 1.12 (1.25), smoke::step 0.81 (1.04), physics_sync 0.68 (0.78),
   propagate_transforms 0.53, puddle_system 0.51, rebuild_world_model 0.44 (1.98), step_movers 0.39. physics_simulate
   costs the same (3.71 ms a frame in the trace, 3.78 in round 28; rapier step 0.61 a run, max 7.91) but runs alone more
   now that ripple_system no longer runs beside it. One 5 s profile (48-53 s) has a 27 ms rapier step (physics_simulate
   max 32.2).
   Per frame in the trace (traces/trace_fortress_inner.txt; min / median / max over the 60 frames):
     engine_destruction::build_system  0.157 ms a frame, max 0.29 (r28 1.04, r27 3.65); per frame 0.11 / 0.13 / 0.29
       build: builders  0.146 a frame, max 0.27
       build: plan  0.129 a frame, 2 runs, 0.064 a run, max 0.20 (r28 0.49 a run, r27 1.71); 0.09 / 0.10 / 0.24 a frame
       build: aim at ground and rock  0.106 a frame, 2.02 runs, 0.053 a run, max 0.17 (r28 0.46 a run)
       build: snap and support 0.016, occupied 0.001, pieces near 0.008, ghosts and support 0.001 ms a frame
     engine_npc::rebuild_world_model  0.440 ms a frame, 1.97 runs, 0.224 a run, max 4.17 (r28 1.976, max 3.81);
       per frame 0.02 / 0.04 / 4.21
       world model update  0.20 runs a frame (in 12 of 60 frames), 1.670 ms a run, max 3.88 (r28 1.05 runs a frame,
         1.762 a run, max 3.74): the same cost a run, a fifth as often
       world model poll  0.25 runs a frame (15 of 60 frames), 0.266 a run, max 0.47
     engine_water::ripple_system  0.237 ms a frame, 1.97 runs, 0.121 a run, max 2.51 (r28 3.118, 1.407 a run);
       per frame 0.08 / 0.18 / 2.60. "ripples: step" ran in 1 of 60 frames (r28: every run, 0.98 ms a run); crests,
       trail and bubbles in none (r28 0.24 / 0.18 / 0.16 a run); the one frame with a rebuild had rebuild 2.14
       (depths 0.25, speeds 1.53, streams 0.27, swash 0.02).
     other water systems in FixedUpdate (ms a frame): flow_step 0.57 (0.61), puddle 0.51 (0.55), splash_drops 0.44
       (0.42), flow_bed 0.41 (0.43), boat_air 0.18, buoyancy 0.14, sync_bodies 0.13, shore_wetness 0.11 (0.28)
       (traces/trace_fortress_vs_r28.txt)
     render "prepare water"  1.116 ms a frame, max 2.37 (r28 2.314); per frame 0.73 / 1.06 / 2.37
       prepare: lock 0.000 (max 0.00), prepare: bodies 0.141 (max 0.28), falls upload 0.528 (max 0.88; 0.36 / 0.48 /
       0.88 a frame), flow upload 0.303 (r28 0.220), ripple upload 0.001 (r28 0.861), surface 0.069 (r28 0.549),
       puddles 0.023 (all 1 run a frame)
   So against round 28 the fortress lost ripple_system's 2.9 ms a frame and ripple upload's 0.9, rebuild_world_model's
   1.5 and build_system's 0.9; FixedUpdate runs 1.97 times a frame in the trace instead of 2.22.
3) Plaza 75 s stand, 25-75 s after arriving (round 28 / round 27 a1 in brackets):
   fps 52.1 (49.9 / 50.7); frame 19.10 ms (20.00 / 19.46)
   one system at a time 13.24 ms (14.06 / 13.83), no system 1.91 (1.94 / 1.82), several at once 3.93 (3.99 / 3.81)
   FixedUpdate 6.96 ms a frame (7.85 / 7.22) at 1.14 runs (1.20 / 1.18), 6.06 a run (6.51 / 6.13)
   Render 4.17 (4.25 / 4.61), render_system 4.13 (4.21 / 4.57)
   meshes: sync 1.03 ms, the mean of the 11 render.cpu. dumps at 25-75 s (1.26 / 1.83); in the traces 1.17 ms a frame,
   median 1.10, max 2.63 (1.21 / 1.90). prepare meshes 2.55 (2.81 / 3.13), prepare side by side 2.65 (2.98 / 3.13).
   Trace (120 frames from 40.1 s): 19.69 ms a frame (50.8 fps; r28 20.54); no system 2.07, one system 15.13, several
   2.49; 1.06 threads busy. Ran alone: render_system 4.34, run_scripts 1.27, smoke::step 0.79, step_bodies 0.63,
   think_and_act 0.43, propagate_transforms 0.42.
   smoke::step is 1.00 ms a frame in the profiles (1.31 / 1.27), the other systems as in rounds 27-28.
4) Lake arrival from the plaza (3-4-plaza-lake/arrival.txt; traces/trace_lake_arrival_inner.txt, _schedules.txt):
   frame ms from the goto's frame (trace): 47, 75, 81, 63, 72, 62, 86, 74, 78, 72, 75, 73, 74, 76, 70, 64, 66, 46, 32, 30
   (mean 65.8, 1316 ms; round 28: 45, 99, 78, 78, 70, 66, 65, 67, 72, 95, 72, 70, 71, 72, 73, 60, 60, 55, 44, 43, mean
   67.8). The answers' wall intervals match frame for frame (73.5, 82.4, 62.7, 72.2, ...).
   render.gi per answer (as round 28: answer j's values match trace frame j-1; no unchanged / shifted keys):
     j   extract_ms build_ms read instances meshes built_triangles building full_gather rebuilt changed
     0   1.53       1.16     256  332       121    0               0        0           0       3729   (before the jump)
     1   4.99       4.32     49   10        131    7986            4        1           1       3819
     2   7.57       6.20     139  111       142    7914            4        0           1       5648
     3   6.67       4.38     139  120       151    6086            4        0           1       5430
     4   3.89       1.10     139  124       155    0               4        0           1       3267
     5   4.09       1.44     142  129       160    864             4        0           1       3261
     6   2.97       1.28     142  133       164    0               4        0           1       3312
     7   3.27       1.07     142  137       168    0               4        0           1       3618
     8   2.89       0.82     142  141       172    0               0        0           1       3422
     9   1.86       1.36     142  141       172    0               0        0           0       4429
     10  2.43       1.58     142  141       172    0               0        0           0       4095
     11  1.99       1.17     142  139       172    0               0        0           1       3072
     12-19 1.35-2.41 0.96-1.52 140 139      172    0               0        0           0       2832-4050
   The same shape as round 28 (one full gather, 4 meshes building for 7 frames, ~22,000 triangles in 3); extract 7.57 /
   6.67 at answers 2-3 (round 28 9.06 / 5.03), and no 8.5 ms spike at answer 10 this time.
   render_system per frame: 31.1, 27.2, 23.2, 14.4, 18.4, 14.4, 19.9, 15.9, 18.8, 14.7, 17.5, 14.7, 17.1, 17.1, 15.3, 12.1,
   12.8, 10.2, 12.4, 9.8 ms (16.9 a frame; frame 0 is the new mirror view: prepare space bodies 8.1 of it).
   Water on the frame: ripple_system 0.02, 12.47 (3 runs), 8.67, 8.92, 9.48, 7.13, 8.11, 7.68, 5.84, 6.10, then 2.2-3.1
   to frame 17 and 0.8-0.9; of it "ripples: rebuild" 10.66, 7.24, 7.17, 7.68, 5.53, 6.16, 5.38, 3.85, 3.80 in frames 1-9
   (in every run, 0.9-3.6 ms a run), then ~0.1; "ripples: step" 0.8-0.9 a frame from frame 12. shore_wetness_system 0.03-0.75 a
   frame (0.10 ms a run; round 28 2.10 a run, 6.6 ms a frame).
   Water off the frame (async workers): "swash: ground" 23.16 ms and "swash: bed" 4.46 ms once, starting in frame 1
   (round 28 had swash: ground 13.0 on the frame, inside ripple_system); "shore ground" once a frame from frame 1 to 17,
   20.4-65.2 ms each (799 ms in all), none on the frame. Neither "swash: bed" nor "shore ground" starts inside
   ripple_system's or shore_wetness's runs.
   run_scripts: 2.00, 1.71, 1.62, 1.59, 2.86, 1.67, 14.53, then 1.5-2.0 ms a frame (2.51 a frame on average); scripts:
   serial 1.01 a frame (max 2.19), parallel 0.76 (max 0.91), draw and profile 0.64 (max 12.68): of it scripts: profiler
   12.67 in frame 6 and ~0 otherwise, scripts: draw 0.001, scripts: debugger output 0.000.
   So the shore work left the frame (shore_wetness -6.2 ms a frame, swash: ground -13 ms in frame 1), but the frames
   stay at 62-86 ms: FixedUpdate now runs 3.88 times a frame in frames 2-17 (round 28 3.12) at ~10.5 ms a run (12.2),
   34-44 ms a frame in frames 2-16. Its biggest systems there: softbody step_beside_physics 3.68 ms a run (max 6.83; r28 3.36),
   physics_simulate 1.55 (1.65), ripple_system 1.34 (1.61), smoke::step 0.93, step_bodies 0.78, flow_step 0.49,
   step_movers 0.49, gather_colliders 0.46, physics_sync 0.40. The frames shorten only at frame 17-18 (46, 32, 30 ms).
