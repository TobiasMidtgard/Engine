//! The head and body a person's hair slides off: tapered capsules on the bones, laid out as in
//! the owner's `gta-hair-sim` project (cranium, back of the head, brow, face, chin, jaw, ears,
//! neck, shoulders, upper arms, back, flanks, chest) and fitted to the engine's people.
//!
//! The hair module sees a person's skeleton and hair, not its skin, so the capsules come from
//! a template in two measures of the person: the head's are placed about the sphere through
//! the hair's roots and sized by its radius, the body's about the neck joint and sized by the
//! shoulders' half width. There are two templates, a woman's and a man's, blended by the
//! person's [`engine_world::character::Appearance::sex`]. Each was fitted to the skin of the
//! character module's people (two women, one man), from both sides: skin standing out of the
//! capsules, and capsules standing out of the skin (a fit to the skin's points alone leaves
//! capsules floating outside the body, touching it). On those people the skin is at most
//! 5 mm out of the capsules over 95% of it and 9 mm over 99%, no capsule stands more than
//! 16 mm out of the skin, and they are up to 20 mm fat where the body is hollow (under the
//! jaw, between the shoulder blades). Hair grown lying on the body there is let as deep
//! into them as it was grown ([`crate::tress::TressVertex::sunk`]).
//!
//! Model space as the animation module's humanoid: +Y up, facing -Z, +X the person's right.

use engine_core::prelude::*;
use engine_world::HairGuides;
use engine_world::animation::SkeletonPose;

/// A capsule on bone `bone` of the pose, in model space where the hair was grown.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyCapsule {
    pub bone: u16,
    pub a: Vec3,
    pub ra: f32,
    pub b: Vec3,
    pub rb: f32,
    pub friction: f32,
    /// Neck, shoulders and torso; not the head and ears.
    pub body: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum On {
    Head,
    Neck,
    Chest,
    Arm,
}

#[derive(Clone, Copy)]
enum Form {
    /// On the midline: `(0, y, z) r` to `(0, y, z) r`.
    Mid([f32; 6]),
    /// Across the midline: `(-x, y, z)` to `(x, y, z)`, radius `r`.
    Bar([f32; 4]),
    /// One on each side: `(x, y, z) r` to `(x, y, z) r`, mirrored in x.
    Pair([f32; 8]),
}

/// (bone, shape on a woman, shape on a man, friction) in head measures (about the root
/// sphere's center, in its radii).
const HEAD: [(On, Form, Form, f32); 8] = [
    (
        On::Head,
        Form::Mid([0.102, -0.152, 0.661, 0.137, -0.059, 0.889]),
        Form::Mid([0.102, -0.152, 0.661, 0.098, -0.107, 0.925]),
        0.22,
    ),
    (
        On::Head,
        Form::Mid([-0.084, 0.235, 0.849, -0.109, 0.089, 0.699]),
        Form::Mid([-0.123, 0.256, 0.86, -0.097, 0.237, 0.847]),
        0.22,
    ),
    (
        On::Head,
        Form::Bar([0.227, 0.109, -0.708, 0.489]),
        Form::Bar([0.235, 0.136, -0.794, 0.451]),
        0.20,
    ),
    (
        On::Head,
        Form::Bar([0.453, -0.587, -0.81, 0.313]),
        Form::Bar([0.423, -0.566, -0.856, 0.35]),
        0.25,
    ),
    (
        On::Head,
        Form::Mid([-1.085, -0.881, 0.267, -1.146, -0.946, 0.332]),
        Form::Mid([-1.144, -0.877, 0.295, -1.228, -0.965, 0.365]),
        0.25,
    ),
    (
        On::Head,
        Form::Pair([0.622, -0.648, -0.033, 0.282, 0.273, -1.265, -0.753, 0.236]),
        Form::Pair([0.661, -0.709, -0.025, 0.265, 0.237, -1.402, -0.773, 0.235]),
        0.25,
    ),
    (
        On::Head,
        Form::Pair([0.534, -0.422, 0.178, 0.114, 0.765, -0.384, 0.066, 0.205]),
        Form::Pair([0.532, -0.359, 0.102, 0.111, 0.748, -0.38, 0.051, 0.228]),
        0.30,
    ),
    (
        On::Neck,
        Form::Mid([-1.481, 0.114, 0.554, -1.377, 0.174, 0.554]),
        Form::Mid([-1.593, 0.299, 0.408, -1.229, 0.264, 0.531]),
        0.30,
    ),
];

