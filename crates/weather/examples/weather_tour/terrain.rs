//! The landscape around the weather station: a valley with a flat pad at the origin, a rolling
//! meadow, wooded hills a few hundred meters out and a mountain range 3–8 km away, curving
//! away with the planet out to the horizon.
//!
//! * [`height`] is the one analytic height function everything samples (meshes, textures,
//!   scattering, the `Ground` query the weather reads).
//! * The mesh is a set of nested square rings around the origin ("clipmap"), each twice the
//!   size and cell size of the one inside it, split into chunks so the renderer can cull them.
//!   Edge vertices of a ring that borders a coarser ring follow the coarser edge, so there are
//!   no cracks.
//! * Near the station the ground uses a tiling meadow material tinted per vertex; further out
//!   it uses baked "macro" textures (albedo and normal detail from the height function).

use engine_core::math::{DVec3, Vec2, Vec3, Vec4};
use engine_render::{Image, Mesh, Vertex};
use engine_world::{GroundQuery, GroundSample, SurfaceId};

use crate::noise::{eroded64, fbm64, gradient64, hash, ridged64, smoothstep};

/// Radius of the perfectly flat pad around the station, m.
pub const PAD_RADIUS: f64 = 30.0;
/// The pad blends into the meadow out to this radius, m.
pub const PAD_BLEND: f64 = 80.0;
/// Planet radius used to curve the far terrain (matches the renderer's default atmosphere).
pub const PLANET_RADIUS: f64 = 6_360_000.0;

const SEED: u32 = 7;

/// Periodic cubic (Catmull-Rom) interpolation of 8 values keyed every 45° of azimuth.
fn azimuth_curve(keys: &[f64; 8], theta: f64) -> f64 {
    let t = (theta / std::f64::consts::TAU).rem_euclid(1.0) * 8.0;
    let i = t.floor() as i32;
    let f = t - i as f64;
    let k = |j: i32| keys[j.rem_euclid(8) as usize];
    let (p0, p1, p2, p3) = (k(i - 1), k(i), k(i + 1), k(i + 2));
    0.5 * (2.0 * p1
        + (-p0 + p2) * f
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * f * f
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * f * f * f)
}

// Azimuth keys start at +X (west) and turn toward +Z (north): W, NW, N, NE, E, SE, S, SW.
// (East is −X and north is +Z in the weather module's sky.)
/// Peak height of the range in each direction, m. Low in the west so the sun sets over it.
const RANGE_HEIGHT: [f64; 8] = [430.0, 720.0, 860.0, 950.0, 1050.0, 1250.0, 1480.0, 1150.0];
/// Distance where the range starts rising, m.
const RANGE_START: [f64; 8] = [
    6200.0, 5200.0, 4800.0, 4300.0, 4000.0, 3500.0, 3100.0, 3800.0,
];
/// Height of the hills between the meadow and the range, m.
const HILL_HEIGHT: [f64; 8] = [28.0, 80.0, 105.0, 120.0, 125.0, 120.0, 110.0, 70.0];

