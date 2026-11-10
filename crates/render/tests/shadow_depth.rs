//! Shadow maps with 16 bits of depth (`RenderSettings::shadow_depth_bits`) cast the same
//! shadows as with 32: the same scene is drawn both ways, at a low sun where depth steps show
//! first (acne on lit ground, shadows detached from their casters), from eye height along the
//! ground and from close above a shadow's edge. At Ultra the maps are 640 MB at 32 bits and
//! half that at 16. Needs a GPU or lavapipe; passes with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use image::RgbaImage;

const W: u32 = 640;
const H: u32 = 360;

/// Posts from beside the camera out to the far cascades: distance ahead (m) and height (m).
const POSTS: [(f64, f64); 7] = [
    (3.0, 1.2),
    (7.0, 2.0),
    (16.0, 3.0),
    (40.0, 6.0),
    (90.0, 10.0),
    (180.0, 16.0),
    (300.0, 24.0),
];

/// The two views: along the ground from eye height, and close above the nearest post's shadow.
const VIEWS: [(DVec3, DVec3, f32); 2] = [
    (
        DVec3::new(0.0, 1.7, 2.0),
        DVec3::new(-6.0, 0.0, -60.0),
        60.0,
    ),
    (
        DVec3::new(-1.5, 1.0, -2.0),
        DVec3::new(-4.5, 0.0, -3.0),
        40.0,
    ),
];

fn picture(bits: u32, view: usize) -> Option<RgbaImage> {
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
        s.shadow_map_size = 4096;
        s.shadow_depth_bits = bits;
    }
    app.world.resource_mut::<FogSettings>().density = 0.0;
    let (eye, target, fov) = VIEWS[view];
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 12.5 },
            ..Camera::perspective(fov)
        },
        Transform::from_translation(eye).looking_at(target, Vec3::Y),
    ));
    // A sun 12 degrees up, across the view: long shadows to the left of every post.
    let e = 12f64.to_radians();
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-e.cos(), -e.sin(), -0.25), Vec3::Y),
    ));
    w.spawn((
        Transform::from_xyz(0.0, 0.0, -400.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [2000.0, 2000.0],
                subdivisions: 8,
            },
            StandardMaterial::color(0.5, 0.5, 0.5).with_roughness(1.0),
        ),
    ));
    for (ahead, height) in POSTS {
        let width = (height * 0.12) as f32;
        w.spawn((
            Transform::from_xyz(0.0, height * 0.5, -ahead),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [width, height as f32, width],
                },
                StandardMaterial::color(0.4, 0.4, 0.4),
            ),
        ));
        // A ball resting on the ground beside it: a curved caster touching its own shadow.
        let radius = height * 0.2;
        w.spawn((
            Transform::from_xyz(-height * 0.6, radius, -ahead - height * 0.3),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: radius as f32,
                    segments: 32,
                },
                StandardMaterial::color(0.6, 0.45, 0.3),
            ),
        ));
    }
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
    app.update_n(5);
    let made = app
        .world
        .resource::<Diagnostics>()
        .get("render.shadow_depth_bits");
    assert_eq!(made, Some(bits as f64), "the shadow maps' depth bits");
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("shadow_depth_{bits}_view{view}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    Some(image::open(&path).expect("png").to_rgba8())
}

#[test]
fn sixteen_bit_shadow_maps_cast_the_same_shadows() {
    for view in 0..VIEWS.len() {
        let Some(full) = picture(32, view) else {
            return;
        };
        let Some(half) = picture(16, view) else {
            return;
        };
        // Sunlit ground, at the bottom of the picture: shadows are clearly darker than it.
        let lit = full.get_pixel(W / 2, H - 4)[0] as f32;
        let (mut sum, mut worst, mut off, mut dark) = (0u64, 0u8, 0u32, 0u32);
        for (x, y, a) in full.enumerate_pixels() {
            let b = half.get_pixel(x, y);
            let d = (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
            sum += d as u64;
            worst = worst.max(d);
            off += (d > 12) as u32;
            dark += (y > H / 2 && (a[0] as f32) < lit * 0.8) as u32;
        }
        let pixels = (W * H) as f32;
        let mean = sum as f32 / pixels;
        eprintln!(
            "view {view}: mean difference {mean:.4} of 255, worst {worst}, {off} pixels over \
             12, {dark} pixels in shadow (lit ground {lit})"
        );
        // The view holds shadows to compare at all.
        assert!(
            dark as f32 > pixels * 0.005,
            "view {view} shows no shadows ({dark} dark pixels)"
        );
        assert!(mean < 0.15, "view {view}: the pictures differ ({mean})");
        // A shadow's edge may move by a fraction of a texel; acne or a detached shadow would
        // change whole areas.
        assert!(
            (off as f32) < pixels * 0.001,
            "view {view}: {off} pixels differ by more than 12 of 255"
        );
    }
}
