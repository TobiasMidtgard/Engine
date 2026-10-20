//! Hair another module grows and draws (the character module's people, through the
//! [`HairGuides`] contract), simulated here: the guides become a hair body on a child of the
//! person, colliding with the person's [`BoneColliders`] (unless it has its own: humanoid
//! capsules with the skull fitted to the hair's roots) and lying on the body where it was grown
//! lying on it, and every fixed step the posed points go back as [`HairGuidesPosed`] in the
//! person's model space. That module draws the hair, so these bodies aren't drawn here.

use std::collections::HashMap;

use engine_core::prelude::*;
use engine_world::animation::SkeletonPose;
use engine_world::{HairGuides, HairGuidesPosed};

use crate::body::{FrameSource, SoftBodySim, SoftKind};
use crate::character::{BoneCapsule, BoneColliders};
use crate::hair::{self, BoneBlend, Guide, HairGroom, HairStyle, Strands};
use crate::settings::SoftMaterial;
use crate::solver::{Sim, SkinCapsule};
use crate::systems::SoftBodyFailed;

/// On the child entity that simulates `person`'s [`HairGuides`] of `version`.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuideHair {
    pub person: Entity,
    pub version: u32,
}

/// On a person whose [`BoneColliders`] this module made (refitted when the guides change).
#[derive(Component, Debug, Clone, Copy)]
pub struct CollidersFittedToHair;

/// Humanoid capsules fitted to the hair: the skull sphere to the hair's roots, which sit on
/// the scalp (a head smaller than the default sphere would push the hair off it, a larger one
/// let it sink in), and every capsule slimmed until the hair as grown lies outside it (the
/// module that grew it kept it out of the body, so a capsule reaching into the hair is fatter
/// than the body there and would shove the hair away from it).
pub fn colliders_for(guides: &HairGuides, pose: &SkeletonPose) -> BoneColliders {
    let mut colliders = fit_skull(guides, pose);
    for c in &mut colliders.capsules {
        let Some(rest) = RestCapsule::new(c, guides, pose) else {
            continue;
        };
        let nearest = guides
            .guides
            .iter()
            .flat_map(|g| g.points.iter().skip(2))
            .map(|p| p.distance(rest.axis_point(*p)))
            .fold(f32::MAX, f32::min);
        c.radius = c.radius.min((nearest - HAIR_CLEARANCE).max(0.02));
    }
    colliders
}

/// A capsule where it was when the hair was grown (model space).
struct RestCapsule {
    bone: u16,
    start: Vec3,
    end: Vec3,
    radius: f32,
}

impl RestCapsule {
    fn new(c: &BoneCapsule, guides: &HairGuides, pose: &SkeletonPose) -> Option<Self> {
        let at = |name: &str| {
            pose.index_of(name)
                .and_then(|i| guides.grown_on.get(i).map(|b| (i, *b)))
        };
        let (bone, a) = at(&c.bone)?;
        let start = a.translation + a.rotation * (Vec3::from(c.offset) * a.scale);
        let end = match &c.to {
            Some(to) => at(to)?.1.translation,
            None => start,
        };
        Some(Self {
            bone: bone as u16,
            start,
            end,
            radius: c.radius,
        })
    }

    /// Where along the axis `p` lies (0 start .. 1 end, outside beyond the ends).
    fn along(&self, p: Vec3) -> f32 {
        let axis = self.end - self.start;
        (p - self.start).dot(axis) / axis.length_squared().max(1e-12)
    }

    /// The point on the axis nearest `p`.
    fn axis_point(&self, p: Vec3) -> Vec3 {
        self.start + (self.end - self.start) * self.along(p).clamp(0.0, 1.0)
    }
}

/// Hair as grown lying within this of the body (by the default humanoid capsules, which are
/// generous) rests on it, m.
const RESTS_ON_BODY: f32 = 0.02;

/// How deep under its floor a resting point may get before the floor lets it go (it came
/// round from elsewhere), m.
const FLOOR_REACH: f32 = 0.04;

/// How far hair as grown stays outside the fitted capsules, m (about a lock's collision
/// radius).
const HAIR_CLEARANCE: f32 = 0.006;