/// Terrain height without the planet curvature and the station pad, m.
fn raw_height(x: f64, z: f64) -> f64 {
    let r = (x * x + z * z).sqrt();
    // Warp the polar frame so the ranges don't follow circles.
    let wx = x + 900.0 * fbm64(x / 5200.0, z / 5200.0, 3, 0.5, SEED + 1);
    let wz = z + 900.0 * fbm64(x / 5200.0 + 17.3, z / 5200.0 - 4.1, 3, 0.5, SEED + 2);
    let rw = (wx * wx + wz * wz).sqrt();
    let theta = wz.atan2(wx);

    // Meadow: soft undulations a meter or two high.
    let meadow = 2.2 * fbm64(x / 120.0, z / 120.0, 3, 0.5, SEED + 3)
        + 0.4 * fbm64(x / 27.0, z / 27.0, 2, 0.5, SEED + 4);
    // The valley floor rises gently toward the hills.
    let rise = 14.0 * smoothstep(120.0, 1600.0, r);

    // Rolling hills a few hundred meters out.
    let hill_amp = azimuth_curve(&HILL_HEIGHT, theta);
    let hn = fbm64(x / 650.0, z / 650.0, 5, 0.5, SEED + 5) * 0.5 + 0.5;
    let hills = hill_amp
        * smoothstep(160.0, 650.0, r)
        * (hn * hn * 1.6 + 0.12 * fbm64(x / 160.0, z / 160.0, 3, 0.5, SEED + 6));

    // The mountain range.
    let start = azimuth_curve(&RANGE_START, theta);
    let peak = azimuth_curve(&RANGE_HEIGHT, theta);
    let envelope = smoothstep(start - 600.0, start + 2600.0, rw)
        * (1.0 - 0.55 * smoothstep(start + 6000.0, start + 16_000.0, rw));
    let mut mountains = 0.0;
    if envelope > 0.0 {
        let ridges = ridged64(wx / 2900.0, wz / 2900.0, 8, SEED + 7);
        let eroded = eroded64(wx / 3600.0, wz / 3600.0, 10, SEED + 8) * 0.5 + 0.5;
        let shape = (0.62 * ridges + 0.55 * eroded).max(0.0);
        mountains = peak * envelope * shape.powf(1.5) * 1.15;
    }
    // Low, far ranges toward the horizon.
    let far = smoothstep(14_000.0, 30_000.0, r)
        * 380.0
        * (fbm64(x / 9000.0, z / 9000.0, 4, 0.5, SEED + 9) * 0.5 + 0.5);

    meadow + rise + hills + mountains + far
}

/// How much of the pad blend applies at radius `r` (0 on the pad, 1 outside the blend).
fn pad_weight(r: f64) -> f64 {
    smoothstep(PAD_RADIUS, PAD_BLEND, r)
}

/// Drop of the planet surface below the tangent plane at horizontal distance `r`, m.
pub fn curvature_drop(r: f64) -> f64 {
    r * r / (2.0 * PLANET_RADIUS) * smoothstep(300.0, 3000.0, r)
}

/// Terrain height at world `x`, `z`, m. Exactly 0 on the station pad.
pub fn height(x: f64, z: f64) -> f64 {
    let r = (x * x + z * z).sqrt();
    let w = pad_weight(r);
    if w <= 0.0 {
        return 0.0;
    }
    raw_height(x, z) * w - curvature_drop(r)
}

/// Smallest normal step used anywhere, m.
const MIN_EPS: f64 = 0.5;

/// Step for normals at a point: grows with distance from the origin like the mesh cells, so
/// normals are continuous across rings (`d∞ / 64`, where `d∞` is the larger of |x| and |z|).
pub fn normal_eps(x: f64, z: f64) -> f64 {
    (x.abs().max(z.abs()) / 64.0).max(MIN_EPS)
}

/// Surface normal from central differences with step `eps`.
pub fn normal(x: f64, z: f64, eps: f64) -> Vec3 {
    let dx = height(x + eps, z) - height(x - eps, z);
    let dz = height(x, z + eps) - height(x, z - eps);
    DVec3::new(-dx, 2.0 * eps, -dz).normalize().as_vec3()
}

// ------------------------------------------------------------------ surface classification

/// How densely trees grow here, 0..1.
pub fn forest_density(x: f64, z: f64, h: f64, n: Vec3) -> f64 {
    let r = (x * x + z * z).sqrt();
    let patch = fbm64(x / 700.0, z / 700.0, 5, 0.55, SEED + 20);
    let more_far = 0.28 * smoothstep(250.0, 2500.0, r);
    let treeline = 1.0 - smoothstep(1000.0, 1300.0, h);
    let not_cliff = smoothstep(0.6, 0.8, n.y as f64);
    let clearing = smoothstep(240.0, 420.0, r);
    smoothstep(0.0, 0.16, patch - 0.02 + more_far) * treeline * not_cliff * clearing
}

