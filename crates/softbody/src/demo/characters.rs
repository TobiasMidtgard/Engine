//! Demo characters: a mannequin and a dog on procedural skeletons (the same `SkeletonPose`
//! contract the animation module writes), wearing this module's cloth, hair and fur.

use super::props::*;
use super::*;

/// Which procedural rig a demo skeleton uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rig {
    Humanoid,
    Dog,
}

/// Animates this entity's `SkeletonPose` with a walk (or trot) cycle of `cycle` seconds.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct DemoSkeleton {
    pub rig: Rig,
    pub cycle: f32,
}

/// A body part mesh placed on the bones of `owner` every step.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct DemoLimb {
    pub owner: Entity,
    pub from: usize,
    pub to: Option<usize>,
    /// Offset in the `from` bone's space.
    pub offset: Vec3,
}

// ---------------------------------------------------------------------------------------
// Characters

const HUMANOID: [(&str, Option<u16>); 17] = [
    ("pelvis", None),
    ("spine", Some(0)),
    ("chest", Some(1)),
    ("neck", Some(2)),
    ("head", Some(3)),
    ("upper_arm_l", Some(2)),
    ("forearm_l", Some(5)),
    ("hand_l", Some(6)),
    ("upper_arm_r", Some(2)),
    ("forearm_r", Some(8)),
    ("hand_r", Some(9)),
    ("thigh_l", Some(0)),
    ("shin_l", Some(11)),
    ("foot_l", Some(12)),
    ("thigh_r", Some(0)),
    ("shin_r", Some(14)),
    ("foot_r", Some(15)),
];

const DOG: [(&str, Option<u16>); 20] = [
    ("hips", None),
    ("spine", Some(0)),
    ("chest", Some(1)),
    ("neck", Some(2)),
    ("head", Some(3)),
    ("tail_1", Some(0)),
    ("tail_2", Some(5)),
    ("tail_3", Some(6)),
    ("leg_fl", Some(2)),
    ("knee_fl", Some(8)),
    ("paw_fl", Some(9)),
    ("leg_fr", Some(2)),
    ("knee_fr", Some(11)),
    ("paw_fr", Some(12)),
    ("leg_hl", Some(0)),
    ("knee_hl", Some(14)),
    ("paw_hl", Some(15)),
    ("leg_hr", Some(0)),
    ("knee_hr", Some(17)),
    ("paw_hr", Some(18)),
];

fn skeleton(bones: &[(&str, Option<u16>)], rig: Rig) -> SkeletonPose {
    let mut pose = SkeletonPose {
        names: bones
            .iter()
            .map(|(n, _)| n.to_string())
            .collect::<Vec<_>>()
            .into(),
        parents: Arc::from(bones.iter().map(|(_, p)| *p).collect::<Vec<_>>()),
        bones: vec![BonePose::default(); bones.len()],
    };
    pose_rig(rig, 0.0, &mut pose);
    pose
}

/// Direction from angle `a` (radians, toward forward = −Z) hanging down.
fn hang(a: f32) -> Vec3 {
    Vec3::new(0.0, -a.cos(), -a.sin())
}

fn aim(from: Vec3, to: Vec3) -> Quat {
    let d = to - from;
    if d.length_squared() < 1e-10 {
        Quat::IDENTITY
    } else {
        Quat::from_rotation_arc(Vec3::NEG_Y, d.normalize())
    }
}

