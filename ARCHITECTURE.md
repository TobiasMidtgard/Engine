# Architecture

The engine is a Rust workspace of small crates around an ECS (`bevy_ecs`, used standalone).
Games depend on the `engine` facade crate and add `DefaultPlugins` plus whatever modules they
need. Every module is a plugin that registers its components, resources and API functions,
so the same feature can be driven by game code, data files, the console or an AI agent.

```
games/*            showcase, hello, (your games)
tools/*            engine-mcp (AI agent bridge), asset tools
crates/engine      facade: prelude, DefaultPlugins, CLI flags
crates/core        App, plugins, schedules, time & dilation, f64 transforms, math, registry/API, scenes, logs
crates/world       shared simulation contracts (weather, wind, heat, surfaces, gravity, ground/water queries, world events)
crates/assets      handles, asset storage, loaders, hot reload
crates/render      wgpu renderer + RenderFeature extension API
crates/input       devices + rebindable actions
crates/platform    winit window/event loop, headless runner
crates/<module>    one crate per system (below)
```

## Principles

1. **Large worlds first.** f64 world positions, camera-relative rendering, reverse-Z infinite
   projection. Physics and simulations run in local frames around the action
   (`SimulationFocus`), so a city, a planet and the space between planets all work.
2. **Every module is replaceable.** Modules never depend on each other's crates; they read
   and write the shared types in `engine_world`. Fire listens for `IgniteRequest` and heat;
   smoke listens for `ProjectileTrace` and `WindField`; footprints listen for `GroundContact`;
   vehicles, cloth, ragdolls and destruction use the physics *contract*, not a physics crate.
   Rip any module out, write a new one against the same contract, and plug it back in with
   one line in the game's `main` (see "Replacing a module").
3. **Everything is data and everything is callable.** Registered components/resources are
   JSON-patchable by name; API functions (`module.verb`) take and return JSON. This is the AI
   integration surface (`engine.describe` lists it all) and also the console, scripting and
   save-game surface.
4. **Time is a first-class resource.** World time dilation and per-entity time scales are in
   core, so movement, animation, AI, particles and physics all slow down together, and a
   player who has leveled up perception can act at full speed inside slow motion.
5. **Physical units.** SI units and photometric light units make scenes look right at noon,
   at dusk, in caves and in orbit with one setup.

## Frame

See `crates/core/src/schedule.rs` and AGENTS.md. Fixed-step simulation (`FixedUpdate`,
`SimSet` order: Input → Gameplay → Physics → PostPhysics → Environment), variable-rate
presentation (`Update`), then `Extract` (prepare render data) and `Render` (GPU frame:
prepare → compute → environment map → shadow cascades → opaque → sky → transparent → bloom →
tonemap → UI overlays → present).

## Modules and ownership

Each crate has one owner: a workstream with its own project thread. Owners change only their
own crates and their own showcase zone. Changes to the shared core crates (`core`, `world`,
`assets`, `input`, `platform`, `engine`, `ai_bridge`, `debug`) are made by the core owner, on
request; anything that touches an `engine_world` contract is reviewed by the core owner, who
also keeps `main` green. `engine_render` changes go through the Rendering and lighting owner.

Every workstream branches from `origin/main` and lands work through pull requests; nobody
pushes to `main` directly. In `games/showcase/src/zones.rs`, a module thread edits only its own
zone's entry; the rest of the showcase belongs to the Beautiful showcase world thread.

