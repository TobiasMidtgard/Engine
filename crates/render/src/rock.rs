//! Procedural rocks: faceted, weathered boulders and stones, different for every seed.
//!
//! A real rock is a fractured block worn down by weather: flat break faces meeting at rounded
//! edges, pitted and layered surfaces, and a flat-ish base where it rests. The generator builds
//! that shape directly on a finely subdivided icosphere:
//!
//! 1. the intersection of 9–14 random half-spaces (the break faces), with the edges between
//!    them rounded by a soft minimum;
//! 2. erosion: a few octaves of gradient noise, ridged at the coarse scale, plus faint
//!    sedimentary layers along a random axis;
//! 3. a softly flattened base;
//! 4. cavity darkening baked into the vertex color (dirt and shadow in cracks).
//!
//! UVs are a box projection in meters by the dominant normal axis; materials that should flow
//! seamlessly over the rock use world-space (triplanar) mapping.

use std::collections::HashMap;

use engine_core::math::{Vec2, Vec3, Vec4};
use serde::{Deserialize, Serialize};

use crate::mesh::{Mesh, Vertex};

/// Rock mesh `size` meters across (x, y, z), centered on the origin, resting on y = -size.y / 2.
/// `detail` is the icosphere subdivision level (1..=7, about 20·4^detail triangles); 0 picks
/// one from the size so facets stay well under a centimeter on boulders.
pub fn rock(size: Vec3, seed: u32, detail: u32) -> Mesh {
    let detail = if detail == 0 {
        let span = size.max_element();
        if span <= 0.3 {
            4
        } else if span <= 2.0 {
            5
        } else {
            6
        }
    } else {
        detail.clamp(1, 7)
    };
    let mut rng = Rng::new(seed);
    let (dirs, indices) = icosphere(detail);

    // Break faces: random planes, unit normal and distance from the center. Large faces cut
    // deep; small chips near the surface knock off edges and corners.
    let mut planes: Vec<(Vec3, f32)> = (0..rng.range(8.0, 12.99) as usize)
        .map(|_| (rng.unit_vector(), rng.range(0.55, 0.85)))
        .collect();
    planes.extend(
        (0..rng.range(6.0, 12.99) as usize).map(|_| (rng.unit_vector(), rng.range(0.8, 0.95))),
    );
    let strata = rng.unit_vector().lerp(Vec3::Y, 0.6).normalize();
    let strata_phase = rng.range(0.0, 100.0);
    let noise_offset = Vec3::new(
        rng.range(-500.0, 500.0),
        rng.range(-500.0, 500.0),
        rng.range(-500.0, 500.0),
    );
    // Relative scale of erosion detail: larger rocks show more fine structure per meter.
    let roughness = rng.range(0.8, 1.2);

    let mut vertices: Vec<Vertex> = dirs
        .iter()
        .map(|&d| {
            // 1. Convex break faces with rounded edges (soft minimum of the plane distances).
            let k = 0.012;
            let mut sum = 0.0;
            for &(n, offset) in &planes {
                let c = d.dot(n);
                if c > 1e-3 {
                    sum += (-(offset / c) / k).exp();
                }
            }
            let mut r = if sum > 0.0 {
                (-k * sum.ln()).min(1.0)
            } else {
                1.0
            };
            // Blend toward a sphere a little so no face is perfectly flat.
            r = r.clamp(0.35, 1.0) * 0.96 + 0.04;

            // 2. Erosion: ridged coarse noise, finer pitting, faint layers.
            let p = d * 2.2 + noise_offset;
            let coarse = 1.0 - (gradient_noise(p) * 2.0).abs();
            let mut fine = 0.0;
            let mut amp = 0.5;
            let mut q = p * 4.0;
            for _ in 0..4 {
                fine += gradient_noise(q) * amp;
                q = q * 2.13 + Vec3::new(3.1, 1.7, 5.3);
                amp *= 0.5;
            }
            let layers = (d.dot(strata) * 45.0 + strata_phase + gradient_noise(p * 1.5) * 4.0)
                .sin()
                * 0.0015;
            r += (coarse - 0.5) * 0.035 * roughness + fine * 0.03 * roughness + layers;

            let mut pos = d * r;
            // 3. A flattened base with a rounded rim (smooth maximum with the floor plane).
            let (floor, k) = (-0.7, 0.12);
            let h = (0.5 + 0.5 * (pos.y - floor) / k).clamp(0.0, 1.0);
            pos.y = floor + (pos.y - floor) * h + k * h * (1.0 - h);
            Vertex::new(pos, d, Vec2::ZERO)
        })
        .collect();

    // Fit the bounds to `size` exactly (the shape is irregular, so scale per axis).
    let (min, max) = vertices.iter().fold(
        (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
        |(lo, hi), v| {
            let p = Vec3::from(v.position);
            (lo.min(p), hi.max(p))
        },
    );
    let center = (min + max) * 0.5;
    let scale = size / (max - min).max(Vec3::splat(1e-4));
    for v in &mut vertices {
        v.position = ((Vec3::from(v.position) - center) * scale).to_array();
    }
    let mut mesh = Mesh::new(vertices, indices);
    mesh.compute_normals();

    // 4. Cavity: how far each vertex sits below its neighbors' average, along its normal.
    let mut neighbor_sum = vec![Vec3::ZERO; mesh.vertices.len()];
    let mut neighbor_count = vec![0.0f32; mesh.vertices.len()];
    for tri in mesh.indices.as_chunks::<3>().0 {
        for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            let (a, b) = (a as usize, b as usize);
            neighbor_sum[a] += Vec3::from(mesh.vertices[b].position);
            neighbor_count[a] += 1.0;
            neighbor_sum[b] += Vec3::from(mesh.vertices[a].position);
            neighbor_count[b] += 1.0;
        }
    }
    // About one edge length of the subdivided mesh.
    let edge = 0.5 * size.max_element() / 2f32.powi(detail as i32);
    for (i, v) in mesh.vertices.iter_mut().enumerate() {
        let p = Vec3::from(v.position);
        let n = Vec3::from(v.normal);
        let avg = neighbor_sum[i] / neighbor_count[i].max(1.0);
        let cavity = ((avg - p).dot(n) * 6.0 / edge).clamp(0.0, 1.0);
        let shade = 1.0 - 0.35 * cavity;
        v.color = Vec4::new(shade, shade, shade, 1.0).to_array();
        // Box projection in meters by the dominant normal axis.
        let a = n.abs();
        v.uv = if a.y >= a.x && a.y >= a.z {
            Vec2::new(p.x, p.z)
        } else if a.x >= a.z {
            Vec2::new(p.z, p.y)
        } else {
            Vec2::new(p.x, p.y)
        }
        .to_array();
    }
    mesh
}

