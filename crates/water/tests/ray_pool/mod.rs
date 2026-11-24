//! The scene of the water's reflection-ray tests (ray_water.rs, ray_water_off.rs): a still pool
//! under a camera tilted 25° down, so the screen's top edge is 10° over the horizon, and a red
//! wall 30 m off whose lowest edge stands 11.9° up: off the screen, while the pool 3 m ahead
//! mirrors its middle. Without the lake mirror the reflection is the screen's march's, then the
//! frame's rays' (`trial_cut_more::RAY_WATER`), then the sky's. A manual clock, so two runs see
//! the same frames.

#![allow(dead_code)]

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{
    GpuShared, RayClass, RayGeometryState, RayKey, RayLook, RaySceneOut, RaySource, RaySphere,
    RayView, RenderDisabled, capture_now,
};
use engine_water::{WaterBody, WaterPlugin, WaterSettings, WaveSettings};
use engine_world::WorldPlugin;

pub const W: u32 = 160;
pub const H: u32 = 120;
/// The red wall's middle and size.
pub const WALL: DVec3 = DVec3::new(0.0, 9.0, -26.0);
const WALL_SIZE: [f32; 3] = [20.0, 3.5, 0.5];
/// Beside the wall, 10 m past its end: open sky, mirrored by the pool at (2, 0, 1).
pub const BESIDE: DVec3 = DVec3::new(20.0, 9.0, -26.0);
/// The camera, and the point it looks at (25 degrees down).
const EYE: DVec3 = DVec3::new(0.0, 1.0, 4.0);
pub const POOL_VIEW: DVec3 = DVec3::new(0.0, -3.66, -6.0);
/// The middle of [`Roof`] and of [`card`]: 12 m up, 5 m before the wall's face toward the pool.
pub const OVER_WALL: DVec3 = DVec3::new(0.0, 12.0, WALL.z + WALL_SIZE[2] as f64 * 0.5 + 5.0);

/// The pool scene with probe GI on ray-tracing hardware and `trial_cuts_more` `bits`.
pub fn app(bits: u32) -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(engine_core::time::TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.add_plugins(RenderPlugin);
    app.add_plugins((WorldPlugin, WaterPlugin));
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.tonemapper = Tonemapper::None;
        s.bloom_intensity = 0.0;
        s.vignette = 0.0;
        s.film_grain = 0.0;
        s.saturation = 1.0;
        s.sharpness = 0.0;
        s.anti_aliasing = AntiAliasing::None;
        s.screen_space_gi = false;
        s.probe_gi = true;
        s.gi_hardware_rays = true;
        s.gi_probe_spacing = 1.0;
        s.gi_probe_grid = [16, 8, 16];
        s.gi_probes_per_frame = 16;
        s.gi_hysteresis = 0.5;
        s.trial_cuts_more |= bits;
    }
    // The lake mirror would answer every pixel of the pool.
    app.world.resource_mut::<WaterSettings>().mirror_reflections = false;
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(70.0)
        },
        Transform::from_translation(EYE).looking_at(POOL_VIEW, Vec3::Y),
    ));
    // From behind the camera, onto the wall's face toward the pool.
    w.spawn((
        DirectionalLight {
            illuminance: 110_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(0.0, -0.5, -1.0), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        WaterBody {
            waves: WaveSettings {
                wind_waves: false,
                height_scale: 0.0,
                ..Default::default()
            },
            interactive: false,
            ..WaterBody::pool(4.0, 4.0, 1.0)
        },
    ));
    w.spawn((
        Transform::from_translation(WALL),
        MeshRenderer::new(
            Shape::Cuboid { size: WALL_SIZE },
            StandardMaterial::color(0.9, 0.05, 0.05).with_roughness(0.9),
        ),
    ));
    app
}

/// A roof of ground over the space in front of the wall, in the rays alone (`RayClass::Ground`
/// through the hook, as the terrain brings its patches): 12 m up, from just behind the wall to
/// 11 m before its face toward the pool, where the sun's rays to that face cross 12 m (2.5 to
/// 9.5 m before it), so it shades the face from the sun while every ray from the pool to the
/// wall passes under it. A ground's triangles are built to be asked: a shadow ray from the
/// wall's reflected face hands the water's shader a candidate.
pub struct Roof;

impl RenderFeature for Roof {
    fn name(&self) -> &str {
        "roof over the wall"
    }

