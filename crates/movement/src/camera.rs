//! [`PilotCamera`]: a third-person camera over a character's shoulder, for playing the
//! movement and combat by hand.
//!
//! Mouse or right stick look around. The boom pulls in when a wall or the ground is behind
//! the character, so the view never ends up inside geometry. While the character is locked on
//! ([`LockOn`]), the camera turns to keep both fighters in view. It works in any gravity: the
//! view's up follows the character's up (planets).

use engine_core::math::{DVec3, Quat, Vec2, Vec3};
use engine_core::prelude::*;
use engine_core::time::FixedTime;
use engine_core::transform::TransformInterpolation;
use engine_core::window::{CursorMode, Window};
use engine_input::{GamepadAxis, Gamepads, Mouse};
use engine_render::Camera;
use engine_world::Ground;
use engine_world::physics::{ColliderShape, PhysicsQueries, QueryFilter};

use crate::fx::CameraFx;
use crate::lockon::LockOn;
use crate::mover::Mover;
use crate::player::PlayerControlled;

/// A third-person camera following a character. Put it on an entity with a [`Camera`].
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PilotCamera {
    /// The character to follow (`None` = the first [`PlayerControlled`] one).
    #[serde(skip)]
    pub target: Option<Entity>,
    /// Look angles, radians: yaw about the character's up (0 = toward −Z on flat ground),
    /// pitch up from level.
    pub yaw: f32,
    pub pitch: f32,
    /// Boom length behind the pivot, m.
    pub distance: f32,
    /// Pivot above the character's center, m.
    pub height: f32,
    /// Pivot to the right of the character (over the shoulder), m.
    pub shoulder: f32,
    /// Mouse look, radians per pixel.
    pub mouse_sensitivity: f32,
    /// Stick look at full tilt, radians per second.
    pub stick_speed: f32,
    pub min_pitch: f32,
    pub max_pitch: f32,
    /// How quickly the pivot catches up with the character, 1/s (lower = more lag).
    pub follow_rate: f32,
    /// How quickly the view turns toward a locked-on target, 1/s.
    pub lock_rate: f32,
    #[serde(skip)]
    state: Option<Rig>,
}

impl Default for PilotCamera {
    fn default() -> Self {
        Self {
            target: None,
            yaw: 0.0,
            pitch: -0.2,
            distance: 3.4,
            height: 0.6,
            shoulder: 0.5,
            mouse_sensitivity: 0.0025,
            stick_speed: 3.2,
            min_pitch: -1.2,
            max_pitch: 1.0,
            follow_rate: 14.0,
            lock_rate: 7.0,
            state: None,
        }
    }
}

impl PilotCamera {
    pub fn following(target: Entity) -> Self {
        Self {
            target: Some(target),
            ..Default::default()
        }
    }
}

/// Smoothed state, frame to frame.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Rig {
    /// Rotates +Y to the character's up; carried along as up changes.
    frame: Quat,
    pivot: DVec3,
    /// Current boom length (shorter than `distance` near walls).
    boom: f32,
}

/// Yaw and pitch of a direction in `frame`.
fn angles(frame: Quat, dir: Vec3) -> (f32, f32) {
    let d = (frame.inverse() * dir).normalize_or(Vec3::NEG_Z);
    ((-d.x).atan2(-d.z), d.y.clamp(-1.0, 1.0).asin())
}

