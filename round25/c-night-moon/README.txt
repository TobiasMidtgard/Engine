Round 25 step c: round 24 step c again on main e11f16aa (Weather's MoonSource adoption c6be90e7 and 2056f9a4 "Water: no
glassy sheets up steep banks" are in it), Tobias's PC, release, Ultra, window 1600x900, showcase.exe --fly --profile
--quality ultra. Started after 24,988 s with no input, 05:19:59 UTC. Input only through the bridge, as round 24:
showcase.time {"hour": h, "hours_per_second": 0}, then showcase.goto each place, 12 s there, then space.bodies,
weather.panel, render.* and a picture. Hours 2.5 and 3.5, places lake and sunset in round 24's order, so h2_5_lake is
again the first place after loading (where round 24 had the green wedge), and h3_5_sunset is again the one picture with
the Moon in it. Day of year 80, moon phase 0.42 (the defaults). Not a timing run: 15.0, 22.4, 34.4 and 28.0 fps
(engine.status, last ~8 s before each picture; round 24: 13.6, 13.6, 25.0, 19.1).

Predicted (night24.py, unchanged; moons_predicted.txt): the same as round 24, the space Moon at
  h2_5_lake    elevation 37.5, bearing 244.1: (991, -305), above the frame
  h2_5_sunset  37.5 / 244.1: (738, -202), above the frame
  h3_5_lake    26.8 / 255.7: (1190, -89), above the frame
  h3_5_sunset  26.8 / 255.7: (914, 19), at the top edge; from the camera 169.6 deg from the Sun, phase angle 10.4 deg,
               lit fraction 0.992, 0.525 deg across
Seen (crops/, nearest; crops.txt has each crop's box, brightest, greenest and mean pixel):
  Green wedge: none. h2_5_lake's near water is dark blue-black everywhere: near_water_h2_5_lake_1x (round 24's wedge box,
    (0,720)-(960,900)) and near_shore_h2_5_lake_1x (the whole bottom), then x4 crops where the wedge's wide end, middle
    and tip were (near_water_left/mid/tip), the near beach and water's edge (near_shore_right, near_shore_far_right),
    and x6 the far shore under the waterfall. Greenest pixel in any of them rgb (10, 26, 30); a count of bright green
    pixels (g > 120 and g - (r + b) / 2 > 40, every 2nd pixel of the bottom third) gives 0 in h2_5_lake and h3_5_lake,
    and 4729 in round 24's h2_5_lake.jpg. The moon glitter runs to the near beach on the right, with a thin bright
    swash line at the water's edge (near_shore_right x4, (1047, 883) rgb 252).
  Moon: one. The only disc in the four pictures is the space Moon in h3_5_sunset at (914, 23) (moon_h3_5_sunset_x6 and
    _x12), predicted (914, 19). It is still drawn as a crescent: a bright right limb ~8-13 px with its glow, dark on the
    left, the same look as round 24's crops/space_moon_h3_5_sunset.png, though it should be ~99% lit (a near-full disc)
    from where it is. h2_5_lake, h2_5_sunset and h3_5_lake have no moon in the frame, as predicted.

Files: moons_predicted.txt, jpg/ (1280x720), crops/, crops.txt, timeline.txt, frames.csv, nvidia_dmon.txt, logs.zip
(stderr.log, bodies_*.json = space.bodies, weather_*.json = weather.panel, render stats, zones.json, places.json).
