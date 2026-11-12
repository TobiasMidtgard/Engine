//! Fighters on the character contract (`engine_world::character`), ready to wear the faces
//! and bodies a character module draws.
//!
//! * **Looks.** Each fighter asks for its look on its first step: an `AppearanceRequest` with
//!   its own [`Seed`], its squad (with its faction) as the family seed, so squad mates look
//!   related, a sex picked from the seed, and an age range by its skill (a rookie 18–26, a
//!   master 34–52). The character module answers with an `Appearance`; one already there is
//!   kept.
//! * **Kit.** The request carries the team's kit ([`team_kit`]), so the first build is
//!   already dressed in armour layers: a quilted gambeson in undyed or earthy cloth (mail over
//!   it on a veteran or better who fights hand to hand), wool hose and tall boots, a tabard in
//!   the team's colour over chest and back, and a mail coif for the fighter's own helmet to
//!   sit over (a hood for archers and riflemen). The cloths come from its seed. A look given
//!   outright is kept.
//! * **Detail.** Each fighter is built at a `CharacterQuality` for its distance from the
//!   camera (the nearest simulation focus), re-ranked twice a second: the two nearest within
//!   5 m in close-up, up to six within 18 m in mid, the rest as a crowd (no hair, no face
//!   detail); a little farther out before it steps down, as a change rebuilds the person.
//! * **Hands.** `HandControls` from what each hand holds ([`super::humanoid::hand_controls`]):
//!   a power grip round the handle (sword grips, the carbine's pistol grip and handguard, the
//!   bow's riser, a magazine), through the shield's straps, the bow hand hooking the string or
//!   pinching the next arrow, bare fists balled; a hand off its handle relaxes.
//! * **Size.** While a character module draws the fighter and has written its `BodyFit`, the
//!   stand-in's pose (and what it holds) is stretched in height to that person's shoulder
//!   height ([`size_of`]), so a short fighter's hands meet the weapon where its arms reach and
//!   the feet stay where the gait plants them.
//! * **Pose and face.** One writer per entity: on fighters without a `BodyRequest` (whose
//!   pose the animation module writes; for an intent body of this crate's, once it has armed
//!   it), this module writes `SkeletonPose` every step from the
//!   stand-in's pose, on the animation module's humanoid skeleton (its bone names and parents,
//!   shared by every fighter through an `Arc`), and `FaceControls`: ARKit weights for blinks,
//!   pain, effort, fear and panting, and the eyes' gaze in the head. Fighters without a
//!   stand-in have no pose to write.
//! * **Bodies drawn elsewhere.** While a character module draws appearances
//!   (`AppearanceBodies` exists, checked every step), a fighter with an `Appearance` (or
//!   asking for one) has its stand-in body hidden; what it holds stays (the weapon, the
//!   shield, a magazine or arrow in hand). Where the animation module poses it from intent
//!   (`intent_body.rs`) its own props are hidden (`hide_props`) and the sword or club with its
//!   hilt goes into the drawn right hand, the shield onto the left, held as the stand-in holds
//!   them. Once the person is built (its `BodyFit`), what
//!   it wears over the clothes stays too, moved and turned with that person's joints as the
//!   character module poses them, from the stand-in's pose or the animation module's
//!   ([`worn_frames`]): the pouches on its hips (and the team's tabards,
//!   where the look has no tabard of its own), the quiver on its chest, and the helmet shell
//!   with its crest and brow light on its head where the look's headgear is `mail_coif` (or
//!   `open_helm`), drawn 9% larger about the middle of the head so it sits over the coif; the
//!   fighter's own coif is left to the look. Without the resource nothing changes.
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
use engine_world::FocusPoints;
use engine_world::animation::{BodyRequest, BonePose, SkeletonPose};
use engine_world::character::{
    ARKIT, Appearance, AppearanceBodies, AppearanceRequest, BodyFit, CharacterQuality,
    FaceControls, HandControls, Outfit, arkit,
};
use engine_world::combat::AttackWindow;
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
pub(super) const HIPS: Vec3 = Vec3::new(0.0, 0.07, 0.0);
pub(super) const CHEST: Vec3 = Vec3::new(0.0, 0.18, 0.0);
pub(super) const HEAD: Vec3 = Vec3::new(0.0, 0.10, 0.0);
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
pub(super) fn bones(s: &Skeleton) -> Vec<BonePose> {
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

/// Whether a bone of a person's skeleton is a body bone (not a finger's or a twist bone).
fn body_bone(name: &str) -> bool {
    const FINGERS: [&str; 5] = ["thumb", "index", "middle", "ring", "pinky"];
    !name.contains("twist") && !FINGERS.iter().any(|f| name.starts_with(f))
}

/// Where the person a character module draws from `fit` has its hips, chest and head joints
/// when posed by `pose` (bones in [`HUMANOID_BONES`] order), model space, as the character
/// module poses them: each bone keeps its own length and takes the pose's turn from its rest,
/// the hips go where the pose puts them, and the body is moved so its feet land on the pose's
/// feet. `None` for a skeleton without those joints.
#[cfg(test)]
fn drawn_joints(fit: &BodyFit, pose: &[BonePose]) -> Option<[Vec3; 3]> {
    let ours = |name: &str| {
        let i = HUMANOID_BONES.iter().position(|(b, _)| *b == name)?;
        pose.get(i).copied()
    };
    drawn_frames(fit, &ours).map(|f| [f[0].0, f[1].0, f[2].0])
}

/// Where the person drawn from `fit` has its hips, chest and head joints (model space) when
/// posed by a pose whose bones `ours` finds by name, as the character module poses them (each
/// bone keeps its own length and takes the pose's turn from its rest, the hips go where the
/// pose puts them, and the body is moved so its feet land on the pose's feet), with each
/// joint's bone as the pose turns it. `None` for a skeleton without those joints.
fn drawn_frames(
    fit: &BodyFit,
    ours: &dyn Fn(&str) -> Option<BonePose>,
) -> Option<[(Vec3, Quat); 5]> {
    let n = fit.names.len().min(fit.rest.len()).min(fit.parents.len());
    let mut turn = vec![Quat::IDENTITY; n];
    let mut at = vec![Vec3::ZERO; n];
    let mut posed = vec![false; n];
    for b in 0..n {
        let name = fit.names[b].as_str();
        if !body_bone(name) {
            continue;
        }
        let mine = ours(name);
        let parent = fit.parents[b].map(usize::from).filter(|&p| p < b);
        turn[b] = match (mine, parent) {
            (Some(m), _) => {
                posed[b] = true;
                m.rotation * fit.rest[b].rotation.inverse()
            }
            (None, Some(p)) => turn[p],
            (None, None) => Quat::IDENTITY,
        };
        at[b] = match parent {
            Some(p) => at[p] + turn[p] * (fit.rest[b].translation - fit.rest[p].translation),
            None => mine.map_or(fit.rest[b].translation, |m| m.translation),
        };
        // The hips are where the pose puts them.
        if let (Some(p), Some(m)) = (parent, mine)
            && fit.parents[p].is_none()
        {
            at[b] = m.translation;
        }
    }
    let (mut shift, mut feet) = (Vec3::ZERO, 0.0);
    for b in 0..n {
        let name = fit.names[b].as_str();
        if posed[b]
            && name.starts_with("foot_")
            && let Some(m) = ours(name)
        {
            shift += m.translation - at[b];
            feet += 1.0;
        }
    }
    if feet > 0.0 {
        shift /= feet;
    }
    let find = |name: &str| {
        let i = fit.names[..n].iter().position(|b| b == name)?;
        Some((at[i] + shift, ours(name)?.rotation))
    };
    Some([
        find("hips")?,
        find("chest")?,
        find("head")?,
        find("hand_l")?,
        find("hand_r")?,
    ])
}

/// How the pieces worn from one of the stand-in's joints move onto the person a character
/// module draws: the stand-in's joint (`own`), where the drawn person has it (`at`), and how
/// much more that person's bone is turned than the stand-in's (`turn`), model space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct WornFrame {
    pub own: Vec3,
    pub at: Vec3,
    pub turn: Quat,
    /// The drawn person's bone itself, turned as posed (model space).
    pub rot: Quat,
}

