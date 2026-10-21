//! A procedural dog of Labrador build: a deep chest and tucked belly, drop ears and an otter
//! tail on springs, a furred coat (golden, lighter underneath, short on the face and legs),
//! four paws on a [`LeggedGait`] with walk, trot, canter and the dog's rotary gallop (the spine
//! flexing with every stride), and a panting, tail-wagging idle.
//!
//! Every function takes the dog's size as `scale` (1 = 0.56 m at the withers): the joints,
//! the coat, the step and the gaits' speeds and rhythm scale with it (dynamic similarity: speeds
//! grow with the square root of size, stride rates shrink with it), so the same rig serves a
//! terrier and a mastiff. Each size has its own skeleton name ([`skeleton_name`]); the idle clip
//! and graph only rotate bones, so every size shares them.

use engine_core::prelude::*;

use super::sdf::{Body, Paint, Shape};
use super::{PoseBuilder, bone, cycle_sin};
use crate::clip::AnimationClip;
use crate::graph::{AnimGraph, NodeDef};
use crate::muscle::{Muscle, MuscleDriver};
use crate::procedural::gait::{FootKind, GaitLeg, GaitStage, GaitSway, LegStyle, LeggedGait};
use crate::procedural::spring::SpringChainDef;
use crate::skeleton::Skeleton;
use crate::skin::{SkinMaterial, SkinnedMesh};

pub const SKELETON: &str = "dog";
pub const GRAPH: &str = "dog";
/// Height at the withers at scale 1, meters.
pub const WITHERS: f32 = 0.56;
const SIDE: [&str; 2] = ["l", "r"];
const X: [f32; 2] = [-0.075, 0.075];

/// The skeleton name for a dog of `scale` (sizes are kept to whole centimeters at the withers).
pub fn skeleton_name(scale: f32) -> String {
    let cm = (scale * WITHERS * 100.0).round() as i32;
    if cm == (WITHERS * 100.0).round() as i32 {
        SKELETON.into()
    } else {
        format!("{SKELETON}@{cm}cm")
    }
}

/// The scale a requested withers height asks for, rounded to whole centimeters.
pub fn scale_for_height(height: f32) -> f32 {
    let cm = (height.clamp(0.2, 1.0) * 100.0).round();
    cm / 100.0 / WITHERS
}

fn name(prefix: &str, part: &str, side: usize) -> String {
    format!("{prefix}_{part}_{}", SIDE[side])
}

/// Joint positions (model space, scale 1): the top of the shoulder blade, shoulder, elbow,
/// carpus, paw.
fn front_leg(side: usize) -> [Vec3; 5] {
    let x = X[side];
    [
        Vec3::new(x * 0.85, 0.5, -0.215),
        Vec3::new(x, 0.4, -0.29),
        Vec3::new(x * 1.07, 0.265, -0.2),
        Vec3::new(x, 0.1, -0.225),
        Vec3::new(x, 0.035, -0.24),
    ]
}

/// Hip, stifle, hock, paw.
fn hind_leg(side: usize) -> [Vec3; 4] {
    let x = X[side];
    [
        Vec3::new(x, 0.45, 0.22),
        Vec3::new(x * 1.13, 0.29, 0.15),
        Vec3::new(x, 0.13, 0.28),
        Vec3::new(x, 0.035, 0.265),
    ]
}

const BODY: Vec3 = Vec3::new(0.0, 0.44, -0.12);
const LOIN: Vec3 = Vec3::new(0.0, 0.46, 0.12);
const NECK: [Vec3; 3] = [
    Vec3::new(0.0, 0.5, -0.3),
    Vec3::new(0.0, 0.6, -0.37),
    Vec3::new(0.0, 0.69, -0.43),
];
const JAW: Vec3 = Vec3::new(0.0, 0.66, -0.465);
const EAR: Vec3 = Vec3::new(0.052, 0.745, -0.43);
const TAIL: [Vec3; 5] = [
    Vec3::new(0.0, 0.5, 0.31),
    Vec3::new(0.0, 0.45, 0.4),
    Vec3::new(0.0, 0.38, 0.47),
    Vec3::new(0.0, 0.3, 0.52),
    Vec3::new(0.0, 0.22, 0.55),
];

