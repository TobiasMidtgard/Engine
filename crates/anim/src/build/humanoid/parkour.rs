//! Parkour clips: a take-off, the air (rising, at the top, falling), a double jump, landings,
//! wall runs on either side with a kick off the wall, and a slide. They answer the movement
//! contract's modes and transitions ([`crate::parkour`] turns `LocomotionState` into the
//! parameters and triggers the graph's states wait for).
//!
//! The wall-run cycle is the sprint's rhythm (0.56 s a stride, 4 m at 7 m/s, after Mann's
//! sprint data) with the feet planted on a wall [`WALL_DISTANCE`] to the side, soles flat on
//! it, the body leaning [`WALL_TILT`] off it about the hips, the inside arm held out toward
//! it and the outside arm pumping. Mirror's Edge and Titanfall keep the runner's head level;
//! the lean is the one friction asks for, stylized down (anim-animals-wallrun-research.md).

use super::*;

/// Where the wall-run clips put the wall: its face this far from the body's axis, m.
/// [`crate::parkour`] moves the feet and hand onto the wall actually sensed.
pub const WALL_DISTANCE: f32 = 0.55;
/// How far the body leans off the wall about the hips while running on it, degrees.
pub const WALL_TILT: f32 = 32.0;
/// One stride of the wall run (right foot down to right foot down), s.
pub const WALL_RUN_SECS: f32 = 0.56;
/// Fraction of a stride each foot is on the wall.
const WALL_DUTY: f32 = 0.27;
/// Ankle joint to sole, m.
const ANKLE_TO_SOLE: f32 = 0.085;
/// The clips and the graph states of the same names.
pub const JUMP: &str = "jump";
pub const DOUBLE_JUMP: &str = "double_jump";
pub const LAND: &str = "land";
pub const LAND_HARD: &str = "land_hard";
pub const SLIDE: &str = "slide";
/// Wall runs and kicks off the wall, by side (left wall, right wall).
pub const WALL_RUN: [&str; 2] = ["wall_run_l", "wall_run_r"];
pub const WALL_KICK: [&str; 2] = ["wall_kick_l", "wall_kick_r"];
/// The air blend's clips, by vertical speed.
pub const AIR: [&str; 3] = ["air_fall", "air_apex", "air_rise"];
/// Curves (0..1) saying how firmly each limb is on the wall, in `Key::plant` order: what
/// [`crate::parkour`] moves onto the wall it senses.
pub const WALL_CURVES: [&str; 4] = ["wall.hand_l", "wall.hand_r", "wall.foot_l", "wall.foot_r"];

const SAMPLES_PER_SEC: f32 = 30.0;

impl Key {
    /// The pose of `self` with the limbs that have a target (in [`LIMBS`] order, character
    /// frame, meters) reached by IK, each bending toward its `pole`, and each end turned to
    /// its `ends` rotation (model space) or kept as the joint angles put it.
    fn reach(
        &self,
        s: &Skeleton,
        targets: [Option<Vec3>; 4],
        ends: [Option<Quat>; 4],
        poles: [Vec3; 4],
    ) -> Vec<(&'static str, Xf)> {
        let (mut local, mut model) = self.forward(s);
        let k = body_scale(s);
        let mut edit = crate::animator::PoseEdit {
            skeleton: s,
            local: &mut local,
            model: &mut model,
        };
        for (limb, [upper, lower, end]) in LIMBS.iter().enumerate() {
            let (upper, lower, end) = (s.bone(upper), s.bone(lower), s.bone(end));
            let held = ends[limb].unwrap_or(edit.model[end].r);
            if let Some(t) = targets[limb] {
                crate::ik::two_bone(&mut edit, upper, lower, end, t * k, poles[limb], 1.0);
            }
            edit.set_model_rotation(end, held);
        }
        GET_UP_BONES
            .iter()
            .map(|&name| {
                let b = s.bone(name);
                let mut x = local[b];
                if name != "hips" {
                    x.t = Vec3::ZERO;
                }
                (name, x)
            })
            .collect()
    }
}

/// A pose of joint angles alone, from [`Key::STAND`].
fn pose(
    hips: Vec3,
    pitch: f32,
    roll: f32,
    spine: [f32; 4],
    arm: [Arm; 2],
    leg: [[f32; 4]; 2],
) -> Key {
    Key {
        at: 0.0,
        hips,
        pitch,
        yaw: 0.0,
        roll,
        spine,
        arm,
        leg,
        plant: [None; 4],
        knee: [Vec3::NEG_Z; 2],
        effort: 0.6,
    }
}

