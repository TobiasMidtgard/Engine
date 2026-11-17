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
pub const PRONE_KNEEL: &str = "prone_kneel";

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
/// half, the right forearm and left knee the second.
fn crawl_pose(s: &Skeleton, phase: f32) -> Vec<(&'static str, Xf)> {
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
    let targets = [
        Some(arm_at(0)),
        Some(arm_at(1)),
        Some(leg_at(0)),
        Some(leg_at(1)),
    ];
    key.reach(s, targets, palms_down(), poles())
}

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
        lying_clip(PRONE_CRAWL, CRAWL_SECS, 0.6, |ph| crawl_pose(s, ph)),
        // The same pulls the other way round: pushing back on the forearms.
        lying_clip(PRONE_BACK, CRAWL_SECS, 0.5, |ph| crawl_pose(s, 1.0 - ph)),
        lying_clip(PRONE_KNEEL, 1.0, 0.4, move |_| {
            Key::sample(s, &[kneel, Key { at: 1.0, ..kneel }], 0.0).1
        }),
    ]
}

/// Lying down by [`PRONED`] over everything `standing` does: through hands and knees to the
/// floor, then still or crawling by `speed` and `heading`.
pub(super) fn graph(standing: NodeDef) -> NodeDef {
    let by_speed = |clips: &[(f32, &str)]| NodeDef::Blend1d {
        param: "speed".into(),
        children: clips
            .iter()
            .map(|&(at, clip)| Blend1dChild {
                at,
                node: NodeDef::clip(clip),
            })
            .collect(),
        sync: true,
        match_speed: true,
    };
    let pi = std::f32::consts::PI;
    let ahead = || by_speed(&[(0.0, PRONE_IDLE), (PRONE_CRAWL_SPEED, PRONE_CRAWL)]);
    let back = || by_speed(&[(0.0, PRONE_IDLE), (PRONE_BACK_SPEED, PRONE_BACK)]);
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
