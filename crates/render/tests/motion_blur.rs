//! Motion blur (`MotionBlur` on the camera): a panning camera smears a thin bright bar over its
//! on-screen speed times the exposure, the same at 30 and 60 fps, less in slow motion; a moving
//! object smears under a still camera while the background stays sharp; a still camera's image
//! is left as it is; and the frame after a camera cut is not blurred. Needs a GPU or lavapipe;
//! passes with a note when neither is available.
//!
//! The scenes are unlit (every pixel is exactly the color it was given) and the camera looks
//! down -Z from the origin.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, Screenshots};
use image::RgbaImage;

const W: u32 = 480;
const H: u32 = 270;
const FOV_Y_DEG: f32 = 40.0;
/// A 180° shutter at 30 fps: 1/60 s.
const EXPOSURE: f64 = 0.5 / 30.0;

/// Pixels per radian of turn at the screen center.
fn px_per_rad() -> f64 {
    0.5 * H as f64 / (0.5 * FOV_Y_DEG as f64).to_radians().tan()
}

/// Turns its entity about the vertical axis (rad/s of world time).
#[derive(Component)]
struct Spin(f32);

/// Moves its entity (m/s of world time).
#[derive(Component)]
struct Drive(DVec3);

fn spin(time: Res<Time>, mut q: Query<(&Spin, &mut Transform)>) {
    for (spin, mut t) in &mut q {
        t.rotation = Quat::from_rotation_y(spin.0 * time.delta_secs()) * t.rotation;
    }
}

fn drive(time: Res<Time>, mut q: Query<(&Drive, &mut Transform)>) {
    for (drive, mut t) in &mut q {
        t.translation += drive.0 * time.delta_secs_f64();
    }
}

/// `step` real seconds per frame, the world running at `time_scale`.
fn app(step: f64, time_scale: f32, aa: AntiAliasing) -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.add_plugins(RenderPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: step });
    if time_scale != 1.0 {
        app.world
            .resource_mut::<TimeDilation>()
            .hold("slow motion", time_scale, 0.0);
    }
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.anti_aliasing = aa;
        s.film_grain = 0.0;
        s.vignette = 0.0;
        s.bloom_intensity = 0.0;
        s.volumetric_fog = false;
    }
    app.world.resource_mut::<FogSettings>().density = 0.0;
    app.add_systems(Update, (spin, drive));
    app
}

fn camera(app: &mut App, blur: Option<MotionBlur>, yaw: f32) -> Entity {
    let mut e = app.world.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 0.0 },
            ..Camera::perspective(FOV_Y_DEG)
        },
        Transform::default().with_rotation(Quat::from_rotation_y(yaw)),
    ));
    if let Some(blur) = blur {
        e.insert(blur);
    }
    e.id()
}

fn unlit(rgb: [f32; 3]) -> StandardMaterial {
    StandardMaterial {
        unlit: true,
        ..StandardMaterial::color(rgb[0], rgb[1], rgb[2])
    }
}

fn quad(app: &mut App, at: DVec3, size: [f32; 2], material: StandardMaterial) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(at),
            MeshRenderer::new(Shape::Quad { size }, material).without_shadows(),
        ))
        .id()
}

/// A black wall 30 m away and a thin, very bright vertical bar 10 m away, 2 px wide.
fn bar_scene(app: &mut App, bar_at: DVec3) -> Entity {
    quad(
        app,
        DVec3::new(0.0, 0.0, -30.0),
        [80.0, 40.0],
        unlit([0.0, 0.0, 0.0]),
    );
    let width = 2.0 * 10.0 / px_per_rad();
    quad(
        app,
        bar_at,
        [width as f32, 2.0],
        unlit([1.0, 1.0, 1.0]).with_emissive([1.0, 1.0, 1.0], 60.0),
    )
}

fn gpu_missing(app: &App) -> bool {
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return true;
    }
    false
}

/// Render one frame (with its motion since the last) and read it back.
fn shoot(app: &mut App, name: &str) -> RgbaImage {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("mb_{name}.png"));
    app.world.resource_mut::<Screenshots>().request(&path);
    app.update();
    let gpu_errors: Vec<String> = engine_core::log::log_buffer()
        .tail(64, Some("error"))
        .into_iter()
        .filter(|l| l.message.contains("GPU error"))
        .map(|l| l.message)
        .collect();
    assert!(
        gpu_errors.is_empty(),
        "GPU errors while rendering {name}: {gpu_errors:#?}"
    );
    image::open(&path).expect("png").to_rgba8()
}

fn luma(img: &RgbaImage, x: u32, y: u32) -> f64 {
    let p = img.get_pixel(x, y);
    0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64
}

/// Width (pixels) of what is lit over half white along the middle rows, averaged.
fn lit_width(img: &RgbaImage) -> f64 {
    let rows = H / 2 - 20..H / 2 + 20;
    let n = rows.len() as f64;
    rows.map(|y| (0..W).filter(|&x| luma(img, x, y) > 128.0).count() as f64)
        .sum::<f64>()
        / n
}

