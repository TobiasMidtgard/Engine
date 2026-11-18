//! Lying prone and crawling on the belly: the low crawl soldiers are taught (FM 3-21.75,
//! "low crawl": body flat, pulled along by one forearm while the opposite knee, drawn up
//! beside the hip, pushes) for getting under low cover or along behind it. Callers set the
//! contract-style `prone` parameter to 0 or 1; the body goes down through hands and knees,
//! lies propped on its forearms, crawls ahead (and shuffles back), and comes up the same way.
//!
//! Every clip here lies on the floor, so its `floor` curve is 1 and foot IK lets go. The
//! crawl's planted forearm and knee stay put on the ground while the body slides over them
//! ([`PRONE_CRAWL_SPEED`] × half a cycle each pull).

use super::*;
use crate::build::weapons::{
    GRIP, RIFLE_AXIS, RIFLE_BUTT, RIFLE_FORE, RIFLE_MAG, RIFLE_SHOT_AT, RIFLE_UP,
};

/// The graph parameter the body lies down by (0 standing or crouched, 0.5 on hands and knees,
/// 1 flat on the floor). The module ramps it toward the contract-style `prone` (0 or 1)
/// over [`PRONE_SECS`].
pub const PRONED: &str = "proned";
/// How long going down (or getting up) takes, s.
pub const PRONE_SECS: f32 = 1.3;
/// Crawling ahead and back, m/s (a trained low crawl is about half a meter a second).
pub const PRONE_CRAWL_SPEED: f32 = 0.45;
pub const PRONE_BACK_SPEED: f32 = 0.25;
/// The clips (and nothing else uses the names).
pub const PRONE_IDLE: &str = "prone_idle";
pub const PRONE_CRAWL: &str = "prone_crawl";
pub const PRONE_BACK: &str = "prone_back";
/// Crawling ahead and back carrying a long gun.
pub const PRONE_RIFLE_CRAWL: &str = "prone_rifle_crawl";
pub const PRONE_RIFLE_BACK: &str = "prone_rifle_back";
pub const PRONE_KNEEL: &str = "prone_kneel";
/// Aiming a long gun lying down (the prone firing position), and a shot from it (the `shot`
/// event as it goes off, like [`RIFLE_FIRE`]'s).
pub const PRONE_RIFLE_AIM: &str = "prone_rifle_aim";
pub const PRONE_RIFLE_FIRE: &str = "prone_rifle_fire";
/// A magazine change lying down (the `mag_out` and `mag_in` events and the `mag_in_hand`
/// curve, like [`RIFLE_RELOAD`]'s).
pub const PRONE_RIFLE_RELOAD: &str = "prone_rifle_reload";
/// A rifleman lying still, not aiming: the gun in the shoulder, the muzzle down, looking
/// over the sights.
pub const PRONE_RIFLE_READY: &str = "prone_rifle_ready";
/// The graph parameter the lying body takes up the firing position by (0..1): the module
/// eases it toward 1 while a rifleman lying down (`rifle_carry` up, [`PRONED`] near 1) has
/// `aim` up.
pub const PRONE_AIMED: &str = "prone_aimed";

/// One crawl cycle: a pull on the left forearm and right knee, then on the right and left, s.
const CRAWL_SECS: f32 = 1.6;
/// Where the hips lie: just behind the spot the person stands on, the chest over it.
const HIPS: Vec3 = Vec3::new(0.0, 0.13, 0.42);
/// How far the body slides over a planted forearm and knee in one pull, m.
const PULL: f32 = PRONE_CRAWL_SPEED * CRAWL_SECS / 2.0;
/// The reaching wrist, and where the pull leaves it (the elbow back beside the ribs).
const REACHED: Vec3 = Vec3::new(0.16, 0.05, -0.64);
/// The drawn-up knee's ankle, out to the side, and the leg pushed straight.
const DRAWN: Vec3 = Vec3::new(0.3, 0.09, 0.92);
const STRAIGHT: Vec3 = Vec3::new(0.24, 0.07, 0.92 + PULL);

