//! Fighters on the character contract (`engine_world::character`), ready to wear the faces
//! and bodies a character module draws.
//!
//! * **Looks.** Each fighter asks for its look on its first step: an `AppearanceRequest` with
//!   its own [`Seed`], its squad (with its faction) as the family seed, so squad mates look
//!   related, a sex picked from the seed, and an age range by its skill (a rookie 18–26, a
//!   master 34–52). The character module answers with an `Appearance`; one already there is
//!   kept.
//! * **Pose and face.** One writer per entity: on fighters without a `BodyRequest` (whose
//!   pose the animation module writes), this module writes `SkeletonPose` every step from the
//!   stand-in's pose, on the animation module's humanoid skeleton (its bone names and parents,
//!   shared by every fighter through an `Arc`), and `FaceControls`: ARKit weights for blinks,
//!   pain, effort, fear and panting, and the eyes' gaze in the head. Fighters without a
//!   stand-in have no pose to write.
//! * **Bodies drawn elsewhere.** While a character module draws appearances
//!   (`AppearanceBodies` exists, checked every step), a fighter with an `Appearance` (or
//!   asking for one) has its stand-in body hidden; what it holds stays (the weapon, the
//!   shield, a magazine or arrow in hand). Without the resource nothing changes.
//! * **Wear.** Blows and falls wear the look of a fighter with an `Appearance`: blood (cuts,
//!   stabs, shots, burns less), bruises (blunt blows and falls) and dirt (going down, lying
//!   on the ground, kneeling by a mate, dashing). A fighter sent home fresh is clean again;
//!   `wet` is the weather's.
//!
//! Where the bones go: the skeleton's joints that the stand-in has are put on it exactly
//! (`spine` on the waist, `neck` on the neck, `upper_arm_*` on the shoulders, `lower_arm_*` on
//! the elbows, `hand_*` on the wrists, `upper_leg_*` on the hips, `lower_leg_*` on the knees,
//! `foot_*` on the ankles), each turned as the segment it starts. Limbs hang along their −Y
//! and hinge about +X, feet are flat with the toes along −Z: the animation module's rest pose
//! has straight legs and level feet unturned and an arm hanging 30° out turned 30° about Z,
//! and so does this pose. The other bones sit at the animation module's offsets in their
//! parent's frame: `hips` 0.07 m above the hip joints, `chest` 0.18 m up from the waist,
//! `head` 0.10 m above the neck, the clavicles 0.155 m in from the shoulders, the toes and the
//! ponytail as in its rest pose. The limbs are the stand-in's own length (thigh 0.45 m, shin
//! 0.44 m, upper arm 0.30 m, forearm 0.26 m), a little longer than the animation module's.

use std::collections::HashSet;
use std::sync::{Arc, LazyLock};

use engine_core::prelude::*;
use engine_world::animation::{BodyRequest, BonePose, SkeletonPose};
use engine_world::character::{ARKIT, Appearance, AppearanceRequest, FaceControls, arkit};
use engine_world::locomotion::{LocomotionMode, LocomotionState};
use engine_world::{DamageEvent, DamageKind};

use super::StandInState;
use super::humanoid::{Bone, Expression, Skeleton};
use crate::agent::{Npc, NpcAction, NpcBrain, NpcClock, NpcSettings, NpcVitals};
use crate::faction::{Faction, Squad};
use crate::seed::Seed;
use crate::skills::NpcSkills;
use crate::wildlife::Animal;

/// The animation module's humanoid skeleton: each bone and its parent, in its order.
pub(crate) const HUMANOID_BONES: [(&str, Option<&str>); 25] = [
    ("root", None),
    ("hips", Some("root")),
    ("spine", Some("hips")),
    ("chest", Some("spine")),
    ("neck", Some("chest")),
    ("head", Some("neck")),
    ("ponytail_1", Some("head")),
    ("ponytail_2", Some("ponytail_1")),
    ("ponytail_3", Some("ponytail_2")),
    ("clavicle_l", Some("chest")),
    ("upper_arm_l", Some("clavicle_l")),
    ("lower_arm_l", Some("upper_arm_l")),
    ("hand_l", Some("lower_arm_l")),
    ("upper_leg_l", Some("hips")),
    ("lower_leg_l", Some("upper_leg_l")),
    ("foot_l", Some("lower_leg_l")),
    ("toe_l", Some("foot_l")),
    ("clavicle_r", Some("chest")),
    ("upper_arm_r", Some("clavicle_r")),
    ("lower_arm_r", Some("upper_arm_r")),
    ("hand_r", Some("lower_arm_r")),
    ("upper_leg_r", Some("hips")),
    ("lower_leg_r", Some("upper_leg_r")),
    ("foot_r", Some("lower_leg_r")),
    ("toe_r", Some("foot_r")),
];

