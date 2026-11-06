//! Poses for the traversal states (see `crate::traverse`): crouched on a perch, hugging a
//! pole, hanging and swinging from a bar, balancing along a line with the arms out, on a rock
//! or ice face hand by hand, crawling, rolling out of a landing, dropping onto a target.
//!
//! The same character space as the rest of the rig: feet (the capsule's bottom) at the
//! origin, +Y up, facing −Z, +X right. The right hand keeps the sword (as on a ledge); the
//! left hand is free.

use std::f32::consts::{PI, TAU};

use engine_core::math::{Quat, Vec3};

use super::{
    ANKLE_HEIGHT, AnimInput, Channels, Pose, Xf, left_hand, lerp, pitch, rx, ry, rz, smooth, sword,
    v3,
};

/// What the body does on a traversal element, as the animation sees it (character space).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TravAnim {
    /// Crouched on a small top under the feet; `squat` 0..1 is the landing still being
    /// absorbed.
    Perch { squat: f32 },
    /// Hugging an upright pole whose axis passes through `at` (feet height): going up at
    /// `speed` m/s (down negative), `radius` its radius.
    Pole { at: Vec3, speed: f32, radius: f32 },
    /// Hanging from a bar at `grip` (between the hands); `swing` rad about the bar, legs
    /// forward when positive, and its rate; `along` m/s shimmying sideways (+ right).
    Bar {
        grip: Vec3,
        swing: f32,
        rate: f32,
        along: f32,
    },
    /// Balancing along a line: `lean` rad (+ toward the right), walking at `speed` m/s
    /// (backward negative), `narrow` 0 on a wide beam .. 1 on a rope, the line `sag` m under
    /// the feet bouncing.
    Balance { lean: f32, speed: f32, narrow: f32 },
    /// On a rock or ice face (−Z): where each hand and foot holds, and which limb is moving
    /// (0 left hand, 1 right hand, 2 left foot, 3 right foot) and how far through its move.
    Face {
        hands: [Vec3; 2],
        feet: [Vec3; 2],
        moving: Option<(u8, f32)>,
        ice: bool,
    },
    /// Crawling on hands and knees at `speed` m/s.
    Crawl { speed: f32 },
    /// Rolling forward out of a landing, `t` 0..1 through it.
    Roll { t: f32 },
    /// Dropping onto someone below, `t` 0..1 (the strike at about 0.8).
    Takedown { t: f32 },
}

impl TravAnim {
    /// A small number per variant (for the animation's state switches).
    pub fn kind(&self) -> u8 {
        match self {
            TravAnim::Perch { .. } => 0,
            TravAnim::Pole { .. } => 1,
            TravAnim::Bar { .. } => 2,
            TravAnim::Balance { .. } => 3,
            TravAnim::Face { .. } => 4,
            TravAnim::Crawl { .. } => 5,
            TravAnim::Roll { .. } => 6,
            TravAnim::Takedown { .. } => 7,
        }
    }
}

/// The traversal animation's own state: cycles and springs.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct State {
    /// Step cycle along a line, crawl cycle, climb cycle on a pole, 0..1.
    cycle: f32,
    /// Balance arm wobble (fast corrections), rad.
    wobble: f32,
    wobble_v: f32,
    /// The bar's legs: a lagging second pendulum (rad from the arms' line) and its rate.
    legs: f32,
    legs_v: f32,
    secs: f32,
}

impl State {
    pub(crate) fn track(&mut self, t: Option<&TravAnim>, _input: &AnimInput, dt: f32) {
        self.secs += dt;
        let Some(t) = t else {
            self.legs *= 1.0 - (6.0 * dt).min(1.0);
            return;
        };
        match *t {
            TravAnim::Balance {
                speed,
                lean,
                narrow,
            } => {
                // A step every 0.55 m on a line (short, careful steps).
                self.cycle = (self.cycle + speed.abs() * dt / 1.1).fract();
                // Fast small arm corrections on top of the lean, more on a rope.
                let push = (self.secs * 7.3).sin() * 0.6 + (self.secs * 11.1).sin() * 0.4;
                let a = -60.0 * self.wobble - 6.0 * self.wobble_v
                    + push * (2.0 + 4.0 * narrow)
                    + lean * 20.0;
                self.wobble_v += a * dt;
                self.wobble += self.wobble_v * dt;
            }
            TravAnim::Crawl { speed } => {
                self.cycle = (self.cycle + speed.abs() * dt / 0.7).fract();
            }
            TravAnim::Pole { speed, .. } => {
                self.cycle = (self.cycle + speed.abs() * dt / 0.6).fract();
            }
            TravAnim::Bar {
                swing, rate, along, ..
            } => {
                // The legs trail the swing (underdamped, ζ ≈ 0.35) and whip through the
                // bottom (research: legs as a second pendulum).
                let w = 7.0;
                let target = -rate * 0.12;
                let a = -w * w * (self.legs - target) - 2.0 * 0.35 * w * self.legs_v;
                self.legs_v += a * dt;
                self.legs = (self.legs + self.legs_v * dt).clamp(-0.9, 0.9);
                let _ = swing;
                self.cycle = (self.cycle + along * dt / 0.5).rem_euclid(1.0);
            }
            _ => {}
        }
    }
}

