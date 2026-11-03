Round 23 step 2b: showcase.exe --route up --profile --quality ultra on main 70b02d16, Tobias's PC (RTX 5080), release,
window as the exe opens it (client 1600x900), real time. The route ran to orbit (23.2 s of route time) and the game
exited by itself, code 0. Started right after step 2a (628 s with no input), with step 2a's samplers, plus a bridge
loop that read space.bodies every frame (the camera's altitude over Earth's sphere) and asked render.screenshot at
the first frame at or past 8, 16, 25 and 42 km (jpg/up_<km>km_bridge.jpg; the route's own up_5km..up_300km beside them).
route.csv row i = frame 56 + i (terrain_ms equal in 850 of 853 rows).

Legs (route.txt, offframe_worst.txt):
leg            | frames | mean ms | median | max ms | over 33 ms | over 50 | RAM max MB | ends at           | r22 mean ms
village        |    6 | 228.7 | 250.0 | 250.0 |   6 |  6 | 4997 |  1.4 s, 5 m       | 194.1
face down      |    1 | 237.7 | 237.7 | 237.7 |   1 |  1 | 4997 |  1.6 s, 5 m       |  78.4
climb to orbit |  776 |  26.5 |  17.8 | 250.0 | 156 | 91 | 7237 | 22.2 s, 406038 m  |  26.1
orbit          |   70 |  16.1 |  15.4 |  33.3 |   2 |  0 | 5007 | 23.2 s, 409146 m  |  21.8
Hitches: 165 over 33 ms of 853 (round 22: 160). RAM: 3791 MB at the start, 7237 max, 5007 at the end; private bytes
11.5 GB at the plaza, 14.8-15.3 GB during the climb.
Climb to orbit by height (route.csv): under 79 m 42 frames, mean 85.9 (max 250.0); 79 m-1 km 95 frames, mean 51.6
(87 over 33, max 139.0; r22 46.8, max 146.0); 1-12.7 km 222 frames, mean 22.8 (18 over 33, max 60.9; r22 21.9);
above 12.7 km 417 frames, mean 16.7 (9 over 33, max 69.0; r22 17.2). Worst: 250.0 at 5 and 9 m, 229.9 at 28 m, 193.8
at 22 m, 139.0 at 201 m. The profiles over the first 10 s of the climb (analysis.txt): smoke::run_demos one run of
1269 ms, npc::think_and_act max 132.3, softbody::step_beside_physics 19.7 then 11.4 ms a frame (to ~1 km up), then
1.7.

Off-frame jobs: as on the full route, only at the start, but here they land in route frames: frame 62 (face down,
237.7 ms): 88 pending, apply_ms 247.4; frame 63 (85.0 ms): apply 89.2; frame 78 (22 m up, 193.8 ms): worker builds of
7034 ms applied, apply 30.6; frame 65: apply 3.0. Before the route (frame 43): apply 73.9. From frame 79 on, 0 in
every frame. So the 238 ms and 85 ms frames at the start are mostly the off-frame apply on the main thread; the
route up starts 1.4 s after the plaza settle (the full route waits 8 s in the village, and its applies fall there).

Near cumulus on the way up (jpg/up_5km, up_8km_bridge, up_10km, up_16km_bridge, up_20km, up_25km_bridge,
up_40km, up_42km_bridge, up_60km; camera pitched 34 deg down):
  The bridge shots landed at: 8 km asked at 8030 m (8347 m when the answer came), 16 km at 16098 m (16785), 25 km at
  27637 m (28591; one frame jumped from 23898 m to 27637 m, so this picture is from ~28 km), 42 km at 42335 m (44135).
- 5 km: the frame is full of near cumulus, big puffs over the land.
- 8 km: near cumulus over the land below and the coast; the sea beyond is clear but for small far puffs.
- 16 km: near cumulus only at the bottom of the frame (over the peninsula, the nearest ~10 km of ground), smaller
  puffs further out; at the top the far layer starts as flat white patches with shadows on the sea.
- 20 km and ~28 km: fewer and smaller near puffs at the bottom; the far flat patches at the top are clearer.
- 40/42 km: the near cumulus are all but gone: a few small specks along the bottom edge (ground ~23-40 km from the
  camera) and over the sea to the right; the flat far patches (top half) are what is left of the clouds, unchanged to
  60 km. No seam or doubled layer is visible where the two meet.
So yes, the near cumulus fade out between ~16 and ~42 km, as the far layer takes over (space's cloud shell fades in
from 9 to 25 km up, crates/space/src/look/render.rs SHELL_FADE_M; the weather's clouds hand over to it between 70%
and all of the shell's hole around the camera). up_40km matches round 22's up_40km.jpg (the same view, the same few
specks), so nothing changed here since 5de4f33e.

Files: route.txt and route.csv, offframe_worst.txt, analysis.txt, timeline.txt (with the UP lines), frames.csv,
nvidia_dmon.txt, jpg/ (legs, up_* from the route and the bridge, the first frames), logs.zip (stderr.log,
engine-profile.txt, sampled_stats.jsonl, fast.jsonl, fast_offframe.jsonl).
