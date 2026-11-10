//! Pose validator: checks every humanoid's final pose each frame against the rules a viewer
//! notices first (characters week, research-ik-hands-skinning.md 6 and
//! research-animation-system.md 5):
//!
//! - **wrists** inside their range: flexion 80°, extension 70°, deviation 35°, roll 100° from
//!   the forearm (the logic skeleton's hand carries the forearm's roll), 5° slack;
//! - **elbows and knees** bend their own way (no reversed or sideways hinges) and never flip
//!   across the limb from one frame to the next (**pole flips**);
//! - **roll jitter**: an upper arm's or thigh's roll about its length changing speed by more
//!   than 2° in a frame;
//! - **hands on grips**: an off hand on a two-handed grip within 1 cm and 10° of its seat,
//!   and the fingers of a hand holding something on the handle (palm gap, deepest finger);
//! - **feet** that foot IK holds planted slide less than 2 cm/s;
//! - **bones** keep their length (0.5%) and nothing is NaN.
//!
//! Opt in by inserting [`PoseAudit`] (the showcase does when `ANIM_POSE_AUDIT=<file>` is set):
//! every humanoid gets a [`PoseCheck`] that keeps its worst values, and the audit writes them
//! as a table to the file every second. Tests read [`PoseCheck::stats`] directly.

use std::fmt::Write as _;

use engine_core::prelude::*;
use serde::{Deserialize, Serialize};

use crate::animator::Animator;
use crate::build::humanoid::{FOOT, HAND, LOWER_ARM, LOWER_LEG, UPPER_ARM, UPPER_LEG};
use crate::grip::PALM;
use crate::skeleton::Skeleton;
use crate::xform::{Xf, twist_angle};

/// A planted foot moving farther than this in a frame went with its body somewhere else (a
/// demo routine starting over, a warp): not a slide.
const TELEPORT_M: f64 = 0.25;

/// The rules' thresholds (degrees, meters, cm/s).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    pub wrist_flex: f32,
    pub wrist_extend: f32,
    pub wrist_deviate: f32,
    pub wrist_roll: f32,
    /// How far past a range a joint must be to count.
    pub slack: f32,
    /// Bend (degrees) under which a hinge counts as straight (no direction to check).
    pub straight: f32,
    pub roll_jitter: f32,
    pub grip_m: f32,
    pub grip_deg: f32,
    pub slide_cms: f32,
    pub stretch: f32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            wrist_flex: 80.0,
            wrist_extend: 70.0,
            wrist_deviate: 35.0,
            wrist_roll: 100.0,
            slack: 5.0,
            straight: 12.0,
            roll_jitter: 2.0,
            grip_m: 0.01,
            grip_deg: 10.0,
            slide_cms: 2.0,
            stretch: 0.005,
        }
    }
}

/// One character's worst values since the check started (or was [`reset`](PoseCheck::reset)).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PoseStats {
    pub frames: u32,
    pub nan: u32,
    /// Frames with a wrist past its range (by more than the slack), and the largest excess.
    pub wrist_out: u32,
    pub wrist_worst_deg: f32,
    /// Which wrist rule the worst excess broke (`flex`, `extend`, `deviate`, `roll`).
    pub wrist_worst: String,
    /// Frames with an elbow or knee bent backward or sideways past the slack.
    pub hinge_bad: u32,
    pub hinge_worst_deg: f32,
    /// Frames where a bent elbow or knee jumped across its limb.
    pub pole_flips: u32,
    /// Frames with roll jitter over the limit, and the largest (degrees per frame²).
    pub roll_jitter: u32,
    pub roll_jitter_deg: f32,
    /// Off hands on two-handed grips: the farthest from the seat (m) and the largest turn.
    pub grip_miss_m: f32,
    pub grip_miss_deg: f32,
    /// Hands holding something: the widest palm gap to the handle (m), the deepest finger in
    /// it (m, positive) and the widest finger gap.
    pub palm_gap_m: f32,
    pub finger_sink_m: f32,
    pub finger_gap_m: f32,
    /// Planted feet: the fastest slide (cm/s), and frames over the limit.
    pub foot_slide_cms: f32,
    pub foot_slides: u32,
    /// The largest bone length change, as a fraction of the rest length.
    pub stretch: f32,
    /// Hinge-bad, flip and roll-jitter frames per limb (left arm, right arm, left leg, right
    /// leg), to tell which limb a regression is in.
    pub by_limb: [[u32; 3]; 4],
    /// Per wrist (left, right): frames out of range, and the largest excess by rule (flex,
    /// extend, deviate, roll), to tell which hand and which way a wrist breaks.
    pub wrist_frames: [u32; 2],
    pub wrist_rules: [[f32; 4]; 2],
}