pub fn skeleton(scale: f32) -> Skeleton {
    let i = Quat::IDENTITY;
    let s = scale;
    let mut bones = vec![
        bone("root", None, Vec3::ZERO, i),
        bone("body", Some("root"), BODY * s, i),
        bone("loin", Some("body"), (LOIN - BODY) * s, i),
        bone("neck_1", Some("body"), (NECK[0] - BODY) * s, i),
        bone("neck_2", Some("neck_1"), (NECK[1] - NECK[0]) * s, i),
        bone("head", Some("neck_2"), (NECK[2] - NECK[1]) * s, i),
        bone("jaw", Some("head"), (JAW - NECK[2]) * s, i),
        bone("tail_1", Some("loin"), (TAIL[0] - LOIN) * s, i),
        bone("tail_2", Some("tail_1"), (TAIL[1] - TAIL[0]) * s, i),
        bone("tail_3", Some("tail_2"), (TAIL[2] - TAIL[1]) * s, i),
        bone("tail_4", Some("tail_3"), (TAIL[3] - TAIL[2]) * s, i),
    ];
    for side in 0..2 {
        let ear = Vec3::new(EAR.x * X[side].signum(), EAR.y, EAR.z);
        bones.push(bone(
            &format!("ear_{}", SIDE[side]),
            Some("head"),
            (ear - NECK[2]) * s,
            i,
        ));
        // The foreleg hangs from a shoulder blade that rocks with the stride.
        let [blade, shoulder, elbow, carpus, paw] = front_leg(side);
        let scapula = name("front", "scapula", side);
        bones.push(bone(&scapula, Some("body"), (blade - BODY) * s, i));
        for (prefix, parent, start, j) in [
            (
                "front",
                scapula.as_str(),
                blade,
                [shoulder, elbow, carpus, paw],
            ),
            ("hind", "loin", LOIN, hind_leg(side)),
        ] {
            let upper = name(prefix, "upper", side);
            let lower = name(prefix, "lower", side);
            let cannon = name(prefix, "cannon", side);
            bones.push(bone(&upper, Some(parent), (j[0] - start) * s, i));
            bones.push(bone(&lower, Some(&upper), (j[1] - j[0]) * s, i));
            bones.push(bone(&cannon, Some(&lower), (j[2] - j[1]) * s, i));
            bones.push(bone(
                &name(prefix, "paw", side),
                Some(&cannon),
                (j[3] - j[2]) * s,
                i,
            ));
        }
    }
    Skeleton::new(&skeleton_name(scale), bones).expect("dog skeleton")
}

/// A paw: the foot, four toes and dark pads underneath, around the paw joint `p` (scale 1).
fn paw(b: &mut Body, bone_name: &str, p: Vec3, coat: Paint, pad: Paint, s: f32) {
    let v = |x: f32, y: f32, z: f32| (p + Vec3::new(x, y, z)) * s;
    b.part(
        bone_name,
        Shape::ellipsoid(v(0.0, -0.008, -0.018), Vec3::new(0.026, 0.022, 0.034) * s),
        coat,
        0.008 * s,
    );
    for (k, dx) in [-0.017f32, -0.006, 0.006, 0.017].into_iter().enumerate() {
        let front = if k == 1 || k == 2 { -0.012 } else { 0.0 };
        b.part(
            bone_name,
            Shape::ellipsoid(
                v(dx, -0.02, -0.045 + front),
                Vec3::new(0.0105, 0.012, 0.014) * s,
            ),
            coat,
            0.004 * s,
        );
        // Claw.
        b.part(
            bone_name,
            Shape::capsule(
                v(dx * 1.05, -0.022, -0.056 + front),
                v(dx * 1.1, -0.03, -0.064 + front),
                0.0035 * s,
                0.0015 * s,
            ),
            Paint::hard([0.03, 0.025, 0.022]),
            0.002 * s,
        );
    }
    // Pads.
    b.part(
        bone_name,
        Shape::ellipsoid(v(0.0, -0.03, -0.022), Vec3::new(0.02, 0.006, 0.018) * s),
        pad,
        0.003 * s,
    );
}