/// Farmland on the gentle ground of the valley: fields of roughly 60–150 m in staggered rows
/// (slightly rotated, borders warped), returning the field's id and the distance to its
/// nearest border, m. Borders carry hedgerows.
pub fn farmland(x: f64, z: f64) -> (u32, f64) {
    const ANGLE: f64 = 0.38;
    const W: f64 = 145.0;
    const L: f64 = 92.0;
    let wx = x + 30.0 * fbm64(x / 450.0, z / 450.0, 2, 0.5, SEED + 40);
    let wz = z + 30.0 * fbm64(x / 450.0 + 7.3, z / 450.0, 2, 0.5, SEED + 41);
    let (c, s) = (ANGLE.cos(), ANGLE.sin());
    let v = (-wx * s + wz * c) / L;
    let row = v.floor();
    let shift = hash(row as i32, 0, SEED + 42) as f64 / u32::MAX as f64;
    let u = (wx * c + wz * s) / W + shift;
    let col = u.floor();
    let mut fu = u - col;
    let id = hash(col as i32, row as i32, SEED + 43);
    // Some fields are split in two lengthwise.
    let mut width = W;
    if id & 3 == 0 {
        fu = (fu * 2.0).fract();
        width *= 0.5;
    }
    let fv = v - row;
    let du = fu.min(1.0 - fu) * width;
    let dv = fv.min(1.0 - fv) * L;
    let sub = if id & 3 == 0 && u.fract() >= 0.5 {
        1
    } else {
        0
    };
    (id.wrapping_add(sub), du.min(dv))
}

/// How much of the ground at a point is farmland (gentle, low, open, away from the station).
pub fn farmland_share(x: f64, z: f64, h: f64, n: Vec3, forest: f64) -> f64 {
    let r = (x * x + z * z).sqrt();
    smoothstep(170.0, 320.0, r)
        * smoothstep(0.93, 0.975, n.y as f64)
        * (1.0 - smoothstep(380.0, 520.0, h))
        * (1.0 - smoothstep(0.2, 0.6, forest))
}

/// Linear albedo and roughness of the ground, as seen from a distance.
#[derive(Clone, Copy, Debug)]
pub struct GroundLook {
    pub albedo: Vec3,
    pub roughness: f32,
}

fn mix3(a: Vec3, b: Vec3, t: f64) -> Vec3 {
    a + (b - a) * t.clamp(0.0, 1.0) as f32
}

