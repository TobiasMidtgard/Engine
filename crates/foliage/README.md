# engine_foliage — Foliage & ecology

**Owner:** Foliage & ecology. **Status:** stub. The owning workstream replaces this with the real module.
Background research: `game-engine-research/graphics.md` in the project files.

## Scope

- Procedural plants (trees, bushes, grass, mushrooms) from species parameters (L-systems / space colonization), with variation per plant.
- GPU instancing, culling, LOD and impostors for forests; grass bends in wind (`wind_offset`) and under feet/wheels.
- Grass blades are drawn with the renderer's shared strand pipeline (the same one hair and fur use), so dense grass, fur and hair get one optimized path.
- Ecology: each species needs resources (water, sunlight, soil nutrients, minerals such as mineral water); plants grow over time where resources allow, compete, spread seeds.
- Chop a tree: it falls (physics), leaves a stump, and new plants regrow where conditions allow.
- Cave mushrooms that drink mineral water and glow (emissive + point lights).
- Plants are `Combustible`; burnt plants die and regrow later.

## Contracts

Reads: Ground query, SurfaceLayers (wetness), WeatherState (sun, rain, temperature), `HeatField`, GroundContact (trampling).

Writes: `Combustible`, DamageEvent (falling trees), `WorldImpulse`.

Cross-module interaction goes through `engine_world` (see AGENTS.md). If a contract is missing, ask the core owner to add it rather than depending on another module's crate.

## Done when

- A forest that differs tree to tree, regrows after chopping, and a cave lit by glowing mushrooms.

## Checklist

- Plugin registers its tunable components/resources (with docs) and `foliage.*` API functions.
- Fixed-step simulation in the right `SimSet`; honors `TimeDilation`/`LocalTimeScale`.
- Rendering through `RenderFeature`; shaders validated in a unit test.
- Unit tests plus a headless screenshot test.
- A `demo` module with `pub fn spawn_demo(commands: &mut Commands, origin: DVec3)` that builds a small showcase of the feature around `origin`; `games/showcase` places it in its zone.
