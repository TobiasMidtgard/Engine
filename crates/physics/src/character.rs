//! Kinematic character controller: capsules moved by sweeps (Rapier's KCC), with steps, slopes,
//! ground snapping, moving platforms and pushing dynamic bodies.
//!
//! Characters move after their region has stepped. Whatever they stood on before the step
//! (a lift, a turntable, a ship at orbital speed) carries them by exactly how far it moved and
//! turned; then they sweep their own move against the world as it is now.

use engine_core::math::Vec3;
use engine_core::prelude::*;
use engine_world::Gravity;
use engine_world::physics::{CharacterController, CharacterState};
use rapier3d::control::{
    CharacterAutostep, CharacterCollision, CharacterLength, KinematicCharacterController,
};
use rapier3d::parry::bounding_volume::BoundingVolume;
use rapier3d::parry::query::{QueryDispatcher, ShapeCastHit, ShapeCastOptions};
use rapier3d::prelude::{
    Collider, ColliderHandle, PhysicsWorld, Pose, QueryFilter, RigidBodyHandle, SharedShape,
};

use crate::capsule_box;
use crate::convert;
use crate::sim::{RegionId, Simulation};

/// Surfaces this close to the capsule count as touching it (the controller keeps a 2 cm gap).
const TOUCH_MARGIN: f32 = 0.05;

fn characters_in(sim: &Simulation, region_id: RegionId) -> Vec<Entity> {
    let mut entities: Vec<Entity> = sim
        .characters
        .iter()
        .filter(|(e, c)| c.region == region_id && !sim.resting.contains(*e))
        .map(|(e, _)| *e)
        .collect();
    entities.sort();
    entities
}

/// Before the region steps: note the body each character stands on and where that body is,
/// so [`move_characters`] can carry the character along with it.
pub fn find_platforms(sim: &mut Simulation, region_id: RegionId, gravity: &Gravity) {
    for e in characters_in(sim, region_id) {
        let entry = &sim.characters[&e];
        let (body, collider, desc) = (entry.body, entry.collider, entry.desc);
        let region = sim.region(region_id);
        let world = &region.world;
        let platform = world.bodies.get(body).and_then(|rb| {
            let pos = rb.translation();
            let up = gravity.up_at(region.anchor + pos.as_dvec3());
            let shape = world.colliders.get(collider)?.shared_shape().clone();
            let (h, hit) = probe_ground(world, body, &shape, &desc, pos, up)?;
            if !walkable(&desc, hit.normal1, up) {
                return None;
            }
            let parent = world.colliders[h].parent()?;
            let prb = world.bodies.get(parent)?;
            (!prb.is_fixed()).then(|| (parent, *prb.position()))
        });
        sim.characters.get_mut(&e).unwrap().platform = platform;
    }
}

/// After the region stepped: carry every character in `region_id` with its platform, move it
/// by its desired velocity and fill in its [`CharacterState`].
pub fn move_characters(sim: &mut Simulation, region_id: RegionId, gravity: &Gravity) {
    for e in characters_in(sim, region_id) {
        move_one(sim, region_id, e, gravity);
    }
}

fn walkable(desc: &CharacterController, normal: Vec3, up: Vec3) -> bool {
    normal.dot(up) >= max_slope(desc).cos() - 1e-3
}

fn max_slope(desc: &CharacterController) -> f32 {
    desc.max_slope_deg.clamp(0.0, 89.0).to_radians()
}

fn query_filter(body: RigidBodyHandle, desc: &CharacterController) -> QueryFilter<'static> {
    QueryFilter::new()
        .exclude_rigid_body(body)
        .exclude_sensors()
        .groups(convert::groups(&desc.layers))
}

/// What is right under the capsule's feet, within snapping distance, with a normal that can be
/// trusted ([`surface_normal`]).
fn probe_ground(
    world: &PhysicsWorld,
    body: RigidBodyHandle,
    shape: &SharedShape,
    desc: &CharacterController,
    pos: Vec3,
    up: Vec3,
) -> Option<(ColliderHandle, ShapeCastHit)> {
    let queries = capsule_box::dispatcher();
    let qp = world.broad_phase.as_query_pipeline(
        &queries,
        &world.bodies,
        &world.colliders,
        query_filter(body, desc),
    );
    let rot = convert::up_rotation(up);
    let (h, mut hit) = qp.cast_shape(
        &Pose::from_parts(pos, rot),
        -up,
        &**shape,
        ShapeCastOptions {
            max_time_of_impact: desc.snap_distance.max(0.05) + 0.05,
            target_distance: 0.0,
            stop_at_penetration: false,
            compute_impact_geometry_on_penetration: true,
        },
    )?;
    let at = Pose::from_parts(pos - up * hit.time_of_impact, rot);
    hit.normal1 = surface_normal(shape, &at, &world.colliders[h], hit.witness1, hit.normal1);
    Some((h, hit))
}

