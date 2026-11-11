//! The humanoid's weapon moves: sword and shield, great sword and bow.
//!
//! Authored by the weapon, not by the joints. Each key says where the fist is, which way the
//! held thing points (a blade, a bow, a shield's handle) and which way its edge or face turns,
//! where the elbow points, where the feet stand and how the hips and trunk turn. Between keys
//! a [`Stroke::Cut`] sweeps the blade around one axis, so the tip draws an arc in one plane
//! with the edge leading, its speed peaking about two thirds of the way through (a cut
//! accelerates over most of its arc); the fist circles the chest and reaches out mid-swing.
//! The body eases ahead of the blade on a plain ease, so the hips and chest lead and the blade
//! catches up. Both arms and both legs are solved with two-bone IK when the clip is built:
//! the fists hold the weapon where the key put it, and the feet stay where they were put while
//! the hips turn over them (a foot that moves between keys steps, lifting on the way).
//!
//! Timings follow fighting games and fencing (For Honor lights land in 0.4–0.5 s and heavies
//! in 0.7–0.9 s; great sword cuts take a second; an explosive cut moves from first motion to
//! impact in 0.25–0.35 s; Elden Ring's shield bash lands at 0.87 s of 1.57 s); the poses follow
//! the German longsword guards (vom Tag high over the shoulder, the Mittelhau rolling the
//! blade over so the edge leads) and target archery (the bow arm still, the draw hand anchored
//! under the jaw with the elbow back at shoulder height, the hand carrying on back along the
//! jaw at the release). [`crate::combat`] turns the moves into attack sets.

use std::f32::consts::PI;

use engine_core::math::{Mat3, Quat, Vec3};

use super::humanoid::{CLAVICLE, FOOT, HAND, LOWER_ARM, LOWER_LEG, TOE, UPPER_ARM, UPPER_LEG};
use super::{PoseBuilder, keyed};
use crate::animator::PoseEdit;
use crate::clip::AnimationClip;
use crate::ik::{Limb, rest_hinge, two_bone_live};
use crate::skeleton::Skeleton;
use crate::validate::{wrist_in_range, wrist_strain};
use crate::xform::Xf;

/// The middle of the fist in either hand's frame.
pub const GRIP: Vec3 = Vec3::new(0.0, -0.085, 0.0);

/// A sword in the fist, in the hand's frame: the blade leaves the thumb side of the fist,
/// tipped a little toward the fingers.
pub const BLADE_AXIS: Vec3 = Vec3::new(0.0, -0.3, -0.953_939_2);

/// Which way a held sword's true edge faces, in the hand's frame (out over the knuckles).
pub const BLADE_EDGE: Vec3 = Vec3::new(0.0, -0.953_939_2, 0.3);

/// A shield's handle, or a bow's grip, in the left fist: across the fist out of its thumb side.
pub const HANDLE_AXIS: Vec3 = Vec3::NEG_Z;

/// Which way a held shield's face (or a bow's back) looks, in the left hand's frame: out over
/// the knuckles, so a shield bash is a punch with the shield.
pub const HANDLE_FACE: Vec3 = Vec3::NEG_Y;

/// Where a sword's blade starts and ends along [`BLADE_AXIS`] from the fist, meters.
pub const SWORD_BLADE: [f32; 2] = [0.07, 0.86];

/// Where a great sword's blade starts and ends along [`BLADE_AXIS`] from the right fist.
pub const GREATSWORD_BLADE: [f32; 2] = [0.08, 1.24];

/// The left fist on a great sword's grip, this far down it from the right fist, meters.
pub const GREATSWORD_HANDS: f32 = 0.13;

/// The middle of a shield's face in the left hand's frame: just past the knuckles.
pub const SHIELD_CENTER: Vec3 = Vec3::new(0.0, -0.14, 0.0);

/// A round shield's radius, meters.
pub const SHIELD_RADIUS: f32 = 0.3;

/// The shield punched out into a blade just as it lands, turning it aside (a deflect).
pub const SHIELD_DEFLECT: &str = "shield_deflect";

/// The guard beaten open by a heavy blow: the shield flung out wide, a stagger step back.
pub const GUARD_BROKEN: &str = "guard_broken";

/// A bow's tips: this far up and down its grip axis from the fist, and this far back toward
/// the archer (the string's brace height), meters.
pub const BOW_HALF: f32 = 0.7;
pub const BOW_TIP_BACK: f32 = 0.1;

/// The pivot the fist circles during a cut (the middle of the chest), model space.
const CHEST: Vec3 = Vec3::new(0.0, 1.35, 0.05);

/// Rest height of the ankle above the ground, meters.
const ANKLE: f32 = 0.08;

/// The weapon moves, for the attack sets and tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeaponMove {
    /// Forehand diagonal from over the right shoulder down to the left hip.
    SwordSlash,
    /// Backhand rising from the left hip up past the right shoulder.
    SwordBackhand,
    /// A lunge with the point (a finisher).
    SwordThrust,
    /// One-handed overhead from high over the head, the shield held aside.
    SwordOverhead,
    /// Driving the shield's face forward with a step and the body's weight.
    ShieldBash,
    /// Horizontal cut from right to left, the blade rolling over (the Mittelhau).
    GreatswordSweep,
    /// Rising cut from low left to high right (the Unterhau).
    GreatswordRise,
    /// Overhead from vom Tag (the hilt above the head), with a step: slow and heavy.
    GreatswordOverhead,
    /// Draw, anchor, release and recover.
    BowShot,
    /// The same with a long aim at full draw.
    BowShotFull,
}

impl WeaponMove {
    pub const ALL: [WeaponMove; 10] = [
        WeaponMove::SwordSlash,
        WeaponMove::SwordBackhand,
        WeaponMove::SwordThrust,
        WeaponMove::SwordOverhead,
        WeaponMove::ShieldBash,
        WeaponMove::GreatswordSweep,
        WeaponMove::GreatswordRise,
        WeaponMove::GreatswordOverhead,
        WeaponMove::BowShot,
        WeaponMove::BowShotFull,
    ];

    pub fn clip_name(self) -> &'static str {
        match self {
            WeaponMove::SwordSlash => "sword_slash",
            WeaponMove::SwordBackhand => "sword_backhand",
            WeaponMove::SwordThrust => "sword_thrust",
            WeaponMove::SwordOverhead => "sword_overhead",
            WeaponMove::ShieldBash => "shield_bash",
            WeaponMove::GreatswordSweep => "greatsword_sweep",
            WeaponMove::GreatswordRise => "greatsword_rise",
            WeaponMove::GreatswordOverhead => "greatsword_overhead",
            WeaponMove::BowShot => "bow_shot",
            WeaponMove::BowShotFull => "bow_shot_full",
        }
    }

    /// Length, and the stroke: from the end of the wind-up to the end of the cut (or the
    /// shield's drive, or the bow's release and follow-through), seconds.
    pub fn timing(self) -> (f32, [f32; 2]) {
        match self {
            WeaponMove::SwordSlash => (0.62, [0.17, 0.39]),
            WeaponMove::SwordBackhand => (0.56, [0.16, 0.37]),
            WeaponMove::SwordThrust => (0.72, [0.22, 0.38]),
            WeaponMove::SwordOverhead => (0.95, [0.5, 0.68]),
            WeaponMove::ShieldBash => (0.8, [0.24, 0.4]),
            WeaponMove::GreatswordSweep => (1.05, [0.38, 0.62]),
            WeaponMove::GreatswordRise => (0.95, [0.26, 0.52]),
            WeaponMove::GreatswordOverhead => (1.45, [0.72, 0.94]),
            WeaponMove::BowShot => (1.45, [0.95, 1.0]),
            WeaponMove::BowShotFull => (2.1, [1.55, 1.6]),
        }
    }

    /// Whether it strikes with the left hand (the shield).
    pub fn left_handed(self) -> bool {
        self == WeaponMove::ShieldBash
    }
}

/// Where a hand holds something at a key, model space (the character faces −Z, its right is
/// +X): the fist, which way the held thing points and faces, and which way the elbow points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Hold {
    pub at: Vec3,
    pub axis: Vec3,
    pub face: Vec3,
    pub elbow: Vec3,
}

impl Hold {
    /// `face` is a hint: it is squared to `axis`.
    pub fn new(at: Vec3, axis: Vec3, face: Vec3, elbow: Vec3) -> Self {
        let axis = axis.normalize();
        Self {
            at,
            axis,
            face: square(face, axis),
            elbow: elbow.normalize(),
        }
    }

    fn at(self, at: Vec3) -> Self {
        Self { at, ..self }
    }

    fn frame(&self) -> Quat {
        frame(self.axis, self.face)
    }

    fn lerp(&self, b: &Hold, u: f32) -> Hold {
        let q = self.frame().slerp(b.frame(), u);
        Hold {
            at: self.at.lerp(b.at, u),
            axis: q * Vec3::X,
            face: q * Vec3::Y,
            elbow: self.elbow.lerp(b.elbow, u).normalize_or(b.elbow),
        }
    }
}

/// `v` made square to `axis` (any square direction if it lies along it).
fn square(v: Vec3, axis: Vec3) -> Vec3 {
    (v - axis * axis.dot(v)).normalize_or(axis.any_orthonormal_vector())
}

/// The rotation taking X to `axis` and Y to `face` (square to each other).
fn frame(axis: Vec3, face: Vec3) -> Quat {
    Quat::from_mat3(&Mat3::from_cols(axis, face, axis.cross(face))).normalize()
}

/// The cut axis that swings a blade pointing along `from` toward `through` (the tip leads
/// along axis × blade).
fn plane(from: Vec3, through: Vec3) -> Vec3 {
    from.cross(through).normalize()
}

/// How a hand gets to a key.
#[derive(Clone, Copy, Debug)]
enum Stroke {
    /// Eases to this hold.
    To(Hold),
    /// A cut: the blade turns `degrees` about `axis` from where the last key left it (the tip
    /// leads along axis × blade, and the edge turns to face that way), its speed peaking late;
    /// the fist circles the chest to `to`, `reach` times as far out halfway, and the elbow
    /// eases to `elbow`. `edge` 1 leads with the true edge, −1 with the false (a rising cut
    /// the wrist can't turn over for).
    Cut {
        axis: Vec3,
        degrees: f32,
        to: Vec3,
        reach: f32,
        elbow: Vec3,
        edge: f32,
    },
    /// The left hand on a two-handed grip, this far down it from the right fist.
    Below(f32),
}

/// Where a foot stands: its ankle on flat ground (model space), its toes turned `yaw`
/// degrees to the left of ahead, and its heel up `heel` degrees (rolling over the toes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Foot {
    pub at: Vec3,
    pub yaw: f32,
    pub heel: f32,
}

const fn foot(x: f32, z: f32, yaw: f32) -> Foot {
    Foot {
        at: Vec3::new(x, ANKLE, z),
        yaw,
        heel: 0.0,
    }
}

impl Foot {
    const fn heel(self, heel: f32) -> Foot {
        Foot { heel, ..self }
    }
}

/// The body at a key: the hips moved (meters from rest) and turned (pitch, yaw, roll,
/// degrees; yaw to the left), the trunk turned over them, the feet, and each clavicle's
/// protraction and elevation (degrees). The neck and head take back most of the turn, so the
/// eyes stay level and ahead.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Body {
    pub hips: Vec3,
    pub turn: Vec3,
    pub trunk: Vec3,
    pub feet: [Foot; 2],
    pub shoulders: [[f32; 2]; 2],
}

const fn body(hips: [f32; 3], turn: [f32; 3], trunk: [f32; 3], feet: [Foot; 2]) -> Body {
    Body {
        hips: Vec3::new(hips[0], hips[1], hips[2]),
        turn: Vec3::new(turn[0], turn[1], turn[2]),
        trunk: Vec3::new(trunk[0], trunk[1], trunk[2]),
        feet,
        shoulders: [[0.0; 2]; 2],
    }
}

impl Body {
    fn lerp(&self, b: &Body, u: f32) -> Body {
        let f = |side: usize| {
            let (x, y) = (self.feet[side], b.feet[side]);
            // A foot going somewhere steps: up off the ground on the way.
            let travel = (x.at - y.at).with_y(0.0).length();
            let lift = (travel * 0.35).min(0.09) * (PI * u).sin();
            Foot {
                at: x.at.lerp(y.at, u) + Vec3::Y * lift,
                yaw: x.yaw + (y.yaw - x.yaw) * u,
                heel: x.heel + (y.heel - x.heel) * u,
            }
        };
        let s = |a: [f32; 2], c: [f32; 2]| [a[0] + (c[0] - a[0]) * u, a[1] + (c[1] - a[1]) * u];
        Body {
            hips: self.hips.lerp(b.hips, u),
            turn: self.turn.lerp(b.turn, u),
            trunk: self.trunk.lerp(b.trunk, u),
            feet: [f(0), f(1)],
            shoulders: [
                s(self.shoulders[0], b.shoulders[0]),
                s(self.shoulders[1], b.shoulders[1]),
            ],
        }
    }

    fn hips(self, hips: [f32; 3], turn: [f32; 3], trunk: [f32; 3]) -> Body {
        Body {
            hips: Vec3::from(hips),
            turn: Vec3::from(turn),
            trunk: Vec3::from(trunk),
            ..self
        }
    }

    fn foot(mut self, side: usize, f: Foot) -> Body {
        self.feet[side] = f;
        self
    }

    const fn shoulders(mut self, left: [f32; 2], right: [f32; 2]) -> Body {
        self.shoulders = [left, right];
        self
    }

    /// The same upper body over square hips: the hips' turn moved into the trunk, so the
    /// chest faces the same way when the legs below walk (for a stance layered over
    /// locomotion from the spine up).
    fn squared(self) -> Body {
        Body {
            turn: Vec3::ZERO,
            trunk: self.turn + self.trunk,
            ..self
        }
    }
}

/// What a hand does with what it holds.
#[derive(Clone, Copy, Debug)]
enum Rig {
    /// Holds it: the fist turns so the thing's `axis` and `face` (in the hand's frame) lie
    /// along the hold's.
    Held { axis: Vec3, face: Vec3 },
    /// Reaches the fist there with a straight wrist (a draw hand on a bow string, a hand
    /// hanging free).
    Free,
}

const SWORD: Rig = Rig::Held {
    axis: BLADE_AXIS,
    face: BLADE_EDGE,
};
const HANDLE: Rig = Rig::Held {
    axis: HANDLE_AXIS,
    face: HANDLE_FACE,
};

struct Key {
    t: f32,
    body: Body,
    /// Left, right.
    hands: [Stroke; 2],
}

fn key(t: f32, body: Body, left: Stroke, right: Stroke) -> Key {
    Key {
        t,
        body,
        hands: [left, right],
    }
}

/// Speeding up through most of a cut, peaking about 70% through.
fn swing(u: f32) -> f32 {
    let x = u.clamp(0.0, 1.0).powf(1.5);
    x * x * (3.0 - 2.0 * x)
}

fn smooth(u: f32) -> f32 {
    let u = u.clamp(0.0, 1.0);
    u * u * (3.0 - 2.0 * u)
}

/// A weapon clip's keys, ready to sample.
struct Moves {
    rigs: [Rig; 2],
    keys: Vec<Key>,
    /// Each key's holds, resolved (left, right).
    holds: Vec<[Hold; 2]>,
    /// How much a held thing gives way to wrist strain ([`ArmMemo`]).
    give: f32,
}

impl Moves {
    fn new(rigs: [Rig; 2], keys: Vec<Key>) -> Self {
        assert!(keys.len() >= 2, "a weapon clip needs two keys");
        let blank = Hold::new(Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::NEG_Y);
        let mut holds: Vec<[Hold; 2]> = vec![[blank; 2]; keys.len()];
        for k in 0..keys.len() {
            for side in [1, 0] {
                holds[k][side] = match keys[k].hands[side] {
                    Stroke::To(h) => h,
                    Stroke::Cut {
                        axis,
                        degrees,
                        to,
                        elbow,
                        edge,
                        ..
                    } => {
                        assert!(k > 0, "a weapon clip can't start on a cut");
                        let axis = axis.normalize();
                        // The edge leads: it faces the way the tip moves (the true edge, in
                        // line with the knuckles, so the forearm drives it).
                        let from = &mut holds[k - 1][side];
                        from.face = square(axis.cross(from.axis) * edge, from.axis);
                        let r = Quat::from_axis_angle(axis, degrees.to_radians());
                        Hold {
                            at: to,
                            axis: r * from.axis,
                            face: r * from.face,
                            elbow: elbow.normalize(),
                        }
                    }
                    Stroke::Below(_) => blank,
                };
            }
        }
        for k in 0..keys.len() {
            if let Stroke::Below(d) = keys[k].hands[0] {
                holds[k][0] = below(&holds[k][1], d);
            }
        }
        Self {
            rigs,
            keys,
            holds,
            give: 1.0,
        }
    }

    /// Give way to only `give` of the wrists' strain (0 holds the keys exactly: a paired
    /// thrust; between, a cut that has to stay on its line).
    fn give(self, give: f32) -> Self {
        Self { give, ..self }
    }

    fn secs(&self) -> f32 {
        self.keys.last().map_or(0.0, |k| k.t)
    }

    /// The body and both hands `t` seconds in.
    fn sample(&self, t: f32) -> (Body, [Hold; 2]) {
        let b = self
            .keys
            .iter()
            .position(|k| k.t >= t)
            .unwrap_or(self.keys.len() - 1)
            .max(1);
        let a = b - 1;
        let (ka, kb) = (&self.keys[a], &self.keys[b]);
        let u = ((t - ka.t) / (kb.t - ka.t).max(1e-6)).clamp(0.0, 1.0);
        let body = ka.body.lerp(&kb.body, smooth(u));
        let mut hands = self.holds[a];
        for side in [1, 0] {
            let from = self.holds[a][side];
            hands[side] = match kb.hands[side] {
                Stroke::To(_) => from.lerp(&self.holds[b][side], smooth(u)),
                Stroke::Cut {
                    axis,
                    degrees,
                    to,
                    reach,
                    elbow,
                    ..
                } => {
                    let e = swing(u);
                    let r = Quat::from_axis_angle(axis.normalize(), degrees.to_radians() * e);
                    Hold {
                        at: arc(from.at, to, e, reach),
                        axis: r * from.axis,
                        face: r * from.face,
                        elbow: from
                            .elbow
                            .lerp(elbow.normalize(), smooth(u))
                            .normalize_or(from.elbow),
                    }
                }
                Stroke::Below(d) => below(&hands[1], d),
            };
        }
        (body, hands)
    }

