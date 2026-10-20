# engine_smoke — Smoke and gas

**Owner:** Fire and smoke. **Status:** first full version: simulated smoke volumes that rise,
drift with the wind and react to bullets, explosions, fans and moving things, drawn as lit,
shadowed volumes.

## What it does

- **Smoke lives in boxes of simulated air** (`SmokeVolume`): a grid of voxels (0.25–2 m)
  holding air velocity, smoke density (kg/m³), temperature and color. Place a volume over a
  battlefield, a burning meadow or a room. Smoke released anywhere else gets a volume of its
  own (`SmokeSettings::auto_size_m`), which goes away once its smoke has gone. Where volumes
  overlap, smoke goes to the finest one, so a small fine box around a campfire (0.3 m) inside a
  meadow's coarse one keeps its thin wisp; volumes finer than 0.5 m also draw smaller billows
  and erode thin smoke less.
- **It moves like smoke:** hot smoke rises (buoyancy from its temperature) and cools as it
  mixes; dense, cold smoke sinks and spreads along the ground. The air takes up the wind
  (`WindField`, sampled every few voxels), swirls (vorticity confinement), stays
  incompressible (a pressure solve: it flows around rather than piling up) and can't go into
  the ground. Smoke mixes outward a little (`diffusion_m2_s`), thins over minutes
  (`dissipation_secs`) and leaves through the open sides and top. Only the air around the
  smoke is solved, so a big, mostly empty volume costs little.
- **Sources:** `SmokeEmission` messages (fire sends them for burning ground and objects), the
  `smoke.emit` API, and `SmokeSource` components: a smoke grenade, a chimney, a steam vent,
  a signal flare. A source's `gas_m3_s` pushes the air outward, so a grenade's cloud billows
  out to several meters in seconds; colors mix where clouds meet.