/// Colliders within [`TOUCH_MARGIN`] of the capsule at `pose`, with their surface normals
/// (pointing away from the surface, toward the character).
fn touching(
    world: &PhysicsWorld,
    filter: QueryFilter,
    shape: &SharedShape,
    pose: &Pose,
) -> Vec<(ColliderHandle, Vec3)> {
    let queries = capsule_box::dispatcher();
    let qp = world
        .broad_phase
        .as_query_pipeline(&queries, &world.bodies, &world.colliders, filter);
    let aabb = shape.compute_aabb(pose).loosened(TOUCH_MARGIN);
    qp.intersect_aabb_conservative(aabb)
        .filter_map(|(h, c)| {
            let pos12 = pose.inv_mul(c.position());
            let mut hit = queries
                .contact(&pos12, &**shape, c.shape(), TOUCH_MARGIN)
                .ok()??;
            hit.transform_by_mut(pose, c.position());
            Some((h, surface_normal(shape, pose, c, hit.point2, hit.normal2)))
        })
        .collect()
}

/// The normal of `collider` where it touches the character at `pose`, pointing away from the
/// surface toward the character. `near` is a point on or near that surface.
///
/// The queries' own normals and hit points can't be trusted when the capsule starts inside
/// the controller's 2 cm gap or a few millimetres into the floor, which is every step on the
/// ground: about one step in a hundred one of them points sideways and reads as a wall in the
/// middle of a field, or tilts the floor. Projecting a point onto a surface is exact, so the
/// normal comes from the closest points between the capsule's core segment and the collider,
/// found by projecting back and forth. Other character shapes keep the query's `fallback`.
fn surface_normal(
    shape: &SharedShape,
    pose: &Pose,
    collider: &Collider,
    near: Vec3,
    fallback: Vec3,
) -> Vec3 {
    let (a, b) = if let Some(cap) = shape.as_capsule() {
        (
            pose.transform_point(cap.segment.a),
            pose.transform_point(cap.segment.b),
        )
    } else if shape.as_ball().is_some() {
        (pose.translation, pose.translation)
    } else {
        return fallback;
    };
    let ab = b - a;
    let on_core =
        |p: Vec3| a + ab * ((p - a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
    let mut core = on_core(near);
    let mut out = fallback;
    for _ in 0..4 {
        let proj = collider
            .shape()
            .project_point(collider.position(), core, false);
        // A core inside the collider (sunk deeper than the capsule's radius) leaves through
        // the nearest face.
        let away = if proj.is_inside {
            proj.point - core
        } else {
            core - proj.point
        };
        out = away.try_normalize().unwrap_or(fallback);
        let next = on_core(proj.point);
        if next.distance_squared(core) < 1.0e-8 {
            break;
        }
        core = next;
    }
    out
}

/// Push the dynamic bodies the character walked into toward its walking velocity, like a
/// short contact between the two masses. Velocities are compared relative to the platform the
/// character rides, so a ship's own speed never leaks into the push.
fn push_bodies(
    world: &mut PhysicsWorld,
    desc: &CharacterController,
    platform: Option<(RigidBodyHandle, Pose)>,
    shape: &SharedShape,
    collisions: &[CharacterCollision],
    walking: Vec3,
) {
    let mass = desc.mass_kg.max(1.0);
    let mut pushed: Vec<RigidBodyHandle> = Vec::new();
    for c in collisions {
        let Some(b) = world.colliders.get(c.handle).and_then(|c| c.parent()) else {
            continue;
        };
        if pushed.contains(&b) {
            continue;
        }
        let point = c.hit.witness1;
        let frame = platform
            .and_then(|(h, _)| world.bodies.get(h))
            .map_or(Vec3::ZERO, |p| p.velocity_at_point(point));
        let Some(rb) = world.bodies.get_mut(b) else {
            continue;
        };
        if !rb.is_dynamic() {
            continue;
        }
        let into = -surface_normal(
            shape,
            &c.character_pos,
            &world.colliders[c.handle],
            c.hit.witness1,
            c.hit.normal1,
        );
        let closing = (walking - (rb.velocity_at_point(point) - frame)).dot(into);
        if closing > 0.0 {
            let m = rb.mass();
            rb.apply_impulse_at_point(into * closing * (m * mass / (m + mass)), point, true);
            pushed.push(b);
        }
    }
}

fn move_one(sim: &mut Simulation, region_id: RegionId, e: Entity, gravity: &Gravity) {
    let entry = &sim.characters[&e];
    let (body, collider, desc, platform) = (entry.body, entry.collider, entry.desc, entry.platform);
    let stood = entry.state.grounded;
    // The character's own move runs on its own clock (`dt`); bodies it pushes run on world
    // time (`world_dt`).
    let (dt, world_dt) = (entry.dt, entry.world_dt.max(1e-6));
    let desired = entry.desired * dt;
    let region = sim.region_mut(region_id);
    let anchor = region.anchor;
    let world = &mut region.world;
    let Some(rb) = world.bodies.get(body) else {
        return;
    };
    let mut pos = rb.translation();
    // Ride along: the platform's motion this step, applied to the point the character was at.
    if let Some((handle, before)) = platform
        && let Some(prb) = world.bodies.get(handle)
    {
        pos = prb
            .position()
            .transform_point(before.inverse_transform_point(pos));
    }
    let up = gravity.up_at(anchor + pos.as_dvec3());
    let rot = convert::up_rotation(up);
    let shape = world.colliders[collider].shared_shape().clone();
    let max_slope = max_slope(&desc);
    let kcc = KinematicCharacterController {
        up,
        offset: CharacterLength::Absolute(0.02),
        slide: true,
        autostep: (desc.step_height > 0.0).then_some(CharacterAutostep {
            max_height: CharacterLength::Absolute(desc.step_height),
            min_width: CharacterLength::Absolute((desc.radius * 0.5).max(0.05)),
            include_dynamic_bodies: true,
        }),
        max_slope_climb_angle: max_slope,
        min_slope_slide_angle: max_slope,
        snap_to_ground: (desc.snap_distance > 0.0)
            .then_some(CharacterLength::Absolute(desc.snap_distance)),
        normal_nudge_factor: 1.0e-4,
    };
    let filter = query_filter(body, &desc);
    let queries = capsule_box::dispatcher();
    // Horizontal first, then vertical: with gravity folded into one diagonal move, the floor
    // stops the sweep before it reaches a step's edge and stairs can't be climbed.
    let vertical = up * desired.dot(up);
    let horizontal = desired - vertical;
    let mut pose = Pose::from_parts(pos, rot);
    let mut collisions: Vec<CharacterCollision> = Vec::new();
    let mut moved = Vec3::ZERO;
    let mut grounded = false;
    // Collisions before this index came from the horizontal move (walking into things).
    let mut walked_into = 0;
    {
        // Things already touching the capsule (a crate resting on its head) must not stop a
        // move away from them: the controller would take the touch as the hit and skip the
        // floor.
        let touched = touching(world, filter, &shape, &pose);
        let mut climbed = false;
        for (i, part) in [horizontal, vertical].into_iter().enumerate() {
            if i == 1 {
                walked_into = collisions.len();
            }
            // The vertical move always runs: it also finds out whether we stand on something.
            if i == 0 && part.length_squared() < 1.0e-12 {
                continue;
            }
            let leaving: Vec<ColliderHandle> = touched
                .iter()
                .filter(|(_, n)| n.dot(part) > 0.0)
                .map(|(h, _)| *h)
                .collect();
            let keep = |h: ColliderHandle, _: &Collider| !leaving.contains(&h);
            let qp = world.broad_phase.as_query_pipeline(
                &queries,
                &world.bodies,
                &world.colliders,
                filter.predicate(&keep),
            );
            // Right after stepping up, the capsule rests on the step's edge; sliding off it
            // under gravity would undo the step, so the vertical move just stops there.
            let kcc = KinematicCharacterController {
                slide: !climbed,
                // No nudge off the floor for gravity's move on it (see below).
                normal_nudge_factor: if i == 1 && stood && part.dot(up) <= 0.0 {
                    0.0
                } else {
                    kcc.normal_nudge_factor
                },
                ..kcc
            };
            // A zero time step turns off the controller's own platform friction: the carry
            // above already moved the character with its platform.
            let m = kcc.move_shape(0.0, &qp, &*shape, &pose, part, |c| collisions.push(c));
            climbed = i == 0 && m.translation.dot(up) > 1.0e-4;
            // Gravity against a floor it can stand on moves the character only up or down.
            // The controller nudges it off every surface it stops on along that surface's
            // normal (`normal_nudge_factor`): on a slope that is partly downhill, and on a
            // steep one partly up, so a character standing still crept 0.1 mm a step (6 mm/s
            // at 45°), down most slopes and up the steepest. Unreal's walking mode likewise
            // only lifts or lowers a pawn onto its floor.
            let step = if i == 1 && m.grounded && !m.is_sliding_down_slope {
                up * m.translation.dot(up)
            } else {
                m.translation
            };
            pose.translation += step;
            moved += step;
            grounded = m.grounded;
        }
    }
    let end = pos + moved;
    if let Some(rb) = world.bodies.get_mut(body) {
        let to = Pose::from_parts(end, *rb.rotation());
        rb.set_position(to, false);
        rb.set_next_kinematic_position(to);
        // Queries until the next step see the character where it is now.
        if let Some(col) = world.colliders.get_mut(collider) {
            col.set_position(to);
            let params = world.integration_parameters;
            let aabb = col.compute_broad_phase_aabb(&params, &world.bodies);
            world.broad_phase.set_aabb(&params, collider, aabb);
        }
    }

    let cos_slope = max_slope.cos();
    let mut state = CharacterState {
        grounded,
        ground_normal: Vec3::ZERO,
        ground_entity: None,
        ground_surface: None,
        // The character's own motion, without what its platform carried it.
        velocity: moved / dt.max(1e-6),
        wall_normal: None,
        ceiling: false,
    };
    let mut stands_on = None;
    if let Some((h, hit)) = probe_ground(world, body, &shape, &desc, end, up) {
        let n = hit.normal1;
        // Ground within snapping reach only counts once the character is on it: the controller
        // snaps down only characters that started the move on the ground, so one falling toward
        // the floor would otherwise read grounded while still 30 cm up and falling fast.
        let on_it = grounded || hit.time_of_impact <= TOUCH_MARGIN;
        if on_it && n.dot(up) >= cos_slope - 1e-3 {
            state.grounded = true;
        }
        if state.grounded {
            let c = &world.colliders[h];
            state.ground_normal = n;
            state.ground_entity = Some(convert::entity_of(c.user_data));
            state.ground_surface = Some(convert::surface_of(c.user_data));
            stands_on = c.parent().map(|b| (b, hit.witness1));
        }
    }
    if desc.push_bodies {
        let weight = gravity.at(anchor + end.as_dvec3()) * desc.mass_kg;
        let walking = horizontal / world_dt;
        push_bodies(
            world,
            &desc,
            platform,
            &shape,
            &collisions[..walked_into],
            walking,
        );
        if let Some((b, point)) = stands_on
            && let Some(rb) = world.bodies.get_mut(b)
            && rb.is_dynamic()
        {
            rb.apply_impulse_at_point(weight * world_dt, point, true);
        }
    }
    // Surfaces hit while moving, plus surfaces touched without moving into them (running
    // along a wall): normals point away from the surface, toward the character.
    let mut normals: Vec<Vec3> = collisions
        .iter()
        .map(|c| {
            surface_normal(
                &shape,
                &c.character_pos,
                &world.colliders[c.handle],
                c.hit.witness1,
                c.hit.normal1,
            )
        })
        .collect();
    normals.extend(
        touching(world, filter, &shape, &Pose::from_parts(end, rot))
            .into_iter()
            .map(|(_, n)| n),
    );
    let mut best_wall: Option<(f32, Vec3)> = None;
    for n in normals {
        let d = n.dot(up);
        if d < -0.5 {
            state.ceiling = true;
        } else if d < cos_slope - 1e-3 && best_wall.is_none_or(|(bd, _)| d.abs() < bd) {
            best_wall = Some((d.abs(), n));
        }
    }
    state.wall_normal = best_wall.map(|(_, n)| n);
    sim.characters.get_mut(&e).unwrap().state = state;
}
