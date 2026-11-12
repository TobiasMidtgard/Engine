//! Pilots as people: where a character module draws appearances (`AppearanceBodies`), each
//! pilot puppet ([`Style::Pilot`]: the player and the course's runners) is a person from the
//! character creator, posed every frame by this module's own rig ([`super::rig`]), which covers
//! every move the movement module has (runs, wall runs, flips, vaults, hangs, climbs, ladders,
//! ropes, bars, sword fights).
//!
//! * **Look.** A pilot asks for its look once: an `AppearanceRequest` with its own seed (from
//!   its name and where it was spawned, never an entity id), aged 22..38, in the pilots' kit
//!   ([`pilot_kit`]): a dark long-sleeved shirt and trousers with the style's teal at the
//!   collar, bare-headed. The player is built in close-up, the other pilots at mid detail.
//!   A look given outright is kept.
//! * **Pose.** One writer: this module writes the pilot's `SkeletonPose` from the rig's solved
//!   bones ([`skeleton`]), on the humanoid skeleton's body bones (no finger or twist bones, so
//!   `HandControls` drives the fingers and the twists follow the limbs). Limbs hang along their
//!   −Y and hinge about +X with the feet flat and the toes along −Z in both skeletons, so the
//!   rig's rotations are the pose's. The character module keeps the person's own limb lengths,
//!   puts the hips where the pose puts them, lands the feet on the pose's feet and reaches the
//!   hands to the pose's hands, so the grip on a bar, a ledge or the sword stays put.
//! * **Hands.** Power grips round the sword's handle (the right hand while it holds it, the
//!   left on the handle), round bars, ropes and ledges as far as the rig closes the fingers.
//! * **Armor.** Once the person is built (its `BodyFit`), the puppet's own armor and body are
//!   hidden ([`Shell`]); the sword, its blade and the scabbard stay. Afterimages still copy
//!   the hidden armor, as a glowing ghost of the body.
//! * **Parked.** A pilot parked by play mode is posed far below the ground, out of sight (the
//!   contract has no way to hide a drawn person).
//!
//! Without the resource nothing changes: the pilots are the armored puppets.

use std::sync::{Arc, LazyLock};

use engine_core::math::{Quat, Vec3};
use engine_core::prelude::*;
use engine_render::MeshRenderer;
use engine_world::animation::{BonePose, HUMANOID_BONES, SkeletonPose};
use engine_world::character::{
    Appearance, AppearanceBodies, AppearanceRequest, BodyFit, CharacterQuality, HandControls,
    HandPose, HandShape, Outfit,
};

use super::fighter::Style;
use super::rig::{BONES, Bone, Pose, Xf, smooth};
use crate::player::PlayerControlled;

/// One part (mesh) of a puppet's body: whose and on which bone.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct BodyPart {
    pub owner: Entity,
    pub bone: Bone,
}

/// A body part hidden because a person is drawn in its place.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default)]
pub struct Shell;

/// The humanoid bones a pilot's pose has, with the rig bone each takes its rotation from.
const POSED: [(&str, Bone); 18] = [
    ("root", Bone::Pelvis),
    ("hips", Bone::Pelvis),
    ("spine", Bone::Spine),
    ("chest", Bone::Chest),
    ("neck", Bone::Neck),
    ("head", Bone::Head),
    ("upper_arm_l", Bone::UpperArmL),
    ("lower_arm_l", Bone::ForearmL),
    ("hand_l", Bone::HandL),
    ("upper_arm_r", Bone::UpperArmR),
    ("lower_arm_r", Bone::ForearmR),
    ("hand_r", Bone::HandR),
    ("upper_leg_l", Bone::ThighL),
    ("lower_leg_l", Bone::ShinL),
    ("foot_l", Bone::FootL),
    ("upper_leg_r", Bone::ThighR),
    ("lower_leg_r", Bone::ShinR),
    ("foot_r", Bone::FootR),
];

/// The humanoid's `hips` above the middle of its hip joints (the animation module's offset),
/// m; the rig's hip joints are 0.04 m below its pelvis joint.
const HIPS_ABOVE_PELVIS: f32 = 0.07 - 0.04;

/// How far below its feet a parked pilot is posed, m.
const PARKED_DROP: f32 = 2000.0;

type Names = (Arc<[String]>, Arc<[Option<u16>]>);