/// Largest per-channel difference between two images.
fn largest_difference(a: &RgbaImage, b: &RgbaImage) -> u8 {
    a.pixels()
        .zip(b.pixels())
        .map(|(p, q)| (0..3).map(|c| p[c].abs_diff(q[c])).max().unwrap())
        .max()
        .unwrap()
}

/// The camera turns at `rate` rad/s of world time, frames `step` real seconds apart with the
/// world at `time_scale`, so the bar 10 m ahead crosses the screen center on the frame
/// captured. Returns the width of the bar's streak.
fn pan(blur: Option<MotionBlur>, rate: f32, step: f64, time_scale: f32, name: &str) -> Option<f64> {
    const SPIN_FRAMES: usize = 3;
    let mut app = app(step, time_scale, AntiAliasing::None);
    let turn = rate * (step as f32 * time_scale);
    let cam = camera(&mut app, blur, -turn * SPIN_FRAMES as f32);
    bar_scene(&mut app, DVec3::new(0.0, 0.0, -10.0));
    app.update_n(4);
    if gpu_missing(&app) {
        return None;
    }
    app.world.entity_mut(cam).insert(Spin(rate));
    app.update_n(SPIN_FRAMES - 1);
    let img = shoot(&mut app, name);
    let width = lit_width(&img);
    eprintln!("{name}: bar {width:.1} px wide");
    Some(width)
}

/// Turn rate (rad/s) that moves the screen center by 30 px over the exposure.
fn pan_rate() -> f32 {
    (30.0 / (px_per_rad() * EXPOSURE)) as f32
}

#[test]
fn a_panning_camera_smears_a_thin_bright_bar_by_its_speed_times_the_exposure() {
    let Some(sharp) = pan(None, pan_rate(), 1.0 / 60.0, 1.0, "pan_off") else {
        return;
    };
    let smeared = pan(
        Some(MotionBlur::default()),
        pan_rate(),
        1.0 / 60.0,
        1.0,
        "pan",
    )
    .unwrap();
    let expected = pan_rate() as f64 * px_per_rad() * EXPOSURE;
    eprintln!("expected streak {expected:.1} px plus the bar");
    assert!(
        sharp < 4.0,
        "the bar without motion blur is {sharp:.1} px wide"
    );
    assert!(
        (smeared - sharp - expected).abs() < 0.15 * expected,
        "the bar smeared to {smeared:.1} px (sharp {sharp:.1}), expected about {expected:.1} more"
    );
}

#[test]
fn the_streak_is_as_long_at_30_fps_as_at_60() {
    let Some(at_60) = pan(
        Some(MotionBlur::default()),
        pan_rate(),
        1.0 / 60.0,
        1.0,
        "pan_60fps",
    ) else {
        return;
    };
    let at_30 = pan(
        Some(MotionBlur::default()),
        pan_rate(),
        1.0 / 30.0,
        1.0,
        "pan_30fps",
    )
    .unwrap();
    assert!(
        (at_30 - at_60).abs() < 0.12 * at_60,
        "the same pan streaks {at_60:.1} px at 60 fps but {at_30:.1} px at 30 fps"
    );
}

#[test]
fn slow_motion_blurs_less() {
    let Some(normal) = pan(
        Some(MotionBlur::default()),
        pan_rate(),
        1.0 / 60.0,
        1.0,
        "pan_normal_speed",
    ) else {
        return;
    };
    let slow = pan(
        Some(MotionBlur::default()),
        pan_rate(),
        1.0 / 60.0,
        0.25,
        "pan_slow_motion",
    )
    .unwrap();
    let sharp = 2.0;
    let expected = sharp + 0.25 * (normal - sharp);
    assert!(
        (slow - expected).abs() < 3.0,
        "at a quarter speed the streak should be about {expected:.1} px, not {slow:.1} (full \
         speed {normal:.1})"
    );
}

/// A checkerboard wall 10 m away, a bright bar in front of it and the camera still: `moving`
/// drives the bar sideways at `speed` m/s (2 m in front of the wall).
fn object_scene(blur: Option<MotionBlur>, speed: f64, name: &str) -> Option<RgbaImage> {
    let mut app = app(1.0 / 60.0, 1.0, AntiAliasing::None);
    camera(&mut app, blur, 0.0);
    let image = app
        .world
        .resource_mut::<engine_assets::Assets<Image>>()
        .add(Image::checker(
            256,
            16,
            [200, 200, 200, 255],
            [30, 30, 30, 255],
        ));
    quad(
        &mut app,
        DVec3::new(0.0, 0.0, -10.0),
        [8.0, 8.0],
        unlit([1.0; 3]).with_base_color_texture(image),
    );
    // Starts left of center so it is in the middle when captured.
    let frames = 6;
    let start = -speed * frames as f64 / 60.0;
    let bar = quad(
        &mut app,
        DVec3::new(start, 0.0, -8.0),
        [(2.0 * 8.0 / px_per_rad()) as f32, 1.0],
        unlit([1.0, 1.0, 1.0]).with_emissive([1.0, 1.0, 1.0], 200.0),
    );
    app.world
        .entity_mut(bar)
        .insert(Drive(DVec3::new(speed, 0.0, 0.0)));
    app.update_n(frames - 1);
    if gpu_missing(&app) {
        return None;
    }
    Some(shoot(&mut app, name))
}

