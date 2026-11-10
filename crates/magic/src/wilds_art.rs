//! The shapes and pictures living wood is drawn with (round-17-plan.md), made in code: the paths
//! roots take (coiling up a leg, writhing up out of the ground, arching over a hedge), a cluster
//! of leaves painted for alpha-cut cards, and a card seen from both sides.
//!
//! A root is drawn as tapered bark segments laid along one of these paths (wilds.rs), so the paths
//! here are plain points: the first under the ground where the root breaks out, the last at its
//! tip.

use std::f32::consts::{PI, TAU};

use engine_core::math::{Vec2, Vec3};
use engine_render::{Image, Mesh, Vertex};

use crate::pact_art::{fbm, hash2, lerp, pixel, smoothstep};

/// The leaf picture's size, px.
pub const LEAF_PX: u32 = 256;
/// Leaves in a cluster.
pub const LEAVES: u32 = 22;
/// How far under the ground a root starts, m: its first segment is hidden in the earth it
/// breaks.
pub const UNDER_M: f32 = 0.12;

// ------------------------------------------------------------------------------- paths

/// A dense polyline through a path, every `step` metres along it (the last point kept).
pub fn resample(points: &[Vec3], step: f32) -> Vec<Vec3> {
    let mut out = Vec::new();
    let Some(&first) = points.first() else {
        return out;
    };
    out.push(first);
    let mut carry = 0.0;
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = a.distance(b);
        if len <= 1e-6 {
            continue;
        }
        let mut t = step - carry;
        while t <= len {
            out.push(a.lerp(b, t / len));
            t += step;
        }
        carry = len - (t - step);
    }
    if let (Some(&last), Some(&end)) = (out.last(), points.last())
        && last.distance(end) > step * 0.3
    {
        out.push(end);
    }
    out
}

/// A path's length, m.
pub fn length(points: &[Vec3]) -> f32 {
    points.windows(2).map(|w| w[0].distance(w[1])).sum()
}

fn bezier(a: Vec3, b: Vec3, c: Vec3, t: f32) -> Vec3 {
    let u = 1.0 - t;
    a * (u * u) + b * (2.0 * u * t) + c * (t * t)
}

/// A root that breaks out of the ground `out_r` from a leg and coils up it: from under the
/// ground at `angle` round the leg (radians, from +X toward +Z), up in an arch to the leg's foot,
/// then round and round the leg (`turns`, counter-clockwise seen from above when `ccw`) at
/// `wrap_r` from its axis up to `top` above the ground, its tip flicked out at the end.
#[allow(clippy::too_many_arguments)]
pub fn coil(
    foot: Vec3,
    angle: f32,
    out_r: f32,
    wrap_r: f32,
    top: f32,
    turns: f32,
    ccw: bool,
) -> Vec<Vec3> {
    let dir = |a: f32| Vec3::new(a.cos(), 0.0, a.sin());
    let start = foot + dir(angle) * out_r - Vec3::Y * UNDER_M;
    let wrap_at = foot + dir(angle) * wrap_r + Vec3::Y * 0.1;
    let lift = (foot + dir(angle) * (out_r * 0.7)).with_y(foot.y + 0.6 + 0.25 * out_r);
    let mut pts: Vec<Vec3> = (0..=16)
        .map(|i| bezier(start, lift, wrap_at, i as f32 / 16.0))
        .collect();
    let sign = if ccw { -1.0 } else { 1.0 };
    let n = (turns * 28.0).ceil().max(4.0) as usize;
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let a = angle + sign * t * turns * TAU;
        // It hugs tighter as it climbs, as a vine pulled round.
        let r = wrap_r * (1.0 - 0.15 * t);
        pts.push(foot + dir(a) * r + Vec3::Y * lerp(0.1, top, t));
    }
    // The tip flicks out from the leg.
    let end = *pts.last().unwrap_or(&foot);
    let out = Vec3::new(end.x - foot.x, 0.0, end.z - foot.z).normalize_or(Vec3::X);
    pts.push(end + out * 0.08 + Vec3::Y * 0.05);
    pts
}

/// A free root writhing up out of the ground at `base`: it rises `length` metres, leaning
/// toward `lean` (level), and curls over at the top like a fern's crozier (`curl` radians
/// of bend by its tip), with a sway to one side from `seed`.
pub fn writhe(base: Vec3, length: f32, lean: Vec3, curl: f32, seed: u32) -> Vec<Vec3> {
    let lean = Vec3::new(lean.x, 0.0, lean.z).normalize_or(Vec3::X);
    let side = Vec3::new(-lean.z, 0.0, lean.x);
    let sway = (hash2(1, 2, seed) - 0.5) * 0.8;
    let mut p = base - Vec3::Y * UNDER_M;
    let n = 40;
    let ds = (length + UNDER_M) / n as f32;
    let mut pts = vec![p];
    for i in 1..=n {
        let s = i as f32 / n as f32;
        // Bent ever more toward its tip, from straight up.
        let phi = 0.15 + curl * s * s;
        let wiggle = sway * (s * PI * 1.5).sin();
        let d = (Vec3::Y * phi.cos() + lean * phi.sin() + side * wiggle * 0.35).normalize();
        p += d * ds;
        pts.push(p);
    }
    pts
}

