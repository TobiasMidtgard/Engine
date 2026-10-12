# engine_space — Space & planets

**Owner:** Physics & vehicles (or Planets & terrain). **Status:** stub. The owning workstream replaces this with the real module.
Background research: `game-engine-research/physics.md` in the project files.

## Scope

- Star systems, planets and moons as data (mass, radius, rotation, atmosphere, orbit).
- Orbital mechanics: patched conics or n-body for ships near bodies; sphere-of-influence switching; time warp via `TimeDilation` speed-ups.
- Reference frames: planet-fixed (rotating) vs inertial; keep the active play area near the floating origin.
- Feeds `engine_world::Gravity` bodies and `engine_render::Atmosphere` for the nearest planet, so sky and sunlight change as you fly between planets.
- Data model for the galaxy / system map UI.

## Contracts

Reads: Transforms (f64), `SimulationFocus`.

Writes: Gravity bodies, Atmosphere resource, DirectionalLight (star) direction.

Cross-module interaction goes through `engine_world` (see AGENTS.md). If a contract is missing, ask the core owner to add it rather than depending on another module's crate.

## Done when

- Fly from the surface of one planet to another in one continuous session with no precision jitter.

## Checklist

- Plugin registers its tunable components/resources (with docs) and `space.*` API functions.
- Fixed-step simulation in the right `SimSet`; honors `TimeDilation`/`LocalTimeScale`.
- Rendering through `RenderFeature`; shaders validated in a unit test.
- Unit tests plus a headless screenshot test; a scene in `games/showcase`.
