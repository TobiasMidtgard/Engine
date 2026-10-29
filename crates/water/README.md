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
  cutting through them. Regions step in parallel bands and skip dry ground. The bed follows
  the world: whatever stands on it in the water (a dropped crate, a boulder; from
  the physics scene, patch by patch) raises it, so the stream parts and piles up round it, and
  terrain edits re-read the ground under it. Fixed scenery that stood there when the stream
  was made is left out (the stream was laid round it).
- **Interaction**: a fluid field follows the camera (or the first `SimulationFocus`) over the
  nearest interactive body, or the stream beside it (where rings, wakes and foam drift
  downstream with the current): shallow water on a staggered grid, so pushes make ripples that
  bend in the shallows and reflect off the shore, and bodies moving through it drag the water
  along. Their wash curls into eddies (vortex shedding behind hulls and swimmers, vorticity
  confinement keeps the swirls tight), a propeller churns, and foam rides the currents.
  `WaterSettings::current_life_s` and `swirl` tune it. It steps only the 16-cell tiles where
  something moves, in parallel: calm water costs nothing. Swimmers and waders
  (`WaterInteractor`, `GroundContact`; wakes come from how they move through the water, so one
  standing in a stream parts it), cloth, ropes, hair and paddles (`WaterDisturbance`), bullets
  (`ProjectileTrace`), blasts (`WorldImpulse`, domes from underwater charges), drops and wakes
  push it. Every splash is published as `WaterSplash` (particles, audio) and `NoiseEvent` (AI),
  including steady puffs of spray from fast boats' bows and from the foot of every waterfall
  the flow finds. The drops a splash throws (a crown round the hole, a jet up the middle after
  a fast, small entry) fly back down and ring the water and fleck it with foam where they land
  (`drops`).
- **Dispersive ripples** (`surface`): over the fluid field, the surface that is drawn and
  floated on is evolved for every wavelength at once in Fourier space (Tessendorf's eWave;
  the hybrid with a shallow-water solver is Jeschke & Wojtan 2023): long waves outrun short
  ones, a stone sends out a spreading train of rings and hulls draw Kelvin wakes, the 19.5°
  wedge of feathered crests. It is carried along streams (through a cubic, which keeps short
  waves where a bilinear trace would blur them away; a hull's own short-lived wash carries
  nothing), shut out of dry land and soaked up at the edge. The renderer runs it on the GPU and reads its
  heights back for floating things; `WaterSettings::cpu_surface` runs it on the CPU where
  there is no GPU (the GPU picks up where it left off). `ENGINE_WATER_CPU_RIPPLES=1` draws the
  field's own waves instead, to compare. It runs on two levels, the fine one over the field
  (64 m) and a coarse one of 1 m cells (256 m) feeling the lake's mean depth; waves shorter
  than 4 m belong to the fine level, longer ones to the coarse, each push going into both
  for its own band, so a boat's long transverse waves spread into its whole wake. Long hulls
  (`WaterInteractor::with_length`) press the water down in the shape of their waterline,
  pointed at the bow (a moving pressure patch, Havelock 1908) rather than pushing it away,
  so the lake neither rises nor sinks round their course; its sharp edges make the short
  divergent waves that draw the wake's V. Its waves fade by `WaterSettings::surface_damping`
  (0.05/s), so a wake rolls on across the lake.
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
  thin crests, foam (whitecaps, wakes, rapids, shorelines), soft waterlines, puddles, and an
  underwater view (Snell's window, fog, caustics, light shafts). Waves too short for the
  Gerstner set, down to 2 cm ripples, continue its spectrum in two FFT tiles evolved on the GPU
  each frame (`detail`, after Tessendorf): they ride every lake, sea and stream (carried
  downstream on streams) and pinch into whitecaps that linger. Their mips keep slope moments
  (LEAN mapping), so what a pixel can't resolve widens the sun's glitter and blurs reflections
  by Cox and Munk's measured slopes instead of shimmering, and the sun glitters as a disk, not
  a point. Shaders are validated in a unit test.
- **Script API**: `water.sample`, `water.splash`, `water.bodies`, `water.spawn_lake`,
  `water.spawn_ocean`, `water.pour`, `water.calm`.

## Test area (`demo::spawn_demo`, showcase zone "water")

A tarn on a hill with a stream out of it; the showcase places it in its "water" zone.
`demo::area_height` and `demo::area_surface` describe the hill's ground, so a world's terrain
can carry it instead (`spawn_demo_with(.., DemoOptions { ground_mesh: false })`).

The showcase valley's own lake and the stream that falls over the cliff into it get real water
from `demo::spawn_valley`: the lake as a body over the landscape's ground, and each stream as a
flow region from its spring, prewarmed so it already runs into the lake.

A game that will show either asks for their art early (`demo::art::WaterDemoArt::prefetch`):
textures, ground, rocks, boats, the jetty, buoys, logs and the streams' beds are made on workers
while it loads (`WaterLoading::is_ready` waits for them), so the spawn itself only places them
(about 20 ms in a release build, from 100 ms).

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
motorboat's wake), ditch, cascade, crevice, ford (a wader in the stream). Asked for one
view, it simulates from there, so a stream's interactive water is live in it.

To try shader changes without rebuilding, point `ENGINE_WATER_SHADER` at a copy of
`src/render/water.wgsl`; the gallery and the showcase then load the shader from that file.

## Contracts

Reads: `WindField`, `WeatherState` (rain, temperature, wind), `Ground` and `SurfaceMaterials`,
`FocusPoints`, `GroundContact`, `WaterDisturbance`, `ProjectileTrace`, `WorldImpulse`.

Writes: the `Water` query (surface height, depth, velocity, normal), `SurfaceLayers` puddles
and wetness by the water, `WaterSplash`, `NoiseEvent`.

Cross-module interaction goes through `engine_world` (see AGENTS.md). If a contract is missing,
ask the core owner to add it rather than depending on another module's crate.

## Next

- Oceans: an FFT spectrum, horizon LOD and a showcase coast; planet oceans (spherical surface,
  per-planet gravity and optics).
- Boats beyond the fine level (32 m from the camera) float on the coarse level's waves too.
- Wave shoaling and shore breakers from a depth map of the ground under each body.

## Checklist

- Plugin registers its tunable components/resources (with docs) and `water.*` API functions.
- Fixed-step simulation in the right `SimSet`; honors `TimeDilation`/`LocalTimeScale`.
- Rendering through `RenderFeature`; shaders validated in a unit test.
- Unit tests plus a headless screenshot test.
- A `demo` module with `pub fn spawn_demo(commands: &mut Commands, origin: DVec3)` that builds a small showcase of the feature around `origin`; `games/showcase` places it in its zone.
