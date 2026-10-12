//! Renders debug shapes over a small lit scene offscreen and checks the pixels along each
//! projected line. Needs a GPU or a software Vulkan driver (lavapipe); passes with a note when
//! neither is available (`RenderDisabled`).

use engine_core::math::{DVec3, Mat4, Quat, Vec3, Vec4};
use engine_core::prelude::*;
use engine_core::registry::call_api;
use engine_core::time::TimeSource;
use engine_debug::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use serde_json::json;

const W: u32 = 480;
const H: u32 = 270;
const FOV_DEG: f32 = 65.0;

/// Camera position relative to the scene offset.
const EYE: DVec3 = DVec3::new(0.0, 1.5, 8.0);
const TARGET: DVec3 = DVec3::new(0.0, 1.0, 0.0);

/// The scene's world offset (0 or far away), stored so the drawing system can use it.
#[derive(Resource, Clone, Copy)]
struct Offset(DVec3);

/// Lines checked by the tests, relative to the scene offset.
const MAGENTA_LINE: (DVec3, DVec3) = (DVec3::new(-3.0, 0.4, 3.0), DVec3::new(3.0, 0.4, 3.0));
const CYAN_HIDDEN: (DVec3, DVec3) = (DVec3::new(-0.5, 1.0, -3.0), DVec3::new(0.5, 1.0, -3.0));
const YELLOW_ON_TOP: (DVec3, DVec3) = (DVec3::new(-0.5, 1.3, -3.0), DVec3::new(0.5, 1.3, -3.0));

/// Headless app with a sun and a 60 m ground plane at `offset` (no camera yet).
fn base_app(offset: DVec3, sun_lux: f32) -> App {
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
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((RenderPlugin, DebugPlugin));
    app.insert_resource(Offset(offset));
    app.world.resource_mut::<Atmosphere>().planet_center =
        offset + DVec3::new(0.0, -6_360_000.0, 0.0);

    let w = &mut app.world;
    w.spawn((
        DirectionalLight {
            illuminance: sun_lux,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(-0.4, -0.8, -0.5), Vec3::Y),
    ));
    w.spawn((
        Transform::from_translation(offset),
        MeshRenderer::new(
            Shape::Plane {
                size: [60.0, 60.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x4f7a3a).with_roughness(0.9),
        ),
    ));
    app
}

fn scene_app(offset: DVec3, sun_lux: f32) -> App {
    let mut app = base_app(offset, sun_lux);
    let w = &mut app.world;
    w.spawn((
        Camera::perspective(FOV_DEG),
        Transform::from_translation(offset + EYE).looking_at(offset + TARGET, Vec3::Y),
    ));
    // Occluder: a 2 m gray cube; the cyan line behind it must be hidden, the yellow one not.
    w.spawn((
        Transform::from_translation(offset + DVec3::new(0.0, 1.0, 0.0)),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [2.0, 2.0, 2.0],
            },
            StandardMaterial::hex(0x808080).with_roughness(0.6),
        ),
    ));

    // A this-frame shape redrawn every frame by a system...
    app.add_systems(
        Update,
        |offset: Res<Offset>, mut debug: ResMut<DebugDraw>| {
            let o = offset.0;
            debug.line(o + MAGENTA_LINE.0, o + MAGENTA_LINE.1, MAGENTA);
            debug.sphere(o + DVec3::new(-2.5, 1.0, 1.0), 0.6, GREEN);
            debug.grid(o, Vec3::Y, 1.0, 6, WHITE);
        },
    );
    // ...and timed shapes added once through the JSON API, the way an AI agent would.
    let p = |v: DVec3| {
        let v = offset + v;
        json!([v.x, v.y, v.z])
    };
    call_api(
        &mut app.world,
        "debug.draw",
        json!({"shapes": [
            {"line": {"a": p(CYAN_HIDDEN.0), "b": p(CYAN_HIDDEN.1), "color": "cyan", "duration": 30}},
            {"line": {"a": p(YELLOW_ON_TOP.0), "b": p(YELLOW_ON_TOP.1), "color": "yellow", "depth": "on_top", "duration": 30}},
            {"arrow": {"from": p(DVec3::new(2.5, 0.0, 1.0)), "to": p(DVec3::new(2.5, 2.0, 1.0)), "color": "orange"}},
            {"box": {"center": p(DVec3::new(0.0, 1.0, 0.0)), "half_extents": [1.05, 1.05, 1.05], "color": [1, 1, 1, 0.5], "duration": -1}}
        ]}),
    )
    .expect("debug.draw");
    app
}

