//! Portals: through a portal the camera sees what a camera standing beyond the exit sees (the
//! same pixels, as if the two places were joined there), nothing that lies behind the exit's
//! plane, and nothing from behind the portal. Needs a GPU or lavapipe; passes with a note when
//! neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{Pipelining, RenderDisabled, capture_now};

const WINDOW: UVec2 = UVec2::new(400, 300);
const FOV: f32 = 60.0;
/// The portal: 2 × 2 m, facing +Z, its middle 1.2 m up.
const ENTRY: DVec3 = DVec3::new(0.0, 1.2, 0.0);
const SIZE: Vec2 = Vec2::new(2.0, 2.0);
/// Where it leads.
const EXIT: DVec3 = DVec3::new(60.0, 1.2, 0.0);
const EYE: DVec3 = DVec3::new(0.3, 1.5, 4.0);
const TARGET: DVec3 = DVec3::new(0.0, 1.2, 0.0);

#[derive(Clone, Copy, PartialEq)]
enum Shot {
    /// The camera in front of the portal.
    Portal,
    /// The camera where the portal's view stands, beyond the exit, and no portal.
    Beyond,
    /// The camera behind the portal.
    Behind,
}

fn app(pipelining: Pipelining) -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = WINDOW.x;
        w.height = WINDOW.y;
    }
    app.add_plugins(RenderPlugin);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.anti_aliasing = AntiAliasing::None;
        s.film_grain = 0.0;
        s.pipelining = pipelining;
        // Screen-space bounce light sees only what is on screen, and the portal's view doesn't
        // see what lies between its eye and the exit: the bounce from there differs.
        s.screen_space_gi = false;
    }
    app
}

/// What the portal's view sees of `p`: the camera's pose carried through the portal.
fn carried(p: DVec3) -> DVec3 {
    // The exit has no rotation: a half turn about Y, then over to the exit.
    let local = p - ENTRY;
    EXIT + DVec3::new(-local.x, local.y, -local.z)
}

fn ball(w: &mut World, at: DVec3, radius: f32, color: (f32, f32, f32)) {
    w.spawn((
        Transform::from_translation(at),
        MeshRenderer::new(
            Shape::Sphere {
                radius,
                segments: 48,
            },
            StandardMaterial::color(color.0, color.1, color.2),
        ),
    ));
}

/// A red ball behind the portal; beyond the exit (its +Z side) a blue ball, and behind the exit
/// (between it and where its view stands) a green one the view must not show.
fn scene(w: &mut World, shot: Shot) {
    let camera = |at: DVec3, target: DVec3| {
        (
            Camera {
                exposure: Exposure::Manual { ev100: 14.0 },
                ..Camera::perspective(FOV)
            },
            Transform::from_translation(at).looking_at(target, Vec3::Y),
        )
    };
    match shot {
        Shot::Portal => {
            w.spawn(camera(EYE, TARGET));
        }
        Shot::Beyond => {
            w.spawn(camera(carried(EYE), carried(TARGET)));
        }
        Shot::Behind => {
            w.spawn(camera(DVec3::new(0.0, 1.5, -4.0), TARGET));
        }
    }
    if shot != Shot::Beyond {
        let exit = w.spawn(Transform::from_translation(EXIT)).id();
        w.spawn((Transform::from_translation(ENTRY), Portal::new(exit, SIZE)));
        ball(w, EXIT + DVec3::new(-0.2, 0.2, -2.0), 0.9, (0.1, 0.8, 0.2));
    }
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.4, -0.8, -0.45), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [300.0, 300.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.35, 0.4, 0.3),
        ),
    ));
    ball(
        w,
        ENTRY + DVec3::new(0.0, -0.4, -3.0),
        0.8,
        (0.8, 0.25, 0.2),
    );
    ball(
        w,
        EXIT + DVec3::new(0.4, -0.4, 3.0),
        0.8,
        (0.15, 0.25, 0.85),
    );
    ball(w, EXIT + DVec3::new(-1.2, -0.7, 6.0), 0.5, (0.85, 0.7, 0.2));
}

/// Frames a moving camera renders, sliding sideways until it reaches its place.
const MOVING_FRAMES: i32 = 16;

/// The picture after a few frames, and how many extra views rendered. `moving`: with temporal
/// anti-aliasing, the camera slides 4 cm a frame on the way to its place.
fn render(
    shot: Shot,
    pipelining: Pipelining,
    moving: bool,
    name: &str,
) -> Option<(image::RgbImage, f64)> {
    let mut app = app(pipelining);
    scene(&mut app.world, shot);
    if moving {
        app.world.resource_mut::<RenderSettings>().anti_aliasing = AntiAliasing::Taa;
        let camera = app
            .world
            .query_filtered::<Entity, With<Camera>>()
            .single(&app.world)
            .expect("one camera");
        for i in -MOVING_FRAMES..=0 {
            let offset = DVec3::X * 0.04 * i as f64;
            let (eye, target) = match shot {
                Shot::Beyond => (carried(EYE + offset), carried(TARGET + offset)),
                _ => (EYE + offset, TARGET + offset),
            };
            *app.world.get_mut::<Transform>(camera).expect("camera") =
                Transform::from_translation(eye).looking_at(target, Vec3::Y);
            app.update();
        }
    } else {
        app.update_n(5);
    }
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let dir = std::env::temp_dir().join("engine-portals");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(name);
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(out) = std::env::var("ENGINE_SHOTS") {
        let _ = std::fs::copy(&path, std::path::Path::new(&out).join(name));
    }
    let views = app
        .world
        .resource::<Diagnostics>()
        .get("render.extra_views")
        .unwrap_or(0.0);
    Some((image::open(&path).expect("read the shot").to_rgb8(), views))
}