impl WornFrame {
    /// A worn piece placed on the stand-in at `pos`, turned `rot`, moved onto the person.
    pub fn carry(&self, pos: Vec3, rot: Quat) -> (Vec3, Quat) {
        (self.at + self.turn * (pos - self.own), self.turn * rot)
    }
}

/// Where the pieces worn from the stand-in's hips, chest and head go on the person a character
/// module draws from `fit`: posed by the stand-in, or by `drawn` (another module's pose, found
/// by bone name).
pub(super) fn worn_frames(
    fit: &BodyFit,
    s: &Skeleton,
    drawn: Option<&SkeletonPose>,
) -> Option<[WornFrame; 5]> {
    let mine = bones(s);
    let ours = |name: &str| {
        let i = HUMANOID_BONES.iter().position(|(b, _)| *b == name)?;
        mine.get(i).copied()
    };
    let theirs = |name: &str| {
        let p = drawn?;
        p.bones.get(p.index_of(name)?).copied()
    };
    let posed = match drawn {
        Some(_) => drawn_frames(fit, &theirs)?,
        None => drawn_frames(fit, &ours)?,
    };
    let own = s.worn_joints();
    let names = ["hips", "chest", "head", "hand_l", "hand_r"];
    let mut out = [WornFrame {
        own: Vec3::ZERO,
        at: Vec3::ZERO,
        turn: Quat::IDENTITY,
        rot: Quat::IDENTITY,
    }; 5];
    for i in 0..5 {
        let stand_in = ours(names[i])?.rotation;
        out[i] = WornFrame {
            own: own[i],
            at: posed[i].0,
            turn: (posed[i].1 * stand_in.inverse()).normalize(),
            rot: posed[i].1,
        };
    }
    Some(out)
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

/// The stand-in's shoulder height standing at rest (m).
const STAND_IN_SHOULDERS: f32 = crate::art::fighter::rig::HIP_Y
    + crate::art::fighter::rig::WAIST.y
    + crate::art::fighter::rig::SHOULDER.y;

/// A person's size as a share of the stand-in's, by the height of their shoulders standing at
/// rest (what decides where their hands reach), kept within 0.75..1.3 (the people drawn run from about 1.5 to 2.1 m); 1 for a skeleton
/// without shoulders.
pub(crate) fn size_of(fit: &BodyFit) -> f32 {
    let at = |name: &str| {
        let i = fit.names.iter().position(|n| n == name)?;
        fit.rest.get(i).map(|b| b.translation.y)
    };
    match (at("upper_arm_l"), at("upper_arm_r")) {
        (Some(l), Some(r)) if l + r > 0.0 => (0.5 * (l + r) / STAND_IN_SHOULDERS).clamp(0.75, 1.3),
        _ => 1.0,
    }
}

/// What the pose and face writer reads and writes of a fighter.
type BodyItem<'a> = (
    Entity,
    &'a StandInState,
    Option<&'a mut SkeletonPose>,
    Option<&'a mut FaceControls>,
    Option<&'a mut HandControls>,
    Option<&'a crate::agent::NpcWeapon>,
    (
        Has<BodyRequest>,
        Option<&'a crate::intent_body::IntentBody>,
        Has<AttackWindow>,
    ),
);

