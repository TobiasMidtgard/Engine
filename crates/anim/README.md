# engine_anim — Animation

**Owner:** Animation system. Background research: `game-engine-research/animation.md` in the
project files.

Skeletal and procedural animation for people, animals, creatures and vehicles: clips, blend
graphs and state machines, IK, legged gaits for any number of legs, vehicle rigs, spring
chains, muscles that tense with effort, and GPU skinning that wrinkles clothing where it
compresses and stretches.

## What it does

- **Clips and graphs.** `AnimationClip` (keyed or generated from a function, with curves such
  as `effort` and events such as `footstep` or `impact`). `AnimGraph` nodes: clip, 1D blend
  with sync and speed matching, additive, masked layers, and state machines with crossfades,
  one-shot triggers and conditions on parameters. Graphs are serde data, so tools and AI
  agents can add or edit them at runtime (`anim.add_graph`).
- **IK.** `FootIk` plants feet on uneven ground (pelvis drop, two-bone legs, foot tilt, toes
  stay flat at toe-off). `LookAt` turns spine, neck and head toward an entity or point.
  `ReachIk` reaches hands to world points or to points on other bones (the off hand on a
  two-handed grip), faded in and out by a clip curve. Feet step on `IkSurface` boxes, ramps
  and stairs, on `Ground`, and on physics colliders through `PhysicsQueries`.
- **Procedural motion.** `LeggedGait` drives any number of legs from the entity's actual
  motion: gait stages by speed (a horse walks, trots, canters and gallops), step timing,
  swing arcs, body bob, pitch and roll, turning, and per-leg load for muscles. Leg styles:
  two-bone, yaw-root (spiders) and hoofed (folding cannon). `VehicleRig` gives cars sprung
  suspension, body roll and pitch from acceleration, steering and wheel spin.
  `SpringChains` make tails, manes and ponytails lag and swing.
- **Muscles.** `MuscleRig` activates each muscle from joint angles, clip curves (`effort`,
  `brace`, `flex`), gait load and speed; activation rises fast and relaxes slowly, relaxed
  muscle jiggles. Activation drives bulge morph targets (with corrected normals), a flush
  on working skin and veins on bare arms and legs.
- **GPU skinning.** `SkinnedMeshRenderer` draws through a `RenderFeature`: a compute pass
  applies morph targets and linear blend skinning, a second pass measures how much each
  vertex's edges compress or stretch against the rest pose. The material bunches compressed
  cloth outward and folds it into rings across the limb (sleeves at the elbow), draws
  diagonal pull lines where it stretches (a shirt across the back), and adds veins, flush
  and fabric sheen. All of it is procedural and fades by distance. Meshes are split into
  submeshes, each drawn with its own textured surface (base color, normal, metal/rough,
  alpha mask for hair cards); the procedural detail goes on top where the mask says skin
  or cloth.
- **Imported characters and clips.** `import` reads rigged glTF/GLB characters (meshes,
  skins, materials, textures) and their animation clips. `Retargeter` drives an imported
  skeleton from a logic skeleton (bones matched by position and direction, in-between
  spine bones blended, fingers closed by grip curves) and turns imported clips back into
  logic clips. `HumanModel` dresses a humanoid in an imported Biped character: the
  humanoid's rig, clips, IK and muscles keep running on a logic skeleton fitted to the
  character, muscles get bulge morphs on the character's own mesh, hair cards hang on the
  ponytail spring chain, and optional motion-capture idle, walk and run replace the
  generated ones. Files load on background threads; until then the person keeps its
  generated body.
- **Contracts.** Reads `AnimationParams` and `AnimationTrigger`; writes `SkeletonPose` every
  frame and `AnimationEvent` at marked frames, so movement, AI, cloth and hair never depend
  on this crate. Footsteps and hoof falls become `GroundContact` (foot, paw or hoof, with
  load and speed) and `NoiseEvent`; hammer impacts are loud `NoiseEvent`s.
- **Bodies on request.** A module that moves an entity itself puts `BodyRequest` on it
  (`human`, `horse` or `spider`, an optional model path, height and seed) and gets a body:
  the paddock's rigs and models, animated from the entity's motion (`LocomotionState` or
  the transform's frame-to-frame movement; speed picks walk or run, a horse's trot, canter
  or gallop, turning leans), with footstep `AnimationEvent`s and `GroundContact`s. The
  `SkeletonPose` appearing is the answer; removing the request takes the body away and
  leaves the entity as it was. People are the fetched Rocketbox characters by seed (or by
  height: 1.74 m or 1.81 m), else generated; the imported horse is scaled to the requested
  shoulder height. Other kinds (a dog) get no body yet and keep the requester's stand-in.
- **Time.** Every animator uses `Time::delta_for(LocalTimeScale)`, so `TimeDilation` slows
  animation and a hero who ignores world dilation keeps moving at full speed. Animators
  beyond `lod_distance` from the simulation focus update at a quarter rate.
