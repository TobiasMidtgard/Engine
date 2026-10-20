//! Offscreen screenshots for the vehicle tests: run a scene on the stand-in solver with the
//! renderer held back, then render a few frames and save one PNG. Needs a GPU or a software
//! Vulkan driver (lavapipe); skips with a note when neither is available.
//!
//! PNGs go to `CARGO_TARGET_TMPDIR`, or to `$VEHICLE_SHOTS` when set.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_vehicles::presets;

use super::*;

/// Close-up galleries (dozens of shots, minutes each on a software GPU) render only when
/// `$VEHICLE_SHOTS` names a folder for them, so CI runs the few shots in `screenshots.rs` and
/// the model budgets, not the galleries. Prints a note and returns false otherwise.
pub fn gallery(name: &str) -> bool {
    let on = std::env::var_os("VEHICLE_SHOTS").is_some();
    if !on {
        eprintln!("skipping {name}: set VEHICLE_SHOTS=<dir> to render the gallery");
    }
    on
}

/// [`shoot`] for gallery shots: only when [`gallery`] says so.
pub fn gallery_shot(name: &str, warmup: f64, scene: impl FnOnce(&mut App), camera: Transform) {
    if gallery(name) {
        shoot(name, warmup, scene, camera);
    }
}

/// A vehicle app that renders: the scene runs `warmup` seconds with the renderer held back,
/// then a few frames render (so exposure settles) and one is saved as `name`.png.
pub fn shoot(name: &str, warmup: f64, scene: impl FnOnce(&mut App), camera: Transform) {
    shoot_from(
        name,
        warmup,
        |app| {
            scene(app);
            None
        },
        camera,
    );
}

/// [`shoot`] with the camera riding on the vehicle the scene returns (`camera` in its
/// frame), for a ship too fast for a camera standing still. Gallery only.
pub fn gallery_ride(
    name: &str,
    warmup: f64,
    scene: impl FnOnce(&mut App) -> Entity,
    camera: Transform,
) {
    if gallery(name) {
        shoot_from(name, warmup, |app| Some(scene(app)), camera);
    }
}

fn shoot_from(
    name: &str,
    warmup: f64,
    scene: impl FnOnce(&mut App) -> Option<Entity>,
    camera: Transform,
) {
    let mut app = vehicle_app();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 1280;
        w.height = 720;
    }
    app.add_plugins(RenderPlugin);
    calm(&mut app);
    // Hold the renderer back while the scene simulates (rendering every step is slow on a
    // software GPU).
    app.insert_resource(RenderDisabled("warming up".into()));
    spawn_with(&mut app, |c| {
        c.spawn((
            DirectionalLight::default(),
            Transform::default().looking_at(DVec3::new(-0.45, -0.55, -0.7), Vec3::Y),
        ))
        .id()
    });
    let ride = scene(&mut app);
    spawn_with(&mut app, |c| {
        let mut e = c.spawn((Camera::perspective(45.0), camera));
        if let Some(vehicle) = ride {
            e.insert(ChildOf(vehicle));
        }
        e.id()
    });
    run_for(&mut app, warmup);
    app.world.remove_resource::<RenderDisabled>();
    for _ in 0..12 {
        app.update();
    }
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping {name}: no GPU ({})", d.0);
        return;
    }
    let dir = std::env::var("VEHICLE_SHOTS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")));
    std::fs::create_dir_all(&dir).expect("screenshot dir");
    let path = dir.join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    let img = image::open(&path).expect("png").to_rgba8();
    let mut distinct = std::collections::HashSet::new();
    for (_, _, p) in img.enumerate_pixels() {
        distinct.insert([p[0] / 16, p[1] / 16, p[2] / 16]);
    }
    assert!(
        distinct.len() > 60,
        "{name} looks flat ({} colors)",
        distinct.len()
    );
    eprintln!("{name} -> {}", path.display());
}

/// A drawn ground plane at y = `y` (the demos bring their own colliders).
pub fn plane(app: &mut App, size: f32, y: f64, color: u32, roughness: f32) {
    spawn_with(app, |c| {
        c.spawn((
            Transform::from_xyz(0.0, y, 0.0),
            MeshRenderer::new(
                Shape::Plane {
                    size: [size, size],
                    subdivisions: 1,
                },
                StandardMaterial::hex(color).with_roughness(roughness),
            ),
        ))
        .id()
    });
}

/// A concrete floor with a collider (top at y = 0) and its drawing.
pub fn floor(app: &mut App, size: f32) {
    ground(app, DVec3::ZERO, size * 0.5, "concrete");
    plane(app, size, 0.0, 0x8a8a86, 0.85);
}

/// A preset parked on the ground at `at` (y = ground height), facing `heading_deg` from −Z.
pub fn parked(app: &mut App, kind: &str, at: DVec3, heading_deg: f32) -> Entity {
    let t = Transform::from_translation(at + DVec3::Y * presets::ride_height(kind))
        .with_rotation(Quat::from_rotation_y(heading_deg.to_radians()));
    let kind = kind.to_string();
    spawn_with(app, move |c| presets::spawn(c, &kind, t).expect("preset"))
}
