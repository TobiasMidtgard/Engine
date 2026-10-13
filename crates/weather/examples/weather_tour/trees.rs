//! Procedural trees: broadleaf trees grown by recursive branching inside a crown envelope
//! (tapered tubes with smooth normals, leaf-cluster cards at the twigs), conifers (a straight
//! trunk with whorls of drooping frond cards), bushes, and impostors (front views of a
//! template baked on the CPU) for distant trees.
//!
//! Wood and foliage of one template are separate meshes (different materials) whose bounds
//! are made identical, so the renderer's height-based wind sway moves them together.

use std::f32::consts::{PI, TAU};

use engine_core::math::{Quat, Vec2, Vec3, Vec4};
use engine_render::{Image, Mesh, Vertex};

use crate::noise::{Rng, smoothstep32 as smoothstep};
use crate::terrain::srgb8_to_linear;
use crate::texgen::{BARK_TILE_M, Canvas, TextureSet};

/// One tree template: wood and foliage meshes, plus its size.
pub struct TreeMeshes {
    pub wood: Mesh,
    pub leaves: Mesh,
    /// Top of the crown above the ground, m.
    pub height: f32,
    /// Horizontal radius of the crown, m.
    pub radius: f32,
}

/// Add a tube along `points` with `radii`, `sides` around, bark UVs (`v` in bark tiles along
/// the length). `color(point, t)` gives the vertex color (t = 0..1 along the tube).
pub fn add_tube(
    mesh: &mut Mesh,
    points: &[Vec3],
    radii: &[f32],
    sides: usize,
    color: &dyn Fn(Vec3, f32) -> Vec4,
) {
    let n = points.len();
    if n < 2 {
        return;
    }
    let tangent = |i: usize| -> Vec3 {
        let (a, b) = if i == 0 {
            (points[0], points[1])
        } else if i == n - 1 {
            (points[n - 2], points[n - 1])
        } else {
            (points[i - 1], points[i + 1])
        };
        (b - a).normalize_or(Vec3::Y)
    };
    let around = ((TAU * radii[0] / BARK_TILE_M.0).round()).max(1.0);
    let base = mesh.vertices.len() as u32;
    let mut t_prev = tangent(0);
    let mut nrm = t_prev.any_orthonormal_vector();
    let mut length = 0.0;
    let total: f32 = points.windows(2).map(|w| (w[1] - w[0]).length()).sum();
    for i in 0..n {
        let t = tangent(i);
        if i > 0 {
            nrm = Quat::from_rotation_arc(t_prev, t) * nrm;
            nrm = (nrm - t * nrm.dot(t)).normalize_or(t.any_orthonormal_vector());
            length += (points[i] - points[i - 1]).length();
        }
        t_prev = t;
        let bin = t.cross(nrm);
        // Slope of the radius for the normals of a tapered tube.
        let (r0, r1) = (radii[i.saturating_sub(1)], radii[(i + 1).min(n - 1)]);
        let (p0, p1) = (points[i.saturating_sub(1)], points[(i + 1).min(n - 1)]);
        let slope = (r0 - r1) / (p1 - p0).length().max(1e-4);
        let c = color(points[i], length / total.max(1e-4));
        for s in 0..=sides {
            let a = s as f32 / sides as f32 * TAU;
            let radial = nrm * a.cos() + bin * a.sin();
            let normal = (radial + t * slope).normalize();
            mesh.vertices.push(
                Vertex::new(
                    points[i] + radial * radii[i],
                    normal,
                    Vec2::new(s as f32 / sides as f32 * around, -length / BARK_TILE_M.1),
                )
                .with_color(c),
            );
        }
    }
    let row = (sides + 1) as u32;
    for i in 0..(n - 1) as u32 {
        for s in 0..sides as u32 {
            let a = base + i * row + s;
            let (b, c, d) = (a + 1, a + row + 1, a + row);
            mesh.indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }
}

/// Add a degenerate triangle spanning `y0..y1` on the axis, so meshes of one template share
/// the same bounds (the renderer scales wind sway by height within the mesh bounds).
fn add_bounds_anchor(mesh: &mut Mesh, y0: f32, y1: f32) {
    let base = mesh.vertices.len() as u32;
    for y in [y0, y1, y1] {
        mesh.vertices
            .push(Vertex::new(Vec3::new(0.0, y, 0.0), Vec3::Y, Vec2::ZERO));
    }
    mesh.indices.extend_from_slice(&[base, base + 1, base + 2]);
}

/// A leaf card: a quad bent along its vertical center line, with its base (texture bottom
/// center) at `base`, growing along `up`, facing `facing` (projected), with vertex normals
/// bent toward `outward` (a rounder canopy).
#[allow(clippy::too_many_arguments)]
pub fn add_card(
    mesh: &mut Mesh,
    base: Vec3,
    up: Vec3,
    facing: Vec3,
    outward: Vec3,
    width: f32,
    height: f32,
    fold: f32,
    uv_rect: [f32; 4],
    color: Vec4,
    normal_bend: f32,
) {
    let up = up.normalize();
    let right = up.cross(facing).normalize_or(up.any_orthonormal_vector());
    let face = right.cross(up).normalize();
    let [u0, v0, u1, v1] = uv_rect;
    let um = (u0 + u1) * 0.5;
    let rows = [(0.0, v1), (1.0, v0)];
    let idx = mesh.vertices.len() as u32;
    for (k, (h, v)) in rows.iter().enumerate() {
        let center =
            base + up * (height * h) + face * (fold * width * if k == 1 { 1.0 } else { 0.6 });
        let left = base + up * (height * h) - right * (width * 0.5);
        let right_p = base + up * (height * h) + right * (width * 0.5);
        for (p, u) in [(left, u0), (center, um), (right_p, u1)] {
            let n = (face * (1.0 - normal_bend) + outward * normal_bend).normalize_or(face);
            mesh.vertices
                .push(Vertex::new(p, n, Vec2::new(u, *v)).with_color(color));
        }
    }
    // Rows: 0 = bottom (left, mid, right), 1 = top.
    let (bl, bm, br, tl, tm, tr) = (idx, idx + 1, idx + 2, idx + 3, idx + 4, idx + 5);
    two_sided(mesh, &[bl, bm, tm, bl, tm, tl, bm, br, tr, bm, tr, tm]);
}

/// Add triangles facing both ways (sharing vertices). Foliage is drawn single-sided with
/// these, so both sides keep the bent "canopy" normals instead of the back side getting a
/// flipped normal (which would make every card seen from behind dark).
pub fn two_sided(mesh: &mut Mesh, tris: &[u32]) {
    mesh.indices.extend_from_slice(tris);
    for t in tris.as_chunks::<3>().0 {
        mesh.indices.extend_from_slice(&[t[0], t[2], t[1]]);
    }
}

// ------------------------------------------------------------------ broadleaf trees

/// Shape of a broadleaf tree.
#[derive(Clone, Copy, Debug)]
pub struct Broadleaf {
    pub seed: u64,
    pub height: f32,
    pub trunk_radius: f32,
    /// Where the crown starts, as a fraction of the height.
    pub crown_base: f32,
    /// Crown radius as a fraction of the height.
    pub crown_width: f32,
    pub scaffolds: usize,
    /// Angle of the main branches from vertical, radians.
    pub scaffold_angle: f32,
    /// Upward (+) or drooping (−) tendency of the branches.
    pub tropism: f32,
    /// Leaf card size, m.
    pub card: f32,
    /// Leaf cards per meter of twig.
    pub leafiness: f32,
    /// Twig geometry (hero trees); off for cheaper versions.
    pub twigs: bool,
    /// Extra cards spread over the outer crown, per m² of crown surface (fills the canopy
    /// between the branch tips).
    pub shell: f32,
    /// Multi-stemmed from the ground (bushes).
    pub bush: bool,
}

struct Envelope {
    center: Vec3,
    radii: Vec3,
}

impl Envelope {
    /// Distance along `dir` from `p` (inside) to the envelope surface.
    fn exit(&self, p: Vec3, dir: Vec3) -> f32 {
        let o = (p - self.center) / self.radii;
        let d = dir / self.radii;
        let a = d.dot(d);
        let b = 2.0 * o.dot(d);
        let c = o.dot(o) - 1.0;
        let disc = b * b - 4.0 * a * c;
        if disc < 0.0 || a <= 0.0 {
            return 0.0;
        }
        ((-b + disc.sqrt()) / (2.0 * a)).max(0.0)
    }

    /// Normalized position: 0 at the center, 1 on the surface.
    fn depth(&self, p: Vec3) -> f32 {
        ((p - self.center) / self.radii).length()
    }

    fn outward(&self, p: Vec3) -> Vec3 {
        ((p - self.center) / (self.radii * self.radii)).normalize_or(Vec3::Y)
    }
}

struct LeafSpot {
    at: Vec3,
    dir: Vec3,
}

struct Grower<'a> {
    style: &'a Broadleaf,
    rng: Rng,
    env: Envelope,
    wood: Mesh,
    spots: Vec<LeafSpot>,
}