/// Bone names and parents.
type Names = (Arc<[String]>, Arc<[Option<u16>]>);

/// The names and parents every fighter's `SkeletonPose` shares.
static NAMES: LazyLock<Names> = LazyLock::new(|| {
    let names = HUMANOID_BONES.iter().map(|(n, _)| n.to_string()).collect();
    let parents = HUMANOID_BONES
        .iter()
        .map(|(_, p)| {
            p.map(|p| {
                let i = HUMANOID_BONES.iter().position(|(n, _)| *n == p);
                i.expect("parent listed") as u16
            })
        })
        .collect();
    (names, parents)
});

/// The animation module's offsets for the bones the stand-in has no joint for (see the module
/// docs): `hips` from between the hip joints, `chest` from the waist, `head` from the neck,
/// the ponytail from the head, a clavicle in from its shoulder (right; the left's x mirrored)
/// and a toe from its ankle.
const HIPS: Vec3 = Vec3::new(0.0, 0.07, 0.0);
const CHEST: Vec3 = Vec3::new(0.0, 0.18, 0.0);
const HEAD: Vec3 = Vec3::new(0.0, 0.10, 0.0);
const PONYTAIL: [Vec3; 3] = [
    Vec3::new(0.0, 0.14, 0.095),
    Vec3::new(0.0, -0.08, 0.035),
    Vec3::new(0.0, -0.09, 0.015),
];
const CLAVICLE_IN: f32 = 0.155;
const TOE: Vec3 = Vec3::new(0.0, -0.05, -0.14);

fn bone(at: Vec3, rot: Quat) -> BonePose {
    BonePose {
        translation: at,
        rotation: rot,
        scale: Vec3::ONE,
    }
}

/// The stand-in's skeleton as the humanoid skeleton's bones, in [`HUMANOID_BONES`] order
/// (model space).
fn bones(s: &Skeleton) -> Vec<BonePose> {
    let on = |b: &Bone, p: Vec3| bone(b.at(p), b.rot);
    let mut out = Vec::with_capacity(HUMANOID_BONES.len());
    out.push(bone(Vec3::ZERO, Quat::IDENTITY));
    out.push(on(&s.pelvis, HIPS));
    out.push(bone(s.chest.pos, s.chest.rot));
    out.push(on(&s.chest, CHEST));
    out.push(bone(s.head.pos, s.head.rot));
    let head = Bone {
        pos: s.head.at(HEAD),
        rot: s.head.rot,
    };
    out.push(on(&head, Vec3::ZERO));
    let mut tail = Vec3::ZERO;
    for p in PONYTAIL {
        tail += p;
        out.push(on(&head, tail));
    }
    for k in 0..2 {
        let sgn = if k == 0 { -1.0 } else { 1.0 };
        let shoulder = s.chest.rot.inverse() * (s.upper[k].pos - s.chest.pos);
        let clavicle = shoulder - Vec3::X * sgn * CLAVICLE_IN;
        out.push(on(&s.chest, clavicle));
        out.push(bone(s.upper[k].pos, s.upper[k].rot));
        out.push(bone(s.fore[k].pos, s.fore[k].rot));
        out.push(bone(s.hand[k].pos, s.hand[k].rot));
        out.push(bone(s.thigh[k].pos, s.thigh[k].rot));
        out.push(bone(s.shin[k].pos, s.shin[k].rot));
        out.push(bone(s.foot[k].pos, s.foot[k].rot));
        out.push(on(&s.foot[k], TOE));
    }
    out
}

