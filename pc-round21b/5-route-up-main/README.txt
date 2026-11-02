Step 5: showcase.exe --route up --profile --quality ultra on main aac391d4 (release; window as the exe opens it,
client 1600x900). The run flew the whole climb (village 1.2 s, face down 1.5 s, climb to orbit done at 22.6 s at
410 km, orbit 23.6 s at 423 km) and exited by itself.

Black screen check: none. Every picture from 5 to 300 km shows the planet (contact_up_5-300km.jpg; mean
brightness falls smoothly from 152 at 5 km to 82 at 300 km as more black space comes into view).

First frames (contact_first_frames.jpg): frames 1-2 look out over water to the horizon with the meadows as
thin strips, overexposed (mean brightness 246 and 218; ev100 NaN then 6.9, metered 10.1 by frame 3);
frames 4 and 8 and the end of the village leg (1.25 s) show a vertical stretched green/brown wall with black
holes across most of the view (terrain patches missing 2-32 while building); frame 16 looks down at grass,
32 and 64 show the village and valley correctly.

frame_ms in route.csv reads 250.00 for the slow frames: the frame time is clamped at 0.25 s, so those are
"250 ms or more". Frames are 90-150 ms while climbing low, ~100 ms higher up, 55-80 ms at orbit.
