//! Necromancy's raise dead (round-6-plan.md): the dead rise and serve.
//!
//! A Summon woven with `raise` picks what rises by where it is aimed:
//! * at a fallen body, the body itself stands as a **thrall** for the raiser (Skyrim's
//!   Reanimate: one at a time, two from Master; when it falls again or its time ends it
//!   collapses into ash);
//! * at the ground, a **skeleton warrior** climbs out with a bone blade, or with an element a
//!   **skeletal mage** that shoots bolts of it; a caster keeps a handful, more with skill
//!   (Diablo II's Raise Skeleton keeps 1 to 8);
//! * with stone, the golem rises in bone (summon.rs).
//!
//! Skeletons walk and fight through summon.rs's walker (planted feet, two-bone legs); this
//! module holds their build, their pose and their blade, and the thralls.

use std::f32::consts::PI;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{MaterialRef, MeshRef, MeshRenderer, Shape, StandardMaterial};
use engine_world::animation::{Ragdoll, SkeletonPose};
use engine_world::physics::{BodyKind, Collider, ColliderShape, RigidBody};
use engine_world::{Faction, Factions};

use crate::caster::{Aim, CastRequest, MagicCaster, SpellChoice, Vitality};
use crate::design::{CastType, SpellDesign};
use crate::looks::{LookCmd, Role};
use crate::runtime::{Allegiance, CastInfo, MagicQueue};
use crate::schools::Mastery;
use crate::summon::{Act, Gait, Minion, MinionKind, Rubble, Stance, two_bone};

// ------------------------------------------------------------------------------- numbers

/// A skeleton at size 1, about 1.85 m: upright, narrow, quick.
pub const SKELETON_GAIT: Gait = Gait {
    hip_y: 0.92,
    hip_x: 0.11,
    thigh: 0.45,
    shin: 0.44,
    ankle_y: 0.07,
    // Quick, short steps (a skittering run): a foot is down only while the other swings,
    // 0.65 m of travel, so a stride of half that each side of the hip stays in a 0.89 m leg.
    step_secs: 0.25,
    step_h: 0.16,
    step_far: 0.3,
    settle: 0.1,
    lead: 0.36,
    walk: 2.6,
    turn: 5.0,
    rise_secs: 1.2,
    rise_depth: 1.95,
};
const UPPER: f32 = 0.3;
const FORE: f32 = 0.28;
const LEAN: f32 = 0.06;

/// A slash: the whole swing, the moment it lands, and the rest after it, s.
pub const SLASH_SECS: f32 = 0.75;
pub const SLASH_HIT: f32 = 0.38;
pub const SLASH_REST: f32 = 0.35;
/// A skeletal mage keeps this far off its foe, m, and shoots this often, s.
pub const MAGE_KEEP_M: f32 = 9.0;
pub const MAGE_EVERY: f32 = 2.0;
/// How far a fallen body may lie from where a raise is aimed, m.
pub const CORPSE_M: f32 = 3.0;
/// A thrall stands at this share of its health, and casts this often, s.
pub const THRALL_HEALTH: f32 = 0.6;
pub const THRALL_EVERY: f32 = 2.2;
/// The violet of the dead's eyes.
pub const GRAVE_LIGHT: [f32; 3] = crate::combine::RAISE_COLOR;
/// Grave dust and ash, grey-brown.
pub const GRAVE_DUST: [f32; 3] = [0.45, 0.42, 0.38];

/// How near a skeleton walks to what it fights, m (arm, blade and the foe's girth).
pub fn reach(k: f32) -> f32 {
    0.95 * k + 0.35
}

/// Where a skeletal mage's casting hand is, under its root (size `k`).
pub fn mage_hand(k: f32) -> Vec3 {
    Vec3::new(0.18, 1.38, -0.5) * k
}

// ------------------------------------------------------------------------------- the body

