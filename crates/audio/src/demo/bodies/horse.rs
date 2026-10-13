//! The bay horse's body (its rig and walk are in [`quadruped`]).
//!
//! The trunk, neck, shoulders and thighs are one sculpted skin: a rib cage and belly with the
//! bones and muscles over them joined by smooth unions (see [`sdf`]), meshed in the rest pose
//! and cut into pieces along lines where their bones agree. Each side's front piece bends
//! with its shoulder and the neck, each rear piece with its thigh, so the shoulder blade
//! turns, the elbow and stifle travel and the flank stretches without a seam. The head is
//! sculpted the same way, finer. Below the elbows and stifles the legs are lofted sections
//! (forearm, knee, flat cannon, fetlock, pastern and hoof); mane, forelock and tail are
//! layered hair cards.

use std::sync::OnceLock;

use engine_core::math::noise::perlin3;
use engine_core::math::{PI, Quat, Vec2, Vec3, Vec4, hash01};
use engine_render::Mesh;

use super::quadruped::{
    self, EAR, FRONT, HEAD, HORSE, NECK, PASTERN, REAR, TAIL, along_y, along_z, foot_info, leg,
    mat, side_of,
};
use super::rig::Xf;
use super::sdf::{Nets, Sculpt, Shape, blob, blob_along, blob_turned, exit, limb};
use super::shape::{Grid, Paint, bump, ellipsoid, key, lerp, merged, placed, rgb, sheet, smooth};
use super::textures::{Textures, albedo, normal};
use super::{Part, Parts};

/// The rest pose every mesh is built in.
fn rest() -> [Xf; 32] {
    let mut out = [Xf::IDENTITY; 32];
    quadruped::rest(&HORSE, &mut out);
    out
}

// ---------------------------------------------------------------------------------------
// Coat
// ---------------------------------------------------------------------------------------

const BLACK: u32 = 0x0f0b09;
const HAIR: u32 = 0x120c0a;

/// A soft dapple pattern: about 1 in the heart of each dapple, 0 at its rim and a little below
/// in the darker net between them.
fn dapples(p: Vec3) -> f32 {
    let q = p / 0.075;
    let base = q.floor();
    let mut near = f32::MAX;
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let c = base + Vec3::new(dx as f32, dy as f32, dz as f32);
                let (i, j, k) = (c.x as i64, c.y as i64, c.z as i64);
                let jitter = Vec3::new(
                    hash01(i, j, k, 71),
                    hash01(i, j, k, 72),
                    hash01(i, j, k, 73),
                );
                near = near.min((c + jitter - q).length());
            }
        }
    }
    smooth(0.55, 0.15, near) - 0.5 * smooth(0.5, 0.8, near)
}

fn noise(p: Vec3, scale: f32, seed: u64) -> f32 {
    let q = p.as_dvec3() * scale as f64;
    perlin3(q.x, q.y, q.z, seed)
}

/// Bay coat at `p` (rest frame) facing `n`: red-brown, darker along the top line, lighter
/// under the belly and up the flank, faintly dappled over the barrel and quarters.
fn bay(p: Vec3, n: Vec3) -> Vec4 {
    let base = rgb(0x5f3420);
    let dark = rgb(0x3a2015);
    let light = rgb(0x85553a);
    let top = smooth(0.2, 0.85, n.y) * smooth(1.3, 1.55, p.y);
    let mut c = base.lerp(dark, 0.55 * top);
    let under = smooth(-0.15, -0.75, n.y);
    let flank = bump(
        Vec3::new(p.x.abs() - 0.27, p.y - 1.1, p.z - 0.18),
        Vec3::new(0.12, 0.16, 0.16),
    );
    c = c.lerp(light, (0.45 * under + 0.3 * flank).min(0.6));
    let body = smooth(0.25, 0.6, n.x.abs()) * smooth(-0.55, -0.3, p.z) * smooth(0.95, 1.1, p.y);
    let broad = noise(p, 3.0, 5) * 0.07 + noise(p, 9.0, 6) * 0.03;
    c * (1.0 + broad + 0.06 * dapples(p) * body)
}

/// The black points: `t` 0 is bay, 1 black.
fn points(c: Vec4, t: f32) -> Vec4 {
    c.lerp(rgb(BLACK), t.clamp(0.0, 1.0))
}

// ---------------------------------------------------------------------------------------
// The sculpted skin
// ---------------------------------------------------------------------------------------

/// Rib cage and belly, chest to buttocks: `[z, centre y, half width, below, above]`. Deepest
/// at the girth, widest over the ribs, the belly tucking up into the flank.
const BARREL: [[f32; 5]; 8] = [
    [-0.64, 1.19, 0.15, 0.25, 0.25],
    [-0.48, 1.18, 0.22, 0.31, 0.3],
    [-0.28, 1.165, 0.275, 0.305, 0.33],
    [-0.06, 1.17, 0.3, 0.29, 0.335],
    [0.14, 1.19, 0.295, 0.275, 0.325],
    [0.32, 1.23, 0.265, 0.245, 0.3],
    [0.5, 1.29, 0.215, 0.2, 0.26],
    [0.7, 1.32, 0.15, 0.15, 0.21],
];

/// The throatlatch, where the neck's skin meets the head's: a plane through this point
/// facing the head.
const THROAT: Vec3 = Vec3::new(0.0, 1.655, -1.15);
const THROAT_N: Vec3 = Vec3::new(0.0, -0.287, -0.958);

/// How far `p` (rest frame) is past the throatlatch, toward the head.
fn throat(p: Vec3) -> f32 {
    (p - THROAT).dot(THROAT_N)
}

/// The eye's centre in the head frame (right side).
const EYE: Vec3 = Vec3::new(0.083, -0.04, -0.15);
const EYE_R: Vec3 = Vec3::new(0.021, 0.021, 0.026);
/// The right nostril's opening (head frame).
const NOSTRIL: Vec3 = Vec3::new(0.043, -0.081, -0.579);
/// The middle of the mouth's line (head frame).
const MOUTH: Vec3 = Vec3::new(0.0, -0.176, -0.578);