/// The lying body's joint angles: trunk flat, chest a little up on the forearms, chin up
/// to look ahead; `lift` (−1..1) rolls toward the side whose knee is drawn (+ right).
fn lying(lift: f32, arms: [Arm; 2], legs: [[f32; 4]; 2]) -> Key {
    Key {
        at: 0.0,
        hips: HIPS + Vec3::new(0.0, 0.015 * lift.abs(), 0.0),
        pitch: -82.0,
        yaw: 5.0 * lift,
        roll: -7.0 * lift,
        spine: [8.0, 14.0, 18.0, 14.0],
        arm: arms,
        leg: legs,
        plant: [None; 4],
        knee: [Vec3::NEG_Y; 2],
        effort: 0.6,
    }
}

/// A forearm along the floor ahead, elbow bent under the shoulder.
const FOREARM: Arm = Arm {
    flex: 150.0,
    twist: 0.0,
    abduct: 20.0,
    elbow: 70.0,
    wrist: 10.0,
    protract: 10.0,
    elevate: 8.0,
};
/// The arm reached out ahead, nearly straight.
const REACHING: Arm = Arm {
    flex: 165.0,
    twist: 0.0,
    abduct: 12.0,
    elbow: 20.0,
    wrist: 10.0,
    protract: 14.0,
    elevate: 12.0,
};
/// A leg lying straight back, toes dug in.
const LEG_STRAIGHT: [f32; 4] = [0.0, 8.0, 6.0, -45.0];
/// A knee drawn up beside the hip.
const LEG_DRAWN: [f32; 4] = [55.0, 45.0, 100.0, -25.0];

fn lerp4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

fn mirror(p: Vec3, side: usize) -> Vec3 {
    Vec3::new(p.x * S[side], p.y, p.z)
}

/// Where a crawling limb is at `phase` (0..1 of its own cycle: planted for the first half,
/// moving the body; carried back for the second), between `from` (planted) and `to`.
fn track(from: Vec3, to: Vec3, phase: f32, lift: f32) -> Vec3 {
    if phase < 0.5 {
        // Planted: the body slides over it at a steady pace.
        from.lerp(to, phase * 2.0)
    } else {
        let t = smooth01((phase - 0.5) * 2.0);
        to.lerp(from, t) + Vec3::Y * lift * (std::f32::consts::PI * t).sin()
    }
}

/// The crawl at `phase` (0..1): the left forearm and right knee pull through the first
/// half, the right forearm and left knee the second. With a `rifle` the right fist pulls
/// on its pistol grip instead of a flat hand, the gun on its side beside the forearm (top
/// in, magazine out), the muzzle up off the ground and the butt dragging.
fn crawl_pose(s: &Skeleton, phase: f32, rifle: bool) -> Vec<(&'static str, Xf)> {
    let pulled = REACHED + Vec3::new(0.08, 0.0, PULL);
    // Each limb's own phase: arm l and leg r together, then the others.
    let own = [phase, (phase + 0.5).fract()];
    let arm_at = |side: usize| {
        let p = own[side];
        track(mirror(REACHED, side), mirror(pulled, side), p, 0.07)
    };
    let leg_at = |side: usize| {
        let p = own[1 - side];
        track(mirror(DRAWN, side), mirror(STRAIGHT, side), p, 0.03)
    };
    // How drawn up each leg is (1 at the start of its push, 0 pushed straight).
    let drawn = |side: usize| {
        let p = own[1 - side];
        if p < 0.5 {
            1.0 - p * 2.0
        } else {
            smooth01((p - 0.5) * 2.0)
        }
    };
    let reach = |side: usize| {
        let p = own[side];
        if p < 0.5 {
            1.0 - p * 2.0
        } else {
            smooth01((p - 0.5) * 2.0)
        }
    };
    let lift = drawn(1) - drawn(0);
    let arms = std::array::from_fn(|side| FOREARM.lerp(&REACHING, reach(side)));
    let legs = std::array::from_fn(|side| lerp4(LEG_STRAIGHT, LEG_DRAWN, drawn(side)));
    let key = lying(lift, arms, legs);
    let mut targets = [
        Some(arm_at(0)),
        Some(arm_at(1)),
        Some(leg_at(0)),
        Some(leg_at(1)),
    ];
    let mut ends = palms_down();
    if rifle {
        let held = carried();
        let fist = arm_at(1) + CARRY_FIST;
        targets[1] = Some(fist - held * GRIP);
        ends[1] = Some(held);
    }
    key.reach(s, targets, ends, poles())
}

