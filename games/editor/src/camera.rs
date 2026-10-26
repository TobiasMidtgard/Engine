//! The scene view camera, with Unreal-style controls:
//!
//! * Right mouse held: look around, WASD to fly, Q/E down/up, wheel changes fly speed.
//! * Middle mouse drag: pan. Wheel: dolly toward the view center.
//! * Alt + left drag: orbit around the pivot (the last framed point).
//! * F: frame the selection.
//!
//! It runs on real time, so it works while the world is paused in Edit mode.

use engine::input::{ButtonInput, KeyCode, Mouse, MouseButton};
use engine::prelude::*;

use crate::editor::Editor;

/// Editor camera state: angles, fly speed (m/s) and the orbit pivot.
#[derive(Component, Debug, Clone)]
pub struct EditorCamera {
    pub yaw: f32,
    pub pitch: f32,
    pub speed: f32,
    pub pivot_distance: f64,
    /// The mouse button whose drag started over the scene view (owns the camera until release).
    drag: Option<MouseButton>,
    /// Smooth framing: target position and pivot distance.
    flight: Option<(DVec3, f64)>,
}

impl EditorCamera {
    /// A camera looking along `forward`.
    pub fn looking(forward: Vec3) -> Self {
        Self {
            yaw: (-forward.x).atan2(-forward.z),
            pitch: forward.y.clamp(-1.0, 1.0).asin(),
            speed: 8.0,
            pivot_distance: 10.0,
            drag: None,
            flight: None,
        }
    }

    fn rotation(&self) -> Quat {
        Quat::from_rotation_y(self.yaw) * Quat::from_rotation_x(self.pitch)
    }

    /// Whether a mouse drag in the scene view is steering the camera.
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }
}

/// Fly, pan, orbit and frame (Update, before the editor UI).
#[allow(clippy::too_many_arguments)]
pub fn editor_camera(
    mut editor: ResMut<Editor>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    mouse: Res<Mouse>,
    time: Res<Time>,
    egui: Res<crate::egui_bridge::Egui>,
    mut window: ResMut<Window>,
    mut q: Query<(&mut Transform, &mut EditorCamera)>,
) {
    let Ok((mut t, mut cam)) = q.get_mut(editor.camera) else {
        return;
    };
    let dt = time.real_delta_secs().min(0.1);
    let typing = egui.ctx.egui_wants_keyboard_input();

    // Who owns the mouse: a drag that started over the scene view keeps it until release.
    if cam.drag.is_none() && editor.viewport_hovered {
        let alt = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);
        if buttons.just_pressed(MouseButton::Right) {
            cam.drag = Some(MouseButton::Right);
        } else if buttons.just_pressed(MouseButton::Middle) {
            cam.drag = Some(MouseButton::Middle);
        } else if alt && buttons.just_pressed(MouseButton::Left) {
            cam.drag = Some(MouseButton::Left);
        }
    }
    if let Some(b) = cam.drag
        && !buttons.pressed(b)
    {
        cam.drag = None;
    }
    let want_lock = cam.drag == Some(MouseButton::Right);
    let mode = if want_lock {
        CursorMode::Locked
    } else {
        CursorMode::Normal
    };
    if window.cursor_mode != mode {
        window.cursor_mode = mode;
    }

    let delta = mouse.delta;
    match cam.drag {
        Some(MouseButton::Right) => {
            cam.flight = None;
            cam.yaw -= delta.x * 0.0035;
            cam.pitch = (cam.pitch - delta.y * 0.0035).clamp(-1.55, 1.55);
            if mouse.scroll.y != 0.0 {
                cam.speed = (cam.speed * 1.2f32.powf(mouse.scroll.y)).clamp(0.2, 5000.0);
            }
            t.rotation = cam.rotation();
            if !typing {
                let axis = |pos: KeyCode, neg: KeyCode| {
                    keys.pressed(pos) as i32 as f32 - keys.pressed(neg) as i32 as f32
                };
                let fast = if keys.pressed(KeyCode::ShiftLeft) {
                    4.0
                } else {
                    1.0
                };
                let mv = t.forward() * axis(KeyCode::W, KeyCode::S)
                    + t.right() * axis(KeyCode::D, KeyCode::A)
                    + Vec3::Y * axis(KeyCode::E, KeyCode::Q);
                t.translation += (mv * cam.speed * fast * dt).as_dvec3();
            }
        }
        Some(MouseButton::Middle) => {
            cam.flight = None;
            let scale = pan_scale(&editor, cam.pivot_distance);
            let pan = t.right() * -delta.x + t.up() * delta.y;
            t.translation += pan.as_dvec3() * scale;
        }
        Some(MouseButton::Left) => {
            cam.flight = None;
            let pivot = t.translation + (t.forward() * cam.pivot_distance as f32).as_dvec3();
            cam.yaw -= delta.x * 0.005;
            cam.pitch = (cam.pitch - delta.y * 0.005).clamp(-1.55, 1.55);
            t.rotation = cam.rotation();
            t.translation = pivot - (t.forward() * cam.pivot_distance as f32).as_dvec3();
        }
        _ => {
            if editor.viewport_hovered && mouse.scroll.y != 0.0 && !egui.ctx.egui_is_using_pointer()
            {
                cam.flight = None;
                let step = (cam.pivot_distance * 0.15).max(0.2) * mouse.scroll.y as f64;
                let step = step.min(cam.pivot_distance - 0.3);
                let fwd = t.forward().as_dvec3();
                t.translation += fwd * step;
                cam.pivot_distance = (cam.pivot_distance - step).max(0.3);
            }
        }
    }

    // Frame the selection: fly so the bounds fill the view.
    if let Some((center, radius)) = editor.focus.take() {
        let fov = 60f64.to_radians();
        let distance = (radius / (fov * 0.5).sin()).max(1.0) * 1.1;
        let target = center - t.forward().as_dvec3() * distance;
        cam.flight = Some((target, distance));
    }
    if let Some((target, distance)) = cam.flight {
        let k = 1.0 - (-12.0 * dt as f64).exp();
        let to_go = target - t.translation;
        t.translation += to_go * k;
        cam.pivot_distance += (distance - cam.pivot_distance) * k;
        if (target - t.translation).length() < 0.005 * distance {
            t.translation = target;
            cam.flight = None;
        }
    }
    editor.frame_secs += (dt - editor.frame_secs) * 0.1;
}

/// Meters per mouse pixel at the pivot distance.
fn pan_scale(editor: &Editor, distance: f64) -> f64 {
    match editor.view {
        Some(v) => v.meters_per_point(distance),
        None => distance * 0.001,
    }
}