/// Trunk, neck, shoulders, thighs and head in the rest pose (rest frame, meters).
fn sculpt() -> &'static Sculpt {
    static S: OnceLock<Sculpt> = OnceLock::new();
    S.get_or_init(build_sculpt)
}

fn build_sculpt() -> Sculpt {
    let rest = rest();
    let v = Vec3::new;
    let r = Vec2::new;
    let x = Vec3::X;
    let turn = Quat::from_rotation_x;
    let mut s = Sculpt::new();

    s.add(
        Shape::Tube {
            keys: &BARREL,
            caps: [0.14, 0.12],
        },
        0.0,
    );
    // The withers over the shoulder blades, the back muscles either side of the spine over
    // the loin.
    s.add(
        blob_along(v(0.0, 1.51, -0.66), v(0.0, 1.45, 0.04), 0.045, 0.1, x),
        0.08,
    );
    s.pair(
        limb(
            v(0.065, 1.44, -0.36),
            v(0.085, 1.48, 0.28),
            r(0.07, 0.075),
            r(0.085, 0.075),
            x,
        ),
        0.08,
    );

    // Hindquarters: the sacrum sloping down the middle to the tail head, the croup's
    // muscles rounding over either side from the points of hip back to the buttocks.
    s.add(
        limb(
            v(0.0, 1.51, 0.24),
            v(0.0, 1.45, 0.8),
            r(0.055, 0.085),
            r(0.045, 0.045),
            x,
        ),
        0.07,
    );
    s.pair(
        blob_turned(v(0.09, 1.435, 0.48), v(0.12, 0.12, 0.32), turn(0.32)),
        0.12,
    );
    // Each quarter: one big rounded mass from the croup down to the stifle and the gaskin.
    s.pair(
        blob_turned(v(0.115, 1.245, 0.6), v(0.15, 0.29, 0.27), turn(0.6)),
        0.12,
    );
    s.pair(blob(v(0.18, 1.465, 0.35), v(0.028, 0.03, 0.036)), 0.09);
    // On the femur: the outer thigh over the hip joint, the quadriceps in front down to the
    // stifle, and the stifle.
    let femur = rest[leg(1)];
    s.frame(femur);
    s.pair(blob(v(0.0, -0.12, 0.07), v(0.1, 0.22, 0.15)), 0.1);
    s.pair(blob(v(-0.005, -0.26, -0.065), v(0.075, 0.16, 0.075)), 0.08);
    s.pair(blob(v(0.02, -0.405, -0.035), v(0.045, 0.055, 0.05)), 0.08);
    s.frame(Xf::IDENTITY);
    // The hamstrings rounding from the point of buttock down the back of the thigh to the
    // gaskin; the tensor fasciae latae from the point of hip to the stifle; the fold of skin
    // from the stifle to the belly.
    s.pair(
        limb(
            v(0.1, 1.24, 0.84),
            femur.point(v(0.0, -0.37, 0.12)),
            r(0.07, 0.07),
            r(0.055, 0.06),
            x,
        ),
        0.12,
    );
    s.pair(
        limb(
            v(0.22, 1.45, 0.37),
            femur.point(v(0.03, -0.4, -0.06)),
            r(0.035, 0.045),
            r(0.03, 0.035),
            x,
        ),
        0.08,
    );
    s.pair(
        limb(
            femur.point(v(-0.03, -0.39, -0.05)),
            v(0.11, 1.0, 0.08),
            r(0.04, 0.035),
            r(0.06, 0.04),
            x,
        ),
        0.09,
    );

    // Shoulder blade, flat and sloping from the withers down to the point of shoulder; the
    // triceps filling the angle behind it down to the elbow; the arm from the point of
    // shoulder back to the elbow; the pectorals either side of the breastbone and down
    // between the legs.
    let arm = rest[leg(2)];
    let elbow = rest[leg(2) + 1].t;
    s.pair(
        blob_along(v(0.085, 1.55, -0.43), v(0.17, 1.14, -0.7), 0.045, 0.11, x),
        0.08,
    );
    s.pair(blob(v(0.165, 1.12, -0.7), v(0.045, 0.05, 0.045)), 0.08);
    s.pair(
        blob_along(
            v(0.15, 1.33, -0.47),
            elbow + v(0.0, 0.09, 0.06),
            0.075,
            0.12,
            x,
        ),
        0.09,
    );
    s.pair(
        limb(
            v(0.165, 1.1, -0.69),
            arm.point(v(0.0, -0.46, -0.05)),
            r(0.055, 0.06),
            r(0.055, 0.06),
            x,
        ),
        0.07,
    );
    s.pair(blob(v(0.07, 1.05, -0.68), v(0.075, 0.13, 0.08)), 0.07);
    s.pair(
        limb(
            v(0.06, 0.98, -0.62),
            elbow + v(-0.04, 0.02, -0.03),
            r(0.05, 0.05),
            r(0.05, 0.045),
            x,
        ),
        0.07,
    );

    // Neck: deep where it springs from the shoulders, the crest arching along the top, the
    // windpipe and sternocephalicus underneath, the brachiocephalicus up each side from the
    // arm to behind the jaw.
    s.add(
        limb(
            v(0.0, 1.33, -0.6),
            v(0.0, 1.67, -1.18),
            r(0.15, 0.21),
            r(0.075, 0.115),
            x,
        ),
        0.12,
    );
    s.add(
        blob_along(v(0.0, 1.6, -0.58), v(0.0, 1.77, -1.14), 0.05, 0.075, x),
        0.08,
    );
    s.add(
        limb(
            v(0.0, 1.11, -0.74),
            v(0.0, 1.47, -1.12),
            r(0.06, 0.055),
            r(0.045, 0.04),
            x,
        ),
        0.11,
    );
    s.pair(
        limb(
            v(0.14, 1.13, -0.7),
            v(0.06, 1.52, -1.13),
            r(0.055, 0.06),
            r(0.045, 0.045),
            x,
        ),
        0.1,
    );

    // Grooves: the jugular groove up the lower neck, the parting between the pectorals and
    // the line between the outer thigh and the hamstrings.
    s.press_pair(v(0.097, 1.33, -0.834), v(0.066, 1.55, -1.13), 0.025, 0.005);
    s.press(v(0.0, 1.2, -0.77), v(0.0, 0.95, -0.67), 0.02, 0.006);
    s.press_pair(v(0.15, 1.42, 0.78), v(0.21, 1.02, 0.66), 0.035, 0.006);

    // Head (its own frame: −Z down the face, +Y off the forehead).
    s.frame(rest[HEAD]);
    // The poll and braincase, with the mounds the ears stand on.
    s.add(blob(v(0.0, -0.065, -0.06), v(0.075, 0.075, 0.11)), 0.05);
    s.pair(blob(v(0.05, 0.0, 0.005), v(0.028, 0.028, 0.028)), 0.05);
    // The forehead, broad and flat between the eyes, narrowing down the nasal bone.
    s.add(
        limb(
            v(0.0, -0.068, -0.11),
            v(0.0, -0.085, -0.48),
            r(0.1, 0.062),
            r(0.05, 0.048),
            x,
        ),
        0.05,
    );
    // The flat round cheeks over the back of the jaw (their lower edge the jowl), the jaw's
    // lower edges converging on the chin, the channel between them and the side of the face
    // under the facial crest.
    s.pair(blob(v(0.06, -0.205, -0.1), v(0.04, 0.11, 0.105)), 0.03);
    s.pair(
        limb(
            v(0.045, -0.25, -0.2),
            v(0.022, -0.195, -0.49),
            r(0.017, 0.019),
            r(0.014, 0.016),
            x,
        ),
        0.035,
    );
    s.add(blob(v(0.0, -0.2, -0.2), v(0.035, 0.06, 0.13)), 0.05);
    s.add(
        limb(
            v(0.0, -0.12, -0.24),
            v(0.0, -0.125, -0.5),
            r(0.072, 0.07),
            r(0.05, 0.055),
            x,
        ),
        0.05,
    );
    // Muzzle, the nostrils' wings, the lips and the chin; the throat behind the jaw.
    s.add(blob(v(0.0, -0.115, -0.548), v(0.048, 0.062, 0.065)), 0.05);
    s.pair(blob(v(0.037, -0.08, -0.565), v(0.025, 0.038, 0.036)), 0.03);
    s.add(blob(v(0.0, -0.158, -0.585), v(0.048, 0.028, 0.045)), 0.025);
    s.add(blob(v(0.0, -0.19, -0.565), v(0.038, 0.02, 0.04)), 0.02);
    s.add(blob(v(0.0, -0.207, -0.52), v(0.03, 0.028, 0.04)), 0.03);
    s.add(blob(v(0.0, -0.15, 0.04), v(0.05, 0.07, 0.07)), 0.06);
    // The facial crest, a low ridge forward from under the eye; the brow over the eye.
    s.pair(
        limb(
            v(0.08, -0.08, -0.17),
            v(0.068, -0.105, -0.29),
            r(0.009, 0.009),
            r(0.006, 0.006),
            x,
        ),
        0.04,
    );
    s.pair(
        blob_along(
            v(0.07, -0.014, -0.12),
            v(0.086, -0.024, -0.18),
            0.014,
            0.011,
            x,
        ),
        0.03,
    );
    // The forehead and nasal bone planed flat on top.
    let up = Vec3::new(0.0, 1.0, -0.084).normalize() * 0.3;
    s.carve(
        limb(
            v(0.0, -0.012, -0.11) + up,
            v(0.0, -0.043, -0.48) + up,
            r(0.5, 0.3),
            r(0.5, 0.3),
            x,
        ),
        0.015,
    );
    // Nostrils, the eye's lids and the line of the mouth.
    s.carve_pair(
        blob_along(
            NOSTRIL + v(-0.016, -0.024, -0.024),
            NOSTRIL + v(0.016, 0.024, 0.01),
            0.013,
            0.011,
            x,
        ),
        0.006,
    );
    // The lids round the eye, the opening between them an almond slanting up behind.
    s.pair(
        blob(EYE + v(0.004, 0.0, 0.0), v(0.022, 0.023, 0.029)),
        0.012,
    );
    s.carve_pair(
        blob_turned(
            EYE + v(0.026, 0.0, 0.0),
            v(0.02, 0.012, 0.022),
            Quat::from_rotation_x(0.25),
        ),
        0.006,
    );
    s.carve(blob(MOUTH, v(0.048, 0.004, 0.06)), 0.006);
    s.frame(Xf::IDENTITY);
    s
}

