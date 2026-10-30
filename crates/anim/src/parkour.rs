//! Parkour for people: jumps, the air, landings, wall runs and slides, answered from the
//! movement contract alone. A person with a [`LocomotionState`] (a movement module's player,
//! an NPC, the paddock's course runner) gets a [`Parkour`]; each frame it turns the state into
//! the humanoid graph's parameters (`airborne`, `on_ground`, `air_vy`, `wall_run`, `sliding`)
//! and, when the state changes, its triggers (a take-off, a second jump, a landing soft or
//! hard, a kick off a wall). Nothing new is needed in the contract: which side the wall is on
//! and how far is found here, with rays to either side at hip height against physics colliders
//! and [`IkSurface`]s, and after the graph [`wall_ik`] moves the feet (and a hand a clip
//! plants) onto the wall actually there (the clips put it [`WALL_DISTANCE`] off).

use engine_core::prelude::*;
use engine_world::animation::AnimationParams;
use engine_world::physics::{PhysicsQueries, QueryFilter};
use engine_world::{LocomotionMode, LocomotionState};

use crate::Animator;
use crate::build::humanoid::{
    TRIGGER_DOUBLE_JUMP, TRIGGER_JUMP, TRIGGER_LAND, TRIGGER_LAND_HARD, TRIGGER_WALL_KICK,
    WALL_CURVES, WALL_DISTANCE, WALL_INSET,
};
use crate::ik::{IkShape, IkSurface, IkSurfaces, two_bone};

/// How far to either side a wall is looked for, from the body's axis, m.
const WALL_REACH: f32 = 1.4;
/// Height above the body's origin the rays go out at, m.
const RAY_HEIGHT: f32 = 0.95;
/// A fall faster than this lands hard, m/s.
pub const HARD_LANDING: f32 = 8.0;
/// A fall slower than this needs no landing at all, m/s.
const SOFT_LANDING: f32 = 2.0;
/// Landing faster than this across the ground runs straight on instead of soaking it up, m/s.
pub const LANDING_RUN: f32 = 4.0;
/// A rise in vertical speed this sudden in the air is a second jump, m/s.
const DOUBLE_JUMP_KICK: f32 = 2.5;
/// Leaving a wall faster than this away from it is a kick off it, m/s.
const KICK_SPEED: f32 = 1.5;
/// How far the feet and hand are moved to meet a wall nearer or farther than the clips', m.
const WALL_REACH_MIN: f32 = -0.25;
const WALL_REACH_MAX: f32 = 0.35;

/// A person's parkour state (published): the wall it runs on, if any.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Parkour {
    pub wall: Option<WallTouch>,
    /// The state last frame: mode, on the ground, vertical speed.
    #[serde(skip)]
    last: Option<(LocomotionMode, bool, f32)>,
}

/// A wall being run on.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WallTouch {
    /// +1 to the right, −1 to the left.
    pub side: f32,
    /// From the body's axis to the wall's face, m.
    pub distance: f32,
    /// Its face's normal (world).
    pub normal: Vec3,
}

/// Give people that report a [`LocomotionState`] their [`Parkour`].
#[allow(clippy::type_complexity)]
pub(crate) fn give_parkour(
    mut commands: Commands,
    q: Query<(Entity, &Animator), (With<LocomotionState>, Without<Parkour>)>,
) {
    for (entity, animator) in &q {
        let s = &animator.skeleton;
        if s.index_of("foot_l").is_some() && s.index_of("hand_l").is_some() {
            commands.entity(entity).try_insert(Parkour::default());
        }
    }
}

