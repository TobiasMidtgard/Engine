//! Lifting surfaces: wing, tail and fin panels with a NACA section, coves, control surfaces
//! with round noses that turn about their hinge inside the cove, ribs and rounded tips.
//!
//! A [`Panel`] lives in its own frame: span along +x (`s`), chord along +z (leading edge at
//! `le(s)`, trailing edge at `te(s)`), thickness along y. Texture coordinates are arc length
//! around the section over `u_scale` (from `u0`) and span over `v_scale` (from `v0`), so
//! texels have the same size on every part of an atlas.

use std::f32::consts::{FRAC_PI_2, PI};

use engine_core::math::{Vec2, Vec3};
use engine_render::{Mesh, Vertex};

use super::toolkit::{fan, grid, naca_at};

/// A lifting panel. Control surfaces hinge on the camber line at `hinge(s)` (a z).
#[derive(Clone, Copy)]
pub struct Panel {
    pub le: fn(f32) -> f32,
    pub te: fn(f32) -> f32,
    pub thick: fn(f32) -> f32,
    pub camber: f32,
    pub hinge: fn(f32) -> f32,
    /// Gap between a control surface and its cove, meters.
    pub gap: f32,
}

/// Where a part's texels go in its atlas.
#[derive(Clone, Copy, Debug)]
pub struct Uv {
    pub u0: f32,
    pub u_scale: f32,
    pub v0: f32,
    pub v_scale: f32,
}

impl Uv {
    pub fn at_u(self, u0: f32) -> Self {
        Self { u0, ..self }
    }
    fn v(&self, s: f32) -> f32 {
        (s - self.v0) / self.v_scale
    }
}

impl Panel {
    pub fn chord(&self, s: f32) -> f32 {
        (self.te)(s) - (self.le)(s)
    }

    /// Surface point at span `s`, chord fraction `x`.
    pub fn pt(&self, s: f32, x: f32, upper: bool) -> Vec3 {
        let c = self.chord(s);
        let (_, yu, yl) = naca_at(x, (self.thick)(s), self.camber, 0.4, 0.004);
        Vec3::new(s, c * if upper { yu } else { yl }, (self.le)(s) + x * c)
    }

    pub fn hinge_x(&self, s: f32) -> f32 {
        ((self.hinge)(s) - (self.le)(s)) / self.chord(s)
    }

    /// Hinge point (on the camber line) and cove radius at span `s`.
    pub fn hinge_pt(&self, s: f32) -> (Vec3, f32) {
        let hx = self.hinge_x(s);
        let c = self.chord(s);
        let (_, yu, yl) = naca_at(hx, (self.thick)(s), self.camber, 0.4, 0.004);
        (
            Vec3::new(s, c * 0.5 * (yu + yl), (self.hinge)(s)),
            c * 0.5 * (yu - yl),
        )
    }
}

/// Chord stations, dense at the leading edge: `k` = 0..=n maps to 0..=`end`.
pub fn le_spacing(k: usize, n: usize, end: f32) -> f32 {
    let t = k as f32 / n as f32;
    end * (1.0 - (t * FRAC_PI_2).cos())
}

/// Grid from rows with arc-length U and per-row V.
pub fn surface_uv(rows: Vec<Vec<Vec3>>, uv: Uv, s_of_row: impl Fn(usize) -> f32) -> Mesh {
    let arcs: Vec<Vec<f32>> = rows
        .iter()
        .map(|r| {
            let mut a = vec![0.0];
            for k in 1..r.len() {
                a.push(a[k - 1] + (r[k] - r[k - 1]).length());
            }
            a
        })
        .collect();
    grid(&rows, false, false, |i, j| {
        Vec2::new(uv.u0 + arcs[i][j] / uv.u_scale, uv.v(s_of_row(i)))
    })
}

