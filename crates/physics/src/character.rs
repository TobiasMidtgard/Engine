//! Kinematic character controller: capsules moved by sweeps (Rapier's KCC), with steps, slopes,
//! ground snapping, moving platforms and pushing dynamic bodies.

use engine_core::math::Vec3;
use engine_core::prelude::*;
use engine_world::Gravity;
use rapier3d::control::{
    CharacterAutostep, CharacterCollision, CharacterLength, KinematicCharacterController,
};
use rapier3d::parry::bounding_volume::BoundingVolume;
use rapier3d::parry::query::{ShapeCastOptions, contact};
use rapier3d::prelude::{Pose, QueryFilter};

use crate::convert;
use crate::sim::{RegionId, Simulation};

/// Surfaces this close to the capsule count as touching it (the controller keeps a 2 cm gap).
const TOUCH_MARGIN: f32 = 0.05;

/// Move every character in `region_id` by its desired velocity for this step and fill in its
/// [`engine_world::physics::CharacterState`].
pub fn move_characters(sim: &mut Simulation, region_id: RegionId, gravity: &Gravity) {
    let entities: Vec<Entity> = sim
        .characters
        .iter()
        .filter(|(_, c)| c.region == region_id)
        .map(|(e, _)| *e)
        .collect();
    for e in entities {
        move_one(sim, region_id, e, gravity);
    }
}

fn move_one(sim: &mut Simulation, region_id: RegionId, e: Entity, gravity: &Gravity) {
    let entry = &sim.characters[&e];
    let (body, collider, desc) = (entry.body, entry.collider, entry.desc);
    // The character's own move runs on its own clock (`dt`); platforms it stands on and bodies
    // it pushes run on world time (`world_dt`).
    let (dt, world_dt) = (entry.dt, entry.world_dt.max(1e-6));
    let desired = entry.desired * dt + entry.platform_velocity * world_dt;
    let region = sim.region_mut(region_id);
    let anchor = region.anchor;
    let world = &mut region.world;
    let Some(rb) = world.bodies.get(body) else {
        return;
    };
    let pos = rb.translation();
    let up = gravity.up_at(anchor + pos.as_dvec3());
    let rot = convert::up_rotation(up);
    let shape = world.colliders[collider].shared_shape().clone();
    let max_slope = desc.max_slope_deg.clamp(0.0, 89.0).to_radians();
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
    let groups = convert::groups(&desc.layers);
    let filter = QueryFilter::new()
        .exclude_rigid_body(body)
        .exclude_sensors()
        .groups(groups);
    // Horizontal first, then vertical: with gravity folded into one diagonal move, the floor
    // stops the sweep before it reaches a step's edge and stairs can't be climbed.
    let vertical = up * desired.dot(up);
    let horizontal = desired - vertical;
    let mut pose = Pose::from_parts(pos, rot);
    let mut collisions: Vec<CharacterCollision> = Vec::new();
    let mut moved = Vec3::ZERO;
    let mut grounded = false;
    {
        let qp = world.broad_phase.as_query_pipeline(
            world.narrow_phase.query_dispatcher(),
            &world.bodies,
            &world.colliders,
            filter,
        );
        let mut climbed = false;
        for (i, part) in [horizontal, vertical].into_iter().enumerate() {
            // The vertical move always runs: it also finds out whether we stand on something.
            if i == 0 && part.length_squared() < 1.0e-12 {
                continue;
            }
            // Right after stepping up, the capsule rests on the step's edge; sliding off it
            // under gravity would undo the step, so the vertical move just stops there.
            let kcc = KinematicCharacterController {
                slide: !climbed,
                ..kcc
            };
            let m = kcc.move_shape(world_dt, &qp, &*shape, &pose, part, |c| collisions.push(c));
            climbed = i == 0 && m.translation.dot(up) > 1.0e-4;
            pose.translation += m.translation;
            moved += m.translation;
            grounded = m.grounded;
        }
    }
    let movement = rapier3d::control::EffectiveCharacterMovement {
        translation: moved,
        grounded,
        is_sliding_down_slope: false,
    };
    if desc.push_bodies && !collisions.is_empty() {
        let mut qp = world.broad_phase.as_query_pipeline_mut(
            world.narrow_phase.query_dispatcher(),
            &mut world.bodies,
            &mut world.colliders,
            filter,
        );
        kcc.solve_character_collision_impulses(
            world_dt,
            &mut qp,
            &*shape,
            desc.mass_kg.max(1.0),
            &collisions,
        );
    }
    let end = pos + movement.translation;
    if let Some(rb) = world.bodies.get_mut(body) {
        rb.set_next_kinematic_translation(end);
    }

    // Ground probe: what is right under the feet (normal, surface, moving platform).
    let probe = {
        let qp = world.broad_phase.as_query_pipeline(
            world.narrow_phase.query_dispatcher(),
            &world.bodies,
            &world.colliders,
            filter,
        );
        qp.cast_shape(
            &Pose::from_parts(end, rot),
            -up,
            &*shape,
            ShapeCastOptions {
                max_time_of_impact: desc.snap_distance.max(0.05) + 0.05,
                target_distance: 0.0,
                stop_at_penetration: false,
                compute_impact_geometry_on_penetration: true,
            },
        )
    };
    let cos_slope = max_slope.cos();
    let mut state = engine_world::physics::CharacterState {
        grounded: movement.grounded,
        ground_normal: Vec3::ZERO,
        ground_entity: None,
        ground_surface: None,
        velocity: movement.translation / dt.max(1e-6),
        wall_normal: None,
        ceiling: false,
    };
    let mut platform = Vec3::ZERO;
    if let Some((h, hit)) = probe {
        let n = hit.normal1;
        if n.dot(up) >= cos_slope - 1e-3 {
            state.grounded = true;
        }
        if state.grounded {
            let c = &world.colliders[h];
            state.ground_normal = n;
            state.ground_entity = Some(convert::entity_of(c.user_data));
            state.ground_surface = Some(convert::surface_of(c.user_data));
            if let Some(parent) = c.parent()
                && let Some(prb) = world.bodies.get(parent)
                && !prb.is_fixed()
            {
                let point = end - up * (desc.height * 0.5);
                platform = prb.velocity_at_point(point);
            }
        }
    }
    // Surfaces hit while moving, plus surfaces touched without moving into them (running
    // along a wall): normals point away from the surface, toward the character.
    let mut normals: Vec<Vec3> = collisions.iter().map(|c| c.hit.normal1).collect();
    {
        let qp = world.broad_phase.as_query_pipeline(
            world.narrow_phase.query_dispatcher(),
            &world.bodies,
            &world.colliders,
            filter,
        );
        let pose = Pose::from_parts(end, rot);
        let aabb = shape.compute_aabb(&pose).loosened(TOUCH_MARGIN);
        for (_, c) in qp.intersect_aabb_conservative(aabb) {
            if let Ok(Some(hit)) = contact(&pose, &*shape, c.position(), c.shape(), TOUCH_MARGIN) {
                normals.push(hit.normal2);
            }
        }
    }
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
    let entry = sim.characters.get_mut(&e).unwrap();
    entry.platform_velocity = platform;
    entry.state = state;
}
