//! A giant procedural spider: eight legs driven by a [`LeggedGait`] (alternating tetrapods,
//! each leg's coxa yawing toward its foot), a bobbing abdomen, hairy legs with orange bands.

use engine_core::prelude::*;

use super::bone;
use super::sdf::{Body, Paint, Shape};
use crate::procedural::gait::{FootKind, GaitLeg, GaitSway, LegStyle, LeggedGait};
use crate::skeleton::Skeleton;
use crate::skin::SkinnedMesh;

pub const SKELETON: &str = "spider";
/// Leg angles from forward (degrees), front to back.
const ANGLES: [f32; 4] = [38.0, 72.0, 106.0, 142.0];
const BODY_HEIGHT: f32 = 0.55;

fn leg_name(side: usize, i: usize, part: &str) -> String {
    format!("leg_{}{}_{part}", ["l", "r"][side], i + 1)
}

/// Horizontal direction from the body to leg `i` on `side` (0 left, 1 right).
fn leg_dir(side: usize, i: usize) -> Vec3 {
    let a = ANGLES[i].to_radians();
    let s = if side == 0 { -1.0 } else { 1.0 };
    Vec3::new(a.sin() * s, 0.0, -a.cos())
}

/// Joint positions of one leg in the rest pose: hip, femur start, knee, ankle, foot tip.
fn leg_joints(side: usize, i: usize) -> [Vec3; 5] {
    let d = leg_dir(side, i);
    let reach = [1.0, 1.08, 1.0, 1.12][i];
    let hip = Vec3::new(0.0, BODY_HEIGHT, -0.05) + d * Vec3::new(0.17, 0.0, 0.21);
    let femur = hip + d * 0.07;
    let knee = femur + d * 0.32 * reach + Vec3::Y * 0.36;
    let ankle = knee + d * 0.42 * reach - Vec3::Y * 0.62;
    let tip = ankle + d * 0.14 * reach - Vec3::Y * (ankle.y - 0.02);
    [hip, femur, knee, ankle, tip]
}

