//! Procedural legged locomotion for any number of legs: horses and dogs (walk, trot, canter,
//! gallop), spiders and insects (alternating tetrapods/tripods), mechs and birds.
//!
//! Feet are planted in the world while on the ground (no sliding), swing along an arc to a
//! landing spot predicted from the body's velocity (half a stance ahead, so each stance is
//! centered under the hip), and land on whatever the ground probe finds (terrain, physics
//! colliders, [`IkSurface`](crate::ik::IkSurface)s). The body follows the ground, tilts to
//! the feet, bobs and rocks with the gait. Speed comes from how the entity actually moves, so
//! whatever drives it (AI, physics, a path), the legs keep up.

use engine_core::prelude::*;
use engine_world::animation::AnimationParams;
use engine_world::physics::PhysicsQueries;
use engine_world::{ContactKind, Ground, SurfaceId, WorldShift};

use crate::animator::{Animator, PoseEdit};
use crate::ik::{GroundProbe, IkSurfaces, to_model, to_world, two_bone};
use crate::xform::{rotation_between, smoothing};

/// How a leg's bones are solved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LegStyle {
    /// `[upper, lower, end]`: hip/knee/ankle (humans, birds, simple creatures).
    #[default]
    TwoBone,
    /// `[root, upper, lower, end]`: the root yaws toward the foot (spider and insect coxa).
    YawRoot,
    /// `[upper, lower, ankle, end]`: the ankle segment stays upright and flips back while
    /// swinging (horse cannon and hoof, dog paws).
    Hoofed,
    /// `[scapula, upper, lower, ankle, end]`: a hoofed leg hung from a shoulder blade that
    /// rocks fore and aft toward the foot (a dog's or cat's foreleg), which lengthens the
    /// stride the way the real shoulder does.
    Scapular,
}

/// What touches the ground, for footprints and sounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FootKind {
    #[default]
    Foot,
    Paw,
    Hoof,
}

impl FootKind {
    pub fn contact(self) -> ContactKind {
        match self {
            FootKind::Foot => ContactKind::Foot,
            FootKind::Paw => ContactKind::Paw,
            FootKind::Hoof => ContactKind::Hoof,
        }
    }
}

/// One leg of a [`LeggedGait`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GaitLeg {
    pub bones: Vec<String>,
    pub style: LegStyle,
    /// Phase offset in the cycle (0..1), used when the gait has no stages.
    pub phase: f32,
    /// Model-space direction the middle joint bends toward.
    pub pole: Vec3,
    pub kind: FootKind,
    /// Footprint (width, length), meters.
    pub foot_size: [f32; 2],
    /// Height of the end bone above the ground when planted, meters.
    pub sole: f32,
    /// Where the stride centers, from the foot's rest spot (model space, meters): a hind leg
    /// that stands behind its hip strides from under the hip, so it reaches as far back as
    /// forward.
    pub stance_offset: Vec3,
}

impl Default for GaitLeg {
    fn default() -> Self {
        Self {
            bones: Vec::new(),
            style: LegStyle::TwoBone,
            phase: 0.0,
            pole: Vec3::NEG_Z,
            kind: FootKind::Foot,
            foot_size: [0.1, 0.25],
            sole: 0.05,
            stance_offset: Vec3::ZERO,
        }
    }
}

/// A gait used above a speed (walk, trot, canter, gallop).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GaitStage {
    pub name: String,
    /// Used from this speed up, m/s.
    pub min_speed: f32,
    /// Phase offset per leg (same order as the legs).
    pub offsets: Vec<f32>,
    /// Fraction of the cycle each foot is on the ground (0.75 slow walk .. 0.3 gallop).
    pub duty: f32,
    /// Vertical body bob, meters.
    pub bob: f32,
    /// Body rocking (nose up/down), degrees.
    pub pitch_deg: f32,
}

impl Default for GaitStage {
    fn default() -> Self {
        Self {
            name: String::new(),
            min_speed: 0.0,
            offsets: Vec::new(),
            duty: 0.6,
            bob: 0.02,
            pitch_deg: 0.0,
        }
    }
}