/// Write a walk (humanoid) or trot (dog) pose at `phase` radians into `pose`.
pub fn pose_rig(rig: Rig, phase: f32, pose: &mut SkeletonPose) {
    let p = &mut pose.bones;
    let set = |p: &mut Vec<BonePose>, i: usize, t: Vec3, r: Quat| {
        p[i].translation = t;
        p[i].rotation = r;
        p[i].scale = Vec3::ONE;
    };
    match rig {
        Rig::Humanoid => {
            let bob = 0.025 * (2.0 * phase).cos();
            let twist = 0.07 * phase.sin();
            let pelvis = Vec3::new(0.0, 0.95 + bob, 0.0);
            let chest = Vec3::new(0.0, 1.30 + bob, -0.02);
            set(p, 0, pelvis, Quat::from_rotation_y(-twist));
            set(p, 1, Vec3::new(0.0, 1.12 + bob, -0.01), Quat::IDENTITY);
            set(p, 2, chest, Quat::from_rotation_y(twist));
            set(p, 3, Vec3::new(0.0, 1.50 + bob, -0.03), Quat::IDENTITY);
            set(p, 4, Vec3::new(0.0, 1.60 + bob, -0.04), Quat::IDENTITY);
            // Facing −Z, the left side is −X.
            for (side, base_arm, base_leg) in [(-1.0f32, 5, 11), (1.0, 8, 14)] {
                let ph = if side > 0.0 { phase } else { phase + PI };
                // Arm swings opposite to the leg on the same side.
                let swing = -0.35 * ph.sin();
                let shoulder = chest + Vec3::new(0.2 * side, 0.15, 0.0);
                let elbow = shoulder + hang(swing) * 0.28 + Vec3::X * (0.02 * side);
                let hand = elbow + hang(swing + 0.35) * 0.26;
                set(p, base_arm, shoulder, aim(shoulder, elbow));
                set(p, base_arm + 1, elbow, aim(elbow, hand));
                set(p, base_arm + 2, hand, aim(elbow, hand));
                // Leg: thigh swings, the knee bends through the swing phase.
                let thigh = 0.4 * ph.sin();
                let knee_bend = 0.1 + 0.9 * ph.cos().max(0.0).powi(2);
                let hip = pelvis + Vec3::new(0.1 * side, -0.05, 0.0);
                let knee = hip + hang(thigh) * 0.43;
                let ankle = knee + hang(thigh - knee_bend) * 0.43;
                set(p, base_leg, hip, aim(hip, knee));
                set(p, base_leg + 1, knee, aim(knee, ankle));
                set(p, base_leg + 2, ankle, Quat::IDENTITY);
            }
        }
        Rig::Dog => {
            let bob = 0.015 * (2.0 * phase).cos();
            let wag = (3.0 * phase).sin();
            let hips = Vec3::new(0.0, 0.55 + bob, 0.28);
            let chest = Vec3::new(0.0, 0.58 + bob, -0.24);
            set(p, 0, hips, Quat::IDENTITY);
            set(p, 1, Vec3::new(0.0, 0.57 + bob, 0.02), Quat::IDENTITY);
            set(p, 2, chest, Quat::IDENTITY);
            set(p, 3, Vec3::new(0.0, 0.68 + bob, -0.38), Quat::IDENTITY);
            let head = Vec3::new(0.0, 0.78 + bob, -0.5);
            set(p, 4, head, Quat::from_rotation_y(0.15 * phase.sin()));
            let t1 = Vec3::new(0.0, 0.6 + bob, 0.4);
            let t2 = t1 + Vec3::new(0.08 * wag, 0.07, 0.18);
            let t3 = t2 + Vec3::new(0.16 * wag, 0.04, 0.17);
            set(p, 5, t1, aim(t1, t2));
            set(p, 6, t2, aim(t2, t3));
            set(p, 7, t3, aim(t2, t3));
            // Trot: diagonal legs move together.
            for (base, side, front, offset) in [
                (8, -1.0f32, true, 0.0),
                (11, 1.0, true, PI),
                (14, -1.0, false, PI),
                (17, 1.0, false, 0.0),
            ] {
                let ph = phase + offset;
                let swing = 0.35 * ph.sin();
                let bend = 0.15 + 0.6 * ph.cos().max(0.0).powi(2);
                let top = if front { chest } else { hips } + Vec3::new(0.11 * side, -0.05, 0.0);
                let knee = top + hang(swing) * 0.24;
                let lower = if front {
                    swing - bend
                } else {
                    swing + bend * 0.6
                };
                let paw = knee + hang(lower) * 0.27;
                set(p, base, top, aim(top, knee));
                set(p, base + 1, knee, aim(knee, paw));
                set(p, base + 2, paw, Quat::IDENTITY);
            }
        }
    }
}

