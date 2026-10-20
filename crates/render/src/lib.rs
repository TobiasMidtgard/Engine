//! # engine_render
//!
//! wgpu renderer built for huge worlds and physically based light.
//!
//! - **Camera-relative rendering**: positions are f64 on the CPU and sent to the GPU relative
//!   to the camera, with reverse-Z infinite projection, so a cockpit button and a planet
//!   100 000 km away both render without jitter or z-fighting.
//! - **Physical units**: sunlight in lux, lamps in lumens, auto-exposure in EV100, filmic
//!   tonemapping (AgX) and bloom. Day, dusk, night and caves all look right with one setup.
//! - **Atmosphere**: multiple-scattering sky that works from the ground and from orbit, aerial
//!   perspective, height fog, a cloud layer, stars; the sky lights every surface (prefiltered
//!   for rough reflections, convolved for diffuse light). Each planet's air is data (Earth's
//!   blue sky, Mars' butterscotch one: [`Atmosphere::earth`], [`Atmosphere::mars`]).
//! - **Volumetric fog**: height fog and [`FogVolume`]s in a froxel grid lit by the shadowed
//!   sun, the sky and lamps: sunbeams through trees and windows, lamps glowing in mist, dark
//!   caves. See [`volumetrics`].
//! - **Cascaded sun shadows** (4 × 2048², texel-snapped), point lights, wind sway, wetness,
//!   snow and fire scorch on surfaces, per pixel from `engine_world`'s `SurfaceLayers`
//!   (`surface_weather` in shaders). Alpha-masked materials (leaves, fences) cut out their
//!   shadows too. Cloud shadows from whichever module draws the clouds (see
//!   [`cloud_shadows`]) shade the sunlight on every surface and in the fog.
//! - **Textured PBR materials**: [`StandardMaterial`] takes base color, normal,
//!   metallic/roughness, emissive and occlusion [`Image`]s (glTF conventions, mipmapped,
//!   anisotropic). Normal maps need no tangents. See [`material`] and [`texture`]. Every feature
//!   shares the renderer's uploaded images ([`GpuShared::image_texture`]), and
//!   `RenderSettings::release_image_pixels` frees their CPU pixels once they are on the GPU (see
//!   [`image_cache`]).
//! - **Lens effects** per camera: [`DepthOfField`] (a physical thin lens from the field of view,
//!   sensor height and f-stop, with autofocus and a gather-based bokeh with near and far
//!   fields) and [`MotionBlur`] (a 180° shutter by default, from camera and object motion, a
//!   reconstruction filter that blurs moving silhouettes over what is behind them). See
//!   [`depth_of_field`] and [`motion_blur`].
//! - **Asset loading**: `RenderPlugin` registers loaders for images (`.png`, `.jpg`, `.hdr`)
//!   and glTF models (`.gltf`, `.glb`) with the [`AssetServer`].
//!   `MeshRenderer`s that name an asset key load it on first use; files hot reload. See
//!   [`gltf_loader`].
//! - **2D and 2.5D**: [`Sprite`]s (sprite sheets, [`SpriteAnimation`], billboards, lit or unlit,
//!   cut-out sprites that write depth and cast shadows), [`TileMap`]s drawn in one or a few
//!   draw calls, [`Parallax`] layers and [`PixelPerfect`] orthographic cameras for crisp pixel
//!   art.
//!   See [`sprites`].
//! - **Extensible**: everything is a [`RenderFeature`]; water, terrain, particles, volumetric
//!   clouds, foliage and UI plug in without touching the core.
//! - **Headless**: renders offscreen without a window and saves PNG screenshots, so tools and
//!   AI agents can look at what they build (`render.screenshot`).
//!
//! ```ignore
//! app.add_plugins(RenderPlugin);
//! commands.spawn((Camera::default(), Transform::from_xyz(0.0, 2.0, 8.0).looking_at(DVec3::ZERO, Vec3::Y)));
//! commands.spawn((DirectionalLight::default(), Transform::default().looking_at(DVec3::new(-0.3, -1.0, -0.2), Vec3::Y)));
//! commands.spawn((Transform::default(), MeshRenderer::new(Shape::Sphere { radius: 1.0, segments: 48 }, StandardMaterial::hex(0xd94f30))));
//!
//! // A textured material.
//! let bark = server.load(&mut images, "textures/bark.png");
//! let bark_n = server.load(&mut images, "textures/bark_normal.png");
//! let mat = StandardMaterial::hex(0xffffff).with_base_color_texture(bark).with_normal_texture(bark_n);
//!
//! // A model: by key in a MeshRenderer…
//! MeshRenderer::new(MeshRef::Asset("models/rock.glb".into()), MaterialRef::Asset("models/rock.glb".into()));
//! // …or the whole node tree.
//! let scene = server.load(&mut scenes, "models/tree.glb");
//! spawn_gltf_scene_deferred(&mut commands, scene, Transform::from_xyz(4.0, 0.0, 0.0));
//! ```

