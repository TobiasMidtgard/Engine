Round 23 step 3a: loops at Ultra on main 70b02d16, Tobias's PC (RTX 5080, 32 threads, 64 GB), release, window as the
exe opens it (client 1600x900), round 22's method (2 rounds), with PHYSICS_REPORT=240 RUST_LOG=info. Started right
after step 2b (no input since), the game exited by itself after round 2, code 0.

PHYSICS_REPORT=240 RUST_LOG=info showcase.exe --loops overview,plaza,market,forest,lake,fortress,field,orbit:2 --profile --quality ultra
Per stop: the last 20 s of its 25 s dwell. fps / 1% low from engine.status twice a second; worst frame from the 5 s
frame profiles; render_system from the last frame profile in the window; GPU ms from the last render perf line; GPU
busy from nvidia-smi. Round 1 took a picture and a render + terrain engine.stats snapshot per stop at 20 s.

stop     | round 1 fps / 1% low | round 2 fps / 1% low | render_system ms r1 / r2 | GPU ms r1 / r2 | GPU busy % r1 / r2 | r22 r1 / r2 fps
overview | 38.8* / 29.5 | 44.9 / 41.0 | 10.15 / 9.63  | 24.1 / 20.8 | 91 / 95 | 37.9 / 43.7
plaza    | 40.4 / 33.5  | 40.6 / 17.2 |  8.98 / 8.88  | 16.5 / 16.1 | 70 / 74 | 41.6 / 40.5
market   | 28.9 / 19.0  | 28.8 / 23.4 |  9.97 / 10.07 | 19.2 / 19.6 | 58 / 61 | 28.8 / 29.1
forest   | 31.1 / 16.8  | 29.2 / 16.8 | 11.26 / 10.78 | 17.3 / 17.5 | 59 / 53 | 31.8 / 30.2
lake     | 24.7 / 20.5  | 23.7 / 18.1 | 22.09 / 18.75 | 17.0 / 17.0 | 53 / 50 | 22.4 / 24.3
fortress | 20.6 / 13.9  | 16.0 / 13.5 | 13.03 / 13.91 | 17.2 / 17.5 | 38 / 28 | 22.6 / 17.1
field    | 41.8 / 35.8  | 39.3 / 26.6 |  8.08 / 8.47  | 18.8 / 19.2 | 82 / 80 | 40.3 / 38.5
orbit    | 65.5 / 59.6  | 64.6 / 59.9 |  6.09 / 6.20  |  6.8 / 6.7  | 52 / 52 | 64.1 / 64.6
(* overview round 1 includes the start.) tables.txt has every column (systems busy, cores, ran alone).
fps is round 22's within ~2.5 at every stop; the GPU does 1.3-4 ms less (the weather clouds, below). The GPU is
busiest at overview (91-95%) and field (80-82%), plaza 70-74%; from market to fortress it is 28-61% busy and the frame
is the main thread's.
Round 2 plaza's 1% low 17.2 and field's 26.6 are single hitches: engine_npc::think_and_act ran 306.0 ms once at the
plaza (profile 22:39:42, the 328.0 ms worst frame), 415.0 ms on arriving at the field (22:41:33), then 184.2 and
202.8 ms (22:41:38, 22:41:53: the 222.8/225.5 ms worst frames). Step 1's plaza stand had one of 231.7 ms.

Per-pass GPU (passes.txt; the last render profile in each window, 5 s averages, ms). Round 2:
stop     | total | scene | scene: terrain | scene: meshes | prepass | shadows | weather clouds | probe gi | world refl. | transparent | bounce light | aerial persp.
overview | 20.79 | 14.00 | 7.75 | 5.46 | 2.10 | 0.79 | 0.85 | 0.03 | 0.02 | 0.38 | 0.62 | 0.34
plaza    | 16.07 |  6.45 | 2.01 | 4.01 | 1.83 | 1.55 | 0.96 | 1.29 | 0.55 | 0.30 | 0.61 | 0.38
market   | 19.58 |  9.23 | 3.75 | 3.33 | 2.19 | 1.72 | 0.92 | 0.70 | 1.32 | 0.33 | 0.56 | 0.41
forest   | 17.48 |  6.82 | 2.26 | 4.17 | 2.67 | 2.88 | 0.95 | 0.74 | 0.16 | 0.11 | 0.58 | 0.40
lake     | 16.96 |  8.69 | 6.40 | 1.83 | 1.16 | 1.38 | 0.91 | 0.08 | 0.16 | 1.53 | 0.61 | 0.41
fortress | 17.48 |  8.24 | 4.25 | 3.56 | 1.55 | 1.48 | 0.81 | 0.11 | 0.12 | 1.80 | 0.63 | 0.39
field    | 19.16 |  8.24 | 2.88 | 4.63 | 1.85 | 1.43 | 0.93 | 2.24 | 0.07 | 1.55 | 0.54 | 0.41
orbit    |  6.74 |  3.99 | 3.62 | 0.01 | 0.23 | 0.45 | 0.16 | 0.03 | 0.02 | 0.59 | 0.32 | 0.03
Weather clouds ("compute weather clouds") at each stop, round 1 / round 2: overview 0.87 / 0.85, plaza 0.95 / 0.96,
market 0.87 / 0.92, forest 0.94 / 0.95, lake 0.98 / 0.91, fortress 0.86 / 0.81, field 0.96 / 0.93, orbit 0.16 / 0.16.
So 0.81-0.98 ms on the ground (round 22: 1.92-2.29), a bit above the expected 0.6-0.8. The other rows are round 22's
within ~0.3 ms except field's world reflections (2.60 -> 0.07 in round 2; round 1 0.02 both rounds).

