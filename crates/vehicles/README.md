# engine_vehicles — Vehicles

**Owner:** Cars, planes and spaceships. **Status:** round 1 (simulation, autopilots, demos and
end-to-end tests done; models being rebuilt at full detail).

Cars, planes and spaceships for a game that travels between planets. Everything is built on the
`engine_world` physics contract, so any physics backend that passes the conformance suite runs
these vehicles unchanged.

## What's in it

| Part | File | What it does |
| --- | --- | --- |
| Chassis | `chassis.rs` | `Vehicle`, `VehicleForces` (per-step force accumulator), `VehicleTelemetry`, body aero (`AeroBody`), `VehicleSettings`. |
| Wheels | `wheels.rs`, `tire.rs` | Raycast suspension, Pacejka magic-formula tires with combined slip and load sensitivity, surface grip from `SurfaceMaterials` and wetness/snow layers, ABS, traction control (a torque limit inside the wheel substeps), stability control, steering limit, anti-roll bars, static friction so cars park on slopes. |
| Drivetrain | `drivetrain.rs` | Engine torque curve, clutch, gearbox with automatic shifting on road speed (with hysteresis), open/locked/limited-slip differentials, AWD splits. |
| Aircraft | `aero.rs` | Lifting surfaces with full-range lift/drag/moment (attached flow, stall blend, separated flow), flaps and control surfaces, trim, propellers and jets. |
| Spacecraft | `ship.rs` | Thrusters (main, hover, RCS) with a least-squares allocator that favors attitude, flight assist modes (off, rotation, full, hover), propellant mass, orbits (periapsis/apoapsis), Sutton–Graves re-entry heating with radiative cooling. |
| Autopilots | `autopilot.rs` | Drive a path, drift a circle, take off and fly waypoints, stall and recover, launch to orbit, hold a point, hop off a pad and land. They write `VehicleControls` like a player or the NPC module would. |
| Events | `events.rs` | Wheel tracks (`GroundContact`), engine `NoiseEvent`s, tire and exhaust `SmokeEmission`, rocket wash fanning out along the ground (`WindField`), crash `WorldImpulse` and `DamageEvent`, heat damage. |
| Models | `models/`, `meshes.rs`, `visuals.rs`, `presets.rs` | Procedural meshes and materials; spinning, steering wheels; moving control surfaces; engine plumes; lights; hull glow when hot. |
| Presets | `presets.rs` | `sports_car`, `rally_car`, `sport_plane`, `spaceplane` (by name through `presets::spawn`). |
| AI tools | `api.rs` | `vehicles.presets`, `vehicles.spawn`, `vehicles.list`, `vehicles.control`, `vehicles.autopilot`, `vehicles.telemetry`. |
| Demos | `demo.rs` | `spawn_track` (circuit, kicker, drift pad, two racing cars, a drifting rally car), `spawn_airfield` (a runway, a plane taking off and one flying circuits) and `spawn_spaceport` (a spaceplane hopping off the pad, one parked). `spawn_demo` builds all three; the showcase puts them in its `track`, `airfield` and `spaceport` zones. |

## Contracts

Reads: `VehicleControls`, the physics contract (`RigidBody`, `Collider`, `Velocity`,
`MassProperties`, `PhysicsQueries` raycasts, `ContactEvent`), `Gravity` (uniform and planets),
`Atmospheres` (the shared air: weather-driven standard atmosphere or per-planet air),
`SurfaceMaterials`, `SurfaceLayers` (mud, snow, wetness), `Ground` (flat worlds without
colliders), `WeatherState` and `WindField`.

Writes: `ExternalForce`, `RigidBody` mass, center of mass and inertia (each preset sets real
values; mass and inertia follow propellant as it burns), `EngineState` and `WheelContacts` on
every vehicle each step (audio plays engines and tires from them), `GroundContact`,
`NoiseEvent`, `SmokeEmission`, `WorldImpulse`, `DamageEvent`, `ApplyImpulse`, `WindField` gusts.

## Tests

Physics comes from a small stand-in solver in `tests/support` written only against the
contract (until the physics module lands).

- `tests/cars.rs`: settles on its springs; reports engine rpm, load and wheel contacts; 0–100 km/h in 4–6.5 s, shifts up through the
  gears and tops out at 250–310 km/h in a straight line; brakes from 100 km/h in 30–48 m
  without leaving the lane; ice takes over 5× longer; holds a 50 m skidpad at ≥ 0.8 g; a
  handbrake turn swings the tail out; the rally car holds a 25° drift on a 15 m circle on dirt;
  jumps a kicker and lands on its wheels; parks on a 15° slope; leaves tracks in mud and makes
  noise.
- `tests/aircraft.rs`: takes off in under 230 m and climbs; flies a circuit holding height
  (±15 m) and speed through 35° banked turns; the stall demo stalls and recovers; with the
  engine off it trims itself into a 7–16 : 1 glide.
- `tests/spacecraft.rs`: hops off a pad, hovers and lands softly; climbs from the ground into a
  stable orbit around a small planet (vertical lift, pitch up, gravity turn, coast,
  circularize); re-entry from orbit heats the shield past 1 200 K without burning through.
- `tests/screenshots.rs`: renders the cars, the aircraft and the track, airfield and spaceport
  demos (set `VEHICLE_SHOTS=<dir>` to keep the PNGs; helpers in `tests/support/shots.rs`).

## Done when

- A car drives, drifts and jumps convincingly; a plane takes off and stalls; a ship flies from a
  planet surface into orbit. ✓ (on the stand-in solver; to re-check on the real backend)
- The models hold up in close-up next to the rest of the showcase.
- Vehicles honor `LocalTimeScale` (time dilation bubbles). World slow motion already works;
  a per-vehicle time scale needs the physics contract to integrate rigid bodies in entity time.