mod atmosphere;
pub mod cloud_shadows;
pub mod components;
pub mod depth_of_field;
pub mod effects;
mod environment;
pub mod exposure;
pub mod features;
pub mod frame;
pub mod gltf_loader;
pub mod gpu;
pub mod grass;
pub mod image_cache;
mod lens;
pub mod material;
pub mod material_library;
pub mod mesh;
pub mod mesh_feature;
pub mod motion_blur;
pub mod renderer;
pub mod rock;
pub mod settings;
pub mod shadows;
pub mod sky_feature;
mod sky_occlusion;
pub mod sprites;
mod surface_weather;
pub mod targets;
pub mod texture;
pub mod virtual_geometry;
pub mod volumetrics;

use engine_assets::{AssetAppExt, AssetPlugin, AssetServer, Assets, Handle};
use engine_core::prelude::*;
use serde_json::json;

pub use cloud_shadows::{
    CLOUD_SHADOW_FORMAT, CloudShadowArea, CloudShadowRequest, CloudShadowTarget,
};
pub use components::{
    AmbientLight, BoxFace, Camera, DirectionalLight, Exposure, FogShape, FogVolume, MaterialRef,
    MaxDrawDistance, MeshRef, MeshRenderer, NoVirtualGeometry, PointLight, Projection,
    ResolvedMesh, SkyLightVolume,
};
pub use depth_of_field::DepthOfField;
pub use features::{
    FeatureStats, FrameContext, GpuShared, GrowBuffer, RenderAppExt, RenderFeature, RenderPhase,
    SHADER_LIB_ATMOSPHERE, SHADER_LIB_FRAME, SHADER_LIB_TYPES, compose_shader, validate_wgsl,
};
pub use frame::ViewData;
pub use gltf_loader::{
    GltfLoader, GltfNode, GltfPrimitive, GltfScene, GltfSceneSpawner, spawn_gltf_scene,
    spawn_gltf_scene_deferred,
};
pub use grass::{GrassBender, GrassField, GrassStyle};
pub use image_cache::{ImageMemoryStats, ImageTexture};
pub use material::{AlphaMode, StandardMaterial, TextureMapping};
pub use material_library::{MaterialMaps, MaterialPreset, MaterialTextures};
pub use mesh::{Mesh, Shape, Vertex};
pub use motion_blur::MotionBlur;
pub use renderer::{
    RenderDisabled, RenderStats, Renderer, ScreenshotInfo, Screenshots, capture_now,
};
pub use rock::{StoneFinish, StoneKind, WallStone, wall_stone};
pub use settings::{
    AntiAliasing, Atmosphere, DebugView, FogSettings, RenderSettings, Tonemapper, VolumetricQuality,
};
pub use sprites::{
    Billboard, Parallax, PixelGrid, PixelPerfect, PixelSnap, Sprite, SpriteAlpha, SpriteAnimation,
    SpriteAnimationMode, SpriteAtlas, SpriteFeature, SpriteFilter, TILE_FLIP_DIAGONAL, TILE_FLIP_X,
    TILE_FLIP_Y, TILE_FRAME_MASK, TileMap,
};
pub use surface_weather::{SURFACE_PUDDLE_SCALE_M, SURFACE_SNOW_SCALE_M};
pub use texture::{Image, ImageLoader, PixelFormat};
pub use virtual_geometry::{Displacement, DisplacementSource};
pub use wgpu;

pub mod prelude {
    pub use crate::{
        AlphaMode, AmbientLight, AntiAliasing, Atmosphere, Billboard, BoxFace, Camera,
        DepthOfField, DirectionalLight, Displacement, DisplacementSource, Exposure, FogSettings,
        FogShape, FogVolume, GltfScene, GltfSceneSpawner, GrassBender, GrassField, GrassStyle,
        Image, MaterialPreset, MaterialRef, MaxDrawDistance, Mesh, MeshRef, MeshRenderer,
        MotionBlur, NoVirtualGeometry, Parallax, PixelPerfect, PixelSnap, PointLight, Projection,
        RenderAppExt, RenderFeature, RenderPhase, RenderPlugin, RenderSettings, Screenshots, Shape,
        SkyLightVolume, Sprite, SpriteAlpha, SpriteAnimation, SpriteAnimationMode, SpriteAtlas,
        SpriteFilter, StandardMaterial, StoneKind, TextureMapping, TileMap, Tonemapper,
        VolumetricQuality, WallStone, spawn_gltf_scene, spawn_gltf_scene_deferred,
    };
}

/// Adds the renderer. The GPU starts on the first frame that has a window (or immediately
/// when `EngineConfig::headless` is set).
///
/// Also adds [`AssetPlugin`] (default settings) if it is not there yet, and registers the
/// image and glTF loaders. To configure the asset server (root, hot reload), add `AssetPlugin`
/// before this plugin.
pub struct RenderPlugin;

