//! Capsule shadows and occlusion for people (characters week R1, "fighters stand on the sand").
//!
//! The shadow maps are too coarse to ground a person: at 2-5 cm texels, with the bias they need,
//! a foot's shadow starts a hand's width away from the foot, and nothing darkens the ground or
//! the skin under the soft light of the sky. So, as Unreal's capsule shadows (Daniel Wright,
//! "Dynamic Occlusion with Signed Distance Fields", SIGGRAPH 2015) and The Last of Us' ellipsoid
//! occlusion do, every person near the camera is stood in for by a handful of capsules along the
//! bones of its `SkeletonPose` (legs, feet, arms, hands, torso, neck, head), and the shading reads
//! them analytically: how much sky they hide from a point (a sphere's cosine-weighted solid
//! angle, Quilez) and whether they block the sun just above it (a soft cone toward the sun).
//!
//! The capsules travel in the point lights' buffer, after the lights (`frame.counts.x` of them):
//! one header per person (center and reach; first capsule and count), then the capsules
//! (end points and radius). `frame.distant_shadows.y` holds the number of people.

use engine_core::math::{DVec3, Frustum, Vec3};
use engine_core::prelude::*;
use engine_world::animation::SkeletonPose;

use crate::frame::GpuPointLight;

/// People with capsules, nearest the camera first.
pub(crate) const MAX_PEOPLE: usize = 24;
/// People further than this from the camera have none (m).
const MAX_DISTANCE_M: f64 = 60.0;
/// How far beyond a person's capsules their occlusion reaches (m), sky and sun alike: past
/// it the shadow maps and screen-space occlusion carry on (`capsule_occlusion`, lib_frame.wgsl).
pub(crate) const REACH_M: f32 = 1.0;

/// The capsules standing in for a person: from one bone to another (or, with an `extend` above
/// 0, onward from the second bone by that times the length from the first), radius in
/// meters at full size. Radii a little under the body's own (the occlusion of a round capsule
/// is fuller than a flattened limb's).
const SEGMENTS: [(&str, &str, f32, f32); 17] = [
    ("hips", "spine", 0.0, 0.13),
    ("spine", "chest", 0.0, 0.14),
    ("chest", "neck", 0.0, 0.13),
    ("neck", "head", 0.0, 0.055),
    // The head: from the head bone upward, as far again as the neck.
    ("neck", "head", 1.2, 0.095),
    ("upper_arm_l", "lower_arm_l", 0.0, 0.05),
    ("lower_arm_l", "hand_l", 0.0, 0.04),
    ("lower_arm_l", "hand_l", 0.35, 0.035),
    ("upper_arm_r", "lower_arm_r", 0.0, 0.05),
    ("lower_arm_r", "hand_r", 0.0, 0.04),
    ("lower_arm_r", "hand_r", 0.35, 0.035),
    ("upper_leg_l", "lower_leg_l", 0.0, 0.075),
    ("lower_leg_l", "foot_l", 0.0, 0.055),
    ("foot_l", "toe_l", 0.0, 0.045),
    ("upper_leg_r", "lower_leg_r", 0.0, 0.075),
    ("lower_leg_r", "foot_r", 0.0, 0.055),
    ("foot_r", "toe_r", 0.0, 0.045),
];

/// One person's capsules, camera-relative: (a, b, radius).
fn person_capsules(
    pose: &SkeletonPose,
    gt: &GlobalTransform,
    origin: DVec3,
    out: &mut Vec<(Vec3, Vec3, f32)>,
) -> bool {
    let start = out.len();
    let at = |name: &str| -> Option<Vec3> {
        let i = pose.index_of(name)?;
        pose.bone_world(gt, i).map(|(p, _)| (p - origin).as_vec3())
    };
    let scale = gt.scale().max_element();
    for (from, to, extend, radius) in SEGMENTS {
        let (Some(a), Some(b)) = (at(from), at(to)) else {
            continue;
        };
        let (a, b) = if extend > 0.0 {
            (b, b + (b - a) * extend)
        } else {
            (a, b)
        };
        out.push((a, b, radius * scale));
    }
    // A pose without legs and a head isn't a person (an animal, a prop rig): none.
    if out.len() - start < 8 {
        out.truncate(start);
        return false;
    }
    true
}

/// The people [`gather`] found, nearest the camera first.
#[derive(Default)]
pub(crate) struct People {
    /// How many have capsules.
    pub count: u32,
    /// The per-person headers, then every capsule, as the shaders read them.
    pub entries: Vec<GpuPointLight>,
    /// Each one's body: a camera-relative sphere around their capsules (without the reach).
    pub bodies: Vec<(Vec3, f32)>,
}

