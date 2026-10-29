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

/// GPU vertex of a terrain patch (48 bytes).
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
    /// Moisture, temperature (−130..+60 °C, so the polar ice of Mars fits), mare darkness,
    /// flags (unorm8).
    pub climate: [u8; 4],
    /// Weights of the painted layers 1..=10 (unorm8, see `edits::PAINT_NAMES`), then regional
    /// variety and local patches. One weight per layer, so they interpolate across triangles
    /// and painted areas meet smoothly.
    pub paint: [u8; 12],
    /// Tint of hand-made ground: color (square root of linear) and amount (unorm8).
    pub tint: [u8; 4],
    /// River and lake water, the lake's part of it and nearness to river banks (unorm8; see
    /// [`ShapeSample::water`](crate::ShapeSample)), then a spare byte.
    pub river: [u8; 4],
    /// The parent patch's normal here (octahedral snorm16), which the shader blends toward as
    /// the vertex morphs.
    pub parent_normal: [i16; 2],
}

impl TerrainVertex {
    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRS: [wgpu::VertexAttribute; 10] = wgpu::vertex_attr_array![
            0 => Float32x3, 1 => Snorm16x4, 2 => Float32, 3 => Unorm8x4, 4 => Unorm8x4,
            9 => Unorm8x4, 10 => Unorm8x4, 8 => Unorm8x4, 11 => Unorm8x4, 13 => Snorm16x2
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

    /// The layer edits painted at a surface column (index and weight; weight 0 if none).
    pub fn edit_paint(&self, base: DVec3, up: DVec3, dir: DVec3) -> (u8, f32) {
        if self.edits.is_empty() {
            return (0, 0.0);
        }
        let coarse = |wavelength: f64| self.shape.sample(dir, wavelength).height;
        let height = self.shape.sample(dir, 0.0).height;
        self.edits.apply(base, up, height, &coarse).1
    }

    /// Ground height only at a surface column (no paint), with detail down to
    /// `min_wavelength`: what colliders and ground queries need.
    pub fn ground_height(&self, base: DVec3, up: DVec3, dir: DVec3, min_wavelength: f64) -> f64 {
        let h = self.shape.sample(dir, min_wavelength).height;
        if self.edits.is_empty() {
            return h;
        }
        let coarse = |wavelength: f64| {
            self.shape
                .sample(dir, wavelength.max(min_wavelength))
                .height
        };
        self.edits.apply(base, up, h, &coarse).0
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
            water: s.water,
            lake: s.lake,
            banks: s.banks,
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
    /// River and lake water, 0..1, the lake's share of it, and nearness to river banks.
    pub water: f32,
    pub lake: f32,
    pub banks: f32,
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

/// A painted layer and its weight as one weight per layer (layers 1..=10), then regional
/// variety and local patches (0..1).
fn encode_paint((layer, weight): (u8, f32), variety: f32, patches: f32) -> [u8; 12] {
    let mut weights = [0.0; 10];
    if (1..=10).contains(&layer) {
        weights[layer as usize - 1] = weight;
    }
    encode_paint_weights(&weights, variety, patches)
}

/// Weights of layers 1..=10, then regional variety and local patches (0..1).
fn encode_paint_weights(weights: &[f32; 10], variety: f32, patches: f32) -> [u8; 12] {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let mut out = [0; 12];
    for (o, &w) in out.iter_mut().zip(weights) {
        *o = q(w);
    }
    out[10] = q(variety);
    out[11] = q(patches);
    out
}

/// Vertices this far apart (m) or more show hand-made ground's paint as the mean over their
/// cell rather than the point under them.
const FILTER_PAINT_SPACING: f64 = 2.0;
/// Paint samples across a vertex's cell, per side.
const PAINT_TAPS: usize = 3;

/// Hand-made ground's paint and tint over the cell of the vertex at face coordinates `(a, b)`
/// (`step` wide), `height` above sea level: per-layer weights and the mean tint. A level's
/// covers change within meters (a wood's edge, a snowfield, a field), so samples a vertex
/// spacing apart alias into blocks and staircases on far patches; the cell's mean is what the
/// patch can show, and the shader shapes the edges per pixel. `None` outside the overlay.
fn filtered_paint(
    src: &ChunkSource,
    face: u8,
    a: f64,
    b: f64,
    step: f64,
    height: f64,
) -> Option<([f32; 10], [f32; 4])> {
    let overlay = src.shape.overlay.as_ref().filter(|o| o.paint.is_some())?;
    let mut weights = [0.0f32; 10];
    let (mut tint, mut amount) = ([0.0f32; 3], 0.0f32);
    let mut any = false;
    for tj in 0..PAINT_TAPS {
        for ti in 0..PAINT_TAPS {
            let off = |t: usize| ((t as f64 + 0.5) / PAINT_TAPS as f64 - 0.5) * step;
            let (_, _, dir) = src.map.base(face, a + off(ti), b + off(tj));
            let Some(op) = overlay.paint_at(dir, src.shape.radius_m, height) else {
                continue;
            };
            any = true;
            if (1..=10).contains(&op.layer) {
                weights[op.layer as usize - 1] += op.weight;
            }
            for (t, c) in tint.iter_mut().zip(op.tint) {
                *t += c * op.tint_amount;
            }
            amount += op.tint_amount;
        }
    }
    if !any {
        return None;
    }
    let n = (PAINT_TAPS * PAINT_TAPS) as f32;
    for w in &mut weights {
        *w /= n;
    }
    let color = tint.map(|c| if amount > 0.0 { c / amount } else { 0.0 });
    Some((weights, [color[0], color[1], color[2], amount / n]))
}

/// Tint color stored as its square root (finer steps in the darks), amount linear.
fn encode_tint(t: [f32; 4]) -> [u8; 4] {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    [q(t[0].sqrt()), q(t[1].sqrt()), q(t[2].sqrt()), q(t[3])]
}

/// A unit vector folded onto the octahedron (Cigolle et al. 2014), snorm16.
fn encode_octahedral(n: DVec3) -> [i16; 2] {
    let n = n / (n.x.abs() + n.y.abs() + n.z.abs()).max(1e-12);
    let (x, z) = if n.y >= 0.0 {
        (n.x, n.z)
    } else {
        (
            (1.0 - n.z.abs()) * n.x.signum(),
            (1.0 - n.x.abs()) * n.z.signum(),
        )
    };
    let q = |v: f64| (v.clamp(-1.0, 1.0) * 32767.0).round() as i16;
    [q(x), q(z)]
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
            water: 0.0,
            lake: 0.0,
            banks: 0.0,
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
    // The parent's normals come along (Losasso and Hoppe 2004 blend a level's shading into the
    // coarser one's with the geometry's own weight), so the light doesn't jump when the
    // parent takes over.
    let mut morph = vec![0.0f64; GRID * GRID];
    let mut parent_normals = vec![None; GRID * GRID];
    if key.level > 0 {
        // The parent's vertices, with a ring beyond the patch for their normals.
        const HALF: usize = GRID / 2 + 1;
        const PS: usize = HALF + 2;
        let mut parent = vec![DVec3::ZERO; PS * PS];
        let mut parent_up = vec![DVec3::ZERO; PS * PS];
        for pj in 0..PS {
            for pi in 0..PS {
                let (fi, fj) = (pi * 2, pj * 2);
                let (base, up, dir) = if (1..=HALF).contains(&pi) && (1..=HALF).contains(&pj) {
                    let idx = (fj - 1) * S + fi - 1;
                    (bases[idx], ups[idx], dirs[idx])
                } else {
                    let a = lo.x + (fi as f64 - 2.0) * step;
                    let b = lo.y + (fj as f64 - 2.0) * step;
                    map.base(key.face, a, b)
                };
                let h = src.sample(base, up, dir, min_wavelength * 2.0).height;
                parent[pj * PS + pi] = base + up * shown(src, h);
                parent_up[pj * PS + pi] = up;
            }
        }
        let mut parent_n = vec![DVec3::ZERO; HALF * HALF];
        for pj in 0..HALF {
            for pi in 0..HALF {
                let at = |di: usize, dj: usize| parent[(pj + dj) * PS + pi + di];
                let up = parent_up[(pj + 1) * PS + pi + 1];
                let mut n = (at(2, 1) - at(0, 1))
                    .cross(at(1, 2) - at(1, 0))
                    .normalize_or(up);
                if n.dot(up) < 0.0 {
                    n = -n;
                }
                parent_n[pj * HALF + pi] = n;
            }
        }
        let p = |i: usize, j: usize| parent[(j / 2 + 1) * PS + i / 2 + 1];
        let pn = |i: usize, j: usize| parent_n[(j / 2) * HALF + i / 2];
        for j in 0..GRID {
            for i in 0..GRID {
                // On the parent's triangles: its vertex, an edge's middle or the diagonal's
                // (see `chunk_indices`).
                let (a, b) = match (i % 2, j % 2) {
                    (0, 0) => ((i, j), (i, j)),
                    (1, 0) => ((i - 1, j), (i + 1, j)),
                    (0, 1) => ((i, j - 1), (i, j + 1)),
                    _ if (i / 2 + j / 2) % 2 == 0 => ((i - 1, j - 1), (i + 1, j + 1)),
                    _ => ((i + 1, j - 1), (i - 1, j + 1)),
                };
                let target = (p(a.0, a.1) + p(b.0, b.1)) * 0.5;
                let idx = (j + 1) * S + i + 1;
                morph[j * GRID + i] = (target - pos[idx]).dot(ups[idx]);
                parent_normals[j * GRID + i] =
                    Some((pn(a.0, a.1) + pn(b.0, b.1)).normalize_or(ups[idx]));
            }
        }
    }

    // Hand-made ground on far patches: its paint averaged over each vertex's cell, unless an
    // edit's paint (a crater's ash, a dug pit) wins there, as in `ChunkSource::sample`.
    let mut filtered = vec![None; GRID * GRID];
    if spacing >= FILTER_PAINT_SPACING {
        for j in 1..=GRID {
            for i in 1..=GRID {
                let idx = j * S + i;
                let a = lo.x + (i as f64 - 1.0) * step;
                let b = lo.y + (j as f64 - 1.0) * step;
                let Some((weights, tint)) =
                    filtered_paint(src, key.face, a, b, step, samples[idx].height)
                else {
                    continue;
                };
                let (layer, weight) = src.edit_paint(bases[idx], ups[idx], dirs[idx]);
                let strongest = weights.iter().copied().fold(0.0, f32::max);
                if layer != 0 && strongest <= weight * 2.0 {
                    continue;
                }
                filtered[(j - 1) * GRID + i - 1] = Some((weights, tint));
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
            let parent_n = if ocean {
                up
            } else {
                parent_normals[(j - 1) * GRID + i - 1].unwrap_or(n)
            };
            vertices.push(TerrainVertex {
                position: (p - origin).as_vec3().to_array(),
                normal,
                parent_normal: encode_octahedral(parent_n),
                altitude: s.height as f32,
                climate: [
                    (s.moisture.clamp(0.0, 1.0) * 255.0) as u8,
                    (((s.temperature_c + 130.0) / 190.0).clamp(0.0, 1.0) * 255.0) as u8,
                    (s.mare.clamp(0.0, 1.0) * 255.0) as u8,
                    encode_convexity(convexity) | if ocean { FLAG_OCEAN } else { 0 },
                ],
                paint: match &filtered[(j - 1) * GRID + i - 1] {
                    Some((weights, _)) => encode_paint_weights(
                        weights,
                        variety(src, dirs[idx], spacing),
                        patches(src, dirs[idx], spacing),
                    ),
                    None => encode_paint(
                        s.paint,
                        variety(src, dirs[idx], spacing),
                        patches(src, dirs[idx], spacing),
                    ),
                },
                tint: encode_tint(filtered[(j - 1) * GRID + i - 1].map_or(s.tint, |f| f.1)),
                river: [
                    (s.water.clamp(0.0, 1.0) * 255.0).round() as u8,
                    (s.lake.clamp(0.0, 1.0) * 255.0).round() as u8,
                    (s.banks.clamp(0.0, 1.0) * 255.0).round() as u8,
                    0,
                ],
            });
        }
    }
    let center = center_sum / (GRID * GRID) as f64;

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
pub(crate) fn variety(src: &ChunkSource, dir: DVec3, spacing: f64) -> f32 {
    let p = dir * (src.shape.radius_m / 18_000.0);
    // Octaves down to three vertex spacings: finer ones alias into the mesh's squares between
    // far vertices (the shader's per-pixel noise carries the detail there).
    let octaves = ((18_000.0 / (3.0 * spacing)).log2() + 1.0).clamp(0.0, 4.0);
    let n = crate::noise::fbm_smooth(p, src.shape.seed ^ 0xB10E, octaves, 0.55);
    (n * 0.6 + 0.5).clamp(0.0, 1.0) as f32
}

/// Local patches 0..1 (woods, meadows and fields a few hundred meters across), fading to 0.5
/// where vertices are too far apart to carry them.
pub(crate) fn patches(src: &ChunkSource, dir: DVec3, spacing: f64) -> f32 {
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
    fn paint_is_one_weight_per_layer() {
        let sand = crate::edits::paint_index("sand");
        let p = encode_paint((sand, 0.5), 0.25, 1.0);
        assert_eq!(p[sand as usize - 1], 128);
        assert_eq!(p.iter().take(10).filter(|&&w| w != 0).count(), 1);
        assert_eq!((p[10], p[11]), (64, 255));
        // Unpainted: no layer has weight.
        assert!(encode_paint((0, 1.0), 0.0, 0.0).iter().all(|&w| w == 0));
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
    fn morphed_vertices_take_the_parents_normals() {
        let src = source();
        let parent = NodeKey {
            face: 4,
            level: 9,
            x: 200,
            y: 154,
        };
        let child = parent.children()[0];
        let (cp, cc) = (build_chunk(&src, parent), build_chunk(&src, child));
        let decode = |e: [i16; 2]| {
            let (x, z) = (e[0] as f64 / 32767.0, e[1] as f64 / 32767.0);
            let mut n = DVec3::new(x, 1.0 - x.abs() - z.abs(), z);
            let t = (-n.y).max(0.0);
            n.x -= t * n.x.signum();
            n.z -= t * n.z.signum();
            n.normalize()
        };
        let normal = |v: &TerrainVertex| {
            DVec3::new(v.normal[0] as f64, v.normal[1] as f64, v.normal[2] as f64).normalize()
        };
        let mut bent = 0;
        for j in 0..=QUADS / 2 {
            for i in 0..=QUADS / 2 {
                let theirs = normal(&cp.vertices[j * GRID + i]);
                let v = &cc.vertices[(j * 2) * GRID + i * 2];
                let ours = decode(v.parent_normal);
                assert!(ours.dot(theirs) > 0.9999, "({i}, {j}): {ours} vs {theirs}");
                bent += (normal(v).dot(theirs) < 0.9999) as u32;
            }
        }
        // The child's own normals carry finer detail than its parent's.
        assert!(bent > 0);
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