/// A body-part mesh on bones `from` (→ `to`) of `owner`.
#[allow(clippy::too_many_arguments)]
fn limb(
    commands: &mut Commands,
    owner: Entity,
    pose: &SkeletonPose,
    from: &str,
    to: Option<&str>,
    radius: f32,
    offset: Vec3,
    material: StandardMaterial,
) {
    let a = pose.index_of(from).expect("bone");
    let b = to.map(|t| pose.index_of(t).expect("bone"));
    let shape = match b {
        Some(b) => Shape::Capsule {
            radius,
            height: pose.bones[a]
                .translation
                .distance(pose.bones[b].translation)
                + 2.0 * radius,
            segments: 14,
        },
        None => Shape::Sphere {
            radius,
            segments: 20,
        },
    };
    commands.spawn((
        Transform::default(),
        TransformInterpolation::default(),
        MeshRenderer::new(shape, material),
        DemoLimb {
            owner,
            from: a,
            to: b,
            offset,
        },
    ));
}

/// A mannequin with a cape and long hair walking around `center`. `time_scale` < 1 runs it
/// (and its cloth and hair) in slow motion.
pub(super) fn mannequin(
    commands: &mut Commands,
    center: DVec3,
    radius: f32,
    time_scale: f32,
    name: &str,
) -> Entity {
    let pose = skeleton(&HUMANOID, Rig::Humanoid);
    let mut root = commands.spawn((
        Name::new(name.to_string()),
        Transform::from_translation(center + DVec3::X * radius as f64),
        TransformInterpolation::default(),
        pose.clone(),
        BoneColliders::humanoid(),
        DemoSkeleton {
            rig: Rig::Humanoid,
            cycle: 1.05,
        },
        DemoMotion::Circle {
            center,
            radius,
            period: TAU * radius / 1.25,
        },
        DemoClock::default(),
    ));
    if time_scale != 1.0 {
        root.insert(LocalTimeScale {
            factor: time_scale,
            ignore_world_dilation: false,
        });
    }
    let man = root.id();
    let skin = mat(0xd9c3a0, 0.55);
    let cloth = mat(0x34495e, 0.8);
    limb(
        commands,
        man,
        &pose,
        "pelvis",
        Some("chest"),
        0.15,
        Vec3::ZERO,
        cloth,
    );
    limb(
        commands,
        man,
        &pose,
        "chest",
        Some("neck"),
        0.155,
        Vec3::ZERO,
        cloth,
    );
    limb(
        commands,
        man,
        &pose,
        "neck",
        Some("head"),
        0.05,
        Vec3::ZERO,
        skin,
    );
    limb(
        commands,
        man,
        &pose,
        "head",
        None,
        0.1,
        Vec3::new(0.0, 0.07, 0.0),
        skin,
    );
    for s in ["l", "r"] {
        let b = |n: &str| format!("{n}_{s}");
        limb(
            commands,
            man,
            &pose,
            &b("upper_arm"),
            Some(&b("forearm")),
            0.05,
            Vec3::ZERO,
            cloth,
        );
        limb(
            commands,
            man,
            &pose,
            &b("forearm"),
            Some(&b("hand")),
            0.042,
            Vec3::ZERO,
            skin,
        );
        limb(
            commands,
            man,
            &pose,
            &b("hand"),
            None,
            0.05,
            Vec3::new(0.0, -0.04, 0.0),
            skin,
        );
        limb(
            commands,
            man,
            &pose,
            &b("thigh"),
            Some(&b("shin")),
            0.075,
            Vec3::ZERO,
            cloth,
        );
        limb(
            commands,
            man,
            &pose,
            &b("shin"),
            Some(&b("foot")),
            0.055,
            Vec3::ZERO,
            cloth,
        );
        limb(
            commands,
            man,
            &pose,
            &b("foot"),
            None,
            0.06,
            Vec3::new(0.0, -0.02, -0.06),
            mat(0x1c1c1c, 0.6),
        );
    }
    // Cape tied across the chest, hanging behind the back.
    let cape = SoftMaterial::hex(0x922b3e)
        .with_pattern(Pattern::Border, 0xd4ac0d, 0xf4d03f)
        .with_roughness(0.85)
        .with_sheen(0.7);
    commands.spawn((
        Name::new(format!("{name} cape")),
        Transform::from_xyz(0.0, 1.46, 0.21),
        Cloth {
            pins: ClothPins::Bones {
                rows: 1,
                bones: vec!["chest".into()],
            },
            ..Cloth::cape([0.72, 1.2], cape, &["chest"])
        },
        ChildOf(man),
    ));
    let style = HairStyle {
        melanin: 0.8,
        redness: 0.2,
        ..HairStyle::long()
    };
    commands.spawn((
        Name::new(format!("{name} hair")),
        Transform::default(),
        HairGroom {
            emitters: vec![HairEmitter::Scalp {
                bone: "head".into(),
                // The head mesh: a 0.1 m ball 7 cm above the head bone.
                center: [0.0, 0.07, 0.0],
                radii: [0.101, 0.106, 0.106],
                guides: 300,
                hairline: 0.55,
            }],
            ..HairGroom::long_hair("head", style)
        },
        ChildOf(man),
    ));
    man
}

