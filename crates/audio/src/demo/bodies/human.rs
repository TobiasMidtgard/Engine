//! People: the hiker (red shell jacket, cargo trousers, boots, daypack, beanie) and the runner
//! (fitted running top, tights, running shoes), and how they walk and run.

use engine_core::math::{DVec3, PI, Quat, TAU, Vec2, Vec3, Vec4};
use engine_render::{Mesh, StandardMaterial};

use super::super::{TrailWalker, yaw};
use super::gait::{self, Gait};
use super::rig::{Xf, aim_down, curve, soft_min, two_bone};
use super::shape::{
    Frames, Grid, Key, Loft, Paint, bump, ellipsoid, key, lerp, merged, mirror_x, placed, rgb,
    sheet, smooth, white,
};
use super::textures::{Textures, albedo, normal};
use super::{Part, Parts};

pub const PELVIS: usize = 0;
pub const CHEST: usize = 1;
pub const HEAD: usize = 2;
/// Left, right.
pub const UPPER_ARM: [usize; 2] = [3, 4];
pub const FOREARM: [usize; 2] = [5, 6];
pub const HAND: [usize; 2] = [7, 8];
pub const THIGH: [usize; 2] = [9, 10];
pub const SHIN: [usize; 2] = [11, 12];
pub const FOOT: [usize; 2] = [13, 14];

const SIDES: [f32; 2] = [-1.0, 1.0];

/// A person's proportions, meters. Bone frames: pelvis at the hip joints' midpoint, chest at
/// the lumbar joint, head at the top of the neck, limbs hanging along −Y from their joint,
/// feet at the ankle with −Z forward.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Build {
    pub thigh: f32,
    pub shin: f32,
    /// Ankle joint above the sole.
    pub ankle: f32,
    /// Heel behind, ball and toe ahead of the ankle.
    pub heel: f32,
    pub ball: f32,
    pub toe: f32,
    /// Hip joints either side of the pelvis centre.
    pub hip_w: f32,
    /// Lumbar joint above the hip joints.
    pub lumbar: f32,
    /// Right shoulder joint from the lumbar joint.
    pub shoulder: Vec3,
    /// Head pivot (top of the neck) from the lumbar joint.
    pub neck: Vec3,
    pub upper_arm: f32,
    pub forearm: f32,
    pub runner: bool,
}

pub const HIKER: Build = Build {
    thigh: 0.445,
    shin: 0.43,
    ankle: 0.095,
    heel: 0.075,
    ball: 0.14,
    toe: 0.215,
    hip_w: 0.09,
    lumbar: 0.10,
    shoulder: Vec3::new(0.172, 0.385, 0.012),
    neck: Vec3::new(0.0, 0.585, 0.014),
    upper_arm: 0.29,
    forearm: 0.26,
    runner: false,
};

pub const RUNNER: Build = Build {
    thigh: 0.45,
    shin: 0.435,
    ankle: 0.085,
    heel: 0.07,
    ball: 0.13,
    toe: 0.2,
    hip_w: 0.085,
    lumbar: 0.10,
    shoulder: Vec3::new(0.166, 0.38, 0.01),
    neck: Vec3::new(0.0, 0.575, 0.012),
    upper_arm: 0.29,
    forearm: 0.255,
    runner: true,
};

// ---------------------------------------------------------------------------------------
// Animation
// ---------------------------------------------------------------------------------------

/// A planted foot `v` (0..1) of the way through its stance on the footprint `(centre,
/// heading)`: rolling onto the heel, flat, then peeling off over the ball.
fn stance_foot(b: &Build, g: &Gait, (centre, heading): (Vec3, Vec3), side: f32, v: f32) -> Xf {
    let flat = yaw(heading) * Quat::from_rotation_y(-side * g.toe_out);
    let fc = (b.toe - b.heel) * 0.5;
    let ankle = centre + flat * Vec3::new(0.0, b.ankle, fc);
    let (heel_up, flat_at, peel_at, toe_down) = if g.runs() {
        (0.12, 0.18, 0.42, 0.75)
    } else {
        (0.32, 0.16, 0.52, 0.85)
    };
    let pitch = if v < flat_at {
        heel_up * (1.0 - smooth(0.0, flat_at, v))
    } else if v > peel_at {
        -toe_down * smooth(peel_at, 1.0, v).powf(1.3)
    } else {
        0.0
    };
    let pivot = if pitch > 0.0 {
        Vec3::new(0.0, -b.ankle, b.heel - 0.02)
    } else {
        // A stiff sole rolls over the toe tip.
        Vec3::new(0.0, -b.ankle, -(b.toe - 0.012))
    };
    let at = ankle + flat * pivot;
    let r = flat * Quat::from_rotation_x(pitch);
    Xf::new(r, at - r * pivot)
}

/// The swinging foot `w` (0..1) of the way from lift-off to its next touchdown.
fn swing_foot(from: Xf, to: Xf, w: f32, run: bool) -> Xf {
    let (prog, lift, turn): (&[f32], &[f32], &[f32]) = if run {
        (
            &[0.0, 0.04, 0.22, 0.56, 0.86, 1.0],
            &[0.0, 0.17, 0.26, 0.17, 0.06, 0.0],
            &[0.0, 0.3, 0.55, 0.7, 0.86, 1.0],
        )
    } else {
        (
            &[0.0, 0.1, 0.44, 0.83, 1.0],
            &[0.0, 0.075, 0.045, 0.02, 0.0],
            &[0.0, 0.5, 0.72, 0.86, 1.0],
        )
    };
    let p = curve(prog, w);
    let t = from.t.lerp(to.t, p) + Vec3::Y * curve(lift, w).max(0.0);
    Xf::new(from.r.slerp(to.r, curve(turn, w).clamp(0.0, 1.0)), t)
}

/// Pose a person walking (or running) the trail: every bone in the walker's root space.
pub fn pose(w: &TrailWalker, b: &Build, out: &mut [Xf]) {
    let g = Gait::of(w);
    let run = g.runs();
    let c = gait::phase(w);
    let pc = PI * c;
    let (root_p, root_d) = gait::root(w);
    let inv = yaw(root_d).inverse();
    let local = |p: DVec3| inv * (p - root_p).as_vec3();

    let mut feet = [Xf::IDENTITY; 2];
    let mut planted = [false; 2];
    for f in 0..2u8 {
        let i = f as usize;
        let (u, td) = gait::foot_cycle(w, f);
        let (fp, fd, _) = gait::footprint(w, f, td);
        let here = (local(fp), inv * fd);
        if u < g.duty {
            feet[i] = stance_foot(b, &g, here, SIDES[i], u / g.duty);
            planted[i] = true;
        } else {
            let (np, nd, _) = gait::footprint(w, f, td + gait::stride(w) as f64);
            let from = stance_foot(b, &g, here, SIDES[i], 1.0);
            let to = stance_foot(b, &g, (local(np), inv * nd), SIDES[i], 0.0);
            feet[i] = swing_foot(from, to, (u - g.duty) / (1.0 - g.duty), run);
        }
    }

    // Pelvis: turns with the forward leg, drops on the swing side, sways over the stance
    // foot, and rises over it (walk) or sinks onto it (run).
    let leg = b.thigh + b.shin;
    let (yaw_amp, list_amp, sway_amp, bob, lean, hip_h) = if run {
        (0.13, 0.05, 0.012, 0.03, 0.12, leg * 0.925 + b.ankle)
    } else {
        (0.075, 0.05, 0.022, 0.014, 0.035, leg * 0.985 + b.ankle)
    };
    let psi = -yaw_amp * pc.cos();
    let list = -list_amp * (pc - 0.35).sin();
    let sway = -sway_amp * (pc - 0.3).sin();
    let desired = if run {
        hip_h - bob * (2.0 * PI * (c - g.duty)).cos()
    } else {
        hip_h - bob * (2.0 * pc).cos()
    };
    let pr =
        Quat::from_rotation_y(psi) * Quat::from_rotation_x(-lean) * Quat::from_rotation_z(list);
    let reach = leg * 0.994;
    let mut y = desired;
    for i in 0..2 {
        if !planted[i] {
            continue;
        }
        let off = pr * Vec3::new(SIDES[i] * b.hip_w, 0.0, 0.0);
        let a = feet[i].t;
        let horiz = Vec2::new(sway + off.x - a.x, off.z - a.z).length();
        let max = a.y - off.y + (reach * reach - horiz * horiz).max(0.0).sqrt();
        y = soft_min(y, max, 0.025);
    }
    let pelvis = Xf::new(pr, Vec3::new(sway, y, 0.0));
    out[PELVIS] = pelvis;

    // Legs: two-bone IK from each hip to its ankle, knees forward.
    let fwd = pr * Vec3::NEG_Z;
    for i in 0..2 {
        let hip = pelvis.point(Vec3::new(SIDES[i] * b.hip_w, 0.0, 0.0));
        let foot_fwd = feet[i].r * Vec3::NEG_Z;
        let pole = (fwd + Vec3::new(foot_fwd.x, 0.0, foot_fwd.z)).normalize_or(fwd)
            + Vec3::X * SIDES[i] * 0.1;
        let (knee, ankle) = two_bone(hip, feet[i].t, b.thigh, b.shin, pole);
        out[THIGH[i]] = Xf::new(aim_down(knee - hip, pole), hip);
        out[SHIN[i]] = Xf::new(aim_down(ankle - knee, pole), knee);
        out[FOOT[i]] = Xf::new(feet[i].r, ankle);
    }

    // Trunk: shoulders turn against the hips; the head stays level and looks down the trail.
    let (twist, extra_lean) = if run { (-0.85, 0.1) } else { (-0.7, 0.03) };
    let chest_yaw = twist * psi;
    let cr = Quat::from_rotation_y(chest_yaw)
        * Quat::from_rotation_x(-(lean + extra_lean))
        * Quat::from_rotation_z(-list * 0.75);
    let chest = Xf::new(cr, pelvis.point(Vec3::new(0.0, b.lumbar, 0.0)));
    out[CHEST] = chest;
    let look_down = if run { -0.05 } else { -0.09 };
    out[HEAD] = Xf::new(
        Quat::from_rotation_y(chest_yaw * 0.2) * Quat::from_rotation_x(look_down),
        chest.point(b.neck),
    );

    // Arms swing against the legs, a little behind them.
    let lag = if run { 0.06 } else { 0.1 };
    for i in 0..2 {
        let side = SIDES[i];
        let s = side * (PI * (c - lag)).cos();
        let (flex, abd, turn, elbow, wrist) = if run {
            (0.12 + 0.62 * s, 0.14, 0.42, 1.5 - 0.22 * s, 0.05)
        } else {
            (0.04 + 0.3 * s, 0.1, 0.22, 0.24 + 0.24 * s.max(0.0), 0.1)
        };
        let ur = cr
            * Quat::from_rotation_z(side * abd)
            * Quat::from_rotation_y(side * turn)
            * Quat::from_rotation_x(flex);
        let upper = Xf::new(
            ur,
            chest.point(Vec3::new(side * b.shoulder.x, b.shoulder.y, b.shoulder.z)),
        );
        let fore = Xf::new(
            ur * Quat::from_rotation_x(elbow),
            upper.point(Vec3::new(0.0, -b.upper_arm, 0.0)),
        );
        out[UPPER_ARM[i]] = upper;
        out[FOREARM[i]] = fore;
        out[HAND[i]] = Xf::new(
            fore.r * Quat::from_rotation_x(wrist),
            fore.point(Vec3::new(0.0, -b.forearm, 0.0)),
        );
    }
}