- **World shifts.** On a `WorldShift` (floating origin, a new planet's frame) planted feet,
  spring chains, muscle jiggle, wheel contacts, look and reach targets and demo paths move
  with the world, so nothing pops, whips or slides.

## Using it

`AnimationPlugin` registers everything. The quickest way to see it is the paddock:

```rust,ignore
engine_anim::demo::spawn_demo(&mut commands, zone_center);
```

or spawn single characters with `demo::spawn_humanoid`, `spawn_horse`, `spawn_spider`,
`spawn_car`, or from an AI agent with `anim.spawn {"kind": "horse", "at": [0, 0, 0]}`.
To animate your own character, register a `Skeleton`, clips and an `AnimGraph` in the
`AnimationLibrary`, then spawn an entity with `AnimatorSettings`, `AnimationParams` and a
`SkinnedMeshRenderer`; set parameters (`speed`, `turn`, ...) and send `AnimationTrigger`s.
`build::sdf::Body` turns smooth-blended capsules, ellipsoids and boxes into a skinned mesh
with automatic weights and muscle morph targets.

Tunable at runtime (JSON, console, scripts, AI agents): `AnimatorSettings`, `FootIk`,
`LookAt`, `ReachIk`, `IkSurface`, `SpringChains`, `LeggedGait`, `VehicleRig`, `MuscleRig`,
`SkinnedMeshRenderer` (with its `SkinMaterial`), `BoneAttachment`, `DemoPath`, `DemoScript`.

API: `anim.list`, `anim.state` (graph state, curves, events, muscles, gait), `anim.clips`,
`anim.set_param`, `anim.trigger`, `anim.add_graph`, `anim.set_graph`, `anim.spawn`,
`anim.demo`, `anim.demo_restart` (start the paddock's test cases over).

## The paddock (showcase test cases)

The four people wear Microsoft Rocketbox characters (MIT licence, © Microsoft) with
Rocketbox motion-capture idle, walk and run. The horse is the CC0 "Rigged Horse" (model and
textures by Lyndon Daniels, rig by ChadM, glTF export from OloEngine), fitted with the horse
rig by [`horse_model`](src/horse_model.rs): the procedural horse's skeleton is placed on the
model's joints (adding the point of shoulder, stifle, fetlocks and two neck vertebrae the file
lacks), its skin weights are handed over, the mesh is smoothed with PN-triangle subdivision,
and it grows a mane, forelock and tail of alpha-cut hair cards. The car is
[Car Concept](https://github.com/KhronosGroup/glTF-Sample-Assets/tree/main/Models/CarConcept)
by Eric Chadwick and Darmstadt Graphics Group GmbH (CC BY 4.0; the Khronos logos are not
shown), its body, wheels, calipers and doors driven by the vehicle rig through a
[`RigModel`](src/model.rs). All are listed in `assets/manifests/animation.toml` and are not in
git; fetch them with `cargo run -p fetch-assets -- animation`. Without them the paddock keeps
its generated people, horse and car, and every test still runs.

| Test case | What to look for |
|---|---|
| Hammer athlete | Two-handed sledgehammer strike onto a tractor tire: arms, shoulders and back tense on the strike and relax after; veins on the forearms; the ponytail whips; then a braced block |
| Lifter | Double-biceps flex: biceps, shoulders and chest bulge |
| Shirt reach | Sleeves bunch at the elbows on the wind-up, the shirt stretches across the back on the reach |
| Step course | A walker climbs a ramp, crosses a deck, walks down stairs with feet planted by IK, then runs back leaning into the turns |
| Horse oval | A textured horse walks, trots, canters and gallops by speed; fetlocks bend, haunches and shoulders flex with each stride; the mane rides the neck and the tail streams |
| Spider garden | Eight procedural legs pick their way over rocks |
| Car loop | A concept car: suspension over speed bumps, roll in corners, steering and spinning wheels with the brake calipers held still |
| Parked car | A second concept car inside the loop swings its doors open on their front hinges, holds, and shuts them with a bounce on the latch |

`cargo run -p engine_anim --example anim_gallery --release -- out_dir` renders close-ups of
each test case (`GALLERY_SEQUENCE=horse:11:12:0.05` renders a strip of one subject).
In the showcase valley the paddock is the `paddock` zone; `demo::VIEWS` are its close-up
tour stops, each restarting the paddock and waiting for its moment
(`cargo run -p showcase --release -- --headless --tour paddock_hammer,paddock_gallop`).

## Tests

Unit tests cover clips, graphs, IK, gaits, muscles, skinning and the shaders (WGSL validated,
GPU struct layouts checked). `tests/demo_scene.rs` runs the paddock and checks each test case
does what it shows (the hammer lands on the tire with both hands on the handle, muscles tense
and relax, the walker climbs and runs, the horse gallops with working haunches); with the
character files fetched it runs the same checks on the imported people. `tests/render.rs`
renders a skinned character on the GPU and checks the pixels follow the pose, and that
textured submeshes each draw with their own surface. `tests/world_shift.rs` runs two
paddocks, shifts one halfway, and checks every pose and place still matches.
`tests/body_request.rs` asks for a hiker, a horse and an unknown kind on entities the test
moves itself: the bodies walk, run and gallop from that motion, step on the ground, keep the
requester's own parameters, rebuild on a changed request and go with a removed one.

## Not yet

Ragdolls from the physics contract; an imported spider (it is still generated from code);
gallop poses tuned for the imported horse's shorter hind legs.
