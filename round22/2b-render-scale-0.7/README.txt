Round 22 step 2b: the same loops as 2a with ENGINE_RENDER_SCALE=0.7, run right after 2a on the same build (main 5de4f33e),
Ultra, window 1600x900 (the internal resolution x0.7, ~1120x630), PHYSICS_REPORT=240 RUST_LOG=info, 2 rounds. Exit 0.

Per-pass GPU, round 2, ms at scale 1.0 (2a) -> 0.7 (scale_compare.txt; ratio in brackets; a pass that pays per pixel
would go to ~0.49):
pass                   | overview         | plaza            | market           | forest           | lake             | fortress         | field            | orbit
GPU total              | 21.73->15.78 .73 | 18.02->14.31 .79 | 21.97->17.27 .79 | 18.74->15.22 .81 | 18.47->13.35 .72 | 19.96->15.41 .77 | 23.41->16.02 .68 | 7.63->4.70 .62
scene                  | 13.80-> 9.44 .68 |  6.48-> 4.75 .73 |  9.41-> 6.56 .70 |  6.89-> 5.23 .76 |  8.67-> 5.81 .67 |  8.86-> 6.47 .73 |  8.34-> 5.87 .70 | 4.57->2.31 .51
scene: terrain         |  7.67-> 5.01 .65 |  1.79-> 1.33 .74 |  3.23-> 2.16 .67 |  2.30-> 1.76 .77 |  6.36-> 4.09 .64 |  4.01-> 2.88 .72 |  2.93-> 2.11 .72 | 4.21->2.03 .48
scene: meshes          |  5.33-> 3.71 .70 |  3.91-> 2.73 .70 |  3.21-> 2.15 .67 |  4.10-> 3.02 .74 |  1.85-> 1.29 .70 |  3.60-> 2.57 .71 |  4.61-> 3.08 .67 | 0.01->0.01
prepass                |  2.10-> 1.82 .87 |  2.22-> 1.93 .87 |  3.01-> 2.72 .90 |  2.73-> 1.97 .72 |  1.19-> 0.95 .80 |  2.24-> 1.99 .89 |  1.93-> 1.60 .83 | 0.24->0.20
shadows                |  0.83-> 0.81     |  1.75-> 1.68     |  1.90-> 1.86     |  2.78-> 2.94     |  1.36-> 1.40     |  1.58-> 1.76     |  1.60-> 1.57     | 0.45->0.45
compute weather clouds |  1.93-> 1.42 .74 |  2.29-> 1.64 .72 |  2.17-> 1.54 .71 |  2.18-> 1.53 .70 |  2.22-> 1.56 .70 |  1.92-> 1.38 .72 |  2.21-> 1.54 .70 | 0.46->0.42
transparent            |  0.39-> 0.28     |  0.26-> 0.24     |  0.30-> 0.22     |  0.12-> 0.10     |  1.66-> 0.91 .55 |  1.80-> 0.86 .48 |  1.55-> 0.95 .61 | 0.59->0.31 .53
probe gi               |  0.03-> 0.03     |  1.28-> 1.24     |  0.76-> 0.72     |  0.76-> 0.72     |  0.09-> 0.08     |  0.12-> 0.10     |  2.33-> 2.21     | 0.03->0.03
bounce light           |  0.62-> 0.30 .48 |  0.61-> 0.29 .48 |  0.56-> 0.27 .48 |  0.58-> 0.28 .48 |  0.61-> 0.30 .49 |  0.62-> 0.30 .48 |  0.54-> 0.26 .48 | 0.32->0.16 .50
volumetric fog         |  0.25-> 0.13     |  0.28-> 0.16     |  0.29-> 0.16     |  0.28-> 0.16     |  0.27-> 0.16     |  0.29-> 0.17     |  0.30-> 0.17     | 0.14->0.08
ambient occlusion      |  0.15-> 0.08     |  0.26-> 0.12     |  0.27-> 0.12     |  0.21-> 0.11     |  0.19-> 0.10     |  0.22-> 0.11     |  0.27-> 0.12     | 0.05->0.04
aerial perspective     |  0.34-> 0.34     |  0.38-> 0.38     |  0.41-> 0.40     |  0.40-> 0.40     |  0.41-> 0.41     |  0.39-> 0.40     |  0.41-> 0.40     | 0.03->0.03
environment            |  0.22-> 0.22     |  0.22-> 0.22     |  0.22-> 0.22     |  0.22-> 0.22     |  0.22-> 0.22     |  0.22-> 0.22     |  0.21-> 0.21     | 0.01->0.01
Summed over the 8 stops: GPU total 149.9 -> 112.1 (0.75); scene 0.69 (terrain 0.66, meshes 0.70); prepass 0.84; shadows
1.02; weather clouds 0.72; transparent 0.58; probe gi 0.95; bounce light 0.48; fog 0.57; AO 0.49; reflections 0.61;
aerial perspective 1.00; environment 1.00; sky occlusion 1.01; anti-aliasing 0.56. world reflections 5.01 -> 2.12 is
2a's field outlier (2.60 there; 0.02-0.06 in the other three field profiles).
So per pixel: bounce light, AO, fog, anti-aliasing, transparent (where it has work); about 0.7: the scene's terrain and
meshes and the weather clouds; mostly per vertex / per draw: prepass (0.84); not by pixels at all: shadows, probe gi,
aerial perspective, environment, sky occlusion.