/// The rest pose the meshes are built in: standing straight, arms hanging at the sides.
pub fn rest(b: &Build, out: &mut [Xf]) {
    let at = |p: Vec3| Xf::new(Quat::IDENTITY, p);
    let pelvis = Vec3::new(0.0, b.thigh + b.shin + b.ankle, 0.0);
    let chest = pelvis + Vec3::new(0.0, b.lumbar, 0.0);
    out[PELVIS] = at(pelvis);
    out[CHEST] = at(chest);
    out[HEAD] = at(chest + b.neck);
    for (i, s) in SIDES.into_iter().enumerate() {
        let hip = pelvis + Vec3::new(s * b.hip_w, 0.0, 0.0);
        out[THIGH[i]] = at(hip);
        out[SHIN[i]] = at(hip - Vec3::Y * b.thigh);
        out[FOOT[i]] = at(hip - Vec3::Y * (b.thigh + b.shin));
        let shoulder = chest + Vec3::new(s * b.shoulder.x, b.shoulder.y, b.shoulder.z);
        out[UPPER_ARM[i]] = at(shoulder);
        out[FOREARM[i]] = at(shoulder - Vec3::Y * b.upper_arm);
        out[HAND[i]] = at(shoulder - Vec3::Y * (b.upper_arm + b.forearm));
    }
}

// ---------------------------------------------------------------------------------------
// Meshes
// ---------------------------------------------------------------------------------------

/// Cross-section on an upward spine: centre (0, y, z), half width, front and back radii.
fn up(y: f32, z: f32, w: f32, front: f32, back: f32) -> Key {
    key(Vec3::new(0.0, y, z), w, front).q2(front, back)
}

/// Cross-section on a downward spine (a limb): half width, front and back radii.
fn down(y: f32, w: f32, front: f32, back: f32) -> Key {
    key(Vec3::new(0.0, y, 0.0), w, back).q2(back, front)
}

/// Cross-section on a forward (−Z) spine: centre (0, y, z), half width, below and above.
fn fwd(z: f32, y: f32, w: f32, below: f32, above: f32) -> Key {
    key(Vec3::new(0.0, y, z), w, below).q2(below, above)
}

/// Garment and skin colours.
#[derive(Clone, Copy, Debug)]
struct Look {
    skin: Vec4,
    lips: Vec4,
    hair: Vec4,
    eyes: Vec4,
    stubble: f32,
}

fn look(b: &Build) -> Look {
    if b.runner {
        Look {
            skin: rgb(0x8a5a3c),
            lips: rgb(0x77372f),
            hair: rgb(0x1b1512),
            eyes: rgb(0x3b2414),
            stubble: 0.0,
        }
    } else {
        Look {
            skin: rgb(0xc99476),
            lips: rgb(0x9c5a50),
            hair: rgb(0x4a3322),
            eyes: rgb(0x4a6a7a),
            stubble: 0.5,
        }
    }
}

/// Head profile from the neck to the crown: y, half width, front, back, centre z, and the
/// front's superellipse exponent (a pointed chin, a broad brow).
const HEAD_PROFILE: [[f32; 6]; 16] = [
    [-0.17, 0.052, 0.038, 0.05, 0.012, 2.0],
    [-0.13, 0.052, 0.041, 0.051, 0.011, 2.0],
    [-0.105, 0.052, 0.047, 0.053, 0.009, 2.0],
    [-0.093, 0.049, 0.066, 0.057, 0.006, 2.0],
    [-0.084, 0.05, 0.078, 0.058, 0.004, 2.0],
    [-0.073, 0.054, 0.085, 0.059, 0.002, 2.1],
    [-0.057, 0.059, 0.087, 0.06, 0.0, 2.2],
    [-0.04, 0.063, 0.089, 0.062, 0.0, 2.3],
    [-0.022, 0.065, 0.09, 0.067, 0.0, 2.35],
    [-0.005, 0.069, 0.089, 0.074, 0.0, 2.5],
    [0.015, 0.073, 0.087, 0.082, 0.0, 2.6],
    [0.036, 0.075, 0.087, 0.09, 0.0, 2.6],
    [0.056, 0.076, 0.094, 0.097, 0.0, 2.5],
    [0.08, 0.077, 0.095, 0.1, 0.0, 2.4],
    [0.105, 0.072, 0.086, 0.096, 0.002, 2.3],
    [0.132, 0.057, 0.062, 0.076, 0.004, 2.2],
];

/// The face's relief at (x, y): how far the surface stands out from the head's profile.
fn face_relief(x: f32, y: f32) -> f32 {
    let ax = x.abs();
    let g = |cx: f32, cy: f32, sx: f32, sy: f32| {
        (-((ax - cx) / sx).powi(2) - ((y - cy) / sy).powi(2)).exp()
    };
    let nose_h = if y > 0.05 {
        0.0
    } else if y > 0.036 {
        0.004 * smooth(0.05, 0.036, y)
    } else if y > -0.002 {
        lerp(0.004, 0.026, smooth(0.036, -0.002, y).powf(0.8))
    } else {
        0.026 * (1.0 - smooth(-0.002, -0.016, y))
    };
    let nose_w = lerp(0.007, 0.0115, smooth(0.03, -0.004, y));
    let nose = nose_h * (-(x / nose_w).powi(2)).exp();
    nose + 0.007 * g(0.0155, -0.009, 0.006, 0.0055) // nostril wings
        - 0.0065 * g(0.033, 0.036, 0.016, 0.0095) // eye sockets
        + 0.005 * g(0.03, 0.055, 0.025, 0.0075) // brow ridge
        + 0.006 * g(0.049, 0.01, 0.015, 0.014) // cheekbones
        + 0.0045 * g(0.0, -0.031, 0.019, 0.0055) // upper lip
        + 0.005 * g(0.0, -0.047, 0.017, 0.005) // lower lip
        - 0.0025 * g(0.0, -0.0388, 0.024, 0.0016) // mouth
        + 0.006 * g(0.0, -0.073, 0.017, 0.01) // chin
        - 0.002 * g(0.0, -0.058, 0.015, 0.004) // under the lip
        - 0.0012 * g(0.0, -0.022, 0.0035, 0.006) // philtrum
}

/// The head's base shape (no face) from the neck up to the crown.
fn head_loft() -> Loft {
    let keys: Vec<Key> = HEAD_PROFILE
        .iter()
        .map(|&[y, w, f, bk, z, p]| up(y, z, w, f, bk).pow2(p, 2.1))
        .collect();
    Loft::new(keys, 120, 0.004).caps(0.01, HEAD_CROWN)
}

/// Height of the crown's dome above the last head profile ring, meters.
const HEAD_CROWN: f32 = 0.022;

/// Head and neck as a grid (head bone frame).
fn head_grid(b: &Build) -> Grid {
    let adam = !b.runner;
    head_loft().grid().deform(move |p, c, _, _| {
        let rel = Vec3::new(p.x, 0.0, p.z - c.z);
        let out = rel.normalize_or(Vec3::NEG_Z);
        let front = smooth(0.1, 0.75, -rel.z / rel.length().max(1e-5));
        let mut d = face_relief(p.x, p.y) * front;
        if adam {
            d += 0.004
                * bump(
                    p - Vec3::new(0.0, -0.112, -0.04),
                    Vec3::new(0.008, 0.01, 0.02),
                );
        }
        p + out * d
    })
}

