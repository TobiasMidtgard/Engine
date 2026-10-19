//! Volumetric fog: sunbeams through a hole in a wall light up the fog behind it, the volumetric
//! and the analytic fog dim distant things alike, fog volumes keep their fog inside their shape
//! (and follow their transform), the temporal accumulation holds still with a still camera, and
//! the sky behind a fogged hillside is as fogged as the hillside. Needs a GPU or lavapipe;
//! passes with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, Renderer, capture_now};
use image::RgbaImage;

const W: u32 = 320;
const H: u32 = 180;

/// An app with a plain tone curve (pixel values follow radiance) and no screen effects that
/// would blur the comparison.
fn app(configure: impl FnOnce(&mut RenderSettings)) -> App {
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
        s.sharpness = 0.0;
        s.anti_aliasing = AntiAliasing::None;
        // Night vision would shift dim pixels toward blue-gray and lower their luma.
        s.night_desaturation = 0.0;
        configure(&mut s);
    }
    app
}

/// A dark world: no sun, no sky, no ambient fill. Only emission lights anything.
fn darken(app: &mut App) {
    app.world.resource_mut::<Atmosphere>().enabled = false;
    app.world.resource_mut::<AmbientLight>().brightness = 0.0;
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

/// Where world point `p` lands on screen, in pixels.
fn screen(app: &App, p: DVec3) -> (i32, i32) {
    let view = app
        .world
        .resource::<Renderer>()
        .last_view()
        .unwrap()
        .clone();
    let clip = view.unjittered_view_proj * (p - view.origin).as_vec3().extend(1.0);
    let ndc = clip.truncate() / clip.w;
    (
        ((ndc.x * 0.5 + 0.5) * W as f32) as i32,
        ((0.5 - ndc.y * 0.5) * H as f32) as i32,
    )
}

fn linear(v: u8) -> f32 {
    let v = v as f32 / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn luma(p: &image::Rgba<u8>) -> f32 {
    0.2126 * linear(p[0]) + 0.7152 * linear(p[1]) + 0.0722 * linear(p[2])
}

/// Mean linear luma of the 5x5 pixels around where `p` lands.
fn luma_at(app: &App, img: &RgbaImage, p: DVec3) -> f32 {
    let (x, y) = screen(app, p);
    let mut sum = 0.0;
    for dy in -2..=2 {
        for dx in -2..=2 {
            let px = img.get_pixel(
                (x + dx).clamp(0, W as i32 - 1) as u32,
                (y + dy).clamp(0, H as i32 - 1) as u32,
            );
            sum += luma(px) / 25.0;
        }
    }
    sum
}

fn spawn_box(w: &mut World, center: DVec3, size: [f32; 3], material: StandardMaterial) {
    w.spawn((
        Transform::from_translation(center),
        MeshRenderer::new(Shape::Cuboid { size }, material),
    ));
}

/// Sunbeam scene: a 12 x 6 x 8 m box of fog in the shadow of a tall wall, sunlight coming in
/// through a 2 m hole in the wall, a black backdrop behind the fog. The sun shines from +x and
/// 27 degrees up; the camera looks along -z at the fog from 14 m, so the beam crosses the view
/// from the upper right to the lower left.
fn shaft_app(configure: impl FnOnce(&mut RenderSettings)) -> App {
    let mut app = app(configure);
    app.world.resource_mut::<FogSettings>().density = 0.0;
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(60.0)
        },
        Transform::from_xyz(0.0, 3.0, 14.0).looking_at(DVec3::new(0.0, 3.0, 0.0), Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-2.0, -1.0, 0.0), Vec3::Y),
    ));
    w.spawn((
        Transform::from_xyz(0.0, 3.0, 0.0),
        FogVolume::new([6.0, 3.0, 4.0])
            .with_density(0.15)
            .with_edge_falloff(0.0)
            .with_anisotropy(0.3),
    ));
    let wall = StandardMaterial::color(0.3, 0.3, 0.3);
    // The wall at x = 7: 16 m wide (z), 14 m tall, with a 2 x 2 m hole centered 6.5 m up.
    spawn_box(w, DVec3::new(7.2, 2.75, 0.0), [0.4, 5.5, 16.0], wall);
    spawn_box(w, DVec3::new(7.2, 10.75, 0.0), [0.4, 6.5, 16.0], wall);
    spawn_box(w, DVec3::new(7.2, 6.5, -4.5), [0.4, 2.0, 7.0], wall);
    spawn_box(w, DVec3::new(7.2, 6.5, 4.5), [0.4, 2.0, 7.0], wall);
    // Black backdrop and floor, so the fog's light is all there is to see.
    let black = StandardMaterial::color(0.01, 0.01, 0.01).with_roughness(1.0);
    spawn_box(w, DVec3::new(0.0, 6.0, -6.0), [30.0, 14.0, 0.4], black);
    spawn_box(w, DVec3::new(0.0, -0.2, 0.0), [40.0, 0.4, 40.0], black);
    app
}