pub fn body(scale: f32) -> Body {
    let s = scale;
    let v = |p: Vec3| p * s;
    let r = |x: f32, y: f32, z: f32| Vec3::new(x, y, z) * s;
    // A yellow Labrador: golden on the back, paler under the chest and belly, darker ears.
    let coat = Paint::fur([0.5, 0.31, 0.12], 1.0);
    let pale = Paint::fur([0.57, 0.38, 0.17], 0.75);
    let face = Paint::fur([0.5, 0.31, 0.12], 0.28);
    let muzzle = Paint::fur([0.55, 0.36, 0.15], 0.12);
    let leg = Paint::fur([0.52, 0.33, 0.13], 0.45);
    let foot = Paint::fur([0.54, 0.35, 0.14], 0.25);
    let ear = Paint::fur([0.43, 0.25, 0.09], 0.45);
    let nose = Paint::hard([0.013, 0.011, 0.011]);
    let eye = Paint::hard([0.035, 0.016, 0.008]);
    let pad = Paint::hard([0.03, 0.022, 0.02]);
    let tongue = Paint::skin([0.62, 0.17, 0.19]);
    let lip = Paint::hard([0.03, 0.02, 0.018]);
    let mut b = Body::default();

    // Trunk: rib cage, prosternum, shoulders over the withers, loin, tucked belly, croup.
    b.part(
        "body",
        Shape::ellipsoid(r(0.0, 0.43, -0.13), r(0.115, 0.145, 0.19)),
        coat,
        0.05 * s,
    )
    .part(
        "body",
        Shape::ellipsoid(r(0.0, 0.335, -0.17), r(0.09, 0.07, 0.13)),
        pale,
        0.05 * s,
    )
    .part(
        "body",
        Shape::ellipsoid(r(0.0, 0.42, -0.275), r(0.09, 0.115, 0.08)),
        pale,
        0.04 * s,
    )
    .part(
        "loin",
        Shape::ellipsoid(r(0.0, 0.47, 0.07), r(0.088, 0.085, 0.14)),
        coat,
        0.05 * s,
    )
    .part(
        "loin",
        Shape::ellipsoid(r(0.0, 0.405, 0.04), r(0.08, 0.07, 0.12)),
        pale,
        0.04 * s,
    )
    .part(
        "loin",
        Shape::ellipsoid(r(0.0, 0.47, 0.22), r(0.098, 0.09, 0.11)),
        coat,
        0.04 * s,
    );
    // Neck and head.
    b.part(
        "neck_1",
        Shape::capsule(
            r(0.0, 0.48, -0.27),
            r(0.0, 0.62, -0.38),
            0.088 * s,
            0.066 * s,
        ),
        coat,
        0.04 * s,
    )
    .part(
        "neck_1",
        Shape::ellipsoid(r(0.0, 0.5, -0.335), r(0.06, 0.065, 0.05)),
        pale,
        0.03 * s,
    )
    .part(
        "neck_2",
        Shape::capsule(
            r(0.0, 0.62, -0.38),
            r(0.0, 0.685, -0.425),
            0.066 * s,
            0.06 * s,
        ),
        coat,
        0.02 * s,
    )
    .part(
        "head",
        Shape::ellipsoid(r(0.0, 0.712, -0.44), r(0.066, 0.052, 0.075)),
        face,
        0.02 * s,
    )
    .part(
        "head",
        Shape::ellipsoid(r(0.0, 0.722, -0.494), r(0.042, 0.03, 0.03)),
        face,
        0.012 * s,
    )
    .part(
        "head",
        Shape::capsule(
            r(0.0, 0.69, -0.5),
            r(0.0, 0.686, -0.578),
            0.045 * s,
            0.037 * s,
        ),
        muzzle,
        0.014 * s,
    )
    .part(
        "head",
        Shape::ellipsoid(r(0.0, 0.664, -0.55), r(0.043, 0.026, 0.045)),
        muzzle,
        0.008 * s,
    )
    .part(
        "head",
        Shape::ellipsoid(r(0.0, 0.69, -0.608), r(0.027, 0.021, 0.02)),
        nose,
        0.006 * s,
    )
    // Lower jaw and the tongue lolling over its front.
    .part(
        "jaw",
        Shape::capsule(
            r(0.0, 0.648, -0.48),
            r(0.0, 0.645, -0.562),
            0.028 * s,
            0.021 * s,
        ),
        muzzle,
        0.006 * s,
    )
    .part(
        "jaw",
        Shape::ellipsoid(r(0.0, 0.642, -0.586), r(0.021, 0.0075, 0.035)),
        tongue,
        0.003 * s,
    );
    for side in 0..2 {
        let x = X[side].signum();
        b.part(
            "head",
            Shape::ellipsoid(r(0.034 * x, 0.692, -0.47), r(0.034, 0.034, 0.05)),
            face,
            0.014 * s,
        )
        // Brows over the eyes, then dark-rimmed eyes at the stop, looking forward.
        .part(
            "head",
            Shape::ellipsoid(r(0.034 * x, 0.735, -0.495), r(0.024, 0.0095, 0.019)),
            face,
            0.008 * s,
        )
        .part(
            "head",
            Shape::ellipsoid(r(0.04 * x, 0.714, -0.506), Vec3::splat(0.0138 * s)),
            lip,
            0.002 * s,
        )
        .part(
            "head",
            Shape::ellipsoid(r(0.042 * x, 0.714, -0.509), Vec3::splat(0.012 * s)),
            eye,
            0.001 * s,
        )
        // The lip line along the flews, from the corner of the mouth forward.
        .part(
            "head",
            Shape::capsule(
                r(0.038 * x, 0.654, -0.5),
                r(0.03 * x, 0.648, -0.57),
                0.0045 * s,
                0.004 * s,
            ),
            lip,
            0.003 * s,
        );
        // Drop ears hang from the top of the skull beside the cheeks, tipped slightly out.
        b.part(
            &format!("ear_{}", SIDE[side]),
            Shape::ellipsoid(r(0.066 * x, 0.69, -0.415), r(0.012, 0.048, 0.034))
                .rotated(Quat::from_rotation_z(0.25 * x)),
            ear,
            0.008 * s,
        );

        // Front leg: the shoulder blade slides over the chest wall; the leg below swings.
        let [_, sh, el, ca, pw] = front_leg(side);
        let f = |part: &str| name("front", part, side);
        b.part(
            &f("scapula"),
            Shape::ellipsoid(r(0.062 * x, 0.455, -0.24), r(0.042, 0.085, 0.065)),
            coat,
            0.04 * s,
        )
        .part(
            &f("upper"),
            Shape::capsule(v(sh), v(el), 0.05 * s, 0.04 * s),
            coat,
            0.025 * s,
        )
        .part(
            &f("upper"),
            Shape::ellipsoid(
                v(el + Vec3::new(0.005 * x, 0.05, 0.012)),
                r(0.04, 0.07, 0.05),
            ),
            coat,
            0.02 * s,
        )
        .part(
            &f("lower"),
            Shape::capsule(v(el), v(ca), 0.032 * s, 0.021 * s),
            leg,
            0.015 * s,
        )
        .part(
            &f("cannon"),
            Shape::ellipsoid(v(ca), r(0.023, 0.025, 0.023)),
            leg,
            0.01 * s,
        )
        .part(
            &f("cannon"),
            Shape::capsule(v(ca), v(pw), 0.021 * s, 0.022 * s),
            foot,
            0.008 * s,
        );
        paw(&mut b, &f("paw"), pw, foot, pad, s);

        // Hind leg: the haunch rides on the croup, the thigh and gaskin below swing.
        let [hip, st, ho, pw] = hind_leg(side);
        let h = |part: &str| name("hind", part, side);
        b.part(
            &h("upper"),
            Shape::ellipsoid(
                v(hip + Vec3::new(0.0, -0.065, 0.005)),
                r(0.052, 0.098, 0.085),
            ),
            coat,
            0.04 * s,
        )
        .part(
            &h("upper"),
            Shape::capsule(v(hip), v(st), 0.06 * s, 0.045 * s),
            coat,
            0.03 * s,
        )
        .part(
            &h("lower"),
            Shape::capsule(v(st), v(ho), 0.04 * s, 0.021 * s),
            leg,
            0.015 * s,
        )
        .part(
            &h("lower"),
            Shape::ellipsoid(v(st + Vec3::new(0.0, -0.05, 0.035)), r(0.034, 0.06, 0.038)),
            leg,
            0.012 * s,
        )
        .part(
            &h("cannon"),
            Shape::ellipsoid(v(ho + Vec3::new(0.0, 0.0, 0.012)), r(0.02, 0.025, 0.022)),
            leg,
            0.008 * s,
        )
        .part(
            &h("cannon"),
            Shape::capsule(v(ho), v(pw), 0.02 * s, 0.021 * s),
            foot,
            0.008 * s,
        );
        paw(&mut b, &h("paw"), pw, foot, pad, s);

        // Muscles.
        let n = |m: &str| format!("{m}_{}", SIDE[side]);
        b.muscle(
            &n("haunch"),
            r(0.085 * x, 0.4, 0.21),
            r(0.05, 0.1, 0.09),
            0.006 * s,
            r(0.0, -0.003, 0.0),
        )
        .muscle(
            &n("shoulder"),
            r(0.085 * x, 0.42, -0.25),
            r(0.045, 0.09, 0.07),
            0.004 * s,
            r(0.0, -0.002, 0.0),
        )
        .muscle(
            &n("forearm"),
            v(el + Vec3::new(0.012 * x, -0.05, -0.005)),
            r(0.028, 0.06, 0.03),
            0.003 * s,
            Vec3::ZERO,
        )
        .muscle(
            &n("gaskin"),
            v(st + Vec3::new(0.012 * x, -0.05, 0.035)),
            r(0.03, 0.06, 0.035),
            0.003 * s,
            Vec3::ZERO,
        );
    }
    // Otter tail: thick at the root, tapering to the tip.
    for (k, (ra, rb)) in [(0.034, 0.03), (0.03, 0.025), (0.025, 0.018), (0.018, 0.01)]
        .into_iter()
        .enumerate()
    {
        b.part(
            &format!("tail_{}", k + 1),
            Shape::capsule(v(TAIL[k]), v(TAIL[k + 1]), ra * s, rb * s),
            coat,
            0.012 * s,
        );
    }
    b.muscle(
        "neck",
        r(0.0, 0.55, -0.33),
        r(0.08, 0.08, 0.07),
        0.003 * s,
        Vec3::ZERO,
    )
    .muscle(
        "belly",
        r(0.0, 0.37, -0.05),
        r(0.08, 0.06, 0.16),
        0.002 * s,
        r(0.0, -0.008, 0.0),
    );
    b
}

