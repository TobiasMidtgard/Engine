# engine_terrain — Planets and terrain

**Owner:** Planets and terrain. **Status:** working (round 1).
Background research: `game-engine-research/graphics.md` in the project files.

Real-scale planets and moons you can walk on, with ground that reacts to feet, wheels and
weather. One `PlanetTerrain` component turns an entity into a planet (or a flat terrain);
everything else (streaming, LOD, colliders, ground queries, prints) follows the focus.

## What it does

- **Cube-sphere planets at real scale.** Six quadtree faces, 33×33-vertex patches with
  skirts, built in parallel on the compute pool and kept in a pool of GPU vertex pages.
  Patches split by distance (`TerrainSettings::split_factor`), with horizon and frustum
  culling; a patch only splits once all its children are built, so there are no holes when
  flying fast, and a teleport builds until the view is complete. Vertices geomorph toward the
  parent patch's shape before a split or merge, so detail never pops. Positions are `f64` and
  rendered camera-relative, so a 6,371 km Earth is exact to the centimeter at the surface.
  Patches also draw the renderer's depth prepass (depth, normal, motion; moving planets
  carry their own motion), so the ground gets ambient occlusion and contact shadows from
  what stands on it and anti-aliasing reprojects it cleanly.
- **Planet shapes.** `PlanetShape` presets for an Earth-like world (continents, oceans,
  eroded mountains whose steep sides break into rock ledges, hills, climate bands), the
  Moon (craters, maria) and Mars (craters, plains, dry ice caps), all seeded and
  deterministic. Detail is generated down to
  `finest_spacing_m` (0.5 m by default).
