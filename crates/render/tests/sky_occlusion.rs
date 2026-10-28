//! The sky occlusion map: ground under a roof loses sky light, open ground and walls keep it.
//! Needs a GPU or lavapipe; passes with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{DebugView, RenderDisabled, Renderer, capture_now};
use image::RgbaImage;

const W: u32 = 320;
const H: u32 = 180;

/// A box: center and size, meters.
type Block = (DVec3, [f32; 3]);

/// A 10 m roof slab 3 m up, like a carport or a cave ceiling. The sun is high, so its own
/// shadow falls straight under it and the ground there is lit by the sky alone.
const CARPORT: Block = (DVec3::new(0.0, 3.2, 0.0), [10.0, 0.4, 10.0]);
/// A house-like block to the right whose wall faces the camera.
const HOUSE: Block = (DVec3::new(12.0, 3.0, -2.0), [6.0, 6.0, 6.0]);

/// Ground plus `blocks`, seen from 14 m away. `configure` tweaks the render settings.
fn scene_app(
    blocks: &[Block],
    debug: DebugView,
    configure: impl FnOnce(&mut RenderSettings),
) -> App {
    scene_app_with_ground(blocks, debug, configure, |_| {
        StandardMaterial::color(0.5, 0.5, 0.5).with_roughness(0.9)
    })
}

/// `scene_app` with the ground's material made by `ground`.
fn scene_app_with_ground(
    blocks: &[Block],
    debug: DebugView,
    configure: impl FnOnce(&mut RenderSettings),
    ground: impl FnOnce(&mut App) -> StandardMaterial,
) -> App {
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
        if debug != DebugView::None {
            // Show the debug value as is (sRGB-encoded, no tone curve).
            s.tonemapper = Tonemapper::None;
            s.bloom_intensity = 0.0;
            s.vignette = 0.0;
            s.film_grain = 0.0;
            s.saturation = 1.0;
        }
        configure(&mut s);
    }
    let ground = ground(&mut app);
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
            ground,
        ),
    ));
    for &(center, size) in blocks {
        w.spawn((
            Transform::from_translation(center),
            MeshRenderer::new(
                Shape::Cuboid { size },
                StandardMaterial::color(0.5, 0.5, 0.5).with_roughness(0.9),
            ),
        ));
    }
    app
}

