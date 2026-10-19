//! The paddock's stunt yard, its ragdoll test case: a stuntman climbs the stairs of a raised
//! deck, sprints across it and goes limp at the edge, falls onto a crash mat with the momentum
//! of his run, lies still, gets up off the mat (face up or face down, whichever way he landed)
//! and walks back round to go again. The deck,
//! stairs and mat are physics colliders (and IK surfaces for his feet); without a physics
//! module he stops at the edge, stays on his feet and walks back.

use std::f32::consts::PI;

use engine_core::prelude::*;
use engine_render::{MaterialPreset, MeshRenderer, Shape as MeshShape, StandardMaterial};
use engine_world::Ground;
use engine_world::animation::{BodyRequest, Ragdoll, SkeletonPose};
use engine_world::physics::{Collider, PhysicsQueries, RigidBody};

use crate::animator::Animator;
use crate::ik::{GroundProbe, IkShape, IkSurface, IkSurfaces};
use crate::ragdoll::{RagdollRig, get_up_placement};
use crate::xform::smoothing;

/// Where the stunt yard is in the paddock: x, z from its center, m.
pub const YARD: [f64; 2] = [0.0, 28.0];
/// Height of the deck, m.
pub const DECK_HEIGHT: f32 = 1.2;
/// Half extents of the crash mat, m.
const MAT: Vec3 = Vec3::new(2.3, 0.18, 1.5);
/// How fast the stuntman changes speed, m/s².
const ACCELERATION: f32 = 5.0;
/// Longest he takes to get up before walking off anyway, s.
const RISE_SECS: f32 = 5.0;

/// What the stuntman is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StuntPhase {
    /// Walking to the stairs, climbing and sprinting across the deck.
    Run,
    /// Limp, falling and lying on the mat.
    Fall,
    /// Getting up off the mat.
    Rise,
    /// Walking back round to the start.
    Back,
}

/// A stuntman's routine (see the module docs). Waypoints are `[x, z, speed]`, m and m/s
/// from the paddock's center; the run ends at the deck's edge.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stunt {
    pub origin: DVec3,
    pub run: Vec<[f32; 3]>,
    pub back: Vec<[f32; 3]>,
    /// How long he lies on the mat, s.
    pub lie_secs: f32,
    pub phase: StuntPhase,
    /// Seconds in this phase.
    pub time: f32,
    next: usize,
    speed: f32,
    /// Whether the entity has been moved under the fallen body (a frame before rising, so
    /// the pose it rises from is where he lies).
    moved: bool,
}

impl Stunt {
    pub fn new(origin: DVec3) -> Self {
        let [x, z] = YARD;
        let (x, z) = (x as f32, z as f32);
        Self {
            origin,
            run: vec![
                [x - 9.5, z, 1.4],
                [x - 6.2, z, 1.3],
                [x - 3.2, z, 1.1],
                [x - 2.2, z, 3.2],
                [x - 0.05, z, 4.2],
            ],
            back: vec![
                [x + 2.4, z + 2.6, 1.3],
                [x - 2.0, z + 3.2, 1.4],
                [x - 7.5, z + 2.4, 1.4],
                [x - 9.5, z, 1.2],
            ],
            lie_secs: 2.6,
            phase: StuntPhase::Run,
            time: 0.0,
            next: 1,
            speed: 0.0,
            moved: false,
        }
    }

    fn waypoints(&self) -> &[[f32; 3]] {
        match self.phase {
            StuntPhase::Back => &self.back,
            _ => &self.run,
        }
    }
}