pub fn skeleton() -> Skeleton {
    let i = Quat::IDENTITY;
    let mut bones = vec![
        bone("root", None, Vec3::ZERO, i),
        bone("body", Some("root"), Vec3::new(0.0, BODY_HEIGHT, 0.0), i),
        bone("abdomen", Some("body"), Vec3::new(0.0, 0.06, 0.2), i),
    ];
    let body = Vec3::new(0.0, BODY_HEIGHT, 0.0);
    for side in 0..2 {
        for leg in 0..4 {
            let [hip, femur, knee, ankle, tip] = leg_joints(side, leg);
            let coxa = leg_name(side, leg, "coxa");
            let fem = leg_name(side, leg, "femur");
            let tib = leg_name(side, leg, "tibia");
            let tar = leg_name(side, leg, "tarsus");
            bones.push(bone(&coxa, Some("body"), hip - body, i));
            bones.push(bone(&fem, Some(&coxa), femur - hip, i));
            bones.push(bone(&tib, Some(&fem), knee - femur, i));
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

pub fn body() -> Body {
    let fur = Paint::hide([0.03, 0.022, 0.017]);
    let band = Paint::hide([0.32, 0.11, 0.025]);
    let eye = Paint::hard([0.005, 0.005, 0.006]);
    let mut b = Body::default();
    let c = Vec3::new(0.0, BODY_HEIGHT, 0.0);
    b.part(
        "body",
        Shape::ellipsoid(c + Vec3::new(0.0, 0.0, -0.06), Vec3::new(0.2, 0.12, 0.25)),
        fur,
        0.04,
    )
    .part(
        "abdomen",
        Shape::ellipsoid(c + Vec3::new(0.0, 0.12, 0.48), Vec3::new(0.28, 0.24, 0.36)),
        fur,
        0.06,
    )
    .part(
        "abdomen",
        Shape::ellipsoid(c + Vec3::new(0.0, 0.3, 0.42), Vec3::new(0.14, 0.06, 0.22)),
        band,
        0.03,
    );
    // Eyes and fangs.
    for (x, y, z, r) in [
        (0.035, 0.1, -0.29, 0.03),
        (-0.035, 0.1, -0.29, 0.03),
        (0.08, 0.085, -0.26, 0.02),
        (-0.08, 0.085, -0.26, 0.02),
        (0.05, 0.125, -0.25, 0.017),
        (-0.05, 0.125, -0.25, 0.017),
    ] {
        b.part(
            "body",
            Shape::ellipsoid(c + Vec3::new(x, y, z), Vec3::splat(r)),
            eye,
            0.006,
        );
    }
    for x in [-0.05f32, 0.05] {
        b.part(
            "body",
            Shape::capsule(
                c + Vec3::new(x, -0.01, -0.28),
                c + Vec3::new(x * 0.7, -0.14, -0.33),
                0.04,
                0.015,
            ),
            fur,
            0.03,
        );
    }
    for side in 0..2 {
        for leg in 0..4 {
            let [hip, femur, knee, ankle, tip] = leg_joints(side, leg);
            b.part(
                &leg_name(side, leg, "coxa"),
                Shape::capsule(hip, femur, 0.05, 0.045),
                fur,
                0.03,
            )
            .part(
                &leg_name(side, leg, "femur"),
                Shape::capsule(femur, knee, 0.045, 0.035),
                fur,
                0.02,
            )
            .part(
                &leg_name(side, leg, "tibia"),
                Shape::ellipsoid(knee, Vec3::splat(0.042)),
                band,
                0.015,
            )
            .part(
                &leg_name(side, leg, "tibia"),
                Shape::capsule(knee, ankle, 0.035, 0.026),
                fur,
                0.02,
            )
            .part(
                &leg_name(side, leg, "tarsus"),
                Shape::ellipsoid(ankle, Vec3::splat(0.03)),
                band,
                0.012,
            )
            .part(
                &leg_name(side, leg, "tarsus"),
                Shape::capsule(ankle, tip, 0.024, 0.014),
                fur,
                0.015,
            );
        }
    }
    b
}

pub fn mesh() -> SkinnedMesh {
    body().build(&skeleton(), 0.016, 0.012)
}

/// Eight legs in alternating tetrapods.
pub fn gait() -> LeggedGait {
    let mut legs = Vec::new();
    for side in 0..2 {
        for leg in 0..4 {
            // L1 R2 L3 R4 step together, then R1 L2 R3 L4.
            let phase = if (side + leg) % 2 == 0 { 0.0 } else { 0.5 };
            legs.push(GaitLeg {
                bones: ["coxa", "femur", "tibia", "tip"]
                    .iter()
                    .map(|p| leg_name(side, leg, p))
                    .collect(),
                style: LegStyle::YawRoot,
                phase,
                pole: Vec3::Y,
                kind: FootKind::Foot,
                foot_size: [0.04, 0.06],
                sole: 0.02,
                stance_offset: Vec3::ZERO,
            });
        }
    }
    LeggedGait {
        body: "body".into(),
        legs,
        duty: 0.58,
        base_frequency: 1.1,
        frequency_per_speed: 0.9,
        step_height: 0.13,
        step_height_per_speed: 0.03,
        body_follow_ground: 0.9,
        body_tilt: 0.9,
        full_speed: 2.5,
        turn_radius: 0.7,
        mass_kg: 35.0,
        sway: vec![GaitSway {
            bone: "abdomen".into(),
            axis: Vec3::X,
            amplitude_deg: 4.0,
            frequency: 2.0,
            phase: 0.0,
        }],
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eight_legs_reach_the_ground() {
        let s = skeleton();
        for side in 0..2 {
            for leg in 0..4 {
                let tip = s.rest_model[s.bone(&leg_name(side, leg, "tip"))].t;
                assert!((tip.y - 0.02).abs() < 1e-4, "{tip}");
                assert!(tip.x.signum() == if side == 0 { -1.0 } else { 1.0 });
            }
        }
        let m = mesh();
        assert!(m.triangle_count() > 4000, "{}", m.triangle_count());
    }
}
