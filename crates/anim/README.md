# engine_anim — Animation

**Owner:** Animation. **Status:** stub. The owning workstream replaces this with the real module.
Background research: `game-engine-research/animation.md` in the project files.

## Scope

- Skeletons and clips from glTF (skins, animations); GPU skinning as a `RenderFeature` using `shade_surface`.
- Blend trees, state machines, layered/masked blending, additive, root motion, animation events.
- IK: two-bone, FABRIK/CCD, foot placement on `Ground`, look-at, hand IK for weapons.
- Procedural animation: spider/creature legs, tails and spines (spring chains), animals (quadruped gaits), vehicles (suspension, landing gear).
- Ragdolls and physical animation with `engine_physics`.
- Respects `TimeDilation` and `LocalTimeScale`; footstep events become `GroundContact`.

## Contracts

Reads: Ground query, Time / LocalTimeScale, physics bodies.

Writes: GroundContact (Foot/Paw/Hoof), NoiseEvent (footsteps).

Cross-module interaction goes through `engine_world` (see AGENTS.md). If a contract is missing, ask the core owner to add it rather than depending on another module's crate.

## Done when

- A humanoid walks/runs/turns with foot IK on slopes; a quadruped and a procedural creature move believably.

## Checklist

- Plugin registers its tunable components/resources (with docs) and `anim.*` API functions.
- Fixed-step simulation in the right `SimSet`; honors `TimeDilation`/`LocalTimeScale`.
- Rendering through `RenderFeature`; shaders validated in a unit test.
- Unit tests plus a headless screenshot test; a scene in `games/showcase`.