- **Flat pads and a view worth having.** `FlatPad` blends the planet into level ground for a
  level, town or spaceport. `PlanetTerrain::home_transform(pad)` places the planet so the pad
  sits at the world origin with +Y up (north toward −Z; `heading_deg` 180 turns north to +Z,
  the weather module's compass, matched by `SpaceFrame::home_heading_deg`). A pad also lays
  out the land around it: rolling hills, a snow-capped range on one horizon (`mountains`,
  `mountain_azimuth_deg`, `mountain_distance_m`; by default 3–4.5 km peaks 26 km north) and a
  ragged coast on another (`sea_azimuth_deg`, `sea_distance_m`; by default 16 km
  south-southwest, a gulf that opens into the ocean).
- **Hand-made ground on a real planet.** `PlanetTerrain::with_overlay(radius, blend, f)`
  replaces the ground around the home spot with your own height function `f(x, z) → y` in
  world coordinates (a level, the showcase valley) and blends it back into the planet at the
  edge. The terrain bends it onto the sphere so the ground passes through `(x, f(x, z), z)`
  to the centimeter (heights are world y as given: subtract the curvature drop
  `(x² + z²) / 2R` yourself only if the level should follow the sphere), and rendering, the
  `Ground` query, prints and colliders all use it.
  `with_overlay_paint(|x, z| …)` paints it: an `OverlayPaint` layer (grass, leaves, dirt,
  mud, sand, gravel, rock, concrete, snow, ash) for roads, plazas and woods, and a tint for
  fields. Footsteps, rain and the `Ground` query follow the painted layer; edits paint over it.
  When the level already has its own material map, `with_ground_surface(Arc<dyn GroundQuery>)`
  lets that query name the material while the terrain answers height and normal, edits
  included (the sea and edit-painted layers stay the terrain's; never pass the world's
  `Ground`, which is the terrain itself).
- **Ground cover from afar.** Climate colors (lush, dry, desert, snow by temperature and
  altitude), woods as dark canopy, meadows and fields in patches a few hundred meters
  across, worn bare patches, beaches. Mountainsides are a patchwork: grass and woods hold
  steep ground below the tree line, outcrops stand on spurs with grass and scree in the
  hollows, and weathered rock (iron-stained faces, dark wet gullies) takes over on cliffs
  and high up. From orbit, noise per pixel gives woods, snowfields and coasts natural
  shapes instead of the vertices' blocky ones. On cratered worlds the shader shades the
  craters too small for the vertices, per pixel, from the same scatter as the shape (so
  nothing pops on the way down), fresh ones with bright walls and ejecta.
- **Detailed ground materials up close.** Nine PBR materials (turf in tufts over thatch,
  leaf litter with needles and twigs, soil, churned mud, rock, snow, rippled sand, gravel,
  concrete) are generated procedurally in `materials.rs` like a node-graph material: height
  from noise, cells and thousands of scattered blades, leaves and stones, then normals,
  occlusion, cavity tint and roughness, with mips that widen roughness where normals
  disagree. They bake once per process on a background thread (about 2 s at 1024²,
  `TerrainSettings::material_resolution`) and need no files. The shader multiplies each
  material's detail into the planet palette's color, so the Earth, Moon and Mars share them
  and the ground looks the same near and far; materials blend by height (stones and blades
  poke through their neighbors), tiles never visibly repeat (two offset lookups switched by
  noise), and rock is projected on three axes at three scales so cliffs read as rock from
  kilometers away. `examples/material_swatches.rs` saves a lit preview of each.
- **Photo-scanned ground where fetched.** Seven materials (grass, forest floor, soil, mud,
  rock, sand, gravel) have CC0 Poly Haven scans listed in `assets/manifests/terrain.toml`.
  After `cargo run -p fetch-assets -- terrain` they replace the procedural versions
  (`TerrainSettings::scanned_materials`, on by default); the 2K color, normal, roughness,
  occlusion and height maps are filtered down to the material resolution through the same
  mip builder. Snow and concrete stay procedural, and any material whose files are missing
  or unreadable falls back on its own. The `terrain.scanned_materials` diagnostic counts the
  scans in use.
- **Edits.** `TerrainEdit` messages dig, raise, smooth and paint (sphere, capsule, box).
  Edits are analytic, so they are exact at every level of detail and from any distance, and
  rebuild only the patches and colliders they touch.
- **Footprints and tracks.** A deformation grid (4 cm cells, 82 m across) follows the
  focus and records every `GroundContact`: feet (heel, arch, toes), paws, hooves, wheels,
  tank tracks and bodies. Each surface responds differently: mud is plastic and pushes up a
  rim, snow compacts down to its depth, sand is granular and slowly fills back in, grass is
  flattened and springs back. Prints outside the grid are kept and re-stamped when it moves.
- **Weather on the ground.** Wetness, puddles, snow depth and scorch from `SurfaceLayers`
  (written by the weather module) darken and gloss the ground, turn soil to mud, fill
  footprints, ruts and the hollows of the mud texture with water (with rain ripples), and
  cover the ground in snow that settles in hollows and on ledges first and that footprints
  cut through. Without `SurfaceLayers`, the global weather in the frame is used.
- **Ocean.** Optional sea surface at sea level (`PlanetTerrain::ocean_surface`) with depth
  tint and waves; turn it off when a water module draws the sea.
- **Ground query and colliders.** Installs the `Ground` provider (height, normal, surface
  material) for every terrain, and keeps heightfield collider tiles around the focus.

## Contracts

Reads: `GroundContact`, `TerrainEdit`, `SurfaceLayers`, `Weather`, `FocusPoints`,
`SurfaceMaterials`.

Writes: `Ground` (the query provider), `TerrainChanged`, static `Collider` heightfield tiles
(tagged `TerrainColliderTile`).

Heightfield layout for physics: `heights[row * cols + col]`, rows along the tile's local +Z,
columns along local +X, both centered on the tile origin; local +Y is the planet's up at the
tile center.

API: `terrain.height {x, y, z}`, `terrain.edit {...}`, `terrain.footprint {...}`,
`terrain.stats`.

## Use it

```rust,ignore
let pad = FlatPad { lat_deg: 40.0, ..Default::default() };
let planet = PlanetTerrain::earthlike(7).with_pad(pad);
commands.spawn((planet.home_transform(pad), planet));

// With the space module: the same planet on the real Earth, so you can fly to the Moon.
let bodies = engine_space::demo::spawn_solar_system(&mut commands);
commands.entity(bodies["Earth"]).insert(
    PlanetTerrain::earthlike(7)
        .with_radius(6_371_000.0)
        .with_pad(FlatPad::default())
        .with_overlay(20_000.0, 6_000.0, |x, z| my_level_height(x, z))
        .with_overlay_paint(|x, z| my_level_paint(x, z)), // Option<OverlayPaint>
);
commands.entity(bodies["Moon"]).insert(PlanetTerrain::moon(7).with_radius(1_737_400.0));
```

`examples/terrain_shots.rs` renders the test views headless (lab prints, rain, snow, painted
ground, the range, the coast, the Moon and Mars from orbit) and `examples/terrain_map.rs` draws a relief
map around the home pad for tuning shapes.

## Test cases in the showcase

`demo::spawn_demo` builds the Terrain lab: four 32 m strips of mud, snow, sand and grass with
a person, a dog, a horse and a rover walking them (prints and tracks appear from the first
frame), plus a dug crater, a trench and a raised mound. The walkers make prints but have no
bodies: attach a character, animal or vehicle from the module that owns it as a child of
each `DemoWalker` entity (its `Transform` moves along the path facing forward), or pass
`LabOptions { stand_in_bodies: true }` to `spawn_demo_with` for simple figures built from
shapes: a hiker with a backpack, a dog, a horse and a rover, whose legs and arms swing with
their steps (`LabLimb`, trotting animals move diagonal legs together) and whose wheels roll
with the distance driven. The strips and edits sit on the ground under each spot (sampled
from `Ground` when they apply), so the lab works on uneven ground too.

## Known limits (next rounds)

- Prints are recorded in world X/Z, which is right on level ground near the focus (the
  showcase map, or anywhere once the space module re-centers the world on the player).
- No caves or overhangs yet (voxel volumes are next).
