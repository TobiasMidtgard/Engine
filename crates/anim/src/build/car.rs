//! A small procedural car: body and four wheels as one rigidly skinned mesh, animated by a
//! [`VehicleRig`] (suspension, body roll and pitch, steering, wheel spin). With the fetched
//! concept car ([`CONCEPT_FILE`]), the same rig drives a detailed glTF model instead
//! ([`concept_model`]): its body rolls on the springs, hubs steer, wheels and discs spin, the
//! calipers stay put and the doors swing on their hinges.

use engine_core::prelude::*;

use super::sdf::{Body, Paint, Shape};
use super::{PoseBuilder, bone, keyed};
use crate::clip::AnimationClip;
use crate::graph::{AnimGraph, NodeDef};
use crate::model::{PartLook, RigModel};
use crate::procedural::vehicle::{VehicleRig, WheelRig};
use crate::skeleton::Skeleton;
use crate::skin::SkinnedMesh;
use crate::xform::Xf;
use engine_assets::Handle;
use engine_render::{AlphaMode, GltfScene, StandardMaterial};

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

/// The concept car (Khronos glTF sample CarConcept, CC-BY 4.0), fetched with
/// `cargo run -p fetch-assets -- animation`.
pub const CONCEPT_FILE: &str = "fetched/anim/vehicles/car_concept.glb";
pub const CONCEPT_SKELETON: &str = "car_concept";
/// The parked concept car's graph: its doors open and shut ([`concept_door_clip`]).
pub const CONCEPT_PARKED_GRAPH: &str = "car_concept_parked";
const CONCEPT_WHEEL_RADIUS: f32 = 0.384;
/// Hubs in the rig's frame (front toward −Z, left toward −X): the file's wheel pivots.
const CONCEPT_WHEELS: [(&str, &str, &str, Vec3); 4] = [
    (
        "fl",
        "WheelFrontL",
        "WheelFrontLBrakePad",
        Vec3::new(-0.976, 0.384, -1.485),
    ),
    (
        "fr",
        "WheelFrontR",
        "WheelFrontRBrakePad",
        Vec3::new(0.977, 0.384, -1.484),
    ),
    (
        "rl",
        "WheelRearL",
        "WheelRearLBrakePad",
        Vec3::new(-0.982, 0.384, 1.314),
    ),
    (
        "rr",
        "WheelRearR",
        "WheelRearRBrakePad",
        Vec3::new(0.982, 0.384, 1.314),
    ),
];
const CONCEPT_BODY: Vec3 = Vec3::new(0.0, 0.55, 0.0);
/// Door hinges in the rig's frame (the file's door pivots), and the door nodes.
const CONCEPT_DOORS: [(&str, &str, Vec3); 2] = [
    (
        "door_l",
        "BodyDoorLColor1",
        Vec3::new(-1.089, 0.643, -0.998),
    ),
    ("door_r", "BodyDoorRColor1", Vec3::new(1.087, 0.643, -0.998)),
];

/// Root, sprung body, a steering hub per wheel with the spinning wheel under it, and the doors.
pub fn concept_skeleton() -> Skeleton {
    let i = Quat::IDENTITY;
    let mut bones = vec![
        bone("root", None, Vec3::ZERO, i),
        bone("body", Some("root"), CONCEPT_BODY, i),
    ];
    for (side, _, _, hub) in CONCEPT_WHEELS {
        let mount = format!("hub_{side}");
        bones.push(bone(&mount, Some("body"), hub - CONCEPT_BODY, i));
        bones.push(bone(&format!("wheel_{side}"), Some(&mount), Vec3::ZERO, i));
    }
    for (name, _, hinge) in CONCEPT_DOORS {
        bones.push(bone(name, Some("body"), hinge - CONCEPT_BODY, i));
    }
    Skeleton::new(CONCEPT_SKELETON, bones).expect("concept car skeleton")
}

/// The parked concept car's doors, every 8 s: the left door swings open (easing out against
/// its check strap with a little overshoot), then the right; both hold, then shut with a small
/// bounce on the latch. Doors are hinged at their front edge and open 62° outward.
pub fn concept_door_clip(s: &Skeleton) -> AnimationClip {
    const KEYS: [(f32, f32); 9] = [
        (0.0, 0.0),
        (0.12, 0.0),
        (0.27, 1.04),
        (0.31, 1.0),
        (0.62, 1.0),
        (0.74, 0.0),
        (0.76, 0.035),
        (0.78, 0.0),
        (1.0, 0.0),
    ];
    AnimationClip::from_fn("car_concept_doors", 8.0, 161, true, |ph| {
        let open = |delay: f32| 62.0 * keyed((ph - delay).rem_euclid(1.0), &KEYS);
        let mut p = PoseBuilder::new(s);
        // Rest rotations are identity, so +Y turns a door's rear edge toward +X.
        p.rot("door_l", 0.0, -open(0.0), 0.0)
            .rot("door_r", 0.0, open(0.06), 0.0);
        p.take()
    })
}

pub fn concept_parked_graph() -> AnimGraph {
    AnimGraph {
        name: CONCEPT_PARKED_GRAPH.into(),
        root: NodeDef::clip("car_concept_doors"),
    }
}