/// Points on the beam's axis (z = 0) and beside it at the same depth in the fog.
fn shaft_points() -> [(DVec3, DVec3); 3] {
    // Through the hole's center (7, 6.5, 0), falling 0.5 m per meter toward -x.
    let on = |x: f64| DVec3::new(x, 6.5 - 0.5 * (7.0 - x), 0.0);
    [
        (on(-3.0), on(-3.0) + DVec3::new(0.0, 2.2, 0.0)),
        (on(0.0), on(0.0) + DVec3::new(0.0, 2.2, 0.0)),
        (on(3.0), on(3.0) - DVec3::new(0.0, 2.2, 0.0)),
    ]
}

#[test]
fn sunbeams_through_a_hole_light_up_the_fog() {
    let mut app = shaft_app(|_| {});
    let Some(img) = render(&mut app, 30, "volumetric_shaft") else {
        return;
    };
    for (on, beside) in shaft_points() {
        let (a, b) = (luma_at(&app, &img, on), luma_at(&app, &img, beside));
        eprintln!("shaft at {on}: {a:.4}, beside it at {beside}: {b:.4}");
        assert!(b > 0.002, "the fog beside the shaft is black: {b}");
        assert!(
            a > b * 1.25,
            "the shaft at {on} ({a:.4}) isn't brighter than the fog beside it ({b:.4})"
        );
    }
    // Without volumetric fog the box is not drawn and there is no shaft.
    let mut flat = shaft_app(|s| s.volumetric_fog = false);
    let Some(img) = render(&mut flat, 4, "volumetric_shaft_off") else {
        return;
    };
    let (on, beside) = shaft_points()[1];
    let (a, b) = (luma_at(&flat, &img, on), luma_at(&flat, &img, beside));
    eprintln!("without volumetric fog: {a:.4} on the shaft, {b:.4} beside it");
    assert!(
        (a - b).abs() < 0.01,
        "a shaft without volumetric fog: {a} vs {b}"
    );
}

/// Two emissive panels in homogeneous height fog: one at 60 m (inside the volumetric range),
/// one at 400 m (beyond it, where the analytic fog takes over).
const NEAR_PANEL: DVec3 = DVec3::new(-6.0, 2.0, -60.0);
const FAR_PANEL: DVec3 = DVec3::new(40.0, 20.0, -400.0);

fn panels_app(volumetric: bool, dark: bool) -> App {
    let mut app = app(|s| s.volumetric_fog = volumetric);
    {
        let mut fog = app.world.resource_mut::<FogSettings>();
        fog.density = 0.004;
        fog.height_falloff = 0.0;
    }
    if dark {
        darken(&mut app);
    }
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual {
                ev100: if dark { 0.0 } else { 14.0 },
            },
            ..Camera::perspective(50.0)
        },
        Transform::from_xyz(0.0, 2.0, 0.0).looking_at(DVec3::new(4.0, 4.0, -100.0), Vec3::Y),
    ));
    if !dark {
        // The sun behind the camera: the panels' shadows fall behind them.
        w.spawn((
            DirectionalLight::default(),
            Transform::default().looking_at(DVec3::new(0.3, -0.6, -1.0), Vec3::Y),
        ));
    }
    let glow = if dark { 1.0 } else { 6000.0 };
    for (center, size) in [(NEAR_PANEL, 4.0), (FAR_PANEL, 40.0)] {
        w.spawn((
            Transform::from_translation(center).looking_at(DVec3::new(0.0, 2.0, 0.0), Vec3::Y),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [size, size, 0.2],
                },
                StandardMaterial::color(0.0, 0.0, 0.0).with_emissive([1.0, 1.0, 1.0], glow),
            ),
        ));
    }
    app
}

