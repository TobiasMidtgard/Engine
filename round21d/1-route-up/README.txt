Round 21d step 1b: showcase.exe --route up --profile --quality ultra on main 1810b539, Tobias's PC (RTX 5080), release,
window as the exe opens it (client 1600x900), real time. The route ran to orbit (23.0 s of route time) and the game
exited by itself, code 0. Started right after step 1a (the same build, nothing else running).

Legs (route.txt):
leg | frames | mean ms | max ms | over 33 ms | terrain prepare max ms | patches missing max | RAM max MB | ends at (route s, m up)
village        |   7 | 189.9 | 250.0 |   7 | 3.2 | 195 | 4613 | 1.3 s, 5 m
face down      |   1 |  82.7 |  82.7 |   1 | 1.8 | 108 | 4613 | 1.4 s, 5 m
climb to orbit | 727 |  28.2 | 250.0 | 153 | 5.1 | 357 | 7172 | 21.9 s, 403094 m
orbit          |  53 |  21.9 |  72.0 |   4 | 2.2 |   9 | 5216 | 23.0 s, 406014 m
RAM (route.txt): 4613 MB at the start, 7172 max, 5111 at the end. Private bytes 14.0-14.4 GB during the climb.
Hitches: 165 of 788 frames over 33 ms; route.txt lists the first 60 with their slowest render steps.
Climb to orbit per frame time (route.csv): 153 over 33 ms, 95 over 50, 4 over 100 (250.0 at 11 m, 234.0 at 16 m, 171.6
at 200 m, 108.3 at 20 m); median 18.7 ms.

Where the time goes (5 s frame profiles, analysis.txt):
- The route starts 1.3 s after the plaza settle, inside the startup: the first profiles have stream_system max 1148 ms,
  npc::build_stand_ins max 632 ms, smoke::run_demos max 1130 ms, fire::run_demos max 858 ms.
- Route s 5-10 (79 m to 1.05 km up): 76 ms a frame, softbody::step_beside_physics 21.9 ms a frame (16.2 ran alone),
  render_system 14.7 (max 121), npc::think_and_act max 178 ms, FixedUpdate 2.3 steps a frame. Route s 10-15 (1.05 to
  12.7 km): 33 ms a frame (step_beside_physics 8.2).
- From route s 15 (12.7 km up) on: 18.8-18.9 ms a frame (53 fps), render_system 7.2-7.5 ms, GPU passes 16.1 ms, GPU
  62-86% busy.
- water::ripple_system does not show on this route (looking down from straight above the village), unlike the 2 km
  legs of step 1a.

Files: route.txt, route.csv, analysis.txt, timeline.txt, nvidia_dmon.txt, jpg/ (each leg's end, up_5km..up_300km, the
first frames), logs.zip (stderr.log, engine-profile.txt).
