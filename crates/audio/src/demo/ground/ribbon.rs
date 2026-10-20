//! Ground ribbons that follow the trail exactly: a grid in (distance along, distance across)
//! displaced by a height function, with UVs in world meters (so neighbouring ribbons of the
//! same material continue seamlessly and nothing stretches round the bends).
//!
//! Edges fray instead of ending in a straight line: the vertex alpha ramps from 1 down to
//! 0.45 over a ragged band, and the ground textures carry a 0.5–1 blob pattern in their
//! alpha, so with an alpha-mask cutoff of 0.5 the edge follows the blobs. A ribbon that
//! starts with a tongue (`fade_in`) lies a few millimeters above the stretch before it, so
//! one ground creeps over the other; where the tongue ends wanders back and forth across
//! the path, so neighbouring grounds meet along a ragged, slanting line rather than a
//! straight cut. Ground that dips under other ground (a snow blanket's edge) reports a
//! negative coverage there, and triangles wholly under it are dropped.

use engine_core::math::{Vec2, Vec3, Vec4};
use engine_render::{Mesh, Vertex};

use super::procgen::{fbm_world, smooth};
use super::{CHUNK, Mat, Part};
use crate::demo::trail_point;

/// Height of the path's surface above the trail's origin, meters (where feet land).
pub(crate) const PATH_Y: f32 = 0.06;
/// How far a tongue lies above the ground it overlaps, meters.
const TONGUE_LIFT: f32 = 0.003;
/// How far a tongue of leaf litter lies above the ground it overlaps: its own thickness, so
/// it ends in a lip that catches the light and casts a shadow.
const LITTER_LIFT: f32 = 0.009;

/// Centre (relative to the origin), heading and right of the trail at `s`.
pub(super) fn frame(s: f64) -> (Vec3, Vec3, Vec3) {
    let (p, d) = trail_point(s);
    (p.as_vec3(), d, Vec3::new(-d.z, 0.0, d.x))
}

/// The point `x` meters right of the centre line at `s`, `y` above the origin.
pub(super) fn point(s: f64, x: f32, y: f32) -> Vec3 {
    let (p, _, r) = frame(s);
    p + r * x + Vec3::Y * y
}

/// 1 on the path, falling to 0 over the shoulders.
pub(super) fn shoulder(x: f32) -> f32 {
    1.0 - smooth(1.15, 2.35, x.abs())
}

/// Lumps on the shoulders (never below the surrounding ground), meters.
pub(super) fn undulation(w: Vec2, x: f32) -> f32 {
    let ax = x.abs();
    let k = smooth(0.95, 1.7, ax) * (1.0 - smooth(2.15, 2.6, ax));
    let lumps = (0.5 + 0.6 * fbm_world(w.x, w.y, 1.6, 3, 0x51)).clamp(0.0, 1.0) * 0.022;
    lumps * k + fbm_world(w.x, w.y, 5.0, 2, 0x52) * 0.002 * (1.0 - smooth(2.2, 2.6, ax))
}

/// The ground's base height at `x` across (path level in the band, sloping down to the
/// surrounding ground), at world position `w` (meters, relative to the origin).
pub(super) fn base(x: f32, w: Vec2) -> f32 {
    (PATH_Y - 0.002) * shoulder(x) + 0.002 + undulation(w, x)
}

/// The verges' height: just under the path's own ground.
pub(super) fn verge_height(x: f32, w: Vec2) -> f32 {
    base(x, w) - 0.003 * shoulder(x)
}

/// Fade of a stretch's own relief toward both ends (so neighbours meet at the base height).
pub(super) fn ends(s: f64, s0: f64, s1: f64, len: f64) -> f32 {
    (smooth(0.0, len as f32, (s - s0) as f32) * smooth(0.0, len as f32, (s1 - s) as f32)).max(0.0)
}

/// Columns (meters across) from `-max` to `max`: `fine` spacing up to `fine_to`, `mid`
/// spacing up to `mid_to`, then `coarse`.
pub(super) fn columns(
    max: f32,
    fine: f32,
    fine_to: f32,
    mid: f32,
    mid_to: f32,
    coarse: f32,
) -> Vec<f32> {
    let mut half = vec![0.0f32];
    let mut x = 0.0;
    while x < max - 1e-4 {
        let step = if x < fine_to {
            fine
        } else if x < mid_to {
            mid
        } else {
            coarse
        };
        x = (x + step).min(max);
        half.push(x);
    }
    let mut all: Vec<f32> = half.iter().rev().map(|x| -x).collect();
    all.extend_from_slice(&half[1..]);
    all
}

