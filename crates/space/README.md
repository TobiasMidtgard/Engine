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
- **Time.** `SpaceClock` counts seconds since the epoch with a time warp (`space.warp`).
- **The world frame.** `SpaceFrame` names the anchor body and the home spot (latitude,
  longitude) that sits at the world origin with +Y up, north toward −Z and east toward +X,
  matching `PlanetTerrain::home_transform`. The anchor's transform never changes, so walking,
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
- **Travel.** `travel_to(world, body, lat, lon, altitude)` / `space.travel` makes a body the
  anchor and puts the focus at a spot on it: the quick trip for tours, tools and fast travel.
- **Gravity, air and light.** Writes `Gravity::bodies` (the anchor and any body whose reach
  holds the focus), sets the renderer's `Atmosphere` to the nearest body with air (off when
  none), and points every `DirectionalLight` marked `StarLight` at its star with the right
  illuminance for the distance (≈127 000 lux at 1 AU).

## Contracts

Reads: `SimulationFocus` / `FocusPoints`, `Time` (fixed steps, dilated).

Writes: body `Transform`s, `Gravity` (bodies, uniform = 0), `engine_render::Atmosphere`,
`StarLight` lights, and the `Transform`/`Velocity` of free entities when the world
re-anchors.

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

## Known limits (next rounds)

- Other modules keep some world-space data outside `Transform` (field grids, particles). A
  re-anchor moves entities but not that data yet; a core `WorldShift` message is requested.
- Bodies without a surface (the Sun's disk aside) are not drawn yet; gas giants and far
  planets need an impostor renderer.
- Ships are not integrated on patched conics yet (`Orbit::from_state` is the first piece).
