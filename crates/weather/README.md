# engine_weather — Weather, sky and the ground it changes

**Owner:** Weather, wind and clouds.

Dynamic weather that moves across the world and changes it: fronts that carry clear skies,
fair-weather clouds, showers, rain, thunderstorms, snow or fog toward the player; temperature, humidity
and wind that follow the weather and the time of day; true volumetric clouds; rain and snow that
wet the ground, fill puddles and pile up; lightning; and the day–night sky with the sun, the
moon and the stars.

```rust,ignore
app.add_plugins((DefaultPlugins, engine_weather::WeatherPlugin));

// From game code:
let mut weather = world.resource_mut::<engine_weather::Weather>();
weather.preset = engine_weather::WeatherPreset::Storm;
weather.transition_s = 300.0; // the storm rolls in over five game minutes

// Or from the console, a script or an AI agent:
call_api(world, "weather.set", json!({"preset": "storm", "transition_s": 300}));
```

## What it does

**Weather that travels.** Asking for new weather doesn't flip a switch: a front forms upwind
and moves toward the player with the wind. You see the towers on the horizon first, the wind
picks up as the gust front arrives, then the rain starts, and later it clears again. Clouds,
rain and the numbers in `WeatherState` all come from one function (`WeatherSim::regime_at`), so
what you see is what falls. Presets: `clear`, `partly_cloudy`, `overcast`, `showers`, `rain`, `storm`,
`snow`, `blizzard`, `fog`. `Weather::auto` picks new weather by itself; `Weather::forecast`
queues changes.

**Air.** Temperature follows the season, the time of day, cloud cover (clouds keep nights
warm and days cool), rain (evaporative cooling) and altitude (6.5 °C per km). Dew point,
humidity, pressure, cloud base height (from the temperature–dew point spread), visibility and
fog (including radiation fog on calm, humid nights) follow. Wind has a prevailing direction,
gusts and a gust front ahead of storms. Fires warm the ground around them through
`HeatField` (snow melts there).

**Volumetric clouds.** Ray-marched clouds carved from a cloud map (coverage, type, rain,
density) with Perlin-Worley shape noise and Worley detail noise: flat stratus sheets, heaped
cumulus with cauliflower tops, and towering storm clouds that narrow as they climb and spread
into an anvil. Lit by the sun or the moon with self-shadowing, a strong forward glow (silver
linings), multiple-scattering diffusion, dark rain bellies, sky and ground ambient light, and
lightning lighting them from inside. They sit in the atmosphere (aerial perspective, distance
haze), cast grey rain shafts under distant showers, wrap around mountains and the camera when it
flies through them, and are visible from above (a sea of clouds from a plane or a mountain).
They also feed the environment map, so reflections and ambient light change with the sky.

**Rain and snow.** Rain streaks with motion blur that catch the light of lamps behind them,
splashes on the ground and on roofs, snowflakes that flutter; all generated on the GPU (no CPU
cost per drop) in a box that follows the camera, blown by the wind, stopped by roofs (the sky
occlusion map).

**The ground changes.** The weather writes the shared `SurfaceLayers` grid around the player:
rain wets exposed ground within a millimeter; what the ground can't soak up collects in the low
spots as puddles (asphalt pools, sand soaks it up, see `GroundPatch`); snow piles up below
freezing and melts by degree-days; sun, wind and dry air evaporate it again. Covered ground
stays dry (roofs with colliders are found by raycasts; `WeatherShelter` marks covers without
one). A ground pass draws it: puddles that mirror the world (screen-space reflections, the sky
where they leave the screen) with rings from raindrops, darker wet ground, and snow lying in the
open. Puddles only show within a few centimeters of the ground height, so grass blades,
tabletops and car roofs stay dry. Footsteps, fire and vehicle grip can read the same grid.

**Lightning.** Storm cells strike on their own (`Lightning`), or on demand
(`weather.lightning`). Bolts are jagged, branching channels with a return-stroke flicker; the
flash lights the clouds from inside and the ground. Strikes hit the tallest thing near the
spot, send `NoiseEvent` (thunder for AI), `WorldImpulse` and, where the ground is dry,
`IgniteRequest`, and emit the world's `LightningStrike` message for sound and gameplay. The
drawn main channel is `LightningStrike::path`, so thunder rolls along the bolt you see.

**Day and night.** A clock and calendar (`Sky`) for a place on a planet (`Climate`: latitude,
axial tilt, year length) drive the sun and moon `DirectionalLight`s: real sun paths through
the seasons, twilight, a moon with phases that rises and sets, and a turning star field with the
Milky Way. Thick cloud dims the sun. With the sun low behind you and a shower ahead, a
rainbow stands on the rain: the primary bow, the fainter reversed secondary and the dark band
between them. A world without a directional light gets a sun and moon
spawned; a game with its own untagged light keeps control (tag it `SunLight` to hand it over).

