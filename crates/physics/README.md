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
  With `ccd` a 5 cm ball at 100 to 1000 m/s never passes through a 2 cm board, a mesh wall with
  no thickness, or a free-floating 5 cm plank; without it, a 1000 m/s ball flies through the
  plank. Contacts closing faster than 50 m/s are made hard by a contact hook: Rapier's contacts
  give a little once shapes touch, and a 1000 m/s ball sank 5 cm into the board, past its middle.
  `MassProperties` is written back.
  Settled bodies cost next to nothing: a step visits only awake bodies and those with forces of
  their own, and never takes a sleeping one mutably from Rapier (which would make Rapier redo its
  bookkeeping for it). 3,000 rock-shaped debris pieces asleep in a pit cost 0.44 ms per step
  (8.4 ms before): `cargo run -p engine_physics --release --example debris_field -- 3000`.
  While they tumble, the touching pairs that need spin friction or report hard hits are found in
  one pass that reads each pair once, in storage order when much of the region is awake: 52 ms
  per step for the 3,000 (102 ms before), of which Rapier's own step is ~37 ms.
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
- **Contacts on small bodies**: Rapier reuses a touching pair's contacts until the two have moved
  5 cm against each other, and a ball sliding along an old contact plane rides on it as if it
  went on forever. Bodies under 5 cm in reach (a 14 mm Galton ball slid 6 cm off a 4 mm pin on
  air, and balls stayed wedged between pins they had left behind) get fresh contacts every step
  while awake, through the same hook; bigger ones keep the reuse.
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
  per frame instead of 8.3. And only groups within `articulate_within_m` (20 m) of a focus are
  articulated (one already articulated keeps it until a quarter farther): past that, the few
  centimeters a constraint joint opens on a hard landing are about a pixel at 1080p. Seen from
  the overview camera, the yard's busy first five seconds cost 8.6 ms a step instead of 14.5, and
  the dummies' replay 4.6 instead of 13.3. With a focus, a group also needs to be moving: it is
  articulated once its fastest point goes over 2.5 m/s (falling, tumbling, thrown) and lets go
  under 1 m/s, when a settling or still ragdoll's constraint joints rest closed, so its place
  goes to one still tumbling. Up close at the stairs the busy first five seconds then cost 12.3
  ms instead of 16.1 and the replay 8.8 instead of 11.0, with the ragdolls' joints opening as
  rarely as before (over 1 cm in 18 frames of 1,800 against 16; at most 3.3 cm for a frame).
  `articulated_joints: false` turns it all off.
- **Heavy loads on light chains** (`Simulation::hang_loads`): a chain pinned at one place holding
  a body over 100× heavier than one of its links (a 17 t wrecking ball on 11 kg links, which
  can't be an articulation) hung with its links up to 22 cm apart on constraint joints: the
  solver's few passes can't carry such a load, so it rests stretched. Extra passes close it but
  cost the whole island (40 ms a frame while the ball plows through the crate wall), and moving
  the links back onto their joints after each step (PhysX's projection) left the ball swinging
  half again too fast. So the heavy body also hangs from the pin on a hidden rope as long as the
  chain pulled straight: one exact constraint carries it, the links carry only their own weight
  and stay within 6 mm, and the ball swings as a pendulum should (7.8 m/s at the bottom of a
  3.1 m drop, back to within 2 cm of where it was let go). A slack chain leaves the rope slack;
  unpinning the chain, or a loop or a second pin, takes the rope away. Ropes, springs and
  sliders in the chain end its rigid part.
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
  hearing) and, against static ground, `GroundContact` (dents, dust, impact sounds). That
  includes hits on pairs that never stop touching: a crate tumbling face over face, a post
  slapping down flat while its foot stays on the ground, a drum bouncing down the runway. Each
  substep whose impulse changes the lighter body's speed by `impact_min_speed` (beyond holding
  it up) counts, once per hit. A `GroundContact`'s `yaw` follows the slide, or for a body landing
  still, its longest axis lying flat.
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
- Level of detail (`src/lod.rs`): far physics simulates more coarsely, where a pixel spans
  more than the difference (15 cm at 150 m, 60 cm at 600 m on a 1080p screen). A region whose
  nearest body is past `coarse_beyond_m` (150 m) from every focus takes substeps twice as
  long; past `half_rate_beyond_m` (600 m) it steps every other frame with substeps four times
  as long, and its bodies are drawn moving on at their velocity in between (a region with
  characters stays coarse: they walk every frame). Most of a scene shares one region, so every
  half second busy far clusters split off: bodies are grouped by how close they are (and by
  joints), and a group wholly far away, with something awake in it and no character, moves
  into a coarse region of its own. Sleeping groups stay put (they cost almost nothing).
  Regions of different tiers merge as soon as their bodies come within reach of each other, so
  a ball thrown in from 260 m hits the crate by the player at full detail. Most of what tumbling
  bodies cost is finding their collisions, which the longer substeps do half or a quarter as
  often; fewer solver iterations saved little more and kept a far heap awake, so they stay.
  906 rocks tumbling in six drums cost 19.8 ms a step at full detail, 12.0 ms coarse and 6.4 ms
  at half rate, and no rock leaves its drum; with two drums near the player, two at 300–380 m
  and two at 0.9–1.1 km, 13.5 ms instead of 19.8
  (`cargo run -p engine_physics --release --example lod_field`). `PHYSICS_FOCUS=400,0,0` on
  the proving ground example simulates the yard as if the player were 400 m away: the
  rockfall pours a little slower there and heaps a little tighter, at half the cost (14 ms a
  step instead of 27.5 while pouring).