fps hardly moves at 0.7 (tables.txt; the stops are CPU-bound, GPU 33-69% busy in round 1, overview 90): round 1 plaza 39.5 (2a 41.6),
market 30.2 (28.8), forest 35.4 (31.8), lake 25.0 (22.4), fortress 19.6 (22.6), field 42.1 (40.3), orbit 65.3 (64.1).

Round 2 in this run did NOT hold round 1 from forest on: forest 35.4 -> 24.5, lake 25.0 -> 21.8, fortress 19.6 -> 15.4,
field 42.1 -> 34.5. physics_simulate (extras.txt): round 2 forest 10.7-14.0 ms a frame, lake 5.3-12.1, fortress
11.2-17.9, field 4.1-8.2 (round 1: 1.4-3.8, 3.7-5.1, 7.7-9.2, 2.0-2.2). The physics reports put it in Rapier's CCD
sweeps (ccd_sweeps_both_runs.txt: per report ms per step / Rapier / Rapier's whole step / CCD sweeps):
- this run, round 2 forest: CCD sweeps 2.06, 2.09, 1.36, 3.98, 2.73, 1.39 ms a step (of 1.7-4.7 for Rapier's whole
  step); lake 3.08 then 0.25-1.86; fortress 0.78-3.36; field 0.63-2.79; even round 2 orbit's first report 2.03. Round 1
  fortress was already 0.92-2.40.
- 2a (scale 1.0, the run before): 0.00-1.25 everywhere, 0.13-0.34 in round 2 from market to fortress.
The regions that cost in those reports (physics_reports.txt) are tier 0-1 (near) regions 145-345 m from the focus with:
the lake's Motorboat (5.7 m/s, at about -330 6 20) and Rowboat; Departing plane and Circuit plane (53-64 m/s, 145 m up);
the Rally car (11 m/s); and the fortress's destruction Debris falling at 19-24 m/s at y = -10 to -20 (around -300 -20
200, below the ground there). Some regions with 0 bodies still cost 0.6-3.1 ms a step in those reports (e.g. round 2
forest 18:49:59, region 1: tier 0, 0 bodies, 1990 colliders, 1 pair, 3.07 ms a step over 177 runs).
So the round-2 slowdown is not gone: it did not show in 2a and did in 2b, from CCD sweeps in near regions.

Files: scale_compare.txt, passes.txt (the per-pass table of this run), tables.txt, extras.txt, physics_reports.txt (all
96 reports with their stop), ccd_sweeps_both_runs.txt (2a and 2b), frames.csv, timeline.txt, nvidia_dmon.txt, jpg/
(round 1 stops at 0.7), logs.zip (stderr, engine-profile.txt, stats JSON per stop).
