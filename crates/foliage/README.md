# engine_foliage — Foliage & ecology

**Owner:** Foliage and ecology. **Status:** round 1 (living forest, chopping, regrowth, glowing
cave fungi).

Plants that grow, compete and spread on their own. Chop a tree down and it falls, leaves a stump
and a log, and new trees grow where there is enough water, light and soil. Every plant differs
from its neighbors through inherited genes, and species can need other resources, such as the
mineral water that cave mushrooms drink before they glow.

```rust,ignore
app.add_plugins(engine_foliage::FoliagePlugin);

// A forest: an area where species seed in, compete and spread.
commands.spawn((
    Transform::from_xyz(0.0, 0.0, 0.0),
    EcologyRegion {
        half_extents: [60.0, 60.0],
        species: vec!["oak".into(), "birch".into(), "pine".into(), "fern".into(), "grass".into()],
        history: RegionHistory { grow_years: 80.0, initial_plants: [200, 50, 800], ..default() },
        ..default()
    },
));
// A spring: wetter, richer soil around it.
commands.spawn((Transform::from_xyz(10.0, 0.0, 5.0), ResourceSource { radius_m: 15.0, water: 0.4, ..default() }));
// One plant by hand (or `foliage.plant` from the AI bridge).
engine_foliage::spawn_plant(world, "oak", DVec3::new(3.0, 0.0, 4.0), 0.2, None)?;
```

## How it works

- **Species** (`PlantLibrary`, a registered resource) are pure data: form (broadleaf, conifer,
  shrub, fern, grass, flower, mushroom), size, growth speed, life span, canopy density, tolerance
  curves for water, light, nutrients, minerals and temperature, seeding, looks, glow, fuel and
  toughness. Built in: oak, birch, pine, spruce, hazel, fern, grass, wildflowers, bluebell,
  glowcap and lantern cap. Add or tune species live with `foliage.define_species`.
- **Genes** per plant (vigor, height, crown, hue, drought and shade tolerance, lean, mesh
  variant) are inherited from the parent with small mutations, so populations adapt (a dry
  ridge drifts toward drought-tolerant pines).
- **Ecology regions** (`EcologyRegion`) hold a soil grid (water, nutrients, minerals, ash and
  rot bonus, trampling). Each step, every plant reads its resources (Liebig's law: the
  scarcest need limits it), light after the shade of taller crowns, and its share of water
  and nutrients against its neighbors' roots. Health follows; plants grow, seed (downwind for
  light seeds), age and die. Dead trees stand as snags, then fall and feed the soil.
  Live wetness from rain (`SurfaceLayers`), ground material (`Ground` + `SurfaceMaterials`),
  temperature (`WeatherState` + `HeatField`) and wind (`WindField`) all count.
- **History**: a region can prewarm (grow 110 years, then clear-cut or burn part of it, then
  regrow) the first time it runs, so a game starts in an old forest with a recovering clearing.
- **Scale**: regions far from every `SimulationFocus` stop and catch up in coarse steps when
  the player returns. Trees use a cheaper mesh far away; grass, ferns and flowers switch to a
  far mesh (about a tenth of the triangles) past `EcologySettings::small_plant_lod_distance_m`
  and vanish beyond `small_plant_distance_m`. Each species has four mesh variants shared by all
  its plants, so a forest draws in a few instanced batches.
- **Chopping** (`felling`): `DamageEvent`s on a plant, its drawn parts or its trunk collider
  add chop damage (axes count fully, bullets a little, fire not at all). Past the species'
  toughness the tree falls away from the blow: it swings down around the stump, crashes
  (`WorldImpulse` for foliage and dust, `NoiseEvent` for AI, `GroundContact`s for audio and
  terrain, `DamageEvent`s for whatever it lands on), crushes saplings and settles as a
  `FelledLog` beside its stump. Logs lose their leaves, then their twigs, grey and rot away,
  feeding the soil. Oak, birch and hazel stumps resprout. Explosions knock trees over too.
- **Fire**: plants are `Combustible`; when the fire module burns them they become charred
  snags that collapse into ash, which enriches the soil for the next generation.
- **Glow**: glowing species pick a brighter material the healthier and better fed they are,
  breathe slowly, and light their surroundings through a pool of point lights (one per
  cluster, at most 40).
- **Trunk colliders**: trees within `EcologySettings::collider_distance_m` (150 m) of a
  `SimulationFocus`, and every log, get a static capsule collider on its own entity
  (`FoliageCollider`), so players, vehicles and bullets hit trunks once a physics backend runs.
  Farther trees drop theirs and get it back when a focus comes near.
- **Looks**: procedural, per species: bark texture sets (albedo, normal, occlusion and
  roughness), painted leaf and needle atlases with normal maps, crowns built from clumps of
  shaded sprays, detailed grass, ferns, flowers and fungi with their own maps. Small plants
  lean with the slope they grow on. `examples/lookdev_*` render each set close up.
- **World shifts**: forests follow `WorldShift` (floating origin, planet frames): soil grids,
  region centers, falling trees, which way logs lie and what plants are posed on move with
  the world (`shift`).

## API (`foliage.*`)

`plant`, `chop`, `fell`, `clear` (fell, burn or remove an area), `species`, `define_species`,
`stats`, `inspect` (why a plant thrives or struggles: what limits it, the soil under it),
`set_speed` (world or per-region growth speed, pause), `add_source` (spring, mineral pool,
dry ridge). Every component and resource above is registered for scenes and the AI bridge.

## Showcase

`demo::spawn_demo(commands, center)` builds the living forest (old mixed forest, a clear-cut
regrowing with a tree felled every 25 s at its edge, a burn scar, a spring-fed pond, a dry
rocky ridge) and `demo::spawn_cave_demo(commands, center)` the mushroom cave (mineral pool and
seep feeding glowcaps and lantern caps, a cave floor with mineral crust). Both run 20–30× faster
than the world default so regrowth is visible while you watch.

`cargo run -p engine_foliage --example foliage_shots --release -- <dir>` renders both from
several viewpoints (including a tree mid-fall and the clearing ten years later).

## Contracts

Reads: `Ground`, `SurfaceMaterials`, `SurfaceLayers` (wetness), `WeatherState`, `HeatField`,
`WindField`, `Water` (no plants under water), `FocusPoints`, `PhysicsQueries`, `Gravity`,
messages `DamageEvent`, `WorldImpulse`, `GroundContact` (trampling), `TerrainChanged`.

Writes: `Combustible` on plants and logs, static `RigidBody` + `Collider` entities, messages
`WorldImpulse`, `NoiseEvent`, `GroundContact`, `DamageEvent`.

## Next

- Grass meadows through the renderer's shared strand pipeline (blades bend under feet).
- Impostors for distant forests and planet-scale scattering on terrain.
- A falling tree as a dynamic body when a physics backend is installed.
- Seasons (autumn color, bare winter crowns, snow on branches).
