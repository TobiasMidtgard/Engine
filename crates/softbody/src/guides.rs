//! Hair another module grows and draws (the character module's people, through the
//! [`HairGuides`] contract), simulated here: the guides become a hair body on a child of the
//! person, colliding with the person's [`BoneColliders`] (humanoid capsules unless it has its
//! own), and every fixed step the posed points go back as [`HairGuidesPosed`] in the person's
//! model space. That module draws the hair, so these bodies aren't drawn here.

use std::collections::HashMap;

use engine_core::prelude::*;
use engine_world::animation::SkeletonPose;
use engine_world::{HairGuides, HairGuidesPosed};

use crate::body::{FrameSource, SoftBodySim, SoftKind};
use crate::character::BoneColliders;
use crate::hair::{self, BoneBlend, Guide, HairGroom, HairStyle, Strands};
use crate::settings::SoftMaterial;
use crate::solver::Sim;
use crate::systems::SoftBodyFailed;

/// On the child entity that simulates `person`'s [`HairGuides`] of `version`.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuideHair {
    pub person: Entity,
    pub version: u32,
}

/// The hair body for `guides` on `person` (at `person_t`, posed as `pose`).
pub fn build(
    guides: &HairGuides,
    person: Entity,
    person_t: &GlobalTransform,
    pose: &SkeletonPose,
) -> Result<SoftBodySim, String> {
    let bones = pose.bones.len();
    if guides.grown_on.len() != bones {
        return Err(format!(
            "HairGuides were grown on {} bones, but the SkeletonPose has {bones}.",
            guides.grown_on.len()
        ));
    }
    let origin = person_t.translation();
    let mut sim = Sim::new(origin);
    let mut frames = Vec::new();
    let mut frame_of: HashMap<(u16, u16, u8), u16> = HashMap::new();
    let mut uses: HashMap<u16, usize> = HashMap::new();
    let mut out = Vec::new();
    let omegas = hair::strand_omegas(guides.stiffness);
    let mut thickness = 0.0;
    for g in &guides.guides {
        if g.points.len() < 2 {
            continue;
        }
        let [a, b] = g.bones;
        if a as usize >= bones || b as usize >= bones {
            return Err(format!("A hair guide rides bone {a} or {b} of {bones}."));
        }
        // Every guide rides a skinning blend of its bones from the pose it was grown on, so
        // its model-space rest points are its points in that frame.
        let t = g.blend.clamp(0.0, 1.0);
        let (b, t) = if t < 1.0 / 32.0 { (a, 0.0) } else { (b, t) };
        let rest = |i: u16| {
            let p = guides.grown_on[i as usize];
            (p.translation, p.rotation)
        };
        let blend = BoneBlend {
            owner: person,
            a: a as usize,
            b: b as usize,
            t,
            rest_a: rest(a),
            rest_b: rest(b),
        };
        let Some((bp, br)) = blend.world(person_t, pose) else {
            continue;
        };
        let key = (a, b, (t * 32.0).round() as u8);
        let frame = *frame_of.entry(key).or_insert_with(|| {
            frames.push(FrameSource::Blend(blend));
            sim.add_frame((bp - origin).as_vec3(), br)
        });
        *uses.entry(frame).or_default() += 1;
        let first = hair::push_strand(
            &mut sim,
            frame,
            ((bp - origin).as_vec3(), br),
            &g.points,
            omegas,
            |arc| hair::swept_hold(g.normal, arc),
            None,
        );
        let tangent = (g.points[1] - g.points[0])
            .reject_from(g.normal)
            .normalize_or(g.normal.any_orthonormal_vector());
        out.push(Guide {
            first,
            count: g.points.len() as u32,
            frame,
            normal: g.normal.normalize_or(Vec3::Y),
            tangent,
        });
        thickness += g.thickness.max(0.0);
    }
    if out.is_empty() {
        return Err("HairGuides has no guide with two or more points.".into());
    }
    let settings = HairGroom::hair_settings();
    let thickness = (thickness / out.len() as f32 * 0.5 + 0.003).max(settings.thickness);
    sim.params.radius = thickness;
    sim.params.drag = 2.5;
    sim.params.turbulence = 0.6;
    // The hair partly follows the frame most of it rides (the head).
    if settings.follow > 0.0
        && let Some((&head, _)) = uses
            .iter()
            .max_by_key(|(f, n)| (**n, std::cmp::Reverse(**f)))
    {
        sim.params.follow = Some((head, settings.follow));
    }
    let style = HairStyle {
        stiffness: guides.stiffness,
        ..HairStyle::long()
    };
    let mut body = SoftBodySim::new(
        SoftKind::Hair,
        sim,
        crate::SimSettings {
            thickness,
            ..settings
        },
        SoftMaterial::default(),
    );
    body.frames = frames;
    body.time_owner = Some(person);
    body.drawn_elsewhere = true;
    body.strands = Some(Strands {
        guides: out,
        style,
        color: style.color(),
    });
    Ok(body)
}

