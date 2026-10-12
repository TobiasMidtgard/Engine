//! Building the mesh of one quadtree patch on the CPU.
//!
//! A patch is a 33×33 vertex grid (32×32 quads) plus a skirt hanging below its border that
//! hides cracks against neighbors at other levels of detail. Heights come from the planet
//! shape (with detail limited to what the grid can show) and the edit layer. Positions are
//! stored relative to the patch origin in `f32`; the origin stays `f64`.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use engine_core::math::DVec3;

use crate::cube::{NodeKey, SurfaceMap};
use crate::edits::EditLayer;
use crate::shape::PlanetShape;

/// Quads per patch edge.
pub const QUADS: usize = 32;
/// Vertices per patch edge.
pub const GRID: usize = QUADS + 1;
/// Grid vertices plus skirt vertices.
pub const VERTS_PER_CHUNK: usize = GRID * GRID + 4 * GRID;

/// Vertex flag: skirt vertex (hangs below the border).
pub const FLAG_SKIRT: u8 = 1;
/// Vertex flag: open ocean surface (sea level drawn over the sea floor).
pub const FLAG_OCEAN: u8 = 2;
/// The flag byte's upper 6 bits hold the vertex's convexity (see [`encode_convexity`]).
pub const CONVEXITY_SHIFT: u8 = 2;

/// GPU vertex of a terrain patch (36 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, PartialEq)]
pub struct TerrainVertex {
    /// Terrain-local position relative to the patch origin, meters.
    pub position: [f32; 3],
    /// Terrain-local unit normal (snorm16 xyz) and the geomorph offset along up (w, in
    /// [`MORPH_RANGE`] vertex spacings).
    pub normal: [i16; 4],
    /// Height above sea level, meters (the sea floor depth under ocean vertices).
    pub altitude: f32,
    /// Moisture, temperature (−60..+60 °C), mare darkness, flags (unorm8).
    pub climate: [u8; 4],
    /// Painted layer index, paint weight, regional variety, local patches.
    pub paint: [u8; 4],
    /// Tint of hand-made ground: color (square root of linear) and amount (unorm8).
    pub tint: [u8; 4],
}

impl TerrainVertex {
    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRS: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
            0 => Float32x3, 1 => Snorm16x4, 2 => Float32, 3 => Unorm8x4, 4 => Uint8x4,
            8 => Unorm8x4
        ];
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<TerrainVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &ATTRS,
        }
    }
}

/// Everything needed to build patches of one terrain (shared with worker threads).
#[derive(Debug, Clone)]
pub struct ChunkSource {
    pub shape: Arc<PlanetShape>,
    pub edits: Arc<EditLayer>,
    pub map: SurfaceMap,
    /// Draw the sea as a surface at sea level (when no water module does it).
    pub ocean_surface: bool,
}

impl ChunkSource {
    /// Height and paint at face coordinates, with detail down to `min_wavelength`.
    pub fn height_at(&self, face: u8, a: f64, b: f64, min_wavelength: f64) -> Sample {
        let (base, up, dir) = self.map.base(face, a, b);
        self.sample(base, up, dir, min_wavelength)
    }

    /// Sample at a surface column (`base` at height 0 with `up`; `dir` for the shape).
    pub fn sample(&self, base: DVec3, up: DVec3, dir: DVec3, min_wavelength: f64) -> Sample {
        let s = self.shape.sample(dir, min_wavelength);
        let (height, mut paint) = if self.edits.is_empty() {
            (s.height, (0, 0.0))
        } else {
            let coarse = |wavelength: f64| {
                self.shape
                    .sample(dir, wavelength.max(min_wavelength))
                    .height
            };
            self.edits.apply(base, up, s.height, &coarse)
        };
        // Hand-made ground's paint, under any edits painted on it.
        let mut tint = [0.0; 4];
        if let Some(op) = self
            .shape
            .overlay
            .as_ref()
            .and_then(|o| o.paint_at(dir, self.shape.radius_m, s.height))
        {
            if op.layer != 0 && op.weight > paint.1 * 2.0 {
                paint = (op.layer, op.weight);
            }
            let [r, g, b] = op.tint;
            tint = [r, g, b, op.tint_amount];
        }
        Sample {
            height,
            moisture: s.moisture,
            temperature_c: s.temperature_c,
            mare: s.mare,
            paint,
            tint,
        }
    }
}