/// A one-shot through `keys` (at seconds), `floor` 1 throughout unless `grounded` gives the
/// phases (from, to) the feet carry the body.
fn keyed_clip(
    s: &Skeleton,
    name: &str,
    secs: f32,
    keys: &[Key],
    looping: bool,
    floor: &[(f32, f32)],
) -> AnimationClip {
    let keys: Vec<Key> = keys
        .iter()
        .map(|k| Key {
            at: k.at / secs,
            ..*k
        })
        .collect();
    let samples = (secs * SAMPLES_PER_SEC) as usize + 1;
    AnimationClip::from_fn(name, secs, samples, looping, |ph| {
        Key::sample(s, &keys, ph).1
    })
    .with_curve("effort", samples, |ph| Key::sample(s, &keys, ph).0.effort)
    .with_curve("floor", 25, |ph| keyed(ph, floor))
}

/// Arms swung up and forward, as at a take-off.
const ARMS_UP: Arm = Arm {
    flex: 70.0,
    twist: 0.0,
    abduct: 10.0,
    elbow: 60.0,
    wrist: 0.0,
    protract: 6.0,
    elevate: 6.0,
};
/// Arms out to the sides for balance.
const ARMS_OUT: Arm = Arm {
    flex: 25.0,
    twist: 0.0,
    abduct: 45.0,
    elbow: 35.0,
    wrist: 10.0,
    protract: 0.0,
    elevate: 8.0,
};

/// Taking off from a run: driving off the planted right leg, the left knee punching up, the
/// arms swinging up; it ends gathered in the air.
fn jump(s: &Skeleton) -> AnimationClip {
    let planted = Some(Vec3::new(0.11, 0.08, 0.12));
    let load = Key {
        at: 0.0,
        plant: [None, None, None, planted],
        knee: [Vec3::NEG_Z; 2],
        ..pose(
            Vec3::new(0.0, 0.86, 0.0),
            -12.0,
            0.0,
            [-8.0, -4.0, 4.0, 2.0],
            [
                Arm {
                    flex: -30.0,
                    elbow: 70.0,
                    ..Arm::HANG
                },
                Arm {
                    flex: 30.0,
                    elbow: 80.0,
                    ..Arm::HANG
                },
            ],
            [[45.0, 2.0, 80.0, 5.0], [10.0, 2.0, 35.0, 5.0]],
        )
    };
    let drive = Key {
        at: 0.13,
        plant: [None, None, None, Some(Vec3::new(0.11, 0.1, 0.3))],
        effort: 1.0,
        ..pose(
            Vec3::new(0.0, 0.98, -0.05),
            -14.0,
            0.0,
            [-6.0, -4.0, 6.0, 2.0],
            [
                ARMS_UP,
                Arm {
                    flex: -10.0,
                    elbow: 70.0,
                    ..ARMS_UP
                },
            ],
            [[80.0, 2.0, 95.0, 10.0], [-12.0, 2.0, 10.0, -25.0]],
        )
    };
    let gathered = Key {
        at: 0.32,
        ..pose(
            Vec3::new(0.0, 0.98, 0.0),
            -8.0,
            0.0,
            [-6.0, -4.0, 4.0, 2.0],
            [ARMS_UP, ARMS_UP],
            [[60.0, 4.0, 85.0, 5.0], [20.0, 4.0, 60.0, -15.0]],
        )
    };
    keyed_clip(
        s,
        JUMP,
        0.32,
        &[load, drive, gathered],
        false,
        &[(0.0, 0.0), (0.4, 0.0), (0.55, 1.0)],
    )
}

/// The air's poses, by vertical speed: rising (knee up, arms up), at the top (legs gathered,
/// arms out), falling (legs reaching down for the ground, arms out and up).
fn air_keys() -> [Key; 3] {
    let rise = pose(
        Vec3::new(0.0, 0.98, 0.0),
        -6.0,
        0.0,
        [-6.0, -4.0, 4.0, 2.0],
        [
            ARMS_UP,
            Arm {
                flex: 40.0,
                ..ARMS_UP
            },
        ],
        [[55.0, 4.0, 75.0, 5.0], [10.0, 4.0, 55.0, -15.0]],
    );
    let apex = pose(
        Vec3::new(0.0, 0.98, 0.0),
        -4.0,
        0.0,
        [-8.0, -6.0, 6.0, 2.0],
        [ARMS_OUT, ARMS_OUT],
        [[50.0, 6.0, 80.0, 0.0], [35.0, 6.0, 75.0, -5.0]],
    );
    let fall = pose(
        Vec3::new(0.0, 0.98, 0.0),
        4.0,
        0.0,
        [2.0, 0.0, -2.0, -6.0],
        [
            Arm {
                abduct: 60.0,
                flex: 20.0,
                ..ARMS_OUT
            },
            Arm {
                abduct: 55.0,
                flex: 30.0,
                ..ARMS_OUT
            },
        ],
        [[30.0, 6.0, 35.0, 8.0], [18.0, 6.0, 25.0, 8.0]],
    );
    [fall, apex, rise]
}