impl PoseStats {
    /// Whether these pass A1's bar: no wrist out of range, no hinge flips or reversals, hands
    /// on their grips, feet still.
    pub fn passes(&self, limits: &Limits) -> bool {
        self.nan == 0
            && self.wrist_out == 0
            && self.pole_flips == 0
            && self.hinge_bad == 0
            && self.grip_miss_m < limits.grip_m
            && self.grip_miss_deg < limits.grip_deg
            && self.foot_slide_cms < limits.slide_cms
    }

    fn merge(&mut self, o: &PoseStats) {
        self.frames += o.frames;
        self.nan += o.nan;
        self.wrist_out += o.wrist_out;
        if o.wrist_worst_deg > self.wrist_worst_deg {
            self.wrist_worst_deg = o.wrist_worst_deg;
            self.wrist_worst = o.wrist_worst.clone();
        }
        self.hinge_bad += o.hinge_bad;
        self.hinge_worst_deg = self.hinge_worst_deg.max(o.hinge_worst_deg);
        self.pole_flips += o.pole_flips;
        self.roll_jitter += o.roll_jitter;
        self.roll_jitter_deg = self.roll_jitter_deg.max(o.roll_jitter_deg);
        self.grip_miss_m = self.grip_miss_m.max(o.grip_miss_m);
        self.grip_miss_deg = self.grip_miss_deg.max(o.grip_miss_deg);
        self.palm_gap_m = self.palm_gap_m.max(o.palm_gap_m);
        self.finger_sink_m = self.finger_sink_m.max(o.finger_sink_m);
        self.finger_gap_m = self.finger_gap_m.max(o.finger_gap_m);
        self.foot_slide_cms = self.foot_slide_cms.max(o.foot_slide_cms);
        self.foot_slides += o.foot_slides;
        self.stretch = self.stretch.max(o.stretch);
        for (a, b) in self.by_limb.iter_mut().zip(o.by_limb) {
            for (x, y) in a.iter_mut().zip(b) {
                *x += y;
            }
        }
        for side in 0..2 {
            self.wrist_frames[side] += o.wrist_frames[side];
            for (x, y) in self.wrist_rules[side].iter_mut().zip(o.wrist_rules[side]) {
                *x = x.max(y);
            }
        }
    }
}

/// The table's header and one row per character.
pub fn table(rows: &[(String, PoseStats)]) -> String {
    let mut s = String::from(
        "character | frames | nan | wrist out (worst°, rule) | hinge bad (worst°) | pole flips | \
         roll jitter (worst°) | grip miss cm / ° | palm gap cm | finger sink mm / gap mm | \
         foot slide cm/s (frames) | stretch % | hinge/flip/jitter by limb (arm L, arm R, leg L, \
         leg R) | wrist frames (worst° flex/extend/deviate/roll) L; R\n",
    );
    for (name, r) in rows {
        let _ = writeln!(
            s,
            "{name} | {} | {} | {} ({:.1}, {}) | {} ({:.1}) | {} | {} ({:.2}) | {:.2} / {:.1} | \
             {:.2} | {:.1} / {:.1} | {:.2} ({}) | {:.2} | {} | {}",
            r.frames,
            r.nan,
            r.wrist_out,
            r.wrist_worst_deg,
            if r.wrist_worst.is_empty() {
                "-"
            } else {
                &r.wrist_worst
            },
            r.hinge_bad,
            r.hinge_worst_deg,
            r.pole_flips,
            r.roll_jitter,
            r.roll_jitter_deg,
            r.grip_miss_m * 100.0,
            r.grip_miss_deg,
            r.palm_gap_m * 100.0,
            r.finger_sink_m * 1000.0,
            r.finger_gap_m * 1000.0,
            r.foot_slide_cms,
            r.foot_slides,
            r.stretch * 100.0,
            r.by_limb
                .iter()
                .map(|[h, f, j]| format!("{h}/{f}/{j}"))
                .collect::<Vec<_>>()
                .join(", "),
            (0..2)
                .map(|k| {
                    let [f, e, d, o] = r.wrist_rules[k];
                    format!("{} ({f:.0}/{e:.0}/{d:.0}/{o:.0})", r.wrist_frames[k])
                })
                .collect::<Vec<_>>()
                .join("; "),
        );
    }
    s
}