/// The skin's pieces: front right, front left, rear right, rear left (all in the body's
/// frame), and the head (head frame).
fn skin() -> &'static [Mesh; 5] {
    static S: OnceLock<[Mesh; 5]> = OnceLock::new();
    S.get_or_init(|| {
        let rest = rest();
        let s = sculpt();
        let hinge = rest[FRONT].t;
        // The body stops a little past the throatlatch and the head a little before it, so
        // they overlap where both follow the neck.
        let body = |p: Vec3| s.d(p).max(throat(p) - 0.02);
        let nets = Nets::new(
            &body,
            Vec3::new(-0.42, 0.7, -1.42),
            Vec3::new(0.42, 1.95, 1.06),
            0.015,
        );
        let body_uv = |p: Vec3, _: Vec3| Vec2::new(p.y + p.x.abs(), p.z - 0.45 * p.y);
        let paint = |v: &Paint| bay(v.p, v.n);
        let mut pieces = nets.split(
            4,
            |c| Some(if c.z < -0.08 { 0 } else { 2 } + usize::from(c.x < 0.0)),
            body_uv,
            paint,
        );
        let head = |p: Vec3| s.d(p).max(-throat(p) - 0.02);
        let to_head = rest[HEAD].inverse();
        let nets = Nets::new(
            &head,
            Vec3::new(-0.15, 1.12, -1.8),
            Vec3::new(0.15, 1.9, -1.02),
            0.0065,
        );
        let head_uv = move |p: Vec3, _: Vec3| {
            let h = to_head.point(p);
            Vec2::new(h.y + h.x.abs(), -h.z - 0.3 * h.y)
        };
        let head = nets
            .split(1, |_| Some(0), head_uv, |v| head_paint(to_head, v.p, v.n))
            .remove(0);
        let mut pieces = pieces.drain(..).map(|m| placed(m, Quat::IDENTITY, -hinge));
        let mut next = || pieces.next().expect("four pieces");
        [next(), next(), next(), next(), into(head, to_head)]
    })
}

