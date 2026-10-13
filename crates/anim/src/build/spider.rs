//! A giant procedural tarantula, built like a Mexican red-knee: a low carapace with a tan rim,
//! a dimpled fovea and radiating grooves, eight eyes on a turret, furred chelicerae with fangs
//! folded underneath, pedipalps that tap the ground, a long-haired abdomen on a narrow waist
//! with spinnerets, and eight stout hairy legs with orange knees and pale joint rings.
//!
//! The legs walk on a [`LeggedGait`] in alternating tetrapods (L1 R2 L3 R4, then R1 L2 R3 L4)
//! with a wave running from the back legs forward, each coxa yawing toward its foot; the
//! abdomen bobs with the steps and lags on a spring. The idle clip works the palps and the
//! chelicerae and breathes, and keeps doing so while the legs walk.

use engine_core::prelude::*;

use super::sdf::{Body, Paint, Shape};
use super::{PoseBuilder, bone, cycle_sin};
use crate::clip::AnimationClip;
use crate::graph::{AnimGraph, NodeDef};
use crate::procedural::gait::{FootKind, GaitLeg, GaitSway, LegStyle, LeggedGait};
use crate::procedural::spring::SpringChainDef;
use crate::skeleton::Skeleton;
use crate::skin::{SkinMaterial, SkinnedMesh};

pub const SKELETON: &str = "spider";
pub const GRAPH: &str = "spider";
/// Leg angles from forward (degrees), front to back.
const ANGLES: [f32; 4] = [36.0, 70.0, 108.0, 145.0];
/// Leg length factors, front to back (the first and last pairs are the longest).
const REACH: [f32; 4] = [1.06, 0.96, 0.95, 1.1];
/// Height of the body bone (the middle of the carapace's underside), meters: a tarantula
/// carries its body low, the knees arching not far above the carapace.
const BODY_HEIGHT: f32 = 0.3;
/// Height of a planted leg tip above the ground, meters.
const SOLE: f32 = 0.02;
const SIDE: [&str; 2] = ["l", "r"];

fn leg_name(side: usize, i: usize, part: &str) -> String {
    format!("leg_{}{}_{part}", SIDE[side], i + 1)
}

fn palp_name(side: usize, part: &str) -> String {
    format!("palp_{}_{part}", SIDE[side])
}

/// Horizontal direction from the body to leg `i` on `side` (0 left, 1 right).
fn leg_dir(side: usize, i: usize) -> Vec3 {
    let a = ANGLES[i].to_radians();
    let s = if side == 0 { -1.0 } else { 1.0 };
    Vec3::new(a.sin() * s, 0.0, -a.cos())
}

/// One leg in the rest pose: hip (coxa), trochanter (femur bone), knee (tibia bone; the
/// patella arches over it), end of the patella, ankle (tarsus bone: metatarsus and tarsus),
/// tip.
fn leg_joints(side: usize, i: usize) -> [Vec3; 6] {
    let d = leg_dir(side, i);
    let r = REACH[i];
    let hip = Vec3::new(0.0, BODY_HEIGHT - 0.03, -0.04) + d * Vec3::new(0.15, 0.0, 0.18);
    let troch = hip + d * 0.08 - Vec3::Y * 0.01;
    let knee = troch + d * 0.25 * r + Vec3::Y * 0.22;
    let patella = knee + d * 0.08 * r - Vec3::Y * 0.02;
    let ankle = patella + d * 0.3 * r - Vec3::Y * 0.3;
    let tip = ankle + d * 0.2 * r - Vec3::Y * (ankle.y - SOLE);
    [hip, troch, knee, patella, ankle, tip]
}

/// A pedipalp in the rest pose: base, trochanter, knee, ankle, tip (held just off the ground).
fn palp_joints(side: usize) -> [Vec3; 5] {
    let x = if side == 0 { -1.0 } else { 1.0 };
    let h = BODY_HEIGHT;
    [
        Vec3::new(0.07 * x, h - 0.03, -0.22),
        Vec3::new(0.1 * x, h - 0.02, -0.29),
        Vec3::new(0.14 * x, h + 0.09, -0.37),
        Vec3::new(0.16 * x, h - 0.12, -0.45),
        Vec3::new(0.16 * x, 0.05, -0.49),
    ]
}