    /// Whether `t` is inside a cut (for the `trail` curve), eased in and out over 30 ms.
    fn cutting(&self, t: f32) -> f32 {
        self.keys
            .windows(2)
            .filter(|w| matches!(w[1].hands[1], Stroke::Cut { .. }))
            .map(|w| {
                let ramp = 0.03;
                ((t - w[0].t + ramp) / ramp)
                    .min((w[1].t + ramp - t) / ramp)
                    .clamp(0.0, 1.0)
            })
            .fold(0.0, f32::max)
    }

    /// The pose `t` seconds in.
    fn pose(&self, s: &Skeleton, t: f32, memo: &mut ArmMemo) -> Vec<(&'static str, Xf)> {
        let (body, hands) = self.sample(t);
        solve(s, self.rigs, &body, &hands, memo)
    }

    /// The clip, sampled 120 times a second (a cut turns the blade 20° in a 60th).
    fn clip(self, s: &Skeleton, name: &str, looping: bool) -> AnimationClip {
        self.clip_reaching(s, name, looping).0
    }

    /// [`Moves::clip`], and the farthest a held weapon's fist fell short of its hold, m.
    fn clip_reaching(self, s: &Skeleton, name: &str, looping: bool) -> (AnimationClip, f32) {
        let secs = self.secs();
        let keys = (secs * 120.0).ceil() as usize + 1;
        let trail_keys = keys;
        let trail: Vec<f32> = (0..trail_keys)
            .map(|k| self.cutting(k as f32 / (trail_keys - 1) as f32 * secs))
            .collect();
        let mut memo = ArmMemo {
            give: self.give,
            ..Default::default()
        };
        let clip = AnimationClip::from_fn(name, secs, keys, looping, |ph| {
            #[cfg(test)]
            tests::sampling(name, ph * secs);
            self.pose(s, ph * secs, &mut memo)
        });
        let clip = clip.with_curve("trail", trail_keys, move |ph| {
            trail[((ph * (trail_keys - 1) as f32).round() as usize).min(trail_keys - 1)]
        });
        (clip, memo.short)
    }
}

/// The left hand `d` down the grip from the right hand's hold, its elbow mirrored.
fn below(main: &Hold, d: f32) -> Hold {
    Hold {
        at: main.at - main.axis * d,
        axis: main.axis,
        face: main.face,
        elbow: Vec3::new(-main.elbow.x, main.elbow.y, main.elbow.z).normalize_or(Vec3::NEG_Y),
    }
}

/// The fist's path through a cut, `e` of the way: round the chest from `from` to `to`,
/// reaching `reach` times as far out halfway.
fn arc(from: Vec3, to: Vec3, e: f32, reach: f32) -> Vec3 {
    let (a, b) = (from - CHEST, to - CHEST);
    let (an, bn) = (a.normalize_or(Vec3::NEG_Z), b.normalize_or(Vec3::NEG_Z));
    let dir = Quat::IDENTITY.slerp(Quat::from_rotation_arc(an, bn), e) * an;
    let radius =
        (a.length() + (b.length() - a.length()) * e) * (1.0 + (reach - 1.0) * (PI * e).sin());
    CHEST + dir * radius
}

/// Which way to bend an elbow so the forearm runs into the hand (`r`, model rotation) at
/// `wrist`, the wrist straight rather than broken sideways: toward where the elbow would be
/// on the hand's line, behind the wrist, blended with the hold's own `elbow` hint (which way
/// the elbow points when that line leaves a choice). Model space.
fn wrist_pole(
    edit: &PoseEdit,
    upper: usize,
    lower: usize,
    hand: usize,
    wrist: Vec3,
    r: Quat,
    elbow: Vec3,
) -> Vec3 {
    let shoulder = edit.position(upper);
    let forearm = edit.position(hand).distance(edit.position(lower));
    // The hand points down its −Y: the forearm in line with it ends a forearm's length
    // back up its +Y.
    let ideal = wrist + r * Vec3::Y * forearm;
    let dir = (wrist - shoulder).normalize_or(Vec3::NEG_Y);
    let across = ideal - shoulder;
    let across = across - dir * dir.dot(across);
    match across.try_normalize() {
        Some(a) => (a * 0.35 + elbow.normalize_or_zero() * 0.65).normalize_or(elbow),
        None => elbow,
    }
}

/// A shield held by `rig` at `hold` (its centre and face normal, model space), if `rig`
/// holds one.
fn shield_of(rig: Rig, hold: &Hold) -> Option<(Vec3, Vec3)> {
    match rig {
        Rig::Held { axis, face } if axis == HANDLE_AXIS && face == HANDLE_FACE => {
            let r = hold.frame() * frame(axis, face).inverse();
            let origin = hold.at - r * GRIP;
            Some((origin + r * SHIELD_CENTER, (r * HANDLE_FACE).normalize()))
        }
        _ => None,
    }
}

/// How deep the blade from `a` to `b` cuts into a shield at `centre` facing `normal`
/// (meters inside its rim where it crosses the shield's face, or lies within 2 cm of it:
/// negative when it passes outside the rim, −∞ when it never comes near the face).
fn blade_through_shield(a: Vec3, b: Vec3, centre: Vec3, normal: Vec3) -> f32 {
    let (da, db) = ((a - centre).dot(normal), (b - centre).dot(normal));
    let depth = |p: Vec3| {
        let d = (p - centre).dot(normal);
        SHIELD_RADIUS - (p - centre - normal * d).length()
    };
    let mut worst = f32::NEG_INFINITY;
    if da.signum() != db.signum() {
        worst = worst.max(depth(a.lerp(b, da / (da - db))));
    }
    for k in 0..=20 {
        let u = k as f32 / 20.0;
        if (da + (db - da) * u).abs() < 0.02 {
            worst = worst.max(depth(a.lerp(b, u)));
        }
    }
    worst
}

/// How each arm (left, right) reached its hold at the last sample: the elbow turned this many
/// degrees round the shoulder–wrist line from [`wrist_pole`]'s choice, and the fist this many
/// round the grip's axis from the hold's frame (an off hand on a two-handed grip only).
/// Samples carry it on so the search for the least strained wrist moves the arm smoothly.
/// `give` is how much of a wrist's strain a held thing gives way to (1 all of it, 0 none: a
/// thrust that has to go into a body where a paired clip puts it).
#[derive(Clone, Copy, Debug)]
struct ArmMemo {
    arms: [Option<(f32, f32)>; 2],
    give: f32,
    /// The farthest a held weapon's fist fell short of its hold so far, m.
    short: f32,
}

impl Default for ArmMemo {
    fn default() -> Self {
        Self {
            arms: [None; 2],
            give: 1.0,
            short: 0.0,
        }
    }
}

/// How far either way the first sample of a clip searches round the shoulder–wrist line, and
/// round a two-handed grip, degrees, and in what steps.
const SWIVEL_SEARCH: (f32, f32) = (90.0, 10.0);
const SPIN_SEARCH: (f32, f32) = (120.0, 15.0);
/// Later samples search this far either way of the last, in these steps.
const FOLLOW_SEARCH: (f32, f32) = (6.0, 2.0);
/// An off hand on a grip moves no blade: its turn round the grip and its elbow may follow
/// faster.
const FOLLOW_SPIN: (f32, f32) = (15.0, 3.0);
/// Strain-degrees a degree of swivel or spin away from the plain reach costs: a strained
/// wrist is worth fixing, an arm turned out of the way for nothing is not.
const TURN_COST: f32 = 0.04;
/// How far clear of the bearer's shield rim a blade the solver turns stays, meters.
const SHIELD_CLEARANCE: f32 = 0.03;
/// How far inside each wrist limit a hand the solver turns back is left, degrees.
const WRIST_MARGIN: f32 = 2.0;

/// Angles from `-reach` to `reach` about `centre`, `step` apart.
fn around(centre: f32, (reach, step): (f32, f32)) -> impl Iterator<Item = f32> {
    let n = (reach / step).round() as i32;
    (-n..=n).map(move |k| centre + k as f32 * step)
}

/// Pose the skeleton: the body by its angles, then legs and arms by two-bone IK onto the feet
/// and holds.
fn solve(
    s: &Skeleton,
    rigs: [Rig; 2],
    body: &Body,
    hands: &[Hold; 2],
    memo: &mut ArmMemo,
) -> Vec<(&'static str, Xf)> {
    let mut hands = *hands;
    let mut p = PoseBuilder::new(s);
    let (turn, trunk) = (body.turn, body.trunk);
    p.rot_move("hips", body.hips, turn.x, turn.y, turn.z)
        .rot("spine", trunk.x * 0.45, trunk.y * 0.45, trunk.z * 0.45)
        .rot("chest", trunk.x * 0.55, trunk.y * 0.55, trunk.z * 0.55);
    let all = turn + trunk;
    p.rot("neck", -all.x * 0.35, -all.y * 0.4, -all.z * 0.4)
        .rot("head", -all.x * 0.35, -all.y * 0.45, -all.z * 0.45);
    for side in 0..2 {
        let sign = [-1.0, 1.0][side];
        let [protract, elevate] = body.shoulders[side];
        p.rot(CLAVICLE[side], 0.0, protract * sign, elevate * sign);
    }
    let mut out = p.take();
    let mut local = s.rest.clone();
    for (bone, x) in &out {
        let i = s.bone(bone);
        local[i].r = x.r;
        if x.t != Vec3::ZERO {
            local[i].t = x.t;
        }
    }
    let mut model = s.rest_model.clone();
    s.to_model(&local, &mut model);
    let mut edit = PoseEdit {
        skeleton: s,
        local: &mut local,
        model: &mut model,
    };
    // Legs: the ankle where the foot puts it (rolled up over the toes for a raised heel), the
    // knee over the toes.
    for side in 0..2 {
        let f = body.feet[side];
        let (upper, lower, ankle, toe) = (
            s.bone(UPPER_LEG[side]),
            s.bone(LOWER_LEG[side]),
            s.bone(FOOT[side]),
            s.bone(TOE[side]),
        );
        let yaw = Quat::from_rotation_y(f.yaw.to_radians());
        let tilt = yaw * Quat::from_rotation_x(-f.heel.to_radians());
        let toes = s.rest[toe].t;
        let joint = f.at + yaw * toes;
        let target = joint - tilt * toes;
        let out = [-0.12, 0.12][side];
        let pole = yaw * Vec3::new(out, 0.0, -1.0);
        let hinge = rest_hinge(s, upper, lower, Vec3::NEG_Z);
        two_bone_live(
            &mut edit,
            upper,
            lower,
            ankle,
            target,
            Limb::posed(pole, hinge),
        );
        edit.set_model_rotation(ankle, tilt);
        edit.set_model_rotation(toe, yaw);
    }
    // Arms: the right first (it holds the weapon).
    for side in [1, 0] {
        let h = hands[side];
        let (upper, lower, hand) = (
            s.bone(UPPER_ARM[side]),
            s.bone(LOWER_ARM[side]),
            s.bone(HAND[side]),
        );
        match rigs[side] {
            Rig::Held { axis, face } => {
                let held = h.frame() * frame(axis, face).inverse();
                let hinge = rest_hinge(s, upper, lower, Vec3::Z);
                // An off hand on the main hand's grip may wrap it from its own side: turned
                // about the grip's axis, the fist staying on it.
                let two_handed = side == 0
                    && matches!(rigs[1], Rig::Held { .. })
                    && h.axis.dot(hands[1].axis) > 0.999
                    && (h.at - hands[1].at).cross(h.axis).length() < 0.01;
                // The elbow turned round the shoulder–wrist line, and the fist round the grip,
                // where the wrist strains least: searched widely at a clip's first sample,
                // then near the last one's.
                let (swivels, spins): (Vec<f32>, Vec<f32>) = match memo.arms[side] {
                    None => (
                        around(0.0, SWIVEL_SEARCH).collect(),
                        if two_handed {
                            around(0.0, SPIN_SEARCH).collect()
                        } else {
                            vec![0.0]
                        },
                    ),
                    Some((swivel, spin)) => (
                        around(
                            swivel,
                            if two_handed {
                                FOLLOW_SPIN
                            } else {
                                FOLLOW_SEARCH
                            },
                        )
                        .collect(),
                        if two_handed {
                            around(spin, FOLLOW_SPIN).collect()
                        } else {
                            vec![0.0]
                        },
                    ),
                };
                let shoulder = edit.position(upper);
                type Trial = (f32, f32, f32, f32, Vec3, Vec<Xf>, Vec<Xf>);
                let mut best: Option<Trial> = None;
                for &spin in &spins {
                    let r = Quat::from_axis_angle(h.axis, spin.to_radians()) * held;
                    let wrist = h.at - r * GRIP;
                    let plain = wrist_pole(&edit, upper, lower, hand, wrist, r, h.elbow);
                    let line = (wrist - shoulder).normalize_or(Vec3::NEG_Y);
                    for &swivel in &swivels {
                        let pole = Quat::from_axis_angle(line, swivel.to_radians()) * plain;
                        let mut local = edit.local.to_vec();
                        let mut model = edit.model.to_vec();
                        let mut trial = PoseEdit {
                            skeleton: s,
                            local: &mut local,
                            model: &mut model,
                        };
                        let miss = two_bone_live(
                            &mut trial,
                            upper,
                            lower,
                            hand,
                            wrist,
                            Limb::posed(pole, hinge),
                        );
                        trial.set_model_rotation(hand, r);
                        let cost = wrist_strain(s, &model, lower, hand, side)
                            + (swivel.abs() + spin.abs()) * TURN_COST
                            + miss * 1000.0;
                        if best.as_ref().is_none_or(|b| cost < b.0) {
                            best = Some((cost, swivel, spin, miss, pole, local, model));
                        }
                    }
                }
                let (_, swivel, spin, miss, pole, local, model) = best.expect("a reach to try");
                memo.short = memo.short.max(miss);
                edit.local.copy_from_slice(&local);
                edit.model.copy_from_slice(&model);
                memo.arms[side] = Some((swivel, spin));
                // What strain is left, a held thing gives way to: the wrist turned back into
                // its range, the thing with it, the fist kept where the key holds it (a key
                // asking past what a wrist can do gets the nearest it can). Not past the
                // bearer's own shield, though, and not an off hand, which keeps to the grip.
                let shield = (side == 1).then(|| shield_of(rigs[0], &hands[0])).flatten();
                if !two_handed
                    && memo.give > 0.0
                    && wrist_strain(s, edit.model, lower, hand, side) > 0.0
                {
                    let asked = edit.model[hand].r;
                    for _ in 0..3 {
                        let eased = wrist_in_range(s, edit.model, lower, hand, side, WRIST_MARGIN);
                        let clear = |u: f32| {
                            let r = asked.slerp(eased, u);
                            shield.is_none_or(|(centre, normal)| {
                                let axis = r * axis;
                                let a = h.at + axis * SWORD_BLADE[0];
                                let b = h.at + axis * SWORD_BLADE[1];
                                blade_through_shield(a, b, centre, normal) <= -SHIELD_CLEARANCE
                            })
                        };
                        let u = if clear(1.0) {
                            1.0
                        } else {
                            let (mut lo, mut hi) = (0.0, 1.0);
                            for _ in 0..8 {
                                let mid = (lo + hi) * 0.5;
                                if clear(mid) { lo = mid } else { hi = mid }
                            }
                            lo
                        };
                        let r = asked.slerp(eased, u * memo.give);
                        two_bone_live(
                            &mut edit,
                            upper,
                            lower,
                            hand,
                            h.at - r * GRIP,
                            Limb::posed(pole, hinge),
                        );
                        edit.set_model_rotation(hand, r);
                    }
                    // An off hand on this grip goes where the grip went.
                    if side == 1
                        && let Rig::Held { axis, face } = rigs[1]
                        && matches!(rigs[0], Rig::Held { .. })
                        && hands[0].axis.dot(hands[1].axis) > 0.999
                        && (hands[0].at - hands[1].at).cross(hands[0].axis).length() < 0.01
                    {
                        let r = edit.model[hand].r;
                        let down = (hands[1].at - hands[0].at).dot(hands[1].axis);
                        let main = Hold {
                            at: edit.model[hand].transform_point(GRIP),
                            axis: r * axis,
                            face: r * face,
                            elbow: hands[1].elbow,
                        };
                        hands[0] = Hold {
                            elbow: hands[0].elbow,
                            ..below(&main, down)
                        };
                    }
                }
                #[cfg(test)]
                tests::note_miss(side, miss);
            }
            Rig::Free => {
                // The fist is a hand's length past the wrist along a straight forearm.
                let mut wrist = h.at;
                for _ in 0..2 {
                    let hinge = rest_hinge(s, upper, lower, Vec3::Z);
                    two_bone_live(
                        &mut edit,
                        upper,
                        lower,
                        hand,
                        wrist,
                        Limb::posed(h.elbow, hinge),
                    );
                    let along =
                        (edit.position(hand) - edit.position(lower)).normalize_or(Vec3::NEG_Y);
                    wrist = h.at - along * -GRIP.y;
                }
            }
        }
    }
    let solved = UPPER_ARM
        .into_iter()
        .chain(LOWER_ARM)
        .chain(HAND)
        .chain(UPPER_LEG)
        .chain(LOWER_LEG)
        .chain(FOOT)
        .chain(TOE);
    for bone in solved {
        out.retain(|(b, _)| *b != bone);
        out.push((
            bone,
            Xf {
                t: Vec3::ZERO,
                r: edit.local[s.bone(bone)].r,
                s: Vec3::ONE,
            },
        ));
    }
    out
}

// -------------------------------------------------------------------------- sword and shield

/// Sword-and-shield stance: the left (shield) foot ahead, the right turned out behind, knees
/// soft, hips turned a little to the right and the chest back toward the foe.
const STANCE: Body = body(
    [0.0, -0.07, 0.0],
    [4.0, -18.0, 0.0],
    [3.0, 8.0, 0.0],
    [foot(-0.13, -0.24, 10.0), foot(0.17, 0.24, -35.0)],
);

/// Standing square, feet under the hips: the body weapon carries are solved on, so their arm
/// poses suit any upright trunk they are layered over.
const SQUARE: Body = body(
    [0.0, -0.01, 0.0],
    [0.0, 0.0, 0.0],
    [0.0, 0.0, 0.0],
    [foot(-0.1, 0.0, 6.0), foot(0.1, 0.0, -6.0)],
);