/// The head's coat (rest frame): bay, darker down the face, a dark soft muzzle with black
/// nostrils and lips, bare dark skin round the eyes.
fn head_paint(to_head: Xf, p: Vec3, n: Vec3) -> Vec4 {
    let q = to_head.point(p);
    let mut c = bay(p, n);
    c = c.lerp(c * 0.82, smooth(-0.2, -0.45, q.z));
    let ax = q.x.abs();
    let muzzle = smooth(-0.47, -0.56, q.z);
    c = c.lerp(rgb(0x2a1b15), muzzle * 0.9);
    let nostril = bump(
        Vec3::new(ax - NOSTRIL.x, q.y - NOSTRIL.y, q.z - NOSTRIL.z),
        Vec3::new(0.018, 0.026, 0.02),
    );
    let mouth = smooth(0.012, 0.004, (q.y - MOUTH.y).abs()) * smooth(-0.5, -0.53, q.z);
    let eye = bump(
        Vec3::new(ax - EYE.x - 0.015, q.y - EYE.y, q.z - EYE.z),
        Vec3::new(0.02, 0.026, 0.034),
    );
    c = c.lerp(rgb(0x2c1a12), eye * 0.6);
    c.lerp(rgb(0x070505), (nostril * 1.4 + mouth).min(1.0))
}

/// Where `p` (rest frame) is on the right leg whose first bone is `bone`: how far down
/// that bone from its top joint and how far behind it.
fn on_bone(bone: usize, p: Vec3) -> (f32, f32) {
    let q = rest()[bone].inverse().point(p);
    (-q.y, q.z)
}

/// How much a point of the right side (rest frame) follows the shoulder: the arm and elbow
/// fully, the blade less toward the withers (it turns about its upper part), the chest in
/// front of the arm half (it stretches), nothing behind the girth, in the neck or on the
/// midline.
fn shoulder_weight(p: Vec3) -> f32 {
    let (down, behind) = on_bone(leg(2), p);
    let side = smooth(0.02, 0.1, p.x);
    let back = smooth(-0.14, -0.36, p.z);
    let chest = 1.0 - 0.5 * smooth(-0.1, -0.2, behind);
    // Ahead of the line from the withers to the point of shoulder is neck.
    let ahead = (p - Vec3::new(0.0, 1.55, -0.45)).dot(Vec3::new(0.0, 0.57, -0.82));
    side * back * smooth(-0.15, 0.25, down) * chest * smooth(0.08, -0.02, ahead)
}

/// How much a point of the right side follows the thigh: everything down the femur from a
/// little below the hip joint, nothing over the croup or round the point of buttock (they
/// are on the pelvis), fading forward into the flank; less low down at the back, which
/// follows the gaskin.
fn thigh_weight(p: Vec3) -> f32 {
    let (down, behind) = on_bone(leg(1), p);
    let w = smooth(0.02, 0.08, p.x) * smooth(-0.1, 0.2, down) * smooth(-0.26, -0.1, behind);
    w - gaskin_weight(p)
}

/// How much the back of the thigh low down follows the gaskin, as the hamstrings running
/// down to it do.
fn gaskin_weight(p: Vec3) -> f32 {
    let (down, behind) = on_bone(leg(1), p);
    0.55 * smooth(0.02, 0.08, p.x) * smooth(0.18, 0.42, down) * smooth(0.02, 0.1, behind)
}

/// How much a point follows the neck (whose rest placement is `neck`): from nothing at its
/// root to all of it 0.3 m up.
fn neck_weight(neck: Xf, p: Vec3) -> f32 {
    let along = (p - neck.t).dot(neck.r * Vec3::NEG_Z);
    smooth(0.0, 0.3, along)
}

/// A mesh built in the rest frame, moved into a bone's frame (`xf` the inverse of the bone's
/// rest placement).
fn into(m: Mesh, xf: Xf) -> Mesh {
    placed(m, xf.r, xf.t)
}

fn mirror(p: Vec3) -> Vec3 {
    Vec3::new(-p.x, p.y, p.z)
}

/// The eyes: dark brown globes, the pupil a dark horizontal bar, with a glossy wet surface.
fn eyes() -> Mesh {
    merged([-1.0f32, 1.0].map(|s| {
        ellipsoid(EYE * Vec3::new(s, 1.0, 1.0), EYE_R, 32, move |v: &Paint| {
            // Looking out to the side and a little forward.
            let d = v.n - Vec3::new(s, 0.0, -0.3).normalize();
            let pupil = smooth(0.32, 0.22, Vec2::new(d.z * 0.7, d.y * 1.6).length());
            rgb(0x2a170c)
                .lerp(rgb(0x140b07), smooth(0.2, 0.6, d.length()))
                .lerp(rgb(0x040303), pupil)
        })
    }))
}

// ---------------------------------------------------------------------------------------
// Ears
// ---------------------------------------------------------------------------------------