pub fn skeleton() -> Skeleton {
    let i = Quat::IDENTITY;
    let body = Vec3::new(0.0, BODY_HEIGHT, 0.0);
    let waist = Vec3::new(0.0, BODY_HEIGHT + 0.04, 0.22);
    let mut bones = vec![
        bone("root", None, Vec3::ZERO, i),
        bone("body", Some("root"), body, i),
        bone("abdomen", Some("body"), waist - body, i),
    ];
    for (side, s) in SIDE.iter().enumerate() {
        let x = if side == 0 { -1.0 } else { 1.0 };
        let base = Vec3::new(0.042 * x, BODY_HEIGHT + 0.02, -0.22);
        let fang = Vec3::new(0.04 * x, BODY_HEIGHT - 0.05, -0.33);
        bones.push(bone(
            &format!("chelicera_{s}"),
            Some("body"),
            base - body,
            i,
        ));
        bones.push(bone(
            &format!("fang_{s}"),
            Some(&format!("chelicera_{s}")),
            fang - base,
            i,
        ));
        let p = palp_joints(side);
        let mut parent = "body".to_string();
        let mut at = body;
        for (k, part) in ["coxa", "femur", "tibia", "tarsus", "tip"]
            .iter()
            .enumerate()
        {
            let name = palp_name(side, part);
            bones.push(bone(&name, Some(&parent), p[k] - at, i));
            parent = name;
            at = p[k];
        }
        for leg in 0..4 {
            let [hip, troch, knee, _, ankle, tip] = leg_joints(side, leg);
            let coxa = leg_name(side, leg, "coxa");
            let fem = leg_name(side, leg, "femur");
            let tib = leg_name(side, leg, "tibia");
            let tar = leg_name(side, leg, "tarsus");
            bones.push(bone(&coxa, Some("body"), hip - body, i));
            bones.push(bone(&fem, Some(&coxa), troch - hip, i));
            bones.push(bone(&tib, Some(&fem), knee - troch, i));
            bones.push(bone(&tar, Some(&tib), ankle - knee, i));
            bones.push(bone(
                &leg_name(side, leg, "tip"),
                Some(&tar),
                tip - ankle,
                i,
            ));
        }
    }
    Skeleton::new(SKELETON, bones).expect("spider skeleton")
}

/// A thin band around a limb at `at`, along `axis`.
fn ring(at: Vec3, axis: Vec3, radius: f32) -> Shape {
    Shape::cylinder(at, axis, radius, 0.007, 0.004)
}