/// The point on the face (front of the head grid) nearest to (x, y).
fn on_face(g: &Grid, x: f32, y: f32) -> Vec3 {
    g.pts
        .iter()
        .filter(|p| p.z < -0.02)
        .min_by(|a, b| {
            let da = (a.x - x).powi(2) + (a.y - y).powi(2);
            let db = (b.x - x).powi(2) + (b.y - y).powi(2);
            da.total_cmp(&db)
        })
        .copied()
        .unwrap_or(Vec3::new(x, y, -0.09))
}

fn head_mesh(b: &Build) -> Mesh {
    let lk = look(b);
    let grid = head_grid(b);
    let hiker = !b.runner;
    grid.mesh(move |v: &Paint| {
        let (x, y, z) = (v.p.x, v.p.y, v.p.z);
        let ax = x.abs();
        let mut c = lk.skin;
        // Lips: a thinner upper and a fuller lower lip either side of a dark mouth line.
        let lip = smooth(0.003, 0.0, (y + 0.0395).abs() - 0.0052) * smooth(0.024, 0.016, ax);
        c = c.lerp(lk.lips, lip * smooth(-0.07, -0.08, z) * 0.7);
        let line = smooth(0.0014, 0.0, (y + 0.0388).abs()) * smooth(0.022, 0.012, ax);
        c = c.lerp(
            c * Vec4::new(0.45, 0.35, 0.35, 1.0),
            line * smooth(-0.07, -0.08, z),
        );
        // Warmer cheeks, nose and ears; cooler under the eyes.
        let warm = (bump(
            Vec3::new(ax - 0.045, y - 0.005, 0.0),
            Vec3::new(0.02, 0.02, 1.0),
        ) + 0.7
            * bump(
                Vec3::new(x, y - 0.0, z + 0.11),
                Vec3::new(0.01, 0.015, 0.03),
            ))
        .min(1.0);
        c = c.lerp(c * Vec4::new(1.12, 0.9, 0.88, 1.0), warm * 0.6);
        let under_eye = bump(
            Vec3::new(ax - 0.033, y - 0.026, 0.0),
            Vec3::new(0.014, 0.006, 1.0),
        );
        c = c.lerp(c * Vec4::new(0.85, 0.82, 0.86, 1.0), under_eye * 0.5);
        if hiker {
            // Stubble on the jaw, chin and upper lip.
            let jaw = smooth(-0.012, -0.03, y) * smooth(-0.105, -0.085, y) * smooth(0.035, 0.0, z);
            let not_lip = 1.0 - lip;
            c = c.lerp(
                c * Vec4::new(0.62, 0.6, 0.62, 1.0),
                jaw * lk.stubble * 1.8 * not_lip,
            );
            // Short hair below the beanie at the back and sides.
            let nape = smooth(0.0, 0.03, z) * smooth(-0.06, -0.035, y) * smooth(0.065, 0.04, y);
            let side = smooth(0.062, 0.07, ax) * smooth(-0.02, 0.0, z) * smooth(0.03, 0.045, y);
            c = c.lerp(lk.hair, (nape + side).min(1.0) * 0.9);
        } else {
            // Close-cropped hair line at the nape and temples (under the hair cap).
            let nape = smooth(0.02, 0.05, z) * smooth(-0.07, -0.04, y);
            c = c.lerp(lk.hair, nape * 0.8);
        }
        c
    })
}

/// Eyes: whites, iris and pupil painted on a sphere looking down −Z.
fn eye_mesh(look: &Look) -> Mesh {
    let iris = look.eyes;
    ellipsoid(Vec3::ZERO, Vec3::splat(EYE_R), 32, move |v: &Paint| {
        let d = Vec2::new(v.p.x, v.p.y).length() / EYE_R;
        let white = rgb(0xd8cfc4).lerp(rgb(0xc9a59a), smooth(0.55, 0.95, d) * 0.5);
        if v.p.z > 0.0 || d > 0.52 {
            white
        } else if d > 0.46 {
            // Dark limbal ring.
            iris * 0.35
        } else if d > 0.2 {
            // Radial streaks in the iris, lighter round the pupil.
            let a = v.p.y.atan2(v.p.x);
            let streak = 0.85 + 0.15 * (a * 23.0).sin() * (a * 7.0).cos();
            iris * streak * lerp(1.25, 0.9, smooth(0.2, 0.45, d))
        } else {
            rgb(0x050505)
        }
    })
}

const EYE_R: f32 = 0.0118;

/// Where an eye sits in the head (side −1 left, +1 right): just behind the socket's surface.
fn eye_centre(b: &Build, side: f32) -> Vec3 {
    on_face(&head_grid(b), 0.032 * side, 0.036) + Vec3::new(0.0, 0.0, 0.0093)
}

/// Upper and lower lids over the right eye (eye frame): skin shells just outside the
/// eyeball, opening in an almond, the upper lid's rim dark with lashes.
fn lids_mesh(look: &Look) -> Mesh {
    let skin = look.skin;
    let r = EYE_R + 0.0009;
    let half = 0.0152;
    let lid = move |upper: bool| {
        let sign = if upper { 1.0 } else { -1.0 };
        // Height of the lid's rim across the eye.
        let rim = move |x: f32| {
            let t = (x / half).clamp(-1.0, 1.0);
            let open = if upper { 0.0052 } else { 0.0042 };
            sign * open * (1.0 - t * t) + 0.0008 * t
        };
        sheet(
            14,
            8,
            move |s, t| {
                let x = lerp(half, -half, s) * if upper { 1.0 } else { -1.0 };
                let y0 = rim(x);
                let y1 = sign * r;
                let y = lerp(y0, y1, t * t);
                let z2 = (r * r - x * x - y * y).max(0.0);
                // The rim stands a little proud of the eyeball.
                let proud = 0.0011 * (1.0 - t).powi(3);
                Vec3::new(x, y, -z2.sqrt() - proud)
            },
            |s, t| Vec2::new(s * 0.03, t * 0.012),
            move |_, t| {
                let lash = if upper {
                    smooth(0.12, 0.0, t)
                } else {
                    0.4 * smooth(0.08, 0.0, t)
                };
                let crease = if upper {
                    bump(Vec3::new(t - 0.6, 0.0, 0.0), Vec3::new(0.12, 1.0, 1.0))
                } else {
                    0.0
                };
                let c = skin * Vec4::new(0.97, 0.9, 0.9, 1.0);
                c.lerp(c * 0.72, crease * 0.6).lerp(rgb(0x15100d), lash)
            },
        )
    };
    merged([lid(true), lid(false)])
}

/// An ear on the right side of the head (head frame).
fn ear_mesh(look: &Look) -> Mesh {
    let skin = look.skin;
    let r = Vec3::new(0.0065, 0.031, 0.017);
    let m = Grid::surface(18, 32, |s, u| {
        let phi = s * PI;
        let a = u * TAU;
        let n = Vec3::new(-phi.cos(), phi.sin() * a.sin(), phi.sin() * a.cos());
        let mut p = n * r;
        // Cup the outer face into a bowl with a rolled rim (the helix).
        let rho = (phi.sin()).min(1.0);
        if n.x > 0.0 {
            p.x -= 0.0075 * (1.0 - rho * rho).max(0.0) * n.x;
        }
        // Lobe: thicker and rounder at the bottom.
        p.x *= 1.0 + 0.4 * smooth(0.0, -0.025, p.y);
        p
    })
    .mesh(move |v: &Paint| {
        let inner =
            smooth(0.4, 0.9, v.n.x) * smooth(0.028, 0.018, Vec2::new(v.p.y, v.p.z).length());
        skin.lerp(skin * Vec4::new(0.85, 0.62, 0.6, 1.0), inner * 0.7)
            * Vec4::new(1.04, 0.95, 0.94, 1.0)
    });
    // Tip the ear back and turn its front edge out from the head.
    placed(
        m,
        Quat::from_rotation_x(0.22) * Quat::from_rotation_y(0.35),
        Vec3::new(0.071, 0.014, 0.016),
    )
}

/// Eyebrow on the right (head frame), riding just over the brow.
fn brow_mesh(g: &Grid, look: &Look) -> Mesh {
    let pts = [
        (0.011, 0.0525),
        (0.022, 0.055),
        (0.036, 0.0558),
        (0.049, 0.0525),
    ];
    let keys: Vec<Key> = pts
        .iter()
        .enumerate()
        .map(|(i, &(x, y))| {
            let p = on_face(g, x, y) + Vec3::new(0.0, 0.0, -0.0008);
            let t = i as f32 / 3.0;
            key(p, lerp(0.0027, 0.0015, t), 0.001)
        })
        .collect();
    let hair = look.hair * 1.2;
    Loft::new(keys, 10, 0.004)
        .frames(Frames::Side(Vec3::Y))
        .caps(0.003, 0.004)
        .mesh(move |_| hair)
}