/// The skin from the hinge (or the trailing edge, `to_te`) forward under the leading edge and
/// back over the top.
pub fn skin(p: &Panel, ss: &[f32], to_te: bool, n: usize, uv: Uv) -> Mesh {
    let rows: Vec<Vec<Vec3>> = ss
        .iter()
        .map(|&s| {
            let end = if to_te { 1.0 } else { p.hinge_x(s) };
            let mut row: Vec<Vec3> = (0..=n)
                .rev()
                .map(|k| p.pt(s, le_spacing(k, n, end), false))
                .collect();
            row.extend((1..=n).map(|k| p.pt(s, le_spacing(k, n, end), true)));
            row
        })
        .collect();
    surface_uv(rows, uv, |i| ss[i])
}

/// One skin (upper or lower) from the hinge to the trailing edge.
pub fn te_skin(p: &Panel, ss: &[f32], upper: bool, n: usize, uv: Uv) -> Mesh {
    let rows: Vec<Vec<Vec3>> = ss
        .iter()
        .map(|&s| {
            let h = p.hinge_x(s);
            let xs: Vec<f32> = (0..=n)
                .map(|k| h + (1.0 - h) * k as f32 / n as f32)
                .collect();
            if upper {
                xs.iter().map(|&x| p.pt(s, x, true)).collect()
            } else {
                xs.iter().rev().map(|&x| p.pt(s, x, false)).collect()
            }
        })
        .collect();
    surface_uv(rows, uv, |i| ss[i])
}

/// The concave cove a control surface's nose turns in (faces aft).
pub fn cove(p: &Panel, ss: &[f32], n: usize, uv: Uv) -> Mesh {
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
    surface_uv(rows, uv, |i| ss[i])
}

/// The outline of a control surface at span `s`: lower trailing edge forward, round nose
/// about the hinge, upper surface back to the trailing edge.
pub fn control_outline(p: &Panel, s: f32, n_skin: usize, n_nose: usize) -> Vec<Vec3> {
    let (h, big_r) = p.hinge_pt(s);
    let r = (big_r - p.gap).max(big_r * 0.5);
    let hx = p.hinge_x(s);
    let skin = |x: f32, upper: bool| -> Vec3 {
        let q = p.pt(s, x, upper);
        // Pull the skin in by the gap at the hinge, fading out toward the trailing edge.
        let fadeout = (1.0 - (x - hx) / (1.0 - hx).max(1e-4)).clamp(0.0, 1.0);
        let toward = if upper { -1.0 } else { 1.0 };
        q + Vec3::Y * toward * (big_r - r) * fadeout
    };
    let mut out = Vec::new();
    for k in (1..=n_skin).rev() {
        out.push(skin(hx + (1.0 - hx) * k as f32 / n_skin as f32, false));
    }
    for k in 0..=n_nose {
        let a = 1.5 * PI - PI * k as f32 / n_nose as f32;
        out.push(h + Vec3::new(0.0, r * a.sin(), r * a.cos()));
    }
    for k in 1..=n_skin {
        out.push(skin(hx + (1.0 - hx) * k as f32 / n_skin as f32, true));
    }
    out
}

/// A control surface between spans `s0..s1` with end ribs, in the panel frame.
pub fn control_surface(p: &Panel, s0: f32, s1: f32, rows: usize, uv: Uv) -> Mesh {
    let ss: Vec<f32> = (0..=rows)
        .map(|k| s0 + (s1 - s0) * k as f32 / rows as f32)
        .collect();
    let outlines: Vec<Vec<Vec3>> = ss.iter().map(|&s| control_outline(p, s, 16, 10)).collect();
    let mut m = surface_uv(outlines.clone(), uv, |i| ss[i]);
    for (k, dir) in [(0usize, Vec3::NEG_X), (rows, Vec3::X)] {
        let (h, _) = p.hinge_pt(ss[k]);
        let o = &outlines[k];
        let base = m.vertices.len() as u32;
        let t = Vec2::new(uv.u0, uv.v(ss[k]));
        m.vertices.push(Vertex::new(h, dir, t));
        for q in o {
            m.vertices.push(Vertex::new(*q, dir, t));
        }
        for j in 0..o.len() as u32 - 1 {
            m.indices
                .extend_from_slice(&[base, base + 1 + j, base + 2 + j]);
        }
        fix_tris(&mut m, base, dir);
    }
    m
}

