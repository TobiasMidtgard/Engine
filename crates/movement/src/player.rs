//! Player input → [`MoveIntent`], so a character with [`PlayerControlled`] is playable with
//! the default action map (`move`, `jump`, `sprint`, `crouch`, `dash`, `attack`, `block`).
//! Movement is relative to the active camera.

use engine_core::prelude::*;
use engine_input::ActionState;
use engine_render::Camera;
use engine_world::AttackKind;
use engine_world::locomotion::MoveIntent;

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
pub(crate) fn player_intent(
    actions: Option<Res<ActionState>>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    mut q: Query<(
        &Transform,
        &Mover,
        &MovementProfile,
        &PlayerControlled,
        &mut MoveIntent,
    )>,
) {
    let Some(actions) = actions else {
        return;
    };
    let camera = cameras
        .iter()
        .filter(|(c, _)| c.active)
        .max_by_key(|(c, _)| c.order)
        .map(|(_, t)| t.forward());
    for (t, m, p, pc, mut intent) in &mut q {
        let up = m.up;
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
        intent.look = Some(fwd);
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
