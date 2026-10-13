//! Screen-space effects: ambient occlusion darkens creases, temporal anti-aliasing smooths edges
//! and stays still when nothing moves. Needs a GPU or lavapipe; passes with a note when neither
//! is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{AntiAliasing, DebugView, RenderDisabled, Renderer, capture_now};
use image::RgbaImage;

const W: u32 = 320;
const H: u32 = 180;

/// A 3 m crate on gray ground, seen from 9 m away, plus a dark slab whose top edge is slanted
/// against the sky. The sun shines from behind the camera. `configure` tweaks the render
/// settings.
fn scene_app(debug: DebugView, configure: impl FnOnce(&mut RenderSettings)) -> App {
    scene_app_lit(debug, Vec3::new(-0.3, -1.0, -0.4), configure)
}

/// `scene_app` with the sunlight traveling along `sun`.
fn scene_app_lit(debug: DebugView, sun: Vec3, configure: impl FnOnce(&mut RenderSettings)) -> App {
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
        s.debug_view = debug;
        s.bloom_intensity = 0.0;
        s.vignette = 0.0;
        s.film_grain = 0.0;
        s.sharpness = 0.0;
        if debug != DebugView::None {
            // Show the debug value as is (sRGB-encoded, no tone curve).
            s.tonemapper = Tonemapper::None;
            s.saturation = 1.0;
        }
        configure(&mut s);
    }
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(60.0)
        },
        Transform::from_xyz(0.0, 2.5, 9.0).looking_at(DVec3::new(0.0, 1.5, 0.0), Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(sun.as_dvec3(), Vec3::Y),
    ));
    let gray = StandardMaterial::color(0.5, 0.5, 0.5).with_roughness(0.9);
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [200.0, 200.0],
                subdivisions: 1,
            },
            gray,
        ),
    ));
    w.spawn((
        Transform::from_xyz(0.0, 1.5, 0.0),
        MeshRenderer::new(Shape::Cuboid { size: [3.0; 3] }, gray),
    ));
    // A tall dark slab on the right, tilted so its top edge crosses pixel rows at a slant.
    w.spawn((
        Transform::from_xyz(6.0, 2.0, -6.0).with_rotation(Quat::from_rotation_z(0.25)),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [6.0, 6.0, 0.5],
            },
            StandardMaterial::color(0.02, 0.02, 0.02).with_roughness(0.9),
        ),
    ));
    app
}

fn render(app: &mut App, frames: usize, name: &str) -> Option<RgbaImage> {
    app.update_n(frames);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    Some(image::open(&path).expect("png").to_rgba8())
}

fn luma(img: &RgbaImage, x: u32, y: u32) -> f32 {
    let p = img.get_pixel(x.min(W - 1), y.min(H - 1));
    0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32
}

/// Where world point `p` lands on screen, in pixels.
fn screen(app: &App, p: DVec3) -> (u32, u32) {
    let view = app
        .world
        .resource::<Renderer>()
        .last_view()
        .unwrap()
        .clone();
    let clip = view.unjittered_view_proj * (p - view.origin).as_vec3().extend(1.0);
    let ndc = clip.truncate() / clip.w;
    (
        ((ndc.x * 0.5 + 0.5) * W as f32) as u32,
        ((0.5 - ndc.y * 0.5) * H as f32) as u32,
    )
}

/// Mean luma in a 3x3 block around where `p` lands.
fn luma_at(app: &App, img: &RgbaImage, p: DVec3) -> f32 {
    let (x, y) = screen(app, p);
    let mut sum = 0.0;
    for dy in 0..3 {
        for dx in 0..3 {
            sum += luma(img, (x + dx).saturating_sub(1), (y + dy).saturating_sub(1));
        }
    }
    sum / 9.0
}