/// The bones a humanoid check reads, looked up once.
#[derive(Debug, Clone, Copy)]
struct Bones {
    upper_arm: [usize; 2],
    lower_arm: [usize; 2],
    hand: [usize; 2],
    upper_leg: [usize; 2],
    lower_leg: [usize; 2],
    foot: [usize; 2],
}

impl Bones {
    fn of(s: &Skeleton) -> Option<Self> {
        let pair = |names: [&str; 2]| Some([s.index_of(names[0])?, s.index_of(names[1])?]);
        Some(Self {
            upper_arm: pair(UPPER_ARM)?,
            lower_arm: pair(LOWER_ARM)?,
            hand: pair(HAND)?,
            upper_leg: pair(UPPER_LEG)?,
            lower_leg: pair(LOWER_LEG)?,
            foot: pair(FOOT)?,
        })
    }

    /// The limbs' (parent, hinge child) pairs: elbows then knees, left then right.
    fn hinges(&self) -> [(usize, usize, bool); 4] {
        [
            (self.upper_arm[0], self.lower_arm[0], true),
            (self.upper_arm[1], self.lower_arm[1], true),
            (self.upper_leg[0], self.lower_leg[0], false),
            (self.upper_leg[1], self.lower_leg[1], false),
        ]
    }
}

/// A bone's rotation relative to its parent, measured from its rest (in its rest frame).
fn local_turn(s: &Skeleton, model: &[Xf], parent: usize, bone: usize) -> Quat {
    let now = model[parent].r.conjugate() * model[bone].r;
    let rest = s.rest_model[parent].r.conjugate() * s.rest_model[bone].r;
    (rest.conjugate() * now).normalize()
}

/// A wrist's turn from its forearm, radians: flexion (positive toward the palm, negative
/// extension), deviation (signed, about the palm's axis) and roll, as the validator
/// measures them (the hand points down −Y, its palm faces
/// [`PALM`]`[side]`).
fn wrist_parts(s: &Skeleton, model: &[Xf], lower: usize, hand: usize, side: usize) -> [f32; 3] {
    let turn = local_turn(s, model, lower, hand);
    let along = Vec3::Y;
    let roll = twist_angle(turn, along);
    let swing = (turn * Quat::from_axis_angle(along, roll).conjugate()).normalize();
    let (axis, angle) = swing.to_axis_angle();
    let angle = if angle > std::f32::consts::PI {
        angle - std::f32::consts::TAU
    } else {
        angle
    };
    let v = axis * angle;
    let palm = PALM[side];
    let flex_axis = Vec3::NEG_Y.cross(palm).normalize();
    [v.dot(flex_axis), v.dot(palm), roll]
}

/// A wrist's turn from its forearm, degrees: flexion (positive toward the palm, negative
/// extension), deviation (either way) and roll, as the validator measures them.
pub fn wrist_angles(
    s: &Skeleton,
    model: &[Xf],
    lower: usize,
    hand: usize,
    side: usize,
) -> [f32; 3] {
    let [flex, deviate, roll] = wrist_parts(s, model, lower, hand, side);
    [
        flex.to_degrees(),
        deviate.to_degrees().abs(),
        roll.to_degrees().abs(),
    ]
}