/// The names and parents every pilot's `SkeletonPose` shares: each bone's parent is its
/// nearest ancestor on the humanoid skeleton that the pose has.
static NAMES: LazyLock<Names> = LazyLock::new(|| {
    let names: Arc<[String]> = POSED.iter().map(|(n, _)| n.to_string()).collect();
    let parent_of = |name: &str| HUMANOID_BONES.iter().find(|(n, _)| *n == name)?.1;
    let parents = POSED
        .iter()
        .map(|(n, _)| {
            let mut up = parent_of(n);
            while let Some(p) = up {
                if let Some(i) = POSED.iter().position(|(m, _)| *m == p) {
                    return Some(i as u16);
                }
                up = parent_of(p);
            }
            None
        })
        .collect();
    (names, parents)
});

/// A fresh pose on the pilots' bones, at rest.
pub fn empty_pose() -> SkeletonPose {
    SkeletonPose {
        names: NAMES.0.clone(),
        parents: NAMES.1.clone(),
        bones: vec![BonePose::default(); POSED.len()],
    }
}

/// The rig's solved bones (character space, feet at the origin) as the pilot's pose bones in
/// model space, with the feet `lift` m above the owner's origin.
pub fn skeleton(solved: &[Xf; BONES], lift: f32, out: &mut Vec<BonePose>) {
    out.clear();
    let up = Vec3::Y * lift;
    for (name, bone) in POSED {
        let x = solved[bone.index()];
        let (at, rot) = match name {
            "root" => (Vec3::ZERO, Quat::IDENTITY),
            "hips" => (x.point(Vec3::Y * HIPS_ABOVE_PELVIS), x.rot),
            _ => (x.pos, x.rot),
        };
        out.push(BonePose {
            translation: at + up,
            rotation: rot,
            scale: Vec3::ONE,
        });
    }
}

/// How the hands close (left, right) for a pose: the right round the sword's handle while it
/// holds it, the left round it while on the handle, either round what the rig grips
/// otherwise (bars, ropes, ledges) as far as it curls the fingers.
pub fn hands(p: &Pose) -> HandControls {
    let held = 1.0 - smooth(0.0, 0.15, p.free_r.clamp(0.0, 1.0));
    let curl = p.curl_l.clamp(0.0, 1.0);
    let left = p.grip_l.clamp(0.0, 1.0).max(curl);
    let right = held + (1.0 - held) * curl;
    let grip = |weight: f32| HandPose {
        shape: HandShape::PowerGrip,
        weight,
        handle_radius_m: 0.016,
        curl: None,
    };
    HandControls {
        grip: [left, right],
        hands: [grip(left), grip(right)],
    }
}

fn linear(hex: u32) -> [f32; 3] {
    engine_render::material::srgb_hex_to_linear(hex)
}

/// What the pilots wear: a near-black long-sleeved shirt and charcoal trousers, the style's
/// teal at the collar.
pub fn pilot_kit() -> Outfit {
    Outfit {
        top: "shirt".into(),
        top_color: linear(0x23282c),
        bottom: "trousers".into(),
        bottom_color: linear(0x2b2b28),
        accent: Style::Pilot.glow(),
        accent_on: "collar".into(),
    }
}

/// A pilot's seed from its name and where it was spawned.
pub fn seed_for(name: &str, at: engine_core::math::DVec3) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |b: u8| {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    };
    name.bytes().for_each(&mut eat);
    for v in [at.x, at.y, at.z] {
        ((v * 10.0).round() as i64)
            .to_le_bytes()
            .into_iter()
            .for_each(&mut eat);
    }
    h
}

/// Who is a person: the pilot puppets, where a character module draws them.
pub fn is_person(style: Style) -> bool {
    style == Style::Pilot
}

type Undressed = (Without<Appearance>, Without<AppearanceRequest>);

/// Ask for each pilot's look and give it a pose and hands to write (see the module docs).
pub fn dress_people(
    mut commands: Commands,
    bodies: Option<Res<AppearanceBodies>>,
    q: Query<(Entity, &crate::demo::Puppet, Has<PlayerControlled>), Undressed>,
) {
    if bodies.is_none() {
        return;
    }
    for (e, p, player) in &q {
        if !is_person(p.style()) {
            continue;
        }
        let quality = if player {
            CharacterQuality::CloseUp
        } else {
            CharacterQuality::Mid
        };
        commands.entity(e).insert((
            AppearanceRequest {
                seed: p.seed(),
                age: Some([22.0, 38.0]),
                outfit: Some(pilot_kit()),
                headgear: Some("none".into()),
                ..Default::default()
            },
            quality,
            empty_pose(),
            HandControls::default(),
        ));
    }
}

