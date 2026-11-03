Round 23 on Tobias's PC (RTX 5080, 32 threads, 64 GB), main 70b02d16 (no asset manifest changes since round 22's
5de4f33e, so no fetch-assets), release, Ultra, window 1600x900 as the exe opens it, real time, every showcase run with
--profile. Every game run was started only after 5+ min with no input (317-1641 s), one at a time (the build ran
before any of them, at below-normal priority), and ended with taskkill /IM showcase.exe /F. Input only through the
game's flags and its bridge. Runs 22:28-22:54 UTC on 2026-11-03.
1-plaza-render-thread   ENGINE_PROFILE=1 --fly, 75 s plaza stand: render.wait/extract/record/cpu keys twice a second,
                        fixed steps a frame, fps, the ordering floor blocks
2-route-full, 2-route-up  --route and --route up: leg tables, hitches, RAM per leg, a picture per leg; offframe.* every
                        frame joined to the worst frames; route up: bridge pictures at 8, 16, 25 (~28) and 42 km
3a-loops-ultra          loops x2 at Ultra with PHYSICS_REPORT=240: tables, per-pass GPU (weather clouds row), physics
                        reports with awake lists, lake flow_bed/ripple rows, plaza/field stream_far_woods
3b-render-scale-0.7     the same at ENGINE_RENDER_SCALE=0.7, per-pass GPU 1.0 vs 0.7
4-night                 showcase.time to 22.5, 2.5 and 3.5 h; plaza, night, sunset and lake views; the moons
scripts                 the drivers and analyses (run23.ps1 is round 22's driver plus several fast prefixes, route
                        shots at set altitudes and a night mode; wait_then.ps1 / chain23.ps1 wait for 5 idle minutes)

Summary
Step 1 answer: no, the plaza is not bound by the render thread. render.wait_ms is 0.01 ms (mean, p90 and max over the
steady 25-75 s), and render.record_total_ms is 6.2 ms a frame (p90 6.8) of a 31.6 ms frame (0.20 of it; the render
thread's CPU 3.2 ms). The main thread is the bound: systems busy 33-34 ms a frame on ~1.1 of 32 cores, the schedules
in series (FixedUpdate 12.0-16.1 at 1.9 runs, Render = render_system = the main thread's extract 9.0-9.6, Update
3.6-4.3, PostUpdate 3.1-3.6); ordering floors Render 9.0-9.5, FixedUpdate 8.3-10.9, PostUpdate 1.9-2.2, Update 1.3-1.6.
GPU 16.3 ms of passes, 56% busy. offframe.* is 0 in the steady state.

Steady fps / frame ms per step:
- 1 plaza stand: 31.9 fps (engine.status, 1.88 fixed steps a frame), 31.6 ms (29.0-34.6 per 5 s profile).
- 2 route: per-leg mean ms village 77.0, climb to 2 km 37.8 (r22 46.8), cruise at 2 km 26.8, climb to orbit 19.1,
  orbit 17.2, descend 19.8, cruise home 30.2, down to 100 m 21.7, low pass 41.0 (r22 50.8), end 20.7; 626 frames over
  33 ms of 3071 (r22 628 of 3184). RAM 4259 MB at the start, 7297 max, 4857 at the end.
  route up: climb to orbit 26.5 ms mean (median 17.8; above 12.7 km 16.7), orbit 16.1; 165 over 33 ms of 853.
- 3 loops, round 2 at Ultra (fps / frame ms): overview 44.9 / 22, plaza 40.6 / 25, market 28.8 / 35, forest
  29.2 / 34, lake 23.7 / 42, fortress 16.0 / 62, field 39.3 / 25, orbit 64.6 / 15 (round 22's within ~2.5 fps).
  At 0.7: 52.7, 41.1, 31.7, 30.5, 24.0, 17.4, 40.0, 65.0: only the GPU-bound overview gains (+8).
  Weather clouds (GPU) 0.81-0.98 ms at the ground stops (round 22 1.92-2.29; 0.62-0.75 at 0.7), 0.16 at orbit.
- 4 night (not a timing run): plaza 23-37 fps, night view 14-25, sunset view 14-16 (main thread: render_system 21 ms,
  FixedUpdate 31 ms at 3.7 runs), lake 23-25.

Worst hitches:
- engine_npc::think_and_act single runs of 415.0 ms (loops round 2, arriving at the field), 306.0 (round 2 plaza,
  the 328 ms frame), 231.7 (step 1, the 279 ms frame), 202.8 and 184.2 (round 2 field), 173.7 and 150.3 (0.7 run,
  plaza). The biggest recurring single-system hitch this round.
- Off-frame applies at the start of the routes: 247.4 ms in one frame (route up, frame 62, a 237.7 ms frame), 89.2,
  60.7 (full route, village, a 209.8 ms frame), 30.6; offframe.* is 0 in every frame after the first ~1.5 s of a route,
  so none of the later route hitches is off-frame work.
- Route, after the start: climb to 2 km 97.0 ms at 28 m (render CPU 18; ripple_system max 25.9, softbody
  step_beside_physics 4-17 ms a frame), low pass 83.6 at 332 m (ripple_system max 31.7), down to 100 m 80.6.
- Lake, on arriving: water::ripple_system 104.7 ms once (round 1; 80.1 round 2, 82.7/74.9 at 0.7), with
  softbody::step_beside_physics 104.7 in the same profile; after that ripple 0.8-1.4 ms a frame and flow_bed_system
  0.13-0.20.
- stream_far_woods: no hitch at the field or the plaza (highest run 4.26 ms, on arriving at the plaza).

Also: near cumulus fade out on the way up between ~16 and ~42 km (at 42 km only specks at the bottom edge; the far
flat layer takes over without a visible seam; the same as round 22). The night sky has two moons, in different places:
the weather's moon (a full ~1.2 deg disc, the scene's light) and the space Moon (a ~0.5 deg crescent, 18-19 deg higher
and 9-12 deg further south at 2.5-3.5 h, drawn as a crescent though it is 170 deg from the Sun). No picture holds both
(4-night/README.txt).
