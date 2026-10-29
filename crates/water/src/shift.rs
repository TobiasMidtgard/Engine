//! Following world shifts (a floating-origin move, or a new planet's frame).
//!
//! In `WorldShiftSet::Apply` everything the water module keeps in world space moves into the
//! new frame: body centers and their waves, the ripple grid and its waiting splashes, flow
//! grids, interactors' last positions, tethers and boats' moorings, things the water floats itself, splash drops in flight, and the test
//! area's routes. The `Water` query, buoyancy and the renderer then agree with the rest of the
//! world in the same frame, and nothing reads the jump as motion (no wake from a boat that
//! "moved" 10 km).

use engine_core::math::DVec3;
use engine_core::prelude::*;
use engine_world::WorldShift;

use crate::boat::Boat;
use crate::buoyancy::{Floating, Tether};
use crate::demo::{DemoRole, DemoState};
use crate::interact::WaterInteractor;
use crate::state::SharedWater;

/// Move the water module's world-space state into the frame after this frame's shifts.
#[allow(clippy::too_many_arguments)]
pub fn shift_system(
    mut shifts: MessageReader<WorldShift>,
    shared: Res<SharedWater>,
    mut interactors: Query<&mut WaterInteractor>,
    mut tethers: Query<&mut Tether>,
    mut boats: Query<&mut Boat>,
    mut floating: Query<&mut Floating>,
    mut roles: Query<&mut DemoRole>,
    demo: Option<ResMut<DemoState>>,
    drops: Option<ResMut<crate::drops::SplashDrops>>,
) {
    let Some(s) = shifts.read().copied().reduce(|a, next| a.then(&next)) else {
        return;
    };
    shared.0.write().shift(&s);
    for mut i in &mut interactors {
        i.last = i.last.map(|p| s.point(p));
    }
    for mut t in &mut tethers {
        t.anchor = s.point(t.anchor);
    }
    for mut b in &mut boats {
        if let Some((_, anchor, _)) = &mut b.mooring {
            *anchor = s.point(*anchor);
        }
    }
    // Floating things move relative to the water, which keeps its place in the world (like
    // `WaterSplash` velocities): their motion only turns.
    for mut f in &mut floating {
        f.velocity = s.direction(f.velocity);
        f.angular_velocity = s.direction(f.angular_velocity);
        f.force = s.direction(f.force);
        f.torque = s.direction(f.torque);
    }
    for mut role in &mut roles {
        role.shift(&s);
    }
    if let Some(mut demo) = demo {
        demo.origin = s.point(demo.origin);
    }
    if let Some(mut drops) = drops {
        for d in &mut drops.drops {
            d.pos = s.point(d.pos);
            d.floor = s.point(DVec3::new(d.pos.x, d.floor, d.pos.z)).y;
            d.vel = s.direction(d.vel);
        }
    }
}