/// What the ground looks like at a point (macro color: grass, forest floor, rock, snow).
pub fn ground_look(x: f64, z: f64, h: f64, n: Vec3) -> GroundLook {
    let lush = Vec3::new(0.052, 0.094, 0.026);
    let mid = Vec3::new(0.085, 0.118, 0.036);
    let dry = Vec3::new(0.150, 0.140, 0.062);
    let alpine = Vec3::new(0.105, 0.112, 0.055);
    let forest = Vec3::new(0.026, 0.044, 0.020);
    let conifer = Vec3::new(0.020, 0.034, 0.024);
    let rock_light = Vec3::new(0.24, 0.225, 0.205);
    let rock_dark = Vec3::new(0.12, 0.112, 0.10);
    let snow = Vec3::new(0.78, 0.80, 0.84);

    let n1 = fbm64(x / 230.0, z / 230.0, 4, 0.5, SEED + 30) * 0.5 + 0.5;
    let n2 = fbm64(x / 61.0, z / 61.0, 3, 0.5, SEED + 31) * 0.5 + 0.5;
    let n3 = fbm64(x / 13.0, z / 13.0, 2, 0.5, SEED + 32) * 0.5 + 0.5;
    let n4 = fbm64(x / 5.5, z / 5.5, 3, 0.5, SEED + 34) * 0.5 + 0.5;
    let mut c = mix3(lush, mid, n1 * 1.3 - 0.15);
    c = mix3(c, dry, smoothstep(0.55, 0.8, n2 * 0.75 + n1 * 0.35) * 0.8);
    // Meter-scale patches: darker lush clumps and paler, seeding grass.
    c = mix3(c, lush * 0.8, smoothstep(0.6, 0.8, n4) * 0.6);
    c = mix3(
        c,
        Vec3::new(0.13, 0.135, 0.06),
        smoothstep(0.62, 0.85, n3 * 0.6 + (1.0 - n4) * 0.5) * 0.5,
    );
    c *= 0.84 + 0.32 * n3 as f32;
    // Alpine grass higher up.
    c = mix3(c, alpine, smoothstep(600.0, 1000.0, h));
    // Fields (early spring): pasture, hay meadow, freshly cut, lush, winter cereal, ploughed,
    // with darker hedgerows along their borders.
    let fd0 = forest_density(x, z, h, n);
    let farm = farmland_share(x, z, h, n, fd0);
    if farm > 0.0 {
        let (id, border) = farmland(x, z);
        let tone = (id >> 8) % 100;
        let field = if tone < 30 {
            c
        } else if tone < 50 {
            Vec3::new(0.115, 0.135, 0.045)
        } else if tone < 62 {
            Vec3::new(0.17, 0.155, 0.08)
        } else if tone < 77 {
            Vec3::new(0.038, 0.072, 0.02)
        } else if tone < 90 {
            Vec3::new(0.07, 0.125, 0.026)
        } else {
            Vec3::new(0.09, 0.068, 0.048)
        };
        let within = fbm64(x / 37.0, z / 37.0, 2, 0.5, SEED + 44) as f32;
        let field = field * (0.9 + 0.15 * within);
        c = mix3(c, field, farm * 0.95);
        let gaps = smoothstep(-0.25, 0.1, fbm64(x / 60.0, z / 60.0, 2, 0.5, SEED + 45));
        let hedge = smoothstep(7.0, 2.5, border) * gaps * farm;
        c = mix3(c, forest, hedge * 0.75);
    }

    let slope = 1.0 - n.y as f64;
    let rock_noise = fbm64(x / 90.0, z / 90.0, 4, 0.5, SEED + 33);
    let mut rock = smoothstep(0.24, 0.40, slope + rock_noise * 0.08)
        .max(smoothstep(1150.0, 1500.0, h + rock_noise * 120.0) * 0.85);
    // Bare ground only on real slopes near the station.
    rock *= smoothstep(150.0, 500.0, (x * x + z * z).sqrt());
    let rock_col = mix3(rock_dark, rock_light, rock_noise * 0.9 + 0.55);

    let fd = fd0;
    let tree_kind = smoothstep(
        500.0,
        900.0,
        h + 150.0 * fbm64(x / 400.0, z / 400.0, 2, 0.5, 5),
    );
    let forest_col = mix3(forest, conifer, tree_kind) * (0.85 + 0.3 * n3 as f32);

    let mut albedo = mix3(c, forest_col, fd);
    albedo = mix3(albedo, rock_col, rock);

    // Snow on the highest, flatter, north-facing ground.
    let north = (n.z as f64).clamp(-1.0, 1.0);
    let snow_line = 1330.0 - 140.0 * north + 90.0 * rock_noise;
    let snow_w = smoothstep(snow_line, snow_line + 90.0, h) * smoothstep(0.45, 0.7, n.y as f64);
    albedo = mix3(albedo, snow, snow_w);

    let roughness = (0.95 - 0.1 * rock - 0.25 * snow_w) as f32;
    GroundLook { albedo, roughness }
}

// ------------------------------------------------------------------ the Ground query

/// The terrain as the weather (and anything else) sees it through `engine_world::Ground`.
pub struct TerrainGround {
    pub grass: SurfaceId,
    pub rock: SurfaceId,
}

impl GroundQuery for TerrainGround {
    fn sample(&self, p: DVec3) -> Option<GroundSample> {
        let h = height(p.x, p.z);
        let n = normal(p.x, p.z, 0.5);
        let rocky = n.y < 0.75;
        Some(GroundSample {
            height: h,
            normal: n,
            surface: if rocky { self.rock } else { self.grass },
        })
    }
}

// ------------------------------------------------------------------ meshes

/// One square ring of the terrain: `[-half, half]²` minus `[-inner, inner]²`, `cell` m cells.
#[derive(Clone, Copy, Debug)]
pub struct Ring {
    pub half: f64,
    pub inner: f64,
    pub cell: f64,
    /// Cell size of the ring outside this one (for crack-free edges), m.
    pub outer_cell: f64,
}

/// Which material a ring uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RingMaterial {
    /// Tiling meadow textures, tinted per vertex.
    Near,
    /// Baked macro texture A (`MACRO_A_HALF`).
    MacroA,
    /// Baked macro texture B (`MACRO_B_HALF`).
    MacroB,
}

/// Half size of the area covered by macro texture A, m.
pub const MACRO_A_HALF: f64 = 16_384.0;
/// Half size of the area covered by macro texture B, m.
pub const MACRO_B_HALF: f64 = 131_072.0;
/// Rings up to this half size use the near material.
pub const NEAR_HALF: f64 = 1024.0;