/// Simulate the hair of people with [`HairGuides`]: build it once their pose is evaluated,
/// rebuild it when the guides' version changes, drop it when they lose the guides.
#[allow(clippy::type_complexity)]
pub fn build_guide_hair(
    mut commands: Commands,
    people: Query<(
        Entity,
        &HairGuides,
        &GlobalTransform,
        &SkeletonPose,
        Has<BoneColliders>,
    )>,
    sims: Query<(Entity, &GuideHair)>,
) {
    let mut built: HashMap<Entity, (Entity, u32)> = sims
        .iter()
        .map(|(e, g)| (g.person, (e, g.version)))
        .collect();
    for (person, guides, t, pose, has_colliders) in &people {
        match built.remove(&person) {
            Some((_, version)) if version == guides.version => continue,
            Some((stale, _)) => commands.entity(stale).despawn(),
            None => {}
        }
        if pose.bones.is_empty() {
            continue; // the pose isn't evaluated yet
        }
        if !has_colliders {
            commands.entity(person).insert(BoneColliders::humanoid());
        }
        let mut child = commands.spawn((
            Name::new("Hair simulation"),
            Transform::default(),
            GuideHair {
                person,
                version: guides.version,
            },
            ChildOf(person),
        ));
        match build(guides, person, t, pose) {
            Ok(body) => {
                child.insert(body);
            }
            Err(reason) => {
                engine_core::log::warn!(
                    "HairGuides on entity {person} can't be simulated: {reason}"
                );
                // Kept (unsimulated) until the guides change, so it isn't retried every frame.
                child.insert(SoftBodyFailed(reason));
            }
        }
    }
    for (stale, _) in built.into_values() {
        commands.entity(stale).despawn();
    }
}

/// Hand the simulated points back to the people, in their model space.
pub fn write_posed_guides(
    mut commands: Commands,
    sims: Query<(&GuideHair, &SoftBodySim)>,
    mut people: Query<(&GlobalTransform, Option<&mut HairGuidesPosed>)>,
) {
    for (hair, body) in &sims {
        let Ok((t, posed)) = people.get_mut(hair.person) else {
            continue;
        };
        let Some(strands) = &body.strands else {
            continue;
        };
        let to_model = t.rotation().conjugate();
        let scale = t.scale().max(Vec3::splat(1e-6));
        let model = |i: u32| {
            to_model * (body.world_position(i as usize) - t.translation()).as_vec3() / scale
        };
        let points = strands
            .guides
            .iter()
            .flat_map(|g| (g.first..g.first + g.count).map(model));
        match posed {
            Some(mut posed) => {
                posed.version = hair.version;
                posed.points.clear();
                posed.points.extend(points);
            }
            None => {
                commands.entity(hair.person).insert(HairGuidesPosed {
                    version: hair.version,
                    points: points.collect(),
                });
            }
        }
    }
}