## Showcase

`demo::spawn_demo(commands, origin)` builds a weather station of about 26 × 16 m from
generated meshes and textures (nothing loaded from disk):

- an asphalt bus yard with wheel ruts, sealed cracks, road markings, a drain and a manhole, where
  rain pools into puddles; a sand pit that soaks it all up and a stone patio between them
- a glazed bus shelter whose ground stays dry, with lights that come on at night
- a lattice wind mast with a cup anemometer and a wind vane, and a windsock whose bands lift
  one after another as the wind picks up
- a fenced enclosure with a Stevenson screen, a rain gauge that fills and a snow stake
- a big thermometer with a display cycling temperature, wind and rain rate
- street lamps that switch on at dusk and in dark storms, and a chrome ball that reflects the sky

In the showcase valley it stands in the `weather` zone, with tour stops for the station, rain,
snow and a storm.

`cargo run -p engine_weather --release --example weather_tour [out_dir] [shots]` renders the
station through a day of changing weather (clear morning, fair-weather clouds, a storm rolling
in, a downpour, puddles after the rain, night rain under lamps, lightning, a foggy morning,
sunset, above the clouds, snow, a moonlit night, a rainbow in the showers), then close-ups of
the station, and saves a screenshot of each. Around the station the tour builds its own
landscape: a meadow with grass, wooded hills, rocks, a farm track and a mountain range. Puddles
only form within a few centimeters of the ground, so grass blades and tabletops stay dry.

## Tuning

| Resource | What it controls |
|---|---|
| `Weather` | Current preset, transition time, auto weather, forecast, time-lapse, prevailing wind, seed |
| `Climate` | Latitude, axial tilt, year length, mean temperature and daily/seasonal swing |
| `Sky` | Hour, day of year, clock speed, moon phase and brightness, night fill |
| `GroundWeather` | Soak and evaporation rates, puddle share and depth, snow melt |
| `Lightning` | Strike rate, flash brightness, fires from strikes |
| `CloudSettings` | March resolution and steps, extinction, draw distance, noise scales, rain shafts, star brightness, moon size |
| `PrecipitationSettings` | Drop/flake/splash counts, area, drop width, motion blur, bolts |
| `GroundEffectSettings` | Puddle reflections (steps), ripples |

Components: `SunLight`, `MoonLight`, `WeatherShelter` (cover without a collider),
`GroundPatch` (ground the terrain doesn't describe: how much it absorbs, where it pools).

## API

| Function | |
|---|---|
| `weather.get` | Weather at the player: preset, temperature, humidity, wind, rain, cover, fog, ground |
| `weather.set` | Change the weather: preset, transition time, auto, prevailing wind |
| `weather.forecast` | Queue weather changes |
| `weather.time` | Set the clock: hour, day of year, clock speed, moon phase |
| `weather.advance` | Run the weather forward (rain falls, ground soaks, snow piles up) |
| `weather.ground` | Set the ground directly: dry, wetness, puddle depth, snow depth |
| `weather.lightning` | Strike at a position |
| `weather.presets` | List presets and what they mean |

## Contracts

Reads: `Time`, `FocusPoints` (where the player is), `Ground` (height), `SurfaceMaterials`,
`HeatField`, physics raycasts (roofs).

Writes: `WeatherState` (temperature, humidity, pressure, wind, cloud cover and base,
precipitation, fog, time of day, sun direction), `SurfaceLayers` (wetness, puddles, snow),
`DirectionalLight` on `SunLight`/`MoonLight` entities, `AmbientLight`; messages `NoiseEvent`,
`WorldImpulse`, `IgniteRequest`, `LightningStrike`.

Follows `WorldShift`: fronts, the cloud pattern, the sky's compass (sun, moon, stars) and
puddle spots live in the weather's own frame (`WeatherSim::frame`), which follows every
floating-origin move or turned frame, so the weather stays over the same ground.

Renders through three `RenderFeature`s: `weather clouds` (march, sky and geometry composites,
environment map), `weather precipitation` (rain, snow, splashes, bolts) and `weather ground`
(puddles, wet ground and snow cover).

## Known gaps

- The renderer's sky still draws its own flat cloud layer when there are clouds; the clouds
  feature paints over it (`CloudSettings::replace_sky`). A renderer toggle is requested.
- Wetness and snow in the mesh shader use the value at the camera; the ground pass adds the
  local difference. Wet grass looks too glossy (renderer material response).
- Clouds don't yet cast shadows on the ground.
