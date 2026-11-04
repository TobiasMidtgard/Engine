Round 27 on Tobias's PC (RTX 5080, 32 threads, 64 GB), release, Ultra, window 1600x900 as the exe opens it, real time,
every showcase run with ENGINE_PROFILE=1 and --profile. All four runs on main ced07d57 (origin/main at the start: "Water:
a still lake with no stream under the grid isn't rebuilt every 2 s; ..."; has 151cb5ed "Spans inside the fortress
builders and the script frame", 1fba5139 ENGINE_EXECUTOR, 7c5ff56d "ColliderShape hull points and triangle meshes are
shared (Arc)", 2aaa2b3e; NOT Physics #591 "meshes streaming in under movers build off the frame; shapes shared", which
is still open). No asset manifest changes since round 26's d3cf57ad, so no fetch-assets. Build at below-normal priority
with nothing else running, 07:35:04-07:38:46 UTC, 3 m 40 s (build.txt). Every game run started only after 5+ min with
no input (33,375-33,782 s), one at a time, and ended with taskkill /IM showcase.exe /F. Input only through the game's
flags, environment and bridge. Runs 07:39-07:48 UTC, 2026-11-04. No loops, no night.
a-plaza-a1-a2-a3.txt         the three plaza runs side by side (ab27.py, 25-75 s; each run's full analysis first);
                             a-plaza-a1-a2-a3-30-75s.txt the same for 30-75 s
a-plaza-schedule-diffs.txt   diff27.py: per schedule, each trace's wall ms a frame and the systems that changed most
                             against a1 (a2 and a3). Its "systems' sum" counts nested systems twice (script_frame_system
                             runs around run_scripts), so read the wall and the per-system rows.
a1-plaza-plain, a2-plaza-single, a3-plaza-single-but-fixed   round 26 a's stand, each with a 120-frame trace at 40 s
b-fortress                   60 s fortress stand with a 60-frame trace at 20 s
scripts                      run27.ps1 = run26.ps1 plus a trace at a set second of a stand without dumps; chain27.ps1 /
                             wait_then.ps1 wait for 5 idle minutes; ab27.py (ab25.py with N runs side by side and every
                             schedule), diff27.py, inner27.py are new; the rest are round 26's
Every trace is a gzip'd Chrome trace (ui.perfetto.dev or scripts/trace-summary.py read .json.gz), 1.1-1.7 MB (9.8-14.3 MB
raw); scripts/trace-summary.py --frames --worst 3 output is beside each (traces/*_summary.txt).

Summary
a) Executor A/B, plaza 75 s stand, 25-75 s after arriving (30-75 s in brackets); means over the 9 profiles in each:
                                 a1 plain              a2 single             a3 single, FixedUpdate pooled
   fps (engine.status)           50.7 (51.6)           43.5 (44.5)           47.3 (48.1)
   frame ms (profiles)           19.46 (19.34)         22.66 (22.41)         21.17 (20.77)
   fixed steps a frame           1.18                  1.38                  1.27
   one system at a time ms       13.83                 20.23                 16.97
   no system ms                  1.82                  1.12                  1.60
   several at once ms            3.81                  1.60 *                2.61
   systems busy ms / cores       22.49 / 1.16          22.76 / 1.01          22.78 / 1.08
   worst frame ms                45.8 (31.7-84.2)      58.6 (41.5-72.7)      44.2 (39.8-56.8)
   schedule ms a frame / ordering floor:
     First                       0.29 / 0.20           0.27 / 0.23           0.25 / 0.21
     PreUpdate                   0.12 / n/a            0.09 / n/a            0.09 / n/a
     FixedPreUpdate              0.11 / 0.08           0.10 / 0.08           0.09 / 0.07
     FixedUpdate                 7.22 / 4.38           9.31 / 5.04           7.92 / 4.75
       runs a frame, ms a run    1.18, 6.13            1.34, 6.92            1.26, 6.29
       floor a run               3.72                  3.75                  3.77
     FixedPostUpdate             0.02 / n/a            0.01 / n/a            0.01 / n/a
     Update                      3.61 / 1.42           4.00 / 1.44           3.98 / 1.49
     PostUpdate                  2.97 / 1.78           3.83 / 1.84           3.77 / 1.83
     Last                        0.29 / 0.20           0.20 / 0.15           0.23 / 0.18
     Extract                     0.17 / 0.14           0.16 / 0.14           0.18 / 0.15
     Render                      4.61 / 4.58           4.62 / 4.61           4.61 / 4.57
   systems, ms a frame (max of a run):
     physics_simulate            0.95 (1.66)           1.05 (1.60)           1.08 (1.72)
     smoke::step                 1.27 (5.50)           1.32 (5.03)           1.37 (5.78)
     run_scripts                 1.16 (7.39)           1.17 (7.63)           1.21 (8.93)
     render_system               4.57 (9.60)           4.61 (9.04)           4.56 (9.81)
   render.cpu_ms (samples)       7.62                  7.79                  7.69
   traces (120 frames at 40 s; trace-summary.py):
     ms a frame (fps)            19.00 (52.6)          22.54 (44.4)          21.37 (46.8)
     idle / alone / several      1.90 / 14.62 / 2.47   1.25 / 21.29 / 0.00   1.70 / 18.52 / 1.15
     threads busy                1.09                  0.94                  1.00
     wall: FixedUpdate           6.65                  9.26                  8.03
           Update / PostUpdate   3.56 / 2.92           3.92 / 3.77           3.85 / 3.84
           First / Last          0.30 / 0.31           0.28 / 0.20           0.27 / 0.23
   (* the profile's "several at once" under single is mostly nested systems on the main thread, script_frame_system
   around run_scripts; trace-summary.py, which stacks spans per thread, gives 0.00. Two neighbouring a2 profiles read
   0.0 and 5.5 ms of it, systems busy 25.9 and 18.8, against 1.0-1.4 and 21.9-24.8 in the other seven; their systems
   busy averages 22.4, in line with the rest, and I don't know why they split. Without those two, a2's several at once
   is 1.27 ms.)
   The single-threaded executor is slower: -7.2 fps for every schedule, -3.4 fps with FixedUpdate left on the pool. It
   does save hand-offs: "no system" falls 1.82 -> 1.12 ms (1.60 in a3), and the small schedules shrink a little (Last
   0.29 -> 0.20, PreUpdate 0.12 -> 0.09, First 0.29 -> 0.27). But it loses the overlap: "several at once" 3.81 -> 1.60
   (2.61). In the traces Update's and PostUpdate's systems take about the same time either way; their walls grow by
   +0.36 / +0.84 ms (a2) and +0.29 / +0.91 (a3) because they no longer run side by side (a-plaza-schedule-diffs.txt). The
   longer frames then bring more fixed steps a frame (1.18 -> 1.34 / 1.26), and each fixed run costs more too (6.13 ->
   6.92 ms in a2; 6.29 in a3, where FixedUpdate stayed on the pool). In the traces think_and_act (0.25 -> 0.80 / 0.73
   ms a frame) and smoke::step (1.12 -> 1.46 / 1.64) grow most inside FixedUpdate in both a2 and a3; fire step_field
   shrinks (0.48 -> 0.17 in a2). Render is the same in all three (render_system 4.6 ms on the main thread).
   a1 against round 26 a (main d3cf57ad): 50.7 fps against 51.2, FixedUpdate 7.22 against 7.06 ms a frame.
b) Fortress, plain, --fly, goto fortress, 60 s stand; trace of 60 frames from 20.0 s after arriving (07:47:05.41;
   b-fortress/traces/trace_fortress_inner.txt, inner27.py):
   engine_destruction::build_system   3.65 ms a frame, 1 run a frame, max 6.51 (Update, on a compute thread; it ran alone
                                      3.64 ms a frame)
     build: builders                  3.64 ms a frame, 1 run a frame, max 6.50 (wraps nearly all of the system)
     build: pieces near               0.008 ms a frame, 2 runs a frame, max 0.02
     build: plan                      3.43 ms a frame, 2 runs a frame (1.71 a run), max 3.22
     build: ghosts and support        0.001 ms a frame, 1 run a frame, max 0.00
   engine_script::run_scripts         1.44 ms a frame, 1 run a frame, max 2.85 (main thread)
     scripts: serial                  0.68 ms a frame, max 2.04
     scripts: parallel                0.68 ms a frame, max 0.91
     scripts: apply parallel          0.009 ms a frame, max 0.01
     scripts: draw and profile        0.003 ms a frame, max 0.01
   So the builders' time is the two plans a frame; pieces near and ghosts and support are almost nothing.
   The trace overall: 47.86 ms a frame (20.9 fps; median 45.95, p90 65.45, max 75.68); no system 3.55, one system 36.62,
   several 7.68; 1.17 threads busy. FixedUpdate 23.79 ms a frame (2.80 runs, 8.50 a run), Render 10.30, Update 8.17,
   PostUpdate 3.87. Ran alone: render_system 10.24, build_system 3.64, npc rebuild_world_model 2.33, think_and_act 1.71,
   simulate_debris 1.48, run_scripts 1.44, smoke::step 1.11.
   The stand: 8.1, 13.3, 14.7, 16.2, 22.5, 23.8, 21.2, 23.6, 22.9, 19.0, 20.7, 18.5 fps per 5 s after arriving; 21.4 fps
   and 2.78 fixed steps a frame at 25-60 s. That is well under round 26's loops visit to the fortress (30.1 fps, 2.03
   fixed runs a frame; its 10 s trace 31.79 ms a frame, render_system alone 6.45, build_system 3.54). The two differ in
   how the camera got there (a --fly goto straight after loading against the sixth loops stop) and in main.
