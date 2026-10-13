//! Following a [`WorldShift`]: when the world is re-anchored (floating origin) or turned into
//! another planet's frame, the simulation moves with it in the same frame.
//!
//! A plain floating-origin move only moves each region's anchor: nothing inside a Rapier world
//! changes, so sleeping bodies stay asleep and contacts carry on. A turn rotates each region's
//! contents about its anchor (sleeping bodies stay asleep, contact manifolds are body-relative
//! and stay valid). The caches the sync compares against are moved exactly as `engine_world`
//! moves the components, so the next step doesn't mistake the shift for teleports.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_world::WorldShift;
use rapier3d::prelude::Pose;

use crate::SharedSimulation;
use crate::sim::Simulation;

/// Move the simulation into the new frame (runs in `WorldShiftSet::Apply`).
pub(crate) fn shift_simulation(
    mut shifts: MessageReader<WorldShift>,
    shared: Res<SharedSimulation>,
    globals: Query<(Option<&GlobalTransform>, Has<ChildOf>)>,
) {
    let Some(s) = shifts.read().copied().reduce(|a, b| a.then(&b)) else {
        return;
    };
    shared.0.write().shift(&s, |e| {
        globals
            .get(e)
            .map(|(g, child)| (g.map(|g| g.translation()), child))
            .unwrap_or((None, false))
    });
}

impl Simulation {
    /// Move everything into the frame `s` leads to. `entity(e)` gives an entity's last
    /// propagated world position and whether it has a parent (only roots move by themselves).
    pub(crate) fn shift(
        &mut self,
        s: &WorldShift,
        entity: impl Fn(Entity) -> (Option<DVec3>, bool),
    ) {
        let turns = !s.is_translation_only();
        let frame_moves = s.velocity != DVec3::ZERO || s.angular_velocity != DVec3::ZERO;
        let r = s.rotation.as_quat();

        // ------------------------------------------------------------ regions
        for region in self.regions.iter_mut().flatten() {
            region.anchor = s.point(region.anchor);
            if !(turns || frame_moves) {
                continue;
            }
            let anchor = region.anchor;
            let ground = region.ground;
            let world = &mut region.world;
            let handles: Vec<_> = world.bodies.iter().map(|(h, _)| h).collect();
            for h in handles {
                if h == ground {
                    continue;
                }
                let rb = &mut world.bodies[h];
                let sleeping = rb.is_sleeping();
                if turns {
                    let p = *rb.position();
                    let pose = Pose::from_parts(r * p.translation, (r * p.rotation).normalize());
                    rb.set_position(pose, false);
                    if rb.is_kinematic() {
                        rb.set_next_kinematic_position(pose);
                    }
                }
                if rb.is_dynamic() || rb.is_kinematic() {
                    // The new frame's velocity: turned, plus the old frame's own motion here.
                    let at = anchor + rb.translation().as_dvec3();
                    let v = s.velocity_at(at, r * rb.linvel());
                    let w = (r * rb.angvel()).as_dvec3() + s.angular_velocity;
                    let w = w.as_vec3();
                    let moving = v != Vec3::ZERO || w != Vec3::ZERO;
                    rb.set_linvel(v, false);
                    rb.set_angvel(w, false);
                    if sleeping && !(frame_moves && moving) {
                        rb.sleep();
                    } else if frame_moves && moving {
                        rb.wake_up(true);
                    }
                }
            }
            if turns {
                // Joints pinned to the world hold their world-side frame on the ground body,
                // which stays put at the local origin: turn those frames instead.
                let turn = Pose::from_parts(Vec3::ZERO, r);
                for (_, j) in world.impulse_joints.iter_mut() {
                    if j.body1() == ground {
                        let f = j.data.local_frame1;
                        j.data.set_local_frame1(turn * f);
                    }
                    if j.body2() == ground {
                        let f = j.data.local_frame2;
                        j.data.set_local_frame2(turn * f);
                    }
                }
                // Attached colliders follow their bodies only when the region steps.
                for (_, col) in world.colliders.iter_mut() {
                    if let Some(parent) = col.parent()
                        && let Some(rb) = world.bodies.get(parent)
                        && let Some(rel) = col.position_wrt_parent()
                    {
                        col.set_position(*rb.position() * *rel);
                    }
                }
                region.needs_refresh = true;
                region.stale_all = true;
            }
        }

        // ------------------------------------------------------------ static colliders
        for c in self.colliders.values_mut().filter(|c| c.body.is_none()) {
            c.world_pose = (s.point(c.world_pose.0), s.orientation(c.world_pose.1));
        }
        if turns {
            let statics: Vec<(Entity, (DVec3, engine_core::math::Quat))> = self
                .colliders
                .iter()
                .filter(|(_, c)| c.body.is_none())
                .map(|(e, c)| (*e, c.world_pose))
                .collect();
            for (e, pose) in statics {
                let handles = self.colliders[&e].handles.clone();
                for (rid, h) in handles {
                    let region = self.region_mut(rid);
                    let local = Pose::from_parts(region.local(pose.0), pose.1);
                    if let Some(col) = region.world.colliders.get_mut(h) {
                        col.set_position(local);
                    }
                }
            }
            for region in self.regions.iter_mut().flatten() {
                region.update_bounds();
            }
        }

        // ------------------------------------------------------------ caches
        // Mirror `engine_world`'s own formulas on the same inputs, so unchanged bodies compare
        // equal at the next sync.
        for (e, b) in self.bodies.iter_mut() {
            let (global, child) = entity(*e);
            let root = !child && b.parent.is_none();
            // `engine_world` takes a root's lever arm from its `Transform` (what we wrote),
            // a child's from its `GlobalTransform`.
            let old = match b.written {
                Some((t, _)) if root => t,
                _ => global.unwrap_or(DVec3::ZERO),
            };
            if root {
                b.written = b.written.map(|(t, q)| (s.point(t), s.orientation(q)));
            }
            let at = s.point(old);
            if let Some(v) = &mut b.written_velocity {
                v.linear = s.velocity_at(at, v.linear);
                v.angular = s.spin(v.angular);
            }
            let (f, t, p) = b.external;
            b.external = (s.direction(f), s.direction(t), p.map(|p| s.point(p)));
        }
        for c in self.characters.values_mut() {
            c.written = c.written.map(|p| s.point(p));
        }
        for j in self.joints.values_mut() {
            if j.desc.body_b.is_none() {
                j.desc.anchor_b = s.point(j.desc.anchor_b);
            }
            // A rest pose seen from the world (or a static body) turns with it.
            if j.desc.body_b.is_none_or(|b| !self.bodies.contains_key(&b))
                && let Some(rest) = &mut j.rest
            {
                *rest = (r * *rest).normalize();
            }
        }
    }
}
