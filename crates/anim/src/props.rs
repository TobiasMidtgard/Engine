//! Weapon props for the humanoid's weapon moves ([`crate::build::weapons`]): a sword, a round
//! shield, a great sword and a bow, built from primitives and held by the hands
//! ([`BoneAttachment`]) where the clips put the grip, plus the bow's string (from both tips
//! to the draw hand while the clip's `draw` curve is on), the arrow on it (while `nocked` is),
//! and arrows in flight.
//!
//! The shapes match what the attack sets strike with ([`Striker::sword`],
//! [`Striker::greatsword`], [`Striker::shield`]) and what a [`Shield`] blocks with.
//!
//! [`Striker::sword`]: crate::combat::Striker::sword
//! [`Striker::greatsword`]: crate::combat::Striker::greatsword
//! [`Striker::shield`]: crate::combat::Striker::shield

use engine_core::prelude::*;
use engine_render::{MeshRenderer, Shape, StandardMaterial};

use crate::animator::Animator;
use crate::attach::BoneAttachment;
use crate::build::weapons::{
    BLADE_AXIS, BLADE_EDGE, BOW_HALF, BOW_TIP_BACK, GREATSWORD_BLADE, GREATSWORD_HANDS, GRIP,
    HANDLE_AXIS, HANDLE_FACE, SHIELD_CENTER, SHIELD_RADIUS, SWORD_BLADE,
};
use crate::combat::Shield;
use crate::ik::{Reach, ReachIk};

fn steel() -> StandardMaterial {
    StandardMaterial::hex(0xc9cdd3)
        .with_metallic(1.0)
        .with_roughness(0.22)
}

fn dark_steel() -> StandardMaterial {
    StandardMaterial::hex(0x4a4d52)
        .with_metallic(1.0)
        .with_roughness(0.4)
}

fn leather() -> StandardMaterial {
    StandardMaterial::hex(0x3b2717).with_roughness(0.8)
}

/// Spawn a part held by `bone` of `person`: `shape` at `offset` turned by `rotation` in the
/// bone's frame.
fn part(
    world: &mut World,
    person: Entity,
    bone: &str,
    shape: Shape,
    material: StandardMaterial,
    offset: Vec3,
    rotation: Quat,
) -> Entity {
    let at = world
        .get::<GlobalTransform>(person)
        .map_or(DVec3::ZERO, |gt| gt.translation());
    let t = Transform::from_translation(at);
    world
        .spawn((
            t,
            GlobalTransform::from(t),
            MeshRenderer::new(shape, material),
            BoneAttachment::new(person, bone).with_offset(offset, rotation),
        ))
        .id()
}

/// A rotation taking X to `width`, Y to `length` (square to each other), Z to their cross.
fn along(width: Vec3, length: Vec3) -> Quat {
    Quat::from_mat3(&Mat3::from_cols(width, length, width.cross(length))).normalize()
}

/// A blade from `blade[0]` to `blade[1]` along the grip, `width` wide (its flat square to the
/// edge), with a crossguard, a leather grip from `grip[0]` to `grip[1]` and a pommel.
fn sword_parts(
    world: &mut World,
    person: Entity,
    blade: [f32; 2],
    width: f32,
    guard: f32,
    grip: [f32; 2],
) -> Vec<Entity> {
    let flat = along(BLADE_EDGE, BLADE_AXIS);
    let at = |d: f32| GRIP + BLADE_AXIS * d;
    let length = blade[1] - blade[0];
    // The last tenth narrows to the point.
    let body = length * 0.9;
    vec![
        part(
            world,
            person,
            "hand_r",
            Shape::Cuboid {
                size: [width, body, 0.007],
            },
            steel(),
            at(blade[0] + body * 0.5),
            flat,
        ),
        part(
            world,
            person,
            "hand_r",
            Shape::Cuboid {
                size: [width * 0.5, length - body, 0.006],
            },
            steel(),
            at(blade[0] + body + (length - body) * 0.5),
            flat,
        ),
        part(
            world,
            person,
            "hand_r",
            Shape::Cuboid {
                size: [guard, 0.024, 0.032],
            },
            dark_steel(),
            at(blade[0] - 0.012),
            flat,
        ),
        part(
            world,
            person,
            "hand_r",
            Shape::Cylinder {
                radius: 0.017,
                height: grip[1] - grip[0],
                segments: 12,
            },
            leather(),
            at((grip[0] + grip[1]) * 0.5),
            flat,
        ),
        part(
            world,
            person,
            "hand_r",
            Shape::Sphere {
                radius: 0.027,
                segments: 12,
            },
            dark_steel(),
            at(grip[0] - 0.02),
            flat,
        ),
    ]
}

