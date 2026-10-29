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
use crate::ik::two_bone;
use crate::skeleton::Skeleton;
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
    /// eases to `elbow`.
    Cut {
        axis: Vec3,
        degrees: f32,
        to: Vec3,
        reach: f32,
        elbow: Vec3,
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
                        ..
                    } => {
                        assert!(k > 0, "a weapon clip can't start on a cut");
                        let axis = axis.normalize();
                        // The edge leads: it faces the way the tip moves.
                        let from = &mut holds[k - 1][side];
                        from.face = square(axis.cross(from.axis), from.axis);
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
        Self { rigs, keys, holds }
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
    fn pose(&self, s: &Skeleton, t: f32) -> Vec<(&'static str, Xf)> {
        let (body, hands) = self.sample(t);
        solve(s, self.rigs, &body, &hands)
    }

    /// The clip, sampled 120 times a second (a cut turns the blade 20° in a 60th).
    fn clip(self, s: &Skeleton, name: &str, looping: bool) -> AnimationClip {
        let secs = self.secs();
        let keys = (secs * 120.0).ceil() as usize + 1;
        let trail_keys = keys;
        let trail: Vec<f32> = (0..trail_keys)
            .map(|k| self.cutting(k as f32 / (trail_keys - 1) as f32 * secs))
            .collect();
        let clip = AnimationClip::from_fn(name, secs, keys, looping, |ph| {
            #[cfg(test)]
            tests::sampling(name, ph * secs);
            self.pose(s, ph * secs)
        });
        clip.with_curve("trail", trail_keys, move |ph| {
            trail[((ph * (trail_keys - 1) as f32).round() as usize).min(trail_keys - 1)]
        })
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

/// Pose the skeleton: the body by its angles, then legs and arms by two-bone IK onto the feet
/// and holds.
fn solve(s: &Skeleton, rigs: [Rig; 2], body: &Body, hands: &[Hold; 2]) -> Vec<(&'static str, Xf)> {
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
        two_bone(
            &mut edit,
            upper,
            lower,
            ankle,
            target,
            yaw * Vec3::new(out, 0.0, -1.0),
            1.0,
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
                let r = h.frame() * frame(axis, face).inverse();
                let _miss = two_bone(&mut edit, upper, lower, hand, h.at - r * GRIP, h.elbow, 1.0);
                #[cfg(test)]
                tests::note_miss(side, _miss);
                edit.set_model_rotation(hand, r);
            }
            Rig::Free => {
                // The fist is a hand's length past the wrist along a straight forearm.
                let mut wrist = h.at;
                for _ in 0..2 {
                    two_bone(&mut edit, upper, lower, hand, wrist, h.elbow, 1.0);
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
                Stroke::To(shield_low().at(Vec3::new(-0.2, 1.08, -0.28))),
                Stroke::Cut {
                    axis: Vec3::new(-0.85, 0.5, 0.0),
                    degrees: 215.0,
                    to: Vec3::new(-0.12, 1.02, -0.36),
                    reach: 1.12,
                    elbow: Vec3::new(0.5, -0.7, -0.5),
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
            // The sword across to the left hip, the trunk coiled to the left, the shield out
            // of its way.
            key(
                wound,
                STANCE.hips([0.0, -0.09, 0.02], [3.0, 20.0, 0.0], [2.0, 22.0, 0.0]),
                Stroke::To(shield_low().at(Vec3::new(-0.3, 1.15, -0.2))),
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
                Stroke::To(shield_low()),
                Stroke::Cut {
                    axis: Vec3::new(0.85, -0.5, 0.0),
                    degrees: 210.0,
                    to: Vec3::new(0.36, 1.5, -0.3),
                    reach: 1.1,
                    elbow: Vec3::new(1.0, -0.2, 0.1),
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
    let sword_back = sword_ready().at(Vec3::new(0.3, 1.05, -0.12));
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
                },
            ),
            key(secs, GS_STANCE, grip, Stroke::To(gs_ready())),
        ],
    )
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

/// Every weapon clip.
pub fn clips(s: &Skeleton) -> Vec<AnimationClip> {
    vec![
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
        bow_shot_clip(s, WeaponMove::BowShot),
        bow_shot_clip(s, WeaponMove::BowShotFull),
        bow_carry(s),
    ]
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

    #[test]
    fn the_solver_puts_the_fist_and_blade_where_the_key_asks() {
        let s = humanoid::skeleton();
        let ready = sword_ready();
        let shield = shield_low();
        let pose = solve(&s, [HANDLE, SWORD], &STANCE, &[shield, ready]);
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