/// The sword ready in front, its point up at the foe's face, the edge down.
fn sword_ready() -> Hold {
    Hold::new(
        Vec3::new(0.24, 1.05, -0.33),
        Vec3::new(-0.15, 0.5, -0.85),
        Vec3::NEG_Y,
        Vec3::new(0.4, -1.0, 0.3),
    )
}

/// The shield held low in front, its face to the foe, the handle across the fist.
fn shield_low() -> Hold {
    Hold::new(
        Vec3::new(-0.13, 1.1, -0.36),
        Vec3::new(1.0, 0.1, 0.0),
        Vec3::new(0.0, 0.05, -1.0),
        Vec3::new(-1.0, -0.6, 0.2),
    )
}

/// The shield up, covering the chest and chin.
fn shield_up() -> Hold {
    Hold::new(
        Vec3::new(-0.05, 1.33, -0.4),
        Vec3::new(1.0, 0.15, 0.0),
        Vec3::new(0.05, 0.1, -1.0),
        Vec3::new(-1.0, -0.25, 0.3),
    )
}

fn sword_slash(s: &Skeleton) -> AnimationClip {
    let (secs, [wound, cut]) = WeaponMove::SwordSlash.timing();
    let back = Vec3::new(0.3, 0.55, 0.78);
    Moves::new(
        [HANDLE, SWORD],
        vec![
            key(
                0.0,
                STANCE,
                Stroke::To(shield_low()),
                Stroke::To(sword_ready()),
            ),
            // Wound: the blade back over the right shoulder, the hips and chest turned away.
            key(
                wound,
                STANCE.hips([0.0, -0.08, 0.04], [2.0, -34.0, 0.0], [0.0, -16.0, 0.0]),
                Stroke::To(shield_low().at(Vec3::new(-0.1, 1.12, -0.33))),
                Stroke::To(Hold::new(
                    Vec3::new(0.3, 1.5, -0.02),
                    back,
                    Vec3::NEG_Z,
                    Vec3::new(1.0, 0.5, 0.3),
                )),
            ),
            // Down through the foe from the right shoulder to the left hip, the hips and
            // chest turning through it and the rear heel coming up; the shield draws aside.
            key(
                cut,
                STANCE
                    .hips([0.0, -0.1, -0.08], [6.0, 12.0, 0.0], [6.0, 18.0, -4.0])
                    .foot(1, STANCE.feet[1].heel(15.0)),
                Stroke::To(shield_low().at(Vec3::new(-0.36, 1.2, -0.14))),
                Stroke::Cut {
                    axis: Vec3::new(-0.85, 0.5, 0.0),
                    degrees: 215.0,
                    to: Vec3::new(-0.12, 1.02, -0.36),
                    reach: 1.12,
                    elbow: Vec3::new(0.5, -0.7, -0.5),
                    edge: 1.0,
                },
            ),
            key(
                secs,
                STANCE,
                Stroke::To(shield_low()),
                Stroke::To(sword_ready()),
            ),
        ],
    )
    .clip(s, WeaponMove::SwordSlash.clip_name(), false)
    .with_curve("effort", 31, move |ph| {
        keyed(
            ph * secs,
            &[(0.0, 0.15), (wound, 0.5), (cut - 0.08, 1.0), (secs, 0.15)],
        )
    })
    .with_event(wound / secs, "swing", Some("hand_r"))
}

fn sword_backhand(s: &Skeleton) -> AnimationClip {
    let (secs, [wound, cut]) = WeaponMove::SwordBackhand.timing();
    // Back, down and to the left: in the plane of a cut rising from the left hip.
    let low = Vec3::new(-0.25, -0.433, 0.866);
    Moves::new(
        [HANDLE, SWORD],
        vec![
            key(
                0.0,
                STANCE,
                Stroke::To(shield_low()),
                Stroke::To(sword_ready()),
            ),
            // The point lifts and the shield drops aside first, so the blade goes over and
            // behind it to the hip instead of through it.
            key(
                wound * 0.5,
                STANCE.hips([0.0, -0.09, 0.02], [3.0, 2.0, 0.0], [2.0, 12.0, 0.0]),
                Stroke::To(shield_low().at(Vec3::new(-0.34, 1.02, -0.3))),
                Stroke::To(Hold::new(
                    Vec3::new(0.06, 1.28, -0.24),
                    Vec3::new(-0.15, 0.95, 0.25),
                    Vec3::NEG_Z,
                    Vec3::new(0.8, -0.4, -0.4),
                )),
            ),
            // The sword across to the left hip, the trunk coiled to the left, the shield out
            // of its way.
            key(
                wound,
                STANCE.hips([0.0, -0.09, 0.02], [3.0, 20.0, 0.0], [2.0, 22.0, 0.0]),
                Stroke::To(shield_low().at(Vec3::new(-0.4, 1.24, -0.18))),
                Stroke::To(Hold::new(
                    Vec3::new(-0.22, 1.02, -0.22),
                    low,
                    Vec3::NEG_Z,
                    Vec3::new(0.5, -0.2, -0.9),
                )),
            ),
            key(
                cut,
                STANCE
                    .hips([0.0, -0.08, -0.06], [4.0, -26.0, 0.0], [4.0, -18.0, 3.0])
                    .foot(1, STANCE.feet[1].heel(8.0)),
                // The shield lifts out to the left while the blade rises under it.
                Stroke::To(shield_low().at(Vec3::new(-0.42, 1.27, -0.18))),
                Stroke::Cut {
                    axis: Vec3::new(0.85, -0.5, 0.0),
                    degrees: 210.0,
                    to: Vec3::new(0.36, 1.5, -0.3),
                    reach: 1.1,
                    elbow: Vec3::new(1.0, -0.2, 0.1),
                    edge: 1.0,
                },
            ),
            key(
                secs,
                STANCE,
                Stroke::To(shield_low()),
                Stroke::To(sword_ready()),
            ),
        ],
    )
    .clip(s, WeaponMove::SwordBackhand.clip_name(), false)
    .with_curve("effort", 31, move |ph| {
        keyed(
            ph * secs,
            &[(0.0, 0.15), (wound, 0.5), (cut - 0.07, 1.0), (secs, 0.15)],
        )
    })
    .with_event(wound / secs, "swing", Some("hand_r"))
}

fn sword_thrust(s: &Skeleton) -> AnimationClip {
    let (secs, [chamber, out]) = WeaponMove::SwordThrust.timing();
    let point = Vec3::new(-0.04, 0.04, -1.0);
    let lunge = STANCE
        .hips([0.0, -0.14, -0.2], [6.0, 4.0, 0.0], [10.0, 6.0, 0.0])
        .foot(0, foot(-0.13, -0.52, 10.0))
        .foot(1, STANCE.feet[1].heel(10.0));
    let extended = Hold::new(
        Vec3::new(0.06, 1.3, -0.68),
        point,
        Vec3::NEG_Y,
        Vec3::new(0.7, -0.7, 0.1),
    );
    Moves::new(
        [HANDLE, SWORD],
        vec![
            key(
                0.0,
                STANCE,
                Stroke::To(shield_low()),
                Stroke::To(sword_ready()),
            ),
            // Chambered by the right hip, point at the foe, the shield out in front.
            key(
                chamber,
                STANCE.hips([0.0, -0.1, 0.06], [2.0, -30.0, 0.0], [0.0, -12.0, 0.0]),
                Stroke::To(shield_low().at(Vec3::new(-0.08, 1.15, -0.38))),
                Stroke::To(Hold::new(
                    Vec3::new(0.24, 1.2, 0.06),
                    point,
                    Vec3::NEG_Y,
                    Vec3::new(0.6, -0.6, 0.6),
                )),
            ),
            // The lunge: the front foot steps out, the arm drives the point through, the
            // shield pulls back against it.
            key(
                out,
                lunge,
                Stroke::To(shield_low().at(Vec3::new(-0.2, 1.12, -0.3))),
                Stroke::To(extended),
            ),
            key(
                out + 0.08,
                lunge,
                Stroke::To(shield_low().at(Vec3::new(-0.2, 1.12, -0.3))),
                Stroke::To(extended.at(extended.at + Vec3::new(0.0, -0.01, 0.02))),
            ),
            key(
                secs,
                STANCE,
                Stroke::To(shield_low()),
                Stroke::To(sword_ready()),
            ),
        ],
    )
    .clip(s, WeaponMove::SwordThrust.clip_name(), false)
    .with_curve("effort", 31, move |ph| {
        keyed(
            ph * secs,
            &[(0.0, 0.15), (chamber, 0.6), (out, 1.0), (secs, 0.15)],
        )
    })
    .with_event(chamber / secs, "swing", Some("hand_r"))
}

fn sword_overhead(s: &Skeleton) -> AnimationClip {
    let (secs, [top, cut]) = WeaponMove::SwordOverhead.timing();
    let raised = STANCE.hips([0.0, -0.05, 0.05], [0.0, -22.0, 0.0], [-4.0, -8.0, 0.0]);
    let high = Hold::new(
        Vec3::new(0.14, 1.78, 0.0),
        Vec3::new(0.05, 0.62, 0.78),
        Vec3::NEG_Z,
        Vec3::new(1.0, 0.6, 0.2),
    );
    let aside = shield_low().at(Vec3::new(-0.14, 1.2, -0.36));
    Moves::new(
        [HANDLE, SWORD],
        vec![
            key(
                0.0,
                STANCE,
                Stroke::To(shield_low()),
                Stroke::To(sword_ready()),
            ),
            key(top - 0.08, raised, Stroke::To(aside), Stroke::To(high)),
            // A beat at the top.
            key(
                top,
                raised,
                Stroke::To(aside),
                Stroke::To(high.at(high.at + Vec3::new(0.0, 0.02, 0.02))),
            ),
            // Straight down through the head, dropping the weight onto the front foot as it
            // steps in.
            key(
                cut,
                STANCE
                    .hips([0.0, -0.16, -0.12], [8.0, 6.0, 0.0], [14.0, 8.0, 0.0])
                    .foot(0, foot(-0.13, -0.42, 10.0))
                    .foot(1, STANCE.feet[1].heel(12.0)),
                Stroke::To(shield_low().at(Vec3::new(-0.2, 1.08, -0.3))),
                Stroke::Cut {
                    axis: Vec3::new(-1.0, 0.08, 0.0),
                    degrees: 205.0,
                    to: Vec3::new(0.06, 1.02, -0.42),
                    reach: 1.05,
                    elbow: Vec3::new(0.6, -0.6, -0.4),
                    edge: 1.0,
                },
            ),
            key(
                secs,
                STANCE,
                Stroke::To(shield_low()),
                Stroke::To(sword_ready()),
            ),
        ],
    )
    .clip(s, WeaponMove::SwordOverhead.clip_name(), false)
    .with_curve("effort", 31, move |ph| {
        keyed(
            ph * secs,
            &[(0.0, 0.15), (top, 0.6), (cut - 0.06, 1.0), (secs, 0.15)],
        )
    })
    .with_event(top / secs, "swing", Some("hand_r"))
}

fn shield_bash(s: &Skeleton) -> AnimationClip {
    let (secs, [coiled, out]) = WeaponMove::ShieldBash.timing();
    let driven = STANCE
        .hips([0.0, -0.12, -0.22], [6.0, -30.0, 0.0], [8.0, -14.0, 0.0])
        .foot(0, foot(-0.13, -0.5, 10.0))
        .foot(1, STANCE.feet[1].heel(12.0))
        .shoulders([18.0, 4.0], [0.0, 0.0]);
    let bash = Hold::new(
        Vec3::new(-0.02, 1.3, -0.72),
        Vec3::new(1.0, 0.1, 0.0),
        Vec3::new(0.05, 0.05, -1.0),
        Vec3::new(-1.0, -0.2, 0.2),
    );
    // The sword drawn back by the right hip, point up, clear of the shield's back.
    let sword_back = Hold::new(
        Vec3::new(0.32, 1.08, -0.04),
        Vec3::new(0.05, 0.92, 0.38),
        Vec3::NEG_Z,
        Vec3::new(0.6, -0.7, 0.4),
    );
    Moves::new(
        [HANDLE, SWORD],
        vec![
            key(
                0.0,
                STANCE,
                Stroke::To(shield_low()),
                Stroke::To(sword_ready()),
            ),
            // Braced: the shield drawn in close, the left shoulder back.
            key(
                coiled,
                STANCE.hips([0.0, -0.1, 0.05], [2.0, -8.0, 0.0], [0.0, 12.0, 0.0]),
                Stroke::To(Hold::new(
                    Vec3::new(-0.14, 1.22, -0.2),
                    Vec3::new(1.0, 0.1, 0.0),
                    Vec3::new(0.1, 0.05, -1.0),
                    Vec3::new(-1.0, -0.4, 0.4),
                )),
                Stroke::To(sword_ready().at(Vec3::new(0.26, 1.08, -0.15))),
            ),
            // Rammed out with a step and the body behind it.
            key(out, driven, Stroke::To(bash), Stroke::To(sword_back)),
            key(
                out + 0.1,
                driven,
                Stroke::To(bash.at(bash.at + Vec3::new(0.0, -0.01, 0.03))),
                Stroke::To(sword_back),
            ),
            key(
                secs,
                STANCE,
                Stroke::To(shield_low()),
                Stroke::To(sword_ready()),
            ),
        ],
    )
    .clip(s, WeaponMove::ShieldBash.clip_name(), false)
    .with_curve("effort", 31, move |ph| {
        keyed(
            ph * secs,
            &[(0.0, 0.15), (coiled, 0.6), (out, 1.0), (secs, 0.15)],
        )
    })
    .with_event(coiled / secs, "swing", Some("hand_l"))
}

/// Holding the sword ready and the shield low, swaying a little (a loop).
fn sword_stance(s: &Skeleton) -> AnimationClip {
    let settle = STANCE.hips([0.0, -0.085, 0.01], [5.0, -19.0, 0.0], [4.0, 9.0, 0.0]);
    Moves::new(
        [HANDLE, SWORD],
        vec![
            key(
                0.0,
                STANCE,
                Stroke::To(shield_low()),
                Stroke::To(sword_ready()),
            ),
            key(
                0.8,
                settle,
                Stroke::To(shield_low().at(shield_low().at - Vec3::Y * 0.015)),
                Stroke::To(sword_ready().at(sword_ready().at - Vec3::Y * 0.012)),
            ),
            key(
                1.6,
                STANCE,
                Stroke::To(shield_low()),
                Stroke::To(sword_ready()),
            ),
        ],
    )
    .clip(s, "sword_stance", true)
    .with_curve("effort", 2, |_| 0.15)
}

/// The shield up over the chest and chin, the sword back ready to answer (a loop).
fn shield_guard(s: &Skeleton) -> AnimationClip {
    let braced = STANCE.hips([0.0, -0.1, 0.02], [6.0, -20.0, 0.0], [5.0, 6.0, 0.0]);
    let sword = guard_sword();
    Moves::new(
        [HANDLE, SWORD],
        vec![
            key(0.0, braced, Stroke::To(shield_up()), Stroke::To(sword)),
            key(
                0.8,
                braced.hips([0.0, -0.11, 0.02], [6.0, -21.0, 0.0], [6.0, 7.0, 0.0]),
                Stroke::To(shield_up().at(shield_up().at + Vec3::new(0.0, -0.01, 0.01))),
                Stroke::To(sword.at(sword.at - Vec3::Y * 0.01)),
            ),
            key(1.6, braced, Stroke::To(shield_up()), Stroke::To(sword)),
        ],
    )
    .clip(s, "shield_guard", true)
    .with_curve("effort", 2, |_| 0.35)
}

/// The sword held back over the guard, ready to answer (as in `shield_guard`).
fn guard_sword() -> Hold {
    Hold::new(
        Vec3::new(0.3, 1.2, -0.1),
        Vec3::new(0.05, 0.85, -0.5),
        Vec3::NEG_Z,
        Vec3::new(0.8, -0.8, 0.4),
    )
}

/// A deflect: from the guard the shield is punched out a hand's breadth into the blade and
/// turned outward, swatting it aside with the trunk turning into it, the sword drawn back
/// to answer; then back to the guard.
fn shield_deflect(s: &Skeleton) -> AnimationClip {
    let braced = STANCE.hips([0.0, -0.1, 0.02], [6.0, -20.0, 0.0], [5.0, 6.0, 0.0]);
    let swat = STANCE
        .hips([0.0, -0.11, -0.05], [8.0, -14.0, 0.0], [7.0, 14.0, 0.0])
        .shoulders([14.0, 4.0], [0.0, 0.0]);
    let out = Hold::new(
        Vec3::new(-0.2, 1.38, -0.52),
        Vec3::new(0.85, 0.15, -0.5),
        Vec3::new(-0.5, 0.1, -0.86),
        Vec3::new(-1.0, -0.3, 0.2),
    );
    let cocked = guard_sword().at(Vec3::new(0.32, 1.22, -0.02));
    let secs = 0.5;
    Moves::new(
        [HANDLE, SWORD],
        vec![
            key(
                0.0,
                braced,
                Stroke::To(shield_up()),
                Stroke::To(guard_sword()),
            ),
            key(0.08, swat, Stroke::To(out), Stroke::To(cocked)),
            key(
                0.18,
                swat,
                Stroke::To(out.at(out.at + Vec3::new(-0.03, 0.0, 0.02))),
                Stroke::To(cocked),
            ),
            key(
                secs,
                braced,
                Stroke::To(shield_up()),
                Stroke::To(guard_sword()),
            ),
        ],
    )
    .clip(s, SHIELD_DEFLECT, false)
    .with_curve("effort", 11, move |ph| {
        keyed(
            ph * secs,
            &[(0.0, 0.35), (0.08, 1.0), (0.2, 0.7), (secs, 0.35)],
        )
    })
}