    fn ray_scene(&mut self, _gpu: &GpuShared, _view: &RayView, out: &mut RaySceneOut<'_>) {
        let square = RayKey(0, 0);
        let state = out.geometry(square, 1, RayClass::Ground.opaque(), RaySource::UnitSquare);
        if !matches!(state, RayGeometryState::Ready | RayGeometryState::Older) {
            return;
        }
        // The unit square, 26 m across x and 12 m along z: from 1 m behind the wall's face
        // toward the pool (0.5 m behind its back) to 11 m in front of it.
        let to_world = DAffine3::from_scale_rotation_translation(
            DVec3::new(13.0, 1.0, 6.0),
            DQuat::IDENTITY,
            OVER_WALL,
        );
        let sphere = RaySphere {
            center: to_world.translation,
            radius: 14.4,
        };
        let look = RayLook {
            albedo: [0.3; 3],
            ..Default::default()
        };
        out.place(
            RayKey(1, 0),
            square,
            &to_world,
            sphere,
            RayClass::Ground,
            0,
            &look,
        );
    }
}

/// A card cut out by its texture in [`Roof`]'s place (26 m by 12 m, facing up, drawn from both
/// sides), drawn as well as in the rays: clear over one half across x and opaque over the
/// other, so it shades only the half of the wall's face under its opaque half from the sun. It
/// holds still, a fence more than a leaf (`Solid`, its triangles asked as a leaf's).
pub fn card(app: &mut App) -> Entity {
    let half_clear = Image::from_fn(64, 64, true, |x, _| {
        [255, 255, 255, if x < 32 { 0 } else { 255 }]
    });
    let texture = app.world.resource_mut::<Assets<Image>>().add(half_clear);
    let mut material = StandardMaterial::color(0.3, 0.3, 0.3)
        .with_roughness(0.9)
        .with_base_color_texture(texture)
        .with_alpha_mask(0.5);
    material.double_sided = true;
    app.world
        .spawn((
            Transform::from_translation(OVER_WALL)
                .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
            MeshRenderer::new(Shape::Quad { size: [26.0, 12.0] }, material),
        ))
        .id()
}

/// Turns the camera, from where it stands, to look at `target`.
pub fn look_at(app: &mut App, target: DVec3) {
    let mut q = app.world.query_filtered::<&mut Transform, With<Camera>>();
    for mut t in q.iter_mut(&mut app.world) {
        *t = Transform::from_translation(EYE).looking_at(target, Vec3::Y);
    }
}

pub fn diag(app: &App, name: &str) -> f64 {
    app.world
        .resource::<Diagnostics>()
        .get(name)
        .unwrap_or(-1.0)
}

/// Whether there is a GPU; says why not.
pub fn has_gpu(app: &App) -> bool {
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return false;
    }
    true
}

/// The picture as the scene stands now (also copied to `ENGINE_SHOTS`, when set).
pub fn picture(app: &mut App, name: &str) -> image::RgbImage {
    let dir = std::env::temp_dir().join("engine-ray-water");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(name);
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(out) = std::env::var("ENGINE_SHOTS") {
        let _ = std::fs::copy(&path, std::path::Path::new(&out).join(name));
    }
    image::open(&path).expect("read the shot").to_rgb8()
}

/// World point `at` mirrored in the pool's still surface (y = 0).
pub fn mirrored(at: DVec3) -> DVec3 {
    DVec3::new(at.x, -at.y, at.z)
}

/// The mean colour (0..255) of the 5x5 pixels around where world point `at` lands.
pub fn colour_at(app: &App, p: &image::RgbImage, at: DVec3) -> [f32; 3] {
    let view = app
        .world
        .resource::<engine_render::Renderer>()
        .last_view()
        .expect("a frame was drawn")
        .clone();
    let clip = view.unjittered_view_proj * (at - view.origin).as_vec3().extend(1.0);
    let ndc = clip.truncate() / clip.w;
    let x = ((ndc.x * 0.5 + 0.5) * W as f32) as u32;
    let y = ((0.5 - ndc.y * 0.5) * H as f32) as u32;
    let mut sum = [0.0f32; 3];
    let mut n = 0.0f32;
    for py in y.saturating_sub(2)..(y + 3).min(H) {
        for px in x.saturating_sub(2)..(x + 3).min(W) {
            let c = p.get_pixel(px, py);
            for (s, v) in sum.iter_mut().zip(c.0) {
                *s += v as f32;
            }
            n += 1.0;
        }
    }
    sum.map(|s| s / n.max(1.0))
}
