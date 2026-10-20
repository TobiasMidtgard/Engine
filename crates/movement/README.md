# engine_movement

Titanfall-style movement, blade combat built on it, and slow motion that comes from each
character's own speed and reactions.

- **Locomotion**: run, sprint, crouch, slide (with a speed boost and slide-hops), jump, double
  jump, coyote time and jump buffering, air strafing that never loses momentum, wall-running
  (with a gravity curve and a time limit), wall-jumps, mantling onto ledges (also straight out
  of a wall-run), ledge hangs (catch a ledge too high to vault, shimmy along it, climb up with
  jump or by holding toward the wall, kick off it, or let go with crouch), dashes with
  charges and a dodge window, and a grapple hook (pilots only: fire at a surface up to 40 m
  away, the rope reels the pilot in at up to 20 m/s and climbs them over the edge at the top;
  jump, crouch or firing again lets go with the momentum), and zip lines (jump into a
  `Zipline` cable to catch it one-handed and slide down it, faster downhill and slower uphill;
  near the far anchor the rider lets go with a small hop, and jump, crouch or a dash lets go
  earlier, with the double jump back), and ladders (anything with the world's `Climbable`:
  walk or jump into one to grab it, climb with forward and back, faster sprinting, slide down
  holding crouch, step off over the top onto its exit or off the bottom, kick off with jump;
  a moving ladder carries its climber, and the rungs ring under each step), and vaults (over
  a railing, barrier or crate up to the profile's `vault_max_height` and `vault_max_depth`
  at a sprint, on a jump or running into it in the air: the body rises just enough to clear
  it, goes over with the left hand planted on its top and drops off the far side keeping
  95% of its speed; taller or deeper obstacles are mantled or hung from instead).
- **Combat movement**: attacks with windup, strike and recovery; dash-strikes that close
  several meters in a blink; blocks, dodges (i-frames), counters on a recovering attacker,
  stagger and knockback. Every strike is logged (`CombatLog`).
- **Time**: every character moves on its own clock. `Tempo::speed` makes an elite's whole
  clock run faster (dash, swing, recovery), and `Tempo::reflexes` is how much the player
  slows the world when something attacks them (reflex slow motion, "focus"). The player
  ignores that slow motion, so a trained player sees an elite's blink-fast dash in slow
  motion and has time to react. Cinematic slow-motion beats play on hits, kills and readable
  enemy attacks.
- **Feel**: afterimages behind fast moves (full-body ghosts, spaced in real time and
  distance, so a still frame shows the motion), a slow-motion screen effect (vignette, cool
  grade, speed lines while dashing) and camera cues (FOV kick, focus narrowing, wall-run
  roll).
- **Effects** (`look::effects`, tuned by `EffectSettings`): blade trails that cut a crescent
  behind every `look::Blade` as it strikes (one sharp arc at full speed, long and silky in
  slow motion), sparks when a strike is blocked, a crimson slash flash on a hit (a crossed
  cut and a flash light on a kill), and lit dust from landings, slides, dashes and wall
  kicks. Everything lives on the world clock, is pooled and capped, and is drawn by one
  render feature.

```rust,ignore
use engine::prelude::*;
use engine_movement::prelude::*;

app.add_plugins(MovementPlugin);

// A playable pilot: input actions → MoveIntent, relative to the active camera.
commands.spawn((
    Transform::from_xyz(0.0, 1.0, 0.0),
    MovementProfile::pilot(),
    PlayerControlled::default(),
    Fighter::new(1, 100.0),
    Tempo::level(10),   // speed 1.45, reflexes 5.5: attacks slow the world to 1/5.5
    Perceiver,          // this character's reflexes drive slow motion
    Afterimages::default(),
));

// An elite that dashes in and slices in a blink (to an untrained eye).
commands.spawn((
    Transform::from_xyz(0.0, 1.0, -12.0),
    MovementProfile::elite(),
    Fighter { damage: 35.0, dash_strike_range: 12.0, ..Fighter::new(2, 50.0) },
    Tempo { speed: 2.5, reflexes: 1.0 },
));
```

