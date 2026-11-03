Round 21c step 5: terrain GPU (Planets), main 145dd3a6, Tobias's PC (RTX 5080), release.
showcase.exe --fly --profile --quality ultra|high [--size 800x450]; showcase.goto plaza, stand 35 s; showcase.goto forest,
stand 30 s. Numbers from the last 10 s of each stay (plaza 25-35 s, forest 21-31 s after arriving; terrain patches
building 0 and missing 0 at both by then). GPU ms and "scene: terrain" are the render profile's 5 s averages (--profile
splits the scene pass by feature); patches from engine.stats terrain.* at the end of the stay.
"patches drawn" = terrain.patches_drawn; "terrain draws" = the render profile's terrain feature draws/triangles (all
its passes) in the last frame. The half window was run twice: run 1's plaza had physics_simulate ~10 ms a frame (physics
budget at levels 2-3 through the window, ran alone 10.6 / 9.8 ms vs 1.8-2.6), so its fps is physics, not resolution.

config | stop | fps | frame ms | GPU ms | scene: terrain GPU ms | patches resident | patches drawn | terrain draws (tris) | GPU busy %
Ultra 1600x900 | plaza | 27.6 | 36.3 | 27.40 | 5.39 | 1669 | 751 | 1581 (1.73 M) | 79
Ultra 1600x900 | forest | 32.7 | 30.6 | 28.53 | 8.37 | 1575 | 700 | 1424 (1.61 M) | 91
Ultra 800x450 run 2 | plaza | 28.7 | 34.8 | 19.78 | 3.22 | 1669 | 751 | 1581 (1.73 M) | 59
Ultra 800x450 run 2 | forest | 37.2 | 26.9 | 18.37 | 4.67 | 1575 | 700 | 1424 (1.61 M) | 71
Ultra 800x450 run 1 | plaza | 20.5 | 48.7 | 19.75 | 3.21 | 1669 | 751 | 1581 (1.73 M) | 44
Ultra 800x450 run 1 | forest | 25.6 | 39.0 | 19.22 | 4.66 | 1575 | 700 | 1424 (1.61 M) | 53
High 1600x900 | plaza | 36.6 | 27.3 | 26.14 | 5.41 | 1581 | 700 | 1443 (1.61 M) | 100
High 1600x900 | forest | 35.9 | 27.8 | 26.37 | 8.50 | 1563 | 702 | 1361 (1.62 M) | 100

Reading:
- A quarter of the pixels takes terrain from 5.39 to 3.22 ms (plaza) and 8.37 to 4.67 ms (forest), 56-60%. A fixed
  part plus a per-pixel part fits it: plaza 2.5 ms + 2.9 ms per 1600x900, forest 3.4 ms + 4.9 ms. So ~40-45% of the
  terrain's GPU time doesn't depend on resolution (geometry, patches); the rest does.
- High draws the terrain at the same GPU cost as Ultra (5.41 / 8.50 ms) with about the same patches (1581/700 vs
  1669/751 at the plaza). The whole GPU frame is also nearly the same at High (26 vs 27-28 ms).
- At 800x450 the GPU frame is 18-20 ms and the GPU 59-71% busy: the frame is then CPU-bound (render CPU 13-16 ms,
  systems busy 30-40 ms a frame).

