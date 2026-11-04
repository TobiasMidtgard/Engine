Round 25 on Tobias's PC (RTX 5080, 32 threads, 64 GB), release, Ultra, window 1600x900 as the exe opens it, real time,
every showcase run with --profile. All three steps on main e11f16aa (origin/main at the start: 25c1e855 + "Character:
motion vectors from the last frame, not the last view"; has 914d9a59, ccfbbb92, dcfb9f34, 804424e8, 6b21689d, 2056f9a4,
b753c1a3, 25c1e855). No asset manifest changes since 7a62904a (git diff empty), so no fetch-assets. Build at
below-normal priority with nothing else running, 05:08:56-05:12:07 UTC, 3 m 11 s. Every game run started only after 5+
min with no input (24,586-24,988 s), one at a time, and ended with taskkill /IM showcase.exe /F. Input only through the
game's flags and its bridge. Runs 05:13-05:22 UTC, 2026-11-04.
a-plaza-r24-r25.txt  the table of round 24 a1 against this round's a (ab25.py on both runs, 25-75 s; each run's full
                     analysis comes first); a-plaza-r24-r25-30-75s.txt the same for 30-75 s
a-plaza-plain        ENGINE_PROFILE=1 --fly --profile, showcase.goto plaza, 75 s stand (round 24 a1's method), plus
                     engine.stats "render.cpu." every 5 s (dumps/); analysis.txt: every profile, render keys, dumps,
                     the render.cpu.prepare rows
b-loops-ultra        --loops overview,plaza,market,forest,lake,fortress,field,orbit:1 with PHYSICS_REPORT=240 (round 24
                     step b's method), plus water.* and render.extra_views twice a second and render.extra_views every
                     frame around the lake arrival: tables, fixed_update.txt, npc_water.txt, lake_arrival.txt,
                     hitches.txt, passes, physics reports, a picture per stop
c-night-moon         round 24 step c again (hours 2.5 and 3.5, lake and sunset), moons_predicted.txt, crops/ (4-12x)
scripts              the drivers and analyses (run25.ps1 = run24.ps1 plus a fast sampler that can run from one loops
                     stop's picture to a few s after the next arrival; chain25.ps1 / wait_then.ps1 wait for 5 idle
                     minutes; ab25.py, loops25.py, crops25.py extend round 24's; fixed25.py and lake25.py are new; the
                     rest are round 24's)

Summary
a) Plaza 75 s stand at Ultra, plain (25-75 s after arriving; 30-75 s in brackets; means over the 5 s profiles, ranges
   in a-plaza-r24-r25.txt; round 24 a1 from the same script):
                                 round 25               round 24
   fps (engine.status)           51.2 (52.1)            39.7 (40.5)
   frame ms (profiles)           19.5 (19.0)            25.3 (24.7)
   fixed steps a frame           1.17 (1.15)            1.51 (1.48)
   one system at a time ms       13.66 (13.32)          20.36 (19.87)
   no system ms                  1.87 (1.82)            2.11 (2.06)
   several at once ms            3.94 (3.85)            2.82 (2.76)
   systems busy ms a frame       22.43 (21.90)          26.95 (26.30)
   cores busy                    1.15 (1.15)            1.06 (1.06)
   FixedUpdate ms a frame        7.14 (6.82)            8.77 (8.37)    range 6.03-9.67 / 7.85-12.40
     ms a run                    6.09 (5.99)            5.74 (5.62)
     ordering floor a frame      4.11 (3.98)            5.01 (4.83)    3.52 / 3.29 a run; 71-72 / 67-73 systems
   Render ms / floor             4.66 / 4.62            8.65 / 8.61
   Update ms / floor             3.63 / 1.49            3.70 / 1.37
   PostUpdate ms / floor         2.98 / 1.80            3.06 / 1.86
   render.wait_ms                0.02 (p90 0.01)        0.01 (p90 0.01)
   render.extract_ms             4.69 (p90 5.87)        8.54 (p90 9.61)
   render.record_total_ms        6.38 (p90 7.34)        6.20 (p90 7.10)
   render.cpu_ms                 7.85 (p90 9.01)        11.64 (p90 12.91)
   worst frame (profiles)        43.8 (31.1-62.1)       46.6 (39.2-59.2)
   The frame is 5.8 ms shorter: Render (render_system on the main thread) 8.65 -> 4.66 ms and extract 8.54 -> 4.69, now
   that the features prepare side by side; FixedUpdate costs a little more a run (5.74 -> 6.09) but runs 1.17 times a
   frame instead of 1.51. The first 25 s after arriving are slower (9, 16, 26, 45, 46 fps per 5 s, then 44 at 25-30 s);
   the 25-30 s profile has the window's worst frame, 62.1 ms: think_and_act 33.1, prepare character 30.4 (prepare side by
   side 30.5).
   render.cpu.* every 5 s, the dumps at 25-75 s (mean (min-max) of 11; each dump is one frame's step times; round 24 a1):
     prepare side by side (wall)   3.19 (2.82-4.06)   n/a
     render.prepare_shared_ms      10.69 (p90 12.22, max 16.67; the jobs' sum, 98 samples twice a second)
                                   -> side by side saves 7.5 ms a frame (3.35x); the wall is about the meshes job's time
     prepare serial                0.02 (0.01-0.09)   n/a
     prepare results               0.03 (0.02-0.04)   n/a
     prepare meshes                3.23 (2.84-3.92)   2.36
     prepare terrain               1.66 (1.50-2.09)   1.17
     prepare softbody              1.53 (1.43-1.74)   1.18
     prepare water                 0.88 (0.79-1.07)   0.56
     prepare grass                 0.18 (0.04-0.81)   0.08
     prepare character             0.60 (0.14-1.00)   0.28
     prepare fire                  0.50 (0.12-1.43)   0.18
     probe gi                      2.30 (1.69-6.53)   0.00 (round 24 had no render.cpu key for it)
   Each job takes 30-180% longer than it did one after the other in round 24 (they share the cores), but the wall is
   3.19 ms against round 24's ~5.8 ms sum of the same features (without probe gi). Other steady steps: prepare on the
   render thread 2.51 (2.25), wait for the finishing threads 1.90 (1.90), meshes: sync 1.91 (1.22), meshes: cull 1.13
   (1.20).
b) Loops x1 at Ultra (fps / 1% low; worst frame; render_system ms a frame in the last profile; GPU ms; GPU busy %):
   stop      | r25          | r24          | fps +   | worst r25 (r24) | render_system r25 (r24) | GPU ms | GPU busy
   overview* | 41.9 / 35.0  | 38.2 / 27.0  |  +3.7   | 159.9 (163.9)   | 10.95 (10.43)           | 23.6   | 100
   plaza     | 54.9 / 47.7  | 42.2 / 34.2  | +12.7   |  78.6 (92.8)    |  4.75 (8.83)            | 15.7   |  93
   market    | 42.4 / 29.2  | 30.5 / 24.5  | +11.9   | 109.2 (107.2)   |  4.96 (9.88)            | 19.0   |  86
   forest    | 44.8 / 24.4  | 33.4 / 15.6  | +11.4   |  96.0 (86.0)    |  4.87 (8.51)            | 17.2   |  80
   lake      | 35.4 / 29.7  | 26.7 / 22.2  |  +8.7   | 103.1 (108.5)   | 11.71 (17.82)           | 16.8   |  80
   fortress  | 28.9 / 17.0  | 19.2 / 12.8  |  +9.7   | 111.1 (119.7)   |  7.67 (12.03)           | 16.6   |  56
   field     | 46.0 / 35.0  | 43.3 / 35.8  |  +2.7   | 159.7 (95.4)    |  6.64 (7.59)            | 20.0   |  97
   orbit     | 83.9 / 76.7  | 66.1 / 61.0  | +17.8   |  49.2 (53.6)    |  3.62 (6.13)            |  6.7   |  66
   (* includes the start.) The overview and the field are GPU-bound (97-100% busy) and gain least. Most 96-160 ms
   worst frames at the stops are my own picture at 20 s (bridge_frame_start 54-127 ms + screenshot save 38-89 ms), as
   in rounds 23-24 (hitches.txt).
   FixedUpdate (fixed_update.txt; mean of the profiles in each stop's last 20 s, 3 this round, 4 in round 24):
     plaza     5.35 ms a frame (5.14-5.52), 1.10 runs, floor 3.05 (2.77 a run)   r24: 6.74, 1.40 runs, floor 4.05 (2.90 a run)
     fortress  14.60 (12.89-17.63), 2.10 runs, floor 8.30 (3.96 a run)           r24: 24.99, 2.88 runs, floor 18.00 (6.24 a run)
   PHYSICS_REPORT lines there (physics_reports.txt):
     plaza     1.47-1.81 ms per step, 8-9 regions, 0 waiting for their ground; regions 0.86-1.18 (Rapier 0.63-0.92)
               r24: 1.45-1.94 ms per step, 10 regions; regions 0.76-1.18 (Rapier 0.36-0.61)
     fortress  2.36-2.87 ms per step, 14 regions, 0-2 waiting; regions 1.32-1.71 (Rapier 0.88-1.04); e.g. 2.56 =
               regions 1.34 + the rest 1.22 (sync 0.36, upkeep 0.12, plans 0.10, substeps 1.18, waiting between them 0.71)
               r24: 3.64-5.37 ms per step, 12-14 regions, 1-12 waiting; e.g. 5.13 = regions 1.67 + the rest 3.45
               (waiting between them 2.83)
   Fortress water (npc_water.txt; ms a frame / max of a run, per 5 s profile):
     water::ripple_system  2.43 / 11.93, 2.67 / 4.04, 2.52 / 4.35, 2.61 / 5.67, 3.51 / 4.03
                           (round 24: 10.04 / 32.92, 13.70 / 36.41, 12.04 / 38.76, 12.14 / 37.02, 5.14 / 30.11)
     ripples: rebuild      only in the first profile after arriving: 1.15 / 10.91 (round 24: 6.6-9.8 / 31-36 in every one)
     ripples: depths       first profile only: 0.37 / 2.46, 29 runs
     ripples: speeds       first profile only: 0.36 / 2.56, 29 runs
     ripples: streams      in no fortress profile's list (nor anywhere in the run)
     ripples: swash        in no fortress profile's list (once 4.7 ms during the start)
     ripples: step         0.54-2.42 ms a frame, max 1.5-3.4 (round 24: 2.1-2.8 / 1.9-4.5) - now the bulk of it
     water.ripple_rebuilds 43 at the fortress: 33 in the first 5 s after arriving, then 3, 2, 3, 2 per 5 s. Every stop
                           with water rebuilds 2-3 per 5 s while standing still (plaza 13 over the stop, 2-3 per 5 s
                           from the start; market 44, forest 45, lake 43, field 43, each 33-35 in its first 5 s);
                           overview 0, orbit 1.
   New: ripples: depths 201.7 ms once (ripple_system 202.0) in the overview's first profile during loading (a 1494 ms
   frame of commands 1408); round 24's run had no ripple run over 39 ms.
   foliage press_fallen_crowns: in no profile's slowest-systems list this round (round 24: 107.3 ms at the plaza, 55.2
   at the fortress); it presses on the task pool now (5f9e0c41). The list's 40th row is at a median 0.11 ms a frame, so
   one short run could hide under it, but no hitch profile names it.
   Lake arrival (lake_arrival.txt; the jump at 05:17:33.81):
     the new extra view's state: render.extra_views_new_ms 2.75, render.extra_views_new_compiled 0 (0 new shaders,
       layouts and pipelines); render.extra_views_new.<part>: shadow maps 1.1, targets 1.0, volumetrics 0.1, screen
       effects 0.1, environment 0.1, sky occlusion 0.1, post 0.1, frame groups 0.1, the rest 0.1, atmosphere 0.0,
       exposure meter 0.0
     render.extra_views_ms on the first mirror frames: 22.94, 17.41, 11.68, 13.35, 14.16, 13.11, 11.97, ... 9-13 to
       1.3 s, 7-9 after 1.5 s; render.extra_views_gpu_ms 0.00 on the first, then 4.3-5.1. The renderer's log of the first
       extra-view frame: CPU 22.67 ms (prepare space bodies 7.22, prepare side by side 3.77, prepare meshes 3.72);
       round 24's: 121.46 ms (prepare space bodies 19.70).
     the profile covering it: 181 frames in 5 s (36.2 fps), worst 91.8 ms, render_system max 43.6, main 12.2 ms a
       frame (round 24: 100 frames, 19.9 fps, worst 168.6, render_system max 150.1).
     No single hitch now; instead the first 17 frames after the jump take 58-93 ms each (frame intervals from the
       one-answer-a-frame sampler: 58, 89, 76, 80, 86, 93, 77, 67, 71, 66, 71, 65, 67, 74, 75, 69, 74 ms over 1.27 s),
       then 60, 60, 52, 46, 42, 40 and ~33-36 ms from 1.7 s; engine.status agrees (45.9, 72.7, 68.5, 39.7, 35.3 ms a frame
       per 0.5 s). GPU busy falls from 92-99% to 32-50% for those 2 s, so they are CPU-side. The profile's single runs
       over 10 ms: render_system 43.6, probe gi 17.5 (prepare side by side 17.5), smoke pressure 15.0 / advect 14.2,
       script_frame_system 15.0 (run scripts 14.5), physics_sync 14.1 (sync from world 14.1), ripple_system 11.7
       (rebuild 11.2); which of them falls on which slow frame the 5 s profile can't tell.
   Also: the fortress's 187.3 ms frame is engine_npc::think_and_act 121.4 ms (inner "choose position" 102.3), right
   after arriving; round 24's worst think_and_act there was 20.6. The market had think_and_act 57.1 (find path 54.5).
c) Night, h2_5_lake again the first place after loading: no green wedge (0 bright-green pixels in the bottom third,
   against 4729 in round 24's picture; x4 crops of the near water and both shores show dark water, the moon glitter
   and a thin swash line at the near beach). One Moon: the only disc in the four pictures is the space Moon in
   h3_5_sunset at (914, 23), predicted (914, 19), and it is still drawn as a crescent (bright right limb, ~8-13 px with
   its glow), as in round 24, though from the camera it is 169.6 deg from the Sun and ~99% lit, so it should be a
   near-full disc. h2_5_lake has no moon in the frame (predicted at (991, -305), above it).
