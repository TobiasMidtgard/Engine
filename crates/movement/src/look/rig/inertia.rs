//! Inertialization of the puppet's pose (Bollo, Gears of War GDC 2018; Holden's spring form):
//! on a discrete switch of state (a grab, a landing, letting go of a rope, the end of a mantle)
//! the new state's pose shows at once, plus an offset from where the old pose was (and how fast
//! it moved) that decays to nothing through a critically damped spring. Unlike a cross-fade of
//! two poses, the motion keeps its velocity through the switch, and a switch during a switch
//! chains cleanly (the offset just accumulates).

use engine_core::math::{Quat, Vec3};

use super::Pose;

const NV: usize = 11;
const NQ: usize = 12;
const NF: usize = 4;

/// A pose as flat channels: positions, rotations, scalars.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Channels {
    v: [Vec3; NV],
    q: [Quat; NQ],
    f: [f32; NF],
}

impl Channels {
    pub(crate) fn of(p: &Pose) -> Self {
        Self {
            v: [
                p.pelvis,
                p.ankle[0],
                p.ankle[1],
                p.knee[0],
                p.knee[1],
                p.sword.pos,
                p.hand_r.pos,
                p.hand_l.pos,
                p.elbow[0],
                p.elbow[1],
                p.pivot,
            ],
            q: [
                p.pelvis_rot,
                p.spine,
                p.chest,
                p.neck,
                p.head,
                p.foot[0],
                p.foot[1],
                p.sword.rot,
                p.hand_r.rot,
                p.hand_l.rot,
                p.sheath,
                p.body,
            ],
            f: [p.free_r, p.grip_l, p.follow_l, p.curl_l],
        }
    }

    fn write(&self, p: &mut Pose) {
        let v = self.v;
        p.pelvis = v[0];
        p.ankle = [v[1], v[2]];
        p.knee = [v[3], v[4]];
        p.sword.pos = v[5];
        p.hand_r.pos = v[6];
        p.hand_l.pos = v[7];
        p.elbow = [v[8], v[9]];
        p.pivot = v[10];
        let q = self.q;
        p.pelvis_rot = q[0];
        p.spine = q[1];
        p.chest = q[2];
        p.neck = q[3];
        p.head = q[4];
        p.foot = [q[5], q[6]];
        p.sword.rot = q[7];
        p.hand_r.rot = q[8];
        p.hand_l.rot = q[9];
        p.sheath = q[10];
        p.body = q[11];
        let f = self.f;
        p.free_r = f[0];
        p.grip_l = f[1];
        p.follow_l = f[2];
        p.curl_l = f[3];
    }
}

/// Which channels a planted contact owns (not offset: a planted foot must not slide).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Locked {
    pub feet: bool,
    pub hands: bool,
}

/// Scaled angle-axis of a (shortest-way) rotation.
fn log(q: Quat) -> Vec3 {
    let q = if q.w < 0.0 { -q } else { q };
    let (axis, angle) = q.to_axis_angle();
    if angle.abs() < 1e-6 || !axis.is_finite() {
        Vec3::ZERO
    } else {
        axis * angle
    }
}

fn exp(v: Vec3) -> Quat {
    let a = v.length();
    if a < 1e-6 {
        Quat::IDENTITY
    } else {
        Quat::from_axis_angle(v / a, a)
    }
}

/// Holden's exact critically damped decay of `x` (with velocity `v`) toward zero.
fn decay(x: &mut f32, v: &mut f32, halflife: f32, dt: f32) {
    let y = 4.0 * std::f32::consts::LN_2 / (halflife + 1e-5) / 2.0;
    let j1 = *v + *x * y;
    let e = (-y * dt).exp();
    *x = e * (*x + j1 * dt);
    *v = e * (*v - j1 * y * dt);
}

fn decay3(x: &mut Vec3, v: &mut Vec3, halflife: f32, dt: f32) {
    for i in 0..3 {
        decay(&mut x[i], &mut v[i], halflife, dt);
    }
}

/// The offset layer. Feed it every frame's raw pose with [`Inertia::apply`]; call
/// [`Inertia::switch`] when the state changes discretely.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Inertia {
    /// The output and its velocity last frame.
    last: Option<(Channels, Channels)>,
    ov: [Vec3; NV],
    ovv: [Vec3; NV],
    oq: [Vec3; NQ],
    oqv: [Vec3; NQ],
    of: [f32; NF],
    ofv: [f32; NF],
    halflife: f32,
    pending: bool,
}

impl Inertia {
    /// The state switched this frame: blend out of where the pose was over about `halflife`
    /// seconds (Holden's knob: 0.06 snappy for impacts, 0.12 normal, 0.25 into idles).
    pub(crate) fn switch(&mut self, halflife: f32) {
        self.pending = true;
        self.halflife = halflife.max(0.01);
    }