/// A broken guard: the shield is flung out wide and up by the blow, the trunk thrown back
/// and twisted, the sword arm out for balance, and the rear foot steps back to catch the
/// weight; then the shield comes back in front, low, and the foot steps back up.
fn guard_broken(s: &Skeleton) -> AnimationClip {
    let braced = STANCE.hips([0.0, -0.1, 0.02], [6.0, -20.0, 0.0], [5.0, 6.0, 0.0]);
    let thrown = STANCE
        .hips([0.02, -0.08, 0.1], [-6.0, -6.0, 0.0], [-12.0, 16.0, 5.0])
        .shoulders([-4.0, 14.0], [0.0, 8.0]);
    let caught = thrown
        .hips([0.03, -0.13, 0.2], [-2.0, -10.0, 0.0], [-6.0, 10.0, 3.0])
        .foot(1, foot(0.2, 0.5, -38.0));
    let flung = Hold::new(
        Vec3::new(-0.5, 1.45, -0.12),
        Vec3::new(0.25, 0.95, -0.1),
        Vec3::new(-0.9, 0.2, -0.3),
        Vec3::new(-0.3, -1.0, 0.4),
    );
    let balance = Hold::new(
        Vec3::new(0.48, 1.18, -0.02),
        Vec3::new(0.5, 0.75, -0.4),
        Vec3::new(0.0, -0.4, -0.9),
        Vec3::new(0.6, -1.0, 0.5),
    );
    let secs = 1.0;
    Moves::new(
        [HANDLE, SWORD],
        vec![
            key(
                0.0,
                braced,
                Stroke::To(shield_up()),
                Stroke::To(guard_sword()),
            ),
            key(0.12, thrown, Stroke::To(flung), Stroke::To(balance)),
            key(
                0.38,
                caught,
                Stroke::To(flung.at(flung.at + Vec3::new(0.04, -0.06, 0.04))),
                Stroke::To(balance.at(balance.at - Vec3::Y * 0.04)),
            ),
            key(
                0.72,
                caught.hips([0.0, -0.11, 0.16], [2.0, -16.0, 0.0], [2.0, 8.0, 0.0]),
                Stroke::To(shield_low()),
                // Still drawn in a little while the weight is back.
                Stroke::To(sword_ready().at(Vec3::new(0.25, 1.06, -0.25))),
            ),
            // The rear foot steps back up into the stance.
            key(
                secs,
                STANCE,
                Stroke::To(shield_low()),
                Stroke::To(sword_ready()),
            ),
        ],
    )
    .clip(s, GUARD_BROKEN, false)
    .with_curve("effort", 21, move |ph| {
        keyed(
            ph * secs,
            &[(0.0, 0.4), (0.12, 1.0), (0.4, 0.8), (secs, 0.2)],
        )
    })
}

/// A stance's upper body for stepping and circling in guard: its pose over square hips (the
/// graph layers it from the spine up over the legs' locomotion), held still.
fn moving(
    s: &Skeleton,
    name: &str,
    rigs: [Rig; 2],
    body: Body,
    hands: [Stroke; 2],
) -> AnimationClip {
    let body = body.squared();
    Moves::new(
        rigs,
        vec![
            key(0.0, body, hands[0], hands[1]),
            key(1.0, body, hands[0], hands[1]),
        ],
    )
    .clip(s, name, true)
    .with_curve("effort", 2, |_| 0.25)
}

/// The weapon stances' upper bodies for walking in guard: `sword_stance_moving`,
/// `shield_guard_moving` and `greatsword_stance_moving`.
fn moving_stances(s: &Skeleton) -> Vec<AnimationClip> {
    let braced = STANCE.hips([0.0, -0.1, 0.02], [6.0, -20.0, 0.0], [5.0, 6.0, 0.0]);
    vec![
        moving(
            s,
            "sword_stance_moving",
            [HANDLE, SWORD],
            STANCE,
            [Stroke::To(shield_low()), Stroke::To(sword_ready())],
        ),
        moving(
            s,
            "shield_guard_moving",
            [HANDLE, SWORD],
            braced,
            [Stroke::To(shield_up()), Stroke::To(guard_sword())],
        ),
        moving(
            s,
            "greatsword_stance_moving",
            [SWORD, SWORD],
            GS_STANCE,
            [Stroke::Below(GREATSWORD_HANDS), Stroke::To(gs_ready())],
        )
        .with_curve("grip", 2, |_| 1.0),
    ]
}

/// Carrying the sword and shield while walking about: arms only, for a layer.
fn sword_carry(s: &Skeleton) -> AnimationClip {
    let sword = Hold::new(
        Vec3::new(0.25, 0.98, -0.16),
        Vec3::new(0.0, 0.55, -0.83),
        Vec3::NEG_Y,
        Vec3::new(0.3, -1.0, 0.4),
    );
    let shield = Hold::new(
        Vec3::new(-0.25, 1.04, -0.2),
        Vec3::new(0.85, 0.2, -0.3),
        Vec3::new(-0.3, 0.0, -1.0),
        Vec3::new(-1.0, -0.7, 0.2),
    );
    Moves::new(
        [HANDLE, SWORD],
        vec![
            key(0.0, SQUARE, Stroke::To(shield), Stroke::To(sword)),
            key(1.0, SQUARE, Stroke::To(shield), Stroke::To(sword)),
        ],
    )
    .clip(s, "sword_carry", true)
}

// ------------------------------------------------------------------------------ great sword

/// Great sword stance: wider and lower than with a sword, the left foot ahead, the left
/// shoulder forward to the grip.
const GS_STANCE: Body = body(
    [0.0, -0.09, 0.0],
    [4.0, -22.0, 0.0],
    [3.0, 10.0, 0.0],
    [foot(-0.15, -0.3, 12.0), foot(0.2, 0.3, -40.0)],
)
// The left shoulder reaches forward to the grip.
.shoulders([16.0, 2.0], [4.0, 0.0]);

/// Both hands on the grip at the belly, the point up at the foe's face (Pflug).
fn gs_ready() -> Hold {
    Hold::new(
        Vec3::new(0.1, 1.06, -0.32),
        Vec3::new(0.0, 0.55, -0.83),
        Vec3::NEG_Y,
        Vec3::new(0.6, -1.0, 0.2),
    )
}

fn greatsword_sweep(s: &Skeleton) -> AnimationClip {
    let (secs, [wound, cut]) = WeaponMove::GreatswordSweep.timing();
    let back = Vec3::new(0.72, 0.45, 0.52).normalize();
    let grip = Stroke::Below(GREATSWORD_HANDS);
    Moves::new(
        [SWORD, SWORD],
        vec![
            key(0.0, GS_STANCE, grip, Stroke::To(gs_ready())),
            // Cocked over the right shoulder, point back, the trunk wound right.
            key(
                wound,
                GS_STANCE.hips([0.0, -0.1, 0.06], [2.0, -48.0, 0.0], [0.0, -22.0, 0.0]),
                grip,
                Stroke::To(Hold::new(
                    Vec3::new(0.34, 1.38, 0.02),
                    back,
                    Vec3::NEG_Z,
                    Vec3::new(1.0, 0.2, 0.4),
                )),
            ),
            // Across at the foe's ribs from right to left, the blade rolling over so the edge
            // leads, the trunk driving it round and the rear heel pivoting up.
            key(
                cut,
                GS_STANCE
                    .hips([0.0, -0.12, -0.06], [6.0, 30.0, 0.0], [4.0, 22.0, 0.0])
                    .foot(1, GS_STANCE.feet[1].heel(25.0)),
                grip,
                Stroke::Cut {
                    axis: plane(back, Vec3::new(0.0, -0.05, -1.0)),
                    degrees: 210.0,
                    to: Vec3::new(-0.28, 1.14, -0.3),
                    reach: 1.1,
                    elbow: Vec3::new(0.3, -1.0, -0.2),
                    edge: 1.0,
                },
            ),
            key(secs, GS_STANCE, grip, Stroke::To(gs_ready())),
        ],
    )
    .clip(s, WeaponMove::GreatswordSweep.clip_name(), false)
    .with_curve("effort", 31, move |ph| {
        keyed(
            ph * secs,
            &[(0.0, 0.2), (wound, 0.6), (cut - 0.08, 1.0), (secs, 0.2)],
        )
    })
    .with_curve("grip", 2, |_| 1.0)
    .with_event(wound / secs, "swing", Some("hand_r"))
}

fn greatsword_rise(s: &Skeleton) -> AnimationClip {
    let (secs, [wound, cut]) = WeaponMove::GreatswordRise.timing();
    let low = Vec3::new(-0.5, -0.55, 0.67).normalize();
    let grip = Stroke::Below(GREATSWORD_HANDS);
    Moves::new(
        [SWORD, SWORD],
        vec![
            key(0.0, GS_STANCE, grip, Stroke::To(gs_ready())),
            // Low on the left, point back and down.
            key(
                wound,
                GS_STANCE.hips([0.0, -0.12, 0.03], [3.0, 18.0, 0.0], [4.0, 18.0, 0.0]),
                grip,
                Stroke::To(Hold::new(
                    Vec3::new(-0.24, 1.0, -0.14),
                    low,
                    Vec3::NEG_Z,
                    Vec3::new(0.4, -0.6, -0.7),
                )),
            ),
            // Up through the foe from the left hip to over the right shoulder.
            key(
                cut,
                GS_STANCE
                    .hips([0.0, -0.08, -0.06], [5.0, -34.0, 0.0], [2.0, -18.0, 0.0])
                    .foot(1, GS_STANCE.feet[1].heel(10.0)),
                grip,
                Stroke::Cut {
                    axis: plane(low, Vec3::new(0.0, 0.1, -1.0)),
                    degrees: 200.0,
                    to: Vec3::new(0.3, 1.58, -0.3),
                    reach: 1.05,
                    elbow: Vec3::new(1.0, -0.2, 0.0),
                    edge: 1.0,
                },
            ),
            key(secs, GS_STANCE, grip, Stroke::To(gs_ready())),
        ],
    )
    .give(0.7)
    .clip(s, WeaponMove::GreatswordRise.clip_name(), false)
    .with_curve("effort", 31, move |ph| {
        keyed(
            ph * secs,
            &[(0.0, 0.2), (wound, 0.6), (cut - 0.08, 1.0), (secs, 0.2)],
        )
    })
    .with_curve("grip", 2, |_| 1.0)
    .with_event(wound / secs, "swing", Some("hand_r"))
}

fn greatsword_overhead(s: &Skeleton) -> AnimationClip {
    let (secs, [top, cut]) = WeaponMove::GreatswordOverhead.timing();
    let grip = Stroke::Below(GREATSWORD_HANDS);
    let raised = GS_STANCE.hips([0.0, -0.06, 0.04], [0.0, -26.0, 0.0], [-5.0, -10.0, 0.0]);
    // vom Tag: the hilt above the head, the blade back over it.
    let high = Hold::new(
        Vec3::new(0.12, 1.8, 0.02),
        Vec3::new(0.02, 0.66, 0.75),
        Vec3::NEG_Z,
        Vec3::new(0.9, 0.6, 0.2),
    );
    overhead_moves_with(top, cut, secs, grip, raised, high)
        .clip(s, WeaponMove::GreatswordOverhead.clip_name(), false)
        .with_curve("effort", 31, move |ph| {
            keyed(
                ph * secs,
                &[(0.0, 0.2), (top, 0.7), (cut - 0.06, 1.0), (secs, 0.2)],
            )
        })
        .with_curve("grip", 2, |_| 1.0)
        .with_event(top / secs, "swing", Some("hand_r"))
}

fn overhead_moves_with(
    top: f32,
    cut: f32,
    secs: f32,
    grip: Stroke,
    raised: Body,
    high: Hold,
) -> Moves {
    Moves::new(
        [SWORD, SWORD],
        vec![
            key(0.0, GS_STANCE, grip, Stroke::To(gs_ready())),
            key(top - 0.12, raised, grip, Stroke::To(high)),
            key(
                top,
                raised.hips([0.0, -0.05, 0.05], [0.0, -27.0, 0.0], [-6.0, -10.0, 0.0]),
                grip,
                Stroke::To(high.at(high.at + Vec3::new(0.0, 0.02, 0.02))),
            ),
            // Straight down, the front foot stepping in as the blade reaches out level and
            // the weight dropping behind it; the point finishes low.
            key(
                cut,
                GS_STANCE
                    .hips([0.0, -0.2, -0.16], [10.0, -6.0, 0.0], [18.0, 0.0, 0.0])
                    .foot(0, foot(-0.15, -0.48, 12.0))
                    .foot(1, GS_STANCE.feet[1].heel(20.0)),
                grip,
                Stroke::Cut {
                    axis: Vec3::NEG_X,
                    degrees: 210.0,
                    to: Vec3::new(-0.03, 1.02, -0.4),
                    reach: 0.97,
                    elbow: Vec3::new(0.6, -0.8, -0.3),
                    edge: 1.0,
                },
            ),
            key(secs, GS_STANCE, grip, Stroke::To(gs_ready())),
        ],
    )
}

/// Both hands on the great sword, point up at the foe (a loop).
fn greatsword_stance(s: &Skeleton) -> AnimationClip {
    let grip = Stroke::Below(GREATSWORD_HANDS);
    let settle = GS_STANCE.hips([0.0, -0.105, 0.01], [5.0, -23.0, 0.0], [4.0, 11.0, 0.0]);
    Moves::new(
        [SWORD, SWORD],
        vec![
            key(0.0, GS_STANCE, grip, Stroke::To(gs_ready())),
            key(
                0.9,
                settle,
                grip,
                Stroke::To(gs_ready().at(gs_ready().at - Vec3::Y * 0.015)),
            ),
            key(1.8, GS_STANCE, grip, Stroke::To(gs_ready())),
        ],
    )
    .clip(s, "greatsword_stance", true)
    .with_curve("effort", 2, |_| 0.2)
    .with_curve("grip", 2, |_| 1.0)
}

/// The great sword on the right shoulder in one hand while walking about, the left hand
/// free: arms only, for a layer.
fn greatsword_carry(s: &Skeleton) -> AnimationClip {
    let shoulder = Hold::new(
        Vec3::new(0.2, 1.12, -0.2),
        Vec3::new(0.15, 0.65, 0.74),
        Vec3::NEG_Z,
        Vec3::new(0.5, -1.0, 0.0),
    );
    let mut free = hand_hanging();
    free.at.x = -free.at.x;
    free.elbow.x = -free.elbow.x;
    Moves::new(
        [Rig::Free, SWORD],
        vec![
            key(0.0, SQUARE, Stroke::To(free), Stroke::To(shoulder)),
            key(1.0, SQUARE, Stroke::To(free), Stroke::To(shoulder)),
        ],
    )
    .clip(s, "greatsword_carry", true)
}

// -------------------------------------------------------------------------------- ripostes

/// The sword-and-shield riposte, and where its victim stands: this far ahead, facing back, m.
pub const SWORD_RIPOSTE: &str = "sword_riposte";
pub const SWORD_RIPOSTE_DISTANCE: f32 = 1.3;
/// The great sword's riposte, and where its victim stands.
pub const GREATSWORD_RIPOSTE: &str = "greatsword_riposte";
pub const GREATSWORD_RIPOSTE_DISTANCE: f32 = 1.6;
/// How long each riposte lasts, s (the blade goes in at
/// [`RIPOSTED_LANDS`](super::humanoid::RIPOSTED_LANDS), with the victim's clip).
pub const SWORD_RIPOSTE_SECS: f32 = 1.7;
pub const GREATSWORD_RIPOSTE_SECS: f32 = 1.9;

/// A riposte's thrust: the fist at `fist` with the blade along `axis` puts its tip `past`
/// meters beyond the victim's middle (`distance` ahead), at `height`.
fn run_through(distance: f32, blade: f32, past: f32, height: f32, axis: Vec3) -> Vec3 {
    let axis = axis.normalize();
    Vec3::new(0.0, height, -(distance + past)) - axis * blade
}

/// After a deflect: the sword chambered by the hip, a step in and the point driven through
/// the body at [`RIPOSTED_LANDS`](super::humanoid::RIPOSTED_LANDS), the shield held aside;
/// held in while the body sags on it, pulled out and back to the stance.
fn sword_riposte(s: &Skeleton) -> AnimationClip {
    use super::humanoid::RIPOSTED_LANDS as LANDS;
    let secs = SWORD_RIPOSTE_SECS;
    let point = Vec3::new(-0.04, -0.03, -1.0);
    let fist = run_through(SWORD_RIPOSTE_DISTANCE, SWORD_BLADE[1], 0.14, 1.12, point);
    let elbow = Vec3::new(0.7, -0.7, 0.1);
    let through = Hold::new(fist, point, Vec3::NEG_Y, elbow);
    let lunge = STANCE
        .hips([0.0, -0.15, -0.26], [6.0, 4.0, 0.0], [14.0, 6.0, 0.0])
        .foot(0, foot(-0.13, -0.58, 10.0))
        .foot(1, STANCE.feet[1].heel(14.0));
    let shield_aside = shield_low().at(Vec3::new(-0.24, 1.1, -0.28));
    let chambered = Hold::new(
        Vec3::new(0.24, 1.18, 0.05),
        point,
        Vec3::NEG_Y,
        Vec3::new(0.6, -0.6, 0.6),
    );
    Moves::new(
        [HANDLE, SWORD],
        vec![
            key(
                0.0,
                STANCE,
                Stroke::To(shield_low()),
                Stroke::To(sword_ready()),
            ),
            key(
                LANDS - 0.24,
                STANCE.hips([0.0, -0.1, 0.06], [2.0, -30.0, 0.0], [0.0, -12.0, 0.0]),
                Stroke::To(shield_low().at(Vec3::new(-0.1, 1.15, -0.36))),
                Stroke::To(chambered),
            ),
            key(LANDS, lunge, Stroke::To(shield_aside), Stroke::To(through)),
            // Held in, dipping with the body as it sags onto it.
            key(
                0.95,
                lunge,
                Stroke::To(shield_aside),
                Stroke::To(through.at(fist + Vec3::new(0.0, -0.04, 0.02))),
            ),
            // Pulled out along the blade, a step back.
            key(
                1.2,
                STANCE.hips([0.0, -0.1, 0.05], [2.0, -24.0, 0.0], [0.0, -8.0, 0.0]),
                Stroke::To(shield_low()),
                Stroke::To(chambered.at(Vec3::new(0.24, 1.12, -0.02))),
            ),
            key(
                secs,
                STANCE,
                Stroke::To(shield_low()),
                Stroke::To(sword_ready()),
            ),
        ],
    )
    .give(0.0)
    .clip(s, SWORD_RIPOSTE, false)
    .with_curve("effort", 31, move |ph| {
        keyed(
            ph * secs,
            &[
                (0.0, 0.2),
                (LANDS - 0.2, 0.6),
                (LANDS, 1.0),
                (1.2, 0.7),
                (secs, 0.15),
            ],
        )
    })
    .with_event((LANDS - 0.24) / secs, "swing", Some("hand_r"))
}

