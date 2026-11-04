Round 24 on Tobias's PC (RTX 5080, 32 threads, 64 GB), release, Ultra, window 1600x900 as the exe opens it, real time,
every showcase run with --profile. Steps a and b on main 69c48190 (origin/main at the start; has 7a62904a), step c on
c6be90e7 (main 69c48190 + Weather's "Weather follows the real Moon"). No asset manifest changes since 70b02d16, so no
fetch-assets. Both builds at below-normal priority with nothing else running (69c48190 23:42-23:45 UTC, 3 m 11 s;
c6be90e7 23:58-00:00, 2 m 23 s). Every game run started only after 5+ min with no input (4957-5837 s), one at a time,
and ended with taskkill /IM showcase.exe /F. Input only through the game's flags and its bridge. Runs 23:46-00:03 UTC,
2026-11-03/04.
a-plaza-ab.txt            the A/B table (both windows) of a1 against a2
a1-plaza-plain            ENGINE_PROFILE=1 --fly --profile, showcase.goto plaza, 75 s stand (round 23 step 1's method), plus
                          engine.stats "render.cpu." every 5 s (dumps/); analysis.txt: every profile, render keys, dumps
a2-plaza-fixed-sync-sets  the same with ENGINE_FIXED_SYNC=sets
b-loops-ultra             --loops overview,plaza,market,forest,lake,fortress,field,orbit:1 with PHYSICS_REPORT=240 (round 23
                          step 3a's method, one round): tables, npc_water.txt (think_and_act, find path, nav chunk,
                          ripple_system per profile), hitches.txt, passes, physics reports, a picture per stop
c-night-moon              showcase.time 2.5 and 3.5 h, lake and sunset views (round 23 step 4's h2_5_lake and h3_5_sunset
                          and their pairs), moons_predicted.txt, crops/ (4-5x)
scripts                   the drivers and analyses (run24.ps1 = round 23's driver plus a stats dump every N s of a stand;
                          chain24.ps1 / wait_then.ps1 wait for 5 idle minutes; ab24.py, loops24.py, hitches24.py,
                          night24.py, crops24.py are new; the rest are round 23's)

Summary
a) Plaza 75 s stand, plain vs ENGINE_FIXED_SYNC=sets (25-75 s after arriving; 30-75 s in brackets, which drops the plain
   run's first profile, the one covering a 25-30 s dip; means over the 5 s profiles, ranges in a-plaza-ab.txt):
                              plain                 sets
   fps (engine.status)        39.7 (40.5)           37.9 (38.4)
   frame ms (profiles)        25.3 (24.7)           26.1 (26.1)
   fixed steps a frame        1.51 (1.48)           1.58 (1.56)
   FixedUpdate ms a frame     8.77 (8.37)           9.27 (9.27)    range 7.85-12.40 / 8.43-10.19
   FixedUpdate ms a run       5.74 (5.62)           5.92 (5.88)
   FixedUpdate ordering floor 5.01 (4.83) a frame   5.77 (5.78)    3.29 / 3.68 a run; 67-73 / 50-53 systems on the line
   systems busy ms a frame    26.95 (26.30)         28.48 (28.55)
   cores busy                 1.06 (1.06)           1.09 (1.09)
   one system at a time ms    20.36 (19.87)         20.89 (20.93)
   no system ms               2.11 (2.06)           1.93 (1.92)
   several at once ms         2.82 (2.76)           3.26 (3.27)
   Render / Update / PostUpdate  8.65 / 3.70 / 3.06   8.79 / 3.75 / 3.12
   render.wait_ms             0.01 (p90 0.01)       0.01 (p90 0.01)
   render.extract_ms          8.54 (p90 9.61)       8.87 (p90 9.79)
   render.record_total_ms     6.20 (p90 7.10)       6.19 (p90 6.84)
   worst frame (profiles)     46.6 (39.2-59.2)      51.2 (39.2-64.9)
   The flag took effect: FixedUpdate has 19 apply_deferred sync points plain (200 systems, 24 exclusive) and 4 with
   sets (185 systems, 9 exclusive; the parallelism report in each stderr.log). With it, systems overlap a little more
   (several at once +0.4-0.5 ms, cores 1.06 -> 1.09) but the fixed step is not faster: FixedUpdate +0.18-0.26 ms a run, its
   ordering floor +0.4 ms a run (the chain sometimes runs through smoke::step 2.0 ms instead of physics_simulate),
   and the frame is ~0.8-1.4 ms slower (fps -1.8 to -2.1). One run each, and the NPCs behave differently (sets:
   think_and_act 0.68 ms a frame against 0.38; smoke::step 1.68 against 1.25; physics_simulate 1.78 against 1.47
   with more steps a frame), so the difference is within what one run each can tell apart; it does not show a gain.
   render.cpu.* every 5 s (dumps/, table in each analysis.txt, A/B means in a-plaza-ab.txt): the same in both within
   ~0.25 ms per step; steady top steps prepare meshes 2.4 / 2.6, prepare on the render thread 2.3 / 2.4, wait for the
   finishing threads 1.9 / 1.7, meshes: cull 1.2, meshes: sync 1.2, prepare softbody 1.2 / 1.1, prepare terrain 1.2 / 1.3.
   Against round 23 step 1 (70b02d16, the same method): 31.9 -> 39.7 fps, FixedUpdate 14.0 -> 8.8 ms a frame (floor
   9.4 -> 5.0), 1.88 -> 1.51 fixed steps a frame, systems busy 34.3 -> 27.0 ms, extract 9.6 -> 8.5 ms, record_total 6.2
   -> 6.2, wait 0.01 -> 0.01. water::flow_bed_system is off the FixedUpdate chain (round 23: 2.2 ms on it).
b) Loops x1 at Ultra (fps / 1% low; round 23 3a round 1 and round 2 fps):
   stop     | r24          | r23 r1       | r23 r2 | worst frame r24 (r23 r1) | render_system | GPU ms | GPU busy %
   overview*| 38.2 / 27.0  | 38.8 / 29.5  | 44.9   | 163.9 (165.2)            | 10.43 (10.15) | 24.0   | 92
   plaza    | 42.2 / 34.2  | 40.4 / 33.5  | 40.6   |  92.8 (90.6)             |  8.83 (8.98)  | 16.3   | 73
   market   | 30.5 / 24.5  | 28.9 / 19.0  | 28.8   | 107.2 (118.3)            |  9.88 (9.97)  | 19.0   | 59
   forest   | 33.4 / 15.6  | 31.1 / 16.8  | 29.2   |  86.0 (153.0)            |  8.51 (11.26) | 17.2   | 65
   lake     | 26.7 / 22.2  | 24.7 / 20.5  | 23.7   | 108.5 (123.7)            | 17.82 (22.09) | 17.0   | 62
   fortress | 19.2 / 12.8  | 20.6 / 13.9  | 16.0   | 119.7 (124.7)            | 12.03 (13.03) | 16.7   | 35
   field    | 43.3 / 35.8  | 41.8 / 35.8  | 39.3   |  95.4 (94.0)             |  7.59 (8.08)  | 18.7   | 84
   orbit    | 66.1 / 61.0  | 65.5 / 59.6  | 64.6   |  53.6 (55.8)             |  6.13 (6.09)  |  6.7   | 52
   (* includes the start.) +1.5 to +2.3 fps against round 23 round 1 from the plaza to the field, but the fortress
   (-1.4); overview (with the start) -0.6, orbit +0.6.
   engine_npc::think_and_act worst single runs: 63.0 ms (find path 63.0, in the first overview profile right after the
   start, a 1513.9 ms frame of smoke::run_demos 1420.9), then 20.6 (fortress), 12.9 (plaza), 12.4 (field), 10.0
   (market). find path is listed in 5 of 43 profiles: 63.0, 7.9, 3.7, 2.4, 2.4. Round 23's loops round 1 had 57.0 at
   most too; its 150-415 ms runs came in round 2 and in step 1's 75 s plaza stand (231.7, 220.6). The like-for-like
   check is this round's two 75 s stands: think_and_act at most 58.7 ms plain (find path 58.7, 5 s after arriving),
   then 25.1; with sets 33.3. So no 150-415 ms run anywhere this round.
   "nav chunk": listed in 3 of 43 profiles (counts are runs a frame x frames, a floor: profiles list the 40 heaviest
   inner spans): overview start ~88 chunks, max 25.1 ms; fortress ~42, max 20.5; field ~37, max 12.4; ~167 in all.
   Plaza stands: ~91 (max 22.6) plain, ~95 (max 18.2) sets. A single chunk takes longer than in round 23 (max 17.3 in
   loops round 1, 12.8 in the stand) now that they build side by side, but the find path around them is much shorter.
   water::ripple_system on arriving at the lake: 10.98 ms at most (ripples: rebuild 10.28) in the first profile, then
   1.09-1.15 ms a frame, max 1.88-2.67 (round 23: 104.7 / 80.1 once). The lake arrival's remaining 168.6 ms frame is
   render_system 150.1: the lake's extra view building its first frame (121.5 ms CPU, prepare space bodies 19.7);
   round 23 had the same, 97-104 ms, under the ripple hitch.
   New: at the fortress ripple_system is 10.0-13.7 ms a frame with runs of 30-39 ms through the whole stop, because
   "ripples: rebuild" runs every fixed step (2.8-3.0 a frame, max 28-36 ms); round 23 round 1 fortress: 1.4-4.3 ms a
   frame, max 4.3-8.9, rebuild only in its first two profiles. The forest also rebuilds (max 17.7-23.4 ms; round 23
   once at 40.8). Also new: foliage::press_fallen_crowns 107.3 ms once at the plaza, 55.2 at the fortress.
   Most other 80-120 ms worst frames at the stops are my own round 1 picture (bridge_frame_start 51-57 ms + screenshot
   save 27-40 ms), as in round 23.
c) Moons (Weather's adoption c6be90e7 is in main, so step c ran): one moon now. In h3_5_sunset the space Moon is at
   (915, 22) of 1600x900, predicted (914, 19) from space.bodies (elevation 26.8, bearing 255.7); the weather draws no
   disc of its own: nothing at round 23's weather-moon spots ((1196, 61) in h2_5_lake, where round 23 had the full disc;
   (1067, 341) in h3_5_sunset, behind the tree either way), and h2_5_lake, h2_5_sunset and h3_5_lake have no moon in the
   frame, as predicted (above the top edge at 2.5 h; (1190, -89) at 3.5 h in the lake view). The moonlight now comes
   from the Moon's direction: the lake's glitter path moved left, under bearing 244 (2.5 h) and 256 (3.5 h), and looks
   dimmer than round 23's. The Moon is still drawn as a crescent (bright right limb, ~8-13 px with its glow, as in round 23) though
   from the camera it is 169.6 deg from the Sun (phase angle 10.4, ~99% lit). Also new: a bright green-white wedge with
   straight edges on the water at the bottom left of h2_5_lake (crops/green_slab_h2_5_lake.png), the first place visited
   after loading; not there at 3.5 h from the same spot, not in round 23's h2_5_lake, not in the daytime loops lake
   picture.
