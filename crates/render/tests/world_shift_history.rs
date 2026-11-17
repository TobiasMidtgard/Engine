//! Histories across a [`WorldShift`]: the renderer moves its previous frame into the frame the
//! world moved to. A camera a plain rebase carries along (a floating-origin move, or one that
//! also turns the world) keeps its histories; a trip that re-anchors the world on another place
//! and puts the camera back at the same coordinates (`engine_space::travel_to`) cuts them. Before,
//! it was the other way round: a trip kept the old place's fog history, which showed as a dark
//! or lit wedge of froxels over the new place for 70 frames and more, and every rebase cut.
//! Needs a GPU or lavapipe; passes with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, Renderer, capture_now};
use engine_world::WorldShift;
use image::RgbaImage;

const W: u32 = 160;
const H: u32 = 96;
const FOG_AT: DVec3 = DVec3::new(2.0, 2.0, 0.0);

/// A dark world with a glowing box of fog to the camera's right, 12 m in front of it.
fn app() -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.add_plugins(RenderPlugin);
    app.add_message::<WorldShift>();
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.tonemapper = Tonemapper::None;
        s.bloom_intensity = 0.0;
        s.vignette = 0.0;
        s.film_grain = 0.0;
        s.sharpness = 0.0;
        s.anti_aliasing = AntiAliasing::None;
        s.night_desaturation = 0.0;
    }
    app.world.resource_mut::<FogSettings>().density = 0.0;
    app.world.resource_mut::<Atmosphere>().enabled = false;
    app.world.resource_mut::<AmbientLight>().brightness = 0.0;
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 0.0 },
            ..Camera::perspective(50.0)
        },
        Transform::from_xyz(0.0, 2.0, 12.0).looking_at(DVec3::new(0.0, 2.0, 0.0), Vec3::Y),
    ));
    w.spawn((
        Transform::from_translation(FOG_AT),
        FogVolume::new([1.5, 1.5, 1.5])
            .with_density(0.05)
            .with_emission([0.2, 0.2, 0.2])
            .with_edge_falloff(0.1),
    ));
    app
}

fn capture(app: &mut App, name: &str) -> RgbaImage {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    image::open(&path).expect("png").to_rgba8()
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

/// Mean linear luma of the 5x5 pixels around `(x, y)`.
fn luma_px(img: &RgbaImage, (x, y): (i32, i32)) -> f32 {
    let mut sum = 0.0;
    for dy in -2..=2 {
        for dx in -2..=2 {
            let p = img.get_pixel(
                (x + dx).clamp(0, W as i32 - 1) as u32,
                (y + dy).clamp(0, H as i32 - 1) as u32,
            );
            sum += (0.2126 * linear(p[0]) + 0.7152 * linear(p[1]) + 0.0722 * linear(p[2])) / 25.0;
        }
    }
    sum
}

fn history_cut(app: &App) -> f64 {
    app.world
        .resource::<Diagnostics>()
        .get("render.history_cut")
        .unwrap_or(-1.0)
}

/// Every root transform moves with the shift, as `engine_world` moves them, and the message
/// goes out.
fn shift_world(app: &mut App, shift: WorldShift) {
    let mut roots = app.world.query::<&mut Transform>();
    for mut t in roots.iter_mut(&mut app.world) {
        *t = shift.transform(&t);
    }
    app.world.write_message(shift);
}

fn camera(app: &mut App) -> Entity {
    app.world
        .query_filtered::<Entity, With<Camera>>()
        .single(&app.world)
        .expect("camera")
}

/// Converged fog (24 frames), or `None` without a GPU.
fn converged(name: &str) -> Option<(App, f32, (i32, i32))> {
    let mut app = app();
    app.update_n(24);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let img = capture(&mut app, &format!("shift_{name}_before"));
    let at = screen(&app, FOG_AT);
    let lit = luma_px(&img, at);
    assert!(
        lit > 0.05,
        "{name}: no glowing fog before the shift ({lit})"
    );
    assert_eq!(history_cut(&app), 0.0, "{name}: a still camera cut");
    Some((app, lit, at))
}

#[test]
fn a_trip_to_another_place_drops_the_old_places_fog() {
    let Some((mut app, lit, at)) = converged("trip") else {
        return;
    };
    // The world re-anchors 300 km away; the camera is put back where it stood.
    let cam = camera(&mut app);
    let kept = *app.world.get::<Transform>(cam).unwrap();
    shift_world(
        &mut app,
        WorldShift::translate(DVec3::new(250_000.0, -40_000.0, -160_000.0)),
    );
    *app.world.get_mut::<Transform>(cam).unwrap() = kept;
    app.update();
    // Read before the capture, which renders a frame of its own.
    let cut = history_cut(&app);
    let img = capture(&mut app, "shift_trip_after");
    let left = luma_px(&img, at);
    eprintln!("trip: fog {lit:.4} before, {left:.4} where it was after (cut {cut})");
    assert_eq!(cut, 1.0, "a trip kept the old place's histories");
    assert!(
        left < lit * 0.02,
        "the old place's fog still shows after the trip: {left:.4} of {lit:.4}"
    );
}

#[test]
fn a_rebase_keeps_the_histories() {
    let turn = DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2);
    let shifts = [
        (
            "move",
            WorldShift::translate(DVec3::new(3000.0, 0.0, -1700.0)),
        ),
        (
            "turn",
            WorldShift {
                rotation: turn,
                ..WorldShift::translate(DVec3::new(-850.0, 0.0, 420.0))
            },
        ),
    ];
    for (name, shift) in shifts {
        let Some((mut app, lit, at)) = converged(name) else {
            return;
        };
        shift_world(&mut app, shift);
        app.update();
        assert_eq!(
            history_cut(&app),
            0.0,
            "{name}: a rebase that carried the camera along cut its histories"
        );
        app.update_n(3);
        let img = capture(&mut app, &format!("shift_{name}_after"));
        // Same picture: the fog where it was on screen, as bright.
        let moved = screen(&app, shift.point(FOG_AT));
        let after = luma_px(&img, at);
        eprintln!("{name}: fog {lit:.4} before, {after:.4} after, at {at:?} / {moved:?}");
        assert!(
            (moved.0 - at.0).abs() <= 1 && (moved.1 - at.1).abs() <= 1,
            "{name}: the fog moved on screen from {at:?} to {moved:?}"
        );
        assert!(
            (after - lit).abs() < lit * 0.1,
            "{name}: the fog changed from {lit:.4} to {after:.4}"
        );
    }
}