/// Beanie (hiker) or close-cropped hair (runner) over the head.
fn cap_mesh(b: &Build) -> Mesh {
    let hiker = !b.runner;
    let loft = head_loft();
    let last = *loft.keys.last().expect("head keys");
    let top = last.c.y + HEAD_CROWN;
    // The rings tilt with the head's spine (the crown sits a little back), so follow them.
    let spine = loft.clone();
    let along =
        move |y: f32| (spine.at_y(y + 0.003).c - spine.at_y(y - 0.003).c).normalize_or(Vec3::Y);
    let crown = along(last.c.y - 0.003);
    // The bare head's surface at height y and angle a (0 right, π/2 front, π left, 3π/2 back).
    let surface = move |y: f32, a: f32| -> Vec3 {
        if y <= last.c.y {
            let k = loft.at_y(y);
            loft.ring_point(&k, along(y), a)
        } else {
            let h = ((y - last.c.y) / HEAD_CROWN).clamp(0.0, 1.0);
            let scale = (1.0 - h * h).max(0.0).sqrt();
            let p = loft.ring_point(&last, crown, a);
            last.c + (p - last.c) * scale + crown * (y - last.c.y)
        }
    };
    let edge = move |a: f32| -> f32 {
        let front = a.sin().max(0.0);
        let back = (-a.sin()).max(0.0);
        if hiker {
            0.04 + 0.034 * front.powf(1.5) - 0.03 * back.powf(1.2)
        } else {
            let temple = (a.cos().abs() - 0.5).max(0.0) * front;
            0.06 + 0.038 * front.powf(1.2) - 0.08 * back.powf(1.6) - 0.012 * temple
        }
    };
    let grid = Grid::surface(44, 120, move |s, u| {
        let a = u * TAU;
        let y0 = edge(a);
        let y = lerp(y0, top, s.powf(1.15));
        let thick = if hiker {
            // Snug knit, a little looser over the crown.
            0.0065 + 0.004 * smooth(0.55, 1.0, s)
        } else {
            0.0045 + 0.006 * smooth(0.0, 0.05, y - y0) * smooth(0.4, 0.9, s.max(0.4))
        };
        let p = surface(y, a);
        let out = (p - Vec3::new(0.0, 0.03, 0.0)).normalize_or(Vec3::Y);
        let mut q = p + out * thick;
        if hiker {
            // A turned-up cuff (its folded edge rolled a little), and a little slouch at
            // the back.
            let h = y - y0;
            let cuff = smooth(0.036, 0.031, h);
            let roll = bump(Vec3::new(h - 0.031, 0.0, 0.0), Vec3::new(0.004, 1.0, 1.0));
            q += out * (0.0028 * cuff + 0.0015 * roll - 0.006 * smooth(0.004, 0.0, h));
            q.z += 0.007 * smooth(0.6, 1.0, s) * lerp(back_of(a), 1.0, s.powi(4));
        }
        q
    });
    let hair = look(b).hair;
    grid.mesh(move |v: &Paint| {
        if hiker {
            Vec4::ONE
        } else {
            hair * (0.85 + 0.15 * v.s)
        }
    })
}

fn back_of(a: f32) -> f32 {
    (-a.sin()).max(0.0)
}

/// A relaxed (or fisted) right hand: palm faces −X, thumb toward −Z, fingers down −Y.
fn hand_mesh(look: &Look, fist: bool) -> Mesh {
    let skin = look.skin;
    let paint = move |v: &Paint| {
        // Knuckles and fingertips a little redder, palm paler.
        let palm = smooth(0.2, 0.8, -v.n.x);
        skin.lerp(skin * Vec4::new(1.08, 0.97, 0.92, 1.0), palm * 0.6)
    };
    let palm = Loft::new(
        vec![
            down(0.035, 0.021, 0.029, 0.029),
            down(0.0, 0.02, 0.031, 0.03),
            down(-0.03, 0.022, 0.041, 0.038).pow(2.6),
            down(-0.068, 0.019, 0.043, 0.042).pow(3.2),
            down(-0.092, 0.016, 0.041, 0.04).pow(3.2),
        ],
        40,
        0.006,
    )
    .caps(0.0, 0.012)
    .mesh(paint);
    let mut parts = vec![palm];
    // Fingers: index (front) to little (back), each curling toward the palm.
    let fingers = [
        (-0.027, 0.084, 0.0094, 0.0),
        (-0.009, 0.094, 0.0098, 0.0),
        (0.01, 0.088, 0.0094, 0.004),
        (0.027, 0.07, 0.0082, 0.01),
    ];
    let curl: [f32; 3] = if fist {
        [1.35, 1.5, 0.9]
    } else {
        [0.28, 0.42, 0.3]
    };
    for (z, len, r, drop) in fingers {
        let mut p = Vec3::new(-0.003, -0.086 + drop, z);
        let mut dir = Vec3::NEG_Y;
        let mut keys = vec![key(p - dir * 0.012, r * 1.05, r * 0.95)];
        let segs = [0.45, 0.32, 0.23];
        let mut ang = 0.0;
        for (k, seg) in segs.iter().enumerate() {
            let n = 3;
            for j in 1..=n {
                let t = j as f32 / n as f32;
                p += dir * seg * len / n as f32;
                let along = (k as f32 + t) / 3.0;
                let rr = r * lerp(1.0, 0.78, along);
                keys.push(key(p, rr, rr * 0.92));
            }
            ang += curl[k];
            // Curl toward the palm (−X).
            dir = Vec3::new(-ang.sin(), -ang.cos(), 0.0);
        }
        parts.push(
            Loft::new(keys, 18, 0.006)
                .frames(Frames::Side(Vec3::Z))
                .caps(0.006, r * 0.8)
                .mesh(paint),
        );
    }
    // Thumb: from the heel of the hand, forward and across the palm.
    let thumb_pts: [(Vec3, f32); 5] = if fist {
        [
            (Vec3::new(-0.006, -0.012, -0.026), 0.014),
            (Vec3::new(-0.016, -0.04, -0.036), 0.013),
            (Vec3::new(-0.03, -0.062, -0.034), 0.011),
            (Vec3::new(-0.034, -0.078, -0.02), 0.0095),
            (Vec3::new(-0.032, -0.086, -0.008), 0.0088),
        ]
    } else {
        [
            (Vec3::new(-0.006, -0.012, -0.026), 0.014),
            (Vec3::new(-0.014, -0.04, -0.04), 0.013),
            (Vec3::new(-0.02, -0.064, -0.047), 0.011),
            (Vec3::new(-0.024, -0.084, -0.046), 0.0098),
            (Vec3::new(-0.025, -0.098, -0.042), 0.0088),
        ]
    };
    parts.push(
        Loft::new(
            thumb_pts
                .iter()
                .map(|&(p, r)| key(p, r, r * 0.88))
                .collect(),
            18,
            0.006,
        )
        .frames(Frames::Transport(Vec3::X))
        .caps(0.008, 0.008)
        .mesh(paint),
    );
    merged(parts)
}

/// Torso garment: the hiker's shell jacket or the runner's fitted top (chest frame).
fn torso_mesh(b: &Build) -> Mesh {
    let keys = if b.runner {
        vec![
            up(-0.12, 0.008, 0.17, 0.093, 0.108),
            up(-0.04, 0.006, 0.162, 0.09, 0.1),
            up(0.05, 0.004, 0.152, 0.088, 0.092),
            up(0.15, 0.0, 0.158, 0.098, 0.092),
            up(0.25, -0.004, 0.172, 0.108, 0.094),
            up(0.32, -0.002, 0.182, 0.102, 0.096),
            up(0.372, 0.004, 0.186, 0.086, 0.088).pow(2.5),
            up(0.415, 0.008, 0.15, 0.066, 0.072),
            up(0.45, 0.01, 0.086, 0.06, 0.062),
            up(0.468, 0.01, 0.064, 0.056, 0.058),
        ]
    } else {
        vec![
            up(-0.13, 0.008, 0.19, 0.105, 0.123),
            up(-0.1, 0.008, 0.188, 0.104, 0.121),
            up(-0.03, 0.006, 0.179, 0.101, 0.115),
            up(0.05, 0.004, 0.17, 0.099, 0.105),
            up(0.15, 0.0, 0.172, 0.107, 0.101),
            up(0.25, -0.004, 0.18, 0.116, 0.101),
            up(0.32, -0.002, 0.19, 0.111, 0.103).pow(2.3),
            up(0.375, 0.004, 0.194, 0.093, 0.093).pow(2.6),
            up(0.42, 0.008, 0.16, 0.073, 0.079).pow(2.3),
            up(0.455, 0.01, 0.1, 0.067, 0.069),
            up(0.48, 0.008, 0.078, 0.069, 0.073),
            up(0.525, 0.006, 0.076, 0.071, 0.074),
        ]
    };
    let runner = b.runner;
    let grid = Loft::new(keys, 96, 0.01)
        .caps(0.02, if runner { 0.006 } else { 0.008 })
        .grid()
        .deform(move |p, c, _, _| {
            // A few soft, broad folds where the fabric bunches above the hem, a chest that
            // fills the front, and shoulder blades behind.
            let r = Vec3::new(p.x, 0.0, p.z - c.z);
            let out = r.normalize_or(Vec3::X);
            let waist = smooth(-0.11, -0.08, p.y) * smooth(0.06, -0.02, p.y);
            let fold = (p.y * 70.0 + 9.0 * p.x).sin() * smooth(0.06, 0.12, p.x.abs());
            let amp = if runner { 0.0 } else { 0.0025 };
            let chest = bump(
                Vec3::new(p.x.abs() - 0.075, p.y - 0.27, 0.0),
                Vec3::new(0.06, 0.06, 1.0),
            ) * smooth(-0.02, -0.08, p.z - c.z);
            let blade = bump(
                Vec3::new(p.x.abs() - 0.07, p.y - 0.31, 0.0),
                Vec3::new(0.045, 0.06, 1.0),
            ) * smooth(0.02, 0.08, p.z - c.z);
            p + out * (fold * amp * waist + 0.006 * chest + 0.005 * blade)
        });
    let jacket = !runner;
    grid.mesh(move |v: &Paint| {
        let (x, y, z) = (v.p.x, v.p.y, v.p.z);
        let mut k = 1.0;
        if jacket {
            // Zip and its storm flap down the front, up the collar.
            if z < -0.03 {
                if x.abs() < 0.004 {
                    k = 0.05;
                } else if x.abs() < 0.011 {
                    k = 0.72;
                }
                // Chest pocket zip (left) and hand pocket zips.
                let d = seg_dist(
                    Vec2::new(x, y),
                    Vec2::new(-0.03, 0.3),
                    Vec2::new(-0.11, 0.255),
                );
                if d < 0.0028 {
                    k = 0.06;
                }
                for s in [-1.0f32, 1.0] {
                    let d = seg_dist(
                        Vec2::new(x, y),
                        Vec2::new(0.085 * s, -0.07),
                        Vec2::new(0.125 * s, 0.1),
                    );
                    if d < 0.0028 {
                        k = 0.06;
                    }
                }
            }
            // Yoke seam and the elastic hem.
            if (y - 0.335).abs() < 0.0025 {
                k *= 0.7;
            }
            if y < -0.115 {
                k *= 0.72;
            }
        } else {
            // Contrasting side panels and neckline trim.
            let side = smooth(0.12, 0.16, x.abs()) * smooth(0.0, -0.05, -(z.abs())) * 0.0;
            k *= 1.0 - side;
            if y > 0.445 || y < -0.112 {
                k *= 0.55;
            }
            if x.abs() > 0.13 && y > 0.05 && y < 0.3 {
                k *= 0.62;
            }
        }
        Vec4::new(k, k, k, 1.0)
    })
}

