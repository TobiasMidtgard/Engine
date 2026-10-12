# engine_terrain — Terrain

**Owner:** Planets and terrain. **Status:** stub. The owning workstream replaces this with the real module.
Background research: `game-engine-research/graphics.md` in the project files.

## Scope

- Heightmap terrain with continuous LOD (CDLOD / clipmaps) and streaming; planet-scale cube-sphere quadtree terrain.
- Voxel volumes for caves and dig/build (Valheim-style) with smooth meshing (marching cubes / dual contouring); edit API used by `engine_destruction`.
- Implements the `Ground` query (height, normal, surface material) so every module can stand on, drive over and grow on terrain.
- Surface material splatting; deformation layers (footprints, tire tracks, snow trails) driven by `GroundContact` and `SurfaceMaterial::deformation`.
- Physics colliders for terrain chunks (coordinate with physics).

## Contracts

Reads: `GroundContact`, SurfaceLayers (wetness/snow), `SimulationFocus`.

Writes: Ground query, SurfaceLayers (deformation).

Cross-module interaction goes through `engine_world` (see AGENTS.md). If a contract is missing, ask the core owner to add it rather than depending on another module's crate.

## Done when

- Walk in mud, grass and snow and see footprints; dig a tunnel; fly from orbit down to the ground without popping.

## Checklist

- Plugin registers its tunable components/resources (with docs) and `terrain.*` API functions.
- Fixed-step simulation in the right `SimSet`; honors `TimeDilation`/`LocalTimeScale`.
- Rendering through `RenderFeature`; shaders validated in a unit test.
- Unit tests plus a headless screenshot test.
- A `demo` module with `pub fn spawn_demo(commands: &mut Commands, origin: DVec3)` that builds a small showcase of the feature around `origin`; `games/showcase` places it in its zone.