pub(super) fn dog(commands: &mut Commands, center: DVec3) {
    let pose = skeleton(&DOG, Rig::Dog);
    let capsules = vec![
        BoneCapsule::capsule("hips", "chest", 0.15),
        BoneCapsule::capsule("neck", "head", 0.07),
        BoneCapsule::sphere("head", 0.1, [0.0, 0.02, -0.04]),
        BoneCapsule::capsule("tail_1", "tail_3", 0.03),
        BoneCapsule::capsule("leg_fl", "paw_fl", 0.04),
        BoneCapsule::capsule("leg_fr", "paw_fr", 0.04),
        BoneCapsule::capsule("leg_hl", "paw_hl", 0.045),
        BoneCapsule::capsule("leg_hr", "paw_hr", 0.045),
    ];
    let radius = 2.0;
    let dog = commands
        .spawn((
            Name::new("Dog"),
            Transform::from_translation(center + DVec3::X * radius as f64),
            TransformInterpolation::default(),
            pose.clone(),
            BoneColliders {
                capsules,
                friction: 0.3,
            },
            DemoSkeleton {
                rig: Rig::Dog,
                cycle: 0.55,
            },
            DemoMotion::Circle {
                center,
                radius,
                period: TAU * radius / 1.6,
            },
            DemoClock::default(),
        ))
        .id();
    let coat = mat(0xb5793c, 0.8);
    let dark = mat(0x3b2a1e, 0.6);
    limb(
        commands,
        dog,
        &pose,
        "hips",
        Some("chest"),
        0.14,
        Vec3::ZERO,
        coat,
    );
    limb(
        commands,
        dog,
        &pose,
        "neck",
        Some("head"),
        0.065,
        Vec3::ZERO,
        coat,
    );
    limb(
        commands,
        dog,
        &pose,
        "head",
        None,
        0.095,
        Vec3::new(0.0, 0.02, -0.03),
        coat,
    );
    limb(
        commands,
        dog,
        &pose,
        "head",
        None,
        0.05,
        Vec3::new(0.0, -0.02, -0.13),
        coat,
    );
    limb(
        commands,
        dog,
        &pose,
        "head",
        None,
        0.022,
        Vec3::new(0.0, 0.0, -0.18),
        dark,
    );
    for side in [-1.0f32, 1.0] {
        limb(
            commands,
            dog,
            &pose,
            "head",
            None,
            0.035,
            Vec3::new(0.07 * side, 0.08, 0.01),
            coat,
        );
    }
    limb(
        commands,
        dog,
        &pose,
        "tail_1",
        Some("tail_2"),
        0.03,
        Vec3::ZERO,
        coat,
    );
    limb(
        commands,
        dog,
        &pose,
        "tail_2",
        Some("tail_3"),
        0.025,
        Vec3::ZERO,
        coat,
    );
    for leg in ["fl", "fr", "hl", "hr"] {
        let b = |n: &str| format!("{n}_{leg}");
        limb(
            commands,
            dog,
            &pose,
            &b("leg"),
            Some(&b("knee")),
            0.045,
            Vec3::ZERO,
            coat,
        );
        limb(
            commands,
            dog,
            &pose,
            &b("knee"),
            Some(&b("paw")),
            0.035,
            Vec3::ZERO,
            coat,
        );
        limb(
            commands,
            dog,
            &pose,
            &b("paw"),
            None,
            0.04,
            Vec3::new(0.0, 0.0, -0.02),
            dark,
        );
    }
    let fur = HairStyle {
        melanin: 0.32,
        redness: 0.75,
        ..HairStyle::fur()
    };
    // Torso, neck and head bones point along the body, so fur lies back (+Z) and a little
    // down. Leg and tail bones are aimed along their −Y, so their fur lies toward paw and tip.
    let back = [0.0, -0.3, 1.0];
    let along = [0.0, -1.0, 0.1];
    let body_fur =
        |bone: &str, to: Option<&str>, radius: f32, guides: u32, comb| HairEmitter::Fur {
            bone: bone.into(),
            to: to.map(Into::into),
            radius,
            guides,
            comb,
        };
    commands.spawn((
        Name::new("Dog fur"),
        Transform::default(),
        HairGroom {
            emitters: vec![
                body_fur("hips", Some("chest"), 0.14, 420, back),
                // The rounded ends of the body.
                body_fur("hips", None, 0.14, 80, back),
                body_fur("chest", None, 0.14, 80, back),
                body_fur("neck", Some("head"), 0.065, 100, back),
                body_fur("head", None, 0.095, 90, back),
                body_fur("leg_fl", Some("knee_fl"), 0.045, 36, along),
                body_fur("leg_fr", Some("knee_fr"), 0.045, 36, along),
                body_fur("leg_hl", Some("knee_hl"), 0.045, 36, along),
                body_fur("leg_hr", Some("knee_hr"), 0.045, 36, along),
            ],
            style: fur,
            sim: HairGroom::hair_settings(),
        },
        ChildOf(dog),
    ));
    commands.spawn((
        Name::new("Dog tail fur"),
        Transform::default(),
        HairGroom {
            emitters: vec![body_fur("tail_1", Some("tail_3"), 0.03, 70, along)],
            style: HairStyle {
                length: 0.11,
                segments: 4,
                stiffness: 0.5,
                ..fur
            },
            sim: HairGroom::hair_settings(),
        },
        ChildOf(dog),
    ));
}