/// Columns on one side only, `from`..`to` meters out (negative `side` for the left).
pub(super) fn side_columns(from: f32, to: f32, step: f32, side: f32) -> Vec<f32> {
    let n = ((to - from) / step).ceil().max(1.0) as usize;
    let mut c: Vec<f32> = (0..=n)
        .map(|i| (from + (to - from) * i as f32 / n as f32) * side)
        .collect();
    c.sort_by(|a, b| a.total_cmp(b));
    c
}

/// Coverage across: 1 inside `solid`, ramping to 0.45 at `ragged` (both measured outward
/// from the centre line).
pub(super) fn fray(x: f32, solid: f32, ragged: f32) -> f32 {
    let ax = x.abs();
    if ax <= solid {
        1.0
    } else {
        1.0 - 0.55 * ((ax - solid) / (ragged - solid).max(1e-3)).min(1.0)
    }
}

/// What a ribbon's ground is like at one point.
pub(super) struct Surf {
    /// Height above the origin, meters.
    pub y: f32,
    /// Coverage: 1 solid, down to 0.45 at a ragged edge. Negative where the surface lies
    /// under other ground: triangles with every corner there are dropped.
    pub cover: f32,
    /// Vertex colour (multiplied with the texture).
    pub tint: Vec3,
}

impl Surf {
    pub fn new(y: f32, cover: f32, tint: Vec3) -> Self {
        Self { y, cover, tint }
    }
}

/// One ribbon of ground.
pub(super) struct Ribbon<'a> {
    pub mat: Mat,
    /// Fully covered from `s0` to `s1` (meters along the trail).
    pub s0: f64,
    pub s1: f64,
    /// Ragged tongue before `s0` / after `s1`, meters (0 = a straight edge).
    pub fade_in: f64,
    pub fade_out: f64,
    /// Row spacing along the trail, meters.
    pub ds: f64,
    /// Column positions across, meters, sorted.
    pub columns: Vec<f32>,
    /// The ground at (s, x, world xz): height, coverage and tint.
    pub surface: &'a (dyn Fn(f64, f32, Vec2) -> Surf + Sync),
    /// Casts shadows (banks, ruts and drifts do; flat ground needn't).
    pub shadows: bool,
}

/// Build a ribbon as one part per [`CHUNK`] of trail.
pub(super) fn ribbon(r: &Ribbon) -> Vec<Part> {
    let start = r.s0 - r.fade_in;
    let end = r.s1 + r.fade_out;
    let k0 = (start / r.ds).floor() as i64;
    let k1 = (end / r.ds).ceil() as i64;
    let per_chunk = (CHUNK / r.ds).round().max(1.0) as i64;
    let mut parts = Vec::new();
    let mut a = k0;
    while a < k1 {
        // Chunk boundaries on a global grid so chunks of different ribbons line up.
        let b = (((a + 1) as f64 * r.ds / CHUNK).ceil() as i64 * per_chunk).min(k1);
        let b = b.max(a + 1);
        if let Some(part) = chunk(r, a, b) {
            parts.push(part);
        }
        a = b;
    }
    parts
}