impl Plugin for RenderPlugin {
    fn build(&self, app: &mut App) {
        if !app.has_plugin::<AssetPlugin>() {
            app.add_plugins(AssetPlugin::default());
        }
        let vsync = app.world.resource::<EngineConfig>().vsync;
        app.init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .init_asset::<Image>()
            .init_asset::<GltfScene>()
            .register_asset_loader(ImageLoader)
            .register_asset_loader(GltfLoader)
            .insert_resource(RenderSettings {
                vsync,
                ..Default::default()
            })
            .init_resource::<FogSettings>()
            .init_resource::<Atmosphere>()
            .init_resource::<AmbientLight>()
            .init_resource::<Screenshots>()
            .init_resource::<RenderStats>()
            .init_resource::<features::PendingRenderFeatures>()
            .register_component::<Camera>(
                "Renders the scene. Perspective (fov_y_deg, near) or Orthographic (height). Exposure Auto or Manual{ev100}. Highest `order` among active cameras wins.",
            )
            .register_component::<DepthOfField>(
                "Physically based depth of field for the camera on this entity (off without it): thin lens from the camera's vertical fov and sensor_height_mm (24 = full frame; fov 27 deg is then a 50 mm lens), f_stop (1.4 shallow .. 16 deep), focus_distance_m (m along the view), or autofocus (true: on what is at the screen center, sky ignored, following at autofocus_speed 1/s, snapping on camera cuts; autofocus_area = size of the center square, fraction of the image height). max_radius_px caps the blur (output pixels, up to 96). Out-of-focus foreground bleeds over what is behind it; bright lights become bokeh discs. Diagnostics: render.dof_focus_m, measured render.dof_focus_lag_frames frames ago (GPU readback).",
            )
            .register_component::<MotionBlur>(
                "Camera motion blur for the camera on this entity (off without it), from camera motion and moving objects: shutter_fraction (0.5 = 180 deg shutter) at reference_fps (Hz; exposure = shutter_fraction / reference_fps s, so blur length doesn't change with frame rate; 0 = the actual frame interval), max_radius_px (longest streak from a pixel to either end, output pixels, up to 96), samples (4..64). Follows on-screen motion, so slow motion blurs less; none on camera cuts or for a still image.",
            )
            .register_component::<MeshRenderer>(
                "Draws a mesh with a material. mesh: {\"shape\": {\"Sphere\": {\"radius\": 1, \"segments\": 32}}} | {\"handle\": n} | {\"asset\": \"models/rock.glb#Mesh0\"} (loads on first use); material: {\"inline\": {base_color, metallic, perceptual_roughness, emissive, emissive_strength, alpha_mode, base_color_texture: <image handle>, normal_texture, metallic_roughness_texture (G=roughness, B=metallic), emissive_texture, occlusion_texture, uv_scale, normal_scale}} | {\"asset\": \"models/rock.glb#Material0\"}.",
            )
            .register_component::<NoVirtualGeometry>(
                "Marker: keeps this entity's dense MeshRenderer off the virtual geometry cluster path (debris, fragments, meshes that change often).",
            )
            .register_component::<Displacement>(
                "Real displaced geometry for this entity's MeshRenderer, made when its virtual geometry clusters are built (background, cached on disk): amplitude_m (m; 0 = the material's parallax_depth), midlevel (height left on the surface), edge_m / max_edge_m (finest / coarsest triangle edges, m), tolerance_m, max_triangles, cavity_darkening, normal_map_strength, source: \"MaterialHeight\" (the material's height map or preset) | {\"Stone\": {kind: SplitFace|RoughHewn|Slab, seed, scale, lichen, grime}}.",
            )
            .register_component::<MaxDrawDistance>(
                "Meters from the camera beyond which this entity's MeshRenderer (and its shadow) is not drawn: small props and clutter. Measured to the mesh's bounding sphere.",
            )
            .register_component::<DirectionalLight>(
                "Sun/moon light along the entity's forward (-Z). illuminance in lux (noon sun 110000).",
            )
            .register_component::<PointLight>(
                "Omni light. intensity_lm in lumens (bulb 800, campfire 2000), range in meters.",
            )
            .register_component::<SkyLightVolume>(
                "Indoor space (cave, room, ship interior): inside the box (half_extents, m, local axes) sky light drops to sky_light (0..1), rising to full within fade meters of each of open_faces (PosX/NegX/PosY/NegY/PosZ/NegZ).",
            )
            .register_component::<GrassField>(
                "Meadow of wind-blown grass blades over a rectangle on the entity's local x/z plane: size [x, z] m (centered), resolution [nx, nz] of the optional maps heights (m), density (0..1) and dryness (0..1), style {blades_per_m2, height, height_variation, width, bend, lush_color, dry_color, dryness, wind_response, draw_distance, shadow_distance}.",
            )
            .register_component::<GrassBender>(
                "Pushes grass aside and flattens it within radius (m) of the entity: feet, bodies, boulders, landing ships. The 16 nearest the camera act.",
            )
            .register_component::<FogVolume>(
                "Local fog (mist, dust, smoke, steam) lit by the volumetric fog: shape Box/Ellipsoid, half_extents (m, local axes, scaled with the entity), density (extinction 1/m: 0.02 mist, 0.5 smoke), albedo, emission (cd/m² per m), edge_falloff (m), noise 0..1 and noise_scale (m, drifts with the wind), anisotropy (-0.95..0.95, forward scattering).",
            )
            .register_component::<Sprite>(
                "2D image on a quad at the entity (sprites/2.5D): image (handle), normal_map, atlas (Whole | {Grid: {columns, rows, cell: [w, h] px (0 = fit), margin, spacing}} | {Rects: [[x, y, w, h] px]}), frame, size [w, h] m, anchor [0..1, 0..1] from bottom-left, color (linear RGBA tint), flip_x, flip_y, layer (draw order at equal depth), billboard Fixed/FaceCamera/Upright, filter Linear/Nearest/PixelArt, alpha Blend | Additive | {Cutout: threshold} (cut-out writes depth and casts shadows), lit (sun, lamps, sky, shadows), roughness, roundness (0 flat .. 1 rounded shading), translucency (light through from behind), emissive (cd/m²), cast_shadows, visible.",
            )
            .register_component::<SpriteAnimation>(
                "Plays frames of the entity's Sprite atlas on game time (slow motion slows it): frames (atlas frame list; empty = all), fps, mode Loop/Once/PingPong, speed (multiplier, negative = backward), playing, time (s into the animation).",
            )
            .register_component::<TileMap>(
                "Grid of tiles drawn in a few draw calls (2D levels, parallax layers, diorama floors) on the entity's local x/y plane: image (tileset handle), normal_map, atlas (as Sprite), size [columns, rows], tile_size [w, h] m, tiles (row by row from the top; value = atlas frame + 1, 0 = empty, flags 1<<31 flip x, 1<<30 flip y, 1<<29 diagonal), anchor, color, layer, filter, alpha, lit, roughness, translucency, emissive, cast_shadows, visible.",
            )
            .register_component::<Parallax>(
                "Draws the entity's Sprite or TileMap offset with the camera: factor [x, y] of camera movement kept (1 = world, 0.5 = far hills, 0 = fixed sky), repeat_x (m between copies for an endless layer, 0 = one copy).",
            )
            .register_component::<PixelPerfect>(
                "On an orthographic Camera: pixel-perfect pixel art. pixels_per_unit (art texels per meter), scale (screen pixels per texel, 0 = largest that shows the camera's height), snap Texel/ScreenPixel (camera and sprites move in whole steps). Turns temporal anti-aliasing off for this camera.",
            )
            .register_resource::<RenderSettings>(
                "Renderer quality and look: shadows (shadow_distance m, far_shadow_distance m: big casters such as mountains beyond the cascades, 0 = off; sun_penumbra_scale, contact_shadow_length m), ambient_occlusion (ao_radius m, ao_intensity), screen_space_reflections, volumetric_fog (bool), volumetric_fog_range (m), volumetric_fog_quality (Low/Medium/High), anti_aliasing (Taa/None) and sharpness, taa_reactive (0..1: TAA keeps thin transparent things such as snowflakes, rain and sparks; found automatically), tonemapper (AgX/Aces/Reinhard/None), bloom (bloom_intensity, bloom_limit: the most a pixel brings into the bloom in multiples of white, 0 = no limit), grading, debug_view (None/Normals/Cascades/SkyVisibility/AmbientOcclusion/Reflections/Clusters/ClusterLevels), virtual_geometry (dense opaque meshes as GPU-culled clusters: virtual_geometry_min_triangles, virtual_geometry_occlusion (two-pass occlusion culling), virtual_geometry_cache (+ _dir, _mb: builds kept on disk), cluster_error_px, cluster_shadow_error_texels), release_image_pixels (free each image's CPU pixels once uploaded and unchanged for 120 frames; Image keep_pixels opts one out).",
            )
            .register_resource::<FogSettings>("Height fog everywhere (volumetric within volumetric_fog_range, with light shafts): density (extinction 1/m at base_height, visibility about 3.9 / density: 0.00002 clear air, 0.0001 light haze, 0.0005 haze, 0.05 ground fog), height_falloff (1/m), base_height (m), max_opacity, color (albedo), sun_scattering (Henyey-Greenstein g, 0.6 = forward scattering, glow around the sun).")
            .register_resource::<Atmosphere>(
                "Planet atmosphere used for the sky and sunlight: planet_center (world), planet_radius, thickness (m), scale heights (m), and the planet's air per meter at the surface, RGB: rayleigh_scattering, mie_scattering, mie_absorption, ozone_absorption; mie_g (+ mie_g_offset per channel), density_scale (1 = as given), mie_forward_peak (0..1: the share g² of the haze's scattering passing as unscattered, delta-Eddington; Mars 1). Presets: Atmosphere::earth() (default), Atmosphere::mars().",
            )
            .register_resource::<AmbientLight>("Ambient fill light in cd/m² added on top of sky light.")
            .add_systems(
                PostUpdate,
                gltf_loader::spawn_pending_gltf_scenes.before(UpdateSet::Propagate),
            )
            .add_systems(Update, sprites::animate_sprites)
            .add_systems(Extract, resolve_mesh_renderers)
            .add_systems(Render, renderer::render_system)
            .add_render_feature(sky_feature::SkyFeature::default())
            .add_render_feature(mesh_feature::MeshFeature::default())
            .add_render_feature(grass::GrassFeature::default())
            .add_render_feature(sprites::SpriteFeature::default());

        app.register_api(
            "render.screenshot",
            "Render a frame now and save the final image as a PNG. Returns {path, width, height}.",
            r#"{"path": "screenshots/shot.png"}"#,
            |world, args| {
                let path = args
                    .get("path")
                    .and_then(|p| p.as_str())
                    .unwrap_or("screenshots/shot.png")
                    .to_string();
                let info = capture_now(world, path)?;
                serde_json::to_value(info).map_err(|e| e.to_string())
            },
        );
        app.register_api(
            "render.stats",
            "Renderer statistics: GPU, size, features, draw calls, triangles, exposure.",
            "{}",
            |world, _| {
                let stats = world
                    .get_resource::<RenderStats>()
                    .map(|s| serde_json::to_value(s).unwrap_or_default())
                    .unwrap_or_default();
                let details = world
                    .get_resource::<Renderer>()
                    .map(|r| r.describe())
                    .unwrap_or_else(|| {
                        json!({"running": false, "reason": world.get_resource::<RenderDisabled>().map(|d| d.0.clone())})
                    });
                Ok(json!({ "stats": stats, "renderer": details }))
            },
        );
        app.register_api(
            "render.shapes",
            "List procedural mesh shapes usable in MeshRenderer.mesh.shape.",
            "{}",
            |_, _| {
                Ok(json!({
                    "Cuboid": {"size": [1.0, 1.0, 1.0]},
                    "Sphere": {"radius": 0.5, "segments": 32},
                    "Plane": {"size": [10.0, 10.0], "subdivisions": 1},
                    "Capsule": {"radius": 0.4, "height": 1.0, "segments": 16},
                    "Cylinder": {"radius": 0.5, "height": 1.0, "segments": 24},
                    "Cone": {"radius": 0.5, "height": 1.0, "segments": 24},
                    "Torus": {"radius": 1.0, "tube": 0.25, "segments": 32},
                    "Quad": {"size": [1.0, 1.0]}
                }))
            },
        );
        app.register_api(
            "render.spawn_gltf",
            "Load a glTF/GLB model (if needed) and spawn its node tree as entities (Transform, Name, \
             MeshRenderer, parented with ChildOf). path may name a scene: \"models/tree.glb#Scene1\". \
             Returns the root entity id.",
            r#"{"path": "models/tree.glb", "transform": {"translation": [0, 0, -5]}, "name": "Tree"}"#,
            spawn_gltf_api,
        );
    }
}