/// The same in body measures (about the neck joint, in half shoulder widths).
const BODY: [(On, Form, Form, f32); 8] = [
    (
        On::Chest,
        Form::Mid([0.038, -0.114, 0.332, -0.564, -0.03, 0.208]),
        Form::Mid([0.067, -0.085, 0.337, -0.598, -0.016, 0.215]),
        0.34,
    ),
    (
        On::Chest,
        Form::Pair([0.219, -0.555, -0.346, 0.359, 0.59, -0.729, 0.188, 0.2]),
        Form::Pair([0.138, -0.555, -0.184, 0.279, 0.588, -0.729, 0.188, 0.199]),
        0.40,
    ),
    (
        On::Arm,
        Form::Pair([0.997, -0.546, -0.089, 0.37, 1.871, -2.106, -0.153, 0.201]),
        Form::Pair([0.921, -0.576, -0.005, 0.354, 1.918, -2.1, -0.091, 0.179]),
        0.42,
    ),
    (
        On::Chest,
        Form::Mid([-0.343, -0.057, 0.433, -1.022, 0.044, 0.387]),
        Form::Mid([-0.258, -0.042, 0.328, -1.012, 0.169, 0.382]),
        0.44,
    ),
    (
        On::Chest,
        Form::Pair([0.439, -0.471, -0.027, 0.471, 0.299, -1.243, -0.204, 0.588]),
        Form::Pair([0.453, -0.552, 0.075, 0.5, 0.215, -1.365, -0.107, 0.657]),
        0.44,
    ),
    (
        On::Chest,
        Form::Pair([0.723, -0.598, -0.128, 0.492, 0.552, -0.989, -0.166, 0.211]),
        Form::Pair([0.769, -0.653, -0.03, 0.452, 0.333, -1.098, -0.416, 0.239]),
        0.44,
    ),
    (
        On::Chest,
        Form::Bar([0.369, -0.671, -0.045, 0.351]),
        Form::Bar([0.369, -0.671, -0.045, 0.351]),
        0.40,
    ),
    (
        On::Chest,
        Form::Bar([0.439, -1.187, -0.566, 0.505]),
        Form::Bar([0.389, -1.103, -0.246, 0.487]),
        0.42,
    ),
];

impl Form {
    /// `t` of the way to `other` (of the same kind).
    fn toward(self, other: Form, t: f32) -> Form {
        fn mix<const N: usize>(a: [f32; N], b: [f32; N], t: f32) -> [f32; N] {
            std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
        }
        match (self, other) {
            (Form::Mid(a), Form::Mid(b)) => Form::Mid(mix(a, b, t)),
            (Form::Bar(a), Form::Bar(b)) => Form::Bar(mix(a, b, t)),
            (Form::Pair(a), Form::Pair(b)) => Form::Pair(mix(a, b, t)),
            (a, _) => a,
        }
    }
}

/// The center of the sphere through `points` that fits them best (least squares), if they
/// span one (at least 8, not all in a plane).
pub(crate) fn fit_sphere(points: &[Vec3]) -> Option<Vec3> {
    if points.len() < 8 {
        return None;
    }
    // |p|² = 2c·p + k, linear in (c, k): normal equations in double precision.
    let mean = points.iter().copied().sum::<Vec3>() / points.len() as f32;
    let mut ata = [[0.0f64; 4]; 4];
    let mut atb = [0.0f64; 4];
    for p in points {
        let q = (*p - mean).as_dvec3();
        let row = [2.0 * q.x, 2.0 * q.y, 2.0 * q.z, 1.0];
        let b = q.length_squared();
        for i in 0..4 {
            for j in 0..4 {
                ata[i][j] += row[i] * row[j];
            }
            atb[i] += row[i] * b;
        }
    }
    let m = DMat4::from_cols_array_2d(&ata);
    if m.determinant().abs() < 1e-12 {
        return None;
    }
    let x = m.inverse() * DVec4::from_array(atb);
    Some(mean + x.truncate().as_vec3())
}