#[test]
fn a_moving_object_smears_while_the_still_background_stays_sharp() {
    // 30 px over the exposure, 8 m away.
    let speed = 30.0 / px_per_rad() * 8.0 / EXPOSURE;
    let Some(off) = object_scene(None, speed, "object_off") else {
        return;
    };
    let on = object_scene(Some(MotionBlur::default()), speed, "object").unwrap();
    // The bar's streak: bright pixels along the middle rows.
    let bright = |img: &RgbaImage| {
        (H / 2 - 20..H / 2 + 20)
            .map(|y| (0..W).filter(|&x| luma(img, x, y) > 230.0).count() as f64)
            .sum::<f64>()
            / 40.0
    };
    let (sharp, smeared) = (bright(&off), bright(&on));
    eprintln!("bar: {sharp:.1} px sharp, {smeared:.1} px with motion blur (expected 30 more)");
    assert!(
        (smeared - sharp - 30.0).abs() < 6.0,
        "the moving bar should streak about 30 px: {smeared:.1} px (sharp {sharp:.1})"
    );
    // The wall away from the bar is untouched.
    let mut worst = 0u8;
    for y in 20..60 {
        for x in 0..W {
            let (p, q) = (off.get_pixel(x, y), on.get_pixel(x, y));
            worst = worst.max((0..3).map(|c| p[c].abs_diff(q[c])).max().unwrap());
        }
    }
    assert!(
        worst <= 2,
        "the still background changed by {worst} levels under motion blur"
    );
}

/// A still camera (with TAA's jitter) over fine detail: a checkered wall and a white bar below
/// the sky, with the sun's disc in view (the sky has no motion vectors of its own: its motion
/// is the camera's). Four frames in a row, so several jitter offsets are seen.
fn still_scene(blur: Option<MotionBlur>, name: &str) -> Option<Vec<RgbaImage>> {
    let mut app = app(1.0 / 60.0, 1.0, AntiAliasing::Taa);
    let cam = camera(&mut app, blur, 0.0);
    app.world.get_mut::<Camera>(cam).unwrap().exposure = Exposure::Manual { ev100: 14.0 };
    app.world.spawn((
        DirectionalLight {
            angular_size_deg: 2.0,
            cast_shadows: false,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(0.15, -0.14, 1.0), Vec3::Y),
    ));
    let image = app
        .world
        .resource_mut::<engine_assets::Assets<Image>>()
        .add(Image::checker(
            256,
            64,
            [220, 220, 220, 255],
            [20, 20, 20, 255],
        ));
    quad(
        &mut app,
        DVec3::new(0.0, -2.6, -10.0),
        [8.0, 5.0],
        unlit([1.0; 3]).with_base_color_texture(image),
    );
    quad(
        &mut app,
        DVec3::new(0.7, -1.2, -6.0),
        [0.05, 1.0],
        unlit([1.0; 3]),
    );
    app.update_n(12);
    if gpu_missing(&app) {
        return None;
    }
    Some(
        (0..4)
            .map(|i| shoot(&mut app, &format!("{name}_{i}")))
            .collect(),
    )
}

/// With a long exposure (a 360° shutter at 24 fps: 1/24 s, 2.5 frames at 60 fps), so that a
/// stray sub-pixel motion (the camera's reprojection picking up TAA's jitter) would become a
/// visible streak.
#[test]
fn a_still_camera_leaves_the_image_as_it_is() {
    let Some(off) = still_scene(None, "still_off") else {
        return;
    };
    let on = still_scene(Some(MotionBlur::new(1.0, 24.0)), "still").unwrap();
    for (i, (off, on)) in off.iter().zip(&on).enumerate() {
        let worst = largest_difference(off, on);
        assert!(
            worst <= 2,
            "motion blur changed a still image by up to {worst} levels (frame {i})"
        );
    }
}

/// The camera hops 12 m sideways between two frames (a cut), the bar still 10 m in front.
fn cut_scene(blur: Option<MotionBlur>, name: &str) -> Option<RgbaImage> {
    let mut app = app(1.0 / 60.0, 1.0, AntiAliasing::None);
    let cam = camera(&mut app, blur, 0.0);
    bar_scene(&mut app, DVec3::new(0.0, 0.0, -10.0));
    bar_scene(&mut app, DVec3::new(12.0, 0.0, -10.0));
    app.update_n(4);
    if gpu_missing(&app) {
        return None;
    }
    app.world.get_mut::<Transform>(cam).unwrap().translation.x += 12.0;
    Some(shoot(&mut app, name))
}

#[test]
fn the_frame_after_a_camera_cut_is_not_blurred() {
    let Some(off) = cut_scene(None, "cut_off") else {
        return;
    };
    let on = cut_scene(Some(MotionBlur::default()), "cut").unwrap();
    let worst = largest_difference(&off, &on);
    assert!(
        lit_width(&off) < 4.0,
        "the bar should be in view after the cut"
    );
    assert!(
        worst <= 2,
        "the frame after the cut was blurred: it differs by up to {worst} levels"
    );
}
