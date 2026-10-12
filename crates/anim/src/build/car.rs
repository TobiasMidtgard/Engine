//! A small procedural car: body and four wheels as one rigidly skinned mesh, animated by a
//! [`VehicleRig`] (suspension, body roll and pitch, steering, wheel spin).

use engine_core::prelude::*;

use super::bone;
use super::sdf::{Body, Paint, Shape};
use crate::procedural::vehicle::{VehicleRig, WheelRig};
use crate::skeleton::Skeleton;
use crate::skin::SkinnedMesh;

pub const SKELETON: &str = "car";
pub const WHEEL_RADIUS: f32 = 0.34;
const WHEELS: [(&str, f32, f32); 4] = [
    ("wheel_fl", -0.82, -1.32),
    ("wheel_fr", 0.82, -1.32),
    ("wheel_rl", -0.82, 1.28),
    ("wheel_rr", 0.82, 1.28),
];

pub fn skeleton() -> Skeleton {
    let i = Quat::IDENTITY;
    let mut bones = vec![
        bone("root", None, Vec3::ZERO, i),
        bone("body", Some("root"), Vec3::new(0.0, 0.62, 0.0), i),
    ];
    for (name, x, z) in WHEELS {
        bones.push(bone(
            name,
            Some("body"),
            Vec3::new(x, WHEEL_RADIUS - 0.62, z),
            i,
        ));
    }
    Skeleton::new(SKELETON, bones).expect("car skeleton")
}

/// The car body in a paint color (linear RGB).
pub fn body(paint: [f32; 3]) -> Body {
    let car = Paint {
        color: Vec3::from(paint).extend(1.0),
        mask: Vec4::new(0.0, 0.0, 0.0, 0.6),
    };
    let glass = Paint::hard([0.02, 0.025, 0.03]);
    let trim = Paint::hard([0.03, 0.03, 0.032]);
    let light = Paint::hard([0.95, 0.92, 0.8]);
    let tail = Paint::hard([0.6, 0.02, 0.02]);
    let rubber = Paint::hard([0.025, 0.025, 0.025]);
    let rim = Paint::hard([0.55, 0.56, 0.58]);
    let mut b = Body::default();
    b.part(
        "body",
        Shape::round_box(Vec3::new(0.0, 0.66, 0.0), Vec3::new(0.86, 0.26, 2.05), 0.2),
        car,
        0.0,
    )
    .part(
        "body",
        Shape::round_box(Vec3::new(0.0, 1.06, 0.25), Vec3::new(0.74, 0.3, 1.05), 0.26),
        car,
        0.18,
    )
    // Windows: a dark band around the cabin.
    .part(
        "body",
        Shape::round_box(
            Vec3::new(0.0, 1.12, 0.25),
            Vec3::new(0.752, 0.16, 1.066),
            0.14,
        ),
        glass,
        0.01,
    )
    // Bumpers, lights.
    .part(
        "body",
        Shape::round_box(
            Vec3::new(0.0, 0.47, -2.06),
            Vec3::new(0.84, 0.08, 0.06),
            0.05,
        ),
        trim,
        0.03,
    )
    .part(
        "body",
        Shape::round_box(
            Vec3::new(0.0, 0.47, 2.06),
            Vec3::new(0.84, 0.08, 0.06),
            0.05,
        ),
        trim,
        0.03,
    );
    for x in [-0.6f32, 0.6] {
        b.part(
            "body",
            Shape::ellipsoid(Vec3::new(x, 0.72, -2.04), Vec3::new(0.15, 0.06, 0.04)),
            light,
            0.01,
        )
        .part(
            "body",
            Shape::ellipsoid(Vec3::new(x, 0.74, 2.04), Vec3::new(0.15, 0.05, 0.04)),
            tail,
            0.01,
        );
    }
    // Wheel arches.
    for (_, x, z) in WHEELS {
        b.cut(
            "body",
            Shape::cylinder(
                Vec3::new(x * 1.04, WHEEL_RADIUS, z),
                Vec3::X,
                WHEEL_RADIUS + 0.07,
                0.3,
                0.03,
            ),
            trim,
            0.03,
        );
    }
    for (name, x, z) in WHEELS {
        let c = Vec3::new(x * 1.04, WHEEL_RADIUS, z);
        b.part(
            name,
            Shape::cylinder(c, Vec3::X, WHEEL_RADIUS, 0.11, 0.06),
            rubber,
            0.0,
        )
        .part(
            name,
            Shape::cylinder(c + Vec3::X * 0.02 * x.signum(), Vec3::X, 0.22, 0.105, 0.02),
            rim,
            0.01,
        );
        // Spokes, so the spin shows.
        for k in 0..5 {
            let a = k as f32 / 5.0 * std::f32::consts::TAU;
            let dir = Vec3::new(0.0, a.cos(), a.sin());
            b.part(
                name,
                Shape::round_box(
                    c + Vec3::X * 0.1 * x.signum() + dir * 0.11,
                    Vec3::new(0.025, 0.1, 0.03),
                    0.015,
                )
                .rotated(Quat::from_rotation_x(a - std::f32::consts::FRAC_PI_2)),
                rubber,
                0.005,
            );
        }
    }
    b
}

pub fn mesh(paint: [f32; 3]) -> SkinnedMesh {
    // Rigid parts: weights must not blend between the body and the wheels.
    body(paint).build(&skeleton(), 0.03, 0.001)
}

pub fn rig() -> VehicleRig {
    VehicleRig {
        wheels: WHEELS
            .iter()
            .map(|(name, _, z)| WheelRig {
                bone: (*name).into(),
                radius: WHEEL_RADIUS,
                steers: *z < 0.0,
                ..Default::default()
            })
            .collect(),
        wheelbase: 2.6,
        travel: 0.13,
        frequency: 1.5,
        damping: 0.35,
        roll_deg_per_g: 5.0,
        pitch_deg_per_g: 3.0,
        mass_kg: 1250.0,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheels_are_rigid() {
        let s = skeleton();
        let mesh = mesh([0.6, 0.05, 0.03]);
        assert!(mesh.triangle_count() > 3000);
        let body = s.bone("body") as u16;
        for v in &mesh.vertices {
            assert!(
                v.weights[0] > 0.999,
                "blended car vertex {:?} {:?} at {}",
                v.weights,
                v.joints,
                v.position
            );
            if v.position.y < 0.2 {
                assert_ne!(v.joints[0], body, "low vertex on the body: {}", v.position);
            }
        }
    }
}