/// One evaluated surface column.
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub height: f64,
    pub moisture: f32,
    pub temperature_c: f32,
    pub mare: f32,
    pub paint: (u8, f32),
    /// Tint of hand-made ground: linear color and amount (0 = none).
    pub tint: [f32; 4],
}

/// A built patch.
#[derive(Debug, Clone)]
pub struct ChunkMesh {
    pub key: NodeKey,
    /// Terrain-local origin the vertices are relative to.
    pub origin: DVec3,
    pub vertices: Vec<TerrainVertex>,
    /// Lowest and highest terrain height in the patch, meters.
    pub min_height: f64,
    pub max_height: f64,
    /// Bounding sphere (terrain-local center, radius).
    pub center: DVec3,
    pub radius: f64,
    /// Vertex spacing, meters.
    pub spacing: f64,
}

/// Height drawn for a sample: the sea surface hides the sea floor when the terrain draws it.
fn shown(src: &ChunkSource, height: f64) -> f64 {
    if src.ocean_surface && src.shape.ocean {
        height.max(-0.25)
    } else {
        height
    }
}

/// Range of the geomorph offset, in vertex spacings (stored in the normal's w).
pub const MORPH_RANGE: f64 = 16.0;

fn encode_morph(offset: f64, spacing: f64) -> i16 {
    ((offset / (spacing * MORPH_RANGE)).clamp(-1.0, 1.0) * 32767.0).round() as i16
}

/// Convexity (vertex spacings above the neighbors' mean) in the flag byte's upper 6 bits:
/// ±0.5 spacings map to the ends of the range.
pub fn encode_convexity(spacings: f64) -> u8 {
    let q = (((spacings * 2.0).clamp(-1.0, 1.0) * 0.5 + 0.5) * 63.0).round() as u8;
    q << CONVEXITY_SHIFT
}

/// The shader's decoding of [`encode_convexity`], −1..1.
pub fn decode_convexity(flags: u8) -> f32 {
    (flags >> CONVEXITY_SHIFT) as f32 / 63.0 * 2.0 - 1.0
}

/// Tint color stored as its square root (finer steps in the darks), amount linear.
fn encode_tint(t: [f32; 4]) -> [u8; 4] {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    [q(t[0].sqrt()), q(t[1].sqrt()), q(t[2].sqrt()), q(t[3])]
}

fn encode_normal(n: DVec3) -> [i16; 4] {
    let q = |v: f64| (v.clamp(-1.0, 1.0) * 32767.0).round() as i16;
    [q(n.x), q(n.y), q(n.z), 0]
}

