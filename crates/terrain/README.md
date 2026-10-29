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
  A planet that fills only part of the view gets finer patches in its share of the screen
  (up to 4× finer split distance, aiming at `TerrainSettings::patch_budget` patches for a
  planet that fills the view), so seen from orbit its canyons, volcanoes and coasts stay
  sharp instead of dissolving into a few big patches.
  Patches also draw the renderer's depth prepass (depth, normal, motion; moving planets
  carry their own motion), so the ground gets ambient occlusion and contact shadows from
  what stands on it and anti-aliasing reprojects it cleanly.
- **Shadows kilometers long.** Patches cast into the sun's shadow cascades and into the
  renderer's far shadow map (out to `RenderSettings::far_shadow_distance`, 8 km), so at low sun
  ridges shade the valleys beside them and a range throws its shadow across the plain. A second
  walk of the quadtree from the camera picks the patches for the far map: the ground the map
  covers plus what lies toward the sun from it, outside the view too, one level coarser than
  the view and never finer than about half a shadow texel between vertices, and builds what
  it lacks a few patches a frame after the view's own.
- **Planet shapes.** `PlanetShape` presets for an Earth-like world (continents, oceans,
  eroded mountains whose steep sides break into rock ledges, hills, climate bands), the
  Moon (craters, maria) and Mars, all seeded and
  deterministic. Detail is generated down to
  `finest_spacing_m` (0.5 m by default). On the Moon the maria are big impact basins ringed by
  worn massifs and flooded with lava of different shades, plus lava that spilled across the
  lowlands, gathered on the face a tidally locked moon turns to its planet (planet-local +Z,
  which engine_space turns to the parent; `PlanetShape::lava_basins` 0 leaves basins pale and
  dusty, as on Mars). Craters age from crisp bowls into soft dishes, and
  on airless worlds hills and mountains are worn round (`PlanetShape::worn`).
- **The real Mars.** `PlanetShape::mars` lays the real planet's landmarks where they are
  (`landforms::Landforms`, by latitude and longitude, degrees east): the northern lowlands,
  the Tharsis and Elysium rises, Olympus Mons (21 km, its scarp cliff and nested caldera) and
  the other shield volcanoes, Valles Marineris (7 km deep and up to ~200 km wide in Melas, with
  Candor, Ophir, Ius's twin troughs and Coprates Catena; steep spurred walls with bays and
  promontories, landslides, dark dune floors) with Noctis Labyrinthus at its head and chaos terrain at its mouth, the Hellas,
  Argyre and Isidis basins, both polar caps on their layered domes with spiral troughs and
  Chasma Boreale, the dark regions seen from Earth (Syrtis Major, Mare Acidalium, ...) with
  wind-streaked edges, and a dusty mottling between them. The seed varies only the small
  things. Any planet can carry its own `Landforms`. Young lava keeps few craters: the shape
  and the shader thin them out on the volcanoes (to a tenth), the rises and the lowland
  plains (`Landforms::youth`). Ice on barren worlds takes on the planet's dust where it is
  thin. Climate temperatures run from −130 to +60 °C. On airless worlds (no atmosphere to
  hide the sun below the horizon) the ground past the terminator stays dark however its
  slopes face, except peaks standing far above the land around them.
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
  Layers can cover everything at full weight and meet edge to edge: each vertex carries a
  weight per layer, so borders blend along the textures' relief instead of in triangle steps,
  and where grass or leaves meet bare ground (a road, a plaza, a bed) the border wanders the
  way turf creeps over a verge instead of following the paint exactly.
  When the level already has its own material map, `with_ground_surface(Arc<dyn GroundQuery>)`
  lets that query name the material while the terrain answers height and normal, edits
  included (the sea and edit-painted layers stay the terrain's; never pass the world's
  `Ground`, which is the terrain itself).