/// A long gun carried crawling in the right fist: on its side (top in toward the body,
/// magazine out), the muzzle a little in and up off the ground.
fn carried() -> Quat {
    let turn = Quat::from_rotation_y(0.08) * Quat::from_rotation_x(4f32.to_radians());
    rifle_frame(turn * Vec3::NEG_Z, turn * Vec3::NEG_X)
}

/// The fist's middle from where a flat hand's wrist crawls: just ahead of it, at the same height.
const CARRY_FIST: Vec3 = Vec3::new(0.0, 0.0, -0.06);

/// Hands flat on the floor, palms down, fingers ahead and a little in (the hand's rest frame
/// hangs with the fingers down and the palm toward the thigh).
fn palms_down() -> [Option<Quat>; 4] {
    let flat = |side: usize| {
        let s = S[side];
        Quat::from_rotation_y(-0.25 * s)
            * Quat::from_rotation_z(std::f32::consts::FRAC_PI_2 * s)
            * Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)
    };
    [Some(flat(0)), Some(flat(1)), None, None]
}

/// Elbows down and out to the floor beside the chest, knees out to the side.
fn poles() -> [Vec3; 4] {
    [
        Vec3::new(-1.0, -0.4, 0.4),
        Vec3::new(1.0, -0.4, 0.4),
        Vec3::new(-1.0, 0.0, -0.2),
        Vec3::new(1.0, 0.0, -0.2),
    ]
}

/// Lying still on the forearms, legs straight, breathing.
fn idle_pose(s: &Skeleton, phase: f32) -> Vec<(&'static str, Xf)> {
    let breath = (std::f32::consts::TAU * phase * 2.0).sin();
    let mut key = lying(0.0, [FOREARM; 2], [LEG_STRAIGHT; 2]);
    key.hips.y += 0.004 * breath;
    key.spine[1] += 1.5 * breath;
    let wrist = Vec3::new(0.17, 0.05, -0.42);
    let ankle = Vec3::new(0.16, 0.07, STRAIGHT.z - 0.02);
    let targets = [
        Some(mirror(wrist, 0)),
        Some(mirror(wrist, 1)),
        Some(mirror(ankle, 0)),
        Some(mirror(ankle, 1)),
    ];
    // Elbows under the shoulders, forearms side by side.
    let mut poles = poles();
    poles[0] = Vec3::new(-0.25, -1.0, 0.4);
    poles[1] = Vec3::new(0.25, -1.0, 0.4);
    key.reach(s, targets, palms_down(), poles)
}

/// On hands and knees: the way down to lying and up from it (the graph passes through it
/// halfway along [`PRONED`]).
fn kneel_key() -> Key {
    let hz = HIPS.z + 0.04;
    Key {
        at: 0.0,
        hips: Vec3::new(0.0, 0.5, hz),
        pitch: -78.0,
        yaw: 0.0,
        roll: 0.0,
        spine: [-6.0, -4.0, 16.0, 10.0],
        arm: [Arm {
            flex: 80.0,
            twist: 8.0,
            abduct: 6.0,
            elbow: 6.0,
            wrist: -76.0,
            protract: 8.0,
            elevate: 4.0,
        }; 2],
        leg: [[86.0, 6.0, 92.0, -75.0]; 2],
        plant: [
            Some(Vec3::new(-0.22, 0.04, hz - 0.5)),
            Some(Vec3::new(0.22, 0.04, hz - 0.5)),
            Some(Vec3::new(-0.14, 0.1, hz + 0.5)),
            Some(Vec3::new(0.14, 0.1, hz + 0.5)),
        ],
        knee: [Vec3::NEG_Y; 2],
        effort: 0.4,
    }
}

