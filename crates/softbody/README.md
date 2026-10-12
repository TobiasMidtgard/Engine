# engine_softbody — Cloth, ropes, soft bodies, hair and fur

**Owner:** Cloth, soft bodies and hair. **Status:** stub. The owning workstream replaces this with the real module.
Background research: `game-engine-research/physics.md` (cloth and soft-body sections: Chaos Cloth/Flesh, Jolt and Godot `SoftBody3D`, XPBD) in the project files.

Rigid bodies live in the physics module; this crate simulates everything that bends, stretches or squishes and couples it to those rigid bodies both ways, through the physics contract only (so either module can be replaced).

## Scope

- One XPBD particle/constraint solver with substeps, used for all of the below. CPU for gameplay-relevant pieces, GPU compute (`RenderFeature::compute`) for large purely visual cloth.
- **Cloth:** capes, clothing on characters (pinned to bones from `engine_world::animation::SkeletonPose`), flags, sails, curtains, tents. Stretch, bend and shear constraints, self-collision, collision with physics colliders and characters, wind from `WindField`, tearing (bullets, blades, fire), wetness making cloth heavier.
- **Ropes and cables:** grappling lines, rope bridges, power lines, tow cables between vehicles; attach to rigid bodies with two-way forces.
- **Hair and fur:** a hair strand is a tiny rope, so it uses the same solver. Simulate a few hundred to a few thousand guide strands per character or animal (XPBD chains with bend/twist stiffness, inertia that follows the moving head or body, collision against body capsules placed on `SkeletonPose` bones, wind, wetness clumping) and interpolate the rendered strands from them. Styles as data: length, curl, clumping, stiffness, color/melanin. Fur on animals uses short guides and shell or strand rendering at distance.
- **Soft bodies:** volumetric bodies (tetrahedral or shape-matching): jelly, slimes, creature flesh/bellies, soft tires and cushions, foliage that bends under weight. Plasticity for dents that stay.
- Two-way coupling: soft bodies push rigid bodies and vice versa, through `PhysicsQueries::closest_point`/`overlap` and `ApplyImpulse` from the physics contract.
- LOD and sleeping: full simulation near `SimulationFocus`, simplified or frozen far away; skinned fallback when off-screen.
- Cloth writes per-vertex tension (stretch vs compression from the solver) so the material shows wrinkles where fabric bunches and smooths where it pulls tight, at no extra simulation cost.
- Hair, fur and grass share the renderer's strand pipeline (`engine_render` strands: GPU-expanded camera-facing strands from guide curves, hair shading with two specular lobes, LOD from strands to cards to shells). `engine_foliage` uses the same pipeline for grass, with wind sway instead of full dynamics.
- Renders deformed meshes through a `RenderFeature` (dynamic vertex buffers, normals recomputed) using `shade_surface`, with `subsurface` for flesh and thin translucent cloth.
- Runs in `FixedUpdate` / `SimSet::Physics` (after rigid bodies), honors `TimeDilation` and `LocalTimeScale`.

## Contracts

Reads: `WindField`, `Gravity`, `WorldImpulse` (explosions push cloth), `ProjectileTrace` (bullets pierce/tear), `HeatField` / `IgniteRequest` (burning cloth), `SurfaceLayers` (rain wetness), `SimulationFocus`; physics contract (`PhysicsQueries`, `Velocity`, `MassProperties`); animation contract (`SkeletonPose`).

Writes: `ApplyImpulse` (pushing rigid bodies), `WorldImpulse` (soft bodies hitting things), `NoiseEvent` (flapping, impacts), `GroundContact` (soft bodies landing), its own components.

No dependencies on other module crates (see ARCHITECTURE.md). Everything goes through `engine_world`; if a contract is missing, ask the core owner to add it.

## Done when

- A cape on a running, dashing character stays on the body without jitter or tunneling, and reacts to wind and to slow motion.
- A flag field (50+ flags) costs under 1 ms on the GPU; a sail pushes a boat.
- A rope bridge carries a player and swings; cutting a rope drops it.
- A jelly cube wobbles, stacks on rigid crates and gets knocked over by them.
- A bullet through a curtain leaves a hole; fire burns cloth away.
- Long hair on a running, dashing character swings and settles without passing through the head or shoulders; a dog's fur ripples in wind; both hold up in slow motion.

## Checklist

- Plugin registers its tunable components/resources (with docs) and `softbody.*` API functions.
- Fixed-step simulation in the right `SimSet`; honors `TimeDilation`/`LocalTimeScale`.
- Rendering through `RenderFeature`; shaders validated in a unit test.
- Unit tests plus a headless screenshot test.
- A `demo` module with `pub fn spawn_demo(commands: &mut Commands, origin: DVec3)` that builds a small showcase of the feature around `origin`; `games/showcase` places it in its zone.