| Crate | Scope | Owner |
|---|---|---|
| `engine_core`, `engine`, `engine_world`, `engine_assets`, `engine_input`, `engine_platform` | Engine core, shared contracts, assets, input, windows and platforms | Core (architecture and integration) |
| `engine_ai_bridge`, `tools/engine-mcp` | JSON-RPC/MCP bridge so AI agents can inspect, edit, step and screenshot a running game | Core (architecture and integration) |
| `engine_debug` | Debug drawing (lines, shapes, gizmos) for every module | Core (architecture and integration) |
| `engine_render` | Renderer, lighting, shadows, sky and sky occlusion, materials; the planned strand renderer (hair, fur, grass), wrinkle maps, and 2D/2.5D rendering | Rendering and lighting |
| `games/showcase` | The tech showcase world: environment, viewpoints, screenshot tour, and the list of every test case with its owner | Beautiful showcase world |
| `engine_physics` | Rigid-body simulation, colliders, joints, ragdolls, queries, character controller, floating-origin physics islands | Physics and rigid bodies |
| `engine_vehicles` | Cars (suspension, tires, drivetrain), planes (aero surfaces), spaceships (thrusters, RCS) | Cars, planes and spaceships |
| `engine_softbody` | Cloth (characters, flags, sails), ropes and cables, soft bodies (jelly, flesh, soft tires), hair and fur dynamics; one XPBD solver with two-way rigid coupling, tearing | Cloth, soft bodies and hair |
| `engine_space` | Orbits, planets and moons, gravity wells, interplanetary travel, reference frames | Planets and terrain |
| `engine_terrain` | Heightmap/planet terrain with LOD, voxel volumes, ground queries, deformation (mud/snow tracks) | Planets and terrain |
| `engine_water` | Oceans, rivers, lakes, interactive water, buoyancy, splashes, rain filling puddles | Water simulation |
| `engine_weather` | Weather state machine, temperature, wind, volumetric clouds, rain/snow, day–night, surface wetness/snow | Weather, wind and clouds |
| `engine_fire` | Combustion, fire spread through fuel (grass, wood), heat, embers | Fire and smoke |
| `engine_smoke` | Smoke/gas volumes that react to wind, bullets and explosions; volumetric fog | Fire and smoke |
| `engine_vfx` | GPU particles (spells, splashes, sparks, debris), trails, decals | Particles and magic effects |
| `engine_foliage` | Procedural plants, growth from water/sun/soil/minerals, chopping and regrowth, glowing cave mushrooms | Foliage and ecology |
| `engine_anim` | Skeletons, clips, blend trees, IK, procedural animation for humanoids, animals, vehicles; muscle flex and jiggle; GPU skinning feature | Animation system |
| `engine_npc` | Perception, tactics, cover/terrain use, spacing, blocking, skill/reaction/logic parameters; wildlife and dynamic fauna | Smart NPC AI |
| `engine_ui` | Game UI: dialogue, shop, inventory, maps (flat, 3D globe, galaxy), HUD | UI system and maps |
| `engine_destruction` | Fracture, breakable walls (siege-style), voxel edits through the terrain API | Destructible terrain and buildings |
| `engine_movement` | Advanced locomotion (wall-run, slide, dash), combat movement, time-dilation gameplay (perception levels, slow-mo on attacks) | Movement and slow motion |
| `engine_audio` | Spatial audio, footsteps/impacts from surface materials | Audio and footsteps |
| `engine_script` | Hot-reloaded gameplay scripting | Later |

### Dependencies between modules: none

A module crate depends only on the core crates (`engine_core`, `engine_world`,
`engine_render`, `engine_assets`, `engine_input`). When one module needs another, it uses that
module's **contract** in `engine_world`:

| Contract (`engine_world::…`) | Implemented by | Used by |
|---|---|---|
| `physics`: `RigidBody`, `Collider`, `Velocity`, `ExternalForce`, `Joint`, `CharacterController`/`CharacterMove`/`CharacterState`, `ApplyImpulse`, `ContactEvent`, `PhysicsQueries` (raycast, shapecast, overlap, closest point) | `engine_physics` | vehicles, softbody, destruction, movement, npc, anim (ragdolls), space, gameplay |
| `animation`: `SkeletonPose`, `AnimationParams`, `AnimationTrigger`, `AnimationEvent`, `Ragdoll` | `engine_anim` | movement, npc, softbody (cloth/hair pinning), vfx, audio |
| `locomotion`: `MoveIntent`, `LocomotionState`, `VehicleControls` | `engine_movement`, `engine_vehicles` | npc, player input, cameras, UI |
| `terrain`: `TerrainEdit`, `TerrainChanged`, plus the `Ground` query | `engine_terrain` | destruction, foliage, physics, water, npc |
| `Water`, `Gravity`, fields and world events (see the crate docs) | water, space, weather, … | everyone |

Anything a module needs that isn't there is a missing contract: ask the core owner to add it
to `engine_world` rather than depending on another module's crate. The test
`crates/engine/tests/module_boundaries.rs` fails the build if a module crate depends on another.

### Replacing a module