/// An arming sword in `person`'s right hand.
pub fn spawn_sword(world: &mut World, person: Entity) -> Vec<Entity> {
    sword_parts(world, person, SWORD_BLADE, 0.045, 0.2, [-0.065, 0.05])
}

/// A great sword in `person`'s right hand, the left hand on the grip below it: the clips put
/// it there, and a [`ReachIk`] on the `grip` curve keeps it there on characters whose
/// proportions differ from the clips'.
pub fn spawn_greatsword(world: &mut World, person: Entity) -> Vec<Entity> {
    let parts = sword_parts(
        world,
        person,
        GREATSWORD_BLADE,
        0.055,
        0.32,
        [-GREATSWORD_HANDS - 0.08, 0.055],
    );
    // The left wrist where the left fist closes round the grip, in the right hand's frame
    // (the hands hold the grip the same way round).
    let left_fist = GRIP - BLADE_AXIS * GREATSWORD_HANDS;
    world.entity_mut(person).insert(ReachIk::new(vec![Reach {
        target_bone: "hand_r".into(),
        target_offset: left_fist - GRIP,
        pole: Vec3::new(-0.6, -1.0, 0.2),
        weight_curve: "grip".into(),
        ..Reach::arm(0)
    }]));
    parts
}

/// A round shield in `person`'s left fist (its boss over the fist, the face out over the
/// knuckles), and the [`Shield`] that blocks with it.
pub fn spawn_shield(world: &mut World, person: Entity) -> Vec<Entity> {
    let face = along(HANDLE_AXIS, HANDLE_FACE);
    let wood = StandardMaterial::hex(0x7b5634).with_roughness(0.75);
    let paint = StandardMaterial::hex(0x8c2a22).with_roughness(0.6);
    let ids = vec![
        part(
            world,
            person,
            "hand_l",
            Shape::Cylinder {
                radius: SHIELD_RADIUS,
                height: 0.016,
                segments: 32,
            },
            wood,
            SHIELD_CENTER,
            face,
        ),
        // Painted front, a little smaller, so the wooden rim shows.
        part(
            world,
            person,
            "hand_l",
            Shape::Cylinder {
                radius: SHIELD_RADIUS - 0.03,
                height: 0.004,
                segments: 32,
            },
            paint,
            SHIELD_CENTER + HANDLE_FACE * 0.009,
            face,
        ),
        part(
            world,
            person,
            "hand_l",
            Shape::Torus {
                radius: SHIELD_RADIUS,
                tube: 0.012,
                segments: 32,
            },
            dark_steel(),
            SHIELD_CENTER,
            face,
        ),
        part(
            world,
            person,
            "hand_l",
            Shape::Sphere {
                radius: 0.075,
                segments: 16,
            },
            steel(),
            SHIELD_CENTER + HANDLE_FACE * 0.005,
            face,
        ),
    ];
    world.entity_mut(person).insert(Shield::default());
    ids
}

/// Which piece of a bow's string (or the arrow on it) an entity is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StringPart {
    Upper,
    Lower,
    Arrow,
}

/// A bow string piece of `archer`'s bow, posed every frame after the pose is final: the
/// string from each tip to the nock (the draw hand while the clip's `draw` curve is on, else
/// straight between the tips), the arrow from the nock along the string's pull toward the
/// bow while the clip's `nocked` curve is on.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BowString {
    #[serde(skip)]
    pub archer: Option<Entity>,
    pub part: StringPart,
}

/// An arrow's length, meters.
pub const ARROW_LENGTH: f32 = 0.78;

/// The bow's tips in the left hand's frame.
fn bow_tips() -> [Vec3; 2] {
    let back = -HANDLE_FACE * BOW_TIP_BACK;
    [
        GRIP + HANDLE_AXIS * BOW_HALF + back,
        GRIP - HANDLE_AXIS * BOW_HALF + back,
    ]
}