/// The hand's model rotation with its wrist turned back inside [`Limits`]' wrist range,
/// `margin` degrees short of each limit (the hand as it is when already inside).
pub fn wrist_in_range(
    s: &Skeleton,
    model: &[Xf],
    lower: usize,
    hand: usize,
    side: usize,
    margin: f32,
) -> Quat {
    let l = Limits::default();
    let [flex, deviate, roll] = wrist_parts(s, model, lower, hand, side);
    let lim = |d: f32| (d - margin).max(0.0).to_radians();
    let flex = flex.clamp(-lim(l.wrist_extend), lim(l.wrist_flex));
    let deviate = deviate.clamp(-lim(l.wrist_deviate), lim(l.wrist_deviate));
    let roll = roll.clamp(-lim(l.wrist_roll), lim(l.wrist_roll));
    let palm = PALM[side];
    let flex_axis = Vec3::NEG_Y.cross(palm).normalize();
    let v = flex_axis * flex + palm * deviate;
    let swing = Quat::from_scaled_axis(v);
    let turn = swing * Quat::from_axis_angle(Vec3::Y, roll);
    // Back from the turn (rest-relative, in the forearm's frame) to a model rotation.
    let rest = s.rest_model[lower].r.conjugate() * s.rest_model[hand].r;
    (model[lower].r * rest * turn).normalize()
}

/// How far a wrist is past its range ([`Limits`]' wrist rules, no slack), degrees: the root
/// of the summed squares of each rule's excess, 0 inside the range. What IK that can choose
/// how an arm reaches minimises.
pub fn wrist_strain(s: &Skeleton, model: &[Xf], lower: usize, hand: usize, side: usize) -> f32 {
    let l = Limits::default();
    let [flex, deviate, roll] = wrist_angles(s, model, lower, hand, side);
    [
        flex - l.wrist_flex,
        -flex - l.wrist_extend,
        deviate - l.wrist_deviate,
        roll - l.wrist_roll,
    ]
    .iter()
    .map(|e| e.max(0.0).powi(2))
    .sum::<f32>()
    .sqrt()
}

/// The per-frame memory a check needs, and the worst values so far.
#[derive(Component, Debug, Clone, Default)]
pub struct PoseCheck {
    pub stats: PoseStats,
    /// Each hinge's bend direction last frame (in its parent's frame), when it was bent.
    bends: [Option<Vec3>; 4],
    /// Upper arms and thighs: last frame's rotation from the parent, and roll speed (radians
    /// a frame).
    rolls: [(Option<Quat>, Option<f32>); 4],
    /// Each foot's world ankle position last frame while held planted.
    feet: [Option<DVec3>; 2],
}