#[test]
fn volumetric_and_analytic_fog_dim_distant_things_alike() {
    // In the dark only the panels' own light reaches the camera, dimmed by the fog.
    let mut volumetric = panels_app(true, true);
    let mut analytic = panels_app(false, true);
    let (Some(v), Some(a)) = (
        render(&mut volumetric, 24, "volumetric_panels_dark"),
        render(&mut analytic, 4, "analytic_panels_dark"),
    ) else {
        return;
    };
    // Unfogged the panels show 1 cd/m^2 at EV 0: 1 / 1.2 on screen.
    for (panel, name) in [(NEAR_PANEL, "near"), (FAR_PANEL, "far")] {
        let expected = (-0.004 * panel.distance(DVec3::new(0.0, 2.0, 0.0))).exp() as f32 / 1.2;
        let (lv, la) = (
            luma_at(&volumetric, &v, panel),
            luma_at(&analytic, &a, panel),
        );
        eprintln!("{name} panel: volumetric {lv:.4}, analytic {la:.4}, expected {expected:.4}");
        assert!(
            (lv / la - 1.0).abs() < 0.04,
            "{name} panel: volumetric {lv} and analytic {la} fog disagree"
        );
        assert!(
            (la / expected - 1.0).abs() < 0.04,
            "{name} panel: {la} on screen, {expected} expected"
        );
    }

    // In daylight the fog also scatters sun and sky light toward the camera; with no shadows
    // on the way both should still agree.
    let mut volumetric = panels_app(true, false);
    let mut analytic = panels_app(false, false);
    let (Some(v), Some(a)) = (
        render(&mut volumetric, 24, "volumetric_panels_day"),
        render(&mut analytic, 4, "analytic_panels_day"),
    ) else {
        return;
    };
    for (panel, name) in [(NEAR_PANEL, "near"), (FAR_PANEL, "far")] {
        let (lv, la) = (
            luma_at(&volumetric, &v, panel),
            luma_at(&analytic, &a, panel),
        );
        eprintln!("{name} panel in daylight: volumetric {lv:.4}, analytic {la:.4}");
        assert!(
            (lv / la - 1.0).abs() < 0.1,
            "{name} panel in daylight: volumetric {lv} and analytic {la} fog disagree"
        );
    }
}

/// A dark ridge 60 m high, 800 m away across the view, in thick ground fog in daylight (seen
/// from 2 m up, the sun behind the camera).
const RIDGE: DVec3 = DVec3::new(0.0, 30.0, -800.0);

fn ridge_app() -> App {
    let mut app = app(|_| {});
    {
        let mut fog = app.world.resource_mut::<FogSettings>();
        fog.density = 0.004;
        fog.height_falloff = 0.01;
    }
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(50.0)
        },
        Transform::from_xyz(0.0, 2.0, 0.0).looking_at(DVec3::new(0.0, 30.0, -800.0), Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.3, -0.5, -1.0), Vec3::Y),
    ));
    spawn_box(
        w,
        RIDGE,
        [2000.0, 60.0, 20.0],
        StandardMaterial::color(0.05, 0.06, 0.04),
    );
    app
}

#[test]
fn the_sky_behind_a_fogged_ridge_is_fogged_too() {
    let mut app = ridge_app();
    let Some(img) = render(&mut app, 24, "fogged_ridge") else {
        return;
    };
    // Below the ridge's top edge and above it, in the sky.
    let ridge = luma_at(&app, &img, RIDGE + DVec3::new(0.0, 10.0, 10.0));
    let sky = luma_at(&app, &img, RIDGE + DVec3::new(0.0, 60.0, 10.0));
    eprintln!("ridge {ridge:.4}, sky above it {sky:.4}");
    // 800 m of fog leaves 4% of the ridge: the sky right above it must look the same, not a
    // bright wall of clear sky.
    assert!(
        (sky / ridge - 1.0).abs() < 0.15,
        "the ridge ({ridge}) and the sky above it ({sky}) should both be fog"
    );
    // 20 degrees up the fog has thinned out (its density halves every 69 m up) and the blue
    // sky shows through the grey.
    let blue = |p: DVec3| {
        let (x, y) = screen(&app, p);
        let px = img.get_pixel(x as u32, y as u32);
        let (r, g, b) = (linear(px[0]), linear(px[1]), linear(px[2]));
        b / (r + g + b)
    };
    let (low, high) = (
        blue(RIDGE + DVec3::new(0.0, 60.0, 10.0)),
        blue(DVec3::new(0.0, 300.0, -790.0)),
    );
    eprintln!("blue share: {low:.3} above the ridge, {high:.3} 20 degrees up");
    assert!(
        high > low + 0.02,
        "the fog should thin out higher up the sky: blue share {high} vs {low}"
    );
}