/// `angle` moved toward `to` by the shortest way, by `t` (0..1) of the gap.
fn approach_angle(angle: f32, to: f32, t: f32) -> f32 {
    let gap = (to - angle + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
        - std::f32::consts::PI;
    angle + gap * t
}

/// Where a character is drawn this frame (between its last two fixed steps).
fn visual(t: &Transform, interp: Option<&TransformInterpolation>, alpha: f32) -> DVec3 {
    interp
        .and_then(|i| i.previous)
        .map_or(t.translation, |p| p.lerp(t, alpha).translation)
}

/// Move pilot cameras after gameplay (`Update`, [`UpdateSet::Camera`]).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn follow_pilots(
    time: Res<Time>,
    fixed: Res<FixedTime>,
    mouse: Option<Res<Mouse>>,
    pads: Option<Res<Gamepads>>,
    queries: Res<PhysicsQueries>,
    ground: Res<Ground>,
    window: Option<Res<Window>>,
    players: Query<Entity, (With<PlayerControlled>, With<Mover>)>,
    characters: Query<
        (
            &Transform,
            Option<&TransformInterpolation>,
            &Mover,
            Option<&LockOn>,
        ),
        Without<PilotCamera>,
    >,
    bodies: Query<(&Transform, Option<&TransformInterpolation>), Without<PilotCamera>>,
    mut cams: Query<(&mut PilotCamera, &Camera, &mut Transform, Option<&CameraFx>)>,
) {
    let dt = time.real_delta_secs().min(0.1);
    let alpha = fixed.alpha();
    // The mouse looks around only while the cursor is captured (in play mode).
    let captured = window.is_some_and(|w| w.cursor_mode == CursorMode::Locked);
    let look_mouse = mouse.filter(|_| captured).map_or(Vec2::ZERO, |m| m.delta);
    let look_stick = pads.map_or(Vec2::ZERO, |p| {
        Vec2::new(
            p.any_axis(GamepadAxis::RightStickX),
            p.any_axis(GamepadAxis::RightStickY),
        )
    });
    for (mut pc, cam, mut transform, fx) in &mut cams {
        if !cam.active {
            continue;
        }
        let Some(target) = pc.target.or_else(|| players.iter().next()) else {
            continue;
        };
        let Ok((t, interp, m, lock)) = characters.get(target) else {
            continue;
        };
        let pos = visual(t, interp, alpha);
        let up = m.up;
        let fresh = pc.state.is_none();
        let mut rig = pc.state.unwrap_or_else(|| {
            let frame = Quat::from_rotation_arc(Vec3::Y, up);
            Rig {
                frame,
                pivot: pos,
                boom: pc.distance,
            }
        });
        rig.frame = (Quat::from_rotation_arc(rig.frame * Vec3::Y, up) * rig.frame).normalize();
        if fresh {
            (pc.yaw, _) = angles(rig.frame, m.facing);
        }

        // Look: the player's input, or the lock-on target.
        let locked = lock
            .and_then(|l| l.target)
            .and_then(|e| bodies.get(e).ok())
            .map(|(t, i)| visual(t, i, alpha));
        match locked {
            Some(enemy) => {
                let to = (enemy - rig.pivot).as_vec3();
                let (yaw, pitch) = angles(rig.frame, to);
                let k = 1.0 - (-pc.lock_rate * dt).exp();
                pc.yaw = approach_angle(pc.yaw, yaw, k);
                // Look a little down at the fight.
                let want = (pitch - 0.12).clamp(pc.min_pitch, pc.max_pitch);
                pc.pitch += (want - pc.pitch) * k;
            }
            None => {
                pc.yaw -= look_mouse.x * pc.mouse_sensitivity + look_stick.x * pc.stick_speed * dt;
                pc.pitch -=
                    look_mouse.y * pc.mouse_sensitivity - look_stick.y * pc.stick_speed * dt;
            }
        }
        pc.pitch = pc.pitch.clamp(pc.min_pitch, pc.max_pitch);
        let rotation = rig.frame * Quat::from_rotation_y(pc.yaw) * Quat::from_rotation_x(pc.pitch);
        let right = rotation * Vec3::X;
        let back = rotation * Vec3::Z;

        // Pivot: follows the character with a little lag (more along up, so landings and
        // steps don't jolt the view).
        let want = pos + (up * pc.height + right * pc.shoulder).as_dvec3();
        if fresh {
            rig.pivot = want;
        } else {
            let k = 1.0 - (-pc.follow_rate * dt).exp();
            let kv = 1.0 - (-pc.follow_rate * 0.6 * dt).exp();
            let gap = want - rig.pivot;
            let along = up.as_dvec3() * gap.dot(up.as_dvec3());
            rig.pivot += (gap - along) * k as f64 + along * kv as f64;
        }

        // Boom: pull in in front of anything behind the character, ease back out.
        let mut filter = QueryFilter::excluding(target);
        if let Some(e) = lock.and_then(|l| l.target) {
            filter.exclude.push(e);
        }
        let probe = ColliderShape::Sphere { radius: 0.25 };
        let room = queries
            .0
            .shapecast(
                &probe,
                rig.pivot,
                Quat::IDENTITY,
                back,
                pc.distance,
                &filter,
            )
            .map_or(pc.distance, |h| h.distance);
        rig.boom = if room < rig.boom || fresh {
            room
        } else {
            rig.boom + (room - rig.boom) * (1.0 - (-3.0 * dt).exp())
        };
        let mut eye = rig.pivot + (back * rig.boom).as_dvec3();
        // Terrain without colliders: keep the eye above the ground surface.
        if up.y > 0.9
            && let Some(g) = ground.height(DVec3::new(eye.x, rig.pivot.y, eye.z))
            && eye.y < g + 0.3
        {
            eye.y = g + 0.3;
        }

        transform.translation = eye;
        // Keep the roll camera cues add on top (they apply their change each frame).
        let roll = fx.map_or(0.0, |f| f.roll_deg.to_radians());
        transform.rotation = rotation * Quat::from_rotation_z(roll);
        pc.state = Some(rig);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angles_round_trip_through_a_tilted_frame() {
        let frame = Quat::from_rotation_arc(Vec3::Y, Vec3::new(0.3, 0.9, -0.2).normalize());
        for (yaw, pitch) in [(0.0, 0.0), (1.2, -0.4), (-2.5, 0.7)] {
            let dir =
                frame * Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch) * Vec3::NEG_Z;
            let (y, p) = angles(frame, dir);
            assert!(
                (y - yaw).abs() < 1e-4 && (p - pitch).abs() < 1e-4,
                "{y} {p}"
            );
        }
    }

    #[test]
    fn angles_take_the_short_way_round() {
        let a = approach_angle(3.0, -3.0, 0.5);
        assert!(a > 3.0 && a < 3.0 + 0.2, "{a}");
        assert!((approach_angle(0.0, 1.0, 1.0) - 1.0).abs() < 1e-6);
    }
}