fn spawn_gltf_api(world: &mut World, args: serde_json::Value) -> Result<serde_json::Value, String> {
    let path = args
        .get("path")
        .and_then(|p| p.as_str())
        .ok_or("missing 'path': a .gltf/.glb file relative to the asset root")?;
    let file = engine_assets::normalize_path(engine_assets::asset_file(path));
    let key = match path.split_once('#') {
        Some((_, label)) => format!("{file}#{label}"),
        None => file.clone(),
    };
    let loaded = world
        .get_resource::<AssetServer>()
        .ok_or("no AssetServer: add AssetPlugin")?
        .is_loaded(&file);
    if !loaded {
        engine_assets::load_now(world, &file)?;
    }
    let handle = world
        .get_resource::<Assets<GltfScene>>()
        .and_then(|a| a.loaded_by_key(&key))
        .ok_or_else(|| format!("{file} has no glTF scene under '{key}' (see assets.status)"))?;
    let transform: Transform = match args.get("transform") {
        Some(t) if !t.is_null() => {
            serde_json::from_value(t.clone()).map_err(|e| format!("bad transform: {e}"))?
        }
        _ => Transform::default(),
    };
    let root = spawn_gltf_scene(world, handle, transform)?;
    if let Some(name) = args.get("name").and_then(|n| n.as_str()) {
        world.entity_mut(root).insert(Name::new(name.to_string()));
    }
    let nodes = world
        .resource::<Assets<GltfScene>>()
        .get(handle)
        .map_or(0, |s| s.reachable_nodes());
    Ok(json!({"id": root.to_bits(), "nodes": nodes}))
}