- **Ground cover from afar.** Climate colors (lush, dry, desert, snow by temperature and
  altitude), woods as dark canopy, meadows and fields in patches a few hundred meters
  across, worn bare patches, beaches. Natural woods seen from well above them (from 30 to
  80 m up, a few dozen meters out, until a pixel spans a few meters) show their trees: crowns of random size in
  groups, heaped from leaf clumps, each tree its own green (now and then a dark spruce or a
  pale birch), casting shadows away from the sun onto the gaps and crowns beside them, and
  thinning out tree by tree into the meadow at the woods' edges. Further out, stands of
  trees lighter and darker over tens of meters. (The foliage module draws the real trees
  near the camera.) Mountainsides are a patchwork: grass and woods hold
  steep ground below the tree line, outcrops stand on spurs with grass and scree in the
  hollows, and weathered rock (iron-stained faces, dark wet gullies) takes over on cliffs
  and high up. On Earth-like worlds, cliff faces are eroded into ribs and runnels along the
  fall line (8 m and 32 m across, sixteen times as long) and streaked where water runs down
  them: dark varnish and pale mineral stains. From orbit, noise per pixel gives woods, snowfields and coasts natural
  shapes instead of the vertices' blocky ones. On cratered worlds the shader shades the
  craters too small for the vertices, per pixel, from the same scatter as the shape (so
  nothing pops on the way down); fresh ones are bright spots with halos, and the youngest
  big ones throw rays of bright ejecta across the globe, like Tycho and Copernicus. Moon
  dust scatters light the way regolith does (`TerrainPalette::regolith`), so a full Moon is
  as bright at its edge as in its middle and brightens toward full phase. Underfoot the
  regolith is powdery and gently lumpy, with stones poking out of it everywhere.
- **Climate zones from orbit.** On worlds with air and sea (`Climate::circulation`, 0 for
  the Moon and Mars), rain follows the planet's wind belts: wet near the equator, desert
  belts around 25° north and south, wetter storm tracks around 55°, dry poles. Ground far
  inland is drier, and land behind a mountain range (upwind is west poleward of 30°, east
  nearer the equator) lies in its rain shadow. Ocean currents warm and cool the sea by up
  to 9 °C, so sea ice reaches further toward the equator in some places than others; the
  ice breaks into floes at its edge. Shallow shelf seas glow teal (more so in warm water),
  cold deep seas are greener, and deserts mix sand with dark stony plains.
- **Rivers and lakes.** Water drains across the whole planet (a priority flood on a 40 km
  grid) and, finer, around the home pad (600 m cells, 240 km across): rivers start in wet
  high ground, join into networks and run downhill to the sea, widening as they gather
  water, so big rivers stay thin threads from orbit and small streams show up close. They
  bend in meanders that flatten out in steep country, cut channels with low banks into
  their valleys, and grow strips of woods along them in dry land. Deep, wet hollows hold
  lakes with ragged shores; shallow ones are breached and drained. River water is a water
  surface underfoot (`Ground` reports it), freezes in the cold, and the home pad and the
  hand-made overlay stay dry. Turn it off with `PlanetShape::rivers = false` (the Moon and
  Mars have none); `shape.river_network()` returns the solved network, and its
  `segments()` give every river piece with its water level and width.