/// Build the mesh for `key`.
pub fn build_chunk(src: &ChunkSource, key: NodeKey) -> ChunkMesh {
    let map = &src.map;
    let (lo, size) = key.face_rect();
    let step = size / QUADS as f64;
    let spacing = map.node_size(key.level) / QUADS as f64;
    let min_wavelength = spacing * 2.0;
    let center_ab = lo + size * 0.5;
    let origin = map.position(key.face, center_ab.x, center_ab.y, 0.0);

    // Samples with a one-vertex border for normals.
    const S: usize = GRID + 2;
    let mut pos = vec![DVec3::ZERO; S * S];
    let mut ups = vec![DVec3::ZERO; S * S];
    let mut dirs = vec![DVec3::ZERO; S * S];
    let mut bases = vec![DVec3::ZERO; S * S];
    let mut samples = vec![
        Sample {
            height: 0.0,
            moisture: 0.0,
            temperature_c: 0.0,
            mare: 0.0,
            paint: (0, 0.0),
            tint: [0.0; 4],
        };
        S * S
    ];
    for j in 0..S {
        for i in 0..S {
            let a = lo.x + (i as f64 - 1.0) * step;
            let b = lo.y + (j as f64 - 1.0) * step;
            let (base, up, dir) = map.base(key.face, a, b);
            let s = src.sample(base, up, dir, min_wavelength);
            pos[j * S + i] = base + up * shown(src, s.height);
            ups[j * S + i] = up;
            dirs[j * S + i] = dir;
            bases[j * S + i] = base;
            samples[j * S + i] = s;
        }
    }

    // Geomorphing: where every vertex would be on the parent patch (sampled at the parent's
    // detail on the even vertices, on the parent's triangles in between). The shader slides
    // vertices there as the patch nears the distance where it merges into its parent, so
    // detail fades in instead of popping.
    let mut morph = vec![0.0f64; GRID * GRID];
    if key.level > 0 {
        const HALF: usize = GRID / 2 + 1;
        let mut parent = vec![DVec3::ZERO; HALF * HALF];
        for pj in 0..HALF {
            for pi in 0..HALF {
                let idx = (pj * 2 + 1) * S + pi * 2 + 1;
                let (base, up, dir) = (bases[idx], ups[idx], dirs[idx]);
                let h = src.sample(base, up, dir, min_wavelength * 2.0).height;
                parent[pj * HALF + pi] = base + up * shown(src, h);
            }
        }
        let p = |i: usize, j: usize| parent[(j / 2) * HALF + i / 2];
        for j in 0..GRID {
            for i in 0..GRID {
                let target = match (i % 2, j % 2) {
                    (0, 0) => p(i, j),
                    (1, 0) => (p(i - 1, j) + p(i + 1, j)) * 0.5,
                    (0, 1) => (p(i, j - 1) + p(i, j + 1)) * 0.5,
                    // The parent quad's diagonal (see `chunk_indices`).
                    _ if (i / 2 + j / 2) % 2 == 0 => (p(i - 1, j - 1) + p(i + 1, j + 1)) * 0.5,
                    _ => (p(i + 1, j - 1) + p(i - 1, j + 1)) * 0.5,
                };
                let idx = (j + 1) * S + i + 1;
                morph[j * GRID + i] = (target - pos[idx]).dot(ups[idx]);
            }
        }
    }

    let mut vertices = Vec::with_capacity(VERTS_PER_CHUNK);
    let (mut min_h, mut max_h) = (f64::MAX, f64::MIN);
    let mut radius2 = 0.0f64;
    let mut center_sum = DVec3::ZERO;
    for j in 1..=GRID {
        for i in 1..=GRID {
            let idx = j * S + i;
            let p = pos[idx];
            let up = ups[idx];
            let da = pos[idx + 1] - pos[idx - 1];
            let db = pos[idx + S] - pos[idx - S];
            let mut n = da.cross(db).normalize_or(up);
            if n.dot(up) < 0.0 {
                n = -n;
            }
            let s = samples[idx];
            let m = morph[(j - 1) * GRID + i - 1];
            min_h = min_h.min(s.height).min(s.height + m);
            max_h = max_h.max(s.height).max(s.height + m);
            center_sum += p;
            let ocean = src.ocean_surface && src.shape.ocean && s.height < -0.25;
            if ocean {
                n = up;
            }
            // How far this vertex stands out from the 8 around it, in vertex spacings (+ crests,
            // − gullies): the shader keeps crests rocky and wind-blown and fills gullies.
            let ring = [
                idx - 1,
                idx + 1,
                idx - S,
                idx + S,
                idx - S - 1,
                idx - S + 1,
                idx + S - 1,
                idx + S + 1,
            ];
            let around = ring.iter().map(|&k| pos[k]).sum::<DVec3>() / ring.len() as f64;
            let convexity = if ocean {
                0.0
            } else {
                (p - around).dot(up) / spacing
            };
            let mut normal = encode_normal(n);
            normal[3] = encode_morph(m, spacing);
            vertices.push(TerrainVertex {
                position: (p - origin).as_vec3().to_array(),
                normal,
                altitude: s.height as f32,
                climate: [
                    (s.moisture.clamp(0.0, 1.0) * 255.0) as u8,
                    (((s.temperature_c + 60.0) / 120.0).clamp(0.0, 1.0) * 255.0) as u8,
                    (s.mare.clamp(0.0, 1.0) * 255.0) as u8,
                    encode_convexity(convexity) | if ocean { FLAG_OCEAN } else { 0 },
                ],
                paint: [
                    s.paint.0,
                    (s.paint.1.clamp(0.0, 1.0) * 255.0) as u8,
                    (variety(src, dirs[idx], spacing) * 255.0) as u8,
                    (patches(src, dirs[idx], spacing) * 255.0) as u8,
                ],
                tint: encode_tint(s.tint),
            });
        }
    }
    let center = center_sum / (GRID * GRID) as f64;

    // The layer index is not interpolated (the shader takes it from one vertex per
    // triangle), so unpainted vertices next to painted ones take the neighbor's layer at
    // weight 0: painted areas then fade out smoothly instead of in triangle steps.
    for j in 0..GRID {
        for i in 0..GRID {
            let v = j * GRID + i;
            if vertices[v].paint[1] != 0 {
                continue;
            }
            let neighbors = [
                (i > 0).then(|| v - 1),
                (i + 1 < GRID).then(|| v + 1),
                (j > 0).then(|| v - GRID),
                (j + 1 < GRID).then(|| v + GRID),
            ];
            if let Some(layer) = neighbors
                .into_iter()
                .flatten()
                .map(|n| vertices[n].paint)
                .find(|p| p[1] != 0)
                .map(|p| p[0])
            {
                vertices[v].paint[0] = layer;
            }
        }
    }

    // Skirts: copies of the border vertices pushed down along `up`.
    let skirt = map.node_size(key.level) * 0.03 + 1.0;
    let border: Vec<usize> = (0..GRID) // bottom row (j = 0)
        .chain((0..GRID).map(|i| (GRID - 1) * GRID + i)) // top row
        .chain((0..GRID).map(|j| j * GRID)) // left column
        .chain((0..GRID).map(|j| j * GRID + GRID - 1)) // right column
        .collect();
    for &v in &border {
        let (i, j) = (v % GRID, v / GRID);
        let up = ups[(j + 1) * S + i + 1];
        let mut sv = vertices[v];
        let p = DVec3::from(sv.position.map(f64::from)) + origin - up * skirt;
        sv.position = (p - origin).as_vec3().to_array();
        sv.climate[3] |= FLAG_SKIRT;
        vertices.push(sv);
    }

    for v in &vertices {
        let p = DVec3::from(v.position.map(f64::from)) + origin;
        radius2 = radius2.max((p - center).length_squared());
    }

    ChunkMesh {
        key,
        origin,
        vertices,
        min_height: min_h,
        max_height: max_h,
        center,
        radius: radius2.sqrt(),
        spacing,
    }
}

