//! Procedural plant meshes and textures.
//!
//! Every plant is built from a few primitives: tapered tubes along curved paths (trunks,
//! branches, stems), two-sided leaf cards (alpha-masked sprays of leaves or needles), blades
//! (grass) and lathed caps (mushrooms). Colors are baked into vertex colors (bark, cut wood,
//! ambient occlusion inside a crown) so one material per part covers every plant of a species.
//!
//! Tubes carry texture coordinates for the bark atlas ([`bark::layout`]): around a tube the
//! bark tile repeats a whole number of times at its real size, along it the tile keeps its
//! aspect, and cut caps map onto the atlas' end-grain disc. Bark textures ([`bark`]) then
//! multiply the vertex colors, which act as tints (moss, a dark foot, an orange crown).
//!
//! Leaf cards use "spherical" normals: each card is shaded as if it were part of the crown's
//! rounded surface, which gives soft, volumetric canopies instead of a mess of flat planes.

pub mod bark;
pub mod fungi;
pub mod small;
pub mod small_textures;
pub mod textures;
pub mod tree;

use engine_core::math::{Vec2, Vec3, Vec4};
use engine_render::{Mesh, Vertex};

use bark::layout;

/// Tubes that start this close to the ground (model space, meters) rise from it: trunks,
/// stumps, shrub stems.
const GROUND_START_M: f32 = 0.1;
/// Highest felling cut (`tree::cut_height` stays below 0.75 m). A tube rising from the ground
/// takes its bark scale from its last ring at or below this height (the cut, for a stump), and
/// a tube starting above the ground from its first ring (the cut, for the wood above it), so
/// the stump and the trunk above it meet without a seam in the bark.
const CUT_MAX_M: f32 = 0.8;

/// How a tube's sides map into the bark atlas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TubeUv {
    /// How many times the bark tile repeats around the tube.
    pub repeats: u32,
    /// Radius the scale was taken at, meters.
    pub ref_radius: f32,
    /// Ring the scale was taken at; rings past it (thinner ones) shrink the tile along the tube
    /// as much as around it, so bark never stretches on tapering branches.
    pub ref_ring: usize,
    /// Length of one tile along the tube at the reference radius, meters.
    pub tile_length_m: f32,
}

impl TubeUv {
    /// The mapping for a tube along `path` with `radius` per point (model space, Y up).
    pub fn new(path: &[Vec3], radius: &[f32]) -> Self {
        let ref_ring = if path[0].y <= GROUND_START_M {
            path.iter()
                .position(|p| p.y > CUT_MAX_M)
                .map_or(path.len() - 1, |i| i.saturating_sub(1))
        } else {
            0
        };
        let ref_radius = radius[ref_ring].max(1e-3);
        let circumference = std::f32::consts::TAU * ref_radius;
        let repeats = ((circumference / layout::TILE_WIDTH_M).round() as u32).clamp(1, 32);
        Self {
            repeats,
            ref_radius,
            ref_ring,
            tile_length_m: circumference / repeats as f32 * layout::TILE_ASPECT,
        }
    }

    /// Texture v of every ring. v falls along the tube (the image's top is up the trunk) and
    /// starts at the tube's height, so tubes that continue each other (a stump and the trunk
    /// above it) line up.
    pub fn v_along(&self, path: &[Vec3], radius: &[f32]) -> Vec<f32> {
        let mut v = -path[0].y / self.tile_length_m;
        let mut out = Vec::with_capacity(path.len());
        out.push(v);
        for i in 1..path.len() {
            let r = 0.5 * (radius[i] + radius[i - 1]);
            let shrink = if i > self.ref_ring {
                (r / self.ref_radius).clamp(0.05, 1.0)
            } else {
                1.0
            };
            v -= path[i].distance(path[i - 1]) / (self.tile_length_m * shrink);
            out.push(v);
        }
        out
    }

    /// Sides to build for a tube asked to have `requested`: thick, fully detailed tubes
    /// (trunks; 8 sides or more) get about one side per 12 cm of circumference, up to 28, so
    /// they stay round up close. Simplified levels of detail and limbs keep what they asked.
    pub fn sides(&self, requested: u32) -> u32 {
        if requested < 8 {
            return requested;
        }
        let wanted = (std::f32::consts::TAU * self.ref_radius / 0.12).round() as u32;
        requested.max(wanted.min(28))
    }

