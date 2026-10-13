# engine_vehicles — Vehicles

**Owner:** Cars, planes and spaceships. **Status:** simulation, autopilots, demos and
end-to-end tests done and running on the Rapier backend in the showcase; detailed procedural
models for every vehicle and for the track, airfield and spaceport.

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
| Models | `models/cars`, `models/aircraft`, `models/scenery`, `visuals.rs` | Procedural meshes with baked PBR textures, built once per kind and shared: sports and rally cars (~145k triangles each: sculpted bodies, liveries, lights, interiors, wheels with discs and calipers), the sport plane (120k) and the spaceplane (165k), and the scenery of the three demo zones. `visuals.rs` spins and steers wheels, moves control surfaces and calipers, sizes plumes, lights brake lamps and makes hot hulls glow. |
| Presets | `presets.rs` | `sports_car`, `rally_car`, `sport_plane`, `spaceplane` (by name through `presets::spawn`). |
| AI tools | `api.rs` | `vehicles.presets`, `vehicles.spawn`, `vehicles.list`, `vehicles.control`, `vehicles.autopilot`, `vehicles.telemetry`. |
| Demos | `demo.rs` | `spawn_track` (circuit, kicker, drift pad, two racing cars, a drifting rally car), `spawn_airfield` (a runway, a plane taking off and one flying circuits) and `spawn_spaceport` (a spaceplane hopping off the pad, one parked). `spawn_demo` builds all three; the showcase puts them in its `track`, `airfield` and `spaceport` zones. Their scenery is solid with or without a renderer (`models/scenery/solid.rs`): Armco, walls and catch fences, tire walls, gravel traps that bury a car's tires, the grandstand, pits, transporters, gantry and footbridge; the hangars (open doors stay open), fire station, tower, terminal and ground equipment; the spaceport's hangars, blast walls, tank farm and towers. |

## Contracts

Reads: `VehicleControls`, the physics contract (`RigidBody`, `Collider`, `Velocity`,
`MassProperties`, `PhysicsQueries` raycasts, `ContactEvent`), `Gravity` (uniform and planets),
`Atmospheres` (the shared air: weather-driven standard atmosphere or per-planet air),
`SurfaceMaterials`, `SurfaceLayers` (mud, snow, wetness), `Ground` (flat worlds without
colliders), `WeatherState` and `WindField`.

Adds to `SurfaceMaterials` when missing: `gravel_trap` (loose pea gravel, rolling resistance
0.6) and `tire_wall` (soft, barely bounces).

Writes: `ExternalForce`, `RigidBody` mass, center of mass and inertia (each preset sets real
values; mass and inertia follow propellant as it burns), `EngineState` and `WheelContacts` on
every vehicle each step (audio plays engines and tires from them), `GroundContact`,
`NoiseEvent`, `SmokeEmission`, `WorldImpulse`, `DamageEvent`, `ApplyImpulse`, `WindField` gusts.

## Tests

Physics comes from a small stand-in solver in `tests/support` written only against the
contract (module crates don't depend on a backend); the showcase runs the same vehicles on
`engine_physics`.

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
- `tests/scenery_solids.rs`: nothing solid within 2 m of the racing line or 3 m of the drift
  circle (the gantry truss and footbridge span the track above 5.5 m), the airfield's
  buildings stay 30 m off the runway's center line and below the circuit, the spaceport's 31 m
  from the pad; a car running wide at 11 m/s stops in the gravel before the tire wall, and
  one hitting the outer wall at 30° stays on the track side of it.
- `tests/screenshots.rs`: renders the cars, the aircraft and the track, airfield and spaceport
  demos (set `VEHICLE_SHOTS=<dir>` to keep the PNGs; helpers in `tests/support/shots.rs`).
- `tests/shots_cars.rs`, `tests/shots_aircraft.rs`, `tests/shots_scenery.rs`: model budgets
  (triangles; scenery build under 1 s per zone) always, and close-up galleries only with
  `VEHICLE_SHOTS=<dir>` set (they take many minutes on a software GPU).

## Done when

- A car drives, drifts and jumps convincingly; a plane takes off and stalls; a ship flies from a
  planet surface into orbit. ✓ (tests on the stand-in solver; in the showcase on Rapier the
  cars race and drift, the plane takes off and the spaceplane hops)
- The models hold up in close-up next to the rest of the showcase. ✓ (galleries in
  `engine-screenshots/vehicles/` in the project files)
- Vehicles honor `LocalTimeScale` (time dilation bubbles). ✓ Every subsystem steps by
  `VehicleFrame::dt` (`Time::delta_for` with the vehicle's `LocalTimeScale`), and the backend
  moves the body in the same seconds, so a car in a 0.5 bubble does in 4 world seconds what
  another does in 2 (`tests/cars.rs`).
