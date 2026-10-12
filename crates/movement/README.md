# engine_movement

Titanfall-style movement, blade combat built on it, and slow motion that comes from each
character's own speed and reactions.

- **Locomotion**: run, sprint, crouch, slide (with a speed boost and slide-hops), jump, double
  jump, coyote time and jump buffering, air strafing that never loses momentum, wall-running
  (with a gravity curve and a time limit), wall-jumps, mantling onto ledges (also straight out
  of a wall-run), and dashes with charges and a dodge window.
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
dash afterimages.

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
`wallrun`, `wall_jump`, `mantle`, `dash`, `attack_light`, `attack_heavy`, `block`,
`hit_react`, `death`); `AttackEvent` (when an attack starts, with the world seconds until it lands);
`DamageEvent` (strikes); `Ragdoll::weight` on defeat; `GroundContact` (landings, footsteps,
slides, wall-run steps); `NoiseEvent`; `LocalTimeScale` (from `Tempo`); `TimeDilation`
requests `"perception"` (focus) and `"cinematic"` (beats). Claims the `LocomotionBackend`.

### Without a physics module

Until a physics module is installed (no `PhysicsBackend` resource), a small stand-in moves
characters itself: static colliders become boxes, the `Ground` provides terrain, and raycasts
see both. It steps aside as soon as a physics backend claims the contract.

## Tuning and AI access

Registered components: `MovementProfile` (presets `pilot`, `human`, `elite`), `Mover`,
`Fighter`, `CombatState`, `Tempo`, `Perceiver`, `Afterimages`, `CameraFx`, `PlayerControlled`,
`DuelDirector`. Resources: `TimePerception` (focus and cinematic settings),
`PerceptionStatus`, `SlowMotionFx`.

API functions: `movement.status`, `movement.set_tempo` (`{"entity": "Player", "level": 10}`),
`movement.preset`, `movement.dash`, `movement.attack`, `movement.revive`,
`movement.combat_log`, `movement.perception` (read or patch the settings).

## Tests

`cargo test -p engine_movement`: unit tests per file; `tests/locomotion.rs` (sprint, jump and
double jump heights, slide under a bar that blocks running, wall-run and wall-jump, mantle,
dash, characters on their own clocks in slow motion, the stand-in stepping aside, bodies from
other modules being moved);
`tests/demo.rs` (each duel round plays out as described, the rounds cycle, the runner
finishes the course, the duel waits without a camera); `tests/render.rs` (headless
screenshot with the slow-motion effect; skipped without a GPU).

## Not done yet

- Ledge hang and climb-up animations come from the animation module; mantling is a motion only.
- Lock-on targeting for the player (attacks aim at the nearest enemy in front).
- Moving platforms: characters don't inherit a platform's velocity yet.