fn arrow_shape() -> (Shape, StandardMaterial) {
    (
        Shape::Cylinder {
            radius: 0.0045,
            height: 1.0,
            segments: 6,
        },
        StandardMaterial::hex(0xb08a5a).with_roughness(0.6),
    )
}

/// A recurve bow in `person`'s left fist (its back out over the knuckles, the string toward
/// the archer), its string and an arrow for the draw hand.
pub fn spawn_bow(world: &mut World, person: Entity) -> Vec<Entity> {
    let wood = StandardMaterial::hex(0x5b3a1e).with_roughness(0.5);
    let back = -HANDLE_FACE;
    let mut ids = vec![part(
        world,
        person,
        "hand_l",
        Shape::Cylinder {
            radius: 0.02,
            height: 0.26,
            segments: 10,
        },
        leather(),
        GRIP,
        along(HANDLE_FACE, HANDLE_AXIS),
    )];
    // Each limb in two pieces: out from the riser bowing a little toward the target, then
    // back to the tip.
    for sign in [1.0, -1.0] {
        let riser = GRIP + HANDLE_AXIS * 0.13 * sign;
        let mid = GRIP + HANDLE_AXIS * 0.45 * sign - back * 0.02;
        let tip = GRIP + HANDLE_AXIS * BOW_HALF * sign + back * BOW_TIP_BACK;
        for (a, b, w) in [(riser, mid, 0.034), (mid, tip, 0.026)] {
            let length = (b - a).normalize();
            ids.push(part(
                world,
                person,
                "hand_l",
                Shape::Cuboid {
                    size: [w, a.distance(b), 0.012],
                },
                wood,
                (a + b) * 0.5,
                along(
                    HANDLE_AXIS.cross(length).cross(length).normalize_or(back),
                    length,
                ),
            ));
        }
    }
    let at = world
        .get::<GlobalTransform>(person)
        .map_or(DVec3::ZERO, |gt| gt.translation());
    let t = Transform::from_translation(at);
    let string = StandardMaterial::hex(0xe8e2d0).with_roughness(0.7);
    for p in [StringPart::Upper, StringPart::Lower, StringPart::Arrow] {
        let (shape, material) = match p {
            StringPart::Arrow => arrow_shape(),
            _ => (
                Shape::Cylinder {
                    radius: 0.0018,
                    height: 1.0,
                    segments: 5,
                },
                string,
            ),
        };
        let mut renderer = MeshRenderer::new(shape, material);
        renderer.cast_shadows = false;
        ids.push(
            world
                .spawn((
                    t,
                    GlobalTransform::from(t),
                    renderer,
                    BowString {
                        archer: Some(person),
                        part: p,
                    },
                ))
                .id(),
        );
    }
    ids
}

/// Where `archer`'s draw hand nocks the arrow (world), the bow's tips (world), and the draw
/// and nocked curves.
fn bow_state(animator: &Animator, gt: &GlobalTransform) -> Option<(DVec3, [DVec3; 2], f32, f32)> {
    let left = animator.model[animator.skeleton.index_of("hand_l")?];
    let right = animator.model[animator.skeleton.index_of("hand_r")?];
    let world = |p: Vec3| crate::ik::to_world(gt, p);
    let [a, b] = bow_tips().map(|t| world(left.transform_point(t)));
    let draw = animator.curve("draw");
    // The fist a hand's length past the wrist, along the forearm (the draw hand is held
    // straight).
    let lower = animator.model
        [animator.skeleton.parents[animator.skeleton.index_of("hand_r")?]? as usize]
        .t;
    let fist = world(right.t + (right.t - lower).normalize_or_zero() * -GRIP.y);
    let rest = (a + b) * 0.5;
    let nock = rest.lerp(fist, (draw as f64 * 20.0).min(1.0));
    Some((nock, [a, b], draw, animator.curve("nocked")))
}

/// Stretch a unit-tall cylinder from `a` to `b`.
fn span(t: &mut Transform, a: DVec3, b: DVec3) {
    let d = (b - a).as_vec3();
    t.translation = (a + b) * 0.5;
    t.rotation = Quat::from_rotation_arc(Vec3::Y, d.normalize_or(Vec3::Y));
    t.scale = Vec3::new(1.0, d.length().max(1e-4), 1.0);
}