/// A bone that sways in time with the gait (head nod, tail swish, abdomen bob).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GaitSway {
    pub bone: String,
    /// Local rotation axis.
    pub axis: Vec3,
    /// Amplitude at full gait speed, degrees.
    pub amplitude_deg: f32,
    /// Cycles per gait cycle.
    pub frequency: f32,
    /// Phase offset (0..1).
    pub phase: f32,
}

impl Default for GaitSway {
    fn default() -> Self {
        Self {
            bone: String::new(),
            axis: Vec3::X,
            amplitude_deg: 5.0,
            frequency: 1.0,
            phase: 0.0,
        }
    }
}

/// Procedural legs. See the module docs.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LeggedGait {
    pub enabled: bool,
    /// Bone that carries the legs and bobs/tilts (pelvis, thorax).
    pub body: String,
    pub legs: Vec<GaitLeg>,
    /// Gaits by speed. Empty = one gait from the legs' `phase` and `duty`.
    pub stages: Vec<GaitStage>,
    pub duty: f32,
    /// Gait cycles per second when barely moving.
    pub base_frequency: f32,
    /// Extra cycles per second per m/s of speed.
    pub frequency_per_speed: f32,
    /// Foot lift, meters (plus `step_height_per_speed` per m/s).
    pub step_height: f32,
    pub step_height_per_speed: f32,
    /// 0..1: how much the body rises and falls with the ground under the feet.
    pub body_follow_ground: f32,
    /// 0..1: how much the body tilts to match the feet.
    pub body_tilt: f32,
    /// Speed at which sways and muscle effort reach full strength, m/s.
    pub full_speed: f32,
    /// Distance from the body center to the feet, for turning in place, meters.
    pub turn_radius: f32,
    pub mass_kg: f32,
    pub sway: Vec<GaitSway>,
    #[serde(skip)]
    pub(crate) rt: GaitRuntime,
}