/// The right ear (ear frame): a pointed leaf cupped forward, dark inside and at the rim.
fn ear() -> Mesh {
    let to_rest = |p: Vec3| p + Vec3::new(0.05, 1.8, -1.3);
    let m = super::shape::Loft::new(
        vec![
            key(Vec3::new(0.0, -0.02, 0.0), 0.03, 0.032).q2(0.032, 0.024),
            key(Vec3::new(0.0, 0.04, 0.002), 0.036, 0.028).q2(0.024, 0.028),
            key(Vec3::new(0.0, 0.1, 0.01), 0.025, 0.018),
            key(Vec3::new(0.0, 0.14, 0.018), 0.009, 0.008),
        ],
        32,
        0.012,
    )
    .caps(0.01, 0.012)
    .grid()
    .deform(|p, c, _, _| {
        let front = smooth(0.0, -0.012, p.z - c.z);
        let mid = (1.0 - (p.x / 0.032).powi(2)).max(0.0);
        let along = smooth(0.0, 0.03, p.y) * smooth(0.14, 0.09, p.y);
        p + Vec3::new(0.0, 0.0, 0.034 * front * mid * along)
    })
    .mesh(move |v: &Paint| {
        let rim = smooth(0.09, 0.14, v.p.y);
        let edge = smooth(0.6, 0.95, v.n.x.abs()) * smooth(0.03, 0.08, v.p.y);
        let inside = smooth(-0.1, -0.6, v.n.z) * smooth(0.022, 0.0, v.p.x.abs() - 0.01);
        bay(to_rest(v.p), v.n)
            .lerp(rgb(0x2a1a12), inside * 0.85)
            .lerp(rgb(BLACK), (rim + 0.5 * edge).min(1.0))
    });
    placed(
        m,
        Quat::from_rotation_x(0.45) * Quat::from_rotation_z(-0.2),
        Vec3::ZERO,
    )
}

// ---------------------------------------------------------------------------------------
// Mane, forelock and tail
// ---------------------------------------------------------------------------------------

fn hair_color(s: f32, t: f32) -> Vec4 {
    let n = (s * 41.0).sin() * (t * 7.0 + s * 13.0).sin();
    // A little browner where the sun has bleached the ends.
    rgb(HAIR).lerp(rgb(0x2a1a12), 0.35 * smooth(0.5, 1.0, t)) * (1.0 + 0.3 * n)
}

/// The crest: the neck's axis from the withers to the poll and the skin round it, as rings
/// of surface points from over the top (`θ` 0) round the right side (`θ` π/2) and below.
struct Crest {
    stations: Vec<(Vec3, Vec<Vec3>)>,
}

const CREST_A: Vec3 = Vec3::new(0.0, 1.36, -0.56);
const CREST_B: Vec3 = Vec3::new(0.0, 1.64, -1.14);
/// Ring angles from just left of the top round the right side, radians.
const RING: [f32; 2] = [-0.25, 2.2];

impl Crest {
    fn new() -> Self {
        let s = sculpt();
        let axis = (CREST_B - CREST_A).normalize();
        let up = (Vec3::Y - axis * axis.y).normalize();
        let stations = (0..=40)
            .map(|i| {
                let a = CREST_A.lerp(CREST_B, i as f32 / 40.0);
                let ring = (0..=24)
                    .map(|j| {
                        let t = lerp(RING[0], RING[1], j as f32 / 24.0);
                        let dir = up * t.cos() + Vec3::X * t.sin();
                        // Only the neck's skin counts, not the head past the throatlatch.
                        exit(&|p| s.d(p).max(throat(p)), a, dir)
                    })
                    .collect();
                (a, ring)
            })
            .collect();
        Self { stations }
    }

    /// The skin at `s` along the crest (0 withers, 1 poll) and angle fraction `u` round it.
    fn at(&self, s: f32, u: f32) -> Vec3 {
        let n = self.stations.len() - 1;
        let x = s.clamp(0.0, 1.0) * n as f32;
        let i = (x as usize).min(n - 1);
        let f = x - i as f32;
        let ring = |k: usize| {
            let r = &self.stations[k].1;
            let y = u.clamp(0.0, 1.0) * (r.len() - 1) as f32;
            let j = (y as usize).min(r.len() - 2);
            r[j].lerp(r[j + 1], y - j as f32)
        };
        ring(i).lerp(ring(i + 1), f)
    }

    fn centre(&self, s: f32) -> Vec3 {
        CREST_A.lerp(CREST_B, s.clamp(0.0, 1.0))
    }
}

/// The mane (rest frame): locks of hair from the crest, lying over to the right and falling
/// down the neck in layers (short ones close to the skin, longer ones over them), each lock
/// gathering toward its tip so the skin shows between them low down.
fn mane() -> Mesh {
    let crest = Crest::new();
    let crest_len = CREST_A.distance(CREST_B);
    let along = (CREST_B - CREST_A).normalize();
    let mut locks = Vec::new();
    for layer in 0..3 {
        let count = 30;
        for i in 0..count {
            let r = |k: i64| hash01(i, layer, k, 91);
            let mid = (i as f32 + 0.5 + (r(1) - 0.5) * 0.7) / count as f32;
            let half = (0.016 + 0.014 * r(2)) / crest_len;
            // Shorter at the withers and behind the ears.
            let ends = 0.55 + 0.45 * smooth(0.0, 0.2, mid) * smooth(1.0, 0.85, mid);
            let len = (0.09 + 0.05 * layer as f32 + 0.11 * r(3)) * ends;
            let lift = 0.004 + 0.011 * layer as f32 + 0.008 * r(4);
            let start = 0.07 + 0.04 * r(5) + 0.025 * layer as f32;
            let sweep = (r(6) - 0.5) * 0.1;
            let phase = r(7);
            let crest = &crest;
            locks.push(sheet(
                8,
                9,
                move |s, t| {
                    let at = mid + (s - 0.5) * 2.0 * half * (1.0 - 0.5 * t * t);
                    hair_path(crest, at, start, len, lift, t) + along * sweep * t * t
                },
                move |s, t| Vec2::new(s * 0.4 + phase, t * 0.97),
                hair_color,
            ));
        }
    }
    // Shade the mane as a mass lying over the neck.
    combed(
        merged(locks),
        move |p| {
            let q = p - CREST_A;
            q - along * q.dot(along)
        },
        0.5,
    )
}