- **Seas and coasts.** The open sea shows the bed through clear shallows (turquoise in warm
  water, greener in cold, with weed patches), darkening with depth. Wind sets how rough it
  is (Cox–Munk slope spread) and so how wide the sun's glitter spreads, and how high the
  breakers run: surf lines roll in toward the shore, break into froth at the waterline and
  wash up the sand, leaving a darker wet band behind. Beaches are strips some tens of meters
  wide whose height follows the coast's slope, with pale dune grass behind them, and they
  fade out from afar so coasts seen from orbit get no outline.
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
  kilometers away. Past a few dozen meters, where a texture is squeezed into a pixel or two
  along the view, each ground hands over to a copy four times larger, so meadows, fields and
  roads keep their grain out to a kilometer; anisotropic filtering keeps textures sharp
  across the view at glancing angles. Grass grows in clumps whatever its color, painted and
  tinted grass included: straw-colored tufts beside dark, dense ones, and sunburnt and lush
  stretches tens of meters across; dirt, sand and gravel lie in damp, darker and dry, paler
  patches. Grass tinted to one color (a level's painted pasture) also lies across the land
  the way grass grows: lusher and darker in hollows, sun-dried on crests and knolls, in
  drifts tens to hundreds of meters across, so it doesn't read as a lawn from the air. `examples/material_swatches.rs` saves a lit preview of each.
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
- **Worn paths.** Ground that many feet cross wears into a path (`wear.rs`). Every
  `GroundContact` adds a little wear to the half-meter cells it lands on (heavier contacts,
  wheels and soaked ground more), and unused ground heals, each kind at its own pace: turf is
  trodden flat and pale, thins to dusty beaten earth over hundreds of passes and regrows over
  days (half-life 4 days); earth packs into a hollow; snow packs within a few dozen passes
  (the busiest tracks to glossy ice) and new snow buries it; sand is churned into lumps and
  the wind fills the trough within the hour; gravel is shoved aside into a firm track over
  weeks; rock barely polishes. One walker leaves no lasting mark; a crowd on one line makes a
  path with a bare middle, a trodden margin and ragged turf edges, sunk a few centimeters.
  In rain the bare earth turns to mud. The map is sparse and persistent (64-cell tiles where
  someone walked, kept per terrain in its own frame, one region per few kilometers walked);
  the renderer draws a 512 m window of it around the camera, and beyond that, out to 2 km,
  a four times coarser map of how much of each 2 m cell is bare path (paths show from hills
  and the air). `WearSettings` sets the cell
  size, a `gain_scale` (paths form quicker) and a `heal_scale` (they fade quicker).
- **City lights at night.** On inhabited worlds (`TerrainPalette::night_lights`, 1 on
  Earth) towns and cities light the night side past twilight, seen from a few kilometers off
  up to orbit. People live on mild, wet, low, gentle land, most by coasts and rivers, and
  regions hundreds of kilometers wide are busy or nearly empty. Cities (some tens of
  kilometers across, with bright middles, lit districts and dark parks, and lit roads
  running out of them), towns and villages strung along the roads lie in cells per size,
  at radiance as satellites measure it (a city's middle about 2 cd/m², so night views want
  an exposure near EV100 −1). Up close a city breaks into winding main roads and clusters of
  lamps, then single lamps; places smaller than a pixel spread over it at the same total
  light, so nothing sparkles from orbit. Seen from far above, the night side reflects no
  sky either.
- **Trails people lay themselves.** `PathWear::walk_between` sends walkers between places
  (each weighted by how busy it is). Each takes the way that feels shortest on a coarse
  grid where a path that shows feels easier than open ground (`TrailWalk::attraction`, 0.5:
  a path up to 60° off the way beats cutting across) and paths a few meters off draw them
  in, each with a whim of their own off the paths; the way is pulled taut (the grid's own
  directions don't show), eased round corners, and feet find a path's trodden middle.
  Unused ground heals between trips. So the walkers' first lines are reinforced or
  forgotten, and trails emerge that share a trunk toward places in the same direction and
  fork in curves, instead of a straight line between every pair.
- **Weather on the ground.** Wetness, puddles, snow depth and scorch from `SurfaceLayers`
  (written by the weather module, and by the water module along shores) darken and gloss
  the ground (wet sand to about half its dry brightness, without turning to mud), turn soil
  to mud, fill
  footprints, ruts and the hollows of the mud texture with water (with rain ripples), and
  cover the ground in snow that settles in hollows and on ledges first and that footprints
  cut through. The wind lays snow in low drifts and ridges along the prevailing east-west
  wind, and up close ice crystals here and there glint in the sun. A deep cover buries the
  litter, grass and pebbles under it; a dusting shows them. `SurfaceLayers` covers a grid
  around the player; beyond it the ground is as wet and snowy as the open ground along the
  grid's rim (its upper quartile), fading in over the grid's outer tenth, so a snowfall or a
  downpour reaches the horizon instead of ending at a square. Without `SurfaceLayers`, the
  global weather in the frame is used.
- **Ocean.** Optional sea surface at sea level (`PlanetTerrain::ocean_surface`) with depth
  tint and waves; turn it off when a water module draws the sea.
- **Ground query and colliders.** Installs the `Ground` provider (height, normal, surface
  material) for every terrain with `ground` on that the focus is within about its radius of
  (far planets, which move every frame, are left out, so the provider is not rebuilt each
  frame and the ground is the one underfoot on the Moon or Mars too), and keeps heightfield collider tiles around every focus
  point and under every dynamic body (awake or asleep, the nearest 64 away from a focus),
  so a crate dropped far off or a car driven by AI never falls through the ground.
- **Rock fields** (`rocks.rs`). Stones and boulders strewn over the Moon and Mars near every
  focus point, as Apollo and Curiosity photos show them (`PlanetTerrain::rocks`, a
  `RockField`; off on Earth-like terrains, where foliage and the game place rocks). Sizes
  follow the power law of rock counts (Golombek & Rapp 1997), rocky ground comes in patches a
  few hundred meters across and on slopes, and each rock is a fractured-stone shape tilted to
  the ground and sunk into it. Rocks lie in quadtree cells of the terrain's own tiling (stones
  in 24 m cells out to 110 m, boulders in 96 m cells out to 450 m), one mesh per cell, built on
  worker threads from the cell's hash, so the same rocks are there when you come back; edits
  rebuild the cells they touch. Diagnostics `terrain.rock_cells` and `terrain.rock_builds`.
- **Where the woods are.** Installs the `ForestCover` provider: canopy cover (0..1) and needle
  share at any point, by the same rules the shader paints its woods with (`forest.rs`; a
  test fails when the shader's rules change without it), so the foliage module plants trees
  where the ground shows forest. About 10 µs a lookup on open ground, 30 µs in woods (release).
  `FOREST_MASK=1 terrain_shots ...` writes the cover under each view to lay over the shot.

- **Far looks.** A `FarLook` next to a `PlanetTerrain` hands the ground how its planet looks
  from afar: an equirectangular map (sRGB albedo, glowing melt in alpha) painted by whatever
  draws the planet far away (the space module's far view, `engine_space::look::bake_surface_on`).
  Where a pixel spans more than a few kilometers the ground wears the map outright, so the
  planet keeps its face as the ground takes over from the far view (the dark plains, dune
  seas, lava cracks and the ice's stained lineae, which the ground alone lacks from orbit);
  below a few hundred meters per pixel the map only tints the ground's own colors by
  `near_tint`, at the scale of whole provinces. Up to four planets' maps are kept on the GPU
  (1024×512 with mips, the least recently drawn replaced). `FarLookMap::resized` filters any
  larger map down in linear light.

## Contracts

Reads: `GroundContact`, `TerrainEdit`, `SurfaceLayers`, `Weather`, `FocusPoints`,
`SurfaceMaterials`, `RigidBody` (dynamic bodies, for their collider tiles).

Writes: `Ground`, `ForestCover` and `GroundWear` (query providers; `GroundWear` from a
snapshot of the worn-path map refreshed twice a second while paths change), `TerrainChanged`, static `Collider` heightfield tiles
(tagged `TerrainColliderTile`).

Heightfield layout for physics: `heights[row * cols + col]`, rows along the tile's local +Z,
columns along local +X, both centered on the tile origin; local +Y is the planet's up at the
tile center.

API: `terrain.height {x, y, z}`, `terrain.edit {...}`, `terrain.footprint {...}`,
`terrain.wear {at}` (how worn the ground is, 0..1), `terrain.walk_path {from, to, passes,
load_kg, spread_m}` (wear a path at once), `terrain.walk_between {places, trips, load_kg,
attraction, trips_per_day}` (people lay their own trails between places), `terrain.stats`.

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

Made-up planets of every kind come from a short `WorldRecipe` (kind, seed, radius, sea share,
craters, pressure, temperature, the star's temperature, the color seen from afar, the melt's
glow): `PlanetTerrain::world(&recipe)` starts from the closest known world (the Moon for
airless rock, Io-like black crust over glowing melt for lava worlds, Mars for hot and cold
deserts, Earth for temperate land and sea worlds with plants colored by the star, ice plains,
Titan's dark dunes and lakes) and tunes it. The showcase gives the space module's planets of
other stars one as a ship comes within 40 radii. On lava worlds `TerrainPalette::lava_glow`
lights fissures, rifts and pools of melt up to the latitude where the climate passes 1000 °C.
Views `exo_lava`, `exo_lava_night`, `exo_airless`, `exo_desert`, `exo_terra`,
`exo_ocean_red_dwarf`, `exo_cold_desert`, `exo_ice` and `exo_hazy` in `terrain_shots`.

`examples/terrain_shots.rs` renders the test views headless (lab prints, rain, snow, painted
ground and a patchwork of painted layers, a meadow tinted the way the showcase paints its
valley and a natural one at eye height, snow at eye height and over the valley, woods from the air, the range, the range at dusk
(`range_dusk`: ridges casting long shadows), the coast, a beach at eye height (`earth_beach`), the painted patchwork after rain (`painted_wet`), the Moon
underfoot, from low orbit, half lit and full, the Earth's climate belts from far out
(`earth_globe`), a river at eye height (`earth_river`) and rivers from the air
(`earth_rivers_air`), worn paths (`paths_meadow`, `paths_meadow_high`, `paths_rain`,
`paths_snow`, `paths_beach`, `paths_hill` from 600 m off; `NO_PATHS=1` for the ground before;
trails laid by walkers between a farm and four places, `paths_desire` from the air and
`paths_desire_eye` at the fork, `STRAIGHT=1` for everyone on straight lines), city lights at
night (`earth_night_globe`, `earth_night_leo` from 420 km, `earth_night_high` from a plane at
9 km; `NO_LIGHTS=1` for the dark side without them), and Mars: from orbit (`mars_orbit`,
`mars_valles`), Olympus Mons (`mars_olympus`), the north pole (`mars_pole`), and the floor and
rim of Valles Marineris (`mars_canyon`, `mars_rim`))
with the game's auto exposure, and `examples/terrain_map.rs` draws a relief map with rivers and lakes around
the home pad for tuning shapes.

## Test cases in the showcase

`demo::spawn_demo` builds the Terrain lab: four 32 m strips of mud, snow, sand and grass with
a person, a dog, a horse and a rover walking them (prints and tracks appear from the first
frame), plus a dug crater, a trench and a raised mound. The walkers make prints but have no
bodies: attach a character, animal or vehicle from the module that owns it as a child of
each `DemoWalker` entity (its `Transform` moves along the path facing forward), or pass
`LabOptions { stand_in_bodies: true }` to `spawn_demo_with` for simple figures built from
shapes: a hiker with a backpack, a dog, a horse and a rover, whose legs and arms swing with
their steps (`LabLimb`, trotting animals move diagonal legs together) and whose wheels roll
with the distance driven. `demo::spawn_path_crowd` sets a crowd walking to and fro along
one line, so a path wears in (raise `WearSettings::gain_scale` to watch it form within
minutes). The strips and edits sit on the ground under each spot (sampled
from `Ground` when they apply), so the lab works on uneven ground too.

## Known limits (next rounds)

- Prints are recorded in world X/Z, which is right on level ground near the focus (the
  showcase map, or anywhere once the space module re-centers the world on the player).
- River and lake water is drawn by the terrain and has no water query yet, so nothing floats
  or swims in it. A water module can draw and simulate it from `river_network().segments()`.
- No caves or overhangs yet (voxel volumes are next).