/// The prone firing position (FM 3-22.9, "prone unsupported"): legs spread and
/// straight, toes out, the body a little to the left of the line of fire, the chest up on
/// both elbows, the butt in the pocket of the right shoulder, the right elbow out on the
/// ground, the left under the gun with the hand on the fore grip, the cheek down on the stock.
/// `kick` (0..1) is a shot's recoil: the gun back into the shoulder and the muzzle up, the
/// shoulders rocked back; `breath` (−1..1) lifts the chest a few millimetres; `ready` (0..1)
/// lowers the muzzle and lifts the head off the stock to look over the sights.
fn aim_pose(s: &Skeleton, kick: f32, breath: f32, ready: f32) -> Vec<(&'static str, Xf)> {
    gun_pose(
        s,
        Gun {
            kick,
            breath,
            ready,
            ..Gun::AIMED
        },
    )
}

/// How the gun is held lying in the firing position, for [`gun_pose`].
#[derive(Clone, Copy, Debug)]
struct Gun {
    kick: f32,
    breath: f32,
    ready: f32,
    /// Turned over on its axis, top to the right, degrees (to bring the magazine round to the
    /// left hand).
    cant: f32,
    /// Where the left fist is, by `left_on` (0 on the fore grip, 1 at `left`).
    left_on: f32,
    left: Left,
}

/// Where the left fist goes off the fore grip.
#[derive(Clone, Copy, Debug)]
enum Left {
    /// On the magazine, this far down out of its well, m.
    Mag(f32),
    /// At the pouch on the left of the chest.
    Pouch,
}

impl Gun {
    const AIMED: Gun = Gun {
        kick: 0.0,
        breath: 0.0,
        ready: 0.0,
        cant: 0.0,
        left_on: 0.0,
        left: Left::Mag(0.0),
    };
}

fn gun_pose(s: &Skeleton, g: Gun) -> Vec<(&'static str, Xf)> {
    let Gun {
        kick,
        breath,
        ready,
        ..
    } = g;
    let legs = [AIM_LEG; 2];
    let mut key = lying(0.0, [FOREARM; 2], legs);
    key.spine = AIM_SPINE;
    key.spine[1] += 1.2 * breath;
    key.hips.z += 0.012 * kick;
    key.hips.y += 0.002 * breath;
    key.effort = 0.35;
    let k = body_scale(s);
    let (_, model) = key.forward(s);
    let shoulder = model[s.bone(UPPER_ARM[1])].t / k;
    // Built square to the body, the gun pointing `AIM_ANGLE` to its left (and level at a
    // target on the ground ahead, climbing a few degrees as it kicks); then the whole body
    // turns right about the butt so the gun points ahead.
    let climb = (AIM_DROP + 4.0 * kick - 9.0 * ready).to_radians();
    let across = Quat::from_rotation_y(AIM_ANGLE.to_radians());
    let dir = across * Vec3::new(0.0, climb.sin(), -climb.cos());
    let up = across * Vec3::new(0.0, climb.cos(), climb.sin());
    let up = Quat::from_axis_angle(dir, -g.cant.to_radians()) * up;
    let butt = shoulder + AIM_POCKET - dir * 0.022 * kick;
    let fist = butt + dir * RIFLE_BUTT;
    let held = rifle_frame(dir, up);
    let on_fore = fist + dir * RIFLE_FORE;
    let off = match g.left {
        Left::Mag(down) => fist + dir * RIFLE_MAG - up * (0.05 + down),
        Left::Pouch => shoulder + POUCH,
    };
    let fore = on_fore.lerp(off, g.left_on);
    let turn = across.inverse();
    let ankle = |side: usize| mirror(AIM_FEET, side);
    let targets = [
        Some(fore - held * GRIP),
        Some(fist - held * GRIP),
        Some(ankle(0)),
        Some(ankle(1)),
    ];
    let mut pose = key.reach(s, targets, [Some(held), Some(held), None, None], AIM_POLES);
    for (name, x) in pose.iter_mut() {
        match *name {
            "hips" => {
                let pivot = butt * k;
                x.t = turn * (x.t - pivot) + pivot;
                x.r = turn * x.r;
            }
            // The cheek down on the stock: the head turned along the gun and tilted right
            // over it.
            "head" => {
                let [turn, tilt] = AIM_HEAD.map(|a| a * (1.0 - 0.6 * ready));
                x.r = x.r
                    * Quat::from_rotation_x((16.0 * ready).to_radians())
                    * Quat::from_rotation_y(turn.to_radians())
                    * Quat::from_rotation_z(-tilt.to_radians());
            }
            _ => {}
        }
    }
    pose
}