/// A hair rooted on the crest at `s`, `start` round the ring from the left of the top: it
/// rises `lift` off the skin, follows the neck round to the right and then hangs straight
/// down, `len` long; `t` 0..1 along it.
fn hair_path(crest: &Crest, s: f32, start: f32, len: f32, lift: f32, t: f32) -> Vec3 {
    let along = t * len;
    // Walk round the ring.
    let steps = 24;
    let mut u = start;
    let mut p = crest.at(s, u);
    let mut walked = 0.0;
    let centre = crest.centre(s);
    while walked < along && u < 0.9 {
        let next_u = u + 1.0 / steps as f32 * 0.5;
        let q = crest.at(s, next_u);
        let d = p.distance(q);
        if walked + d >= along {
            p = p.lerp(q, (along - walked) / d.max(1e-6));
            walked = along;
            break;
        }
        walked += d;
        p = q;
        u = next_u;
    }
    let out = (p - centre).normalize_or(Vec3::Y);
    let off = lift * (0.4 + 0.6 * smooth(0.0, 0.3, t)) + 0.012 * t;
    // Past the side of the neck the hair hangs.
    p + out * off + Vec3::NEG_Y * (along - walked)
}

/// The forelock (head frame): locks from between the ears falling over the forehead.
fn forelock() -> Mesh {
    let s = sculpt();
    let head = rest()[HEAD];
    let to_head = head.inverse();
    // The skin down the middle of the forehead, from behind the poll.
    let skin = move |z: f32, x: f32| {
        let from = head.point(Vec3::new(x, -0.07, z));
        to_head.point(s.exit(from, head.r * Vec3::Y))
    };
    merged((0..5).map(|i| {
        let r = |k: i64| hash01(i, k, 0, 93);
        let x0 = (i as f32 - 2.0) * 0.013 + (r(1) - 0.5) * 0.008;
        let len = 0.16 + 0.06 * r(2);
        let lift = 0.006 + 0.004 * (i % 2) as f32;
        let phase = r(3);
        sheet(
            10,
            6,
            move |t, u| {
                let z = lerp(0.03, 0.03 - len, t);
                let x = (x0 + (u - 0.5) * 0.04 * (1.0 - 0.4 * t)) * (1.0 - 0.3 * t);
                let p = skin(z, x);
                p + Vec3::new(0.0, lift + 0.01 * t, 0.0)
            },
            move |t, u| Vec2::new(u * 0.5 + phase, t * 0.97),
            hair_color,
        )
    }))
}

/// Tail segment `k` (its bone's frame): the dock's skin and the hair that grows from it
/// (`k` 0), or the fall below (1 and 2): ribbons of hair rooted along the top and sides of
/// the dock in layers, lying over it, gathering just below it and then hanging as a full
/// fall that wraps round under it, wider side to side than front to back.
fn tail(k: usize) -> Mesh {
    let segs = HORSE.tail_seg;
    let len = segs[k];
    let before: f32 = segs[..k].iter().sum();
    let total: f32 = segs.iter().sum();
    let mut parts = Vec::new();
    if k == 0 {
        let to_rest = rest()[TAIL[0]];
        parts.push(
            super::shape::Loft::new(
                vec![
                    key(Vec3::new(0.0, 0.0, -0.02), 0.04, 0.044),
                    key(Vec3::new(0.0, 0.0, 0.1), 0.042, 0.046),
                    key(Vec3::new(0.0, 0.0, len + 0.03), 0.034, 0.036),
                ],
                28,
                0.025,
            )
            .caps(0.03, 0.03)
            .mesh(move |v: &Paint| {
                points(
                    bay(to_rest.point(v.p), to_rest.r * v.n),
                    smooth(-0.04, 0.05, v.p.z),
                )
            }),
        );
    }
    let ribbons = 48;
    for i in 0..ribbons {
        let r = |q: i64| hash01(i, q, 0, 97);
        // Four layers, the inner ones shortest and closest to the dock.
        let layer = (i % 4) as f32;
        // Round the top and sides of the dock (0 out to the right, π/2 over the top).
        let a0 = lerp(-0.5, PI + 0.5, (i as f32 + r(1)) / ribbons as f32);
        let root = 0.042 + 0.008 * layer;
        let fall = 0.025 + 0.028 * layer + 0.02 * r(2);
        let start = r(3) * 0.06 + 0.02 * layer;
        let width = 0.04 + 0.025 * r(4);
        let end = total * (0.78 + 0.07 * layer + 0.15 * r(5)).min(1.0);
        let twist = (r(7) - 0.5) * 1.2;
        let phase = r(6);
        let (z0, z1) = ((start - before).max(-0.04), (len + 0.04).min(end - before));
        if z1 <= z0 {
            continue;
        }
        let card = sheet(
            11,
            4,
            move |s, t| {
                let z = lerp(z0, z1, s);
                let along = (before + z) / total;
                let open = smooth(0.12, 0.6, along);
                // Hair from the sides wraps round under the dock as it falls.
                let a = PI * 0.5 + (a0 - PI * 0.5) * (1.0 + 0.45 * open);
                // Off the dock the hair gathers round it before it spreads again.
                let pinch = 1.0 - 0.25 * bump(Vec3::new(along - 0.32, 0.0, 0.0), Vec3::splat(0.12));
                let out = Vec3::new(a.cos(), a.sin(), 0.0);
                let around = Vec3::new(out.x * 1.15, out.y * 0.85, 0.0) * lerp(root, fall, open);
                let across = Vec3::new(-a.sin(), a.cos(), 0.0) * twist.cos() + out * twist.sin();
                let w = width * (1.0 - 0.45 * smooth(0.6, 1.0, along));
                around * pinch + across * (0.5 - t) * w + Vec3::Z * z
            },
            move |s, t| {
                let along = (before + lerp(z0, z1, s) - start) / (end - start);
                Vec2::new(t * 0.6 + phase, along * 0.97)
            },
            |s, t| hair_color(t, s),
        );
        // Shade the fall as a bundle: normals lean out from its middle.
        parts.push(combed(card, |p| Vec3::new(p.x, p.y, 0.0), 0.65));
    }
    merged(parts)
}

/// Bend a hair mesh's normals by `k` toward `out(p)` (away from the middle of the lock or
/// bundle it belongs to), so a mass of cards shades round rather than flat.
fn combed(mut m: Mesh, out: impl Fn(Vec3) -> Vec3, k: f32) -> Mesh {
    for v in &mut m.vertices {
        let p = Vec3::from(v.position);
        let n = Vec3::from(v.normal);
        let o = out(p).normalize_or(n);
        // Keep the side the card faces.
        let o = if o.dot(n) < 0.0 { -o } else { o };
        v.normal = n.lerp(o, k).normalize_or(n).to_array();
    }
    m
}