/// The skeleton's pieces, in order.
pub const PELVIS: usize = 0;
pub const SPINE: [usize; 5] = [1, 2, 3, 4, 5];
pub const RIBS: [usize; 4] = [6, 7, 8, 9];
pub const STERNUM: usize = 10;
pub const COLLAR: usize = 11;
pub const SKULL: usize = 12;
pub const JAW: usize = 13;
pub const EYES: [usize; 2] = [14, 15];
/// Thigh, shin, foot; left then right.
pub const LEG: [usize; 2] = [16, 19];
/// Upper arm, forearm, hand; left then right.
pub const ARM: [usize; 2] = [22, 25];
/// A warrior's blade, or a mage's glow in its hand.
pub const HELD: usize = 28;
pub const SKELETON_PARTS: usize = 29;

/// Rib hoops from the top: height over the pelvis and radius, m at size 1.
const RIB_HOOPS: [(f32, f32); 4] = [(0.5, 0.12), (0.43, 0.15), (0.36, 0.16), (0.29, 0.145)];

/// Bone: ivory, a little waxy.
pub fn bone_material() -> StandardMaterial {
    StandardMaterial::default()
        .with_tint(0.86, 0.8, 0.66)
        .with_roughness(0.55)
}

fn piece(shape: Shape, material: StandardMaterial) -> MeshRenderer {
    MeshRenderer::new(MeshRef::Shape(shape), MaterialRef::Inline(material))
}

fn glow(radius: f32, color: [f32; 3], nits: f32) -> MeshRenderer {
    MeshRenderer::new(
        MeshRef::Shape(Shape::Sphere {
            radius,
            segments: 12,
        }),
        MaterialRef::Inline(
            StandardMaterial::default()
                .with_tint(color[0], color[1], color[2])
                .with_emissive(color, nits),
        ),
    )
    .without_shadows()
}

/// A skeleton's pieces at size `k`: (index, size for its rubble box, mesh). Glowing pieces
/// have no size (they go out rather than fall).
pub fn skeleton_parts(k: f32, color: [f32; 3], mage: bool) -> Vec<(usize, Vec3, MeshRenderer)> {
    let bone = bone_material();
    let capsule = |r: f32, h: f32| Shape::Capsule {
        radius: r * k,
        height: h * k,
        segments: 10,
    };
    let cuboid = |x: f32, y: f32, z: f32| Shape::Cuboid {
        size: [x * k, y * k, z * k],
    };
    let mut out = vec![(
        PELVIS,
        Vec3::new(0.3, 0.12, 0.16) * k,
        piece(
            Shape::RoundedCuboid {
                size: [0.3 * k, 0.12 * k, 0.16 * k],
                radius: 0.04 * k,
                segments: 3,
            },
            bone,
        ),
    )];
    for i in SPINE {
        out.push((
            i,
            Vec3::splat(0.06) * k,
            piece(
                Shape::Cylinder {
                    radius: 0.04 * k,
                    height: 0.07 * k,
                    segments: 8,
                },
                bone,
            ),
        ));
    }
    for (j, i) in RIBS.iter().enumerate() {
        let r = RIB_HOOPS[j].1;
        out.push((
            *i,
            Vec3::new(2.0 * r, 0.04, 1.6 * r) * k,
            piece(
                Shape::Torus {
                    radius: r * k,
                    tube: 0.022 * k,
                    segments: 20,
                },
                bone,
            ),
        ));
    }
    out.push((
        STERNUM,
        Vec3::new(0.04, 0.22, 0.03) * k,
        piece(cuboid(0.04, 0.22, 0.025), bone),
    ));
    out.push((
        COLLAR,
        Vec3::new(0.05, 0.42, 0.05) * k,
        piece(capsule(0.028, 0.42), bone),
    ));
    out.push((
        SKULL,
        Vec3::splat(0.2) * k,
        piece(
            Shape::Sphere {
                radius: 0.105 * k,
                segments: 16,
            },
            bone,
        ),
    ));
    out.push((
        JAW,
        Vec3::new(0.1, 0.04, 0.09) * k,
        piece(cuboid(0.1, 0.04, 0.09), bone),
    ));
    for i in EYES {
        out.push((i, Vec3::ZERO, glow(0.026 * k, color, 90.0)));
    }
    for leg in LEG {
        out.push((
            leg,
            Vec3::new(0.07, SKELETON_GAIT.thigh, 0.07) * k,
            piece(capsule(0.045, SKELETON_GAIT.thigh), bone),
        ));
        out.push((
            leg + 1,
            Vec3::new(0.06, SKELETON_GAIT.shin, 0.06) * k,
            piece(capsule(0.038, SKELETON_GAIT.shin), bone),
        ));
        out.push((
            leg + 2,
            Vec3::new(0.07, 0.04, 0.2) * k,
            piece(cuboid(0.07, 0.04, 0.2), bone),
        ));
    }
    for arm in ARM {
        out.push((
            arm,
            Vec3::new(0.05, UPPER, 0.05) * k,
            piece(capsule(0.036, UPPER), bone),
        ));
        out.push((
            arm + 1,
            Vec3::new(0.045, FORE, 0.045) * k,
            piece(capsule(0.03, FORE), bone),
        ));
        out.push((
            arm + 2,
            Vec3::new(0.06, 0.1, 0.03) * k,
            piece(cuboid(0.06, 0.1, 0.025), bone),
        ));
    }
    if mage {
        out.push((HELD, Vec3::ZERO, glow(0.07 * k, color, 45.0)));
    } else {
        // A notched blade of bone, darker at the grip.
        out.push((
            HELD,
            Vec3::new(0.05, 0.75, 0.015) * k,
            piece(
                cuboid(0.055, 0.75, 0.014),
                bone_material()
                    .with_tint(0.74, 0.68, 0.55)
                    .with_roughness(0.4),
            ),
        ));
    }
    out
}

