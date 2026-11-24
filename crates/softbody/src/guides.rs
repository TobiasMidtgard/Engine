//! Hair another module grows and draws (the character module's people, through the
//! [`HairGuides`] contract), simulated here: the guides become a hair body on a child of the
//! person, colliding with the person's [`BoneColliders`] (unless it has its own: humanoid
//! capsules with the skull fitted to the hair's roots) and lying on the body where it was grown
//! lying on it, and every fixed step the posed points go back as [`HairGuidesPosed`] in the
//! person's model space. That module draws the hair, so these bodies aren't drawn here.
//!
//! The grown shape is the hair at rest. Left to itself the simulation rests a little off it
//! (its soft hold on the grown shape gives under gravity, its spheres and capsules stand in
//! for the head and body the hair was combed over), so each body learns where it rests with
//! nothing moving ([`GroomedRest`], solved on a worker) and hands on the grown shape moved by
//! how far the simulation is from there: still hair is exactly as grown, and every swing,
//! drape and push of the simulation shows on top of it.

use std::collections::HashMap;

use engine_core::off_frame::OffFrameCommands;
use engine_core::prelude::*;
use engine_world::animation::SkeletonPose;
use engine_world::{HairGuides, HairGuidesPosed};

use crate::body::{FrameSource, SoftBodySim, SoftKind};
use crate::character::{BoneCapsule, BoneColliders};
use crate::collide::LocalCollider;
use crate::hair::{self, BoneBlend, Guide, HairGroom, HairStyle, Strands};
use crate::settings::SoftMaterial;
use crate::solver::{Sim, SkinCapsule};
use crate::systems::{SETTLE_DAMPING, SoftBodyFailed};

/// On the child entity that simulates `person`'s [`HairGuides`] of `version`.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuideHair {
    pub person: Entity,
    pub version: u32,
}

/// On the hair body: the grown shape it is handed on in at rest.
///
/// Drawn straight from the simulation, combed hair came out ruffled: guides that were grown
/// side by side in ordered layers rested millimetres apart from where they were grown, each
/// by its own amount (pushed out by the skull's sphere, sagging on its goal springs), so the
/// hairs drawn between them crossed, and locks framing the face hung out from the temple.
/// With the rest shape known, [`write_posed_guides`] hands on `grown + (simulated - rest)`.
#[derive(Component, Debug, Clone, Default)]
pub struct GroomedRest {
    /// Every particle as grown, in its guide's frame.
    grown: Vec<Vec3>,
    /// Where the simulation leaves it with nothing moving, in the same frame. Empty until it
    /// is solved; the hair is handed on as grown, riding its bones, until then.
    rest: Vec<Vec3>,
    /// 0..1: how much of the simulation's motion is handed on. It comes in over
    /// [`SHOWN_OVER`] steps once the rest shape is known and the body has settled.
    shown: f32,
}

impl GroomedRest {
    /// Whether the simulation's motion is handed on in full.
    pub fn shown(&self) -> bool {
        self.shown >= 1.0
    }
}

/// Fixed steps over which a body's motion comes in once its rest shape is known.
const SHOWN_OVER: f32 = 24.0;
/// Most steps the rest shape is solved for, and the speed (m/s) under which it is at rest.
const REST_STEPS: u32 = 600;
const REST_SPEED: f32 = 0.0005;
/// Settling steps a body takes in its own pose once it is put in its rest shape.
const SETTLE_IN_POSE: u32 = 12;

/// Every particle of `guides` in its guide's frame.
fn in_guide_frames(sim: &Sim, guides: &[Guide]) -> Vec<Vec3> {
    let mut out = sim.pos.clone();
    for g in guides {
        let Some(f) = sim.frames.get(g.frame as usize) else {
            continue;
        };
        let back = f.to_rot.conjugate();
        let run = g.first as usize..(g.first + g.count) as usize;
        for (local, at) in out[run.clone()].iter_mut().zip(&sim.pos[run]) {
            *local = back * (*at - f.to_pos);
        }
    }
    out
}