// ---------------------------------------------------------------------------------------
// Legs
// ---------------------------------------------------------------------------------------

/// A leg segment hanging from its joint: `[y, half width, front, back]` sections with the
/// back half squarer (tendons), and the knobs of its joints raised by `knobs`.
fn leg_loft(keys: &[[f32; 4]], knobs: impl Fn(Vec3) -> Vec3) -> Grid {
    quadruped::limb(keys, 32, 0.016)
        .grid()
        .deform(move |p, _, _, _| p + knobs(p))
}

/// The forearm (elbow to knee): muscular under the elbow with the point of the elbow behind,
/// slim above the knee; the knee flat in front, the accessory bone behind.
fn forearm() -> Grid {
    let l = HORSE.fore[1];
    leg_loft(
        &[
            [0.11, 0.055, 0.06, 0.05],
            [0.05, 0.068, 0.075, 0.072],
            [-0.03, 0.07, 0.078, 0.066],
            [-0.14, 0.062, 0.068, 0.056],
            [-0.26, 0.05, 0.052, 0.044],
            [-l + 0.1, 0.041, 0.04, 0.036],
            [-l + 0.04, 0.047, 0.04, 0.044],
            [-l - 0.01, 0.05, 0.042, 0.05],
            [-l - 0.05, 0.04, 0.034, 0.04],
        ],
        move |p| {
            let olecranon = bump(
                Vec3::new(p.x, p.y - 0.06, p.z - 0.07),
                Vec3::new(0.03, 0.04, 0.02),
            );
            let accessory = bump(
                Vec3::new(p.x - 0.01, p.y + l - 0.01, p.z - 0.045),
                Vec3::new(0.025, 0.022, 0.012),
            );
            let flat = smooth(-l + 0.08, -l + 0.03, p.y) * smooth(-0.01, -0.03, p.z);
            Vec3::new(
                0.0,
                0.0,
                0.02 * olecranon + 0.014 * accessory + 0.006 * flat,
            )
        },
    )
}

/// The gaskin (stifle to hock): the gaskin muscle bulging behind the shin, the hamstring
/// tendon running down behind it to the point of the hock, and the hock.
fn gaskin() -> Grid {
    let l = HORSE.hind[1];
    leg_loft(
        &[
            [0.1, 0.06, 0.05, 0.11],
            [0.02, 0.072, 0.056, 0.14],
            [-0.08, 0.066, 0.05, 0.125],
            [-0.18, 0.054, 0.044, 0.085],
            [-l + 0.09, 0.044, 0.04, 0.06],
            [-l + 0.03, 0.05, 0.044, 0.066],
            [-l - 0.02, 0.054, 0.05, 0.06],
            [-l - 0.06, 0.044, 0.04, 0.042],
        ],
        move |p| {
            // The point of the hock stands up behind the joint.
            let point = bump(
                Vec3::new(p.x, p.y + l - 0.05, p.z - 0.06),
                Vec3::new(0.025, 0.035, 0.018),
            );
            let inner = bump(
                Vec3::new(p.x + 0.04, p.y + l + 0.01, p.z),
                Vec3::new(0.012, 0.03, 0.03),
            );
            Vec3::new(-0.006 * inner, 0.0, 0.02 * point)
        },
    )
}

/// A cannon (knee or hock to fetlock): flat and slim, the tendons standing out behind the
/// bone, then the rounded fetlock with the ergot behind it.
fn cannon(fore: bool) -> Grid {
    let (l, top) = if fore {
        (HORSE.fore[2], 0.045)
    } else {
        (HORSE.hind[2], 0.05)
    };
    leg_loft(
        &[
            [0.05, top, top - 0.005, top],
            [-0.02, top - 0.006, top - 0.012, top - 0.004],
            [-0.07, 0.034, 0.027, 0.042],
            [-l * 0.55, 0.032, 0.025, 0.044],
            [-l + 0.06, 0.034, 0.027, 0.047],
            [-l + 0.02, 0.042, 0.035, 0.056],
            [-l - 0.02, 0.045, 0.04, 0.062],
            [-l - 0.045, 0.041, 0.036, 0.048],
        ],
        move |p| {
            let ergot = bump(
                Vec3::new(p.x, p.y + l + 0.025, p.z - 0.06),
                Vec3::new(0.018, 0.015, 0.012),
            );
            // A groove between the cannon bone and the tendons behind it.
            let groove = bump(
                Vec3::new(p.x.abs() - 0.03, p.y + l * 0.5, p.z - 0.012),
                Vec3::new(0.008, l * 0.35, 0.01),
            );
            Vec3::new(-p.x.signum() * 0.004 * groove, 0.0, 0.008 * ergot)
        },
    )
}

/// The pastern, from the fetlock down to the coronet along its bone (−Y).
fn pastern() -> Grid {
    let len = HORSE.coronet.length();
    quadruped::limb(
        &[
            [0.03, 0.04, 0.035, 0.05],
            [0.0, 0.042, 0.04, 0.052],
            [-len * 0.45, 0.035, 0.032, 0.036],
            [-len * 0.8, 0.039, 0.036, 0.04],
            [-len, 0.049, 0.047, 0.05],
        ],
        32,
        0.014,
    )
    .grid()
}

