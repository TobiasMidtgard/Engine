//! Distant shadows (`RenderSettings::distant_shadows`): rocks 600 m away, past the shadow
//! cascades and too small for the far shadow map, still shade the ground beside them through
//! the screen-space march toward the sun, and don't without it. The camera looks down from
//! 200 m, as from a hill or a plane: from near the ground a shadow that far is under a pixel tall. Needs a GPU or lavapipe; passes
//! with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use image::RgbaImage;

const W: u32 = 480;
const H: u32 = 270;
const FOV: f64 = 12.0;
const EYE: DVec3 = DVec3::new(0.0, 200.0, 0.0);
const TARGET: DVec3 = DVec3::new(0.0, 0.0, -600.0);
/// The rocks: 6 m cubes in a row across the view, 600 m ahead.
const ROCKS: [f64; 4] = [-45.0, -15.0, 15.0, 45.0];
const ROCK: f64 = 6.0;
const ROW_Z: f64 = -600.0;

fn app(distant: f32) -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.add_plugins(RenderPlugin);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.tonemapper = Tonemapper::None;
        s.bloom_intensity = 0.0;
        s.vignette = 0.0;
        s.film_grain = 0.0;
        s.saturation = 1.0;
        s.anti_aliasing = AntiAliasing::None;
        s.volumetric_fog = false;
        s.probe_gi = false;
        s.distant_shadows = distant;
    }
    app.world.resource_mut::<FogSettings>().density = 0.0;
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 12.5 },
            ..Camera::perspective(FOV as f32)
        },
        Transform::from_translation(EYE).looking_at(TARGET, Vec3::Y),
    ));
    // A low afternoon sun (20° up) shining along -x: each rock's shadow falls 16 m to its left.
    let e = 20f64.to_radians();
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-e.cos(), -e.sin(), 0.0), Vec3::Y),
    ));
    w.spawn((
        Transform::from_xyz(0.0, 0.0, -600.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [4000.0, 4000.0],
                subdivisions: 8,
            },
            StandardMaterial::color(0.5, 0.5, 0.5).with_roughness(1.0),
        ),
    ));
    for x in ROCKS {
        w.spawn((
            Transform::from_xyz(x, ROCK * 0.5, ROW_Z),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [ROCK as f32; 3],
                },
                StandardMaterial::color(0.4, 0.4, 0.4),
            ),
        ));
    }
    app
}

/// Pixel of world point `p` (the camera's pinhole, vertical field of view `FOV`).
fn pixel(p: DVec3) -> (u32, u32) {
    let f = (TARGET - EYE).normalize();
    let r = f.cross(DVec3::Y).normalize();
    let u = r.cross(f);
    let d = p - EYE;
    let t = (FOV.to_radians() * 0.5).tan();
    let aspect = W as f64 / H as f64;
    let x = d.dot(r) / (d.dot(f) * t * aspect);
    let y = d.dot(u) / (d.dot(f) * t);
    (
        ((x + 1.0) * 0.5 * W as f64) as u32,
        ((1.0 - y) * 0.5 * H as f64) as u32,
    )
}

/// Mean linear luminance of the ground over x `x0..x1`, z `ROW_Z ± 2.5` (sampled every 0.5 m).
fn ground(img: &RgbaImage, x0: f64, x1: f64) -> f32 {
    let lin = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let (mut sum, mut n) = (0.0, 0.0);
    let mut x = x0;
    while x <= x1 {
        let mut z = ROW_Z - 2.5;
        while z <= ROW_Z + 2.5 {
            let (px, py) = pixel(DVec3::new(x, 0.0, z));
            let p = img.get_pixel(px.min(W - 1), py.min(H - 1));
            sum += 0.2126 * lin(p[0]) + 0.7152 * lin(p[1]) + 0.0722 * lin(p[2]);
            n += 1.0;
            z += 0.5;
        }
        x += 0.5;
    }
    sum / n
}

/// The ground in each rock's shadow (2-10 m to its left) and on its sunlit side (2-10 m to its
/// right), averaged over the rocks.
fn measure(distant: f32) -> Option<(f32, f32)> {
    let mut app = app(distant);
    // Whole-second steps keep the post dither the same frame to frame (as in mesh_lod.rs).
    app.world
        .resource_mut::<Time>()
        .set_source(engine_core::time::TimeSource::Manual { step_secs: 1.0 });
    app.world.resource_mut::<Time>().set_max_delta(1.0);
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    app.update_n(4);
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("distant_shadows_{distant}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    let img = image::open(&path).expect("png").to_rgba8();
    let half = ROCK * 0.5;
    let (mut shaded, mut open) = (0.0, 0.0);
    for x in ROCKS {
        shaded += ground(&img, x - half - 10.0, x - half - 2.0);
        open += ground(&img, x + half + 2.0, x + half + 10.0);
    }
    Some((shaded / ROCKS.len() as f32, open / ROCKS.len() as f32))
}

#[test]
fn rocks_past_the_cascades_shade_the_ground_beside_them() {
    let Some((on_shaded, on_open)) = measure(1.0) else {
        return;
    };
    let Some((off_shaded, off_open)) = measure(0.0) else {
        return;
    };
    eprintln!(
        "distant shadows on: shaded {on_shaded:.4}, open {on_open:.4}; off: {off_shaded:.4}, \
         {off_open:.4}"
    );
    assert!(on_open > 0.02, "the open ground is not lit ({on_open})");
    assert!(
        off_shaded > off_open * 0.85,
        "without distant shadows the ground beside the rocks should be lit ({off_shaded} vs \
         {off_open})"
    );
    assert!(
        on_shaded < on_open * 0.6,
        "rocks 600 m away cast no shadow ({on_shaded} vs {on_open})"
    );
    assert!(
        on_open > off_open * 0.95,
        "the sunlit side darkened with distant shadows ({on_open} vs {off_open})"
    );
}
