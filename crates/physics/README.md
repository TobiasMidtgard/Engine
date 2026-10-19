# engine_physics — Physics

**Owner:** Physics and rigid bodies. **Status:** rigid bodies, colliders, joints, characters,
scene queries and planet-scale regions work on Rapier 0.36 and pass the contract's
conformance suite. Background research: `game-engine-research/physics.md` in the project files.

```rust,ignore
app.add_plugins(engine_physics::PhysicsPlugin); // the only line a game needs
```

Games and other modules never see Rapier: they use the `engine_world::physics` contract
(`RigidBody`, `Collider`, `Velocity`, `ExternalForce`, `Joint`, `CharacterController`,
`ApplyImpulse`, `ContactEvent`, `PhysicsQueries`). Swap this crate for another backend by
changing that one plugin line.

## What it does

- **Bodies**: dynamic, kinematic, static. A kinematic body moves by `Velocity`, or by writing
  its `Transform`: it then glides there over the step, carrying what stands on it (a train car,
  a door swung by gameplay); writing the `Transform` of one moving by `Velocity` jumps it there
  and it goes on from there. Mass from
  collider densities or `RigidBody::mass_kg`; a designed `center_of_mass` and `inertia` (a car's
  low balance point, a plane's trim) override what the shapes imply, and a moved center of mass
  keeps the shapes' inertia about it. Damping, gravity scale, CCD, sleeping, rotation locks.
  `MassProperties` is written back.
- **Colliders**: sphere, cuboid, capsule, cylinder, convex hull, triangle mesh, heightfield,
  compound; on the body entity or on its children (one compound body). Shapes scale with the
  entity's `Transform::scale`. Changing a `Collider` rebuilds it. Friction and restitution come
  from `SurfaceMaterials`, layers from `CollisionLayers`, sensors report overlaps.
- **Rolling resistance**: balls, barrels, logs and capsule limbs slow down and stop like real ones
  (Rapier has none, so they would roll forever). Each rolling contact pushes back with a torque of
  `Crr · N · r`: `rolling_resistance` on hard ground, more on mud, sand, snow and grass (from the
  surface's deformation, the same amounts the vehicle tires use), alike on boxes, terrain and
  meshes. Boxes tipping over an edge are left alone. The contract's default `angular_damping`
  (0.05) slows rolling a little too. Fast-rolling balls, barrels and wheels roll flat and
  straight over terrain and meshes: Rapier judges an early ("speculative") contact by how fast
  the point of the body there moves, and on a body spinning about its own axis the point just
  ahead of the one it rests on dives at the ground although the surface doesn't move. Every
  new triangle brought such a contact and a kick (a barrel at 8 m/s jumped 20 cm and veered
  meters off its line). A contact hook takes the spin's share out of each contact's closing
  speed, for colliders spinning that fast only, so piles keep Rapier's contact reuse.
- **Spin friction**: a ball spinning on its spot, or a ragdoll's head, slows and stops (Rapier
  only resists spin for flat contacts with several points, and never for articulation links).
  Each such contact takes up to `μ · N · 1 cm` of torque out of the spin about its normal; spin
  under 5% of the roll is left alone, since a normal a little off on terrain shows that much of
  a fast roll as spin (it steered rolling barrels). Only awake bodies' contacts are walked, so a
  settled pile costs nothing here.
- **Joints**: fixed, spherical (cone limit), revolute and prismatic (limits, motors), distance
  (rope), spring. `break_force` breaks a joint: the joint entity is despawned (or the `Joint`
  removed from a body entity). With `collide_connected: false` the two never touch, also when
  one side is a fixed body (a door hinged inside its frame, a lid on a static box). Angles count
  from the pose the bodies had when the joint was first made: a fixed joint welds them as they
  were, a hinge's limits and motor targets are relative to it.
- **Cone limits** (`src/cones.rs`) are round, around the direction body B's center lay from the
  joint when it was made (an arm may swing `cone_limit` away from where it hung), and leave
  twist free. Rapier limits each angle on its own, a square cone that mixes swing and twist; so
  before every substep the joint frames turn to the plane the limb swings in, and only that
  angle is limited. A limb pressed against its cone's rim rubs there, with friction in
  proportion to how hard the limit pushes back (half of it).
- **Joint friction** in free-floating groups (ragdolls, loose chains): up to 10% of the torque
  the lighter body's weight puts on the joint from its center (its weight, on a slider), like a
  real joint's. Without it a bent arm sways about its free twist for minutes; with it, 288 drops
  of a ragdoll all came to rest and fell asleep (about one in eight never did before), in about
  2.5 s. Axes a motor drives get none; joints pinned to the world or a kinematic body (doors,
  bridges, wrecking balls, lamps) get none either, so they swing on.
- **Articulations** (`src/articulation.rs`): the fixed, spherical, revolute and prismatic joints
  of a group of dynamic bodies jointed together are solved in reduced coordinates (Rapier
  multibodies): each body only moves along its joint's free axes under its parent, so a ragdoll
  or a loose chain never opens up at a joint, however hard it lands (constraint joints gapped up
  to 54 mm for a frame, and 1–3 mm at rest, as the proving ground's dummies tumble down the
  stairs; articulated, 0 every frame: `cargo run -p engine_physics --release --example joint_gap`
  measures both and writes them to a CSV). Everything
  a body gets still works: a written pose sets the joint coordinates (moving one part of a
  ragdoll far, or its root, moves the whole ragdoll; a small move bends the joints), written
  velocities, impulses, character pushes and blasts act through the articulation's mass matrix
  (an impulse on a hand moves the whole ragdoll by its momentum), damping is each body's, and
  world shifts, region changes, sleep and joint removal (the articulation splits) carry on. A
  cone limit, a joint that would close a loop, breakable joints, ropes and springs stay
  constraint joints inside the group. Two kinds of group keep constraint joints throughout,
  because Rapier's articulations blow up there: groups pinned to the world, a static or a
  kinematic body (reduced coordinates can't give way to a contact they can't satisfy: a bridge
  plank pressed against its tower), and groups whose heaviest body is over 100× its lightest
  (a 17 t wrecking ball on 12 kg links hitting a wall). Pinning or unpinning a group, or joining
  or dropping a heavy body, switches it over on the spot. Cost: Rapier rebuilds each
  articulation's dense mass matrix every internal substep, about 0.9 ms per tumbling 10-part
  ragdoll per frame on the build machine (constraint joints: 0.15 ms). Asleep they cost next to
  nothing: the engine leaves an untouched sleeping articulation alone (Rapier still redoes its
  forward kinematics every step, about a microsecond each). So only the `max_articulated` (4)
  moving groups nearest a `SimulationFocus` are articulated; the others use constraint joints,
  and switch over when they rank among the nearest (a group already articulated counts as a
  fifth nearer, so groups at about the same distance don't trade places back and forth;
  sleeping groups keep their joints and don't count). Ten ragdolls tumbling at once cost 4.8 ms
  per frame instead of 8.3. `articulated_joints: false` turns it all off.
- **Characters**: `CharacterController` capsules moved by sweeps, so even an 800 m/s dash stops
  at a thin wall: steps (also while gravity pulls), slopes, ground snapping, crouching (change
  `height` between steps). They move after the bodies have stepped: whatever a character stood
  on (a lift, a turntable, a seesaw, a ship at 30 km/s) first carries it by exactly how far it
  moved and turned, then the character makes its own move. Characters push what they walk into
  (relative to what they ride, so a ship's speed never leaks into a push), press on what they
  stand on with their weight, and stop falling bodies like any kinematic body. `CharacterMove`
  is in the character's own seconds; `CharacterState::velocity` is the character's own motion,
  without the platform's. `CharacterState` reports ground, ceiling and wall contacts, including
  walls it only touches (running along a wall). Its normals come from exact point projections
  between the capsule's core and the surface, not from the sweeps: those point sideways about
  one step in a hundred while the capsule sits in its 2 cm gap, a wall in the middle of a field.
  A falling character reads `grounded` only once it is on the ground (within 5 cm), not while
  ground is merely within snapping reach below it.
  Capsules against boxes are solved exactly (`src/capsule_box.rs`) for characters and
  `physics.shapecast`: Parry's GJK let a character sink 20 cm into a 25 m box when it stood
  within a few millimeters of the box's center, and meters into a 500 m one. Characters now
  keep their 2 cm gap on boxes from 1 m to 10 km across. A character running along a wall it
  presses into keeps its whole slide, also when the wall moves: a cast that starts touching a
  surface and moves along it is not a hit (it used to stop the slide every other step on a
  kinematic wall).
- **Events**: one `ContactEvent::Started` per touch with the impact impulse and a normal from
  `a` to `b`, one `Ended` when it stops; sensors too. Hard impacts also send `NoiseEvent` (AI
  hearing) and, against static ground, `GroundContact` (dents, dust, impact sounds).
- **World inputs**: `Gravity` (uniform or planets; each body gets the gravity at its own
  position), buoyancy and drag in `Water`, `WorldImpulse` blasts (blast wind drag), `ApplyImpulse`.
  Buoyancy integrates water pressure over each collider's real shape (boxes, balls, capsules,
  cylinders, cones, convex hulls and compounds of them; `src/buoyancy.rs`), clipped at the
  waterline sampled every 2 m under the body, so floating bodies get the righting moments real
  ones do: a log lies down, a half-density cube floats on its edge, a 0.9-density cube floats
  flat, a barge pitches on a passing swell. Heave and rocking are damped across the wetted
  bottom (half of critical), so floaters settle instead of bobbing forever; water forces are
  recomputed every substep. Turning drag is per axis, by how far the shape reaches across it
  (a log rolls easily, end over end it drags its whole length). Other shapes (meshes,
  heightfields) use the submerged share of their bounding box. Sleeping bodies notice the water
  changing: every 6 steps the water under each one is compared with what it was when the body fell
  asleep, and a body the water reaches wakes once its level moved 2 cm or its current 0.1 m/s, so
  a plank rides a slow tide up, a crate floats off a flooded quay and a floater drops with the
  water when a sluice opens. A body afloat in a current is kept awake while it touches nothing
  fixed, so it keeps drifting even below Rapier's sleep speed; a stone on the bed sleeps on, and a
  body the water stays clear of, or stays well over, is never woken by waves.
- **Time**: runs in `FixedUpdate` / `SimSet::Physics` on world time, so `TimeDilation` slows it;
  each fixed step is split into substeps of at most `max_substep_secs`, so 10× fast-forward is
  as stable as 1×. `LocalTimeScale` speeds a single body up or down, in everything: its
  `Velocity` is in its own seconds (like `CharacterMove`), and gravity, `ExternalForce`, water
  and damping act per second of its own time, so a car on thrusters in a slow-motion bubble
  hovers just as it did outside, and a lift driven by `Velocity` moves at the bubble's pace.
- **Queries**: `PhysicsQueries` raycast, shapecast, overlap and closest point, from any system
  or thread; they see the state after the last step.
- **Deterministic**: the same scene gives the same result every run.

## Regions: planet-scale precision

Rapier works in `f32`, which is millimeter-precise only within a few kilometers of its origin.
Every moving thing lives in a *region*: its own Rapier world whose origin is an `f64` anchor
near the bodies in it. A crate on the Moon and a car on Earth are in different regions and both
simulate at full precision (a scene 384 000 km out matches the same scene at the origin to
under a micrometer).

- A moving body joins the nearest region within `region_radius_m / 2`, or starts one.
- Regions re-center on their bodies as they travel; a body (with everything jointed to it)
  that leaves its region's radius moves to another region; two regions whose bodies come close
  merge so they can collide.
- Static colliders are copied into every region that could touch them (the shape data is
  shared); a region travelling with its bodies drops the copies it leaves behind, so a ship
  crossing a continent holds only the statics near it. Regions far from every `SimulationFocus` pause (`freeze_distance_m`).
- A `WorldShift` (the space module re-anchoring the world) moves the simulation in the same
  frame. A floating-origin move only moves region anchors: sleeping bodies stay asleep and
  contacts carry on. A turn into another planet's frame rotates each region's contents about
  its anchor, world-pinned joints included; a moving frame adds its velocity to every body.

## Seeing what physics sees (`PhysicsDebug`)

With `engine_debug::DebugPlugin` in the app, `PhysicsDebug` (a registered resource, also
`physics.debug {"all": true}`) draws over the scene, within `radius_m` (150 m) of the focus or
camera:

- `colliders`: wireframes, green awake, blue-gray asleep, yellow kinematic, gray static,
  magenta sensors, cyan characters (big meshes and heightfields as their bounds).
- `contacts`: touching points with an arrow along the normal that grows with the force.
- `joints`: both anchors joined by a line, orange turning red as a breakable joint nears its
  break force or the anchors pull apart; a ring marks a world pin.
- `regions`, `centers_of_mass`, `velocities` (where each body will be in 0.25 s).

`PHYSICS_DEBUG=1 cargo run -p engine_physics --release --example proving_ground` renders the
proving ground with the overlay.

## Settings (`PhysicsSettings`, editable with `resource.patch`)

| Field | Default | What it does |
|---|---|---|
| `solver_iterations` | 4 | Iterations per substep; tall stacks and long chains like 6–8. |
| `max_substep_secs` | 1/120 | Longest substep. With constraint joints, 1/60 lets ragdoll joints gap ~5 cm on impact; 1/120 ~1.5 cm. |
| `articulated_joints` | on | Solve free-floating jointed groups as articulations (no joint gap; see Articulations). |
| `max_articulated` | 4 | Most moving groups articulated at once, nearest the focus first; the rest use constraint joints. |
| `max_substeps` | 24 | Cap per fixed step (10× speed needs 20). |
| `allowed_penetration_m` | 0.001 | How far contacts may sink before being pushed apart. |
| `contact_stiffness_hz` | 60 | Contact spring frequency (static contacts 2×). 30 Hz lets a 30-box tower sag 24 cm, 60 Hz ~6 cm. |
| `region_radius_m` | 1000 | Size of a region. |
| `freeze_distance_m` | 25 000 | Pause regions this far from every focus (`None` = never). |
| `rolling_resistance` | 0.01 | Rolling resistance on hard ground (rubber on concrete); soft surfaces add more, 0 = roll forever. |
| `max_speed` | 100 000 m/s | Velocity clamp (spaceships are fast). |
| `contact_events`, `impact_events` | on | Send `ContactEvent` / impact `NoiseEvent` + `GroundContact`. |
| `impact_min_speed` | 1.5 m/s | Smallest speed change that counts as an impact. |
| `buoyancy` | on | Float and drag bodies in `Water`. |
| `blast_response`, `blast_scale` | on, 1 | Explosions push bodies; 1 = real blast wind, action games use 3–10. |

`PhysicsStats` (resource, also `physics.*` in `Diagnostics`): bodies, awake bodies, regions,
touching pairs, step time, substeps, articulated joints, static copies.

## API functions

`physics.raycast`, `physics.shapecast`, `physics.overlap`, `physics.closest_point`,
`physics.impulse`, `physics.blast`, `physics.stats`, `physics.debug`,
`physics.proving_ground.replay`. `engine.describe` lists their arguments.

## Contract details a replacement backend must match

- `ColliderShape::Heightfield` heights are row-major, rows along Z and columns along X,
  centered on the collider, spanning `size = [x, z]`.
- `ContactEvent::impulse` is the contact's total normal impulse in its first touching step.
- Joint break force is the constraint force, N (Rapier reports impulses per solver iteration;
  this crate converts).
- Joint angles count from the bodies' relative pose when the joint is first made (a fixed joint
  keeps it; revolute and prismatic limits and motor targets are relative to it), and that rest
  pose survives the joint being remade (region changes, `articulated_joints` toggled).
- Free-floating jointed groups never open at their joints (the articulated tests check under
  1e-4 m); a replacement backend without articulations should at least keep them within 2 cm.

## Demo and tests

`demo::spawn_demo(commands, origin)` builds the **physics proving ground**: a 70 × 50 m concrete
yard (with a curb, so nothing rolls off) with a 30-crate tower, a bin filling with 250
crates/barrels/balls, a domino snake, ten crash test dummy ragdolls tumbling down a steep staircase together,
a wrecking ball smashing a crate wall, a seesaw catapult, a motor-driven paddle wheel, a sagging
plank bridge, a kinematic turntable, bouncing rubber balls and, along the front, a **barrel run**:
three oil drums roll off a 2 m ramp and 47 m down an asphalt runway (one triangle mesh, an edge
every 2 m) into a sand pit, staying flat and in their painted lanes, past galvanized guardrails to a
tire wall. It sits on the ground at `origin`, its top `demo::SLAB_RISE` (10 cm) above it; the
showcase's physics zone places it. Render it alone with
`cargo run -p engine_physics --release --example proving_ground -- out_dir`.

The stations replay on their own cycles (`demo::Station::cycle_secs`, world time: ragdolls,
seesaw and balls every 20 s, the barrel run every 25 s, dominoes, wrecking ball and bridge every
30 s, the pile every 45 s; the tower, wheel and turntable run on), so the zone is always
playing when someone walks in.
They pause while no focus point is within 250 m.
`physics.proving_ground.replay {"station": "dominoes"}` restarts one now (`{}` restarts all).
`demo::VIEWS` are tour stops for the action stations: arriving restarts the station, and the
warm-up lands on its best moment. `tests/proving_ground.rs` checks that stations replay without
piling up bodies, that the barrel run's drums stay within 1 cm of the deck and 30 cm of their
lanes and stop in the sand, and that the ten dummies all leave the landing and are asleep well
before their replay (about 6.6 s; four tumble articulated, six on constraint joints).

Everything in it is drawn with detailed procedural art (`src/demo/art/`): beveled meshes sized to
their colliders (shipping crates with brackets and bolts, 200 l drums, chain links, a cast
concrete stair with handrails, a steel gantry, glulam and diamond-plate machines, Hybrid III-style
dummies) and PBR texture sets (albedo, normal, roughness/metal/occlusion) generated at startup,
about a second in release builds (debug builds make them a quarter the size). Apps without a
renderer get colliders only. `cargo run -p engine_physics --release --example art_props`
(`art_structures`, `art_dummy`, `art_textures`) previews each part on its own.

`demo::spawn_floaters(commands, at)` puts the **float test** on any water (`at` on the surface,
about 14 × 8 m of open water at least 2 m deep): eight things dropped in, each settling as the real
one does. A 2.4 m timber post dropped on end topples and floats rolled onto a corner; a raft of
five such posts floats flat; a half-density crate tips onto an edge, 45° over; a waterlogged crate
(0.9 of the water's density) floats flat with 8 cm showing; a plank thrown in on its edge lands
flat; a light ball rides high; a sealed empty drum floats on its side; a steel ball sinks. It drops
again every 40 s of world time (`demo::FLOAT_CYCLE_SECS`) while someone is within 250 m.
`cargo run -p engine_physics --release --example float_test -- out_dir` renders it in a concrete
pool; `tests/floaters.rs` checks each result.

Tests (`cargo test -p engine_physics`): the conformance suite; the "done when" scenarios below
(`tests/scenarios.rs`); articulations (`tests/articulation.rs`: an impulse on one link moves the
whole by its momentum, written velocities and poses come through, cone limits hold and are
round, a limb twisted half a turn stays twisted, a limb swung along its cone's rim stops, hinge
motors and limits match constraint joints, fixed joints weld bodies as they were, world shifts, a chain
flying into another region stays whole, removing a joint splits it, switching the setting
rebuilds joints, pinned chains and far heavier bodies keep constraint joints until let go, the
budget articulates the groups nearest the focus and follows it); behaviors other modules rely on (`tests/behaviors.rs`: removal, compound
bodies, joint breaking and motors, hinges swinging through the fixed body they hang on, kinematic
platforms, stairs and steep slopes, dashing into thin walls, wall-running, no phantom walls on flat
ground, running over heightfield hills, crouching, landing from a fall, standing exactly on boxes
from 1 m to 10 km across, characters on their own clock, sensors and layers, local time (forces, damping and lifts
on a slow clock too), hovering,
buoyancy, floating the way real bodies do (logs, planks, cubes on their edge, a ball's draft, a
barge on a swell), blasts, bodies crossing and merging regions, a region dropping the statics it travels past); the float test
(`tests/floaters.rs`); riding moving bodies (`tests/platforms.rs`:
crates and characters on a deck at up to 30 km/s, lifts, a turntable, a thrusting and turning ship,
a seesaw tipped by a character's weight, crates landing on heads, rays hitting bodies at orbital
speed); mass overrides (`tests/mass.rs`: a moved center of mass, dropping an override); following
world shifts (`tests/world_shift.rs`: a sleeping pile stays asleep through floating-origin moves, a
stack stands through a tilting turn, world-pinned hinges turn with the world, a moving or spinning
frame adds its motion at each body, characters keep their footing, queries see the new frame at
once); rolling resistance (`tests/rolling.rs`: balls and logs slow at `Crr·g/(1 + I/mr²)` on
concrete, faster on grass and sand, the same on terrain and meshes, barrels stop on sand, barrels at
4 and 8 m/s roll over terrain and meshes within 5 mm of flat and 25 cm of straight, a ball spun
on its spot slows at `μ·g·1 cm/(⅖r²)` and stops where it is); the debug view
(`tests/debug_draw.rs`, contacts on terrain included); a headless screenshot of the proving ground.

## Done when

- [x] A 1 000-body pile settles without jitter at 60 Hz (≈5 ms per step on the build machine);
  a 30-box tower stands still.
- [x] A ragdoll and a chain of jointed bodies stay stable under slow motion and at 10× time
  scale (the ragdoll's joints hold exactly, the chain hung from the world within 2 cm; the
  solver adds no energy). Ragdolls knocked over every which way all fall asleep, articulated
  or not (`ragdolls_knocked_over_every_which_way_fall_asleep`).
- [x] A body 1e8 m from the origin behaves exactly like one at the origin.
- [x] `engine_world::physics::conformance::assert_all` passes in this crate's tests.
- [x] Raycasts/shapecasts available as API functions (`physics.raycast`).

## Next

- Cheaper articulations: Rapier rebuilds the whole dense mass matrix every internal substep
  (63% of a tumbling ragdoll's step). Skipping the zero columns of each link's Jacobian (a
  prototype patch of Rapier) saved only 30%, the rest being its dense LU factorization and
  constraint setup, so not worth a fork; an O(n) articulated-body solve would make many ragdolls
  at once affordable, and with contacts that soften where the coordinates can't give way, chains
  hung from the world (bridges, wrecking balls, lamps) could be articulated too.
- Joint friction and twist limits as `Joint` fields, so a game can make a stiff old door or a
  loose ragdoll (today every free-floating group's joints get the same 10%).
- A `rolling_resistance` field on `SurfaceMaterial`, so games can tune it per surface (today
  it comes from the surface's deformation).
- Co-moving region frames, so cargo resting in a ship at 7 km/s can sleep (today bodies moving
  together at speed stay awake; they ride correctly, they just cost CPU).
- A `Ground` query backed by static colliders for levels without terrain.
- Buoyancy along the local "up" on planets: today it measures depth along world Y, as the
  `Water` contract's `surface_height` does (near the focus the space module turns the world so
  local up is +Y, so this only matters far from it).
