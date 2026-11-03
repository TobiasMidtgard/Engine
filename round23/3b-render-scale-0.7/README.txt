Round 23 step 3b: the same loops as 3a with ENGINE_RENDER_SCALE=0.7, run right after 3a on the same build (main 70b02d16),
Ultra, window 1600x900 (the internal resolution x0.7, ~1120x630), PHYSICS_REPORT=240 RUST_LOG=info, 2 rounds. Exit 0.

stop     | round 1 fps / 1% low | round 2 fps / 1% low | GPU ms r1 / r2 | GPU busy % r1 / r2 | 3a (scale 1.0) r1 / r2 fps | r22 2b r1 fps
overview | 46.6* / 30.9 | 52.7 / 47.8 | 17.4 / 14.6 | 82 / 79 | 38.8 / 44.9 | n/a
plaza    | 40.6 / 35.1  | 41.1 / 35.1 | 13.2 / 12.8 | 57 / 58 | 40.4 / 40.6 | 39.5
market   | 28.0 / 19.1  | 31.7 / 25.1 | 14.7 / 14.9 | 41 / 51 | 28.9 / 28.8 | 30.2
forest   | 33.9 / 18.0  | 30.5 / 16.7 | 13.7 / 14.0 | 51 / 45 | 31.1 / 29.2 | 35.4
lake     | 23.9 / 17.9  | 24.0 / 17.6 | 12.5 / 12.5 | 43 / 43 | 24.7 / 23.7 | 25.0
fortress | 21.5 / 13.9  | 17.4 / 13.4 | 12.8 / 13.6 | 32 / 25 | 20.6 / 16.0 | 19.6
field    | 42.3 / 36.6  | 40.0 / 27.7 | 14.6 / 14.8 | 67 / 63 | 41.8 / 39.3 | 42.1
orbit    | 66.1 / 62.1  | 65.0 / 57.6 |  4.3 / 4.2  | 34 / 35 | 65.5 / 64.6 | 65.3
(* overview round 1 includes the start.) tables.txt has every column.
fps moves with the scale only at the overview (GPU-bound at 1.0: 91-95% busy -> 79-82% at 0.7, +8 fps); elsewhere
within ~3 fps, the frame being the main thread's (render_system 7.7-19.2 ms, systems busy 19-75 ms a frame on
~1.0-1.26 cores). Round 1 plaza had engine_npc::think_and_act runs of 150.3 and 173.7 ms (profiles 22:44:20, 22:44:25).

Per-pass GPU, round 2, ms at scale 1.0 (3a) -> 0.7 (scale_compare.txt has every pass, rounds 2 and 1; ratio in brackets;
a pass that pays per pixel would go to ~0.49):
pass                   | overview    | plaza       | market      | forest      | lake        | fortress    | field       | orbit
GPU total              | 20.79->14.62 | 16.07->12.76 | 19.58->14.87 | 17.48->13.98 | 16.96->12.54 | 17.48->13.62 | 19.16->14.85 | 6.74->4.25
scene                  | 14.00->9.16 | 6.45->4.53  | 9.23->6.11  | 6.82->5.05  | 8.69->5.86  | 8.24->6.01  | 8.24->5.74  | 3.99->2.14
scene: terrain         | 7.75->4.75  | 2.01->1.38  | 3.75->2.43  | 2.26->1.67  | 6.40->4.14  | 4.25->3.01  | 2.88->1.97  | 3.62->1.85
scene: meshes          | 5.46->3.71  | 4.01->2.75  | 3.33->2.14  | 4.17->3.03  | 1.83->1.28  | 3.56->2.60  | 4.63->3.15  | 0.01->0.01
prepass                | 2.10->1.78  | 1.83->1.51  | 2.19->1.87  | 2.67->1.90  | 1.16->0.94  | 1.55->1.37  | 1.85->1.54  | 0.23->0.20
shadows                | 0.79->0.73  | 1.55->1.55  | 1.72->1.69  | 2.88->2.82  | 1.38->1.38  | 1.48->1.75  | 1.43->1.43  | 0.45->0.45
compute weather clouds | 0.85->0.68  | 0.96->0.75  | 0.92->0.71  | 0.95->0.72  | 0.91->0.72  | 0.81->0.62  | 0.93->0.69  | 0.16->0.15
transparent            | 0.38->0.27  | 0.30->0.26  | 0.33->0.23  | 0.11->0.10  | 1.53->0.97  | 1.80->0.86  | 1.55->1.00  | 0.59->0.31
probe gi               | 0.03->0.03  | 1.29->1.30  | 0.70->0.73  | 0.74->0.74  | 0.08->0.09  | 0.11->0.13  | 2.24->2.20  | 0.03->0.03
bounce light           | 0.62->0.30  | 0.61->0.29  | 0.56->0.27  | 0.58->0.29  | 0.61->0.30  | 0.63->0.31  | 0.54->0.26  | 0.32->0.16
Summed over the 8 stops: GPU total 134.3 -> 101.5 (0.76; round 22 149.9 -> 112.1); scene 0.68 (terrain 0.64, meshes
0.69); prepass 0.82; shadows 1.01; weather clouds 0.78; transparent 0.61; probe gi 1.01; world reflections 0.90; bounce
light 0.49; fog 0.57; AO 0.49; reflections 0.61; aerial perspective 0.99; environment 1.00; sky occlusion 1.01;
anti-aliasing 0.56. The same split as round 22's. Weather clouds at each stop, round 2 at 0.7: 0.62-0.75 on the ground,
0.15 at orbit (round 22 at 0.7: 1.38-1.64).

The lake (systems_physics.txt): round 1 flow_bed_system 0.68 / max 7.70 in the profile that covers arriving, then
0.15-0.17 / max 0.13-0.20; ripple_system 10.71 / max 82.74 on arriving, then 0.94-1.03 / max 1.87-1.98. Round 2:
flow_bed 1.13 / max 10.92 on arriving, then 0.18-0.21 / max 0.15-0.23; ripple 13.19 / max 74.88, then 1.14-1.31 / max
1.83-2.22.
stream_far_woods at the plaza and the field: 0.11-0.25 ms a frame; its highest runs 4.06 (plaza round 1, on arriving),
2.24 (plaza round 2), 2.05-2.14 (field, on arriving), else 0.2-1.2. No stream_far_woods hitch.
Physics reports (physics_reports.txt; awake lists per region in systems_physics.txt), ms a step r1 / r2: plaza
1.48-1.87 / 1.37-2.52, market 2.59-4.50 / 2.07-3.27, forest 1.83-4.13 / 1.13-3.93, lake 2.38-5.43 / 2.08-6.09,
fortress 2.28-2.98 / 2.22-3.37, field 1.82-3.31 / 1.84-3.61, orbit 0.56-1.81 / 0.60-1.86; the awake bodies are 3a's
(cars, planes, boats, the yard's swing seat, wrecking ball and bead curtain, freight car, ragdoll parts, debris).

Files: tables.txt, passes.txt, scale_compare.txt, extras.txt, systems_physics.txt, physics_reports.txt, summary.json,
frames.csv, timeline.txt, nvidia_dmon.txt, jpg/, logs.zip.