/// The people nearest the camera in `frustum` (grown by their reach and a sun shadow's length):
/// their capsules as the shaders read them, entries numbered from `first` (the number of point
/// lights before them), and their bodies.
pub(crate) fn gather(world: &mut World, origin: DVec3, frustum: &Frustum, first: u32) -> People {
    let mut people: Vec<(f32, Vec3, f32, std::ops::Range<usize>)> = Vec::new();
    let mut capsules: Vec<(Vec3, Vec3, f32)> = Vec::new();
    let mut q = world.query::<(&SkeletonPose, &GlobalTransform)>();
    for (pose, gt) in q.iter(world) {
        let d = gt.translation() - origin;
        if d.length_squared() > MAX_DISTANCE_M * MAX_DISTANCE_M || pose.index_of("head").is_none() {
            continue;
        }
        let start = capsules.len();
        if !person_capsules(pose, gt, origin, &mut capsules) {
            continue;
        }
        let span = &capsules[start..];
        // A sphere around every capsule.
        let lo = span
            .iter()
            .fold(Vec3::splat(f32::MAX), |m, c| m.min(c.0.min(c.1) - c.2));
        let hi = span
            .iter()
            .fold(Vec3::splat(f32::MIN), |m, c| m.max(c.0.max(c.1) + c.2));
        let center = (lo + hi) * 0.5;
        let body = (hi - lo).length() * 0.5;
        let radius = body + REACH_M;
        if !frustum.intersects_sphere(center, radius + 2.0) {
            capsules.truncate(start);
            continue;
        }
        people.push((center.length(), center, body, start..capsules.len()));
    }
    people.sort_by(|a, b| a.0.total_cmp(&b.0));
    people.truncate(MAX_PEOPLE);
    let mut out =
        Vec::with_capacity(people.len() + people.iter().map(|p| p.3.len()).sum::<usize>());
    let mut next = first + people.len() as u32;
    for (_, center, body, span) in &people {
        out.push(GpuPointLight {
            position_range: [center.x, center.y, center.z, body + REACH_M],
            color_radius: [next as f32, span.len() as f32, 0.0, 0.0],
        });
        next += span.len() as u32;
    }
    for (_, _, _, span) in &people {
        for (a, b, r) in &capsules[span.clone()] {
            out.push(GpuPointLight {
                position_range: [a.x, a.y, a.z, *r],
                color_radius: [b.x, b.y, b.z, 0.0],
            });
        }
    }
    People {
        count: people.len() as u32,
        entries: out,
        bodies: people.iter().map(|p| (p.1, p.2)).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_world::animation::{BonePose, HUMANOID_BONES};
    use std::sync::Arc;

    /// A person standing at the origin: every humanoid bone at a plausible height.
    fn standing() -> SkeletonPose {
        let height = |name: &str| -> Vec3 {
            let side = if name.ends_with("_l") {
                -0.2
            } else if name.ends_with("_r") {
                0.2
            } else {
                0.0
            };
            let y = match name.trim_end_matches("_l").trim_end_matches("_r") {
                "root" => 0.0,
                "hips" => 1.0,
                "spine" => 1.15,
                "chest" => 1.3,
                "neck" => 1.5,
                "head" => 1.6,
                "upper_arm" | "clavicle" => 1.45,
                "lower_arm" => 1.2,
                "hand" => 0.95,
                "upper_leg" => 0.95,
                "lower_leg" => 0.5,
                "foot" => 0.08,
                "toe" => 0.03,
                _ => 1.0,
            };
            let z = if name.starts_with("toe") { -0.12 } else { 0.0 };
            Vec3::new(side, y, z)
        };
        SkeletonPose {
            names: HUMANOID_BONES
                .iter()
                .map(|(n, _)| n.to_string())
                .collect::<Vec<_>>()
                .into(),
            parents: Arc::from(vec![None; HUMANOID_BONES.len()]),
            bones: HUMANOID_BONES
                .iter()
                .map(|(n, _)| BonePose {
                    translation: height(n),
                    ..Default::default()
                })
                .collect(),
        }
    }

    /// A camera at `CAMERA` looking down -Z (camera-relative, as `gather` reads it).
    fn everything() -> Frustum {
        Frustum::from_view_proj(&crate::frame::perspective_infinite_reverse(1.2, 1.0, 0.1))
    }

    const CAMERA: DVec3 = DVec3::new(0.0, 1.5, 5.0);

    #[test]
    fn a_person_gets_capsules_and_a_header() {
        let mut world = World::new();
        world.spawn((standing(), GlobalTransform::default()));
        // Not a person: no head.
        world.spawn((
            SkeletonPose {
                names: Arc::from(vec!["root".to_string()]),
                parents: Arc::from(vec![None]),
                bones: vec![BonePose::default()],
            },
            GlobalTransform::default(),
        ));
        let found = gather(&mut world, CAMERA, &everything(), 3);
        let (people, entries) = (found.count, found.entries);
        assert_eq!(people, 1);
        assert_eq!(found.bodies.len(), 1);
        assert!((found.bodies[0].1 + REACH_M - entries[0].position_range[3]).abs() < 1e-5);
        assert_eq!(entries.len(), 1 + SEGMENTS.len());
        let header = entries[0];
        assert_eq!(
            header.color_radius[0], 4.0,
            "capsules follow the 3 lights and the header"
        );
        assert_eq!(header.color_radius[1], SEGMENTS.len() as f32);
        // Camera-relative: the person is 5 m in front of the camera, its center about 0.8 m up.
        assert!((header.position_range[2] + 5.0).abs() < 0.3, "{header:?}");
        // Every capsule lies inside the person's reach.
        let c = Vec3::from_slice(&header.position_range[..3]);
        for e in &entries[1..] {
            let a = Vec3::from_slice(&e.position_range[..3]);
            let b = Vec3::from_slice(&e.color_radius[..3]);
            assert!((a - c).length() + e.position_range[3] < header.position_range[3]);
            assert!((b - c).length() + e.position_range[3] < header.position_range[3]);
        }
    }

    #[test]
    fn far_people_have_none() {
        let mut world = World::new();
        world.spawn((
            standing(),
            GlobalTransform::from_translation_rotation(
                DVec3::new(0.0, 0.0, -200.0),
                Quat::IDENTITY,
            ),
        ));
        let found = gather(&mut world, CAMERA, &everything(), 0);
        assert_eq!((found.count, found.entries.len()), (0, 0));
    }
}
