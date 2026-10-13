# engine_space — Space and planets

**Owner:** Planets and terrain. **Status:** working (round 1).
Background research: `game-engine-research/physics.md` in the project files.

Real star systems at real scale, in one continuous world. Bodies follow Kepler orbits on
rails (exact at any time and any time warp), spin about tilted axes, and the body you are
near anchors the world so that its surface is still and precise. Fly far enough and the
world re-anchors on the next body.

## What it does

- **Bodies as data.** `CelestialBody` (mass, radius, orbit around its parent, spin, tilt,
  tidal locking, atmosphere, luminosity). `system::solar_system()` is the Sun, the eight
  planets and the Moon with J2000 elements and IAU radii; `demo::spawn_solar_system` spawns it.
- **Orbits on rails.** `Orbit` solves Kepler's equation for position and velocity at any time
  (no drift), converts a position and velocity back to elements (for predicted ship orbits),
  and gives sphere-of-influence radii.
- **Time.** `SpaceClock` counts seconds since the epoch (2000-01-01 12:00 UT, noon at 0° E)
  with a time warp (`space.warp`). Earth's seasons fall on their real dates.
- **The world frame.** `SpaceFrame` names the anchor body and the home spot (latitude,
  longitude) that sits at the world origin with +Y up, north toward −Z and east toward +X
  (`home_heading_deg` turns that), matching `PlanetTerrain::home_transform`. The anchor's transform never changes, so walking,
  driving and physics on it are as precise as on a flat level. Other bodies and the sun move
  around it: the sun rises and sets as the anchor turns.
- **Turning and fixed frames.** Near a body (below 15 % of its radius above the top of its
  air) the world turns with it, so its surface stands still. Higher up the world stops
  turning and the body spins in it, so orbits and flights between planets see their true
  speeds rather than the huge apparent speeds of a turning frame.
- **Re-anchoring.** When the focus enters another body's sphere of influence (or leaves the
  anchor's), the anchor switches to the body that holds it, with the home spot right under
  the focus. Every free entity keeps its true position, orientation and velocity across the
  switch (velocities include the change of frame motion and spin).
- **What you leave stays put.** When the world stops turning with a body (climbing out, or a
  trip elsewhere), everything on or near it (below half that height: the valley, its
  houses, props, parked cars and NPCs) is noted in `Parked` with its pose and motion on the
  body, and put back there when the world turns with that body again, wherever you land.
  Otherwise those things would keep the place the new frame mapped them to while the planet
  turned away beneath them. `SpaceSettings::keep_surfaces` turns it off.
- **Travel.** `travel_to(world, body, lat, lon, altitude)` / `space.travel` makes a body the
  anchor and puts the focus at a spot on it: the quick trip for tours, tools and fast travel.
  `travel_home(world, altitude)` / `space.travel {"home": true}` returns to the frame the
  world started in, so a level built around the home spot is exactly where it was.
- **Gravity, air and light.** Writes `Gravity::bodies` (the anchor and any body whose reach
  holds the focus), sets the renderer's `Atmosphere` to the nearest body with air (off when
  none), and points every `DirectionalLight` marked `StarLight` at its star with the right
  illuminance for the distance (≈127 000 lux at 1 AU).
- **One sun with the weather.** Either module can run the day:
  - With a `StarLight`, space is the day/night cycle: it writes engine_world's
    `WeatherState::sun_direction`, `time_of_day_h` (local solar time at the world origin) and
    `day_of_year` (the weather's calendar), so clouds, sky, wildlife and sound follow the real
    sun.
  - With the weather's sun (its `SunLight`) and `SpaceSettings::follow_world_clock`, the space
    clock follows the weather's hour and day instead: the real sun stands where the weather's
    does over the home spot, so the Moon's phase and the planets are lit from that sun and
    the weather's time keys move them too. The weather's compass is north +Z, east −X, half a
    turn from the planet's: set `SpaceFrame::home_heading_deg` and the terrain pad's
    `heading_deg` to 180 so they agree.

## Contracts

Reads: `SimulationFocus` / `FocusPoints`, `Time` (fixed steps, dilated), and with
`follow_world_clock` the `WeatherState` time and day.

Writes: body `Transform`s, `Gravity` (bodies, uniform = 0), `engine_render::Atmosphere`,
`StarLight` lights and, with one, `WeatherState`'s sun direction, time and day, and the
`Transform`/`Velocity` of free entities when the world re-anchors.

API: `space.bodies`, `space.warp {warp, days}`, `space.travel {body, lat, lon, altitude}`.

## Use it

```rust,ignore
let bodies = engine_space::demo::spawn_solar_system(&mut commands);
let pad = FlatPad::default(); // engine_terrain, 40° N 0° E, matches SpaceFrame::default()
commands.entity(bodies["Earth"]).insert(
    PlanetTerrain::earthlike(7).with_radius(6_371_000.0).with_pad(pad));
commands.entity(bodies["Moon"]).insert(PlanetTerrain::moon(3).with_radius(1_737_400.0));
commands.entity(bodies["Mars"]).insert(PlanetTerrain::mars(5).with_radius(3_389_500.0));
commands.spawn((StarLight::default(), DirectionalLight::default(), Transform::default()));
```

## Far views

Every planet and moon is drawn from afar by the `space bodies` render feature: a ray-traced
spheroid (real polar flattening) with anti-aliased edges, limb darkening, relief at the
terminator, Earth's clouds as a separate layer with shadows and sun glint on its oceans,
thin haze rims (Venus, Mars; Earth uses the renderer's atmosphere), and Saturn's rings with
their real radial profile, lit and unlit sides and the planet's shadow. Each look
(`BodyLook`) is painted on the CPU into 2048×1024 maps (rings 4096 texels) with mips: a
small map first, the full one on a background thread once the body is bigger than ~40 px.
Gas giants are painted through their winds (`look/flow.rs`): bands and cloud texture are
sampled where the flow brought them from, so jets shear them into streaks and vortices wind
them up. `cargo run --release -p engine_space --example space_shots -- --maps` saves every
map and prints bake times.

## Known limits (next rounds)

- While the world is anchored elsewhere, what was left on a body is not simulated with it:
  it keeps the place the shift mapped it to (far out of view) until it is put back.
- The far Earth's continents are its own; they don't match what `engine_terrain` builds up
  close. A shared continent field (or terrain baking the far map) needs a contract.
- Full-size bakes cost 0.4-1.5 s of CPU per body; a few looks (Mars albedo regions, Moon
  maria outlines, Neptune's cloud streaks) still read as procedural up close.
- Ships are not integrated on patched conics yet (`Orbit::from_state` is the first piece).