/// A skeleton's body for physics once it stands: a thin capsule from knee to neck.
pub fn skeleton_collider(k: f32) -> (Collider, RigidBody) {
    (
        Collider {
            shape: ColliderShape::Compound(vec![(
                Vec3::new(0.0, 1.0 * k, 0.0),
                Quat::IDENTITY,
                ColliderShape::Capsule {
                    radius: 0.24 * k,
                    half_height: 0.55 * k,
                },
            )]),
            density: 600.0,
            ..Default::default()
        },
        RigidBody {
            kind: BodyKind::Kinematic,
            ..Default::default()
        },
    )
}

// ------------------------------------------------------------------------------- the pose

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn ramp(a: f32, b: f32, t: f32) -> f32 {
    smooth((t - a) / (b - a).max(1e-4))
}

/// The skeleton's stance for an act (size 1, root axes), with its feet (root-local) driving
/// the arm swing, bob and sway. A warrior carries its blade low and forward; a mage holds its
/// glowing hand out.
pub fn skeleton_stance(
    act: Act,
    feet: [Vec3; 2],
    stepping: Option<(usize, f32)>,
    dip: f32,
    mage: bool,
) -> Stance {
    let reach = UPPER + FORE;
    let swing = |other: Vec3| (0.9 * other.z).clamp(-0.4, 0.4);
    let mut hang = [
        Vec3::new(-0.05, -reach * 0.92, swing(feet[1])),
        Vec3::new(0.05, -reach * 0.92, swing(feet[0])),
    ];
    if mage {
        hang[1] = Vec3::new(0.08, -0.22, -0.36);
    } else {
        hang[1] = Vec3::new(0.08, -0.42, -0.22 + 0.5 * swing(feet[0]));
    }
    let (lift, sway) = match stepping {
        Some((i, t)) => {
            let s = (PI * t).sin();
            (s, if i == 0 { 0.03 } else { -0.03 } * s)
        }
        None => (0.0, 0.0),
    };
    let mut st = Stance {
        twist: 0.0,
        lunge: 0.0,
        // Leaning into the run.
        pitch: if stepping.is_some() { 0.16 } else { LEAN },
        bob: 0.035 * lift - 0.02 - dip,
        sway,
        fists: hang,
        eyes: 1.0,
    };
    match act {
        Act::Rise { t } => {
            // Clawing up over the rim: both hands overhead first, the eyes lighting at the top.
            let rise = SKELETON_GAIT.rise_secs;
            let up = 1.0 - ramp(rise * 0.55, rise, t);
            for (i, f) in st.fists.iter_mut().enumerate() {
                let side = if i == 0 { -1.0 } else { 1.0 };
                let claw = 0.08 * (t * 14.0 + i as f32 * PI).sin();
                *f = f.lerp(Vec3::new(0.12 * side, 0.5 + claw, -0.3), up);
            }
            st.pitch = 0.35 * up;
            st.eyes = ramp(rise * 0.6, rise, t);
        }
        Act::Slash { t } => {
            let cock = ramp(0.0, 0.3, t) * (1.0 - ramp(0.3, 0.42, t));
            let cut = ramp(0.3, 0.42, t) * (1.0 - ramp(0.55, SLASH_SECS, t));
            st.twist = 0.45 * cock - 0.55 * cut;
            st.lunge = -0.14 * cut;
            st.pitch += 0.18 * cut;
            let cocked = Vec3::new(0.2, 0.42, 0.06);
            let cleft = Vec3::new(-0.36, -0.4, -0.42);
            st.fists[1] = st.fists[1].lerp(cocked, cock).lerp(cleft, cut);
            let guard = Vec3::new(-0.04, -0.3, -0.28);
            st.fists[0] = st.fists[0].lerp(guard, cock.max(cut));
        }
        Act::Shoot { t } => {
            let out = ramp(0.0, 0.25, t);
            st.fists[1] = st.fists[1].lerp(Vec3::new(0.02, -0.02, -0.56), out);
            st.fists[0] = st.fists[0].lerp(Vec3::new(-0.1, -0.25, -0.3), out);
            st.pitch = LEAN;
        }
        Act::Ready | Act::Punch { .. } | Act::Slam { .. } => {}
    }
    st
}

