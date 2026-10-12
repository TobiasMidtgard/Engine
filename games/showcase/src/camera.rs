//! Free camera that flies from walking speed on the ground to hundreds of km/s in orbit.
//!
//! - Hold right mouse to look, WASD to move, Space/Ctrl (`ascend`) for up/down, Shift to sprint.
//! - Mouse wheel scales speed. Speed also grows with altitude, so orbit is reachable.
//! - 1–9 jump to the zones, 0 to the overview, O to orbit, M to the Moon.
//! - `[` and `]` change the hour, `T` starts or stops the clock.

use crate::day_cycle::DayCycle;
use crate::places::{self, Place};
use engine::prelude::*;

#[derive(Component, Debug, Clone)]
pub struct ShowcaseCamera {
    pub yaw: f32,
    pub pitch: f32,
    /// Multiplier from the mouse wheel.
    pub speed_scale: f32,
}

pub fn spawn_camera(commands: &mut Commands) {
    let mut cam = ShowcaseCamera { yaw: 0.0, pitch: 0.0, speed_scale: 1.0 };
    let mut t = Transform::default();
    let start = places::find("plaza").expect("plaza");
    look(&mut t, &mut cam, start.position, start.target);
    commands.spawn((
        Camera {
            projection: Projection::Perspective { fov_y_deg: 55.0, near: 0.1 },
            ..Default::default()
        },
        cam,
        t,
    ));
}

/// Place the camera at `position` looking at `target`.
pub fn look(t: &mut Transform, cam: &mut ShowcaseCamera, position: DVec3, target: DVec3) {
    let d = (target - position).normalize().as_vec3();
    cam.yaw = (-d.x).atan2(-d.z);
    cam.pitch = d.y.clamp(-1.0, 1.0).asin();
    t.translation = position;
    t.rotation = Quat::from_rotation_y(cam.yaw) * Quat::from_rotation_x(cam.pitch);
}

pub fn go_to(world: &mut World, place: &Place) {
    let mut q = world.query::<(&mut Transform, &mut ShowcaseCamera)>();
    for (mut t, mut cam) in q.iter_mut(world) {
        look(&mut t, &mut cam, place.position, place.target);
    }
}

/// Height above the planet surface (the ground plane is at y = 0, planet center below it).
fn altitude(world_pos: DVec3, atmosphere: &Atmosphere) -> f64 {
    (world_pos - atmosphere.planet_center).length() - atmosphere.planet_radius
}

#[allow(clippy::too_many_arguments)]
pub fn fly(
    actions: Res<ActionState>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    mouse: Res<Mouse>,
    time: Res<Time>,
    atmosphere: Res<Atmosphere>,
    mut cycle: ResMut<DayCycle>,
    mut window: ResMut<Window>,
    mut q: Query<(&mut Transform, &mut ShowcaseCamera)>,
) {
    if mouse_buttons.just_pressed(MouseButton::Right) {
        window.cursor_mode = CursorMode::Locked;
    }
    if mouse_buttons.just_released(MouseButton::Right) {
        window.cursor_mode = CursorMode::Normal;
    }
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
    .find(|(k, _)| keys.just_pressed(*k))
    .and_then(|(_, key)| places::find(key));

    if keys.just_pressed(KeyCode::BracketLeft) {
        cycle.hour = (cycle.hour - 1.0).rem_euclid(24.0);
    }
    if keys.just_pressed(KeyCode::BracketRight) {
        cycle.hour = (cycle.hour + 1.0).rem_euclid(24.0);
    }
    if keys.just_pressed(KeyCode::T) {
        cycle.hours_per_second = if cycle.hours_per_second == 0.0 { 0.25 } else { 0.0 };
    }

    let dt = time.real_delta_secs();
    for (mut t, mut cam) in &mut q {
        if let Some(place) = &bookmark {
            look(&mut t, &mut cam, place.position, place.target);
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
        t.rotation = Quat::from_rotation_y(cam.yaw) * Quat::from_rotation_x(cam.pitch);

        let m = actions.axis("move");
        let dir = t.forward() * m.y + t.right() * m.x + Vec3::Y * actions.value("ascend");
        let h = altitude(t.translation, &atmosphere).max(0.0);
        let base = (6.0 + h * 0.5).min(2.0e6);
        let sprint = if actions.pressed("sprint") { 6.0 } else { 1.0 };
        let speed = base * sprint * cam.speed_scale as f64;
        t.translation += dir.as_dvec3() * speed * dt as f64;
    }
}
