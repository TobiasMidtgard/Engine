//! World shifts (floating origin, a new planet's frame): engine_world moves the shared
//! contracts (root transforms, velocities, contact messages); this moves what animation keeps
//! in world space the same way, in `WorldShiftSet::Apply`: planted feet, spring and jiggle
//! history, wheel contacts, look and reach targets, and demo paths.

use engine_core::prelude::*;
use engine_world::WorldShift;

use crate::demo::DemoPath;
use crate::ik::{LookAt, ReachIk};
use crate::muscle::MuscleRig;
use crate::procedural::gait::LeggedGait;
use crate::procedural::spring::SpringChains;
use crate::procedural::vehicle::VehicleRig;

#[allow(clippy::too_many_arguments)]
pub(crate) fn follow_world_shifts(
    mut shifts: MessageReader<WorldShift>,
    mut springs: Query<&mut SpringChains>,
    mut muscles: Query<&mut MuscleRig>,
    mut gaits: Query<&mut LeggedGait>,
    mut vehicles: Query<&mut VehicleRig>,
    mut looks: Query<&mut LookAt>,
    mut reaches: Query<&mut ReachIk>,
    mut paths: Query<&mut DemoPath>,
) {
    let Some(s) = shifts.read().copied().reduce(|acc, next| acc.then(&next)) else {
        return;
    };
    for mut c in &mut springs {
        c.follow_shift(&s);
    }
    for mut m in &mut muscles {
        m.follow_shift(&s);
    }
    for mut g in &mut gaits {
        g.rt.follow_shift(&s);
    }
    for mut v in &mut vehicles {
        v.rt.follow_shift(&s);
    }
    for mut l in &mut looks {
        l.target = s.point(l.target);
    }
    for mut r in &mut reaches {
        for c in &mut r.chains {
            c.target = s.point(c.target);
        }
    }
    // A demo loop moves with the ground under it (keeping its compass heading).
    for mut p in &mut paths {
        p.origin = s.point(p.origin);
    }
}