/// Build the yard around the paddock's `origin` and spawn the stuntman; returns him.
pub fn spawn_yard(world: &mut World, origin: DVec3) -> Entity {
    let [x, z] = YARD;
    let at = |dx: f64, y: f64, dz: f64| origin + DVec3::new(x + dx, y, z + dz);
    let wood = StandardMaterial::preset(MaterialPreset::WoodPlanks).with_triplanar_world();
    let stone = StandardMaterial::preset(MaterialPreset::Concrete).with_triplanar_world();
    let solid = |world: &mut World, t: Transform, half: Vec3| {
        world.spawn((
            t,
            GlobalTransform::from(t),
            RigidBody::fixed(),
            Collider::cuboid(half),
        ));
    };
    // The paddock floor under the yard, flush with the pad (the terrain may have no collider
    // here).
    solid(
        world,
        Transform::from_translation(at(0.0, -0.25, 1.0)),
        Vec3::new(8.0, 0.25, 6.0),
    );

    // The deck: its top at DECK_HEIGHT, its east edge at the yard's x.
    let h = DECK_HEIGHT;
    let deck = Transform::from_translation(at(-1.5, h as f64 * 0.5, 0.0));
    world.spawn((
        deck,
        GlobalTransform::from(deck),
        IkSurface {
            shape: IkShape::Box,
            half_extents: [1.5, h * 0.5, 1.5],
            surface: 12,
        },
        MeshRenderer::new(
            MeshShape::Cuboid {
                size: [3.0, h, 3.0],
            },
            wood,
        ),
        RigidBody::fixed(),
        Collider::cuboid(Vec3::new(1.5, h * 0.5, 1.5)),
    ));
    // Stairs up its west side, rising toward +X.
    let steps = 8;
    let stairs = Transform::from_translation(at(-4.5, h as f64 * 0.5, 0.0))
        .with_rotation(Quat::from_rotation_y(-PI * 0.5));
    let surface = IkSurface {
        shape: IkShape::Stairs { steps },
        half_extents: [1.0, h * 0.5, 1.5],
        surface: 2,
    };
    world.spawn((stairs, GlobalTransform::from(stairs), surface));
    let [hx, hy, hz] = surface.half_extents;
    let depth = 2.0 * hz / steps as f32;
    for k in 0..steps {
        // Step k spans local z from hz - k·depth to hz - (k+1)·depth, rising toward −Z.
        let height = 2.0 * hy * (k + 1) as f32 / steps as f32;
        let local = Vec3::new(0.0, -hy + height * 0.5, hz - (k as f32 + 0.5) * depth);
        let t = Transform {
            translation: stairs.translation + (stairs.rotation * local).as_dvec3(),
            rotation: stairs.rotation,
            scale: Vec3::ONE,
        };
        let half = Vec3::new(hx, height * 0.5, depth * 0.5);
        world.spawn((
            t,
            GlobalTransform::from(t),
            MeshRenderer::new(
                MeshShape::Cuboid {
                    size: (half * 2.0).to_array(),
                },
                stone,
            ),
            RigidBody::fixed(),
            Collider::cuboid(half),
        ));
    }
    // The crash mat east of the deck: three folding panels of blue vinyl, the creases between
    // them where their rounded edges meet.
    let mat = Transform::from_translation(at(MAT.x as f64 + 0.25, MAT.y as f64, 0.0));
    world.spawn((
        mat,
        GlobalTransform::from(mat),
        IkSurface {
            shape: IkShape::Box,
            half_extents: MAT.to_array(),
            surface: 0,
        },
        RigidBody::fixed(),
        Collider::cuboid(MAT),
    ));
    let vinyl = StandardMaterial::hex(0x618aba).with_roughness(0.55);
    let panel = MAT.x * 2.0 / 3.0;
    for k in [-1.0f32, 0.0, 1.0] {
        let t = Transform::from_translation(mat.translation + DVec3::X * (k * panel) as f64);
        world.spawn((
            t,
            GlobalTransform::from(t),
            MeshRenderer::new(
                MeshShape::RoundedCuboid {
                    size: [panel - 0.004, MAT.y * 2.0, MAT.z * 2.0],
                    radius: 0.06,
                    segments: 0,
                },
                vinyl,
            ),
        ));
    }

    let stunt = Stunt::new(origin);
    let [sx, sz, _] = stunt.run[0];
    let t = Transform::from_translation(origin + DVec3::new(sx as f64, 0.0, sz as f64))
        .with_rotation(Quat::from_rotation_y(-PI * 0.5));
    world
        .spawn((
            Name::new("Stuntman"),
            t,
            GlobalTransform::from(t),
            BodyRequest::human(2),
            stunt,
        ))
        .id()
}

