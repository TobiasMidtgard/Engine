Round 22 step 1a: showcase.exe --route --profile --quality ultra on main 5de4f33e, Tobias's PC (RTX 5080, 32 threads,
64 GB), release, window as the exe opens it (client 1600x900, not maximized), real time. The whole route ran (84.9 s of
route time) and the game exited by itself, code 0. No ERROR or panic in stderr. Started after 39 h with no input.
New this round: a bridge client sampled the run (21d had none on the route): engine.status, and engine.stats softbody.*,
water.*, terrain.collider* twice a second; softbody.stats and softbody.list once a second (apis.jsonl); and a second
connection read engine.stats render.cpu.* back to back, which answered for every route frame (fast.jsonl). The bridge's
own system costs 0.15 ms a frame (bridge_frame_start_system in the frame profiles).

Legs (route.txt; frame ms is real time, clamped at 250):
leg             | frames | mean ms | max ms | over 33 ms | terrain prepare max ms | patches missing max | RAM max MB | ends at   | 21d mean ms
village         |  111 | 72.7 | 250.0 | 111 |  4.3 | 228 | 7396 |  8.1 s, 5 m      | 78.6
face up         |    1 | 55.7 |  55.7 |   1 |  1.0 |   0 | 7396 |  8.1 s, 5 m      | 67.7
climb to 2 km   |  185 | 46.8 | 222.8 | 134 |  9.7 |   8 | 7431 | 16.8 s, 2005 m   | 67.5
cruise at 2 km  |  472 | 25.5 |  64.4 |  62 |  2.9 |   1 | 6338 | 28.8 s, 4644 m   | 66.2
face down       |    1 | 28.9 |  28.9 |   0 |  0.7 |   0 | 5077 | 28.8 s, 4654 m   | 83.2
climb to orbit  |  473 | 19.1 |  51.6 |  12 |  2.4 |  89 | 5248 | 37.9 s, 402763 m | 27.2
orbit           |  377 | 16.0 |  33.1 |   1 |  1.6 |   5 | 5264 | 43.9 s, 407124 m | 16.9
descend to 2 km |  546 | 19.4 |  35.9 |   2 |  2.1 |  16 | 5275 | 54.5 s, 1989 m   | 27.1
turn home       |    1 | 16.9 |  16.9 |   0 |  0.8 |   0 | 5273 | 54.5 s, 1970 m   | 23.1
cruise home     |  415 | 29.0 |  57.0 |  87 | 11.0 | 661 | 5273 | 66.5 s, 983 m    | 67.1
down to 100 m   |  290 | 21.6 |  73.9 |  20 |  2.7 |  12 | 4996 | 72.8 s, 99 m     | 38.6
low pass        |  198 | 50.8 | 104.2 | 191 |  5.5 | 285 | 5010 | 82.8 s, 265 m    | 35.8
end             |  114 | 18.5 |  48.3 |   7 |  4.0 |  18 | 4945 | 84.9 s, 271 m    | 22.0
Hitches: 628 frames over 33 ms of 3184 (21d: 972 of 2165). route.txt lists the first 60 (all in the village leg).
RAM (route.txt): 6091 MB at the start, 7431 max, 4936 at the end. Private bytes (timeline, every 10 s): 8.7 GB at the
plaza settle, 13.8-15.7 GB from the village on (working set 5.0-7.3 GB).
The low pass now ends 265 m up (21d: 575 m), lower over the ground for longer; its 50.8 ms mean is the one leg slower
than 21d.

The 2 km legs (softbody_terrain.txt; softbody.* from the bridge, systems from the 5 s frame profiles):
leg            | softbody.bodies / asleep / frozen | softbody.slowest_ms mean / max (2 Hz) | slowest body (polls)                      | pending_prewarm > 0
climb to 2 km  | 47 / 0 / 15-47                    | 1.36 / 8.89                           | Dog fur 5 of 8 (0.23 ms); Unveiling sheet  | 0-1 (Unveiling sheet, up to 46)
               |                                   |                                       | once 8.51 ms (pending_prewarm 46)          |