/// A glowing box of fog in the dark, 6 x 2 x 2 m along its local axes, turned on its end (90
/// degrees about z, so it stands 6 m tall) and centered 3 m to the right, seen from 12 m.
const VOLUME_CENTER: DVec3 = DVec3::new(3.0, 2.0, 0.0);

fn volume_app() -> App {
    let mut app = app(|_| {});
    app.world.resource_mut::<FogSettings>().density = 0.0;
    darken(&mut app);
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 0.0 },
            ..Camera::perspective(50.0)
        },
        Transform::from_xyz(0.0, 2.0, 12.0).looking_at(DVec3::new(0.0, 2.0, 0.0), Vec3::Y),
    ));
    w.spawn((
        Transform::from_translation(VOLUME_CENTER)
            .with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)),
        FogVolume::new([3.0, 1.0, 1.0])
            .with_density(0.05)
            .with_emission([0.2, 0.2, 0.2])
            .with_edge_falloff(0.1),
    ));
    app
}

#[test]
fn fog_volumes_keep_their_fog_inside() {
    let mut app = volume_app();
    let Some(img) = render(&mut app, 24, "volumetric_box") else {
        return;
    };
    // Standing on its end the box spans y -1..5 and x 2..4. Unturned it would span y 1..3 and
    // x 0..6, so the points above and below its center and those to its sides only pass if it
    // follows its transform.
    let c = VOLUME_CENTER;
    let inside = [
        c,
        c + DVec3::new(0.0, 2.0, 0.0),
        c - DVec3::new(0.0, 2.0, 0.0),
    ];
    let outside = [
        c + DVec3::new(2.5, 0.0, 0.0),
        c - DVec3::new(2.5, 0.0, 0.0),
        c + DVec3::new(0.0, 5.0, 0.0),
        c - DVec3::new(0.0, 5.0, 0.0),
    ];
    for p in inside {
        let l = luma_at(&app, &img, p);
        eprintln!("inside at {p}: {l:.4}");
        assert!(l > 0.1, "no fog inside the volume at {p}: {l}");
    }
    for p in outside {
        let l = luma_at(&app, &img, p);
        eprintln!("outside at {p}: {l:.4}");
        assert!(l < 0.01, "fog outside the volume at {p}: {l}");
    }
}

#[test]
fn fog_holds_still_with_a_still_camera() {
    let mut app = shaft_app(|_| {});
    if render(&mut app, 40, "volumetric_still_0").is_none() {
        return;
    }
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    let mut frames = Vec::new();
    for i in 0..8 {
        let path = dir.join(format!("volumetric_still_{}.png", i + 1));
        capture_now(&mut app.world, &path).expect("screenshot");
        frames.push(image::open(&path).expect("png").to_rgba8());
    }
    // The fog's part of the picture, as the mean luma of 8x8-pixel blocks (the display dither
    // changes every frame; averaging blocks removes it, flicker would remain).
    let (x0, y0) = screen(&app, DVec3::new(-5.5, 5.5, 4.0));
    let (x1, y1) = screen(&app, DVec3::new(5.5, 0.5, 4.0));
    let region = |img: &RgbaImage| -> Vec<f32> {
        let mut blocks = Vec::new();
        for by in (y0.max(0)..y1.min(H as i32) - 7).step_by(8) {
            for bx in (x0.max(0)..x1.min(W as i32) - 7).step_by(8) {
                let mut sum = 0.0;
                for y in by..by + 8 {
                    for x in bx..bx + 8 {
                        sum += luma(img.get_pixel(x as u32, y as u32)) / 64.0;
                    }
                }
                blocks.push(sum);
            }
        }
        blocks
    };
    let regions: Vec<Vec<f32>> = frames.iter().map(region).collect();
    let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
    let mut worst_mean_step = 0.0f32;
    let mut worst_step = 0.0f32;
    for pair in regions.windows(2) {
        let steps: Vec<f32> = pair[0]
            .iter()
            .zip(&pair[1])
            .map(|(a, b)| (a - b).abs())
            .collect();
        worst_mean_step = worst_mean_step.max(mean(&steps));
        worst_step = worst_step.max(steps.iter().copied().fold(0.0, f32::max));
    }
    let brightness = mean(&regions[0]);
    eprintln!(
        "fog region: {} blocks, mean {brightness:.4}; frame to frame: mean change {worst_mean_step:.5}, largest {worst_step:.5}",
        regions[0].len()
    );
    // Each frame samples new points in every froxel; without the temporal blend this scene's
    // blocks change by 17% on average (67% at most) from frame to frame.
    assert!(
        worst_mean_step < brightness * 0.025,
        "the fog flickers: blocks change by {worst_mean_step} on average per frame"
    );
    assert!(
        worst_step < brightness * 0.1,
        "a patch of fog flickers by {worst_step} per frame"
    );

    // Drift: 40 frames later the picture is the same.
    app.update_n(40);
    let path = dir.join("volumetric_still_late.png");
    capture_now(&mut app.world, &path).expect("screenshot");
    let late = mean(&region(&image::open(&path).expect("png").to_rgba8()));
    eprintln!("fog region mean {brightness:.4} then {late:.4} 48 frames later");
    assert!(
        (late / brightness - 1.0).abs() < 0.01,
        "the fog drifts: {brightness} then {late}"
    );
}