/// The magazine pouch on the left of the chest, from the right shoulder joint (body frame,
/// lying): the left hand reaches back under the chest for it.
const POUCH: Vec3 = Vec3::new(-0.3, -0.16, 0.12);

/// A magazine change lying in the firing position, s.
const RELOAD_SECS: f32 = 2.6;

/// A magazine change lying down at `t` s: the gun canted over to bring the magazine to the
/// left hand, the old one out and away, a fresh one from the pouch in and slapped home, the
/// hand back on the fore grip and the gun upright on the target again.
fn reload_pose(s: &Skeleton, t: f32) -> Vec<(&'static str, Xf)> {
    let [grab, out, stowed, back, home, slapped, regrip] = RELOAD_KEYS;
    let cant = keyed(
        t,
        &[
            (0.0, 0.0),
            (grab, 50.0),
            (regrip, 50.0),
            (RELOAD_SECS - 0.2, 0.0),
        ],
    );
    let ready = keyed(
        t,
        &[
            (0.0, 0.0),
            (grab, 0.7),
            (regrip, 0.7),
            (RELOAD_SECS - 0.2, 0.0),
        ],
    );
    // The left fist: along the magazine (down out of its well, m), or at the pouch.
    let keys: [(f32, Option<f32>); 9] = [
        (0.0, None),
        (grab, Some(0.0)),
        (out, Some(0.16)),
        (stowed, None),
        (back, Some(0.1)),
        (home, Some(0.0)),
        (slapped, Some(-0.012)),
        (regrip, Some(0.0)),
        (RELOAD_SECS, None),
    ];
    let i = keys
        .iter()
        .rposition(|k| k.0 <= t)
        .unwrap_or(0)
        .min(keys.len() - 2);
    let (a, b) = (keys[i], keys[i + 1]);
    let u = smooth01(((t - a.0) / (b.0 - a.0)).clamp(0.0, 1.0));
    // Each end of the move as (off the fore grip, where): the first and last keys are on the
    // fore grip, the stowed one at the pouch.
    let at = |k: usize, key: (f32, Option<f32>)| match key.1 {
        Some(d) => (1.0, Left::Mag(d)),
        None if k == 0 || k == keys.len() - 1 => (0.0, Left::Mag(0.0)),
        None => (1.0, Left::Pouch),
    };
    let ((on_a, left_a), (on_b, left_b)) = (at(i, a), at(i + 1, b));
    let pose_at = |on: f32, left: Left| {
        gun_pose(
            s,
            Gun {
                ready,
                cant,
                left_on: on,
                left,
                ..Gun::AIMED
            },
        )
    };
    // The two ends' arms blended (their bodies and gun are the same).
    let from = pose_at(on_a, left_a);
    let to = pose_at(on_b, left_b);
    from.into_iter()
        .zip(to)
        .map(|((name, x), (_, y))| (name, x.lerp(&y, u)))
        .collect()
}