// ---------------------------------------------------------------------------------------------
// Building stones for dry-stone walls.

/// How a building stone was split and dressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum StoneKind {
    /// Split with a hammer and set: broad, rippled fracture faces with flake scars, crisp
    /// broken arrises, chipped corners.
    #[default]
    SplitFace,
    /// Roughly squared with a pick and a punch: pick marks and short punch furrows on every
    /// face, worn and rounded arrises.
    RoughHewn,
    /// A bedded slab (coping, through-stones, flags): flat beds, edges that show their
    /// laminations, flaking.
    Slab,
}

/// The surface of a building stone below a couple of centimeters, as a displacement height
/// ([`DisplacementSource::Stone`](crate::virtual_geometry::DisplacementSource)): fracture
/// ripples and flake scars, pick marks and punch furrows, laminations, grain. Also paints
/// lichen and grime into the vertex colors at the tessellation's resolution.
///
/// It works in the mesh's own space and needs no UVs, so it can displace one stone or many
/// stones merged into one mesh.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoneFinish {
    pub kind: StoneKind,
    pub seed: u32,
    /// Size of the marks relative to the usual (1: pick marks about 1.4 cm apart).
    pub scale: f32,
    /// Lichen cover, 0..1 (crusts on faces open to the sky).
    pub lichen: f32,
    /// Dirt in pits and cracks, 0..1.
    pub grime: f32,
}

impl Default for StoneFinish {
    fn default() -> Self {
        Self {
            kind: StoneKind::SplitFace,
            seed: 0,
            scale: 1.0,
            lichen: 0.3,
            grime: 0.5,
        }
    }
}

impl StoneFinish {
    pub fn new(kind: StoneKind, seed: u32) -> Self {
        Self {
            kind,
            seed,
            ..Default::default()
        }
    }

    /// Identifies the finish for the virtual geometry build cache (and changes whenever the
    /// finish's code makes different stones, so builds of older stones aren't reused).
    pub fn key(&self) -> u64 {
        const FINISH_VERSION: u32 = 2;
        let mut h = crate::virtual_geometry::StableHasher::new(0x0057_0ef1);
        h.write_u32(FINISH_VERSION);
        h.write_u32(self.kind as u32);
        h.write_u32(self.seed);
        for v in [self.scale, self.lichen, self.grime] {
            h.write_u32(v.to_bits());
        }
        h.finish()
    }

    /// Relief of one face's projection (axis `k`, facing `side`) at `uv` (scaled meters),
    /// about -1..1.
    fn face_relief(&self, k: usize, side: usize, uv: Vec2, q: Vec3) -> f32 {
        let face = hash3([self.seed as i32, k as i32, side as i32]);
        let z = (face & 0xffff) as f32 * 0.013;
        let angle = (face >> 16) as f32 / 65535.0 * std::f32::consts::PI;
        match self.kind {
            StoneKind::SplitFace => {
                // Beds and joints split flatter than the face.
                let flat = if k == 1 { 0.5 } else { 1.0 };
                // Conchoidal scars a few centimeters across meeting in straight, sharp ridges,
                // chips knocked off them, and a gentle swell over the whole face.
                let (scar, inside) = facets(uv, 0.055, 0.6, face ^ 0x51ab);
                let (chip, _) = facets(uv, 0.018, 0.7, face ^ 0x22c7);
                // Sand grains and the little steps between them.
                let grain = noise2(uv * (1.0 / 0.005), z + 11.0);
                let broad = fbm2(uv * (1.0 / 0.16), z, 2);
                // Hackle: faint ripples across each scar, fading out at its ridges.
                let ripple = (uv.dot(Vec2::from_angle(angle)) * (1.0 / 0.0055)
                    + 2.5 * noise2(uv * 25.0, z + 3.0))
                .sin()
                    * smoothstep(0.0, 0.12, inside);
                flat * (0.35 * broad + 1.6 * (scar - 0.05) + 0.5 * (chip - 0.05))
                    + 0.07 * grain
                    + if k == 1 { 0.0 } else { 0.03 * ripple }
            }
            StoneKind::RoughHewn => {
                // The mason's working planes, then dense pick gouges struck at much the same
                // angle, and a few rows of punch furrows.
                let (planes, _) = facets(uv, 0.05, 0.5, face ^ 0x7a1);
                let broad = fbm2(uv * (1.0 / 0.08), z, 2);
                let pick = gouges(uv, 0.009, angle, face ^ 0x91c);
                let furrow = furrows(uv, 0.017, angle + 0.3, face ^ 0x3f7);
                0.9 * (planes - 0.05) + 0.2 * broad - 0.55 * pick - 0.3 * furrow + 0.25
            }
            StoneKind::Slab => {
                if k == 1 {
                    // Beds: gently undulating, with shallow flake scars.
                    let (scar, _) = facets(uv, 0.06, 0.6, face ^ 0x7e1);
                    0.35 * fbm2(uv * (1.0 / 0.09), z, 2) + 0.55 * (scar - 0.05) + 0.05
                } else {
                    // Edges: laminae a few millimeters thick, each standing out or set back.
                    let y = q.y * (1.0 / 0.0075) + 0.9 * noise3(q * (1.0 / 0.04) + Vec3::splat(z));
                    let layer = y.floor();
                    let f = y - layer;
                    let offset = |l: f32| {
                        hash3([l as i32, self.seed as i32, 77]) as f32 / u32::MAX as f32 - 0.5
                    };
                    let a = offset(layer);
                    let b = offset(layer + 1.0);
                    let step = a + (b - a) * smoothstep(0.7, 1.0, f);
                    let parting = 1.0 - smoothstep(0.0, 0.2, f.min(1.0 - f));
                    1.1 * step - 0.5 * parting + 0.3 * fbm2(uv * (1.0 / 0.035), z, 2)
                }
            }
        }
    }