impl PoseCheck {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Check one frame of a humanoid pose (`model`, model space of `skeleton`).
    pub fn check(&mut self, skeleton: &Skeleton, model: &[Xf], limits: &Limits) {
        let st = &mut self.stats;
        st.frames += 1;
        if model
            .iter()
            .any(|x| !(x.t.is_finite() && x.r.is_finite()) || (x.r.length() - 1.0).abs() > 1e-2)
        {
            st.nan += 1;
            return;
        }
        let Some(bones) = Bones::of(skeleton) else {
            return;
        };
        // Bone lengths of the limbs.
        for &(p, c, _) in &bones.hinges() {
            let end = if c == bones.lower_arm[0] {
                bones.hand[0]
            } else if c == bones.lower_arm[1] {
                bones.hand[1]
            } else if c == bones.lower_leg[0] {
                bones.foot[0]
            } else {
                bones.foot[1]
            };
            for (a, b) in [(p, c), (c, end)] {
                let rest = skeleton.rest_model[a].t.distance(skeleton.rest_model[b].t);
                if rest > 1e-3 {
                    let now = model[a].t.distance(model[b].t);
                    st.stretch = st.stretch.max((now / rest - 1.0).abs());
                }
            }
        }
        // Wrists.
        let mut out = false;
        for side in 0..2 {
            let mut side_out = false;
            let [flex, deviate, roll] = wrist_angles(
                skeleton,
                model,
                bones.lower_arm[side],
                bones.hand[side],
                side,
            );
            for (k, (excess, rule)) in [
                (flex - limits.wrist_flex, "flex"),
                (-flex - limits.wrist_extend, "extend"),
                (deviate - limits.wrist_deviate, "deviate"),
                (roll - limits.wrist_roll, "roll"),
            ]
            .into_iter()
            .enumerate()
            {
                if excess > limits.slack {
                    out = true;
                    side_out = true;
                    st.wrist_rules[side][k] = st.wrist_rules[side][k].max(excess);
                    if excess > st.wrist_worst_deg {
                        st.wrist_worst_deg = excess;
                        st.wrist_worst = format!("{rule} {}", ["L", "R"][side]);
                    }
                }
            }
            st.wrist_frames[side] += side_out as u32;
        }
        if out {
            st.wrist_out += 1;
        }
        // Elbows and knees: the child's direction in the parent's rest frame. An elbow bends
        // the forearm forward (−Z), a knee the shin back (+Z).
        let mut bad = false;
        let mut flipped = false;
        for (k, &(p, c, elbow)) in bones.hinges().iter().enumerate() {
            let turn = local_turn(skeleton, model, p, c);
            let dir = turn * Vec3::NEG_Y;
            let bend = dir.dot(Vec3::NEG_Y).clamp(-1.0, 1.0).acos().to_degrees();
            if bend < limits.straight {
                self.bends[k] = None;
            } else {
                let way = Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero();
                // The bend's angle off its own way (0 forward for an elbow, back for a knee).
                let own = if elbow { Vec3::NEG_Z } else { Vec3::Z };
                let off = own.dot(way).clamp(-1.0, 1.0).acos().to_degrees();
                // Elbows carry the forearm's carrying angle and knees some toe-out: allow
                // 35° off the plane before counting, scaled in as the bend grows.
                let excess = (off - 35.0) * ((bend - limits.straight) / 30.0).min(1.0);
                if excess > limits.slack {
                    bad = true;
                    st.by_limb[k][0] += 1;
                    st.hinge_worst_deg = st.hinge_worst_deg.max(excess);
                }
                if let Some(was) = self.bends[k]
                    && was.dot(way) < 0.0
                {
                    flipped = true;
                    st.by_limb[k][1] += 1;
                }
                self.bends[k] = Some(way);
            }
        }
        if bad {
            st.hinge_bad += 1;
        }
        if flipped {
            st.pole_flips += 1;
        }
        // Roll jitter of the upper arms and thighs: how fast each rolls about its own length
        // from one frame to the next, and how much that speed changes in a frame (a smooth
        // motion changes it little; a roll fought over by IK and the clip jumps).
        let mut jitter = false;
        for (k, &(p, c, _)) in bones.hinges().iter().enumerate() {
            let Some(parent) = skeleton.parents[p].map(usize::from) else {
                continue;
            };
            let local = (model[parent].r.conjugate() * model[p].r).normalize();
            let along = skeleton.rest[c].t.normalize_or(Vec3::NEG_Y);
            let (was, rate_was) = self.rolls[k];
            let rate = was.map(|w| twist_angle(w.conjugate() * local, along));
            if let (Some(r), Some(r0)) = (rate, rate_was) {
                let accel = (r - r0).abs().to_degrees();
                st.roll_jitter_deg = st.roll_jitter_deg.max(accel);
                if accel > limits.roll_jitter {
                    jitter = true;
                    st.by_limb[k][2] += 1;
                }
            }
            self.rolls[k] = (Some(local), rate);
        }
        if jitter {
            st.roll_jitter += 1;
        }
    }

