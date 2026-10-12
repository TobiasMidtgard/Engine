//! Renders particles offscreen and checks the pixels: glowing particles appear where their
//! emitter is (also 30 000 km from the world origin), a spell's light brightens the ground
//! around it, and smoke lit by the sun covers what's behind it. Needs a GPU or a software
//! Vulkan driver (lavapipe); passes with a note when neither is available (`RenderDisabled`),
//! unless `ENGINE_REQUIRE_GPU=1`.

use engine_core::math::{DVec3, Mat4, Quat, Vec3, Vec4};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_vfx::prelude::*;
use engine_vfx::{EffectDesc, Gradient, Sprite};

const W: u32 = 320;
const H: u32 = 180;
const FOV_DEG: f32 = 60.0;
const EYE: DVec3 = DVec3::new(0.0, 1.5, 6.0);
const TARGET: DVec3 = DVec3::new(0.0, 1.0, 0.0);

/// A dusk scene around `offset`: low sun, dark ground, fixed exposure.
fn app(offset: DVec3, sun_lux: f32) -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.add_plugins((RenderPlugin, VfxPlugin));
    app.world.resource_mut::<Atmosphere>().planet_center =
        offset + DVec3::new(0.0, -6_360_000.0, 0.0);
    // The flat ground the particles collide with follows the scene.
    app.insert_resource(engine_world::Ground(std::sync::Arc::new(FlatAt(offset.y))));
    let w = &mut app.world;
    w.spawn((
        DirectionalLight {
            illuminance: sun_lux,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(-0.5, -0.5, -0.7), Vec3::Y),
    ));
    w.spawn((
        Transform::from_translation(offset),
        MeshRenderer::new(
            Shape::Plane {
                size: [40.0, 40.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x5a5550).with_roughness(0.9),
        ),
    ));
    w.spawn((
        Camera::perspective(FOV_DEG).with_exposure(Exposure::Manual { ev100: 8.0 }),
        Transform::from_translation(offset + EYE).looking_at(offset + TARGET, Vec3::Y),
    ));
    app
}

struct FlatAt(f64);

impl engine_world::GroundQuery for FlatAt {
    fn sample(&self, _p: DVec3) -> Option<engine_world::GroundSample> {
        Some(engine_world::GroundSample {
            height: self.0,
            normal: Vec3::Y,
            surface: engine_world::SurfaceId(2),
        })
    }
}

/// Render and save a screenshot. `None` when there is no GPU.
fn shot(app: &mut App, name: &str) -> Option<image::RgbaImage> {
    app.update_n(2);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let info = capture_now(&mut app.world, &path).expect("screenshot");
    assert_eq!((info.width, info.height), (W, H));
    eprintln!("wrote {}", path.display());
    Some(image::open(&path).expect("png").to_rgba8())
}

/// Pixel position of a point relative to the scene offset (same math as the renderer).
fn project(p: DVec3) -> (u32, u32) {
    let eye = Transform::from_translation(EYE).looking_at(TARGET, Vec3::Y);
    let proj = engine_render::frame::perspective_infinite_reverse(
        FOV_DEG.to_radians(),
        W as f32 / H as f32,
        0.05,
    );
    let view = Mat4::from_quat(Quat::inverse(eye.rotation));
    let rel = (p - eye.translation).as_vec3();
    let clip = proj * view * Vec4::new(rel.x, rel.y, rel.z, 1.0);
    let ndc = clip.truncate() / clip.w;
    (
        ((ndc.x * 0.5 + 0.5) * W as f32) as u32,
        ((0.5 - ndc.y * 0.5) * H as f32) as u32,
    )
}

/// Average luma (0..255) in a square of `r` pixels around `(x, y)`.
fn luma(img: &image::RgbaImage, (x, y): (u32, u32), r: i32) -> f32 {
    let mut sum = 0.0f32;
    let mut n = 0.0f32;
    for dy in -r..=r {
        for dx in -r..=r {
            let (px, py) = (x as i32 + dx, y as i32 + dy);
            if px < 0 || py < 0 || px >= W as i32 || py >= H as i32 {
                continue;
            }
            let c = img.get_pixel(px as u32, py as u32).0;
            sum += 0.2126 * c[0] as f32 + 0.7152 * c[1] as f32 + 0.0722 * c[2] as f32;
            n += 1.0;
        }
    }
    sum / n.max(1.0)
}