const SIDES: [usize; 4] = [12, 7, 4, 3];
const SEG_LEN: [f32; 4] = [0.55, 0.6, 0.6, 1.0];

impl Grower<'_> {
    fn wood_color(&self, p: Vec3) -> Vec4 {
        // Inner crown wood is shaded; the trunk base a little greener (moss).
        let depth = self.env.depth(p).min(1.2);
        let ao = 0.55 + 0.45 * smoothstep(0.2, 1.0, depth);
        let ground = smoothstep(0.0, 1.2, p.y);
        Vec4::new(
            ao * (0.8 + 0.2 * ground),
            ao * (0.9 + 0.1 * ground),
            ao * (0.75 + 0.25 * ground),
            1.0,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn grow(
        &mut self,
        level: usize,
        start: Vec3,
        dir: Vec3,
        length: f32,
        radius: f32,
        tip_radius: f32,
    ) {
        let s = *self.style;
        let segs = ((length / SEG_LEN[level]).ceil() as usize).clamp(1, 16);
        let mut pts = vec![start];
        let mut radii = vec![radius];
        let mut d = dir.normalize();
        let mut p = start;
        let step = length / segs as f32;
        for k in 1..=segs {
            let t = k as f32 / segs as f32;
            // Bend: tropism toward the light (or droop), a little randomness, and an extra
            // droop for long side branches toward their tips.
            let mut bend = Vec3::Y * s.tropism * 0.12
                + Vec3::new(
                    self.rng.signed(),
                    self.rng.signed() * 0.5,
                    self.rng.signed(),
                ) * if level == 0 { 0.04 } else { 0.14 };
            if level > 0 {
                bend.y -= 0.05 * t * (length / 6.0).min(1.5);
            }
            d = (d + bend).normalize();
            if level == 0 && !s.bush {
                d = (d + Vec3::Y * 0.25).normalize();
            }
            p += d * step;
            pts.push(p);
            radii.push(radius + (tip_radius - radius) * t.powf(0.8));
        }
        // Root flare: a wider, slightly lobed base below the first ring.
        if level == 0 && !s.bush {
            pts.insert(0, start - Vec3::Y * 0.6);
            radii.insert(0, radius * 1.45);
        }
        if level <= 1 || s.twigs || level < 3 {
            let sides = SIDES[level];
            let env_depth: Vec<f32> = pts.iter().map(|q| self.wood_color(*q).x).collect();
            let _ = env_depth;
            let env = Envelope {
                center: self.env.center,
                radii: self.env.radii,
            };
            let color = move |q: Vec3, _t: f32| {
                let depth = env.depth(q).min(1.2);
                let ao = 0.55 + 0.45 * smoothstep(0.2, 1.0, depth);
                let ground = smoothstep(0.0, 1.2, q.y);
                Vec4::new(
                    ao * (0.8 + 0.2 * ground),
                    ao,
                    ao * (0.75 + 0.25 * ground),
                    1.0,
                )
            };
            add_tube(&mut self.wood, &pts, &radii, sides, &color);
        }
        let point_at = |t: f32, pts: &[Vec3]| -> (Vec3, Vec3) {
            let f = t * (pts.len() - 1) as f32;
            let i = (f.floor() as usize).min(pts.len() - 2);
            let fr = f - i as f32;
            (
                pts[i] + (pts[i + 1] - pts[i]) * fr,
                (pts[i + 1] - pts[i]).normalize_or(Vec3::Y),
            )
        };
        let radius_at = |t: f32| radius + (tip_radius - radius) * t.powf(0.8);
        match level {
            0 => {
                // Main branches around the trunk (phyllotaxis), from the crown base up.
                let h = s.height;
                let n = s.scaffolds;
                let mut phi = self.rng.f32() * TAU;
                for k in 0..n {
                    let t0 = if s.bush { 0.02 } else { s.crown_base * 0.95 };
                    let t = t0 + (0.92 - t0) * (k as f32 + self.rng.range(0.1, 0.9)) / n as f32;
                    let t = t.min(0.95);
                    phi += 2.4 + self.rng.signed() * 0.35;
                    let angle = s.scaffold_angle * (1.0 - 0.45 * t) + self.rng.signed() * 0.12;
                    let (at, axis) = point_at(t.min(0.98), &pts);
                    let side = Vec3::new(phi.cos(), 0.0, phi.sin());
                    let cdir = (axis * angle.cos() + side * angle.sin()).normalize();
                    let reach = self.env.exit(at, cdir);
                    let len = (reach * self.rng.range(0.75, 0.95)).max(h * 0.12);
                    let r = (radius_at(t) * self.rng.range(0.55, 0.72)).max(0.03);
                    self.grow(1, at, cdir, len, r, (r * 0.18).max(0.012));
                }
                // The leader carries leaves at the top too.
                let (top, axis) = point_at(1.0, &pts);
                self.spots.push(LeafSpot { at: top, dir: axis });
            }
            1 | 2 => {
                let per_m = if level == 1 { 1.5 } else { 2.2 };
                let n = ((length * per_m).round() as usize).max(2);
                let mut phi = self.rng.f32() * TAU;
                for k in 0..n {
                    let t = 0.22 + 0.78 * (k as f32 + self.rng.range(0.2, 0.8)) / n as f32;
                    phi += 2.4 + self.rng.signed() * 0.4;
                    let (at, axis) = point_at(t, &pts);
                    let side = axis.any_orthonormal_vector();
                    let side = Quat::from_axis_angle(axis, phi) * side;
                    // Children lean outward and up a little.
                    let side = (side + self.env.outward(at) * 0.6 + Vec3::Y * 0.2).normalize();
                    let angle = self.rng.range(0.6, 1.0);
                    let cdir = (axis * angle.cos() + side * angle.sin()).normalize();
                    let reach = self.env.exit(at, cdir);
                    let want = if level == 1 {
                        length * self.rng.range(0.35, 0.55) * (1.0 - 0.4 * t) + 0.4
                    } else {
                        self.rng.range(0.45, 1.0)
                    };
                    let len = want.min(reach + 0.3).max(0.3);
                    let r = (radius_at(t) * 0.6).max(0.008);
                    self.grow(level + 1, at, cdir, len, r, (r * 0.3).max(0.006));
                }
                if level == 2 {
                    // Leaves along the outer part of the branch as well.
                    let count = ((length * s.leafiness * 0.5).round() as usize).max(1);
                    for k in 0..count {
                        let t = 0.45 + 0.55 * (k as f32 + 0.5) / count as f32;
                        let (at, axis) = point_at(t, &pts);
                        self.spots.push(LeafSpot { at, dir: axis });
                    }
                }
            }
            _ => {
                let count = ((length * s.leafiness).round() as usize).max(1);
                for k in 0..count {
                    let t = 0.35 + 0.65 * (k as f32 + 0.5) / count as f32;
                    let (at, axis) = point_at(t, &pts);
                    self.spots.push(LeafSpot { at, dir: axis });
                }
            }
        }
    }
}

/// Grow a broadleaf tree (or bush).
pub fn broadleaf(style: &Broadleaf) -> TreeMeshes {
    let s = style;
    let crown_radius = s.height * s.crown_width;
    let crown_bottom = s.height * s.crown_base;
    let env = Envelope {
        center: Vec3::new(0.0, (crown_bottom + s.height) * 0.5 + s.height * 0.03, 0.0),
        radii: Vec3::new(crown_radius, (s.height - crown_bottom) * 0.58, crown_radius),
    };
    let mut g = Grower {
        style: s,
        rng: Rng::new(s.seed),
        env,
        wood: Mesh::default(),
        spots: Vec::new(),
    };
    if s.bush {
        // Several stems from the ground act as the main branches.
        let n = s.scaffolds;
        for k in 0..n {
            let phi = k as f32 / n as f32 * TAU + g.rng.signed() * 0.4;
            let angle = s.scaffold_angle * g.rng.range(0.5, 1.1);
            let side = Vec3::new(phi.cos(), 0.0, phi.sin());
            let dir = (Vec3::Y * angle.cos() + side * angle.sin()).normalize();
            let start = side * 0.05 - Vec3::Y * 0.2;
            let reach = g.env.exit(start + Vec3::Y * 0.21, dir);
            let r = s.trunk_radius * g.rng.range(0.6, 1.0);
            let len = (reach * g.rng.range(0.8, 0.98)).max(0.5);
            g.grow(1, start, dir, len, r, r * 0.2);
        }
    } else {
        let lean = Vec3::new(g.rng.signed() * 0.05, 1.0, g.rng.signed() * 0.05).normalize();
        g.grow(0, Vec3::ZERO, lean, s.height * 0.97, s.trunk_radius, 0.03);
    }

    // Fill the outer crown: cards near the envelope surface, hanging from the nearest
    // branch direction-wise (outward).
    let mut rng = Rng::new(s.seed ^ 0xabcdef);
    let r = g.env.radii;
    let p = 1.6f32;
    let area = 4.0
        * PI
        * (((r.x * r.y).powf(p) + (r.x * r.z).powf(p) + (r.y * r.z).powf(p)) / 3.0).powf(1.0 / p);
    let shell = (area * s.shell) as usize;
    for _ in 0..shell {
        let d = loop {
            let v = Vec3::new(rng.signed(), rng.signed(), rng.signed());
            if v.length_squared() <= 1.0 && v.length_squared() > 0.01 {
                break v.normalize();
            }
        };
        // Fewer cards on the underside.
        if d.y < -0.4 && rng.chance(0.6) {
            continue;
        }
        let depth = rng.range(0.72, 1.0);
        let at = g.env.center + d * r * depth;
        if at.y < s.height * s.crown_base * 0.9 {
            continue;
        }
        let out = g.env.outward(at);
        let dir =
            (out + Vec3::new(rng.signed(), rng.signed() + 0.4, rng.signed()) * 0.6).normalize();
        g.spots.push(LeafSpot { at, dir });
    }
    // Leaf cards.
    let mut leaves = Mesh::default();
    for spot in &g.spots {
        let out = g.env.outward(spot.at);
        let depth = g.env.depth(spot.at).min(1.3);
        let up = (spot.dir * 0.55 + out * 0.45 + Vec3::Y * 0.25).normalize();
        let facing = (out + Vec3::new(rng.signed(), rng.signed(), rng.signed()) * 0.5).normalize();
        let size = s.card * rng.range(0.8, 1.2) * (0.85 + 0.25 * depth.min(1.0));
        // Shaded inside the crown and underneath, sunlit on the outside and on top.
        let top = smoothstep(-0.8, 0.9, (spot.at.y - g.env.center.y) / g.env.radii.y);
        let ao = (0.35 + 0.65 * smoothstep(0.25, 1.0, depth)) * (0.7 + 0.3 * top);
        let hue = rng.range(0.9, 1.1);
        let yellow = if rng.chance(0.12) { 1.18 } else { 1.0 };
        let color = Vec4::new(ao * hue * yellow, ao * hue, ao * (2.0 - hue) * 0.95, 1.0);
        add_card(
            &mut leaves,
            spot.at - up * size * 0.08,
            up,
            facing,
            out,
            size,
            size,
            0.12,
            [0.0, 0.0, 1.0, 1.0],
            color,
            0.7,
        );
    }
    let bottom = if s.bush { -0.2 } else { -0.6 };
    let top = s.height * 1.08;
    add_bounds_anchor(&mut g.wood, bottom, top);
    add_bounds_anchor(&mut leaves, bottom, top);
    TreeMeshes {
        wood: g.wood,
        leaves,
        height: s.height,
        radius: crown_radius,
    }
}

// ------------------------------------------------------------------ conifers

/// Shape of a conifer (spruce/fir).
#[derive(Clone, Copy, Debug)]
pub struct Conifer {
    pub seed: u64,
    pub height: f32,
    pub trunk_radius: f32,
    /// Lowest branches, as a fraction of the height.
    pub crown_base: f32,
    /// Longest branch as a fraction of the height.
    pub width: f32,
    /// Whorl spacing, m.
    pub whorl: f32,
    pub branches_per_whorl: usize,
    /// How far the lower branches sag before their tips turn up (spruce ≈ 0.4, fir ≈ 0.15).
    pub droop: f32,
    /// Hanging side shoots under the branches (the "comb" of a Norway spruce), 0..1.
    pub comb: f32,
}

/// Add a frond card along a path (`path[0]` is where it attaches; the texture's bottom), its
/// plane spanned by the path and `side`, tapering toward the tip.
fn add_frond(
    mesh: &mut Mesh,
    path: &[Vec3],
    side: Vec3,
    width: f32,
    normal: &dyn Fn(Vec3, f32) -> Vec3,
    color: &dyn Fn(f32) -> Vec4,
) {
    let base = mesh.vertices.len() as u32;
    let segs = path.len() - 1;
    for (k, &center) in path.iter().enumerate() {
        let t = k as f32 / segs as f32;
        let w = width * (1.0 - 0.35 * t);
        let c = color(t);
        let n = normal(center, t);
        for (u, off) in [(0.0, -0.5), (1.0, 0.5)] {
            mesh.vertices.push(
                Vertex::new(center + side * (w * off), n, Vec2::new(u, 1.0 - t)).with_color(c),
            );
        }
    }
    for k in 0..segs as u32 {
        let a = base + k * 2;
        // (a, a+1) this row; (a+2, a+3) the next one along the path.
        two_sided(mesh, &[a, a + 1, a + 3, a, a + 3, a + 2]);
    }
}

/// Points along a quadratic curve: from `start` heading `dir` for `length`, bending by
/// `bend` (m, along +Y) at the end.
fn curve(start: Vec3, dir: Vec3, length: f32, bend: f32, segs: usize) -> Vec<Vec3> {
    (0..=segs)
        .map(|k| {
            let t = k as f32 / segs as f32;
            start + dir * (length * t) + Vec3::Y * (bend * t * t)
        })
        .collect()
}

/// Grow a conifer: a straight trunk with whorls of branches; each branch is a terminal frond
/// along its sagging, tip-up curve, side fronds fanning out from it, and (spruce) shoots
/// hanging beneath. Tiers stay readable because the fronds are flat and lit from above.
pub fn conifer(style: &Conifer) -> TreeMeshes {
    let s = style;
    let mut rng = Rng::new(s.seed);
    let mut wood = Mesh::default();
    let mut leaves = Mesh::default();
    let h = s.height;
    // Trunk: straight with a slight wobble.
    let segs = 14;
    let mut pts = Vec::new();
    let mut radii = Vec::new();
    let wobble = Vec3::new(rng.signed(), 0.0, rng.signed()) * 0.08;
    for k in 0..=segs {
        let t = k as f32 / segs as f32;
        let y = -0.6 + (h + 0.6) * t;
        pts.push(Vec3::new(0.0, y, 0.0) + wobble * (t * PI).sin());
        let flare = 1.0 + 0.5 * smoothstep(0.08, 0.0, t);
        radii.push((s.trunk_radius * (1.0 - t).powf(1.1) * flare).max(0.015));
    }
    add_tube(&mut wood, &pts, &radii, 9, &|q, _| {
        let ao = 0.5 + 0.5 * smoothstep(h * 0.2, h * 0.7, q.y);
        Vec4::new(ao * 0.9, ao * 0.9, ao * 0.85, 1.0)
    });
    let base = h * s.crown_base;
    let max_len = h * s.width;
    // The crown is a cone; foliage normals lean out of it (and up, toward the sky).
    let slope = max_len / (h - base);
    let envelope = move |p: Vec3, out: Vec3| -> Vec3 {
        let radial = Vec3::new(p.x, 0.0, p.z).normalize_or(out);
        (radial + Vec3::Y * (slope + 0.35)).normalize()
    };
    // Dead stubs below the crown (seen between the lowest branches).
    let mut y = base * 0.45;
    while y < base {
        for _ in 0..3 {
            let a = rng.f32() * TAU;
            let out = Vec3::new(a.cos(), -0.15, a.sin()).normalize();
            let p0 = Vec3::new(0.0, y, 0.0) + wobble * (y / h * PI).sin();
            let len = rng.range(0.3, 0.8);
            add_tube(
                &mut wood,
                &[p0, p0 + out * len],
                &[0.025, 0.006],
                3,
                &|_, _| Vec4::new(0.45, 0.42, 0.38, 1.0),
            );
        }
        y += s.whorl * 1.6;
    }
    // Whorls of branches.
    let mut y = base;
    let mut phi = rng.f32() * TAU;
    while y < h - 0.6 {
        let t = (y - base) / (h - base);
        let len_whorl = (max_len * (1.0 - t).powf(0.9) + 0.3) * rng.range(0.88, 1.08);
        let n = s.branches_per_whorl + (rng.f32() * 2.0) as usize;
        phi += rng.range(0.3, 0.9);
        // Lower whorls are shaded by the crown above.
        let shade = 0.55 + 0.45 * smoothstep(0.0, 0.7, t);
        for k in 0..n {
            let a = phi + k as f32 / n as f32 * TAU + rng.signed() * 0.25;
            // Now and then a branch is missing (broken, shaded out): gaps in the crown.
            if rng.chance(0.1) {
                continue;
            }
            let out = Vec3::new(a.cos(), 0.0, a.sin());
            let side = out.cross(Vec3::Y).normalize();
            let len = len_whorl * rng.range(0.75, 1.08);
            let trunk = Vec3::new(0.0, y, 0.0) + wobble * (y / h * PI).sin();
            // Low branches start downward and turn up at the tip; high ones reach up.
            let pitch = -s.droop + (s.droop + 0.55) * t.powf(0.8) + rng.signed() * 0.06;
            let upturn = s.droop * 0.9 * (1.0 - t) * len;
            let dir = (out + Vec3::Y * pitch).normalize();
            let branch = curve(trunk + out * 0.04, dir, len, upturn, 6);
            let at = |f: f32| -> Vec3 {
                let x = f * 6.0;
                let i = (x as usize).min(5);
                branch[i].lerp(branch[i + 1], x - i as f32)
            };
            // A thin woody axis for the inner half (seen through the gaps).
            if len > 1.2 {
                add_tube(
                    &mut wood,
                    &[branch[0], branch[2], branch[4]],
                    &[0.035 * len.min(4.0) / 2.0, 0.02, 0.01],
                    3,
                    &|_, _| Vec4::new(0.55, 0.5, 0.45, 1.0),
                );
            }
            let tint = rng.range(0.9, 1.1);
            let color = move |from: f32, to: f32, dark: f32| {
                move |tt: f32| {
                    let f = from + (to - from) * tt;
                    let ao = (0.35 + 0.65 * smoothstep(0.0, 0.75, f)) * shade * dark;
                    Vec4::new(ao * tint, ao * tint, ao * (2.0 - tint), 1.0)
                }
            };
            let up_n = |face_w: f32| {
                move |p: Vec3, _t: f32| -> Vec3 {
                    (Vec3::Y * face_w + envelope(p, out) * (1.0 - face_w)).normalize()
                }
            };
            // Sprays are rolled about their axis (flat ones would vanish edge-on from the
            // ground), so the crown has volume from every side.
            let rolled = |axis: Vec3, roll: f32| -> Vec3 {
                let flat = axis.cross(Vec3::Y).normalize_or(side);
                let up = flat.cross(axis).normalize_or(Vec3::Y);
                flat * roll.cos() + up * roll.sin()
            };
            // The terminal frond, two crossed cards along the outer two thirds of the branch.
            let from = 0.3;
            let path: Vec<Vec3> = (0..=3)
                .map(|j| at(from + (1.0 - from) * j as f32 / 3.0))
                .collect();
            let roll = rng.range(0.5, 0.9);
            for r in [roll, -roll] {
                add_frond(
                    &mut leaves,
                    &path,
                    rolled(dir, r),
                    len * 0.6,
                    &up_n(0.45),
                    &color(from, 1.0, 1.0),
                );
            }
            // Side shoots fanning out along the branch, alternating sides.
            let m = ((len / 0.6).round() as usize).clamp(1, 6);
            for j in 0..m {
                let f = 0.1 + 0.62 * (j as f32 + rng.f32() * 0.5) / m as f32;
                let p = at(f);
                let sign = if (j + k) % 2 == 0 { 1.0 } else { -1.0 };
                let fan = rng.range(0.55, 0.95);
                let sdir =
                    (dir * fan.cos() + side * (sign * fan.sin()) - Vec3::Y * 0.12).normalize();
                let slen = (len * (0.55 - 0.3 * f)).clamp(0.4, 1.6);
                let sside = rolled(sdir, sign * rng.range(0.35, 1.2));
                let path = curve(p, sdir, slen, -s.droop * 0.35 * slen, 2);
                add_frond(
                    &mut leaves,
                    &path,
                    sside,
                    slen * 0.7,
                    &up_n(0.5),
                    &color(f, f + 0.4, 0.95),
                );
                // Spruce: shoots hanging beneath like a comb.
                if rng.f32() < s.comb * (1.0 - t) {
                    let hdir = (Vec3::Y * -1.0 + sdir * 0.35).normalize();
                    let hlen = slen * rng.range(0.6, 0.9);
                    let path = curve(p - Vec3::Y * 0.03, hdir, hlen, 0.0, 2);
                    add_frond(
                        &mut leaves,
                        &path,
                        sdir.cross(hdir).normalize_or(side),
                        hlen * 0.5,
                        &up_n(0.1),
                        &color(f, f + 0.2, 0.7),
                    );
                }
            }
        }
        y += s.whorl * rng.range(0.85, 1.15);
    }
    // Leader at the top.
    for k in 0..3 {
        let a = k as f32 / 3.0 * TAU + rng.f32();
        let side = Vec3::new(a.cos(), 0.0, a.sin());
        let start = Vec3::new(0.0, h - 1.0, 0.0) + wobble * 0.1;
        let path = curve(start, (Vec3::Y + side * 0.12).normalize(), 1.2, 0.0, 2);
        add_frond(
            &mut leaves,
            &path,
            side.cross(Vec3::Y).normalize(),
            0.45,
            &|p, _| (Vec3::new(p.x, 0.0, p.z).normalize_or(Vec3::X) + Vec3::Y).normalize(),
            &|tt| Vec4::splat(0.75 + 0.25 * tt).with_w(1.0),
        );
    }
    let top = h * 1.05;
    add_bounds_anchor(&mut wood, -0.6, top);
    add_bounds_anchor(&mut leaves, -0.6, top);
    TreeMeshes {
        wood,
        leaves,
        height: h,
        radius: max_len,
    }
}

// ------------------------------------------------------------------ impostors

/// A baked impostor view: linear RGBA (straight alpha) and normals per texel, row-major.
pub type ImpostorCell = (Vec<[f32; 4]>, Vec<[f32; 3]>);

/// What one mesh of a template looks like to the impostor baker.
pub struct BakeInput<'a> {
    pub mesh: &'a Mesh,
    pub albedo: &'a Image,
    /// Discard texels below 0.5 alpha (foliage).
    pub masked: bool,
    /// Texture tiling (the material's uv scale).
    pub uv_scale: Vec2,
}

