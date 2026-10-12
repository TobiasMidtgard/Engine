# engine_movement — Movement & time dilation gameplay

**Owner:** Movement and slow motion. **Status:** stub. The owning workstream replaces this with the real module.
Background research: `game-engine-research/animation.md` in the project files.

## Scope

- Titanfall-style locomotion on the physics character controller: sprint, slide, wall-run, wall-jump, mantle, double jump, air control, momentum.
- Combat movement: dashes that cover distance in a blink, lunges, evasions, lock-on.
- Perception and speed stats: as the player levels speed and reactions, fast enemies appear slower (player `LocalTimeScale` vs world `TimeDilation`).
- Cinematic slow motion when attacks land or the player strikes (Zoro / Sung Jin-Woo feel) via `TimeDilation` pulses; camera FOV and blur cues.
- Deterministic in `FixedUpdate`; uses `ActionState::pressed_since_tick`.

## Contracts

Reads: `MoveIntent` (from player input or NPC AI), physics contract (`CharacterController`/`CharacterState`, `PhysicsQueries` for wall and ledge detection), Time / TimeDilation, `AttackEvent` (slow motion on attacks).

Writes: `CharacterMove`, `LocomotionState`, `AnimationParams`/`AnimationTrigger`, TimeDilation requests, `LocalTimeScale`, GroundContact (landings), `NoiseEvent`.

Cross-module interaction goes through `engine_world` (see AGENTS.md). If a contract is missing, ask the core owner to add it rather than depending on another module's crate.

## Done when

- A high-level enemy dashes in and slices in a blink; after leveling up, the same enemy moves in visible slow motion and can be countered.

## Checklist

- Plugin registers its tunable components/resources (with docs) and `movement.*` API functions.
- Fixed-step simulation in the right `SimSet`; honors `TimeDilation`/`LocalTimeScale`.
- Rendering through `RenderFeature`; shaders validated in a unit test.
- Unit tests plus a headless screenshot test.
- A `demo` module with `pub fn spawn_demo(commands: &mut Commands, origin: DVec3)` that builds a small showcase of the feature around `origin`; `games/showcase` places it in its zone.