/// Each piece's place, turn and scale under the skeleton's root (m at size `k`), from its
/// stance and its ankles (root-local, at size `k`).
pub fn pose_skeleton(st: &Stance, ankles: [Vec3; 2], k: f32) -> Vec<(Vec3, Quat, Vec3)> {
    let g = &SKELETON_GAIT;
    let mut out = vec![(Vec3::ZERO, Quat::IDENTITY, Vec3::ONE); SKELETON_PARTS];
    let ry = Quat::from_rotation_y;
    let pelvis = Vec3::new(st.sway, g.hip_y + st.bob, st.lunge) * k;
    let r_pelvis = ry(st.twist * 0.35);
    let r_torso = ry(st.twist) * Quat::from_rotation_x(-st.pitch);
    let r_head = r_torso * Quat::from_rotation_x(0.6 * st.pitch);
    let at = |p: Vec3| pelvis + r_torso * p * k;
    out[PELVIS] = (pelvis, r_pelvis, Vec3::ONE);
    for (i, s) in SPINE.iter().enumerate() {
        out[*s] = (
            at(Vec3::new(0.0, 0.08 + 0.105 * i as f32, 0.05)),
            r_torso,
            Vec3::ONE,
        );
    }
    for (j, r) in RIBS.iter().enumerate() {
        // Hoops a little deeper than wide... no: wider than deep, as a chest is.
        out[*r] = (
            at(Vec3::new(0.0, RIB_HOOPS[j].0, -0.04)),
            r_torso,
            Vec3::new(1.0, 1.0, 0.72),
        );
    }
    out[STERNUM] = (
        at(Vec3::new(0.0, 0.4, -0.15)),
        r_torso * Quat::from_rotation_x(0.12),
        Vec3::ONE,
    );
    out[COLLAR] = (
        at(Vec3::new(0.0, 0.57, -0.01)),
        r_torso * Quat::from_rotation_z(PI * 0.5),
        Vec3::ONE,
    );
    let head = at(Vec3::new(0.0, 0.76, -0.02));
    out[SKULL] = (head, r_head, Vec3::new(0.92, 1.05, 1.12));
    out[JAW] = (
        head + r_head * Vec3::new(0.0, -0.095, -0.045) * k,
        r_head,
        Vec3::ONE,
    );
    let bone = |a: Vec3, b: Vec3| {
        (
            (a + b) * 0.5,
            Quat::from_rotation_arc(Vec3::Y, (b - a).normalize_or(Vec3::NEG_Y)),
            Vec3::ONE,
        )
    };
    for i in 0..2 {
        let side = if i == 0 { -1.0 } else { 1.0 };
        let hip = pelvis + r_pelvis * Vec3::new(side * g.hip_x, -0.04, 0.0) * k;
        let (knee, ankle) = two_bone(
            hip,
            ankles[i],
            g.thigh * k,
            g.shin * k,
            Vec3::new(0.1 * side, 0.0, -1.0),
        );
        out[LEG[i]] = bone(hip, knee);
        out[LEG[i] + 1] = bone(knee, ankle);
        out[LEG[i] + 2] = (
            ankle + Vec3::new(0.0, -0.035, -0.06) * k,
            Quat::IDENTITY,
            Vec3::ONE,
        );
        let shoulder = at(Vec3::new(side * 0.2, 0.56, 0.0));
        let (elbow, wrist) = two_bone(
            shoulder,
            shoulder + st.fists[i] * k,
            UPPER * k,
            FORE * k,
            Vec3::new(0.4 * side, -0.3, 1.0),
        );
        out[ARM[i]] = bone(shoulder, elbow);
        out[ARM[i] + 1] = bone(elbow, wrist);
        let fore = (wrist - elbow).normalize_or(Vec3::NEG_Y);
        let turn = Quat::from_rotation_arc(Vec3::Y, fore);
        out[ARM[i] + 2] = (wrist + fore * 0.05 * k, turn, Vec3::ONE);
        // Points of light set a little proud of the face, where the sockets would be.
        out[EYES[i]] = (
            head + r_head * Vec3::new(side * 0.038, 0.012, -0.1) * k,
            r_head,
            Vec3::splat(st.eyes.max(0.001)),
        );
        if i == 1 {
            // The blade runs on from the forearm, tipped a little up off it; a mage's glow sits
            // in its palm.
            let blade = (fore + r_torso * Vec3::new(0.0, 0.35, -0.25)).normalize_or(fore);
            out[HELD] = (
                wrist + blade * (0.1 + 0.375) * k,
                Quat::from_rotation_arc(Vec3::Y, blade),
                Vec3::ONE,
            );
        }
    }
    out
}

