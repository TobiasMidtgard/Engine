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
- **The nearby stars and their planets.** `system::nearby_stars()` places the stars nearest
  the Sun at their true distances and directions, with Alpha Centauri B, Sirius B and Procyon B
  on their measured orbits. `nearby::planets(false)` gives them planets: the real ones
  surveys found (Proxima b and d, Barnard's Star b-e, Epsilon Eridani b; candidates only with
  `true`, refuted ones never), then seeded, physically plausible ones from each star's name
  (a compact inner chain spaced in mutual Hill radii, a habitable-zone planet for about half
  the stars, a giant or ice world past the snow line, all inside binary stability limits and
  below what radial-velocity surveys rule out). Rocky ones carry a `SurfaceRecipe` (class:
  airless, lava, desert, terra, ocean, cold desert, ice, hazy; sea share, craters, pressure,
  surface temperature with greenhouse, the star's temperature) that a game turns into terrain
  when a ship comes close. Sources in the module docs and the research write-up.
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
- **Landing anywhere.** Low over the turning anchor (below `SpaceSettings::rehome_altitude_m`,
  30 km) and more than `rehome_distance_m` (40 km) along the ground from the home spot, the
  home spot moves under the focus, like KSP's floating origin: the world origin stays near
  the player, +Y stays up and f32 physics keeps its precision wherever you land. Coming back
  within `home_return_m` (30 km) of the `HomeFrame`'s spot puts the world back in that frame
  exactly. `SpaceState::to_home` / `from_home` map points between the world and home
  coordinates, `at_home` says whether they are the same, and
  `travel_home_to(world, at)` jumps home with the focus at a place laid out there.
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
- **Ships coast through time warp.** Physics runs in real time, so while the clock runs faster
  (`SpaceSettings::rails_above_warp`, 1.5×) every free physics body out in space (above the
  top of a body's air, or 0.8 % of its radius over an airless one) flies its exact Kepler path
  instead: `OnRails` holds the body it falls around and its `Conic` (ellipse, parabola or
  escape hyperbola, `conic.rs`). Paths are patched where they cross a sphere of influence, so a
  ship sent toward the Moon swings past it or falls into orbit around it, and one past escape
  speed ends up around the Sun. Coasting ships hold their attitude until the game turns them, and
  their engines still push: a railed ship's `ExternalForce` over its mass bends its path in
  steps of up to 2 s of space time (64 a frame), so burns can be flown at any warp. When the warp drops, physics
  takes each ship back with its position and motion; a ship that coasts down into air or low
  space drops the warp to real time there, so physics flies the reentry or landing. Things on
  the ground, things left on other bodies (`Parked`) and anything marked `NoRails` stay with
  physics. `space.rails` lists coasting ships (body, altitude, speed, lowest and highest
  points, period). Orient a chase camera after `rails::fly` (`PostUpdate`), or it trails a
  frame behind a ship moving thousands of kilometers per frame.
- **Gravity, air and light.** Writes `Gravity::bodies` (the anchor and any body whose reach
  holds the focus; never what the anchor orbits, which it falls around with us, so on the
  Moon only the Moon pulls), sets the renderer's `Atmosphere` to the nearest body with air (off when
  none), and points every `DirectionalLight` marked `StarLight` at its star with the right
  illuminance for the distance (≈127 000 lux at 1 AU). Each body's air carries its own
  scattering (`BodyAtmosphere`, with the renderer's presets `earth()` and `mars()`): Earth's
  sky is blue with red sunsets, and on Mars the sky is butterscotch by day with a blue glow
  around the setting sun, its dust veils far cliffs and fills shadows with orange light, and
  its limb wears that dust seen from orbit.
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
`StarLight` lights and, with one, `WeatherState`'s sun direction, time and day, the
`Transform`/`Velocity` of free entities when the world re-anchors, and those of free
`RigidBody`s out in space while they coast through time warp (`OnRails`, reading their
`ExternalForce` and `MassProperties` to burn), and
`engine_world::FarCloudLayer` while Earth's cloud tops are drawn from high up (handover from
70 % of `cloud_hole_m` to all of it, weighted by the layer's fade; removed when none is drawn),
so the weather's clouds give way to the layer instead of doubling it.

API: `space.bodies`, `space.warp {warp, days}`, `space.rails`, `space.travel {body, lat, lon,
altitude}`.

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
thin haze rims (Venus, and Earth or Mars while the renderer's atmosphere is on another body;
the body it is on is seen through it), and Saturn's rings with
their real radial profile, lit and unlit sides and the planet's shadow. Each look
(`BodyLook`) is painted on the CPU into 2048×1024 maps (rings 4096 texels) with mips: a
small map first, the full one on a background thread once the body is bigger than ~40 px.
Gas giants are painted through their winds (`look/flow.rs`): bands and cloud texture are
sampled where the flow brought them from, so jets shear them into streaks and vortices wind
them up. `cargo run --release -p engine_space --example space_shots -- --maps` saves every
map and prints bake times.

Planets of other stars are painted from their `SurfaceRecipe`: barren worlds (`look/world.rs`:
cratered rock, lava worlds whose melt glows by its own light, Mars-like deserts with dark
provinces, dunes and frost caps, ice shells crossed by lineae) and living ones with the Earth
painter turned by a `Climate` (their share of sea, warmer or colder, plants colored by
their star's light, and the "eyeball" of a world locked to its star). Views `proxima_b`,
`acen_b_frozen`, `eps_eri_ocean`, `acen_a_lava_night`, `proxima_ice` and others in
`space_shots`. Planets farther than 2000 AU aren't enlarged into dots, nor baked until they
cover a twentieth of a pixel.

From high up (above 9 km, fading in by 25 km, out to 40 % of the radius), Earth's cloud tops
are drawn as their own layer 5 km above the ground, over whatever terrain covers the body:
the baked cloud map broken up with procedural detail down to a few hundred meters, so far
away it shows the weather systems and up close it breaks into crisp cumulus covering the
same share of sky, with a thin veil under broken cloud. The tops are lit through the air and
veiled by it, and cast their shadows on the ground between them, offset away from the sun
(long at a low sun), so the terrain under the layer is shaded like the far globe. Within
`SpaceSettings::cloud_hole_m` (60 km) of the camera the layer leaves
the sky to the weather's own volumetric clouds, and near that hole the layer takes the
weather's `cloud_cover` (back to its own map by three hole radii), so the two decks meet at the
same share of sky. Views `earth_leo` (400 km) and `earth_edge`
(35 km) in `space_shots` show it, and `mars_low` and `mars_edge` the same heights over Mars.

## Known limits (next rounds)

- Planets of other stars: the far map's continents and the terrain's are drawn separately
  (as for Earth); a locked world's terrain climate follows latitude, not the angle from its
  star; their weather is the home weather.

- While the world is anchored elsewhere, what was left on a body is not simulated with it:
  it keeps the place the shift mapped it to (far out of view) until it is put back.
- The far Earth's continents are its own; they don't match what `engine_terrain` builds up
  close. A shared continent field (or terrain baking the far map) needs a contract.
- Full-size bakes cost 0.4-1.5 s of CPU per body; a few looks (Mars albedo regions, Moon
  maria outlines, Neptune's cloud streaks) still read as procedural up close.
- The cloud layer's pattern from high up is the far look's own, not the weather's (they meet
  at the edge of `cloud_hole_m`).
- Coasting ships feel one body at a time (patched conics): no Lagrange points or slow
  three-body drifts, and a sphere of influence grazed by less than ~1 % of its radius in one
  sampled step can be missed at extreme warps. Burns on rails are stepped, so at extreme warps
  a long burn is less exact than physics flying it in real time (planned maneuver nodes are a
  later round).
- Physics below the rails' floor still runs in real time at any warp, so an aircraft flying
  while the clock races sees the sky and the other planets move fast.