#[allow(clippy::type_complexity)]
pub(crate) fn stunt_system(
    mut commands: Commands,
    time: Res<Time>,
    ground: Res<Ground>,
    physics: Option<Res<PhysicsQueries>>,
    surfaces: Res<IkSurfaces>,
    mut q: Query<(
        Entity,
        &mut Stunt,
        &mut Transform,
        Option<&mut Ragdoll>,
        Option<&SkeletonPose>,
        &GlobalTransform,
        Option<(&Animator, &RagdollRig)>,
    )>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (entity, mut stunt, mut t, ragdoll, pose, gt, rig) in &mut q {
        stunt.time += dt;
        let probe = GroundProbe {
            ground: &ground,
            physics: physics.as_deref(),
            surfaces: &surfaces.0,
            exclude: Some(entity),
        };
        let ground_at = |p: DVec3| {
            probe
                .probe(DVec3::new(p.x, p.y + 0.4, p.z), 0.8, 3.0)
                .map_or(p.y, |h| h.height)
        };
        match stunt.phase {
            StuntPhase::Run | StuntPhase::Back => {
                let points = stunt.waypoints().to_vec();
                let Some(&[wx, wz, want]) = points.get(stunt.next) else {
                    continue;
                };
                let goal = stunt.origin + DVec3::new(wx as f64, 0.0, wz as f64);
                let mut to = goal - t.translation;
                to.y = 0.0;
                let left = to.length() as f32;
                // Ease toward the waypoint's speed.
                let step = ACCELERATION * dt;
                stunt.speed += (want - stunt.speed).clamp(-step, step);
                let travel = (stunt.speed * dt).min(left);
                if left > 1e-4 {
                    let dir = (to / left as f64).as_vec3();
                    t.translation += dir.as_dvec3() * travel as f64;
                    let yaw = (-dir.x).atan2(-dir.z);
                    let turn = Quat::from_rotation_y(yaw);
                    t.rotation = t.rotation.slerp(turn, smoothing(dt, 0.12));
                }
                let y = ground_at(t.translation);
                t.translation.y += (y - t.translation.y) * smoothing(dt, 0.06) as f64;
                if left - travel < 0.05 {
                    stunt.next += 1;
                    if stunt.next >= points.len() {
                        stunt.next = 1;
                        stunt.time = 0.0;
                        if stunt.phase == StuntPhase::Run {
                            // Off the edge: limp, with the run's momentum.
                            stunt.phase = StuntPhase::Fall;
                            match ragdoll {
                                Some(mut r) => r.weight = 1.0,
                                None => {
                                    commands.entity(entity).insert(Ragdoll { weight: 1.0 });
                                }
                            }
                        } else {
                            stunt.phase = StuntPhase::Run;
                        }
                    }
                }
            }
            StuntPhase::Fall => {
                if stunt.time < stunt.lie_secs {
                    continue;
                }
                if !stunt.moved {
                    // Stand the entity where his get-up starts from the way he lies, so he
                    // rises there without sliding; or under his hips, facing the way he fell.
                    let placed = pose.zip(rig).and_then(|(pose, (animator, _))| {
                        get_up_placement(&animator.skeleton, pose, gt)
                    });
                    let hips =
                        pose.and_then(|p| p.index_of("hips").and_then(|i| p.bone_world(gt, i)));
                    if let Some((at, rotation)) = placed {
                        t.translation = DVec3::new(at.x, ground_at(at), at.z);
                        t.rotation = rotation;
                    } else if let Some((p, _)) = hips {
                        let fell = (p - t.translation).as_vec3();
                        t.translation = DVec3::new(p.x, ground_at(p), p.z);
                        if fell.x.hypot(fell.z) > 0.2 {
                            t.rotation = Quat::from_rotation_y((-fell.x).atan2(-fell.z));
                        }
                    }
                    stunt.moved = true;
                    continue;
                }
                stunt.moved = false;
                stunt.phase = StuntPhase::Rise;
                stunt.time = 0.0;
                if let Some(mut r) = ragdoll {
                    r.weight = 0.0;
                }
            }
            StuntPhase::Rise => {
                let rising = rig.is_some_and(|(_, r)| r.is_blending_back());
                if (stunt.time > 0.5 && !rising) || stunt.time > RISE_SECS {
                    stunt.phase = StuntPhase::Back;
                    stunt.time = 0.0;
                    stunt.next = 0;
                    stunt.speed = 0.0;
                }
            }
        }
    }
}
