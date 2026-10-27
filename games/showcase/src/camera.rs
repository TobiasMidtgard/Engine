//! Free camera that flies from walking speed on the ground to hundreds of km/s in orbit.
//!
//! - Hold right mouse to look, WASD to move, Space/Ctrl (`ascend`) for up/down, Shift to sprint.
//! - Mouse wheel scales speed. Speed also grows with altitude, so orbit is reachable.
//! - 1–9 jump to the zones, 0 to the overview, O to orbit, M to the Moon, N to Mars.
//! - Down low on any planet or moon the camera keeps level with its ground (up is away from
//!   its center), flies at a speed set by the height over it and never goes underground;
//!   the space module re-anchors the world under it as it lands, wherever that is.
//! - `[` and `]` change the hour, `T` starts or stops the clock.
//! - While play mode (P) is on, the pilot's camera has the controls; this one picks up from
//!   there when play ends.

use crate::places::{self, Place};
use engine::prelude::*;

#[derive(Component, Debug, Clone)]
pub struct ShowcaseCamera {
    pub yaw: f32,
    pub pitch: f32,
    /// Multiplier from the mouse wheel.
    pub speed_scale: f32,
    /// The camera's level frame: turns world +Y to the local up (yaw is about it, pitch from
    /// it). Follows the ground down low and turns with the world when it shifts.
    pub level: Quat,
    /// A trip this camera asked for: its world shift is already in the new pose.
    pub tripping: bool,
}

pub fn spawn_camera(commands: &mut Commands) {
    let mut cam = ShowcaseCamera {
        yaw: 0.0,
        pitch: 0.0,
        speed_scale: 1.0,
        level: Quat::IDENTITY,
        tripping: false,
    };
    let mut t = Transform::default();
    let start = places::find("plaza").expect("plaza");
    look(&mut t, &mut cam, start.position, start.target);
    commands.spawn((
        Camera {
            projection: Projection::Perspective {
                fov_y_deg: 55.0,
                near: 0.1,
            },
            ..Default::default()
        },
        cam,
        t,
        // Water, fire, smoke and weather grids follow the camera around the valley.
        engine::world::SimulationFocus::default(),
    ));
}

/// Place the camera at `position` looking at `target`, level with world +Y.
pub fn look(t: &mut Transform, cam: &mut ShowcaseCamera, position: DVec3, target: DVec3) {
    look_level(t, cam, position, target, Quat::IDENTITY);
}

/// Place the camera at `position` looking at `target`, level in the frame `level` (see
/// [`ShowcaseCamera::level`]).
fn look_level(
    t: &mut Transform,
    cam: &mut ShowcaseCamera,
    position: DVec3,
    target: DVec3,
    level: Quat,
) {
    let d = level.inverse() * (target - position).normalize().as_vec3();
    cam.yaw = (-d.x).atan2(-d.z);
    cam.pitch = d.y.clamp(-1.0, 1.0).asin();
    cam.level = level;
    t.translation = position;
    t.rotation = level * Quat::from_rotation_y(cam.yaw) * Quat::from_rotation_x(cam.pitch);
}

/// Jump to a place (laid out in the valley's home coordinates). Away from home (on the Moon,
/// or landed far off), the world goes home first.
pub fn go_to(world: &mut World, place: &Place) {
    let place = crate::planet::aim(world, place);
    let tripping = engine_space::travel_home_to(world, place.position);
    let mut q = world.query::<(&mut Transform, &mut ShowcaseCamera)>();
    for (mut t, mut cam) in q.iter_mut(world) {
        look(&mut t, &mut cam, place.position, place.target);
        cam.tripping |= tripping;
    }
}

/// The body that pulls hardest at `p`: its center and radius.
fn nearest_body(gravity: &Gravity, p: DVec3) -> Option<(DVec3, f64)> {
    gravity
        .bodies
        .iter()
        .max_by(|a, b| {
            let g = |c: DVec3, mu: f64| mu / (c - p).length_squared().max(1.0);
            g(a.center, a.mu).total_cmp(&g(b.center, b.mu))
        })
        .map(|b| (b.center, b.radius))
}

/// Up at `p` and how much the camera keeps level with it (1 down low, fading out between a
/// quarter and one radius up, 0 out in space where it flies free).
fn up_here(gravity: &Gravity, p: DVec3) -> (Vec3, f32) {
    match nearest_body(gravity, p) {
        Some((center, radius)) => {
            let up = (p - center).normalize_or(DVec3::Y).as_vec3();
            let h = ((p - center).length() - radius) / radius;
            (up, (1.0 - (h - 0.25) / 0.75).clamp(0.0, 1.0) as f32)
        }
        None => (Vec3::Y, 1.0),
    }
}