/// The rings, innermost first.
pub fn rings() -> Vec<Ring> {
    // (half size, cell): 64 cells per half size near the station, 128 further out.
    let mut spec = Vec::new();
    let mut half = 64.0;
    while half <= MACRO_B_HALF {
        // Finer where the mountains are (1–16 km), coarser toward the horizon.
        let per_half = if half <= NEAR_HALF || half > MACRO_A_HALF {
            64.0
        } else {
            128.0
        };
        spec.push((half, half / per_half));
        half *= 2.0;
    }
    (0..spec.len())
        .map(|i| Ring {
            half: spec[i].0,
            inner: if i == 0 { 0.0 } else { spec[i - 1].0 },
            cell: spec[i].1,
            outer_cell: spec.get(i + 1).map_or(spec[i].1, |s| s.1),
        })
        .collect()
}

impl Ring {
    pub fn material(&self) -> RingMaterial {
        if self.half <= NEAR_HALF {
            RingMaterial::Near
        } else if self.half <= MACRO_A_HALF {
            RingMaterial::MacroA
        } else {
            RingMaterial::MacroB
        }
    }
}

/// Cells per chunk side.
pub const CHUNK_CELLS: usize = 32;

/// A chunk of a ring: its lower corner and size in cells.
#[derive(Clone, Copy, Debug)]
pub struct Chunk {
    pub ring: Ring,
    pub x0: f64,
    pub z0: f64,
}

/// All chunks of all rings.
pub fn chunks() -> Vec<Chunk> {
    let mut out = Vec::new();
    for ring in rings() {
        let size = ring.cell * CHUNK_CELLS as f64;
        let n = (2.0 * ring.half / size).round() as i32;
        for j in 0..n {
            for i in 0..n {
                let x0 = -ring.half + i as f64 * size;
                let z0 = -ring.half + j as f64 * size;
                let (cx, cz) = (x0 + size * 0.5, z0 + size * 0.5);
                if cx.abs().max(cz.abs()) < ring.inner {
                    continue;
                }
                out.push(Chunk { ring, x0, z0 });
            }
        }
    }
    out
}

/// Turns world (x, z, height, normal) into a vertex color and UV for a ring's material.
pub trait GroundPaint: Sync {
    fn paint(&self, material: RingMaterial, x: f64, z: f64, h: f64, n: Vec3) -> (Vec2, Vec4);
}

/// Height of the vertex at (x, z) in a ring, snapping edge vertices to the coarser ring
/// outside so the two meet without cracks.
fn ring_vertex_height(ring: &Ring, x: f64, z: f64) -> f64 {
    let c = ring.outer_cell;
    if c <= ring.cell * 1.5 {
        return height(x, z);
    }
    let on_x_edge = (x.abs() - ring.half).abs() < 1e-6;
    let on_z_edge = (z.abs() - ring.half).abs() < 1e-6;
    let snap = |t: f64, at: &dyn Fn(f64) -> f64| {
        let t0 = (t / c).floor() * c;
        let f = (t - t0) / c;
        if f < 1e-9 {
            at(t0)
        } else {
            at(t0) * (1.0 - f) + at(t0 + c) * f
        }
    };
    if on_x_edge && !on_z_edge {
        snap(z, &|t| height(x, t))
    } else if on_z_edge && !on_x_edge {
        snap(x, &|t| height(t, z))
    } else {
        height(x, z)
    }
}

/// Build the mesh of one chunk, with vertices relative to `center` (returned).
pub fn chunk_mesh(chunk: &Chunk, paint: &dyn GroundPaint) -> (Mesh, DVec3) {
    let ring = chunk.ring;
    let n = CHUNK_CELLS;
    let size = ring.cell * n as f64;
    let center = DVec3::new(chunk.x0 + size * 0.5, 0.0, chunk.z0 + size * 0.5);
    let material = ring.material();
    let mut vertices = Vec::with_capacity((n + 1) * (n + 1));
    for j in 0..=n {
        for i in 0..=n {
            let x = chunk.x0 + i as f64 * ring.cell;
            let z = chunk.z0 + j as f64 * ring.cell;
            let h = ring_vertex_height(&ring, x, z);
            let nrm = normal(x, z, normal_eps(x, z));
            let (uv, color) = paint.paint(material, x, z, h, nrm);
            let p = DVec3::new(x, h, z) - center;
            vertices.push(Vertex::new(p.as_vec3(), nrm, uv).with_color(color));
        }
    }
    let row = (n + 1) as u32;
    let mut indices = Vec::with_capacity(n * n * 6);
    for j in 0..n as u32 {
        for i in 0..n as u32 {
            let a = j * row + i;
            // Skip cells inside the hole (chunks straddling the inner edge never happen: the
            // inner half size is a multiple of the chunk size).
            indices.extend_from_slice(&[a, a + row, a + 1, a + 1, a + row, a + row + 1]);
        }
    }
    (Mesh::new(vertices, indices), center)
}

