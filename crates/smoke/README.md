# engine_smoke — Smoke and gas

**Owner:** Fire and smoke. **Status:** first full version: simulated smoke volumes that rise,
drift with the wind and react to bullets, explosions, fans and moving things, drawn as lit,
shadowed volumes.

## What it does

- **Smoke lives in boxes of simulated air** (`SmokeVolume`): a grid of voxels (0.25–2 m)
  holding air velocity, smoke density (kg/m³), temperature and color. Place a volume over a
  battlefield, a burning meadow or a room. Smoke released anywhere else gets a volume of its
  own (`SmokeSettings::auto_size_m`), which goes away once its smoke has gone.
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
  down toward the ground, and how much firelight and lamplight (point lights) reaches it. A
  raymarch through the box then lights every step with the sun (forward scattering, a few
  orders of multiple scattering so thick white smoke looks white from every side, and the
  scene's shadow map), sky light from above and ground light from below, and firelight. Fine
  billows are added with noise (fading out with distance), the march stops at the scene's
  depth, and the result is fogged like the rest of the scene. Motion between simulation steps
  is blended so it stays smooth at any frame rate.
- **Visibility for gameplay:** `SmokeDomains::transmittance(a, b, …)` and the
  `smoke.visibility` API say how much of a line of sight gets through the smoke, for AI that
  can't see through a smoke screen.

## Contracts

Reads: `SmokeEmission`, `ProjectileTrace`, `WorldImpulse`, `WindField`, `WeatherState` (air
temperature), `Ground` (the floor of each volume), `FocusPoints` (volumes far from every
focus, or the camera, pause).

Writes: `WindField` (fans).

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

// Can the guard see the player?
let seen = domains.transmittance(eye, target, settings.extinction_m2_kg) > 0.3;
```

AI bridge: `smoke.emit`, `smoke.clear`, `smoke.status`, `smoke.sample`, `smoke.visibility`,
`smoke.demo` (where the demo is in its loop; restart it);
tune `SmokeSettings` (buoyancy, wind coupling, swirl, mixing, thickness, bullet tunnels,
detail) and the demo live.

## Demo

`demo::spawn_demo(commands, origin)` builds a 38 × 44 m smoke range. Every 50 s: smoke
grenades pop (a white screen in the middle, red, violet and green signal smoke by the
targets), a turret fires bursts through the white screen at the targets (each round leaves a
see-through tunnel and a tracer), a blast in the cloud blows a hole in it and leaves a dark,
hot puff that rises away, then three fans along the side push what's left across the range.
A cart drives back and forth through the smoke on its rails and a steam vent sends up a hot
plume that bends with the wind. The first round starts 9 s in, so the smoke is there right
away. `demo::VIEWS` lists camera spots (bullet holes from behind the turret, the whole range,
signal smoke, the fans), each with how long after a restart (`smoke.demo` with
`{"restart": true}`) its moment comes, so a screenshot tour catches it.

## Performance

Volumes step at `sim_hz` (20 Hz) of world time, in parallel, and only inside the box around
their smoke (plus a margin). The demo range (80 × 28 × 88 voxels) takes a few milliseconds a
step on four cores while it's full of smoke. Volumes far from every focus point pause; empty
ones cost nothing. Drawing marches at most `max_steps` per pixel, stops early in thick smoke,
and relights a volume only when it changed.

## Not yet

- Smoke crossing from one volume into a neighbor (each volume is on its own).
- Volumetric fog banks (ground fog that rolls in valleys) on the same simulation.
- An `Obscurance` provider once that contract lands in `engine_world`, so AI asks the world
  rather than this module.
- Heat haze and smoke lit from below by the flames' own shape (firelight is point lights).
- Half-resolution raymarching for very large volumes on slow GPUs.
