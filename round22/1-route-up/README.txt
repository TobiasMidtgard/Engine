Round 22 step 1b: showcase.exe --route up --profile --quality ultra on main 5de4f33e, Tobias's PC (RTX 5080), release,
window as the exe opens it (client 1600x900), real time. The route ran to orbit (23.0 s of route time) and the game
exited by itself, code 0. Started right after step 1a (the same build, nothing else running), with step 1a's bridge
samplers (see 1-route-full/README.txt).

Legs (route.txt):
leg            | frames | mean ms | max ms | over 33 ms | terrain prepare max ms | patches missing max | RAM max MB | ends at          | 21d mean ms
village        |   7 | 194.1 | 250.0 |   7 | 3.1 | 173 | 4599 |  1.4 s, 5 m      | 189.9
face down      |   1 |  78.4 |  78.4 |   1 | 1.3 | 138 | 4599 |  1.4 s, 5 m      |  82.7
climb to orbit | 786 |  26.1 | 250.0 | 148 | 5.0 | 291 | 7229 | 22.0 s, 404847 m |  28.2
orbit          |  53 |  21.8 |  37.1 |   4 | 1.5 |   3 | 5138 | 23.0 s, 408605 m |  21.9
RAM (route.txt): 4599 MB at the start, 7229 max, 5138 at the end. Private bytes 13.9-14.5 GB during the climb.
Climb to orbit (route.csv): 148 over 33 ms, 90 over 50, 4 over 100 (250.0 at 11 m, 173.0 at 17 m, 146.0 at 200 m,
104.9 at 10 m); median 18.4 ms. By height: 79 m-1 km 104 frames, mean 46.8 ms (median 45.6, 83 over 33, max 146.0);
1-12.7 km 233 frames, mean 21.9 (11 over 33, max 49.3); above 12.7 km 404 frames, mean 17.2 (max 80.5).

79 m to 1 km up (route s 5.1-9.9, 18:31:38.7-43.5 UTC; softbody_terrain.txt):
- softbody.stats: 47 bodies, asleep 0, frozen 13-46; softbody.slowest_ms mean 5.67, max 11.84 (2 Hz);
  softbody.step_ms max 11.90. Slowest body by poll: Dog fur 2 (0.16 ms), Shot curtain 1 (8.42 ms, pending_prewarm 40),
  Burning curtain 1 (8.16, 12), Unveiling sheet 1 (7.99, 95). Over the whole climb the slowest was Unveiling sheet up to
  16.75 ms (pending_prewarm 151, 13 substeps), Burning curtain 10.72 (94), Shot curtain 9.53 (82).
- Bodies with pending_prewarm > 0: 1-3 per poll: Unveiling sheet in all 5 polls (up to 139), Shot curtain (up to 40),
  Burning curtain (up to 38). Over the whole climb 0-8 (also Dog fur up to 37, Mannequin hair, Rinsing silk).
- The two frame profiles that cover the band: 18:31:36-41 (65.9 ms a frame, 15.2 fps): softbody::step_beside_physics
  18.12 ms a frame, max 18.69 a run, 2.4 runs a frame (14.12 ran alone; 21d: 21.9 / 16.2); 18:31:41-46 (29.4 ms a frame):
  step_beside_physics 5.73, max 11.87. From 18:31:46 on 0.32-0.45 a frame.
- water::ripple_system: not in the slowest 40 systems in either profile (under ~0.13 ms); 5.13 ms a frame in the one
  before (the village and the first 2 s of the climb).
- terrain::update_colliders: 0.32 ms a frame, max 0.64; 0.24, max 0.48.
- Worst frames' terrain rows (the frame's own render.cpu.terrain: *): 146.0 ms at 200 m: render CPU 15.5, prepare terrain
  1.49 (maps 1.03, draw lists 0.53, select 0.32, look-ahead 0.22, far shadow 0.12); 71.7 at 215 m: render CPU 122.1
  (the next frame of that hitch), prepare terrain 2.36; 71.5 at 101 m: 2.61 (draw lists 0.55, select 0.35); 70.8 at
  116 m: 1.62 (draw lists 1.14). Band max: prepare terrain 2.64, maps 1.28, draw lists 1.15.
So the 79 m-1 km band is still the yard's soft bodies prewarming (the climb starts 3.9 s of wall time after the plaza settle, while
Unveiling sheet, Shot curtain and Burning curtain have prewarm steps left), not the terrain or the water.

Files: route.txt, route.csv, analysis.txt, softbody_terrain.txt (per leg, and the 79-1000 m band), timeline.txt,
frames.csv, nvidia_dmon.txt, jpg/ (each leg's end, up_5km..up_300km, the first frames), logs.zip (stderr.log,
engine-profile.txt, apis.jsonl, sampled_stats.jsonl, fast.jsonl).
