Round 22 step 3: GPU occlusion culling of instanced meshes, ENGINE_MESH_OCCLUSION=on vs off, main 5de4f33e, Tobias's PC
(RTX 5080), release, Ultra, window 1600x900. showcase.exe --fly --profile --quality ultra, then through the bridge
forest_trail 35 s (the first stop takes the startup), plaza 30 s, market 30 s, village_garden 30 s, a picture at each
stay's end. Four runs, one after another: on, off, on, off (on_r1, off_r1, on_r2, off_r2), so every stop (the market
too) has two pictures per setting.
"town street" is not a showcase place (no such key in places.rs, the zone demos' views or the docs); village_garden
(behind a house on the village's south side, among the houses) stands in for it.
render.meshes.triangles does not exist; render.meshes.* has occlusion_tested / occlusion_drawn / occlusion_triangles.
For the meshes' triangles the render profile's "features (last frame)" row for meshes and the mesh_feature
"mesh triangles: view N M" log line stand in (both CPU-side counts, before the GPU's occlusion test).

Per stop (occlusion.txt): fps over the stay's last 10 s; GPU ms from the last render profile in the stay (5 s average);
engine.stats render.meshes.* twice a second from 10 s after arriving, mean.
stop           | setting | fps         | GPU total   | scene: meshes | prepass   | prepass: meshes | occlusion culling | tested / drawn (drawn %) | occlusion tris | view tris (CPU)
forest_trail   | on      | 33.6 / 34.3 | 26.84 / 26.70 | 5.46 / 5.53 | 3.36 / 3.26 | 1.93 / 1.83 | 0.14 / 0.14 | 7,932 / 3,100 (39%)  | 6.23 M | 8.67 M
               | off     | 23.4 / 33.9 | 28.38 / 27.48 | 5.53 / 5.76 | 3.66 / 3.68 | 2.23 / 2.25 | 0.07 / 0.07 | -                    | -      | 8.72 M
plaza          | on      | 38.3 / 38.8 | 18.25 / 18.27 | 4.31 / 4.34 | 2.23 / 2.21 | 1.40 / 1.38 | 0.13 / 0.13 | 7,815 / 5,114 (65%)  | 5.00 M | 5.90 M
               | off     | 34.1 / 38.2 | 18.69 / 18.28 | 4.42 / 4.43 | 2.40 / 2.35 | 1.57 / 1.52 | 0.07 / 0.07 | -                    | -      | 6.72 / 5.93 M
market         | on      | 33.7 / 32.5 | 21.88 / 21.85 | 3.19 / 3.23 | 3.08 / 3.06 | 1.15 / 1.13 | 0.15 / 0.15 | 7,660 / 4,266 (56%)  | 4.25 M | 5.97 M
               | off     | 31.9 / 35.0 | 22.23 / 22.17 | 3.36 / 3.37 | 3.30 / 3.31 | 1.38 / 1.38 | 0.07 / 0.07 | -                    | -      | 5.95 M
village_garden | on      | 39.2 / 40.1 | 24.00 / 23.65 | 2.74 / 2.74 | 2.78 / 2.71 | 0.44 / 0.43 | 0.13 / 0.13 | 4,860 / 693 (14%)    | 1.76 M | 4.68 M
               | off     | 39.0 / 38.8 | 24.28 / 24.62 | 3.10 / 3.13 | 3.12 / 3.26 | 0.96 / 0.98 | 0.07 / 0.07 | -                    | -      | 4.65 M
(run 1 / run 2 of each setting; occlusion figures are run 1's, run 2's agree within 1.5%: occlusion.txt has both and
the ranges.)
- GPU saved with occlusion on (run means): forest_trail 1.2 ms of the total (prepass: meshes -0.36, prepass -0.36,
  scene: meshes -0.15, compute meshes 1.73/1.64 vs 2.74/1.70), plaza 0.2 (prepass: meshes -0.16), market 0.3
  (prepass: meshes -0.24, scene: meshes -0.16), village_garden 0.6 (prepass: meshes -0.54, scene: meshes -0.38). The
  occlusion culling pass costs 0.13-0.15 ms on, 0.07 off (the row is there in both).
- fps: on and off agree within run-to-run spread except off_r1's forest_trail (23.4): that stay had
  physics_simulate 15.7 ms a frame (the round-2 physics slowdown of step 2b), not the culling.
- Pictures (picture_diffs.txt, PNG pairs): on vs off differ as much as two runs with the same setting. Market: on vs off
  0.33-1.17% of pixels off by more than 24, mean abs diff 1.9-2.7; on vs on 0.30%, off vs off 1.32%. market_diffs/ has
  the brightened diffs: grass sway and clouds, no object missing. Plaza 0.51-0.64% in every pair; forest_trail and
  village_garden 11-24% in every pair (foliage in the wind). Both settings show the same pale band at the market
  picture's left edge (x 0-100).

Files: occlusion.txt, picture_diffs.txt, market_diffs/, and per run (on_r1, off_r1, on_r2, off_r2): passes.txt,
timeline.txt, frames.csv, sampled_stats.jsonl (render.meshes.* and render.gpu.* twice a second), nvidia_dmon.txt, jpg/,
logs.zip (stderr, engine-profile.txt, stats JSON per stop).
