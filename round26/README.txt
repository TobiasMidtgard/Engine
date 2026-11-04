Round 26 on Tobias's PC (RTX 5080, 32 threads, 64 GB), release, Ultra, window 1600x900 as the exe opens it, real time,
every showcase run with --profile. Both steps on main d3cf57ad (origin/main at the start: "Merge pull request #590 from
TobiasMidtgard/render/round15"; has b15e1ccf "Bridge: engine.trace ...", Water #587 "ripple grid rebuilds once a move"
= ab985f0f, 8fa0a1d7 "Arrival hitches: NPC spot ground built at once, character uploads off the frame", f485053b
"Physics: big static meshes build off the frame" and 9fb1b4b0; NOT Physics #588 "identical big shapes are built once
and shared", which is still open). No asset manifest changes since round 25's e11f16aa (git diff empty), so no
fetch-assets. Build at below-normal priority with nothing else running, 06:28:00-06:31:14 UTC, 3 m 14 s (build.txt).
Every game run started only after 5+ min with no input (29,282 and 29,422 s), one at a time, and ended with taskkill
/IM showcase.exe /F. Input only through the game's flags and its bridge. Runs 06:31-06:38 UTC, 2026-11-04. No night step.
a-plaza-r25-r26.txt  round 25 a against this round's a (ab25.py on both, 25-75 s; "r25a" = round 25's a_plaza_plain;
                     each run's full analysis first); a-plaza-r25-r26-30-75s.txt the same for 30-75 s
a-plaza-plain        round 25 a's method, plus engine.trace 120 frames 40 s after arriving (traces/)
b-loops-ultra        round 25 b's method, plus engine.trace 60 frames 10 s after arriving at plaza, market, lake and
                     fortress, and 30 frames over the lake arrival (traces/, with summaries)
scripts              run26.ps1 = run25.ps1 plus the engine.trace calls (one at a time, each waiting for the log's
                     "trace of N frames written" line) and a 50 ms log-tail reader for the loops lines; chain26.ps1 /
                     wait_then.ps1 wait for 5 idle minutes; trace26.py, sched26.py, top26.py read the traces;
                     spanmax26.py the profiles' rows for named systems and spans; gz26.py, package26.ps1; the rest are
                     round 25's (ab25.py, fixed25.py, loops25.py, lake25.py, ...)
Every trace is a gzip'd Chrome trace (ui.perfetto.dev or scripts/trace-summary.py read .json.gz); the largest is 1.7 MB
(13.6 MB raw).

Summary
a) Plaza 75 s stand at Ultra, 25-75 s after arriving (30-75 s in brackets), means over the 5 s profiles:
                                 round 26               round 25
   fps (engine.status)           51.2 (52.1)            51.2 (52.1)
   frame ms (profiles)           19.3 (19.2)            19.5 (19.0)
   fixed steps a frame           1.17 (1.15)            1.17 (1.15)
   one system at a time ms       13.57 (13.50)          13.66 (13.32)
   no system ms                  1.84 (1.85)            1.87 (1.82)
   several at once ms            3.87 (3.86)            3.94 (3.85)
   systems busy ms a frame       22.17 (22.10)          22.43 (21.90)
   cores busy                    1.15 (1.15)            1.15 (1.15)
   FixedUpdate ms a frame        7.06 (6.92)            7.14 (6.82)    range 6.63-8.15 / 6.03-9.67
     ms a run                    6.11 (6.02)            6.09 (5.99)
     ordering floor a frame      4.13 (4.05)            4.11 (3.98)    3.58 / 3.52 a run
   Render ms / floor             4.64 / 4.60            4.66 / 4.62
   Update ms / floor             3.60 / 1.42            3.63 / 1.49
   PostUpdate ms / floor         2.93 / 1.76            2.98 / 1.80
   render.extract_ms             4.64 (p90 5.45)        4.69 (p90 5.87)
   render.record_total_ms        6.22 (p90 7.42)        6.38 (p90 7.34)
   render.cpu_ms                 7.68 (p90 9.08)        7.85 (p90 9.01)
   render.prepare_shared_ms      10.76 (p90 12.20)      10.69 (p90 12.22); side by side wall 3.21 / 3.19 (3.35x both)
   worst frame (profiles)        40.5 (32.7-48.4)       43.8 (31.1-62.1)
   Steady state is the same as round 25. What changed is the first 25 s: 9.9, 20.5, 38.1, 47.8, 48.8 fps per 5 s after
   arriving (round 25: 9.0, 15.9, 25.7, 44.5, 46.4). The profile with the jump has a 1486.3 ms worst frame,
   engine_core::{{closure}} (commands) 1410.8 in one run (round 25: 2262.4, engine_weather::ground_system 2184.3 and the
   commands closure 1096.6); the next profile's worst is 175.1 ms (round 25: 1425.0).
   "prepare character" (render): max 1.6 ms over the whole stand (round 25: 30.4 in the 25-30 s profile, 24.5 and 23.0
   in the first 10 s). think_and_act: max 74.0 ms in the jump's profile (round 25: 60.3), 32.5 in the steady window
   (round 25: 33.1 in the 25-30 s profile).
   Trace (traces/trace_plaza.json.gz, 120 frames from 40.1 s): 19.22 ms a frame (52.0 fps; median 18.76, p90 23.27, max
   31.36), the same as the profile covering it (18.8 ms, 53.3 fps), so recording costs nothing visible. Per frame: no
   system 1.95 ms, one system 14.65, several 2.62; 1.08 threads busy. FixedUpdate 6.94 ms a frame (1.16 runs, 5.98 a run),
   Render 4.74, Update 3.60, PostUpdate 2.89. Ran alone: render_system 4.69, script run_scripts 1.16, smoke::step 0.63,
   softbody step_bodies 0.59, physics_simulate 0.50, propagate_transforms 0.40. The main thread ran 7.21 ms of systems a
   frame, so at least half of the 14.65 ms with one system running is a compute thread running one system after
   another; by schedule the one-at-a-time time is FixedUpdate 4.85, Render 4.69, Update 2.41, PostUpdate 1.83, the
   rest 0.74. The 1.95 ms with no
   system is many tiny hand-offs (the largest named gap is 0.03 ms a frame).