pub fn body() -> Body {
    // Linear colors: black legs and body, orange-red knees, pale rings, a tan carapace rim
    // and a long, reddish-brown coat on the abdomen.
    let black = Paint::fur([0.018, 0.014, 0.012], 0.7);
    let leg_black = Paint::fur([0.02, 0.015, 0.012], 0.6);
    let knee = Paint::fur([0.44, 0.12, 0.022], 0.7);
    let pale = Paint::fur([0.36, 0.24, 0.13], 0.4);
    let carapace = Paint::fur([0.03, 0.021, 0.015], 0.3);
    let rim = Paint::fur([0.4, 0.2, 0.06], 0.45);
    let groove = Paint::fur([0.012, 0.01, 0.009], 0.15);
    let abdomen = Paint::fur([0.055, 0.027, 0.016], 1.0);
    let pad = Paint::hide([0.016, 0.013, 0.012]);
    let eye = Paint::hard([0.003, 0.003, 0.004]);
    let fang = Paint::hard([0.012, 0.009, 0.008]);
    let h = BODY_HEIGHT;
    let mut b = Body::default();

    // Carapace: low, higher over the head, a tan rim at its edge; the sternum underneath.
    b.part(
        "body",
        Shape::ellipsoid(
            Vec3::new(0.0, h + 0.01, -0.05),
            Vec3::new(0.19, 0.075, 0.23),
        ),
        carapace,
        0.03,
    )
    .part(
        "body",
        Shape::ellipsoid(
            Vec3::new(0.0, h + 0.035, -0.13),
            Vec3::new(0.12, 0.06, 0.12),
        ),
        carapace,
        0.05,
    )
    .part(
        "body",
        Shape::ellipsoid(
            Vec3::new(0.0, h - 0.004, -0.05),
            Vec3::new(0.2, 0.034, 0.24),
        ),
        rim,
        0.012,
    )
    .part(
        "body",
        Shape::ellipsoid(
            Vec3::new(0.0, h - 0.045, -0.04),
            Vec3::new(0.13, 0.04, 0.16),
        ),
        black,
        0.03,
    );
    // The fovea, and grooves radiating from it toward each leg.
    b.cut(
        "body",
        Shape::ellipsoid(
            Vec3::new(0.0, h + 0.085, -0.02),
            Vec3::new(0.02, 0.012, 0.013),
        ),
        groove,
        0.01,
    );
    for side in 0..2 {
        for leg in 0..4 {
            let d = leg_dir(side, leg);
            let from = Vec3::new(0.0, h + 0.08, -0.02) + d * 0.04;
            let to = Vec3::new(0.0, h + 0.05, -0.05) + d * Vec3::new(0.14, 0.0, 0.16);
            b.cut(
                "body",
                Shape::capsule(from, to, 0.006, 0.004),
                groove,
                0.008,
            );
        }
    }
    // Eyes on a turret at the front of the head: two large, six small.
    b.part(
        "body",
        Shape::ellipsoid(
            Vec3::new(0.0, h + 0.085, -0.2),
            Vec3::new(0.036, 0.026, 0.03),
        ),
        carapace,
        0.02,
    );
    for (x, y, z, r) in [
        (0.012, 0.108, -0.226, 0.013),
        (0.029, 0.1, -0.222, 0.009),
        (0.014, 0.114, -0.204, 0.008),
        (0.031, 0.105, -0.199, 0.009),
    ] {
        for sx in [-1.0, 1.0] {
            b.part(
                "body",
                Shape::ellipsoid(Vec3::new(x * sx, h + y, z), Vec3::splat(r)),
                eye,
                0.003,
            );
        }
    }

    for (side, s) in SIDE.iter().enumerate() {
        let x = if side == 0 { -1.0 } else { 1.0 };
        // Chelicerae: furred jaws tipped down at the front, fangs folded back beneath them.
        b.part(
            &format!("chelicera_{s}"),
            Shape::ellipsoid(
                Vec3::new(0.043 * x, h, -0.275),
                Vec3::new(0.044, 0.05, 0.075),
            )
            .rotated(Quat::from_rotation_x(-0.4)),
            black,
            0.02,
        )
        .part(
            &format!("fang_{s}"),
            Shape::capsule(
                Vec3::new(0.04 * x, h - 0.05, -0.335),
                Vec3::new(0.034 * x, h - 0.066, -0.27),
                0.012,
                0.004,
            ),
            fang,
            0.004,
        );

        // Pedipalps: short legs beside the jaws, orange at the knee.
        let p = palp_joints(side);
        let palp_patella = p[2] + (p[3] - p[2]).normalize() * 0.05;
        b.part(
            &palp_name(side, "coxa"),
            Shape::capsule(p[0], p[1], 0.032, 0.027),
            black,
            0.02,
        )
        .part(
            &palp_name(side, "femur"),
            Shape::capsule(p[1], p[2], 0.026, 0.023),
            leg_black,
            0.012,
        )
        .part(
            &palp_name(side, "tibia"),
            Shape::capsule(p[2], palp_patella, 0.025, 0.023),
            knee,
            0.008,
        )
        .part(
            &palp_name(side, "tibia"),
            Shape::capsule(palp_patella, p[3], 0.022, 0.019),
            leg_black,
            0.008,
        )
        .part(
            &palp_name(side, "tarsus"),
            Shape::capsule(p[3], p[4], 0.019, 0.018),
            leg_black,
            0.01,
        );

        // Legs: stout and hairy, the patella orange-red, pale rings at the joints, a dark
        // pad of dense hair under each tip.
        for leg in 0..4 {
            let [hip, troch, kn, patella, ankle, tip] = leg_joints(side, leg);
            let femur_axis = (kn - troch).normalize();
            let tibia_axis = (ankle - patella).normalize();
            let foot_axis = (tip - ankle).normalize();
            b.part(
                &leg_name(side, leg, "coxa"),
                Shape::capsule(hip, troch, 0.042, 0.036),
                black,
                0.025,
            )
            .part(
                &leg_name(side, leg, "femur"),
                Shape::capsule(troch, kn, 0.034, 0.029),
                leg_black,
                0.015,
            )
            .part(
                &leg_name(side, leg, "femur"),
                ring(kn - femur_axis * 0.012, femur_axis, 0.031),
                pale,
                0.004,
            )
            .part(
                &leg_name(side, leg, "tibia"),
                Shape::capsule(kn, patella, 0.032, 0.03),
                knee,
                0.01,
            )
            .part(
                &leg_name(side, leg, "tibia"),
                Shape::capsule(patella, ankle, 0.028, 0.023),
                leg_black,
                0.012,
            )
            .part(
                &leg_name(side, leg, "tibia"),
                ring(patella + tibia_axis * 0.03, tibia_axis, 0.029),
                knee,
                0.004,
            )
            .part(
                &leg_name(side, leg, "tibia"),
                ring(ankle - tibia_axis * 0.014, tibia_axis, 0.025),
                pale,
                0.004,
            )
            .part(
                &leg_name(side, leg, "tarsus"),
                Shape::capsule(ankle, tip, 0.021, 0.015),
                leg_black,
                0.012,
            )
            .part(
                &leg_name(side, leg, "tarsus"),
                Shape::capsule(tip - foot_axis * 0.05 - Vec3::Y * 0.004, tip, 0.016, 0.014),
                pad,
                0.006,
            );
        }
    }

    // A narrow waist, then the abdomen: egg-shaped under a long coat, spinnerets at the end.
    b.part(
        "abdomen",
        Shape::capsule(
            Vec3::new(0.0, h + 0.01, 0.16),
            Vec3::new(0.0, h + 0.04, 0.25),
            0.035,
            0.04,
        ),
        black,
        0.02,
    )
    .part(
        "abdomen",
        Shape::ellipsoid(Vec3::new(0.0, h + 0.09, 0.5), Vec3::new(0.25, 0.21, 0.31)),
        abdomen,
        0.05,
    )
    .part(
        "abdomen",
        Shape::ellipsoid(Vec3::new(0.0, h + 0.07, 0.6), Vec3::new(0.22, 0.18, 0.24)),
        abdomen,
        0.08,
    );
    for x in [-1.0f32, 1.0] {
        b.part(
            "abdomen",
            Shape::capsule(
                Vec3::new(0.03 * x, h + 0.02, 0.79),
                Vec3::new(0.045 * x, h, 0.86),
                0.02,
                0.012,
            ),
            black,
            0.015,
        );
    }
    b
}