/// Pose bow strings and nocked arrows (after the pose is final).
pub(crate) fn bow_string_system(
    archers: Query<(&Animator, &GlobalTransform), Without<BowString>>,
    mut q: Query<(
        &BowString,
        &mut Transform,
        &mut GlobalTransform,
        &mut MeshRenderer,
    )>,
) {
    for (piece, mut t, mut gt, mut renderer) in &mut q {
        let Some((nock, [upper, lower], _, nocked)) = piece
            .archer
            .and_then(|e| archers.get(e).ok())
            .and_then(|(a, gt)| bow_state(a, gt))
        else {
            continue;
        };
        match piece.part {
            StringPart::Upper => span(&mut t, upper, nock),
            StringPart::Lower => span(&mut t, lower, nock),
            StringPart::Arrow => {
                let visible = nocked > 0.5;
                if renderer.visible != visible {
                    renderer.visible = visible;
                }
                // From the nock toward the bow, the point a little past its back.
                let toward = ((upper + lower) * 0.5 - nock).normalize_or(DVec3::NEG_Z);
                span(&mut t, nock, nock + toward * ARROW_LENGTH as f64);
            }
        }
        *gt = GlobalTransform::from(*t);
    }
}

/// An arrow in flight: it falls under gravity and sticks where it has flown `left` meters
/// (in what it was shot at), staying a while.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Arrow {
    pub velocity: DVec3,
    pub left: f64,
    /// Seconds since it stuck.
    pub stuck: Option<f32>,
}

impl Default for Arrow {
    fn default() -> Self {
        Self {
            velocity: DVec3::ZERO,
            left: 100.0,
            stuck: None,
        }
    }
}

/// How deep a shot arrow's head sinks into what it hits, m.
const ARROW_SINKS: f64 = 0.1;

/// How long a stuck arrow stays, s.
const ARROW_STAYS: f32 = 8.0;

/// Loose an arrow from `from` at `speed` m/s toward `at`, aimed high enough to drop onto it;
/// it sticks there.
pub fn shoot_arrow(world: &mut World, from: DVec3, at: DVec3, speed: f64) -> Entity {
    let to = at - from;
    let flight = to.length() / speed;
    // Aim above it by how far it falls on the way.
    let aim = to + DVec3::Y * (0.5 * 9.81 * flight * flight);
    let velocity = aim.normalize_or(DVec3::NEG_Z) * speed;
    let (shape, material) = arrow_shape();
    let mut t = Transform::default();
    span(
        &mut t,
        from,
        from + velocity.normalize() * ARROW_LENGTH as f64,
    );
    world
        .spawn((
            Name::new("Arrow"),
            t,
            GlobalTransform::from(t),
            MeshRenderer::new(shape, material),
            Arrow {
                velocity,
                // Until its head is a hand's width into what it was shot at.
                left: (to.length() - ARROW_LENGTH as f64 + ARROW_SINKS).max(0.0),
                stuck: None,
            },
        ))
        .id()
}

/// Fly arrows, stick them, clear old ones.
pub(crate) fn fly_arrows(
    mut commands: Commands,
    time: Res<Time>,
    mut q: Query<(Entity, &mut Arrow, &mut Transform)>,
) {
    let dt = time.delta_secs() as f64;
    for (e, mut arrow, mut t) in &mut q {
        if let Some(s) = &mut arrow.stuck {
            *s += dt as f32;
            if *s > ARROW_STAYS {
                commands.entity(e).despawn();
            }
            continue;
        }
        let step = arrow.velocity * dt;
        let go = step.length().min(arrow.left);
        let dir = step.normalize_or(DVec3::NEG_Z);
        t.translation += dir * go;
        arrow.left -= go;
        arrow.velocity.y -= 9.81 * dt;
        t.rotation =
            Quat::from_rotation_arc(Vec3::Y, arrow.velocity.as_vec3().normalize_or(Vec3::Y));
        if arrow.left <= 1e-6 {
            arrow.stuck = Some(0.0);
        }
    }
}
