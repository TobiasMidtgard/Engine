Step 1: showcase.exe --route --profile --quality ultra at 104d82b8 (release), window as the exe opens it
(client 1600x900, not maximized). Tobias's PC: RTX 5080, 32 threads, 64 GB.

The route crashed during "climb to orbit", just after the 150 km picture (22:09:48.7Z, about 63 s
after start, frame 188):
  thread 'compute (4)' panicked at crates\anim\src\motion.rs:584:37:
  index out of bounds: the len is 4456 but the index is 4488
  (MotionDb::end_of: self.ranges[self.frame_range[frame] as usize] - frame past the clip; fixed on
  main by dcaded71 "Anim: motion matching never steps past the clip playing on a long frame")
The frame then hung; the engine's watchdog logged "frame 188 has not finished for 20 s" at
22:10:09.7Z and the game was killed. So route.txt and route.csv were never written (the route
writes them at its end).

Legs finished before the crash (route log): village done 8.2 s (5 m), face up 8.4 s,
climb to 2 km done 17.8 s (2100 m), cruise at 2 km done 30.0 s (5496 m), face down 30.2 s (5570 m).
Render perf lines (10 s windows): 4.6 fps (218.8 ms, worst 433.8; GPU 27.2 ms), 4.3 fps (230.3 ms,
worst 352.1; GPU 27.3 ms), 5.3 fps (188.2 ms, worst 321.3; GPU 25.6 ms).
RAM (process, from the driver): working set 5064 MB / private 6977 MB right after the build;
7513 / 13936 MB at 49 s; 7455 / 14795 MB at 79 s (just before the hang).
Pictures: 01_village, 03_climb_to_2_km, 04_cruise_at_2_km, up_20km, up_60km, up_150km (1280 wide).
