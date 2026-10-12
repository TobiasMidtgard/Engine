# engine_anim — Animation

**Owner:** Animation system. **Status:** stub. The owning workstream replaces this with the real module.
Background research: `game-engine-research/animation.md` in the project files.

## Scope

- Skeletons and clips from glTF (skins, animations); GPU skinning as a `RenderFeature` using `shade_surface`.
- Blend trees, state machines, layered/masked blending, additive, root motion, animation events.
- IK: two-bone, FABRIK/CCD, foot placement on `Ground`, look-at, hand IK for weapons.
- Procedural animation: spider/creature legs, tails and spines (spring chains), animals (quadruped gaits), vehicles (suspension, landing gear).
- Ragdolls and physical animation built from physics-contract components (`RigidBody`, `Collider`, `Joint` with motors), driven by `Ragdoll::weight`.
- **Implements the animation contract** (`engine_world::animation`): reads `AnimationParams`/`AnimationTrigger`, writes `SkeletonPose` every frame and `AnimationEvent` at marked frames, so movement, AI, cloth and hair never depend on this crate.
- **Muscles:** per-muscle activation computed from what the body is doing (joint angles plus effort: lifting, punching, sprinting, bracing for a block), driving corrective blend shapes (biceps bulge, forearm flex, neck tendons, calves on push-off) and tension maps that fade in veins/striations. Relaxed muscle and fat get secondary jiggle (spring-driven, cheap). Optional later: an ML deformer trained offline on a full muscle simulation for hero characters. Blend shapes run in the skinning compute pass; full detail only near the camera.
- Skinned clothing that isn't simulated still wrinkles: the skinning pass compares edge lengths to the rest pose and writes a per-vertex tension value for the material's wrinkle maps.
- Respects `TimeDilation` and `LocalTimeScale`; footstep events become `GroundContact`.

## Contracts

Reads: Ground query, Time / LocalTimeScale, physics bodies.

Writes: GroundContact (Foot/Paw/Hoof), NoiseEvent (footsteps).

Cross-module interaction goes through `engine_world` (see AGENTS.md). If a contract is missing, ask the core owner to add it rather than depending on another module's crate.

## Done when

- A humanoid walks/runs/turns with foot IK on slopes; a quadruped and a procedural creature move believably.
- Arms visibly tense on a heavy swing or a block and relax after; a horse's flank muscles flex at a gallop; a shirt bunches at the elbow and stretches across the back on a reach.

## Checklist

- Plugin registers its tunable components/resources (with docs) and `anim.*` API functions.
- Fixed-step simulation in the right `SimSet`; honors `TimeDilation`/`LocalTimeScale`.
- Rendering through `RenderFeature`; shaders validated in a unit test.
- Unit tests plus a headless screenshot test.
- A `demo` module with `pub fn spawn_demo(commands: &mut Commands, origin: DVec3)` that builds a small showcase of the feature around `origin`; `games/showcase` places it in its zone.
