Round 21d step 1a: showcase.exe --route --profile --quality ultra on main 1810b539, Tobias's PC (RTX 5080, 32 threads,
64 GB), release, window as the exe opens it (client 1600x900, not maximized), real time. The whole route ran (83.1 s of
route time) and the game exited by itself, code 0. No ERROR or panic in stderr.
Start: world built 18 s after launch; "the ground settled after 8 frames", "and the view from the plaza after 4 more".
softbody yard: made 27 materials (62.0 MB of textures) in 0.25 s.

Legs (route.txt, the route's own log; frame ms is real time, clamped at 250):
leg | frames | mean ms | max ms | over 33 ms | terrain prepare max ms | patches missing max | RAM max MB | ends at (route s, m up)
village         | 103 | 78.6 | 250.0 | 103 | 4.5  | 228 | 7274 | 8.1 s, 5 m
face up         |   1 | 67.7 |  67.7 |   1 | 1.0  |   0 | 7274 | 8.2 s, 5 m
climb to 2 km   | 130 | 67.5 | 227.7 | 130 | 8.1  |  12 | 7317 | 16.9 s, 2049 m
cruise at 2 km  | 183 | 66.2 | 110.7 | 183 | 10.1 |   4 | 6769 | 29.0 s, 4872 m
face down       |   1 | 83.2 |  83.2 |   1 | 0.8  |   0 | 4958 | 29.1 s, 4906 m
climb to orbit  | 331 | 27.2 |  69.8 |  97 | 3.0  | 247 | 5216 | 38.1 s, 405875 m
orbit           | 357 | 16.9 |  58.9 |   5 | 1.8  |  12 | 5248 | 44.2 s, 418360 m
descend to 2 km | 392 | 27.1 |  65.3 |  90 | 2.5  |  36 | 5291 | 54.8 s, 1996 m
turn home       |   1 | 23.1 |  23.1 |   0 | 1.1  |   0 | 5291 | 54.8 s, 1975 m
cruise home     | 180 | 67.1 | 121.8 | 180 | 3.2  | 829 | 5377 | 66.9 s, 1562 m
down to 100 m   | 107 | 38.6 | 101.6 |  51 | 3.7  |  22 | 4927 | 71.0 s, 100 m
low pass        | 282 | 35.8 |  81.9 | 121 | 3.0  |  27 | 5050 | 81.1 s, 575 m
end             |  97 | 22.0 |  75.7 |  10 | 4.3  |  34 | 4976 | 83.1 s, 661 m
RAM (route.txt): 6069 MB at the start, 7317 max, 4976 at the end. Process private bytes (timeline, every 10 s): 8.6 GB
at the plaza settle, 13.4-15.7 GB from the village on (working set 4.9-7.2 GB).
Hitches: 972 frames over 33 ms of 2165. route.txt lists the first 60 (all in the village leg, with their slowest render
steps); analysis.txt has the worst 25 frames of route.csv and frames over 33/50/100 ms per leg.
The low pass starts 98 m up and ends 575 m up (sprinting forward, 1231 m/s at the end): the ground falls away under it;
in 21b it ended 1 m up against rock.

Where the time goes (the 5 s frame profiles inside each leg, analysis.txt):
- The 2 km legs (climb to 2 km, cruise at 2 km, cruise home, ~62-72 ms a frame, GPU 20-26% busy) are CPU-bound on
  engine_water::ripple_system: 18.8-39.1 ms a frame (max 30-53 ms a run), 12.5-32.2 ms of it ran alone. render_system is
  8.5-14 ms a frame there. softbody::step_beside_physics has 23-53 ms maxima on those legs; terrain::update_colliders
  max 27-56 ms.
- Climb to orbit, orbit and the descent run 20-29 ms a frame (36-50 fps): render_system 5.3-9.8 ms, ripple_system
  2.7-11 ms; the GPU 54-85% busy, GPU passes 14.8-15.9 ms.
- The village leg is the startup: the first 5 s profile has stream_system 152 ms a frame (max 485), fire::run_demos max
  886 ms, npc::build_stand_ins max 490 ms; the next has smoke::run_demos max 1196 ms; then 62 ms frames with
  softbody::step_beside_physics 16.9 ms and physics_simulate 9.7 ms (3.0 fixed steps a frame).
- Slow render frames (stderr): before the route, "prepare on the render thread" 2029 ms (meshes: pipelines 1969) and
  "prepare meshes" 2079 ms (meshes: materials 2069); during the route only screenshot saves (40-53 ms each).

Files: route.txt and route.csv (the route's own), analysis.txt (per leg: the frame profiles, GPU passes, GPU busy;
worst frames; slow render frames), timeline.txt (my driver: launch, leg lines, memory every 10 s), nvidia_dmon.txt
(the file was cut before its last rows were flushed: no GPU busy for the end leg), jpg/ (each leg's end, the heights
on the way up, the first frames), logs.zip (stderr.log, engine-profile.txt).
