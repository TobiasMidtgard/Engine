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
- **Floors, roofs and hatches** (Siege vertical play) are the same walls laid flat
  (`DestructibleWall::flat()`): `floor` is 5 cm boards between joists, `hatch` a wooden hatch
  cover, `steel_hatch` a steel plate over one. Bullets fired from below punch up through
  boards, a charge on top blows a hole to drop through and what breaks loose falls into the
  room below; a sledgehammer smashes a wooden hatch in, while a steel one only dents until
  thermite cuts it out. Built openings (`with_opening`: doorways, windows, hatch holes) come
  back every time a wall is rebuilt.
- **Breach charges** (`BreachCharge`): an explosive charge blows a hole through soft walls; a
  thermite charge burns around its edge (sparks, smoke, glowing strip), cutting rebar too, and
  the panel inside drops out at the end.
- **Reinforcement** (`reinforce`, Siege style): a `ReinforceRequest` sets steel panels against
  a wall's back, one per 2.8 m of width. Each is a 4 cm steel `DestructibleWall` on a bolted
  frame (worn paint, a yellow kick member, lugs and handles), carried in, pressed home and
  braced by two yellow hydraulic struts that swing down off its back onto the floor. Bullets
  that get through the wall ring off the steel, a breach charge strips the wall and only
  scars the panel, and thermite on the wall burns through the panel behind too. Panels go
  when their wall goes.
- **Building pieces** (`StructuralPiece`, Valheim style) carry support from the ground up:
  every piece gets the best support any path to a grounded piece gives it, losing some per
  meter up and more per meter sideways, by material (wood carries far less sideways than
  stone). Break a post and whatever depended on it falls after a short creak. `ShowSupport`
  paints pieces blue (grounded), green, yellow, orange and red (about to fall), like Valheim's
  build mode. With a physics module, standing pieces are solid: each gets a static collider
  (its box a skin smaller, a gable's triangle, a stair's sloping slab whose top is a ramp
  through its treads) with its material's surface, so you walk on floors and decks and up
  stairs and walls stop you, debris lands on what still stands, and a car that hits
  a wall breaks it. A piece that falls becomes a debris body.