// ------------------------------------------------------------------------------- thralls

/// A fallen body raised to fight for its raiser: it stands at [`THRALL_HEALTH`], takes the
/// raiser's side, casts at its foes, and collapses into ash when it falls again or its time
/// is up (Skyrim's Reanimate).
#[derive(Component, Debug, Clone)]
pub struct Thrall {
    pub cast: CastInfo,
    pub age: f32,
    pub life: f32,
    pub cooldown: f32,
    pub target: Option<Entity>,
}

/// A violet glow in a thrall's eye, kept on its head by [`run_thralls`].
#[derive(Component, Debug, Clone, Copy)]
pub struct ThrallEye {
    pub owner: Entity,
    /// -1 left, +1 right.
    pub side: f32,
}

/// Where a standing body's eye is from its head bone (root axes), and the height over its feet
/// above which the head counts as up (a get-up is over), m.
const EYE_FROM_HEAD: Vec3 = Vec3::new(0.033, 0.075, -0.095);
const HEAD_UP_M: f64 = 1.35;

/// How many thralls a caster keeps: one, two from Master up (Twin Souls).
pub fn thrall_limit(mastery: Mastery) -> usize {
    crate::summon::limit(MinionKind::Golem, mastery)
}

/// A fallen body near `at` (or `on` itself): down, whole, not already raised or summoned.
pub fn corpse_near<'a>(
    on: Option<Entity>,
    at: DVec3,
    bodies: impl Iterator<Item = (Entity, DVec3, bool)> + Clone + 'a,
) -> Option<Entity> {
    if let Some(e) = on
        && bodies.clone().any(|(b, _, down)| b == e && down)
    {
        return Some(e);
    }
    bodies
        .filter(|(_, p, down)| {
            let d = *p - at;
            *down && DVec3::new(d.x, 0.0, d.z).length() <= CORPSE_M as f64
        })
        .min_by(|a, b| a.1.distance(at).total_cmp(&b.1.distance(at)))
        .map(|(e, ..)| e)
}