pub fn mesh(scale: f32) -> SkinnedMesh {
    body(scale).build(&skeleton(scale), 0.0055 * scale, 0.014 * scale)
}

/// The coat and skin: shell fur over the golden hide.
pub fn material(scale: f32) -> SkinMaterial {
    SkinMaterial {
        skin_roughness: 0.62,
        subsurface: 0.25,
        sheen: 0.6,
        veins: 0.0,
        wrinkles: 0.0,
        fur_length: 0.013 * scale,
        fur_density: 750.0 / scale,
        fur_shells: 18,
        fur_lod: 7.0 * scale.sqrt(),
        fur_droop: 0.3,
        fur_comb: 0.8,
        fur_root: 0.42,
        fur_tip: [1.14, 1.1, 1.02],
        ..Default::default()
    }
}

/// Legs in the order front left, front right, hind left, hind right.
pub fn gait(scale: f32) -> LeggedGait {
    let s = scale;
    // Dynamic similarity: speeds scale with √size, rhythm with 1/√size.
    let speed = s.sqrt();
    let rate = 1.0 / speed;
    let mut legs = Vec::new();
    for prefix in ["front", "hind"] {
        for side in 0..2 {
            let (parts, style): (&[&str], _) = if prefix == "front" {
                (
                    &["scapula", "upper", "lower", "cannon", "paw"],
                    LegStyle::Scapular,
                )
            } else {
                (&["upper", "lower", "cannon", "paw"], LegStyle::Hoofed)
            };
            legs.push(GaitLeg {
                bones: parts.iter().map(|p| name(prefix, p, side)).collect(),
                style,
                phase: 0.0,
                pole: if prefix == "front" {
                    Vec3::Z
                } else {
                    Vec3::NEG_Z
                },
                kind: FootKind::Paw,
                foot_size: [0.055 * s, 0.07 * s],
                sole: 0.035 * s,
                // The hind paws stand behind the hips; their stride centers under them.
                stance_offset: if prefix == "front" {
                    Vec3::ZERO
                } else {
                    Vec3::new(0.0, 0.0, -0.04 * s)
                },
                ..Default::default()
            });
        }
    }
    let stage = |name: &str, min_speed: f32, offsets: [f32; 4], duty: f32, bob: f32, pitch: f32| {
        GaitStage {
            name: name.into(),
            min_speed: min_speed * speed,
            offsets: offsets.to_vec(),
            duty,
            bob: bob * s,
            pitch_deg: pitch,
        }
    };
    LeggedGait {
        body: "body".into(),
        legs,
        stages: vec![
            // Four-beat walk: LH, LF, RH, RF.
            stage("walk", 0.0, [0.25, 0.75, 0.0, 0.5], 0.62, 0.008, 1.5),
            // Trot: diagonal pairs.
            stage("trot", 1.7, [0.0, 0.5, 0.5, 0.0], 0.42, 0.016, 1.0),
            // Canter on the left lead: RH, then LH + RF, then LF.
            stage("canter", 4.2, [0.58, 0.32, 0.32, 0.0], 0.36, 0.025, 6.0),
            // Rotary gallop: LH, RH, then RF, LF, with a flight phase after each pair.
            stage("gallop", 6.5, [0.6, 0.48, 0.0, 0.1], 0.26, 0.03, 8.0),
        ],
        base_frequency: 1.35 * rate,
        frequency_per_speed: 0.4 * rate / speed,
        step_height: 0.045 * s,
        step_height_per_speed: 0.01 * s / speed,
        body_follow_ground: 0.8,
        body_tilt: 0.8,
        full_speed: 9.0 * speed,
        turn_radius: 0.3 * s,
        mass_kg: 30.0 * s * s * s,
        sway: vec![
            // The spine flexes and extends with each stride (strongest at the gallop).
            GaitSway {
                bone: "loin".into(),
                axis: Vec3::X,
                amplitude_deg: 9.0,
                frequency: 1.0,
                phase: 0.15,
            },
            GaitSway {
                bone: "neck_1".into(),
                axis: Vec3::X,
                amplitude_deg: 4.0,
                frequency: 2.0,
                phase: 0.0,
            },
            GaitSway {
                bone: "head".into(),
                axis: Vec3::X,
                amplitude_deg: -3.0,
                frequency: 2.0,
                phase: 0.1,
            },
        ],
        ..Default::default()
    }
}