/// Make the triangles that use vertices from `base` on face `dir`.
pub fn fix_tris(m: &mut Mesh, base: u32, dir: Vec3) {
    let start = m
        .indices
        .iter()
        .position(|&i| i >= base)
        .map_or(m.indices.len(), |p| p - p % 3);
    let (verts, idx) = (&m.vertices, &mut m.indices);
    for t in idx[start..].as_chunks_mut::<3>().0 {
        let p = |i: u32| Vec3::from(verts[i as usize].position);
        if (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0])).dot(dir) < 0.0 {
            t.swap(1, 2);
        }
    }
}

/// A rib closing a panel at span `s` between chord fractions `x0..x1`, facing `dir`.
pub fn rib(p: &Panel, s: f32, x0: f32, x1: f32, n: usize, dir: Vec3, uv: Vec2) -> Mesh {
    let mut m = Mesh::default();
    for k in 0..=n {
        let x = x0 + (x1 - x0) * k as f32 / n as f32;
        m.vertices.push(Vertex::new(p.pt(s, x, true), dir, uv));
        m.vertices.push(Vertex::new(p.pt(s, x, false), dir, uv));
    }
    for k in 0..n as u32 {
        let a = 2 * k;
        m.indices
            .extend_from_slice(&[a, a + 1, a + 3, a, a + 3, a + 2]);
    }
    fix_tris(&mut m, 0, dir);
    m
}

/// Half-disc closing a cove's end at span `s`.
pub fn cove_cap(p: &Panel, s: f32, dir: Vec3, uv: Vec2) -> Mesh {
    let (h, r) = p.hinge_pt(s);
    let outline: Vec<Vec3> = (0..=10)
        .map(|k| {
            let a = FRAC_PI_2 + PI * k as f32 / 10.0;
            h + Vec3::new(0.0, r * a.sin(), r * a.cos())
        })
        .collect();
    fan(&outline, dir, |_| uv)
}

/// A rounded tip closing the panel past span `s_tip` over `len` meters.
pub fn tip_cap(p: &Panel, s_tip: f32, len: f32, n: usize, rows: usize, uv: Uv) -> Mesh {
    let ss: Vec<f32> = (0..=rows)
        .map(|k| s_tip + len * k as f32 / rows as f32)
        .collect();
    let c = p.chord(s_tip);
    let outlines: Vec<Vec<Vec3>> = ss
        .iter()
        .map(|&s| {
            let q = ((s - s_tip) / len).clamp(0.0, 1.0);
            let round = (1.0 - q * q).max(0.0).sqrt();
            (0..=n)
                .rev()
                .map(|k| (le_spacing(k, n, 1.0), false))
                .chain((1..=n).map(|k| (le_spacing(k, n, 1.0), true)))
                .map(|(x, up)| {
                    let base = p.pt(s_tip, x, up);
                    // Shrink toward the 60 % chord point: rounded leading-edge corner, thin edge.
                    let xc = 0.6 + (x - 0.6) * (0.4 + 0.6 * round);
                    let mid = 0.5 * (p.pt(s_tip, xc, true).y + p.pt(s_tip, xc, false).y);
                    let half = 0.5 * (p.pt(s_tip, xc, true).y - p.pt(s_tip, xc, false).y);
                    let sign = if up { 1.0 } else { -1.0 };
                    let _ = base;
                    Vec3::new(s, mid + sign * half * round, (p.le)(s_tip) + xc * c)
                })
                .collect()
        })
        .collect();
    surface_uv(outlines, uv, |i| ss[i])
}

/// Span stations from `a` to `b` about `step` apart, always including `marks` inside.
pub fn span_stations(a: f32, b: f32, step: f32, marks: &[f32]) -> Vec<f32> {
    let mut cuts = vec![a];
    cuts.extend(
        marks
            .iter()
            .copied()
            .filter(|&m| m > a + 1e-4 && m < b - 1e-4),
    );
    cuts.push(b);
    cuts.sort_by(f32::total_cmp);
    let mut out = vec![a];
    for w in cuts.windows(2) {
        let n = ((w[1] - w[0]) / step).ceil().max(1.0) as usize;
        for k in 1..=n {
            out.push(w[0] + (w[1] - w[0]) * k as f32 / n as f32);
        }
    }
    out
}