    /// Check planted feet: `held[n]` says whether foot IK holds that foot still this frame,
    /// `at[n]` is its ankle (world).
    pub fn check_feet(&mut self, held: [bool; 2], at: [DVec3; 2], dt: f32, limits: &Limits) {
        if dt <= 0.0 {
            return;
        }
        let mut slid = false;
        for n in 0..2 {
            if !held[n] {
                self.feet[n] = None;
                continue;
            }
            if let Some(was) = self.feet[n]
                && (at[n] - was).length() < TELEPORT_M
            {
                let d = at[n] - was;
                let speed = ((d.x * d.x + d.z * d.z).sqrt() as f32 / dt) * 100.0;
                self.stats.foot_slide_cms = self.stats.foot_slide_cms.max(speed);
                if speed > limits.slide_cms {
                    slid = true;
                }
            }
            self.feet[n] = Some(at[n]);
        }
        if slid {
            self.stats.foot_slides += 1;
        }
    }

    /// An off hand on a two-handed grip: how far it is from its seat (m) and turned off it.
    pub fn check_grip(&mut self, miss_m: f32, miss_deg: f32) {
        self.stats.grip_miss_m = self.stats.grip_miss_m.max(miss_m);
        self.stats.grip_miss_deg = self.stats.grip_miss_deg.max(miss_deg);
    }

    /// A hand holding something, as [`GripReport`](crate::grip::GripReport) measures it.
    pub fn check_hold(&mut self, report: &crate::grip::GripReport) {
        let st = &mut self.stats;
        st.palm_gap_m = st.palm_gap_m.max(report.palm.abs());
        st.finger_sink_m = st.finger_sink_m.max(-report.deepest);
        st.finger_gap_m = st.finger_gap_m.max(report.widest);
    }
}

/// Turns the validator on: every humanoid gets a [`PoseCheck`], and with `out` set the
/// table of their worst values is written there every `every_secs`.
#[derive(Resource, Debug, Clone)]
pub struct PoseAudit {
    pub limits: Limits,
    pub out: Option<std::path::PathBuf>,
    pub every_secs: f32,
    timer: f32,
}

impl Default for PoseAudit {
    fn default() -> Self {
        Self {
            limits: Limits::default(),
            out: None,
            every_secs: 1.0,
            timer: 0.0,
        }
    }
}

impl PoseAudit {
    /// The audit `ANIM_POSE_AUDIT=<file>` asks for, if set.
    pub fn from_env() -> Option<Self> {
        let path = std::env::var_os("ANIM_POSE_AUDIT")?;
        Some(Self {
            out: Some(path.into()),
            ..Default::default()
        })
    }
}

/// The name a character goes by in the table.
fn label(name: Option<&Name>, e: Entity) -> String {
    match name {
        Some(n) => format!("{n} ({e:?})"),
        None => format!("{e:?}"),
    }
}

/// Every audited character's worst values, and all of them merged (`"all"`, last).
pub fn rows(world: &mut World) -> Vec<(String, PoseStats)> {
    let mut q = world.query::<(Entity, &PoseCheck, Option<&Name>)>();
    let rows: Vec<(String, PoseStats)> = q
        .iter(world)
        .map(|(e, c, n)| (label(n, e), c.stats.clone()))
        .collect();
    with_total(rows)
}

fn with_total(mut rows: Vec<(String, PoseStats)>) -> Vec<(String, PoseStats)> {
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    let mut all = PoseStats::default();
    for (_, r) in &rows {
        all.merge(r);
    }
    rows.push(("all".into(), all));
    rows
}

pub(crate) fn add_pose_checks(
    mut commands: Commands,
    audit: Option<Res<PoseAudit>>,
    q: Query<(Entity, &Animator), Without<PoseCheck>>,
) {
    if audit.is_none() {
        return;
    }
    for (e, animator) in &q {
        if Bones::of(&animator.skeleton).is_some() {
            commands.entity(e).insert(PoseCheck::default());
        }
    }
}