/// The face as ARKit weights, and the eyes' gaze in the head (yaw toward its left, pitch up).
fn controls(s: &Skeleton, x: &Expression, out: &mut FaceControls) {
    out.weights.clear();
    out.weights.resize(ARKIT.len(), 0.0);
    let mut both = |l: usize, r: usize, v: f32| {
        out.weights[l] = v.clamp(0.0, 1.0);
        out.weights[r] = v.clamp(0.0, 1.0);
    };
    let (pain, effort, fear) = (x.pain, x.effort, x.fear);
    both(arkit::EYE_BLINK_LEFT, arkit::EYE_BLINK_RIGHT, x.blink);
    // Pain screws the eyes up, knits and lowers the brows, wrinkles the nose and pulls the
    // mouth wide and down; effort narrows the eyes, lowers the brows and presses the lips
    // (open, it shouts); fear lifts the inner brows, widens the eyes and stretches the mouth.
    both(
        arkit::EYE_SQUINT_LEFT,
        arkit::EYE_SQUINT_RIGHT,
        (0.8 * pain).max(0.5 * effort),
    );
    both(
        arkit::CHEEK_SQUINT_LEFT,
        arkit::CHEEK_SQUINT_RIGHT,
        (0.6 * pain).max(0.4 * effort),
    );
    both(
        arkit::BROW_DOWN_LEFT,
        arkit::BROW_DOWN_RIGHT,
        (0.7 * pain).max(0.7 * effort) * (1.0 - 0.6 * fear),
    );
    both(arkit::NOSE_SNEER_LEFT, arkit::NOSE_SNEER_RIGHT, 0.7 * pain);
    both(
        arkit::MOUTH_STRETCH_LEFT,
        arkit::MOUTH_STRETCH_RIGHT,
        (0.6 * pain).max(0.5 * fear),
    );
    both(
        arkit::MOUTH_FROWN_LEFT,
        arkit::MOUTH_FROWN_RIGHT,
        0.4 * pain,
    );
    let open = (0.7 * x.jaw).clamp(0.0, 1.0);
    both(
        arkit::MOUTH_PRESS_LEFT,
        arkit::MOUTH_PRESS_RIGHT,
        0.8 * effort * (1.0 - open),
    );
    both(
        arkit::EYE_WIDE_LEFT,
        arkit::EYE_WIDE_RIGHT,
        0.7 * fear * (1.0 - pain),
    );
    out.weights[arkit::BROW_INNER_UP] = (0.9 * fear).max(0.35 * pain).min(1.0);
    // Panting: the jaw drops with each breath, the lips round on the way out.
    out.weights[arkit::JAW_OPEN] = open;
    out.set(
        "mouthFunnel",
        (0.125 * x.pant * (1.0 - x.breath)).clamp(0.0, 1.0),
    );
    let ahead = s.eyes[0] * Vec3::NEG_Z;
    out.gaze_yaw = (-ahead.x).atan2(-ahead.z);
    out.gaze_pitch = ahead.y.clamp(-1.0, 1.0).asin();
}

/// What the pose and face writer reads and writes of a fighter.
type BodyItem<'a> = (
    Entity,
    &'a StandInState,
    Option<&'a mut SkeletonPose>,
    Option<&'a mut FaceControls>,
);

/// Write each fighter's `SkeletonPose` and `FaceControls` from its stand-in's pose (not where
/// a `BodyRequest` has the animation module write the pose).
pub(crate) fn write_bodies(
    mut commands: Commands,
    mut q: Query<BodyItem, (Without<Animal>, Without<BodyRequest>)>,
) {
    for (e, st, pose, face) in &mut q {
        let Some((s, x)) = st.motion.body() else {
            continue;
        };
        match pose {
            Some(mut pose) => {
                if !Arc::ptr_eq(&pose.names, &NAMES.0) {
                    pose.names = NAMES.0.clone();
                    pose.parents = NAMES.1.clone();
                }
                pose.bones = bones(s);
            }
            None => {
                commands.entity(e).try_insert(SkeletonPose {
                    names: NAMES.0.clone(),
                    parents: NAMES.1.clone(),
                    bones: bones(s),
                });
            }
        }
        match face {
            Some(mut face) => controls(s, x, &mut face),
            None => {
                let mut face = FaceControls::default();
                controls(s, x, &mut face);
                commands.entity(e).try_insert(face);
            }
        }
    }
}