    /// Lichen crusts: (cover 0..1, which lichen 0..1, age 0..1 from the rim inward).
    fn lichen_at(&self, p: Vec3, n: Vec3) -> (f32, f32, f32) {
        // Crusts grow where rain and light reach: tops, and the faces.
        let open = smoothstep(-0.35, 0.5, n.y + 0.25 * n.z.abs());
        let chance = self.lichen.clamp(0.0, 1.0) * open;
        if chance <= 0.0 {
            return (0.0, 0.0, 0.0);
        }
        let cell = 0.03;
        let g = p / cell;
        let i = g.floor();
        let mut best = (0.0f32, 0.0f32, 0.0f32);
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let c = [i.x as i32 + dx, i.y as i32 + dy, i.z as i32 + dz];
                    let h = hash3([c[0] ^ self.seed as i32, c[1], c[2]]);
                    if (h & 0xff) as f32 / 255.0 >= chance * 0.6 {
                        continue;
                    }
                    let h2 = hash3([c[0], c[1] ^ 0x5bd1, c[2] ^ self.seed as i32]);
                    let center = Vec3::new(
                        c[0] as f32 + (h2 & 0xff) as f32 / 255.0,
                        c[1] as f32 + ((h2 >> 8) & 0xff) as f32 / 255.0,
                        c[2] as f32 + ((h2 >> 16) & 0xff) as f32 / 255.0,
                    );
                    let radius = 0.25 + 0.65 * ((h >> 8) & 0xff) as f32 / 255.0;
                    // Lobed, ragged rims.
                    let d = (g - center).length() / radius
                        * (1.0 + 0.35 * noise3(p * (1.0 / 0.004)) + 0.2 * noise3(p * 90.0));
                    let cover = 1.0 - smoothstep(0.8, 1.0, d);
                    if cover > best.0 {
                        best = (cover, ((h >> 16) & 0xff) as f32 / 255.0, 1.0 - d.min(1.0));
                    }
                }
            }
        }
        best
    }
}

impl crate::virtual_geometry::HeightField for StoneFinish {
    fn height(&self, p: Vec3, n: Vec3, _uv: Vec2) -> f32 {
        let q = p * (1.0 / self.scale.max(0.05));
        // Each face's marks, blended over the arrises like triplanar texturing.
        let mut w = n.abs().powf(8.0);
        w /= (w.x + w.y + w.z).max(1e-12);
        w = (w - Vec3::splat(0.03)).max(Vec3::ZERO);
        w /= (w.x + w.y + w.z).max(1e-6);
        let mut relief = 0.0;
        for k in 0..3 {
            if w[k] <= 0.0 {
                continue;
            }
            let side = (n[k] >= 0.0) as usize;
            let uv = match k {
                0 => Vec2::new(q.z, q.y),
                1 => Vec2::new(q.x, q.z),
                _ => Vec2::new(q.x, q.y),
            };
            relief += w[k] * self.face_relief(k, side, uv, q);
        }
        // Calmer over the arrises, where two faces' marks meet and would fold into each other.
        relief *= 0.45 + 0.55 * smoothstep(0.62, 0.9, n.abs().max_element());
        // Coarse grain, every face alike (finer grain is the material's: triangles a few
        // millimeters long can't show it without aliasing).
        relief += 0.1 * noise3(q * (1.0 / 0.009) + Vec3::splat(self.seed as f32 * 0.37));
        (0.5 + 0.5 * relief).clamp(0.0, 1.0)
    }

    fn color(&self, p: Vec3, n: Vec3, h: f32, base: Vec4) -> Vec4 {
        let mut c = base.truncate();
        // Grime settles in pits and fracture troughs.
        let pit = smoothstep(0.42, 0.12, h);
        c *= 1.0 - 0.4 * self.grime.clamp(0.0, 1.0) * pit;
        // Fresh crests catch light-colored grit.
        c *= 1.0 + 0.12 * smoothstep(0.62, 0.85, h);
        let (cover, kind, age) = self.lichen_at(p, n);
        if cover > 0.0 {
            let lichen = if kind < 0.62 {
                // Pale grey-green crust (Lecanora), paler toward the old center.
                Vec3::new(0.36, 0.39, 0.31).lerp(Vec3::new(0.46, 0.48, 0.42), age)
            } else if kind < 0.74 {
                // Yellow-orange (Xanthoria), duller where it is old.
                Vec3::new(0.46, 0.3, 0.08).lerp(Vec3::new(0.4, 0.33, 0.2), age)
            } else if kind < 0.9 {
                // White crust.
                Vec3::new(0.5, 0.5, 0.47)
            } else {
                // Black tar lichen.
                Vec3::new(0.05, 0.05, 0.045)
            };
            // Crusts sit on the crests and fill in less in the pits.
            // Never a flat fill: thalli and bare grains alternate at the millimeter scale.
            let a = cover
                * (0.45 + 0.4 * smoothstep(0.2, 0.6, h))
                * (0.7 + 0.3 * noise3(p * (1.0 / 0.003)));
            c = c.lerp(lichen, a);
        }
        c.extend(base.w)
    }
}