/// After a broken guard: the great sword drawn back to the right shoulder, point at the foe
/// (Ochs), then a step in and a two-handed thrust through the body at
/// [`RIPOSTED_LANDS`](super::humanoid::RIPOSTED_LANDS); held, pulled out, back to the stance.
fn greatsword_riposte(s: &Skeleton) -> AnimationClip {
    use super::humanoid::RIPOSTED_LANDS as LANDS;
    let secs = GREATSWORD_RIPOSTE_SECS;
    let grip = Stroke::Below(GREATSWORD_HANDS);
    let point = Vec3::new(-0.04, -0.04, -1.0);
    let fist = run_through(
        GREATSWORD_RIPOSTE_DISTANCE,
        GREATSWORD_BLADE[1],
        0.14,
        1.12,
        point,
    );
    let through = Hold::new(fist, point, Vec3::Y, Vec3::new(0.6, -0.8, 0.0));
    let ochs = Hold::new(
        Vec3::new(0.2, 1.45, 0.02),
        Vec3::new(-0.1, -0.14, -1.0),
        Vec3::Y,
        Vec3::new(0.9, 0.2, 0.3),
    );
    let drawn = GS_STANCE.hips([0.0, -0.08, 0.06], [0.0, -26.0, 0.0], [-4.0, -8.0, 0.0]);
    let lunge = GS_STANCE
        .hips([0.0, -0.18, -0.24], [8.0, -4.0, 0.0], [16.0, 0.0, 0.0])
        .foot(0, foot(-0.15, -0.55, 12.0))
        .foot(1, GS_STANCE.feet[1].heel(18.0));
    Moves::new(
        [SWORD, SWORD],
        vec![
            key(0.0, GS_STANCE, grip, Stroke::To(gs_ready())),
            key(LANDS - 0.22, drawn, grip, Stroke::To(ochs)),
            key(LANDS, lunge, grip, Stroke::To(through)),
            key(
                1.0,
                lunge,
                grip,
                Stroke::To(through.at(fist + Vec3::new(0.0, -0.04, 0.02))),
            ),
            key(
                1.3,
                GS_STANCE.hips([0.0, -0.1, 0.05], [2.0, -24.0, 0.0], [0.0, 4.0, 0.0]),
                grip,
                Stroke::To(through.at(Vec3::new(0.14, 1.16, -0.08))),
            ),
            key(secs, GS_STANCE, grip, Stroke::To(gs_ready())),
        ],
    )
    .give(0.0)
    .clip(s, GREATSWORD_RIPOSTE, false)
    .with_curve("effort", 31, move |ph| {
        keyed(
            ph * secs,
            &[
                (0.0, 0.2),
                (LANDS - 0.2, 0.7),
                (LANDS, 1.0),
                (1.3, 0.7),
                (secs, 0.2),
            ],
        )
    })
    .with_curve("grip", 2, |_| 1.0)
    .with_event((LANDS - 0.22) / secs, "swing", Some("hand_r"))
}

// ------------------------------------------------------------------------------------- bow

/// Side-on to the target (ahead, −Z): feet across the line, toes to the right, hips turned
/// square to them, the head turned back to the target.
const BOW_STANCE: Body = body(
    [0.0, -0.03, 0.0],
    [0.0, -86.0, 0.0],
    [0.0, -4.0, 0.0],
    [foot(0.02, -0.18, -82.0), foot(0.0, 0.2, -82.0)],
);

/// The bow arm out at the target, the bow upright with a little cant.
fn bow_up() -> Hold {
    Hold::new(
        Vec3::new(0.0, 1.45, -0.76),
        Vec3::new(0.12, 1.0, 0.0),
        Vec3::NEG_Z,
        Vec3::new(-0.5, -1.0, 0.0),
    )
}

/// The bow carried at the left side, pointing ahead and a little up, the string toward the leg.
fn bow_carry_hold() -> Hold {
    Hold::new(
        Vec3::new(-0.26, 0.94, -0.02),
        Vec3::new(0.0, 0.35, -0.94),
        Vec3::NEG_X,
        Vec3::new(-0.3, -1.0, 0.3),
    )
}

/// The free hand hanging at the right side.
fn hand_hanging() -> Hold {
    Hold::new(
        Vec3::new(0.24, 0.84, 0.03),
        Vec3::NEG_Y,
        Vec3::NEG_Z,
        Vec3::new(0.2, -1.0, 0.3),
    )
}

/// Draw hand positions: on the string at brace, anchored under the jaw, carried back.
fn on_string() -> Hold {
    Hold::new(
        Vec3::new(0.03, 1.44, -0.62),
        Vec3::NEG_Y,
        Vec3::NEG_Z,
        Vec3::new(0.3, -0.3, 1.0),
    )
}

/// The draw hand anchored under the jaw at full draw, the elbow back at shoulder height.
pub(crate) const ANCHOR: Vec3 = Vec3::new(0.04, 1.49, -0.07);

fn anchor() -> Hold {
    Hold::new(ANCHOR, Vec3::NEG_Y, Vec3::NEG_Z, Vec3::new(0.15, -0.1, 1.0))
}

fn bow_shot_clip(s: &Skeleton, m: WeaponMove) -> AnimationClip {
    let (secs, [release, follow]) = m.timing();
    let set = 0.3;
    let full = 0.78;
    let drawn = BOW_STANCE.hips([0.0, -0.035, 0.01], [0.0, -87.0, 0.0], [-2.0, -4.0, 0.0]);
    let after = anchor().at(ANCHOR + Vec3::new(0.03, -0.02, 0.07));
    let free = Rig::Free;
    Moves::new(
        [HANDLE, free],
        vec![
            key(
                0.0,
                SQUARE,
                Stroke::To(bow_carry_hold()),
                Stroke::To(hand_hanging()),
            ),
            // Turned side-on, the bow up at the target and the fingers on the string.
            key(
                set,
                BOW_STANCE,
                Stroke::To(bow_up()),
                Stroke::To(on_string()),
            ),
            // Drawn back to the anchor under the jaw, the shoulders set.
            key(
                full,
                drawn.shoulders([4.0, -2.0], [-10.0, 4.0]),
                Stroke::To(bow_up()),
                Stroke::To(anchor()),
            ),
            // Aiming.
            key(
                release,
                drawn.shoulders([4.0, -2.0], [-12.0, 4.0]),
                Stroke::To(bow_up()),
                Stroke::To(anchor().at(ANCHOR + Vec3::new(0.0, 0.0, 0.01))),
            ),
            // The string goes; the hand carries on back along the jaw.
            key(
                follow,
                drawn.shoulders([4.0, -2.0], [-14.0, 4.0]),
                Stroke::To(bow_up()),
                Stroke::To(after),
            ),
            key(
                follow + 0.14,
                drawn,
                Stroke::To(bow_up()),
                Stroke::To(after.at(after.at + Vec3::new(0.01, -0.03, 0.0))),
            ),
            key(
                secs,
                SQUARE,
                Stroke::To(bow_carry_hold()),
                Stroke::To(hand_hanging()),
            ),
        ],
    )
    .clip(s, m.clip_name(), false)
    .with_curve("effort", 31, move |ph| {
        keyed(
            ph * secs,
            &[
                (0.0, 0.1),
                (set, 0.3),
                (full, 0.8),
                (release, 0.9),
                (follow + 0.1, 0.3),
                (secs, 0.1),
            ],
        )
    })
    // How far the string is drawn (0 at brace, 1 at the anchor), for the bow and the arrow.
    .with_curve("draw", (secs * 60.0) as usize, move |ph| {
        let t = ph * secs;
        if t >= release {
            0.0
        } else {
            keyed(t, &[(set, 0.0), (full, 1.0)])
        }
    })
    // An arrow on the string from when the hand reaches it until it flies.
    .with_curve("nocked", (secs * 60.0) as usize, move |ph| {
        let t = ph * secs;
        if (set - 0.05..release).contains(&t) {
            1.0
        } else {
            0.0
        }
    })
    .with_event(release / secs, "release", Some("hand_r"))
}

/// The bow carried at the left side while walking about: arms only, for a layer.
fn bow_carry(s: &Skeleton) -> AnimationClip {
    Moves::new(
        [HANDLE, Rig::Free],
        vec![
            key(
                0.0,
                SQUARE,
                Stroke::To(bow_carry_hold()),
                Stroke::To(hand_hanging()),
            ),
            key(
                1.0,
                SQUARE,
                Stroke::To(bow_carry_hold()),
                Stroke::To(hand_hanging()),
            ),
        ],
    )
    .clip(s, "bow_carry", true)
}

/// The draw held for as long as the archer likes (`draw` and `loose` triggers, for the
/// game's own aiming): [`BOW_DRAW`] brings the string to the anchor and stops there (its
/// `drawn` event at full draw), [`BOW_HOLD`] holds it, a breath of sway in the aim, and
/// [`BOW_LOOSE`] lets it go (`release` at once) and lowers the bow; [`BOW_LOWER`] lets the
/// string down without shooting. The poses are the shot's ([`bow_shot_clip`]).
pub const BOW_DRAW: &str = "bow_draw";
pub const BOW_HOLD: &str = "bow_hold";
pub const BOW_LOOSE: &str = "bow_loose";
pub const BOW_LOWER: &str = "bow_lower";

/// When the `release` event comes in [`BOW_LOOSE`], s.
pub const LOOSE_RELEASE: f32 = 0.03;

fn bow_drawn() -> Body {
    BOW_STANCE.hips([0.0, -0.035, 0.01], [0.0, -87.0, 0.0], [-2.0, -4.0, 0.0])
}

fn bow_draw_clips(s: &Skeleton) -> Vec<AnimationClip> {
    let (set, full) = (0.3, 0.78);
    let drawn = bow_drawn();
    let aim = |back: f32, at: Vec3| {
        (
            drawn.shoulders([4.0, -2.0], [back, 4.0]),
            Stroke::To(bow_up()),
            Stroke::To(anchor().at(ANCHOR + at)),
        )
    };
    let k = |t: f32, (b, l, r): (Body, Stroke, Stroke)| key(t, b, l, r);
    let carried = || {
        (
            SQUARE,
            Stroke::To(bow_carry_hold()),
            Stroke::To(hand_hanging()),
        )
    };
    let draw_secs = full + 0.12;
    let draw = Moves::new(
        [HANDLE, Rig::Free],
        vec![
            k(0.0, carried()),
            key(
                set,
                BOW_STANCE,
                Stroke::To(bow_up()),
                Stroke::To(on_string()),
            ),
            k(full, aim(-10.0, Vec3::ZERO)),
            k(draw_secs, aim(-11.0, Vec3::new(0.0, 0.0, 0.005))),
        ],
    )
    .clip(s, BOW_DRAW, false)
    .with_curve("effort", 16, move |ph| {
        keyed(ph * draw_secs, &[(0.0, 0.1), (set, 0.3), (full, 0.8)])
    })
    .with_curve("draw", (draw_secs * 60.0) as usize, move |ph| {
        keyed(ph * draw_secs, &[(set, 0.0), (full, 1.0)])
    })
    .with_curve("nocked", (draw_secs * 60.0) as usize, move |ph| {
        if ph * draw_secs >= set - 0.05 {
            1.0
        } else {
            0.0
        }
    })
    .with_event(full / draw_secs, "drawn", Some("hand_r"));
    // Holding: the back keeps working, the anchor creeps a few millimetres and comes back.
    let hold_secs = 2.4;
    let hold = Moves::new(
        [HANDLE, Rig::Free],
        vec![
            k(0.0, aim(-11.0, Vec3::new(0.0, 0.0, 0.005))),
            k(0.8, aim(-12.0, Vec3::new(0.002, -0.003, 0.009))),
            k(1.6, aim(-11.5, Vec3::new(-0.002, 0.002, 0.003))),
            k(hold_secs, aim(-11.0, Vec3::new(0.0, 0.0, 0.005))),
        ],
    )
    .clip(s, BOW_HOLD, true)
    .with_curve("draw", 2, |_| 1.0)
    .with_curve("nocked", 2, |_| 1.0)
    .with_curve("effort", 2, |_| 0.9);
    // The string goes; the hand carries on back along the jaw; the bow comes down.
    let after = anchor().at(ANCHOR + Vec3::new(0.03, -0.02, 0.07));
    let r = LOOSE_RELEASE;
    let loose_secs = r + 0.5;
    let loose = Moves::new(
        [HANDLE, Rig::Free],
        vec![
            k(0.0, aim(-11.0, Vec3::new(0.0, 0.0, 0.005))),
            key(
                r + 0.05,
                drawn.shoulders([4.0, -2.0], [-14.0, 4.0]),
                Stroke::To(bow_up()),
                Stroke::To(after),
            ),
            key(
                r + 0.19,
                drawn,
                Stroke::To(bow_up()),
                Stroke::To(after.at(after.at + Vec3::new(0.01, -0.03, 0.0))),
            ),
            k(loose_secs, carried()),
        ],
    )
    .clip(s, BOW_LOOSE, false)
    .with_curve("effort", 16, move |ph| {
        keyed(
            ph * loose_secs,
            &[(0.0, 0.9), (r + 0.15, 0.3), (loose_secs, 0.1)],
        )
    })
    .with_curve("draw", (loose_secs * 60.0) as usize, move |ph| {
        if ph * loose_secs < r { 1.0 } else { 0.0 }
    })
    .with_curve("nocked", (loose_secs * 60.0) as usize, move |ph| {
        if ph * loose_secs < r { 1.0 } else { 0.0 }
    })
    .with_event(r / loose_secs, "release", Some("hand_r"));
    // Letting down: the string eases forward with the arrow still on it, the bow comes down.
    let lower_secs = 0.7;
    let lower = Moves::new(
        [HANDLE, Rig::Free],
        vec![
            k(0.0, aim(-11.0, Vec3::new(0.0, 0.0, 0.005))),
            key(
                0.4,
                BOW_STANCE,
                Stroke::To(bow_up()),
                Stroke::To(on_string()),
            ),
            k(lower_secs, carried()),
        ],
    )
    .clip(s, BOW_LOWER, false)
    .with_curve("draw", (lower_secs * 60.0) as usize, move |ph| {
        keyed(ph * lower_secs, &[(0.0, 1.0), (0.4, 0.0)])
    })
    .with_curve("nocked", (lower_secs * 60.0) as usize, move |ph| {
        if ph * lower_secs < 0.55 { 1.0 } else { 0.0 }
    });
    vec![draw, hold, loose, lower]
}

// ------------------------------------------------------------------------------- long gun

/// A long gun (ItemClass::Firearm) in the right fist on its pistol grip: the barrel runs out
/// over the knuckles (along the hand), its top on the thumb side, in the right hand's frame.
pub const RIFLE_AXIS: Vec3 = Vec3::NEG_Y;
pub const RIFLE_UP: Vec3 = Vec3::NEG_Z;

/// The bore this far above the right fist's middle (along [`RIFLE_UP`]), meters.
pub const RIFLE_BORE: f32 = 0.075;

/// The left fist on the fore grip, this far ahead of the right fist along the barrel.
pub const RIFLE_FORE: f32 = 0.26;

/// The magazine's well this far ahead of the right fist along the barrel.
pub const RIFLE_MAG: f32 = 0.1;

/// The muzzle and the butt this far ahead of and behind the right fist along the barrel.
pub const RIFLE_MUZZLE: f32 = 0.72;
pub const RIFLE_BUTT: f32 = 0.41;

/// The long gun's states: shouldered and aimed while `aim` is up ([`RIFLE_AIM`], walking in
/// it from the spine up), a shot on `fire` ([`RIFLE_FIRE`], the `shot` event as it goes off:
/// the game's to fly), a magazine changed on `reload` ([`RIFLE_RELOAD`], `mag_out` and
/// `mag_in` events, the `mag_in_hand` curve while the left hand holds one), and carried low
/// at the ready while walking about (`rifle_carry`, a layer).
pub const RIFLE_AIM: &str = "rifle_aim";
pub const RIFLE_FIRE: &str = "rifle_fire";
pub const RIFLE_RELOAD: &str = "rifle_reload";
pub const RIFLE_CARRY: &str = "rifle_carry";

/// When a [`RIFLE_FIRE`] shot goes off, s.
pub const RIFLE_SHOT_AT: f32 = 0.01;

const RIFLE: Rig = Rig::Held {
    axis: RIFLE_AXIS,
    face: RIFLE_UP,
};

/// Bladed a little to the target, the left foot ahead, the weight forward over it.
const RIFLE_STANCE: Body = body(
    [0.0, -0.05, -0.02],
    [0.0, -30.0, 0.0],
    [-5.0, 12.0, 0.0],
    [foot(-0.12, -0.13, -12.0), foot(0.15, 0.16, -48.0)],
);

/// The right fist that puts the bore at `bore`, pointing `dir` with its top toward `up`.
fn rifle_at(bore: Vec3, dir: Vec3, up: Vec3, elbow: Vec3) -> Hold {
    let h = Hold::new(bore, dir, up, elbow);
    h.at(bore - h.face * RIFLE_BORE)
}

/// Shouldered: the butt in at the chest under the chin, the sights under the eye, the muzzle
/// at the target ahead (the fist on the middle line: further right the wrist can't turn the
/// barrel back in line).
fn rifle_aimed() -> Hold {
    rifle_at(
        Vec3::new(0.0, 1.5, -0.3),
        Vec3::NEG_Z,
        Vec3::Y,
        Vec3::new(0.6, -1.0, 0.2),
    )
}

/// Low ready: the gun under the arm, the muzzle down ahead of the feet.
fn rifle_low() -> Hold {
    let dir = Vec3::new(-0.3, -0.5, -0.81).normalize();
    rifle_at(
        Vec3::new(0.17, 1.21, -0.1),
        dir,
        Vec3::Y,
        Vec3::new(0.3, -1.0, 0.4),
    )
}

/// The left hand off the gun at `at` (reaching with a straight wrist).
fn left_free(at: Vec3) -> Hold {
    Hold::new(at, Vec3::NEG_Z, Vec3::NEG_Y, Vec3::new(-0.5, -1.0, 0.2))
}

/// Where the left fist closes on the fore grip of the gun held `r`.
fn on_fore(r: &Hold) -> Vec3 {
    r.at + r.axis * RIFLE_FORE
}