/// Grab, out, stowed, back, home, slapped, regrip, s.
const RELOAD_KEYS: [f32; 7] = [0.4, 0.58, 0.98, 1.4, 1.6, 1.72, 2.0];

/// The rotation that turns a long gun held in the right fist (its barrel along
/// [`RIFLE_AXIS`], top along [`RIFLE_UP`] in the hand's frame) to point `dir`, top `up`.
fn rifle_frame(dir: Vec3, up: Vec3) -> Quat {
    let frame = |a: Vec3, f: Vec3| Quat::from_mat3(&Mat3::from_cols(a, f, a.cross(f)));
    (frame(dir, up) * frame(RIFLE_AXIS, RIFLE_UP).inverse()).normalize()
}

/// The firing position's body: turned right of the line of fire (degrees, the left shoulder
/// ahead, the legs off to the left behind), back arched up on the elbows, and a gun a degree
/// or two down at a target on the ground ahead.
const AIM_ANGLE: f32 = 35.0;
const AIM_SPINE: [f32; 4] = [8.0, 16.0, 14.0, 22.0];
const AIM_DROP: f32 = -1.5;
/// The butt's heel from the right shoulder joint: in, ahead of it and a little down.
const AIM_POCKET: Vec3 = Vec3::new(-0.09, -0.05, 0.03);
/// The head turned left along the gun and tilted right onto the stock, degrees.
const AIM_HEAD: [f32; 2] = [20.0, 14.0];
/// Legs straight and spread, toes turned out (the inside of each foot on the ground).
const AIM_LEG: [f32; 4] = [0.0, 18.0, 4.0, -55.0];
const AIM_FEET: Vec3 = Vec3::new(0.27, 0.08, 1.18);
/// The right elbow out to the side on the ground, the left down under the gun.
const AIM_POLES: [Vec3; 4] = [
    Vec3::new(0.2, -1.0, 0.0),
    Vec3::new(0.6, -1.0, 0.1),
    Vec3::new(-1.0, 0.0, -0.2),
    Vec3::new(1.0, 0.0, -0.2),
];