/// The hoof (foot frame): the wall sloping from the coronet to the ground, wider at the
/// ground and rounder at the toe, low at the heels with the bulbs behind; the coronet band
/// round its top.
fn hoof() -> Mesh {
    let sp = HORSE;
    let floor = -sp.foot_h;
    let toe_z = -sp.foot_z - sp.toe;
    // The wall over the first rows, then the sole closing the bottom.
    let wall = 0.86;
    Grid::surface(16, 36, move |row, u| {
        let s = (row / wall).min(1.0);
        let sole = 1.0 - 0.9 * smooth(wall, 1.0, row);
        let a = u * 2.0 * PI;
        let (sn, cs) = a.sin_cos();
        // s 0 is the coronet, 1 the ground; the coronet is high at the toe, low at the heel.
        let front = smooth(0.3, -1.0, cs);
        let top = lerp(floor + 0.05, sp.coronet.y + 0.006, front);
        let y = lerp(top, floor, s);
        let half = lerp(0.05, 0.064, s);
        let fwd = lerp(0.054, 0.076, s);
        let back = lerp(0.05, 0.058, s);
        let centre = lerp(sp.coronet.z + 0.02, toe_z + 0.07, s.sqrt());
        let r = if cs < 0.0 { fwd } else { back };
        // Squarer at the heels.
        let e = if cs < 0.0 { 1.0 } else { 0.8 };
        let x = sn.signum() * sn.abs().powf(e) * half * sole;
        let z = cs.signum() * cs.abs().powf(e) * r * sole;
        Vec3::new(x, y, centre + z)
    })
    .mesh(move |v: &Paint| {
        let band = smooth(0.0, 0.08, v.s / wall);
        let ridges = 0.94 + 0.06 * (v.p.y * 320.0).sin();
        rgb(0x26211d).lerp(rgb(BLACK), 1.0 - band) * ridges
    })
}

// ---------------------------------------------------------------------------------------
// Parts
// ---------------------------------------------------------------------------------------

pub fn parts(tex: &Textures) -> Vec<Part> {
    let sp = HORSE;
    let rest = rest();
    let coat = albedo(
        normal(mat(0.7).with_subsurface(0.15), tex.coat, 0.08, 0.8),
        tex.coat_albedo,
    );
    let mut hair = mat(0.62).with_subsurface(0.2).with_alpha_mask(0.35);
    hair.double_sided = true;
    hair.reflectance = 0.35;
    if let Some(t) = tex.hair_cards {
        hair = hair.with_base_color_texture(t);
    }
    let hoof_mat = normal(mat(0.45), tex.leather, 0.03, 0.25);
    let mut parts = Parts::new("horse");
    let hinge = rest[FRONT].t;

    // The skin: each side's front piece bends with its shoulder and the neck, each rear
    // piece with its thigh and gaskin; the head blends into the neck at the throatlatch.
    for (k, name) in ["body_fr", "body_fl", "body_rr", "body_rl"]
        .into_iter()
        .enumerate()
    {
        let left = k % 2 == 1;
        let side = move |p: Vec3| if left { mirror(p + hinge) } else { p + hinge };
        if k < 2 {
            parts
                .child(leg(if left { 0 } else { 2 }), move |p| {
                    shoulder_weight(side(p))
                })
                .child(NECK, move |p| neck_weight(rest[NECK], p + hinge));
        } else {
            let b = leg(if left { 3 } else { 1 });
            parts
                .child(b, move |p| thigh_weight(side(p)))
                .child(b + 1, move |p| gaskin_weight(side(p)));
        }
        parts.add(FRONT, name, move || skin()[k].clone(), coat);
    }
    let head = rest[HEAD];
    parts
        .parent(NECK, move |p| smooth(-0.1, -0.02, throat(head.point(p))))
        .add(HEAD, "head", || skin()[4].clone(), coat);
    parts.add(HEAD, "eyes", eyes, mat(0.05));
    parts.add(HEAD, "forelock", forelock, hair);
    let neck = rest[NECK];
    parts
        .parent(FRONT, move |p| 1.0 - neck_weight(neck, neck.point(p)))
        .add(NECK, "mane", move || into(mane(), neck.inverse()), hair);
    for (i, s) in [-1.0f32, 1.0].into_iter().enumerate() {
        parts.add(EAR[i], &format!("ear{i}"), move || side_of(s, ear()), coat);
    }

    // Tail: the dock with the hair that grows from it, then the fall.
    parts
        .parent(REAR, along_z(0.06, -0.04))
        .child(
            TAIL[1],
            along_z(sp.tail_seg[0] - 0.04, sp.tail_seg[0] + 0.04),
        )
        .add(TAIL[0], "tail0", || tail(0), hair);
    for k in 1..3 {
        parts.parent(TAIL[k - 1], along_z(0.04, -0.04));
        if k == 1 {
            let len = sp.tail_seg[1];
            parts.child(TAIL[2], along_z(len - 0.04, len + 0.04));
        }
        parts.add(TAIL[k], &format!("tail{k}"), move || tail(k), hair);
    }

    // Legs below the elbows and stifles.
    for (f, pastern_bone) in PASTERN.into_iter().enumerate() {
        let (fore, s) = foot_info(f);
        let b = leg(f);
        let lens = if fore { sp.fore } else { sp.hind };
        for k in 1..3 {
            let len = lens[k];
            let bone = rest[b + k];
            parts.parent(b + k - 1, along_y(-0.06, 0.06));
            let end = [0.0, 0.05, 0.035][k];
            let below = if k == 2 { pastern_bone } else { b + k + 1 };
            parts.child(below, along_y(-len + end, -len - end));
            parts.add(
                b + k,
                &format!("leg{f}_{k}"),
                move || {
                    let grid = match (fore, k) {
                        (true, 1) => forearm(),
                        (false, 1) => gaskin(),
                        _ => cannon(fore),
                    };
                    side_of(
                        s,
                        grid.mesh(move |v: &Paint| {
                            let c = bay(bone.point(v.p), bone.r * v.n);
                            // Black from the knee or hock down, shading up into the bay.
                            let black = match (fore, k) {
                                (true, 1) => smooth(-0.3, -0.44, v.p.y),
                                (false, 1) => smooth(-0.22, -0.36, v.p.y),
                                _ => 1.0,
                            };
                            points(c, black)
                        }),
                    )
                },
                coat,
            );
        }
        parts.parent(b + 2, along_y(-0.03, 0.03)).add(
            pastern_bone,
            &format!("pastern{f}"),
            move || side_of(s, pastern().mesh(|_| rgb(BLACK))),
            coat,
        );
        parts.add(
            b + 3,
            &format!("hoof{f}"),
            move || side_of(s, hoof()),
            hoof_mat,
        );
    }
    parts.list
}