/// Turn each person's movement state into parameters and triggers (`AnimSet::Setup`).
#[allow(clippy::type_complexity)]
pub(crate) fn sense_parkour(
    physics: Option<Res<PhysicsQueries>>,
    surfaces: Res<IkSurfaces>,
    mut q: Query<(
        Entity,
        &GlobalTransform,
        &LocomotionState,
        &mut AnimationParams,
        &mut Animator,
        &mut Parkour,
    )>,
) {
    for (entity, gt, state, mut params, mut animator, mut parkour) in &mut q {
        let mode = state.mode;
        // The mode says where the body is: a mover that leaves `grounded` unset (or reports a
        // ground mode a frame early) still walks; only `Air` is the air.
        let grounded = matches!(
            mode,
            LocomotionMode::Idle
                | LocomotionMode::Walk
                | LocomotionMode::Run
                | LocomotionMode::Sprint
                | LocomotionMode::Crouch
                | LocomotionMode::Slide
                | LocomotionMode::Dash
        );
        let vy = state.velocity.y;
        let on_wall = mode == LocomotionMode::WallRun;
        if on_wall && let Some(w) = find_wall(entity, gt, physics.as_deref(), &surfaces.0) {
            parkour.wall = Some(w);
        }
        let free = mode == LocomotionMode::Air;
        let side = parkour.wall.filter(|_| on_wall).map_or(0.0, |w| w.side);
        params.set("airborne", if free { 1.0 } else { 0.0 });
        params.set("on_ground", if grounded { 1.0 } else { 0.0 });
        params.set("air_vy", vy);
        params.set("wall_run", side);
        params.set(
            "sliding",
            if mode == LocomotionMode::Slide {
                1.0
            } else {
                0.0
            },
        );
        if let Some((last_mode, was_grounded, last_vy)) = parkour.last {
            let flat = state.velocity.with_y(0.0).length();
            if was_grounded && !grounded && !on_wall && vy > 1.0 {
                animator.trigger(TRIGGER_JUMP);
            } else if !was_grounded
                && !grounded
                && mode == LocomotionMode::Air
                && last_mode == LocomotionMode::Air
                && vy - last_vy > DOUBLE_JUMP_KICK
            {
                animator.trigger(TRIGGER_DOUBLE_JUMP);
            } else if !was_grounded && grounded {
                let fall = -last_vy;
                if fall > HARD_LANDING {
                    animator.trigger(TRIGGER_LAND_HARD);
                } else if fall > SOFT_LANDING && flat < LANDING_RUN {
                    animator.trigger(TRIGGER_LAND);
                }
            }
            if last_mode == LocomotionMode::WallRun
                && !on_wall
                && parkour
                    .wall
                    .is_some_and(|w| state.velocity.dot(w.normal) > KICK_SPEED)
            {
                animator.trigger(TRIGGER_WALL_KICK);
            }
        }
        if !on_wall {
            parkour.wall = None;
        }
        parkour.last = Some((mode, grounded, vy));
    }
}

/// The nearest wall to the left or right of a body, at hip height.
fn find_wall(
    entity: Entity,
    gt: &GlobalTransform,
    physics: Option<&PhysicsQueries>,
    surfaces: &[(GlobalTransform, IkSurface)],
) -> Option<WallTouch> {
    let origin = gt.translation() + DVec3::Y * RAY_HEIGHT as f64;
    let right = (gt.rotation() * Vec3::X).with_y(0.0).normalize_or_zero();
    if right == Vec3::ZERO {
        return None;
    }
    let filter = QueryFilter {
        layers: !crate::ragdoll::RAGDOLL_LAYER,
        exclude: vec![entity],
        include_sensors: false,
    };
    let mut best: Option<WallTouch> = None;
    for side in [1.0f32, -1.0] {
        let dir = right * side;
        let mut consider = |distance: f32, normal: Vec3| {
            // Walls only, facing the body.
            if normal.y.abs() > 0.5 || normal.dot(dir) > -0.3 {
                return;
            }
            if best.is_none_or(|b| distance < b.distance) {
                best = Some(WallTouch {
                    side,
                    distance,
                    normal,
                });
            }
        };
        if let Some(phys) = physics
            && let Some(hit) = phys.0.raycast(origin, dir, WALL_REACH, &filter)
        {
            consider(hit.distance, hit.normal);
        }
        for (sgt, s) in surfaces {
            if let Some((d, n)) = ray_box(sgt, s, origin, dir, WALL_REACH) {
                consider(d, n);
            }
        }
    }
    best
}

/// Where a ray first meets a box-shaped [`IkSurface`] (its distance and the face's normal).
fn ray_box(
    gt: &GlobalTransform,
    surface: &IkSurface,
    origin: DVec3,
    dir: Vec3,
    reach: f32,
) -> Option<(f32, Vec3)> {
    if surface.shape != IkShape::Box {
        return None;
    }
    let inv = gt.inverse();
    let o = inv.transform_point3(origin).as_vec3();
    let d = inv.transform_vector3(dir.as_dvec3()).as_vec3();
    let h = Vec3::from_array(surface.half_extents);
    let (mut near, mut far) = (0.0f32, reach);
    let mut axis = None;
    for i in 0..3 {
        if d[i].abs() < 1e-6 {
            if o[i].abs() > h[i] {
                return None;
            }
            continue;
        }
        let (a, b) = ((-h[i] - o[i]) / d[i], (h[i] - o[i]) / d[i]);
        let (t0, t1) = if a < b { (a, b) } else { (b, a) };
        if t0 > near {
            near = t0;
            axis = Some(i);
        }
        far = far.min(t1);
        if near > far {
            return None;
        }
    }
    let i = axis?;
    let mut n = Vec3::ZERO;
    n[i] = -d[i].signum();
    Some((near, (gt.rotation() * n).normalize()))
}