/// Suspension and steering for the concept car: a 1.5 t hatchback on soft springs.
pub fn concept_rig() -> VehicleRig {
    VehicleRig {
        wheels: CONCEPT_WHEELS
            .iter()
            .map(|(side, _, _, hub)| WheelRig {
                bone: format!("wheel_{side}"),
                mount: Some(format!("hub_{side}")),
                radius: CONCEPT_WHEEL_RADIUS,
                steers: hub.z < 0.0,
                ..Default::default()
            })
            .collect(),
        wheelbase: 2.8,
        travel: 0.12,
        frequency: 1.4,
        damping: 0.35,
        roll_deg_per_g: 4.5,
        pitch_deg_per_g: 2.5,
        mass_kg: 1500.0,
        ..Default::default()
    }
}

/// How the concept car's file maps onto [`concept_skeleton`]: the file faces +Z, so it turns
/// half a circle; wheels follow their wheel bones (modeled steered, so they stand upright),
/// calipers their hubs, doors their hinges. The Khronos logos on the plate and the steering
/// wheel are trademarks the model license doesn't cover, so the plate is plain and the emblem
/// hidden; the glass, which the file makes clear with transmission, is drawn as tinted glass.
pub fn concept_model(scene: Handle<GltfScene>) -> RigModel {
    let mut m = RigModel::new(scene, "body");
    m.placement = Xf::from_tr(Vec3::ZERO, Quat::from_rotation_y(std::f32::consts::PI));
    for (side, wheel, caliper, _) in CONCEPT_WHEELS {
        m.follow.push((wheel.into(), format!("wheel_{side}")));
        m.follow.push((caliper.into(), format!("hub_{side}")));
        m.upright.push(wheel.into());
    }
    for (name, node, _) in CONCEPT_DOORS {
        m.follow.push((node.into(), name.into()));
    }
    let plate = StandardMaterial {
        base_color: [0.78, 0.78, 0.75, 1.0],
        perceptual_roughness: 0.35,
        metallic: 0.2,
        ..Default::default()
    };
    m.looks = vec![
        ("License Plate".into(), PartLook::Material(plate)),
        ("InteriorSteeringEmblem".into(), PartLook::Hidden),
    ];
    // Material indices of CarConcept.glb (the file is pinned by SHA-256).
    m.materials = vec![
        // "Glass" (transmission in the file).
        (1, |m| {
            *m = StandardMaterial {
                base_color: [0.015, 0.02, 0.025, 0.42],
                perceptual_roughness: 0.04,
                metallic: 0.0,
                reflectance: 0.5,
                alpha_mode: AlphaMode::Blend,
                double_sided: true,
                ..Default::default()
            }
        }),
        // "Paint 1 Carmine" and "Paint 2 Carmine": metallic paint under a clear coat, which
        // the renderer doesn't have; a glossy dielectric reads as coated paint. Their flake
        // normal maps tile 30 times.
        (11, |m| {
            m.base_color = [0.48, 0.0, 0.004, 1.0];
            m.metallic = 0.0;
            m.perceptual_roughness = 0.14;
            m.reflectance = 0.65;
            m.uv_scale = [30.0, 30.0];
            m.normal_scale = 0.35;
        }),
        (10, |m| {
            m.base_color = [0.03, 0.03, 0.032, 1.0];
            m.metallic = 0.0;
            m.perceptual_roughness = 0.2;
            m.reflectance = 0.6;
            m.uv_scale = [30.0, 30.0];
            m.normal_scale = 0.35;
        }),
        // Normal maps the file tiles with texture transforms.
        (3, |m| m.uv_scale = [200.0, 400.0]),
        (4, |m| m.uv_scale = [2.0, 3.0]),
        (6, |m| m.uv_scale = [200.0, 400.0]),
        (7, |m| m.uv_scale = [100.0, 100.0]),
        (19, |m| m.uv_scale = [1.0, 4.0]),
    ];
    m
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
    use crate::clip::ClipBinding;

    #[test]
    fn concept_car_doors_swing_outward_and_shut() {
        let s = concept_skeleton();
        let clip = concept_door_clip(&s);
        let binding = ClipBinding::new(&clip, &s);
        // A point on each door's rear edge, in the door bone's frame.
        let rear = Vec3::new(0.0, 0.0, 1.0);
        let at = |t: f32, door: &str| {
            let mut pose = s.rest.clone();
            clip.sample(t, &binding, &mut pose);
            pose[s.bone(door)].r * rear
        };
        // Closed at the start, both open at 3.5 s (left toward −X, right toward +X), shut by 7.
        assert!(at(0.2, "door_l").x.abs() < 1e-3 && at(0.2, "door_r").x.abs() < 1e-3);
        let (l, r) = (at(3.5, "door_l"), at(3.5, "door_r"));
        assert!(l.x < -0.8 && r.x > 0.8, "{l} {r}");
        assert!(at(7.2, "door_l").x.abs() < 0.02 && at(7.6, "door_r").x.abs() < 0.02);
    }

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
