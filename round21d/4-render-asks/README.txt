Round 21d step 4: Rendering's asks, main 1810b539, Tobias's PC (RTX 5080), release, Ultra, window as the exe opens it
(1600x900). showcase.exe --fly --profile --quality ultra; through the bridge:
- mirror-on/: ENGINE_MIRROR_SHARES_LIGHTING=on (the default): plaza 35 s (the first stop waits out the startup), forest
  30 s, forest_trail 25 s, lake 30 s. (Its plaza and forest are also step 5's default row.)
- mirror-off/: ENGINE_MIRROR_SHARES_LIGHTING=off: plaza 35 s, lake 30 s. Run right after, the same build.
engine.stats render.gi.*, render.meshes.* and render.extra_views* sampled twice a second (sampled_stats.jsonl); below,
mean / max over each stay from 10 s after arriving to the stay's end. probe gi ms: the render profile's "probe gi" render
CPU step and GPU pass (5 s averages), every profile in that span. fps etc.: the last 10 s of the stay.

stop (mirror-on run) | plaza | forest_trail | lake | (forest)
fps / frame ms | 31.2 / 32.1 | 33.2 / 30.1 | 27.6 / 36.2 | 40.3 / 24.8
probe gi CPU ms (profiles) | 1.31, 1.31 | 2.14 | 1.18, 1.13 | 1.66, 1.56
probe gi GPU ms | 1.81, 1.82 | 3.70 | 0.11, 0.11 | 1.94, 1.90
render.gi.full_gather (mean / max) | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0
render.gi.read | 166.5 / 256 | 254.1 / 358 | 138.2 / 140 | 1171.9 / 1717
render.gi.changed | 5166.6 / 17978 | 4572.9 / 6817 | 3536.1 / 4462 | 4680.8 / 16569
render.gi.rebuilt | 0 / 0 | 0.067 / 1 (2 of 30 samples) | 0.075 / 1 (3 of 40) | 0.05 / 1 (2 of 40)
render.gi.building | 0 / 0 | 0.43 / 3 | 0.03 / 1 | 0.05 / 2
render.gi.build_ms / extract_ms | 0.95 / 1.22 | 1.29 / 1.73 | 0.80 / 1.09 | 1.18 / 1.55
render.gi.probes_updated | 1853 / 2048 | 128 / 128 | 2048 / 2048 | 1118 / 1938
render.gi.gpu_ms_per_kray | 0.016 | 0.461 | 0.001 | 0.028
render.gi.instances | 332 | 2333 | 137 | 2177
render.meshes.edits | 0 / 0 | 0.5 / 5 (3 of 30) | 0 / 0 | 0.98 / 13 (10 of 40)
render.meshes.uploads | 50.8 / 210 | 104.6 / 115 | 41 / 41 | 136.7 / 211
render.meshes.dirty_slots | 5269 / 7584 | 4349 / 6595 | 0 / 0 | 5045 / 7844
render_system ms (last frame profile) | 7.27 (0.8 runs/frame) | 11.31 (max 51.57) | 17.26 | 8.36
GPU ms (render profile) | 21.91 | 29.08 | 18.63 | 19.93
Probe GI spikes at forest_trail: one "slow render frame" led by probe gi in the 16 s (66.5 ms of 79, 05:55:32); in 21c
they came every 3-4 s (71.3, 42.8, 46.2, 83.5 ms). render.gi.full_gather was 0 in every sample at every stop.

Mirror at the lake (render.extra_views 1 throughout both):
ENGINE_MIRROR_SHARES_LIGHTING | render.extra_views_ms mean / max | render.extra_views_gpu_ms mean / max | render_system ms | lake fps | GPU ms (main passes)
on  |  9.06 / 10.71 | 5.47 / 5.65 | 17.26 | 27.6 | 18.63
off | 11.65 / 14.96 | 8.93 / 10.02 | 19.68 | 25.1 | 18.48
Sharing the main view's lighting saves 2.6 ms of CPU and 3.5 ms of GPU in the extra view (21c on 145dd3a6: 10.85 / 11.37).
The plaza in both runs agrees (31.2 / 31.4 fps, GPU 21.91 / 21.95, probe gi 1.31-1.33 / 1.80-1.82).

Per run: render_asks.txt (every sampled stat per stay, the probe gi profiles, the slow render frames), analysis.txt (21c's
stops analysis: render steps, mesh triangles, terrain at END), passes.txt (per-pass GPU table), sampled_stats.jsonl,
frames.csv, timeline.txt, nvidia_dmon.txt, a picture per stop, logs.zip (stderr, stats JSON per stop).