/// Procedural walk and trot cycles.
pub fn animate_skeletons(mut rigs: Query<(&DemoSkeleton, &DemoClock, &mut SkeletonPose)>) {
    for (rig, clock, mut pose) in &mut rigs {
        let phase = clock.0 / rig.cycle.max(0.05) * TAU;
        pose_rig(rig.rig, phase, &mut pose);
    }
}

/// Put body-part meshes on their bones.
pub fn place_limbs(
    owners: Query<(&Transform, &SkeletonPose), Without<DemoLimb>>,
    mut limbs: Query<(&DemoLimb, &mut Transform)>,
) {
    for (limb, mut t) in &mut limbs {
        let Ok((root, pose)) = owners.get(limb.owner) else {
            continue;
        };
        let g = GlobalTransform::from(*root);
        let Some((a, ra)) = pose.bone_world(&g, limb.from) else {
            continue;
        };
        let offset = (ra * limb.offset).as_dvec3();
        match limb.to.and_then(|b| pose.bone_world(&g, b)) {
            Some((b, _)) => {
                let axis = (b - a).as_vec3();
                t.translation = (a + b) * 0.5 + offset;
                t.rotation = if axis.length_squared() > 1e-10 {
                    Quat::from_rotation_arc(Vec3::Y, axis.normalize())
                } else {
                    ra
                };
            }
            None => {
                t.translation = a + offset;
                t.rotation = ra;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rigs_have_valid_parents_and_keep_feet_near_the_ground() {
        for (bones, rig) in [(&HUMANOID[..], Rig::Humanoid), (&DOG[..], Rig::Dog)] {
            let mut pose = skeleton(bones, rig);
            for (i, p) in pose.parents.iter().enumerate() {
                assert!(p.is_none_or(|p| (p as usize) < i), "parent after child");
            }
            for k in 0..32 {
                pose_rig(rig, k as f32 / 32.0 * TAU, &mut pose);
                let lowest = pose
                    .bones
                    .iter()
                    .map(|b| b.translation.y)
                    .fold(f32::MAX, f32::min);
                assert!((-0.02..0.15).contains(&lowest), "{rig:?} lowest {lowest}");
                assert!(pose.bones.iter().all(|b| b.rotation.is_normalized()));
            }
        }
    }
}