/// Size of the world rectangle an impostor cell shows: width and height in m (bottom center
/// at the tree's base).
#[derive(Clone, Copy, Debug)]
pub struct ImpostorFrame {
    pub width: f32,
    pub height: f32,
}

/// The frame for a tree of crown radius `radius` and height `height` in a cell of aspect
/// `w / h`.
pub fn impostor_frame(radius: f32, height: f32, aspect: f32) -> ImpostorFrame {
    let height = height * 1.1;
    let width = (radius * 2.25).max(height * aspect);
    ImpostorFrame {
        width,
        height: width / aspect,
    }
}

fn sample(image: &Image, uv: Vec2) -> [f32; 4] {
    let x = ((uv.x.rem_euclid(1.0)) * image.width as f32) as u32;
    let y = ((uv.y.rem_euclid(1.0)) * image.height as f32) as u32;
    let p = image.pixel(x.min(image.width - 1), y.min(image.height - 1));
    [
        srgb8_to_linear(p[0]),
        srgb8_to_linear(p[1]),
        srgb8_to_linear(p[2]),
        p[3] as f32 / 255.0,
    ]
}

/// Render a template seen from +Z (after turning it by `yaw`), orthographic, into a
/// `w × h` cell: linear albedo + coverage, and view-space normals (x right, y up, z toward
/// the viewer) encoded as a tangent-space normal map for a card facing the viewer.
pub fn bake_impostor(
    inputs: &[BakeInput],
    frame: ImpostorFrame,
    yaw: f32,
    w: usize,
    h: usize,
) -> ImpostorCell {
    const SS: usize = 2;
    let (sw, sh) = (w * SS, h * SS);
    let mut color = vec![[0.0f32; 4]; sw * sh];
    let mut normal = vec![[0.0f32; 3]; sw * sh];
    let mut depth = vec![f32::MIN; sw * sh];
    let rot = Quat::from_rotation_y(yaw);
    let to_screen = |p: Vec3| -> Vec3 {
        let q = rot * p;
        Vec3::new(
            (q.x / frame.width + 0.5) * sw as f32,
            (1.0 - q.y / frame.height) * sh as f32,
            q.z,
        )
    };
    for input in inputs {
        let m = input.mesh;
        for tri in m.indices.as_chunks::<3>().0 {
            let v = [
                &m.vertices[tri[0] as usize],
                &m.vertices[tri[1] as usize],
                &m.vertices[tri[2] as usize],
            ];
            let s: Vec<Vec3> = v
                .iter()
                .map(|v| to_screen(Vec3::from(v.position)))
                .collect();
            let area =
                (s[1].x - s[0].x) * (s[2].y - s[0].y) - (s[2].x - s[0].x) * (s[1].y - s[0].y);
            if area.abs() < 1e-6 {
                continue;
            }
            // Facing the viewer? (screen y points down, so the sign flips.)
            let front = area < 0.0;
            let x0 = s
                .iter()
                .map(|p| p.x)
                .fold(f32::MAX, f32::min)
                .floor()
                .max(0.0) as usize;
            let x1 = (s.iter().map(|p| p.x).fold(f32::MIN, f32::max).ceil() as usize).min(sw - 1);
            let y0 = s
                .iter()
                .map(|p| p.y)
                .fold(f32::MAX, f32::min)
                .floor()
                .max(0.0) as usize;
            let y1 = (s.iter().map(|p| p.y).fold(f32::MIN, f32::max).ceil() as usize).min(sh - 1);
            if x0 > x1 || y0 > y1 {
                continue;
            }
            for py in y0..=y1 {
                for px in x0..=x1 {
                    let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
                    let w0 = ((s[1].x - fx) * (s[2].y - fy) - (s[2].x - fx) * (s[1].y - fy)) / area;
                    let w1 = ((s[2].x - fx) * (s[0].y - fy) - (s[0].x - fx) * (s[2].y - fy)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let z = s[0].z * w0 + s[1].z * w1 + s[2].z * w2;
                    let i = py * sw + px;
                    if z <= depth[i] {
                        continue;
                    }
                    let lerp2 = |a: [f32; 2], b: [f32; 2], c: [f32; 2]| {
                        Vec2::new(
                            a[0] * w0 + b[0] * w1 + c[0] * w2,
                            a[1] * w0 + b[1] * w1 + c[1] * w2,
                        )
                    };
                    let uv = lerp2(v[0].uv, v[1].uv, v[2].uv) * input.uv_scale;
                    let tex = sample(input.albedo, uv);
                    if input.masked && tex[3] < 0.5 {
                        continue;
                    }
                    let col = Vec4::from(v[0].color) * w0
                        + Vec4::from(v[1].color) * w1
                        + Vec4::from(v[2].color) * w2;
                    let mut n = (Vec3::from(v[0].normal) * w0
                        + Vec3::from(v[1].normal) * w1
                        + Vec3::from(v[2].normal) * w2)
                        .normalize_or(Vec3::Z);
                    n = rot * n;
                    if !front && !input.masked {
                        continue;
                    }
                    if n.z < 0.0 {
                        // Seen from behind: keep it facing the viewer (no back lighting).
                        n.z = -n.z * 0.3;
                        n = n.normalize();
                    }
                    depth[i] = z;
                    color[i] = [tex[0] * col.x, tex[1] * col.y, tex[2] * col.z, 1.0];
                    normal[i] = n.to_array();
                }
            }
        }
    }
    // Downsample.
    let mut out_c = vec![[0.0f32; 4]; w * h];
    let mut out_n = vec![[0.0f32, 0.0, 1.0]; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut c = [0.0f32; 4];
            let mut n = Vec3::ZERO;
            for sy in 0..SS {
                for sx in 0..SS {
                    let i = (y * SS + sy) * sw + x * SS + sx;
                    if color[i][3] > 0.0 {
                        for k in 0..3 {
                            c[k] += color[i][k];
                        }
                        c[3] += 1.0;
                        n += Vec3::from(normal[i]);
                    }
                }
            }
            if c[3] > 0.0 {
                out_c[y * w + x] = [
                    c[0] / c[3],
                    c[1] / c[3],
                    c[2] / c[3],
                    c[3] / (SS * SS) as f32,
                ];
                out_n[y * w + x] = n.normalize_or(Vec3::Z).to_array();
            }
        }
    }
    (out_c, out_n)
}