/// Move the feet (and any hand a clip plants) of a wall run onto the wall that is there
/// (`AnimSet::Ik`, after the feet): each by how firmly its clip has it on the wall. A limb is
/// taken to the wall's plane itself, not moved by the clip's offset from it, so a foot still
/// blending in from a jump lands on the wall as soon as its clip says it is there.
pub(crate) fn wall_ik(mut q: Query<(&GlobalTransform, &mut Animator, &Parkour)>) {
    for (gt, mut animator, parkour) in &mut q {
        let Some(wall) = parkour.wall else {
            continue;
        };
        let scale = gt.scale().y.max(1e-3);
        let plane = WALL_DISTANCE
            + (wall.distance / scale - WALL_DISTANCE).clamp(WALL_REACH_MIN, WALL_REACH_MAX);
        let firm: Vec<f32> = WALL_CURVES
            .iter()
            .map(|c| animator.curve(c).clamp(0.0, 1.0))
            .collect();
        if firm.iter().all(|&f| f <= 0.0) {
            continue;
        }
        let skeleton = animator.skeleton.clone();
        let mut edit = animator.edit();
        for (limb, [upper, lower, end]) in [
            ["upper_arm_l", "lower_arm_l", "hand_l"],
            ["upper_arm_r", "lower_arm_r", "hand_r"],
            ["upper_leg_l", "lower_leg_l", "foot_l"],
            ["upper_leg_r", "lower_leg_r", "foot_r"],
        ]
        .into_iter()
        .enumerate()
        {
            let w = firm[limb];
            let (Some(a), Some(b), Some(c)) = (
                skeleton.index_of(upper),
                skeleton.index_of(lower),
                skeleton.index_of(end),
            ) else {
                continue;
            };
            if w <= 0.0 {
                continue;
            }
            let (pa, pb, pc) = (edit.position(a), edit.position(b), edit.position(c));
            let pole = pb - (pa + pc) * 0.5;
            let shift = wall.side * (plane - WALL_INSET[limb]) - pc.x;
            if shift.abs() < 1e-3 {
                continue;
            }
            let target = pc + Vec3::X * shift * w;
            let held = edit.model[c].r;
            two_bone(&mut edit, a, b, c, target, pole, 1.0);
            edit.set_model_rotation(c, held);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ray_finds_a_box_face_and_its_normal() {
        let gt = GlobalTransform::from(Transform::from_translation(DVec3::new(0.0, 1.0, 2.0)));
        let surface = IkSurface {
            shape: IkShape::Box,
            half_extents: [2.0, 1.0, 0.15],
            surface: 0,
        };
        let (d, n) = ray_box(&gt, &surface, DVec3::new(0.5, 1.0, 0.0), Vec3::Z, 3.0).unwrap();
        assert!((d - 1.85).abs() < 1e-4, "{d}");
        assert!(n.distance(Vec3::NEG_Z) < 1e-4, "{n}");
        // Out of reach, or pointing away: nothing.
        assert!(ray_box(&gt, &surface, DVec3::new(0.5, 1.0, 0.0), Vec3::Z, 1.0).is_none());
        assert!(ray_box(&gt, &surface, DVec3::new(0.5, 1.0, 0.0), Vec3::NEG_Z, 3.0).is_none());
    }

    #[test]
    fn a_wall_beside_the_body_is_found_on_its_side() {
        // Facing +X (right is +Z), a wall 0.6 m to the left.
        let body = GlobalTransform::from(
            Transform::from_translation(DVec3::new(0.0, 0.0, 0.0))
                .with_rotation(Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2)),
        );
        let wall = (
            GlobalTransform::from(Transform::from_translation(DVec3::new(0.0, 1.0, -0.75))),
            IkSurface {
                shape: IkShape::Box,
                half_extents: [3.0, 1.5, 0.15],
                surface: 3,
            },
        );
        let touch = find_wall(Entity::PLACEHOLDER, &body, None, &[wall]).unwrap();
        assert_eq!(touch.side, -1.0);
        assert!((touch.distance - 0.6).abs() < 1e-3, "{}", touch.distance);
        assert!(touch.normal.distance(Vec3::Z) < 1e-3);
    }
}
