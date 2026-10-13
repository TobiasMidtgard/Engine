# engine_destruction — Destruction

**Owner:** Destructible terrain and buildings. **Status:** first full version: Siege-style
walls and barricades, reinforced concrete with rebar, breach charges (explosive and thermite),
Valheim-style structural integrity, voxel cliffs and rock you can dig and collapse, Voronoi
fracture and debris.

## What it does

- **Walls and barricades** (`DestructibleWall`, Rainbow Six Siege style) are a 2.5D grid of
  cells, each with a remaining depth. A hit carries energy (a bullet's kinetic energy, a melee
  swing, an explosion's flux at that distance) and removes `energy / toughness` of depth around
  where it lands, so drywall gets a fist-sized hole from a kick and a hand-sized one from a
  burst, planks splinter, brick and concrete only scar unless the charge is big. Cells that end
  up cut off from the wall's anchors (the floor, the posts beside it) fall out as chunks.
- **Reinforced walls** have a rebar mesh in their cells. Explosions strip the concrete off
  the bars but leave the mesh standing until the energy per square meter is enough to cut
  steel (2.5 MJ/m²), the way a reinforced wall in Siege needs a hard breacher.
- **Breach charges** (`BreachCharge`): an explosive charge blows a hole through soft walls; a
  thermite charge burns around its edge (sparks, smoke, glowing strip), cutting rebar too, and
  the panel inside drops out at the end.
- **Building pieces** (`StructuralPiece`, Valheim style) carry support from the ground up:
  every piece gets the best support any path to a grounded piece gives it, losing some per
  meter up and more per meter sideways, by material (wood carries far less sideways than
  stone). Break a post and whatever depended on it falls after a short creak. `ShowSupport`
  paints pieces blue (grounded), green, yellow, orange and red (about to fall), like Valheim's
  build mode.
- **Build mode** (`build`, Valheim's hammer): a `Builder` (the player's camera, a script)
  picks one of ten pieces (wood floor, wall, pole, beam, 26° and 45° shingle roofs, a 26°
  board gable to close a roof's end; stone floor, wall and pillar) and aims. A translucent ghost of the piece sits on what the aim hits
  (on top of the ground or a floor, against a wall's side or a floor's edge) and snaps so its
  nearest snap point meets a neighbor's (pole and beam ends, panel corners and edge midpoints,
  a floor's on its top face, block corners), skipping spots inside other pieces. The ghost
  shows the support it would get in the support colors, or a deep red where it would sit
  inside something, touch nothing, or reach too far from support to stand; place it, turn it
  in 22.5° steps, or take built pieces down (what they held falls). Pieces stand on the
  terrain or on voxel rock alike (dig the rock out from under them and they fall). Built
  pieces show their support colors while their builder has the support view on. Keys (bound
  unless the game binds them): B build mode, left mouse place, X remove, R rotate, C next
  piece, 1 to 9 and 0 pick a piece.
- **Burning wood** (`burn`): wooden building pieces get an `engine_world::Combustible`
  holding their wood as fuel, so the fire module (when installed) can light and burn them.
  As they burn they char (scorched, then black alligator char with embers glowing in its
  cracks, smoldering, cold) and lose strength faster than fuel (the square of the fuel left),
  so a burning building sags piece by piece and comes down; fallen pieces burn on where they
  land.
- **Cracks** (`cracks`): hairline cracks run out from holes and craters across the intact
  faces of brittle walls (concrete, plaster, brick, stone), forking and tapering out.
- **Voxel volumes** (`VoxelVolume`): cliffs, arches and boulders made of 0.25 m rock voxels
  (signed distance, meshed with surface nets, colored with strata, streaks and topsoil).
  `TerrainEdit` digs, adds, smooths and paints them; explosions crater them. Rock that loses
  its connection to the ground (dig under an overhang, blow out an arch's foot) breaks into a
  few big chunks and falls.
- **Fracture** (`Fracturable`): props break into Voronoi pieces with real hulls on a big
  enough hit, and debris that lands hard shatters again. Fracture faces are rough (broken
  into small bumpy triangles that meet the piece's edges, `BreakLook`) and drawn in a
  joint-free broken material, so a shattered ashlar block shows broken stone, not mortar
  lines; glass and metal break clean.
- **Debris** uses the physics module when one is installed (physics-contract bodies) and a
  built-in solver otherwise: oriented boxes with friction, bounce and sleep that land on the
  ground and on voxel rock (not yet on each other), and wake when the rock under them is dug.
  Boulders cut from the rock itself (a cave-in, a blast's rubble) pass through rock onto the
  ground so they don't catch on the rim of the hole they fall out of. Chips and dust are
  cheap; the oldest debris sinks away past the budget (`max_debris`, `max_chips`).

What this module does not do: the big terrain heightfield belongs to the planets and terrain
module. Voxel volumes here are local destructible rock (cliffs, caves, arches) placed on it.

## Contracts

Reads: `DamageEvent` (bullets, melee, explosions; `target` picks the wall or piece),
`WorldImpulse` (explosions: energy falls off with distance), `ProjectileTrace` (bullets pass
through soft material and lose energy), `ContactEvent` (hard impacts, debris landing),
`TerrainEdit` (dig and build rock), `Combustible` state, the `build_*` input actions,
`BuildRequest` (script a builder).

Writes: `WorldImpulse` (falling structures, strength only), `NoiseEvent` (breaks, collapses),
`SmokeEmission` (dust and thermite smoke for the smoke module), `GroundContact` (debris
landing on a surface, for footstep-style impact sounds), physics-contract bodies for debris.

## Using it

```rust,ignore
// A drywall with a door you can kick in, and a reinforced wall that needs thermite.
commands.spawn((Transform::from_xyz(0.0, 0.0, 0.0), DestructibleWall::soft(4.0, 3.0)));
let wall = commands.spawn((Transform::from_xyz(6.0, 0.0, 0.0),
    DestructibleWall::reinforced(4.0, 3.0))).id();
commands.spawn(BreachCharge::thermite(wall, Vec2::new(0.0, 1.0), [1.1, 1.9]));

// A wooden post and a beam on it; break the post and the beam falls.
commands.spawn((Transform::from_xyz(0.0, 1.5, 0.0),
    StructuralPiece::new(DestructibleMaterial::Wood, Vec3::new(0.2, 3.0, 0.2))));
commands.spawn((Transform::from_xyz(1.2, 3.1, 0.0),
    StructuralPiece::new(DestructibleMaterial::Wood, Vec3::new(2.6, 0.2, 0.2))));

// A cliff to mine into.
let mut cliff = VoxelVolume::cliff(26.0, 12.0, 9.0, 0.25);
cliff.overhang = 2.4;
commands.spawn((Transform::from_xyz(0.0, 0.0, 20.0), cliff));

// Build mode on the player's camera: B toggles it, the mouse places and removes pieces.
commands.entity(camera).insert(Builder::player());
```

Anything else hits them through the world messages above: a gun writes `ProjectileTrace`, a
grenade writes `WorldImpulse`, a pickaxe writes `TerrainEdit`.

### AI functions

| Function | Does |
|---|---|
| `destruction.spawn_wall` | `{preset: soft\|barricade\|reinforced\|brick\|stone\|concrete, at, yaw_deg, size, thickness, material}` |
| `destruction.spawn_voxels` | `{preset: cliff\|arch\|boulder, at, yaw_deg, size, voxel_size, material}` |
| `destruction.shoot` | A burst of bullets `{from, to, speed, count, spread}` |
| `destruction.hit` | One impact `{at, dir, energy_j, melee}` |
| `destruction.blast` | An explosion `{at, energy_j}` |
| `destruction.breach` | Plant a charge `{wall, kind: explosive\|thermite, at, size, energy_j, fuse_secs, back}` |
| `destruction.dig` | Edit rock `{at, radius, op: remove\|add\|smooth\|paint}` |
| `destruction.repair` | Rebuild a wall or volume `{entity}` |
| `destruction.build` | Place a piece like a player in build mode: `{kind, from, to}` or `{kind, at}`, `yaw_deg`, `preview` |
| `destruction.unbuild` | Take down a built piece `{entity}` or the one aimed at `{from, to}` |
| `destruction.build_pieces` | The pieces build mode can place, with sizes |
| `destruction.inspect` | Integrity, cells, rebar, volume of one entity `{entity}` |
| `destruction.stats`, `destruction.clear_debris` | Counters; remove all debris |

Every function's error says what it expected and gives an example.

### Tuning

`DestructionSettings` (resource): debris budget and lifetimes, joules per point of damage,
bullet mass and efficiency, contact damage threshold, terrain craters, chunks per hit,
collapse delay, debris mode. `DestructionMaterials`: density, toughness, chunk size, colors and
support losses per material. Each wall, piece and volume has its own fields (thickness, cell
size, paint, rebar spacing, anchors, voxel size). All are registered with docs, so the AI
bridge and the editor can change them live.

## Test cases

`demo::spawn_demo(commands, origin)` builds the destruction range (`games/showcase` puts it in
the Siege fortress zone). It is about 80 × 40 m and runs a 20 s loop, then rebuilds:

- **Seven breach bays.** Five show the aftermath from the start: drywall shot to pieces with
  a vault hole; a drywall with a kicked-in door barricade and a window barricade; a reinforced
  wall with a thermite doorway; a reinforced wall after an explosive (concrete gone, rebar
  standing); a brick wall with a blast hole. Two run live: thermite burning through reinforced
  concrete, and a drywall shot up and then blown open by a charge.
- **Valheim tower.** The support colors come on, then the posts break one by one and the tower
  comes down.
- **Gate** of stone blocks blown apart.
- **Cliff** mined into until the overhang drops.
- **Rock arch** whose foot is blown out until it falls.
- **Burning storehouse** (`barn`): a timber shed in the courtyard, lit from a woodpile inside,
  that chars, sags and burns down on its own 120 s loop (needs the fire module; the showcase
  has it, and gives the shed a straw floor in a bare yard ringed by green grass so the fire
  stays a house fire). `--example burn` renders it with a stand-in fire (char and collapse,
  no flames).

`cargo run -p engine_destruction --release --example fortress -- <out_dir>` renders all of it
headless to 22 screenshots.

`hill::spawn_voxel_hill(commands, origin)` builds the dig-and-build hill (the showcase's voxel
hill zone) on a 28 s loop: a tunnel dug into the hill until its hall caves in, a terrace raised
and leveled with the hoe, a timber hut in support colors losing two posts, and the **build
yard**, where a builder in build mode puts up a cabin piece by piece, each ghost snapping into
place before the piece goes down: a stone plinth (blue), corner posts and board walls with a
doorway, a loft floor, a shingle roof with board gables closing its ends, and a deck reaching
out from the +X gable whose second row is orange at the edge of support, then the red ghost
of a third row that couldn't stand.
Then the hoe levels a floor pad in the terrace and a second builder puts up a **shelter on the
rock**: a stone floor laid on the bare rock (blue, because rock is ground), board walls at the
ends, a shingle roof, and the back walls and their gable against the cut bank, open toward
the valley.
`--example hill -- <out_dir> [WxH] [only]` renders it (`only`: `build` for the yard,
`shelter` for the terrace).

## Tests

- Unit tests per module (cell grid, energy model, rebar, support graph, Voronoi, surface nets).
- `tests/plugin.rs`: the done-when cases end to end through the plugin (bullets through a
  barricade, thermite through reinforced concrete, a charge on drywall vs reinforced, a tower
  falling when its post breaks, digging under an overhang drops it, the AI functions, the demo,
  a burning shed charring and coming down, build mode placing snapped pieces and taking them
  down, the hill's cabin going up with its red ghost and its shelter standing on the terrace's
  rock, pieces on voxel rock falling when it is dug out from under them).
- `hill::tests::the_yard_cabin_snaps_together_as_planned` and
  `the_terrace_shelter_snaps_together_as_planned` aim every step of the cabin and the shelter
  and check each piece lands where it should with the support the solver gives it.
- `tests/headless.rs`: renders a cut wall offscreen and checks the doorway shows.

## Done when

- Breach a reinforced wall with a charge; shoot holes in a wooden barricade; dig and collapse a
  voxel cliff. **Done** (tests above and the showcase).

Next: debris resting on other debris without a physics module, cave-in boulders resting on
rock ledges (they need side contacts, not just floors), doors and windows in walls.
