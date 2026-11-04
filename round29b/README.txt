Round 29b on Tobias's PC (RTX 5080, 32 threads, 64 GB), release, Ultra, window 1600x900 as the exe opens it, real time.
All runs on main 4c164e62 ("Merge pull request #599 from TobiasMidtgard/render/round17": Rendering round 17 is in, with
probe GI change ticks, render.gi.unchanged / shifted / ms_<part>, grass grown once a blade in a compute cull pass; also
a18ced30 falls strips built once, 7eff6ecf still depths on a worker, 7f5b31f7 ripple hand-over spans, fb8f0676 script
profiler stop, 1b125b12 showcase colliders). NOT in it: Physics #591 (still open). No asset manifest changes since
94ab0ecc, so no fetch-assets. Build at below-normal priority with nothing else running, 11:07:40-11:10:43 UTC, 3 m 03 s
(build.txt). Each game run started only after 5+ min with no input (46,048-46,376 s), one at a time, and ended with
taskkill /IM showcase.exe /F. Input only through the game's flags, environment and bridge. Runs 11:10-11:21 UTC.
2-3-plaza-lake       round 29's plaza + lake run: ENGINE_PROFILE=1 --fly --profile, plaza 75 s (render.cpu. dumps every
                     5 s, a 120-frame trace 40.1 s in), then engine.trace 20 frames and showcase.goto lake the moment it
                     was answered (the goto's frame is the trace's frame 0), then each of 20 frames engine.stats
                     "render.gi" on the main connection and, written right after it, engine.stats "render.cpu.meshes" on
                     a second connection: all 20 pairs were answered in the same frame (3488-3507)
4-loops              step 4: round 26 step b's loops x1 (overview, plaza, market, forest, lake, fortress, field, orbit;
                     PHYSICS_REPORT=240 RUST_LOG=info; engine.stats water. and render.extra_views twice a second; no
                     fast sampler, no traces), the tables over each stop's last 20 s; grass.txt (grass29b.py) every grass
                     and shadow GPU row in the render profiles there
4x-market-field-stands  my first try at step 4: --fly, goto market 20 s, goto field 20 s straight after loading. The
                     market stand is mostly its load (25.4 fps over 5-20 s), so step 4's numbers are from 4-loops; the
                     GPU rows are in grass.txt here too