/// Pack impostor cells side by side into one atlas texture set.
pub fn impostor_atlas(cells: &[ImpostorCell], w: usize, h: usize) -> TextureSet {
    let aw = w * cells.len();
    let mut canvas = Canvas::new(aw, h, false);
    let mut normals = vec![[0.5f32, 0.5, 1.0]; aw * h];
    for (k, (c, n)) in cells.iter().enumerate() {
        for y in 0..h {
            for x in 0..w {
                let i = y * aw + k * w + x;
                canvas.color[i] = c[y * w + x];
                // Alpha-tested at 0.5: sharpen the coverage a little.
                canvas.color[i][3] = smoothstep(0.15, 0.65, c[y * w + x][3]);
                normals[i] = n[y * w + x];
                canvas.rough[i] = 0.8;
                canvas.ao[i] = 1.0;
            }
        }
    }
    let mut set = canvas.finish(1.0, Some(0.5));
    let mut data = Vec::with_capacity(aw * h * 4);
    for n in &normals {
        data.extend_from_slice(&[
            ((n[0] * 0.5 + 0.5) * 255.0).round() as u8,
            ((n[1] * 0.5 + 0.5) * 255.0).round() as u8,
            ((n[2] * 0.5 + 0.5) * 255.0).round() as u8,
            255,
        ]);
    }
    set.normal = Image::new(aw as u32, h as u32, data, false);
    set
}

