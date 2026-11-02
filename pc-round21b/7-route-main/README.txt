Step 7: showcase.exe --route --profile --quality ultra on main aac391d4 (release; window as the exe opens it,
client 1600x900). The whole route ran (84.9 s of route time) and the game exited by itself: no crash in
crates\anim\src\motion.rs (step 1's crash at 104d82b8 is gone).

Legs (route log): village 8.2 s, face up 8.3, climb to 2 km 17.3 (2023 m), cruise at 2 km 29.5 (4931 m),
face down 29.6, climb to orbit 38.7 (401 km), orbit 44.8 (419 km), descend to 2 km 55.4 (1990 m), turn home
55.5, cruise home 67.6 (1646 m), down to 100 m 72.7 (97 m), low pass 82.8 (1 m up), end 84.9 (1 m up).
The low pass ends 1 m above the ground: 12_low_pass.jpg and 13_end.jpg face a stretched rock surface.
First frames: first_001 is near-white over water, first_004/008 the stretched green/brown wall (as in step 5).

frame_ms reads 250.00 for frames of 0.25 s or more (the frame time is clamped). Every frame of the route is
over the 33 ms hitch line, so route.txt's hitch list (941) is every frame; it prints the first 60.