const A: f32 = ANKLE_HEIGHT;

/// The body's pose (legs, pelvis, spine, head, whole-body rotation).
pub(crate) fn body(t: &TravAnim, s: &State) -> Pose {
    match *t {
        TravAnim::Perch { squat } => perch(squat, s),
        TravAnim::Pole { at, speed, radius } => pole(at, speed, radius, s),
        TravAnim::Bar { grip, swing, .. } => bar(grip, swing, s),
        TravAnim::Balance {
            lean,
            speed,
            narrow,
        } => balance(lean, speed, narrow, s),
        TravAnim::Face {
            hands,
            feet,
            moving,
            ice,
        } => face(hands, feet, moving, ice),
        TravAnim::Crawl { speed } => crawl(speed, s),
        TravAnim::Roll { t } => roll(t),
        TravAnim::Takedown { t } => takedown(t),
    }
}

/// The arms (sword in the right hand, the free left hand), given the body.
pub(crate) fn arms(t: &TravAnim, s: &State, body: &Pose, cur: Pose) -> Pose {
    let torso = body.torso_xf();
    let mut p = cur;
    p.free_r = 0.0;
    p.grip_l = 0.0;
    p.follow_l = 0.0;
    match *t {
        TravAnim::Perch { squat } => {
            // One hand down on the perch between the feet, the sword arm out for balance.
            p.hand_l = Xf::new(
                v3(-0.05, 0.12 + 0.05 * squat, -0.12),
                left_hand(Vec3::NEG_Y, Vec3::NEG_Z),
            );
            p.curl_l = 0.4;
            p.sword = sword(
                v3(0.45, 0.85 - 0.1 * squat, 0.05),
                v3(0.4, -0.6, 0.6),
                v3(0.0, -1.0, 0.0),
            );
            p.elbow = [
                torso.rot * v3(-0.6, -0.4, -0.5),
                torso.rot * v3(0.6, -0.5, 0.4),
            ];
        }
        TravAnim::Pole { at, radius, .. } => {
            // Hands round the pole, one above the other, swapping as the body climbs.
            let k = (s.cycle * TAU).sin();
            let hy = 1.55 + 0.12 * k;
            let ly = 1.25 - 0.12 * k;
            let r = radius + 0.04;
            p.hand_l = Xf::new(at + v3(-r * 0.8, ly, r * 0.5), left_hand(Vec3::X, Vec3::Y));
            p.sword = sword(at + v3(r * 0.8, hy, r * 0.5), v3(0.3, -0.7, 0.6), Vec3::X);
            p.curl_l = 0.95;
            p.elbow = [
                torso.rot * v3(-0.8, -0.5, 0.2),
                torso.rot * v3(0.8, -0.5, 0.2),
            ];
        }
        TravAnim::Bar { grip, along, .. } => {
            let k = (s.cycle * TAU).sin() * smooth(0.1, 0.6, along.abs());
            p.hand_l = Xf::new(
                grip + v3(-0.2 + 0.06 * k, -0.05, 0.03),
                left_hand(Vec3::Y, Vec3::NEG_Z),
            );
            p.sword = sword(
                grip + v3(0.2 + 0.06 * k, -0.05, 0.03),
                v3(0.3, -0.8, 0.4),
                Vec3::X,
            );
            p.curl_l = 1.0;
            p.elbow = [
                torso.rot * v3(-0.9, 0.0, 0.3),
                torso.rot * v3(0.9, 0.0, 0.3),
            ];
        }
        TravAnim::Balance { lean, narrow, .. } => {
            // Arms out near level, the arm line rolling against the lean, small fast
            // corrections on top (research: k_a ≈ 2 against the lean).
            let roll = -lean * 2.0 + s.wobble;
            let span = lerp(0.5, 0.68, narrow);
            let y = 1.3 + 0.05 * (1.0 - narrow);
            let l = v3(-span, y + roll * span * 0.5, 0.02);
            let r = v3(span, y - roll * span * 0.5, 0.02);
            p.hand_l = Xf::new(l, left_hand(Vec3::NEG_Y, Vec3::NEG_X));
            p.curl_l = 0.25;
            p.sword = sword(r, v3(0.6, -0.5, -0.4), v3(0.0, -1.0, 0.0));
            p.elbow = [v3(-0.2, -1.0, 0.2), v3(0.2, -1.0, 0.2)];
        }
        TravAnim::Face {
            hands, moving, ice, ..
        } => {
            // Each hand on its hold; a hand on the move reaches in an arc out of the wall.
            let lift = |i: u8| match moving {
                Some((m, u)) if m == i => (u * PI).sin(),
                _ => 0.0,
            };
            let hl = hands[0] + v3(0.0, 0.0, 0.12 * lift(0));
            let hr = hands[1] + v3(0.0, 0.0, 0.12 * lift(1));
            p.hand_l = Xf::new(
                hl,
                if ice {
                    left_hand(Vec3::NEG_Z, Vec3::Y)
                } else {
                    left_hand(Vec3::NEG_Z, Vec3::NEG_Y)
                },
            );
            p.curl_l = 0.9;
            // The sword hand holds the pick (ice) or the hold, blade up out of the way.
            p.sword = sword(hr, v3(0.0, 0.8, 0.4), v3(0.0, 0.0, -1.0));
            p.elbow = [
                torso.rot * v3(-0.7, -0.6, 0.4),
                torso.rot * v3(0.7, -0.6, 0.4),
            ];
        }
        TravAnim::Crawl { .. } => {
            // Hands on the ground under the shoulders, alternating with the knees.
            let c = s.cycle * TAU;
            let step = |ph: f32| v3(0.0, 0.06 * ph.sin().max(0.0), -0.18 * ph.cos());
            p.hand_l = Xf::new(
                v3(-0.2, 0.06, -0.6) + step(c + PI),
                left_hand(Vec3::NEG_Y, Vec3::NEG_Z),
            );
            p.curl_l = 0.2;
            p.sword = sword(
                v3(0.2, 0.08, -0.6) + step(c),
                v3(0.2, 0.05, 1.0),
                v3(1.0, 0.0, 0.0),
            );
            p.elbow = [v3(-0.4, 0.0, 0.6), v3(0.4, 0.0, 0.6)];
        }
        TravAnim::Roll { t } => {
            // Tucked: hands to the shins.
            let k = (t * PI).sin();
            p.hand_l = Xf::new(v3(-0.18, 0.75, -0.3), Quat::IDENTITY);
            p.curl_l = 0.8;
            p.sword = sword(v3(0.18, 0.78, -0.3), v3(0.2, 0.4, 1.0), Vec3::NEG_Y);
            p.elbow = [v3(-1.0, -0.2, 0.2), v3(1.0, -0.2, 0.2)];
            let _ = k;
        }
        TravAnim::Takedown { t } => {
            // Sword raised high, then driven down at the strike.
            let strike = smooth(0.6, 0.85, t);
            let up = sword(v3(0.25, 1.75, 0.0), v3(0.0, 1.0, 0.3), v3(0.0, 0.0, -1.0));
            let down = sword(v3(0.15, 0.7, -0.45), v3(0.0, -1.0, -0.2), v3(0.0, 0.0, 1.0));
            p.sword = up.lerp(down, strike);
            p.grip_l = 1.0;
            p.elbow = [v3(-0.6, -0.3, 0.5), v3(0.6, -0.3, 0.5)];
        }
    }
    let mut out = cur;
    out.blend(&p, 1.0, Channels::Arms);
    out
}

