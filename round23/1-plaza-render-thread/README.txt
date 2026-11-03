Round 23 step 1: plaza stand in the full world, main 70b02d16, Tobias's PC (RTX 5080, 32 threads, 64 GB), release, Ultra,
window as the exe opens it (client 1600x900, not maximized), real time. ENGINE_PROFILE=1 showcase.exe --fly --profile
--quality ultra; showcase.goto plaza 5 s after the world was built (22:29:08 UTC), stay 75 s. Started after 317 s with no
input. Twice a second: engine.status (frame, fixed_tick, fps) and engine.stats "render." and "offframe." (each answer is
one frame's, with its frame number).

Steady state = 25-75 s after arriving (22:29:33-22:30:23 UTC), 98 render.* samples (render_thread.txt lists all 155):
key                       | mean  | median | p90   | max
frame_ms (Diagnostics)    | 31.56 | 30.96  | 34.25 | 42.52
render.wait_ms            |  0.01 |  0.01  |  0.01 |  0.01
render.extract_ms         |  9.62 |  9.11  | 10.85 | 38.65
render.extract_wait_ms    |  0.00 |  0.00  |  0.00 |  0.00
render.record_ms          |  3.05 |  2.96  |  3.62 |  5.17
render.record_total_ms    |  6.16 |  6.06  |  6.83 |  8.66
render.record_cpu_ms      |  3.22 |  3.12  |  4.60 |  6.63
render.cpu_ms             | 12.76 | 12.34  | 14.17 | 42.18
(record_total's other parts: finish 0.43, submit 0.47, present 0.07, record_other 2.08.)
record_total_ms / frame_ms: mean 0.20 (p90 0.23, max 0.29). engine.status over the span: 31.9 fps, 1.88 fixed steps a
frame (1582 frames in 49.6 s). The 5 s frame profiles: 29.0-34.6 ms a frame (28.9-34.5 fps).
offframe.*: 0 in every steady sample (pending 35-44 in the first ~2 s at the plaza, then nothing).

The answer: no, the plaza is not bound by the render thread. render.wait_ms is 0.01 ms (the main thread never waits
for the render thread), and record_total_ms is ~6.2 ms of a ~31 ms frame (the render thread's CPU 3.2 ms). The main
thread is the bound: the profiles show systems busy 33-34 ms a frame on 1.08-1.10 of 32 cores, the schedules one
after another: FixedUpdate 12.0-16.1 ms a frame (x1.8-1.9 runs), Render 9.0-9.6 (render_system alone, the main
thread's extract: render.extract_ms 9.6), Update 3.6-4.3, PostUpdate 3.1-3.6. The GPU runs 16.3 ms of passes
(passes.txt, render profile 22:30:15; weather clouds 0.96) and is 56% busy (nvidia-smi).

Ordering floor (ordering_floor.txt: the block of all 10 steady profiles; analysis.txt: every profile of the run).
Steady ranges, floor of the schedule's ms a frame:
  Render         9.00-9.54  of  9.04-9.58
  FixedUpdate    8.29-10.88 of 11.95-16.13 (1.8-1.9 runs a frame)
  PostUpdate     1.87-2.19  of  3.06-3.60
  Update         1.31-1.64  of  3.61-4.29
  First          0.21-0.24  of  0.30-0.33
  Last           0.21-0.23  of  0.31-0.33
  Extract        0.15-0.16  of  0.19-0.20
  FixedPreUpdate 0.11-0.13  of  0.14-0.17
The FixedUpdate chain is round 22's: npc rebuild_world_model -> think_and_act -> boat_air_system (0.77) ->
wheels_system -> physics_sync -> physics_simulate (2.17-2.62) -> physics_write_back -> softbody gather_colliders ->
step_bodies (~1.0) -> write_posed_guides -> water sync_bodies_system -> flow_bed_system (2.15-2.21) ->
disturbance_system -> shore_wetness_system. Against round 22 step 6 (31.6 fps, 1.89 steps): the same within noise.

Hitch: one 279 ms frame ~37 s after arriving (profile 22:29:48): engine_npc::think_and_act ran 231.7 ms once (2.31 ms
a frame over that profile; 0.27-0.66 in the others). render.* that second: extract 9.2-10.5, record_total 5.9-6.8.

Files: render_thread.txt (every sample and the steady summary), ordering_floor.txt, analysis.txt (21d's tour script:
each profile's schedules, physics_simulate and ordering floor), passes.txt, frames.csv, timeline.txt, nvidia_dmon.txt,
plaza.jpg, logs.zip (stderr.log, engine-profile.txt, sampled_stats.jsonl, stats_*.json).