/// Flat ground under a layer of ground fog, seen from 120 m up with the sun low ahead (like a
/// valley at dawn from a cliff): the view of the ground sweeps across froxel rows and slices.
fn ground_fog_app(volumetric: bool) -> App {
    let mut app = app(|s| {
        s.volumetric_fog = volumetric;
        s.volumetric_fog_range = 1000.0;
        s.shadow_distance = 1000.0;
    });
    {
        let mut fog = app.world.resource_mut::<FogSettings>();
        fog.density = 0.016;
        fog.height_falloff = 0.12;
        fog.base_height = 4.0;
    }
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 13.0 },
            ..Camera::perspective(50.0)
        },
        Transform::from_xyz(0.0, 120.0, 0.0).looking_at(DVec3::new(0.0, 0.0, -600.0), Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.05, -0.07, 1.0), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [4000.0, 4000.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.2, 0.2, 0.2).with_roughness(1.0),
        ),
    ));
    app
}

/// How much the luma of the rows of the ground (averaged across the middle of the picture)
/// wobbles around its smooth trend (banding), and its mean.
fn row_wobble(app: &App, img: &RgbaImage) -> (f32, f32) {
    let (_, top) = screen(app, DVec3::new(0.0, 0.0, -1500.0));
    let rows: Vec<f32> = (top.max(0) as u32 + 4..H)
        .map(|y| {
            (W / 4..W * 3 / 4)
                .map(|x| luma(img.get_pixel(x, y)))
                .sum::<f32>()
                / (W / 2) as f32
        })
        .collect();
    let mean = rows.iter().sum::<f32>() / rows.len() as f32;
    // Each row against the average of the 9 rows around it.
    let mut sq = 0.0;
    let mut n = 0;
    for i in 4..rows.len() - 4 {
        let trend = rows[i - 4..=i + 4].iter().sum::<f32>() / 9.0;
        sq += (rows[i] - trend).powi(2);
        n += 1;
    }
    ((sq / n as f32).sqrt(), mean)
}

#[test]
fn ground_fog_seen_at_a_glance_has_no_bands() {
    let mut volumetric = ground_fog_app(true);
    let mut analytic = ground_fog_app(false);
    let (Some(v), Some(a)) = (
        render(&mut volumetric, 30, "volumetric_ground_fog"),
        render(&mut analytic, 4, "analytic_ground_fog"),
    ) else {
        return;
    };
    let (wv, mv) = row_wobble(&volumetric, &v);
    let (wa, ma) = row_wobble(&analytic, &a);
    eprintln!(
        "ground in fog: volumetric mean {mv:.4}, wobble {wv:.5}; analytic mean {ma:.4}, wobble {wa:.5}"
    );
    // Bands one row of froxels apart came from froxels behind the ground (in its shadow and under
    // its cover), from blending rows whose rays meet the fog layer meters apart, and from blending
    // their light rather than their radiance where the sun's glow changes from row to row
    // (wobbles of 0.0245, 0.0035 and 0.0026 against the analytic fog's 0.0009).
    assert!(
        wv < 1.5 * wa + 0.001 * mv,
        "the fogged ground shows bands: row wobble {wv} (analytic fog: {wa})"
    );
}
