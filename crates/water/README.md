# engine_water — Water

**Owner:** World simulation. **Status:** stub. The owning workstream replaces this with the real module.
Background research: `game-engine-research/graphics.md` in the project files.

## Scope

- Oceans (FFT or Gerstner spectrum, foam, LOD to the horizon), lakes and rivers with flow.
- Interactive surface near the camera: ripples, wakes and splashes from bodies, players, rain and explosions (GPU heightfield sim).
- Implements the `Water` query (surface height, velocity, normal) for buoyancy, swimming and boats.
- Rendering: refraction (scene color copy), reflections, depth absorption, caustics, underwater fog.
- Rain fills puddles (`SurfaceLayers::puddle_m`) and drains them.

## Contracts

Reads: `WindField`, WeatherState (rain), `WorldImpulse`, `GroundContact`, Ground query.

Writes: Water query, SurfaceLayers puddles, NoiseEvent (splashes).

Cross-module interaction goes through `engine_world` (see AGENTS.md). If a contract is missing, ask the core owner to add it rather than depending on another module's crate.

## Done when

- Run into a lake and see ripples follow you; boats bob; rain makes rings and puddles.

## Checklist

- Plugin registers its tunable components/resources (with docs) and `water.*` API functions.
- Fixed-step simulation in the right `SimSet`; honors `TimeDilation`/`LocalTimeScale`.
- Rendering through `RenderFeature`; shaders validated in a unit test.
- Unit tests plus a headless screenshot test.
- A `demo` module with `pub fn spawn_demo(commands: &mut Commands, origin: DVec3)` that builds a small showcase of the feature around `origin`; `games/showcase` places it in its zone.