Per run: analysis.txt (all numbers above plus each stop's frame profile render steps and its mesh triangles line),
plaza.jpg and forest.jpg, frames.csv, timeline.txt, nvidia_dmon.txt, logs.zip (stderr, engine-profile, stats JSON).
######## ultra-1600x900
== plaza: window 00:25:35-00:25:45 (26-36 s after arriving)
fps 27.6, frame 36.3 ms, 1% low 21.5 (0.5 s samples), slowest 0.5 s sample 46 ms/frame, worst frame 130.2 ms (2 profiles), GPU busy 79%
frame profile 00:25:45: 133 frames in 5.0 s: 37.8 ms per frame (26.5 fps), worst 130.2 ms
  render_system (ms/frame, max, runs): 11.45 / 16.39 / 1.0
  prepare meshes: 2.23 / 3.30 / 1.0
  prepare on the render thread: 2.06 / 5.37 / 1.0
  meshes: cull: 1.12 / 3.42 / 1.0
  wait for the finishing threads: 1.26 / 1.72 / 1.0
  meshes: shadow lists: 0.35 / 0.85 / 1.0
  prepare terrain: 1.24 / 2.22 / 1.0
render profile 00:25:44 (5 s avg): GPU 27.40 ms: scene: terrain 5.39, shadows (all cascades + far shadow, one timestamp) 1.85, sky occlusion 0.15, scene 10.92, scene: meshes 4.71, prepass 2.46; render CPU 15.90 ms; terrain draws 1581 (1730304 triangles) last frame
mesh triangles 00:25:44: view 6.12 M + 1918 impostors, cascade 0 0.03 M + 0.56 M still (482 frames ago), cascade 1 0.08 M + 4.29 M still (419 frames ago), cascade 2 0.98 M + 9.03 M still (1 frames ago), cascade 3 0.85 M + 13.40 M still (1 frames ago), sky occlusion 1.03 M + 5.22 M still (19 frames ago), far shadow 0.01 M + 6.17 M still (1 frames ago); clusters 0.93 M
at END: terrain.patches_resident 1669.0, patches_drawn 751.0, building 0.0, missing 0.0; render.gpu.scene: terrain 5.63 ms, gpu.shadows 1.39, gpu.sky occlusion 0.14, gpu_ms 25.11 (one frame)
== forest: window 00:26:07-00:26:17 (21-31 s after arriving)
fps 32.7, frame 30.6 ms, 1% low 25.7 (0.5 s samples), slowest 0.5 s sample 39 ms/frame, worst frame 51.8 ms (1 profiles), GPU busy 91%
frame profile 00:26:15: 166 frames in 5.0 s: 30.2 ms per frame (33.1 fps), worst 51.8 ms
  render_system (ms/frame, max, runs): 9.17 / 12.23 / 1.0
  prepare meshes: 2.64 / 3.69 / 1.0
  prepare on the render thread: 2.16 / 3.63 / 1.0
  meshes: cull: 1.19 / 2.22 / 1.0
  wait for the finishing threads: 1.12 / 1.98 / 1.0
  meshes: shadow lists: 0.39 / 0.86 / 1.0
  prepare terrain: 1.10 / 3.01 / 1.0
render profile 00:26:14 (5 s avg): GPU 28.53 ms: scene: terrain 8.37, shadows (all cascades + far shadow, one timestamp) 2.82, sky occlusion 0.20, scene 14.12, scene: meshes 5.24, prepass 2.65; render CPU 15.35 ms; terrain draws 1424 (1612800 triangles) last frame
mesh triangles 00:26:14: view 5.70 M + 1904 impostors, cascade 0 0.32 M + 0.05 M still (282 frames ago), cascade 1 1.51 M + 0.92 M still (72 frames ago), cascade 2 1.50 M + 9.70 M still (23 frames ago), cascade 3 1.00 M + 13.38 M still (23 frames ago), sky occlusion 2.01 M + 2.59 M still (23 frames ago), far shadow 0.63 M + 5.65 M still (0 frames ago); clusters 0.37 M
at END: terrain.patches_resident 1575.0, patches_drawn 700.0, building 0.0, missing 0.0; render.gpu.scene: terrain 8.43 ms, gpu.shadows 3.03, gpu.sky occlusion 0.21, gpu_ms 29.09 (one frame)
######## ultra-800x450-run1
== plaza: window 00:27:42-00:27:52 (25-35 s after arriving)
fps 20.5, frame 48.7 ms, 1% low 14.3 (0.5 s samples), slowest 0.5 s sample 70 ms/frame, worst frame 103.7 ms (2 profiles), GPU busy 44%
frame profile 00:27:52: 97 frames in 5.0 s: 51.7 ms per frame (19.4 fps), worst 101.3 ms
  render_system (ms/frame, max, runs): 11.85 / 17.44 / 1.0
  prepare meshes: 2.22 / 3.67 / 1.0
  prepare on the render thread: 2.05 / 4.17 / 1.0
  meshes: cull: 1.08 / 2.06 / 1.0
  wait for the finishing threads: 1.33 / 1.89 / 1.0
  meshes: shadow lists: 0.39 / 1.78 / 1.0
  prepare terrain: 1.30 / 2.28 / 1.0
render profile 00:27:50 (5 s avg): GPU 19.75 ms: scene: terrain 3.21, shadows (all cascades + far shadow, one timestamp) 2.08, sky occlusion 0.15, scene 6.28, scene: meshes 2.42, prepass 1.97; render CPU 15.89 ms; terrain draws 1581 (1730304 triangles) last frame
mesh triangles 00:27:50: view 5.80 M + 1590 impostors, cascade 0 0.03 M + 0.57 M still (385 frames ago), cascade 1 0.08 M + 4.38 M still (319 frames ago), cascade 2 1.70 M + 8.97 M still (12 frames ago), cascade 3 2.05 M + 12.94 M still (12 frames ago), sky occlusion 1.01 M + 5.07 M still (12 frames ago), far shadow 0.19 M + 5.04 M still (1 frames ago); clusters 0.55 M
at END: terrain.patches_resident 1669.0, patches_drawn 751.0, building 0.0, missing 0.0; render.gpu.scene: terrain 3.21 ms, gpu.shadows 1.62, gpu.sky occlusion 0.13, gpu_ms 17.55 (one frame)
== forest: window 00:28:13-00:28:23 (21-31 s after arriving)
fps 25.6, frame 39.0 ms, 1% low 20.7 (0.5 s samples), slowest 0.5 s sample 48 ms/frame, worst frame 59.9 ms (1 profiles), GPU busy 53%
frame profile 00:28:22: 132 frames in 5.0 s: 38.1 ms per frame (26.3 fps), worst 59.9 ms
  render_system (ms/frame, max, runs): 8.85 / 13.95 / 1.0
  prepare meshes: 2.32 / 3.11 / 1.0
  prepare on the render thread: 1.83 / 2.66 / 1.0
  meshes: cull: 0.94 / 1.59 / 1.0
  wait for the finishing threads: 0.92 / 1.41 / 1.0
  meshes: shadow lists: 0.35 / 0.63 / 1.0
  prepare terrain: 1.12 / 2.67 / 1.0
render profile 00:28:20 (5 s avg): GPU 19.22 ms: scene: terrain 4.66, shadows (all cascades + far shadow, one timestamp) 2.92, sky occlusion 0.21, scene 8.16, scene: meshes 3.07, prepass 1.46; render CPU 13.10 ms; terrain draws 1424 (1612800 triangles) last frame
mesh triangles 00:28:20: view 4.35 M + 2789 impostors, cascade 0 0.32 M + 0.05 M still (186 frames ago), cascade 1 1.53 M + 0.96 M still (19 frames ago), cascade 2 1.68 M + 9.81 M still (17 frames ago), cascade 3 1.03 M + 13.35 M still (17 frames ago), sky occlusion 2.10 M + 2.08 M still (17 frames ago), far shadow 0.28 M + 4.37 M still (0 frames ago); clusters 0.14 M
at END: terrain.patches_resident 1575.0, patches_drawn 700.0, building 0.0, missing 0.0; render.gpu.scene: terrain 4.59 ms, gpu.shadows 3.08, gpu.sky occlusion 0.20, gpu_ms 18.22 (one frame)
######## ultra-800x450-run2
== plaza: window 00:32:50-00:33:00 (25-35 s after arriving)
fps 28.7, frame 34.8 ms, 1% low 21.6 (0.5 s samples), slowest 0.5 s sample 46 ms/frame, worst frame 106 ms (2 profiles), GPU busy 59%
frame profile 00:33:01: 138 frames in 5.0 s: 36.6 ms per frame (27.3 fps), worst 106.0 ms
  render_system (ms/frame, max, runs): 11.74 / 21.69 / 1.0
  prepare meshes: 2.31 / 4.19 / 1.0
  prepare on the render thread: 2.04 / 3.42 / 1.0
  meshes: cull: 1.08 / 1.80 / 1.0
  wait for the finishing threads: 1.28 / 1.87 / 1.0
  meshes: shadow lists: 0.37 / 0.98 / 1.0
  prepare terrain: 1.26 / 2.25 / 1.0
render profile 00:32:59 (5 s avg): GPU 19.78 ms: scene: terrain 3.22, shadows (all cascades + far shadow, one timestamp) 1.93, sky occlusion 0.15, scene 6.54, scene: meshes 2.66, prepass 1.88; render CPU 15.92 ms; terrain draws 1581 (1730304 triangles) last frame
mesh triangles 00:32:59: view 5.22 M + 2197 impostors, cascade 0 0.03 M + 0.54 M still (498 frames ago), cascade 1 0.08 M + 4.33 M still (433 frames ago), cascade 2 1.28 M + 8.89 M still (0 frames ago), cascade 3 1.14 M + 13.12 M still (0 frames ago), sky occlusion 1.01 M + 4.87 M still (21 frames ago), far shadow 0.01 M + 5.03 M still (0 frames ago); clusters 0.55 M
at END: terrain.patches_resident 1669.0, patches_drawn 751.0, building 0.0, missing 0.0; render.gpu.scene: terrain 3.19 ms, gpu.shadows 1.42, gpu.sky occlusion 0.14, gpu_ms 16.47 (one frame)
== forest: window 00:33:22-00:33:32 (21-31 s after arriving)
fps 37.2, frame 26.9 ms, 1% low 32.6 (0.5 s samples), slowest 0.5 s sample 31 ms/frame, worst frame 49.8 ms (1 profiles), GPU busy 71%
frame profile 00:33:31: 192 frames in 5.0 s: 26.2 ms per frame (38.2 fps), worst 49.8 ms
  render_system (ms/frame, max, runs): 8.50 / 12.21 / 1.0
  prepare meshes: 2.58 / 3.66 / 1.0
  prepare on the render thread: 2.00 / 5.89 / 1.0
  meshes: cull: 1.05 / 1.95 / 1.0
  wait for the finishing threads: 0.93 / 1.49 / 1.0
  meshes: shadow lists: 0.38 / 3.76 / 1.0
  prepare terrain: 1.11 / 2.27 / 1.0
render profile 00:33:29 (5 s avg): GPU 18.37 ms: scene: terrain 4.67, shadows (all cascades + far shadow, one timestamp) 2.67, sky occlusion 0.19, scene 8.17, scene: meshes 3.05, prepass 1.48; render CPU 12.91 ms; terrain draws 1424 (1612800 triangles) last frame
mesh triangles 00:33:29: view 4.61 M + 2325 impostors, cascade 0 0.32 M + 0.05 M still (323 frames ago), cascade 1 1.51 M + 0.92 M still (92 frames ago), cascade 2 0.03 M + 11.10 M still (12 frames ago), cascade 3 0.02 M + 14.18 M still (0 frames ago), sky occlusion 1.53 M + 2.50 M still (10 frames ago), far shadow 0.02 M + 4.86 M still (1 frames ago); clusters 0.15 M
at END: terrain.patches_resident 1575.0, patches_drawn 700.0, building 0.0, missing 0.0; render.gpu.scene: terrain 4.79 ms, gpu.shadows 2.90, gpu.sky occlusion 0.19, gpu_ms 20.31 (one frame)
######## high-1600x900
== plaza: window 00:29:48-00:29:58 (25-35 s after arriving)
fps 36.6, frame 27.3 ms, 1% low 32.9 (0.5 s samples), slowest 0.5 s sample 30 ms/frame, worst frame 173.2 ms (2 profiles), GPU busy 100%
frame profile 00:29:58: 188 frames in 5.0 s: 26.7 ms per frame (37.4 fps), worst 173.2 ms
  render_system (ms/frame, max, runs): 11.09 / 22.49 / 1.0
  prepare meshes: 2.27 / 3.21 / 1.0
  prepare on the render thread: 2.12 / 5.14 / 1.0
  meshes: cull: 1.15 / 2.26 / 1.0
  wait for the finishing threads: 0.23 / 1.93 / 1.0
  meshes: shadow lists: 0.36 / 3.66 / 1.0
  prepare terrain: 1.19 / 2.26 / 1.0
render profile 00:29:57 (5 s avg): GPU 26.14 ms: scene: terrain 5.41, shadows (all cascades + far shadow, one timestamp) 0.95, sky occlusion 0.13, scene 12.55, scene: meshes 6.46, prepass 2.13; render CPU 22.74 ms; terrain draws 1443 (1612800 triangles) last frame
mesh triangles 00:29:57: view 5.53 M + 2495 impostors, cascade 0 0.03 M + 0.30 M still (679 frames ago), cascade 1 0.04 M + 2.23 M still (736 frames ago), cascade 2 1.03 M + 6.46 M still (46 frames ago), cascade 3 1.23 M + 10.88 M still (2 frames ago), sky occlusion 0.81 M + 4.70 M still (46 frames ago), far shadow 0.00 M + 4.25 M still (55 frames ago); clusters 0.72 M
at END: terrain.patches_resident 1581.0, patches_drawn 700.0, building 0.0, missing 0.0; render.gpu.scene: terrain 5.45 ms, gpu.shadows 0.89, gpu.sky occlusion 0.12, gpu_ms 25.73 (one frame)
== forest: window 00:30:20-00:30:30 (21-31 s after arriving)
fps 35.9, frame 27.8 ms, 1% low 32.1 (0.5 s samples), slowest 0.5 s sample 31 ms/frame, worst frame 42.1 ms (1 profiles), GPU busy 100%
frame profile 00:30:28: 185 frames in 5.0 s: 27.2 ms per frame (36.8 fps), worst 42.1 ms
  render_system (ms/frame, max, runs): 12.50 / 24.93 / 1.0
  prepare meshes: 2.34 / 3.75 / 1.0
  prepare on the render thread: 2.17 / 15.83 / 1.0
  meshes: cull: 1.29 / 14.85 / 1.0
  wait for the finishing threads: 0.21 / 1.75 / 1.0
  meshes: shadow lists: 0.34 / 0.61 / 1.0
  prepare terrain: 1.13 / 2.34 / 1.0
render profile 00:30:27 (5 s avg): GPU 26.37 ms: scene: terrain 8.50, shadows (all cascades + far shadow, one timestamp) 0.99, sky occlusion 0.14, scene 14.89, scene: meshes 5.88, prepass 2.48; render CPU 25.86 ms; terrain draws 1361 (1617408 triangles) last frame
mesh triangles 00:30:27: view 5.05 M + 1898 impostors, cascade 0 0.15 M + 0.02 M still (942 frames ago), cascade 1 0.86 M + 0.24 M still (439 frames ago), cascade 2 1.03 M + 3.05 M still (36 frames ago), cascade 3 0.72 M + 6.91 M still (1 frames ago), sky occlusion 1.21 M + 1.92 M still (36 frames ago), far shadow 0.01 M + 3.31 M still (59 frames ago); clusters 0.25 M
at END: terrain.patches_resident 1563.0, patches_drawn 702.0, building 0.0, missing 0.0; render.gpu.scene: terrain 8.45 ms, gpu.shadows 0.82, gpu.sky occlusion 0.12, gpu_ms 26.53 (one frame)