/// Whether pixel `(x, y)` lies inside the portal on screen, `margin` pixels in from its edges.
fn inside_portal(x: u32, y: u32, margin: f32) -> bool {
    let view = Transform::from_translation(EYE).looking_at(TARGET, Vec3::Y);
    let world_to_view = view.rotation.inverse();
    let aspect = WINDOW.x as f32 / WINDOW.y as f32;
    let f = 1.0 / (FOV.to_radians() * 0.5).tan();
    let pixel = |p: DVec3| {
        let v = world_to_view * (p - EYE).as_vec3();
        let ndc = Vec2::new(f / aspect * v.x, f * v.y) / -v.z;
        Vec2::new(
            (ndc.x + 1.0) * 0.5 * WINDOW.x as f32,
            (1.0 - ndc.y) * 0.5 * WINDOW.y as f32,
        )
    };
    let h = SIZE.as_dvec2() * 0.5;
    let c = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
        .map(|(sx, sy)| pixel(ENTRY + DVec3::new(sx * h.x, sy * h.y, 0.0)));
    let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
    // Inside every edge (the corners wind clockwise on screen) by `margin`.
    (0..4).all(|i| {
        let (a, b) = (c[i], c[(i + 1) % 4]);
        let edge = (b - a).normalize();
        let cross = edge.x * (p.y - a.y) - edge.y * (p.x - a.x);
        cross <= -margin
    })
}

fn through_the_portal(pipelining: Pipelining, moving: bool, tag: &str) {
    let Some((portal, views)) = render(
        Shot::Portal,
        pipelining,
        moving,
        &format!("portal_{tag}.png"),
    ) else {
        return;
    };
    let Some((beyond, _)) = render(
        Shot::Beyond,
        pipelining,
        moving,
        &format!("beyond_{tag}.png"),
    ) else {
        return;
    };
    assert_eq!(views, 1.0, "render.extra_views");
    let mut diffs = Vec::new();
    let mut green = 0;
    let mut blue = 0;
    for (x, y, p) in portal.enumerate_pixels() {
        if !inside_portal(x, y, 2.0) {
            continue;
        }
        let q = beyond.get_pixel(x, y);
        diffs.push(
            p.0.iter()
                .zip(q.0)
                .map(|(a, b)| a.abs_diff(b))
                .max()
                .unwrap_or(0),
        );
        green += (p.0[1] as i32 > p.0[0] as i32 + 50 && p.0[1] as i32 > p.0[2] as i32 + 50) as u32;
        blue += (p.0[2] as i32 > p.0[0] as i32 + 40) as u32;
    }
    diffs.sort_unstable();
    let n = diffs.len();
    assert!(n > 5000, "only {n} pixels inside the portal");
    let mean = diffs.iter().map(|&d| d as f64).sum::<f64>() / n as f64;
    let p95 = diffs[n * 95 / 100];
    eprintln!(
        "{n} pixels through the portal differ from the camera beyond by {mean:.2} on average, \
         {p95} at the 95th percentile, {} at most; {blue} blue, {green} green",
        diffs[n - 1]
    );
    assert!(
        blue > 300,
        "the blue ball beyond the exit shows in {blue} pixels"
    );
    assert!(
        green == 0,
        "{green} pixels show the green ball behind the exit"
    );
    // Resampling the picture blurs edges a little; the rest matches. Moving, the picture is
    // resolved twice (by its own view's anti-aliasing and the main view's): softer still, but
    // without the ghosts the flat rectangle's motion would leave (that measured 15 here).
    let (max_mean, max_p95) = if moving { (2.5, 9) } else { (2.0, 3) };
    assert!(mean < max_mean, "mean difference {mean:.2}");
    assert!(p95 <= max_p95, "95th percentile difference {p95}");
}

#[test]
fn a_portal_shows_what_a_camera_beyond_the_exit_sees() {
    through_the_portal(Pipelining::Off, false, "inline");
}

#[test]
fn a_portal_works_pipelined() {
    through_the_portal(Pipelining::Always, false, "pipelined");
}

#[test]
fn a_moving_camera_sees_through_a_portal_without_smearing() {
    // Temporal anti-aliasing follows what lies beyond the portal (its motion vectors), so the
    // picture stays as sharp as the camera beyond's.
    through_the_portal(Pipelining::Off, true, "moving");
}

#[test]
fn a_portal_seen_from_behind_shows_nothing_and_costs_no_view() {
    let Some((shot, views)) = render(Shot::Behind, Pipelining::Off, false, "behind.png") else {
        return;
    };
    assert_eq!(views, 0.0, "render.extra_views");
    // The red ball (in front of the camera now) shows, the blue one beyond the exit doesn't
    // (the sky is less saturated).
    let red = shot
        .pixels()
        .filter(|p| p.0[0] as i32 > p.0[2] as i32 + 60)
        .count();
    let blue = shot
        .pixels()
        .filter(|p| p.0[2] as i32 > p.0[1] as i32 + 70)
        .count();
    assert!(red > 300, "{red} red pixels");
    assert!(blue < 20, "{blue} blue pixels");
}