/// A wall stone to build: what kind, how big, which one, and how weathered.
/// [`WallStone::mesh`] makes it ready to merge into a wall mesh (dense, so virtual geometry
/// draws it); [`WallStone::base_mesh`] and [`WallStone::displacement`] make the same stone from a
/// light mesh plus a [`Displacement`](crate::virtual_geometry::Displacement) on its entity,
/// tessellated in the background when its clusters are built (and cached on disk).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WallStone {
    pub kind: StoneKind,
    /// Size along the wall (x), up (y) and into the wall (z), meters.
    pub size: Vec3,
    pub seed: u32,
    /// Lichen cover, 0..1.
    pub lichen: f32,
    /// Dirt in pits and toward the bottom, 0..1.
    pub grime: f32,
    /// Shortest triangle edges of the finished surface, meters (3 mm by default).
    pub edge_m: f32,
}

impl Default for WallStone {
    fn default() -> Self {
        Self {
            kind: StoneKind::SplitFace,
            size: Vec3::new(0.45, 0.22, 0.28),
            seed: 0,
            lichen: 0.3,
            grime: 0.5,
            edge_m: 0.003,
        }
    }
}

/// Weathered gritstone and limestone shades (linear albedo).
const STONE_TINTS: [Vec3; 8] = [
    Vec3::new(0.46, 0.44, 0.4),
    Vec3::new(0.38, 0.37, 0.35),
    Vec3::new(0.5, 0.46, 0.39),
    Vec3::new(0.33, 0.32, 0.3),
    Vec3::new(0.44, 0.39, 0.32),
    Vec3::new(0.41, 0.41, 0.4),
    Vec3::new(0.31, 0.28, 0.25),
    Vec3::new(0.47, 0.45, 0.4),
];

impl WallStone {
    pub fn new(kind: StoneKind, size: Vec3, seed: u32) -> Self {
        Self {
            kind,
            size,
            seed,
            ..Default::default()
        }
    }

    pub fn with_lichen(mut self, lichen: f32) -> Self {
        self.lichen = lichen;
        self
    }

    pub fn with_grime(mut self, grime: f32) -> Self {
        self.grime = grime;
        self
    }

    /// Finest edges of the finished surface, meters.
    pub fn with_edge(mut self, edge_m: f32) -> Self {
        self.edge_m = edge_m;
        self
    }

    /// The fine surface: tool marks, fractures, grain, lichen and grime.
    pub fn finish(&self) -> StoneFinish {
        StoneFinish {
            kind: self.kind,
            seed: self.seed,
            scale: 1.0,
            lichen: self.lichen,
            grime: self.grime,
        }
    }

    /// Relief of the finish, meters from its deepest pit to its highest crest.
    pub fn relief_m(&self) -> f32 {
        let span = self.size.min_element().clamp(0.03, 0.4);
        match self.kind {
            StoneKind::SplitFace => 0.022f32,
            StoneKind::RoughHewn => 0.013,
            StoneKind::Slab => 0.011,
        }
        .min(span * 0.2)
    }

    /// The displacement that turns [`base_mesh`](Self::base_mesh) into the finished stone.
    pub fn displacement(&self) -> crate::virtual_geometry::Displacement {
        crate::virtual_geometry::Displacement {
            amplitude_m: self.relief_m(),
            midlevel: 0.5,
            edge_m: self.edge_m.max(0.0005),
            max_edge_m: (self.edge_m * 4.0).max(0.002),
            tolerance_m: (self.edge_m * 0.08).max(0.00005),
            max_triangles: 600_000,
            cavity_darkening: 0.0,
            normal_map_strength: 0.35,
            source: crate::virtual_geometry::DisplacementSource::Stone(self.finish()),
        }
    }

    /// The finished stone, centered on the origin and `size` across: the base shape
    /// tessellated and displaced by its finish (tens of thousands of triangles for a stone
    /// half a meter long; merged into a wall, virtual geometry draws it with level of detail).
    pub fn mesh(&self) -> Mesh {
        let d = self.displacement();
        crate::virtual_geometry::tessellate(&self.base_mesh(), &d, d.amplitude_m, &self.finish())
    }