1. Write the new crate against the same contract (the old crate's README lists what it reads
   and writes).
2. Pass the contract's conformance tests. Physics has a full suite:
   `engine_world::physics::conformance::assert_all(|app| { app.add_plugins(MyPhysics); })`
   (gravity, resting contact, impulses, contact events, raycasts, time dilation, precision
   100 000 km from the origin, the character controller). `crates/world/tests/physics_contract.rs`
   shows a 300-line backend that passes it.
3. Swap the plugin in the game: `app.add_plugins(MyPhysics)` instead of
   `engine_physics::PhysicsPlugin`. Nothing else changes. Physics backends call
   `claim_backend`, so accidentally running two is a clear error.

### How a module plugs in

```rust
pub struct FirePlugin;
impl Plugin for FirePlugin {
    fn build(&self, app: &mut App) {
        app.register_component::<Flammable>("…")       // tunable by data/AI
            .add_systems(FixedUpdate, spread_fire.in_set(SimSet::Environment))
            .add_render_feature(FireFeature::default()) // GPU drawing
            .register_api("fire.ignite", "…", "{…}", ignite_api);
    }
}
```

Reads: `Env` (weather, wind, heat, surfaces, gravity, ground/water queries, materials).
Writes: `HeatField`, `WorldImpulse`, `IgniteRequest`, `DamageEvent`, `NoiseEvent`,
`SurfaceLayers` (scorch), its own components.

## Rendering extension points

`RenderFeature` hooks (see `crates/render/src/features.rs`):

| Hook | Use |
|---|---|
| `prepare(world, …)` | read the ECS, cull, upload buffers |
| `compute(encoder, …)` | GPU simulation (particles, fluids, clouds) |
| `draw_environment` | add to the sky environment map (clouds, nebulae) |
| `draw_shadow(cascade)` | cast sun shadows; with `cascade == SKY_OCCLUSION_VIEW`, draw into the top-down sky occlusion map so whatever roofs over the ground (rock, buildings, canopies) keeps sky light and rain out |
| `draw(Opaque/Sky/Transparent)` | draw into the HDR scene; transparent can read scene color/depth copies |
| `overlay` | draw onto the final LDR image (UI, debug) |

**Sky occlusion** (built): a 512² top-down depth map of the highest surface within 128 m of
the camera. `sky_visibility(rel, normal)` in `lib_frame.wgsl` returns how much open sky a point
sees; shading and haze use it, and weather effects should use it to keep rain and snow off
covered ground. It integrates the view factor of the cover above the point out to 96 m, so it is
height-aware: a 10 m carport 3 m up leaves about 0.25 of the sky, a roof 10 m up about 0.75, a
roof 30 m up about 0.97 and an aircraft 100 m up next to nothing. The map sees cover from above
only, so walls don't keep the low sky out: caves, rooms, tunnels and ship interiors add a
`SkyLightVolume` (a box fitted to the inside, with the faces light comes in through, such as a
cave mouth; it follows its entity, so a ship's interior moves with the ship). Up to 16 volumes
nearest the camera are used. `RenderSettings::debug_view = SkyVisibility` shows the result.

Shared pipelines the core will provide for modules (planned): **strands** for hair, fur and grass
(guide curves in, GPU-expanded strands with hair shading and LOD out; `engine_softbody` drives
hair/fur, `engine_foliage` drives grass) and **tension-driven wrinkle maps** in `StandardMaterial`
(a per-vertex tension value blends wrinkle/vein normal maps; written by `engine_anim` for skin,
muscles and skinned clothes and by `engine_softbody` for simulated cloth).

Shader library: `lib_types.wgsl` (types, noise, BRDF) and `lib_frame.wgsl` (group 0 frame
data, `shade_surface`, `shadow_factor`, `sky_visibility`, `apply_fog`, atmosphere, `wind_offset`).

## AI integration

- `engine.describe` → every registered component, resource and API with docs and examples.
- `entity.*`, `resource.*`, `scene.*` → inspect and edit the world by name.
- Module APIs (`fire.ignite`, `weather.set`, `npc.set_skill`, …).
- `render.screenshot` → see the result; `input.*` → play the game.
- The AI bridge serves these over a socket (JSON-RPC) and as an MCP server, for a running
  windowed game or a headless instance.