pub fn mesh() -> SkinnedMesh {
    body().build(&skeleton(), 0.007, 0.012)
}

/// Long, stiff, sparse hairs that catch the light orange-brown at their tips.
pub fn material() -> SkinMaterial {
    SkinMaterial {
        skin_roughness: 0.7,
        subsurface: 0.05,
        sheen: 0.5,
        veins: 0.0,
        wrinkles: 0.0,
        fur_length: 0.03,
        fur_density: 380.0,
        fur_shells: 20,
        fur_lod: 9.0,
        fur_droop: 0.2,
        fur_comb: 0.15,
        fur_root: 0.5,
        fur_tip: [1.8, 1.4, 1.2],
        ..Default::default()
    }
}

/// Eight legs in alternating tetrapods, with a wave from the back legs forward.
pub fn gait() -> LeggedGait {
    let mut legs = Vec::new();
    for side in 0..2 {
        for leg in 0..4 {
            // L1 R2 L3 R4 step together, then R1 L2 R3 L4; within each set the back leg
            // lifts first.
            let set = if (side + leg) % 2 == 0 { 0.0 } else { 0.5 };
            legs.push(GaitLeg {
                bones: ["coxa", "femur", "tibia", "tip"]
                    .iter()
                    .map(|p| leg_name(side, leg, p))
                    .collect(),
                style: LegStyle::YawRoot,
                phase: set + 0.05 * (3 - leg) as f32,
                pole: Vec3::Y,
                kind: FootKind::Foot,
                foot_size: [0.04, 0.07],
                sole: SOLE,
                stance_offset: Vec3::ZERO,
            });
        }
    }
    LeggedGait {
        body: "body".into(),
        legs,
        duty: 0.6,
        base_frequency: 1.0,
        frequency_per_speed: 0.8,
        step_height: 0.09,
        step_height_per_speed: 0.03,
        body_follow_ground: 0.9,
        body_tilt: 0.9,
        full_speed: 2.5,
        turn_radius: 0.8,
        mass_kg: 40.0,
        sway: vec![GaitSway {
            bone: "abdomen".into(),
            axis: Vec3::X,
            amplitude_deg: 3.0,
            frequency: 2.0,
            phase: 0.0,
        }],
        ..Default::default()
    }
}