fn perch(squat: f32, s: &State) -> Pose {
    let mut p = Pose::BIND;
    // Deep crouch on the balls of the feet, feet close, knees wide, torso pitched forward,
    // head up scanning (AC3's V-pose).
    let breathe = (s.secs * 1.6).sin() * 0.01;
    let drop = 0.08 * squat;
    p.pelvis = v3(0.0, 0.5 - drop + breathe, 0.12);
    p.pelvis_rot = pitch(0.45);
    p.ankle = [v3(-0.08, A + 0.04, -0.02), v3(0.08, A + 0.04, 0.03)];
    p.foot = [pitch(0.75) * ry(0.25), pitch(0.75) * ry(-0.25)];
    p.knee = [v3(-0.8, 0.2, -1.0), v3(0.8, 0.2, -1.0)];
    p.spine = pitch(0.35 + 0.2 * squat);
    p.chest = pitch(0.15);
    p.neck = pitch(-0.25);
    p.head = pitch(-0.45) * ry((s.secs * 0.4).sin() * 0.35);
    p
}

fn pole(at: Vec3, speed: f32, radius: f32, s: &State) -> Pose {
    let mut p = Pose::BIND;
    // Inchworm: knees up, soles on the pole, then stand up on them (hips swing in and out).
    let c = s.cycle * TAU;
    let moving = smooth(0.1, 0.6, speed.abs());
    let tuck = (0.5 - 0.5 * c.cos()) * moving;
    let behind = at.z + radius + 0.28;
    p.pelvis = v3(0.0, 0.9 + 0.12 * tuck, behind);
    p.pelvis_rot = pitch(-0.1);
    let foot_y = A + 0.15 + 0.35 * tuck;
    p.ankle = [
        v3(-0.1, foot_y, at.z + radius + 0.06),
        v3(0.1, foot_y + 0.05, at.z + radius + 0.06),
    ];
    p.foot = [pitch(-0.5) * ry(0.5), pitch(-0.5) * ry(-0.5)];
    p.knee = [v3(-0.7, 0.3, -1.0), v3(0.7, 0.3, -1.0)];
    p.spine = pitch(0.1);
    p.chest = pitch(0.05);
    p.head = pitch(-0.35);
    p
}