- **It reacts:** a bullet (`ProjectileTrace`) clears a tunnel along its path and drags air
  with it; you can see through the hole for a moment (`tunnel_secs`) before the smoke flows
  back in. An explosion (`WorldImpulse`) blows a hole in the cloud. A `SmokeFan` (a fan, a
  vent, a rotor's downwash) blows smoke along its forward direction and stirs the shared
  `WindField` too. A `SmokeObstacle` (a car, a character) pushes smoke aside as it moves.
- **Drawing:** each volume's density goes to a 3D texture. Once per simulation step a compute
  pass works out, for every voxel, how much smoke lies toward the sun, up toward the sky and
  down toward the ground, and how much firelight and lamplight (point lights) reaches it.
  Smoke still hot from a fire (its heat goes up as a second texture) also gets the light of
  the flames under it, so a plume's underside glows orange at night. A
  raymarch through the box then lights every step with the sun (forward scattering, a few
  orders of multiple scattering so thick white smoke looks white from every side, and the
  scene's shadow map, softened by about a cell and taken only partly from the light that
  bounced around inside the smoke, so a tree's shadow falls through a plume as a soft shaft
  rather than a sharp dark copy of its crown), sky light from above and ground light from below, and firelight. Fine
  billows are added with noise (fading out with distance), the march stops at the scene's
  depth, and the result is fogged like the rest of the scene. Motion between simulation steps
  is blended so it stays smooth at any frame rate.
- **Eddies in the wind:** the wind smoke feels has gusts that swing its direction too
  (`SmokeSettings::turbulence`, a share of the wind's speed; 0.3 by default, half that up and
  down), about 14 m across, drifting with the wind and slowly changing. A fire's plume
  meanders, puffs and widens as it drifts instead of streaming off as a straight tube.
- **Fog banks:** a `FogBank` on a `SmokeVolume` fills that volume's low ground with fog up to
  a level (`depth_m` above the lowest ground, with a soft top): valleys and hollows fill,
  hills stand out of it. The fog is chilled, white air in the same simulation, so it sinks,
  drains off slopes and pools, the wind carries it, bullets, blasts, fans and vehicles cut
  through it, and it forms again behind them (`form_secs`). It follows the weather's fog
  (`WeatherState::fog`) unless given a fixed `amount`, and burns off when there should be
  less of it (`burn_secs`). `visibility_m` sets how far you see inside it. It's drawn as a
  wide, soft bank rather than billowing smoke, so the low sun lights its top and trees cast
  shafts across it.
- **Visibility for gameplay:** after every step a copy of the smoke goes to the engine's
  `Obscurance` contract (as "smoke"), so NPC AI asks `obscurance.transmittance(eye, target)`
  without depending on this module, and fog or dust from other modules stack with it. The
  `smoke.visibility` API answers the same question.

## Contracts

Reads: `SmokeEmission`, `ProjectileTrace`, `WorldImpulse`, `WindField`, `WeatherState` (air
temperature; fog for fog banks), `Ground` (the floor of each volume), `FocusPoints` (volumes far from every
focus, or the camera, pause).

Writes: `WindField` (fans), `Obscurance` (the "smoke" source).

Follows `WorldShift`: in `WorldShiftSet::Apply` every volume moves its origin (exact at any
distance; a shift that turns the world resamples its air into a box of the same size around
where it went, velocities turned, and measures its floor again at the next step), along with
blasts and gas still expanding, bullet tunnels, queued `smoke.emit` puffs, obstacles' last
positions and the demo cart's track. A `SmokeVolume` keeps its smoke as its entity moves with
the world. `Obscurance` gets the moved smoke in `WorldShiftSet::Rebuild`.

## Using it

```rust,ignore
// Air over a battlefield, 0.5 m voxels.
commands.spawn((Transform::from_xyz(0.0, 0.0, 0.0),
    SmokeVolume { size: [40.0, 14.0, 40.0], cell_m: 0.5 }));

// A red smoke grenade that runs for 25 s.
commands.spawn((transform, SmokeSource::grenade([0.85, 0.12, 0.08], 25.0)));

// A chimney: thin, hot, forever.
commands.spawn((transform, SmokeSource { rate_kg_s: 0.01, temperature_c: 180.0, ..default() }));

// A fan blowing along the entity's forward direction, and a car that stirs the smoke.
commands.spawn((transform, SmokeFan { speed: 9.0, radius: 2.0, reach: 20.0, on: true }));
commands.spawn((car_transform, SmokeObstacle { radius: 2.0 }));

// Ground fog in a valley: follows the weather, 40 m visibility inside, 15 m deep.
commands.spawn((valley_transform, SmokeVolume { size: [200.0, 30.0, 400.0], cell_m: 3.0 },
    FogBank { visibility_m: 40.0, depth_m: 15.0, ..default() }));

// Can the guard see the player? (Any module can ask; this one answers.)
let seen = env.obscurance.transmittance(eye, target) > 0.3; // env: engine_world::Env
```

AI bridge: `smoke.emit`, `smoke.clear`, `smoke.status`, `smoke.sample`, `smoke.visibility`,
`smoke.demo` (where the demo is in its loop; restart it);
tune `SmokeSettings` (buoyancy, wind coupling, swirl, mixing, thickness, bullet tunnels,
detail) and the demo live.

## Demo

`demo::spawn_demo(commands, origin)` builds a smoke range (about 45 × 52 m): a 38 × 44 m
concrete pad, a firing line of Jersey barriers and sandbags with a belt-fed machine gun,
steel targets and gongs in front of an earth berm, three drum fans whose rotors spin up, a
skip wagon on rails, a steam vent, and clutter (cables, range flags, ammo crates, spent
grenades, dry grass). Every prop is a procedural mesh with baked textures (`demo/`),
registered once by key; `examples/smoke_gallery.rs` renders the views and close-ups. Every
50 s: smoke
grenades pop (a white screen in the middle, red, violet and green signal smoke by the
targets), a turret fires bursts through the white screen at the targets (each round leaves a
see-through tunnel and a tracer), a blast in the cloud blows a hole in it and leaves a dark,
hot puff that rises away, then three fans along the side push what's left across the range.
A cart drives back and forth through the smoke on its rails and a steam vent sends up a hot
plume that bends with the wind. The first round starts 9 s in, so the smoke is there right
away. `demo::VIEWS` lists camera spots (bullet holes from behind the turret, the whole range,
signal smoke, the fans), each with how long after a restart (`smoke.demo` with
`{"restart": true}`) its moment comes, so a screenshot tour catches it.

`examples/smoke_fog.rs` is the fog test scene: a wooded valley at dawn filled with a fog
bank up to about 14 m, seen from a ridge, down the valley, from inside the fog and from just
above its top.

## Performance

Volumes step at `sim_hz` (20 Hz) of world time, in parallel, and only inside the box around
their smoke (plus a margin). The demo range (80 × 28 × 88 voxels) takes a few milliseconds a
step on four cores while it's full of smoke. Volumes far from every focus point pause; empty
ones cost nothing. Drawing marches at most `max_steps` per pixel, stops early in thick smoke,
and relights a volume only when it changed.

## Not yet

- Smoke crossing from one volume into a neighbor (each volume is on its own).
- Smoke lit by the exact shape of the flames (hot smoke gets the flames' light from its
  heat, other smoke from point lights). Heat haze is drawn by the fire module.
- Half-resolution raymarching for very large volumes on slow GPUs.
