Round 22 step 4: lake and forest_trail stays (30 s each), and the plaza, main 5de4f33e, Tobias's PC (RTX 5080), release,
Ultra, window 1600x900. showcase.exe --fly --profile --quality ultra, then through the bridge: plaza 30 s (takes the
startup), lake 30 s, forest_trail 30 s, plaza 30 s again (for the mesh uploads after the startup). engine.stats with
the prefixes render.gpu.scene, render.gpu.prepass, render.gpu.probe, render.extra_views (that covers
render.extra_views_gpu.* and _cpu.*), render.gi, render.meshes twice a second. Below: means over each stay from 10 s
after arriving to its end (20 s, 40 samples); render_asks.txt has mean / max / last of every key, render_asks_short.txt
the asked ones.

stay              | fps (last 10 s) | GPU ms (profile) | render_system ms
plaza (2nd)       | 38.1 | 18.16 | 9.01
lake              | 27.7 | 18.32 | 17.78
forest_trail      | 39.1 | 24.07 | 9.49   (GPU 98% busy)
plaza (startup)   | 30.4 | 20.36 | 9.79

render.gpu.scene / scene:* (ms)
- lake: scene 8.58; terrain 6.22, meshes 1.91, grass 0.17, character 0.09, softbody 0.06
- forest_trail: scene 6.63; meshes 4.29, grass 1.17, foliage scenery 0.51, terrain 0.46, character 0.09
- plaza (2nd): scene 6.79; meshes 4.27, terrain 1.74, grass 0.50, character 0.09, foliage scenery 0.06
render.gpu.prepass / prepass:* (ms)
- lake: prepass 1.18; meshes 0.53, grass 0.31, terrain 0.20, character 0.06
- forest_trail: prepass 3.31; meshes 1.87, grass 0.92, foliage scenery 0.20, terrain 0.19, character 0.06
- plaza (2nd): prepass 2.18; meshes 1.37, grass 0.46, terrain 0.19, character 0.06
render.extra_views_gpu.* at the lake (render.extra_views 1, sharing the main view's lighting; 0 at forest_trail and
the plaza): render.extra_views_gpu_ms 5.12 mean (max 15.83: one sample with compute meshes 11.12),
render.extra_views_ms (CPU) 9.45 mean (max 11.42). Rows: scene 2.14 (scene: terrain 0.93, scene: meshes 0.85,
character 0.09, grass 0.08, softbody 0.06), prepass 0.70 (meshes 0.25, terrain 0.17, grass 0.13, character 0.06),
compute weather clouds 0.49, aerial perspective 0.41, compute meshes 0.34 (0.06 without the spike), compute softbody
0.22, transparent 0.15, occlusion culling 0.11, volumetric fog 0.10, bounce light 0.10, subsurface scattering 0.06,
reflections 0.05, compute vfx_particles 0.05. CPU rows: prepare meshes 2.82, wait for the finishing threads 1.64,
prepare terrain 0.77, prepare water 0.72, prepare on the render thread 0.57, submit 0.55, prepare character 0.30,
prepare weather ground 0.25 (max 1.85), prepare softbody 0.23.
'extra views:' log lines (every 10 s, all four in the log):
  19:12:55 (arriving at the lake) CPU 114.15 ms (prepare space bodies 12.25, prepare meshes 6.32, ...); GPU 0.00 ms
  19:13:05 CPU 10.83 ms: prepare meshes 3.13, prepare weather ground 1.73, wait for the finishing threads 1.48,
           prepare terrain 0.89, prepare water 0.69, ...; GPU 4.84 ms: scene 2.16, scene: terrain 0.94, scene: meshes
           0.85, prepass 0.71, compute weather clouds 0.48, aerial perspective 0.41, prepass: meshes 0.25, ...
  19:13:15 CPU 8.74 ms (prepare meshes 2.63, wait 1.57, terrain 0.83, water 0.69, submit 0.65); GPU 4.85 ms (the same rows)
  19:13:25 CPU 9.60 ms (prepare meshes 2.79, wait 1.76, terrain 0.90, water 0.85, submit 0.73); GPU 4.86 ms
21d (mirror shares lighting on): 9.06 ms CPU, 5.47 GPU.

Probe GI
stay         | probe gi GPU ms (render profiles) | CPU ms | render.gi.gpu_ms_per_kray mean (max) | probes_updated mean (max) | instances
plaza (2nd)  | 1.31, 1.29 | 1.26, 1.25 | 0.010 (0.016) | 2043 (2048) | 332
lake         | 0.09       | 1.20       | 0.001 (0.001) | 2048 (2048) | 130-139
forest_trail | 1.88       | 1.84       | 0.075 (0.289) | 417 (533)   | 2364
(render.gpu.probe gi sampled: plaza 1.30 mean, max 2.85; lake 0.08; forest_trail 1.82, max 2.53.) 21d forest_trail:
probe gi GPU 3.70, gpu_ms_per_kray 0.461, probes_updated 128. render.gi.full_gather 0 in every sample; gi.rebuilt 0-1
(lake 4 samples, forest_trail 3); gi.building 0 except forest_trail (10 of 40 samples, up to 3).

render.meshes.uploads / reuploads / edits (per frame; mean, and the values seen)
stay          | uploads           | reuploads | edits
plaza (2nd)   | 41.0 (41 in all 40) | 41.0    | 0
lake          | 41.0 (41 in all 41) | 41.0    | 0
forest_trail  | 41.2 (41-48)        | 41.0    | 0.18 (one sample of 7)
plaza startup | 47.0 (41-103)       | 40.0    | 0
So uploads are not high with reuploads 0: the same 41 a frame are counted as reuploads ("uploaded before, copy let go,
needed again") in every frame, at every stay; over the whole run 188 of 236 samples read 41/41, the rest 42-60/41. The
loops' per-stop snapshots (2a) agree: 41/41 at overview, plaza, fortress, orbit, with render.upload_mb 0.0 in those
frames; market 48/41, field 47/41, lake 86/86, forest 104/97. 41 meshes a frame counted as re-uploaded with 0.0 MB of
uploads looks like a counting loop rather than real traffic (Rendering's call).
'uploads in play' lines anyway (the whole log has 7): six in the first 25 s after the world was built (16.1, 16.0,
16.1, 16.0, 16.0, 7.4 MB a frame; LOD meshes, plant stand-ins, MarketArt, then 1024 and 2048 textures), one on
arriving at forest_trail (19:13:28, 1.0 MB a frame, plant stand-in meshes 1066 and 1312). None during the lake or
the second plaza stay.

Files: render_asks.txt (every sampled key per stay), render_asks_short.txt, analysis.txt (21c's stops analysis),
passes.txt, sampled_stats.jsonl, frames.csv, timeline.txt, nvidia_dmon.txt, jpg/ (lake, forest_trail, plaza at the
end), logs.zip (stderr, engine-profile.txt, stats JSON per stay).