fn bar(grip: Vec3, swing: f32, s: &State) -> Pose {
    let mut p = Pose::BIND;
    // Hanging straight under the hands (arms straight: weight), the whole body swinging about
    // the bar; the legs trail as a second pendulum and pike toward the front of the swing.
    let hang = grip.y - 0.85 - 0.97;
    p.pelvis = v3(0.0, 0.97 + hang + 0.05, grip.z + 0.05);
    p.spine = pitch(-0.05);
    p.chest = pitch(-0.1);
    p.head = pitch(-0.25);
    let pike = (swing * 0.6 + s.legs).clamp(-0.6, 0.9);
    let leg = |x: f32| {
        let dir = (rx(pike) * Vec3::NEG_Y).normalize();
        v3(x, p.pelvis.y - 0.08, p.pelvis.z) + dir * 0.82
    };
    p.ankle = [leg(-0.08), leg(0.08)];
    p.foot = [pitch(0.6 + pike * 0.3); 2];
    p.knee = [v3(-0.05, 0.0, -1.0), v3(0.05, 0.0, -1.0)];
    p.body = rx(-swing);
    p.pivot = grip;
    p
}

fn balance(lean: f32, speed: f32, narrow: f32, s: &State) -> Pose {
    let mut p = Pose::BIND;
    // Feet in line, one in front of the other, toes a little out; knees soft; careful steps.
    let c = s.cycle * TAU;
    let moving = smooth(0.05, 0.5, speed.abs());
    let stride = 0.24 * moving;
    let dir = if speed < 0.0 { -1.0 } else { 1.0 };
    let foot = |ph: f32, x: f32| {
        let z = -dir * stride * ph.sin();
        let lift = 0.07 * moving * ph.cos().max(0.0);
        v3(x, A + lift, z)
    };
    let w = lerp(0.07, 0.015, narrow);
    p.ankle = [foot(c, -w), foot(c + PI, w)];
    p.foot = [ry(0.3), ry(-0.3)];
    p.knee = [v3(-0.25, 0.0, -1.0), v3(0.25, 0.0, -1.0)];
    p.pelvis = v3(0.0, 0.9 - 0.04 * narrow, 0.0);
    // Hip strategy: the pelvis shifts against the lean (an S through the body).
    p.pelvis.x -= lean * 0.18;
    p.spine = pitch(0.06);
    p.chest = rz(lean * 0.4);
    // The head stays level, looking along the line.
    p.head = rz(lean * 0.7) * pitch(0.15);
    p.body = rz(-lean);
    p.pivot = Vec3::ZERO;
    p
}