b) Loops x1 at Ultra (fps / 1% low; worst frame; render_system ms a frame in the last profile; GPU busy %):
   stop      | r26          | r25          | fps +  | worst r26 (r25) | render_system r26 (r25) | GPU busy r26 (r25)
   overview* | 42.0 / 30.6  | 41.9 / 35.0  |  +0.1  | 149.8 (159.9)   | 10.52 (10.95)           | 97 (100)
   plaza     | 54.7 / 46.9  | 54.9 / 47.7  |  -0.2  | 122.8 (78.6)    |  4.76 (4.75)            | 91 (93)
   market    | 47.3 / 39.7  | 42.4 / 29.2  |  +4.9  | 137.4 (109.2)   |  4.98 (4.96)            | 91 (86)
   forest    | 46.9 / 26.7  | 44.8 / 24.4  |  +2.1  | 118.6 (96.0)    |  4.77 (4.87)            | 87 (80)
   lake      | 36.9 / 29.8  | 35.4 / 29.7  |  +1.5  | 109.8 (103.1)   | 10.89 (11.71)           | 82 (80)
   fortress  | 30.1 / 18.5  | 28.9 / 17.0  |  +1.2  | 105.4 (111.1)   |  7.16 (7.67)            | 49 (56)
   field     | 46.2 / 34.0  | 46.0 / 35.0  |  +0.2  | 171.5 (159.7)   |  5.75 (6.64)            | 99 (97)
   orbit     | 84.0 / 77.1  | 83.9 / 76.7  |  +0.1  |  49.8 (49.2)    |  3.60 (3.62)            | 63 (66)
   (* includes the start.) The worst frames at the stops are again my own 20 s picture (bridge_frame_start 53.6-116.6 ms
   + screenshot save 40.7-77.1; hitches.txt), the plaza's 122.8 too. Each 10 s trace falls inside its stop's window; the
   profiles covering them (plaza 55.9, market 48.2, lake 37.7, fortress 32.3 fps) are in line with their neighbours.
   Fortress NPCs: think_and_act max 30.7 ms (round 25: 121.4), in the profile with my picture (17-22 s after arriving;
   that 105.4 ms frame = think_and_act 30.7 + bridge_frame_start 53.6); "choose position" max 7.2 (round 25: 102.3). In
   the fortress trace think_and_act is 0.11 ms a run, max 5.55. Market: think_and_act 30.6 (find path 30.5; round 25:
   57.1 / 54.5).
   Plaza "prepare character": max 1.5 ms over the stop (round 25 b: 30.6); every stop 0.5-3.4 (round 25: 26.4 at the
   overview, 30.6 at the plaza).
   FixedUpdate (fixed_update.txt; mean of the 3 profiles in each stop's last 20 s):
     plaza     5.10 ms a frame (4.99-5.26), 1.10 runs, floor 2.99 (2.72 a run)   r25: 5.35, 1.10 runs, floor 3.05 (2.77 a run)
     fortress  13.52 (12.29-15.58), 2.03 runs, floor 7.99 (3.94 a run)           r25: 14.60, 2.10 runs, floor 8.30 (3.96 a run)
     (traces: plaza 5.20 ms a frame, 1.05 runs, 4.95 a run; fortress 13.08, 1.88 runs, 6.95 a run)
   PHYSICS_REPORT lines (physics_reports.txt; Physics #588 is not in this build):
     plaza     1.29-1.75 ms per step (mean 1.48), 8 regions, 0 waiting; regions 0.73-1.08 (Rapier 0.52-0.82)
               r25: 1.47-1.81 (mean 1.64), 8-9 regions; regions 0.86-1.18 (Rapier 0.63-0.92)
     fortress  2.01-2.85 ms per step (mean 2.38), 14 regions, 1-4 waiting for their ground; regions 1.11-1.62 (Rapier
               0.74-1.11)   r25: 2.36-2.87 (mean 2.65), 14 regions, 0-2 waiting; regions 1.32-1.71 (Rapier 0.88-1.04)
   Fortress water (npc_water.txt; ms a frame / max of a run, per 5 s profile):
     water::ripple_system  1.82 / 10.13, 2.37 / 7.89, 2.64 / 4.17, 2.58 / 7.03, 3.23 / 3.84
                           (round 25: 2.43 / 11.93, 2.67 / 4.04, 2.52 / 4.35, 2.61 / 5.67, 3.51 / 4.03)
     ripples: step         0.54 / 1.60, 1.60 / 7.51, 1.79 / 1.82, 1.76 / 6.39, 2.21 / 2.39 (round 25: 0.54-2.42 / 1.5-3.4):
                           about 70% of ripple_system after the first profile
     ripples: rebuild      first profile only, 0.53 / 9.36 (round 25: 1.15 / 10.91); depths 0.36 / 2.50, first only
     fortress trace        ripple_system 2.50 ms a frame (1.33 a run, max 3.54); ripples: step 1.65 (0.88 a run, max
                           2.91), crests 0.45
     water.ripple_rebuilds 13-14 per stop over the dwell, 3-4 of them in the first 5 s (round 25: 43-45, 33-35 in the
                           first 5 s): #587 shows. Standing still still rebuilds 2-3 per 5 s at every stop with water, as
                           in round 25 (plaza 13, 2-3 per 5 s; overview 0).
   Fortress Update: engine_destruction::build_system ran alone 3.54 ms a frame in the trace (max 9.64; 7.31 in the last
   profile's ran-alone list, round 25: 7.33). Update is 7.90 ms a frame there against 3.6-4.2 at the other traced stops.
   Lake arrival (the forest's loop line at 06:35:52.205; lake_arrival.txt, traces/trace_lake_arrival*.txt,
   traces/lake_arrival_vs_steady.txt):
     the first 20 frames from the jump frame, per the trace: 54, 98, 71, 76, 78, 77, 81, 70, 65, 70, 74, 76, 73, 80, 76,
       66, 64, 65, 68, 55 ms (53.6-98.0, 1.44 s; the one-answer-a-frame sampler agrees: 54, 98, 72, 75, 77, 77, 82, 70,
       65, 69, 76, 76, 73, 79, 76, 67, 64, 64, 69, 54), then 47, 44, 50, 48, 46, 33, 36, 43 to ~1.8 s. Round 25: 58-93 ms
       for 17 frames over 1.27 s, then 60, 60, 52, 46, 42, 40. So no better.
     What the frames are (sched26.py): before the jump FixedUpdate runs once or twice a frame at 5.5 ms a run. From the
       jump, frame 2 (53.6 ms) is render_system 39.7: the new mirror view's prepare space bodies 7.0 and its own prepare
       side by side 17.0 (probe gi 17.0; the main view's is 3.9). Frame 3 (98.0) has two fixed runs (28.3 + 20.4 ms) with
       ripple_system 17.7 and 10.6 (ripples: rebuild 17.3 / 9.7, of it "swash: ground" 12.7 / 7.2), then render_system
       30.3. Frame 4 (71.3): physics_sync 12.7 (sync removals 11.9) on the main thread and npc rebuild_world_model 7.8.
       Frame 8 (81.0): script run_scripts 14.6. Frames 5-20 are steady catch-up: FixedUpdate 3-4 runs a frame (3.44 on
       average) at 10-14 ms a run = 36-45 ms a frame, plus render_system 14-22 ms. GPU busy falls from 94-99% to 23,
       48, 60, 58, 69% in the 1 s samples after the jump (nvidia_dmon.txt), so it is CPU-side, as in round 25.
     Why a fixed run costs 10.6 ms then and 5.35 ms in the lake trace 10 s later (top26.py, ms a run, arrival / later):
       engine_water::shore_wetness_system 2.19 / 0.08, physics_simulate 2.44 / 1.49, softbody step_beside_physics 2.22 /
       1.62, ripple_system 1.67 / 0.53 (ripples: rebuild 0.97 a run, every run; depths 1.74 at 1.81 runs a frame),
       step_bodies 0.91 / 0.58, flow_step 0.58 / 0.33, gather_colliders 0.51 / 0.26, smoke::step 0.45. shore_wetness is
       2-2.8 ms a run for ~8 s after arriving (profiles: 0.77 / max 2.81, then 3.36 ms a frame at 2.4-7.4 s, 0.45, 0.13),
       exactly as in round 25 (0.94 / 2.86, 3.14, ...): #587 didn't touch it, and it is the heaviest FixedUpdate system in
       the 2.4-7.4 s profile.
     the new extra view's state: 1.67 ms, 0 new shaders, layouts and pipelines (round 25: 2.75); the renderer's log of
       the first extra-view frame: CPU 20.14 ms (prepare space bodies 7.02, prepare side by side 3.85, prepare meshes
       3.81; round 25: 22.67).
     the profile covering it: 184 frames in 5 s (36.7 fps), worst 97.9 ms, render_system max 39.6 (round 25: 181 frames,
       36.2 fps, worst 91.8, render_system max 43.6).
   The four 10 s traces (traces/*_summary.txt): plaza 17.69 ms a frame (56.5 fps), market 20.59 (48.6), lake 26.67
   (37.5), fortress 31.79 (31.5); one system at a time 13.65 / 15.60 / 20.70 / 23.73 ms of it, 1.07-1.18 threads busy.
   render_system ran alone 4.69 / 4.92 / 10.83 / 6.45 ms a frame (the lake's is the mirror view); FixedUpdate 5.20 /
   7.11 / 8.57 / 13.10 ms a frame at 1.05 / 1.23 / 1.60 / 1.88 runs.
