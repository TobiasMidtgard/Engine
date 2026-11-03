Round 23 step 4: night sky, main 70b02d16, Tobias's PC, release, Ultra, window 1600x900, showcase.exe --fly --profile
--quality ultra. Input only through the bridge: showcase.time {"hour": h, "hours_per_second": 0} (games/showcase/src/
main.rs: the sky clock; the weather's Sky.hour, which the Sun, the Moon and the planets follow), then showcase.goto
each place without its own hour, 12 s there, then space.bodies, weather.panel, render.* and a picture. Hours 22.5
(the "night" place's own hour), 2.5 and 3.5; places plaza, night (across the plaza to the fountain, WSW), sunset
(over the plaza, WSW) and lake (SW). Day of year 80, moon phase 0.42 (the defaults). Exit: killed after the last
picture, as planned.
The bridge can't turn the camera, so "toward the moon" is by the hour: from the weather's moon formula
(crates/weather/src/climate.rs moon(): phase 0.42 puts it 10.1 h behind the sun) it stands low in the WSW at 2.5-3.5 h,
in the lake, night and sunset views. The plaza place looks SSE, where the moon is never low at night (it passes due
south at 55 deg up near 22.1 h), so its pictures have no moon.

There are two moons in the sky, in different places, and they look different (moons_predicted.txt: each one's
elevation, bearing and pixel in every picture, from space.bodies and the weather clock; crops/ are 4-6x crops):
1. The weather's moon: the light that lights the night scene (it makes the glitter on the lake), drawn as the light's
   disc. A full, round, cream disc ~20 px across (~1.2 deg; the real Moon is 0.52 deg), though phase 0.42 is ~94% lit.
   Seen in h2_5_lake.jpg at (957, 48) of the 1280x720 jpg (predicted (1196, 61) of 1600x900: elevation 19.5,
   bearing 256.3), just above the right-hand ridge (crops/weather_moon_h2_5_lake.png).
2. The space Moon (engine_space's body, 378 900 km away on its orbit, keeping to the same clock): a small crescent,
   two bright slivers on its right limb, ~8-13 px. Seen in h3_5_sunset.jpg at (733, 14) of the jpg (predicted (914, 19):
   elevation 26.8, bearing 255.7; crops/space_moon_h3_5_sunset.png). From the camera it is 169.6 deg from the Sun,
   where a real Moon would be ~99% lit, so its crescent doesn't match where it is.
   At 2.5 h it is at 37.5 deg elevation, bearing 244.1 (the weather moon: 19.5, 256.3); at 3.5 h 26.8 / 255.7 against
   7.4 / 265.4. So the space Moon is ~18-19 deg higher and 9-12 deg further south.
No single picture shows both: in h3_5_sunset the weather moon falls at (1067, 341) of 1600x900, behind the big plaza
tree (crops/weather_moon_behind_tree_h3_5_sunset.png: leaves only); in h2_5_lake the space Moon is above the frame.
At 3.5 h the weather moon is behind the lake's right-hand ridge (h3_5_lake, predicted (1372, 269); its glitter still on
the water) and behind the mountains in h3_5_night (915, 163). At 22.5 h both are high (47-55 deg) and above every
frame; at 2.5 h in the night and sunset views the weather moon is behind the tree or just above the top edge.
A third, the sky shader's stand-in moon opposite the Sun (crates/render/src/shaders/sky.wgsl; frame.rs sky_night:
drawn only while the Sun is the main light), was in no frame (predicted 0-3 px above the top edge at best, in
h3_5_sunset at (738, -3)); with the weather's moon up and the main light, the code keeps it off.

Per picture (1280x720 jpg/): moons seen
  h22_5_plaza  none (both above the frame; fireworks over the square)   h22_5_night  none   h22_5_sunset none
  h22_5_lake   none (a glitter path mid-lake under a moonlit cloud)
  h2_5_plaza   none   h2_5_night   none   h2_5_sunset  none (weather moon behind the tree)
  h2_5_lake    1: the weather moon, full disc, top right
  h3_5_plaza   none   h3_5_night   none (weather moon behind the mountains)
  h3_5_sunset  1: the space Moon, a crescent at the top edge (weather moon behind the tree)
  h3_5_lake    none (weather moon behind the ridge)

Frame rate at night (engine.status over each stop's last ~8 s): plaza 22.9 / 37.0 / 34.4 fps at 22.5 / 2.5 / 3.5 h,
night 14.4 / 24.5 / 23.0, sunset 15.5 / 16.1 / 14.1, lake 23.0 / 25.2 / 23.6. The sunset view (the whole lit plaza)
is the main thread's: profile 22:52:43, 62.0 ms a frame: render_system 21.1 ms a frame, FixedUpdate 31.2 at 3.7 runs a
frame (softbody::step_beside_physics 10.6, water::flow_bed_system 5.7, physics_simulate 3.6); GPU 17.6 ms.

Files: moons_predicted.txt (night23.py), jpg/ (the 12 pictures), crops/, timeline.txt, frames.csv, nvidia_dmon.txt,
logs.zip (stderr.log, bodies_*.json = space.bodies, weather_*.json = weather.panel, render stats, zones.json, places.json).