/// Distance from `p` to the segment `a`–`b`.
fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Trousers' seat (hiker) or tights (runner), pelvis frame.
fn pelvis_mesh(b: &Build) -> Mesh {
    let s = if b.runner { 0.93 } else { 1.0 };
    Loft::new(
        vec![
            up(-0.115, 0.01, 0.085 * s, 0.055 * s, 0.066 * s),
            up(-0.07, 0.008, 0.15 * s, 0.082 * s, 0.1 * s),
            up(-0.02, 0.006, 0.172 * s, 0.09 * s, 0.11 * s),
            up(0.04, 0.004, 0.172 * s, 0.09 * s, 0.108 * s),
            up(0.1, 0.004, 0.155 * s, 0.087 * s, 0.094 * s),
            up(0.14, 0.004, 0.142 * s, 0.08 * s, 0.084 * s),
        ],
        72,
        0.012,
    )
    .caps(0.03, 0.02)
    .mesh(|v: &Paint| {
        // Seams down the outside of the legs.
        let k = if v.p.z.abs() < 0.003 && v.p.x.abs() > 0.1 {
            0.75
        } else {
            1.0
        };
        Vec4::new(k, k, k, 1.0)
    })
}

fn thigh_mesh(b: &Build) -> Mesh {
    let keys = if b.runner {
        vec![
            down(0.0, 0.08, 0.08, 0.084),
            down(-0.06, 0.08, 0.079, 0.084),
            down(-0.15, 0.075, 0.079, 0.074),
            down(-0.27, 0.064, 0.068, 0.06),
            down(-0.38, 0.054, 0.055, 0.052),
            down(-0.45, 0.05, 0.052, 0.05),
        ]
    } else {
        vec![
            down(0.0, 0.088, 0.086, 0.09),
            down(-0.06, 0.087, 0.086, 0.09),
            down(-0.16, 0.08, 0.082, 0.082),
            down(-0.28, 0.07, 0.072, 0.07),
            down(-0.38, 0.061, 0.062, 0.06),
            down(-0.445, 0.058, 0.06, 0.057),
        ]
    };
    let cap = keys[0].x[0];
    let end = keys[keys.len() - 1].x[0];
    let runner = b.runner;
    Loft::new(keys, 56, 0.012)
        .caps(cap, end)
        .grid()
        .deform(move |p, _, _, _| {
            if runner {
                return p;
            }
            // Cargo pocket on the outside of the thigh.
            let pocket = smooth(-0.19, -0.21, p.y)
                * smooth(-0.345, -0.325, p.y)
                * smooth(-0.035, -0.02, p.z)
                * smooth(0.04, 0.025, p.z);
            Vec3::new(p.x + 0.007 * pocket * smooth(0.02, 0.05, p.x), p.y, p.z)
        })
        .mesh(move |v: &Paint| {
            let mut k = 1.0;
            if v.p.z.abs() < 0.0025 && v.p.x > 0.03 {
                k = 0.75;
            }
            if !runner {
                let flap = (v.p.y + 0.205).abs() < 0.004 && v.p.x > 0.04 && v.p.z.abs() < 0.035;
                if flap {
                    k = 0.7;
                }
            }
            Vec4::new(k, k, k, 1.0)
        })
}

fn shin_mesh(b: &Build) -> Mesh {
    let (keys, end_cap) = if b.runner {
        (
            vec![
                down(0.0, 0.05, 0.052, 0.05),
                down(-0.06, 0.05, 0.048, 0.056),
                down(-0.13, 0.05, 0.046, 0.064),
                down(-0.25, 0.04, 0.04, 0.044),
                down(-0.36, 0.031, 0.031, 0.033),
                down(-0.43, 0.031, 0.034, 0.031),
            ],
            0.02,
        )
    } else {
        (
            vec![
                down(0.0, 0.058, 0.06, 0.057),
                down(-0.06, 0.057, 0.056, 0.06),
                down(-0.14, 0.056, 0.052, 0.066),
                down(-0.25, 0.052, 0.05, 0.056),
                down(-0.33, 0.055, 0.055, 0.055),
                down(-0.37, 0.059, 0.059, 0.059),
                down(-0.385, 0.059, 0.059, 0.059),
            ],
            0.012,
        )
    };
    let cap = keys[0].x[0];
    let runner = b.runner;
    Loft::new(keys, 48, 0.012)
        .caps(cap, end_cap)
        .grid()
        .deform(move |p, _, _, u| {
            if runner {
                return p;
            }
            // Fabric stacking over the boot.
            let stack = smooth(-0.3, -0.35, p.y);
            let r = Vec3::new(p.x, 0.0, p.z).normalize_or(Vec3::X);
            p + r * (0.003 * stack * (p.y * 160.0 + 2.0 * (u * TAU * 2.0).sin()).sin())
        })
        .mesh(move |v: &Paint| {
            let k = if v.p.z.abs() < 0.0025 && v.p.x > 0.02 {
                0.75
            } else {
                1.0
            };
            let hem = if !runner && v.p.y < -0.375 { 0.7 } else { 1.0 };
            Vec4::new(k * hem, k * hem, k * hem, 1.0)
        })
}

/// Boot or running shoe upper (foot frame).
fn upper_mesh(b: &Build) -> Mesh {
    let runner = b.runner;
    let toe = b.toe;
    let (foot_keys, shaft_keys) = if runner {
        (
            vec![
                fwd(0.072, -0.047, 0.034, 0.024, 0.026),
                fwd(0.055, -0.042, 0.042, 0.03, 0.04),
                fwd(0.01, -0.036, 0.046, 0.036, 0.05),
                fwd(-0.05, -0.047, 0.049, 0.025, 0.034),
                fwd(-0.11, -0.055, 0.052, 0.017, 0.025),
                fwd(-0.155, -0.057, 0.048, 0.015, 0.019),
                fwd(-0.18, -0.057, 0.038, 0.014, 0.014),
            ],
            vec![
                up(-0.04, 0.008, 0.041, 0.044, 0.046),
                up(0.0, 0.01, 0.04, 0.042, 0.047),
                up(0.022, 0.012, 0.039, 0.036, 0.046),
            ],
        )
    } else {
        // Flat underneath where it sits on the sole (y = -0.07), a tall toe box, and a
        // padded shaft above the ankle.
        (
            vec![
                fwd(0.083, -0.036, 0.036, 0.034, 0.03).pow2(3.5, 2.2),
                fwd(0.066, -0.032, 0.046, 0.038, 0.046).pow2(3.5, 2.2),
                fwd(0.02, -0.026, 0.049, 0.044, 0.062).pow2(3.5, 2.2),
                fwd(-0.04, -0.038, 0.052, 0.032, 0.046).pow2(3.5, 2.2),
                fwd(-0.1, -0.047, 0.055, 0.023, 0.034).pow2(3.5, 2.3),
                fwd(-0.16, -0.049, 0.053, 0.021, 0.029).pow2(3.5, 2.6),
                fwd(-0.2, -0.051, 0.044, 0.019, 0.022).pow2(3.5, 2.6),
            ],
            vec![
                up(-0.05, 0.01, 0.047, 0.052, 0.05),
                up(0.02, 0.009, 0.046, 0.05, 0.05),
                up(0.07, 0.01, 0.048, 0.05, 0.052),
                up(0.088, 0.011, 0.054, 0.056, 0.058),
                up(0.1, 0.012, 0.052, 0.054, 0.057),
            ],
        )
    };
    let paint = move |v: &Paint| {
        let (x, y, z) = (v.p.x, v.p.y, v.p.z);
        if runner {
            // Pale grey mesh upper, a blue swoosh-like band and a white heel counter.
            let band = seg_dist(
                Vec2::new(z, y),
                Vec2::new(0.04, -0.03),
                Vec2::new(-0.1, -0.055),
            );
            let mut c = rgb(0xd9dde2);
            if band < 0.007 && x.abs() > 0.03 {
                c = rgb(0x1f5fbf);
            }
            if z > 0.045 && y < -0.02 {
                c = rgb(0xf2f2f2);
            }
            if y > 0.0 {
                c = rgb(0x2a2d33);
            }
            // Laces.
            if z < -0.005 && z > -0.1 && x.abs() < 0.014 && y > -0.045 && (z * 220.0).sin() > 0.3 {
                c = rgb(0xf4f4f4);
            }
            c
        } else {
            // Brown nubuck panels with darker leather overlays at the toe, heel and round
            // the lacing, a rubber rand along the bottom, and a padded collar.
            let nubuck = rgb(0x6a4a33);
            let leather = rgb(0x3b2a1e);
            let overlay = z < -(toe - 0.075)
                || z > 0.055
                || (z < 0.03 && x.abs() < 0.03 && y > -0.045)
                || (y < -0.058);
            let mut c = if overlay { leather } else { nubuck };
            // Stitching just inside the overlays' edges.
            let toe_edge = (z + toe - 0.075).abs() < 0.0018 && y > -0.058;
            if toe_edge {
                c *= 1.6;
            }
            if y < -0.062 {
                c = rgb(0x24201d);
            }
            if y > 0.075 {
                c = rgb(0x2b211a);
            }
            // Laces criss-crossing up the tongue, and eyelets either side.
            let along = if z < 0.0 {
                -z * 0.7 + (y + 0.05) * 0.6
            } else {
                y + 0.05
            };
            let lace_zone = x.abs() < 0.017 && y > -0.05 && z < 0.035;
            if lace_zone {
                let a = ((along + x.abs() * 0.9) * 75.0).fract();
                let b = ((along - x.abs() * 0.9) * 75.0).fract();
                if a < 0.22 || b < 0.22 {
                    c = rgb(0xa8875c);
                }
            }
            let eyelet = (x.abs() - 0.0195).abs() < 0.0025
                && !lace_zone
                && y > -0.045
                && z < 0.035
                && (along * 75.0).fract() < 0.3;
            if eyelet {
                c = rgb(0x8f8f8f);
            }
            c
        }
    };
    merged([
        Loft::new(foot_keys, 48, 0.01)
            .caps(0.014, 0.016)
            .mesh(paint),
        Loft::new(shaft_keys, 48, 0.01).caps(0.0, 0.01).mesh(paint),
    ])
}