/// The air blend's clips ([`AIR`] order): slow loops on the [`air_keys`] poses whose arms and
/// legs paddle a little, so a long fall doesn't freeze.
fn air(s: &Skeleton) -> [AnimationClip; 3] {
    let keys = air_keys();
    std::array::from_fn(|i| {
        let k = keys[i];
        let a = Key { at: 0.0, ..k };
        let b = Key {
            at: 0.5,
            leg: [
                [
                    k.leg[0][0] - 6.0,
                    k.leg[0][1],
                    k.leg[0][2] + 6.0,
                    k.leg[0][3],
                ],
                [
                    k.leg[1][0] + 6.0,
                    k.leg[1][1],
                    k.leg[1][2] - 4.0,
                    k.leg[1][3],
                ],
            ],
            arm: [
                Arm {
                    abduct: k.arm[0].abduct + 6.0,
                    ..k.arm[0]
                },
                Arm {
                    abduct: k.arm[1].abduct - 4.0,
                    ..k.arm[1]
                },
            ],
            ..k
        };
        let c = Key { at: 1.0, ..k };
        keyed_clip(s, AIR[i], 1.0, &[a, b, c], true, &[(0.0, 1.0)])
    })
}

/// A second jump in the air: knees tucked to the chest and the arms swept down, then opening
/// out into the rise.
fn double_jump(s: &Skeleton) -> AnimationClip {
    let rise = air_keys()[2];
    let start = Key { at: 0.0, ..rise };
    let tuck = Key {
        at: 0.16,
        effort: 1.0,
        ..pose(
            Vec3::new(0.0, 1.0, 0.0),
            -10.0,
            0.0,
            [-22.0, -14.0, 6.0, 4.0],
            [
                Arm {
                    flex: -20.0,
                    abduct: 20.0,
                    elbow: 40.0,
                    ..Arm::HANG
                },
                Arm {
                    flex: -20.0,
                    abduct: 20.0,
                    elbow: 40.0,
                    ..Arm::HANG
                },
            ],
            [[105.0, 8.0, 125.0, -10.0], [100.0, 8.0, 120.0, -10.0]],
        )
    };
    let open = Key { at: 0.46, ..rise };
    keyed_clip(
        s,
        DOUBLE_JUMP,
        0.46,
        &[start, tuck, open],
        false,
        &[(0.0, 1.0)],
    )
}

/// Where the feet come down in a landing (character frame): a little apart, the left ahead.
const LAND_FEET: [Option<Vec3>; 2] = [
    Some(Vec3::new(-0.12, 0.08, -0.06)),
    Some(Vec3::new(0.12, 0.08, 0.08)),
];

/// Landing from a jump: the feet come down reaching, the knees and hips give to soak it up,
/// the trunk folds over them and the arms come forward, then up again ready to go on.
fn land(s: &Skeleton) -> AnimationClip {
    let fall = air_keys()[0];
    let feet = [None, None, LAND_FEET[0], LAND_FEET[1]];
    let touch = Key {
        at: 0.0,
        plant: feet,
        ..fall
    };
    let soak = Key {
        at: 0.13,
        plant: feet,
        effort: 0.8,
        ..pose(
            Vec3::new(0.0, 0.74, 0.02),
            -18.0,
            0.0,
            [-14.0, -8.0, 10.0, 4.0],
            [
                Arm {
                    flex: 45.0,
                    abduct: 10.0,
                    elbow: 50.0,
                    ..Arm::HANG
                },
                Arm {
                    flex: 35.0,
                    abduct: 12.0,
                    elbow: 55.0,
                    ..Arm::HANG
                },
            ],
            [[60.0, 4.0, 95.0, 30.0], [55.0, 4.0, 90.0, 28.0]],
        )
    };
    let up = Key {
        at: 0.42,
        plant: feet,
        effort: 0.2,
        ..Key::STAND
    };
    keyed_clip(s, LAND, 0.42, &[touch, soak, up], false, &[(0.0, 0.0)])
}

