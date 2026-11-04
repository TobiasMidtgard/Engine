Round 24 step c: the moon after Weather's adoption of engine_world::MoonSource (c6be90e7, "Weather follows the real
Moon"; git log --grep MoonSource shows it beside the lead's 3699fcf1), main c6be90e7, Tobias's PC, release, Ultra,
window 1600x900, showcase.exe --fly --profile --quality ultra. Started after 5837 s with no input, 00:00:48 UTC.
Input only through the bridge, as round 23 step 4: showcase.time {"hour": h, "hours_per_second": 0}, then showcase.goto
each place, 12 s there, then space.bodies, weather.panel, render.* and a picture. Hours 2.5 and 3.5, places lake (SW)
and sunset (WSW over the plaza): round 23's h2_5_lake and h3_5_sunset and their pairs h2_5_sunset and h3_5_lake.
Day of year 80, moon phase 0.42 (the defaults). Not a timing run: the lake at 2.5 h is the first place after loading
(13.6 fps); then sunset 13.6, lake 25.0 and sunset 19.1 fps at 3.5 h (engine.status, last ~8 s before each picture).

Expected now (night24.py, moons_predicted.txt): the weather's moonlight and disc follow the space Moon, and the weather
draws no disc when space draws the body, so there is one moon, at the space Moon's place:
  h2_5_lake    elevation 37.5, bearing 244.1: (991, -305), above the frame
  h2_5_sunset  37.5 / 244.1: (738, -202), above the frame
  h3_5_lake    26.8 / 255.7: (1190, -89), above the frame
  h3_5_sunset  26.8 / 255.7: (914, 19), at the top edge
Seen (crops/, 4-5x, nearest; crops.txt has each crop's box and brightest pixel):
  h3_5_sunset: the Moon at (915, 22) (crops/space_moon_h3_5_sunset.png), a crescent with a bright right limb, ~8-13 px
    with its glow, as round 23's space Moon; seen from the camera it is 169.6 deg from the Sun (phase angle 10.4 deg,
    ~99% lit), so the drawn phase still doesn't match where it is. Nothing else: at round 23's weather-moon spot
    (1067, 341) only the tree's leaves (crops/old_clock_moon_spot_h3_5_sunset.png).
  h2_5_lake: no moon in the frame; round 23's full disc at (1196, 61) is gone (crops/old_clock_moon_spot_h2_5_lake.png:
    sky and the ridge only); the top edge under the Moon shows dark sky, a faint wisp and a star, no glow (crops/top_edge_below_moon_h2_5_lake.png). The
    glitter on the water has moved left, under the Moon's bearing, and is dimmer than round 23's.
  h3_5_lake: no moon in the frame (crops/top_edge_below_moon_h3_5_lake.png: clouds and the ridge, no glow); the glitter path runs to the bottom right,
    under bearing 256.
  h2_5_sunset: no moon (above the frame).
So no picture shows two moons, and the moonlight comes from where the Moon is.
Also: a bright green-white wedge with straight edges on the water at the bottom left of h2_5_lake
(crops/green_slab_h2_5_lake.png, 1x): the first place visited after loading, 12 s after arriving; not in h3_5_lake from
the same spot 28 s later, not in round 23's h2_5_lake, not in this round's daytime loops lake picture (b, 05-lake.jpg).

Files: moons_predicted.txt (each moon's elevation, bearing and pixel in each picture: the space Moon, the Sun, the sky
shader's stand-in opposite the Sun, round 23's clock moon for reference; the Moon's elongation, phase angle, lit
fraction and apparent size from the camera; weather.panel's moon fields), jpg/ (1280x720), crops/, crops.txt,
timeline.txt, frames.csv, nvidia_dmon.txt, logs.zip (stderr.log, bodies_*.json = space.bodies, weather_*.json =
weather.panel, render stats, zones.json, places.json).
