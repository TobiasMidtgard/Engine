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
  with sync and speed matching, additive, masked layers, and state machines with crossfades
  or inertialized transitions, one-shot triggers and conditions on parameters. An
  inertialized transition shows the new state at once and decays the difference from the old
  pose on a spring together with the old pose's velocity (Bollo's Gears of War
  inertialization in Holden's spring form; half-life 0.3 of the transition, clamped so it
  can't overshoot, faded out by the end), so a cut-off swing carries on a moment instead of
  stopping dead. Graphs are serde data, so tools and AI
  agents can add or edit them at runtime (`anim.add_graph`).
- **Motion matching.** A `motion_match` graph node plays whichever moment of captured motion
  best continues the pose toward where the character is going (`motion::MotionDatabase`,
  after Clavet's For Honor talk and Holden's articles): 27 features per frame (feet
  positions and velocities, hips velocity, the root's place and facing 1/3, 2/3 and 1 s
  ahead), brute-force search every 0.1 s, jumps hidden by spring inertialization that keeps
  velocity. Takes keep their root motion (starts, stops, turns on the spot and into a walk,
  walk and run cycles at five paces), each also mirrored; loops play round, and a take's
  trajectory carries on past its end. Every frame is labelled with which heels and toes are
  on the ground (below 3 cm and 0.25 m/s, lifted above 5 cm or 0.5 m/s, smoothed over five
  frames, MotionBricks' thresholds); the node publishes them as `contact.foot_l`/`_r` curves
  and foot IK holds a foot as soon as the data has it down, before it has come to rest. Strafing and backpedalling hand over to the blend
  tree. Diagonals up to 60° stay on the captured walk, turned (below). Fetched people move with 28 Rocketbox takes (`anim/moves/`); `examples/motion_db.rs`
  reports a database and which moves a scripted walk plays.
- **Parkour.** People jump, fly (an air blend by vertical speed: rising, at the top,
  falling), double-jump, land soft or hard, slide, and run on walls either side with a kick
  off across to the next. `parkour::Parkour` turns a `LocomotionState` (its mode and velocity)
  into the graph's parameters and triggers, so whatever moves a person (Movement's
  player, an NPC, the paddock's course runner) gets them with no new contract fields. Which
  side the wall is on and how far is found with rays at hip height against physics colliders
  and `IkSurface` boxes; the wall-run cycle (0.56 s a stride, 4 m at 7 m/s, after sprint data)
  plants the feet flat on a wall 0.55 m off with the body leaning 32° from it, the inside
  arm held out toward it and the head level, and `wall_ik` moves the feet onto the wall that
  is actually there (`build/humanoid/parkour.rs`).
- **Animal actions.** A horse grazes, drinks, rears, shakes its head and paws; a dog sits,
  lies down (from standing or from the sit), bows to play, shakes off and sniffs a trail.
  Each is a clip played over the gait that frees the legs it moves with `release` curves
  (`build::quadruped`, poses keyed as body, neck, tail and folded legs placed by IK) and puts
  them back on the ground as they fall. `actions::AnimalActions` takes requests (or a
  parameter `act.<action>`) and queues them, cutting into a hold where an action allows; an
  idle picker plays one when the animal has stood a while. Borrowed from MotionBricks, with
  none of its code or data: of an action's entry clips the one whose first pose is nearest
  the pose shown plays, with a fade from that distance (a sitting dog lies down from the
  sit), and an `ActionSpot` (a trough) names its action and the point it is about, the neck
  reaching the muzzle to it by CCD while the clip's `reach` curve is up.
- **Locomotion warping.** Before foot IK, `LocomotionWarp` (Unreal's Animation Warping, as
  Lyra uses it) fits the captured walk to how the body actually moves. Orientation warping
  turns the hips toward the travel, up to 60°, while the spine, chest, neck and head take the
  turn back (35%, 70%, 90%, 100%), so the legs walk where the body goes and the eyes face
  where it faces. Motion matching reads `warp_max` from the parameters, searches for a
  forward walk within it and hands over to the blend tree only past it (+0.15 rad).
  Stride warping scales each ankle's reach along the travel by the body's speed over the
  speed the playing feet cover (`MotionPlayer::stride_speed`, after motion matching's own
  0.8 to 1.25 play rate), within 0.75 to 1.35, and the hips drop just enough for the farther
  foot to reach. Both ease in from 0.3 to 0.6 m/s, so starts, stops and turns on the spot
  play as captured. People with `FootIk` get one; it acts only while motion matching plays.
- **Combat.** `combat::AttackSet`s are serde data like graphs: each move plays a clip over
  the character's graph (`Animator::play_over`, an overlay slot with fades and crossfades)
  and carries timed windows (Unreal's montage notify states): `hit` (the striking hand
  lands a `DamageEvent` on the target when its knuckles come within reach of the face or
  chest, once per window, and both fighters freeze for a few frames of hitstop,
  `Animator::freeze_for`), `cancel` (a buffered press that matches one of the move's links
  starts the next move), `warp` (the attacker closes to striking range and turns to face
  the target, at a capped speed, like Unreal's Motion Warping) and `armor`. An `Attacker`
  takes the `attack_light` / `attack_heavy` triggers; only the newest press counts and it
  waits 0.2 s (For Honor's buffering), so an early press still links and mashing doesn't
  queue a combo. A hit window that a long frame steps right over (a slow machine, or a film
  at 8 frames a second against a 0.11 s window) still lands. While a strike plays, two-bone IK pulls the fist onto the target's face,
  weighted by the clip's `strike` curve, so blows land where the target is rather than
  where the clip was authored. The built-in `boxing` set chains jab, cross, hook and
  uppercut (or, on a heavy press after the hook, `uppercut_heavy`, a knockdown finisher;
  on a heavy press after the jab, `body_hook`, a lead hook to the liver thrown from bent
  knees with the trunk leaning over, which a high guard doesn't block) (generated punch
  clips with chest turn, a dip on the uppercut and a pivoting rear foot); the struck person's flinch (`hit`) snaps the head away from each blow, and
  the attacker's face sets as each punch lands (the `strike` curve feeds `Face` effort).
  A target holding up its guard (`guarding` > 0.5) blocks high strikes from within 70° of
  its front: the fist is aimed at and lands on the nearer glove, with the set's `Block`
  share of damage (0.2) and push (0.35) so the guarding arm flinches back, 0.6 of the
  hitstop (blockstun is shorter than hitstun in fighting games), and the attacker is pushed
  back 0.12 m (`HitReaction::knock`), as fighting games push a blocked attacker out of
  reach; `Attacker::blocked` counts them. Damage numbers and who may hit whom stay with the
  combat module, which reads the published phase, cancel and armor flags.
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
  leg's stride, so hind legs stride from under the hip, and a leg's `release` curve frees it
  from the ground for the animated pose (a tarantula's raised front legs). `VehicleRig` gives cars sprung
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
  a stop. `grade` (from the motion: rise per meter on the ground, full at a flight of
  stairs) leans the body into a climb, and going down softens the knees, drops the chin to
  watch the steps and holds the arms a little out. A `DamageEvent` makes the
  struck person flinch (`HitReaction`): joints below the hit give with the blow, joints above
  lag, a low blow shoves the hips (the knees give and the trunk leans against it), the
  face winces, and it all springs back. `guarding` > 0.5 holds a boxer's `guard`.
- **Contracts.** Reads `AnimationParams` and `AnimationTrigger` (`attack_heavy` swings,
  `block` raises the guard, and a mover's `LocomotionState.blocking` holds it); writes `SkeletonPose` every
  frame and `AnimationEvent` at marked frames, so movement, AI, cloth and hair never depend
  on this crate. Footsteps and hoof falls become `GroundContact` (foot, paw or hoof, with
  load and speed) and `NoiseEvent`; hammer impacts are loud `NoiseEvent`s.
- **Character contract.** People with an `Appearance` get a `Face` seeded from it that
  writes `FaceControls` every frame (ARKit blink, squint, brows, cheeks, mouth and jaw
  weights from effort, wincing and limpness, plus gaze from the humanoid head). While a
  character module draws appearances (`AppearanceBodies`), those people keep their humanoid
  `SkeletonPose` but no fetched model and no mesh of this crate's (`tests/appearance.rs`).
  Requested people also get `HandControls`: each hand's grip from the clips' `grip`, `flex`
  and `fist` curves, at least what it holds (`HandGrip`), for the character module to curl
  the fingers (`tests/hands.rs`).
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
- **Physical hit reactions.** `PhysicalBlend` makes part of a body physical for a moment
  (Unreal's `SetAllBodiesBelowPhysicsBlendWeight`): each `PhysicalPulse` simulates the parts
  from a root bone down, driven toward the animated pose with drives softened by its weight,
  and shows up to `peak` of their simulated pose, rising and falling back to the animation.
  The parts it doesn't reach are kinematic bodies moved with the animation, so the legs of a
  person rocked by a blow stay planted. A blow of at least `HitReaction::physical_above`
  (15 N·s) starts one from the spine up, peaking at about half the simulated pose and
  settling in half a second, and pushes the part nearest the hit with a share of the blow,
  so each punch rocks the head and chest its own way on top of the flinch, without bending
  the neck or back past what looks unbroken. The bodies exist only while a pulse runs.
- **Stagger steps.** A blow pushing at least `HitReaction::stagger_above` (25 N·s) along the
  ground knocks the body back to its capture point (inverted pendulum, Pratt et al. 2006):
  `v·sqrt(h/g)` along the push for the speed `v` the blow gives 75 kg (scaled by size³),
  times `stagger_scale`, at most `stagger_max`. The body glides there, fast at first and
  easing out over `sqrt(h/g)` (0.3 s), and `FootIk::hurry` makes the feet step after it
  even though it moves fast: the foot farthest along the push first, then the other.
  A character with a `CharacterController` (its movement owns the transform and collides),
  or with `stagger_move` off, only publishes `stagger_velocity` for its movement to apply. `HitReaction::knock` moves a body the same way for other reasons;
  the paddock's sparring partner and boxer step back to their marks with it
  (`DemoStep::home`).
- **Knockdowns.** A blow that would carry the body farther than two steps can catch it
  (`HitReaction::fall_beyond`, 0.55 m of knock-back still to come, counting what earlier
  blows left: N-step capturability, Koolen, Pratt et al. 2012) knocks it down when a physics
  backend can catch it (`Knockdown`); without one it staggers. The body goes limp with
  `RagdollTone` holding a protective brace until it lands: `fall_brace` (arms forward and
  down) when pushed from behind, else `fall_back_brace` (chin tucked, hips sitting down,
  arms back and out at about 45°, palms down, as a judo back breakfall). The blow's push is
  spread over the parts by mass, more to the parts higher up, so the body topples over its
  feet. Once the hips have lain still for `down_secs` (1 s), the entity is stood where the
  get-up starts and the body gets up off its back or front; `Knockdown` goes once it
  stands. Blows while down only push the limp parts, a walk asked for (`knock`) waits until
  it is up, and an `Attacker` drops presses aimed at a downed target. The `boxing` set's
  `uppercut_heavy` (a heavy press after the hook: 95 N·s, 30 damage, 0.13 s hitstop) does it.
- **Hurt.** Where a blow lands matters, and blows add up (`Hurt`, like EA UFC's separate
  head, body and leg damage). Each blow is sorted by where it struck the skeleton: the head
  (nearer the face than either hand), the trunk, the liver (the trunk on the person's own
  right below the chest) or a glove or limb (nothing lasting). Head blows fill `daze`
  (push over 120 N·s, draining 0.08/s): past `rocked_at` (0.6) the legs go to rubber, as
  balance and gait go first after a concussive blow. The hips sway on two slow sines that
  don't line up, the knees dip, the trunk and head loll against the sway, the hands drop,
  and every second or so the body lurches and the feet step to catch it. Body blows fill
  `wind` (over 60 N·s, draining 0.2/s): the trunk folds over the stomach, the knees give and
  two-bone IK brings the hands down to cover it (from a guard or from hanging). A liver blow
  of 30 N·s or more folds the body onto a knee 0.45 s later, as a liver shot's reaction is
  delayed while the nerves respond: `kneel_hurt` (4 s, generated) slides the right foot back
  onto its knee, the right hand clutching the side and the left forearm on the left knee,
  head down, then pushes up off the left leg. A rocked, winded or kneeling body doesn't
  block (`Hurt::guard_down`) and attackers hold off while it kneels (`Hurt::is_down`).
- **Slips, rolls and counters.** Good fighters move their heads instead of only raising a
  guard. A thrown punch goes where the head *was*: an `Attacker`'s aim follows the target
  until 0.2 s before the move's first hit window, then holds, and the hit still has to come
  within `reach` of the target's current head or chest (`Attacker::missed` counts the
  ones that didn't). Each move names the evasion that beats it (`AttackMove::evaded_by`):
  in `boxing` the jab is beaten by `slip_right` (outside it), the cross by `slip_left`, the
  hook by `roll` (a U-shaped dip under it) and the uppercuts by `pull_back`; a body hook
  can't be slipped. The clips are generated from the boxer's stance: knees bent and hips
  under the body rather than a bend at the waist, trunk and head turning with the slip, the
  head moving about 25 cm. An `Evader` (`skill` 0..1, `read_secs` 0.18, like an action
  game's perfect-dodge window) reads a strike aimed at it that close to its hit and plays the
  evasion as an overlay with probability `skill` (deterministic per strike); the
  `AnimationParams` "evasive", when set, switches it. After a roll or pull-back an evader
  with its own `Attacker` presses `attack_heavy` halfway through, coming up with a counter.
  A fighter in the middle of their own strike or evasion doesn't block.
- **Chaining kills (freeflow).** As in Batman Arkham, Spider-Man and Sifu, each press goes at
  the foe the stick points at. A `Freeflow` on an `Attacker` picks the target again on every
  press: foes within `reach()` (6 m, growing 0.25 m with each blow of the combo up to 7.5 m)
  and 60° of the stick, scored 0.7 by angle and 0.3 by distance, the current target kept
  unless the stick points clearly elsewhere (`keep_bonus`). With no stick it stays on the
  current target, or takes the nearest in front. A target that goes down (knocked down, on a
  knee, despawned) is handed over at once and the waiting press kept, so a kill never eats
  the next press; `Attacker::struck` is who the current move is aimed at, and once its blows
  are over a press at the new target starts at once (from the top of the combo) instead of
  waiting out the recovery. A foe past the strike's range is closed on first with one of the
  attack set's `approaches`: in `boxing` a `lunge` (0.45 s, a push-off with both feet off the
  ground for a moment) beyond 1 m, a `leap` (0.75 s, a deep load, tucked flight, a landing
  that sinks) beyond 2.6 m. Their warp is eased (`WindowKind::Warp::ease`: speeding up over
  the first quarter and slowing over the last, capped at 12 and 18 m/s), turns to face the
  target by 40% of the travel, and the punch follows on touch-down. `combo` counts blows in a
  row (and is set as the `combo` parameter); it ends after 1.5 s without one or when the
  fighter is hit. Gameplay steers it with `Freeflow::stick` or the `stick_x` / `stick_z`
  parameters and limits it to `Freeflow::foes`; `freeflow::pick` is the choice on its own.
- **Weapons.** [`build::weapons`](src/build/weapons.rs) authors sword, shield, great sword
  and bow clips by the weapon rather than the joints: each key says where the grip is,
  which way the blade points and which way its edge faces, plus the hips, trunk and feet;
  between keys the blade swings round the shoulders with its speed peaking 60 to 70% of the
  way through the cut and following through past the target, and both arms and legs are
  solved with two-bone IK when the clip is built (so the fists stay on the hilt and the
  feet on the ground). Sets: `sword_shield` (a forehand slash, a rising backhand, a thrust
  finisher, a heavy overhead and `shield_bash`), `greatsword` (a horizontal sweep with the
  blade rolling over so the edge leads, a rising cut, an overhead from high guard armored
  through its wind-up), `bow` (`bow_shot` and the held `bow_shot_full`: draw to an anchor
  under the jaw with the draw elbow behind it, a `release` event, the hand carrying on back
  along the jaw). A hit window can name a `Striker`, a segment in a bone's frame (the blade,
  the shield's face): it lands when that segment, swept from last frame's place to this
  one's, passes within reach of the target, pushes along the blade's motion, and sends
  `Slash`, `Pierce` or `Blunt`. A `Shield` on the target (`shield_guard` > 0.5) blocks high
  and low strikes from within 80° of its front at the shield's face, letting through 5% of
  the damage. A weapon that lands on a guard bounces off it: after the hitstop the swing
  runs back up its own arc at 1.4 times its speed for 0.22 s and fades to the stance, the
  move ending with it (`Animator::rebound`); `Attacker::recoil` counts down meanwhile and no
  new move starts, which is the defender's opening for a guard counter (Elden Ring's
  bounce). In every hitstop the struck body shakes side to side, tapering off, by more for
  longer hitstops (`Animator::shake_for`, Sakurai's hit-stop shake). A guard has three
  answers, kept in a `Defense` on anything an `Attacker` goes for: raised within 0.2 s of a
  weapon's blow landing (0.07 s if it came down under 0.4 s before, so mashing doesn't
  work), it deflects (Sekiro, Elden Ring's parry): no damage, the attacker's blade flies
  back harder and it stays open 0.9 s, and the shield swats out (`shield_deflect`); a
  blocked blow shoves the defender back and holds it in shield-stun (no new move for the
  hitstop and 0.15 s); a hit window with `breaks_guard` (the great sword overhead, the
  shield bash) breaks the guard: the shield is flung open (`guard_broken`) and blocks
  nothing for 1 s, whatever the parameters say. Presses wait in the buffer while a fighter
  can't act and go the moment it can. Graph states `sword_stance`, `shield_guard`, `greatsword_stance` and the
  arms-only layers `sword_carry`, `greatsword_carry` and `bow_carry` are set by parameters
  of the same names. A weapon stance walks: by `speed` it blends into the locomotion (walk,
  sidestep, backpedal) with the stance's upper body layered on from the spine up (its
  `*_moving` clip: hips square, the trunk turned by what the standing stance puts in both,
  so the guard holds while the legs step). [`Footwork`](src/footwork.rs) keeps a fighter at
  its `measure` from its foe (its `Attacker`'s target unless set), facing it: it steps in
  (0.8 m/s) or back (0.7 m/s) once it is more than `slack` off, circles round the foe when
  the `circle` parameter says (-1 right, 1 left), rests while its own move, an evasion, a
  knock-back or a fall plays (a fighter shoved back by a bash steps back in after), and
  stands its ground while the foe's move plays. It moves the `Transform` and writes `speed`
  and `heading`, or publishes `velocity` with `move_transform` off. A move with a `Pairing` is
  a riposte played with its victim ([`paired`](src/paired.rs), Unreal's Contextual
  Animation and FromSoftware's throws): a light press at a foe whose blade was just
  deflected (`Attacker::open`) or whose guard was just broken, within 2.4 m and 60° of the
  front, throws the set's riposte (`sword_riposte`, `greatsword_riposte`; in a cancel window
  it comes before the link). The foe starts `riposted` on the same frame, is moved onto its
  spot in front facing back over 0.25 s and held there (`Riposted`); the blade goes in on
  the keyed frame, unblockable, for about three times a thrust; nobody else's blows touch
  the pair; the foe sags on the blade, drops to a knee, falls on its back and gets up out of
  `get_up_back`. [`props`](src/props.rs) spawns matching weapons on the hands
  (`spawn_sword`, `spawn_shield`, `spawn_greatsword` with the off hand reaching for the hilt
  by the clip's `grip` curve, `spawn_bow` with a string from both tips to the draw hand and
  a nocked arrow) and `shoot_arrow` flies a prop arrow that sticks. Blade blows under the
  ribs don't fold a body onto a knee (`Hurt` keeps that for blunt blows to the liver).
- **Grips.** [`grip`](src/grip.rs) fits held items to the hands people see. Clips and props
  share one standard fist per hand (origin at `GRIP`, fingers −Y, thumb side −Z, palm toward
  `PALM`). An imported model's [`Retargeter`] turns each visible hand so its palm faces the
  standard way (knuckles and all, not just the hand's direction) and measures where its fist
  closes: behind the knuckles and a palm's depth plus a sword grip's radius below them,
  scaled by the hand's size. Props attached `in_fist` and two-handed reaches with `fist`
  then hold the item in that fist, and a `Reach` with `turn` also turns the off hand to the
  grip (the great sword's left hand is held the same way round as the right). Each prop
  declares its handle (`HeldItems`: axis, radius, length), a hand holding one is closed on
  it, and `Retargeter::wrap` curls each finger segment from the knuckle out until it
  touches the handle (joint-lock-on-contact, Rijpkema and Girard 1991), so a bow's thick
  riser and a sword's grip close differently. `Retargeter::grip_report` measures a posed
  hand on its handle (palm gap, finger penetration and gaps, thumb opposition, handle size
  over hand length) and `cargo run -p engine_anim --example grip_check --release` runs the
  paddock and reports every held hand. Each item's grip is data, `GripPresets`
  (`assets/anim/grips.json`, built in): its handle's size, its seat in the fist (slid along
  the handle, rolled about it) and an extra curl per finger. Changing an entry re-seats every
  hand holding that item; the editor's Grips tab draws a chosen hand's grip over the scene
  view (handle, palm, finger segments coloured by their gap), shows the measurements against
  the grip research's limits, tunes the seat and finger curls live and saves the file.
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
  beyond `lod_distance` from the simulation focus update at a quarter rate. Beyond the
  `SimulationRange` they rest: the last pose stays, with a fresh one every 2 s
  (`REST_POSE_SECS`), and procedural layers, IK, skinning and demo walkers skip them
  (`Animator::posed`); on waking, gaits, springs, muscles and car rigs start their motion
  history over (`Animator::fresh`). Car wheels probe the ground again only once they move.
  `cargo run -p engine_anim --release --example paddock_bench -- 20 150 --profile` times the
  paddock without a renderer.
- **Loading.** The paddock's generated bodies take seconds to make, so they are made off the
  main thread: `demo::prefetch` starts them on a worker (`demo::ready` says when they are
  in) and `spawn_demo` then makes none; a streamed cell can use `demo::build_demo` (worker)
  and `demo::place_demo` (about a millisecond). The skinning pass keeps each mesh's morph and
  edge tables, so a character arriving doesn't redo every other mesh's.
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

People's `SkeletonPose` uses the humanoid skeleton (`build::humanoid::skeleton()`), whether
they wear a generated body or a fetched model (fitted with `fit_skeleton`, which keeps these
names and parents). Model space, meters, facing −Z, +X to the person's right; `_l` / `_r` are
the person's own left and right. Each bone below is followed by its parent:

| Bone | Parent | | Bone | Parent |
|---|---|---|---|---|
| `root` | — | | `clavicle_l`, `clavicle_r` | `chest` |
| `hips` | `root` | | `upper_arm_l`, `upper_arm_r` | `clavicle_l`, `clavicle_r` |
| `spine` | `hips` | | `lower_arm_l`, `lower_arm_r` | `upper_arm_l`, `upper_arm_r` |
| `chest` | `spine` | | `hand_l`, `hand_r` | `lower_arm_l`, `lower_arm_r` |
| `neck` | `chest` | | `upper_leg_l`, `upper_leg_r` | `hips` |
| `head` | `neck` | | `lower_leg_l`, `lower_leg_r` | `upper_leg_l`, `upper_leg_r` |
| `ponytail_1` | `head` | | `foot_l`, `foot_r` | `lower_leg_l`, `lower_leg_r` |
| `ponytail_2` | `ponytail_1` | | `toe_l`, `toe_r` | `foot_l`, `foot_r` |
| `ponytail_3` | `ponytail_2` | | | |

There are no finger bones yet (hands pose as a whole); the ponytail bones swing on spring
chains. The order in `SkeletonPose::names` is the one to index by; don't assume the order
of this table.

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
abdomen on a spring, and stout hairy legs with orange knees; the `threat` parameter rears
it up with its front legs raised and fangs out. A `DemoPath` can stop at waypoints
(`DemoPause`), easing a parameter up while it waits. The paddock's props wear the
renderer's generated materials (planks, concrete, sand, rock), and the spider climbs
[`build::rock`](src/build/rock.rs) boulders whose flat tops are its walkable boxes.

| Test case | What to look for |
|---|---|
| Hammer athlete | Two-handed sledgehammer strike onto a tractor tire: arms, shoulders and back tense on the strike and relax after; veins on the forearms; the ponytail whips; then a braced guard (`hammer_block`, held while `hammer_guard` is on): left foot forward, knees bent, the handle across the face with the left hand on it by `ReachIk` |
| Lifter | Front double-biceps: a breath in, the fists curl up and the elbows swing out to shoulder height, a hit that overshoots and settles, two squeezes (fists curl, shoulders lift, chest up, face strained), then the elbows drop and the arms shake loose; biceps, shoulders and chest bulge |
| Shirt reach | Up on tiptoe to a hanging lantern: a glance up and a dip, the elbow leads the arm up, `ReachIk` puts the hand on the glass while `grip` closes it, then down onto the heels with a give in the knees; the sleeve bunches at the elbow and rides up the raised arm |
| Step course | A walker leans into a ramp, crosses a deck, walks down stairs watching the steps with feet planted by IK and arms a little out, then runs back leaning into the turns (`paddock_stairs`) |
| Footwork drill | An athlete walks a square facing one way: ahead, sidestepping left, backpedalling, sidestepping right, blending through the corners, every planted foot still on the sand (`paddock_footwork`) |
| Shuttle lane | An athlete sprints the lane flat out from a standing start (leaning into it, then at 7 m/s with the trunk forward, knees driven high, heels kicked up and arms pumping from hip to chin), digs in to stop, runs back backwards, turns on the spot and runs sideways to the start; a coach beside the lane, arms folded, turns where they stand to keep watching, the head first and the feet stepping round once the runner is well off to one side (`paddock_sprint_start`, `paddock_sprint_stop`) |
| Sparring partner | A lifter in a boxer's guard (lead foot and fist forward, elbows tucked, bouncing lightly) blocks the boxer's jab, but a hook to the body gets under the guard to the liver and half a second later he folds onto his right knee, clutching his side, and gets up (`paddock_body_shot`); then he blocks the jab and cross on the gloves, takes the hook and uppercut to the face (the head snaps back and the feet step back to catch the body), a blow to the stomach (folds over it) and a shove on the back of a shoulder, flinching away from each and stepping back to the mark (`paddock_flinch`); in the last combo the guard drops early, the head shots leave his legs wobbling and a heavy uppercut knocks him down onto his back, braced (chin tucked, arms back), he lies a beat, gets up and comes back to the mark (`paddock_knockdown`) |
| Boxer | An athlete squares up to the sparring partner and throws jab, cross, hook and uppercut, each press buffered until the last punch's cancel window opens; she steps in to range on the jab and closer for the hook and uppercut, each fist reaching the face, and both freeze for a beat as each lands (`paddock_combo`) |
| Freeflow ring | An athlete tapping the light attack drops three dummies one after another with a jab, cross and hook each: the stick swings to the next as one goes down and she goes straight at it, a lunge to the one on the left, a leap to the one on the right, facing it before she lands and punching on touch-down; the dummies get up and walk back (`paddock_freeflow`) |
| Weapons yard | A swordsman cuts a slash, a rising backhand and a thrust into his partner; his overhead bounces off the partner's raised shield and the partner cuts back while he recovers; then a slash, his own shield rammed in and a thrust land; his shield comes up just as the partner's cut lands, swats it aside and he runs the partner through while it staggers open (a riposte: the two lined up, the partner sags on the blade, drops to a knee, falls on its back and gets up). Each struck body shakes through the hitstop. Between blows the two step back to measure and circle each other in guard. A great sword's sweep bounces off a dummy's raised shield, its overhead from high guard flings the shield open and a riposte runs the dummy through; it falls and gets up. An archer draws to the jaw and looses three arrows into a butt 12 m off, one held at full draw (`paddock_sword`, `paddock_shield`, `paddock_bash`, `paddock_circle`, `paddock_deflect`, `paddock_riposte`, `paddock_greatsword`, `paddock_greatsword_riposte`, `paddock_bow`, `paddock_butt`) |
| Horse oval | A textured horse walks, trots, canters and gallops by speed; in the swing the knees fold, the hocks bend and the hooves curl back behind the fetlocks; haunches and shoulders flex with each stride; the mane rides the neck and the tail streams. At the end of the lap it stops by a water trough and drinks, its neck reaching for the water, then grazes beside it (`paddock_gallop`, `paddock_horse_drink`, `paddock_horse_graze`) |
| Dog run | A furred Labrador walks off, trots, canters out of a turn and gallops down the far straight, the spine flexing with each stride, ears and tail swinging; it pants as it goes. Then it stops to shake off, casts about with its nose down, sits, lies down from the sit and gets up into a play bow (`paddock_dog_trot`, `paddock_dog`, `paddock_dog_shake`, `paddock_dog_sniff`, `paddock_dog_sit`, `paddock_dog_lie`, `paddock_dog_bow`) |
| Wall-run course | An athlete sprints down a lane between two staggered walls, jumps onto the left one and runs along it (feet on the wall, body tilted off it, inside arm out toward it), kicks across onto the right one, runs that, drops off and lands, then jogs back (`paddock_wallrun`, `paddock_wallkick`, `paddock_wallrun_right`, `paddock_wall_land`) |
| Spider garden | A furred red-knee tarantula climbs over boulders in alternating sets of legs, a wave running from the back legs forward; its palps tap, its jaws work and its abdomen sways (`paddock_spider`). On the far side it slows to a stop and rears up in threat: the front of the body tips up on the planted back legs, the first pair of legs rises high and the second reaches out level, the palps lift and the fangs unfold, then it settles back onto its footholds and walks on (`paddock_spider_threat`) |
| Car loop | A concept car: suspension over speed bumps, roll in corners, steering and spinning wheels with the brake calipers held still |
| Parked car | A second concept car inside the loop swings its doors open on their front hinges, holds, and shuts them with a bounce on the latch |
| Stunt yard | A stuntman climbs the stairs to a deck, sprints across and goes off the edge braced, arms out and knees bent; he goes limp on the crash mat with the run's momentum, lies still with a slack face, gets up off his back or front with a hand and a knee planted, and walks back round (`paddock_stunt`, `paddock_stunt_down`, `paddock_stunt_up`; needs the physics module) |
| Faces | The imported people blink at their own times and glance about; under effort (the lifter's flex, the hammer strike, the block) the eyes narrow, the brows knit and the teeth grit (`paddock_face`; needs the fetched characters) |
| Spectators | Three people in front of the stage who ask a character module for their look (`AppearanceRequest`): two follow the hammer down to the tire, one glances about with his arms folded, all blink and wince at each strike (the `wince` clip: shoulders up, arms drawn in and the hands up in front of the body; its `effort` squeezes the eyes and knits the brows through `FaceControls`). Each starts the graph at a different point (`AnimatorSettings::start_secs`), so they don't move in step; the generated bodies' standing idle settles the weight on one leg and then the other, arms hanging close at the sides. Without a character module they keep anim's bodies (`paddock_crowd`, `paddock_crowd_wince`, `paddock_crowd_glance`) |

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
`tests/motion_matching.rs` builds a database from the humanoid's clips given root motion
and checks paces and turns, that standing, walking, running and turning each play their
move with every jump blended (no bone turns over 0.3 rad in a frame), that motion-matched
walking fires footsteps, and that strafing hands over to the fallback and back; a 45°
diagonal hands over too unless `warp_max` is set, and then plays the forward walk.
`tests/warp.rs` turns the humanoid's walk toward a 45° diagonal (hips 45°, head within 1°,
the ankles' line swung with the hips), stretches a stride by 1.3 (each ankle reaches 30%
farther, keeps its height and stays within reach, the hips drop under 8 cm), and changes
nothing when standing or while the fallback shows. A unit test in `graph.rs` checks that an
inertialized transition carries on the old motion's velocity and settles without overshoot.
`tests/combat.rs` puts a boxer in front of a partner and checks that four presses link into
jab, cross, hook and uppercut that step in and all land with a few frames of hitstop, that
an early press waits in the buffer and a stale one is dropped, that mashing plays one move
at a time, that blows out of reach miss (with the warp it would have taken published), and
that a raised guard blocks all four on the gloves and pushes the boxer back.
`tests/sparring.rs` checks the paddock partner's guard blocks the boxer's jab and cross,
the hook and uppercut land once it drops, and the head snaps back from the first one.
`tests/hits.rs` punches a person and checks the head snaps back and recovers.
`tests/body_request.rs` asks for a hiker, a horse, a 40 cm dog and an unknown kind on
entities the test moves itself: the bodies walk, run and gallop from that motion, step on the ground, keep the
requester's own parameters, rebuild on a changed request and go with a removed one.
`tests/hit_physics.rs` punches a person on a stand-in backend that integrates forces: a
hard blow spawns bodies with kinematic legs, pushes the neck, moves the head beyond the
flinch while the feet stay within 5 mm, and the bodies are gone and the pose is the
animation's again a second later, and the neck and back bend less than 30° and 20° past
the flinch; a light blow only flinches.
`tests/knockdown.rs` (stand-in backend) knocks a person down with a 95 N·s blow: limp and
toned, braced to fall back, the parts pushed by about the blow; laid on its back, it waits a
second still, gets up where it lies and the knockdown is over. Without a backend the same
blow only staggers, and two 55 N·s blows close together knock down where one does not.
`tests/combat.rs` also checks a heavy press after the hook throws `uppercut_heavy` and that
presses at a downed target are dropped, and that a body hook lands under a raised guard
(the jab before it blocked) and presses wait while the target kneels, and at 8 frames a
second the body hook still lands.
`tests/freeflow.rs` rings a fighter with four dummies at 1.1, 3, 5 and 2 m (ahead, left,
right, behind), taps the light attack every 0.22 s with the stick on the next dummy still up,
and three blows put one down: all four go down in order, the left and behind ones after a
lunge and the right one after a leap, the next move starts within 0.3 s of each kill, no
blow whiffs, the warp never passes 18 m/s and the combo reaches 12. Unit tests in
`freeflow.rs` check the stick picks the foe it points at over a nearer one, the current
target is kept unless the stick points clearly elsewhere, no stick stays on it or takes the
nearest in front, and reach grows with the combo up to its limit.
Against a skilled `Evader` the jab, cross and hook all miss (`missed` 3, no hits or
blocks), slipped right, slipped left and rolled under, and the counter cross lands on the
attacker; with "evasive" off the combo lands.
`tests/hurt.rs` lands three quick blows on the chin: rocked, the hips sway more than 5 cm
from side to side and dip, the body lurches, and it wears off; a blow to the stomach folds
the chest forward with both hands within 20 cm of the stomach, and the wind comes back; a
liver blow takes the body down on a knee (hips under 0.72 of their height) only after the
delay, and it stands again.
`tests/stagger.rs` shoves a person in the chest: 40 N·s knocks them 0.1 to 0.5 m back, both
feet lift and end in their places in the stance, 10 N·s moves nothing, and with
`stagger_move` off only `stagger_velocity` says where to go.
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