/// Raise summons: a fallen body near the aim stands as a thrall; otherwise bones climb out of
/// the ground (a skeleton, through the summon flush).
#[allow(clippy::type_complexity)]
pub fn raise_dead(
    mut queue: ResMut<MagicQueue>,
    mut bodies: Query<
        (
            Entity,
            &GlobalTransform,
            &mut Vitality,
            Option<&mut MagicCaster>,
        ),
        (Without<Minion>, Without<Thrall>),
    >,
    mut thralls: Query<&mut Thrall>,
    mut commands: Commands,
) {
    if queue.raises.is_empty() {
        return;
    }
    for nm in std::mem::take(&mut queue.raises) {
        let seen: Vec<(Entity, DVec3, bool)> = bodies
            .iter()
            .map(|(e, g, v, _)| (e, g.translation(), v.down()))
            .collect();
        // Plain raise only: with an element or stone it calls bones from the ground.
        let corpse = if nm.kind == MinionKind::Skeleton {
            corpse_near(nm.on, nm.at, seen.iter().copied())
        } else {
            None
        };
        let Some(body) = corpse else {
            queue.minions.push(nm);
            continue;
        };
        tracing::debug!("magic: a raise stands {body:?} up as a thrall");
        let Ok((e, g, mut v, caster)) = bodies.get_mut(body) else {
            continue;
        };
        // Past the limit, the oldest thrall of this raiser turns to dust.
        let mut mine: Vec<Mut<Thrall>> = thralls
            .iter_mut()
            .filter(|t| t.cast.caster == nm.cast.caster && t.age < t.life)
            .collect();
        mine.sort_by(|a, b| b.age.total_cmp(&a.age));
        let keep = thrall_limit(nm.cast.spell.mastery).saturating_sub(1);
        for (i, t) in mine.iter_mut().enumerate() {
            if i >= keep {
                t.life = t.age;
            }
        }
        v.health = v.max * THRALL_HEALTH;
        v.down_secs = 0.0;
        v.revive_secs = 0.0;
        match caster {
            Some(mut c) => {
                c.side = nm.cast.side;
                c.max_mana = c.max_mana.max(400.0);
                c.mana = c.max_mana;
                c.mana_regen = c.mana_regen.max(60.0);
            }
            None => {
                commands.entity(e).insert(MagicCaster {
                    learns: false,
                    side: nm.cast.side,
                    max_mana: 400.0,
                    mana: 400.0,
                    mana_regen: 60.0,
                    ..MagicCaster::new(40.0, Mastery::Adept)
                });
            }
        }
        match nm.cast.faction {
            Some(f) => {
                commands.entity(e).insert(f);
            }
            None => {
                commands.entity(e).remove::<Faction>();
            }
        }
        // Up off the floor, with the grave's light in its eyes and the shadow on it.
        commands.entity(e).insert((
            Ragdoll { weight: 0.0 },
            Thrall {
                cast: nm.cast.clone(),
                age: 0.0,
                life: nm.cast.spell.carrier.duration.max(1.0),
                cooldown: 1.4,
                target: None,
            },
        ));
        // Its eyes follow its head (run_thralls), lit once it stands.
        for side in [-1.0, 1.0] {
            commands.spawn((
                Name::new("Thrall eye"),
                Transform::from_translation(g.translation()).with_scale(Vec3::ZERO),
                glow(0.022, GRAVE_LIGHT, 90.0),
                ThrallEye { owner: e, side },
            ));
        }
        let at = g.translation();
        queue.looks.push(LookCmd::Attach {
            entity: e,
            look: "shadow".into(),
            role: Role::Aura,
            scale: 1.0,
            color: GRAVE_LIGHT,
            power_j: nm.cast.spell.power.energy_j as f32 * 0.2,
        });
        queue.looks.push(LookCmd::Burst {
            at: at + DVec3::Y * 0.9,
            up: Vec3::Y,
            look: "shadow".into(),
            role: Role::Impact,
            scale: 1.1,
            color: GRAVE_LIGHT,
        });
        queue.looks.push(LookCmd::Burst {
            at,
            up: Vec3::Y,
            look: "raise".into(),
            role: Role::Impact,
            scale: 0.9,
            color: GRAVE_DUST,
        });
    }
}