- Fine steps (`FineSteps`, `src/fine.rs`): the solver settles all of a substep's contacts
  together, so when a Newton's cradle ball swings in, the 3 mm gaps of the whole row close in
  one 8 ms substep and the row swings off as one lump, where steel passes the blow down the
  row and only the last ball flies. A body with `FineSteps` asks for substeps short enough
  that each touch is its own (1/960 s by default: a ball at 2 m/s moves 2 mm, less than the
  gap). Rows of hard bodies that pass a blow on (cradles, billiard racks, clockwork) need it.
  Stepping a whole scene that finely would cost several times as much, so such bodies keep to
  regions of their own: a group of them (by distance and joints) leaves a region that holds
  others too, and they merge again only when bodies come within 0.5 m plus three steps of
  travel, looked at every step, so a ball rolled at the cradle joins it before it hits. A fine
  region steps finely only in frames where two of its bodies may meet, and then with as many
  solver iterations per second as elsewhere (one per 1 ms substep): a five-ball cradle near
  the player costs about 0.3 ms a step (1.4 ms without those two savings). Gaps between
  resting bodies must exceed Rapier's 2 mm contact prediction, or resting balls touch before
  the blow arrives. `PhysicsStats::fine_awake` counts the awake bodies stepping finely.
- A `WorldShift` (the space module re-anchoring the world) moves the simulation in the same
  frame. A floating-origin move only moves region anchors: sleeping bodies stay asleep and
  contacts carry on. A turn into another planet's frame rotates each region's contents about
  its anchor, world-pinned joints included; a moving frame adds its velocity to every body.

## Seeing what physics sees (`PhysicsDebug`)

With `engine_debug::DebugPlugin` in the app, `PhysicsDebug` (a registered resource, also
`physics.debug {"all": true}`) draws over the scene, within `radius_m` (150 m) of the focus or
camera:

- `colliders`: wireframes, green awake (orange far away and coarse, red at half rate),
  blue-gray asleep, yellow kinematic, gray static, magenta sensors, cyan characters (big
  meshes and heightfields as their bounds).
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
| `articulate_within_m` | 20 | Only groups this close to a focus are articulated (farther, joint gaps are under a pixel or two). |
| `max_substeps` | 24 | Cap per fixed step (10× speed needs 20). |
| `allowed_penetration_m` | 0.001 | How far contacts may sink before being pushed apart. |
| `contact_stiffness_hz` | 60 | Contact spring frequency (static contacts 2×). 30 Hz lets a 30-box tower sag 24 cm, 60 Hz ~6 cm. |
| `region_radius_m` | 1000 | Size of a region. |
| `freeze_distance_m` | 25 000 | Pause regions this far from every focus (`None` = never). |
| `coarse_beyond_m` | 150 | Bodies this far from every focus take substeps twice as long (`None` = full detail everywhere). |
| `half_rate_beyond_m` | 600 | Farther still, they step every other frame (`None` = never). |
| `rolling_resistance` | 0.01 | Rolling resistance on hard ground (rubber on concrete); soft surfaces add more, 0 = roll forever. |
| `max_speed` | 100 000 m/s | Velocity clamp (spaceships are fast). |
| `contact_events`, `impact_events` | on | Send `ContactEvent` / impact `NoiseEvent` + `GroundContact`. |
| `impact_min_speed` | 1.5 m/s | Smallest speed change that counts as an impact. |
| `buoyancy` | on | Float and drag bodies in `Water`. |
| `blast_response`, `blast_scale` | on, 1 | Explosions push bodies; 1 = real blast wind, action games use 3–10. |

