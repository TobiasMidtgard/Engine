# engine_physics — Physics

**Owner:** Physics & vehicles. **Status:** stub. The owning workstream replaces this with the real module.
Background research: `game-engine-research/physics.md` in the project files.

## Scope

- Rigid bodies, colliders (primitives, convex hulls, trimesh, heightfield), joints, sleeping, continuous collision for fast vehicles and projectiles.
- Scene queries: raycast, shapecast, overlap, with layer/filters. These are what AI perception, weapons, footsteps and cameras use.
- Kinematic character controller (steps, slopes, ground snapping, moving platforms) that `engine_movement` builds wall-running and dashing on.
- Planet-scale worlds: simulate in local physics islands/frames anchored at f64 positions around each `SimulationFocus` (floating origin), so a car on a moon 400 000 km away is as stable as one at the origin.
- Gravity from `engine_world::Gravity` (uniform or planet bodies), friction/restitution from `SurfaceMaterials`, buoyancy from the `Water` query.
- Runs in `FixedUpdate` / `SimSet::Physics`, honors `TimeDilation` and per-entity `LocalTimeScale`.
- rapier3d 0.36 is already a workspace dependency; wrap it so games never touch rapier types directly.

## Contracts

Reads: `Gravity`, `SurfaceMaterials`, Water query, `WorldImpulse`, `SimulationFocus`.

Writes: GroundContact (body impacts), NoiseEvent (impacts), DamageEvent (crush/impact, optional), Ground query implementation for colliders.

Cross-module interaction goes through `engine_world` (see AGENTS.md). If a contract is missing, ask the core owner to add it rather than depending on another module's crate.

## Done when

- A 1 000-body pile settles without jitter at 60 Hz on a laptop.
- A body 1e8 m from the origin behaves exactly like one at the origin.
- Raycasts/shapecasts available as API functions (`physics.raycast`).

## Checklist

- Plugin registers its tunable components/resources (with docs) and `physics.*` API functions.
- Fixed-step simulation in the right `SimSet`; honors `TimeDilation`/`LocalTimeScale`.
- Rendering through `RenderFeature`; shaders validated in a unit test.
- Unit tests plus a headless screenshot test; a scene in `games/showcase`.