/// Muscles matching [`mesh`]'s targets. Leg loads come from the gait (`gait.load.N`).
pub fn muscles() -> Vec<Muscle> {
    let input = |name: &str, weight: f32| MuscleDriver::Input {
        name: name.into(),
        weight,
    };
    let muscle = |name: &str, bone: &str, drivers: Vec<MuscleDriver>| Muscle {
        name: name.into(),
        bulge: name.into(),
        sway: Some(format!("{name}.sway")),
        drivers,
        bone: bone.into(),
        rise_secs: 0.05,
        fall_secs: 0.25,
        ..Default::default()
    };
    let mut out = Vec::new();
    for (side, suffix) in SIDE.iter().enumerate() {
        let n = |m: &str| format!("{m}_{suffix}");
        let front_load = format!("gait.load.{side}");
        let hind_load = format!("gait.load.{}", side + 2);
        out.extend([
            muscle(
                &n("haunch"),
                &name("hind", "upper", side),
                vec![input(&hind_load, 0.55), input("gait.intensity", 0.55)],
            ),
            muscle(
                &n("shoulder"),
                &name("front", "upper", side),
                vec![input(&front_load, 0.45), input("gait.intensity", 0.45)],
            ),
            muscle(
                &n("forearm"),
                &name("front", "lower", side),
                vec![input(&front_load, 0.6), input("gait.intensity", 0.3)],
            ),
            muscle(
                &n("gaskin"),
                &name("hind", "lower", side),
                vec![input(&hind_load, 0.6), input("gait.intensity", 0.3)],
            ),
        ]);
    }
    out.push(muscle("neck", "neck_1", vec![input("gait.intensity", 0.6)]));
    out.push(Muscle {
        jiggle_hz: 3.5,
        jiggle_damping: 0.14,
        tone: 0.0,
        ..muscle("belly", "body", Vec::new())
    });
    out
}