/// Thralls fight: each faces the nearest foe and casts at it (its own spell when it knows one,
/// else a shadow bolt); when it falls again or its time is up it collapses into ash.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn run_thralls(
    time: Res<Time>,
    factions: Option<Res<Factions>>,
    mut thralls: Query<(
        Entity,
        &mut Thrall,
        &mut Transform,
        &GlobalTransform,
        &Vitality,
        &MagicCaster,
    )>,
    bodies: Query<(Entity, &GlobalTransform, &Vitality, Allegiance)>,
    creatures: Query<(), Or<(With<Minion>, With<Thrall>)>>,
    poses: Query<&SkeletonPose>,
    mut eyes: Query<(Entity, &ThrallEye, &mut Transform), Without<Thrall>>,
    mut casts: MessageWriter<CastRequest>,
    mut queue: ResMut<MagicQueue>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    // Eyes on their thrall's head while it stands, out while it gets up; gone with it.
    for (eye, te, mut etf) in &mut eyes {
        let Ok((_, _, _, g, ..)) = thralls.get(te.owner) else {
            commands.entity(eye).despawn();
            continue;
        };
        let head = poses
            .get(te.owner)
            .ok()
            .and_then(|p| p.bone_world(g, p.index_of("head")?));
        let (at, up) = match head {
            Some((h, _)) => (h, h.y - g.translation().y > HEAD_UP_M),
            // No animated body: a man's eye height.
            None => (g.translation() + DVec3::Y * 1.555, true),
        };
        let off = Vec3::new(te.side * EYE_FROM_HEAD.x, EYE_FROM_HEAD.y, EYE_FROM_HEAD.z);
        etf.translation = at + (g.rotation() * off).as_dvec3();
        etf.scale = Vec3::splat(if up { 1.0 } else { 0.0 });
    }
    for (e, mut t, mut tf, g, v, c) in &mut thralls {
        t.age += dt;
        t.cooldown -= dt;
        let at = g.translation();
        if v.down() || t.age >= t.life {
            collapse(e, at, &mut queue, &mut commands);
            continue;
        }
        // Still getting up off the floor (anim's get-up): it neither turns nor casts.
        let head = poses
            .get(e)
            .ok()
            .and_then(|p| p.bone_world(g, p.index_of("head")?));
        if head.is_some_and(|(h, _)| h.y - at.y < HEAD_UP_M) {
            t.cooldown = t.cooldown.max(0.6);
            continue;
        }
        let foe = bodies
            .iter()
            .filter(|(b, bg, bv, al)| {
                *b != e
                    && !bv.down()
                    && bg.translation().distance(at) <= 22.0
                    && !t
                        .cast
                        .friend(*b, al.caster, al.faction, factions.as_deref())
            })
            // Its classmates' creatures first, as minions engage minions.
            .min_by(|a, b| {
                (!creatures.contains(a.0), a.1.translation().distance(at))
                    .partial_cmp(&(!creatures.contains(b.0), b.1.translation().distance(at)))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(b, bg, ..)| (b, bg.translation()));
        t.target = foe.map(|f| f.0);
        let Some((foe, fp)) = foe else { continue };
        // Turn to it, then cast.
        let to = (fp - at).as_vec3();
        let want = (-to.x).atan2(-to.z);
        let (yaw, ..) = tf.rotation.to_euler(EulerRot::YXZ);
        let diff = (want - yaw + PI).rem_euclid(2.0 * PI) - PI;
        tf.rotation = Quat::from_rotation_y(yaw + diff.clamp(-3.0 * dt, 3.0 * dt));
        if diff.abs() < 0.35 && t.cooldown <= 0.0 {
            t.cooldown = THRALL_EVERY;
            let design = c
                .grown(c.selected)
                .filter(|d| matches!(d.cast, CastType::Bolt | CastType::Lob | CastType::Chain))
                .unwrap_or_else(|| SpellDesign::new(&["shadow"], CastType::Bolt));
            casts.write(CastRequest {
                caster: e,
                spell: SpellChoice::Design(design),
                aim: Aim::Entity(foe),
                charge: 0.0,
                gathered: false,
            });
        }
    }
}

