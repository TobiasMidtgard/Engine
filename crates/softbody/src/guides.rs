//! Hair another module grows and draws (the character module's people, through the
//! [`HairGuides`] contract), simulated here: the guides become strands of the strand solver
//! ([`crate::tress`]: the owner's `gta-hair-sim` simulation) on a child of the person, sliding
//! off the person's head and body ([`crate::hair_body`]), and every fixed step the posed points
//! go back as [`HairGuidesPosed`] in the person's model space. That module draws the hair, so
//! these bodies aren't drawn here.
//!
//! A guide is simulated as points about two centimeters apart ([`SEGMENT`]), whatever it was
//! grown with, and handed back at its own points: each rides the simulated strand where it lay
//! along it.
//!
//! The grown shape is the hair at rest. Left to itself the simulation rests off it (a
//! centimeter or two by the tips of long hair: its hold on the grown shape gives under
//! gravity, and the hair's own pressure spreads it), so each body learns where it rests with
//! nothing moving ([`GroomedRest`], solved on a worker) and hands on the grown shape moved by
//! how far the simulation is from there: still hair is exactly as grown, and every swing,
//! drape and push of the simulation shows on top of it. (But a lock that rests on the body far
//! from where it was grown, one grown through a shoulder, is handed on as simulated:
//! [`GROWN_OFF`].) The shapes the hair is held out of go along
//! ([`HairGuidesPosed::capsules`]), for the hairs drawn between the guides.

use std::collections::HashMap;

use engine_core::off_frame::OffFrameCommands;
use engine_core::prelude::*;
use engine_world::animation::SkeletonPose;
use engine_world::character::Appearance;
use engine_world::{HairCapsule, HairGuides, HairGuidesPosed};

use crate::body::{FrameSource, SoftBodySim, SoftKind};
use crate::character::{BoneCapsule, BoneColliders};
use crate::hair::{self, BoneBlend, Guide, HairGroom, HairStyle, Strands};
use crate::hair_body;
use crate::settings::SoftMaterial;
use crate::solver::Sim;
use crate::systems::{SETTLE_DAMPING, SoftBodyFailed};
use crate::tress::{MAX_VERTS, Tress, TressCapsule, TressStrand, TressVertex};

/// On the child entity that simulates `person`'s [`HairGuides`] of `version`.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuideHair {
    pub person: Entity,
    pub version: u32,
}