/// The tail and the two ears, on springs.
pub fn springs(scale: f32) -> Vec<SpringChainDef> {
    let tail = SpringChainDef {
        bones: (1..=4).map(|k| format!("tail_{k}")).collect(),
        stiffness: 0.12,
        damping: 0.86,
        gravity: 0.25,
        wind: 0.3,
        tip_length: 0.09 * scale,
    };
    let ears = SIDE.iter().map(|side| SpringChainDef {
        bones: vec![format!("ear_{side}")],
        stiffness: 0.3,
        damping: 0.8,
        gravity: 0.4,
        wind: 0.4,
        tip_length: 0.08 * scale,
    });
    std::iter::once(tail).chain(ears).collect()
}

/// Idle: panting (the jaw drops and the chest heaves), looking about, ears flicking, and a
/// wagging tail. Rotations only, so dogs of every size share it.
fn idle(s: &Skeleton) -> AnimationClip {
    AnimationClip::from_fn("dog_idle", 6.0, 181, true, |ph| {
        let mut p = PoseBuilder::new(s);
        let look = cycle_sin(ph);
        // 2.5 pants a second, 2 wags a second.
        let pant = cycle_sin(ph * 15.0);
        let wag = cycle_sin(ph * 12.0);
        p.rot("jaw", 9.0 + 4.0 * pant, 0.0, 0.0)
            .rot("body", 0.6 * pant, 0.0, 0.0)
            .rot("neck_1", -4.0 + 1.0 * pant, 6.0 * look, 0.0)
            .rot("neck_2", 2.0, 6.0 * look, 2.0 * look)
            .rot(
                "head",
                4.0 + 3.0 * cycle_sin(ph * 2.0),
                10.0 * look,
                -4.0 * look,
            )
            .rot("ear_l", 0.0, 0.0, 4.0 * cycle_sin(ph * 3.0 + 0.2))
            .rot("ear_r", 0.0, 0.0, -4.0 * cycle_sin(ph * 3.0))
            .rot("tail_1", 18.0, 26.0 * wag, 0.0)
            .rot("tail_2", -6.0, 18.0 * cycle_sin(ph * 12.0 - 0.08), 0.0)
            .rot("tail_3", -6.0, 12.0 * cycle_sin(ph * 12.0 - 0.16), 0.0);
        p.take()
    })
}

