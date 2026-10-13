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

- **Bodies**: dynamic, kinematic (moved by `Transform` *or* by `Velocity`), static. Mass from
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
- **Joints**: fixed, spherical (cone limit), revolute and prismatic (limits, motors), distance
  (rope), spring. `break_force` breaks a joint: the joint entity is despawned (or the `Joint`
  removed from a body entity). With `collide_connected: false` the two never touch, also when
  one side is a fixed body (a door hinged inside its frame, a lid on a static box).
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
- **Time**: runs in `FixedUpdate` / `SimSet::Physics` on world time, so `TimeDilation` slows it;
  each fixed step is split into substeps of at most `max_substep_secs`, so 10× fast-forward is
  as stable as 1×. `LocalTimeScale` speeds a single body up or down (its `Velocity` is in its
  own seconds, like `CharacterMove`).
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
  shared). Regions far from every `SimulationFocus` pause (`freeze_distance_m`).
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
| `max_substep_secs` | 1/120 | Longest substep. 1/60 lets ragdoll joints gap ~5 cm on impact; 1/120 ~1.5 cm. |
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
touching pairs, step time, substeps.

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

## Demo and tests

`demo::spawn_demo(commands, origin)` builds the **physics proving ground**: a 70 × 50 m concrete
yard (with a curb, so nothing rolls off) with a 30-crate tower, a bin filling with 250
crates/barrels/balls, a domino snake, crash test dummy ragdolls tumbling down a steep staircase,
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
piling up bodies, and that the barrel run's drums stay within 1 cm of the deck and 30 cm of their
lanes and stop in the sand.

Everything in it is drawn with detailed procedural art (`src/demo/art/`): beveled meshes sized to
their colliders (shipping crates with brackets and bolts, 200 l drums, chain links, a cast
concrete stair with handrails, a steel gantry, glulam and diamond-plate machines, Hybrid III-style
dummies) and PBR texture sets (albedo, normal, roughness/metal/occlusion) generated at startup,
about a second in release builds (debug builds make them a quarter the size). Apps without a
renderer get colliders only. `cargo run -p engine_physics --release --example art_props`
(`art_structures`, `art_dummy`, `art_textures`) previews each part on its own.

Tests (`cargo test -p engine_physics`): the conformance suite; the "done when" scenarios below
(`tests/scenarios.rs`); behaviors other modules rely on (`tests/behaviors.rs`: removal, compound
bodies, joint breaking and motors, hinges swinging through the fixed body they hang on, kinematic
platforms, stairs and steep slopes, dashing into thin walls, wall-running, no phantom walls on flat
ground, running over heightfield hills, crouching, landing from a fall, standing exactly on boxes
from 1 m to 10 km across, characters on their own clock, sensors and layers, local time, hovering,
buoyancy, blasts, bodies crossing and merging regions); riding moving bodies (`tests/platforms.rs`:
crates and characters on a deck at up to 30 km/s, lifts, a turntable, a thrusting and turning ship,
a seesaw tipped by a character's weight, crates landing on heads, rays hitting bodies at orbital
speed); mass overrides (`tests/mass.rs`: a moved center of mass, dropping an override); following
world shifts (`tests/world_shift.rs`: a sleeping pile stays asleep through floating-origin moves, a
stack stands through a tilting turn, world-pinned hinges turn with the world, a moving or spinning
frame adds its motion at each body, characters keep their footing, queries see the new frame at
once); rolling resistance (`tests/rolling.rs`: balls and logs slow at `Crr·g/(1 + I/mr²)` on
concrete, faster on grass and sand, the same on terrain and meshes, barrels stop on sand, barrels at
4 and 8 m/s roll over terrain and meshes within 5 mm of flat and 25 cm of straight); the debug view
(`tests/debug_draw.rs`, contacts on terrain included); a headless screenshot of the proving ground.

## Done when

- [x] A 1 000-body pile settles without jitter at 60 Hz (≈5 ms per step on the build machine);
  a 30-box tower stands still.
- [x] A ragdoll and a chain of jointed bodies stay stable under slow motion and at 10× time
  scale (joints hold within 2 cm, the solver adds no energy).
- [x] A body 1e8 m from the origin behaves exactly like one at the origin.
- [x] `engine_world::physics::conformance::assert_all` passes in this crate's tests.
- [x] Raycasts/shapecasts available as API functions (`physics.raycast`).

## Next

- Articulated (reduced-coordinate) joints for ragdolls and robot arms: zero joint gap.
- A `rolling_resistance` field on `SurfaceMaterial`, so games can tune it per surface (today
  it comes from the surface's deformation).
- Co-moving region frames, so cargo resting in a ship at 7 km/s can sleep (today bodies moving
  together at speed stay awake; they ride correctly, they just cost CPU).
- A `Ground` query backed by static colliders for levels without terrain.
- Buoyancy along the local "up" on planets: today it measures depth along world Y, as the
  `Water` contract's `surface_height` does (near the focus the space module turns the world so
  local up is +Y, so this only matters far from it).