    /// Relative radius change at `around` (0..1) and `s` meters along the tube's texture
    /// (consistent between tubes that continue each other): trunks are not perfect circles.
    /// Zero for thin tubes.
    pub fn bulge(&self, around: f32, s: f32) -> f32 {
        let amount = 0.045 * engine_core::math::smoothstep(0.05, 0.2, self.ref_radius);
        if amount <= 0.0 {
            return 0.0;
        }
        let seed = self.ref_radius.to_bits() as u64;
        let (x, y) = (around * 7.0, s / 0.7);
        amount
            * (0.7 * bark::periodic_noise(x, y, 7, 1 << 20, seed)
                + 0.3 * bark::periodic_noise(x * 2.0, y * 2.0, 14, 1 << 21, seed + 1))
    }

    /// Columns around the tube for `sides` sides: `(around 0..1, u)`. Where the tile repeats,
    /// a column is doubled (the end of one tile, the start of the next) so the texture wraps
    /// within the atlas' bark half.
    pub fn columns(&self, sides: u32) -> Vec<(f32, f32)> {
        let n = self.repeats as f32;
        let mut around: Vec<f32> = (0..=sides).map(|k| k as f32 / sides as f32).collect();
        around.extend((1..self.repeats).map(|j| j as f32 / n));
        around.sort_by(f32::total_cmp);
        around.dedup_by(|a, b| (*a - *b).abs() < 1e-5);
        let span = layout::BARK_U1 - layout::BARK_U0;
        let mut out = Vec::with_capacity(around.len() + self.repeats as usize);
        for &a in &around {
            let t = a * n;
            if a < 1e-5 {
                out.push((0.0, layout::BARK_U0));
            } else if a > 1.0 - 1e-5 {
                out.push((1.0, layout::BARK_U1));
            } else if (t - t.round()).abs() < 1e-4 {
                out.push((a, layout::BARK_U1));
                out.push((a, layout::BARK_U0));
            } else {
                out.push((a, layout::BARK_U0 + t.fract() * span));
            }
        }
        out
    }
}

/// Collects vertices and triangles.
#[derive(Default)]
pub struct MeshBuilder {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl MeshBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn vertex(&mut self, p: Vec3, n: Vec3, uv: Vec2, color: Vec4) -> u32 {
        self.vertices
            .push(Vertex::new(p, n.normalize_or(Vec3::Y), uv).with_color(color));
        self.vertices.len() as u32 - 1
    }