**Play it yourself**: where the demo is installed, press P (the `pilot` action, gamepad d-pad
down) to put a level 10 pilot on the ground in front of the camera, with a third-person
[`PilotCamera`](camera::PilotCamera) over its shoulder (mouse or right stick to look; the boom
pulls in at walls and the view's up follows gravity). Controls are the default action map:
move, jump (again in the air to double jump), sprint, crouch (slide while running), dash,
`grapple` (F, left bumper: the hook flies where the camera aims), attack (a dash-strike when
the enemy is far), block, and `lock_on` (middle mouse, right stick click): the pilot faces the enemy nearest the center of view, attacks go to it, the camera
keeps both in frame and a marker hangs over it. Escape frees the mouse; P again ends play
mode. `movement.play` and `movement.lock_on` do the same from the console.

NPC AI drives characters the same way the player does: write `MoveIntent` (direction, look,
sprint, crouch, jump, `dash_to`, `block`, `attack`) before `MovementSet::Intent`. Jump, dash
and attack are edge-triggered, so hold them for one step. Because movement is the installed
`LocomotionBackend`, it also moves bodies spawned by other modules: any entity with a
`MoveIntent` and a `CharacterController` but no `MovementProfile` gets
`MovementProfile::human()` and keeps its capsule. Bodies without a `CharacterController`
(animals, birds) are left to the module that spawned them.

## Showcase

`demo::spawn_demo(commands, origin)` builds the test cases around a point (they fill about
64 × 68 m, from x −28 to +36 and z −28 to +40). The showcase places it in its movement
course zone.

- **Parkour course**: a runner loops sprint → slide under a bar → wall-run → wall-jump →
  wall-run → mantle onto a tower → jump and double jump off it. Behind the tower a pilot
  climbs its caged ladder, steps off onto the roof, looks out over the course, then climbs
  back down a few rungs and slides the rest of the way.
- **Vault lane** (south of the runway): a pilot sprints down a 26 m lane and vaults a
  railing, a concrete barrier and a pair of strapped equipment cases without slowing down.
- **Ledge station** (between the course and the ring): a climber jumps at a 3.6 m block, too
  high to vault, catches its top, shimmies along it, climbs up and walks off the far end.
- **Grapple station** (behind the course): a pilot sprints at a 9 m tower, jumps, fires the
  hook at an anchor plate under its top edge, is reeled up the face, climbs over the edge,
  walks to the zip line gantry on the roof, runs and jumps into the line and rides it 26 m
  down to a landing gate, letting go over the pad.
- **Freight station** (west of the grapple station): a box car shuttles along a short track
  beside a loading platform. A pilot waits for it to pull out, sprints down the platform,
  leaps onto the car's side, wall-runs along it, climbs onto the roof, rides it to the end of
  the track and steps off.
- **Duel ring**, three rounds on a loop (only while a camera is within
  `DuelDirector::watch_radius`, because slow motion is world-wide):
  1. *Blink*: an elite (speed 2.5) dash-strikes a level 1 player, who is hit before a human
     could react (0.25 s).
  2. *Focus*: the same elite against the same player at level 10. The world slows to 1/5.5
     when the elite attacks; the player sidesteps the dash-strike and counters while the
     elite recovers.
  3. *Pack*: four grunts attack the level 10 player one after another. Each attack plays in
     slow motion and each grunt falls in a cinematic beat.

`cargo run -p engine_movement --release --example movement_filmstrip -- <dir>` renders the
key moments of the demo (and a contact sheet) headless, and
`cargo run -p engine_movement --release --example effects_preview -- <dir>` renders the
effects up close: a strike at full speed and in slow motion, a parry, a kill, slide dust and
dash afterimages. `fighter_preview` renders the four fighter styles, close-ups and animation
poses (`ladder` and `vault` render only those), and `course_preview` renders the course and
the ring from fixed cameras.

## Contracts

Reads: `MoveIntent`; the physics contract (`CharacterState` after each step, `PhysicsQueries`
raycasts for walls and ledges); `Climbable` (ladders); `Gravity` (up and strength at the
character); `Time`,
`TimeDilation`, `LocalTimeScale`; `ActionState` (player input; `PhysicsQueries` also aims the
player's grapple along the camera); `DamageEvent` (to the
characters it moves); `SkeletonPose` (when present, the animation module emits footsteps).

Writes: `CharacterMove` (velocity in the character's own seconds) and the capsule height
(`CharacterController::height` shrinks while sliding or crouching); `LocomotionState` (mode,
velocity in world seconds, blocking, attacking, vulnerable, dash stamina);
`AnimationParams` (`speed`, `direction`, `lean`, `crouch`, `effort`, `airborne`,
`vertical_speed`, `wallrun`, `grapple`, `zipline`, `ladder`, `climb_speed`) and
`AnimationTrigger` (`jump`, `double_jump`, `land`, `slide`, `wallrun`, `wall_jump`, `mantle`,
`ledge_grab`, `let_go`, `dash`, `grapple_fire`, `grapple_hit`, `grapple_release`, `zip_grab`,
`zip_release`, `ladder_grab`, `ladder_exit`, `vault`, `attack_light`, `attack_heavy`, `block`,
`hit_react`, `death`); `AttackEvent` (when an attack starts, with the world seconds until it lands);
`DamageEvent` (strikes); `Ragdoll::weight` on defeat; `GroundContact` (landings, footsteps,
slides, wall-run steps, rungs climbed, on the ladder collider's surface or metal, and the hand
planted on a vaulted top as `ContactKind::Body`); `NoiseEvent`; `LocalTimeScale` (from `Tempo`); `TimeDilation`
requests `"perception"` (focus) and `"cinematic"` (beats). Claims the `LocomotionBackend`.

### Without a physics module

Until a physics module is installed (no `PhysicsBackend` resource), a small stand-in moves
characters itself: static colliders become boxes, the `Ground` provides terrain, and raycasts
see both. It steps aside as soon as a physics backend claims the contract.

### Moving platforms and terrain

The physics backend carries a character standing on a moving body; movement keeps the deck's
velocity when the character jumps or walks off (a jump from a moving train keeps the train's
speed), takes it off again when it lands from the air (a jump straight up comes down on the
same spot), and turns the character with a turning deck (`platform`). Walls, ledges and
hooks ride along too: a character running along a moving wall, hanging from or climbing onto
a moving ledge moves with that body (physics only carries what stands on it, so movement
carries the rest), the ledge and a grapple hook are pinned to their body (`platform::Pin`), and
the rope reels in relative to a hook in a moving body. Where physics reports no
ground but the feet are on the [`Ground`](engine_world::Ground) surface (terrain that has no
collider yet), the character stands on the surface instead of falling through. A backend
that reports the ground before the feet touch it (within its snap distance) doesn't leave a
landing character hovering: it keeps coming down at its fall speed until the backend stops it.

The demo's freight car is a kinematic body moved by `demo::Shuttle`. With a physics backend
the shuttle sets the body's `Velocity` and the backend moves it (a kinematic body whose
`Transform` is written each frame doesn't move on Rapier, so nothing standing on it would be
carried); with only the stand-in it writes the `Transform`. After a teleport (a tour stop
restarting the loop) it stands still for a step so the backend places it first.

### World shifts

When the world re-anchors (floating origin, or a new planet's frame), `engine_world` moves
root transforms and the shared character state. Movement moves its own world-space state in
`WorldShiftSet::Apply`: velocities, facing, wall contacts, ledge targets, grapple anchors and
aims, dash directions, knockback, the course runner's, climber's, grappler's, rider's and
duel's anchor points, the freight car's track ends, afterimage ghosts, and effects in
flight (blade trails, sparks and their floors, slashes, dust, lights), so a wall-run, a dash
or a spray of sparks carries on through the shift.

### Look

`look` is the demo's procedural art, built once at startup and stored under keys
(`movement/<name>`; `look::mesh(key)`, `look::material(key)`; `LookSettings::texture_size`
sets the bake size). Nothing in it is third-party art.

- `look::meshgen` (rounded and chamfered boxes, lofts, lathes, extrusions, tubes) and
  `look::texgen` (tiling noise and a baker that turns a surface function into base color,
  normal, roughness/metal and occlusion maps).
- `look::materials`: shared tiling PBR materials sized in meters (concrete, precast panels,
  painted steel, brushed metal, rubber, hazard stripes, tread plate, floor boards, timber,
  granite, raked gravel, cloth, lacquer).
- `look::fighter`: armored sword fighters in four `Style`s (Pilot for the runner, Duelist for
  the player, Elite, Grunt), each a set of rigid armor parts with baked materials and a
  katana. The duelist and elite are armored samurai (laced lames, pleated hakama, sleeves of
  mail with laced splints; the duelist in a ribbed helmet with gilt horns over a sculpted
  face mask and a throat guard, the elite in a riveted black helmet over a snarling demon
  mask with glowing eyes and fangs); the pilot and grunt are soldiers in plate carriers with
  magazine pouches and cargo trousers, the pilot in a close helmet with a narrow visor and
  a respirator, the grunt in a gas mask under a brimmed helmet. The demo puts one on every
  character it spawns.
- `look::rig`: the fighters' skeleton and procedural animation. `Animator` turns the
  movement and combat state (on the character's own clock) into a pose: run cycles, slides,
  wall-runs, jumps, ledge hangs, grapple shots and reels, zip line rides, ladder climbs (the
  sword sheathed, hands and feet taking turns on the rungs, or clamping the rails on a
  slide), speed vaults (the left hand planted on the top, the legs swung out to the side
  over it, the sword arm out for balance), dashes, windups, strikes, blocks, hit reactions and falls, with two-bone IK for
  arms and legs.
- `look::rope`: the grapple's braided steel cable and three-clawed hook, laid each frame from
  the pilot's left fist (as drawn) to the hook: snaking behind it in flight, snapping taut
  when it bites, claws spread.
- `look::zipline`: zip line cables (a galvanized wire rope sagging between its anchors) for
  every `Zipline`, pulled into a shallow V through each rider's fist as drawn, with sparks
  streaming off a fast rider's glove.
- `look::course`: the course as a pilot training facility (precast wall panels with guide
  lights, a hazard-striped slide bar, a tower with a caged ladder, a start gantry, masts,
  numbered signs, the grapple station's nine-meter tower with its glowing anchor plate, the
  zip line's launch gantry on its roof and its landing gate over a rubber-matted pad, and
  the freight station: ballast, concrete sleepers and rails between buffer stops, a loading
  platform with granite coping and painted warnings, lamp posts, and a weathered box car on
  two bogies with sprung wheelsets, sliding doors, ladders, buffers and couplings, and the
  vault lane: a saw-cut pad with rubber landing mats, a yellow steel railing, a precast
  barrier and olive cases strapped onto pallets, with its own backlit sign)
  and the ring as a samurai duel arena (board floor in a carved stone band,
  lanterns, a torii, banners, a drum, a roofed stand), plus the level column. It adds no
  colliders: the demo's boxes stay the same and every surface a character touches is drawn
  on a box face.

## Tuning and AI access

Registered components: `MovementProfile` (presets `pilot`, `human`, `elite`), `Mover`,
`Fighter`, `CombatState`, `Tempo`, `Zipline`, `Perceiver`, `Afterimages`, `CameraFx`, `PlayerControlled`,
`LockOn`, `PilotCamera`, `DuelDirector`. Resources: `TimePerception` (focus and cinematic settings),
`PerceptionStatus`, `SlowMotionFx`.

API functions: `movement.status`, `movement.set_tempo` (`{"entity": "Player", "level": 10}`),
`movement.preset`, `movement.dash`, `movement.attack`, `movement.revive`,
`movement.combat_log`, `movement.perception` (read or patch the settings), `movement.play`
(`{"on": true}`: play mode), `movement.lock_on` (`{"target": "Elite" | "auto" | null}`),
`movement.duel`
(`{"round": "Blink" | "Focus" | "Pack" | null}`: restart the duel on that round and keep
replaying it, or `null` to go back to cycling; tour stops use it to catch a chosen moment),
`movement.climber` (restart the ledge climber's loop), `movement.ladder` (restart the
ladder climber's loop), `movement.vaulter` (restart the vaulter's run), `movement.grappler`
(restart the
grappler's loop), `movement.rider` (restart the freight rider's loop with the car about to
pull out).

## Tests

`cargo test -p engine_movement`: unit tests per file; `tests/locomotion.rs` (sprint, footprints
pointing toe-first, jump and double jump heights, slide under a bar that blocks running, wall-run and wall-jump, mantle,
catching a ledge too high to vault, shimmying to its end, climbing up, letting go and kicking
off, grappling up onto a roof, a shot at nothing coming back, a jump letting go of the rope,
humans having no hook, jumping into a zip line, riding it down and letting go at the end, a
jump letting go of it with the double jump back, zip lines being faster downhill than
uphill, a rider moving with its line, climbing a ladder onto a roof and grabbing it again
from the top to climb down, rungs ringing on metal, sliding down it and kicking off it,
catching it out of a jump, a moving ladder carrying its climber and one that goes dropping
it, sprinting over a low barrier without slowing down, a running jump vaulting where a run
does not, a jump too early for the vault vaulting out of the air, deep or tall obstacles and
drops beyond not being vaulted, wall-running on a wall that slides away and jumping off it with its
speed, hanging from a rising, sliding ledge and climbing onto it, a hook riding a moving roof,
dash, characters on their own clocks in slow motion, the stand-in stepping aside, bodies from
other modules being moved);
`tests/demo.rs` (each duel round plays out as described, the rounds cycle, the runner
finishes the course, the climber catches the ledge, shimmies, climbs up and drops off, the
grappler hooks the anchor plate, is reeled up, climbs over, rides the zip line down and lands
on its pad, the ladder climber climbs onto the tower and slides back down, the vaulter
vaults the railing, the barrier and the cases at a sprint, the rider
leaps onto the moving freight car, wall-runs along it, climbs aboard and steps off at the end
of the track, the duel
waits without a camera, `movement.duel` restarts a chosen round,
the demo's collider boxes stay the same under the new art);
`tests/play.rs` (play mode puts a pilot in front of the camera and follows it, a locked-on
pilot dash-strikes its target past a closer enemy, the lock moves on when its target falls);
`src/platform.rs` (a jump keeps a deck's speed and lands on the same spot, a turning deck
turns the character, a pin moves and turns with its body); `src/shift.rs` (a world shift moves walls, ledges, velocities and sparks
in flight); `tests/render.rs` (headless
screenshot with the slow-motion effect; skipped without a GPU).

## Not done yet

- Ledge corners (shimmying stops at the end of a straight edge).
- Starting a wall-run checks the runner's speed in the world's frame, not relative to the
  wall, so a runner keeping pace with a fast wall can start a run it drops right away.