/// Renderers that are new or changed since they were last resolved.
type UnresolvedRenderers<'w, 's> = Query<
    'w,
    's,
    (Entity, &'static MeshRenderer, Option<&'static ResolvedMesh>),
    Or<(Changed<MeshRenderer>, Without<ResolvedMesh>)>,
>;

/// Turn [`MeshRenderer`] references (shapes, inline materials, asset keys) into asset
/// handles. Runs when a renderer changes or hasn't been resolved yet.
///
/// Asset keys (`MeshRef::Asset`, `MaterialRef::Asset`) start loading their file through the
/// [`AssetServer`] the first time they are seen and resolve to a stable handle right away;
/// the entity appears once the asset has loaded (and updates when it hot reloads).
///
/// Inline materials are shared by value: two entities with identical inline materials use
/// one GPU material. For materials that animate every frame, create the material once
/// (`Assets<StandardMaterial>::add`) and edit it through its handle instead.
pub fn resolve_mesh_renderers(
    mut commands: Commands,
    server: Option<Res<AssetServer>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    query: UnresolvedRenderers,
) {
    for (entity, renderer, existing) in &query {
        let mesh = match &renderer.mesh {
            MeshRef::Handle(h) => Some(*h),
            MeshRef::Shape(shape) => {
                Some(meshes.get_or_insert_keyed(&shape.key(), || shape.mesh()))
            }
            MeshRef::Asset(key) => keyed_handle(server.as_deref(), &mut meshes, key),
        };
        let material = match &renderer.material {
            MaterialRef::Handle(h) => Some(*h),
            MaterialRef::Inline(m) => Some(materials.get_or_insert_keyed(&m.key(), || *m)),
            MaterialRef::Asset(key) => keyed_handle(server.as_deref(), &mut materials, key),
        };
        let (Some(mesh), Some(material)) = (mesh, material) else {
            continue;
        };
        let resolved = ResolvedMesh { mesh, material };
        if existing != Some(&resolved) {
            commands.entity(entity).insert(resolved);
        }
    }
}