/// A thrall's end: it falls into a heap of ash, which settles and sinks.
fn collapse(e: Entity, at: DVec3, queue: &mut MagicQueue, commands: &mut Commands) {
    // Particles' ash pours down from about 0.9 m over the floor point it is given.
    queue.looks.push(LookCmd::Burst {
        at,
        up: Vec3::Y,
        look: "ash".into(),
        role: Role::Impact,
        scale: 1.0,
        color: [0.3, 0.29, 0.3],
    });
    queue.looks.push(LookCmd::Burst {
        at: at + DVec3::Y * 1.0,
        up: Vec3::Y,
        look: "shadow".into(),
        role: Role::Impact,
        scale: 0.8,
        color: GRAVE_LIGHT,
    });
    let ash = StandardMaterial::default()
        .with_tint(0.2, 0.19, 0.19)
        .with_roughness(1.0);
    commands.spawn((
        Name::new("Ash heap"),
        Transform::from_translation(at),
        MeshRenderer::new(
            MeshRef::Shape(Shape::Rock {
                size: [0.9, 0.22, 0.6],
                seed: (at.x.to_bits() ^ at.z.to_bits().rotate_left(13)) as u32 % 100_000,
                detail: 2,
            }),
            MaterialRef::Inline(ash),
        ),
        Rubble {
            age: 0.0,
            life: 6.0,
        },
    ));
    commands.entity(e).despawn();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summon::stance;

    #[test]
    fn a_skeleton_stands_about_a_man_high_and_its_blade_cuts_down() {
        let g = &SKELETON_GAIT;
        let rest = [
            Vec3::new(-g.hip_x, g.ankle_y, 0.0),
            Vec3::new(g.hip_x, g.ankle_y, 0.0),
        ];
        let pose = |act| pose_skeleton(&skeleton_stance(act, rest, None, 0.0, false), rest, 1.0);
        let idle = pose(Act::Ready);
        let top = idle[SKULL].0.y + 0.11;
        assert!((1.7..2.0).contains(&top), "{top}");
        // The legs reach the feet: the shin's lower end (its +Y runs knee to ankle) is near
        // the ankle.
        let shin = idle[LEG[0] + 1];
        let lower = shin.0 + shin.1 * Vec3::Y * g.shin * 0.5;
        assert!(lower.distance(rest[0]) < 0.02, "{lower}");
        let cocked = pose(Act::Slash { t: 0.3 })[HELD].0;
        let cut = pose(Act::Slash { t: SLASH_HIT })[HELD].0;
        assert!(cocked.y > idle[HELD].0.y + 0.4, "{cocked}");
        assert!(cut.y < cocked.y - 0.5, "{cut} vs {cocked}");
        assert!(cut.z < -0.2, "forward: {cut}");
        // The golem's stance is untouched by it.
        let _ = stance(Act::Ready, rest, None, 0.0);
    }

    #[test]
    fn a_raise_finds_the_fallen_body_near_its_aim() {
        let a = Entity::from_raw_u32(1).unwrap();
        let b = Entity::from_raw_u32(2).unwrap();
        let c = Entity::from_raw_u32(3).unwrap();
        let bodies = [
            (a, DVec3::new(0.0, 0.0, 0.0), false),
            (b, DVec3::new(1.5, 0.0, 0.5), true),
            (c, DVec3::new(9.0, 0.0, 0.0), true),
        ];
        assert_eq!(
            corpse_near(None, DVec3::ZERO, bodies.iter().copied()),
            Some(b)
        );
        assert_eq!(
            corpse_near(Some(c), DVec3::ZERO, bodies.iter().copied()),
            Some(c)
        );
        assert_eq!(
            corpse_near(None, DVec3::new(20.0, 0.0, 0.0), bodies.iter().copied()),
            None
        );
        // A standing body is no corpse.
        assert_eq!(
            corpse_near(Some(a), DVec3::new(30.0, 0.0, 0.0), bodies.iter().copied()),
            None
        );
    }
}