/// The abdomen hangs from the waist on a spring, so it lags when the spider starts and stops.
pub fn springs() -> Vec<SpringChainDef> {
    vec![SpringChainDef {
        bones: vec!["abdomen".into()],
        stiffness: 0.3,
        damping: 0.82,
        gravity: 0.1,
        wind: 0.0,
        tip_length: 0.5,
    }]
}

/// Idle: the palps tap the ground in turn, the chelicerae work, and the abdomen breathes.
/// Only the palps, jaws and abdomen move, so the walking legs keep it running.
fn idle(s: &Skeleton) -> AnimationClip {
    AnimationClip::from_fn("spider_idle", 8.0, 241, true, |ph| {
        let mut p = PoseBuilder::new(s);
        // Six taps per palp in a loop, the two palps half a tap apart.
        let tap_l = cycle_sin(ph * 6.0).max(0.0);
        let tap_r = cycle_sin(ph * 6.0 + 0.5).max(0.0);
        let chew = cycle_sin(ph * 4.0);
        p.rot("palp_l_femur", 14.0 * tap_l, 4.0 * tap_l, 0.0)
            .rot("palp_l_tibia", -10.0 * tap_l, 0.0, 0.0)
            .rot("palp_r_femur", 14.0 * tap_r, -4.0 * tap_r, 0.0)
            .rot("palp_r_tibia", -10.0 * tap_r, 0.0, 0.0)
            .rot("chelicera_l", 3.0 * chew, 0.0, 0.0)
            .rot("chelicera_r", -3.0 * chew, 0.0, 0.0)
            .rot("fang_l", 6.0 * chew.max(0.0), 0.0, 0.0)
            .rot("fang_r", 6.0 * (-chew).max(0.0), 0.0, 0.0)
            .rot("abdomen", 1.2 * cycle_sin(ph * 3.0), 0.0, 0.0);
        p.take()
    })
}

pub fn clips(s: &Skeleton) -> Vec<AnimationClip> {
    vec![idle(s)]
}

pub fn graph() -> AnimGraph {
    AnimGraph {
        name: GRAPH.into(),
        root: NodeDef::clip("spider_idle"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eight_legs_reach_the_ground_and_the_palps_hang_beside_the_jaws() {
        let s = skeleton();
        for side in 0..2 {
            for leg in 0..4 {
                let tip = s.rest_model[s.bone(&leg_name(side, leg, "tip"))].t;
                assert!((tip.y - SOLE).abs() < 1e-4, "{tip}");
                assert!(tip.x.signum() == if side == 0 { -1.0 } else { 1.0 });
                // The knee is the highest point of the leg, above the body.
                let knee = s.rest_model[s.bone(&leg_name(side, leg, "tibia"))].t;
                assert!(knee.y > BODY_HEIGHT + 0.12, "knee at {}", knee.y);
            }
            let palp = s.rest_model[s.bone(&palp_name(side, "tip"))].t;
            assert!(palp.y > SOLE && palp.y < 0.12, "palp tip at {}", palp.y);
            assert!(palp.z < -0.4, "palp ahead of the body: {palp}");
        }
        let m = mesh();
        assert!(m.triangle_count() > 30_000, "{}", m.triangle_count());
        // Hair on the legs and abdomen, none on the eyes and fangs.
        let furred = m.vertices.iter().filter(|v| v.fur > 0.6).count();
        let bare = m.vertices.iter().filter(|v| v.fur < 0.05).count();
        assert!(furred > m.vertices.len() / 2, "{furred} furred vertices");
        assert!(bare > 100, "{bare} bare vertices");
        let lowest = m
            .vertices
            .iter()
            .map(|v| v.position.y)
            .fold(f32::MAX, f32::min);
        assert!(lowest.abs() < 0.01, "legs end at {lowest}");
    }

    #[test]
    fn the_idle_moves_only_palps_jaws_and_abdomen() {
        let clip = idle(&skeleton());
        for track in &clip.tracks {
            let name = &track.bone;
            assert!(
                name.starts_with("palp_")
                    || name.starts_with("chelicera_")
                    || name.starts_with("fang_")
                    || name == "abdomen",
                "the idle moves {name}"
            );
        }
    }
}