/// Where the simulation leaves `guides` with nothing moving: the person standing as the hair
/// was grown (`grown`: its skeleton at [`HairGuides::grown_on`]), in still air, among
/// `colliders`. Every particle in its guide's frame. Seconds of simulation: run on a worker.
fn rest_shape(
    guides: &HairGuides,
    person: Entity,
    grown: &SkeletonPose,
    colliders: &BoneColliders,
) -> Option<Vec<Vec3>> {
    let stand = GlobalTransform::IDENTITY;
    let mut body = build(guides, person, &stand, grown).ok()?;
    let mut shapes = Vec::new();
    colliders.resolve(&stand, grown, &mut shapes);
    let origin = body.sim.origin;
    let local: Vec<LocalCollider> = shapes
        .into_iter()
        .enumerate()
        .map(|(k, (shape, pos, rot))| {
            let pos = (pos - origin).as_vec3();
            LocalCollider {
                shape,
                from_pos: pos,
                from_rot: rot,
                to_pos: pos,
                to_rot: rot,
                friction: colliders.friction,
                id: k as u32,
                dynamic: false,
            }
        })
        .collect();
    let sim = &mut body.sim;
    // Damped as a body settling in view is: it sags into place instead of swinging there.
    sim.params.damping = sim.params.damping.max(SETTLE_DAMPING);
    for step in 0..REST_STEPS {
        sim.step(1.0 / 60.0, &local);
        let fastest = sim
            .vel
            .iter()
            .fold(0.0f32, |m, v| m.max(v.length_squared()));
        if step >= 30 && fastest < REST_SPEED * REST_SPEED {
            break;
        }
    }
    let strands = body.strands.as_ref()?;
    Some(in_guide_frames(&body.sim, &strands.guides))
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
    let anchor = out.first().map_or(0, |g| g.frame);
    body.strands = Some(Strands {
        guides: out,
        style,
        color: style.color(),
        anchor,
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
        Option<&BoneColliders>,
        Has<CollidersFittedToHair>,
    )>,
    sims: Query<(Entity, &GuideHair)>,
) {
    let mut built: HashMap<Entity, (Entity, u32)> = sims
        .iter()
        .map(|(e, g)| (g.person, (e, g.version)))
        .collect();
    for (person, guides, t, pose, own_colliders, fitted) in &people {
        match built.remove(&person) {
            Some((_, version)) if version == guides.version => continue,
            Some((stale, _)) => commands.entity(stale).despawn(),
            None => {}
        }
        if pose.bones.is_empty() {
            continue; // the pose isn't evaluated yet
        }
        let colliders = match own_colliders {
            Some(own) if !fitted => own.clone(),
            _ => {
                let made = colliders_for(guides, pose);
                commands
                    .entity(person)
                    .insert((made.clone(), CollidersFittedToHair));
                made
            }
        };
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
                let grown = body
                    .strands
                    .as_ref()
                    .map(|s| in_guide_frames(&body.sim, &s.guides))
                    .unwrap_or_default();
                child.insert((
                    body,
                    GroomedRest {
                        grown,
                        ..GroomedRest::default()
                    },
                ));
                let hair = child.id();
                let version = guides.version;
                let guides = guides.clone();
                let stand = SkeletonPose {
                    names: pose.names.clone(),
                    parents: pose.parents.clone(),
                    bones: guides.grown_on.clone(),
                };
                commands.off_frame(
                    "softbody hair at rest",
                    move || rest_shape(&guides, person, &stand, &colliders),
                    move |world: &mut World, rest: Option<Vec<Vec3>>| {
                        rests_at(world, hair, version, rest);
                    },
                );
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

/// The hair body `hair` (built from guides of `version`) rests at `rest`: it is put there
/// now, in the pose its person is in, to settle the little that pose differs by, and its
/// motion is handed on from then.
fn rests_at(world: &mut World, hair: Entity, version: u32, rest: Option<Vec<Vec3>>) {
    let Some(rest) = rest else {
        return;
    };
    // The body may have gone or been rebuilt meanwhile.
    if world.get::<GuideHair>(hair).map(|g| g.version) != Some(version) {
        return;
    }
    let Some(mut body) = world.get_mut::<SoftBodySim>(hair) else {
        return;
    };
    if rest.len() != body.sim.len() {
        return;
    }
    let body = &mut *body;
    if let Some(strands) = &body.strands {
        for g in &strands.guides {
            let Some(f) = body.sim.frames.get(g.frame as usize).copied() else {
                continue;
            };
            let run = g.first as usize..(g.first + g.count) as usize;
            for (at, local) in body.sim.pos[run.clone()].iter_mut().zip(&rest[run]) {
                *at = f.to_pos + f.to_rot * *local;
            }
        }
        body.sim.rest_here();
        body.pending_prewarm = SETTLE_IN_POSE;
        body.calm = 0;
    }
    if let Some(mut groomed) = world.get_mut::<GroomedRest>(hair)
        && groomed.grown.len() == rest.len()
    {
        groomed.rest = rest;
        groomed.shown = 0.0;
    }
}

/// Hand the simulated points back to the people, in their model space: the grown shape,
/// moved by how far the simulation is from where it rests ([`GroomedRest`]).
pub fn write_posed_guides(
    mut commands: Commands,
    mut sims: Query<(&GuideHair, &SoftBodySim, Option<&mut GroomedRest>)>,
    mut people: Query<(&GlobalTransform, Option<&mut HairGuidesPosed>)>,
) {
    for (hair, body, groomed) in &mut sims {
        let Ok((t, posed)) = people.get_mut(hair.person) else {
            continue;
        };
        let Some(strands) = &body.strands else {
            continue;
        };
        let mut groomed = groomed.filter(|g| g.grown.len() == body.sim.len());
        if let Some(g) = &mut groomed
            && g.shown < 1.0
            && g.rest.len() == g.grown.len()
            && body.pending_prewarm == 0
        {
            g.shown = (g.shown + 1.0 / SHOWN_OVER).min(1.0);
        }
        let groomed = groomed.as_deref();
        let to_model = t.rotation().conjugate();
        let scale = t.scale().max(Vec3::splat(1e-6));
        let sim = &body.sim;
        let points = strands.guides.iter().flat_map(|g| {
            let frame = sim.frames.get(g.frame as usize).copied();
            (g.first as usize..(g.first + g.count) as usize).map(move |i| {
                let at = match (groomed, frame) {
                    (Some(groomed), Some(f)) => {
                        // grown + shown * (simulated - rest), about the guide's frame.
                        let w = groomed.shown;
                        let rest = groomed.rest.get(i).copied().unwrap_or(Vec3::ZERO);
                        f.to_pos * (1.0 - w)
                            + sim.pos[i] * w
                            + f.to_rot * (groomed.grown[i] - rest * w)
                    }
                    _ => sim.pos[i],
                };
                let world = sim.origin + at.as_dvec3();
                to_model * (world - t.translation()).as_vec3() / scale
            })
        });
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
                    capsules: Vec::new(),
                });
            }
        }
    }
}