fn chunk(r: &Ribbon, ka: i64, kb: i64) -> Option<Part> {
    let s_of = |k: i64| k as f64 * r.ds;
    let mid = (s_of(ka) + s_of(kb)) * 0.5;
    let anchor = frame(mid).0;
    let nc = r.columns.len();
    if nc < 2 {
        return None;
    }
    // Columns with one extra on each side, rows with one extra before and after (for normals).
    let mut cols = Vec::with_capacity(nc + 2);
    cols.push(r.columns[0] - (r.columns[1] - r.columns[0]));
    cols.extend_from_slice(&r.columns);
    cols.push(r.columns[nc - 1] + (r.columns[nc - 1] - r.columns[nc - 2]));
    let rows = (kb - ka + 3) as usize;
    let w = cols.len();
    let mut pos = vec![Vec3::ZERO; rows * w];
    let mut world = vec![Vec2::ZERO; rows * w];
    let mut look = vec![(0.0f32, Vec3::ONE); rows * w];
    for i in 0..rows {
        let k = ka - 1 + i as i64;
        // Extra rows are sampled just outside the ribbon (unclamped) for smooth normals.
        let s = k as f64 * r.ds;
        let (p, _, right) = frame(s);
        for (j, &x) in cols.iter().enumerate() {
            let flat = p + right * x;
            let wxz = Vec2::new(flat.x, flat.z);
            let surf = (r.surface)(s, x, wxz);
            look[i * w + j] = (surf.cover, surf.tint);
            let mut y = surf.y;
            let lift = if r.mat == Mat::Forest {
                LITTER_LIFT
            } else {
                TONGUE_LIFT
            };
            if r.fade_in > 0.0 {
                y += lift * (1.0 - smooth(r.s0 as f32, r.s0 as f32 + 0.3, s as f32));
            }
            if r.fade_out > 0.0 {
                y += lift * smooth(r.s1 as f32 - 0.3, r.s1 as f32, s as f32);
            }
            pos[i * w + j] = flat + Vec3::Y * y;
            world[i * w + j] = wxz;
        }
    }
    // Where a tongue is solid wanders back and forth across the path and slants (by up to
    // half the tongue), so neighbouring grounds meet along a ragged, slanting line. The
    // coverage always reaches its fully ragged 0.45 at the tongue's tip.
    let wander = |x: f32, at: f64| {
        let slant = if (at * 0.37).fract() < 0.5 {
            0.16
        } else {
            -0.16
        };
        (0.5 + 0.4 * fbm_world(x, at as f32 * 0.37, 0.7, 3, 0x3e) + slant * x).clamp(0.0, 1.0)
    };
    let along_cover = |s: f64, x: f32| -> f32 {
        let a = if r.fade_in > 0.0 {
            let tip = r.s0 - r.fade_in;
            let solid = r.s0 - r.fade_in * 0.5 * wander(x, r.s0) as f64;
            let t = ((solid - s) / (solid - tip)).clamp(0.0, 1.0);
            1.0 - 0.55 * t as f32
        } else {
            1.0
        };
        let b = if r.fade_out > 0.0 {
            let tip = r.s1 + r.fade_out;
            let solid = r.s1 + r.fade_out * 0.5 * wander(x, r.s1) as f64;
            let t = ((s - solid) / (tip - solid)).clamp(0.0, 1.0);
            1.0 - 0.55 * t as f32
        } else {
            1.0
        };
        a.min(b)
    };
    let mut m = Mesh::default();
    let out_rows = (kb - ka + 1) as usize;
    // Coverage per vertex; negative where the ground lies under other ground.
    let mut alpha = Vec::with_capacity(out_rows * nc);
    for i in 1..=out_rows {
        let k = ka - 1 + i as i64;
        let s = s_of(k);
        for j in 1..=nc {
            let at = |ii: usize, jj: usize| pos[ii * w + jj];
            let across = at(i, j + 1) - at(i, j - 1);
            let fwd = at(i + 1, j) - at(i - 1, j);
            let n = across.cross(fwd).normalize_or(Vec3::Y);
            let n = if n.y < 0.0 { -n } else { n };
            let x = cols[j];
            let wxz = world[i * w + j];
            let (cover, c) = look[i * w + j];
            let a = cover * along_cover(s, x);
            alpha.push(a);
            // Corners under other ground keep a solid alpha (no ragged edge leaks toward
            // them); the depth test hides that part of the triangle.
            let va = if a < 0.0 { 1.0 } else { a.min(1.0) };
            m.vertices.push(
                Vertex::new(pos[i * w + j] - anchor, n, wxz)
                    .with_color(Vec4::new(c.x, c.y, c.z, va)),
            );
        }
    }
    for i in 0..out_rows - 1 {
        for j in 0..nc - 1 {
            let a = (i * nc + j) as u32;
            let b = a + 1;
            let c = a + nc as u32;
            let d = c + 1;
            let vis = |t: [u32; 3]| t.iter().any(|&v| alpha[v as usize] >= 0.5);
            for t in [[a, b, c], [b, d, c]] {
                if vis(t) {
                    m.indices.extend_from_slice(&t);
                }
            }
        }
    }
    // Make sure every triangle faces up (rows advance along the trail, columns rightward).
    for t in m.indices.as_chunks_mut::<3>().0 {
        let [pa, pb, pc] = [t[0], t[1], t[2]].map(|v| Vec3::from(m.vertices[v as usize].position));
        if (pb - pa).cross(pc - pa).y < 0.0 {
            t.swap(1, 2);
        }
    }
    if m.indices.is_empty() {
        return None;
    }
    let mut part = Part::own(m, r.mat, anchor);
    part.shadows = r.shadows;
    Some(part)
}
