Round 21c step 4: probe GI at the plaza, main 145dd3a6, Tobias's PC (RTX 5080), release, window as the exe opens it
(client 1600x900). showcase.exe --fly --profile --quality ultra, showcase.goto plaza through the bridge, a 20 s window;
GI default (on) vs ENGINE_PROBE_GI=off. Same columns as step 3. render.gi.* sampled twice a second with engine.stats
(sampled_stats.jsonl), so "gathered" is the share of sampled frames with render.gi.gathered = 1, not of every frame.

Two windows per setting:
- 5-25 s after the jump (as 21b): the jump comes 7 s after the world is built, and the first frames still load
  (250 ms frames for the window's first ~3 s; "meshes: materials 223 ms" etc.), so these are pulled down by startup.
- 25-45 s after the jump (a second pair of runs): settled. Read these.

window | GI | fps | 1% low | worst ms | frame ms | systems busy / one at a time | render_system (max) | GPU ms | GPU busy % | cores | top 5 ran alone
25-45 s | on | 24.2 | 19.6 | 91.1 | 41.4 | 47.2 / 34.1 | 11.43 (16.93) | 26.6 | 73 | 1.11 | render_system 11.43, physics_simulate 4.01, water::flow_bed_system 3.03, softbody::step_bodies 1.28, script::run_scripts 1.19
25-45 s | off | 34.9 | 26.6 | 74.9 | 28.6 | 32.1 / 23.6 | 7.28 (10.49) | 21.8 | 82 | 1.10 | render_system 7.28, water::flow_bed_system 2.16, script::run_scripts 1.17, physics_simulate 1.06, softbody::step_bodies 0.90
5-25 s | on | 17.2 | 5.1 | 165.1 | 58.2 | 59.2 / 42.1 | 14.61 (40.17) | 26.4 | 49 | 1.13 | render_system 14.61, physics_simulate 7.91, softbody::step_bodies 1.50, script::run_scripts 1.44, water::boat_air_system 1.41
5-25 s | off | 23.3 | 8.5 | 104.4 | 43.0 | 35.1 / 25.7 | 9.29 (35.97) | 23.6 | 51 | 1.09 | render_system 9.29, script::run_scripts 1.37, softbody::step_bodies 1.04, physics_simulate 1.03, water::boat_air_system 0.92
21b (aac391d4), 5-25 s | on | 10.5 | 6.2 | 142.1 | 95.2 | 96.9 / 79.4 | 48.21 (54.46) | 26.5 | 28 | 1.08 |
21b (aac391d4), 5-25 s | off | 29.6 | 11.4 | 80.2 | 33.8 | 28.7 / 21.3 | 7.75 (10.77) | 24.1 | 74 | 1.08 |

render.gi (GI on):
- 25-45 s: 38 samples (frames 351-809), gathered = 1 in 38 of 38 (100%); extract_ms mean 4.30, min 1.16, max 7.51;
  build_ms mean 3.95, max 7.15; probes updated 1373-2048 a frame (mean 1857 of 16384); 332 instances, 123-128 meshes.
- 5-25 s: 36 samples (frames 18-316), gathered = 1 in 36 of 36 (100%); extract_ms mean 3.52, min 1.19, max 7.66.
- ENGINE_PROBE_GI=off: render.gi.on = 0 in every sample.

Where GI's ~13 ms a frame goes (render profile at the end of the 25-45 s windows, on vs off):
- render CPU steps 15.98 vs 11.92 ms: "probe gi" 3.93 vs 0.01.
- GPU passes 26.57 vs 21.84 ms: probe gi 1.86, compute meshes 1.85 (not in the off run's top), world reflections 0.82.
- the rest is the longer frame itself: physics_simulate steps at a fixed rate, so 4.01 vs 1.06 ms a frame.
- GI on is 21b's 48 ms render_system -> 11.4 now; GI's frame cost fell from ~61 ms (95.2 vs 33.8) to ~13 ms (41.4 vs 28.6).

Pictures: plaza-gi-on.jpg and plaza-gi-off.jpg (after the 25-45 s windows); each run folder has its plaza.jpg and
early_frame.jpg (a picture taken as soon as the bridge answered, frame 3-4, before the jump).

Frame 1: no longer blown out white.
- route-up-first-frames/: showcase.exe --route up --profile --quality ultra (the run where 21b's first_001 was
  near-white, brightness 246, ev100 NaN). The route now waits at the plaza until "the ground settled after 13
  frames"; first_001 shows the plaza normally exposed: mean brightness 100, ev100 12.86 (metered 14.98) on the first
  route.csv row; frames 2/4/8/16/32/64 brightness 94/82/64/85/84/89, ev100 finite throughout.
- Still there: first_004 has a green ground wedge drawn over the paving in front of the fountain (terrain patches
  missing 87-120 then, route.csv), and the fountain basin is dry in frames 1-16. The --fly runs' early_frame.jpg
  (frame 4, before the jump; ev100 12.1 GI on, 9.4 off, so not white either) shows the stretched green wall with a black
  slit that 21b saw on route frames 4/8.
The climb itself ran to 403 km in 22.0 s and the game exited by itself.
