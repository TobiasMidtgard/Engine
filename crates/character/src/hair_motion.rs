//! Moving hair: long and medium scalp hair is handed to the hair simulation and drawn where it
//! moved it.
//!
//! A person whose hair moves (its scalp [`StrandSet`] has guides) and who has a `SkeletonPose`
//! gets [`HairGuides`]: every guide in the animation module's model space at the rig's rest
//! pose, riding the two bones its root's skin vertex is weighted to most. The simulation (the
//! softbody module) writes back [`HairGuidesPosed`] every fixed step. Each drawn hair follows its
//! guide: the shaders add to every point how far the guide's matching point moved from where
//! the pose alone would carry it ([`guide_motion`]), so the hair keeps its own groom and only
//! the motion comes from the simulation. Without a simulation the hair rides the head as before.

use engine_core::prelude::*;
use engine_world::animation::{BonePose, SkeletonPose};
use engine_world::{HairGuide, HairGuides, HairGuidesPosed};

use crate::BuiltCharacter;
use crate::build::CharacterMeshes;

/// How firmly hair holds its grown shape for the simulation, by length: long hair swings,
/// shoulder-length hair a little less.
fn stiffness(points_per: u32) -> f32 {
    if points_per >= 24 { 0.2 } else { 0.3 }
}

/// A lock's thickness at the root for collisions, m (guides stand for locks about a centimeter
/// apart).
const LOCK_THICKNESS: f32 = 0.004;

/// The [`HairGuides`] of a built person for its `pose` skeleton (`None` when its hair doesn't
/// move, or the pose lacks the bones the hair grows from).
pub fn hair_guides(
    meshes: &CharacterMeshes,
    pose: &SkeletonPose,
    version: u32,
) -> Option<HairGuides> {
    let rig = meshes.rig.as_ref()?;
    let (si, set) = meshes
        .strands
        .iter()
        .enumerate()
        .find(|(_, s)| s.guide_count() > 0)?;
    let roots = meshes.guide_roots.get(si)?;
    let rest = rig.rest_pose();
    let mut palette = Vec::new();
    rig.palette(&rest, &rig.map(&rest), &mut palette);
    let map = rig.map(pose);
    let head = rig
        .bones
        .iter()
        .position(|b| b == "head")
        .and_then(|h| map[h].map(|p| (h, p)))?;
    // The rest pose in the person's skeleton's bone order.
    let grown_on: Vec<BonePose> = (0..pose.bones.len())
        .map(|i| {
            map.iter()
                .position(|m| *m == Some(i))
                .map_or(pose.bones[i], |r| rest.bones[r])
        })
        .collect();
    let per = set.points_per as usize;
    let mut guides = Vec::with_capacity(set.guide_count());
    for (g, &v) in roots.iter().enumerate().take(set.guide_count()) {
        let v = v as usize;
        let bones = meshes.skin.bones.get(v).copied().unwrap_or_default();
        let weights = meshes
            .skin
            .weights
            .get(v)
            .copied()
            .unwrap_or([255, 0, 0, 0]);
        // The two bones the root is weighted to most (rig and pose indices).
        let mut order = [0usize, 1, 2, 3];
        order.sort_by_key(|&k| std::cmp::Reverse(weights[k]));
        let bone = |k: usize| {
            let r = bones[order[k]] as usize;
            map.get(r).copied().flatten().map(|p| (r, p))
        };
        let a = bone(0).unwrap_or(head);
        let (b, wb) = match bone(1) {
            Some(b) if weights[order[1]] > 0 => (b, weights[order[1]] as f32),
            _ => (a, 0.0),
        };
        let blend = wb / (weights[order[0]] as f32 + wb).max(1.0);
        let place = |p: Vec3| {
            palette[a.0]
                .transform_point3(p)
                .lerp(palette[b.0].transform_point3(p), blend)
        };
        let points = set.guides[g * per..(g + 1) * per]
            .iter()
            .map(|p| place(*p))
            .collect();
        let n = meshes.skin.normals.get(v).copied().unwrap_or(Vec3::Y);
        guides.push(HairGuide {
            bones: [a.1 as u16, b.1 as u16],
            blend,
            points,
            normal: palette[a.0].transform_vector3(n).normalize_or(Vec3::Y),
            thickness: LOCK_THICKNESS,
        });
    }
    Some(HairGuides {
        version,
        guides,
        grown_on,
        stiffness: stiffness(set.points_per),
    })
}