#[allow(clippy::type_complexity)]
pub(crate) fn audit_poses(
    audit: Option<ResMut<PoseAudit>>,
    time: Res<Time>,
    mut q: Query<(
        Entity,
        &Animator,
        &mut PoseCheck,
        &GlobalTransform,
        Option<&crate::ik::FootIk>,
        Option<&crate::ik::ReachIk>,
        Option<&crate::retarget::Retarget>,
        Option<&crate::grip::HeldItems>,
        Option<&Name>,
    )>,
) {
    let Some(mut audit) = audit else {
        return;
    };
    let limits = audit.limits;
    for (_e, animator, mut check, gt, feet, reach, retarget, held, _name) in &mut q {
        if !animator.posed() {
            continue;
        }
        let s = &animator.skeleton;
        check.check(s, &animator.model, &limits);
        if let (Some(feet), Some(bones)) = (feet, Bones::of(s)) {
            let mut held_now = [false; 2];
            let mut at = [DVec3::ZERO; 2];
            for n in 0..2 {
                let l = feet.locks.get(n);
                held_now[n] = feet.enabled && l.is_some_and(|l| l.held && l.blend > 0.99);
                at[n] = crate::ik::to_world(gt, animator.model[bones.foot[n]].t);
            }
            check.check_feet(held_now, at, animator.dt, &limits);
        }
        if let Some(reach) = reach {
            for &(m, deg) in &reach.misses {
                check.check_grip(m, deg);
            }
        }
        if let (Some(r), Some(held)) = (retarget, held) {
            for side in 0..2 {
                if let Some(handle) = &held.hands[side]
                    && let Some(report) =
                        r.retargeter
                            .grip_report(&animator.model, s, &r.model, side, handle)
                {
                    check.check_hold(&report);
                }
            }
        }
    }
    audit.timer += time.delta_secs();
    if audit.timer < audit.every_secs || audit.out.is_none() {
        return;
    }
    audit.timer = 0.0;
    let rows = with_total(
        q.iter()
            .map(|(e, _, c, _, _, _, _, _, n)| (label(n, e), c.stats.clone()))
            .collect(),
    );
    if let Some(path) = &audit.out {
        let _ = std::fs::write(path, table(&rows));
    }
}

