//! Removed assets give their GPU memory back: when a streamed zone unloads and removes its
//! meshes and images (`Assets::remove`), the mesh pool and the image cache drop their copies at
//! once instead of keeping them until the slots are reused. Needs a GPU or a software Vulkan
//! driver (lavapipe); passes with a note when neither is available.

use engine_assets::{AssetPlugin, Assets};
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{Exposure, RenderDisabled};

fn app() -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 96;
        w.height = 96;
    }
    app.add_plugins(AssetPlugin {
        hot_reload: false,
        root: None,
    });
    app.add_plugins(RenderPlugin);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.anti_aliasing = AntiAliasing::None;
        s.mesh_lod_pixels = 0.0;
        s.virtual_geometry = false;
    }
    app.world.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(60.0)
        },
        Transform::from_xyz(0.0, 0.0, 12.0).looking_at(DVec3::ZERO, Vec3::Y),
    ));
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.3, -0.3, -1.0), Vec3::Y),
    ));
    app
}

fn diagnostic(app: &App, key: &str) -> f64 {
    app.world
        .resource::<Diagnostics>()
        .get(key)
        .unwrap_or_else(|| panic!("{key} not published"))
}

#[test]
fn removed_meshes_and_images_leave_the_gpu() {
    let mut app = app();
    // A "zone" of eight textured meshes of its own.
    let mut zone = Vec::new();
    for i in 0..8 {
        let mesh = app.world.resource_mut::<Assets<Mesh>>().add(
            Shape::Sphere {
                radius: 0.5,
                segments: 24 + i,
            }
            .mesh(),
        );
        let image = app
            .world
            .resource_mut::<Assets<Image>>()
            .add(Image::solid([30 * i as u8, 120, 200, 255], true));
        let material = StandardMaterial::color(1.0, 1.0, 1.0).with_base_color_texture(image);
        let entity = app
            .world
            .spawn((
                Transform::from_xyz(i as f64 - 3.5, 0.0, 0.0),
                MeshRenderer::new(mesh, material),
            ))
            .id();
        zone.push((entity, mesh, image));
    }
    app.update_n(4);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let (meshes, images, pool) = (
        diagnostic(&app, "render.meshes.resident"),
        diagnostic(&app, "render.gpu_images"),
        diagnostic(&app, "render.meshes.pool_used_mb"),
    );
    assert!(
        meshes >= 8.0 && images >= 8.0,
        "{meshes} meshes, {images} images"
    );

    // The zone unloads: its entities go, then its assets.
    for (entity, mesh, image) in zone {
        app.world.despawn(entity);
        app.world.resource_mut::<Assets<Mesh>>().remove(mesh);
        app.world.resource_mut::<Assets<Image>>().remove(image);
    }
    app.update_n(2);
    let (meshes_after, images_after, pool_after) = (
        diagnostic(&app, "render.meshes.resident"),
        diagnostic(&app, "render.gpu_images"),
        diagnostic(&app, "render.meshes.pool_used_mb"),
    );
    eprintln!(
        "meshes {meshes} -> {meshes_after}, images {images} -> {images_after}, \
         pool {pool:.2} -> {pool_after:.2} MB"
    );
    assert!(meshes_after <= meshes - 8.0, "{meshes} -> {meshes_after}");
    assert!(images_after <= images - 8.0, "{images} -> {images_after}");
    assert!(pool_after < pool, "{pool} -> {pool_after}");
}