/// Hide the armor of pilots drawn as people, and show it again on those that aren't (see the
/// module docs).
pub fn hide_shells(
    mut commands: Commands,
    bodies: Option<Res<AppearanceBodies>>,
    people: Query<(), (With<crate::demo::Puppet>, With<BodyFit>)>,
    mut parts: Query<(Entity, &BodyPart, &mut MeshRenderer, Has<Shell>)>,
) {
    let drawing = bodies.is_some();
    for (e, part, mut r, shell) in &mut parts {
        let kept = matches!(part.bone, Bone::Sword | Bone::Blade | Bone::Sheath);
        let person = drawing && !kept && people.contains(part.owner);
        if person && !shell {
            r.visible = false;
            commands.entity(e).insert(Shell);
        } else if !person && shell {
            r.visible = true;
            commands.entity(e).remove::<Shell>();
        }
    }
}

/// Write a person's pose and hands for this frame's rig pose (`solved` from `pose`).
pub fn write_person(
    pose: &Pose,
    solved: &[Xf; BONES],
    lift: f32,
    parked: bool,
    out: &mut SkeletonPose,
    hands_out: &mut HandControls,
) {
    if out.names.len() != POSED.len() {
        *out = empty_pose();
    }
    let lift = if parked { lift - PARKED_DROP } else { lift };
    skeleton(solved, lift, &mut out.bones);
    let h = hands(pose);
    if *hands_out != h {
        *hands_out = h;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::look::rig::{self, Pose};

    fn named<'a>(p: &'a SkeletonPose, name: &str) -> &'a BonePose {
        &p.bones[p.index_of(name).expect("bone")]
    }

    #[test]
    fn the_pose_names_humanoid_bones_with_their_nearest_posed_parents() {
        let p = empty_pose();
        for (i, n) in p.names.iter().enumerate() {
            assert!(HUMANOID_BONES.iter().any(|(b, _)| b == n), "{n}");
            if let Some(parent) = p.parents[i] {
                assert!((parent as usize) < i, "{n} before its parent");
            }
        }
        let parent =
            |n: &str| p.parents[p.index_of(n).unwrap()].map(|i| p.names[i as usize].as_str());
        assert_eq!(parent("upper_arm_l"), Some("chest"));
        assert_eq!(parent("hand_r"), Some("lower_arm_r"));
        assert_eq!(parent("hips"), Some("root"));
        assert_eq!(parent("root"), None);
    }

    #[test]
    fn standing_puts_the_feet_on_the_floor_and_the_hips_over_them_turned_as_the_rig() {
        let solved = rig::solve(&Pose::BIND);
        let mut p = empty_pose();
        let mut hands = HandControls::default();
        write_person(&Pose::BIND, &solved, -0.9, false, &mut p, &mut hands);
        let foot = named(&p, "foot_l");
        let ankle = solved[Bone::FootL.index()].pos;
        assert!(foot.translation.distance(ankle - Vec3::Y * 0.9) < 1e-4);
        assert!(foot.translation.y < 0.15 - 0.9, "{}", foot.translation);
        let hips = named(&p, "hips");
        let pelvis = solved[Bone::Pelvis.index()].pos;
        assert!((hips.translation.y - (pelvis.y + 0.03 - 0.9)).abs() < 1e-4);
        assert!(hips.translation.x.abs() < 1e-4);
        // Standing straight: the legs and the arms hang unturned, as the humanoid's rest
        // without its arms' 30°.
        for b in [
            "upper_leg_l",
            "lower_leg_r",
            "upper_arm_l",
            "lower_arm_r",
            "spine",
        ] {
            assert!(
                named(&p, b).rotation.angle_between(Quat::IDENTITY) < 0.05,
                "{b}"
            );
        }
        // The rig's sword is in the right hand: a closed grip.
        assert!(hands.grip[1] > 0.95 && hands.hands[1].shape == HandShape::PowerGrip);
    }

    #[test]
    fn a_parked_pilot_is_posed_far_below_the_ground() {
        let solved = rig::solve(&Pose::BIND);
        let mut p = empty_pose();
        let mut hands = HandControls::default();
        write_person(&Pose::BIND, &solved, -0.9, true, &mut p, &mut hands);
        assert!(p.bones.iter().all(|b| b.translation.y < -1000.0));
    }

    #[test]
    fn seeds_differ_by_name_and_spawn_point() {
        let at = engine_core::math::DVec3::new(1.0, 2.0, 3.0);
        assert_ne!(seed_for("Runner", at), seed_for("Climber", at));
        assert_ne!(
            seed_for("Runner", at),
            seed_for("Runner", at + engine_core::math::DVec3::X)
        );
        assert_eq!(seed_for("Runner", at), seed_for("Runner", at));
    }
}