/// Height of the rendered (triangulated) surface at (x, z), m: what objects should stand on.
pub fn rendered_height(x: f64, z: f64) -> f64 {
    let d = x.abs().max(z.abs());
    let ring = rings()
        .into_iter()
        .find(|r| d < r.half)
        .unwrap_or_else(|| *rings().last().unwrap());
    let c = ring.cell;
    let (gx, gz) = ((x / c).floor() * c, (z / c).floor() * c);
    let (fx, fz) = ((x - gx) / c, (z - gz) / c);
    let h = |i: f64, j: f64| ring_vertex_height(&ring, gx + i * c, gz + j * c);
    // Cells are split along the (x0, z1)–(x1, z0) diagonal.
    if fx + fz <= 1.0 {
        let h00 = h(0.0, 0.0);
        h00 + fx * (h(1.0, 0.0) - h00) + fz * (h(0.0, 1.0) - h00)
    } else {
        let h11 = h(1.0, 1.0);
        h11 + (1.0 - fx) * (h(0.0, 1.0) - h11) + (1.0 - fz) * (h(1.0, 0.0) - h11)
    }
}

// ------------------------------------------------------------------ macro textures

/// Run `f(row, out_row)` for every row of a `w × h` buffer on all cores.
pub fn par_rows<T: Send + Clone + Default>(
    w: usize,
    h: usize,
    f: impl Fn(usize, &mut [T]) + Sync,
) -> Vec<T> {
    let mut data = vec![T::default(); w * h];
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .max(1);
    let rows_per = h.div_ceil(threads * 4).max(1);
    let f = &f;
    std::thread::scope(|s| {
        let mut chunks: Vec<(usize, &mut [T])> = data
            .chunks_mut(rows_per * w)
            .enumerate()
            .map(|(i, c)| (i * rows_per, c))
            .collect();
        // Hand out chunks round-robin to `threads` workers.
        let mut per_thread: Vec<Vec<(usize, &mut [T])>> =
            (0..threads).map(|_| Vec::new()).collect();
        for (k, c) in chunks.drain(..).enumerate() {
            per_thread[k % threads].push(c);
        }
        for work in per_thread {
            s.spawn(move || {
                for (row0, chunk) in work {
                    for (r, out) in chunk.chunks_mut(w).enumerate() {
                        f(row0 + r, out);
                    }
                }
            });
        }
    });
    data
}

/// Albedo (sRGB), tangent-space normal detail and ORM images covering `[-half, half]²`.
pub struct MacroTextures {
    pub albedo: Image,
    pub normal: Image,
    pub orm: Image,
}