The lake (systems_physics.txt: each 5 s profile of each stop's 25 s; ms a frame / max ms of a run / runs a frame;
"-" = not among the profile's listed systems):
  round 1: flow_bed_system 0.13-0.19 / max 0.21 in the four profiles at the lake; ripple_system 12.29 / max 104.69 in
  the first one after arriving (22:37:27, 61.4 ms a frame, softbody::step_beside_physics max 104.74 there too), then
  0.80-1.36 / max 1.87-8.81.
  round 2: flow_bed_system 0.18-0.20 / max 0.18-0.27; ripple_system 12.17 / max 80.14 in the first profile (22:40:48,
  59.6 ms a frame), then 0.91-1.02 / max 1.73-2.09.
  (flow_bed_system is heavy on the way out of the forest and at the fortress: 1.6-4.7 ms a frame, runs up to 24.8 ms.
  At the plaza it is not listed in the loops' 25 s stops; step 1's 75 s plaza stand shows it ramping to 2.2 ms a frame
  from ~15 s after arriving.)
  Lake extra view (round 1 snapshot, extras.txt): render.extra_views 1, 8.98 ms CPU, 4.65 ms GPU (round 22: 10.28 / 4.95).
stream_far_woods at the field and the plaza (systems_physics.txt): plaza round 1 0.12-0.21 ms a frame, max 4.26 in the
first profile after arriving, 0.33-1.18 after; plaza round 2 0.13-0.30, max 4.17 on arriving, 0.34-0.55 after; field
round 1 0.20-0.25, max 0.45-1.94; field round 2 0.21-0.24, max 0.30-2.16. Its highest single run of the whole run is
4.26 ms, so no stream_far_woods hitch.

Physics reports (physics_reports.txt: all blocks with the stop each falls in; systems_physics.txt: each block's
regions with their awake bodies as the report names them, its first four):
stop     | round 1 ms a step | round 2 ms a step | awake bodies (of all, summed over regions) r1 / r2
overview | 0.72-19.33*       | 0.59-1.45         |  7-89 / 9-64
plaza    | 1.57-2.01         | 1.31-2.80         | 50-107 / 50-93
market   | 2.99-4.33         | 2.74-3.97         | 66-94 / 51-112
forest   | 2.43-4.62         | 1.49-4.35         | 87-193 / 41-96
lake     | 2.04-6.02         | 2.10-6.24         | 50-181 / 75-130 (of ~177-205)
fortress | 2.80-3.44         | 3.09-3.73         | 64-99 / 38-93
field    | 1.88-3.51         | 1.72-3.97         | 48-77 / 46-89
orbit    | 0.72-1.85         | 0.54-2.25         | 0-49 / 0-103
(* the start.) The awake lists name the same bodies at every stop: the cars (Red, Blue, Rally, Parked), Circuit plane,
Departing plane, Hopper, Parked spaceplane, Motorboat, Rowboat, Sailing dinghy, the yard's Wrecking ball, Swing seat,
Bead curtain ball and Press, the Freight car, the softbody bridge's planks, the dummies' ragdoll parts, and, from the
forest on, destruction Debris; at the fortress also Plank door and Grappler.
physics_simulate (extras.txt) per profile, round 2: market 2.3-2.9 ms a frame, forest 2.1-9.0, lake 4.4-5.1, fortress
7.5-8.5 (round 22: 2.6-2.9, 1.8-4.8, 3.8-4.0, 4.4-4.8).

Files: tables.txt, passes.txt, extras.txt, systems_physics.txt (loops23.py), physics_reports.txt, summary.json,
frames.csv, timeline.txt, nvidia_dmon.txt, jpg/ (round 1 stops), logs.zip (stderr with every profile, stats JSON per stop).
