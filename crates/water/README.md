# engine_water — Water

**Owner:** Water simulation. **Status:** round 2: lakes, streams, interaction, puddles, the
test area and the showcase valley's lake and stream work end to end. Oceans and planet-scale
water come next.
Background research: `game-engine-research/graphics.md` in the project files.

## What it does

- **Lakes, pools and oceans** (`WaterBody`): wind waves from `WindField` (a fetch-limited
  spectrum of Gerstner waves, choppiness, swell), drawn on one camera-centered polar grid whose
  rings grow with distance, so a pond and a 30 km sea cost the same. Positions are re-anchored at
  the camera in f64, so waves stay smooth far from the world origin.
- **Streams, waterfalls and floods** (`FlowRegion`): shallow-water flow on a grid (virtual
  pipes): water runs downhill, pools in hollows and spills over lips. Sources and drains,
  basins filled at the start, open or walled edges. Lake bodies feed regions that overlap them (reservoirs).
  Water over a lip falls on its arc until it meets the rock: onto a ledge or into a pool it
  lands, down a face too steep to land on it clings and slides, white, and leaves it again
  where the face drops away (`FlowSim::fall_path`), so falls cascade down crevices instead of
  cutting through them. Regions step in parallel bands and skip dry ground.
- **Interaction**: a fluid field follows the camera (or the first `SimulationFocus`) over the
  nearest interactive body: shallow water on a staggered grid, so pushes make ripples that
  bend in the shallows and reflect off the shore, and bodies moving through it drag the water
  along. Their wash curls into eddies (vortex shedding behind hulls and swimmers, vorticity
  confinement keeps the swirls tight), a propeller churns, and foam rides the currents.
  `WaterSettings::current_life_s` and `swirl` tune it. It steps only the 16-cell tiles where
  something moves, in parallel: calm water costs nothing. Swimmers and waders (`WaterInteractor`, `GroundContact`), bullets
  (`ProjectileTrace`), blasts (`WorldImpulse`, domes from underwater charges), drops and wakes
  push it. Every splash is published as `WaterSplash` (particles, audio) and `NoiseEvent` (AI),
  including steady puffs of spray from fast boats' bows and from the foot of every waterfall
  the flow finds.
- **Floating** (`Buoyancy`): Archimedes over a probe grid with drag and wave forces, for props
  the module moves itself (`Floating`): buoys, moored boats, debris. Physics bodies
  (`RigidBody`) float in the physics module from their real collider shapes, through the same
  `Water` query, so `Buoyancy` leaves them alone. Blasts throw floating things; hitting the
  water splashes. `Tether` moors boats and buoys.
- **Rain**: weather rain and local `RainShower`s draw rings on water and fill puddles in hollows
  (`SurfaceLayers::puddle_m`), which soak in and evaporate by material, temperature and wind.
- **Wet shores**: water wets the ground it reaches (`SurfaceLayers::wetness`): a band of dark
  sand above a lake's or sea's waterline, wider when the waves are bigger, stream banks, and the
  ground and rock round a waterfall's foot, soaked by its spray. The weather dries them as it
  dries rain. A `ShoreCover` (the jetty's deck) keeps the ground under it out of it, since
  surfaces read the wetness of the ground they stand on.
- **Rendering** (`WaterFeature`): refraction of the scene behind, depth absorption and
  scattering, caustics on the bed, screen-space and sky reflections, sun glitter, light through
  thin crests, foam (whitecaps, wakes, rapids, shorelines), soft waterlines, ripples that fade
  with the pixel footprint (no shimmer at grazing angles), puddles, and an underwater view
  (Snell's window, fog, caustics, light shafts). Shaders are validated in a unit test.
- **Script API**: `water.sample`, `water.splash`, `water.bodies`, `water.spawn_lake`,
  `water.spawn_ocean`, `water.pour`, `water.calm`.

## Test area (`demo::spawn_demo`, showcase zone "water")

A tarn on a hill with a stream out of it; the showcase places it in its "water" zone.
`demo::area_height` and `demo::area_surface` describe the hill's ground, so a world's terrain
can carry it instead (`spawn_demo_with(.., DemoOptions { ground_mesh: false })`).

The showcase valley's own lake and the stream that falls over the cliff into it get real water
from `demo::spawn_valley`: the lake as a body over the landscape's ground, and each stream as a
flow region from its spring, prewarmed so it already runs into the lake.

| Test case | Shows |
| --- | --- |
| Tarn with a sandy shore and a jetty | Wind waves, depth color, shallows, caustics, reflections |
| Motorboat lapping the lake | Wake and bow waves, buoyancy with thrust and keel forces |
| Rowboat and five buoys on tethers | Floating objects bobbing on waves, moorings |
| Cork, pine, ice and concrete blocks dropped every 14 s | Floating by density (cork high, ice nearly awash, concrete sinks), splashes |
| Swimmer and wader | Ripples following people, footsteps in shallow water |
| Gunfire volleys from the rim | Bullet splashes along each trace |
| Depth charge every 11 s | A dome and ring waves from an underwater blast, boats thrown |
| Logs over the falls | Floating things carried by flowing water |
| Outlet, waterfall, rapids and stream | Shallow-water flow, white water, flow-mapped ripples |
| Crag across the tarn: a spring cascading down a crevice into a cove | Falls in tiers: a free fall onto a ledge pool, rapids down a slab, a curtain sliding down a face, a last fall into the lake |
| Pond, spill ditch and culvert | Water pooling at the foot of the stream, a drain |
| Two rain showers | Raindrop rings on water, puddles filling in hollows |

`tests/demo_area.rs` runs the area for 12 s without drawing and checks each case: floating order
by density, the ice block's freeboard, the concrete block on the bed, the stream running and the
pond full, the motorboat moving and staying on the water, and at least ten splashes.

Screenshots from several viewpoints, without a window:

```text
cargo run -p engine_water --example water_gallery --release -- <out_dir> [seconds] [views]
```

Views: overview, falls, shore, under, pond, wake, aerial, swirl (looking down on the
motorboat's wake), ditch, cascade, crevice.

To try shader changes without rebuilding, point `ENGINE_WATER_SHADER` at a copy of
`src/render/water.wgsl`; the gallery and the showcase then load the shader from that file.

## Contracts

Reads: `WindField`, `WeatherState` (rain, temperature, wind), `Ground` and `SurfaceMaterials`,
`FocusPoints`, `GroundContact`, `ProjectileTrace`, `WorldImpulse`.

Writes: the `Water` query (surface height, depth, velocity, normal), `SurfaceLayers` puddles
and wetness by the water, `WaterSplash`, `NoiseEvent`.

Cross-module interaction goes through `engine_world` (see AGENTS.md). If a contract is missing,
ask the core owner to add it rather than depending on another module's crate.

## Next

- Oceans: an FFT spectrum, horizon LOD and a showcase coast; planet oceans (spherical surface,
  per-planet gravity and optics).
- A larger, finer interactive area (on the GPU); flow beds that follow terrain the
  destruction module changes.
- Wave shoaling and shore breakers from a depth map of the ground under each body.

## Checklist

- Plugin registers its tunable components/resources (with docs) and `water.*` API functions.
- Fixed-step simulation in the right `SimSet`; honors `TimeDilation`/`LocalTimeScale`.
- Rendering through `RenderFeature`; shaders validated in a unit test.
- Unit tests plus a headless screenshot test.
- A `demo` module with `pub fn spawn_demo(commands: &mut Commands, origin: DVec3)` that builds a small showcase of the feature around `origin`; `games/showcase` places it in its zone.