- **Build mode** (`build`, Valheim's hammer): a `Builder` (the player's camera, a script)
  picks one of thirteen pieces (wood floor, wall, pole, beam, 26° and 45° shingle roofs, a
  26° board gable to close a roof's end, a wall with a doorway and one with a window, a
  flight of stairs rising 1.2 m over 2 m in seven steps; stone floor, wall and pillar) and
  aims. A translucent ghost of the piece sits on what the aim hits
  (on top of the ground or a floor, against a wall's side or a floor's edge) and snaps so its
  nearest snap point meets a neighbor's (pole and beam ends, panel corners and edge midpoints,
  a floor's on its top face, block corners, a stair's foot and head, so flights meet head to
  foot and a head meets a floor's edge: two flights climb from the ground to a floor over a
  0.4 m stone plinth), skipping spots inside other pieces. The ghost
  shows the support it would get in the support colors, or a deep red where it would sit
  inside something, touch nothing, reach too far from support to stand, or (with a physics
  module) close around someone standing there; place it, turn it
  in 22.5° steps, or take built pieces down (what they held falls). Pieces stand on the
  terrain or on voxel rock alike (dig the rock out from under them and they fall). Built
  pieces show their support colors while their builder has the support view on. Keys (bound
  unless the game binds them): B build mode, left mouse place, X remove, R rotate, C next
  piece, 1 to 9, 0, −, = and [ pick a piece.
- **Stairs** (`BuildKind::WoodStair`, `props::wood_stair`): seven plank treads with rounded
  nosings across the full width (open risers, each tread a little over the one below), nailed
  down onto three cut stringers: a level seat sawn for each tread and a plumb riser under its
  nosing, a hand's depth of wood left under the notches, grain up the pitch, level where they
  sit and plumb at the head, so two flights built head to foot read as one stair. Boots have
  dished and darkened the treads' middles. Real stair proportions: 171 mm rise, 286 mm
  going.
- **Doors** (`door`): a ledged and braced plank door (iron strap hinges, clenched nails, a
  ring pull on each face) hangs in every doorway built in build mode. `interact` (E, gamepad
  West) opens or shuts the door nearest a character within 2.2 m; it swings 95° away from
  whoever opens it, either way, and with a physics module it is a kinematic body that swings
  with it (shut, it closes the doorway; swinging, it shoves crates out of its way). When its
  doorway falls the door swings shut and falls with it; take the doorway down and the door
  goes too.
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
  lines; glass and metal break clean. With a physics module they stand solid (their box or
  column) until they break, and set dressing (`ArchModel` masonry, a `PropModel` on nothing
  that breaks) is solid as drawn, a static triangle mesh of its model: the fortress's walls
  stop you and you can walk along their wall walks.
- **Debris** uses the physics module when one is installed (physics-contract bodies for building
  pieces that give way and chunks of `physics_min_kg` and up, at most `max_physics_debris` at
  once, heaviest first) and a
  built-in solver otherwise: oriented boxes with friction, bounce and sleep that land on the
  ground, on voxel rock and on each other, and wake when the rock or the chunk under them
  goes. A chunk at rest is a box the rest land on (sampled box on box, so a slab lies across
  the edge of one below rather than tipping through it), so a collapse leaves a heap. Rock
  floors hold debris up like the ground (even a roof one voxel thick), while rock walls and
  ceilings push it out of their face (toward the nearest open voxels), so a boulder slides
  down a cliff onto the ledge below. Rock cut loose (a cave-in's roof, a blast's rubble)
  first drops out of its own hole through the rock around it, so it never hangs on the
  hole's rims, then lands on rock like the rest: on a ledge, or on a cave's floor above the
  ground. Chips and dust are cheap; the oldest debris sinks away past the budget
  (`max_debris`, `max_chips`).

What this module does not do: the big terrain heightfield belongs to the planets and terrain
module. Voxel volumes here are local destructible rock (cliffs, caves, arches) placed on it.

## Contracts

Reads: `DamageEvent` (bullets, melee, explosions; `target` picks the wall or piece),
`WorldImpulse` (explosions: energy falls off with distance), `ProjectileTrace` (bullets pass
through soft material and lose energy), `ContactEvent` (hard impacts, debris landing),
`TerrainEdit` (dig and build rock), `Combustible` state, the `build_*` and `interact` input
actions, `BuildRequest` (script a builder), `DoorRequest` (open or shut a door),
`ReinforceRequest` (reinforce a wall).

Writes: `WorldImpulse` (falling structures, strength only), `NoiseEvent` (breaks, collapses,
doors, reinforcement going up),
`SmokeEmission` (dust and thermite smoke for the smoke module), `GroundContact` (debris
landing on a surface, for footstep-style impact sounds), physics-contract bodies for debris
and static colliders for standing building pieces, breakable props and set dressing, and
kinematic bodies for doors (moved by their `Transform`), and `Climbable` on the breach house's
ladders (climbed by the movement module, linked by NPC navigation).

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
| `destruction.spawn_wall` | `{preset: soft\|barricade\|reinforced\|brick\|stone\|concrete\|floor\|hatch\|steel_hatch, at, yaw_deg, size, thickness, material}` (floors and hatches lie flat) |
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
| `destruction.reinforce` | Set steel panels behind the wall nearest `{near}` (within 3 m) or `{entity}`; returns the wall and how many panels |
| `destruction.door` | Open or shut the door nearest `{near}` (it swings away from there) or `{entity}`; `open` true, false or left out to toggle |
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

- **Eight breach bays.** Five show the aftermath from the start: drywall shot to pieces with
  a vault hole; a drywall with a kicked-in door barricade and a window barricade; a reinforced
  wall with a thermite doorway; a reinforced wall after an explosive (concrete gone, rebar
  standing); a brick wall with a blast hole. Three run live: thermite burning through
  reinforced concrete; a drywall shot up and then blown open by a charge; and a drywall
  reinforced while it is being shot (bay 8, west end): the panels go up behind it, the next
  bursts stop on the steel, a charge strips the drywall and leaves the panels standing, and
  thermite burns a doorway through wall and panel together.
- **Breach house** (vertical play, north of the tower): a brick room with an open front and a
  window in each side wall under a timber roof deck on joists, with a stair up its back wall
  to the roof and a climbable steel ladder under each of its two rimmed hatches. Galvanized
  tube-and-clamp guard railings (`PropKind::PipeRailing`: cast clamp fittings, base flanges,
  a timber toe board) run round the roof, open at the stair's head, and up the stair's open
  side on palms bolted to its stringer. Its west roof bay was
  shot up from below long ago, so sunlight comes through the holes; every loop the room fires
  up through the east bay, a charge on the roof blows a hole and the boards rain in, thermite
  cuts the steel hatch out and its plate drops onto the floor, and a sledgehammer smashes the
  wooden hatch in.
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
headless to 36 screenshots (`house` for the breach house's, `reinforce` for bay 8's).

`hill::spawn_voxel_hill(commands, origin)` builds the dig-and-build hill (the showcase's voxel
hill zone) on a 28 s loop: a tunnel dug into the hill until its hall caves in, a terrace raised
and leveled with the hoe, a timber hut in support colors losing two posts, and the **build
yard**, where a builder in build mode puts up a cabin piece by piece, each ghost snapping into
place before the piece goes down: a stone plinth (blue), corner posts and board walls with a
window in each side, a loft floor, a shingle roof with board gables closing its ends, a
framed doorway in the front whose plank door then swings open, and a deck reaching
out from the +X gable whose second row is orange at the edge of support, then the red ghost
of a third row that couldn't stand. Then the builder puts the support view away and builds
**stairs** up to the deck's end: a flight on the ground, and one whose foot snaps onto its
head and whose head snaps onto the deck's edge.
Then the hoe levels a floor pad in the terrace and a second builder puts up a **shelter on the
rock**: a stone floor laid on the bare rock (blue, because rock is ground), board walls at the
ends, a shingle roof, and the back walls and their gable against the cut bank, open toward
the valley. Then a pickaxe digs a pit under it: under one floor slab first (the other slab
still holds it, red), then under both, and the shelter drops into the pit.
`--example hill -- <out_dir> [WxH] [only]` renders it (`only`: `build` for the yard,
`stairs` for its stairs, `shelter` for the terrace).

## Tests

- Unit tests per module (cell grid, energy model, rebar, support graph, Voronoi, surface nets).
- `tests/plugin.rs`: the done-when cases end to end through the plugin (bullets through a
  barricade, thermite through reinforced concrete, a charge on drywall vs reinforced, a tower
  falling when its post breaks, digging under an overhang drops it, the AI functions, the demo,
  a burning shed charring and coming down, build mode placing snapped pieces and taking them
  down, the hill's cabin going up with its red ghost and its door swung open and its shelter
  standing on the terrace's rock, pieces on voxel rock falling when it is dug out from under
  them, a slab dropped onto a resting one lying on it and falling when it goes, boulders
  landing on a rock ledge, sliding off a rock wall onto it and resting on a roof one voxel
  thick, rock cut loose dropping out through a hole narrower than itself onto a cave's
  floor, doors opening away from whoever presses interact and falling with their doorway,
  stairs built up to the cabin's deck and their ramp-shaped colliders, a floor shot through
  from below and blown open from above, hatches smashed in or cut out while steel stops
  bullets and charges, built openings surviving a rebuild, the breach house's loop,
  reinforcement panels set and braced behind a drywall that stop bullets and a charge until
  thermite burns through wall and panel, and the AI call reinforcing the wall nearest a
  point).
- `hill::tests::the_yard_cabin_snaps_together_as_planned` and
  `the_terrace_shelter_snaps_together_as_planned` aim every step of the cabin and the shelter
  and check each piece lands where it should with the support the solver gives it.
- `tests/headless.rs`: renders a cut wall offscreen and checks the doorway shows.

## Done when

- Breach a reinforced wall with a charge; shoot holes in a wooden barricade; dig and collapse a
  voxel cliff. **Done** (tests above and the showcase).

Next: build mode in play mode (it needs an input-focus contract, so build keys and the
player's keys don't both act).
