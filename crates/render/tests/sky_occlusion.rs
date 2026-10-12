//! The sky occlusion map: ground under a roof loses sky light, open ground and walls keep it.
//! Needs a GPU or lavapipe; passes with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{DebugView, RenderDisabled, Renderer, capture_now};
use image::RgbaImage;

const W: u32 = 320;
const H: u32 = 180;

/// Ground, a 10 m roof slab 3 m up (like a carport or a cave ceiling) and a house-like block
/// to the right whose wall faces the camera. The sun is high, so the roof's own shadow falls
/// straight under it and the ground there is lit by the sky alone.
fn roof_app(occlusion: bool, debug: DebugView) -> App {
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
        s.sky_occlusion = occlusion;
        s.debug_view = debug;
        if debug != DebugView::None {
            // Show the debug value as is (sRGB-encoded, no tone curve).
            s.tonemapper = Tonemapper::None;
            s.bloom_intensity = 0.0;
            s.vignette = 0.0;
            s.film_grain = 0.0;
            s.saturation = 1.0;
        }
    }
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(60.0)
        },
        Transform::from_xyz(0.0, 2.0, 14.0).looking_at(DVec3::new(0.0, 0.5, 0.0), Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-0.2, -1.0, -0.1), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [200.0, 200.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.5, 0.5, 0.5).with_roughness(0.9),
        ),
    ));
    w.spawn((
        Transform::from_xyz(0.0, 3.2, 0.0),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [10.0, 0.4, 10.0],
            },
            StandardMaterial::color(0.5, 0.5, 0.5),
        ),
    ));
    w.spawn((
        Transform::from_xyz(12.0, 3.0, -2.0),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [6.0, 6.0, 6.0],
            },
            StandardMaterial::color(0.5, 0.5, 0.5).with_roughness(0.9),
        ),
    ));
    app
}

fn render(app: &mut App, name: &str) -> Option<RgbaImage> {
    app.update_n(2);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    Some(image::open(&path).expect("png").to_rgba8())
}

/// Mean luma in a 5x5 block around where world point `p` lands on screen.
fn luma_at(app: &App, img: &RgbaImage, p: DVec3) -> f32 {
    let view = app
        .world
        .resource::<Renderer>()
        .last_view()
        .unwrap()
        .clone();
    let clip = view.view_proj * (p - view.origin).as_vec3().extend(1.0);
    let ndc = clip.truncate() / clip.w;
    let (x, y) = (
        ((ndc.x * 0.5 + 0.5) * W as f32) as i32,
        ((0.5 - ndc.y * 0.5) * H as f32) as i32,
    );
    let mut sum = 0.0;
    for dy in -2..=2 {
        for dx in -2..=2 {
            let px = img.get_pixel(
                (x + dx).clamp(0, W as i32 - 1) as u32,
                (y + dy).clamp(0, H as i32 - 1) as u32,
            );
            sum += 0.2126 * px[0] as f32 + 0.7152 * px[1] as f32 + 0.0722 * px[2] as f32;
        }
    }
    sum / 25.0
}

const UNDER_ROOF: DVec3 = DVec3::new(0.0, 0.0, 0.0);
const OPEN_GROUND: DVec3 = DVec3::new(0.0, 0.0, 9.0);
const HOUSE_WALL: DVec3 = DVec3::new(12.0, 3.0, 1.0);

/// The debug view's value back from an sRGB 8-bit luma.
fn decode(luma: f32) -> f32 {
    let c = luma / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[test]
fn sky_visibility_is_low_under_a_roof_and_high_in_the_open() {
    let mut app = roof_app(true, DebugView::SkyVisibility);
    let Some(img) = render(&mut app, "sky_visibility") else {
        return;
    };
    let (under, open, wall) = (
        decode(luma_at(&app, &img, UNDER_ROOF)),
        decode(luma_at(&app, &img, OPEN_GROUND)),
        decode(luma_at(&app, &img, HOUSE_WALL)),
    );
    eprintln!("sky visibility: under roof {under}, open ground {open}, house wall {wall}");
    // 9 m out the roof still hides a little of the low sky.
    assert!(open > 0.85, "open ground should see the sky ({open})");
    // Truth for the middle of a 10 m roof 3.4 m up, open on all sides, is about 0.27.
    assert!(
        (0.1..0.45).contains(&under),
        "ground under the roof sees only the sky past its edges ({under})"
    );
    assert!(
        wall > 0.85,
        "a wall facing open ground keeps its sky light ({wall})"
    );
}

#[test]
fn roofs_darken_sky_light_only_with_occlusion_on() {
    let mut on = roof_app(true, DebugView::None);
    let Some(lit_on) = render(&mut on, "sky_occlusion_on") else {
        return;
    };
    let mut off = roof_app(false, DebugView::None);
    let Some(lit_off) = render(&mut off, "sky_occlusion_off") else {
        return;
    };
    let under_on = luma_at(&on, &lit_on, UNDER_ROOF);
    let under_off = luma_at(&off, &lit_off, UNDER_ROOF);
    let open_on = luma_at(&on, &lit_on, OPEN_GROUND);
    let open_off = luma_at(&off, &lit_off, OPEN_GROUND);
    eprintln!("under roof {under_on} (off {under_off}); open ground {open_on} (off {open_off})");
    assert!(open_off > 40.0, "test scene too dark to judge ({open_off})");
    assert!(
        decode(under_on) < decode(under_off) * 0.5,
        "the roof should block most sky light ({under_on} vs {under_off} without occlusion)"
    );
    assert!(
        (open_on - open_off).abs() < open_off * 0.1 + 3.0,
        "open ground should look the same either way ({open_on} vs {open_off})"
    );
}