/// The debug view's value back from an sRGB 8-bit luma.
fn decode(luma: f32) -> f32 {
    let c = luma / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Ground 15 cm in front of the crate's front face, in the crease between the two.
const CREASE: DVec3 = DVec3::new(0.0, 0.0, 1.65);
/// Open ground, far from anything.
const OPEN: DVec3 = DVec3::new(-3.5, 0.0, 4.0);

#[test]
fn ambient_occlusion_darkens_the_crease_but_not_open_ground() {
    let mut app = scene_app(DebugView::AmbientOcclusion, |s| {
        s.anti_aliasing = AntiAliasing::None
    });
    let Some(img) = render(&mut app, 2, "ao_debug") else {
        return;
    };
    let crease = decode(luma_at(&app, &img, CREASE));
    let open = decode(luma_at(&app, &img, OPEN));
    eprintln!("ambient visibility: crease {crease:.2}, open ground {open:.2}");
    assert!(open > 0.9, "open ground is occluded: {open:.2}");
    assert!(crease < 0.75, "the crease is not occluded: {crease:.2}");
}

#[test]
fn ambient_occlusion_off_shows_full_visibility() {
    let mut app = scene_app(DebugView::AmbientOcclusion, |s| {
        s.anti_aliasing = AntiAliasing::None;
        s.ambient_occlusion = false;
    });
    let Some(img) = render(&mut app, 2, "ao_off_debug") else {
        return;
    };
    let crease = decode(luma_at(&app, &img, CREASE));
    assert!(
        crease > 0.95,
        "occlusion without ambient occlusion: {crease:.2}"
    );
}

#[test]
fn ambient_occlusion_darkens_the_lit_image_in_the_crease() {
    // Sun from behind the crate: the crease lies in its shadow, lit by the sky alone, which is
    // the light ambient occlusion affects.
    let crease_luma = |ao: bool| {
        let mut app = scene_app_lit(DebugView::None, Vec3::new(-0.3, -1.0, 0.6), |s| {
            s.anti_aliasing = AntiAliasing::None;
            s.ambient_occlusion = ao;
        });
        render(&mut app, 2, if ao { "ao_on" } else { "ao_off" })
            .map(|img| (luma_at(&app, &img, CREASE), luma_at(&app, &img, OPEN)))
    };
    let (Some((with_ao, open_ao)), Some((without_ao, open_plain))) =
        (crease_luma(true), crease_luma(false))
    else {
        return;
    };
    eprintln!("crease luma: {with_ao:.1} with AO, {without_ao:.1} without");
    assert!(
        with_ao < without_ao - 2.0,
        "ambient occlusion doesn't darken the crease: {with_ao:.1} vs {without_ao:.1}"
    );
    assert!(
        (open_ao - open_plain).abs() < 2.0,
        "ambient occlusion changed open ground: {open_ao:.1} vs {open_plain:.1}"
    );
}

/// Pixels along the slab's slanted top edge whose luma lies strictly between the slab's and
/// the sky's: partial coverage, which only anti-aliasing produces.
fn edge_blend_pixels(img: &RgbaImage) -> usize {
    // The slab stands right of center; scan its top edge column by column.
    let mut blended = 0;
    for x in (W / 2)..(W - 4) {
        let column: Vec<f32> = (0..H / 2).map(|y| luma(img, x, y)).collect();
        // The edge: the first big drop from sky to slab going down.
        let Some(edge) = (1..column.len()).find(|&y| column[y] < column[y - 1] - 20.0) else {
            continue;
        };
        let sky = column[edge.saturating_sub(3)];
        let slab = column[(edge + 3).min(column.len() - 1)];
        if sky - slab < 40.0 {
            continue;
        }
        for &l in &column[edge.saturating_sub(1)..=(edge + 1).min(column.len() - 1)] {
            let t = (l - slab) / (sky - slab);
            if (0.15..0.85).contains(&t) {
                blended += 1;
            }
        }
    }
    blended
}

#[test]
fn temporal_anti_aliasing_smooths_slanted_edges() {
    let mut aliased = scene_app(DebugView::None, |s| s.anti_aliasing = AntiAliasing::None);
    let mut smoothed = scene_app(DebugView::None, |s| s.anti_aliasing = AntiAliasing::Taa);
    let (Some(a), Some(b)) = (
        render(&mut aliased, 2, "taa_off"),
        render(&mut smoothed, 16, "taa_on"),
    ) else {
        return;
    };
    let (hard, soft) = (edge_blend_pixels(&a), edge_blend_pixels(&b));
    eprintln!("blended edge pixels: {hard} without anti-aliasing, {soft} with TAA");
    assert!(
        soft > hard + 15,
        "TAA doesn't blend the edge: {soft} blended pixels vs {hard} without"
    );
}

#[test]
fn temporal_anti_aliasing_is_still_when_nothing_moves() {
    let mut app = scene_app(DebugView::None, |s| s.anti_aliasing = AntiAliasing::Taa);
    let Some(a) = render(&mut app, 24, "taa_still_a") else {
        return;
    };
    let Some(b) = render(&mut app, 0, "taa_still_b") else {
        return;
    };
    let mut worst = 0.0f32;
    let mut total = 0.0f32;
    for y in 0..H {
        for x in 0..W {
            let d = (luma(&a, x, y) - luma(&b, x, y)).abs();
            worst = worst.max(d);
            total += d;
        }
    }
    let mean = total / (W * H) as f32;
    eprintln!("frame-to-frame change with a still camera: mean {mean:.2}, worst {worst:.1}");
    assert!(mean < 0.5, "the image shimmers: mean change {mean:.2}");
    assert!(worst < 24.0, "a pixel flickers: change {worst:.1}");
}