/// Where the left fist holds the magazine of the gun held `r`, down out of the well `down`
/// meters.
fn at_mag(r: &Hold, down: f32) -> Vec3 {
    r.at + r.axis * RIFLE_MAG - r.face * (0.05 + down)
}

fn rifle_clips(s: &Skeleton) -> Vec<AnimationClip> {
    let aimed = rifle_aimed();
    let fore = Stroke::Below(-RIFLE_FORE);
    let grip = |c: AnimationClip| c.with_curve("grip", 2, |_| 1.0);
    // Aiming: still but breathing, the muzzle wandering a few millimetres.
    let breathe = RIFLE_STANCE.hips([0.0, -0.055, -0.02], [0.0, -30.0, 0.0], [-5.5, 12.0, 0.0]);
    let aim_secs = 2.4;
    let aim = Moves::new(
        [RIFLE, RIFLE],
        vec![
            key(0.0, RIFLE_STANCE, fore, Stroke::To(aimed)),
            key(
                1.2,
                breathe,
                fore,
                Stroke::To(aimed.at(aimed.at + Vec3::new(0.002, -0.004, 0.0))),
            ),
            key(aim_secs, RIFLE_STANCE, fore, Stroke::To(aimed)),
        ],
    )
    .clip(s, RIFLE_AIM, true)
    .with_curve("effort", 2, |_| 0.35);
    let aim_moving = moving(
        s,
        "rifle_aim_moving",
        [RIFLE, RIFLE],
        RIFLE_STANCE,
        [fore, Stroke::To(aimed)],
    );
    // A shot: the gun kicks back into the shoulder and the muzzle climbs, then settles.
    let fire_secs = 0.24;
    let kick = Hold::new(
        aimed.at + Vec3::new(0.0, 0.012, 0.028),
        Vec3::new(0.0, 0.06, -1.0),
        Vec3::Y,
        aimed.elbow,
    );
    let rocked = RIFLE_STANCE.hips([0.0, -0.05, -0.008], [0.0, -30.0, 0.0], [-3.0, 12.0, 0.0]);
    let fire = Moves::new(
        [RIFLE, RIFLE],
        vec![
            key(0.0, RIFLE_STANCE, fore, Stroke::To(aimed)),
            key(RIFLE_SHOT_AT + 0.035, rocked, fore, Stroke::To(kick)),
            key(fire_secs, RIFLE_STANCE, fore, Stroke::To(aimed)),
        ],
    )
    .clip(s, RIFLE_FIRE, false)
    .with_curve("effort", 8, move |ph| {
        keyed(ph * fire_secs, &[(0.0, 0.5), (0.05, 0.8), (fire_secs, 0.4)])
    })
    .with_event(RIFLE_SHOT_AT / fire_secs, "shot", Some("hand_r"));
    // A magazine change: the gun down off the shoulder and canted to the left hand, the old
    // magazine out and away to the pouch, a fresh one in and slapped home, the hand back on
    // the fore grip and the gun up again.
    let canted = {
        let dir = Vec3::new(-0.05, -0.18, -1.0).normalize();
        let up = Quat::from_axis_angle(dir, 0.45) * Vec3::Y;
        rifle_at(
            Vec3::new(0.07, 1.36, -0.32),
            dir,
            up,
            Vec3::new(1.0, -0.8, 0.3),
        )
    };
    let pouch = Vec3::new(-0.1, 1.02, -0.16);
    let ducked = RIFLE_STANCE.hips([0.0, -0.06, -0.02], [0.0, -30.0, 0.0], [-9.0, 12.0, 4.0]);
    let r = |h: Hold| Stroke::To(h);
    let l = |at: Vec3| Stroke::To(left_free(at));
    let reload_secs = 2.3;
    let [grab, out, stowed, back, home, slapped, regrip] = [0.32, 0.5, 0.88, 1.28, 1.48, 1.6, 1.9];
    let reload = Moves::new(
        [Rig::Free, RIFLE],
        vec![
            key(0.0, RIFLE_STANCE, l(on_fore(&aimed)), r(aimed)),
            key(grab, ducked, l(at_mag(&canted, 0.0)), r(canted)),
            key(out, ducked, l(at_mag(&canted, 0.16)), r(canted)),
            key(stowed, ducked, l(pouch), r(canted)),
            key(back, ducked, l(at_mag(&canted, 0.1)), r(canted)),
            key(home, ducked, l(at_mag(&canted, 0.0)), r(canted)),
            key(slapped, ducked, l(at_mag(&canted, -0.012)), r(canted)),
            key(regrip, ducked, l(on_fore(&canted)), r(canted)),
            key(reload_secs, RIFLE_STANCE, l(on_fore(&aimed)), r(aimed)),
        ],
    )
    .clip(s, RIFLE_RELOAD, false)
    .with_curve("effort", 16, |_| 0.4)
    // The left hand on the fore grip only at the ends: off it, it goes where the clip says.
    .with_curve("grip", (reload_secs * 30.0) as usize, move |ph| {
        keyed(
            ph * reload_secs,
            &[
                (0.1, 1.0),
                (grab - 0.08, 0.0),
                (regrip, 0.0),
                (regrip + 0.12, 1.0),
            ],
        )
    })
    .with_curve("mag_in_hand", (reload_secs * 60.0) as usize, move |ph| {
        if (grab + 0.04..home).contains(&(ph * reload_secs)) {
            1.0
        } else {
            0.0
        }
    })
    .with_event((grab + 0.04) / reload_secs, "mag_out", Some("hand_l"))
    .with_event(home / reload_secs, "mag_in", Some("hand_l"));
    // Walking about with it at the low ready: arms only, for a layer.
    let low = rifle_low();
    let carry = Moves::new(
        [RIFLE, RIFLE],
        vec![
            key(0.0, SQUARE, fore, Stroke::To(low)),
            key(1.0, SQUARE, fore, Stroke::To(low)),
        ],
    )
    .clip(s, RIFLE_CARRY, true);
    vec![grip(aim), grip(aim_moving), grip(fire), reload, grip(carry)]
}

// ------------------------------------------------------------------------------ aimed lines

/// How high an aimed cut or stab goes: at the head and neck, the trunk and arms, or the legs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Height {
    High,
    Mid,
    Low,
}

impl Height {
    pub const ALL: [Height; 3] = [Height::High, Height::Mid, Height::Low];

    pub fn name(self) -> &'static str {
        match self {
            Height::High => "high",
            Height::Mid => "mid",
            Height::Low => "low",
        }
    }

    /// Where on a foe of the generated size it crosses (meters up).
    fn at(self) -> f32 {
        match self {
            Height::High => 1.58,
            Height::Mid => 1.2,
            Height::Low => 0.55,
        }
    }
}

/// The eight lines an aimed cut can travel, degrees round the foe as the attacker sees it:
/// 0 straight down, 45 down from the right shoulder (the forehand diagonal), 90 across from
/// the right, 135 rising from the right hip, 180 straight up, 225 rising from the left hip,
/// 270 across from the left (the backhand) and 315 down from the left shoulder.
pub const LINE_ANGLES: [f32; 8] = [0.0, 45.0, 90.0, 135.0, 180.0, 225.0, 270.0, 315.0];

/// The way a cut along `angle` travels across the foe (model space, the attacker facing −Z,
/// its right +X).
pub fn line_travel(angle: f32) -> Vec3 {
    let a = angle.to_radians();
    Vec3::new(-a.sin(), -a.cos(), 0.0)
}

/// An aimed cut's clip: `set` (`sword` or `greatsword`), the line and the height.
pub fn line_clip(set: &str, angle: f32, height: Height) -> String {
    format!("{set}_line_{:03}_{}", angle.round() as i32, height.name())
}

/// An aimed stab's clip.
pub fn stab_clip(set: &str, height: Height) -> String {
    format!("{set}_stab_{}", height.name())
}

/// An aimed cut's or stab's length and stroke (end of the wind-up, end of the cut), seconds,
/// one-handed or with both hands.
pub fn line_timing(two_handed: bool, stab: bool) -> (f32, [f32; 2]) {
    match (two_handed, stab) {
        (false, false) => (0.62, [0.18, 0.4]),
        (false, true) => (0.66, [0.22, 0.36]),
        (true, false) => (1.0, [0.34, 0.6]),
        (true, true) => (0.95, [0.32, 0.5]),
    }
}

/// Where the fist is as the blade crosses the foe at `height` (model space).
fn line_contact(height: Height, two_handed: bool) -> Vec3 {
    let y = (0.75 + height.at() * 0.42).clamp(0.98, 1.42);
    Vec3::new(0.06, y, if two_handed { -0.4 } else { -0.44 })
}

/// How far ahead of the attacker the foe's middle is when its aimed cuts and stabs cross
/// it, m.
pub fn line_reach(two_handed: bool) -> f32 {
    if two_handed { 1.45 } else { 1.15 }
}

/// The point a cut or stab at `height` passes through, the foe's middle a step ahead.
fn line_point(height: Height, two_handed: bool) -> Vec3 {
    Vec3::new(0.0, height.at(), -line_reach(two_handed))
}

/// An aimed cut along `angle` at `height`: wound back on the side it comes from, the blade
/// turning about the line's axis through the foe and on past it, the body turning and
/// sinking with it (low lines bend at the hips and knees), then back to the stance.
fn line_cut_with(
    s: &Skeleton,
    two_handed: bool,
    angle: f32,
    height: Height,
    plan: LinePlan,
) -> (AnimationClip, f32) {
    let LinePlan {
        shift,
        give,
        pull,
        elbow: lead,
        edge,
    } = plan;
    let (secs, [wound, cut]) = line_timing(two_handed, false);
    let travel = line_travel(angle);
    let (low, dip) = match height {
        Height::Low => (1.0, 18.0),
        Height::Mid => (0.35, 6.0),
        Height::High => (0.0, 2.0),
    };
    let sink = 0.1 + 0.17 * low;
    // The fist meets the line as low as the body sinks: a blade sits at least 37° off its
    // forearm, so a fist held high can't turn it down at a shin.
    let contact = line_contact(height, two_handed) - Vec3::Y * (sink - 0.1);
    let point = (line_point(height, two_handed) - contact).normalize();
    let along = square(travel, point);
    // The tip leads along axis × blade: at the foe the blade points at it and moves along
    // the line.
    let axis = point.cross(along).normalize();
    let back = 105.0_f32;
    // On past the foe far enough to follow through, short of the wrist folding over: a low
    // cut stops sooner, the arm already reaching down.
    let through = if height == Height::Low {
        140.0_f32
    } else {
        165.0
    };
    let wound_dir = Quat::from_axis_angle(axis, -back.to_radians()) * point;
    // How far the body sinks into the cut, and where its right shoulder is then: a fist
    // the arm can't reach is drawn in toward it (`pull` thirds of the way).
    // The trunk bows `dip` at the hips and again above them (a negative pitch leans it
    // forward), carrying the shoulder 0.45 m above the hips forward and down.
    let bow = (2.0_f32 * dip).to_radians();
    let shoulder = Vec3::new(
        0.17,
        1.42 - sink - 0.45 * (1.0 - bow.cos()),
        -0.05 - 0.45 * bow.sin(),
    );
    let reachable = |p: Vec3| shoulder + (p - shoulder) * (1.0 - pull / 3.0);
    // The fist starts on the side the line comes from, drawn in toward the body, and ends on
    // the far side.
    let side = |v: Vec3, k: f32| {
        let p = contact + v * k + Vec3::new(0.0, 0.0, 0.24);
        Vec3::new(p.x, p.y.clamp(0.95 - sink, 1.62), p.z)
    };
    let start = reachable(side(-along, 0.36)) + shift;
    let end = contact + along * 0.34 + Vec3::new(0.0, 0.0, 0.1);
    let end = reachable(Vec3::new(end.x, end.y.clamp(0.85 - sink, 1.62), end.z)) + shift;
    // The elbow out on the side the cut comes from, down for rising cuts; through the cut
    // it leads (`lead` 1), stays out and down (0) or trails (−1), as the wrist likes best.
    let elbow = |toward: Vec3| (Vec3::new(0.7, -0.6, 0.2) + toward * 0.6).normalize();
    let wound_hold = Hold::new(start, wound_dir, Vec3::NEG_Z, elbow(-along));
    // Turn into it from the far side: a cut from the right winds the chest right.
    let turn = -along.x;
    let base = if two_handed { GS_STANCE } else { STANCE };
    let wound_body = base.hips(
        [0.0, -0.08 - 0.04 * low, 0.04],
        [2.0, base.turn.y + 26.0 * turn, 0.0],
        [base.trunk.x, base.trunk.y + 12.0 * turn, 0.0],
    );
    let cut_body = base
        .hips(
            [0.0, -sink, -0.08],
            [-dip, base.turn.y - 16.0 * turn, 0.0],
            [base.trunk.x - dip, base.trunk.y - 12.0 * turn, -4.0 * turn],
        )
        .foot(1, base.feet[1].heel(10.0 + 6.0 * low));
    let strike = Stroke::Cut {
        axis,
        degrees: through,
        to: end,
        reach: 1.0,
        elbow: elbow(along * lead),
        edge,
    };
    let (rigs, keys) = if two_handed {
        let grip = Stroke::Below(GREATSWORD_HANDS);
        (
            [SWORD, SWORD],
            vec![
                key(0.0, GS_STANCE, grip, Stroke::To(gs_ready())),
                key(wound, wound_body, grip, Stroke::To(wound_hold)),
                key(cut, cut_body, grip, strike),
                key(secs, GS_STANCE, grip, Stroke::To(gs_ready())),
            ],
        )
    } else {
        (
            [HANDLE, SWORD],
            vec![
                key(
                    0.0,
                    STANCE,
                    Stroke::To(shield_low()),
                    Stroke::To(sword_ready()),
                ),
                key(
                    wound,
                    wound_body,
                    Stroke::To(shield_low().at(Vec3::new(-0.12, 1.14 - 0.04 * low, -0.32))),
                    Stroke::To(wound_hold),
                ),
                key(
                    cut,
                    cut_body,
                    // The shield sinks with the body.
                    Stroke::To(shield_low().at(Vec3::new(-0.34, 1.28 - sink, -0.16))),
                    strike,
                ),
                key(
                    secs,
                    STANCE,
                    Stroke::To(shield_low()),
                    Stroke::To(sword_ready()),
                ),
            ],
        )
    };
    let set = if two_handed { "greatsword" } else { "sword" };
    let (clip, short) =
        Moves::new(rigs, keys)
            .give(give)
            .clip_reaching(s, &line_clip(set, angle, height), false);
    let clip = clip
        .with_curve("effort", 31, move |ph| {
            keyed(
                ph * secs,
                &[(0.0, 0.15), (wound, 0.5), (cut - 0.08, 1.0), (secs, 0.15)],
            )
        })
        .with_curve("grip", 2, move |_| if two_handed { 1.0 } else { 0.0 })
        .with_event(wound / secs, "swing", Some("hand_r"));
    (clip, short)
}