/// A bramble arching over from `a` to `b` (both on the ground) through `height` above them,
/// bowed to one side and wavering, both ends under the ground.
pub fn bramble(a: Vec3, b: Vec3, height: f32, seed: u32) -> Vec<Vec3> {
    let span = Vec3::new(b.x - a.x, 0.0, b.z - a.z);
    let side = Vec3::new(-span.z, 0.0, span.x).normalize_or(Vec3::Z);
    let bow = (hash2(3, 4, seed) - 0.5) * 0.5;
    let n = 36;
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let arch = 4.0 * t * (1.0 - t);
            let waver = (fbm(t * 3.0, (seed % 997) as f32 * 0.37, 64, seed) - 0.5) * 0.3;
            a.lerp(b, t)
                + Vec3::Y * (height * arch - UNDER_M * (1.0 - arch))
                + side * (bow * arch + waver * arch)
        })
        .collect()
}

/// A root's girth along it, m, from `base` at the ground to a sharp tip (`s` 0..1 of its
/// length).
pub fn girth(base: f32, s: f32) -> f32 {
    base * (1.0 - 0.82 * s.clamp(0.0, 1.0).powf(0.9))
}

// ------------------------------------------------------------------------------- leaves

/// One leaf of the cluster: its base, its heading (radians), length and width (in the card's
/// 0..1), and its green.
fn leaf(i: u32) -> (Vec2, f32, f32, f32, [f32; 3]) {
    let h = |s: u32| hash2(i as i32, s as i32, 0x5eaf);
    // Round the twig a golden angle apart, so no two crowd one side.
    let a = i as f32 * 2.399_963 + 0.4 * h(1);
    // Each starts just behind the middle, where the leaves meet on their twig.
    let base = Vec2::new(0.5, 0.5) - Vec2::new(a.cos(), a.sin()) * (0.02 + 0.05 * h(2));
    // Leaves point out from the middle of the cluster.
    let heading = a + (h(3) - 0.5) * 0.9;
    let len = 0.36 + 0.1 * h(4);
    let wide = len * (0.42 + 0.14 * h(5));
    // From a fresh yellow-green to a deep summer green.
    let t = h(6);
    let green = [
        lerp(0.16, 0.035, t),
        lerp(0.36, 0.17, t),
        lerp(0.035, 0.03, t),
    ];
    (base, heading, len, wide, green)
}

/// The cluster's colour and cover at a point of the card (0..1 across, y down): the topmost leaf
/// under it, its midrib and veins lighter and its edge and base darker.
pub fn leaf_at(x: f32, y: f32) -> ([f32; 3], f32) {
    let p = Vec2::new(x, y);
    let mut out = ([0.0; 3], 0.0);
    for i in 0..LEAVES {
        let (base, heading, len, wide, green) = leaf(i);
        let along_dir = Vec2::new(heading.cos(), heading.sin());
        let d = p - base;
        let u = d.dot(along_dir) / len;
        let v = d.perp_dot(along_dir) / wide;
        if !(0.0..=1.0).contains(&u) {
            continue;
        }
        // Widest a third of the way up, to a point at the tip and a round base.
        let half = (PI * u.powf(0.75)).sin() * 0.5;
        let edge = half - v.abs();
        if edge <= 0.0 {
            continue;
        }
        let rib = smoothstep(0.035, 0.0, v.abs());
        let vein = smoothstep(
            0.03,
            0.0,
            ((v.abs() * 3.2 - u * 1.6).rem_euclid(0.42) - 0.21).abs(),
        ) * smoothstep(0.02, 0.1, v.abs());
        let shade = 0.65 + 0.35 * smoothstep(0.0, 0.3, u) - 0.25 * smoothstep(0.08, 0.0, edge);
        let light = 1.0 + 0.55 * rib + 0.2 * vein;
        out = (
            [
                green[0] * shade * light,
                green[1] * shade * light,
                green[2] * shade * light,
            ],
            smoothstep(0.0, 0.02, edge),
        );
    }
    out
}

/// The leaf cluster as a picture: its colour, and cut out round the leaves.
pub fn leaf_image() -> Image {
    let n = LEAF_PX;
    let mut img = Image::from_fn(n, n, true, |px, py| {
        let (c, a) = leaf_at((px as f32 + 0.5) / n as f32, (py as f32 + 0.5) / n as f32);
        pixel(c, a)
    });
    img.generate_mips();
    img
}