    /// The stone's shape without its fine finish (about 1 cm triangles): a squared block with
    /// faces a few degrees off true, chipped corners and arrises, a pitched face, its color
    /// (shade of stone, mottling, a darker, damp foot). Centered on the origin, `size` across.
    pub fn base_mesh(&self) -> Mesh {
        let size = self.size.max(Vec3::splat(0.02));
        let h = size * 0.5;
        let mut rng = Rng::new(self.seed ^ 0x5701_e000);
        let (tilt, round, chips) = match self.kind {
            StoneKind::SplitFace => (0.05, 0.004, rng.range(3.0, 7.99) as usize),
            StoneKind::RoughHewn => (0.06, 0.011, rng.range(4.0, 9.99) as usize),
            StoneKind::Slab => (0.03, 0.005, rng.range(2.0, 5.99) as usize),
        };
        let support = |n: Vec3| (h * n.abs()).element_sum();
        // Faces a little off square, then chips knocked off arrises and corners.
        let mut planes: Vec<(Vec3, f32)> = Vec::new();
        for axis in 0..3 {
            for s in [-1.0f32, 1.0] {
                let mut n = Vec3::ZERO;
                n[axis] = s;
                let n = (n + rng.unit_vector() * tilt * rng.next()).normalize();
                planes.push((n, support(n) * rng.range(0.97, 1.0)));
            }
        }
        for _ in 0..chips {
            let mut d = Vec3::new(
                if rng.next() < 0.5 { -1.0 } else { 1.0 },
                if rng.next() < 0.5 { -1.0 } else { 1.0 },
                if rng.next() < 0.5 { -1.0 } else { 1.0 },
            );
            if rng.next() < 0.6 {
                d[(rng.next() * 2.999) as usize] = 0.0;
            }
            let n = (d / h.max(Vec3::splat(1e-3)) * h.min_element() + rng.unit_vector() * 0.35)
                .normalize();
            let depth = rng.range(0.15, 0.6) * h.min_element().min(0.06);
            planes.push((n, support(n) - depth));
        }
        let main = 6;
        let bulge = match self.kind {
            StoneKind::SplitFace => rng.range(0.004, 0.012),
            StoneKind::RoughHewn => rng.range(0.002, 0.006),
            StoneKind::Slab => rng.range(0.0, 0.003),
        }
        .min(h.min_element() * 0.2);
        let lumps = Vec3::new(
            rng.range(-300.0, 300.0),
            rng.range(-300.0, 300.0),
            rng.range(-300.0, 300.0),
        );
        let surface = |d: Vec3| -> f32 {
            // Distance along the ray to each plane; a stable soft minimum rounds the arrises.
            let mut t = [f32::INFINITY; 24];
            let mut t_min = f32::INFINITY;
            for (i, &(n, o)) in planes.iter().enumerate().take(24) {
                let c = d.dot(n);
                if c > 1e-4 {
                    t[i] = o / c;
                    t_min = t_min.min(t[i]);
                }
            }
            let sum: f32 = t
                .iter()
                .take(planes.len())
                .filter(|x| x.is_finite())
                .map(|&x| (-(x - t_min) / round).exp())
                .sum();
            let mut r = t_min - round * sum.ln();
            // The faces bow out a little between their arrises (a pitched face), most on the
            // ones that face along z.
            let face = (0..main)
                .filter(|&i| t[i].is_finite())
                .map(|i| ((-(t[i] - t_min) / 0.02).exp(), i))
                .fold((0.0, 0.0), |(sw, sb), (w, i)| {
                    let amount = if i / 2 == 2 { 1.0 } else { 0.45 };
                    (sw + w, sb + w * amount)
                });
            let p = d * r;
            let inner = (Vec3::ONE - (p / h).abs().powf(4.0)).max(Vec3::ZERO);
            let dome = inner.x.min(1.0) * inner.y.min(1.0) * inner.z.min(1.0);
            r += bulge * (face.1 / face.0.max(1e-6)) * dome.powf(0.25)
                + bulge * 0.5 * noise3(p * (1.0 / 0.09) + lumps);
            r
        };

        // A box lattice: points on the box's surface, one triangle pair per cell of its faces,
        // pushed out along their ray onto the stone.
        let base_edge = (size.max_element() / 45.0).clamp(0.006, 0.02);
        let cells = (size / base_edge).ceil().max(Vec3::splat(2.0)).as_uvec3();
        let mut vertices: Vec<Vertex> = Vec::new();
        let mut index: HashMap<[u32; 3], u32> = HashMap::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut vertex = |l: [u32; 3], vertices: &mut Vec<Vertex>| -> u32 {
            *index.entry(l).or_insert_with(|| {
                let q = Vec3::new(
                    l[0] as f32 / cells.x as f32,
                    l[1] as f32 / cells.y as f32,
                    l[2] as f32 / cells.z as f32,
                ) * 2.0
                    - Vec3::ONE;
                let d = (q * h).normalize();
                vertices.push(Vertex::new(d * surface(d), d, Vec2::ZERO));
                vertices.len() as u32 - 1
            })
        };
        let n = [cells.x, cells.y, cells.z];
        for axis in 0..3 {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for side in [0, n[axis]] {
                for i in 0..n[u] {
                    for j in 0..n[v] {
                        let corner = |di: u32, dj: u32| {
                            let mut l = [0u32; 3];
                            l[axis] = side;
                            l[u] = i + di;
                            l[v] = j + dj;
                            l
                        };
                        let [a, b, c, d] = [corner(0, 0), corner(1, 0), corner(1, 1), corner(0, 1)]
                            .map(|l| vertex(l, &mut vertices));
                        // Counter-clockwise seen from outside; alternate the diagonals.
                        let quad = if (i + j) % 2 == 0 {
                            [a, b, c, a, c, d]
                        } else {
                            [a, b, d, b, c, d]
                        };
                        if side == 0 {
                            indices.extend([quad[0], quad[2], quad[1], quad[3], quad[5], quad[4]]);
                        } else {
                            indices.extend(quad);
                        }
                    }
                }
            }
        }

        // Fit the bounds to `size` exactly.
        let (min, max) = vertices.iter().fold(
            (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
            |(lo, hi), v| {
                let p = Vec3::from(v.position);
                (lo.min(p), hi.max(p))
            },
        );
        let center = (min + max) * 0.5;
        let scale = size / (max - min).max(Vec3::splat(1e-4));
        for v in &mut vertices {
            v.position = ((Vec3::from(v.position) - center) * scale).to_array();
        }
        let mut mesh = Mesh::new(vertices, indices);
        mesh.compute_normals();

        // Color: the stone's own shade, mottled, darker and damp toward its foot.
        let tint = STONE_TINTS[(rng.next() * 7.999) as usize] * rng.range(0.62, 0.85);
        let warm = rng.range(-0.04, 0.04);
        let mottle_offset = Vec3::splat(rng.range(-100.0, 100.0));
        for v in &mut mesh.vertices {
            let p = Vec3::from(v.position);
            let n = Vec3::from(v.normal);
            let m = 1.0
                + 0.14 * noise3(p * (1.0 / 0.08) + mottle_offset)
                + 0.07 * noise3(p * (1.0 / 0.025) + mottle_offset);
            let mut c = tint * m * Vec3::new(1.0 + warm, 1.0, 1.0 - warm);
            let low = smoothstep(0.1, -1.0, p.y / h.y);
            let damp = self.grime.clamp(0.0, 1.0) * low;
            c = c.lerp(c * Vec3::new(0.62, 0.66, 0.55), damp);
            // Undersides stay darker (no sky, splashed mud).
            c *= 1.0 - 0.18 * self.grime * smoothstep(0.0, -0.9, n.y);
            v.color = c.extend(1.0).to_array();
            // Box projection in meters by the dominant normal axis.
            let a = n.abs();
            v.uv = if a.y >= a.x && a.y >= a.z {
                Vec2::new(p.x, p.z)
            } else if a.x >= a.z {
                Vec2::new(p.z, p.y)
            } else {
                Vec2::new(p.x, p.y)
            }
            .to_array();
        }
        mesh
    }
}

/// A finished wall stone (see [`WallStone`]): `size` meters along the wall (x), up (y) and
/// into it (z), centered on the origin, the same stone for the same seed. Dense enough that
/// virtual geometry draws it (alone or merged into a wall mesh) with level of detail.
pub fn wall_stone(kind: StoneKind, size: Vec3, seed: u32) -> Mesh {
    WallStone::new(kind, size, seed).mesh()
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Gradient noise, about -1..1.
fn noise3(p: Vec3) -> f32 {
    gradient_noise(p) * 2.0
}

/// 2D gradient noise on the plane `z` of the 3D noise, about -1..1.
fn noise2(uv: Vec2, z: f32) -> f32 {
    gradient_noise(Vec3::new(uv.x, uv.y, z)) * 2.0
}

fn fbm2(mut uv: Vec2, z: f32, octaves: u32) -> f32 {
    let (mut sum, mut amp) = (0.0, 0.6);
    for _ in 0..octaves {
        sum += amp * noise2(uv, z);
        uv = Vec2::new(uv.x * 1.7 - uv.y * 1.1, uv.x * 1.1 + uv.y * 1.7) + Vec2::splat(3.7);
        amp *= 0.5;
    }
    sum
}

/// Random point of a 2D cell: its offset (0..1 per axis) and three more random numbers.
fn cell_random(c: (i32, i32), salt: u32) -> (Vec2, [f32; 3]) {
    let h1 = hash3([c.0, c.1, salt as i32]);
    let h2 = hash3([c.1 ^ 0x2c1b, c.0, salt as i32 ^ 0x51ed]);
    let f = |x: u32| x as f32 / 255.0;
    (
        Vec2::new(f(h1 & 0xff), f((h1 >> 8) & 0xff)),
        [f((h1 >> 16) & 0xff), f(h1 >> 24), f(h2 & 0xff)],
    )
}

/// Conchoidal fracture scars about `cell` meters across, as the lower envelope of one shallow
/// tilted dish per cell (`tilt` sets how far their facets lean): dished facets meeting in
/// straight, sharp ridges. Returns the height (about -0.3..0.45, ridges high) and how far
/// inside its scar the point is (0 on a ridge).
fn facets(uv: Vec2, cell: f32, tilt: f32, salt: u32) -> (f32, f32) {
    let g = uv / cell;
    let i = g.floor();
    let (mut m1, mut m2) = (f32::INFINITY, f32::INFINITY);
    for dy in -1..=1 {
        for dx in -1..=1 {
            let c = (i.x as i32 + dx, i.y as i32 + dy);
            let (o, r) = cell_random(c, salt);
            let d = g - (Vec2::new(c.0 as f32, c.1 as f32) + o);
            let lean = Vec2::new(r[0] - 0.5, r[1] - 0.5) * (2.0 * tilt);
            let v = 0.25 * r[2] + lean.dot(d) + 0.5 * d.length_squared();
            if v < m1 {
                m2 = m1;
                m1 = v;
            } else if v < m2 {
                m2 = v;
            }
        }
    }
    (m1, m2 - m1)
}

/// Pick gouges, 0..1 deep: narrow V-section strokes a pick point tears out, about `cell` meters
/// apart, most struck along `angle` (give or take) and some across it.
fn gouges(uv: Vec2, cell: f32, angle: f32, salt: u32) -> f32 {
    let g = uv / cell;
    let i = g.floor();
    let mut depth = 0.0f32;
    for dy in -1..=1 {
        for dx in -1..=1 {
            let c = (i.x as i32 + dx, i.y as i32 + dy);
            let (o, r) = cell_random(c, salt);
            if r[2] < 0.1 {
                continue; // an unworked spot
            }
            let across_stroke = if r[2] > 0.82 { 1.1 } else { 0.0 };
            let a = Vec2::from_angle(angle + across_stroke + (r[0] - 0.5) * 0.7);
            let off = g - (Vec2::new(c.0 as f32, c.1 as f32) + o);
            let half_len = 0.55 + 0.45 * r[1];
            let half_width = 0.2 + 0.12 * r[0];
            let along = off.dot(a) / half_len;
            let across = off.perp_dot(a) / half_width;
            if along.abs() < 1.0 && across.abs() < 1.0 {
                // Deepest just past the middle, where the point bit in.
                let t = along + 0.3;
                let shape = (1.0 - across.abs()) * (1.0 - t * t / 1.69).max(0.0);
                depth = depth.max(shape * (0.55 + 0.45 * r[1]));
            }
        }
    }
    depth
}

/// Punch furrows, 0..1 deep: parallel strokes `spacing` meters apart along `angle`, each a few
/// spacings long.
fn furrows(uv: Vec2, spacing: f32, angle: f32, salt: u32) -> f32 {
    let dir = Vec2::from_angle(angle);
    let x = uv.perp_dot(dir) / spacing + 0.25 * noise2(uv * (0.6 / spacing), salt as f32 * 0.01);
    let row = x.floor();
    let f = x - row;
    let (o, r) = cell_random((row as i32, 0), salt);
    let along = uv.dot(dir) / (spacing * (3.0 + 3.0 * r[0])) + o.x * 7.0;
    let seg = along.floor();
    let fa = along - seg;
    let (_, rs) = cell_random((row as i32, seg as i32), salt ^ 0xf00);
    if rs[0] < 0.3 {
        return 0.0;
    }
    let profile = (std::f32::consts::PI * f).sin().powi(2);
    let taper = smoothstep(0.0, 0.25, fa) * smoothstep(1.0, 0.7, fa);
    profile * taper * (0.5 + 0.5 * rs[1])
}

/// Unit icosphere subdivided `level` times: vertex directions and counter-clockwise triangles.
fn icosphere(level: u32) -> (Vec<Vec3>, Vec<u32>) {
    let t = (1.0 + 5f32.sqrt()) / 2.0;
    let mut dirs: Vec<Vec3> = [
        (-1.0, t, 0.0),
        (1.0, t, 0.0),
        (-1.0, -t, 0.0),
        (1.0, -t, 0.0),
        (0.0, -1.0, t),
        (0.0, 1.0, t),
        (0.0, -1.0, -t),
        (0.0, 1.0, -t),
        (t, 0.0, -1.0),
        (t, 0.0, 1.0),
        (-t, 0.0, -1.0),
        (-t, 0.0, 1.0),
    ]
    .iter()
    .map(|&(x, y, z)| Vec3::new(x, y, z).normalize())
    .collect();
    let mut tris: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for _ in 0..level {
        let mut midpoints: HashMap<(u32, u32), u32> = HashMap::new();
        let mut midpoint = |a: u32, b: u32, dirs: &mut Vec<Vec3>| {
            *midpoints.entry((a.min(b), a.max(b))).or_insert_with(|| {
                dirs.push((dirs[a as usize] + dirs[b as usize]).normalize());
                dirs.len() as u32 - 1
            })
        };
        let mut next = Vec::with_capacity(tris.len() * 4);
        for [a, b, c] in tris {
            let ab = midpoint(a, b, &mut dirs);
            let bc = midpoint(b, c, &mut dirs);
            let ca = midpoint(c, a, &mut dirs);
            next.extend_from_slice(&[[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        tris = next;
    }
    (dirs, tris.into_iter().flatten().collect())
}

/// Small deterministic random numbers (PCG-style) so rocks are reproducible from their seed.
struct Rng(u64);

impl Rng {
    fn new(seed: u32) -> Self {
        Self((seed as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0x2545_F491_4F6C_DD1D)
    }

    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next()
    }

    fn unit_vector(&mut self) -> Vec3 {
        let z = self.range(-1.0, 1.0);
        let a = self.range(0.0, std::f32::consts::TAU);
        let r = (1.0 - z * z).max(0.0).sqrt();
        Vec3::new(r * a.cos(), z, r * a.sin())
    }
}

fn hash3(i: [i32; 3]) -> u32 {
    let mut h = (i[0] as u32).wrapping_mul(0x8DA6_B343)
        ^ (i[1] as u32).wrapping_mul(0xD816_3841)
        ^ (i[2] as u32).wrapping_mul(0xCB1A_B31F);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5BD1_E995);
    h ^ (h >> 15)
}

/// Gradient (Perlin-style) noise in about -0.5..0.5.
fn gradient_noise(p: Vec3) -> f32 {
    let i = p.floor();
    let f = p - i;
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let grad = |c: [i32; 3], d: Vec3| {
        let h = hash3(c);
        let g = Vec3::new(
            (h & 0xFF) as f32 / 127.5 - 1.0,
            ((h >> 8) & 0xFF) as f32 / 127.5 - 1.0,
            ((h >> 16) & 0xFF) as f32 / 127.5 - 1.0,
        );
        g.dot(d)
    };
    let (x, y, z) = (i.x as i32, i.y as i32, i.z as i32);
    let mut corners = [0.0f32; 8];
    for (k, corner) in corners.iter_mut().enumerate() {
        let o = [(k & 1) as i32, ((k >> 1) & 1) as i32, ((k >> 2) & 1) as i32];
        *corner = grad(
            [x + o[0], y + o[1], z + o[2]],
            f - Vec3::new(o[0] as f32, o[1] as f32, o[2] as f32),
        );
    }
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let x0 = lerp(corners[0], corners[1], u.x);
    let x1 = lerp(corners[2], corners[3], u.x);
    let x2 = lerp(corners[4], corners[5], u.x);
    let x3 = lerp(corners[6], corners[7], u.x);
    lerp(lerp(x0, x1, u.y), lerp(x2, x3, u.y), u.z) * 0.6
}

#[cfg(test)]
mod tests {
    use super::*;

    fn closed(m: &Mesh) -> bool {
        let mut edges: HashMap<(u32, u32), i32> = HashMap::new();
        for tri in m.indices.as_chunks::<3>().0 {
            for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                *edges.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        edges.values().all(|&n| n == 2)
    }

    #[test]
    fn rocks_fit_their_size_and_are_closed() {
        let size = Vec3::new(2.0, 1.2, 1.6);
        let m = rock(size, 7, 4);
        let b = m.aabb();
        assert!((b.max - b.min).abs_diff_eq(size, 1e-3), "{b:?}");
        assert!((b.min.y + 0.6).abs() < 1e-3, "rests on its base: {b:?}");
        assert!(closed(&m), "the rock has holes");
        assert!(m.vertices.iter().all(|v| {
            let n = Vec3::from(v.normal);
            n.is_finite() && (n.length() - 1.0).abs() < 1e-3
        }));
    }

    #[test]
    fn seeds_make_different_rocks_and_the_same_seed_the_same_rock() {
        let a = rock(Vec3::ONE, 1, 3);
        let b = rock(Vec3::ONE, 1, 3);
        let c = rock(Vec3::ONE, 2, 3);
        assert_eq!(a.vertices, b.vertices);
        let diff: f32 = a
            .vertices
            .iter()
            .zip(&c.vertices)
            .map(|(x, y)| Vec3::from(x.position).distance(Vec3::from(y.position)))
            .sum::<f32>()
            / a.vertices.len() as f32;
        assert!(diff > 0.02, "seeds 1 and 2 look alike: {diff}");
    }

    #[test]
    fn rocks_have_flat_faces_and_rough_detail() {
        // Break faces: many normals cluster around a few directions, unlike a noisy sphere.
        let m = rock(Vec3::ONE, 3, 5);
        let mut buckets: HashMap<(i32, i32, i32), usize> = HashMap::new();
        for v in &m.vertices {
            let n = Vec3::from(v.normal) * 4.0;
            *buckets
                .entry((n.x.round() as i32, n.y.round() as i32, n.z.round() as i32))
                .or_default() += 1;
        }
        let mut counts: Vec<usize> = buckets.into_values().collect();
        counts.sort_unstable_by(|a, b| b.cmp(a));
        let top: usize = counts.iter().take(12).sum();
        assert!(
            top as f32 > m.vertices.len() as f32 * 0.35,
            "no dominant faces: {top} of {}",
            m.vertices.len()
        );
    }

    /// Edges by position that aren't matched by the opposite edge (0: closed).
    fn open_edges(m: &Mesh) -> usize {
        let mut ids: HashMap<[u32; 3], u32> = HashMap::new();
        let pid: Vec<u32> = m
            .vertices
            .iter()
            .map(|v| {
                let n = ids.len() as u32;
                *ids.entry(bytemuck::cast(v.position)).or_insert(n)
            })
            .collect();
        let mut count: HashMap<(u32, u32), i32> = HashMap::new();
        for t in m.indices.as_chunks::<3>().0 {
            for k in 0..3 {
                let (a, b) = (pid[t[k] as usize], pid[t[(k + 1) % 3] as usize]);
                *count.entry((a, b)).or_insert(0) += 1;
                *count.entry((b, a)).or_insert(0) -= 1;
            }
        }
        count.values().filter(|&&c| c != 0).count()
    }

    #[test]
    fn wall_stones_are_closed_dense_and_fit_their_size() {
        for kind in [StoneKind::SplitFace, StoneKind::RoughHewn, StoneKind::Slab] {
            let size = match kind {
                StoneKind::Slab => Vec3::new(0.5, 0.08, 0.32),
                _ => Vec3::new(0.42, 0.22, 0.26),
            };
            let stone = WallStone::new(kind, size, 11);
            let base = stone.base_mesh();
            assert_eq!(open_edges(&base), 0, "{kind:?}: the base has holes");
            let b = base.aabb();
            assert!((b.max - b.min).abs_diff_eq(size, 1e-3), "{kind:?}: {b:?}");
            let started = std::time::Instant::now();
            let m = stone.mesh();
            eprintln!(
                "{kind:?}: base {} triangles, finished {} triangles in {:.2} s",
                base.triangle_count(),
                m.triangle_count(),
                started.elapsed().as_secs_f32()
            );
            assert_eq!(open_edges(&m), 0, "{kind:?}: the finished stone has holes");
            // Dense enough for virtual geometry's default threshold, light enough to merge
            // dozens into a wall.
            assert!(
                (50_000..600_000).contains(&m.triangle_count()),
                "{kind:?}: {} triangles",
                m.triangle_count()
            );
            let f = m.aabb();
            let relief = stone.relief_m();
            assert!(
                (f.max - f.min - size).abs().max_element() <= relief + 1e-3,
                "{kind:?}: {f:?}"
            );
            assert!(m.vertices.iter().all(|v| {
                let n = Vec3::from(v.normal);
                n.is_finite() && (n.length() - 1.0).abs() < 1e-3
            }));
        }
    }

    #[test]
    fn wall_stones_are_the_same_for_a_seed_and_differ_between_seeds() {
        let size = Vec3::new(0.3, 0.16, 0.2);
        let a = WallStone::new(StoneKind::RoughHewn, size, 3).with_edge(0.005);
        let b = WallStone::new(StoneKind::RoughHewn, size, 4).with_edge(0.005);
        let (ma, ma2, mb) = (a.mesh(), a.mesh(), b.mesh());
        assert_eq!(ma.vertices, ma2.vertices);
        assert_eq!(ma.indices, ma2.indices);
        assert_ne!(ma.vertices.len(), mb.vertices.len());
        // The finish itself is deterministic and seeded.
        use crate::virtual_geometry::HeightField;
        let p = Vec3::new(0.05, 0.02, 0.1);
        let n = Vec3::Z;
        assert_eq!(
            a.finish().height(p, n, Vec2::ZERO),
            a.finish().height(p, n, Vec2::ZERO)
        );
        let differs = (0..64)
            .filter(|&i| {
                let p = Vec3::new(i as f32 * 0.003, 0.01, 0.1);
                (a.finish().height(p, n, Vec2::ZERO) - b.finish().height(p, n, Vec2::ZERO)).abs()
                    > 0.02
            })
            .count();
        assert!(differs > 16, "seeds 3 and 4 have the same marks");
    }

    #[test]
    fn finishes_have_the_relief_of_their_kind() {
        // Sample a face: the heights spread over most of 0..1, with pits (pick marks) and
        // crests, and the split face rougher at the centimeter scale than the slab's bed.
        let spread = |f: StoneFinish, n: Vec3| {
            let hs: Vec<f32> = (0..4000)
                .map(|i| {
                    let p = Vec3::new((i % 64) as f32 * 0.0021, (i / 64) as f32 * 0.0023, 0.0);
                    let p = if n == Vec3::Y {
                        Vec3::new(p.x, 0.0, p.y)
                    } else {
                        p
                    };
                    f.height(p, n, Vec2::ZERO)
                })
                .collect();
            let mean = hs.iter().sum::<f32>() / hs.len() as f32;
            let var = hs.iter().map(|h| (h - mean).powi(2)).sum::<f32>() / hs.len() as f32;
            (mean, var.sqrt())
        };
        use crate::virtual_geometry::HeightField;
        let (m, split) = spread(StoneFinish::new(StoneKind::SplitFace, 1), Vec3::Z);
        assert!((0.25..0.75).contains(&m), "split face mean {m}");
        let (_, hewn) = spread(StoneFinish::new(StoneKind::RoughHewn, 1), Vec3::Z);
        let (_, bed) = spread(StoneFinish::new(StoneKind::Slab, 1), Vec3::Y);
        assert!(split > 0.08 && hewn > 0.06, "split {split}, hewn {hewn}");
        assert!(bed < split, "bed {bed} vs split face {split}");
    }
}