/// The level frame at `p`, for a camera arriving there.
fn level_at(gravity: &Gravity, p: DVec3) -> Quat {
    let (up, w) = up_here(gravity, p);
    Quat::IDENTITY.slerp(Quat::from_rotation_arc(Vec3::Y, up), w)
}

/// Height above the ground under `p` (the nearest body's radius from far up, its terrain down
/// low while world +Y is up there).
fn altitude(gravity: &Gravity, ground: &engine::world::Ground, level: Quat, p: DVec3) -> f64 {
    let Some((center, radius)) = nearest_body(gravity, p) else {
        return p.y;
    };
    let h = (p - center).length() - radius;
    if h < 20_000.0
        && (level * Vec3::Y).y > 0.999
        && let Some(g) = ground.0.sample(p)
    {
        return (p.y - g.height).min(h.max(0.0));
    }
    h
}

#[allow(clippy::too_many_arguments)]
pub fn fly(
    actions: Res<ActionState>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    mouse: Res<Mouse>,
    time: Res<Time>,
    (gravity, ground, space): (
        Res<Gravity>,
        Res<engine::world::Ground>,
        Option<Res<engine_space::SpaceState>>,
    ),
    (mut shifts, mut commands): (MessageReader<engine::world::WorldShift>, Commands),
    mut sky: ResMut<engine_weather::Sky>,
    mut window: ResMut<Window>,
    mut q: Query<(&mut Transform, &mut ShowcaseCamera)>,
    play: Option<Res<engine_movement::Play>>,
    views: Query<&GlobalTransform, Without<ShowcaseCamera>>,
    mut play_view: Local<Option<(DVec3, Vec3)>>,
    bodies: Query<(&engine_space::CelestialBody, &Transform), Without<ShowcaseCamera>>,
    ui: Option<Res<engine_ui::UiInputCapture>>,
    mut held: Option<ResMut<crate::build::HeldJump>>,
) {
    // The world turned under the camera (re-anchored on landing, or moved home under it):
    // its level frame turns with it, so the view carries on unchanged.
    for shift in shifts.read() {
        for (_, mut cam) in &mut q {
            if cam.tripping {
                cam.tripping = false;
            } else {
                cam.level = (shift.rotation.as_quat() * cam.level).normalize();
            }
        }
    }
    // While someone plays (P), the pilot's camera renders and owns the keys, mouse and cursor.
    // This one waits, then carries on from where the pilot's camera was when play ends.
    if let Some(view) = play.as_ref().and_then(|p| p.camera) {
        if let Ok(g) = views.get(view) {
            *play_view = Some((g.translation(), g.forward()));
        }
        return;
    }
    if let Some((position, forward)) = play_view.take() {
        let level = level_at(&gravity, position);
        for (mut t, mut cam) in &mut q {
            look_level(
                &mut t,
                &mut cam,
                position,
                position + forward.as_dvec3(),
                level,
            );
        }
    }
    if mouse_buttons.just_pressed(MouseButton::Right) {
        window.cursor_mode = CursorMode::Locked;
    }
    if mouse_buttons.just_released(MouseButton::Right) {
        window.cursor_mode = CursorMode::Normal;
    }
    // While a modal UI screen has the keyboard its keys are its own (2 on the quick-select
    // wheel picks a weapon, not the forest bookmark). WASD and look come through ActionState,
    // which the UI clears.
    let ui_has_keys = ui.is_some_and(|c| c.keyboard);
    let pressed = |k: KeyCode| !ui_has_keys && keys.just_pressed(k);
    let bookmark = [
        (KeyCode::Digit0, "overview"),
        (KeyCode::Digit1, "plaza"),
        (KeyCode::Digit2, "forest"),
        (KeyCode::Digit3, "lake"),
        (KeyCode::Digit4, "cave"),
        (KeyCode::Digit5, "fortress"),
        (KeyCode::Digit6, "arena"),
        (KeyCode::Digit7, "field"),
        (KeyCode::Digit8, "track"),
        (KeyCode::Digit9, "spaceport"),
        (KeyCode::O, "orbit"),
        (KeyCode::M, "moon"),
    ]
    .into_iter()
    .find(|(k, _)| pressed(*k))
    .and_then(|(_, key)| places::find(key))
    .map(|p| match bodies.iter().find(|(b, _)| b.name == "Moon") {
        // The Moon moves: aim at it where it is now (in the valley's home coordinates).
        Some((_, moon)) if p.key == "moon" => {
            let moon = space
                .as_ref()
                .map_or(moon.translation, |s| s.to_home(moon.translation));
            crate::planet::at_moon(&p, moon, sky.day_of_year)
        }
        _ => p,
    });
    // N: 2000 km over Mars's afternoon side, with the world anchored on Mars.
    if pressed(KeyCode::N) {
        let lon = space
            .as_ref()
            .and_then(|s| s.subsolar("Mars"))
            .map_or(0.0, |(_, lon)| lon + 50.0);
        commands.queue(move |world: &mut World| {
            let Ok(at) = engine_space::travel_to(world, "Mars", 15.0, lon, 2_000_000.0) else {
                return;
            };
            let mut q = world.query::<(&mut Transform, &mut ShowcaseCamera)>();
            for (mut t, mut cam) in q.iter_mut(world) {
                look(
                    &mut t,
                    &mut cam,
                    at,
                    DVec3::new(0.0, -900_000.0, -2_500_000.0),
                );
                cam.tripping = true;
            }
        });
    }

    if pressed(KeyCode::BracketLeft) {
        sky.hour = (sky.hour - 1.0).rem_euclid(24.0);
    }
    if pressed(KeyCode::BracketRight) {
        sky.hour = (sky.hour + 1.0).rem_euclid(24.0);
    }
    if pressed(KeyCode::T) {
        sky.hours_per_second = if sky.hours_per_second == 0.0 {
            0.25
        } else {
            0.0
        };
    }

    // The exe covers the view first and moves it a couple of frames later, once the cover
    // has been drawn (`build::cover_jumps`).
    let arrive = match held.as_mut() {
        Some(h) if bookmark.is_some() => {
            let place = bookmark.as_ref().expect("checked");
            h.to = Some((place.position, place.target));
            h.frames = 0;
            None
        }
        Some(h) if h.to.is_some() => {
            h.frames += 1;
            if h.frames > crate::build::COVER_FRAMES {
                h.moved = true;
                h.to.take()
            } else {
                None
            }
        }
        Some(_) => None,
        None => bookmark.as_ref().map(|p| (p.position, p.target)),
    };

    // Places are laid out at home: away from it, the world goes home with the jump.
    let away = space.as_ref().is_some_and(|s| !s.at_home);
    if let Some((position, _)) = arrive
        && away
    {
        commands.queue(move |world: &mut World| {
            engine_space::travel_home_to(world, position);
        });
    }

    let dt = time.real_delta_secs();
    for (mut t, mut cam) in &mut q {
        if let Some((position, target)) = arrive {
            let level = if away {
                Quat::IDENTITY
            } else {
                level_at(&gravity, position)
            };
            look_level(&mut t, &mut cam, position, target, level);
            cam.tripping = away;
            continue;
        }
        if mouse.scroll.y != 0.0 {
            cam.speed_scale = (cam.speed_scale * 1.25f32.powf(mouse.scroll.y)).clamp(0.01, 1000.0);
        }
        if window.cursor_mode == CursorMode::Locked {
            let look = actions.axis("look");
            cam.yaw -= look.x * 0.003;
            cam.pitch = (cam.pitch + look.y * 0.003).clamp(-1.55, 1.55);
        }
        // Down low the level frame follows the ground's up; out in space it stays as it is.
        let (up, w) = up_here(&gravity, t.translation);
        if w > 0.0 {
            let now = cam.level * Vec3::Y;
            let follow = 1.0 - (1.0 - w).powf(dt * 60.0);
            let turn = Quat::IDENTITY.slerp(Quat::from_rotation_arc(now, up), follow);
            cam.level = (turn * cam.level).normalize();
        }
        t.rotation = cam.level * Quat::from_rotation_y(cam.yaw) * Quat::from_rotation_x(cam.pitch);

        let m = actions.axis("move");
        let up = cam.level * Vec3::Y;
        let dir = t.forward() * m.y + t.right() * m.x + up * actions.value("ascend");
        let h = altitude(&gravity, &ground, cam.level, t.translation).max(0.0);
        let base = (6.0 + h * 0.5).min(2.0e6);
        let sprint = if actions.pressed("sprint") { 6.0 } else { 1.0 };
        let speed = base * sprint * cam.speed_scale as f64;
        let before = t.translation;
        t.translation += dir.as_dvec3() * speed * dt as f64;
        // Land on the ground, never through it (the first ground below where it was, so caves
        // and overhangs keep their roofs).
        let probe = DVec3::new(
            t.translation.x,
            before.y.max(t.translation.y) + 0.5,
            t.translation.z,
        );
        if up.y > 0.999
            && let Some(g) = ground.0.sample(probe)
        {
            t.translation.y = t.translation.y.max(g.height + 1.2);
        }
    }
}
