//! Lifting surfaces with planar texture mapping, for the spaceplane: every side of a panel
//! (lower, upper, and the edges between) is its own grid, so a texture can be projected
//! straight down onto the lower and upper skins (tiles stay square across the wing, the
//! elevons and the gaps between them).
//!
//! Panels use [`Panel`]'s frame (span +x, chord +z, thickness y). `uv(p, side)` maps a
//! panel-frame point on a side to texture coordinates.

use std::f32::consts::{FRAC_PI_2, PI};

use engine_core::math::{Vec2, Vec3};
use engine_render::{Mesh, Vertex};

use super::surfaces::{Panel, fix_tris, le_spacing, span_stations, within};
use super::toolkit::{fan, grid};

/// Which side of a panel a point is on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Lower,
    Upper,
    /// Coves, ribs, control-surface noses: a plain patch.
    Edge,
}

pub type UvFn<'a> = &'a (dyn Fn(Vec3, Side) -> Vec2 + Sync);

fn rows_mesh(rows: Vec<Vec<Vec3>>, side: Side, uv: UvFn) -> Mesh {
    grid(&rows, false, false, |i, j| uv(rows[i][j], side))
}

/// One skin from chord fraction `x0(s)` to `x1(s)` (front to back) on `upper` or lower.
pub fn skin_side(
    p: &Panel,
    ss: &[f32],
    upper: bool,
    x0: &dyn Fn(f32) -> f32,
    x1: &dyn Fn(f32) -> f32,
    n: usize,
    uv: UvFn,
) -> Mesh {
    let rows: Vec<Vec<Vec3>> = ss
        .iter()
        .map(|&s| {
            let (a, b) = (x0(s), x1(s));
            let xs: Vec<f32> = (0..=n)
                .map(|k| {
                    if a <= 1e-4 {
                        le_spacing(k, n, b)
                    } else {
                        a + (b - a) * k as f32 / n as f32
                    }
                })
                .collect();
            if upper {
                xs.iter().map(|&x| p.pt(s, x, true)).collect()
            } else {
                xs.iter().rev().map(|&x| p.pt(s, x, false)).collect()
            }
        })
        .collect();
    rows_mesh(rows, if upper { Side::Upper } else { Side::Lower }, uv)
}

/// The cove a control surface's nose turns in.
pub fn cove(p: &Panel, ss: &[f32], n: usize, uv: UvFn) -> Mesh {
    let rows: Vec<Vec<Vec3>> = ss
        .iter()
        .map(|&s| {
            let (h, r) = p.hinge_pt(s);
            (0..=n)
                .map(|k| {
                    let a = FRAC_PI_2 + PI * k as f32 / n as f32;
                    h + Vec3::new(0.0, r * a.sin(), r * a.cos())
                })
                .collect()
        })
        .collect();
    rows_mesh(rows, Side::Edge, uv)
}

/// A rib at span `s` between chord fractions `x0..x1`, facing `dir`.
pub fn rib(p: &Panel, s: f32, x0: f32, x1: f32, n: usize, dir: Vec3, uv: UvFn) -> Mesh {
    let mut m = Mesh::default();
    for k in 0..=n {
        let x = if x0 <= 1e-4 {
            le_spacing(k, n, x1)
        } else {
            x0 + (x1 - x0) * k as f32 / n as f32
        };
        for up in [true, false] {
            let q = p.pt(s, x, up);
            m.vertices.push(Vertex::new(q, dir, uv(q, Side::Edge)));
        }
    }
    for k in 0..n as u32 {
        let a = 2 * k;
        m.indices
            .extend_from_slice(&[a, a + 1, a + 3, a, a + 3, a + 2]);
    }
    fix_tris(&mut m, 0, dir);
    m
}