/// Where a clip's blade passes nearest `point` during `[from, to]` seconds: the gap from the
/// blade to the point there (model space, point minus blade) and how much the blade there
/// moves along `travel` (−1 to 1, across the foe).
fn blade_gap(
    s: &Skeleton,
    clip: &AnimationClip,
    length: f32,
    point: Vec3,
    span: [f32; 2],
    travel: Vec3,
) -> (Vec3, f32) {
    let binding = crate::clip::ClipBinding::new(clip, s);
    let mut local = s.rest.clone();
    let mut model = s.rest_model.clone();
    let mut blade = |t: f32| {
        local.copy_from_slice(&s.rest);
        clip.sample(t, &binding, &mut local);
        s.to_model(&local, &mut model);
        let hand = model[s.bone("hand_r")];
        let a = hand.transform_point(GRIP);
        (a, a + hand.r * BLADE_AXIS * length)
    };
    let mut best = (f32::INFINITY, Vec3::ZERO, 0.0);
    let n = 40;
    let mut was = blade(span[0]);
    for k in 1..=n {
        let t = span[0] + (span[1] - span[0]) * k as f32 / n as f32;
        let (a, b) = blade(t);
        let ab = b - a;
        let u = ((point - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
        let gap = point - (a + ab * u);
        if gap.length() < best.0 {
            let moved = (a + ab * u) - was.0.lerp(was.1, u);
            let along = moved.with_z(0.0).normalize_or_zero().dot(travel);
            best = (gap.length(), gap, along);
        }
        was = (a, b);
    }
    (best.1, best.2)
}

/// How far a two-handed clip's left fist comes off its grip, at worst (m, 240 samples a
/// second: a slip can last a frame).
fn grip_gap(s: &Skeleton, clip: &AnimationClip) -> f32 {
    let binding = crate::clip::ClipBinding::new(clip, s);
    let mut local = s.rest.clone();
    let mut model = s.rest_model.clone();
    let n = ((clip.duration * 240.0).ceil() as usize).max(1);
    let mut worst = 0.0f32;
    for k in 0..=n {
        local.copy_from_slice(&s.rest);
        clip.sample(clip.duration * k as f32 / n as f32, &binding, &mut local);
        s.to_model(&local, &mut model);
        let (r, l) = (model[s.bone("hand_r")], model[s.bone("hand_l")]);
        let want = r.transform_point(GRIP) - r.r * BLADE_AXIS * GREATSWORD_HANDS;
        worst = worst.max(l.transform_point(GRIP).distance(want));
    }
    worst
}

/// How many of a clip's samples (60 a second) take a wrist past its range, both counted.
fn wrist_out(s: &Skeleton, clip: &AnimationClip) -> usize {
    let binding = crate::clip::ClipBinding::new(clip, s);
    let mut local = s.rest.clone();
    let mut model = s.rest_model.clone();
    let n = ((clip.duration * 60.0).ceil() as usize).max(1);
    let mut out = 0;
    for k in 0..=n {
        local.copy_from_slice(&s.rest);
        clip.sample(clip.duration * k as f32 / n as f32, &binding, &mut local);
        s.to_model(&local, &mut model);
        for side in 0..2 {
            let (lower, hand) = (s.bone(LOWER_ARM[side]), s.bone(HAND[side]));
            if wrist_strain(s, &model, lower, hand, side) > 5.0 {
                out += 1;
            }
        }
    }
    out
}

/// How a line's clip is built: the fist's path moved by `shift` (m), the wrist given
/// `give` of its strain, the fists drawn `pull` thirds of the way in toward the shoulder and
/// the elbow leading the cut by `elbow` (−1 trailing to 1 leading) and the cut led by its
/// true `edge` (1) or false (−1; cuts only). Found by [`plan_cut`] and [`plan_stab`] and kept in [`LINE_PLANS`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinePlan {
    pub shift: Vec3,
    pub give: f32,
    pub pull: f32,
    pub elbow: f32,
    pub edge: f32,
}

/// The search for a line's plan: the cut's fist moved until the blade, turned as the wrists
/// let it, crosses the line's point going its way with every fist on its hold; of those, the
/// one that strains the wrists least (samples out, then centimeters off). Building a clip
/// takes a few ms and the search hundreds of them, so it runs offline (`cargo test -p
/// engine_anim --lib -- --ignored print_line_plans`) and its answers sit in [`LINE_PLANS`].
pub fn plan_cut(s: &Skeleton, two_handed: bool, angle: f32, height: Height) -> LinePlan {
    let length = if two_handed {
        GREATSWORD_BLADE[1]
    } else {
        SWORD_BLADE[1]
    };
    let point = line_point(height, two_handed);
    let travel = line_travel(angle);
    let (_, span) = line_timing(two_handed, false);
    // A low cut takes the shin with the blade's last hand, past the 85% mark.
    let close = if height == Height::Low { 0.2 } else { 0.15 };
    let mut nearest: Option<(f32, LinePlan)> = None;
    let mut best: Option<(f32, LinePlan)> = None;
    'search: for (edge, elbow) in [1.0, -1.0]
        .into_iter()
        .flat_map(|e| [(e, 1.0), (e, 0.0), (e, -1.0)])
    {
        for pull in [0.0, 0.5, 1.0, 1.5, 2.0] {
            for give in [1.0, 0.8, 0.6, 0.4, 0.2, 0.0] {
                let mut shift = Vec3::ZERO;
                for _ in 0..10 {
                    let plan = LinePlan {
                        shift,
                        give,
                        pull,
                        elbow,
                        edge,
                    };
                    let (clip, short) = line_cut_with(s, two_handed, angle, height, plan);
                    // The left fist off the grip counts as short of its hold.
                    let short = if two_handed {
                        short.max(grip_gap(s, &clip) - 0.02)
                    } else {
                        short
                    };
                    let (gap, along) = blade_gap(s, &clip, length * 0.85, point, span, travel);
                    let wrong = if along < 0.3 { 1.0 } else { 0.0 };
                    if short <= 0.004 && gap.length() + wrong < close {
                        let score = wrist_out(s, &clip) as f32 + gap.length() * 30.0;
                        if best.is_none_or(|(b, _)| score < b) {
                            best = Some((score, plan));
                        }
                        if score < 1.0 {
                            break 'search;
                        }
                    } else {
                        let off = gap.length() + wrong + short * 100.0;
                        if nearest.is_none_or(|(n, _)| off < n) {
                            nearest = Some((off, plan));
                        }
                    }
                    shift = (shift + gap * 0.5).clamp_length_max(0.6);
                }
            }
        }
    }
    best.or(nearest).expect("a cut").1
}

/// The search for a stab's plan: the fist moved until the point, turned as the wrists let
/// it, goes in where aimed with the fist on its hold; the wrists get all of their strain,
/// then less (see [`plan_cut`]).
pub fn plan_stab(s: &Skeleton, two_handed: bool, height: Height) -> LinePlan {
    let at = line_point(height, two_handed);
    let mut nearest: Option<(f32, LinePlan)> = None;
    let mut best: Option<(f32, LinePlan)> = None;
    'search: for give in [1.0, 0.8, 0.6, 0.4, 0.2, 0.0] {
        let mut shift = Vec3::ZERO;
        for _ in 0..8 {
            let plan = LinePlan {
                shift,
                give,
                pull: 0.0,
                elbow: 1.0,
                edge: 1.0,
            };
            let (clip, short) = line_stab_with(s, two_handed, height, plan);
            let short = if two_handed {
                short.max(grip_gap(s, &clip) - 0.02)
            } else {
                short
            };
            let gap = at - stab_tip(s, &clip, two_handed);
            if short <= 0.004 && gap.length() < 0.08 {
                let score = wrist_out(s, &clip) as f32 + gap.length() * 30.0;
                if best.is_none_or(|(b, _)| score < b) {
                    best = Some((score, plan));
                }
                if score < 1.0 {
                    break 'search;
                }
            } else {
                let off = gap.length() + short * 100.0;
                if nearest.is_none_or(|(n, _)| off < n) {
                    nearest = Some((off, plan));
                }
            }
            shift = (shift + gap * 0.8).clamp_length_max(0.6);
        }
    }
    best.or(nearest).expect("a stab").1
}

/// A line's plan from [`LINE_PLANS`] (none moved, all strain given, when it isn't there).
fn line_plan(name: &str) -> LinePlan {
    LINE_PLANS.iter().find(|(n, _)| *n == name).map_or(
        LinePlan {
            shift: Vec3::ZERO,
            give: 1.0,
            pull: 0.0,
            elbow: 1.0,
            edge: 1.0,
        },
        |(_, [x, y, z, give, pull, elbow, edge])| LinePlan {
            shift: Vec3::new(*x, *y, *z),
            give: *give,
            pull: *pull,
            elbow: *elbow,
            edge: *edge,
        },
    )
}

fn line_cut(s: &Skeleton, two_handed: bool, angle: f32, height: Height) -> AnimationClip {
    let set = if two_handed { "greatsword" } else { "sword" };
    let plan = line_plan(&line_clip(set, angle, height));
    line_cut_with(s, two_handed, angle, height, plan).0
}

/// How far a stab's edge is turned about its point from facing down (the fist thumb-up, as
/// a handshake), degrees: the turn that leaves the wrists in range, found by trying every
/// 30° (edge up with both hands; with one, up for the head and turned out from up below).
fn stab_turn(two_handed: bool, height: Height) -> f32 {
    match (two_handed, height) {
        (false, Height::High) => 180.0,
        (false, _) => 150.0,
        (true, Height::Mid) => 210.0,
        (true, _) => 180.0,
    }
}

/// Which way a stab's edge faces, the blade along `point`.
fn stab_face(two_handed: bool, height: Height, point: Vec3) -> Vec3 {
    Quat::from_axis_angle(point, stab_turn(two_handed, height).to_radians())
        * square(Vec3::NEG_Y, point)
}

/// Where a stab's point is as it goes in (model space).
fn stab_tip(s: &Skeleton, clip: &AnimationClip, two_handed: bool) -> Vec3 {
    let (_, [_, drive]) = line_timing(two_handed, true);
    let length = if two_handed {
        GREATSWORD_BLADE[1]
    } else {
        SWORD_BLADE[1]
    };
    let binding = crate::clip::ClipBinding::new(clip, s);
    let mut local = s.rest.clone();
    clip.sample(drive + 0.04, &binding, &mut local);
    let mut model = s.rest_model.clone();
    s.to_model(&local, &mut model);
    let hand = model[s.bone("hand_r")];
    hand.transform_point(GRIP) + hand.r * BLADE_AXIS * length
}

fn line_stab(s: &Skeleton, two_handed: bool, height: Height) -> AnimationClip {
    let set = if two_handed { "greatsword" } else { "sword" };
    line_stab_with(s, two_handed, height, line_plan(&stab_clip(set, height))).0
}

/// An aimed stab at `height`: drawn back by the hip, the point driven at the foe's middle
/// at that height with a step, and back.
fn line_stab_with(
    s: &Skeleton,
    two_handed: bool,
    height: Height,
    plan: LinePlan,
) -> (AnimationClip, f32) {
    let LinePlan { shift, give, .. } = plan;
    let (secs, [wound, drive]) = line_timing(two_handed, true);
    let face = stab_face(
        two_handed,
        height,
        (line_point(height, two_handed) - line_contact(height, two_handed)).normalize(),
    );
    let contact = line_contact(height, two_handed);
    let at = line_point(height, two_handed);
    let point = (at - contact).normalize();
    let blade = if two_handed {
        GREATSWORD_BLADE[1]
    } else {
        SWORD_BLADE[1]
    };
    // The fist a hand short of where the point goes in, a little past the foe's front.
    let fist = at + Vec3::new(0.0, 0.0, -0.12) - point * blade;
    let fist = Vec3::new(fist.x + 0.06, fist.y.clamp(0.9, 1.5), fist.z) + shift;
    let drawn = Hold::new(
        Vec3::new(0.24, contact.y.clamp(1.0, 1.25), 0.02),
        point,
        face,
        Vec3::new(0.6, -0.6, 0.6),
    );
    let home = Hold::new(fist, point, face, Vec3::new(0.6, -0.8, 0.1));
    let low = if height == Height::Low { 1.0 } else { 0.0 };
    let base = if two_handed { GS_STANCE } else { STANCE };
    let lunge = base
        .hips(
            [0.0, -0.14 - 0.1 * low, -0.22],
            [6.0 + 10.0 * low, base.turn.y + 8.0, 0.0],
            [base.trunk.x + 10.0 + 8.0 * low, base.trunk.y, 0.0],
        )
        .foot(
            0,
            foot(
                base.feet[0].at.x,
                base.feet[0].at.z - 0.26,
                base.feet[0].yaw,
            ),
        )
        .foot(1, base.feet[1].heel(14.0));
    let drawn_body = base.hips(
        [0.0, -0.1, 0.06],
        [2.0, base.turn.y - 22.0, 0.0],
        [base.trunk.x, base.trunk.y - 8.0, 0.0],
    );
    let (rigs, keys) = if two_handed {
        let grip = Stroke::Below(GREATSWORD_HANDS);
        (
            [SWORD, SWORD],
            vec![
                key(0.0, GS_STANCE, grip, Stroke::To(gs_ready())),
                key(wound, drawn_body, grip, Stroke::To(drawn)),
                key(drive, lunge, grip, Stroke::To(home)),
                key(drive + 0.08, lunge, grip, Stroke::To(home)),
                key(secs, GS_STANCE, grip, Stroke::To(gs_ready())),
            ],
        )
    } else {
        let aside = shield_low().at(Vec3::new(-0.24, 1.1, -0.28));
        (
            [HANDLE, SWORD],
            vec![
                key(
                    0.0,
                    STANCE,
                    Stroke::To(shield_low()),
                    Stroke::To(sword_ready()),
                ),
                key(
                    wound,
                    drawn_body,
                    Stroke::To(shield_low()),
                    Stroke::To(drawn),
                ),
                key(drive, lunge, Stroke::To(aside), Stroke::To(home)),
                key(drive + 0.08, lunge, Stroke::To(aside), Stroke::To(home)),
                key(
                    secs,
                    STANCE,
                    Stroke::To(shield_low()),
                    Stroke::To(sword_ready()),
                ),
            ],
        )
    };
    let set = if two_handed { "greatsword" } else { "sword" };
    let (clip, short) =
        Moves::new(rigs, keys)
            .give(give)
            .clip_reaching(s, &stab_clip(set, height), false);
    let clip = clip
        .with_curve("effort", 31, move |ph| {
            keyed(
                ph * secs,
                &[(0.0, 0.2), (wound, 0.6), (drive, 1.0), (secs, 0.15)],
            )
        })
        .with_curve("grip", 2, move |_| if two_handed { 1.0 } else { 0.0 })
        .with_event(wound / secs, "swing", Some("hand_r"));
    (clip, short)
}

/// Each aimed line's [`LinePlan`] as [`plan_cut`] and [`plan_stab`] found it: clip, then
/// shift x, y, z (m), give, pull, elbow and edge.
#[rustfmt::skip]
const LINE_PLANS: &[(&str, [f32; 7])] = &[
    ("sword_line_000_high", [0.079, 0.052, -0.079, 1.0, 0.0, 1.0, 1.0]),
    ("sword_line_045_high", [0.040, 0.015, -0.030, 1.0, 0.0, 1.0, 1.0]),
    ("sword_line_090_high", [0.050, -0.020, -0.017, 1.0, 0.0, 1.0, 1.0]),
    ("sword_line_135_high", [0.029, -0.048, -0.033, 1.0, 0.0, 1.0, 1.0]),
    ("sword_line_180_high", [0.171, 0.045, -0.148, 1.0, 0.0, 1.0, 1.0]),
    ("sword_line_225_high", [0.099, 0.026, -0.198, 1.0, 0.5, 0.0, 1.0]),
    ("sword_line_270_high", [0.015, -0.111, -0.083, 1.0, 0.0, 1.0, 1.0]),
    ("sword_line_315_high", [-0.040, -0.054, -0.320, 1.0, 2.0, 1.0, 1.0]),
    ("sword_stab_high", [-0.281, -0.002, 0.109, 1.0, 0.0, 1.0, 1.0]),
    ("sword_line_000_mid", [-0.031, -0.004, -0.007, 1.0, 0.0, 1.0, 1.0]),
    ("sword_line_045_mid", [0.004, 0.014, -0.009, 1.0, 0.0, 1.0, 1.0]),
    ("sword_line_090_mid", [0.052, 0.037, -0.012, 1.0, 0.0, 1.0, 1.0]),
    ("sword_line_135_mid", [-0.007, 0.027, -0.003, 1.0, 0.0, 1.0, 1.0]),
    ("sword_line_180_mid", [0.049, 0.172, -0.231, 1.0, 1.0, -1.0, 1.0]),
    ("sword_line_225_mid", [0.112, 0.068, -0.225, 1.0, 1.5, 0.0, 1.0]),
    ("sword_line_270_mid", [-0.005, -0.051, -0.033, 0.8, 0.0, 1.0, 1.0]),
    ("sword_line_315_mid", [0.023, -0.049, -0.125, 1.0, 1.0, 0.0, 1.0]),
    ("sword_stab_mid", [-0.172, -0.004, 0.117, 1.0, 0.0, 1.0, 1.0]),
    ("sword_line_000_low", [0.076, 0.001, -0.156, 1.0, 0.0, 0.0, 1.0]),
    ("sword_line_045_low", [0.015, -0.063, -0.164, 1.0, 1.5, 1.0, 1.0]),
    ("sword_line_090_low", [0.001, -0.033, -0.072, 1.0, 1.0, -1.0, 1.0]),
    ("sword_line_135_low", [0.000, 0.000, 0.000, 0.8, 1.0, 0.0, 1.0]),
    ("sword_line_180_low", [-0.041, -0.053, -0.099, 0.0, 2.0, 1.0, 1.0]),
    ("sword_line_225_low", [0.000, 0.000, 0.000, 0.0, 1.5, 1.0, 1.0]),
    ("sword_line_270_low", [0.000, 0.000, 0.000, 0.8, 1.5, 1.0, 1.0]),
    ("sword_line_315_low", [0.000, 0.000, 0.000, 1.0, 0.5, -1.0, 1.0]),
    ("sword_stab_low", [-0.106, -0.025, 0.132, 1.0, 0.0, 1.0, 1.0]),
    ("greatsword_line_000_high", [-0.113, 0.009, -0.221, 1.0, 1.0, 1.0, 1.0]),
    ("greatsword_line_045_high", [0.050, -0.015, -0.125, 0.8, 0.5, 1.0, 1.0]),
    ("greatsword_line_090_high", [0.000, -0.011, -0.226, 1.0, 1.5, -1.0, 1.0]),
    ("greatsword_line_135_high", [0.009, -0.103, -0.127, 1.0, 0.5, -1.0, 1.0]),
    ("greatsword_line_180_high", [-0.013, 0.041, -0.081, 0.0, 0.0, -1.0, 1.0]),
    ("greatsword_line_225_high", [-0.000, -0.096, -0.255, 0.2, 1.5, 0.0, 1.0]),
    ("greatsword_line_270_high", [0.080, -0.001, -0.313, 1.0, 2.0, 1.0, 1.0]),
    ("greatsword_line_315_high", [0.011, 0.134, -0.154, 1.0, 0.5, 0.0, 1.0]),
    ("greatsword_stab_high", [-0.060, -0.000, 0.120, 0.0, 0.0, 1.0, 1.0]),
    ("greatsword_line_000_mid", [0.087, 0.076, -0.176, 1.0, 0.5, 0.0, 1.0]),
    ("greatsword_line_045_mid", [0.014, -0.019, -0.084, 1.0, 0.5, 1.0, 1.0]),
    ("greatsword_line_090_mid", [-0.046, -0.020, -0.173, 1.0, 1.5, -1.0, 1.0]),
    ("greatsword_line_135_mid", [-0.036, -0.011, -0.029, 1.0, 0.5, 0.0, 1.0]),
    ("greatsword_line_180_mid", [-0.058, -0.039, -0.155, 0.0, 1.5, 1.0, 1.0]),
    ("greatsword_line_225_mid", [0.070, -0.030, -0.113, 0.2, 0.5, 1.0, 1.0]),
    ("greatsword_line_270_mid", [0.019, -0.074, -0.070, 0.8, 0.0, 1.0, 1.0]),
    ("greatsword_line_315_mid", [-0.024, -0.111, -0.169, 1.0, 1.0, -1.0, 1.0]),
    ("greatsword_stab_mid", [-0.060, -0.000, 0.120, 0.0, 0.0, 1.0, 1.0]),
    ("greatsword_line_000_low", [-0.025, -0.011, -0.136, 0.8, 0.0, 1.0, 1.0]),
    ("greatsword_line_045_low", [0.000, 0.000, 0.000, 1.0, 1.0, 0.0, 1.0]),
    ("greatsword_line_090_low", [0.000, 0.000, 0.000, 0.0, 2.0, -1.0, 1.0]),
    ("greatsword_line_135_low", [0.000, 0.000, 0.000, 0.0, 1.5, 1.0, 1.0]),
    ("greatsword_line_180_low", [-0.041, -0.022, -0.092, 0.0, 2.0, 1.0, 1.0]),
    ("greatsword_line_225_low", [0.000, 0.000, 0.000, 0.0, 1.5, 1.0, 1.0]),
    ("greatsword_line_270_low", [0.000, 0.000, 0.000, 0.2, 1.5, -1.0, 1.0]),
    ("greatsword_line_315_low", [0.000, 0.000, 0.000, 1.0, 1.0, -1.0, 1.0]),
    ("greatsword_stab_low", [-0.058, 0.000, 0.115, 0.0, 0.0, 1.0, 1.0]),
];