/// A well-mixed 64-bit hash.
fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// The look a fighter asks for (see the module docs).
pub(crate) fn appearance_request(
    seed: Seed,
    squad: Option<Squad>,
    faction: Option<Faction>,
    skills: Option<&NpcSkills>,
) -> AppearanceRequest {
    let family = squad.map(|s| {
        let team = faction.map_or(0, |f| u64::from(f.0));
        mix(u64::from(s.0) ^ (team << 32) ^ 0x5eed_fa31_1a00_0000)
    });
    let sex = if mix(seed.0 ^ 0x5e) & 1 == 0 {
        0.0
    } else {
        1.0
    };
    let level = skills.map_or(0.5, |s| s.level()).clamp(0.0, 1.0);
    let young = 18.0 + 16.0 * level;
    AppearanceRequest {
        seed: seed.0,
        family,
        sex: Some(sex),
        age: Some([young, young + 8.0 + 10.0 * level]),
        ancestry: None,
    }
}

type Unasked<'a> = (
    Entity,
    &'a Seed,
    Option<&'a Squad>,
    Option<&'a Faction>,
    Option<&'a NpcSkills>,
);

/// Give each new fighter its `AppearanceRequest`.
#[allow(clippy::type_complexity)]
pub(crate) fn request_appearances(
    mut commands: Commands,
    settings: Res<NpcSettings>,
    q: Query<
        Unasked,
        (
            With<Npc>,
            Without<Animal>,
            Without<AppearanceRequest>,
            Without<Appearance>,
        ),
    >,
) {
    if !settings.character_looks {
        return;
    }
    for (e, &seed, squad, faction, skills) in &q {
        let request = appearance_request(seed, squad.copied(), faction.copied(), skills);
        commands.entity(e).try_insert(request);
    }
}

/// How much a blow of each kind bloodies and bruises, per its share of full health.
fn marks(kind: DamageKind) -> (f32, f32) {
    match kind {
        DamageKind::Slash | DamageKind::Pierce | DamageKind::Ballistic => (1.2, 0.4),
        DamageKind::Blunt => (0.3, 1.5),
        DamageKind::Explosive => (0.8, 0.8),
        DamageKind::Fall => (0.2, 1.0),
        DamageKind::Fire | DamageKind::Cold | DamageKind::Magic => (0.2, 0.2),
    }
}

type WearItem<'a> = (
    Entity,
    &'a mut Appearance,
    &'a NpcVitals,
    Option<&'a NpcClock>,
    Option<&'a NpcBrain>,
    Option<&'a LocomotionState>,
);

/// Wear the look of fighters with an `Appearance` as the fight goes (see the module docs).
pub(crate) fn wear(
    mut blows: MessageReader<DamageEvent>,
    mut q: Query<WearItem, With<Npc>>,
    mut down: Local<HashSet<Entity>>,
) {
    for b in blows.read() {
        if b.amount <= 0.0 {
            continue;
        }
        let Ok((_, mut look, vitals, ..)) = q.get_mut(b.target) else {
            continue;
        };
        let share = (b.amount / vitals.max_health.max(1.0)).min(1.0);
        let (blood, bruises) = marks(b.kind);
        let w = &mut look.wear;
        w.blood = (w.blood + blood * share).min(1.0);
        w.bruises = (w.bruises + bruises * share).min(1.0);
        if b.kind == DamageKind::Fall {
            w.dirt = (w.dirt + 0.5 * share).min(1.0);
        }
    }
    for (e, mut look, vitals, clock, brain, loco) in &mut q {
        let dt = clock.map_or(0.0, |c| c.dt);
        let was_down = down.contains(&e);
        if vitals.downed != was_down {
            if vitals.downed {
                down.insert(e);
            } else {
                down.remove(&e);
            }
        }
        // Sent home fresh: clean again (the weather's wet stays).
        if was_down && !vitals.downed && vitals.fraction() >= 0.999 {
            let wet = look.wear.wet;
            look.wear = Default::default();
            look.wear.wet = wet;
            continue;
        }
        let dirt = if vitals.downed {
            if was_down { 0.05 * dt } else { 0.3 }
        } else if brain.is_some_and(|b| b.action == NpcAction::Revive && b.revive_kneel > 0.0) {
            0.03 * dt
        } else if loco.is_some_and(|l| l.mode == LocomotionMode::Dash) {
            0.2 * dt
        } else {
            0.0
        };
        if dirt > 0.0 && look.wear.dirt < 1.0 {
            look.wear.dirt = (look.wear.dirt + dirt).min(1.0);
        }
    }
    // Fighters gone from the world.
    down.retain(|&e| q.contains(e));
}