cruise at 2 km | 47-48 / 0 / 47-48                 | 0.91 / 8.95                           | Dog fur 12 of 12 (max 0.27 ms)             | 0-1 (Hair simulation, up to 50)
descend to 2 km| 48 / 0 / 48                       | 0.20 / 0.27                           | Dog fur 10 of 10 (max 0.27 ms)             | 0
cruise home    | 48 / 0 / 48                       | 0.20 / 0.29                           | Dog fur 12 of 12 (max 0.30 ms)             | 0
softbody.asleep is 0 in every poll of the run (frozen counts the far ones).
water::ripple_system: not in any profile's slowest 40 systems (under ~0.13 ms a frame) on cruise at 2 km, descend to
2 km and cruise home (21d: 18.8-39.1 ms a frame there). On climb to 2 km it is 9.04 ms a frame, max 30.87 a run, in the
profile covering 175 m to 2011 m up (it rests from 250 m above the ground, water 43633ffd); 0 in the one before.
It comes back below 250 m: down to 100 m / low pass 6.32 (max 42.45) and 3.32 (max 52.18) ms a frame.
terrain::update_colliders: 0.28-0.44 ms a frame on the 2 km legs, maxima 0.42-1.01 ms, except the climb to 2 km
profile (175 m to 2011 m): 0.86 a frame, max 19.51 (21d: max 27-56 ms).
softbody::step_beside_physics: 4.95 (max 13.37) on the climb, 2.75 -> 1.13 -> 0.33 a frame through the cruise at
2 km, 0.21-0.48 after (21d: 23-53 ms maxima on the 2 km legs).

The worst frames' terrain rows (render.cpu.terrain: * of that very frame, joined to route.csv by frame number; the
join's terrain_ms agrees for all 3184 rows): none of the 6 worst frames of a 2 km leg has prepare terrain over 2.2 ms.
- climb to 2 km: 222.8 ms at 459 m, render CPU 13.1, prepare terrain 0.81 (select 0.27, look-ahead 0.16, far shadow
  0.10, draw lists 0.09, uploads 0.09); 169.1 ms at 630 m, render CPU 16.6, prepare terrain 1.66 (maps 0.73, select
  0.25); 166.0 at 754 m, prepare terrain 1.80; 100.4 at 28 m, 1.17 (meshes: materials 8.54). The leg's max rows: prepare
  terrain 9.70, maps 8.38 (frame 253, 78.5 ms; maps can read more than prepare terrain).
- cruise at 2 km: 64.4 ms at 2022 m, render CPU 10.5, prepare terrain 0.94 (maps 0.67, select 0.26); 54.9 ms with render
  CPU 40.1 (prepare character 31.3), prepare terrain 1.49; 51.1, 1.95. Leg max: prepare terrain 2.84, maps 1.88.
- descend to 2 km: 35.9 ms at 4253 m, prepare terrain 0.72 (maps 1.20). Leg max 2.11.
- cruise home: 57.0 ms at 1976 m, render CPU 18.5 (meshes: materials 10.20), prepare terrain 1.54 (maps 2.01, uploads
  0.53); 52.4, 1.84; 49.6, 2.20. Leg max: prepare terrain 11.03 with uploads 10.21 (one frame).
- low pass (for comparison): 104.2 ms at 251 m, render CPU 16.9, prepare terrain 3.09 (maps 0.86, uploads 0.85, select
  0.51, look-ahead 0.45); 91.6 at 89 m, 3.56; 88.7 at 144 m, 2.96. Leg max 5.52, maps 3.01.
So the climb's 166-223 ms hitches are not the terrain's render CPU (13-17 ms of render CPU in those frames); the main
thread's systems are: the same profile has ripple_system max 30.87 and update_colliders max 19.51.

Low pass (profile 18:27:59, 45.7 ms a frame, GPU 35% busy): systems busy 45.4 ms a frame on 0.99 of 32 cores (one at
a time 36.8): render_system 12.0, FixedUpdate 18.36 at 2.5 runs a frame (ripple 3.32, physics_simulate 2.52 max 14.97,
weather ground_system 2.14), Update 8.20, PostUpdate 4.93. Its worst frame 104.2 ms at 251 m: render CPU 16.9, prepare
terrain 3.09.

Files: route.txt and route.csv (the route's own), analysis.txt (21d's per-leg profiles, worst 25 frames, slow render
frames), softbody_terrain.txt (per leg: softbody stats/list, terrain colliders, water, the profiles' ripple /
update_colliders / softbody rows, the worst 6 frames' terrain rows), timeline.txt, frames.csv, nvidia_dmon.txt,
jpg/ (each leg's end, up_5km..up_300km, the first frames), logs.zip (stderr.log, engine-profile.txt, apis.jsonl,
sampled_stats.jsonl, fast.jsonl).