/// On the hair body: the grown shape it is handed on in at rest, and where each grown point
/// rides the simulated strand.
///
/// Drawn straight from the simulation, combed hair came out ruffled: guides that were grown
/// side by side in ordered layers rested millimetres apart from where they were grown, each
/// by its own amount, so the hairs drawn between them crossed, and locks framing the face hung
/// out from the temple. With the rest shape known, [`write_posed_guides`] hands on the grown
/// shape moved as the simulation is moved from its rest.
#[derive(Component, Debug, Clone, Default)]
pub struct GroomedRest {
    /// Every guide point as grown, in its guide's frame, guides in order.
    grown: Vec<Vec3>,
    /// Per grown point: the particle before it along its strand, and how far on to the next
    /// (0..1).
    at: Vec<(u32, f32)>,
    /// Grown points per simulated guide.
    counts: Vec<u32>,
    /// Every particle where the simulation leaves it with nothing moving, in its guide's
    /// frame; the strand as grown until that is solved.
    rest: Vec<Vec3>,
    /// Per guide, 0..1: how much of what it was grown off its strand's rest goes along
    /// ([`GROWN_OFF`]); empty until worked out for this `rest`.
    kept: Vec<f32>,
    /// `rest` is solved; the hair is handed on as grown, riding its bones, until then.
    solved: bool,
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

/// A guide that rests on the body (within [`ON_BODY`] of the neck, the shoulders or the
/// trunk) further than the second of these from where it was grown, m, is handed on as
/// simulated, one nearer than the first as grown. A lock grown through a shoulder or the
/// back rests a hand's breadth from there, on the body, and that much carried along with it
/// stood stiffly out in the air beside it whenever the head turned. (Hair that holds a shape
/// off the body, grown up or out, is handed on as grown however far it sags.)
const GROWN_OFF: (f32, f32) = (0.03, 0.08);
const ON_BODY: f32 = 0.015;
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
/// was grown (`grown`: its skeleton at [`HairGuides::grown_on`]), in still air. Every particle
/// in its guide's frame. Seconds of simulation: run on a worker.
fn rest_shape(
    guides: &HairGuides,
    person: Entity,
    grown: &SkeletonPose,
    male: f32,
) -> Option<Vec<Vec3>> {
    let stand = GlobalTransform::IDENTITY;
    let (mut body, _) = build(guides, person, &stand, grown, male).ok()?;
    let sim = &mut body.sim;
    // Damped as a body settling in view is: it sags into place instead of swinging there.
    sim.params.damping = sim.params.damping.max(SETTLE_DAMPING);
    for step in 0..REST_STEPS {
        sim.step(1.0 / 60.0, &[]);
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

/// A capsule's axis where it was when the hair was grown (model space).
struct RestCapsule {
    start: Vec3,
    end: Vec3,
}

impl RestCapsule {
    fn new(c: &BoneCapsule, guides: &HairGuides, pose: &SkeletonPose) -> Option<Self> {
        let at = |name: &str| {
            pose.index_of(name)
                .and_then(|i| guides.grown_on.get(i).copied())
        };
        let a = at(&c.bone)?;
        let start = a.translation + a.rotation * (Vec3::from(c.offset) * a.scale);
        let end = match &c.to {
            Some(to) => at(to)?.translation,
            None => start,
        };
        Some(Self { start, end })
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
    let Some(center) = hair_body::fit_sphere(&roots) else {
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

/// Length of a simulated strand's segments, m (the source project's guides are 20 points over
/// 0.25 to 0.5 m), and the fewest points of a strand.
pub const SEGMENT: f32 = 0.02;
const MIN_VERTS: usize = 4;
/// [`HairGuides::stiffness`] of long hair: the source project's stiffnesses are for that.
const LONG_HAIR: f32 = 0.2;
/// The source project's long, free-hanging strands, root to tip: how firmly every segment
/// holds its grown direction and every bend its shape, and how firmly the head carries the
/// first points. (The free cascade of its Lucia hairstyle holds at 0.76 to 0.46 and 0.78 to
/// 0.52: with those a lock thrown off by a jolt of the head stayed caught in front of the
/// face, and long locks let go from level whipped through faster than they fall.)
const SHAPE: (f32, f32) = (0.50, 0.14);
const BEND: (f32, f32) = (0.46, 0.16);
const PINS: [f32; 3] = [1.0, 0.38, 0.12];
/// How firmly the head carries hair lying on the scalp (the source project's sleek crown is
/// carried at 0.9 to 0.96).
const SWEPT_PIN: f32 = 0.9;

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
/// Hair grown closer than this to a capsule's surface (or inside it) may come that close
/// again, m; up to [`SUNK_MAX`] deep. That is deep: a lock grown through the face hangs
/// through it as grown (capped at 3 cm, what was grown deeper was pushed out of capsules it
/// couldn't leave, and thrashed about at 6 m/s for good).
const GROWN_CLEAR: f32 = 0.003;
const SUNK_MAX: f32 = 0.1;

/// `points` (root first) as `n` points evenly along its length, and where each of `points`
/// lies among those: the resampled point before it and how far on to the next (0..1).
fn resample(points: &[Vec3], n: usize) -> (Vec<Vec3>, Vec<(u32, f32)>) {
    let mut along = Vec::with_capacity(points.len());
    let mut total = 0.0f32;
    for (i, p) in points.iter().enumerate() {
        if i > 0 {
            total += p.distance(points[i - 1]);
        }
        along.push(total);
    }
    let total = total.max(1e-6);
    let mut out = Vec::with_capacity(n);
    let mut seg = 1;
    for k in 0..n {
        let s = total * k as f32 / (n - 1) as f32;
        while seg + 1 < points.len() && along[seg] < s {
            seg += 1;
        }
        let span = (along[seg] - along[seg - 1]).max(1e-9);
        let t = ((s - along[seg - 1]) / span).clamp(0.0, 1.0);
        out.push(points[seg - 1].lerp(points[seg], t));
    }
    let at = along
        .iter()
        .map(|s| {
            let u = s / total * (n - 1) as f32;
            let k = (u.floor() as usize).min(n - 2);
            (k as u32, (u - k as f32).clamp(0.0, 1.0))
        })
        .collect();
    (out, at)
}

/// The hair body for `guides` on `person` (at `person_t`, posed as `pose`; `male`: 0 a woman's
/// build .. 1 a man's, for the body the hair slides off), and how its particles are handed
/// back as the guides' points.
pub fn build(
    guides: &HairGuides,
    person: Entity,
    person_t: &GlobalTransform,
    pose: &SkeletonPose,
    male: f32,
) -> Result<(SoftBodySim, GroomedRest), String> {
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
    let mut thickness = 0.0;
    let rest_of = |i: u16| {
        let p = guides.grown_on[i as usize];
        (p.translation, p.rotation)
    };
    // Every guide rides a skinning blend of its bones from the pose it was grown on, so its
    // model-space rest points are its points in that frame; a capsule rides its bone alike.
    let mut frame_for = |sim: &mut Sim, a: u16, b: u16, t: f32| {
        let blend = BoneBlend {
            owner: person,
            a: a as usize,
            b: b as usize,
            t,
            rest_a: rest_of(a),
            rest_b: rest_of(b),
        };
        let (bp, br) = blend.world(person_t, pose)?;
        let placed = ((bp - origin).as_vec3(), br);
        Some((
            *frame_of
                .entry((a, b, (t * 32.0).round() as u8))
                .or_insert_with(|| {
                    frames.push(FrameSource::Blend(blend));
                    sim.add_frame(placed.0, placed.1)
                }),
            placed,
        ))
    };
    let body = hair_body::capsules(guides, pose, male);
    let scalp = hair_body::root_sphere(guides, pose).filter(|_| !body.is_empty());
    let mut tress = Tress::default();
    tress.config.stiffness = (guides.stiffness / LONG_HAIR).clamp(0.5, 4.0);
    let mut groomed = GroomedRest::default();
    for (index, g) in guides.guides.iter().enumerate() {
        if g.points.len() < 2 {
            continue;
        }
        let [a, b] = g.bones;
        if a as usize >= bones || b as usize >= bones {
            return Err(format!("A hair guide rides bone {a} or {b} of {bones}."));
        }
        let t = g.blend.clamp(0.0, 1.0);
        let (b, t) = if t < 1.0 / 32.0 { (a, 0.0) } else { (b, t) };
        let Some((frame, placed)) = frame_for(&mut sim, a, b, t) else {
            continue;
        };
        *uses.entry(frame).or_default() += 1;
        let length: f32 = g.points.windows(2).map(|w| w[0].distance(w[1])).sum();
        let n = ((length / SEGMENT).round() as usize + 1).clamp(MIN_VERTS, MAX_VERTS);
        let (rest, at) = resample(&g.points, n);
        // Hair lying on the scalp is held there by the hair around it: where it was grown
        // within a centimeter or two of the cranium, above the ears. Without a fitted head,
        // what was grown swept back from the front and the crown, as far as that sweep goes.
        let held = |q: Vec3, arc: f32| match scalp {
            Some((center, radius)) => {
                let off = hair_body::distance(&body, q, false);
                (1.0 - smoothstep(0.008, 0.02, off))
                    * smoothstep(-0.35, -0.1, (q.y - center.y) / radius)
            }
            None => {
                let sweep = (-g.normal.z).max(g.normal.y);
                smoothstep(0.5, 0.9, sweep) * (1.0 - smoothstep(0.14, 0.26, arc))
            }
        };
        let first = sim.len() as u32;
        let mass = hair::GUIDE_MASS / n as f32;
        for (k, q) in rest.iter().enumerate() {
            sim.add_particle(placed.0 + placed.1 * *q, mass);
            let t = k as f32 / (n - 1) as f32;
            let sunk = |kind: bool| {
                let d = hair_body::distance(&body, *q, kind);
                (GROWN_CLEAR - d).clamp(0.0, SUNK_MAX)
            };
            tress.verts.push(TressVertex {
                rest: *q,
                rest_len: if k > 0 { q.distance(rest[k - 1]) } else { 0.0 },
                t,
                stiffness: SHAPE.0 + (SHAPE.1 - SHAPE.0) * t,
                bend: BEND.0 + (BEND.1 - BEND.0) * t,
                pin: PINS
                    .get(k)
                    .copied()
                    .unwrap_or(0.0)
                    .max(SWEPT_PIN * held(*q, length * t)),
                damping: 1.0,
                sunk: [sunk(false), sunk(true)],
            });
        }
        sim.pin(first, frame, rest[0]);
        tress.strands.push(TressStrand {
            first,
            count: n as u32,
            frame,
            seed: (index as f32 * 0.618_034).fract(),
        });
        groomed.grown.extend_from_slice(&g.points);
        groomed
            .at
            .extend(at.into_iter().map(|(k, f)| (first + k, f)));
        groomed.counts.push(g.points.len() as u32);
        let tangent = (g.points[1] - g.points[0])
            .reject_from(g.normal)
            .normalize_or(g.normal.any_orthonormal_vector());
        out.push(Guide {
            first,
            count: n as u32,
            frame,
            normal: g.normal.normalize_or(Vec3::Y),
            tangent,
        });
        thickness += g.thickness.max(0.0);
    }
    if out.is_empty() {
        return Err("HairGuides has no guide with two or more points.".into());
    }
    for c in &body {
        let Some((frame, _)) = frame_for(&mut sim, c.bone, c.bone, 0.0) else {
            continue;
        };
        tress.capsules.push(TressCapsule {
            frame,
            a: c.a,
            ra: c.ra,
            b: c.b,
            rb: c.rb,
            friction: c.friction,
            body: c.body,
        });
    }
    // The frame most of the hair rides (the head): what its motion is measured against.
    tress.carrier = uses
        .iter()
        .max_by_key(|(f, n)| (**n, std::cmp::Reverse(**f)))
        .map_or(0, |(f, _)| *f);
    groomed.rest = tress.verts.iter().map(|v| v.rest).collect();
    sim.tress = Some(Box::new(tress));
    let settings = crate::SimSettings {
        // The strand solver has its own damping and follows a dash itself; what is left
        // here is only what a settling body adds.
        damping: 0.0,
        follow: 0.0,
        ..HairGroom::hair_settings()
    };
    let thickness = (thickness / out.len() as f32 * 0.5 + 0.003).max(settings.thickness);
    sim.params.radius = thickness;
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
    Ok((body, groomed))
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
        Option<&Appearance>,
    )>,
    sims: Query<(Entity, &GuideHair)>,
) {
    let mut built: HashMap<Entity, (Entity, u32)> = sims
        .iter()
        .map(|(e, g)| (g.person, (e, g.version)))
        .collect();
    for (person, guides, t, pose, own_colliders, fitted, look) in &people {
        match built.remove(&person) {
            Some((_, version)) if version == guides.version => continue,
            Some((stale, _)) => commands.entity(stale).despawn(),
            None => {}
        }
        if pose.bones.is_empty() {
            continue; // the pose isn't evaluated yet
        }
        // The person's own capsules (for cloth, and for other bodies' hair and cloth to
        // meet): fitted to the hair when the person has none.
        if own_colliders.is_none() || fitted {
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
        // Without a look, shapes midway between a woman's and a man's.
        let male = look.map_or(0.5, |a| a.sex);
        match build(guides, person, t, pose, male) {
            Ok((body, groomed)) => {
                child.insert((body, groomed));
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
                    move || rest_shape(&guides, person, &stand, male),
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
        && groomed.rest.len() == rest.len()
    {
        groomed.rest = rest;
        groomed.kept.clear();
        groomed.solved = true;
        groomed.shown = 0.0;
    }
}

/// A point `t` (0..1) of the way from `points[k]` to `points[k + 1]` on the smooth curve
/// through a strand's particles `lo..=hi` (Catmull-Rom), and the curve's direction there.
fn on_strand(points: &[Vec3], (lo, hi): (usize, usize), k: usize, t: f32) -> (Vec3, Vec3) {
    let (p1, p2) = (points[k], points[k + 1]);
    let p0 = if k > lo { points[k - 1] } else { p1 * 2.0 - p2 };
    let p3 = if k + 1 < hi {
        points[k + 2]
    } else {
        p2 * 2.0 - p1
    };
    let (t2, t3) = (t * t, t * t * t);
    let at = 0.5
        * (p1 * 2.0
            + (p2 - p0) * t
            + (p0 * 2.0 - p1 * 5.0 + p2 * 4.0 - p3) * t2
            + (p1 * 3.0 - p0 - p2 * 3.0 + p3) * t3);
    let along = 0.5
        * ((p2 - p0)
            + (p0 * 2.0 - p1 * 5.0 + p2 * 4.0 - p3) * (2.0 * t)
            + (p1 * 3.0 - p0 - p2 * 3.0 + p3) * (3.0 * t2));
    (at, along)
}

/// Hand the simulated points back to the people, in their model space: every grown point on
/// its simulated strand, carrying what it was grown off the strand at rest ([`GroomedRest`]).
pub fn write_posed_guides(
    mut commands: Commands,
    mut sims: Query<(&GuideHair, &SoftBodySim, &mut GroomedRest)>,
    mut people: Query<(&GlobalTransform, Option<&mut HairGuidesPosed>)>,
) {
    for (hair, body, mut groomed) in &mut sims {
        let Ok((t, posed)) = people.get_mut(hair.person) else {
            continue;
        };
        let Some(strands) = &body.strands else {
            continue;
        };
        let sim = &body.sim;
        if groomed.rest.len() != sim.len() || groomed.counts.len() != strands.guides.len() {
            continue;
        }
        if groomed.shown < 1.0 && groomed.solved && body.pending_prewarm == 0 {
            groomed.shown = (groomed.shown + 1.0 / SHOWN_OVER).min(1.0);
        }
        if groomed.kept.len() != groomed.counts.len() {
            // The body's shapes where they are now, in the solver's space.
            let body: Vec<(Vec3, Vec3, f32, f32)> = sim
                .tress
                .iter()
                .flat_map(|t| &t.capsules)
                .filter(|c| c.body)
                .filter_map(|c| {
                    let f = sim.frames.get(c.frame as usize)?;
                    Some((
                        f.to_pos + f.to_rot * c.a,
                        f.to_pos + f.to_rot * c.b,
                        c.ra,
                        c.rb,
                    ))
                })
                .collect();
            let mut first_point = 0usize;
            let kept = strands
                .guides
                .iter()
                .zip(&groomed.counts)
                .map(|(g, count)| {
                    let start = first_point;
                    first_point += *count as usize;
                    let run = (g.first as usize, (g.first + g.count) as usize - 1);
                    let Some(f) = sim.frames.get(g.frame as usize) else {
                        return 1.0;
                    };
                    let mut on_body = false;
                    let off = (start..start + *count as usize)
                        .map(|i| {
                            let (k, along) = groomed.at[i];
                            let k = (k as usize).clamp(run.0, run.1.saturating_sub(1));
                            let rest = on_strand(&groomed.rest, run, k, along).0;
                            let at = f.to_pos + f.to_rot * rest;
                            on_body |= body.iter().any(|&(a, b, ra, rb)| {
                                crate::tress::tapered_capsule(at, a, b, ra, rb).1 < ON_BODY
                            });
                            groomed.grown[i].distance(rest)
                        })
                        .fold(0.0f32, f32::max);
                    if on_body {
                        1.0 - smoothstep(GROWN_OFF.0, GROWN_OFF.1, off)
                    } else {
                        1.0
                    }
                })
                .collect();
            groomed.kept = kept;
        }
        let groomed = &*groomed;
        let w = groomed.shown;
        let to_model = t.rotation().conjugate();
        let scale = t.scale().max(Vec3::splat(1e-6));
        let mut first_point = 0usize;
        let points = strands
            .guides
            .iter()
            .zip(groomed.counts.iter().zip(&groomed.kept))
            .flat_map(|(g, (count, kept))| {
                let start = first_point;
                first_point += *count as usize;
                let frame = sim.frames.get(g.frame as usize).copied();
                let run = (g.first as usize, (g.first + g.count) as usize - 1);
                (start..start + *count as usize).map(move |i| {
                    let Some(f) = frame else {
                        return Vec3::ZERO;
                    };
                    let grown = groomed.grown[i];
                    let carried = f.to_pos + f.to_rot * grown;
                    if w <= 0.0 {
                        return carried;
                    }
                    let (k, along) = groomed.at[i];
                    let k = (k as usize).clamp(run.0, run.1.saturating_sub(1));
                    let (now, _) = on_strand(&sim.pos, run, k, along);
                    let (rest, _) = on_strand(&groomed.rest, run, k, along);
                    // What the point was grown off where the strand rests goes along as it
                    // is, with the bone: turned as the strand bends there, the centimeters
                    // between the two by the tips swung about with every kink of a lock (a
                    // comb of hairs stood out sideways from a turning head).
                    carried.lerp(now + f.to_rot * (grown - rest) * *kept, w)
                })
            })
            .map(|at| {
                let world = sim.origin + at.as_dvec3();
                to_model * (world - t.translation()).as_vec3() / scale
            });
        // The shapes the hair was held out of, for the hairs drawn between the guides.
        let capsules = sim.tress.iter().flat_map(|t| &t.capsules).filter_map(|c| {
            let f = sim.frames.get(c.frame as usize)?;
            let to_model = |at: Vec3| {
                let world = sim.origin + (f.to_pos + f.to_rot * at).as_dvec3();
                to_model * (world - t.translation()).as_vec3() / scale
            };
            let size = scale.max_element();
            Some(HairCapsule {
                a: to_model(c.a),
                ra: c.ra / size,
                b: to_model(c.b),
                rb: c.rb / size,
            })
        });
        match posed {
            Some(mut posed) => {
                posed.version = hair.version;
                posed.points.clear();
                posed.points.extend(points);
                posed.capsules.clear();
                posed.capsules.extend(capsules);
            }
            None => {
                commands.entity(hair.person).insert(HairGuidesPosed {
                    version: hair.version,
                    points: points.collect(),
                    capsules: capsules.collect(),
                    ..Default::default()
                });
            }
        }
    }
}
