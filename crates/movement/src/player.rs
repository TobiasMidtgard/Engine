//! Player input → [`MoveIntent`], so a character with [`PlayerControlled`] is playable with
//! the default action map (`move`, `jump`, `sprint`, `crouch`, `dash`, `attack`, `block`, plus
//! `lock_on`, `pilot` and `grapple`, which the module binds if the game hasn't). Movement is
//! relative to the active camera, and the grapple fires at what the camera looks at.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_input::{ActionState, Binding, GamepadButton, InputMap, KeyCode, MouseButton};
use engine_render::Camera;
use engine_world::AttackKind;
use engine_world::locomotion::MoveIntent;
use engine_world::physics::{PhysicsQueries, QueryFilter};

use crate::lockon::LockOn;
use crate::mover::{Mover, flat};
use crate::profile::MovementProfile;

/// Drive this character from the player's input actions.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerControlled {
    /// Hold `sprint` to sprint (false = always sprint when moving).
    pub hold_to_sprint: bool,
}

/// Fill `MoveIntent` from actions (FixedUpdate, `SimSet::Input`).
#[allow(clippy::type_complexity)]
pub(crate) fn player_intent(
    actions: Option<Res<ActionState>>,
    queries: Res<PhysicsQueries>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    bodies: Query<&Transform>,
    mut q: Query<(
        Entity,
        &Transform,
        &mut Mover,
        &MovementProfile,
        &PlayerControlled,
        &mut MoveIntent,
        Option<&LockOn>,
    )>,
) {
    let Some(actions) = actions else {
        return;
    };
    let view = cameras
        .iter()
        .filter(|(c, _)| c.active)
        .max_by_key(|(c, _)| c.order)
        .map(|(_, t)| (t.translation(), t.forward()));
    let camera = view.map(|(_, f)| f);
    for (entity, t, mut m, p, pc, mut intent, lock) in &mut q {
        let up = m.up;
        if actions.pressed_since_tick("grapple") {
            m.pending_grapple = Some(grapple_aim(&queries, entity, t, &m, p, view));
        }
        let fwd = camera
            .map(|f| flat(f, up))
            .and_then(|f| f.try_normalize())
            .unwrap_or(m.facing);
        let right = fwd.cross(up);
        let axis = actions.axis("move");
        let mut dir = right * axis.x + fwd * axis.y;
        if dir.length() > 1.0 {
            dir = dir.normalize();
        }
        let tap = |a: &str| actions.pressed(a) || actions.pressed_since_tick(a);
        intent.direction = dir;
        // Locked on: face the enemy, so the stick strafes around it.
        let at_enemy = lock
            .and_then(|l| l.target)
            .and_then(|e| bodies.get(e).ok())
            .and_then(|e| flat((e.translation - t.translation).as_vec3(), up).try_normalize());
        intent.look = Some(at_enemy.unwrap_or(fwd));
        intent.jump = tap("jump");
        intent.sprint = if pc.hold_to_sprint {
            actions.pressed("sprint")
        } else {
            true
        };
        intent.crouch = actions.pressed("crouch");
        intent.block = actions.pressed("block");
        intent.dash_to = actions.pressed_since_tick("dash").then(|| {
            let d = dir.try_normalize().unwrap_or(m.facing);
            t.translation + (d * p.dash_distance).as_dvec3()
        });
        intent.attack = actions
            .pressed_since_tick("attack")
            .then_some(AttackKind::Melee);
    }
}

/// Where a grapple shot goes: what the camera looks at (seen past the character), or straight
/// ahead and up without a camera.
fn grapple_aim(
    queries: &PhysicsQueries,
    me: Entity,
    t: &Transform,
    m: &Mover,
    p: &MovementProfile,
    view: Option<(DVec3, Vec3)>,
) -> DVec3 {
    let Some((eye, fwd)) = view else {
        let dir = (m.facing + m.up * 0.6).normalize_or(m.facing);
        return t.translation + (dir * p.grapple_range).as_dvec3();
    };
    let reach = p.grapple_range + (t.translation - eye).length() as f32;
    queries
        .0
        .raycast(eye, fwd, reach, &QueryFilter::excluding(me))
        .map_or(eye + (fwd * reach).as_dvec3(), |h| h.point)
}

/// Bind `lock_on` (middle mouse, right stick click), `pilot` (P, d-pad down) and `grapple`
/// (F, left bumper) unless the game's action map already has them (`Startup`).
pub(crate) fn bind_actions(map: Option<ResMut<InputMap>>) {
    let Some(mut map) = map else {
        return;
    };
    if !map.actions.contains_key("lock_on") {
        map.bind("lock_on", Binding::Mouse(MouseButton::Middle))
            .bind("lock_on", Binding::Gamepad(GamepadButton::RightStick));
    }
    if !map.actions.contains_key("pilot") {
        map.bind("pilot", Binding::Key(KeyCode::P))
            .bind("pilot", Binding::Gamepad(GamepadButton::DPadDown));
    }
    if !map.actions.contains_key("grapple") {
        map.bind("grapple", Binding::Key(KeyCode::F))
            .bind("grapple", Binding::Gamepad(GamepadButton::LeftBumper));
    }
}
