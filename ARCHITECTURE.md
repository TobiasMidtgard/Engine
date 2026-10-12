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
2. **Contracts over coupling.** Modules don't depend on each other; they read and write the
   shared types in `engine_world`. Fire listens for `IgniteRequest` and heat; smoke listens for
   `ProjectileTrace` and `WindField`; footprints listen for `GroundContact`. A module can be
   replaced without touching the others.
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

Each module crate has one owner (a workstream). Owners change only their own crates. Changes
to shared crates (`core`, `world`, `render`, `assets`, `input`, `platform`, `engine`) are made
by the core owner, on request.

| Crate | Scope | Owner |
|---|---|---|
| `engine_core`, `engine`, `engine_world`, `engine_assets`, `engine_render`, `engine_input`, `engine_platform` | Engine core, shared contracts, renderer, assets, input, window | Core |
| `engine_ai_bridge`, `tools/engine-mcp` | JSON-RPC/MCP bridge so AI agents can inspect, edit, step and screenshot a running game | Core |
| `engine_debug` | Debug drawing (lines, shapes, gizmos) for every module | Core |
| `engine_audio` | Spatial audio, footsteps/impacts from surface materials | Core (until assigned) |
| `games/showcase` | The tech showcase test area | Core |
| `engine_physics` | Rigid bodies, colliders, joints, queries, character controller, floating-origin physics islands | Physics & vehicles |
| `engine_vehicles` | Cars (suspension, tires, drivetrain), planes (aero surfaces), spaceships (thrusters, RCS) | Physics & vehicles |
| `engine_space` | Orbits, planets and moons, gravity wells, interplanetary travel, reference frames | Physics & vehicles (or Planets) |
| `engine_terrain` | Heightmap/planet terrain with LOD, voxel volumes, ground queries, deformation (mud/snow tracks) | Planets & terrain (proposed) |
| `engine_water` | Oceans, rivers, lakes, interactive water, buoyancy, splashes, rain filling puddles | World simulation |
| `engine_weather` | Weather state machine, temperature, wind, clouds/rain/snow, day–night, surface wetness/snow | World simulation |
| `engine_fire` | Combustion, fire spread through fuel (grass, wood), heat, embers | World simulation |
| `engine_smoke` | Smoke/gas volumes that react to wind, bullets and explosions; volumetric clouds and fog | World simulation |
| `engine_vfx` | GPU particles (spells, splashes, sparks, debris), trails, decals | VFX (proposed) |
| `engine_foliage` | Procedural plants, growth from water/sun/soil/minerals, chopping and regrowth, glowing cave mushrooms | Foliage & ecology |
| `engine_anim` | Skeletons, clips, blend trees, IK, procedural animation for humanoids, animals, vehicles; GPU skinning feature | Animation |
| `engine_npc` | Perception, tactics, cover/terrain use, spacing, blocking, skill/reaction/logic parameters | NPC AI |
| `engine_ui` | Game UI: dialogue, shop, inventory, maps (flat, 3D globe, galaxy), HUD | UI |
| `engine_destruction` | Fracture, breakable walls (siege-style), voxel edits through the terrain API | Destruction |
| `engine_movement` | Advanced locomotion (wall-run, slide, dash), combat movement, time-dilation gameplay (perception levels, slow-mo on attacks) | Movement & time |
| `engine_script` | Hot-reloaded gameplay scripting | Later |

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
| `draw_shadow(cascade)` | cast sun shadows |
| `draw(Opaque/Sky/Transparent)` | draw into the HDR scene; transparent can read scene color/depth copies |
| `overlay` | draw onto the final LDR image (UI, debug) |

Shader library: `lib_types.wgsl` (types, noise, BRDF) and `lib_frame.wgsl` (group 0 frame
data, `shade_surface`, `shadow_factor`, `apply_fog`, atmosphere, `wind_offset`).

## AI integration

- `engine.describe` → every registered component, resource and API with docs and examples.
- `entity.*`, `resource.*`, `scene.*` → inspect and edit the world by name.
- Module APIs (`fire.ignite`, `weather.set`, `npc.set_skill`, …).
- `render.screenshot` → see the result; `input.*` → play the game.
- The AI bridge serves these over a socket (JSON-RPC) and as an MCP server, for a running
  windowed game or a headless instance.
