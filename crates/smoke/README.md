# engine_smoke — Smoke, gas & volumetric clouds

**Owner:** World simulation. **Status:** stub. The owning workstream replaces this with the real module.
Background research: `game-engine-research/particles.md` in the project files.

## Scope

- Volumetric smoke/gas on a 3D grid near the focus (advection, buoyancy from heat, dissipation).
- Reacts to bullets (`ProjectileTrace` punches tunnels through smoke), explosions (`WorldImpulse`), wind (`WindField`) and moving bodies.
- Volumetric rendering with sun shadowing and light scattering.
- True volumetric clouds (replace the sky's 2D cloud layer through `draw_environment` + sky phase) and volumetric fog.

## Contracts

Reads: `SmokeEmission`, `ProjectileTrace`, `WorldImpulse`, `WindField`, `HeatField`, WeatherState (cloud cover).

Writes: (rendering only) optional NoiseEvent for gas hiss.

Cross-module interaction goes through `engine_world` (see AGENTS.md). If a contract is missing, ask the core owner to add it rather than depending on another module's crate.

## Done when

- Shoot through a smoke grenade cloud and see the bullet path clear; fly through volumetric clouds.

## Checklist

- Plugin registers its tunable components/resources (with docs) and `smoke.*` API functions.
- Fixed-step simulation in the right `SimSet`; honors `TimeDilation`/`LocalTimeScale`.
- Rendering through `RenderFeature`; shaders validated in a unit test.
- Unit tests plus a headless screenshot test; a scene in `games/showcase`.
