Round 21c step 6: Rendering's asks, main 145dd3a6, Tobias's PC (RTX 5080), release, Ultra, window as the exe opens it
(1600x900). showcase.exe --fly --profile --quality ultra; through the bridge: plaza (35 s, the first stop also waits
out the startup), forest_overview 20 s, fortress 20 s (the "fortress" place is the view over the fortress yard),
forest_trail 20 s, then from there 20 s walking forward along the trail (input.axis move y=1: the fly camera at ground
speed, ~7 m/s). Standing-still numbers are the last 10 s of each stay (10-20 s after arriving; plaza 25-35 s).
- triangles: the "mesh triangles" line the profiler logs every 5 s (the last one 10+ s after arriving).
- GPU per shadow pass: the GPU has one timestamp for all sun cascades plus the far shadow ("shadows") and one for sky
  occlusion; cascades are not timed apart. Render profile 5 s averages.
- meshes: cull / wait for the finishing threads / prepare meshes: the frame profile's "inside systems" rows,
  ms a frame / max ms (5 s).
Caveat: physics was heavy in this run. Physics budget sat at levels 1-3 from 00:35:42 to 00:37:00, and
physics_simulate ran alone 11-12 ms a frame at plaza, forest_overview and fortress (7-8 ms at the trail) against
1-3 ms in other runs. So the fps here is lower than step 5's (plaza 21.9 here vs 27.6). The render numbers are not
affected.

stop | fps | frame ms | GPU ms | GPU shadows / sky occlusion ms | meshes: cull | wait for the finishing threads | prepare meshes | mesh triangles line
plaza | 21.9 | 45.6 | 27.37 | 1.99 / 0.14 | 1.10 / 2.02 | 1.34 / 2.16 | 2.22 / 2.91 | view 6.36 M + 1652 impostors, cascade 0 0.03 M + 0.54 M still (398 frames ago), cascade 1 0.08 M + 4.27 M still (333), cascade 2 1.52 M + 8.69 M still (12), cascade 3 1.86 M + 12.76 M still (12), sky occlusion 1.05 M + 5.11 M still (12), far shadow 0.36 M + 5.75 M still (39); clusters 0.92 M
forest_overview | 23.3 | 42.9 | 29.29 | 3.72 / 0.33 | 1.03 / 1.83 | 0.89 / 2.24 | 2.41 / 5.34 | view 5.59 M + 1584 impostors, cascade 0 0.32 M + 0.05 M still (12), cascade 1 1.51 M + 0.92 M still (12), cascade 2 1.38 M + 9.31 M still (12), cascade 3 0.90 M + 13.19 M still (12), sky occlusion 2.22 M + 2.63 M still (12), far shadow 0.64 M + 5.61 M still (12); clusters 0.38 M
fortress | 16.4 | 60.9 | 30.03 | 1.83 / 0.09 | 1.45 / 2.70 | 1.43 / 2.69 | 2.63 / 5.96 | view 4.72 M + 1564 impostors, cascade 0 0.00 M + 0.19 M still (3), cascade 1 0.05 M + 0.92 M still (3), cascade 2 0.62 M + 4.14 M still (3), cascade 3 1.64 M + 11.61 M still (3), sky occlusion 0.32 M + 0.91 M still (3), far shadow 0.40 M + 4.75 M still (3); clusters 1.49 M
forest_trail | 23.9 | 41.9 | 34.06 | 5.31 / 0.40 | 1.10 / 1.85 | 0.85 / 1.83 | 2.54 / 4.40 | view 9.85 M + 1743 impostors, cascade 0 1.26 M + 0.19 M still (0), cascade 1 3.40 M + 0.46 M still (0), cascade 2 1.85 M + 6.97 M still (0), cascade 3 0.51 M + 13.68 M still (0), sky occlusion 3.64 M + 3.39 M still (0), far shadow 1.13 M + 6.04 M still (1); clusters 0.00 M
walk (trail) | 22.3 | 44.9 | 34.25 | 5.79 / 0.66 | 1.36 / 2.45 | 0.98 / 2.24 | 3.25 / 5.40 | view 8.07 M + 1105 impostors, cascade 0 1.25 M + 0.15 M still (2), cascade 1 1.72 M + 0.33 M still (0), cascade 2 0.97 M + 5.57 M still (0), cascade 3 0.69 M + 12.45 M still (0), sky occlusion 5.03 M + 3.01 M still (0), far shadow 2.30 M + 5.94 M still (1); clusters 0.00 M
(numbers in brackets after "still": frames since that static image was drawn)

Reading:
- At the fortress the static images were redrawn 3 frames before the line, all six at once. At forest_trail they were
  redrawn every frame (0 frames ago) while standing still. Under the trees there are no clusters (0.00 M) and 9.85 M
  triangles in view, the most of any stop.
- The trail's GPU frame is the heaviest (34 ms): shadows 5.3 ms (vs 1.8-2.0 at plaza/fortress) and prepass 3.6.
- prepare meshes 2.2-2.6 ms standing, 3.25 walking; meshes: cull 1.0-1.5; wait for the finishing threads 0.9-1.4.

Probe GI spikes in the forest. Standing at forest_trail, "slow render frame" lines come every 3-4 s, led by
probe gi 71.3, 42.8, 46.2 and 83.5 ms (00:37:02-00:37:14). They go on while walking: 50.3, 66.2, 53.0 ms. No other stop
has them; elsewhere the slow frames are screenshots and the first frames' "prepare character". render.gi at the trail:
probes_updated 128 (vs 962-2048 at the other stops), building 2, build_ms 2.93, extract_ms 3.88, gpu_ms_per_kray 0.44
(0.00-0.03 elsewhere), 2326 instances.

Walking the forest trail (20 s): fps 22.3, frame 44.9 ms, 1% low 11.8 (0.5 s samples), slowest 0.5 s sample 84 ms a frame.
Worst frame 260.3 ms (the 5 s profile ending 00:37:21), then 239.0, 236.7, 230.3.
Biggest system maxes in those profiles:
- engine_water::shelter_system 214.9, 207.1, 205.2 ms (avg 1.8-4.2)
- engine_render::render_system 59-81 ms (the probe GI spikes)
- engine_script::draw_profile_boards 37 ms
- engine_npc::rebuild_world_model 26-29 ms
So the walk's worst frames are water's shelter_system (~210 ms) plus probe GI.

Files: analysis.txt (every number per stop, with each frame profile's render steps), plaza/forest_overview/fortress/
forest_trail.jpg, frames.csv, timeline.txt, nvidia_dmon.txt, logs.zip (stderr with every profile, stats JSON per stop).
