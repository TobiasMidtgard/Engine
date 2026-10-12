# engine_vehicles — Vehicles

**Owner:** Physics & vehicles. **Status:** stub. The owning workstream replaces this with the real module.
Background research: `game-engine-research/physics.md` in the project files.

## Scope

- Cars: raycast/shapecast suspension, tire model with slip (Pacejka-style), engine torque curve, gearbox, differential, brakes, handbrake drift, downforce.
- Planes: aerodynamic surfaces (lift/drag curves, stall), control surfaces, thrust, landing gear.
- Spaceships: thrusters and RCS, flight assist / inertia dampening, atmosphere entry (drag + heating), docking.
- Enter/exit, seats, cameras; wheels write `GroundContact { kind: Wheel }` so mud/snow show tracks.

## Contracts

Reads: engine_physics bodies/queries, `Gravity`, SurfaceMaterials (tire friction), Water (boats/amphibious), Atmosphere density (via space/weather).

Writes: GroundContact (Wheel/Track), NoiseEvent (engines), WorldImpulse (crashes).

Cross-module interaction goes through `engine_world` (see AGENTS.md). If a contract is missing, ask the core owner to add it rather than depending on another module's crate.

## Done when

- A car drives, drifts and jumps convincingly; a plane takes off and stalls; a ship flies from a planet surface into orbit.

## Checklist

- Plugin registers its tunable components/resources (with docs) and `vehicles.*` API functions.
- Fixed-step simulation in the right `SimSet`; honors `TimeDilation`/`LocalTimeScale`.
- Rendering through `RenderFeature`; shaders validated in a unit test.
- Unit tests plus a headless screenshot test; a scene in `games/showcase`.