pub fn clips(s: &Skeleton) -> Vec<AnimationClip> {
    vec![idle(s)]
}

pub fn graph() -> AnimGraph {
    AnimGraph {
        name: GRAPH.into(),
        root: NodeDef::clip("dog_idle"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::muscle::MorphWeights;

    #[test]
    fn dog_stands_on_its_paws_at_its_size() {
        for scale in [1.0, 0.6] {
            let s = skeleton(scale);
            for side in 0..2 {
                for prefix in ["front", "hind"] {
                    let p = s.rest_model[s.bone(&name(prefix, "paw", side))].t;
                    assert!(
                        (p.y - 0.035 * scale).abs() < 1e-4,
                        "{prefix} paw at {}",
                        p.y
                    );
                }
            }
            let mesh = mesh(scale);
            let (lowest, highest) = mesh.vertices.iter().fold((f32::MAX, f32::MIN), |a, v| {
                (a.0.min(v.position.y), a.1.max(v.position.y))
            });
            assert!(lowest.abs() < 0.01 * scale, "paws end at {lowest}");
            // The head stands above the withers.
            assert!(
                (0.7 * scale..0.82 * scale).contains(&highest),
                "top of the head at {highest}"
            );
            let morphs = MorphWeights::new(mesh.morph_names());
            for m in muscles() {
                assert!(morphs.index_of(&m.bulge).is_some(), "{}", m.bulge);
            }
            // Fur on the coat, none on the nose, eyes and pads.
            let furred = mesh.vertices.iter().filter(|v| v.fur > 0.5).count();
            let bare = mesh.vertices.iter().filter(|v| v.fur < 0.05).count();
            assert!(furred > mesh.vertices.len() / 3, "{furred} furred vertices");
            assert!(bare > 100, "{bare} bare vertices");
        }
        assert_eq!(skeleton_name(1.0), "dog");
        assert_eq!(skeleton_name(scale_for_height(0.56)), "dog");
        assert_eq!(skeleton_name(scale_for_height(0.351)), "dog@35cm");
    }
}
