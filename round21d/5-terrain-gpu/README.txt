Round 21d step 5: Planets' terrain asks, main 1810b539, Tobias's PC (RTX 5080), release, Ultra, window as the exe opens
it (1600x900). showcase.exe --fly --profile --quality ultra; through the bridge: plaza 35 s (the first stop waits out the
startup), forest 30 s. Numbers: the last 10 s of each stay; GPU from the last render profile 10+ s after arriving (5 s
averages; --profile splits the scene pass by feature); render_system from the last 5 s frame profile.
Runs back to back on the same build, each with one switch set at launch (both are read when the pipeline is made):
- default: step 4's mirror-on run (its plaza and forest stays are this same pattern; ../4-render-asks/mirror-on/).
- depth-greater/: TERRAIN_DEPTH=greater (the scene pass the old way: GreaterEqual with depth writes and the mirror clip).
- shader-off-16/: TERRAIN_SHADER_OFF=16 (lighting off: albedo only).
- shader-off-256/: TERRAIN_SHADER_OFF=256 (everything after the materials are blended off).
(16 and 256 were run at the plaza too, since the run starts there.)

stop | config | "scene: terrain" GPU ms | prepass GPU ms | scene GPU ms | GPU total ms | render CPU ms | render_system ms/frame (max) | fps | GPU busy %
plaza  | default     | 1.72 | 2.39 |  6.76 | 21.91 | 13.36 | 7.27 (14.71; 0.8 runs a frame) | 31.2 | 70
plaza  | depth greater | 4.58 | 2.42 |  9.33 | 24.73 | 14.04 | 8.75 (20.78) | 30.2 | 78
plaza  | shader off 16 | 1.08 | 2.45 |  5.82 | 21.38 | 14.26 | 8.59 (14.34) | 26.2 | 62
plaza  | shader off 256 | 0.59 | 2.42 |  5.42 | 20.86 | 13.52 | 8.56 (15.98) | 26.7 | 59
forest | default     | 2.09 | 2.70 |  7.02 | 19.93 | 12.68 | 8.36 (12.01) | 40.3 | 84
forest | depth greater | 6.82 | 2.72 | 11.74 | 25.01 | 17.37 | 9.02 (14.58) | 37.5 | 98
forest | shader off 16 | 1.46 | 2.66 |  6.59 | 19.80 | 12.72 | 8.28 (11.75) | 40.9 | 78
forest | shader off 256 | 0.77 | 2.66 |  5.90 | 18.82 | 12.61 | 8.25 (14.29) | 41.7 | 85

Reading:
- The equal depth test (default) against the old GreaterEqual-with-writes: terrain 1.72 vs 4.58 ms at the plaza (-2.86)
  and 2.09 vs 6.82 at the forest (-4.73). In 21c (145dd3a6, the old way, Ultra 1600x900) it was 5.39 / 8.37.
- With the equal test, the terrain's lighting is 0.64 ms at the plaza (1.72 -> 1.08) and 0.63 at the forest (2.09 ->
  1.46); everything after the blend 1.13 / 1.32; what is left with 256 (geometry, materials, blending) 0.59 / 0.77.
- The prepass doesn't move with any switch: 2.39-2.45 at the plaza, 2.66-2.72 at the forest.
- The plaza's fps with 16 and 256 (26.2 / 26.7, GPU 59-62% busy) is a CPU-side difference at that stop, not the
  terrain: their GPU totals are lower than the default's.
- The default plaza profile ran render_system 0.8 times a frame (7.27 ms a frame = ~9.1 a run); the others 1.0.

Per run: analysis.txt (21c's stops analysis: the frame profile's render steps, mesh triangles, terrain at END),
passes.txt (every GPU pass), frames.csv, timeline.txt, nvidia_dmon.txt, plaza.jpg / forest.jpg, logs.zip (stderr, stats).
