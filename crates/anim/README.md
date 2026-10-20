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
  stay flat at toe-off) and locks them: a foot that comes down still is held where it
  landed until it lifts, however the clip's stride matches the body's speed, and a body
  turned or moved off its standing feet steps them round one at a time. `LookAt` turns spine, neck and head toward an entity or point.
  `ReachIk` reaches hands to world points or to points on other bones (the off hand on a
  two-handed grip), faded in and out by a clip curve. Feet step on `IkSurface` boxes, ramps
  and stairs, on `Ground`, and on physics colliders through `PhysicsQueries`.
- **Procedural motion.** `LeggedGait` drives any number of legs from the entity's actual
  motion: gait stages by speed (a horse walks, trots, canters and gallops), step timing,
  swing arcs, body bob, pitch and roll, turning, and per-leg load for muscles. Leg styles:
  two-bone, yaw-root (spiders), hoofed (in the swing the cannon folds and the hoof curls back
  behind the fetlock; a planted foot far behind tips the cannon on its toe) and scapular (a
  hoofed leg under a rocking shoulder blade, for dogs and cats); `stance_offset` centers a
  leg's stride, so hind legs stride from under the hip. `VehicleRig` gives cars sprung
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
- **Fur.** A `SkinMaterial` with `fur_length` grows a coat where the vertices say so
  (`SkinVertex::fur`): the mesh is drawn again as shells pushed out along the normal, the
  strands' tips sagging and combed back along the body, each shell cut into tapering strands
  on a jittered grid that sticks to the skin. Roots are darker than tips, light scatters
  through the coat, and the shells thin out with distance (`fur_lod`).
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
- **Directions and blows.** People blend by `heading` (where they move relative to where
  they face, positive left): walking ahead, sidestepping (stance widened so the feet never
  cross) and backpedalling, jogging and running sideways (hips turned into the run) and
  backwards, clips solved by leg IK so planted feet move at the body's speed; generated
  people stroll on whole steps below walking speed. `accel` (from the motion: speeding up
  along the facing) tips the trunk forward into a start and sits it back on bent knees into
  a stop. A `DamageEvent` makes the
  struck person flinch (`HitReaction`): joints below the hit give with the blow, joints above
  lag, the hips are shoved, the face winces, and it all springs back.
