Round 21d step 2a: loops at Ultra on main 1810b539, Tobias's PC (RTX 5080, 32 threads, 64 GB), release, window as the
exe opens it (client 1600x900, not maximized). Same method as round 21c step 3 (2 rounds).

showcase.exe --loops overview,plaza,market,forest,lake,fortress,field,orbit:2 --profile --quality ultra
Per stop: the last 20 s of its 25 s dwell. fps / frame ms / 1% low from engine.status twice a second; worst frame from the
5 s frame profiles inside the window; render_system, systems busy, cores, ran alone from the last frame profile in the
window; GPU ms from the last render perf line; GPU busy from nvidia-smi dmon. Round 1 took one screenshot plus one render
and one terrain engine.stats snapshot per stop at 20 s (overview's in round 1 too this time: frames ran 10 s after the
world was built, so overview round 1 is not mostly startup as in 21c). The game exited by itself after round 2, code 0.
softbody yard: made 27 materials (62.0 MB of textures) in 0.25 s.

tables.txt has both rounds in 21c's format; passes.txt the per-pass GPU table; extras.txt physics_simulate per profile and
the terrain/extra-view snapshots. Round 1 here:
stop | fps | 1% low | worst frame ms | frame ms | systems busy / one at a time ms | render_system ms (max) | GPU ms | GPU busy %
overview | 37.7 | 26.8 | 177.5 | 27 | 27.2 / 22.6 | 11.11 (36.77) | 25.0 | 94
plaza    | 36.0 | 30.6 | 106.9 | 28 | 29.2 / 22.8 |  8.49 (14.53) | 19.1 | 71
market   | 26.8 | 13.1 | 284.9 | 37 | 43.9 / 25.3 |  9.33 (15.19) | 22.9 | 68
forest   | 34.4 | 16.9 |  98.4 | 29 | 27.1 / 19.9 |  8.45 (34.42) | 20.1 | 73
lake     | 25.8 | 21.1 | 122.3 | 39 | 45.2 / 31.0 | 17.35 (22.35) | 18.7 | 65
fortress | 21.5 | 15.9 | 145.1 | 47 | 66.7 / 41.9 | 12.41 (21.15) | 19.6 | 44
field    | 40.4 | 30.1 | 105.0 | 25 | 26.9 / 19.9 |  7.75 (14.47) | 21.2 | 92
orbit    | 64.7 | 59.6 |  63.5 | 15 | 14.7 / 12.3 |  5.82 (12.24) | 11.6 | 81
Round 2: overview 43.0, plaza 39.8, market 20.6, forest 17.4, lake 16.5, fortress 14.3, field 38.7, orbit 63.6 fps.

Against 21c (main 145dd3a6, round 2): GPU ms 18.7-22.7 at the ground stops (21c 24.4-31.1), overview 21.7 (38.6), orbit
11.8 (13.6). Plaza 39.8 fps (35.6), overview 43.0 (25.1), field 38.7 (28.6), orbit 63.6 (48.4).

Physics in round 2: from market to fortress (05:37:10-05:38:31 UTC) physics_simulate costs 18.7-36.3 ms a frame (ran
alone 21.7-28.7), against 2.0-7.9 at the same stops in round 1; that is the round 2 fps drop at those four stops (GPU
busy 30-48%). The rapier step runs about as often as in round 1 (6.4-15.4 times a frame vs 4.0-12.5) but takes ~1.6-3.7 ms
each instead of ~0.2-0.6 (the frame profile's "in engine_physics: rapier step", 20-36 ms a frame). It falls back at the
field (05:38:36). Entities stay at 122-131k through it (frames.csv), so it is not a pile-up of spawned things. (Step 3's
PHYSICS_REPORT runs are the place to see which regions are awake; this run had no report.)

Per-pass GPU (passes.txt; the last render profile in each window, 5 s averages, ms). Round 2:
stop | total | scene | scene: terrain | scene: meshes | prepass | shadows | compute weather clouds | probe gi | world refl. | transparent | volumetric fog | environment | bounce light | aerial perspective
overview | 21.74 | 13.98 | 7.65 | 5.55 | 2.03 | 0.82 | 1.94 | 0.03 | 0.01 | 0.38 | 0.25 | 0.24 | 0.62 | 0.34
plaza    | 18.98 |  6.71 | 1.80 | 4.13 | 2.38 | 1.68 | 2.29 | 1.81 | 0.74 | 0.26 | 0.28 | 0.24 | 0.61 | 0.39
market   | 22.69 |  9.70 | 3.31 | 3.45 | 3.21 | 1.83 | 2.17 | 0.92 | 1.52 | 0.30 | 0.29 | 0.23 | 0.56 | 0.41
forest   | 20.11 |  6.92 | 2.29 | 4.15 | 2.73 | 2.81 | 2.19 | 1.92 | 0.26 | 0.11 | 0.28 | 0.24 | 0.57 | 0.40
lake     | 18.84 |  8.82 | 6.40 | 1.97 | 1.21 | 1.42 | 2.23 | 0.10 | 0.27 | 1.68 | 0.27 | 0.24 | 0.61 | 0.41
fortress | 20.33 |  9.15 | 4.06 | 3.85 | 2.37 | 1.71 | 1.92 | 0.14 | 0.18 | 1.57 | 0.29 | 0.24 | 0.61 | 0.39
field    | 21.43 |  8.54 | 3.02 | 4.73 | 1.92 | 1.60 | 2.25 | 2.74 | 0.08 | 1.52 | 0.30 | 0.23 | 0.54 | 0.41
orbit    | 11.85 |  8.76 | 8.40 | 0.02 | 0.26 | 0.45 | 0.47 | 0.03 | 0.02 | 0.59 | 0.14 | 0.02 | 0.32 | 0.03
21c round 2 (same script on its log): overview total 38.57, scene: terrain 21.52, volumetric fog 0.95, environment 0.94;
plaza 24.43 / terrain 5.42 / fog 1.00 / environment 0.93; lake 26.86 / terrain 12.05 / fog 0.93 / environment 0.93. So
the drop is the terrain's scene pass (overview -13.9 ms, plaza -3.6, lake -5.7) plus ~0.7 ms each from fog and the
environment; the aerial perspective pass costs 0.34-0.41 on the ground. Other passes
over 0.3 ms: scene: grass 2.57 at the market, 1.04 at the fortress, 0.43-0.49 at plaza/field; compute meshes 2.78 at
overview round 1 only (the start). Round 1's rows agree with round 2's within ~0.2 ms except overview's compute meshes.
Lake extra view (round 1 snapshot): render.extra_views 1, 8.94 ms CPU, 5.35 ms GPU (21c: 10.85 / 11.37). At fortress, field
and orbit render.extra_views is 0 now (21c's stuck value is gone).

Files: tables.txt, passes.txt, extras.txt, summary.json, frames.csv, timeline.txt, nvidia_dmon.txt, jpg/ (round 1 stops),
logs.zip (stderr with every profile, stats JSON per stop).