/// Boot or shoe sole (foot frame): rubber outsole with a midsole band.
fn sole_mesh(b: &Build) -> Mesh {
    let runner = b.runner;
    let floor = -b.ankle;
    let (t, keys) = if runner {
        let t = 0.013;
        (
            t,
            vec![
                fwd(0.08, floor + t + 0.004, 0.038, t + 0.004, t + 0.004),
                fwd(0.06, floor + t + 0.003, 0.046, t + 0.003, t + 0.003),
                fwd(0.0, floor + t + 0.001, 0.045, t + 0.001, t + 0.001),
                fwd(-0.06, floor + t, 0.054, t, t),
                fwd(-0.12, floor + t, 0.056, t, t),
                fwd(-0.17, floor + t + 0.003, 0.05, t - 0.002, t - 0.002),
                fwd(-0.198, floor + t + 0.009, 0.034, t - 0.004, t - 0.004),
            ],
        )
    } else {
        let t = 0.0125;
        (
            t,
            vec![
                fwd(0.088, floor + t, 0.04, t, t),
                fwd(0.07, floor + t, 0.05, t, t),
                fwd(0.02, floor + t, 0.05, t, t),
                fwd(-0.03, floor + t, 0.047, t, t),
                fwd(-0.1, floor + t, 0.058, t, t),
                fwd(-0.16, floor + t + 0.001, 0.058, t, t),
                fwd(-0.205, floor + t + 0.005, 0.047, t - 0.001, t - 0.001),
                fwd(-0.222, floor + t + 0.011, 0.03, t - 0.003, t - 0.003),
            ],
        )
    };
    let keys: Vec<Key> = keys.into_iter().map(|k| k.pow(3.2)).collect();
    Loft::new(keys, 48, 0.01)
        .caps(0.008, 0.008)
        .mesh(move |v: &Paint| {
            let h = v.p.y - floor;
            if runner {
                if h < 0.004 {
                    rgb(0x202124)
                } else {
                    rgb(0xf0f0ee)
                }
            } else if h > t * 1.45 {
                rgb(0x4d443b)
            } else {
                // Lugs: darker grooves round the edge.
                let lug = ((v.p.z * 160.0).sin() > 0.6) as u8 as f32;
                rgb(0x2a2725) * (1.0 - 0.35 * lug * smooth(0.004, 0.0, h - 0.006))
            }
        })
}

/// Hiker's sleeve on the upper arm, or the runner's short sleeve and bare arm.
fn upper_arm_meshes(b: &Build) -> (Mesh, Option<Mesh>) {
    let fabric = |v: &Paint| {
        let k = if v.p.z.abs() < 0.0025 && v.p.x > 0.02 {
            0.78
        } else {
            1.0
        };
        Vec4::new(k, k, k, 1.0)
    };
    if b.runner {
        let sleeve = Loft::new(
            vec![
                down(-0.006, 0.047, 0.05, 0.05),
                down(-0.035, 0.054, 0.054, 0.054),
                down(-0.11, 0.051, 0.051, 0.051),
                down(-0.128, 0.05, 0.05, 0.05),
            ],
            48,
            0.012,
        )
        .caps(0.02, 0.006)
        .mesh(|v: &Paint| {
            let k = if v.p.y < -0.118 { 0.55 } else { 1.0 };
            Vec4::new(k, k, k, 1.0)
        });
        let skin = look(b).skin;
        let arm = Loft::new(
            vec![
                down(-0.04, 0.046, 0.044, 0.046),
                down(-0.1, 0.044, 0.046, 0.043),
                down(-0.18, 0.04, 0.041, 0.038),
                down(-0.255, 0.035, 0.035, 0.034),
                down(-0.29, 0.034, 0.035, 0.035),
            ],
            40,
            0.012,
        )
        .caps(0.02, 0.034)
        .mesh(move |_| skin);
        (sleeve, Some(arm))
    } else {
        let sleeve = Loft::new(
            vec![
                down(-0.004, 0.05, 0.054, 0.054),
                down(-0.04, 0.059, 0.058, 0.057),
                down(-0.15, 0.052, 0.05, 0.052),
                down(-0.25, 0.047, 0.045, 0.047),
                down(-0.29, 0.046, 0.044, 0.046),
            ],
            48,
            0.012,
        )
        .caps(0.022, 0.045)
        .grid()
        .deform(|p, _, _, u| {
            // Creases at the inside of the elbow.
            let elbow = smooth(-0.2, -0.27, p.y) * smooth(0.0, -0.04, p.z.min(0.0) + 0.0);
            let r = Vec3::new(p.x, 0.0, p.z).normalize_or(Vec3::X);
            p + r * (0.0025 * elbow * (p.y * 220.0 + (u * TAU).sin()).sin())
        })
        .mesh(fabric);
        (sleeve, None)
    }
}

fn forearm_meshes(b: &Build) -> (Mesh, Option<Mesh>) {
    if b.runner {
        let skin = look(b).skin;
        let arm = Loft::new(
            vec![
                down(0.0, 0.034, 0.035, 0.035),
                down(-0.05, 0.038, 0.036, 0.037),
                down(-0.12, 0.033, 0.03, 0.031),
                down(-0.2, 0.026, 0.023, 0.023),
                down(-0.25, 0.024, 0.019, 0.019),
            ],
            40,
            0.012,
        )
        .caps(0.034, 0.012)
        .mesh(move |_| skin);
        // A sports watch on the wrist.
        let watch = merged([
            Loft::new(
                vec![
                    down(-0.222, 0.027, 0.022, 0.022),
                    down(-0.242, 0.027, 0.022, 0.022),
                ],
                32,
                0.005,
            )
            .caps(0.003, 0.003)
            .mesh(|_| rgb(0x15171a)),
            placed(
                ellipsoid(
                    Vec3::ZERO,
                    Vec3::new(0.007, 0.017, 0.016),
                    24,
                    |v: &Paint| {
                        if v.p.x > 0.004 {
                            rgb(0x0a0c10)
                        } else {
                            rgb(0x2a2d30)
                        }
                    },
                ),
                Quat::IDENTITY,
                Vec3::new(0.024, -0.232, 0.0),
            ),
        ]);
        (arm, Some(watch))
    } else {
        let sleeve = Loft::new(
            vec![
                down(0.0, 0.046, 0.044, 0.046),
                down(-0.07, 0.047, 0.045, 0.046),
                down(-0.16, 0.042, 0.04, 0.041),
                down(-0.222, 0.039, 0.037, 0.038),
            ],
            48,
            0.012,
        )
        .caps(0.045, 0.01)
        .mesh(white);
        let cuff = Loft::new(
            vec![
                down(-0.208, 0.04, 0.038, 0.039),
                down(-0.235, 0.038, 0.036, 0.037),
                down(-0.258, 0.036, 0.034, 0.035),
            ],
            40,
            0.006,
        )
        .caps(0.004, 0.006)
        .mesh(|v: &Paint| {
            // Velcro tab on the outside.
            let tab = v.p.x > 0.02 && v.p.y > -0.245 && v.p.z.abs() < 0.014;
            if tab { rgb(0x2a2b2e) } else { rgb(0x1a1b1d) }
        });
        (sleeve, Some(cuff))
    }
}