/// Bake the macro textures for the square `[-half, half]²` at `size`² texels.
pub fn bake_macro(half: f64, size: usize) -> MacroTextures {
    let texel = 2.0 * half / size as f64;
    // Heights on the texel grid with a one-texel border (texel centers).
    let border = 1usize;
    let gw = size + 2 * border;
    let heights: Vec<f64> = par_rows(gw, gw, |j, row: &mut [f64]| {
        let z = -half + (j as f64 - border as f64 + 0.5) * texel;
        for (i, out) in row.iter_mut().enumerate() {
            let x = -half + (i as f64 - border as f64 + 0.5) * texel;
            *out = height(x, z);
        }
    });
    let hgrid = |i: isize, j: isize| -> f64 {
        let ii = (i + border as isize).clamp(0, gw as isize - 1) as usize;
        let jj = (j + border as isize).clamp(0, gw as isize - 1) as usize;
        heights[jj * gw + ii]
    };
    #[derive(Clone)]
    struct Px {
        albedo: [u8; 4],
        normal: [u8; 4],
        orm: [u8; 4],
    }
    impl Default for Px {
        fn default() -> Self {
            Self {
                albedo: [0; 4],
                normal: [128, 128, 255, 255],
                orm: [255; 4],
            }
        }
    }
    let pixels: Vec<Px> = par_rows(size, size, |j, row: &mut [Px]| {
        let z = -half + (j as f64 + 0.5) * texel;
        for (i, out) in row.iter_mut().enumerate() {
            let x = -half + (i as f64 + 0.5) * texel;
            let (ii, jj) = (i as isize, j as isize);
            let h = hgrid(ii, jj);
            // Fine normal from neighbouring texels.
            let fine = DVec3::new(
                -(hgrid(ii + 1, jj) - hgrid(ii - 1, jj)),
                2.0 * texel,
                -(hgrid(ii, jj + 1) - hgrid(ii, jj - 1)),
            )
            .normalize();
            // The mesh's vertex normals use a wider step: express the fine normal relative
            // to that smooth frame (tangent = +X, bitangent = +Z on the surface).
            let eps = normal_eps(x, z);
            let k = ((eps / texel).round() as isize).max(1);
            let ke = k as f64 * texel;
            let coarse = DVec3::new(
                -(hgrid(ii + k, jj) - hgrid(ii - k, jj)),
                2.0 * ke,
                -(hgrid(ii, jj + k) - hgrid(ii, jj - k)),
            )
            .normalize();
            let t = (DVec3::X - coarse * coarse.x).normalize();
            let b = (DVec3::Z - coarse * coarse.z).normalize();
            // Tangent space: +X along +u (world +X), +Y along −v (world −Z).
            let tn = DVec3::new(fine.dot(t), -fine.dot(b), fine.dot(coarse)).normalize();
            let look = ground_look(x, z, h, fine.as_vec3());
            // Cavities (lower than the surroundings) are darker.
            let around =
                (hgrid(ii + 2, jj) + hgrid(ii - 2, jj) + hgrid(ii, jj + 2) + hgrid(ii, jj - 2))
                    * 0.25;
            let cavity = ((h - around) / (texel * 0.6)).clamp(-1.0, 1.0);
            let ao = (0.9 + 0.1 * cavity) as f32;
            let a = look.albedo;
            out.albedo = [
                linear_to_srgb8(a.x),
                linear_to_srgb8(a.y),
                linear_to_srgb8(a.z),
                255,
            ];
            out.normal = [
                ((tn.x * 0.5 + 0.5) * 255.0).round() as u8,
                ((tn.y * 0.5 + 0.5) * 255.0).round() as u8,
                ((tn.z * 0.5 + 0.5) * 255.0).round() as u8,
                255,
            ];
            out.orm = [(ao * 255.0) as u8, (look.roughness * 255.0) as u8, 0, 255];
        }
    });
    let mut albedo = Vec::with_capacity(size * size * 4);
    let mut normal = Vec::with_capacity(size * size * 4);
    let mut orm = Vec::with_capacity(size * size * 4);
    for p in &pixels {
        albedo.extend_from_slice(&p.albedo);
        normal.extend_from_slice(&p.normal);
        orm.extend_from_slice(&p.orm);
    }
    MacroTextures {
        albedo: Image::new(size as u32, size as u32, albedo, true),
        normal: Image::new(size as u32, size as u32, normal, false),
        orm: Image::new(size as u32, size as u32, orm, false),
    }
}

/// Linear 0..1 to an sRGB byte.
pub fn linear_to_srgb8(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

/// sRGB byte to linear 0..1.
pub fn srgb8_to_linear(v: u8) -> f32 {
    let s = v as f32 / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

/// Quick 2D gradient noise in world space for callers that scatter things.
pub fn world_noise(x: f64, z: f64, scale: f64, seed: u32) -> f64 {
    gradient64(x / scale, z / scale, seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pad_is_flat_and_rings_nest() {
        for (x, z) in [(0.0, 0.0), (20.0, -10.0), (-29.0, 0.0)] {
            assert_eq!(height(x, z), 0.0);
        }
        let rings = rings();
        for w in rings.windows(2) {
            assert_eq!(w[0].half, w[1].inner);
            assert!(w[1].cell >= w[0].cell);
        }
    }
}