/// A hard landing from high up: down deep onto the balls of the feet with the right hand
/// slapping the ground beside them, the head down, a beat there, then up.
fn land_hard(s: &Skeleton) -> AnimationClip {
    let fall = air_keys()[0];
    let feet = [None, None, LAND_FEET[0], LAND_FEET[1]];
    let hand = Some(Vec3::new(0.3, 0.06, -0.38));
    let touch = Key {
        at: 0.0,
        plant: feet,
        ..fall
    };
    let deep = Key {
        at: 0.16,
        plant: [None, hand, feet[2], feet[3]],
        effort: 1.0,
        ..pose(
            Vec3::new(0.0, 0.5, 0.05),
            -34.0,
            0.0,
            [-20.0, -14.0, 14.0, 6.0],
            [
                Arm {
                    flex: 50.0,
                    abduct: 25.0,
                    elbow: 40.0,
                    ..Arm::HANG
                },
                Arm {
                    flex: 60.0,
                    abduct: 15.0,
                    elbow: 10.0,
                    ..Arm::HANG
                },
            ],
            [[95.0, 8.0, 135.0, 40.0], [90.0, 8.0, 130.0, 38.0]],
        )
    };
    let hold = Key { at: 0.42, ..deep };
    let up = Key {
        at: 0.9,
        plant: feet,
        effort: 0.3,
        ..Key::STAND
    };
    keyed_clip(
        s,
        LAND_HARD,
        0.9,
        &[touch, deep, hold, up],
        false,
        &[(0.0, 0.0)],
    )
}

/// The model-space turn of a hand flat on a wall, fingers up and forward, palm toward the
/// wall (on either side: both hands' palms face in, so the same turn faces either out).
fn hand_on_wall() -> Quat {
    let fingers = Vec3::new(0.0, 0.6, -0.8);
    let x = Vec3::NEG_X;
    let y = -fingers;
    Quat::from_mat3(&Mat3::from_cols(x, y, x.cross(y)))
}

/// Which limbs (in [`LIMBS`] order) are the wall side's: its hand and foot.
fn wall_side(w: f32) -> (usize, usize) {
    if w > 0.0 { (1, 3) } else { (0, 2) }
}

