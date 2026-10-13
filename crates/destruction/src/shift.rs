//! Following a [`WorldShift`] (a floating-origin move or a new reference frame). The shared
//! contracts already move root transforms and physics velocities; this moves the world-space
//! state destruction keeps itself: the built-in solver's debris motion, dust, debris queued for
//! the next step, scripted builders' aim and the demo scripts' anchors.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_world::WorldShift;

use crate::build::Builder;
use crate::debris::{DebrisBody, DustPuff, PendingDebris};
use crate::demo::DestructionDemo;
use crate::hill::HillDemo;

/// Shift destruction's own world-space state (PostUpdate, `WorldShiftSet::Apply`).
pub fn shift_system(
    mut shifts: MessageReader<WorldShift>,
    mut bodies: Query<(&mut DebrisBody, Option<&GlobalTransform>)>,
    mut puffs: Query<(&mut DustPuff, Option<&GlobalTransform>)>,
    mut pending: ResMut<PendingDebris>,
    mut demos: Query<&mut DestructionDemo>,
    mut hills: Query<&mut HillDemo>,
    mut builders: Query<&mut Builder>,
) {
    let Some(s) = shifts.read().copied().reduce(|a, b| a.then(&b)) else {
        return;
    };
    // `GlobalTransform` still holds the old frame until transforms propagate after this set.
    let at = |g: Option<&GlobalTransform>| s.point(g.map_or(DVec3::ZERO, |g| g.translation()));
    for (mut b, g) in &mut bodies {
        // Resting debris stays at rest on whatever it lies on, which moved with it.
        if b.asleep || b.sinking {
            continue;
        }
        let p = at(g);
        b.velocity = s.velocity_at(p, b.velocity);
        b.angular = s.spin(b.angular);
    }
    for (mut d, g) in &mut puffs {
        let p = at(g);
        d.velocity = s.velocity_at(p, d.velocity);
    }
    for item in &mut pending.items {
        item.position = s.point(item.position);
        item.rotation = s.orientation(item.rotation);
        item.velocity = s.velocity_at(item.position, item.velocity);
        item.angular = s.spin(item.angular);
    }
    for d in &mut pending.dust {
        d.position = s.point(d.position);
        d.velocity = s.velocity_at(d.position, d.velocity);
    }
    for mut d in &mut demos {
        d.origin = s.point(d.origin);
    }
    for mut h in &mut hills {
        h.origin = s.point(h.origin);
    }
    for mut b in &mut builders {
        if let Some((o, d)) = b.aim {
            let o = s.point(DVec3::from_array(o)).to_array();
            let d = s.direction(Vec3::from_array(d)).to_array();
            b.aim = Some((o, d));
        }
    }
}
