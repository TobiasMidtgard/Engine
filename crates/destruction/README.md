# engine_destruction — Destruction

**Owner:** Destructible terrain and buildings. **Status:** stub. The owning workstream replaces this with the real module.
Background research: `game-engine-research/physics.md` in the project files.

## Scope

- Pre-fractured (Voronoi) and runtime fracture of meshes; debris with physics, sleeping and cleanup.
- Siege-style destructible walls and barricades: holes punched by impacts, structural support graph so unsupported parts collapse.
- Voxel destruction through the terrain edit API (Valheim-style).
- Damage from `DamageEvent`/`WorldImpulse`; dust and debris effects via smoke/vfx.

## Contracts

Reads: `DamageEvent`, `WorldImpulse`, `ProjectileTrace`, `ContactEvent` impulses; spawns debris as physics-contract bodies.

Writes: WorldImpulse (collapses), `NoiseEvent`, `TerrainEdit` (voxel digging/blasting).

Cross-module interaction goes through `engine_world` (see AGENTS.md). If a contract is missing, ask the core owner to add it rather than depending on another module's crate.

## Done when

- Breach a reinforced wall with a charge; shoot holes in a wooden barricade; dig and collapse a voxel cliff.

## Checklist

- Plugin registers its tunable components/resources (with docs) and `destruction.*` API functions.
- Fixed-step simulation in the right `SimSet`; honors `TimeDilation`/`LocalTimeScale`.
- Rendering through `RenderFeature`; shaders validated in a unit test.
- Unit tests plus a headless screenshot test.
- A `demo` module with `pub fn spawn_demo(commands: &mut Commands, origin: DVec3)` that builds a small showcase of the feature around `origin`; `games/showcase` places it in its zone.