/// Regional variety 0..1 (patches of forest, meadow and scrub a few kilometers across),
/// without the layers finer than the vertices can carry: sampled kilometers apart they turned
/// into random blotches the size of a vertex, a camouflage pattern seen from orbit.
fn variety(src: &ChunkSource, dir: DVec3, spacing: f64) -> f32 {
    let p = dir * (src.shape.radius_m / 18_000.0);
    let octaves = ((18_000.0 / (2.0 * spacing)).log2() + 1.0).clamp(0.0, 4.0);
    let n = crate::noise::fbm_smooth(p, src.shape.seed ^ 0xB10E, octaves, 0.55);
    (n * 0.6 + 0.5).clamp(0.0, 1.0) as f32
}

/// Local patches 0..1 (woods, meadows and fields a few hundred meters across), fading to 0.5
/// where vertices are too far apart to carry them.
fn patches(src: &ChunkSource, dir: DVec3, spacing: f64) -> f32 {
    let fade = 1.0 - ((spacing - 60.0) / 240.0).clamp(0.0, 1.0);
    if fade <= 0.0 {
        return 0.5;
    }
    let p = dir * (src.shape.radius_m / 900.0);
    let n = crate::noise::fbm(p, src.shape.seed ^ 0xF1E1D, 3, 0.5);
    (0.5 + n * 0.9 * fade).clamp(0.0, 1.0) as f32
}