/// Gives every built person with a pose and hair that moves its [`HairGuides`] (again after a
/// rebuild or a different skeleton), and takes them away when the hair no longer moves.
pub fn publish_hair_guides(
    mut commands: Commands,
    q: Query<(
        Entity,
        &BuiltCharacter,
        Option<&SkeletonPose>,
        Option<&HairGuides>,
    )>,
) {
    for (entity, built, pose, guides) in &q {
        let version = built.version as u32;
        let moves = built.meshes.strands.iter().any(|s| s.guide_count() > 0);
        let Some(pose) = pose.filter(|_| moves) else {
            if guides.is_some() && !moves {
                commands.entity(entity).remove::<HairGuides>();
            }
            continue;
        };
        if guides.is_some_and(|g| g.version == version && g.grown_on.len() == pose.bones.len()) {
            continue;
        }
        match hair_guides(&built.meshes, pose, version) {
            Some(g) => {
                commands.entity(entity).insert(g);
            }
            None if guides.is_some() => {
                commands.entity(entity).remove::<HairGuides>();
            }
            None => {}
        }
    }
}

/// How far every guide point moved from where `pose` alone would carry it (model space), all
/// guides concatenated, into `out` (`[x, y, z, 0]`); zeros where the simulation hasn't caught
/// up with these guides. Drawn hairs add their guide's.
pub fn guide_motion(
    guides: &HairGuides,
    posed: Option<&HairGuidesPosed>,
    pose: Option<&SkeletonPose>,
    out: &mut Vec<[f32; 4]>,
) {
    out.clear();
    let count = guides.point_count();
    let (Some(posed), Some(pose)) = (posed, pose) else {
        out.resize(count, [0.0; 4]);
        return;
    };
    if posed.version != guides.version
        || posed.points.len() != count
        || pose.bones.len() != guides.grown_on.len()
    {
        out.resize(count, [0.0; 4]);
        return;
    }
    let ride = |b: u16| {
        let (now, grown) = (pose.bones[b as usize], guides.grown_on[b as usize]);
        let m = Mat4::from_scale_rotation_translation(now.scale, now.rotation, now.translation);
        let g =
            Mat4::from_scale_rotation_translation(grown.scale, grown.rotation, grown.translation);
        m * g.inverse()
    };
    let mut at = 0;
    for g in &guides.guides {
        let (a, b) = (ride(g.bones[0]), ride(g.bones[1]));
        for p in &g.points {
            let carried = a.transform_point3(*p).lerp(b.transform_point3(*p), g.blend);
            let d = posed.points[at] - carried;
            out.push([d.x, d.y, d.z, 0.0]);
            at += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn pose(head: Quat) -> SkeletonPose {
        SkeletonPose {
            names: Arc::from(vec!["root".to_string(), "head".to_string()]),
            parents: Arc::from(vec![None, Some(0)]),
            bones: vec![
                BonePose::default(),
                BonePose {
                    translation: Vec3::new(0.0, 1.6, 0.0),
                    rotation: head,
                    scale: Vec3::ONE,
                },
            ],
        }
    }

    fn guides() -> HairGuides {
        HairGuides {
            version: 3,
            guides: vec![HairGuide {
                bones: [1, 1],
                blend: 0.0,
                points: vec![Vec3::new(0.0, 1.7, 0.05), Vec3::new(0.0, 1.6, 0.12)],
                normal: Vec3::Y,
                thickness: LOCK_THICKNESS,
            }],
            grown_on: pose(Quat::IDENTITY).bones,
            stiffness: 0.2,
        }
    }

    #[test]
    fn hair_moves_only_by_what_the_simulation_adds_to_the_pose() {
        let g = guides();
        let turned = pose(Quat::from_rotation_y(0.8));
        let carry = |p: Vec3| {
            Vec3::new(0.0, 1.6, 0.0) + Quat::from_rotation_y(0.8) * (p - Vec3::new(0.0, 1.6, 0.0))
        };
        // Hair the simulation left riding the turned head doesn't move off it.
        let mut posed = HairGuidesPosed {
            version: 3,
            points: g.guides[0].points.iter().map(|p| carry(*p)).collect(),
        };
        let mut out = Vec::new();
        guide_motion(&g, Some(&posed), Some(&turned), &mut out);
        assert_eq!(out.len(), 2);
        assert!(
            out.iter()
                .all(|d| Vec3::from_slice(&d[..3]).length() < 1e-5),
            "{out:?}"
        );
        // A tip the simulation swung 5 cm aside moves 5 cm aside.
        posed.points[1] += Vec3::X * 0.05;
        guide_motion(&g, Some(&posed), Some(&turned), &mut out);
        assert!((Vec3::from_slice(&out[1][..3]) - Vec3::X * 0.05).length() < 1e-5);
        // Points simulated for other guides move nothing.
        posed.version = 2;
        guide_motion(&g, Some(&posed), Some(&turned), &mut out);
        assert!(out.iter().all(|d| *d == [0.0; 4]));
    }
}