/// A square card a metre across in the XY plane, centred, seen from both sides, the picture
/// laid across it (y down in the picture is down the card).
pub fn card_mesh() -> Mesh {
    let mut m = Mesh::new(Vec::new(), Vec::new());
    let corners = [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)];
    for normal in [Vec3::NEG_Z, Vec3::Z] {
        let base = m.vertices.len() as u32;
        for (x, y) in corners {
            m.vertices.push(Vertex::new(
                Vec3::new(x, y, 0.0),
                normal,
                Vec2::new(x + 0.5, 0.5 - y),
            ));
        }
        if normal.z < 0.0 {
            m.indices
                .extend([base, base + 2, base + 1, base, base + 3, base + 2]);
        } else {
            m.indices
                .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampled_paths_keep_their_spacing_and_ends() {
        let pts = vec![
            Vec3::ZERO,
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
        ];
        let r = resample(&pts, 0.1);
        assert_eq!(r[0], Vec3::ZERO);
        assert!(r.last().unwrap().distance(Vec3::new(1.0, 1.0, 0.0)) < 1e-4);
        for w in r.windows(2) {
            assert!(w[0].distance(w[1]) <= 0.1 + 1e-4);
        }
        assert!((length(&r) - 2.0).abs() < 0.05, "{}", length(&r));
    }

    #[test]
    fn a_coil_breaks_out_beside_the_leg_and_climbs_round_it() {
        let foot = Vec3::new(2.0, 1.0, -3.0);
        let p = coil(foot, 0.7, 0.7, 0.13, 0.9, 1.5, true);
        // It starts under the ground, out from the leg.
        let s = p[0];
        assert!(s.y < foot.y);
        assert!((Vec3::new(s.x - foot.x, 0.0, s.z - foot.z).length() - 0.7).abs() < 1e-3);
        // Its arch rises clear of the ground before it reaches the leg.
        assert!(p.iter().take(17).any(|q| q.y > foot.y + 0.3));
        // The coil stays round the leg and climbs to the top.
        for q in &p[17..p.len() - 1] {
            let r = Vec3::new(q.x - foot.x, 0.0, q.z - foot.z).length();
            assert!(r <= 0.131 && r > 0.1, "{r}");
        }
        let top = p[p.len() - 2];
        assert!((top.y - (foot.y + 0.9)).abs() < 1e-3);
    }

    #[test]
    fn a_writhing_root_rises_and_curls_over() {
        let p = writhe(Vec3::ZERO, 1.2, Vec3::X, 2.4, 7);
        assert!(p[0].y < 0.0);
        let top = p.iter().map(|q| q.y).fold(f32::MIN, f32::max);
        assert!(top > 0.55 && top < 1.2, "{top}");
        // The tip has bent over: it hangs lower than the top and out along the lean.
        let tip = *p.last().unwrap();
        assert!(tip.y < top - 0.05 && tip.x > 0.2, "{tip}");
        assert!((length(&p) - (1.2 + UNDER_M)).abs() < 0.02);
    }

    #[test]
    fn a_bramble_arches_from_one_end_to_the_other() {
        let (a, b) = (Vec3::ZERO, Vec3::new(2.0, 0.0, 0.0));
        let p = bramble(a, b, 1.5, 3);
        assert!(p[0].y < 0.0 && p.last().unwrap().y < 0.0);
        let mid = p[p.len() / 2];
        assert!((mid.y - 1.5).abs() < 0.01 && (mid.x - 1.0).abs() < 0.05);
    }

    #[test]
    fn roots_taper_to_a_point() {
        assert_eq!(girth(0.1, 0.0), 0.1);
        assert!(girth(0.1, 0.5) < 0.07);
        assert!(girth(0.1, 1.0) < 0.02);
    }

    #[test]
    fn the_leaf_cluster_covers_its_middle_in_greens_and_leaves_its_corners_open() {
        let n = 64;
        let mut cover = 0;
        for j in 0..n {
            for i in 0..n {
                let (c, a) = leaf_at((i as f32 + 0.5) / n as f32, (j as f32 + 0.5) / n as f32);
                if a > 0.5 {
                    cover += 1;
                    assert!(c[1] > c[0] && c[1] > c[2], "{c:?}");
                }
            }
        }
        let share = cover as f32 / (n * n) as f32;
        assert!(share > 0.3 && share < 0.8, "{share}");
        assert_eq!(leaf_at(0.01, 0.01).1, 0.0);
        assert!(leaf_at(0.5, 0.5).1 > 0.5);
    }

    #[test]
    fn the_card_faces_both_ways() {
        let m = card_mesh();
        assert_eq!(m.vertices.len(), 8);
        assert_eq!(m.indices.len(), 12);
    }
}