impl Default for LeggedGait {
    fn default() -> Self {
        Self {
            enabled: true,
            body: "body".into(),
            legs: Vec::new(),
            stages: Vec::new(),
            duty: 0.6,
            base_frequency: 0.6,
            frequency_per_speed: 0.4,
            step_height: 0.1,
            step_height_per_speed: 0.02,
            body_follow_ground: 0.8,
            body_tilt: 0.8,
            full_speed: 4.0,
            turn_radius: 0.5,
            mass_kg: 80.0,
            sway: Vec::new(),
            rt: GaitRuntime::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct FootState {
    planted: bool,
    pos: DVec3,
    liftoff: DVec3,
    normal: Vec3,
    surface: SurfaceId,
    swing: f32,
}

/// A foot that just touched down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Footfall {
    pub leg: usize,
    pub position: DVec3,
    pub normal: Vec3,
    pub surface: SurfaceId,
    pub speed: f32,
    pub load_kg: f32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct GaitRuntime {
    initialized: bool,
    phase: f32,
    offsets: Vec<f32>,
    duty: f32,
    stage: usize,
    feet: Vec<FootState>,
    prev_origin: Option<DVec3>,
    prev_forward: Option<Vec3>,
    velocity: Vec3,
    yaw_rate: f32,
    body_offset: f32,
    body_pitch: f32,
    body_roll: f32,
    pub(crate) footfalls: Vec<Footfall>,
    /// Bone indices per leg, resolved once per skeleton.
    legs: Vec<LegBones>,
    legs_skeleton: String,
}

impl GaitRuntime {
    /// Carry planted feet and motion history into a shifted world frame.
    pub(crate) fn follow_shift(&mut self, s: &WorldShift) {
        for f in &mut self.feet {
            f.pos = s.point(f.pos);
            f.liftoff = s.point(f.liftoff);
            f.normal = s.direction(f.normal);
        }
        for f in &mut self.footfalls {
            f.position = s.point(f.position);
            f.normal = s.direction(f.normal);
        }
        self.prev_origin = self.prev_origin.map(|o| s.point(o));
        self.prev_forward = self.prev_forward.map(|f| s.direction(f));
        self.velocity = s.direction(self.velocity);
    }
}

impl LeggedGait {
    /// Current speed (m/s) as measured from the entity's motion.
    pub fn speed(&self) -> f32 {
        Vec3::new(self.rt.velocity.x, 0.0, self.rt.velocity.z).length()
    }

    /// Name of the current gait stage.
    pub fn stage_name(&self) -> &str {
        self.stages
            .get(self.rt.stage)
            .map_or("default", |s| s.name.as_str())
    }

    /// Whether leg `i` is on the ground.
    pub fn planted(&self, i: usize) -> bool {
        self.rt.feet.get(i).is_some_and(|f| f.planted)
    }

    pub fn phase(&self) -> f32 {
        self.rt.phase
    }

    /// Footfalls since the last call (drained by the event system).
    pub fn take_footfalls(&mut self) -> Vec<Footfall> {
        std::mem::take(&mut self.rt.footfalls)
    }
}

/// Wrap-aware move of a phase toward a target, at most `max` per call.
fn approach_phase(current: f32, target: f32, max: f32) -> f32 {
    let mut d = (target - current).rem_euclid(1.0);
    if d > 0.5 {
        d -= 1.0;
    }
    (current + d.clamp(-max, max)).rem_euclid(1.0)
}

#[allow(clippy::type_complexity)]
pub(crate) fn gait_system(
    ground: Res<Ground>,
    physics: Option<Res<PhysicsQueries>>,
    surfaces: Res<IkSurfaces>,
    mut q: Query<(
        Entity,
        &mut Animator,
        &mut LeggedGait,
        &mut AnimationParams,
        &GlobalTransform,
    )>,
) {
    q.par_iter_mut()
        .for_each(|(entity, mut animator, mut gait, mut params, gt)| {
            if !gait.enabled || gait.legs.is_empty() {
                return;
            }
            let probe = GroundProbe {
                ground: &ground,
                physics: physics.as_deref(),
                surfaces: &surfaces.0,
                exclude: Some(entity),
            };
            step_gait(&mut animator, &mut gait, &mut params, gt, &probe);
        });
}

#[derive(Debug, Clone, PartialEq, Default)]
struct LegBones {
    bones: Vec<usize>,
    end: usize,
    neutral: Vec3,
}

/// Advance one creature's gait and pose its legs.
pub fn step_gait(
    animator: &mut Animator,
    gait: &mut LeggedGait,
    params: &mut AnimationParams,
    gt: &GlobalTransform,
    probe: &GroundProbe,
) {
    let skeleton = animator.skeleton.clone();
    let Some(body) = skeleton.index_of(&gait.body) else {
        return;
    };
    if gait.rt.legs_skeleton != skeleton.name || gait.rt.legs.len() != gait.legs.len() {
        let resolved: Option<Vec<LegBones>> = gait
            .legs
            .iter()
            .map(|l| {
                let bones: Vec<usize> = l
                    .bones
                    .iter()
                    .map(|b| skeleton.index_of(b))
                    .collect::<Option<_>>()?;
                let end = *bones.last()?;
                let rest = skeleton.rest_model[end].t;
                Some(LegBones {
                    bones,
                    end,
                    neutral: Vec3::new(
                        rest.x + l.stance_offset.x,
                        l.sole,
                        rest.z + l.stance_offset.z,
                    ),
                })
            })
            .collect();
        let Some(resolved) = resolved else {
            return;
        };
        gait.rt.legs = resolved;
        gait.rt.legs_skeleton = skeleton.name.clone();
    }
    let legs = std::mem::take(&mut gait.rt.legs);
    let dt = animator.dt;
    let origin = gt.translation();
    let forward = gt.forward();
    let rt = &mut gait.rt;
    let n = legs.len();
    let scale = gt.scale().y.max(1e-3);

    // Measure motion.
    if dt > 0.0 {
        if let Some(prev) = rt.prev_origin {
            let raw = ((origin - prev) / dt as f64).as_vec3();
            // Ignore teleports.
            let raw = if raw.length() > 60.0 {
                rt.velocity
            } else {
                raw
            };
            rt.velocity = rt.velocity.lerp(raw, smoothing(dt, 0.12));
        }
        if let Some(pf) = rt.prev_forward {
            let a = pf.cross(forward).y.clamp(-1.0, 1.0).asin();
            rt.yaw_rate += (a / dt - rt.yaw_rate) * smoothing(dt, 0.12);
        }
    }
    rt.prev_origin = Some(origin);
    rt.prev_forward = Some(forward);
    let speed = Vec3::new(rt.velocity.x, 0.0, rt.velocity.z).length();
    let effective = speed + rt.yaw_rate.abs() * gait.turn_radius;

    // Pick the gait for this speed (with a little hysteresis).
    if !gait.stages.is_empty() {
        let mut stage = rt.stage.min(gait.stages.len() - 1);
        while stage + 1 < gait.stages.len() && speed > gait.stages[stage + 1].min_speed + 0.2 {
            stage += 1;
        }
        while stage > 0 && speed < gait.stages[stage].min_speed - 0.2 {
            stage -= 1;
        }
        rt.stage = stage;
    }
    let (target_offsets, target_duty, bob, pitch_amp): (Vec<f32>, f32, f32, f32) =
        match gait.stages.get(rt.stage) {
            Some(s) => (
                (0..n)
                    .map(|i| s.offsets.get(i).copied().unwrap_or(gait.legs[i].phase))
                    .collect(),
                s.duty,
                s.bob,
                s.pitch_deg.to_radians(),
            ),
            None => (
                gait.legs.iter().map(|l| l.phase).collect(),
                gait.duty,
                0.02,
                0.0,
            ),
        };
    if !rt.initialized {
        rt.offsets = target_offsets.clone();
        rt.duty = target_duty;
        rt.feet = legs
            .iter()
            .map(|l| {
                let p = to_world(gt, l.neutral);
                let hit = probe.probe(p, 0.6, 1.0);
                let pos = hit.map_or(p, |h| DVec3::new(p.x, h.height + l.neutral.y as f64, p.z));
                FootState {
                    planted: true,
                    pos,
                    liftoff: pos,
                    normal: hit.map_or(Vec3::Y, |h| h.normal),
                    surface: hit.map_or(SurfaceId::DEFAULT, |h| h.surface),
                    swing: 0.0,
                }
            })
            .collect();
        rt.initialized = true;
    }
    // Ease into a new gait's leg timing instead of jumping.
    let blend = (dt * 1.5).min(1.0);
    for (o, t) in rt.offsets.iter_mut().zip(&target_offsets) {
        *o = approach_phase(*o, *t, blend * 0.5);
    }
    rt.duty += (target_duty - rt.duty) * smoothing(dt, 0.3);
    let frequency = gait.base_frequency + gait.frequency_per_speed * speed;
    let any_swinging = rt.feet.iter().any(|f| !f.planted);
    if effective > 0.05 || any_swinging {
        rt.phase = (rt.phase + frequency * dt).rem_euclid(1.0);
    }
    let duty = rt.duty.clamp(0.1, 0.95);
    let step_height = (gait.step_height + gait.step_height_per_speed * speed) as f64 * scale as f64;
    let intensity = (speed / gait.full_speed.max(0.1)).clamp(0.0, 1.0);

    // Feet.
    let stance_secs = duty / frequency.max(0.05);
    let swing_secs = (1.0 - duty) / frequency.max(0.05);
    for (i, leg) in legs.iter().enumerate() {
        let psi = (rt.phase + rt.offsets[i]).rem_euclid(1.0);
        let in_stance = psi < duty;
        let neutral_world = to_world(gt, leg.neutral);
        let foot = &mut rt.feet[i];
        if in_stance && !foot.planted {
            // Touchdown.
            foot.planted = true;
            if let Some(h) = probe.probe(foot.pos, 0.4, 0.8) {
                foot.pos.y = h.height + (gait.legs[i].sole * scale) as f64;
                foot.normal = h.normal;
                foot.surface = h.surface;
            }
            rt.footfalls.push(Footfall {
                leg: i,
                position: foot.pos - DVec3::Y * (gait.legs[i].sole * scale) as f64,
                normal: foot.normal,
                surface: foot.surface,
                speed,
                load_kg: gait.mass_kg / (n as f32 * duty).max(1.0),
            });
        } else if !in_stance && foot.planted && (effective > 0.05 || dt == 0.0) {
            foot.planted = false;
            foot.liftoff = foot.pos;
        }
        if foot.planted {
            foot.swing = 0.0;
            // Never leave a planted foot out of reach (after a teleport or a sharp turn).
            let reach = 0.9
                * (to_world(gt, skeleton.rest_model[leg.bones[0]].t) - neutral_world)
                    .length()
                    .max(0.2);
            let off = foot.pos - neutral_world;
            let flat = DVec3::new(off.x, 0.0, off.z);
            if flat.length() > reach {
                let fixed = neutral_world + flat.normalize() * reach;
                foot.pos.x = fixed.x;
                foot.pos.z = fixed.z;
            }
        } else {
            let s = ((psi - duty) / (1.0 - duty)).clamp(0.0, 1.0);
            foot.swing = s;
            let remaining = (1.0 - s) * swing_secs;
            let lead = rt.velocity.as_dvec3() * (remaining + stance_secs * 0.5) as f64;
            let mut landing = neutral_world + DVec3::new(lead.x, 0.0, lead.z);
            if let Some(h) = probe.probe(landing, 0.6, 1.0) {
                landing.y = h.height + (gait.legs[i].sole * scale) as f64;
                foot.normal = h.normal;
                foot.surface = h.surface;
            } else {
                landing.y = neutral_world.y;
            }
            let e = s * s * (3.0 - 2.0 * s);
            let mut p = foot.liftoff.lerp(landing, e as f64);
            p.y += step_height * (std::f64::consts::PI * s as f64).sin();
            foot.pos = p;
        }
    }

    // Body: follow the ground under the feet, tilt to them, bob and rock with the gait.
    animator.refresh_model();
    let up_model = (gt.rotation().conjugate() * Vec3::Y).normalize();
    let mut ground_offset = 0.0;
    let (mut front, mut back, mut left, mut right) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (i, leg) in legs.iter().enumerate() {
        let foot_model = to_model(gt, rt.feet[i].pos);
        let planted_height = if rt.feet[i].planted {
            foot_model.dot(up_model) - leg.neutral.y
        } else {
            // A swinging foot counts by where it will land, not how high it is lifted.
            (to_model(gt, rt.feet[i].liftoff).dot(up_model) - leg.neutral.y)
                * (1.0 - rt.feet[i].swing)
        };
        ground_offset += planted_height / n as f32;
        if leg.neutral.z < 0.0 {
            front.push(planted_height);
        } else {
            back.push(planted_height);
        }
        if leg.neutral.x < 0.0 {
            left.push(planted_height);
        } else {
            right.push(planted_height);
        }
    }
    let avg = |v: &Vec<f32>| {
        if v.is_empty() {
            0.0
        } else {
            v.iter().sum::<f32>() / v.len() as f32
        }
    };
    let length = legs
        .iter()
        .map(|l| l.neutral.z)
        .fold(0.0f32, |a, z| a.max(z.abs()))
        .max(0.1)
        * 2.0;
    let width = legs
        .iter()
        .map(|l| l.neutral.x)
        .fold(0.0f32, |a, x| a.max(x.abs()))
        .max(0.1)
        * 2.0;
    let pitch_target = ((avg(&front) - avg(&back)) / length).atan() * gait.body_tilt;
    let roll_target = ((avg(&left) - avg(&right)) / width).atan() * gait.body_tilt;
    let k = smoothing(dt, 0.1);
    rt.body_offset += (ground_offset * gait.body_follow_ground - rt.body_offset) * k;
    rt.body_pitch += (pitch_target - rt.body_pitch) * k;
    rt.body_roll += (roll_target - rt.body_roll) * k;
    let cycle = rt.phase * std::f32::consts::TAU;
    let bob_now = -bob * intensity * (2.0 * cycle).cos();
    let rock = pitch_amp * intensity * cycle.sin();
    let side = up_model.cross(Vec3::NEG_Z).normalize_or(Vec3::X);
    let fwd_axis = Vec3::NEG_Z;
    {
        let mut edit = animator.edit();
        edit.translate_model(body, up_model * (rt.body_offset + bob_now));
        let tilt = Quat::from_axis_angle(side, -(rt.body_pitch + rock))
            * Quat::from_axis_angle(fwd_axis, rt.body_roll);
        edit.rotate_model(body, tilt);
        // Rhythmic sways.
        for sway in &gait.sway {
            if let Some(b) = skeleton.index_of(&sway.bone) {
                let a = sway.amplitude_deg.to_radians()
                    * intensity.max(0.25)
                    * ((rt.phase * sway.frequency + sway.phase) * std::f32::consts::TAU).sin();
                edit.rotate_local(b, Quat::from_axis_angle(sway.axis.normalize_or(Vec3::X), a));
            }
        }
    }

    // Legs.
    for (i, leg) in legs.iter().enumerate() {
        let def = &gait.legs[i];
        let target = to_model(gt, rt.feet[i].pos);
        let swing = rt.feet[i].swing;
        let normal_model = (gt.rotation().conjugate() * rt.feet[i].normal).normalize();
        let mut edit = animator.edit();
        match (def.style, leg.bones.as_slice()) {
            (LegStyle::TwoBone, &[a, b, c, ..]) => {
                let end_rot = edit.model[c].r;
                two_bone(&mut edit, a, b, c, target, def.pole, 1.0);
                edit.set_model_rotation(c, end_rot);
            }
            (LegStyle::YawRoot, &[root, a, b, c, ..]) => {
                let r = edit.position(root);
                let cur = edit.position(c) - r;
                let want = target - r;
                let flat = |v: Vec3| (v - up_model * v.dot(up_model)).normalize_or(Vec3::NEG_Z);
                edit.rotate_model(root, rotation_between(flat(cur), flat(want)));
                two_bone(&mut edit, a, b, c, target, def.pole, 1.0);
            }
            (LegStyle::Scapular, &[scap, a, b, ankle, end, ..]) => {
                // Rock the shoulder blade toward the foot: its lower end (the shoulder joint)
                // goes forward as the paw reaches ahead and back as it pushes off, carrying
                // half the paw's offset, then the leg below reaches like a hoofed one.
                let ankle_rot = edit.model[ankle].r;
                let top = edit.position(scap);
                let arm = edit.position(a) - top;
                let ahead = (target - leg.neutral).dot(fwd_axis);
                let want = (ahead * 0.5 / arm.length().max(1e-3)).clamp(-0.5, 0.5);
                let lateral = up_model.cross(fwd_axis).normalize_or(Vec3::X);
                let rock = |angle: f32| Quat::from_axis_angle(lateral, -angle);
                // The leg below must still reach the foot: rocking a sloped blade forward also
                // lifts the shoulder, so it rocks no further than the leg can reach from.
                let span = |i: usize, j: usize| (edit.position(j) - edit.position(i)).length();
                let leg_length = (span(a, b) + span(b, ankle) + span(ankle, end)) * 0.98;
                let miss = |angle: f32| (target - (top + rock(angle) * arm)).length() - leg_length;
                let mut angle = want;
                if miss(want) > 0.0 {
                    let candidates = (-12..=12).map(|k| k as f32 * 0.05);
                    angle = candidates
                        .clone()
                        .filter(|&c| miss(c) <= 0.0)
                        .min_by(|x, y| (x - want).abs().total_cmp(&(y - want).abs()))
                        .or_else(|| candidates.min_by(|x, y| miss(*x).total_cmp(&miss(*y))))
                        .unwrap_or(want);
                }
                edit.rotate_model(scap, rock(angle));
                hoofed(
                    &mut edit,
                    [a, b, ankle, end],
                    ankle_rot,
                    target,
                    def.pole,
                    swing,
                    up_model,
                    fwd_axis,
                );
            }
            (LegStyle::Hoofed, &[a, b, ankle, end, ..]) => {
                let ankle_rot = edit.model[ankle].r;
                hoofed(
                    &mut edit,
                    [a, b, ankle, end],
                    ankle_rot,
                    target,
                    def.pole,
                    swing,
                    up_model,
                    fwd_axis,
                );
            }
            _ => {}
        }
        // Planted feet follow the ground slope.
        if rt.feet[i].planted {
            let tilt = rotation_between(up_model, normal_model);
            let (axis, angle) = tilt.to_axis_angle();
            if angle > 1e-3 {
                edit.rotate_model(leg.end, Quat::from_axis_angle(axis, angle.min(0.6)));
            }
        }
    }
    gait_outputs(gait, params, dt, legs);
}

/// A hoofed leg `[upper, lower, ankle, end]`: keep the cannon as animated (`ankle_rot`),
/// folded during the swing (and tipped on the toe when out of reach), and reach the knee
/// (hock) to where that cannon puts the hoof on its path. In the swing a foreleg's cannon
/// folds back under the knee and a hind leg's swings forward as the hock bends; either way
/// the hoof (paw) curls back behind the fetlock, as a horse's or dog's does.
#[allow(clippy::too_many_arguments)]
fn hoofed(
    edit: &mut PoseEdit,
    [a, b, ankle, end]: [usize; 4],
    ankle_rot: Quat,
    target: Vec3,
    pole: Vec3,
    swing: f32,
    up_model: Vec3,
    fwd: Vec3,
) {
    let cannon = edit.position(end) - edit.position(ankle);
    let lift = (std::f32::consts::PI * swing.clamp(0.0, 1.0)).sin();
    // A knee that bends forward (the stifle) marks a hind leg, whose hock folds less.
    let hind = pole.dot(fwd) > 0.0;
    let fold = lift * if hind { 0.8 } else { 1.1 };
    let lateral = up_model.cross(pole).normalize_or(Vec3::X);
    let flip = Quat::from_axis_angle(lateral, -fold);
    // A foot the leg can't reach with the cannon as animated pivots on its toe: the cannon
    // tips (the hock or fetlock rises toward the hip) as far as it takes, the way a dog's
    // hock lifts as the paw pushes off far behind it, instead of the foot leaving the ground.
    let hip = edit.position(a);
    let reach = ((edit.position(b) - hip).length()
        + (edit.position(ankle) - edit.position(b)).length())
        * 0.995;
    let hock = |q: Quat| target - q * flip * cannon;
    let mut tip = Quat::IDENTITY;
    if (hock(tip) - hip).length() > reach {
        let mut best = f32::MAX;
        'search: for step in 1..=24 {
            for sign in [1.0, -1.0] {
                let q = Quat::from_axis_angle(lateral, sign * step as f32 * 0.05);
                let d = (hock(q) - hip).length();
                if d < best {
                    best = d;
                    tip = q;
                }
                if d <= reach {
                    break 'search;
                }
            }
        }
    }
    two_bone(edit, a, b, ankle, hock(tip), pole, 1.0);
    edit.set_model_rotation(ankle, ankle_rot);
    if fold > 0.0 || tip != Quat::IDENTITY {
        edit.rotate_model(ankle, tip * flip);
    }
    if lift > 0.0 {
        // The fetlock flexes: the hoof swings back toward the back of the cannon (on a hind
        // leg first undoing the hock's forward fold).
        let curl = 0.6 * lift + if hind { fold } else { 0.0 };
        let back = fwd.cross(up_model).normalize_or(Vec3::X);
        edit.rotate_model(end, Quat::from_axis_angle(back, -curl));
    }
}

/// Inputs for muscles and other systems: speed, intensity, stage and each leg's load.
fn gait_outputs(gait: &mut LeggedGait, params: &mut AnimationParams, dt: f32, legs: Vec<LegBones>) {
    let speed = gait.speed();
    params.set("gait.speed", speed);
    params.set(
        "gait.intensity",
        (speed / gait.full_speed.max(0.1)).clamp(0.0, 1.0),
    );
    params.set("gait.stage", gait.rt.stage as f32);
    for (i, f) in gait.rt.feet.iter().enumerate() {
        let load = if f.planted { 1.0 } else { 0.0 };
        let key = GAIT_LOAD_KEYS.get(i).copied().unwrap_or("gait.load.x");
        let prev = params.get(key);
        params.set(key, prev + (load - prev) * smoothing(dt, 0.08));
    }
    gait.rt.legs = legs;
}

/// Parameter names for per-leg load (`gait.load.0` …), so muscles can flex on stance.
pub const GAIT_LOAD_KEYS: [&str; 8] = [
    "gait.load.0",
    "gait.load.1",
    "gait.load.2",
    "gait.load.3",
    "gait.load.4",
    "gait.load.5",
    "gait.load.6",
    "gait.load.7",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_wrap_the_short_way() {
        assert!((approach_phase(0.95, 0.05, 1.0) - 0.05).abs() < 1e-5);
        assert!((approach_phase(0.95, 0.05, 0.02) - 0.97).abs() < 1e-5);
        assert!((approach_phase(0.1, 0.3, 0.05) - 0.15).abs() < 1e-5);
    }
}