/// Hiker's daypack with lid, front pocket and shoulder straps (chest frame).
fn pack_mesh() -> Mesh {
    // A rounded box: flat against the back, fuller lower down, piped seams round the back
    // panel and the front, side compression straps and a darker base.
    let body = Loft::new(
        vec![
            up(0.0, 0.19, 0.122, 0.068, 0.064).pow(4.5),
            up(0.04, 0.192, 0.14, 0.08, 0.077).pow(4.8),
            up(0.2, 0.196, 0.148, 0.086, 0.088).pow(4.8),
            up(0.34, 0.194, 0.14, 0.08, 0.08).pow(4.6),
            up(0.4, 0.192, 0.13, 0.072, 0.07).pow(4.2),
        ],
        80,
        0.012,
    )
    .caps(0.03, 0.025)
    .mesh(|v: &Paint| {
        let (x, y, z) = (v.p.x, v.p.y, v.p.z);
        let mut k = if y < 0.03 { 0.5 } else { 1.0 };
        // Piping where the side panels meet the front and the back panel.
        let corner = |cx: f32, cz: f32| Vec2::new((x.abs() - cx) / 0.02, (z - cz) / 0.02).length();
        if corner(0.13, 0.27).min(corner(0.13, 0.115)) < 0.45 {
            k *= 0.55;
        }
        // Side compression straps.
        if x.abs() > 0.12 && ((y - 0.15).abs() < 0.01 || (y - 0.29).abs() < 0.01) {
            k = 0.3;
        }
        Vec4::new(k, k, k, 1.0)
    });
    let lid = Loft::new(
        vec![
            up(0.375, 0.198, 0.13, 0.08, 0.086).pow(3.6),
            up(0.425, 0.202, 0.138, 0.086, 0.094).pow(3.6),
            up(0.452, 0.2, 0.128, 0.076, 0.086).pow(3.0),
        ],
        64,
        0.01,
    )
    .caps(0.006, 0.02)
    .mesh(|v: &Paint| {
        // A zip round the lid's front and its darker underside.
        let mut k = 0.62;
        if (v.p.y - 0.43).abs() < 0.0022 && v.p.z > 0.24 {
            k = 0.08;
        }
        if v.p.y < 0.385 {
            k *= 0.7;
        }
        Vec4::new(k, k, k, 1.0)
    });
    let pocket = Loft::new(
        vec![
            up(0.03, 0.27, 0.1, 0.018, 0.02).pow(5.0),
            up(0.12, 0.282, 0.112, 0.025, 0.028).pow(5.0),
            up(0.25, 0.278, 0.104, 0.02, 0.023).pow(5.0),
        ],
        56,
        0.01,
    )
    .caps(0.01, 0.01)
    .mesh(|v: &Paint| {
        // Its zip, and piping round the edge.
        let mut k = if (v.p.y - 0.24).abs() < 0.0025 {
            0.1
        } else {
            0.8
        };
        if v.p.x.abs() > 0.098 {
            k *= 0.6;
        }
        Vec4::new(k, k, k, 1.0)
    });
    // A buckled strap down the front of the lid onto the pocket.
    let lid_strap = Loft::new(
        vec![
            key(Vec3::new(0.0, 0.452, 0.297), 0.012, 0.0025),
            key(Vec3::new(0.0, 0.41, 0.301), 0.012, 0.0025),
            key(Vec3::new(0.0, 0.375, 0.289), 0.012, 0.0025),
            key(Vec3::new(0.0, 0.33, 0.279), 0.012, 0.0025),
            key(Vec3::new(0.0, 0.285, 0.283), 0.012, 0.0025),
            key(Vec3::new(0.0, 0.255, 0.303), 0.012, 0.0025),
        ],
        12,
        0.01,
    )
    .frames(Frames::Side(Vec3::X))
    .caps(0.002, 0.002)
    .mesh(|v: &Paint| {
        let buckle = (v.p.y - 0.36).abs() < 0.011;
        if buckle {
            Vec4::splat(0.1)
        } else {
            Vec4::splat(0.35)
        }
    });
    let mut parts = vec![body, lid, pocket, lid_strap];
    // Shoulder straps: from the top of the pack over the shoulder, down the chest and back
    // under the arm to the pack's lower corner.
    for s in [-1.0f32, 1.0] {
        let pts = [
            Vec3::new(0.075, 0.37, 0.115),
            Vec3::new(0.092, 0.442, 0.06),
            Vec3::new(0.1, 0.462, 0.0),
            Vec3::new(0.104, 0.43, -0.068),
            Vec3::new(0.106, 0.34, -0.118),
            Vec3::new(0.11, 0.23, -0.127),
            Vec3::new(0.135, 0.12, -0.108),
            Vec3::new(0.172, 0.06, -0.035),
            Vec3::new(0.17, 0.05, 0.06),
            Vec3::new(0.15, 0.05, 0.13),
        ];
        let n = pts.len();
        let keys: Vec<Key> = pts
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let t = i as f32 / (n - 1) as f32;
                // Padded and wide over the shoulder, a thin webbing strap lower down.
                let w = lerp(0.029, 0.013, smooth(0.45, 0.65, t));
                let h = lerp(0.0055, 0.0025, smooth(0.45, 0.65, t));
                key(Vec3::new(p.x * s, p.y, p.z), w, h).pow(3.5)
            })
            .collect();
        let strap = Loft::new(keys, 20, 0.01)
            .frames(Frames::Transport(Vec3::X))
            .caps(0.004, 0.004)
            .mesh(white);
        parts.push(strap);
    }
    // Sternum strap and its buckle.
    parts.push(
        Loft::new(
            vec![
                key(Vec3::new(-0.106, 0.3, -0.126), 0.008, 0.0025),
                key(Vec3::new(-0.05, 0.3, -0.122), 0.008, 0.0025),
                key(Vec3::new(0.0, 0.3, -0.124), 0.008, 0.0025),
                key(Vec3::new(0.05, 0.3, -0.122), 0.008, 0.0025),
                key(Vec3::new(0.106, 0.3, -0.126), 0.008, 0.0025),
            ],
            12,
            0.01,
        )
        .frames(Frames::Side(Vec3::Y))
        .caps(0.002, 0.002)
        .mesh(|_| Vec4::new(0.4, 0.4, 0.4, 1.0)),
    );
    parts.push(ellipsoid(
        Vec3::new(0.0, 0.3, -0.127),
        Vec3::new(0.017, 0.011, 0.0045),
        16,
        |_| Vec4::new(0.12, 0.12, 0.12, 1.0),
    ));
    merged(parts)
}

/// The jacket's hood, down: a soft, flattened fold lying from the back of the collar onto
/// the top of the pack, its face opening a rolled edge on top (chest frame).
fn hood_mesh() -> Mesh {
    // Round the back of the collar from the right side to the left, fattest at the back.
    let n = 11;
    let keys: Vec<Key> = (0..n)
        .map(|i| {
            let t = i as f32 / (n - 1) as f32;
            let a = lerp(0.45, PI - 0.45, t);
            let fat = (PI * t).sin().max(0.0).sqrt();
            let c = Vec3::new(0.09 * a.cos(), 0.5 + 0.006 * fat, 0.008 + 0.088 * a.sin());
            key(c, 0.016 + 0.008 * fat, 0.011 + 0.008 * fat)
        })
        .collect();
    Loft::new(keys, 40, 0.008)
        .frames(Frames::Side(Vec3::Y))
        .caps(0.012, 0.012)
        .grid()
        .deform(|p, c, _, _| {
            // Soft folds across the roll.
            let a = p.z.atan2(p.x);
            let out = (p - c).normalize_or(Vec3::Y);
            p + out * (0.0018 * (a * 14.0).sin() * smooth(0.0, 0.004, p.y - c.y + 0.004))
        })
        .mesh(|v: &Paint| {
            // The zip that closes the hood's collar pocket, along the bottom edge.
            let k = if (v.n.y + 0.55).abs() < 0.08 {
                0.45
            } else {
                1.0
            };
            Vec4::new(k, k, k, 1.0)
        })
}

fn mat(base: Vec4, roughness: f32) -> StandardMaterial {
    let mut m = StandardMaterial::color(base.x, base.y, base.z).with_roughness(roughness);
    m.receives_wetness = true;
    m
}