/// Render a few frames and save a screenshot. `None` when there is no GPU.
fn render(app: &mut App, name: &str) -> Option<image::RgbaImage> {
    app.update_n(3);
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

/// The perspective camera of [`scene_app`], relative to the scene offset.
fn perspective_view() -> (Transform, Mat4) {
    let eye = Transform::from_translation(EYE).looking_at(TARGET, Vec3::Y);
    let proj = engine_render::frame::perspective_infinite_reverse(
        FOV_DEG.to_radians(),
        W as f32 / H as f32,
        0.05,
    );
    (eye, proj)
}

/// Pixel position of a point relative to the scene offset (same math as the renderer).
fn project(p: DVec3) -> Vec3 {
    project_with(perspective_view(), p)
}

/// Pixel position of `p` seen by the camera `eye` with projection `proj`.
fn project_with((eye, proj): (Transform, Mat4), p: DVec3) -> Vec3 {
    let view = Mat4::from_quat(Quat::inverse(eye.rotation));
    let rel = (p - eye.translation).as_vec3();
    let clip = proj * view * Vec4::new(rel.x, rel.y, rel.z, 1.0);
    let ndc = clip.truncate() / clip.w;
    Vec3::new(
        (ndc.x * 0.5 + 0.5) * W as f32,
        (0.5 - ndc.y * 0.5) * H as f32,
        ndc.z,
    )
}

/// Pixels sampled along the middle 80% of a projected line: for each sample, the first pixel
/// within one pixel of it that matches `is_color`. Returns (fraction of samples matched,
/// average matching color).
fn along_line(
    img: &image::RgbaImage,
    line: (DVec3, DVec3),
    is_color: impl Fn([u8; 4]) -> bool,
) -> (f32, [f32; 3]) {
    along_projected(img, (project(line.0), project(line.1)), is_color)
}

/// [`along_line`] for a line already projected to pixels.
fn along_projected(
    img: &image::RgbaImage,
    (a, b): (Vec3, Vec3),
    is_color: impl Fn([u8; 4]) -> bool,
) -> (f32, [f32; 3]) {
    let samples = 40;
    let mut hits = 0;
    let mut sum = [0.0f32; 3];
    for i in 0..samples {
        let t = 0.1 + 0.8 * i as f32 / (samples - 1) as f32;
        let p = a.lerp(b, t);
        let found = (-1..=1)
            .flat_map(|dy| (-1..=1).map(move |dx| (dx, dy)))
            .find_map(|(dx, dy)| {
                let (x, y) = (p.x.floor() as i32 + dx, p.y.floor() as i32 + dy);
                if x < 0 || y < 0 || x >= W as i32 || y >= H as i32 {
                    return None;
                }
                let px = img.get_pixel(x as u32, y as u32).0;
                is_color(px).then_some(px)
            });
        if let Some(px) = found {
            hits += 1;
            for c in 0..3 {
                sum[c] += px[c] as f32;
            }
        }
    }
    let n = hits.max(1) as f32;
    (hits as f32 / samples as f32, sum.map(|s| s / n))
}

fn magenta(p: [u8; 4]) -> bool {
    p[0] > 150 && p[2] > 150 && (p[1] as i32) + 50 < p[0] as i32
}

fn cyan(p: [u8; 4]) -> bool {
    p[1] > 140 && p[2] > 140 && (p[0] as i32) + 50 < p[1] as i32
}

fn yellow(p: [u8; 4]) -> bool {
    p[0] > 150 && p[1] > 140 && (p[2] as i32) + 50 < p[1] as i32
}

#[test]
fn draws_shapes_with_both_depth_modes() {
    let mut app = scene_app(DVec3::ZERO, 110_000.0);
    let Some(img) = render(&mut app, "debug_shapes.png") else {
        return;
    };

    let (seen, color) = along_line(&img, MAGENTA_LINE, magenta);
    assert!(
        seen > 0.9,
        "depth-tested magenta line in front of the cube is visible ({seen}, {color:?})"
    );
    let (seen, _) = along_line(&img, CYAN_HIDDEN, cyan);
    assert!(
        seen < 0.1,
        "depth-tested cyan line behind the cube is hidden ({seen})"
    );
    let (seen, color) = along_line(&img, YELLOW_ON_TOP, yellow);
    assert!(
        seen > 0.9,
        "on-top yellow line shows through the cube ({seen}, {color:?})"
    );

    // The ground grid lies exactly on the ground plane and is still visible (depth nudge).
    let grid_line = (DVec3::new(-3.0, 0.0, 2.0), DVec3::new(3.0, 0.0, 2.0));
    let (seen, color) = along_line(&img, grid_line, |p| p[0] > 140 && p[1] > 140 && p[2] > 140);
    assert!(
        seen > 0.8,
        "grid on the ground is visible ({seen}, {color:?})"
    );

    let stats = app.world.resource::<engine_render::RenderStats>();
    assert!(
        stats.features.iter().any(|f| f == "debug_lines"),
        "{stats:?}"
    );
    let diag = app.world.resource::<Diagnostics>();
    // Sphere 96 + grid 14 + line 1 + timed: 2 lines, arrow 5, box 12.
    assert_eq!(diag.get("debug.lines"), Some(130.0));
}

#[test]
fn line_color_is_stable_between_day_and_night() {
    let mut day = scene_app(DVec3::ZERO, 110_000.0);
    let Some(day_img) = render(&mut day, "debug_day.png") else {
        return;
    };
    let mut night = scene_app(DVec3::ZERO, 0.25);
    let Some(night_img) = render(&mut night, "debug_night.png") else {
        return;
    };
    let ev_day = day.world.resource::<engine_render::RenderStats>().ev100;
    let ev_night = night.world.resource::<engine_render::RenderStats>().ev100;
    assert!(
        ev_day - ev_night > 5.0,
        "exposure differs a lot: day {ev_day}, night {ev_night}"
    );

    let (seen_day, c_day) = along_line(&day_img, MAGENTA_LINE, magenta);
    let (seen_night, c_night) = along_line(&night_img, MAGENTA_LINE, magenta);
    assert!(
        seen_day > 0.9 && seen_night > 0.9,
        "{seen_day} {seen_night}"
    );
    for i in 0..3 {
        assert!(
            (c_day[i] - c_night[i]).abs() < 25.0,
            "line color day {c_day:?} vs night {c_night:?}"
        );
    }
}

#[test]
fn lines_stay_precise_1e8_m_from_the_origin() {
    let far = DVec3::new(1.0e8, 0.0, -1.0e8);
    let mut near_app = scene_app(DVec3::ZERO, 110_000.0);
    let Some(near_img) = render(&mut near_app, "debug_origin.png") else {
        return;
    };
    let mut far_app = scene_app(far, 110_000.0);
    let Some(far_img) = render(&mut far_app, "debug_far.png") else {
        return;
    };

    // Each line lands on the same pixels as at the origin. (At 1e8 m an f32 world position
    // is only good to ±4 m, which would move these lines off-screen.)
    for (name, line, is) in [
        ("magenta", MAGENTA_LINE, magenta as fn([u8; 4]) -> bool),
        ("yellow", YELLOW_ON_TOP, yellow),
    ] {
        let (seen, _) = along_line(&far_img, line, is);
        assert!(seen > 0.9, "{name} line at 1e8 m ({seen})");
    }
    let (seen, _) = along_line(&far_img, CYAN_HIDDEN, cyan);
    assert!(seen < 0.1, "depth test still works at 1e8 m ({seen})");

    // And the whole magenta mask matches the origin render.
    let mask =
        |img: &image::RgbaImage| -> Vec<bool> { img.pixels().map(|p| magenta(p.0)).collect() };
    let (a, b) = (mask(&near_img), mask(&far_img));
    let inter = a.iter().zip(&b).filter(|(x, y)| **x && **y).count();
    let union = a.iter().zip(&b).filter(|(x, y)| **x || **y).count();
    assert!(union > 50, "magenta pixels exist ({union})");
    let iou = inter as f32 / union as f32;
    assert!(
        iou > 0.85,
        "magenta pixels match between origin and 1e8 m (IoU {iou})"
    );
}

#[test]
fn depth_tested_lines_work_with_an_orthographic_camera() {
    // Near 0.1 m, far 1000 m: the ground 20 m below the camera sits in the first 2% of the
    // depth range, where the vertex depth nudge is clamped at the near plane. The fragment
    // test must still use the true depth there.
    let mut app = base_app(DVec3::ZERO, 110_000.0);
    let height = 12.0;
    let (near, far) = (0.1, 1000.0);
    let eye = Transform::from_xyz(0.0, 20.0, 0.0).looking_at(DVec3::ZERO, Vec3::NEG_Z);
    app.world.spawn((
        Camera {
            projection: Projection::Orthographic { height, near, far },
            ..Default::default()
        },
        eye,
    ));
    let above = (DVec3::new(-3.0, 1.0, -2.0), DVec3::new(3.0, 1.0, -2.0));
    let below = (DVec3::new(-3.0, -1.0, 0.0), DVec3::new(3.0, -1.0, 0.0));
    let on_top = (DVec3::new(-3.0, -1.0, 2.0), DVec3::new(3.0, -1.0, 2.0));
    app.add_systems(Update, move |mut debug: ResMut<DebugDraw>| {
        debug.line(above.0, above.1, MAGENTA);
        debug.line(below.0, below.1, CYAN);
        debug.line(on_top.0, on_top.1, YELLOW).on_top();
    });
    let Some(img) = render(&mut app, "debug_ortho.png") else {
        return;
    };

    let proj =
        engine_render::frame::orthographic_reverse(height * W as f32 / H as f32, height, near, far);
    let px = |line: (DVec3, DVec3)| {
        (
            project_with((eye, proj), line.0),
            project_with((eye, proj), line.1),
        )
    };
    let (seen, color) = along_projected(&img, px(above), magenta);
    assert!(
        seen > 0.9,
        "depth-tested line 1 m above the ground is visible ({seen}, {color:?})"
    );
    let (seen, _) = along_projected(&img, px(below), cyan);
    assert!(
        seen < 0.1,
        "depth-tested line 1 m under the ground is hidden ({seen})"
    );
    let (seen, color) = along_projected(&img, px(on_top), yellow);
    assert!(
        seen > 0.9,
        "on-top line under the ground is visible ({seen}, {color:?})"
    );
}

/// Linear display value to the 8-bit sRGB value a screenshot stores.
fn srgb8(linear: f32) -> f32 {
    let c = linear.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    s * 255.0
}

#[test]
fn line_colors_are_exact_on_a_dark_night() {
    // Drawn after tonemapping: no tone curve and no night (scotopic) desaturation.
    let mut night = scene_app(DVec3::ZERO, 0.02);
    night.update_n(60);
    let Some(img) = render(&mut night, "debug_dark_night.png") else {
        return;
    };
    let ev = night.world.resource::<engine_render::RenderStats>().ev100;
    assert!(
        ev < 1.0,
        "the scene should be exposed for night (EV100 {ev})"
    );
    let (seen, color) = along_line(&img, MAGENTA_LINE, magenta);
    assert!(seen > 0.9, "magenta line visible at night ({seen})");
    let want = [srgb8(MAGENTA[0]), srgb8(MAGENTA[1]), srgb8(MAGENTA[2])];
    for i in 0..3 {
        assert!(
            (color[i] - want[i]).abs() < 12.0,
            "night line color {color:?}, expected {want:?}"
        );
    }
}

#[test]
fn debug_lines_do_not_change_the_metered_exposure() {
    let metered = |grid: bool| -> Option<f64> {
        let mut app = base_app(DVec3::ZERO, 0.25);
        app.world.spawn((
            Camera::perspective(FOV_DEG),
            Transform::from_translation(EYE).looking_at(TARGET, Vec3::Y),
        ));
        if grid {
            // A dense bright grid covering most of the view.
            app.add_systems(Update, |mut debug: ResMut<DebugDraw>| {
                debug.grid(DVec3::ZERO, Vec3::Y, 0.15, 40, WHITE);
            });
        }
        app.update_n(12);
        if app.world.contains_resource::<RenderDisabled>() {
            return None;
        }
        app.world
            .resource::<Diagnostics>()
            .get("render.ev100_metered")
    };
    let (Some(plain), Some(with_grid)) = (metered(false), metered(true)) else {
        return;
    };
    assert!(
        (plain - with_grid).abs() < 0.1,
        "metered EV100 without lines {plain}, with a dense debug grid {with_grid}"
    );
}