/// Handle for an asset key: the asset already registered under it (loaded from disk or
/// inserted in code with `Assets::insert_keyed`), else a reserved handle while the server
/// loads the file the key names.
fn keyed_handle<T: engine_assets::Asset>(
    server: Option<&AssetServer>,
    assets: &mut Assets<T>,
    key: &str,
) -> Option<Handle<T>> {
    if let Some(h) = assets.by_key(key) {
        return Some(h);
    }
    match server {
        Some(server) => Some(server.load(assets, key)),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_assets::AssetServer;

    /// Every shader must compile against the shader library (no GPU needed).
    #[test]
    fn shaders_validate() {
        let mut shaders: Vec<(&str, std::borrow::Cow<str>, bool)> = vec![
            ("mesh", include_str!("shaders/mesh.wgsl").into(), true),
            (
                "mesh_shadow",
                include_str!("shaders/mesh_shadow.wgsl").into(),
                false,
            ),
            ("sky", include_str!("shaders/sky.wgsl").into(), true),
            ("post", include_str!("shaders/post.wgsl").into(), false),
            ("bloom", include_str!("shaders/bloom.wgsl").into(), false),
            ("blit", include_str!("shaders/blit.wgsl").into(), false),
            (
                "exposure",
                include_str!("shaders/exposure.wgsl").into(),
                false,
            ),
            ("gtao", include_str!("shaders/gtao.wgsl").into(), true),
            ("taa", include_str!("shaders/taa.wgsl").into(), true),
            ("ssr", include_str!("shaders/ssr.wgsl").into(), true),
            (
                "ssr_chain",
                include_str!("shaders/ssr_chain.wgsl").into(),
                false,
            ),
            ("ssgi", include_str!("shaders/ssgi.wgsl").into(), true),
            (
                "atmosphere_luts",
                include_str!("shaders/atmosphere_luts.wgsl").into(),
                true,
            ),
            (
                "sky_camera",
                include_str!("shaders/sky_camera.wgsl").into(),
                true,
            ),
            (
                "volumetric_fog",
                include_str!("shaders/volumetric_fog.wgsl").into(),
                true,
            ),
            (
                "environment_filter",
                include_str!("shaders/environment_filter.wgsl").into(),
                false,
            ),
            (
                "material_gen_mips",
                include_str!("shaders/material_gen_mips.wgsl").into(),
                false,
            ),
        ];
        // Every material preset's generator.
        for preset in MaterialPreset::ALL {
            shaders.push((preset.name(), preset.generator_source().into(), false));
        }
        let mut errors = Vec::new();
        for (name, src, frame) in shaders {
            if let Err(e) = validate_wgsl(name, &compose_shader(&src, frame)) {
                errors.push(e);
            }
        }
        // Sprites: a shared part plus the scene and shadow shaders.
        for (name, src, frame) in sprites::shader_sources() {
            if let Err(e) = validate_wgsl(name, &compose_shader(&src, frame)) {
                errors.push(e);
            }
        }
        // Virtual geometry shaders are made of several files.
        for (name, src, frame) in virtual_geometry::shader_sources() {
            if let Err(e) = validate_wgsl(name, &compose_shader(&src, frame)) {
                errors.push(e);
            }
        }
        assert!(errors.is_empty(), "{}", errors.join("\n\n"));
    }

    /// Other crates' blended shaders (particles, smoke) take the fog apart with `surface_fog`:
    /// its `SurfaceFog` keeps those names and per-channel types.
    #[test]
    fn surface_fog_splits_apply_fog() {
        let src = "
            @fragment
            fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
                let rel = vec3<f32>(p.x, 0.0, -p.y);
                let f: SurfaceFog = surface_fog(rel);
                let t: vec3<f32> = f.transmittance;
                let l: vec3<f32> = f.inscatter;
                return vec4<f32>(vec3<f32>(0.2) * t + l + apply_fog(vec3<f32>(0.2), rel), 0.5);
            }";
        validate_wgsl("surface_fog caller", &compose_shader(src, true)).unwrap();
    }

    #[test]
    fn mesh_renderer_round_trips_through_json() {
        let r = MeshRenderer::new(
            Shape::Sphere {
                radius: 1.0,
                segments: 16,
            },
            StandardMaterial::hex(0x336699).with_roughness(0.3),
        );
        let v = serde_json::to_value(&r).unwrap();
        let back: MeshRenderer = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(r, back, "{v}");
    }

    #[test]
    fn resolves_shapes_and_shares_materials() {
        let mut app = App::new();
        app.add_plugins(RenderPlugin);
        let mat = StandardMaterial::hex(0x808080);
        let a = app
            .world
            .spawn(MeshRenderer::new(Shape::Cuboid { size: [1.0; 3] }, mat))
            .id();
        let b = app
            .world
            .spawn(MeshRenderer::new(Shape::Cuboid { size: [1.0; 3] }, mat))
            .id();
        app.world.run_schedule(Extract);
        let ra = *app.world.get::<ResolvedMesh>(a).unwrap();
        let rb = *app.world.get::<ResolvedMesh>(b).unwrap();
        assert_eq!(ra, rb);
        assert_eq!(app.world.resource::<Assets<Mesh>>().len(), 1);
    }

    pub(crate) fn temp_dir(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "engine_render_{name}_{}_{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn app_with_root(root: &std::path::Path) -> App {
        let mut app = App::new();
        app.add_plugins((
            AssetPlugin {
                hot_reload: false,
                root: Some(root.to_path_buf()),
            },
            RenderPlugin,
        ));
        app
    }

    fn base64(data: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for c in data.chunks(3) {
            let n = (c[0] as u32) << 16
                | (*c.get(1).unwrap_or(&0) as u32) << 8
                | *c.get(2).unwrap_or(&0) as u32;
            for i in 0..4 {
                if i <= c.len() {
                    out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    /// A two-node glTF (parent quad without normals + child quad with a material) with its
    /// buffer embedded as base64.
    pub(crate) fn write_test_gltf(dir: &std::path::Path) -> &'static str {
        let mut bin: Vec<u8> = Vec::new();
        for p in [
            [-0.5f32, 0.0, 0.5],
            [0.5, 0.0, 0.5],
            [0.5, 0.0, -0.5],
            [-0.5, 0.0, -0.5],
        ] {
            for v in p {
                bin.extend_from_slice(&v.to_le_bytes());
            }
        }
        for uv in [[0.0f32, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]] {
            for v in uv {
                bin.extend_from_slice(&v.to_le_bytes());
            }
        }
        for i in [0u16, 1, 2, 0, 2, 3] {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        let gltf = json!({
            "asset": {"version": "2.0"},
            "scene": 0,
            "scenes": [{"name": "Main", "nodes": [0]}],
            "nodes": [
                {"name": "Base", "mesh": 0, "translation": [0.0, 1.0, 0.0], "children": [1]},
                {"name": "Top", "mesh": 1, "translation": [0.0, 0.5, 0.0], "scale": [2.0, 2.0, 2.0]}
            ],
            "meshes": [
                {"primitives": [{"attributes": {"POSITION": 0}, "indices": 2}]},
                {"primitives": [{"attributes": {"POSITION": 0, "TEXCOORD_0": 1}, "indices": 2, "material": 0}]}
            ],
            "materials": [{
                "name": "Red",
                "pbrMetallicRoughness": {"baseColorFactor": [0.8, 0.1, 0.1, 1.0], "metallicFactor": 0.0, "roughnessFactor": 0.7},
                "alphaMode": "MASK", "alphaCutoff": 0.3, "doubleSided": true
            }],
            "buffers": [{"byteLength": bin.len(), "uri": format!("data:application/octet-stream;base64,{}", base64(&bin))}],
            "bufferViews": [
                {"buffer": 0, "byteOffset": 0, "byteLength": 48},
                {"buffer": 0, "byteOffset": 48, "byteLength": 32},
                {"buffer": 0, "byteOffset": 80, "byteLength": 12}
            ],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3", "min": [-0.5, 0.0, -0.5], "max": [0.5, 0.0, 0.5]},
                {"bufferView": 1, "componentType": 5126, "count": 4, "type": "VEC2"},
                {"bufferView": 2, "componentType": 5123, "count": 6, "type": "SCALAR"}
            ]
        });
        std::fs::create_dir_all(dir.join("models")).unwrap();
        std::fs::write(
            dir.join("models/quads.gltf"),
            serde_json::to_vec_pretty(&gltf).unwrap(),
        )
        .unwrap();
        "models/quads.gltf"
    }

    #[test]
    fn gltf_loads_and_spawns() {
        let root = temp_dir("gltf");
        let path = write_test_gltf(&root);
        let mut app = app_with_root(&root);
        engine_assets::load_now(&mut app.world, path).unwrap();

        let meshes = app.world.resource::<Assets<Mesh>>();
        let base = meshes
            .get(meshes.by_key("models/quads.gltf#Mesh0/Primitive0").unwrap())
            .unwrap();
        // No normals in the file: flat normals, facing +Y (counter-clockwise seen from above).
        assert_eq!(base.vertices.len(), 6);
        assert!(
            base.vertices.iter().all(|v| v.normal == [0.0, 1.0, 0.0]),
            "{:?}",
            base.vertices[0]
        );
        assert_eq!(
            meshes.by_key("models/quads.gltf#Mesh1"),
            meshes.by_key("models/quads.gltf#Mesh1/Primitive0")
        );
        let mats = app.world.resource::<Assets<StandardMaterial>>();
        let red = mats
            .get(mats.by_key("models/quads.gltf#Material0").unwrap())
            .unwrap();
        assert_eq!(red.alpha_mode, AlphaMode::Mask(0.3));
        assert!(red.double_sided);
        assert!((red.base_color[0] - 0.8).abs() < 1e-6);

        let status = engine_core::registry::call_api(
            &mut app.world,
            "assets.status",
            json!({"path": "models/quads.gltf"}),
        )
        .unwrap();
        assert_eq!(status["status"], "loaded", "{status}");

        // Spawn through the API and check the hierarchy.
        let out = engine_core::registry::call_api(
            &mut app.world,
            "render.spawn_gltf",
            json!({"path": "models/quads.gltf", "transform": {"translation": [10.0, 0.0, 0.0]}, "name": "Quads"}),
        )
        .unwrap();
        assert_eq!(out["nodes"], 2);
        let root_e = Entity::try_from_bits(out["id"].as_u64().unwrap()).unwrap();
        app.update();
        let top = engine_core::registry::find_by_name(&mut app.world, "Top").unwrap();
        let base_e = engine_core::registry::find_by_name(&mut app.world, "Base").unwrap();
        assert_eq!(app.world.get::<ChildOf>(base_e).unwrap().0, root_e);
        assert_eq!(app.world.get::<ChildOf>(top).unwrap().0, base_e);
        let g = app.world.get::<GlobalTransform>(top).unwrap();
        assert!(
            (g.translation() - DVec3::new(10.0, 1.5, 0.0)).length() < 1e-6,
            "{:?}",
            g.translation()
        );
        assert!(app.world.get::<ResolvedMesh>(top).is_some());
        let renderer = app.world.get::<MeshRenderer>(top).unwrap();
        assert_eq!(
            renderer.material,
            MaterialRef::Asset("models/quads.gltf#Material0".into())
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn asset_keys_load_on_first_use() {
        let root = temp_dir("lazy");
        write_test_gltf(&root);
        let mut app = app_with_root(&root);
        let e = app
            .world
            .spawn(MeshRenderer::new(
                MeshRef::Asset("models/quads.gltf#Mesh1".into()),
                MaterialRef::Asset("models/quads.gltf#Material0".into()),
            ))
            .id();
        let start = std::time::Instant::now();
        loop {
            app.update();
            let resolved = app.world.get::<ResolvedMesh>(e).copied();
            if let Some(r) = resolved
                && app.world.resource::<Assets<Mesh>>().contains(r.mesh)
            {
                assert!(
                    app.world
                        .resource::<Assets<StandardMaterial>>()
                        .contains(r.material)
                );
                break;
            }
            assert!(start.elapsed().as_secs() < 10, "the model never loaded");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // Deferred spawning from a handle.
        let handle = {
            let world = &mut app.world;
            world.resource_scope(|world, mut scenes: Mut<Assets<GltfScene>>| {
                world
                    .resource::<AssetServer>()
                    .load(&mut scenes, "models/quads.gltf")
            })
        };
        let spawner = app
            .world
            .spawn((Transform::default(), GltfSceneSpawner(handle)))
            .id();
        app.update();
        assert!(app.world.get::<GltfSceneSpawner>(spawner).is_none());
        assert_eq!(app.world.get::<Children>(spawner).map(|c| c.len()), Some(1));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn in_memory_asset_keys_resolve_without_loading() {
        // Keys registered in code (procedural meshes, shared materials) are not files: they
        // resolve directly and must not start a load (which would fail with "no loader").
        let root = temp_dir("memkeys");
        let mut app = app_with_root(&root);
        let mesh = app
            .world
            .resource_mut::<Assets<Mesh>>()
            .insert_keyed("procedural/rock_3", Shape::Cuboid { size: [1.0; 3] }.mesh());
        let material = app
            .world
            .resource_mut::<Assets<StandardMaterial>>()
            .insert_keyed("materials/rock", StandardMaterial::hex(0x808080));
        let e = app
            .world
            .spawn(MeshRenderer::new(
                MeshRef::Asset("procedural/rock_3".into()),
                MaterialRef::Asset("materials/rock".into()),
            ))
            .id();
        app.update();
        assert_eq!(
            app.world.get::<ResolvedMesh>(e).copied(),
            Some(ResolvedMesh { mesh, material })
        );
        let server = app.world.resource::<AssetServer>();
        assert_eq!(
            server.status("procedural/rock_3"),
            engine_assets::LoadStatus::NotLoaded
        );
        assert!(server.list().is_empty(), "{:?}", server.list());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn images_load_from_disk() {
        let root = temp_dir("images");
        let img = ::image::RgbaImage::from_fn(8, 4, |x, y| {
            ::image::Rgba([x as u8 * 30, y as u8 * 60, 7, 255])
        });
        img.save(root.join("tex.png")).unwrap();
        let mut app = app_with_root(&root);
        engine_assets::load_now(&mut app.world, "tex.png").unwrap();
        let images = app.world.resource::<Assets<Image>>();
        let tex = images.get(images.by_key("tex.png").unwrap()).unwrap();
        assert_eq!((tex.width, tex.height), (8, 4));
        assert!(tex.srgb);
        assert_eq!(tex.mips.len(), 3);
        assert_eq!(tex.pixel(2, 1), [60, 60, 7, 255]);
        let _ = std::fs::remove_dir_all(root);
    }
}
