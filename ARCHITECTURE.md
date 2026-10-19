# Architecture

The engine is a Rust workspace of small crates around an ECS (`bevy_ecs`, used standalone).
Games depend on the `engine` facade crate and add `DefaultPlugins` plus whatever modules they
need. Every module is a plugin that registers its components, resources and API functions,
so the same feature can be driven by game code, data files, the console or an AI agent.

```
games/*            showcase, hello, (your games)
tools/*            engine-mcp (AI agent bridge), fetch-assets (large art, see AGENTS.md)
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
| `tools/fetch-assets`, `assets/manifests/core.toml` | Large art pipeline: manifests, download cache, release mirror, the no-large-files test (each thread owns its own `assets/manifests/<area>.toml`) | Core (architecture and integration) |
| `engine_debug` | Debug drawing (lines, shapes, gizmos) for every module | Core (architecture and integration) |
| `engine_render` | Renderer, lighting, shadows, sky and sky occlusion, materials, grass; the planned strand renderer (hair, fur), wrinkle maps, and 2D/2.5D rendering | Rendering and lighting |
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
`engine_render`, `engine_assets`, `engine_input`, `engine_debug`). When one module needs another, it uses that
module's **contract** in `engine_world`:

| Contract (`engine_world::…`) | Implemented by | Used by |
|---|---|---|
| `physics`: `RigidBody`, `Collider`, `Velocity`, `ExternalForce`, `Joint`, `CharacterController`/`CharacterMove`/`CharacterState`, `ApplyImpulse`, `ContactEvent`, `PhysicsQueries` (raycast, shapecast, overlap, closest point) | `engine_physics` | vehicles, softbody, destruction, movement, npc, anim (ragdolls), space, gameplay |
| `animation`: `SkeletonPose`, `AnimationParams`, `AnimationTrigger`, `AnimationEvent`, `Ragdoll`, `BodyRequest` (a person or animal on an entity someone else moves) | `engine_anim` | movement, npc, softbody (cloth/hair pinning), vfx, audio (trail walkers) |
| `locomotion`: `MoveIntent`, `LocomotionState`, `VehicleControls`, `LocomotionBackend` (`claim_backend`, so fallback movers can step aside), `Climbable` (ladders any module places) | `engine_movement`, `engine_vehicles` (`Climbable` placed by destruction and games, climbed by movement) | npc, player input, cameras, UI |
| `EngineState`, `WheelContacts`, `Exhausts`: what a vehicle's engine and wheels are doing, and where each nozzle is and how hard it fires (`ExhaustEffects`/`draws_exhausts`, so stand-in plumes step aside) | `engine_vehicles` (also movement's jump kits for `Exhausts`) | audio (engine and tyre sounds), ui (HUDs), npc (hearing), vfx (spray, flames, plumes, smoke) |
| `air`: `Atmospheres` per planet and `Env::air(p)` (density, pressure, temperature, speed of sound, wind, altitude) | games and `engine_space` tune `Atmospheres`; weather drives sea-level conditions | vehicles, weather, audio, vfx, smoke, space |
| `terrain`: `TerrainEdit`, `TerrainChanged`, plus the `Ground` query | `engine_terrain` | destruction, foliage, physics, water, npc |
| `Obscurance`: smoke, fog and dust between two points (`transmittance`, `extinction`); sources stack by name and implementations pass `check_obscurance` | `engine_smoke` (weather fog and destruction dust can add their own) | npc, gameplay, ui |
| `WorldShift` (+ `WorldShiftSet`, `FollowsWorldShift`): the world was re-anchored (floating origin, a new planet's frame). `engine_world` moves transforms, velocities, forces, intents, gravity, wind and sun, the fields, and pending contract messages; query providers answer in the new frame from `WorldShiftSet::Apply` on | written by `engine_space` | every module or crate that keeps world positions or directions of its own shifts them in `WorldShiftSet::Apply`: physics (bodies, statics, query caches), softbody, vehicles, movement, npc (memories, nav goals), anim (IK targets), vfx, fire, smoke, water, weather, terrain, foliage, destruction, audio, ui (map markers), render (sky and planet centers), debug |
| `Water`, `Gravity`, fields and world events (see the crate docs) | water, space, weather, … | everyone |

Anything a module needs that isn't there is a missing contract: ask the core owner to add it
to `engine_world` rather than depending on another module's crate. The test
`crates/engine/tests/module_boundaries.rs` fails the build if a module crate depends on another.

### Replacing a module

1. Write the new crate against the same contract (the old crate's README lists what it reads
   and writes).
2. Pass the contract's conformance tests. Physics has a full suite:
   `engine_world::physics::conformance::assert_all(|app| { app.add_plugins(MyPhysics); })`
   (gravity, resting contact, impulses, contact events with start, end, impulse and normal,
   raycasts, time dilation, precision 100 000 km from the origin, the character controller and
   its per-character time scale). `crates/world/tests/physics_contract.rs`
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
| `cloud_shadows()` | ask for the top-down cloud shadow map (`CloudShadowRequest`), then draw it in `compute` into `ctx.cloud_shadows` (see "Cloud shadows") |
| `draw_environment` | add to the sky environment map (clouds, nebulae); it is filtered afterwards, so what is drawn here lights every surface and shows in rough reflections |
| `draw_prepass` | write depth, normal, roughness and motion for opaque geometry (`vs`/`fs` using `prepass_output` from `lib_frame.wgsl`, targets `GpuShared::prepass_targets()`); ambient occlusion, contact shadows and TAA motion work only on surfaces drawn here, and the scene pass must then write the same depth (mark clip positions `@invariant`) |
| `draw_shadow(cascade)` | cast sun shadows; with `cascade == SKY_OCCLUSION_VIEW`, draw into the top-down sky occlusion map so whatever roofs over the ground (rock, buildings, canopies) keeps sky light and rain out |
| `draw_far_shadow` | big casters only (terrain, mountains, cliffs, large buildings; cull with `ctx.view.far_shadow.casts(center, radius)`): the coarse far shadow map that shades the land beyond the cascades |
| `draw(Opaque/Sky/Transparent)` | draw into the HDR scene; transparent can read scene color/depth copies |
| `overlay` | draw onto the final LDR image (UI, debug) |

**Sun shadows** (built): four cascades out to `RenderSettings::shadow_distance` (rotation-
invariant spheres snapped to texels, PCSS filtering), then a **far shadow map** out to
`far_shadow_distance` (8 km by default; 0 = off): one `shadow_map_size`² map around the camera
(texels of about 8 m), snapped to world texels, in the shadow array's last layer
(`FAR_SHADOW_LAYER`). Only casters at least two of its texels across draw into it
(`draw_far_shadow`; the mesh feature, instanced and clustered, does), so it costs little, and
mountains shade the valley at low sun and ridges shade themselves kilometers away.
`shadow_factor` blends from the cascades into it over their last tenth; `DebugView::Cascades`
shows its region in magenta.

**Cloud shadows** (built; `cloud_shadows.rs`): a top-down map of the sunlight the clouds let
through, frame binding 22 (`cloud_shadow_map`), anchored to the world. The module that draws
clouds (weather) asks for it from its render feature: `RenderFeature::cloud_shadows()` returns a
`CloudShadowRequest` (texels per side, meters across, the cloud layer's `base_height` and
`top_height` in world y, the transmittance `outside` the map), and `compute` then gets
`ctx.cloud_shadows`: a view of the map (`CLOUD_SHADOW_FORMAT`, Rgba16Float, a render target and
a storage texture, already cleared to 1) and the `CloudShadowArea` it covers this frame. The
renderer places the square around the camera snapped to whole texels, so texels never swim as
the camera moves. Red holds the sunlight reaching the cloud base plane down the sun direction;
shading reads it where the sun ray through the shaded point crosses that plane, so shadows fall
away from the sun and are right at any height below the clouds, fade out inside the layer, and
beyond the square take the `outside` value (the map fades into it over its outer tenth).
`shade_surface`, `shade_thin_surface`, grass, the volumetric fog's sunlight, the height fog's
sun glow beyond it and the planet's far ground multiply the sun by it; other crates' shaders
call `cloud_shadow(rel)`. Writers draw a fullscreen triangle and get the camera-relative point on
the cloud base under a texel from `cloud_shadow_plane_point(uv)`, or rasterize with
`frame.cloud_shadow` (camera-relative → map clip space, projected along the sun); during
`compute` the frame group binds a white stand-in at 22, so the pass that draws the map can bind
it. Green, blue and alpha are not read: they are free for the writer's own top-down data
(group 0 binds 15 sampled textures with the ground weather map; don't add another). Until a
module draws it the map stays 1, the lookup is skipped and every image is unchanged;
`RenderSettings::cloud_shadows` turns it off.

**Sky occlusion** (built): a 512² top-down depth map of the highest surface within 128 m of
the camera. `sky_visibility(rel, normal)` in `lib_frame.wgsl` returns how much open sky a point
sees; shading and haze use it, and weather effects should use it to keep rain and snow off
covered ground. It integrates the view factor of the cover above the point out to 96 m, so it is
height-aware: a 10 m carport 3 m up leaves about 0.25 of the sky, a roof 10 m up about 0.75, a
roof 30 m up about 0.97 and an aircraft 100 m up next to nothing. The map sees cover from above
only, so walls don't keep the low sky out: caves, rooms, tunnels and ship interiors add a
`SkyLightVolume` (a box fitted to the inside, with the faces light comes in through, such as a
cave mouth; it follows its entity, so a ship's interior moves with the ship). Up to 16 volumes
nearest the camera are used. Walls and ceilings under cover read the map with a small bias (only
upward-facing ground gets the large one), and a surface counts as inside a volume when it or the
air just in front of it is, so corners, seams and rounded edges indoors stay as dark as the
walls around them. Sky light that does reach a covered point came in through openings and
bounced on the way, so it arrives more evenly from all around the less sky the point sees
(`sky_ambient_covered(normal, sky_vis)`, used by `shade_surface`): a ceiling indoors isn't lit
by the sunlit ground outside (olive by default), nor a cave's floor by the blue sky overhead.
`shade_surface` looks the cover up with the surface's smooth normal (`Surface::smooth_normal`,
the interpolated vertex normal; `StandardMaterial` meshes set it), not the normal-mapped one, so
cover doesn't change wrinkle by wrinkle on a strong normal map; the shading normal still picks
which part of the sky lights the point. Shaders in other crates that build a `Surface` with
`default_surface()` and a normal map should set `smooth_normal` too (left at zero it falls back
to `normal`; `surface_smooth_normal(s)` resolves it). Reflected sky is occluded along the
reflection, not just by the cover overhead: `specular_sky_visibility(rel, dir, smooth_normal,
roughness)` marches the map outward along the reflection's heading (18 taps from about 1 m to
55 m, each a quarter farther, shifted per pixel so TAA averages walls thinner than the far gaps)
and hides the sky where cover rises above the reflection, so a forest pond or a wet courtyard
doesn't mirror the bright low sky that the trees or walls around it hide (with SSR off or off
screen, the hidden part is left to the ambient fill). Cover at the surface's own level doesn't
count, so open water stays bright to the horizon. `specular_horizon_fade(r, smooth_normal)`
(Russell's horizon occlusion) fades reflections that a normal map tilts below the surface's own
horizon. `shade_surface` takes the smaller of `sky_visibility` and the directional value for
glossy surfaces (rough ones, whose lobe is wide, use the cover overhead alone; the march is
skipped from perceptual roughness 0.71) and applies the fade to all. `RenderSettings::debug_view =
SkyVisibility` shows the result.

**Ground weather** (built): `SurfaceLayers` (the weather's 512 m grid of 1 m cells around the
focus: wetness, standing water, snow, scorch) is uploaded as a texture (frame binding 21,
`Rgba8Unorm` in the weather module's encoding: wetness, standing water / 0.04 m, snow / 0.3 m,
scorch) whenever it changes (change detection, at most 10 times a second while it keeps changing)
or moves, and drawn where the uploaded grid lay. `surface_weather(rel)` in `lib_frame.wgsl` returns
that vec4 at a camera-relative point, filtered, in any stage (`SURFACE_PUDDLE_SCALE_M`,
`SURFACE_SNOW_SCALE_M` decode it); terrain and foliage shaders use it to match the meshes. Beyond
the grid (fading in over its outer tenth) it is the open ground's state, the upper quartile of the
grid's rim (`frame.surface_open`), as the terrain uses there. The grid describes the ground, so
under a roof it is dry; a surface with nothing above it in the sky occlusion map that stands on
ground much drier than the open ground is the cover itself (a roof, a vehicle's top, a canopy) and
gets the open ground's wetness and snow instead, leaving puddles and scorch to the ground below.
Materials with `receives_wetness` (the default) read it per pixel (walls half a meter in front of
them): scorch singes and then chars them to matte soot, snow settles on top faces, and rain wets
them by porosity, which follows roughness (Lagarde 2013): rough, porous surfaces (roughness 0.8 and
up: grass, soil, unsealed stone) darken to about half and lose only a fifth of their roughness,
while smooth, sealed ones (0.35 and below: paint, glazed tile, metal) hardly darken and gloss to
roughness 0.1. Puddle-level gloss on porous ground is the weather's own ground pass.
`render.surface_weather_uploads` in `Diagnostics` counts the uploads.

**Screen-space effects** (built): every frame starts with a depth prepass that also writes
normals, roughness and motion vectors. From it come ground-truth ambient occlusion (GTAO,
`RenderSettings::ambient_occlusion`, `ao_radius`, `ao_intensity`; `screen_ao(rel)` in shaders,
applied to ambient light only, with multi-bounce color and specular occlusion), screen-space
contact shadows toward the sun (`contact_shadow_length`), and temporal anti-aliasing
(`anti_aliasing = Taa`, the default). Under TAA the projection is jittered by a sub-pixel
offset every frame (`frame.jitter`); `frame.unjittered_view_proj` and `prev_view_proj` give
stable positions, and overlays get an unjittered frame group. Sun shadows use PCSS: the
penumbra widens with distance from the caster like the real sun's (`sun_penumbra_scale`). Screen-space
reflections (`screen_space_reflections`, `ssr_max_roughness`; `screen_reflection(rel)` in
shaders, used by `shade_surface`) trace the prepass depth and read the previous frame's finished
image, so polished floors, wet streets, metal and glossy paint mirror what is on screen, and fall
back to the prefiltered sky elsewhere. Glossy reflections gather over the GGX lobe's footprint at
the hit from a blurred mip chain of that image, so a rough floor shows a soft glow of the window
rather than a sharp image broken up by its bumps; they fade out over the upper half of
`ssr_max_roughness`. A ray that passes over the sky before anything hides it (out through a
window or a gap in the canopy) reflects what the camera sees there, or the sky in its exact
direction for a mirror, even indoors. `DebugView::Reflections` shows them. Screen-space global illumination (`screen_space_gi`, `gi_intensity`; `screen_gi(rel)` in
shaders, added by `shade_surface` times the diffuse color) traces short cosine-distributed rays
through the prepass depth into the same previous image, so sunlit walls tint the ground beside
them and lamps, fires and other emissive surfaces light their surroundings; light keeps bouncing
over frames, and a temporal blend plus an edge-aware blur clean it up. Rays that leave the screen
or find nothing bring no light (the sky's share is the AO-darkened sky light).
`DebugView::IndirectLight` shows the bounced light.

**Grass** (built): a `GrassField` component covers a rectangle of ground on its entity's local
x/z plane with individually drawn blades, grown in the vertex shader from a map of ground
height, density and dryness (`GrassField::sample` fills it from any function, such as a
terrain's height and ground cover), so millions of blades cost one small texture. Blades sway
with the wind (`WindField`/`WeatherState`), with gusts rolling across the field, lean and curl,
thin out smoothly with distance (survivors widen to keep the cover; `style.draw_distance`), cast
sun shadows near the camera, and go through the depth prepass, so AO, bounce light and TAA see
them. Lit from behind, sunlight shines through the blades. Meadows grow in patches (taller,
greener hollows, shorter, drier rises). `GrassBender { radius }` on feet, bodies or vehicles
pushes blades aside (16 nearest the camera). `RenderSettings::grass_density` scales every field
(quality). Give the ground under a field `style.ground_color()` so it reads as grass past the
draw distance. `engine_foliage` drives grass by spawning fields from its soil grid; game code
can spawn them directly.

**Volumetric fog** (built): with `RenderSettings::volumetric_fog` (the default), fog within
`volumetric_fog_range` (default 256 m) of the camera lives in a froxel grid fitted to the view,
after Wronski 2014 and Hillaire 2015 (`volumetric_fog_quality`: `High` 8 px tiles and 128
exponential depth slices, `Medium` 64 slices, `Low` 16 px and 64). A compute pass right after
the depth prepass evaluates the medium in every froxel and lights it with the sun through the
shadow cascades, the cloud shadows and the air (`sun_transmittance`, phase function
`henyey_greenstein`), the sky
(kept out by the sky occlusion map and `SkyLightVolume`s) and the point lights (same falloff as
surfaces, no shadows); air just below the ground seen at a froxel's pixel is lit from just above
it, so the ground's own shadow doesn't darken the fog lying on it. It blends that with
the reprojected previous frame (jittered samples, 90% history) and integrates it front to back,
energy-conserving, into a 3D texture of in-scattered light (pre-exposed) and transmittance:
frame binding 19, read with `volumetric_fog_at(rel, depth)`, which blends the four froxel
columns around the pixel (their fog's radiance, and their transmittance with the height fog's
share swapped analytically for what the pixel's own ray crosses, so thin ground fog seen at a
glance doesn't band). So sunbeams show through trees and windows, lamps glow in mist and caves
stay dark. Other modules feed it two inputs and need nothing else:

- `FogSettings` (resource): the height fog everywhere. `density` (extinction per meter at
  `base_height`; weather writes it), `height_falloff`, `base_height`, `color` (albedo),
  `sun_scattering` (forward scattering, Henyey-Greenstein g, 0.6) and `max_opacity`.
- `FogVolume` (component, follows its entity's transform, scale included): local fog for a
  smoky room, a mist bank, steam or a dust cloud. `shape` (`Box` or `Ellipsoid`),
  `half_extents`, `density`, `albedo`, `emission` (cd/m² per meter, for fire-lit smoke or
  glowing spores), `edge_falloff` (meters), `noise` and `noise_scale` (wisps that drift with
  the wind) and `anisotropy`; `FogVolume::new(half_extents).with_density(0.05)...`. The 64
  nearest volumes in range are drawn. They fade out over the last 15% of the range, and they
  are not drawn beyond it or with volumetric fog off.

`apply_fog(color, rel)` reads the volume up to the range and continues the height fog
analytically beyond it, so nothing is counted twice, then adds aerial perspective. Beyond the
range the far shadow map and the cloud shadows shade the haze's sunlight and its glow around the
sun (`height_fog_sun_visibility`: eight taps spread over what the haze scatters), so haze in a
mountain's or a cloud's shadow doesn't glow and a mountain with the sun behind it keeps its
contrast. With
volumetric fog off, the analytic fog starts at the camera. `apply_sky_fog(color, dir)` fogs the
sky up to the range. The frame uniform's `volumetrics` field holds the range (0 = off) and the
slice mapping. Passes that write the volume see a clear stand-in at binding 19.

Shared pipelines the core will provide for modules (planned): **strands** for hair and fur
(`engine_softbody` draws its own today) and **tension-driven wrinkle maps** in `StandardMaterial`
(a per-vertex tension value blends wrinkle/vein normal maps; written by `engine_anim` for skin,
muscles and skinned clothes and by `engine_softbody` for simulated cloth).

Shader library: `lib_types.wgsl` (types, noise, BRDF, `henyey_greenstein`, `default_surface`, `surface_smooth_normal`), `lib_frame.wgsl` (group 0 frame
data, `shade_surface`, `shade_surface_shadowed`, `shade_thin_surface`, `shadow_factor` (cascades, then `far_shadow_visibility`), `cloud_shadow` (and `cloud_shadow_plane_point` for writers), `contact_shadow`, `screen_ao`, `sky_visibility`, `apply_fog` (`color * transmittance + inscatter` of `surface_fog`, which returns a `SurfaceFog` for callers that need the two apart, such as blended particles),
`apply_sky_fog`, `volumetric_fog_at`, `height_fog_density`, `fog_sky_light`,
`sample_env`, `sample_env_rough`, `sky_ambient`, `sky_ambient_covered`, `specular_sky_visibility`, `specular_horizon_fade`, `surface_weather`, `wind_offset`, `prepass_output`) and `lib_atmosphere.wgsl` (`atmosphere` for aerial perspective,
`sun_transmittance`, `sky_view_radiance`, `planet_height`, `ray_shell`).

**Shapes** (built): `Shape` builds procedural meshes. Round shapes given `segments: 0` get
enough segments to keep their outlines smooth at any size. `RoundedCuboid` rounds a box's edges
and corners, as real objects are, so edges catch the light. `Rock` makes a faceted, weathered
boulder or stone, different for every seed: break faces with rounded edges, erosion, a flat
base, and darkened cracks in the vertex color. Prefer these over plain cuboids and spheres for
anything meant to look real.

**Surface detail** (built): `StandardMaterial::with_detail(strength, scale)` adds procedural
bumps, mottled color and roughness from 3D noise to any material, textured or not, so plain
colors (stone, plaster, bark, soil, metal plates) don't look flat; it fades out with distance
instead of sparkling. It sticks to the object; terrain and other meshes split into pieces use
`with_detail_in_world()` so it lines up across seams.

**Wind sway** (built): `StandardMaterial::wind_sway` (m) bends a mesh downwind, growing with height
in the mesh, in the scene, depth and shadow passes alike. Each vertex scales it by its color's
alpha, its wind weight (0 = still, such as a trunk's base or a branch's joint; 1 = full sway, the
default of meshes without vertex colors); on swaying materials the vertex alpha is not opacity.

**Draw distance** (built): a `MaxDrawDistance(m)` component next to a `MeshRenderer` stops drawing
it, and its shadow, once its bounding sphere is wholly farther than that from the camera: small
props and clutter. Instanced and cluster-drawn meshes alike; skipped entities count as `culled`.

**Bloom** (built): a chain of 13-tap downsamples and tent upsamples (Jimenez 2014) without a
threshold, mixed in by `RenderSettings::bloom_intensity` (0.04). What a pixel brings in is
limited to `bloom_limit` times white at the current exposure (64; it rolls off smoothly from half
of that), so a neon sign at night or the sun's disc glows instead of swamping the frame.

**Thin sheets** (built): double-sided and alpha-masked materials (leaves, grass and fern cards,
paper, cloth, fences) are shaded as sheets with no inside (`shade_thin_surface(s, rel,
sun_visibility, face)`, `face` being the side of the geometry facing the camera): only that side
reflects light, whatever the vertex normals say, so a crown's bent normals or a normal map never
make the side away from the sun glint or light up. `StandardMaterial::subsurface` is how much
light comes through from the other side (sun, point lights and sky), tinted by the color, spread
like diffuse light with a forward glow toward the light, and shadowed like the lit side, so
backlit leaves glow green; at 0 (fences, grates, hair cards) nothing comes through. Opaque
single-sided materials keep the ordinary `shade_surface` (`subsurface` wraps light around them).

**Virtual geometry** (built: cluster LOD and GPU-driven drawing; `crates/render/src/virtual_geometry`):
dense opaque meshes (`RenderSettings::virtual_geometry_min_triangles`, 50 000 by default) are
split into 128-triangle clusters with a level-of-detail DAG (groups simplified with locked
borders, so mixed levels never crack), built once per mesh version on a background thread. Every
frame a compute pass culls instances and clusters for the camera, the shadow cascades and the sky
occlusion map, picks each cluster's level by its projected error (`cluster_error_px` on screen,
`cluster_shadow_error_texels` in shadow maps), culls clusters that face away, and writes indirect
draws; the clusters then go through the depth prepass, the scene pass (the same
`StandardMaterial` shading as `mesh.wgsl`) and the shadow maps. Only storage buffers, 32-bit
atomics and indirect draws: every backend. Smaller meshes, alpha-masked or wind-animated
materials and entities with the `NoVirtualGeometry` marker stay instanced; a changed mesh is drawn
instanced until its own clusters are rebuilt. `DebugView::Clusters` and `ClusterLevels` show
them; `render.stats` reports `clusters` (and their triangles). Next: occlusion culling (two-pass
HZB), streaming, a compute rasterizer for tiny triangles, cached shadow pages.

**Material presets** (built): `StandardMaterial::preset(MaterialPreset::Brick)` (and cobblestone,
rock, concrete, plaster, wood planks, bark, rusty steel, painted metal, sand, forest soil,
gravel, snow, marble, grass) is a ready-made PBR surface whose maps (sRGB base color, normal,
occlusion/roughness/metalness, height) are generated on the GPU by compute shaders the first time
it is drawn, once per process, at `RenderSettings::material_texture_size` (1024 by default),
tileable and mipmapped. Each preset is a small WGSL program in `shaders/materials/` over a
library of tileable noise, Voronoi, brick and scratch generators, slope blur, and maps derived
from its height (normal, ambient occlusion, curvature for edge wear and dirt), so a new preset
is one file. Presets work in meters: `with_texture_scale(m)` sets the size of one tile (the
relief depth follows), `with_tint` recolors, `detail` adds large-scale variation. Generated maps
fill the material's empty texture slots in the mesh feature's ordinary texture sets;
`GpuShared::material_textures` hands them to other features, and `read_material_maps` reads them
back. Any material with a height map (`with_height_texture`, every preset) gets parallax
occlusion mapping, fading out with distance and at grazing angles, with soft self-shadowing
toward the sun; `with_triplanar()` (object space) or `with_triplanar_world()` projects the
textures along three axes in meters with sharpened blending and surface-gradient normal
blending, for rocks, spheres and terrain without UVs. The prepass evaluates the same material
(parallax, triplanar, normal maps), so ambient occlusion, reflections and TAA see the detailed
normals.

**Shared images and image memory** (built; `image_cache.rs`): the renderer keeps one GPU copy
of each `Image` asset that is drawn, shared by every feature. `GpuShared::image_texture(&images,
handle, color)` returns it (`Arc<ImageTexture>`: `view`, `srgb`, size, mip levels, and an `id`
that changes only when new pixels are uploaded, so bind groups are rebuilt only then), uploading
it with its mip chain on first use and again when the asset gets new pixels; images nobody asks
for in 600 frames leave the GPU. The mesh feature (and with it the cluster path) binds its
material textures this way; features that upload `Image::data` themselves (the skinning feature,
today) should switch to it. With `RenderSettings::release_image_pixels` (off by default) the
renderer frees an uploaded image's CPU pixels (`data` and `mips`; size, format and flags stay,
and `Image::is_released()` turns true) once its asset version has stayed the same for
`RELEASE_AFTER_FRAMES` (120) frames, counted in frames so that a texture written every frame or
every few frames never qualifies at any frame rate. Releasing bumps the asset version; the cache
recognizes the emptied image and keeps its texture (no upload, same `id`), keeps a released image
on the GPU however long it goes unused, and uploads again when new `data` is assigned.
`Image::keep_pixels` opts an image out: set it on images that code reads on the CPU, writes in
place less often than every two seconds, or uploads itself (an image that both the mesh feature
and a skin use, until the skinning feature uses `image_texture`). `Diagnostics` reports
`render.images_released` and `render.image_bytes_released` (since startup),
`render.image_uploads` and `render.gpu_images`; `render.stats` includes them.

**Sky light** (built): every time the sky environment map is redrawn
(`RenderSettings::environment_interval`), the GPU filters it twice: prefiltered for GGX
reflections, one mip per roughness step (`sample_env_rough(dir, roughness)`), and convolved into
a small irradiance map for diffuse light (`sky_ambient(normal)`). `shade_surface` uses both, so
rough metal, plastic and wet stone reflect a blurred sky, and sky light follows the real sky:
blue in shadows by day, warm from the sunset side. Without a planet, the ground below the horizon
is a plane lit by the sun and sky.

**Atmosphere** (built): Rayleigh, Mie and ozone scattering on a spherical planet with multiple
scattering, after Hillaire 2020. Each planet's air is data on `Atmosphere`: Rayleigh and Mie
scattering, Mie and ozone absorption (per meter at the surface, RGB), scale heights, a Mie
anisotropy per color channel and a density scale, which a planet module fills in per body;
`Atmosphere::earth()` (the default) and `Atmosphere::mars()` (thin CO₂ under dust: a butterscotch
sky by day, a blue glow around the setting sun) are presets. Look-up tables (sunlight
transmittance and multiple scattering, recomputed when the planet or its air changes; the sky
seen from the camera, every frame) make the sky cheap and physically consistent from the
ground, from orbit and from the Moon. Clouds, fog and anything else lit by the sun should use
`sun_transmittance(rel)` for the sunlight reaching a point. The sky draws its own flat cloud layer (a 2D deck 1.5 km up whose cover follows
`WeatherState::cloud_cover`) in the sky and in the environment map; a feature that draws its own
clouds turns it off with `RenderSettings::sky_cloud_layer = false` and blends over a clear sky.
The sun is the brightest `DirectionalLight`, so a moon becomes the sky's light once it outshines
a fading sun; its disc is then drawn as the light's. Night (stars and the night sky's floor,
`frame.sky.y`, 0 day .. 1 night) is decided by that light (`frame::sky_night`): it brings day when
it is above the horizon (night falls as it sinks from 10° above to 4° below; up is away from the
planet's center) and brighter than a moon, more than 0.5 to 2 lux after the air at the camera
(`frame.sun_camera`). So a sun is day however dim (about 10 lux under a storm deck at noon), a moon
(0.3 lux at most) never makes day, and a sun at full illuminance below the horizon gives night even
without an atmosphere. A stand-in moon glows opposite a main light brighter than a moon once it is
below the horizon (`frame.sky.z`); a moon-dim light is the moon itself, or a scene with a real
moon, so then there is none.

## AI integration

- `engine.describe` → every registered component, resource and API with docs and examples.
- `entity.*`, `resource.*`, `scene.*` → inspect and edit the world by name.
- Module APIs (`fire.ignite`, `weather.set`, `npc.set_skill`, …).
- `render.screenshot` → see the result; `input.*` → play the game.
- The AI bridge serves these over a socket (JSON-RPC) and as an MCP server, for a running
  windowed game or a headless instance.