/// The stations of `ss` within `a..=b`.
pub fn within(ss: &[f32], a: f32, b: f32) -> Vec<f32> {
    ss.iter()
        .copied()
        .filter(|&s| s >= a - 1e-4 && s <= b + 1e-4)
        .collect()
}

/// Atlas part ids used by the panel builders' callers.
pub const PART_SKIN: u8 = 1;
pub const PART_CONTROL: u8 = 2;
pub const PART_FIXED_TE: u8 = 3;
pub const PART_COVE: u8 = 4;
pub const PART_TIP: u8 = 5;

/// UV columns of a panel atlas, as offsets added to the skin's `u0`.
#[derive(Clone, Copy, Debug)]
pub struct PanelLayout {
    pub control: f32,
    pub te_lower: f32,
    pub te_upper: f32,
    pub cove: f32,
}

/// A panel's fixed parts by atlas part: the skin from `s0` to `s_tip` (to the hinge where
/// `controls` spans are, to the trailing edge elsewhere), coves, ribs and a tip cap.
#[allow(clippy::too_many_arguments)]
pub fn fixed_parts(
    p: &Panel,
    s0: f32,
    s_tip: f32,
    tip_len: f32,
    controls: &[(f32, f32)],
    step: f32,
    chord_n: usize,
    uv: Uv,
    lay: PanelLayout,
) -> Vec<(u8, Mesh)> {
    let marks: Vec<f32> = controls.iter().flat_map(|c| [c.0, c.1]).collect();
    let ss = span_stations(s0, s_tip, step, &marks);
    let mut skin_m = skin(p, &ss, false, chord_n, uv);
    // Fixed trailing-edge pieces between the control surfaces.
    let mut gaps = Vec::new();
    let mut at = s0;
    let mut sorted = controls.to_vec();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
    for c in &sorted {
        if c.0 > at + 1e-4 {
            gaps.push((at, c.0));
        }
        at = c.1;
    }
    if s_tip > at + 1e-4 {
        gaps.push((at, s_tip));
    }
    let mut fixed = Mesh::default();
    for (a, b) in gaps {
        let sub = span_stations(a, b, step, &[]);
        let n = (chord_n / 3).max(4);
        fixed.merge(&te_skin(p, &sub, false, n, uv.at_u(uv.u0 + lay.te_lower)));
        fixed.merge(&te_skin(p, &sub, true, n, uv.at_u(uv.u0 + lay.te_upper)));
        let t = Vec2::new(uv.u0 + lay.te_lower, uv.v(a));
        if a > s0 + 1e-3 {
            fixed.merge(&rib(p, a, p.hinge_x(a), 1.0, 8, Vec3::NEG_X, t));
        }
        if b < s_tip - 1e-3 {
            fixed.merge(&rib(p, b, p.hinge_x(b), 1.0, 8, Vec3::X, t));
        }
    }
    let mut coves = Mesh::default();
    for &(a, b) in &sorted {
        let sub = within(&ss, a, b);
        coves.merge(&cove(p, &sub, 10, uv.at_u(uv.u0 + lay.cove)));
        let t = Vec2::new(uv.u0 + lay.cove, uv.v(a));
        coves.merge(&cove_cap(p, a, Vec3::X, t));
        coves.merge(&cove_cap(p, b, Vec3::NEG_X, t));
    }
    // Root rib (usually inside the fuselage) closes the shell for shadows.
    skin_m.merge(&rib(
        p,
        s0,
        0.0,
        p.hinge_x(s0).min(1.0),
        12,
        Vec3::NEG_X,
        Vec2::new(uv.u0, uv.v(s0)),
    ));
    let tip = tip_cap(p, s_tip, tip_len, chord_n, 6, uv);
    vec![
        (PART_SKIN, skin_m),
        (PART_FIXED_TE, fixed),
        (PART_COVE, coves),
        (PART_TIP, tip),
    ]
}