- **Contracts.** Reads `AnimationParams` and `AnimationTrigger` (`attack_heavy` swings,
  `block` raises the guard, and a mover's `LocomotionState.blocking` holds it); writes `SkeletonPose` every
  frame and `AnimationEvent` at marked frames, so movement, AI, cloth and hair never depend
  on this crate. Footsteps and hoof falls become `GroundContact` (foot, paw or hoof, with
  load and speed) and `NoiseEvent`; hammer impacts are loud `NoiseEvent`s.
- **Bodies on request.** A module that moves an entity itself puts `BodyRequest` on it
  (`human`, `horse`, `dog` or `spider`, an optional model path, height and seed) and gets a body:
  the paddock's rigs and models, animated from the entity's motion (`LocomotionState` or
  the transform's frame-to-frame movement; speed picks walk or run, a horse's trot, canter
  or gallop, turning leans), with footstep `AnimationEvent`s and `GroundContact`s. The
  `SkeletonPose` appearing is the answer; removing the request takes the body away and
  leaves the entity as it was. People are the fetched Rocketbox characters by seed (or by
  height: 1.74 m or 1.81 m), else generated; the imported horse is scaled to the requested
  shoulder height and the dog is built at it. Other kinds get no body and keep the
  requester's stand-in.
- **Ragdolls.** A person, dog or horse with `Ragdoll { weight }` (the physics contract)
  turns physical. A person is fourteen capsule bodies (pelvis, belly, chest, head, upper and
  lower arms and legs, feet) weighing 75 kg at 1.75 m, with real joint ranges (hinged elbows
  and knees, cones and twist limits at hips, shoulders, spine and ankles) and joint friction.
  Dogs (30 kg at 56 cm, scaled by size) and horses (500 kg) are sixteen: trunk, neck, head
  and three parts a leg, the elbows and hocks folding forward, the carpi and stifles back. At weight 1 the body is
  limp and carries the momentum it had; between 0 and 1 each part is pulled toward the
  animated pose and held up against gravity, so a half-limp person staggers and sags instead
  of collapsing. The pose follows the bodies (feet, gait and footsteps stop) and blends back
  to the animation when the weight returns to 0. Parts collide with the world but not with
  each other or their owner's controller (`RAGDOLL_LAYER`), foot IK ignores them, and hard
  hits become `GroundContact`s of kind `Body` and `NoiseEvent`s. Without a physics module the
  body stays animated. It runs in `AnimSet::Ragdoll`, after IK and before muscles.
- **Getting up.** A person whose weight returns to 0 while lying flat gets up rather than
  blending straight to standing: `get_up_back` (sits up, tucks a leg, plants a hand and a
  knee, steps up) or `get_up_front` (palms down, pushes up to all fours, kneels, steps up),
  with hands, knees and feet planted by IK on the floor. The clip starts from where and how
  the body lies, its first frame lined up with the body and blended in from the limp pose;
  foot IK stays off while on the floor. It reports `getting_up` and `got_up`
  `AnimationEvent`s so a mover knows when to take over again. A body moved far from its
  entity while limp (more than `GET_UP_REACH`) blends straight back to standing.
- **Bracing.** `RagdollTone` gives a limp body muscle tone: each joint turns toward its
  animated angle, with no pull toward a place and no help against gravity, so a falling
  person holds the shape of the `fall_brace` clip (arms reaching to take the fall, knees
  bent, chin tucked) until the first hard landing relaxes it.
- **Faces.** Imported people with Biped face joints (eyes, lids, jaw, mouth corners, lips,
  brows, cheeks) get a `Face`: seeded blinks, eyes that lead the head toward `LookAt` or
  `Face::look_at` (clamped to their sockets) and glance about otherwise, an effort
  expression from the `effort`, `brace` and `flex` curves (narrowed eyes, brows down and
  knit, mouth pulled into a grimace over gritted teeth), and a limp body's drooping lids and
  hanging jaw. It runs after retargeting, on the imported skeleton.
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

or spawn single characters with `demo::spawn_humanoid`, `spawn_horse`, `spawn_dog`, `spawn_spider`,
`spawn_car`, or from an AI agent with `anim.spawn {"kind": "horse", "at": [0, 0, 0]}`.
To animate your own character, register a `Skeleton`, clips and an `AnimGraph` in the
`AnimationLibrary`, then spawn an entity with `AnimatorSettings`, `AnimationParams` and a
`SkinnedMeshRenderer`; set parameters (`speed`, `turn`, ...) and send `AnimationTrigger`s.
`build::sdf::Body` turns smooth-blended capsules, ellipsoids and boxes into a skinned mesh
with automatic weights and muscle morph targets.

Tunable at runtime (JSON, console, scripts, AI agents): `AnimatorSettings`, `FootIk`,
`LookAt`, `ReachIk`, `IkSurface`, `SpringChains`, `LeggedGait`, `VehicleRig`, `MuscleRig`,
`SkinnedMeshRenderer` (with its `SkinMaterial`), `BoneAttachment`, `DemoPath`, `DemoScript`,
`Stunt`.

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
its generated people, horse and car, and every test still runs. The dog is generated from
code ([`build::dog`](src/build/dog.rs)) at any size: a Labrador's deep chest and tucked
belly, drop ears and an otter tail on springs, a furred golden coat, and a rotary gallop
whose speeds and stride rate scale with its size. The spider is a giant red-knee tarantula
from code ([`build::spider`](src/build/spider.rs)): a tan-rimmed carapace with fovea and
grooves, eight eyes, furred chelicerae over folded fangs, tapping pedipalps, a long-haired
abdomen on a spring, and stout hairy legs with orange knees. The paddock's props wear the
renderer's generated materials (planks, concrete, sand, rock), and the spider climbs
[`build::rock`](src/build/rock.rs) boulders whose flat tops are its walkable boxes.

| Test case | What to look for |
|---|---|
| Hammer athlete | Two-handed sledgehammer strike onto a tractor tire: arms, shoulders and back tense on the strike and relax after; veins on the forearms; the ponytail whips; then a braced block |
| Lifter | Double-biceps flex: biceps, shoulders and chest bulge |
| Shirt reach | Sleeves bunch at the elbows on the wind-up, the shirt stretches across the back on the reach |
| Step course | A walker climbs a ramp, crosses a deck, walks down stairs with feet planted by IK, then runs back leaning into the turns |
| Footwork drill | An athlete walks a square facing one way: ahead, sidestepping left, backpedalling, sidestepping right, blending through the corners, every planted foot still on the sand (`paddock_footwork`) |
| Shuttle lane | An athlete sprints the lane from a standing start (leaning into it), digs in to stop, runs back backwards, turns on the spot and runs sideways to the start; a coach beside the lane turns where they stand to keep watching, the head first and the feet stepping round once the runner is well off to one side (`paddock_sprint_start`, `paddock_sprint_stop`) |
| Sparring partner | A lifter takes a jab to the chin (head snaps back), a hook to the side of the head, a blow to the stomach (folds over it) and a shove on the back of a shoulder, flinching away from each and recovering (`paddock_flinch`) |
| Horse oval | A textured horse walks, trots, canters and gallops by speed; in the swing the knees fold, the hocks bend and the hooves curl back behind the fetlocks; haunches and shoulders flex with each stride; the mane rides the neck and the tail streams |
| Dog run | A furred Labrador walks off, trots, canters out of a turn and gallops down the far straight, the spine flexing with each stride, ears and tail swinging; it pants as it goes (`paddock_dog_trot`, `paddock_dog`) |
| Spider garden | A furred red-knee tarantula climbs over boulders in alternating sets of legs, a wave running from the back legs forward; its palps tap, its jaws work and its abdomen sways (`paddock_spider`) |
| Car loop | A concept car: suspension over speed bumps, roll in corners, steering and spinning wheels with the brake calipers held still |
| Parked car | A second concept car inside the loop swings its doors open on their front hinges, holds, and shuts them with a bounce on the latch |
| Stunt yard | A stuntman climbs the stairs to a deck, sprints across and goes off the edge braced, arms out and knees bent; he goes limp on the crash mat with the run's momentum, lies still with a slack face, gets up off his back or front with a hand and a knee planted, and walks back round (`paddock_stunt`, `paddock_stunt_down`, `paddock_stunt_up`; needs the physics module) |
| Faces | The imported people blink at their own times and glance about; under effort (the lifter's flex, the hammer strike, the block) the eyes narrow, the brows knit and the teeth grit (`paddock_face`; needs the fetched characters) |

`cargo run -p engine_anim --example anim_gallery --release -- out_dir` renders close-ups of
each test case (`GALLERY_SEQUENCE=horse:11:12:0.05` renders a strip of one subject;
`GALLERY_TRIGGER=get_up_back:1.0` with subject `stuntman` plays a graph state on him).
In the showcase valley the paddock is the `paddock` zone; `demo::VIEWS` are its close-up
tour stops, each restarting the paddock and waiting for its moment
(`cargo run -p showcase --release -- --headless --tour paddock_hammer,paddock_gallop`).

## Tests

Unit tests cover clips, graphs, IK, gaits, muscles, skinning and the shaders (WGSL validated,
GPU struct layouts checked). `tests/demo_scene.rs` runs the paddock and checks each test case
does what it shows (the hammer lands on the tire with both hands on the handle, muscles tense
and relax, the walker climbs and runs, the horse gallops with working haunches, the dog's
planted paws stay on the ground at a walk and trot); with the
character files fetched it runs the same checks on the imported people. `tests/render.rs`
renders a skinned character on the GPU and checks the pixels follow the pose, and that
textured submeshes each draw with their own surface, and that fur shells grow strands over a
furred skin. `tests/world_shift.rs` runs two
paddocks, shifts one halfway, and checks every pose and place still matches.
`tests/feet.rs` walks, runs, turns, sidesteps, backpedals and runs sideways and backwards
a person and measures how far each planted foot slides (under 1 cm per step), that one
turned on the spot steps round, and that a sprint start and a hard stop lean it forward and
back.
`tests/hits.rs` punches a person and checks the head snaps back and recovers.
`tests/body_request.rs` asks for a hiker, a horse, a 40 cm dog and an unknown kind on
entities the test moves itself: the bodies walk, run and gallop from that motion, step on the ground, keep the
requester's own parameters, rebuild on a changed request and go with a removed one.
`tests/ragdoll.rs` turns a walking person limp on a stand-in physics backend: fourteen
jointed parts that start together, keep the walk's momentum and ignore each other and the
owner's controller; the pose follows them, footsteps stop, half-limp parts are pulled
toward the animation and held up, hard hits sound like a body falling, and the pose blends
back to standing. A body laid face down gets up off its front with `getting_up` and
`got_up` events and stands on its entity; one moved far away blends straight up; muscle
tone turns joints without pulling or holding the body up, and relaxes on landing. A dog
and a horse go limp at their weight and get up again too. `face.rs` tests blinks, gaze
within the sockets, the effort and limp expressions, and that a skeleton without face
joints is left alone.

## Not yet

Ragdolls for spiders; an imported spider (no free realistic one exists, so it is generated
from code).
