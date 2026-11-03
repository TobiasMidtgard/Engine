Round 21d step 2b: ENGINE_AERIAL_TABLE=on vs off, main 1810b539, Tobias's PC (RTX 5080), release, Ultra, window as the
exe opens it (1600x900). showcase.exe --fly --profile --quality ultra; through the bridge: overview 35 s (the first stop
also waits out the startup), then forest_overview 25 s. Two runs back to back, the same build: aerial-on/ with
ENGINE_AERIAL_TABLE=on (the default) and aerial-off/ with =off.
woods_aerial is a view of crates/terrain/examples/terrain_shots.rs, not a showcase place; I used forest_overview (the
foliage demo's "living forest from above", camera at (-148, 32, -78)) as the nearest showcase view of the woods from above.
Numbers: the last 10 s of each stay; GPU passes from the last render profile 10+ s after arriving (5 s averages).

stop | aerial | fps | frame ms | 1% low | GPU busy % | render_system ms | GPU total | scene | scene: terrain | scene: meshes | prepass | aerial perspective
overview        | on  | 42.2 | 23.7 | 34.6 |  95 |  8.91 | 21.90 | 12.74 | 6.96 | 5.04 | 2.35 | 0.34
overview        | off | 39.3 | 25.5 | 31.3 | 100 | 12.65 | 24.29 | 15.33 | 8.19 | 6.27 | 2.35 | 0.00
forest_overview | on  | 40.3 | 24.8 | 34.8 |  84 |  8.84 | 20.51 |  6.98 | 2.23 | 4.26 | 2.82 | 0.40
forest_overview | off | 41.2 | 24.3 | 37.3 |  93 |  8.53 | 21.02 |  7.98 | 2.65 | 4.82 | 2.80 | 0.00

Reading: the table saves 2.39 ms of GPU at overview (the scene pass 2.59 ms cheaper: terrain -1.23, meshes -1.23; its
own pass costs 0.34) and 0.51 ms at forest_overview (scene -1.00, its pass 0.40). Every other pass agrees within 0.05 ms
(volumetric fog 0.25/0.28, environment 0.24, shadows 0.75-0.78 / 3.00-3.02). overview's render_system with it off was
12.65 ms (1.2 runs a frame in that profile) vs 8.91: not a GPU-side effect of the table; forest_overview's is the same.

Per run: passes.txt (the per-pass table, all passes sorted), analysis.txt (21c's stops analysis: frame profile render
steps, mesh triangles, terrain at END), overview.jpg / forest_overview.jpg, frames.csv, timeline.txt, nvidia_dmon.txt,
logs.zip (stderr, stats JSON per stop).