/// Every mesh of a person, with its bone and material. `garment` is the jacket or top colour
/// (sRGB hex).
pub fn parts(b: &Build, garment: u32, tex: &Textures) -> Vec<Part> {
    let who = if b.runner { "runner" } else { "hiker" };
    let lk = look(b);
    let skin = normal(
        mat(Vec4::ONE, 0.6).with_subsurface(0.55),
        tex.skin,
        0.02,
        0.35,
    );
    let shirt = rgb(garment);
    let mut parts = Parts::new(who);
    let bb = *b;
    let head_grid_b = bb;
    // The neck's base stays with the chest.
    parts.parent(CHEST, |p| smooth(-0.1, -0.17, p.y));
    parts.add(HEAD, "head", move || head_mesh(&head_grid_b), skin);
    let eye_mat = mat(Vec4::ONE, 0.08).with_subsurface(0.2);
    for (i, s) in SIDES.into_iter().enumerate() {
        let lk2 = lk;
        parts.add(
            HEAD,
            &format!("eye{i}"),
            move || placed(eye_mesh(&lk2), Quat::IDENTITY, eye_centre(&bb, s)),
            eye_mat,
        );
        parts.add(
            HEAD,
            &format!("lids{i}"),
            move || {
                // Lids tip with the eye's outward turn so the outer corner sits further back.
                let m = placed(
                    lids_mesh(&lk2),
                    Quat::from_rotation_y(-0.12),
                    eye_centre(&bb, 1.0),
                );
                if s < 0.0 { mirror_x(m) } else { m }
            },
            skin,
        );
        parts.add(
            HEAD,
            &format!("ear{i}"),
            move || {
                let m = ear_mesh(&lk2);
                if s < 0.0 { mirror_x(m) } else { m }
            },
            skin,
        );
        parts.add(
            HEAD,
            &format!("brow{i}"),
            move || {
                let m = brow_mesh(&head_grid(&bb), &lk2);
                if s < 0.0 { mirror_x(m) } else { m }
            },
            mat(Vec4::ONE, 0.7),
        );
    }
    let cap_mat = if b.runner {
        albedo(
            normal(mat(Vec4::ONE, 0.65), tex.hair, 0.03, 0.6),
            tex.hair_albedo,
        )
    } else {
        normal(mat(rgb(0x3d4a52), 0.95), tex.knit, 0.03, 1.0)
    };
    parts.add(HEAD, "cap", move || cap_mesh(&bb), cap_mat);

    let garment_mat = if b.runner {
        normal(mat(shirt, 0.65).with_subsurface(0.1), tex.sport, 0.012, 0.6)
    } else {
        normal(mat(shirt, 0.55), tex.ripstop, 0.03, 0.5)
    };
    // The hem turns with the hips.
    parts.parent(PELVIS, |p| smooth(0.05, -0.1, p.y));
    parts.add(CHEST, "torso", move || torso_mesh(&bb), garment_mat);
    let legs_mat = if b.runner {
        normal(mat(rgb(0x17181b), 0.55), tex.sport, 0.01, 0.25)
    } else {
        normal(mat(rgb(0x3a3d40), 0.85), tex.twill, 0.02, 0.6)
    };
    // The seat follows each thigh.
    for (i, s) in SIDES.into_iter().enumerate() {
        parts.child(THIGH[i], move |p| {
            0.6 * smooth(-0.02, -0.11, p.y) * smooth(0.0, 0.06, s * p.x)
        });
    }
    parts.add(PELVIS, "pelvis", move || pelvis_mesh(&bb), legs_mat);
    if !b.runner {
        let pack = normal(mat(rgb(0x3d4a35), 0.75), tex.cordura, 0.02, 0.8);
        parts.add(CHEST, "pack", pack_mesh, pack);
        parts.add(CHEST, "hood", hood_mesh, garment_mat);
    }
    let boot = if b.runner {
        normal(mat(Vec4::ONE, 0.6), tex.sport, 0.01, 0.6)
    } else {
        normal(mat(Vec4::ONE, 0.74), tex.leather, 0.03, 0.35)
    };
    let sole = mat(Vec4::ONE, 0.85);
    for (i, s) in SIDES.into_iter().enumerate() {
        let side = move |m: Mesh| if s < 0.0 { mirror_x(m) } else { m };
        let thigh = b.thigh;
        parts
            .parent(PELVIS, |p| smooth(-0.15, 0.04, p.y))
            .child(SHIN[i], move |p| smooth(-thigh + 0.05, -thigh - 0.05, p.y));
        parts.add(
            THIGH[i],
            &format!("thigh{i}"),
            move || side(thigh_mesh(&bb)),
            legs_mat,
        );
        parts.parent(THIGH[i], |p| smooth(-0.05, 0.05, p.y));
        parts.add(
            SHIN[i],
            &format!("shin{i}"),
            move || side(shin_mesh(&bb)),
            legs_mat,
        );
        parts.add(
            FOOT[i],
            &format!("upper{i}"),
            move || side(upper_mesh(&bb)),
            boot,
        );
        parts.add(
            FOOT[i],
            &format!("sole{i}"),
            move || side(sole_mesh(&bb)),
            sole,
        );
        // Shoulders stay with the chest; elbows bend smoothly.
        let ua = b.upper_arm;
        let upper = move |parts: &mut Parts| {
            parts
                .parent(CHEST, |p| smooth(-0.1, 0.02, p.y))
                .child(FOREARM[i], move |p| smooth(-ua + 0.04, -ua - 0.04, p.y));
        };
        let fore = move |parts: &mut Parts| {
            parts.parent(UPPER_ARM[i], |p| smooth(-0.04, 0.04, p.y));
        };
        upper(&mut parts);
        parts.add(
            UPPER_ARM[i],
            &format!("sleeve{i}"),
            move || side(upper_arm_meshes(&bb).0),
            garment_mat,
        );
        if b.runner {
            upper(&mut parts);
            parts.add(
                UPPER_ARM[i],
                &format!("arm{i}"),
                move || side(upper_arm_meshes(&bb).1.unwrap_or_default()),
                skin,
            );
            fore(&mut parts);
            parts.add(
                FOREARM[i],
                &format!("forearm{i}"),
                move || side(forearm_meshes(&bb).0),
                skin,
            );
            if i == 0 {
                parts.add(
                    FOREARM[i],
                    "watch",
                    move || side(forearm_meshes(&bb).1.unwrap_or_default()),
                    mat(Vec4::ONE, 0.3),
                );
            }
        } else {
            fore(&mut parts);
            parts.add(
                FOREARM[i],
                &format!("forearm{i}"),
                move || side(forearm_meshes(&bb).0),
                garment_mat,
            );
            parts.add(
                FOREARM[i],
                &format!("cuff{i}"),
                move || side(forearm_meshes(&bb).1.unwrap_or_default()),
                mat(Vec4::ONE, 0.8),
            );
        }
        let fist = b.runner;
        parts.add(
            HAND[i],
            &format!("hand{i}"),
            move || side(hand_mesh(&look(&bb), fist)),
            skin,
        );
    }
    parts.list
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_world::ContactKind;

    /// Move `w` forward the way `walk_trail` does.
    fn step(w: &mut TrailWalker, dx: f32) {
        w.along += dx as f64;
        w.to_next_step -= dx;
        while w.to_next_step <= 0.0 {
            w.to_next_step += w.step_len;
            w.next_foot = (w.next_foot + 1) % w.feet;
        }
    }

    #[test]
    fn planted_feet_stay_put_and_knees_bend_forward() {
        for (b, speed) in [(HIKER, 1.4), (RUNNER, 4.0)] {
            // Start before the far bend so the walk covers a straight and a curve.
            let origin = DVec3::new(3.0, 0.0, 1.0);
            let mut w = TrailWalker::new(origin, 33.0, ContactKind::Foot, 70.0, speed);
            let g = Gait::of(&w);
            let mut out = [Xf::IDENTITY; 15];
            let mut prev: [Option<(bool, DVec3)>; 2] = [None; 2];
            let (mut lo, mut hi, mut knee_max, mut slide) = (f32::MAX, f32::MIN, 0.0f32, 0.0f64);
            let mut under = (0.0f64, 0.0f32, "");
            let dx = 0.004;
            for _ in 0..(3.0 * gait::stride(&w) / dx) as usize {
                step(&mut w, dx);
                pose(&w, &b, &mut out);
                let (rp, rd) = gait::root(&w);
                let rot = yaw(rd);
                let world = |p: Vec3| rp + (rot * p).as_dvec3();
                lo = lo.min(out[PELVIS].t.y);
                hi = hi.max(out[PELVIS].t.y);
                for i in 0..2 {
                    let foot = out[FOOT[i]];
                    let heel = world(foot.point(Vec3::new(0.0, -b.ankle, b.heel - 0.02)));
                    let ball = world(foot.point(Vec3::new(0.0, -b.ankle, -(b.toe - 0.012))));
                    // The toe tip, sprung up off the ground the way the soles are.
                    let toe = world(foot.point(Vec3::new(0.0, -b.ankle + 0.012, -b.toe)));
                    let ground = rp.y;
                    let low = heel.y.min(ball.y).min(toe.y);
                    let (u, _) = gait::foot_cycle(&w, i as u8);
                    if ground - low > under.0 {
                        let which = if low == heel.y {
                            "heel"
                        } else if low == ball.y {
                            "ball"
                        } else {
                            "toe"
                        };
                        under = (ground - low, u, which);
                    }
                    if u < g.duty {
                        let on_heel = heel.y < ball.y - 1e-4;
                        let p = if on_heel { heel } else { ball };
                        assert!(
                            (p.y - ground).abs() < 0.003,
                            "planted foot {i} off the ground: {}",
                            p.y - ground
                        );
                        if let Some((was_heel, q)) = prev[i]
                            && was_heel == on_heel
                        {
                            slide = slide.max(DVec3::new(p.x - q.x, 0.0, p.z - q.z).length());
                        }
                        prev[i] = Some((on_heel, p));
                    } else {
                        prev[i] = None;
                    }
                    let hip = out[THIGH[i]].t;
                    let knee = out[SHIN[i]].t;
                    assert!((knee.distance(hip) - b.thigh).abs() < 1e-3);
                    assert!(
                        (foot.t.distance(knee) - b.shin).abs() < 1e-3,
                        "ankle out of reach"
                    );
                    // The knee is in front of the hip–ankle line.
                    let line = (foot.t - hip).normalize();
                    let off = (knee - hip) - line * (knee - hip).dot(line);
                    let fwd = out[PELVIS].r * Vec3::NEG_Z;
                    assert!(off.dot(fwd) > -1e-3, "knee {i} bends backward");
                    let flex = PI - (hip - knee).angle_between(foot.t - knee);
                    knee_max = knee_max.max(flex);
                }
            }
            eprintln!(
                "runner {}: pelvis {lo:.3}..{hi:.3}, knee flex up to {:.0} deg, slide {slide:.5}",
                b.runner,
                knee_max.to_degrees(),
            );
            eprintln!(
                "  deepest under the ground: {:.4} at u {:.3} ({})",
                under.0, under.1, under.2
            );
            assert!(slide < 0.0015, "a planted foot slid {slide} m");
            assert!(under.0 < 0.003, "a foot went under the ground");
        }
    }
}