fn roof_app(occlusion: bool, debug: DebugView) -> App {
    scene_app(&[CARPORT, HOUSE], debug, |s| {
        s.sky_occlusion = occlusion;
        // The sky occlusion map on its own: probe GI's rays find the roof either way.
        s.probe_gi = false;
    })
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

/// Sky visibility of the ground at the origin with `blocks` around it.
fn visibility_under(
    blocks: &[Block],
    name: &str,
    configure: impl FnOnce(&mut RenderSettings),
) -> Option<f32> {
    let mut app = scene_app(blocks, DebugView::SkyVisibility, configure);
    let img = render(&mut app, name)?;
    Some(decode(luma_at(&app, &img, UNDER_ROOF)))
}

#[test]
fn high_roofs_hide_less_sky_than_low_ones() {
    let roof_at = |y: f64| [(DVec3::new(0.0, y, 0.0), [10.0, 0.4, 10.0])];
    let Some(low) = visibility_under(&roof_at(3.2), "sky_roof_3m", |_| {}) else {
        return;
    };
    let mid = visibility_under(&roof_at(10.2), "sky_roof_10m", |_| {}).unwrap();
    let high = visibility_under(&roof_at(30.2), "sky_roof_30m", |_| {}).unwrap();
    eprintln!("sky visibility under a 10 m roof: 3 m up {low}, 10 m up {mid}, 30 m up {high}");
    // Truth for the middle of a 10 m square roof: about 0.25 at 3 m, 0.76 at 10 m, 0.97 at 30 m.
    assert!(low < 0.45, "a low roof hides most of the sky ({low})");
    assert!(
        (0.6..0.9).contains(&mid),
        "a roof 10 m up hides about a quarter of the sky ({mid})"
    );
    assert!(high > 0.9, "a roof 30 m up hides very little sky ({high})");
}

#[test]
fn an_aircraft_overhead_barely_darkens_the_ground() {
    // A 12 m object 100 m up hides about 0.5 % of the sky.
    let aircraft = [(DVec3::new(0.0, 100.0, 0.0), [12.0, 3.0, 12.0])];
    let Some(vis) = visibility_under(&aircraft, "sky_aircraft", |_| {}) else {
        return;
    };
    eprintln!("sky visibility under an aircraft 100 m up: {vis}");
    assert!(
        vis > 0.97,
        "the ground under an aircraft keeps its sky light ({vis})"
    );
}

#[test]
fn a_coarse_map_still_darkens_under_a_carport() {
    // 128² over 256 m is 2 m per texel: the carport is only five texels wide.
    let Some(vis) = visibility_under(&[CARPORT], "sky_coarse", |s| s.sky_occlusion_size = 128)
    else {
        return;
    };
    eprintln!("sky visibility under the carport with a 2 m map: {vis}");
    assert!(vis < 0.5, "the carport still hides most of the sky ({vis})");
}

#[test]
fn a_sky_light_volume_keeps_sky_light_out_except_at_its_open_face() {
    // The space under the carport, open toward the camera (+z).
    let mut app = scene_app(&[CARPORT], DebugView::SkyVisibility, |_| {});
    app.world.spawn((
        Transform::from_xyz(0.0, 1.5, 0.0),
        SkyLightVolume::new([5.0, 1.5, 5.0])
            .open(BoxFace::PosZ)
            .with_fade(4.0)
            .with_sky_light(0.0),
    ));
    let Some(img) = render(&mut app, "sky_volume") else {
        return;
    };
    let deep = decode(luma_at(&app, &img, DVec3::new(0.0, 0.0, -2.0)));
    let mouth = decode(luma_at(&app, &img, DVec3::new(0.0, 0.0, 4.6)));
    let open = decode(luma_at(&app, &img, OPEN_GROUND));
    eprintln!(
        "sky visibility with a volume: deep inside {deep}, at the open face {mouth}, outside {open}"
    );
    assert!(deep < 0.03, "deep inside sees no sky ({deep})");
    assert!(
        mouth > deep + 0.15,
        "sky light comes in at the open face ({mouth})"
    );
    assert!(open > 0.85, "outside the volume nothing changes ({open})");
}

/// Mean color (sRGB, 0..255) in a 5x5 block around where world point `p` lands on screen.
fn color_at(app: &App, img: &RgbaImage, p: DVec3) -> Vec3 {
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
    let mut sum = Vec3::ZERO;
    for dy in -2..=2 {
        for dx in -2..=2 {
            let px = img.get_pixel(
                (x + dx).clamp(0, W as i32 - 1) as u32,
                (y + dy).clamp(0, H as i32 - 1) as u32,
            );
            sum += Vec3::new(px[0] as f32, px[1] as f32, px[2] as f32);
        }
    }
    sum / 25.0
}

#[test]
fn a_closed_room_isnt_lit_by_the_ground_outside() {
    // A closed room (inside x -3..3, y 0..3, z -3..3) on a planet with a bright green ground,
    // seen from inside. A little sky light gets in; it has bounced around on the way, so the
    // ceiling must not take on the green of the sunlit ground outside, nor the floor the blue
    // of the sky: both get light of the same color.
    let room: [Block; 5] = [
        (DVec3::new(0.0, 3.15, 0.0), [6.6, 0.3, 6.6]),
        (DVec3::new(-3.15, 1.5, 0.0), [0.3, 3.0, 6.6]),
        (DVec3::new(3.15, 1.5, 0.0), [0.3, 3.0, 6.6]),
        (DVec3::new(0.0, 1.5, -3.15), [6.0, 3.0, 0.3]),
        (DVec3::new(0.0, 1.5, 3.15), [6.0, 3.0, 0.3]),
    ];
    let mut app = scene_app(&room, DebugView::None, |s| {
        s.tonemapper = Tonemapper::None;
        s.bloom_intensity = 0.0;
        s.vignette = 0.0;
        s.film_grain = 0.0;
        s.saturation = 1.0;
        s.screen_space_gi = false;
        // The sky light guess on its own (probe GI replaces it where its grid covers).
        s.probe_gi = false;
    });
    app.world.resource_mut::<Atmosphere>().ground_color = [0.02, 0.4, 0.02];
    app.world.spawn((
        Transform::from_xyz(0.0, 1.5, 0.0),
        SkyLightVolume::new([3.0, 1.5, 3.0]).with_sky_light(0.05),
    ));
    // Inside, looking at the far wall; exposed for the dim room.
    let mut cameras = app.world.query::<(&mut Camera, &mut Transform)>();
    let (mut camera, mut transform) = cameras.single_mut(&mut app.world).unwrap();
    camera.exposure = Exposure::Manual { ev100: 9.0 };
    *transform = Transform::from_xyz(0.0, 1.5, 2.5).looking_at(DVec3::new(0.0, 1.5, -3.0), Vec3::Y);
    let Some(img) = render(&mut app, "closed_room") else {
        return;
    };
    let ceiling = color_at(&app, &img, DVec3::new(0.0, 3.0, -1.0));
    let floor = color_at(&app, &img, DVec3::new(0.0, 0.0, -1.0));
    eprintln!("closed room: ceiling {ceiling:.1}, floor {floor:.1}");
    let green = |c: Vec3| c.y / (c.x + c.y + c.z).max(1.0);
    assert!(
        ceiling.length() > 20.0 && floor.length() > 20.0,
        "test scene too dark to judge: ceiling {ceiling:.1}, floor {floor:.1}"
    );
    assert!(
        (green(ceiling) - green(floor)).abs() < 0.04,
        "the ceiling and the floor should get the same light: ceiling {ceiling:.1}, floor {floor:.1}"
    );
}

/// RGBA8 texel of a tangent-space normal.
fn encode_normal(n: Vec3) -> [u8; 4] {
    let n = n.normalize();
    let c = |v: f32| ((v * 0.5 + 0.5) * 255.0).round() as u8;
    [c(n.x), c(n.y), c(n.z), 255]
}

/// Ground with a strong normal map: bumps 2 m apart in both directions that tilt the shading
/// normal by up to 40° (like the folds of a shirt seen from close up).
fn wrinkled_ground(app: &mut App) -> StandardMaterial {
    let tau = std::f32::consts::TAU;
    let bumps = Image::from_fn(64, 64, false, |x, y| {
        let a = (x as f32 + 0.5) / 64.0 * tau;
        let b = (y as f32 + 0.5) / 64.0 * tau;
        encode_normal(Vec3::new(0.85 * a.sin(), 0.85 * b.cos(), 1.0))
    });
    let bumps = app
        .world
        .resource_mut::<engine_assets::Assets<Image>>()
        .add(bumps);
    // The plane's UVs are in meters: one tile every 2 m.
    StandardMaterial::color(0.5, 0.5, 0.5)
        .with_roughness(0.9)
        .with_normal_texture(bumps)
        .with_uv_scale(0.5, 0.5)
}

/// Where the ray through pixel `(x, y)` meets the ground (y = 0), if it does.
fn ground_under_pixel(app: &App, x: u32, y: u32) -> Option<DVec3> {
    let view = app.world.resource::<Renderer>().last_view()?.clone();
    let inv = view.unjittered_view_proj.inverse();
    let ndc = Vec2::new(
        (x as f32 + 0.5) / W as f32 * 2.0 - 1.0,
        1.0 - (y as f32 + 0.5) / H as f32 * 2.0,
    );
    // Reverse-Z: depth 1 is the near plane.
    let near = inv.project_point3(ndc.extend(1.0));
    let far = inv.project_point3(ndc.extend(0.5));
    let dir = (far - near).normalize();
    let origin = view.origin + near.as_dvec3();
    if dir.y >= -1e-4 {
        return None;
    }
    let t = -origin.y / dir.y as f64;
    Some(origin + dir.as_dvec3() * t)
}

/// Linear luminance of pixel `(x, y)`.
fn linear_luma(img: &RgbaImage, x: u32, y: u32) -> f32 {
    let p = img.get_pixel(x, y);
    decode(0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32)
}

/// The pixels showing ground within `half` meters of the carport's middle (in x and z).
fn carport_ground_pixels(app: &App, half: f64) -> Vec<(u32, u32)> {
    let mut pixels = Vec::new();
    for y in 0..H {
        for x in 0..W {
            if let Some(g) = ground_under_pixel(app, x, y)
                && g.x.abs() <= half
                && g.z.abs() <= half
            {
                pixels.push((x, y));
            }
        }
    }
    assert!(
        pixels.len() > 1000,
        "too few ground pixels ({})",
        pixels.len()
    );
    pixels
}

#[test]
fn normal_map_wrinkles_dont_show_in_sky_visibility() {
    // Sky cover is large-scale: it comes from the smooth (vertex) normal, so a normal-mapped
    // ground under and around the carport sees the same sky as a flat one, instead of cover
    // that changes bump by bump.
    let mut flat = scene_app(&[CARPORT], DebugView::SkyVisibility, |_| {});
    let mut wrinkled = scene_app_with_ground(
        &[CARPORT],
        DebugView::SkyVisibility,
        |_| {},
        wrinkled_ground,
    );
    let (Some(flat_img), Some(wrinkled_img)) = (
        render(&mut flat, "sky_visibility_flat"),
        render(&mut wrinkled, "sky_visibility_wrinkled"),
    ) else {
        return;
    };
    let pixels = carport_ground_pixels(&flat, 7.0);
    let (mut sum, mut far_off) = (0.0, 0.0);
    for &(x, y) in &pixels {
        let d = (linear_luma(&flat_img, x, y) - linear_luma(&wrinkled_img, x, y)).abs();
        sum += d;
        if d > 0.05 {
            far_off += 1.0;
        }
    }
    let n = pixels.len() as f32;
    let (mean, far_off) = (sum / n, far_off / n);
    eprintln!(
        "sky visibility, wrinkled vs flat ground over {n} pixels: mean difference {mean:.4}, \
         {:.1}% off by more than 0.05",
        far_off * 100.0
    );
    assert!(
        mean < 0.01 && far_off < 0.01,
        "the normal map shows in sky visibility (mean {mean:.4}, {:.1}% off by more than 0.05)",
        far_off * 100.0
    );
}