/// A control surface between spans `s0..s1`: lower and upper skins (pulled in by the gap
/// near the hinge), a round nose about the hinge, and end ribs.
pub fn control(p: &Panel, s0: f32, s1: f32, rows: usize, uv: UvFn) -> Mesh {
    let ss: Vec<f32> = (0..=rows)
        .map(|k| s0 + (s1 - s0) * k as f32 / rows as f32)
        .collect();
    let n_skin = 10;
    let n_nose = 10;
    let skin = |s: f32, k: usize, upper: bool| -> Vec3 {
        let (_, big_r) = p.hinge_pt(s);
        let r = (big_r - p.gap).max(big_r * 0.5);
        let hx = p.hinge_x(s);
        let x = hx + (1.0 - hx) * k as f32 / n_skin as f32;
        let q = p.pt(s, x, upper);
        let fadeout = (1.0 - (x - hx) / (1.0 - hx).max(1e-4)).clamp(0.0, 1.0);
        let toward = if upper { -1.0 } else { 1.0 };
        q + Vec3::Y * toward * (big_r - r) * fadeout
    };
    let lower: Vec<Vec<Vec3>> = ss
        .iter()
        .map(|&s| (0..=n_skin).rev().map(|k| skin(s, k, false)).collect())
        .collect();
    let upper: Vec<Vec<Vec3>> = ss
        .iter()
        .map(|&s| (0..=n_skin).map(|k| skin(s, k, true)).collect())
        .collect();
    let nose: Vec<Vec<Vec3>> = ss
        .iter()
        .map(|&s| {
            let (h, big_r) = p.hinge_pt(s);
            let r = (big_r - p.gap).max(big_r * 0.5);
            (0..=n_nose)
                .map(|k| {
                    let a = 1.5 * PI - PI * k as f32 / n_nose as f32;
                    h + Vec3::new(0.0, r * a.sin(), r * a.cos())
                })
                .collect()
        })
        .collect();
    let mut m = Mesh::default();
    m.merge(&rows_mesh(lower, Side::Lower, uv));
    m.merge(&rows_mesh(nose, Side::Edge, uv));
    m.merge(&rows_mesh(upper, Side::Upper, uv));
    // End ribs.
    for (s, dir) in [(s0, Vec3::NEG_X), (s1, Vec3::X)] {
        let (h, big_r) = p.hinge_pt(s);
        let r = (big_r - p.gap).max(big_r * 0.5);
        let mut outline: Vec<Vec3> = (1..=n_skin).rev().map(|k| skin(s, k, false)).collect();
        outline.extend((0..=n_nose).map(|k| {
            let a = 1.5 * PI - PI * k as f32 / n_nose as f32;
            h + Vec3::new(0.0, r * a.sin(), r * a.cos())
        }));
        outline.extend((1..=n_skin).map(|k| skin(s, k, true)));
        let t = uv(h, Side::Edge);
        m.merge(&fan(&outline, dir, |_| t));
    }
    m
}

/// A panel's fixed parts: skins to the hinge where `controls` are (to the trailing edge
/// elsewhere), coves, ribs between controls, and a root rib. `tip` closes the outer end
/// with a rib when true.
#[allow(clippy::too_many_arguments)]
pub fn fixed(
    p: &Panel,
    s0: f32,
    s1: f32,
    controls: &[(f32, f32)],
    step: f32,
    chord_n: usize,
    tip_rib: bool,
    uv: UvFn,
) -> Mesh {
    let marks: Vec<f32> = controls.iter().flat_map(|c| [c.0, c.1]).collect();
    let ss = span_stations(s0, s1, step, &marks);
    let inside = |s: f32| controls.iter().any(|c| s > c.0 + 1e-4 && s < c.1 - 1e-4);
    let mut m = Mesh::default();
    // Skins over spans with and without controls, split at the control ends.
    let mut cuts = vec![s0];
    cuts.extend(
        marks
            .iter()
            .copied()
            .filter(|&v| v > s0 + 1e-4 && v < s1 - 1e-4),
    );
    cuts.push(s1);
    cuts.sort_by(f32::total_cmp);
    cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
    for w in cuts.windows(2) {
        let sub = within(&ss, w[0], w[1]);
        if sub.len() < 2 {
            continue;
        }
        let mid = 0.5 * (w[0] + w[1]);
        let end = |s: f32| if inside(mid) { p.hinge_x(s) } else { 1.0 };
        for upper in [false, true] {
            m.merge(&skin_side(p, &sub, upper, &|_| 0.0, &end, chord_n, uv));
        }
        if inside(mid) {
            m.merge(&cove(p, &sub, 8, uv));
        }
    }
    // Ribs closing the fixed trailing edge next to each control and the coves' ends, all
    // facing the control's gap.
    for c in controls {
        for (s, dir) in [(c.0, Vec3::X), (c.1, Vec3::NEG_X)] {
            if s > s0 + 1e-3 && s < s1 - 1e-3 {
                m.merge(&rib(p, s, p.hinge_x(s), 1.0, 6, dir, uv));
            }
            let (h, r) = p.hinge_pt(s);
            let outline: Vec<Vec3> = (0..=8)
                .map(|k| {
                    let a = FRAC_PI_2 + PI * k as f32 / 8.0;
                    h + Vec3::new(0.0, r * a.sin(), r * a.cos())
                })
                .collect();
            let t = uv(h, Side::Edge);
            m.merge(&fan(&outline, dir, |_| t));
        }
    }
    m.merge(&rib(p, s0, 0.0, 1.0, chord_n, Vec3::NEG_X, uv));
    if tip_rib {
        m.merge(&rib(p, s1, 0.0, 1.0, chord_n, Vec3::X, uv));
    }
    m
}