`PhysicsStats` (resource, also `physics.*` in `Diagnostics`): bodies, awake bodies, regions,
touching pairs, step time, substeps, articulated joints, static copies, and the awake bodies
simulated coarsely and at half rate.

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
plank bridge, a kinematic turntable, bouncing rubber balls, a **pendulum wave** (15 lacquered steel bobs under an A-frame gantry,
swinging 20 to 34 times a minute: they fall out of step into waves and snakes, swing in two
alternate rows at half a minute and are back in step at a minute) in front of a **Newton's
cradle** on a plinth (five 20 cm chrome balls on 80 cm wires, stepping finely, so the blow passes
down the row and only the last ball flies), a **pool table** (a 9 ft table with mahogany rails,
pockets cut into them and six turned legs: the cue ball breaks a rack of fifteen numbered balls at
8 m/s, in 0.5 ms substeps so the blow passes from ball to ball, and they spread over the cloth,
off the cushions and down a few pockets, still within about ten seconds), a **Galton board** (500
14 mm steel balls pour from a hopper through a walnut funnel and twelve rows of brass pins, set
a little off true as by hand, into seventeen bins behind glass, and heap up in the bell curve
printed on the back board: steel bounces over more than one pin, so the curve is the normal
curve this board gives, σ ≈ 3.2 bins, wider than a coin tossed at every row would make), a **rockfall** (300 crushed granite
rocks, faceted convex hulls 30 to 55 cm across, pour out of a hopper, slide down a 40° steel chute
and drop off its lip into a heap with a 35° face), a **counterweight catapult** (a 1.2 t
iron-bound box of stone on the short end of a 5 m oak arm swings it up into a padded crossbar,
and the 38 kg granite ball flies on out of its cup, 7.4 m up and 11 m down range, into the top of
a five-course crate wall; the arm is one body with the oak's and the stone's masses set on it,
stopped by its hinge's limit where the crossbar stands, and nothing lets the ball go: the cup
stops and it doesn't) and, along the front, a **barrel run**:
three oil drums roll off a 2 m ramp and 47 m down an asphalt runway (one triangle mesh, an edge
every 2 m) into a sand pit, staying flat and in their painted lanes, past galvanized guardrails to a
tire wall. It sits on the ground at `origin`, its top `demo::SLAB_RISE` (10 cm) above it; the
showcase's physics zone places it. Render it alone with
`cargo run -p engine_physics --release --example proving_ground -- out_dir`.

The stations replay on their own cycles (`demo::Station::cycle_secs`, world time: ragdolls,
seesaw, balls, cradle and catapult every 20 s, the barrel run and pool table every 25 s, dominoes, wrecking ball and bridge every
30 s, the rockfall every 30 s, the Galton board every 40 s, the pile every 45 s, the pendulum wave every minute, as it comes
back into step; the tower, wheel and turntable run on), so the zone is always
playing when someone walks in.
They pause while no focus point is within 250 m.
`physics.proving_ground.replay {"station": "dominoes"}` restarts one now (`{}` restarts all).
`demo::VIEWS` are tour stops for the action stations: arriving restarts the station, and the
warm-up lands on its best moment. `tests/proving_ground.rs` checks that stations replay without
piling up bodies, that the barrel run's drums stay within 1 cm of the deck and 30 cm of their
lanes and stop in the sand, that the rockfall's rocks leave the chute and come to rest in a
heap, and that the ten dummies all leave the landing and are asleep well
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
(`tests/debug_draw.rs`, contacts on terrain included); the level of detail (`tests/lod.rs`:
far drums tumble coarsely and at half rate and keep their rocks, far heaps still fall asleep,
a ball flying in from afar hits the crate by the player at full detail, a far walker steps every
frame); pendulums (`tests/pendulums.rs`: a Newton's cradle with fine steps passes one or two
balls' blow down the row while the middle stays within 2 cm, without them the middle swings off,
a cradle keeps to its own region beside tumbling crates and a ball rolled at it still hits it,
and a pendulum wave is in two alternate rows at half a minute and back in step at a minute); the
pool table's break (`tests/proving_ground.rs`: all sixteen balls step finely, at least 12 of the 15
spread more than 30 cm, at least one goes down a pocket, none leaves the table, and all are still
before the table resets); the Galton board (`tests/proving_ground.rs`: no ball leaves the board, all
are still, at most one stays caught on the pins, and the heap is centered within 0.6 bins, spread
2.5 to 4 bins, over 28% of the balls in the middle three bins, under 8% in the outer four, and no
more than 12% more on one side); contacts on small bodies (`tests/contacts.rs`: a 14 mm ball
sliding off a 4 mm pin falls within 10 frames and 2 cm, and one dropped on one of two pins three
ball widths apart drops through); the catapult (`tests/proving_ground.rs`: the arm stays on its
axle and comes to rest against the crossbar, the ball leaves the cup at 95 to 101% of the speed
the counterweight's fall gives it without losses (99%: 10.1 of 10.2 m/s), rises over 7 m and
brings down at least four of the wall's crates, and no crate leaves the yard); a headless
screenshot of the proving ground.

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