plaza-r29b-r29-r28.txt  ab28.py on this plaza stand and rounds 29 and 28's
scripts              round 29's, plus a second connection in run29b.ps1's arrive stop (-ArriveStatsPrefix2), arrive29b.py
                     (both connections' answers), grass29b.py, and round 26's loops tables (analyze21b, table21c,
                     passes21d)
scripts/trace-summary.py crashed once on the plaza trace (a TypeError in schedule_at, then a segfault); rerun on the
same file it finished, and that output is the one here.

Summary
2) Lake arrival from the plaza (2-3-plaza-lake/arrival.txt; traces/trace_lake_arrival_inner.txt, _schedules.txt):
   frame ms from the goto's frame (trace): 47, 80, 71, 70, 75, 66, 68, 67, 74, 83, 70, 66, 68, 67, 68, 65, 68, 52, 59, 38
   (mean 66.1; round 29: 47, 75, 81, 63, 72, 62, 86, 74, 78, 72, 75, 73, 74, 76, 70, 64, 66, 46, 32, 30, mean 65.8)
   render.gi per answer (answer j's values match trace frame j-1, as in round 28), and render.cpu "meshes: sync" from
   the second connection the same frame:
     j  extract build  changed unchanged shifted read full rebuilt | ms_changes ms_refresh ms_lists ms_read ms_builds ms_upload | meshes: sync
     0  0.94   0.32   2080    1588      0       77   0    0       | 0.61       0.28       0.02     0.02    0.00      0.00      | 0.019
     1  3.66   2.06   2088    1425      0       49   1    1       | 1.49       0.37       0.48     0.31    0.90      0.10      | 0.062
     2  6.59   4.72   3626    2666      0       214  0    1       | 1.63       3.33       0.01     0.42    0.96      0.23      | 0.034
     3  4.39   1.86   1608    2633      0       139  0    1       | 1.90       0.42       0.01     0.39    1.04      0.62      | 0.067
     4  4.84   1.18   1500    2150      0       142  0    1       | 1.76       0.40       0.02     0.52    0.25      1.86      | 0.028
     5  3.70   0.65   1590    1433      0       142  0    1       | 2.41       0.30       0.02     0.34    0.00      0.63      | 0.062
     6  1.98   0.54   1588    2682      0       142  0    1       | 1.21       0.20       0.01     0.33    0.00      0.23      | 0.040
     7  2.95   0.48   1557    2646      0       142  0    1       | 2.08       0.15       0.02     0.31    0.00      0.39      | 0.075
     8  3.05   0.57   1571    2974      0       142  0    1       | 2.04       0.53       0.01     0.03    0.00      0.43      | 0.051
     9  1.95   0.63   1559    2712      0       142  0    0       | 1.28       0.54       0.02     0.07    0.00      0.03      | 0.087
     10 1.85   0.59   1461    1912      0       142  0    0       | 1.22       0.49       0.03     0.07    0.00      0.03      | 0.064
     11-19 0.87-1.57 0.22-0.58 1432-1555 1236-1817 0 140-142 0 0-1 | 0.63-1.05 0.18-0.50 0.01-0.02 0.03-0.07 0.00 0.01-0.14 | 0.016-0.052
   (building 4 for answers 1-7, built_triangles 7986, 7914, 6086, 864 in answers 1-4; instances 332 -> 10 -> 141,
   meshes 121 -> 172, as in rounds 28-29.) Against round 29: extract 3.66 / 6.59 / 4.39 / 4.84 in answers 1-4 (round 29
   4.99 / 7.57 / 6.67 / 3.89) and 0.87-1.57 from answer 11 (round 29 1.35-2.41); "changed" is about half of round 29's now that
   "unchanged" (1,236-2,974 a frame) is counted apart; shifted stays 0. ms_changes is the largest part every frame
   (0.6-2.4 ms); ms_refresh peaks at 3.33 (answer 2), ms_upload at 1.86 (answer 4), ms_builds at ~1 while building.
   meshes: sync on the second connection is 0.02-0.09 ms a frame at the lake. The trace shows two runs in frames 0 and 1
   (0.82 and 1.82 ms: the main view and the new mirror view), none in frame 2, then one of 0.02-0.09 ms a frame; the
   render.cpu key seems to hold only the last view's run in those first frames. At the plaza it is ~1.1 ms (step 3).
   From the trace, per frame (sched26.py):
     render_system  30.5, 27.2, 24.0, 19.8, 21.5, 16.4, 18.9, 17.6, 19.9, 19.4, 20.3, 15.0, 19.2, 16.7, 18.4, 16.1, 18.3,
                    12.8, 14.1, 11.7 ms (18.9 a frame; round 29 16.9); of it prepare side by side 9.6, 17.6, 11.3, 9.9,
                    8.8, then 5.2-8.2; prepare space bodies 6.7 in frame 0; probe gi 4.4 in frame 2
     FixedUpdate    runs 1, 3, 4, 4, 4, 4, 4, 4, 3, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 3; ms 7.3, 29.2, 34.0, 34.9, 39.2, 37.5,
                    35.5, 33.5, 40.1, 36.9, 36.1, 37.1, 36.3, 33.9, 35.0, 35.4, 36.1, 29.0, 35.1, 18.1 (33.0 a frame,
                    3.70 runs, 8.92 a run; round 29 34.6 at 3.50 runs, 9.89 a run). Frames 2-17 run 3.94 times a frame;
                    biggest there per run: step_beside_physics 2.45 (round 29 3.68), physics_simulate 1.46, step_bodies
                    1.07, smoke::step 0.87, ripple_system 0.78, flow_step 0.49, gather_colliders 0.47, physics_sync 0.40
     ripple_system  0.02, 9.18 (3 runs), 5.40, 4.96, then 2.3-3.3 a frame (3.16 a frame, 0.85 a run, max 4.71; round 29
                    4.85, 1.39). Inside it: ripples: rebuild 7.62, 3.57, 2.46 in frames 1-3, then 0.06-0.10 (0.20 a run,
                    max 4.44; round 29 kept rebuilding 3.8-10.7 ms a frame to frame 9); ripples: depths 4.87, 3.43, 0.71
                    in frames 1-3 (1.0 a run, 0.45 runs a frame); ripples: speeds 2.06 (frame 1), 1.22 (frame 3);
                    ripples: hand over 0.6-1.3 a frame every frame from frame 1 (0.25 a run); ripples: step 0.6-0.9 a
                    frame from frame 6; ripples: swash 0.09, 0.01, 0.04 in frames 1-3.
     still depths   on the async workers, 15 runs in frames 1-3, 0.38-1.75 ms each (9.9, 10.4, 0.9 ms a frame; 13 of
                    them start during a ripple_system run, which is what inner29.py lists). Also on
                    the workers: swash: ground 22.7 and swash: bed 5.1 once (frame 1), shore ground about every second
                    frame, 11-133 ms each (frames 1-18), none of it on the frame.
     run_scripts    1.22, 2.96, 1.72, 1.65, 1.40, 1.90, 1.86, 1.81, 1.83, 3.00, 1.67, 1.73, 1.60, 4.19, 1.33, 1.68, 1.74,
                    2.06, 1.37, 1.04 ms (1.89 a frame, max 4.19; round 29 2.51, max 14.53). scripts: serial 0.98 a frame
                    (max 2.08), parallel 0.75 (max 0.90), draw and profile 0.07 (max 1.29): of it scripts: profiler 0.07
                    (max 1.28; round 29 12.67 once), scripts: draw 0.001, scripts: debugger output 0.000.
   So the arrival is no shorter (66 ms a frame for 20 frames): the ripple rebuild settles in 3 frames instead of 9 and
   the script profiler's stall is gone, but FixedUpdate still runs ~4 times a frame at ~9 ms a run (softbody, physics,
   smoke and ripples) and render_system is 15-30 ms.