/// The sphere through the roots of the hair riding the head: center and the median root's
/// distance (model space where the hair was grown).
pub fn root_sphere(guides: &HairGuides, pose: &SkeletonPose) -> Option<(Vec3, f32)> {
    let head = pose.index_of("head")?;
    let roots: Vec<Vec3> = guides
        .guides
        .iter()
        .filter(|g| g.bones[0] as usize == head && g.blend < 0.3 && g.points.len() > 1)
        .map(|g| g.points[0])
        .collect();
    let center = fit_sphere(&roots)?;
    let mut d: Vec<f32> = roots.iter().map(|r| r.distance(center)).collect();
    d.sort_by(f32::total_cmp);
    let radius = d[d.len() / 2];
    (0.05..0.2).contains(&radius).then_some((center, radius))
}

/// The capsules of the person the `guides` were grown on (`pose`: its skeleton, for the bones'
/// names; `male`: 0 a woman's shapes .. 1 a man's, [`engine_world::character::Appearance::sex`]).
/// Empty when the hair's roots don't span a head; without the neck, chest and upper arm bones
/// only the head's.
pub fn capsules(guides: &HairGuides, pose: &SkeletonPose, male: f32) -> Vec<BodyCapsule> {
    let male = male.clamp(0.0, 1.0);
    let mut out = Vec::new();
    let Some((center, radius)) = root_sphere(guides, pose) else {
        return out;
    };
    let bone = |name: &str| {
        let i = pose.index_of(name)?;
        Some((i as u16, guides.grown_on.get(i)?.translation))
    };
    let Some((head, _)) = bone("head") else {
        return out;
    };
    let neck = bone("neck");
    let chest = bone("chest");
    let arms = bone("upper_arm_l").zip(bone("upper_arm_r"));
    let mut lay = |table: &[(On, Form, Form, f32)], origin: Vec3, scale: f32, body: bool| {
        let at = |x: f32, y: f32, z: f32| origin + Vec3::new(x, y, z) * scale;
        for &(on, woman, man, friction) in table {
            let form = woman.toward(man, male);
            let ride = |x: f32| match on {
                On::Head => Some(head),
                On::Neck => Some(neck.map_or(head, |n| n.0)),
                On::Chest => chest.map(|c| c.0),
                // The arm on the side the capsule is on.
                On::Arm => arms.map(|(l, r)| {
                    if (l.1.x < r.1.x) == (x < 0.0) {
                        l.0
                    } else {
                        r.0
                    }
                }),
            };
            let mut push = |x: f32, a: Vec3, ra: f32, b: Vec3, rb: f32| {
                if let Some(bone) = ride(x) {
                    out.push(BodyCapsule {
                        bone,
                        a,
                        ra: ra * scale,
                        b,
                        rb: rb * scale,
                        friction,
                        body: body || on == On::Neck,
                    });
                }
            };
            match form {
                Form::Mid([ay, az, ra, by, bz, rb]) => {
                    push(0.0, at(0.0, ay, az), ra, at(0.0, by, bz), rb);
                }
                Form::Bar([x, y, z, r]) => push(0.0, at(-x, y, z), r, at(x, y, z), r),
                Form::Pair([ax, ay, az, ra, bx, by, bz, rb]) => {
                    for side in [-1.0, 1.0] {
                        push(side, at(side * ax, ay, az), ra, at(side * bx, by, bz), rb);
                    }
                }
            }
        }
    };
    lay(&HEAD, center, radius, false);
    if let (Some((_, neck)), Some((l, r))) = (neck, arms) {
        let half = l.1.distance(r.1) * 0.5;
        if (0.08..0.4).contains(&half) {
            lay(&BODY, neck, half, true);
        }
    }
    out
}

/// How far `p` is outside the nearest of `capsules` of a kind (negative inside): the head's
/// and ears' when `body` is false, the neck's and body's when true. Infinite with none.
pub fn distance(capsules: &[BodyCapsule], p: Vec3, body: bool) -> f32 {
    capsules
        .iter()
        .filter(|c| c.body == body)
        .map(|c| crate::tress::tapered_capsule(p, c.a, c.b, c.ra, c.rb).1)
        .fold(f32::INFINITY, f32::min)
}