fn lying_clip(
    name: &str,
    secs: f32,
    effort: f32,
    pose: impl Fn(f32) -> Vec<(&'static str, Xf)>,
) -> AnimationClip {
    let samples = (secs * 30.0) as usize + 1;
    AnimationClip::from_fn(name, secs, samples, true, pose)
        .with_curve("effort", 2, |_| effort)
        .with_curve("floor", 2, |_| 1.0)
}

/// The prone clips: lying still, crawling ahead and back, and on hands and knees.
pub(super) fn clips(s: &Skeleton) -> Vec<AnimationClip> {
    let kneel = kneel_key();
    vec![
        lying_clip(PRONE_IDLE, 3.0, 0.1, |ph| idle_pose(s, ph)),
        lying_clip(PRONE_CRAWL, CRAWL_SECS, 0.6, |ph| crawl_pose(s, ph, false)),
        // The same pulls the other way round: pushing back on the forearms.
        lying_clip(PRONE_BACK, CRAWL_SECS, 0.5, |ph| {
            crawl_pose(s, 1.0 - ph, false)
        }),
        // The same, carrying a long gun (the left hand off it).
        lying_clip(PRONE_RIFLE_CRAWL, CRAWL_SECS, 0.65, |ph| {
            crawl_pose(s, ph, true)
        })
        .with_curve("grip", 2, |_| 0.0),
        lying_clip(PRONE_RIFLE_BACK, CRAWL_SECS, 0.55, |ph| {
            crawl_pose(s, 1.0 - ph, true)
        })
        .with_curve("grip", 2, |_| 0.0),
        lying_clip(PRONE_KNEEL, 1.0, 0.4, move |_| {
            Key::sample(s, &[kneel, Key { at: 1.0, ..kneel }], 0.0).1
        }),
        lying_clip(PRONE_RIFLE_AIM, 3.2, 0.3, |ph| {
            aim_pose(s, 0.0, (std::f32::consts::TAU * ph).sin(), 0.0)
        })
        .with_curve("grip", 2, |_| 1.0),
        lying_clip(PRONE_RIFLE_READY, 3.2, 0.15, |ph| {
            aim_pose(s, 0.0, (std::f32::consts::TAU * ph).sin(), 1.0)
        })
        .with_curve("grip", 2, |_| 1.0),
        {
            let [grab, .., home, _, regrip] = RELOAD_KEYS;
            let secs = RELOAD_SECS;
            AnimationClip::from_fn(
                PRONE_RIFLE_RELOAD,
                secs,
                (secs * 30.0) as usize + 1,
                false,
                move |ph| reload_pose(s, ph * secs),
            )
            .with_curve("floor", 2, |_| 1.0)
            .with_curve("effort", 8, |_| 0.4)
            // The left hand on the fore grip only at the ends.
            .with_curve("grip", (secs * 30.0) as usize, move |ph| {
                keyed(
                    ph * secs,
                    &[
                        (0.1, 1.0),
                        (grab - 0.1, 0.0),
                        (regrip, 0.0),
                        (regrip + 0.12, 1.0),
                    ],
                )
            })
            .with_curve("mag_in_hand", (secs * 60.0) as usize, move |ph| {
                if (grab + 0.04..home).contains(&(ph * secs)) {
                    1.0
                } else {
                    0.0
                }
            })
            .with_event((grab + 0.04) / secs, "mag_out", Some("hand_l"))
            .with_event(home / secs, "mag_in", Some("hand_l"))
        },
        {
            let secs = FIRE_SECS;
            let kick = move |ph: f32| {
                keyed(
                    ph * secs,
                    &[
                        (RIFLE_SHOT_AT, 0.0),
                        (RIFLE_SHOT_AT + 0.04, 1.0),
                        (secs, 0.0),
                    ],
                )
            };
            AnimationClip::from_fn(
                PRONE_RIFLE_FIRE,
                secs,
                (secs * 60.0) as usize + 1,
                false,
                move |ph| aim_pose(s, kick(ph), 0.0, 0.0),
            )
            .with_curve("floor", 2, |_| 1.0)
            .with_curve("grip", 2, |_| 1.0)
            .with_curve("effort", 8, move |ph| 0.4 + 0.4 * kick(ph))
            .with_event(RIFLE_SHOT_AT / secs, "shot", Some("hand_r"))
        },
    ]
}

/// A prone shot: the kick and the settle back on the target, s.
const FIRE_SECS: f32 = 0.3;

/// Lying down by [`PRONED`] over everything `standing` does: through hands and knees to the
/// floor, then still or crawling by `speed` and `heading`.
pub(super) fn graph(standing: NodeDef) -> NodeDef {
    let ready = |node: NodeDef| NodeDef::Blend1d {
        param: RIFLE_CARRY.into(),
        children: vec![
            Blend1dChild { at: 0.0, node },
            Blend1dChild {
                at: 1.0,
                node: NodeDef::clip(PRONE_RIFLE_READY),
            },
        ],
        sync: false,
        match_speed: false,
    };
    // Still, or crawling at `speed` (a rifleman lying still keeps the gun in the shoulder,
    // ready).
    let carrying = |unarmed: &str, armed: &str| NodeDef::Blend1d {
        param: RIFLE_CARRY.into(),
        children: vec![
            Blend1dChild {
                at: 0.0,
                node: NodeDef::clip(unarmed),
            },
            Blend1dChild {
                at: 1.0,
                node: NodeDef::clip(armed),
            },
        ],
        sync: true,
        match_speed: false,
    };
    let by_speed = |speed: f32, (unarmed, armed): (&str, &str)| NodeDef::Blend1d {
        param: "speed".into(),
        children: vec![
            Blend1dChild {
                at: 0.0,
                node: ready(NodeDef::clip(PRONE_IDLE)),
            },
            Blend1dChild {
                at: speed,
                node: carrying(unarmed, armed),
            },
        ],
        sync: true,
        match_speed: true,
    };
    let pi = std::f32::consts::PI;
    let ahead = || by_speed(PRONE_CRAWL_SPEED, (PRONE_CRAWL, PRONE_RIFLE_CRAWL));
    let back = || by_speed(PRONE_BACK_SPEED, (PRONE_BACK, PRONE_RIFLE_BACK));
    // Sideways the body crawls slowly on (a mover turns it to face the way it goes).
    let lying = NodeDef::Blend1d {
        param: "heading".into(),
        children: [
            (-pi, back()),
            (-pi / 2.0, ahead()),
            (0.0, ahead()),
            (pi / 2.0, ahead()),
            (pi, back()),
        ]
        .into_iter()
        .map(|(at, node)| Blend1dChild { at, node })
        .collect(),
        sync: true,
        match_speed: false,
    };
    // A rifleman aiming lies in the firing position instead.
    let lying = NodeDef::Blend1d {
        param: PRONE_AIMED.into(),
        children: vec![
            Blend1dChild {
                at: 0.0,
                node: lying,
            },
            Blend1dChild {
                at: 1.0,
                node: NodeDef::clip(PRONE_RIFLE_AIM),
            },
        ],
        sync: false,
        match_speed: false,
    };
    NodeDef::Blend1d {
        param: PRONED.into(),
        children: vec![
            Blend1dChild {
                at: 0.0,
                node: standing,
            },
            Blend1dChild {
                at: 0.5,
                node: NodeDef::clip(PRONE_KNEEL),
            },
            Blend1dChild {
                at: 1.0,
                node: lying,
            },
        ],
        sync: false,
        match_speed: false,
    }
}

/// Out of `move` only while not lying down: a stance, a swing, a jump or a shot stands the
/// body up, so lying (or going down) it stays on the floor instead. Falls and get-ups (from
/// anywhere) are untouched.
pub(super) fn upright_only(t: TransitionDef) -> TransitionDef {
    if t.from != "move" {
        return t;
    }
    TransitionDef {
        when: Condition::All(vec![t.when, Condition::param(PRONED, Cmp::Lt, 0.25)]),
        ..t
    }
}

/// The prone shot's state, beside `move`.
pub(super) fn states() -> Vec<StateDef> {
    [PRONE_RIFLE_FIRE, PRONE_RIFLE_RELOAD]
        .map(|name| StateDef {
            name: name.into(),
            node: NodeDef::one_shot(name),
            speed: 1.0,
            speed_param: None,
        })
        .into()
}

/// Lying down with a long gun, `fire` shoots from the firing position (taken up in the
/// shot's first frames when it wasn't aimed), back to lying as before when it's done.
pub(super) fn transitions() -> Vec<TransitionDef> {
    let lying = |trigger: &str| {
        Condition::All(vec![
            Condition::param(RIFLE_CARRY, Cmp::Gt, 0.5),
            Condition::param(PRONED, Cmp::Gt, 0.75),
            Condition::trigger(trigger),
        ])
    };
    vec![
        TransitionDef::new("move", PRONE_RIFLE_FIRE, lying("fire"), 0.06),
        TransitionDef::new("move", PRONE_RIFLE_RELOAD, lying("reload"), 0.2),
        TransitionDef::new(
            PRONE_RIFLE_FIRE,
            PRONE_RIFLE_RELOAD,
            Condition::trigger("reload"),
            0.15,
        ),
        TransitionDef::new(PRONE_RIFLE_FIRE, "move", Condition::Finished, 0.12),
        TransitionDef::new(PRONE_RIFLE_RELOAD, "move", Condition::Finished, 0.2),
    ]
}