type BodyFilter = (Without<Animal>, Without<crate::rest::Resting>);

/// Write each fighter's `SkeletonPose`, `FaceControls` and `HandControls` from its stand-in's
/// pose (not where a `BodyRequest` has the animation module write the pose: see the module
/// docs).
pub(crate) fn write_bodies(mut commands: Commands, mut q: Query<BodyItem, BodyFilter>) {
    for (e, st, pose, face, hands, weapon, (requested, intent, armed)) in &mut q {
        // The animation module writes the pose where it was asked for a body: one this crate
        // asked for (`intent_body.rs`) once it has answered (armed it), any other at once.
        let ours = intent.is_some_and(|b| b.asked);
        if requested && (!ours || armed) {
            continue;
        }
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
        let held = super::humanoid::hand_controls(s, &weapon.copied().unwrap_or_default());
        match hands {
            Some(mut hands) => {
                if *hands != held {
                    *hands = held;
                }
            }
            None => {
                commands.entity(e).try_insert(held);
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
    weapon: Option<&crate::agent::NpcWeapon>,
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
    let (outfit, headgear) = team_kit(seed, faction.as_ref(), skills, weapon);
    AppearanceRequest {
        seed: seed.0,
        family,
        sex: Some(sex),
        age: Some([young, young + 8.0 + 10.0 * level]),
        ancestry: None,
        outfit: Some(outfit),
        headgear: Some(headgear),
    }
}

type Unasked<'a> = (
    Entity,
    &'a Seed,
    Option<&'a Squad>,
    Option<&'a Faction>,
    Option<&'a NpcSkills>,
    Option<&'a crate::agent::NpcWeapon>,
);

/// How near the camera (the nearest simulation focus) a fighter is built at each detail, m:
/// promoted inside the first distance, kept up to the second; and how many fighters at most
/// are built that finely. A close-up person (subdivided skin, every hair, the face's parts)
/// holds about 156 MB, a mid one 62 MB and a crowd one 7 MB (8 built headless, VM), so a
/// zone's worth of fighters all in close-up filled the machine's memory.
const CLOSE_UP_DETAIL: (f64, f64, usize) = (5.0, 6.5, 2);
const MID_DETAIL: (f64, f64, usize) = (18.0, 23.0, 6);
/// How often the detail is re-ranked, s (a change rebuilds the person in the background).
const DETAIL_EVERY: f64 = 0.5;

/// The detail a fighter `d` m from the camera is built at, with `close` and `mid` fighters
/// nearer already at those details (see [`CLOSE_UP_DETAIL`]); `now` is what it has, so it
/// keeps it a little farther out than it would get it.
pub(crate) fn detail_for(
    d: f64,
    now: Option<CharacterQuality>,
    close: usize,
    mid: usize,
) -> CharacterQuality {
    let (c_in, c_out, c_max) = CLOSE_UP_DETAIL;
    let (m_in, m_out, m_max) = MID_DETAIL;
    let close_r = if now == Some(CharacterQuality::CloseUp) {
        c_out
    } else {
        c_in
    };
    let mid_r = if matches!(now, Some(CharacterQuality::CloseUp | CharacterQuality::Mid)) {
        m_out
    } else {
        m_in
    };
    if close < c_max && d < close_r {
        CharacterQuality::CloseUp
    } else if close + mid < m_max && d < mid_r {
        CharacterQuality::Mid
    } else {
        CharacterQuality::Crowd
    }
}

/// The fighters whose detail is ranked: where each is and what it is built at.
type Ranked<'a> = (Entity, &'a Transform, &'a mut CharacterQuality);

/// Give each new fighter its `AppearanceRequest` and a `CharacterQuality` for its distance from
/// the camera, and every [`DETAIL_EVERY`] re-rank the detail fighters are built at: the
/// nearest few in close-up, a few more in mid, the rest as a crowd ([`detail_for`]).
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub(crate) fn request_appearances(
    mut commands: Commands,
    settings: Res<NpcSettings>,
    focus: Res<FocusPoints>,
    time: Res<Time>,
    q: Query<
        (Unasked, &Transform),
        (
            With<Npc>,
            Without<Animal>,
            Without<AppearanceRequest>,
            Without<Appearance>,
        ),
    >,
    mut ranked: Query<Ranked, (With<Npc>, Without<Animal>)>,
    mut next: Local<f64>,
    mut order: Local<Vec<(f64, Entity)>>,
    (drawn, mut bodies, animals): (
        Option<Res<AppearanceBodies>>,
        Query<crate::intent_body::Asking, crate::intent_body::AskingFilter>,
        Query<crate::animal_body::Asking, Without<Npc>>,
    ),
) {
    crate::intent_body::give_bodies(&mut commands, &settings, drawn.is_some(), &mut bodies);
    crate::animal_body::give_animal_bodies(&mut commands, &settings, &animals);
    if !settings.character_looks {
        return;
    }
    let away = |p: DVec3| {
        focus
            .points
            .iter()
            .map(|f| f.distance(p))
            .fold(f64::INFINITY, f64::min)
    };
    let now = time.elapsed_secs_f64();
    if now >= *next {
        *next = now + DETAIL_EVERY;
        order.clear();
        // Each finer detail is handed out nearest first, a fighter already built at it
        // counting as nearer by the band between promoting and demoting, so two fighters
        // about as near don't trade it (and rebuilds) on every re-rank.
        order.extend(ranked.iter().map(|(e, t, _)| (away(t.translation), e)));
        let mut given: Vec<(Entity, CharacterQuality)> = Vec::with_capacity(order.len());
        let mut left: Vec<(f64, Entity, CharacterQuality)> = order
            .iter()
            .filter_map(|&(d, e)| Some((d, e, *ranked.get(e).ok()?.2)))
            .collect();
        for (tier, (c_in, c_out, places)) in [
            (CharacterQuality::CloseUp, CLOSE_UP_DETAIL),
            (CharacterQuality::Mid, MID_DETAIL),
        ] {
            let holds = |q: CharacterQuality| match tier {
                CharacterQuality::CloseUp => q == CharacterQuality::CloseUp,
                _ => q != CharacterQuality::Crowd,
            };
            let key = |&(d, _, q): &(f64, Entity, CharacterQuality)| {
                if holds(q) { d - (c_out - c_in) } else { d }
            };
            left.sort_by(|a, b| key(a).total_cmp(&key(b)));
            let room = places.saturating_sub(given.len());
            let mut taken = 0;
            left.retain(|&(d, e, q)| {
                let reach = if holds(q) { c_out } else { c_in };
                if taken < room && d < reach {
                    taken += 1;
                    given.push((e, tier));
                    false
                } else {
                    true
                }
            });
        }
        given.extend(left.iter().map(|&(_, e, _)| (e, CharacterQuality::Crowd)));
        for (e, want) in given {
            if let Ok((_, _, mut q)) = ranked.get_mut(e)
                && *q != want
            {
                *q = want;
            }
        }
    }
    for ((e, &seed, squad, faction, skills, weapon), t) in &q {
        let request = appearance_request(seed, squad.copied(), faction.copied(), skills, weapon);
        // Close-up only once ranked among the nearest.
        let detail = match detail_for(away(t.translation), None, 0, 0) {
            CharacterQuality::CloseUp => CharacterQuality::Mid,
            d => d,
        };
        commands.entity(e).try_insert((request, detail));
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
        DamageKind::Bleed => (0.0, 0.0),
    }
}

/// sRGB 0..255 to linear 0..1.
fn linear(hex: u32) -> [f32; 3] {
    let c = |v: u32| {
        let v = (v & 0xff) as f32 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    [c(hex >> 16), c(hex >> 8), c(hex)]
}

/// Undyed and earthy cloth for the gambeson (the padding under mail): linen and oatmeal, and
/// wool in tan, grey, dark brown, olive drab and charcoal (as the stand-ins' own, linear RGB
/// of these sRGB colours).
const GAMBESONS: [u32; 7] = [
    0xbcb09a, 0x7f6449, 0x7c7a73, 0xa08c6c, 0x5f4a38, 0x6b6649, 0x5c5b56,
];
/// Wool for the hose: dark brown, charcoal, olive-brown and umber.
const HOSE: [u32; 4] = [0x5a4836, 0x4a4a4d, 0x585340, 0x463a30];
/// The skill from which a fighter who closes in wears mail over the gambeson.
const MAIL_FROM: f32 = 0.6;

/// The team's kit for a fighter whose look leaves the clothes as they come (see the module
/// docs): a gambeson in an undyed or earthy cloth (with mail over it on a veteran or better
/// who fights up close), hose and boots, a tabard in the team's colour, and a mail coif for
/// the helmet to sit over (a hood for those who fight from range). The cloths are picked by
/// the seed.
pub(crate) fn team_kit(
    seed: Seed,
    faction: Option<&Faction>,
    skills: Option<&NpcSkills>,
    weapon: Option<&crate::agent::NpcWeapon>,
) -> (Outfit, String) {
    let team = linear(super::faction_color(faction));
    let level = skills.map_or(0.5, |s| s.level());
    let ranged = weapon.is_some_and(|w| w.style == crate::agent::WeaponStyle::Ranged);
    let pick =
        |cloths: &[u32], salt: u64| cloths[(mix(seed.0 ^ salt) % cloths.len() as u64) as usize];
    let outfit = Outfit {
        top: if !ranged && level >= MAIL_FROM {
            "mail"
        } else {
            "gambeson"
        }
        .into(),
        top_color: linear(pick(&GAMBESONS, 0x9a3b)),
        bottom: "hose".into(),
        bottom_color: linear(pick(&HOSE, 0x7a05)),
        accent: team,
        accent_on: "tabard".into(),
    };
    (outfit, if ranged { "hood" } else { "mail_coif" }.into())
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
        // A bleeding wound marks nothing new (its blow already did).
        if b.amount <= 0.0 || b.kind == DamageKind::Bleed {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A standing skeleton on the humanoid bones, every bone unturned (`legs` scales the legs).
    fn standing(legs: f32) -> Vec<BonePose> {
        let hip_y = 0.97 * legs;
        let at = |name: &str| -> Vec3 {
            let x = |v: f32| if name.ends_with("_l") { -v } else { v };
            match name.trim_end_matches("_l").trim_end_matches("_r") {
                "root" => Vec3::ZERO,
                "hips" => Vec3::new(0.0, hip_y + 0.07, 0.0),
                "spine" => Vec3::new(0.0, hip_y + 0.12, 0.0),
                "chest" => Vec3::new(0.0, hip_y + 0.30, 0.0),
                "neck" => Vec3::new(0.0, hip_y + 0.55, 0.0),
                "head" => Vec3::new(0.0, hip_y + 0.65, 0.0),
                "ponytail_1" | "ponytail_2" | "ponytail_3" => Vec3::new(0.0, hip_y + 0.8, 0.1),
                "clavicle" => Vec3::new(x(0.03), hip_y + 0.46, 0.0),
                "upper_arm" => Vec3::new(x(0.18), hip_y + 0.46, 0.0),
                "lower_arm" => Vec3::new(x(0.24), hip_y + 0.2, 0.0),
                "hand" => Vec3::new(x(0.28), hip_y - 0.05, 0.0),
                "upper_leg" => Vec3::new(x(0.095), hip_y, 0.0),
                "lower_leg" => Vec3::new(x(0.095), hip_y * 0.52, 0.0),
                "foot" => Vec3::new(x(0.095), 0.075, 0.0),
                "toe" => Vec3::new(x(0.095), 0.025, -0.14),
                other => panic!("{other}"),
            }
        };
        HUMANOID_BONES
            .iter()
            .map(|(n, _)| bone(at(n), Quat::IDENTITY))
            .collect()
    }

    fn fit(rest: Vec<BonePose>) -> BodyFit {
        BodyFit {
            names: NAMES.0.to_vec(),
            parents: NAMES.1.to_vec(),
            rest,
            height: 1.7,
        }
    }

    fn named(pose: &[BonePose], name: &str) -> Vec3 {
        let i = HUMANOID_BONES.iter().position(|(b, _)| *b == name).unwrap();
        pose[i].translation
    }

    #[test]
    fn a_person_of_the_poses_own_size_has_its_joints_where_the_pose_puts_them() {
        let rest = standing(1.0);
        // Bent forward at the waist and the head turned.
        let mut pose = rest.clone();
        let bend = Quat::from_rotation_x(-0.5);
        let waist = named(&rest, "spine");
        for name in ["spine", "chest", "neck", "head"] {
            let i = HUMANOID_BONES.iter().position(|(b, _)| *b == name).unwrap();
            pose[i].translation = waist + bend * (rest[i].translation - waist);
            pose[i].rotation = bend;
        }
        let i = HUMANOID_BONES
            .iter()
            .position(|(b, _)| *b == "head")
            .unwrap();
        pose[i].rotation = Quat::from_rotation_y(0.7) * bend;
        let [hips, chest, head] = drawn_joints(&fit(rest), &pose).unwrap();
        for (got, name) in [(hips, "hips"), (chest, "chest"), (head, "head")] {
            let want = named(&pose, name);
            assert!(got.distance(want) < 1e-4, "{name}: {got} vs {want}");
        }
    }

    #[test]
    fn a_short_legged_person_stands_lower_on_the_same_feet() {
        let pose = standing(1.0);
        let short = standing(0.9);
        let [hips, _, head] = drawn_joints(&fit(short.clone()), &pose).unwrap();
        let drop = named(&pose, "hips").y - named(&short, "hips").y;
        assert!(
            (named(&pose, "hips").y - hips.y - drop).abs() < 1e-3,
            "{hips}"
        );
        assert!(
            (named(&pose, "head").y - head.y - drop).abs() < 1e-3,
            "{head}"
        );
    }
}