fn face(hands: [Vec3; 2], feet: [Vec3; 2], moving: Option<(u8, f32)>, ice: bool) -> Pose {
    let mut p = Pose::BIND;
    // Pelvis between the feet and the hands, held off the wall (research: α ≈ 0.4, 0.2 m
    // off the wall plane); it shifts toward the supports while a limb is on the move.
    let fm = (feet[0] + feet[1]) * 0.5;
    let hm = (hands[0] + hands[1]) * 0.5;
    let mut pel = fm + (hm - fm) * 0.42;
    let wall = hands[0].z.min(hands[1].z).min(feet[0].z).min(feet[1].z);
    pel.z = wall + if ice { 0.32 } else { 0.24 };
    if let Some((m, u)) = moving {
        let away = match m {
            0 => v3(0.06, -0.02, 0.0),
            1 => v3(-0.06, -0.02, 0.0),
            _ => v3(0.0, 0.04, 0.0),
        };
        pel += away * (u * PI).sin();
    }
    p.pelvis = pel;
    p.pelvis_rot = pitch(-0.1);
    let lift = |i: u8| match moving {
        Some((m, u)) if m == i => (u * PI).sin(),
        _ => 0.0,
    };
    p.ankle = [
        feet[0] + v3(0.0, 0.08 * lift(2), 0.1 + 0.1 * lift(2)),
        feet[1] + v3(0.0, 0.08 * lift(3), 0.1 + 0.1 * lift(3)),
    ];
    p.foot = [pitch(if ice { -0.1 } else { 0.2 }); 2];
    p.knee = [v3(-0.6, 0.2, -1.0), v3(0.6, 0.2, -1.0)];
    p.spine = pitch(-0.05);
    p.chest = pitch(-0.12);
    // Look up at the next hold.
    p.head = pitch(-0.5);
    p
}

fn crawl(speed: f32, s: &State) -> Pose {
    let mut p = Pose::BIND;
    let c = s.cycle * TAU;
    let moving = smooth(0.05, 0.5, speed.abs());
    // Hands and knees: the pelvis over the knees, the back near level.
    p.pelvis = v3(0.0, 0.5 + 0.015 * (2.0 * c).sin() * moving, 0.32);
    p.pelvis_rot = pitch(1.25);
    p.spine = pitch(0.1);
    p.chest = pitch(0.05);
    p.neck = pitch(-0.6);
    p.head = pitch(-0.5);
    let knee = |ph: f32, x: f32| {
        let z = 0.62 + 0.14 * ph.cos() * moving;
        v3(x, A + 0.05 + 0.04 * ph.sin().max(0.0) * moving, z)
    };
    p.ankle = [knee(c, -0.13), knee(c + PI, 0.13)];
    p.foot = [pitch(-0.3); 2];
    p.knee = [v3(-0.1, -1.0, -0.5), v3(0.1, -1.0, -0.5)];
    p
}

fn roll(t: f32) -> Pose {
    let mut p = Pose::BIND;
    // Tucked ball rolling forward over the shoulder: the whole body turns once about a pivot
    // moving along the ground.
    let tuck = smooth(0.0, 0.15, t) * (1.0 - smooth(0.8, 1.0, t));
    p.pelvis = v3(0.0, lerp(0.95, 0.55, tuck), 0.0);
    p.ankle = [
        v3(-0.12, A + 0.4 * tuck, lerp(0.0, -0.05, tuck)),
        v3(0.12, A + 0.4 * tuck, lerp(0.0, -0.05, tuck)),
    ];
    p.knee = [v3(-0.2, 0.6, -1.0), v3(0.2, 0.6, -1.0)];
    p.foot = [pitch(0.6 * tuck); 2];
    p.spine = pitch(0.7 * tuck);
    p.chest = pitch(0.4 * tuck);
    p.neck = pitch(0.3 * tuck);
    p.head = pitch(0.4 * tuck);
    let turn = smooth(0.05, 0.85, t);
    p.body = rx(-TAU * turn) * rz(0.15 * (turn * PI).sin());
    p.pivot = v3(0.0, lerp(0.95, 0.45, tuck), -0.05);
    p
}

fn takedown(t: f32) -> Pose {
    let mut p = Pose::BIND;
    // Falling with the knees up, then driving down onto the target.
    let strike = smooth(0.6, 0.85, t);
    p.pelvis = v3(0.0, lerp(0.95, 0.7, strike), 0.0);
    p.ankle = [
        v3(-0.14, A + lerp(0.45, 0.05, strike), 0.15),
        v3(0.14, A + lerp(0.5, 0.0, strike), -0.2),
    ];
    p.knee = [v3(-0.3, 0.3, -1.0), v3(0.3, 0.3, -1.0)];
    p.spine = pitch(lerp(-0.1, 0.5, strike));
    p.chest = pitch(lerp(-0.15, 0.3, strike));
    p.head = pitch(lerp(0.3, -0.2, strike));
    p
}