/// Every aimed cut and stab, one-handed (sword and shield) and with the great sword.
fn line_clips(s: &Skeleton) -> Vec<AnimationClip> {
    let mut all = Vec::new();
    for two_handed in [false, true] {
        for height in Height::ALL {
            for angle in LINE_ANGLES {
                all.push(line_cut(s, two_handed, angle, height));
            }
            all.push(line_stab(s, two_handed, height));
        }
    }
    all
}

/// Every weapon clip.
pub fn clips(s: &Skeleton) -> Vec<AnimationClip> {
    let mut all = vec![
        sword_slash(s),
        sword_backhand(s),
        sword_thrust(s),
        sword_overhead(s),
        shield_bash(s),
        sword_stance(s),
        shield_guard(s),
        shield_deflect(s),
        guard_broken(s),
        sword_carry(s),
        greatsword_sweep(s),
        greatsword_rise(s),
        greatsword_overhead(s),
        greatsword_stance(s),
        greatsword_carry(s),
        sword_riposte(s),
        greatsword_riposte(s),
        bow_shot_clip(s, WeaponMove::BowShot),
        bow_shot_clip(s, WeaponMove::BowShotFull),
        bow_carry(s),
    ];
    all.extend(bow_draw_clips(s));
    all.extend(rifle_clips(s));
    all.extend(moving_stances(s));
    all.extend(line_clips(s));
    all
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::humanoid;

    /// Model-space pose of `clip` at `t` seconds.
    fn model_at(s: &Skeleton, clip: &AnimationClip, t: f32) -> Vec<Xf> {
        let binding = crate::clip::ClipBinding::new(clip, s);
        let mut local = s.rest.clone();
        clip.sample(t, &binding, &mut local);
        let mut model = s.rest_model.clone();
        s.to_model(&local, &mut model);
        model
    }

    /// The right fist and the blade's tip (model space) at `t`.
    fn blade(s: &Skeleton, clip: &AnimationClip, t: f32, length: f32) -> (Vec3, Vec3) {
        let m = model_at(s, clip, t);
        let hand = m[s.bone("hand_r")];
        let fist = hand.transform_point(GRIP);
        (fist, fist + hand.r * BLADE_AXIS * length)
    }

    /// How deep a sword's blade cuts into the bearer's own shield at `t` (m: how far inside
    /// the rim the blade crosses the shield's face, 0 if it passes clear).
    fn blade_in_shield(s: &Skeleton, clip: &AnimationClip, t: f32) -> f32 {
        let m = model_at(s, clip, t);
        let hand = m[s.bone("hand_r")];
        let fist = hand.transform_point(GRIP);
        let axis = hand.r * BLADE_AXIS;
        let (a, b) = (fist + axis * SWORD_BLADE[0], fist + axis * SWORD_BLADE[1]);
        let left = m[s.bone("hand_l")];
        let center = left.transform_point(SHIELD_CENTER);
        let normal = (left.r * HANDLE_FACE).normalize();
        blade_through_shield(a, b, center, normal)
    }

    #[test]
    fn a_sword_never_cuts_through_its_own_shield() {
        let s = humanoid::skeleton();
        let mut report = Vec::new();
        for clip in [
            sword_slash(&s),
            sword_backhand(&s),
            sword_thrust(&s),
            sword_overhead(&s),
            shield_bash(&s),
            sword_stance(&s),
            shield_guard(&s),
            shield_deflect(&s),
            sword_riposte(&s),
        ] {
            let steps = (clip.duration * 60.0).ceil() as usize;
            let mut worst = (0.0f32, 0.0f32);
            for k in 0..=steps {
                let t = clip.duration * k as f32 / steps.max(1) as f32;
                let d = blade_in_shield(&s, &clip, t);
                if d > worst.0 {
                    worst = (d, t);
                }
            }
            report.push(format!(
                "{}: {:.3} m at {:.2} s",
                clip.name, worst.0, worst.1
            ));
        }
        eprintln!("{}", report.join("\n"));
        assert!(
            report.iter().all(|r| r.contains(": 0.000")),
            "{}",
            report.join("\n")
        );
    }

    #[test]
    fn the_solver_puts_the_fist_and_blade_where_the_key_asks() {
        let s = humanoid::skeleton();
        let ready = sword_ready();
        let shield = shield_low();
        let pose = solve(
            &s,
            [HANDLE, SWORD],
            &STANCE,
            &[shield, ready],
            &mut ArmMemo::default(),
        );
        let mut local = s.rest.clone();
        for (bone, x) in pose {
            let i = s.bone(bone);
            local[i].r = x.r;
            if x.t != Vec3::ZERO {
                local[i].t = x.t;
            }
        }
        let mut m = s.rest_model.clone();
        s.to_model(&local, &mut m);
        let hand = m[s.bone("hand_r")];
        assert!(hand.transform_point(GRIP).distance(ready.at) < 0.01);
        assert!((hand.r * BLADE_AXIS).dot(ready.axis) > 0.999);
        let left = m[s.bone("hand_l")];
        assert!(left.transform_point(GRIP).distance(shield.at) < 0.01);
        assert!((left.r * HANDLE_FACE).dot(shield.face) > 0.999);
        // The feet are where the stance put them, flat.
        for side in 0..2 {
            let ankle = m[s.bone(FOOT[side])].t;
            assert!(
                ankle.distance(STANCE.feet[side].at) < 0.01,
                "{side}: {ankle} vs {}",
                STANCE.feet[side].at
            );
        }
    }

    thread_local! {
        /// Where a held weapon's fist fell short of its hold while clips were built: the
        /// clip and time being sampled, the hand and how far.
        static MISSES: std::cell::RefCell<Vec<(String, f32, usize, f32)>> =
            const { std::cell::RefCell::new(Vec::new()) };
        static SAMPLING: std::cell::RefCell<(String, f32)> =
            const { std::cell::RefCell::new((String::new(), 0.0)) };
    }

    pub(super) fn sampling(name: &str, t: f32) {
        SAMPLING.with(|s| *s.borrow_mut() = (name.to_string(), t));
    }

    pub(super) fn note_miss(side: usize, miss: f32) {
        if miss > 0.005 {
            let (name, t) = SAMPLING.with(|s| s.borrow().clone());
            MISSES.with(|m| m.borrow_mut().push((name, t, side, miss)));
        }
    }

    #[test]
    fn every_weapon_clip_keeps_the_fists_on_the_weapon_and_the_feet_down() {
        let s = humanoid::skeleton();
        MISSES.with(|m| m.borrow_mut().clear());
        let all = clips(&s);
        let misses = MISSES.with(|m| m.borrow().clone());
        assert!(
            misses.is_empty(),
            "fists fell short of where keys held the weapon (clip, s, hand, m): {misses:?}"
        );
        for clip in all {
            let steps = 60;
            for k in 0..=steps {
                let t = clip.duration * k as f32 / steps as f32;
                let m = model_at(&s, &clip, t);
                for side in 0..2 {
                    // Feet never sink, and stand back on the ground at the ends.
                    let ankle = m[s.bone(FOOT[side])].t;
                    assert!(
                        ankle.y > ANKLE - 0.01,
                        "{} at {t:.2}: ankle {ankle}",
                        clip.name
                    );
                }
                // A two-handed sword keeps both fists on its grip.
                if clip.name.starts_with("greatsword") && clip.name != "greatsword_carry" {
                    let r = m[s.bone("hand_r")];
                    let l = m[s.bone("hand_l")];
                    let want = r.transform_point(GRIP) - r.r * BLADE_AXIS * GREATSWORD_HANDS;
                    let gap = l.transform_point(GRIP).distance(want);
                    assert!(
                        gap < 0.03,
                        "{} at {t:.2}: left fist {gap} m off the grip",
                        clip.name
                    );
                }
            }
        }
    }

    #[test]
    fn cuts_sweep_one_way_and_peak_late() {
        let s = humanoid::skeleton();
        let all = clips(&s);
        for m in [
            WeaponMove::SwordSlash,
            WeaponMove::SwordBackhand,
            WeaponMove::SwordOverhead,
            WeaponMove::GreatswordSweep,
            WeaponMove::GreatswordRise,
            WeaponMove::GreatswordOverhead,
        ] {
            let clip = all.iter().find(|c| c.name == m.clip_name()).unwrap();
            let (_, [a, b]) = m.timing();
            let n = 40;
            let mut speeds = Vec::new();
            let mut was = blade(&s, clip, a, 0.86).1;
            for k in 1..=n {
                let t = a + (b - a) * k as f32 / n as f32;
                let tip = blade(&s, clip, t, 0.86).1;
                speeds.push(tip.distance(was) / ((b - a) / n as f32));
                was = tip;
            }
            let (peak_at, peak) =
                speeds.iter().enumerate().fold(
                    (0, 0.0f32),
                    |best, (k, &v)| if v > best.1 { (k, v) } else { best },
                );
            let at = peak_at as f32 / n as f32;
            assert!(
                (0.5..0.9).contains(&at),
                "{}: the tip peaks {at:.2} through the cut",
                m.clip_name()
            );
            // A real cut: the tip moves at many meters a second.
            assert!(
                peak > 8.0,
                "{}: the tip peaks at {peak:.1} m/s",
                m.clip_name()
            );
        }
    }

    #[test]
    fn the_draw_hand_anchors_under_the_jaw_at_full_draw() {
        let s = humanoid::skeleton();
        let all = clips(&s);
        let clip = all.iter().find(|c| c.name == "bow_shot").unwrap();
        let (_, [release, _]) = WeaponMove::BowShot.timing();
        let m = model_at(&s, clip, release - 0.05);
        let hand = m[s.bone("hand_r")];
        let lower = m[s.bone("lower_arm_r")].t;
        let fist = hand.t + (hand.t - lower).normalize() * -GRIP.y;
        assert!(fist.distance(ANCHOR) < 0.04, "fist {fist}");
        let head = m[s.bone("head")].t;
        assert!(
            fist.y < head.y && fist.y > head.y - 0.2,
            "fist {fist} head {head}"
        );
        // The draw elbow back behind the anchor, near shoulder height.
        let elbow = lower;
        let shoulder = m[s.bone("upper_arm_r")].t;
        assert!(elbow.z > ANCHOR.z + 0.2, "elbow {elbow}");
        assert!(
            (elbow.y - shoulder.y).abs() < 0.12,
            "elbow {elbow} shoulder {shoulder}"
        );
        assert!(clip.curve("draw", release - 0.05).unwrap() > 0.99);
        assert!(clip.curve("draw", release + 0.02).unwrap() < 0.01);
    }
}

#[cfg(test)]
mod wrists {
    use super::*;
    use crate::build::humanoid;

    /// Every weapon clip's wrists, sampled at 60 Hz, stay in their range (5° slack, the
    /// validator's) but for a few samples where a sword would otherwise cut its own shield or
    /// an off hand follows its grip round, and the paired thrusts (held exactly, not counted);
    /// the 54 aimed cuts and stabs, held to their marks, have their own budget: the sword's
    /// high and mid ones next to none, the low ones and the great sword's (whose left hand
    /// must stay on the grip) still bend past range for about a third of each clip, measured
    /// here so it can only fall; the table gives the samples out, the worst strain
    /// and the worst flexion, extension, deviation and roll per hand.
    #[test]
    fn weapon_clips_keep_their_wrists_in_range() {
        let s = humanoid::skeleton();
        let mut lines = Vec::new();
        let (mut total, mut aimed, mut sword_upper) = (0, 0, 0);
        for clip in clips(&s) {
            let binding = crate::clip::ClipBinding::new(&clip, &s);
            let n = ((clip.duration * 60.0).ceil() as usize).max(1);
            let mut out = [0usize; 2];
            let mut worst = [0.0f32; 2];
            let mut angles = [[0.0f32; 4]; 2];
            for k in 0..=n {
                let t = clip.duration * k as f32 / n as f32;
                let mut local = s.rest.clone();
                clip.sample(t, &binding, &mut local);
                let mut model = s.rest_model.clone();
                s.to_model(&local, &mut model);
                for side in 0..2 {
                    let strain = wrist_strain(
                        &s,
                        &model,
                        s.bone(LOWER_ARM[side]),
                        s.bone(HAND[side]),
                        side,
                    );
                    if strain > 5.0 {
                        out[side] += 1;
                    }
                    worst[side] = worst[side].max(strain);
                    let [f, d, r] = crate::validate::wrist_angles(
                        &s,
                        &model,
                        s.bone(LOWER_ARM[side]),
                        s.bone(HAND[side]),
                        side,
                    );
                    angles[side][0] = angles[side][0].max(f);
                    angles[side][1] = angles[side][1].max(-f);
                    angles[side][2] = angles[side][2].max(d);
                    angles[side][3] = angles[side][3].max(r);
                }
            }
            // The paired thrusts are held exactly where their foes take them; the aimed
            // cuts and stabs, which must cross their mark, have their own budget.
            if clip.name.contains("_line_") || clip.name.contains("_stab_") {
                aimed += out[0] + out[1];
                if clip.name.starts_with("sword") && !clip.name.ends_with("_low") {
                    sword_upper += out[0] + out[1];
                }
            } else if clip.name != SWORD_RIPOSTE && clip.name != GREATSWORD_RIPOSTE {
                total += out[0] + out[1];
            }
            lines.push(format!(
                "{:28} L {:3}/{:3} {:5.1}° {:3.0?}  R {:3}/{:3} {:5.1}° {:3.0?}",
                clip.name,
                out[0],
                n + 1,
                worst[0],
                angles[0],
                out[1],
                n + 1,
                worst[1],
                angles[1]
            ));
        }
        eprintln!("{}", lines.join("\n"));
        assert!(
            total <= 60 && aimed <= 440 && sword_upper <= 20,
            "{total} samples out of range, {aimed} in the aimed lines ({sword_upper} in the \
             sword's high and mid ones)\n{}",
            lines.join("\n")
        );
    }
}

#[cfg(test)]
mod lines {
    use super::*;
    use crate::build::humanoid;

    /// Where the blade (hilt end, tip) is at `t`, model space.
    fn blade_at(s: &Skeleton, clip: &AnimationClip, t: f32, length: f32) -> (Vec3, Vec3) {
        let binding = crate::clip::ClipBinding::new(clip, s);
        let mut local = s.rest.clone();
        clip.sample(t, &binding, &mut local);
        let mut model = s.rest_model.clone();
        s.to_model(&local, &mut model);
        let hand = model[s.bone("hand_r")];
        let fist = hand.transform_point(GRIP);
        (fist, fist + hand.r * BLADE_AXIS * length)
    }

    /// Closest distance from `p` to the segment `a`–`b`, and how far along it.
    fn closest(a: Vec3, b: Vec3, p: Vec3) -> (f32, f32) {
        let ab = b - a;
        let u = ((p - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
        ((a + ab * u).distance(p), u)
    }

    /// Prints [`LINE_PLANS`] from the searches, to paste over it after a change to how
    /// lines are built.
    #[test]
    #[ignore = "a tool: regenerates LINE_PLANS"]
    fn print_line_plans() {
        let s = humanoid::skeleton();
        let mut rows = Vec::new();
        for two_handed in [false, true] {
            let set = if two_handed { "greatsword" } else { "sword" };
            for height in Height::ALL {
                let mut plans: Vec<(String, LinePlan)> = LINE_ANGLES
                    .iter()
                    .map(|&a| {
                        (
                            line_clip(set, a, height),
                            plan_cut(&s, two_handed, a, height),
                        )
                    })
                    .collect();
                plans.push((stab_clip(set, height), plan_stab(&s, two_handed, height)));
                for (name, p) in plans {
                    rows.push(format!(
                        "    (\"{name}\", [{:.3}, {:.3}, {:.3}, {:.1}, {:.1}, {:.1}, {:.1}]),",
                        p.shift.x, p.shift.y, p.shift.z, p.give, p.pull, p.elbow, p.edge
                    ));
                }
            }
        }
        println!("LINE_PLANS\n{}", rows.join("\n"));
    }

    /// Every aimed cut's blade sweeps within 20 cm of its line's point at its height, moving
    /// within 75° of its line there (the wrists turn some lines off theirs); every stab's
    /// point goes in at its height.
    #[test]
    fn aimed_cuts_cross_their_point_along_their_line() {
        let s = humanoid::skeleton();
        let all = clips(&s);
        let mut report = Vec::new();
        let mut bad = Vec::new();
        for two_handed in [false, true] {
            let set = if two_handed { "greatsword" } else { "sword" };
            let length = if two_handed {
                GREATSWORD_BLADE[1]
            } else {
                SWORD_BLADE[1]
            };
            for height in Height::ALL {
                let target = line_point(height, two_handed);
                for angle in LINE_ANGLES {
                    let name = line_clip(set, angle, height);
                    let clip = all.iter().find(|c| c.name == name).expect(&name);
                    let (_, [a, b]) = line_timing(two_handed, false);
                    let mut best = (f32::INFINITY, Vec3::ZERO, 0.0);
                    let n = 60;
                    let mut was = blade_at(&s, clip, a, length);
                    for k in 1..=n {
                        let t = a + (b - a) * k as f32 / n as f32;
                        let now = blade_at(&s, clip, t, length);
                        let (d, u) = closest(now.0, now.1, target);
                        if d < best.0 {
                            let p_now = now.0.lerp(now.1, u);
                            let p_was = was.0.lerp(was.1, u);
                            best = (d, p_now - p_was, t);
                        }
                        was = now;
                    }
                    let travel = line_travel(angle);
                    let motion = best.1.with_z(0.0).normalize_or_zero();
                    let along = motion.dot(travel);
                    report.push(format!(
                        "{name}: {:.2} m at {:.2} s, along {along:.2}",
                        best.0, best.2
                    ));
                    if best.0 > 0.2 || along < 0.25 {
                        bad.push(name);
                    }
                }
                let name = stab_clip(set, height);
                let clip = all.iter().find(|c| c.name == name).expect(&name);
                let (_, [_, drive]) = line_timing(two_handed, true);
                let tip = blade_at(&s, clip, drive + 0.04, length).1;
                let off = (tip - target).with_z(0.0).length();
                report.push(format!("{name}: tip {off:.2} m off at {:.2}", tip.y));
                if off > 0.15 || tip.z > target.z + 0.05 {
                    bad.push(name);
                }
            }
        }
        eprintln!("{}", report.join("\n"));
        assert!(bad.is_empty(), "{bad:?}\n{}", report.join("\n"));
    }
}
