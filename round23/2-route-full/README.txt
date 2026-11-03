Round 23 step 2a: showcase.exe --route --profile --quality ultra on main 70b02d16, Tobias's PC (RTX 5080, 32 threads,
64 GB), release, window as the exe opens it (client 1600x900, not maximized), real time. The whole route ran (82.5 s of
route time) and the game exited by itself, code 0. Started after 481 s with no input.
Bridge clients: engine.status and engine.stats offframe.* twice a second; and two connections reading back to back
(each answered every route frame): engine.stats render.cpu.* (fast.jsonl) and engine.stats offframe.*
(fast_offframe.jsonl). route.csv row i = frame 48 + i (the rows' terrain_ms equal the sampler's in all 3071).

Legs (route.txt, offframe_worst.txt; frame ms is real time, clamped at 250):
leg             | frames | mean ms | median | max ms | over 33 ms | over 50 | RAM max MB | ends at           | r22 mean ms
village         |  105 | 77.0 |  63.2 | 250.0 | 105 | 97 | 7297 |  8.1 s, 5 m       | 72.7
face up         |    1 | 48.5 |  48.5 |  48.5 |   1 |  0 | 7175 |  8.1 s, 5 m       | 55.7
climb to 2 km   |  228 | 37.8 |  31.4 |  97.0 | 104 | 53 | 7214 | 16.8 s, 2022 m    | 46.8
cruise at 2 km  |  449 | 26.8 |  25.8 |  57.5 |  52 |  1 | 5717 | 28.8 s, 4681 m    | 25.5
face down       |    1 | 22.2 |  22.2 |  22.2 |   0 |  0 | 5024 | 28.8 s, 4693 m    | 28.9
climb to orbit  |  472 | 19.1 |  18.1 |  40.8 |  11 |  0 | 5121 | 37.8 s, 403823 m  | 19.1
orbit           |  351 | 17.2 |  16.9 |  34.4 |   1 |  0 | 5150 | 43.8 s, 407730 m  | 16.0
descend to 2 km |  535 | 19.8 |  19.4 |  37.8 |   2 |  0 | 5202 | 54.5 s, 1988 m    | 19.4
turn home       |    1 | 19.0 |  19.0 |  19.0 |   0 |  0 | 5178 | 54.5 s, 1965 m    | 16.9
cruise home     |  398 | 30.2 |  29.2 |  47.8 | 104 |  0 | 5178 | 66.5 s, 972 m     | 29.0
down to 100 m   |  182 | 21.7 |  18.6 |  80.6 |  13 |  7 | 4923 | 70.5 s, 100 m     | 21.6
low pass        |  245 | 41.0 |  40.1 |  83.6 | 221 | 17 | 4940 | 80.5 s, 787 m     | 50.8
end             |  103 | 20.7 |  17.5 |  52.2 |  12 |  1 | 4857 | 82.5 s, 791 m     | 18.5
Hitches: 626 frames over 33 ms of 3071 (round 22: 628 of 3184). route.txt lists every one with its slowest render
steps. RAM (route.txt): 4259 MB at the start, 7297 max, 4857 at the end. Private bytes (timeline, every 10 s) 11.2 GB
at the village, 14.7-15.8 GB from the climb on (working set 4.9-7.2 GB).
Against round 22: the climb to 2 km is 9 ms faster on average (max 97.0, was 222.8) and the low pass 10 ms (max 83.6,
was 104.2); the low pass ends 787 m up (r22 265 m). The other legs are within ~1 ms.

Off-frame jobs (offframe.*, per frame): they run only at the start. In the village leg, 43-59 jobs pending in its
first frames, worker builds of 12229 and 11434 ms applied in frames 51 and 53, and one apply of 60.7 ms on the main
thread in frame 53 (a 209.8 ms frame: render CPU 97.2, screenshot save 142.0, prepare character 63.0). One more
before the route started (frame 36, the plaza settle): apply 124.3 ms. From frame 58 on offframe.pending,
building_ms and apply_ms are 0 in every frame of the route, so none of the later hitches is off-frame work.
The worst frames (each leg's worst 6, and the run's worst 25, with their offframe.* and slowest render CPU rows):
offframe_worst.txt. In short:
- village: four 250 ms frames in the first 1.8 s (render CPU 25-85: meshes: sync 67.5, prepare meshes 71.2).
- climb to 2 km: 97.0 ms at 28 m (render CPU 18.4), 76.3 at 75 m, 74.3 at 163 m: render CPU 14-19, so the main
  thread's systems; the profile 22:32:05 has water::ripple_system 7.45 ms a frame (max 25.85) and
  softbody::step_beside_physics 4.19 (max 11.56); the one before step_beside_physics 17.48 a frame.
- cruise at 2 km: 57.5 at 2056 m (render CPU 7.8); 41.9 with prepare character 30.9.
- cruise home: 46-48 ms frames at ~1970 m, render CPU 12-17; foliage::stream_far_woods max 13.27 a run in 22:32:55.
- low pass: 83.6 at 332 m, 71.1 at 138 m (render CPU 10-12); profile 22:33:05: render_system 11.45 a frame,
  water::ripple_system 4.83 (max 31.65), physics_simulate 2.03.
analysis.txt has each leg's 5 s frame profiles (schedules, slowest systems, ran alone, GPU busy).
Pictures: jpg/10_cruise_home.jpg is grey: that leg ends inside a cloud at 972 m (round 22's is the same, 18 KB).
The others show their leg's end as in round 22.

Files: route.txt and route.csv (the route's own), offframe_worst.txt (route23.py), analysis.txt (21d's per-leg
profiles), timeline.txt, frames.csv, nvidia_dmon.txt, jpg/ (each leg's end, up_5km..up_300km, the first frames),
logs.zip (stderr.log, engine-profile.txt, sampled_stats.jsonl, fast.jsonl, fast_offframe.jsonl).