/// Humanoid capsules with the skull sphere fitted to the hair's roots.
fn fit_skull(guides: &HairGuides, pose: &SkeletonPose) -> BoneColliders {
    let mut colliders = BoneColliders::humanoid();
    let Some(head) = pose.index_of("head") else {
        return colliders;
    };
    let roots: Vec<Vec3> = guides
        .guides
        .iter()
        .filter(|g| g.bones[0] as usize == head && g.points.len() > 1)
        .map(|g| g.points[0])
        .collect();
    let Some(center) = fit_sphere(&roots) else {
        return colliders;
    };
    // Inside nearly every root (a few sit lower, at the nape or temples), by a little more
    // than the hair stands off the scalp.
    let mut d: Vec<f32> = roots.iter().map(|r| r.distance(center)).collect();
    d.sort_by(f32::total_cmp);
    let radius = d[d.len() / 10] - 0.004;
    let Some(bone) = guides.grown_on.get(head) else {
        return colliders;
    };
    if !(0.04..0.2).contains(&radius) {
        return colliders;
    }
    let offset = bone.rotation.inverse() * (center - bone.translation) / bone.scale;
    colliders
        .capsules
        .retain(|c| !(c.bone == "head" && c.to.is_none()));
    colliders
        .capsules
        .push(BoneCapsule::sphere("head", radius, offset.to_array()));
    colliders
}

/// The center of the sphere through `points` that fits them best (least squares), if they
/// span one (at least 8, not all in a plane).
fn fit_sphere(points: &[Vec3]) -> Option<Vec3> {
    if points.len() < 8 {
        return None;
    }
    // |p|² = 2c·p + k, linear in (c, k): normal equations in double precision.
    let mean = points.iter().copied().sum::<Vec3>() / points.len() as f32;
    let mut ata = [[0.0f64; 4]; 4];
    let mut atb = [0.0f64; 4];
    for p in points {
        let q = (*p - mean).as_dvec3();
        let row = [2.0 * q.x, 2.0 * q.y, 2.0 * q.z, 1.0];
        let b = q.length_squared();
        for i in 0..4 {
            for j in 0..4 {
                ata[i][j] += row[i] * row[j];
            }
            atb[i] += row[i] * b;
        }
    }
    let m = DMat4::from_cols_array_2d(&ata);
    if m.determinant().abs() < 1e-12 {
        return None;
    }
    let x = m.inverse() * DVec4::from_array(atb);
    Some(mean + x.truncate().as_vec3())
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
    // Where the body was when the hair was grown, generously (the skull fitted to the roots).
    let body: Vec<RestCapsule> = fit_skull(guides, pose)
        .capsules
        .iter()
        .filter_map(|c| RestCapsule::new(c, guides, pose))
        .collect();
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
        let mut frame_for = |sim: &mut Sim, a: u16, b: u16, t: f32| {
            let blend = BoneBlend {
                owner: person,
                a: a as usize,
                b: b as usize,
                t,
                rest_a: rest(a),
                rest_b: rest(b),
            };
            let (bp, br) = blend.world(person_t, pose)?;
            Some(
                *frame_of
                    .entry((a, b, (t * 32.0).round() as u8))
                    .or_insert_with(|| {
                        frames.push(FrameSource::Blend(blend));
                        sim.add_frame((bp - origin).as_vec3(), br)
                    }),
            )
        };
        let Some(frame) = frame_for(&mut sim, a, b, t) else {
            continue;
        };
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
        // Hair as grown lies on the body where it rests on it (on the shoulders, down the
        // back): there it may lift off but not sink in, as it would through capsules slimmed
        // to clear it. Each point keeps at least its grown distance from every body capsule
        // near it, so sliding round one onto the next doesn't take it in (a flat floor would
        // fling it back out from under the curve).
        for (k, p) in g.points.iter().enumerate().skip(2) {
            for c in &body {
                let from_axis = p.distance(c.axis_point(*p));
                if from_axis - c.radius > RESTS_ON_BODY {
                    continue;
                }
                let Some(on) = frame_for(&mut sim, c.bone, c.bone, 0.0) else {
                    continue;
                };
                sim.skin_capsules.push(SkinCapsule {
                    p: first + k as u32,
                    frame: on,
                    a: c.start,
                    b: c.end,
                    radius: (from_axis - 0.002).max(0.0),
                    reach: FLOOR_REACH,
                });
            }
        }
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
        Has<CollidersFittedToHair>,
    )>,
    sims: Query<(Entity, &GuideHair)>,
) {
    let mut built: HashMap<Entity, (Entity, u32)> = sims
        .iter()
        .map(|(e, g)| (g.person, (e, g.version)))
        .collect();
    for (person, guides, t, pose, has_colliders, fitted) in &people {
        match built.remove(&person) {
            Some((_, version)) if version == guides.version => continue,
            Some((stale, _)) => commands.entity(stale).despawn(),
            None => {}
        }
        if pose.bones.is_empty() {
            continue; // the pose isn't evaluated yet
        }
        if !has_colliders || fitted {
            commands
                .entity(person)
                .insert((colliders_for(guides, pose), CollidersFittedToHair));
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