3) Plaza 75 s stand, 25-75 s after arriving (round 29 / round 28 in brackets):
   fps 52.5 (52.1 / 49.9); frame 19.03 ms (19.10 / 20.00)
   one system at a time 13.18 ms (13.24 / 14.06), no system 1.89 (1.91 / 1.94), several at once 3.99 (3.93 / 3.99)
   FixedUpdate 7.16 ms a frame (6.96 / 7.85) at 1.16 runs, 6.18 a run; Render 4.07 (4.17 / 4.25), render_system 4.02;
   render.extract_ms 3.98 (4.28), render.prepare_shared_ms 9.04 (10.00)
   meshes: sync 1.18 ms, the mean of the 11 render.cpu. dumps at 25-75 s (1.03 / 1.26); in the trace 1.08 ms a frame,
   median 1.03, max 1.65 (1.17 / 1.21). probe gi 1.15 (1.77 / 1.74), meshes: cull 1.16 (1.61), prepare grass 0.15 (0.23).
   run_scripts max of a run 2.23 ms (7.06 / 7.01).
   Trace (120 frames from 40.1 s): 19.07 ms a frame (52.4 fps); no system 1.93, one system 14.64, several 2.50; 1.08
   threads busy. Ran alone: render_system 4.13, run_scripts 1.20, smoke::step 0.94, think_and_act 0.69, step_bodies 0.64.
4) Market and field, the loops run's stops (last 20 s of each 25 s dwell; round 26 b in brackets; 4-loops/tables.txt,
   grass.txt):
   market  44.7 fps / 1% low 36.1 (47.3 / 39.7); GPU total 17.90 ms (19.04), GPU busy 78% (91)
           compute grass 0.08 (n/a), prepass: grass 0.12 (0.71), scene: grass 1.31 (1.73), overlay grass 0.00;
           every grass row 1.51 ms (2.44); shadows 1.61 (1.67)
   field   51.0 fps / 1% low 27.5 (46.2 / 34.0); GPU total 17.95 (21.08), GPU busy 98% (99)
           compute grass 0.04, prepass: grass 0.02 (0.41), scene: grass 0.10 (0.32); every grass row 0.16 (0.73);
           shadows 1.28 (1.43)
   There is no separate grass shadow row in the GPU passes list (in this build or round 26's): grass's shadow cost is
   inside "shadows". The market is CPU-bound now (GPU 78% busy): softbody step_beside_physics ran alone 3.65 ms a frame
   there (round 26 1.62).
   The whole loop (fps / 1% low, round 26 b in brackets): overview 41.0 / 33.7 (42.0 / 30.6), plaza 55.2 / 44.5
   (54.7 / 46.9), market 44.7 / 36.1 (47.3 / 39.7), forest 49.2 / 31.2 (46.9 / 26.7), lake 39.1 / 33.9 (36.9 / 29.8),
   fortress 46.4 / 28.0 (30.1 / 18.5), field 51.0 / 27.5 (46.2 / 34.0), orbit 91.9 / 86.1 (84.0 / 77.1).
   The 20 s stands straight after loading (4x-market-field-stands/grass.txt): market 25.4 fps, compute grass 0.08,
   prepass 0.12, scene 1.21, shadows 2.07; field 49.2 fps, 0.04 / 0.02 / 0.10, shadows 1.21.