/// The wall run on the wall at side `w` (+1 right, −1 left) at `phase` of a stride, and how
/// firmly each limb is on the wall (`WALL_CURVES` order).
fn wall_run_pose(s: &Skeleton, w: f32, phase: f32) -> (Vec<(&'static str, Xf)>, [f32; 4]) {
    let wall_x = w * (WALL_DISTANCE - ANKLE_TO_SOLE);
    let mut targets = [None; 4];
    let mut ends = [None; 4];
    let mut on_wall = [0.0; 4];
    let (hand, near_foot) = wall_side(w);
    let rest_foot = s.rest_model[s.bone("foot_l")].r;
    for side in 0..2 {
        let limb = 2 + side;
        // The right foot lands at phase 0, the left at 0.5.
        let q = (phase + if side == 1 { 0.0 } else { 0.5 }).rem_euclid(1.0);
        // The far leg reaches across under the body, so it plants higher up the wall.
        let height = if limb == near_foot { 0.32 } else { 0.46 };
        let (p, roll, pitch, firm) = if q < WALL_DUTY {
            let u = q / WALL_DUTY;
            let z = -0.36 + 0.72 * u;
            let y = height - 0.04 * (std::f32::consts::PI * u).sin();
            (Vec3::new(wall_x, y, z), 72.0, 5.0 - 25.0 * u, 1.0)
        } else {
            let u = (q - WALL_DUTY) / (1.0 - WALL_DUTY);
            // Heel kicked up behind, knee driven through, the foot reaching ahead for the
            // wall again.
            let z = keyed(
                u,
                &[
                    (0.0, 0.36),
                    (0.25, 0.46),
                    (0.6, 0.0),
                    (0.88, -0.36),
                    (1.0, -0.36),
                ],
            );
            let y = height
                + keyed(
                    u,
                    &[
                        (0.0, 0.0),
                        (0.3, 0.32),
                        (0.6, 0.42),
                        (0.88, 0.12),
                        (1.0, 0.0),
                    ],
                );
            let off = 0.22 * (std::f32::consts::PI * u).sin();
            let pitch = keyed(u, &[(0.0, -20.0), (0.35, -35.0), (0.85, 5.0), (1.0, 5.0)]);
            let firm = keyed(u, &[(0.0, 0.5), (0.2, 0.0), (0.85, 0.0), (1.0, 0.5)]);
            (
                Vec3::new(wall_x - w * off, y, z),
                72.0 - 30.0 * off / 0.22,
                pitch,
                firm,
            )
        };
        targets[limb] = Some(p);
        ends[limb] = Some(
            Quat::from_rotation_z((w * roll).to_radians())
                * Quat::from_rotation_x(pitch.to_radians())
                * rest_foot,
        );
        on_wall[limb] = firm;
    }
    // The inside arm reaches down and out toward the wall for balance, palm to it, swaying a
    // little with the stride. (Leaning 32° off the wall the shoulder is 0.65 m from it, out of
    // an arm's reach: Titanfall's runners hold the hand out the same way.)
    targets[hand] = Some(Vec3::new(
        w * (WALL_DISTANCE - 0.23),
        1.15,
        -0.12 + 0.05 * cycle_sin(phase),
    ));
    ends[hand] = Some(hand_on_wall());
    let c = cycle(phase);
    let out = 1 - hand;
    let mut arms = [Arm::HANG; 2];
    // The outside arm pumps with the stride, out a little for balance.
    let cq = cycle(phase + if out == 1 { 0.0 } else { 0.5 } - 0.02);
    arms[out] = Arm {
        flex: 20.0 - 55.0 * cq,
        abduct: 12.0,
        elbow: 88.0 - 28.0 * cq,
        wrist: 4.0,
        protract: -10.0 * cq,
        ..Arm::HANG
    };
    arms[hand] = Arm {
        flex: 50.0,
        abduct: 40.0,
        elbow: 40.0,
        ..Arm::HANG
    };
    let key = Key {
        at: 0.0,
        hips: Vec3::new(0.0, 0.92 + 0.02 * cycle(phase * 2.0 - 0.15), 0.0),
        pitch: -8.0,
        yaw: 7.0 * c,
        roll: w * WALL_TILT,
        // The trunk leans a little further off the wall; the neck and head come back toward
        // upright so the eyes stay level, looking ahead along the wall.
        spine: [-6.0, -5.0, 4.0, 4.0],
        arm: arms,
        leg: [[30.0, 0.0, 40.0, 0.0]; 2],
        plant: [None; 4],
        knee: [Vec3::NEG_Z; 2],
        effort: 0.9,
    };
    let poles = [
        Vec3::new(-0.2 * w, -1.0, 0.4),
        Vec3::new(-0.2 * w, -1.0, 0.4),
        Vec3::new(-0.3 * w, 0.4, -1.0),
        Vec3::new(-0.3 * w, 0.4, -1.0),
    ];
    let mut out_pose = key.reach(s, targets, ends, poles);
    // Spine roll against the hips' lean: chest a touch further off, neck and head back.
    for (name, x) in out_pose.iter_mut() {
        let roll = match *name {
            "chest" => 4.0,
            "neck" => -10.0,
            "head" => -12.0,
            _ => continue,
        };
        x.r = (x.r * Quat::from_rotation_z((w * roll).to_radians())).normalize();
    }
    (out_pose, on_wall)
}

/// The wall run on the wall at side `w` (+1 right): a stride, looping.
fn wall_run(s: &Skeleton, w: f32) -> AnimationClip {
    let name = WALL_RUN[usize::from(w > 0.0)];
    let samples = (WALL_RUN_SECS * 60.0) as usize + 1;
    let mut clip = AnimationClip::from_fn(name, WALL_RUN_SECS, samples, true, |ph| {
        wall_run_pose(s, w, ph).0
    })
    .with_curve("floor", 2, |_| 1.0)
    .with_curve("effort", 2, |_| 0.9);
    for (i, curve) in WALL_CURVES.iter().enumerate() {
        clip = clip.with_curve(curve, 33, |ph| wall_run_pose(s, w, ph).1[i]);
    }
    clip.with_event(0.0, "wall_step", Some("foot_r"))
        .with_event(0.5, "wall_step", Some("foot_l"))
}

/// Kicking off the wall at side `w`: the near foot drives against it as the body swings
/// upright and away, the far knee punching up and across, the hand pushing off, ending
/// gathered in the air.
fn wall_kick(s: &Skeleton, w: f32) -> AnimationClip {
    let name = WALL_KICK[usize::from(w > 0.0)];
    let (hand, foot) = wall_side(w);
    let mut plant = [None; 4];
    plant[foot] = Some(Vec3::new(w * (WALL_DISTANCE - ANKLE_TO_SOLE), 0.34, 0.0));
    plant[hand] = Some(Vec3::new(w * (WALL_DISTANCE - 0.04), 1.32, -0.2));
    let near = usize::from(w > 0.0);
    let far = 1 - near;
    let mut legs = [[0.0; 4]; 2];
    legs[near] = [30.0, 10.0, 60.0, 0.0];
    legs[far] = [70.0, -10.0, 90.0, 5.0];
    let mut knee = [Vec3::NEG_Z; 2];
    knee[near] = Vec3::new(-0.3 * w, 0.4, -1.0);
    let on = Key {
        at: 0.0,
        plant,
        knee,
        effort: 0.9,
        ..pose(
            Vec3::new(0.0, 0.92, 0.0),
            -8.0,
            w * WALL_TILT,
            [-6.0, -5.0, 4.0, 4.0],
            [ARMS_OUT, ARMS_OUT],
            legs,
        )
    };
    let mut push_plant = [None; 4];
    push_plant[foot] = Some(Vec3::new(w * (WALL_DISTANCE - ANKLE_TO_SOLE), 0.3, 0.12));
    legs[far] = [90.0, -20.0, 105.0, 5.0];
    legs[near] = [0.0, 20.0, 5.0, -25.0];
    let mut arms = [ARMS_UP; 2];
    arms[usize::from(w > 0.0)] = Arm {
        abduct: 30.0,
        ..ARMS_UP
    };
    let push = Key {
        at: 0.14,
        plant: push_plant,
        knee,
        effort: 1.0,
        ..pose(
            Vec3::new(-w * 0.12, 0.98, 0.0),
            -10.0,
            w * 6.0,
            [-6.0, -4.0, 4.0, 2.0],
            arms,
            legs,
        )
    };
    let rise = air_keys()[2];
    let away = Key {
        at: 0.36,
        hips: Vec3::new(-w * 0.1, 0.98, 0.0),
        roll: -w * 8.0,
        ..rise
    };
    keyed_clip(s, name, 0.36, &[on, push, away], false, &[(0.0, 1.0)])
}

/// Sliding on the hip and thigh: the left leg out ahead with the heel skimming, the right
/// folded under, the trunk leaning back over the right hand trailing on the ground, the left
/// arm forward for balance. A loop that trembles a little with the ground.
fn slide(s: &Skeleton) -> AnimationClip {
    let base = Key {
        at: 0.0,
        plant: [None, Some(Vec3::new(0.34, 0.05, 0.3)), None, None],
        effort: 0.5,
        ..pose(
            Vec3::new(0.0, 0.3, 0.1),
            38.0,
            -6.0,
            [-10.0, -8.0, -6.0, -10.0],
            [
                Arm {
                    flex: 55.0,
                    abduct: 20.0,
                    elbow: 30.0,
                    ..Arm::HANG
                },
                Arm::HANG,
            ],
            [[75.0, 4.0, 12.0, 15.0], [10.0, 25.0, 120.0, -20.0]],
        )
    };
    let shake = Key {
        at: 0.3,
        hips: base.hips + Vec3::new(0.0, 0.012, 0.0),
        pitch: base.pitch - 1.5,
        ..base
    };
    let end = Key { at: 0.6, ..base };
    keyed_clip(s, SLIDE, 0.6, &[base, shake, end], true, &[(0.0, 1.0)])
}

/// Every parkour clip.
pub(super) fn clips(s: &Skeleton) -> Vec<AnimationClip> {
    let mut out = vec![jump(s), double_jump(s), land(s), land_hard(s), slide(s)];
    out.extend(air(s));
    for w in [-1.0, 1.0] {
        out.push(wall_run(s, w));
        out.push(wall_kick(s, w));
    }
    out
}

/// The parkour states for the humanoid graph (beside `move`).
pub(super) fn states() -> Vec<StateDef> {
    let state = |name: &str, node: NodeDef| StateDef {
        name: name.into(),
        node,
        speed: 1.0,
    };
    let air = NodeDef::Blend1d {
        param: "air_vy".into(),
        children: [(-6.0, AIR[0]), (0.0, AIR[1]), (4.0, AIR[2])]
            .map(|(at, clip)| Blend1dChild {
                at,
                node: NodeDef::clip(clip),
            })
            .to_vec(),
        sync: false,
        match_speed: false,
    };
    let mut out = vec![
        state(JUMP, NodeDef::one_shot(JUMP)),
        state("air", air),
        state(DOUBLE_JUMP, NodeDef::one_shot(DOUBLE_JUMP)),
        state(LAND, NodeDef::one_shot(LAND)),
        state(LAND_HARD, NodeDef::one_shot(LAND_HARD)),
        state(SLIDE, NodeDef::clip(SLIDE)),
    ];
    for i in 0..2 {
        out.push(state(WALL_RUN[i], NodeDef::clip(WALL_RUN[i])));
        out.push(state(WALL_KICK[i], NodeDef::one_shot(WALL_KICK[i])));
    }
    out
}

/// The triggers [`crate::parkour`] sends, by what happened.
pub const TRIGGER_JUMP: &str = "parkour.jump";
pub const TRIGGER_DOUBLE_JUMP: &str = "parkour.double_jump";
pub const TRIGGER_LAND: &str = "parkour.land";
pub const TRIGGER_LAND_HARD: &str = "parkour.land_hard";
pub const TRIGGER_WALL_KICK: &str = "parkour.wall_kick";

/// The parkour transitions. Parameters ([`crate::parkour`]): `airborne` (1 in the air, off
/// any wall), `on_ground` (1 standing on something), `air_vy` (vertical speed, m/s),
/// `wall_run` (−1 on a wall to the left, 1 to the right, 0 off), `sliding`.
pub(super) fn transitions() -> Vec<TransitionDef> {
    let p = |name: &str, op: Cmp, value: f32| Condition::param(name, op, value);
    let t = TransitionDef::new;
    let airborne = || p("airborne", Cmp::Gt, 0.5);
    let grounded = || p("on_ground", Cmp::Gt, 0.5);
    let off_wall = || {
        Condition::All(vec![
            p("wall_run", Cmp::Gt, -0.5),
            p("wall_run", Cmp::Lt, 0.5),
        ])
    };
    let in_air = [JUMP, "air", DOUBLE_JUMP, WALL_KICK[0], WALL_KICK[1]];
    let mut out = vec![
        t("move", JUMP, Condition::trigger(TRIGGER_JUMP), 0.08).inertialize(),
        t(LAND, JUMP, Condition::trigger(TRIGGER_JUMP), 0.08).inertialize(),
        t(SLIDE, JUMP, Condition::trigger(TRIGGER_JUMP), 0.1).inertialize(),
        t(JUMP, "air", Condition::Finished, 0.15),
        t("move", "air", airborne(), 0.2).inertialize(),
        t(SLIDE, "air", airborne(), 0.2).inertialize(),
        t(
            "air",
            DOUBLE_JUMP,
            Condition::trigger(TRIGGER_DOUBLE_JUMP),
            0.05,
        )
        .inertialize(),
        t(
            JUMP,
            DOUBLE_JUMP,
            Condition::trigger(TRIGGER_DOUBLE_JUMP),
            0.05,
        )
        .inertialize(),
        t(DOUBLE_JUMP, "air", Condition::Finished, 0.2),
        t(LAND, "move", Condition::Finished, 0.25).inertialize(),
        t(LAND_HARD, "move", Condition::Finished, 0.3).inertialize(),
        t("move", SLIDE, p("sliding", Cmp::Gt, 0.5), 0.15).inertialize(),
        t(SLIDE, "move", p("sliding", Cmp::Lt, 0.5), 0.3).inertialize(),
    ];
    // Kicking off a wall comes before falling off it (the kick's trigger arrives with the
    // change of mode).
    for i in 0..2 {
        out.push(
            t(
                WALL_RUN[i],
                WALL_KICK[i],
                Condition::trigger(TRIGGER_WALL_KICK),
                0.05,
            )
            .inertialize(),
        );
        out.push(t(WALL_KICK[i], "air", Condition::Finished, 0.15));
    }
    for from in in_air {
        out.push(t(from, LAND_HARD, Condition::trigger(TRIGGER_LAND_HARD), 0.05).inertialize());
        out.push(t(from, LAND, Condition::trigger(TRIGGER_LAND), 0.05).inertialize());
        // Landing on the run: straight back into the stride.
        out.push(t(from, "move", grounded(), 0.15).inertialize());
    }
    for from in ["move", SLIDE].into_iter().chain(in_air) {
        out.push(t(from, WALL_RUN[1], p("wall_run", Cmp::Gt, 0.5), 0.12).inertialize());
        out.push(t(from, WALL_RUN[0], p("wall_run", Cmp::Lt, -0.5), 0.12).inertialize());
    }
    for run in WALL_RUN {
        out.push(
            t(
                run,
                "air",
                Condition::All(vec![off_wall(), airborne()]),
                0.2,
            )
            .inertialize(),
        );
        out.push(
            t(
                run,
                "move",
                Condition::All(vec![off_wall(), grounded()]),
                0.25,
            )
            .inertialize(),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::graph::{EvalCtx, GraphInstance};
    use engine_world::animation::AnimationParams;

    fn model_at(s: &Skeleton, clip: &AnimationClip, t: f32) -> Vec<Xf> {
        let binding = crate::clip::ClipBinding::new(clip, s);
        let mut local = s.rest.clone();
        clip.sample(t, &binding, &mut local);
        let mut model = s.rest_model.clone();
        s.to_model(&local, &mut model);
        model
    }

    #[test]
    fn wall_run_feet_stand_on_the_wall_and_the_hand_touches_it() {
        let s = skeleton();
        let clips = clips(&s);
        for (i, w) in [(0, -1.0f32), (1, 1.0)] {
            let clip = clips.iter().find(|c| c.name == WALL_RUN[i]).unwrap();
            let (hand, near) = wall_side(w);
            let mut planted = 0;
            for k in 0..40 {
                let t = clip.duration * k as f32 / 40.0;
                let model = model_at(&s, clip, t);
                for (limb, bone) in [(2, "foot_l"), (3, "foot_r")] {
                    let firm = clip.curve(WALL_CURVES[limb], t).unwrap();
                    if firm > 0.99 {
                        planted += 1;
                        let x = model[s.bone(bone)].t.x;
                        let want = w * (WALL_DISTANCE - ANKLE_TO_SOLE);
                        assert!((x - want).abs() < 0.03, "{bone} at {x}, wall {want}");
                    }
                }
                // The inside hand is out toward the wall, off it.
                let x = model[s.bone(if w > 0.0 { "hand_r" } else { "hand_l" })].t.x * w;
                assert!(x > 0.2 && x < WALL_DISTANCE, "hand at {x}");
                let _ = hand;
            }
            assert!(planted > 10, "{planted}");
            let _ = near;
            // The body leans off the wall: the head is farther from it than the hips.
            let model = model_at(&s, clip, 0.0);
            let head = model[s.bone("head")].t.x * w;
            let hips = model[s.bone("hips")].t.x * w;
            assert!(head < hips - 0.15, "head {head}, hips {hips}");
        }
    }

    fn step(
        g: &mut GraphInstance,
        s: &Skeleton,
        params: &AnimationParams,
        triggers: &mut Vec<(String, f32)>,
        secs: f32,
    ) {
        let mut out = s.rest.clone();
        let (mut events, mut curves) = (Vec::new(), Vec::new());
        for _ in 0..(secs * 60.0) as usize {
            let mut ctx = EvalCtx {
                params,
                triggers,
                dt: 1.0 / 60.0,
                events: &mut events,
                curves: &mut curves,
                root_delta: Vec3::ZERO,
            };
            g.evaluate(&mut ctx, &mut out);
            triggers.retain_mut(|t| {
                t.1 -= 1.0 / 60.0;
                t.1 > 0.0
            });
        }
    }

    #[test]
    fn the_graph_jumps_runs_a_wall_kicks_off_and_lands() {
        let s = Arc::new(skeleton());
        let all: Vec<Arc<AnimationClip>> = crate::build::humanoid::clips(&s)
            .into_iter()
            .map(Arc::new)
            .collect();
        let lookup = |n: &str| all.iter().find(|c| c.name == n).cloned();
        let mut g = GraphInstance::compile(&graph(), &s, &lookup).unwrap();
        let mut params = AnimationParams::default();
        let mut triggers = Vec::new();
        params.set("on_ground", 1.0);
        step(&mut g, &s, &params, &mut triggers, 0.2);
        assert!(g.in_state("move"));
        // Take-off.
        params.set("on_ground", 0.0);
        params.set("airborne", 1.0);
        params.set("air_vy", 4.0);
        triggers.push((TRIGGER_JUMP.into(), 0.3));
        step(&mut g, &s, &params, &mut triggers, 0.1);
        assert!(g.in_state(JUMP), "{:?}", g.active_states());
        step(&mut g, &s, &params, &mut triggers, 0.4);
        assert!(g.in_state("air"), "{:?}", g.active_states());
        // Onto a wall on the left.
        params.set("airborne", 0.0);
        params.set("wall_run", -1.0);
        step(&mut g, &s, &params, &mut triggers, 0.2);
        assert!(g.in_state(WALL_RUN[0]), "{:?}", g.active_states());
        // Kicked off it.
        params.set("wall_run", 0.0);
        params.set("airborne", 1.0);
        triggers.push((TRIGGER_WALL_KICK.into(), 0.3));
        step(&mut g, &s, &params, &mut triggers, 0.1);
        assert!(g.in_state(WALL_KICK[0]), "{:?}", g.active_states());
        // Onto the right wall straight from the kick.
        params.set("airborne", 0.0);
        params.set("wall_run", 1.0);
        step(&mut g, &s, &params, &mut triggers, 0.2);
        assert!(g.in_state(WALL_RUN[1]), "{:?}", g.active_states());
        // Dropping off and landing hard.
        params.set("wall_run", 0.0);
        params.set("airborne", 1.0);
        params.set("air_vy", -9.0);
        step(&mut g, &s, &params, &mut triggers, 0.3);
        assert!(g.in_state("air"), "{:?}", g.active_states());
        params.set("airborne", 0.0);
        params.set("on_ground", 1.0);
        triggers.push((TRIGGER_LAND_HARD.into(), 0.3));
        step(&mut g, &s, &params, &mut triggers, 0.1);
        assert!(g.in_state(LAND_HARD), "{:?}", g.active_states());
        step(&mut g, &s, &params, &mut triggers, 1.2);
        assert!(g.in_state("move"), "{:?}", g.active_states());
    }
}