    /// Applies the offsets to `pose` (the raw pose of the current state) and advances them.
    pub(crate) fn apply(&mut self, pose: &mut Pose, locked: Locked, dt: f32) {
        let raw = Channels::of(pose);
        let dt = dt.max(1e-4);
        if self.pending {
            self.pending = false;
            if let Some((out, out_vel)) = self.last {
                // Velocity of the new state's pose: from its last raw frame if it had one (a
                // switch of state rarely jumps the raw pose's motion much), else none.
                // (The new pose's own velocity isn't known yet: its first frame is the jump
                // being hidden. The offset carries the old motion on and eases out of it.)
                for i in 0..NV {
                    self.ov[i] = out.v[i] - raw.v[i];
                    self.ovv[i] = out_vel.v[i];
                }
                for i in 0..NQ {
                    self.oq[i] = log(out.q[i] * raw.q[i].inverse());
                    self.oqv[i] = out_vel.q[i].xyz() * 2.0;
                }
                for i in 0..NF {
                    self.of[i] = out.f[i] - raw.f[i];
                    self.ofv[i] = out_vel.f[i];
                }
            }
        }
        let h = self.halflife.max(0.01);
        for i in 0..NV {
            decay3(&mut self.ov[i], &mut self.ovv[i], h, dt);
        }
        for i in 0..NQ {
            decay3(&mut self.oq[i], &mut self.oqv[i], h, dt);
        }
        for i in 0..NF {
            decay(&mut self.of[i], &mut self.ofv[i], h, dt);
        }
        if locked.feet {
            for i in [1, 2] {
                self.ov[i] = Vec3::ZERO;
                self.ovv[i] = Vec3::ZERO;
            }
            for i in [5, 6] {
                self.oq[i] = Vec3::ZERO;
                self.oqv[i] = Vec3::ZERO;
            }
        }
        if locked.hands {
            for i in [5, 6, 7] {
                self.ov[i] = Vec3::ZERO;
                self.ovv[i] = Vec3::ZERO;
            }
            for i in [7, 8, 9] {
                self.oq[i] = Vec3::ZERO;
                self.oqv[i] = Vec3::ZERO;
            }
        }
        let mut out = raw;
        for i in 0..NV {
            out.v[i] += self.ov[i];
        }
        for i in 0..NQ {
            out.q[i] = (exp(self.oq[i]) * raw.q[i]).normalize();
        }
        for i in 0..NF {
            out.f[i] += self.of[i];
        }
        out.write(pose);
        let vel = match self.last {
            Some((prev, _)) => velocity(&prev, &out, dt),
            None => zero(),
        };
        self.last = Some((out, vel));
    }
}

fn zero() -> Channels {
    Channels {
        v: [Vec3::ZERO; NV],
        q: [Quat::from_xyzw(0.0, 0.0, 0.0, 0.0); NQ],
        f: [0.0; NF],
    }
}

/// Finite-difference velocity from `a` to `b` over `dt`: positions in m/s, rotations as
/// half angular velocity in a quaternion's xyz (log/2 per second), scalars per second.
fn velocity(a: &Channels, b: &Channels, dt: f32) -> Channels {
    let mut out = zero();
    for i in 0..NV {
        out.v[i] = (b.v[i] - a.v[i]) / dt;
    }
    for i in 0..NQ {
        let w = log(b.q[i] * a.q[i].inverse()) / dt;
        out.q[i] = Quat::from_xyzw(w.x * 0.5, w.y * 0.5, w.z * 0.5, 0.0);
    }
    for i in 0..NF {
        out.f[i] = (b.f[i] - a.f[i]) / dt;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_switch_starts_from_the_old_pose_and_ends_on_the_new() {
        let mut inert = Inertia::default();
        let dt = 1.0 / 60.0;
        let mut old = Pose::BIND;
        old.pelvis.y = 0.97;
        for _ in 0..5 {
            let mut p = old;
            inert.apply(&mut p, Locked::default(), dt);
        }
        // Switch to a pose 0.3 m lower: the first frame is still where the old one was.
        let mut new = Pose::BIND;
        new.pelvis.y = 0.67;
        new.spine = Quat::from_rotation_x(0.5);
        inert.switch(0.12);
        let mut p = new;
        inert.apply(&mut p, Locked::default(), dt);
        assert!(p.pelvis.y > 0.9, "{}", p.pelvis.y);
        let mut prev = p.pelvis.y;
        let mut max_step: f32 = 0.0;
        for _ in 0..60 {
            let mut p = new;
            inert.apply(&mut p, Locked::default(), dt);
            max_step = max_step.max((p.pelvis.y - prev).abs());
            prev = p.pelvis.y;
        }
        assert!((prev - 0.67).abs() < 0.01, "{prev}");
        // No pop: never more than a few cm in a frame.
        assert!(max_step < 0.05, "{max_step}");
        let mut p = new;
        inert.apply(&mut p, Locked::default(), dt);
        assert!(p.spine.angle_between(new.spine) < 0.02);
    }

    #[test]
    fn locked_feet_stay_planted() {
        let mut inert = Inertia::default();
        let dt = 1.0 / 60.0;
        let mut a = Pose::BIND;
        a.ankle[0].y = 0.5;
        inert.apply(&mut a, Locked::default(), dt);
        inert.switch(0.2);
        let b = Pose::BIND;
        let mut p = b;
        inert.apply(
            &mut p,
            Locked {
                feet: true,
                hands: false,
            },
            dt,
        );
        assert_eq!(p.ankle[0], b.ankle[0]);
    }
}