/// A still cloud of glowing motes that lives long enough to be photographed.
fn glow_cloud() -> EffectDesc {
    EffectDesc {
        name: "test.glow".into(),
        emitters: vec![EmitterDesc {
            bursts: vec![engine_vfx::Burst {
                count: 400,
                ..Default::default()
            }],
            shape: engine_vfx::EmitShape::Sphere {
                radius: 0.35,
                surface: false,
            },
            speed: [0.0, 0.0],
            lifetime: [20.0, 20.0],
            size: [0.12, 0.2],
            sprite: Sprite::Glow,
            color: Gradient::constant([1.0, 0.5, 0.15, 1.0]),
            emissive: 4_000.0,
            additive: 1.0,
            wind: 0.0,
            ..Default::default()
        }],
        ..Default::default()
    }
}

#[test]
fn glowing_particles_appear_where_the_emitter_is_even_far_from_the_origin() {
    for (i, offset) in [DVec3::ZERO, DVec3::new(3.0e7, 0.0, -2.0e7)]
        .into_iter()
        .enumerate()
    {
        let mut app = app(offset, 400.0);
        let Some(before) = shot(&mut app, &format!("vfx_glow_{i}_before.png")) else {
            return;
        };
        app.world
            .resource_mut::<EffectLibrary>()
            .insert(glow_cloud())
            .unwrap();
        app.world.spawn((
            Transform::from_translation(offset + TARGET),
            ParticleEffect::new("test.glow"),
        ));
        let after = shot(&mut app, &format!("vfx_glow_{i}_after.png")).unwrap();
        let center = project(TARGET);
        let (b, a) = (luma(&before, center, 3), luma(&after, center, 3));
        assert!(a > b + 60.0, "offset {offset}: center luma {b} -> {a}");
        // Far from the cloud nothing changed.
        let corner = (W / 10, H / 10);
        let (b, a) = (luma(&before, corner, 2), luma(&after, corner, 2));
        assert!(
            (a - b).abs() < 6.0,
            "offset {offset}: corner luma {b} -> {a}"
        );
    }
}

#[test]
fn a_spell_lights_the_ground_around_it() {
    let mut app = app(DVec3::ZERO, 200.0);
    let ground = project(DVec3::new(1.2, 0.0, 1.0));
    let Some(before) = shot(&mut app, "vfx_spell_before.png") else {
        return;
    };
    app.world.spawn((
        Transform::from_translation(DVec3::new(0.0, 0.8, 0.0)),
        ParticleEffect::new("spell.fireball"),
    ));
    app.update_n(10);
    let after = shot(&mut app, "vfx_spell_after.png").unwrap();
    let (b, a) = (luma(&before, ground, 3), luma(&after, ground, 3));
    assert!(a > b + 10.0, "ground luma {b} -> {a}");
    // The core glows.
    let core = project(DVec3::new(0.0, 0.8, 0.0));
    assert!(luma(&after, core, 2) > 150.0);
}

#[test]
fn sunlit_smoke_covers_what_is_behind_it() {
    let mut app = app(DVec3::ZERO, 30_000.0);
    // A bright emissive panel behind where the smoke will be.
    app.world.spawn((
        Transform::from_translation(DVec3::new(0.0, 1.0, -3.0)),
        MeshRenderer::new(
            Shape::Quad { size: [6.0, 4.0] },
            StandardMaterial::hex(0x000000).with_emissive([1.0, 1.0, 1.0], 20_000.0),
        ),
    ));
    let at = project(DVec3::new(0.0, 1.0, 0.0));
    let Some(before) = shot(&mut app, "vfx_smoke_before.png") else {
        return;
    };
    let smoke = EffectDesc {
        name: "test.smoke".into(),
        emitters: vec![EmitterDesc {
            bursts: vec![engine_vfx::Burst {
                count: 120,
                ..Default::default()
            }],
            shape: engine_vfx::EmitShape::Sphere {
                radius: 0.4,
                surface: false,
            },
            speed: [0.0, 0.0],
            lifetime: [20.0, 20.0],
            size: [1.2, 1.5],
            sprite: Sprite::Smoke,
            lit: 1.0,
            color: Gradient::constant([0.12, 0.12, 0.12, 1.0]),
            wind: 0.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    app.world
        .resource_mut::<EffectLibrary>()
        .insert(smoke)
        .unwrap();
    app.world.spawn((
        Transform::from_translation(TARGET),
        ParticleEffect::new("test.smoke"),
    ));
    let after = shot(&mut app, "vfx_smoke_after.png").unwrap();
    let (b, a) = (luma(&before, at, 3), luma(&after, at, 3));
    assert!(
        a < b - 30.0,
        "dark smoke should hide the bright panel: {b} -> {a}"
    );
}