    pub fn tri(&mut self, a: u32, b: u32, c: u32) {
        self.indices.extend_from_slice(&[a, b, c]);
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// A tube along `path` with a radius per point. `color(i, around)` gives the vertex color of
    /// ring `i` (around = 0..1). Caps close the start and/or end with a cut face.
    ///
    /// Texture coordinates follow the bark atlas ([`TubeUv`], [`bark::layout`]), so bark keeps
    /// its real size on thick trunks and thin twigs alike. `_v_scale` is no longer used (the
    /// atlas sets the scale) and stays only so existing callers compile. Thick trunks get more
    /// than `sides` sides up close ([`TubeUv::sides`]) and gentle lumps ([`TubeUv::bulge`]), so
    /// they read as wood rather than pipes.
    #[allow(clippy::too_many_arguments)]
    pub fn tube(
        &mut self,
        path: &[Vec3],
        radius: &[f32],
        sides: u32,
        _v_scale: f32,
        color: &dyn Fn(usize, f32) -> Vec4,
        cap_start: Option<&dyn Fn(f32) -> Vec4>,
        cap_end: Option<&dyn Fn(f32) -> Vec4>,
    ) {
        if path.len() < 2 {
            return;
        }
        let frames = frames(path);
        let map = TubeUv::new(path, radius);
        let columns = map.columns(map.sides(sides.max(3)));
        let vs = map.v_along(path, radius);
        let first = self.vertices.len() as u32;
        let along = |i: usize| vs[i] * map.tile_length_m;
        for (i, &p) in path.iter().enumerate() {
            let (t, n, b) = frames[i];
            for &(around, u) in &columns {
                let a = around * std::f32::consts::TAU;
                let dir = n * a.cos() + b * a.sin();
                let side = b * a.cos() - n * a.sin();
                // Lumps bend the normal: around (per radian) and along the tube (per meter).
                let d = map.bulge(around, along(i));
                let e = 1e-3;
                let d_around = (map.bulge(around + e, along(i)) - map.bulge(around - e, along(i)))
                    / (2.0 * e * std::f32::consts::TAU);
                let d_along =
                    (map.bulge(around, along(i) + e) - map.bulge(around, along(i) - e)) / (2.0 * e);
                let normal = dir - side * (d_around / (1.0 + d)) - t * (radius[i] * d_along);
                self.vertex(
                    p + dir * radius[i] * (1.0 + d),
                    normal,
                    Vec2::new(u, vs[i]),
                    color(i, around),
                );
            }
        }
        let ring = columns.len() as u32;
        for i in 0..path.len() as u32 - 1 {
            for k in 0..ring - 1 {
                // A doubled seam column encloses no surface.
                if columns[k as usize + 1].0 - columns[k as usize].0 < 1e-6 {
                    continue;
                }
                let a = first + i * ring + k;
                let b = a + 1;
                let c = a + ring + 1;
                let d = a + ring;
                self.tri(a, b, c);
                self.tri(a, c, d);
            }
        }
        // Caps share the tube's columns, so their rims meet its sides without gaps.
        let mut around: Vec<f32> = columns.iter().map(|c| c.0).collect();
        around.dedup();
        around.pop();
        let rim = |i: usize| -> Vec<f32> {
            around
                .iter()
                .map(|&a| radius[i] * (1.0 + map.bulge(a, along(i))))
                .collect()
        };
        if let Some(cap) = cap_start {
            let (t, n, b) = frames[0];
            self.disc_at(path[0], -t, n, b, &around, &rim(0), cap, true);
        }
        if let Some(cap) = cap_end {
            let last = path.len() - 1;
            let (t, n, b) = frames[last];
            self.disc_at(path[last], t, n, b, &around, &rim(last), cap, false);
        }
    }

    /// A flat disc made of concentric rings (so a cut trunk can show growth rings).
    /// `color(r)` gives the color at relative radius r (0 center .. 1 rim). Texture
    /// coordinates map it onto the bark atlas' end-grain disc ([`bark::layout::CUT_CENTER`]).
    #[allow(clippy::too_many_arguments)]
    pub fn disc(
        &mut self,
        center: Vec3,
        normal: Vec3,
        n: Vec3,
        b: Vec3,
        radius: f32,
        sides: u32,
        color: &dyn Fn(f32) -> Vec4,
        flip: bool,
    ) {
        let sides = sides.max(3);
        let around: Vec<f32> = (0..sides).map(|k| k as f32 / sides as f32).collect();
        let rim = vec![radius; around.len()];
        self.disc_at(center, normal, n, b, &around, &rim, color, flip);
    }

    /// [`Self::disc`] with rim vertices at the given fractions of a turn (ascending, 0..1),
    /// each at its own radius.
    #[allow(clippy::too_many_arguments)]
    fn disc_at(
        &mut self,
        center: Vec3,
        normal: Vec3,
        n: Vec3,
        b: Vec3,
        around: &[f32],
        rim: &[f32],
        color: &dyn Fn(f32) -> Vec4,
        flip: bool,
    ) {
        const RINGS: [f32; 6] = [0.0, 0.18, 0.4, 0.62, 0.86, 1.0];
        let [cu, cv] = layout::CUT_CENTER;
        let [ru, rv] = layout::CUT_RADIUS;
        let sides = around.len() as u32;
        let c = self.vertex(center, normal, Vec2::new(cu, cv), color(0.0));
        let mut prev: Vec<u32> = Vec::new();
        for (ri, &r) in RINGS.iter().enumerate().skip(1) {
            let ring: Vec<u32> = around
                .iter()
                .zip(rim)
                .map(|(&f, &radius)| {
                    let a = f * std::f32::consts::TAU;
                    let dir = n * a.cos() + b * a.sin();
                    // Slightly uneven inner rings look sawn rather than machined; the rim
                    // stays on the tube's sides.
                    let wobble = 1.0 + 0.04 * ((a * 3.0 + ri as f32).sin());
                    let rel = if r < 1.0 {
                        r * wobble.min(1.0 / r.max(0.01))
                    } else {
                        1.0
                    };
                    self.vertex(
                        center + dir * radius * rel,
                        normal,
                        Vec2::new(cu + ru * a.cos() * rel, cv + rv * a.sin() * rel),
                        color(r),
                    )
                })
                .collect();
            for k in 0..sides as usize {
                let k1 = (k + 1) % sides as usize;
                if prev.is_empty() {
                    if flip {
                        self.tri(c, ring[k1], ring[k]);
                    } else {
                        self.tri(c, ring[k], ring[k1]);
                    }
                } else if flip {
                    self.tri(prev[k], ring[k1], ring[k]);
                    self.tri(prev[k], prev[k1], ring[k1]);
                } else {
                    self.tri(prev[k], ring[k], ring[k1]);
                    self.tri(prev[k], ring[k1], prev[k1]);
                }
            }
            prev = ring;
        }
    }

    /// A two-sided card: `corners` counter-clockwise seen from the front. Both faces use
    /// `normal` (the crown's normal, not the card's), so cards shade like a rounded crown.
    pub fn card(&mut self, corners: [Vec3; 4], uvs: [Vec2; 4], normal: Vec3, colors: [Vec4; 4]) {
        let base = self.vertices.len() as u32;
        for i in 0..4 {
            self.vertex(corners[i], normal, uvs[i], colors[i]);
        }
        self.tri(base, base + 1, base + 2);
        self.tri(base, base + 2, base + 3);
        self.tri(base, base + 2, base + 1);
        self.tri(base, base + 3, base + 2);
    }

    /// A two-sided strip along a path (fern fronds, grass blades, leaves): `left`/`right`
    /// edge points per row, `normals` per row, `v` per row.
    pub fn strip(
        &mut self,
        left: &[Vec3],
        right: &[Vec3],
        normals: &[Vec3],
        v: &[f32],
        colors: &[Vec4],
    ) {
        let base = self.vertices.len() as u32;
        for i in 0..left.len() {
            self.vertex(left[i], normals[i], Vec2::new(0.0, v[i]), colors[i]);
            self.vertex(right[i], normals[i], Vec2::new(1.0, v[i]), colors[i]);
        }
        for i in 0..left.len() as u32 - 1 {
            let a = base + i * 2;
            let (b, c, d) = (a + 1, a + 3, a + 2);
            // Front and back.
            self.tri(a, b, c);
            self.tri(a, c, d);
            self.tri(a, c, b);
            self.tri(a, d, c);
        }
    }

    pub fn finish(self) -> Mesh {
        Mesh::new(self.vertices, self.indices)
    }
}

/// Parallel-transport frames (tangent, normal, binormal) along a path: tubes don't twist.
pub fn frames(path: &[Vec3]) -> Vec<(Vec3, Vec3, Vec3)> {
    let n = path.len();
    let tangent = |i: usize| {
        let a = path[i.saturating_sub(1)];
        let b = path[(i + 1).min(n - 1)];
        (b - a).normalize_or(Vec3::Y)
    };
    let mut out = Vec::with_capacity(n);
    let t0 = tangent(0);
    let mut normal = t0.any_orthonormal_vector();
    for i in 0..n {
        let t = tangent(i);
        normal = (normal - t * normal.dot(t)).normalize_or(t.any_orthonormal_vector());
        let b = t.cross(normal);
        out.push((t, normal, b));
    }
    out
}

/// Quadratic Bézier point.
pub fn bezier(a: Vec3, b: Vec3, c: Vec3, t: f32) -> Vec3 {
    let u = 1.0 - t;
    a * (u * u) + b * (2.0 * u * t) + c * (t * t)
}

/// Color as a vertex color.
pub fn rgb(c: [f32; 3], scale: f32) -> Vec4 {
    Vec4::new(c[0] * scale, c[1] * scale, c[2] * scale, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Front faces wind counter-clockwise: each triangle's geometric normal agrees with the
    /// vertex normals (for tubes) on the outside.
    #[test]
    fn tube_faces_outward() {
        let mut b = MeshBuilder::new();
        let path = [
            Vec3::ZERO,
            Vec3::new(0.1, 1.0, 0.0),
            Vec3::new(0.3, 2.0, 0.1),
        ];
        b.tube(
            &path,
            &[0.3, 0.2, 0.1],
            8,
            1.0,
            &|_, _| Vec4::ONE,
            Some(&|_| Vec4::ONE),
            Some(&|_| Vec4::ONE),
        );
        let m = b.finish();
        for tri in m.indices.as_chunks::<3>().0 {
            let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| m.vertices[i as usize]);
            let pa = Vec3::from(a.position);
            let geo = (Vec3::from(b.position) - pa).cross(Vec3::from(c.position) - pa);
            if geo.length() < 1e-8 {
                continue;
            }
            let n = Vec3::from(a.normal) + Vec3::from(b.normal) + Vec3::from(c.normal);
            assert!(geo.dot(n) > 0.0, "inward-facing triangle {tri:?}");
        }
    }

    /// A trunk like the tree generator's: the ground, half the cut, the cut, then up.
    fn trunk(cut: f32) -> (Vec<Vec3>, Vec<f32>) {
        let mut ys = vec![0.0, cut * 0.5, cut];
        ys.extend((1..=30).map(|i| cut + i as f32 * 0.55));
        let path: Vec<Vec3> = ys
            .iter()
            .map(|&y| Vec3::new(0.01 * (y * 0.7).sin(), y, 0.008 * y))
            .collect();
        let top = *ys.last().unwrap();
        let radius = ys
            .iter()
            .map(|&y| 0.42 * (0.1 + 0.9 * (1.0 - y / top)) + 0.21 * (-y / 0.45).exp())
            .collect();
        (path, radius)
    }

    /// The stump, the trunk above the cut and the whole trunk agree on the bark's layout, so
    /// a standing tree shows no seam at the cut.
    #[test]
    fn stump_and_trunk_bark_line_up_at_the_cut() {
        for cut in [0.35, 0.5, 0.56, 0.75] {
            let (path, radius) = trunk(cut);
            let stump = TubeUv::new(&path[..=2], &radius[..=2]);
            let wood = TubeUv::new(&path[2..], &radius[2..]);
            let whole = TubeUv::new(&path, &radius);
            assert_eq!(stump.repeats, wood.repeats, "cut {cut}");
            assert_eq!(whole.repeats, wood.repeats, "cut {cut}");
            assert!((stump.tile_length_m - wood.tile_length_m).abs() < 1e-5);
            let v_stump = stump.v_along(&path[..=2], &radius[..=2]);
            let v_wood = wood.v_along(&path[2..], &radius[2..]);
            let v_whole = whole.v_along(&path, &radius);
            assert!((v_stump[2] - v_wood[0]).abs() < 0.01, "cut {cut}");
            for (i, v) in v_wood.iter().enumerate() {
                assert!((v - v_whole[i + 2]).abs() < 0.01, "ring {i}");
            }
            // v falls up the trunk (the image's top is up).
            assert!(v_whole.windows(2).all(|w| w[1] < w[0]));
            // An oak-sized trunk repeats the half-meter tile several times around.
            assert!((4..=8).contains(&wood.repeats), "{}", wood.repeats);
        }
    }

    /// Columns stay inside the atlas' bark tile, doubling at each repeat.
    #[test]
    fn tube_columns_wrap_inside_the_bark_tile() {
        for (repeats, sides) in [(1, 6), (3, 12), (6, 12), (5, 10), (4, 3)] {
            let map = TubeUv {
                repeats,
                ref_radius: 0.3,
                ref_ring: 0,
                tile_length_m: 0.5,
            };
            let cols = map.columns(sides);
            assert_eq!(cols[0], (0.0, layout::BARK_U0));
            assert_eq!(*cols.last().unwrap(), (1.0, layout::BARK_U1));
            assert!(
                cols.iter()
                    .all(|c| (layout::BARK_U0..=layout::BARK_U1).contains(&c.1))
            );
            assert!(cols.windows(2).all(|w| w[1].0 >= w[0].0));
            let doubled = cols.windows(2).filter(|w| w[1].0 == w[0].0).count();
            assert_eq!(doubled as u32, repeats - 1, "{repeats} × {sides}");
            // Within a tile, u grows with the angle.
            for w in cols.windows(2).filter(|w| w[1].0 > w[0].0) {
                assert!(w[1].1 > w[0].1, "{w:?}");
            }
        }
    }

    #[test]
    fn frames_stay_orthonormal() {
        let path: Vec<Vec3> = (0..10)
            .map(|i| {
                Vec3::new(
                    (i as f32 * 0.7).sin(),
                    i as f32 * 0.5,
                    (i as f32 * 0.3).cos(),
                )
            })
            .collect();
        for (t, n, b) in frames(&path) {
            assert!(t.dot(n).abs() < 1e-4 && t.dot(b).abs() < 1e-4 && n.dot(b).abs() < 1e-4);
            assert!((n.length() - 1.0).abs() < 1e-4);
        }
    }
}
