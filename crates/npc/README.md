# engine_npc — NPC AI

**Owner:** Smart NPC AI. **Status:** stub. The owning workstream replaces this with the real module.
Background research: `game-engine-research/scripting.md` in the project files.

## Scope

- Perception: sight cones with occlusion raycasts, hearing (`NoiseEvent`), memory and suspicion.
- Decision making: utility AI / GOAP / behavior trees, with planning depth set by a logic skill.
- Tactics: use cover, high ground and chokepoints; flank; space the player (keep range, circle, feint); block, parry and dodge; coordinate in groups.
- Editable skills per NPC: reaction time, accuracy, logic capability, aggression, courage, learning.
- Debug views through `engine_debug`.

## Contracts

Reads: `PhysicsQueries` (line of sight, cover), `NoiseEvent`, `DamageEvent`, `AttackEvent` (to block and dodge), Ground query, `LocomotionState` and `SkeletonPose` of targets.

Writes: `MoveIntent` (the same input a player produces), `VehicleControls` when driving, `AttackEvent` (telegraphs), NoiseEvent (shouts).

Cross-module interaction goes through `engine_world` (see AGENTS.md). If a contract is missing, ask the core owner to add it rather than depending on another module's crate.

## Done when

- Enemies that feel smarter than other games: they take cover, flank, punish mistakes, and block.

## Checklist

- Plugin registers its tunable components/resources (with docs) and `npc.*` API functions.
- Fixed-step simulation in the right `SimSet`; honors `TimeDilation`/`LocalTimeScale`.
- Rendering through `RenderFeature`; shaders validated in a unit test.
- Unit tests plus a headless screenshot test.
- A `demo` module with `pub fn spawn_demo(commands: &mut Commands, origin: DVec3)` that builds a small showcase of the feature around `origin`; `games/showcase` places it in its zone.
