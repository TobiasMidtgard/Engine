//! # engine_render
//!
//! wgpu renderer built for huge worlds and physically based light.
//!
//! - **Camera-relative rendering**: positions are f64 on the CPU and sent to the GPU relative
//!   to the camera, with reverse-Z infinite projection, so a cockpit button and a planet
//!   100 000 km away both render without jitter or z-fighting.
//! - **Physical units**: sunlight in lux, lamps in lumens, auto-exposure in EV100, filmic
//!   tonemapping (AgX) and bloom. Day, dusk, night and caves all look right with one setup.
//! - **Atmosphere**: single-scattering sky that works from the ground and from orbit, aerial
//!   perspective, height fog, a cloud layer, stars; an environment map lights every surface.
//! - **Cascaded sun shadows** (4 × 2048², texel-snapped), point lights, wind sway, wetness
//!   and snow on surfaces driven by `engine_world` weather.
//! - **Extensible**: everything is a [`RenderFeature`]; water, terrain, particles, volumetric
//!   clouds, foliage and UI plug in without touching the core.
//! - **Headless**: renders offscreen without a window and saves PNG screenshots, so tools and
//!   AI agents can look at what they build (`render.screenshot`).
//!
//! ```ignore
//! app.add_plugins(RenderPlugin);
//! commands.spawn((Camera::default(), Transform::from_xyz(0.0, 2.0, 8.0).looking_at(DVec3::ZERO, DVec3::Y)));
//! commands.spawn((DirectionalLight::default(), Transform::default().looking_to(DVec3::new(-0.3, -1.0, -0.2), DVec3::Y)));
//! commands.spawn((Transform::default(), MeshRenderer::new(Shape::Sphere { radius: 1.0, segments: 48 }, StandardMaterial::hex(0xd94f30))));
//! ```

pub mod components;
pub mod exposure;
pub mod features;
pub mod frame;
pub mod gpu;
pub mod material;
pub mod mesh;
pub mod mesh_feature;
pub mod renderer;
pub mod settings;
pub mod shadows;
pub mod sky_feature;
pub mod targets;

use engine_assets::{AssetAppExt, Assets};
use engine_core::prelude::*;
use serde_json::json;

pub use components::{
    AmbientLight, Camera, DirectionalLight, Exposure, MaterialRef, MeshRef, MeshRenderer,
    PointLight, Projection, ResolvedMesh,
};
pub use features::{
    FeatureStats, FrameContext, GpuShared, GrowBuffer, RenderAppExt, RenderFeature, RenderPhase,
    SHADER_LIB_FRAME, SHADER_LIB_TYPES, compose_shader, validate_wgsl,
};
pub use frame::ViewData;
pub use material::{AlphaMode, StandardMaterial};
pub use mesh::{Mesh, Shape, Vertex};
pub use renderer::{
    RenderDisabled, RenderStats, Renderer, ScreenshotInfo, Screenshots, capture_now,
};
pub use settings::{Atmosphere, DebugView, FogSettings, RenderSettings, Tonemapper};
pub use wgpu;

pub mod prelude {
    pub use crate::{
        AlphaMode, AmbientLight, Atmosphere, Camera, DirectionalLight, Exposure, FogSettings,
        MaterialRef, Mesh, MeshRef, MeshRenderer, PointLight, Projection, RenderAppExt,
        RenderFeature, RenderPhase, RenderPlugin, RenderSettings, Screenshots, Shape,
        StandardMaterial, Tonemapper,
    };
}

/// Adds the renderer. The GPU starts on the first frame that has a window (or immediately
/// when `EngineConfig::headless` is set).
pub struct RenderPlugin;

impl Plugin for RenderPlugin {
    fn build(&self, app: &mut App) {
        let vsync = app.world.resource::<EngineConfig>().vsync;
        app.init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
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
            .register_component::<MeshRenderer>(
                "Draws a mesh with a material. mesh: {\"shape\": {\"Sphere\": {\"radius\": 1, \"segments\": 32}}} | {\"handle\": n} | {\"asset\": \"path\"}; material: {\"inline\": {base_color, metallic, perceptual_roughness, emissive, emissive_strength, ...}}.",
            )
            .register_component::<DirectionalLight>(
                "Sun/moon light along the entity's forward (-Z). illuminance in lux (noon sun 110000).",
            )
            .register_component::<PointLight>(
                "Omni light. intensity_lm in lumens (bulb 800, campfire 2000), range in meters.",
            )
            .register_resource::<RenderSettings>(
                "Renderer quality and look: shadows, tonemapper (AgX/Aces/Reinhard/None), bloom, grading, debug_view (None/Normals/Cascades).",
            )
            .register_resource::<FogSettings>("Height fog: density per meter, height_falloff, base_height, color (albedo).")
            .register_resource::<Atmosphere>(
                "Planet atmosphere used for the sky and sunlight: planet_center (world), planet_radius, thickness, scale heights.",
            )
            .register_resource::<AmbientLight>("Ambient fill light in cd/m² added on top of sky light.")
            .add_systems(Extract, resolve_mesh_renderers)
            .add_systems(Render, renderer::render_system)
            .add_render_feature(sky_feature::SkyFeature::default())
            .add_render_feature(mesh_feature::MeshFeature::default());

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
    }
}

/// Turn [`MeshRenderer`] references (shapes, inline materials, asset paths) into asset
/// handles. Runs when a renderer changes or hasn't been resolved yet (e.g. its asset was
/// still loading).
///
/// Inline materials are shared by value: two entities with identical inline materials use
/// one GPU material. For materials that animate every frame, create the material once
/// (`Assets<StandardMaterial>::add`) and edit it through its handle instead.
pub fn resolve_mesh_renderers(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    query: Query<
        (Entity, &MeshRenderer, Option<&ResolvedMesh>),
        Or<(Changed<MeshRenderer>, Without<ResolvedMesh>)>,
    >,
) {
    for (entity, renderer, existing) in &query {
        let mesh = match &renderer.mesh {
            MeshRef::Handle(h) => Some(*h),
            MeshRef::Shape(shape) => {
                Some(meshes.get_or_insert_keyed(&shape.key(), || shape.mesh()))
            }
            MeshRef::Asset(path) => meshes.by_key(path),
        };
        let material = match &renderer.material {
            MaterialRef::Handle(h) => Some(*h),
            MaterialRef::Inline(m) => Some(materials.get_or_insert_keyed(&m.key(), || *m)),
            MaterialRef::Asset(path) => materials.by_key(path),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every shader must compile against the shader library (no GPU needed).
    #[test]
    fn shaders_validate() {
        let shaders: [(&str, &str, bool); 7] = [
            ("mesh", include_str!("shaders/mesh.wgsl"), true),
            (
                "mesh_shadow",
                include_str!("shaders/mesh_shadow.wgsl"),
                false,
            ),
            ("sky", include_str!("shaders/sky.wgsl"), true),
            ("post", include_str!("shaders/post.wgsl"), false),
            ("bloom", include_str!("shaders/bloom.wgsl"), false),
            ("blit", include_str!("shaders/blit.wgsl"), false),
            ("exposure", include_str!("shaders/exposure.wgsl"), false),
        ];
        let mut errors = Vec::new();
        for (name, src, frame) in shaders {
            if let Err(e) = validate_wgsl(name, &compose_shader(src, frame)) {
                errors.push(e);
            }
        }
        assert!(errors.is_empty(), "{}", errors.join("\n\n"));
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
}
