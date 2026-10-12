# engine_movement

Titanfall-style movement, blade combat built on it, and slow motion that comes from each
character's own speed and reactions.

- **Locomotion**: run, sprint, crouch, slide (with a speed boost and slide-hops), jump, double
  jump, coyote time and jump buffering, air strafing that never loses momentum, wall-running
  (with a gravity curve and a time limit), wall-jumps, mantling onto ledges (also straight out
  of a wall-run), ledge hangs (catch a ledge too high to vault, shimmy along it, climb up with
  jump or by holding toward the wall, kick off it, or let go with crouch), and dashes with
  charges and a dodge window.
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
attack (a dash-strike when the enemy is far), block, and `lock_on` (middle mouse, right stick
click): the pilot faces the enemy nearest the center of view, attacks go to it, the camera
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
64 × 45 m, from x −28 to +36 and z −28 to +17). The showcase places it in its movement
course zone.

- **Parkour course**: a runner loops sprint → slide under a bar → wall-run → wall-jump →
  wall-run → mantle onto a tower → jump and double jump off it.
- **Ledge station** (between the course and the ring): a climber jumps at a 3.6 m block, too
  high to vault, catches its top, shimmies along it, climbs up and walks off the far end.
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
poses, and `course_preview` renders the course and the ring from fixed cameras.

## Contracts

Reads: `MoveIntent`; the physics contract (`CharacterState` after each step, `PhysicsQueries`
raycasts for walls and ledges); `Gravity` (up and strength at the character); `Time`,
`TimeDilation`, `LocalTimeScale`; `ActionState` (player input); `DamageEvent` (to the
characters it moves); `SkeletonPose` (when present, the animation module emits footsteps).

Writes: `CharacterMove` (velocity in the character's own seconds) and the capsule height
(`CharacterController::height` shrinks while sliding or crouching); `LocomotionState` (mode,
velocity in world seconds, blocking, attacking, vulnerable, dash stamina);
`AnimationParams` (`speed`, `direction`, `lean`, `crouch`, `effort`, `airborne`,
`vertical_speed`, `wallrun`) and `AnimationTrigger` (`jump`, `double_jump`, `land`, `slide`,
`wallrun`, `wall_jump`, `mantle`, `ledge_grab`, `let_go`, `dash`, `attack_light`, `attack_heavy`, `block`,
`hit_react`, `death`); `AttackEvent` (when an attack starts, with the world seconds until it lands);
`DamageEvent` (strikes); `Ragdoll::weight` on defeat; `GroundContact` (landings, footsteps,
slides, wall-run steps); `NoiseEvent`; `LocalTimeScale` (from `Tempo`); `TimeDilation`
requests `"perception"` (focus) and `"cinematic"` (beats). Claims the `LocomotionBackend`.

### Without a physics module

Until a physics module is installed (no `PhysicsBackend` resource), a small stand-in moves
characters itself: static colliders become boxes, the `Ground` provides terrain, and raycasts
see both. It steps aside as soon as a physics backend claims the contract.

### Moving platforms and terrain

The physics backend carries a character standing on a moving body; movement keeps the deck's
velocity when the character jumps or walks off (a jump from a moving train keeps the train's
speed), takes it off again when it lands from the air (a jump straight up comes down on the
same spot), and turns the character with a turning deck (`platform`). Where physics reports no
ground but the feet are on the [`Ground`](engine_world::Ground) surface (terrain that has no
collider yet), the character stands on the surface instead of falling through.

### World shifts

When the world re-anchors (floating origin, or a new planet's frame), `engine_world` moves
root transforms and the shared character state. Movement moves its own world-space state in
`WorldShiftSet::Apply`: velocities, facing, wall contacts, ledge targets, dash directions,
knockback, the course runner's and duel's anchor points, afterimage ghosts, and effects in
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
  katana. The demo puts one on every character it spawns.
- `look::rig`: the fighters' skeleton and procedural animation. `Animator` turns the
  movement and combat state (on the character's own clock) into a pose: run cycles, slides,
  wall-runs, jumps, dashes, windups, strikes, blocks, hit reactions and falls, with two-bone
  IK for arms and legs.
- `look::course`: the course as a pilot training facility (precast wall panels with guide
  lights, a hazard-striped slide bar, a tower with a caged ladder, a start gantry, masts,
  numbered signs) and the ring as a samurai duel arena (board floor in a carved stone band,
  lanterns, a torii, banners, a drum, a roofed stand), plus the level column. It adds no
  colliders: the demo's boxes stay the same and every surface a character touches is drawn
  on a box face.

## Tuning and AI access

Registered components: `MovementProfile` (presets `pilot`, `human`, `elite`), `Mover`,
`Fighter`, `CombatState`, `Tempo`, `Perceiver`, `Afterimages`, `CameraFx`, `PlayerControlled`,
`LockOn`, `PilotCamera`, `DuelDirector`. Resources: `TimePerception` (focus and cinematic settings),
`PerceptionStatus`, `SlowMotionFx`.

API functions: `movement.status`, `movement.set_tempo` (`{"entity": "Player", "level": 10}`),
`movement.preset`, `movement.dash`, `movement.attack`, `movement.revive`,
`movement.combat_log`, `movement.perception` (read or patch the settings), `movement.play`
(`{"on": true}`: play mode), `movement.lock_on` (`{"target": "Elite" | "auto" | null}`),
`movement.duel`
(`{"round": "Blink" | "Focus" | "Pack" | null}`: restart the duel on that round and keep
replaying it, or `null` to go back to cycling; tour stops use it to catch a chosen moment),
`movement.climber` (restart the ledge climber's loop).

## Tests

`cargo test -p engine_movement`: unit tests per file; `tests/locomotion.rs` (sprint, jump and
double jump heights, slide under a bar that blocks running, wall-run and wall-jump, mantle,
catching a ledge too high to vault, shimmying to its end, climbing up, letting go and kicking
off, dash, characters on their own clocks in slow motion, the stand-in stepping aside, bodies from
other modules being moved);
`tests/demo.rs` (each duel round plays out as described, the rounds cycle, the runner
finishes the course, the climber catches the ledge, shimmies, climbs up and drops off, the duel waits without a camera, `movement.duel` restarts a chosen round,
the demo's collider boxes stay the same under the new art);
`tests/play.rs` (play mode puts a pilot in front of the camera and follows it, a locked-on
pilot dash-strikes its target past a closer enemy, the lock moves on when its target falls);
`src/platform.rs` (a jump keeps a deck's speed and lands on the same spot, a turning deck
turns the character); `src/shift.rs` (a world shift moves walls, ledges, velocities and sparks
in flight); `tests/render.rs` (headless
screenshot with the slow-motion effect; skipped without a GPU).

## Not done yet

- Ledge hangs on moving bodies, and corners (shimmying stops at the end of a straight
  edge).
- Wall-running on moving walls (only standing on a moving body carries a character).
