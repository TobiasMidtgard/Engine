# engine_fire — Fire

**Owner:** Fire and smoke. **Status:** first full version: spreading ground fires, burning
objects, embers and spot fires, flames, scorch, light, heat, smoke emission and damage.

## What it does

- **Ground fires** spread through fuel on a sparse grid of 0.5 m cells that exists only where
  something is burning (32 × 32-cell tiles, created as the fire reaches them). Each cell holds
  fuel (kind, load in kg/m², moisture), absorbed heat, flame intensity, coals, scorch and the
  ground height. Every step, burning cells heat the fuel around them through a kernel that
  leans with the local wind (`WindField`), so fires run downwind, creep on their flanks and
  barely back into the wind. A cell ignites once it has absorbed the energy to heat its fuel
  and boil off its water. Calibrated against CSIRO grassland spread: about 0.03 m/s in still
  air, 0.6 m/s at 3 m/s wind, 2 m/s at 10 m/s (`field::tests::calibrate`).
- **Fuel** comes from the ground material (`SurfaceMaterials`: grass, leaves and wood burn;
  rock, sand, water don't) or from `FuelPatch` components that override it in an area: a dry
  meadow, a forest floor of litter, brush, or a firebreak (`FuelKind::None`).
- **Moisture** matters the way it does in reality: wet fuel needs more heat to ignite and
  burns weaker (Rothermel damping), and above the fuel's extinction moisture it won't carry a
  fire at all. Rain soaks fuel and puts fires out; dry weather dries it toward the humidity's
  equilibrium; puddles, snow and surface wetness (`SurfaceLayers`) count too.
- **Burnout and regrowth:** fuel burns down to glowing coals, then black scorch. With
  `regrow_secs` the char fades and the fuel grows back.
- **Objects** with a `Combustible` (trees, walls, crates, hay) heat up from the ground fire,
  from radiant heat of other fires and from flames touching them, ignite at their ignition
  temperature, burn through their fuel and smolder out. While burning they heat the ground
  (a burning tree lights the grass under it), throw embers, smoke and light. Their flames fill
  a `FlameShape` (a trunk, a crown). `Charring` darkens and shrinks simple props as they burn.
  `ContainedFire` is a campfire, brazier or torch that never spreads; `perpetual` keeps it lit.
- **Embers** are lofted by the plume, carried by the wind and can start spot fires where they
  land, which is how fires jump roads.
- **Rendering:** flames are camera-facing sprites, leaning with the wind, that play a flame
  flipbook: a small 2D fire simulation (`flipbook.rs`: fuel, combustion, buoyancy, vortex
  confinement, MacCormack advection) run once at startup on a worker thread and recorded into
  a looping 64 × 128 × 96 texture, so flames neck in at the base, lick and shed tongues the
  way real ones do. Each sprite plays it from its own point, at its own pace (taller flames
  flicker slower, as √height), some mirrored. Color is blackbody (deep red tips to a yellow
  core) with soot at the tips; brightness follows the camera's exposure within limits, so
  flames show by day and stay orange at night instead of burning out to white. Heat haze bends
  the scene above them. Embers are motion-blurred streaks; a screen-space decal chars the
  ground (and the grass on it), with ragged edges, glowing coal cracks and the burning litter
  under the flames. Point lights flicker with the fire.

## Contracts

Reads: `IgniteRequest`, hot `WorldImpulse` (explosions), `WindField`, `WeatherState` (rain,
humidity, temperature), `SurfaceLayers` (wetness, puddles, snow), `Ground`, `Water`,
`SurfaceMaterials`, `Combustible`, `FocusPoints`.

Writes: `HeatField` (°C, decays when nothing keeps it hot), `SurfaceLayers::scorch` (so
foliage can show burnt grass), `SmokeEmission` (per 2 × 2 m block of burning ground and per
burning object, with mass, temperature and darkness: white steam from wet fuel, dark smoke
from wood), `DamageEvent` (kind `Fire`) for anything with `FireVulnerable` or a
`CharacterController` standing in flames, `WindField` updrafts over big fires,
`Combustible::state` (owners react: foliage swaps a burnt tree, destruction drops a beam).

Follows `WorldShift`: in `WorldShiftSet::Apply` the fire field moves its origin and ground
heights (exact at any distance; a shift that turns the world resamples it into the new frame,
nearest cell, and the cells it newly covers get their fuel at the next step), and embers,
pending spot-fire landings, placed `FuelPatch`es, light targets and queued `fire.*` commands
move with the world. Patches a shift moved are not refueled.

## Using it

```rust,ignore
// Light a fire (or send an IgniteRequest, or an explosion with heat).
world.write_message(IgniteRequest { position, radius: 1.5, temperature_c: 900.0, source: None });

// A dry meadow that regrows two minutes after it burns, with a gravel road through it.
commands.spawn((Transform::from_xyz(0.0, 0.0, 0.0),
    FuelPatch::grass(PatchShape::Rect { half_extents: [30.0, 20.0] }).with_regrow(120.0)));
commands.spawn((Transform::from_xyz(10.0, 0.0, 0.0),
    FuelPatch::firebreak(PatchShape::Rect { half_extents: [1.5, 20.0] }).with_priority(10)));

// A tree whose crown can burn.
commands.spawn((transform, Combustible { fuel_kg: 40.0, moisture: 0.3, ..default() },
    FlameShape { radius: 2.3, height: 5.0, offset: [0.0, -2.0, 0.0] },
    Charring { shrink: 0.55, ..default() }));
```

AI bridge: `fire.ignite`, `fire.extinguish`, `fire.clear`, `fire.status`, `fire.sample`;
tune `FireSettings` (spread, embers, damage, lights) and the demo live.

## Demo

`demo::spawn_demo(commands, origin)` builds a 64 × 44 m meadow of dry grass (the showcase's
grass field): a fire starts at its upwind edge and runs across it with the wind, stops at a
gravel road and a soaked patch, lights four trees (trunk first, then the crown), a wooden
shed and hay bales, while the bales across the road survive. A campfire burns forever by the
path. Once the meadow has regrown, the next fire starts. The first fire is already 50 s along
when the demo starts so there's something to see right away. `demo::VIEWS` lists camera spots
for it (fire front, burning trees, campfire, the whole meadow at night).

`examples/flame_look.rs` is a bench for the flames alone (a grass fire, a campfire, a burning
crate, by day or night); `flame_look sheet` writes the flipbook as a contact sheet.

## Performance

Only tiles with fire, coals or heat are stepped, in parallel, at `sim_hz` (20 Hz) of world
time (slow motion slows the fire too). Tiles farther than `sim_radius_m` from every focus
point (or the camera) pause. A 60 × 40 m grass fire is about 10 tiles. Flame sprites merge
into 1 × 1, 2 × 2 or 4 × 4-cell blocks with distance.

## Not yet

- Fire climbing slopes faster (the field knows ground height; the kernel doesn't use slope).
- Fuel from foliage instances (each grass blade or bush as fuel) once foliage exposes them.
- Building interiors: fire spreading room to room through openings.