/// Put a pose that went NaN back to the rest pose before IK reads it (`AnimSet::Ik`, first),
/// warning once per character: one bad frame upstream (a blow's push, a knockdown off a
/// ledge) must not take the frame, or the game, down with it.
pub(crate) fn heal_poses(
    mut q: Query<(Entity, &mut Animator, Option<&Name>)>,
    mut warned: Local<std::collections::HashSet<Entity>>,
) {
    for (e, mut animator, name) in &mut q {
        let bad = animator
            .model
            .iter()
            .any(|x| !(x.t.is_finite() && x.r.is_finite()));
        if !bad {
            continue;
        }
        if warned.insert(e) {
            tracing::warn!("{}: pose went NaN; reset to rest", label(name, e));
        }
        let a = &mut *animator;
        a.local.clone_from(&a.skeleton.rest);
        a.refresh_model();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::humanoid;

    fn pose() -> (Skeleton, Vec<Xf>) {
        let s = humanoid::skeleton();
        let m = s.rest_model.clone();
        (s, m)
    }

    #[test]
    fn the_rest_pose_passes() {
        let (s, m) = pose();
        let mut c = PoseCheck::default();
        let limits = Limits::default();
        for _ in 0..3 {
            c.check(&s, &m, &limits);
        }
        assert!(c.stats.passes(&limits), "{:?}", c.stats);
        assert!(c.stats.stretch < 1e-4);
    }

    /// Turn `bone` by `q` about its own head (in its rest frame), carrying its children.
    fn turn(s: &Skeleton, m: &mut [Xf], bone: usize, q: Quat) {
        let head = m[bone].t;
        let r = m[bone].r * q * m[bone].r.conjugate();
        let mut stack = vec![bone];
        while let Some(b) = stack.pop() {
            m[b].t = head + r * (m[b].t - head);
            m[b].r = (r * m[b].r).normalize();
            stack.extend(s.children(b).iter().map(|&c| c as usize));
        }
    }

    #[test]
    fn a_wrist_bent_past_its_range_counts() {
        let (s, mut m) = pose();
        let hand = s.index_of("hand_r").unwrap();
        // 100° toward the palm.
        let axis = Vec3::NEG_Y.cross(PALM[1]).normalize();
        turn(
            &s,
            &mut m,
            hand,
            Quat::from_axis_angle(axis, 100f32.to_radians()),
        );
        let mut c = PoseCheck::default();
        c.check(&s, &m, &Limits::default());
        assert_eq!(c.stats.wrist_out, 1);
        assert!(c.stats.wrist_worst.starts_with("flex"), "{:?}", c.stats);
        assert!(
            (c.stats.wrist_worst_deg - 20.0).abs() < 0.5,
            "{:?}",
            c.stats
        );
        // 60° the same way is fine.
        let (s, mut m) = pose();
        turn(
            &s,
            &mut m,
            hand,
            Quat::from_axis_angle(axis, 60f32.to_radians()),
        );
        let mut c = PoseCheck::default();
        c.check(&s, &m, &Limits::default());
        assert_eq!(c.stats.wrist_out, 0, "{:?}", c.stats);
    }

    #[test]
    fn wrist_deviation_and_roll_are_told_apart() {
        for side in 0..2 {
            let (s, mut m) = pose();
            let hand = s.index_of(HAND[side]).unwrap();
            // 50° about the palm's normal (deviation), then 60° of roll about the forearm.
            turn(
                &s,
                &mut m,
                hand,
                Quat::from_axis_angle(PALM[side], 50f32.to_radians()),
            );
            let mut c = PoseCheck::default();
            c.check(&s, &m, &Limits::default());
            assert!(
                (c.stats.wrist_worst_deg - 15.0).abs() < 0.5,
                "{side} {:?}",
                c.stats
            );
            assert!(c.stats.wrist_worst.starts_with("deviate"), "{:?}", c.stats);
            let (s, mut m) = pose();
            turn(
                &s,
                &mut m,
                hand,
                Quat::from_axis_angle(Vec3::Y, 60f32.to_radians()),
            );
            let mut c = PoseCheck::default();
            c.check(&s, &m, &Limits::default());
            assert_eq!(c.stats.wrist_out, 0, "{side} {:?}", c.stats);
        }
    }

    #[test]
    fn a_reversed_knee_and_a_flip_count() {
        let (s, mut m) = pose();
        let shin = s.index_of("lower_leg_l").unwrap();
        let back = m.clone();
        // A knee bends the shin back (+Z): −40° about X.
        turn(&s, &mut m, shin, Quat::from_rotation_x(-40f32.to_radians()));
        let mut c = PoseCheck::default();
        c.check(&s, &m, &Limits::default());
        assert_eq!(c.stats.hinge_bad, 0, "{:?}", c.stats);
        let mut m2 = back.clone();
        turn(&s, &mut m2, shin, Quat::from_rotation_x(40f32.to_radians()));
        let mut c2 = PoseCheck::default();
        c2.check(&s, &m2, &Limits::default());
        assert_eq!(c2.stats.hinge_bad, 1, "{:?}", c2.stats);
        // Going from one to the other in a frame is a flip.
        let mut c3 = PoseCheck::default();
        c3.check(&s, &m, &Limits::default());
        c3.check(&s, &m2, &Limits::default());
        assert_eq!(c3.stats.pole_flips, 1);
    }

    #[test]
    fn a_sliding_planted_foot_counts() {
        let mut c = PoseCheck::default();
        let limits = Limits::default();
        let dt = 1.0 / 60.0;
        c.check_feet([true, true], [DVec3::ZERO, DVec3::X], dt, &limits);
        c.check_feet(
            [true, true],
            [DVec3::new(0.001, 0.0, 0.0), DVec3::X],
            dt,
            &limits,
        );
        assert!((c.stats.foot_slide_cms - 6.0).abs() < 0.01, "{:?}", c.stats);
        assert_eq!(c.stats.foot_slides, 1);
    }
}
