# engine_fire — Fire

**Owner:** World simulation. **Status:** stub. The owning workstream replaces this with the real module.
Background research: `game-engine-research/particles.md` in the project files.

## Scope

- Combustion on a grid near the focus (`HeatField`): fuel from `Combustible` and `SurfaceMaterials` (grass, wood, leaves), moisture from rain and wetness, oxygen, wind-driven spread.
- Fire moves through burnable objects (grass fields, trees, wooden walls) and burns out, leaving scorch (`SurfaceLayers::scorch`).
- Ignition from `IgniteRequest`, explosions (`WorldImpulse` with heat), lightning, spells.
- Rendering: flames (volumetric or particle), embers, light (PointLights), smoke emission handed to `engine_smoke`.
- Damage to entities in flames (`DamageEvent`).

## Contracts

Reads: `IgniteRequest`, `WindField`, SurfaceLayers (wetness), WeatherState (rain), `Combustible`.

Writes: `HeatField`, SurfaceLayers scorch, `DamageEvent`, `SmokeEmission`.

Cross-module interaction goes through `engine_world` (see AGENTS.md). If a contract is missing, ask the core owner to add it rather than depending on another module's crate.

## Done when

- Light a grass field: fire spreads downwind, stops at wet ground and rivers, trees catch and burn out.

## Checklist

- Plugin registers its tunable components/resources (with docs) and `fire.*` API functions.
- Fixed-step simulation in the right `SimSet`; honors `TimeDilation`/`LocalTimeScale`.
- Rendering through `RenderFeature`; shaders validated in a unit test.
- Unit tests plus a headless screenshot test; a scene in `games/showcase`.