/// Shared index buffer for every patch: the grid, then the four skirts.
pub fn chunk_indices() -> Vec<u16> {
    let mut idx = Vec::with_capacity(QUADS * QUADS * 6 + 4 * QUADS * 6);
    let g = GRID as u16;
    for j in 0..QUADS as u16 {
        for i in 0..QUADS as u16 {
            let a = j * g + i;
            let b = a + 1;
            let c = a + g;
            let d = c + 1;
            // Alternate the diagonal for a less directional pattern.
            if (i + j) % 2 == 0 {
                idx.extend_from_slice(&[a, b, d, a, d, c]);
            } else {
                idx.extend_from_slice(&[a, b, c, b, d, c]);
            }
        }
    }
    let base = (GRID * GRID) as u16;
    let side = |k: u16| base + k * g;
    // Border vertex t of each side: grid index = start + t * stride.
    // (start, stride, skirt base, winding flip)
    let rows: [(u16, u16, u16, bool); 4] = [
        (0, 1, side(0), true),            // bottom: faces -b
        ((g - 1) * g, 1, side(1), false), // top: faces +b
        (0, g, side(2), false),           // left: faces -a
        (g - 1, g, side(3), true),        // right: faces +a
    ];
    for &(start, stride, skirt, flip) in rows.iter() {
        for t in 0..QUADS as u16 {
            let (a, b) = (start + t * stride, start + (t + 1) * stride);
            let (sa, sb) = (skirt + t, skirt + t + 1);
            if flip {
                idx.extend_from_slice(&[a, sa, b, b, sa, sb]);
            } else {
                idx.extend_from_slice(&[a, b, sa, b, sb, sa]);
            }
        }
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cube::TerrainDomain;
    use crate::shape::PlanetShape;

    fn source() -> ChunkSource {
        let shape = PlanetShape::moon(1);
        ChunkSource {
            map: SurfaceMap::new(TerrainDomain::Sphere, shape.radius_m),
            shape: Arc::new(shape),
            edits: Arc::new(EditLayer::default()),
            ocean_surface: true,
        }
    }

    #[test]
    fn chunk_has_grid_and_skirts() {
        let src = source();
        let key = NodeKey {
            face: 2,
            level: 6,
            x: 10,
            y: 20,
        };
        let c = build_chunk(&src, key);
        assert_eq!(c.vertices.len(), VERTS_PER_CHUNK);
        assert!(c.max_height >= c.min_height);
        // Every vertex lies within the bounding sphere.
        for v in &c.vertices {
            let p = DVec3::from(v.position.map(f64::from)) + c.origin;
            assert!((p - c.center).length() <= c.radius + 1e-3);
        }
        // Normals point away from the planet center.
        for v in &c.vertices[..GRID * GRID] {
            let n = DVec3::new(v.normal[0] as f64, v.normal[1] as f64, v.normal[2] as f64);
            let p = DVec3::from(v.position.map(f64::from)) + c.origin;
            assert!(n.dot(p) > 0.0);
        }
    }

    #[test]
    fn convexity_marks_crests() {
        for (spacings, want) in [
            (0.0, 0.0),
            (0.25, 0.5),
            (-0.25, -0.5),
            (3.0, 1.0),
            (-3.0, -1.0),
        ] {
            let f = encode_convexity(spacings);
            assert_eq!(f & (FLAG_SKIRT | FLAG_OCEAN), 0);
            let got = decode_convexity(f | FLAG_OCEAN);
            assert!((got - want).abs() < 0.02, "{spacings}: {got}");
        }
        // On a real patch, every peak (above all 8 neighbors) reads as a crest.
        let c = build_chunk(
            &source(),
            NodeKey {
                face: 2,
                level: 6,
                x: 10,
                y: 20,
            },
        );
        let h = |i: usize, j: usize| c.vertices[j * GRID + i].altitude;
        let mut peaks = 0;
        for j in 1..GRID - 1 {
            for i in 1..GRID - 1 {
                let around = [
                    (i - 1, j),
                    (i + 1, j),
                    (i, j - 1),
                    (i, j + 1),
                    (i - 1, j - 1),
                    (i + 1, j - 1),
                    (i - 1, j + 1),
                    (i + 1, j + 1),
                ];
                if around.iter().all(|&(x, y)| h(i, j) > h(x, y) + 0.5) {
                    peaks += 1;
                    assert!(decode_convexity(c.vertices[j * GRID + i].climate[3]) > 0.0);
                }
            }
        }
        assert!(peaks > 0);
    }

    #[test]
    fn neighbors_share_border_heights() {
        let src = source();
        let a = NodeKey {
            face: 4,
            level: 8,
            x: 100,
            y: 77,
        };
        let b = NodeKey { x: 101, ..a };
        let ca = build_chunk(&src, a);
        let cb = build_chunk(&src, b);
        for j in 0..GRID {
            let pa =
                DVec3::from(ca.vertices[j * GRID + GRID - 1].position.map(f64::from)) + ca.origin;
            let pb = DVec3::from(cb.vertices[j * GRID].position.map(f64::from)) + cb.origin;
            assert!((pa - pb).length() < 0.05, "row {j}: {}", (pa - pb).length());
        }
    }

    #[test]
    fn indices_are_in_range_and_wind_outward() {
        let idx = chunk_indices();
        assert!(idx.iter().all(|&i| (i as usize) < VERTS_PER_CHUNK));
        let src = source();
        let c = build_chunk(
            &src,
            NodeKey {
                face: 0,
                level: 5,
                x: 3,
                y: 9,
            },
        );
        let p = |i: u16| DVec3::from(c.vertices[i as usize].position.map(f64::from)) + c.origin;
        let tri = &idx[..3];
        let n = (p(tri[1]) - p(tri[0])).cross(p(tri[2]) - p(tri[0]));
        assert!(n.dot(p(tri[0])) > 0.0);
    }
}