/// A camera-facing card for a distant tree: a quad standing at `base` (ground), facing
/// `toward` (horizontal), showing atlas cell `cell` of `cells`.
#[allow(clippy::too_many_arguments)]
pub fn add_billboard(
    mesh: &mut Mesh,
    base: Vec3,
    toward: Vec3,
    frame: ImpostorFrame,
    scale: f32,
    cell: usize,
    cells: usize,
    mirror: bool,
    tint: Vec4,
) {
    let f = Vec3::new(toward.x, 0.0, toward.z).normalize_or(Vec3::Z);
    let right = Vec3::Y.cross(f).normalize();
    let (w, h) = (frame.width * scale, frame.height * scale);
    let (mut u0, mut u1) = (cell as f32 / cells as f32, (cell + 1) as f32 / cells as f32);
    if mirror {
        std::mem::swap(&mut u0, &mut u1);
    }
    let i = mesh.vertices.len() as u32;
    let sink = Vec3::Y * (h * 0.012);
    for (p, uv) in [
        (base - right * (w * 0.5) - sink, Vec2::new(u0, 1.0)),
        (base + right * (w * 0.5) - sink, Vec2::new(u1, 1.0)),
        (
            base + right * (w * 0.5) + Vec3::Y * h - sink,
            Vec2::new(u1, 0.0),
        ),
        (
            base - right * (w * 0.5) + Vec3::Y * h - sink,
            Vec2::new(u0, 0.0),
        ),
    ] {
        mesh.vertices.push(Vertex::new(p, f, uv).with_color(tint));
    }
    mesh.indices
        .extend_from_slice(&[i, i + 1, i + 2, i, i + 2, i + 3]);
}
