//! Rocks for the valley: boulders with fractured planar faces and weathered rounded parts, the
//! layered cliff along each [`Scarp`] (with a notch and plunge pool where a stream falls over
//! it), talus at its foot, and the textured rock, wet rock and moss materials they share.
//!
//! Everything a renderer-side rock could replace sits behind three functions:
//! [`rock_materials`], [`boulder_mesh`] and [`cliff_meshes`].
//!
//! - **Boulders** are star-shaped: a lumpy ellipsoid cut by joint planes (smooth-min, so some
//!   edges stay sharp and weathered ones round off), ridged erosion noise that is weaker on the
//!   fresh fracture faces, cracks, bedding grooves and a flattened base. Their vertex colors
//!   bake occlusion (from multi-scale curvature), tint, lichen, dirt and the dark tone under the
//!   moss; a second, alpha-masked mesh carries the moss itself.
//! - **The cliff** is a signed distance field sampled on a voxel grid in the scarp's own frame
//!   (along, side, height) and meshed with surface nets: a face built from strata of hard and
//!   soft beds broken into blocks by joints, a caprock that overhangs, a top that covers the
//!   terrain's own (coarse) step, a talus apron, and the stream's channel and plunge pool cut
//!   out so the water falls over rock.
//! - **UVs** are box-mapped per triangle (the dominant axis of its normal), since the renderer
//!   has no triplanar shader; vertices are split only where the axis changes.

use crate::landscape::{Landscape, Scarp, Stream};
use crate::meshgen::icosphere;
use crate::rock_textures::{self, MOSS_TILE_M, ROCK_TILE_M, RockImages};
use crate::scenery::Lod;
use engine::core::math::noise::{fbm2, fbm3, perlin3};
use engine::core::math::smoothstep;
use engine::prelude::*;
use engine::render::Vertex;
use std::sync::Mutex;
use std::thread::JoinHandle;

// ---------------------------------------------------------------------------------------------
// Small helpers.

fn smin(a: f32, b: f32, k: f32) -> f32 {
    if k <= 0.0 {
        return a.min(b);
    }
    let h = (k - (a - b).abs()).max(0.0) / k;
    a.min(b) - h * h * k * 0.25
}

fn smax(a: f32, b: f32, k: f32) -> f32 {
    -smin(-a, -b, k)
}

fn n3(p: Vec3, seed: u64) -> f32 {
    perlin3(p.x as f64, p.y as f64, p.z as f64, seed)
}

fn fbm3v(p: Vec3, seed: u64, octaves: u32) -> f32 {
    fbm3(p.x as f64, p.y as f64, p.z as f64, seed, octaves, 2.0, 0.5)
}

fn n2(x: f32, y: f32, seed: u64, octaves: u32) -> f32 {
    fbm2(x as f64, y as f64, seed, octaves, 2.0, 0.5)
}

/// Ridged multifractal over 3D Perlin noise, about [0, 1] (sharp crests, soft hollows).
fn ridged3(p: Vec3, seed: u64, octaves: u32) -> f32 {
    let (mut sum, mut amp, mut norm, mut freq, mut weight) = (0.0, 1.0, 0.0, 1.0f32, 1.0f32);
    for i in 0..octaves {
        let n = 1.0 - n3(p * freq, seed.wrapping_add(i as u64 * 977)).abs();
        let n = n * n * weight;
        weight = (n * 1.8).clamp(0.0, 1.0);
        sum += n * amp;
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    sum / norm
}

/// Area-weighted smooth vertex normals.
fn smooth_normals(pos: &[Vec3], idx: &[u32]) -> Vec<Vec3> {
    let mut n = vec![Vec3::ZERO; pos.len()];
    for t in idx.as_chunks::<3>().0 {
        let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
        let f = (pos[b] - pos[a]).cross(pos[c] - pos[a]);
        n[a] += f;
        n[b] += f;
        n[c] += f;
    }
    n.into_iter().map(|v| v.normalize_or(Vec3::Y)).collect()
}

/// Rebuild an indexed mesh with box-mapped UVs: every triangle projects along the axis its
/// normal points most along, so the texture is never stretched by more than √2. Vertices are
/// shared between triangles that pick the same axis. Returns the mesh and, per new vertex, the
/// index of its source vertex (to carry other attributes over).
fn box_mapped(
    pos: &[Vec3],
    nrm: &[Vec3],
    col: &[Vec4],
    idx: &[u32],
    uv_at: impl Fn(Vec3, usize) -> Vec2,
) -> (Mesh, Vec<u32>) {
    let mut slot = vec![[u32::MAX; 3]; pos.len()];
    let mut mesh = Mesh::default();
    let mut src = Vec::with_capacity(pos.len() + pos.len() / 4);
    mesh.indices.reserve(idx.len());
    for t in idx.as_chunks::<3>().0 {
        let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
        let f = (pos[b] - pos[a]).cross(pos[c] - pos[a]).abs();
        let axis = if f.x >= f.y && f.x >= f.z {
            0
        } else if f.y >= f.z {
            1
        } else {
            2
        };
        for &v in t {
            let s = &mut slot[v as usize][axis];
            if *s == u32::MAX {
                *s = mesh.vertices.len() as u32;
                let p = pos[v as usize];
                mesh.vertices.push(
                    Vertex::new(p, nrm[v as usize], uv_at(p, axis)).with_color(col[v as usize]),
                );
                src.push(v);
            }
            mesh.indices.push(*s);
        }
    }
    (mesh, src)
}

/// Planar UV of `p` (m) for projection `axis` (0 = along X, 1 = Y, 2 = Z), in texture tiles.
fn box_uv(p: Vec3, axis: usize) -> Vec2 {
    let q = match axis {
        0 => Vec2::new(p.z, -p.y),
        1 => Vec2::new(p.x, p.z),
        _ => Vec2::new(p.x, -p.y),
    };
    q / ROCK_TILE_M
}

/// The moss skin: copies of the triangles where any corner has moss, pushed out along the
/// normal by `lift` m, colored `tint` with the coverage in alpha (the masked moss material
/// turns that into ragged patches).
fn moss_layer(mesh: &Mesh, src: &[u32], cover: &[f32], tint: &[Vec3], lift: f32) -> Mesh {
    let mut out = Mesh::default();
    let mut map = vec![u32::MAX; mesh.vertices.len()];
    for t in mesh.indices.as_chunks::<3>().0 {
        if t.iter().all(|&v| cover[src[v as usize] as usize] < 0.04) {
            continue;
        }
        for &v in t {
            let m = &mut map[v as usize];
            if *m == u32::MAX {
                *m = out.vertices.len() as u32;
                let s = src[v as usize] as usize;
                let mut vert = mesh.vertices[v as usize];
                let n = Vec3::from(vert.normal);
                vert.position = (Vec3::from(vert.position) + n * lift).to_array();
                vert.color = tint[s].extend(cover[s].clamp(0.0, 1.0)).to_array();
                out.vertices.push(vert);
            }
            out.indices.push(*m);
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Materials.

/// The materials every rock shares (vertex colors multiply the textures).
#[derive(Debug, Clone, Copy)]
pub struct RockMaterials {
    /// Dry rock: albedo, normal and occlusion/roughness textures.
    pub rock: Handle<StandardMaterial>,
    /// The same rock, wet: darker and glossy (stream banks, the falls).
    pub wet: Handle<StandardMaterial>,
    /// Alpha-masked cushion moss for the moss skins.
    pub moss: Handle<StandardMaterial>,
}

/// Make the three rock materials from the generated textures ([`rock_textures::generate`]).
pub fn rock_materials(
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    t: RockImages,
) -> RockMaterials {
    let albedo = images.insert_keyed("showcase/rock/albedo", t.rock_albedo);
    let normal = images.insert_keyed("showcase/rock/normal", t.rock_normal);
    let orm = images.insert_keyed("showcase/rock/orm", t.rock_orm);
    // Roughness comes from the texture (G, 0.45–1); the factor scales it.
    let mut rock = StandardMaterial::hex(0xffffff)
        .with_roughness(1.0)
        .with_base_color_texture(albedo)
        .with_normal_texture(normal)
        .with_metallic_roughness_texture(orm)
        .with_occlusion_texture(orm);
    rock.occlusion_strength = 0.85;
    rock.reflectance = 0.45;
    let mut wet = rock;
    wet.perceptual_roughness = 0.42;
    wet.base_color = [0.82, 0.82, 0.82, 1.0];
    wet.normal_scale = 0.75;
    wet.reflectance = 0.55;
    let tiles = ROCK_TILE_M / MOSS_TILE_M;
    let mut moss = StandardMaterial::hex(0xffffff)
        .with_roughness(0.92)
        .with_base_color_texture(images.insert_keyed("showcase/moss/albedo", t.moss_albedo))
        .with_normal_texture(images.insert_keyed("showcase/moss/normal", t.moss_normal))
        .with_alpha_mask(0.5)
        .with_uv_scale(tiles, tiles)
        .with_subsurface(0.3);
    moss.reflectance = 0.3;
    RockMaterials {
        rock: materials.add(rock),
        wet: materials.add(wet),
        moss: materials.add(moss),
    }
}

// ---------------------------------------------------------------------------------------------
// Boulders.

/// Size classes of boulder meshes (meshes are built in meters at their nominal size; instances
/// scale them by about 0.6–2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RockSize {
    /// About 3 m across.
    Large,
    /// About 1.3 m across.
    Medium,
    /// Stones about 0.5 m across.
    Small,
}

impl RockSize {
    /// Horizontal half extent of the mesh (m).
    pub fn radius(self) -> f32 {
        match self {
            RockSize::Large => 1.5,
            RockSize::Medium => 0.65,
            RockSize::Small => 0.24,
        }
    }

    /// Icosphere subdivisions of the near and far meshes.
    fn subdivisions(self) -> (u32, u32) {
        match self {
            RockSize::Large => (5, 3),
            RockSize::Medium => (4, 2),
            RockSize::Small => (3, 1),
        }
    }

    /// Distance (m, at scale 1) where the near mesh gives way to the far one.
    fn switch(self) -> f32 {
        match self {
            RockSize::Large => 55.0,
            RockSize::Medium => 32.0,
            RockSize::Small => 16.0,
        }
    }
}

/// A boulder's meshes: near (full detail), far (same silhouette, a sixteenth of the
/// triangles) and the moss skin for the near mesh. Origin at the middle of its flat base.
pub struct BoulderMeshes {
    pub near: Mesh,
    pub far: Mesh,
    pub moss: Mesh,
    /// Half extents in x and z, and the height (m).
    pub extent: Vec3,
}

/// Shape of one boulder in unit space (about 1 across), evaluated per direction.
struct BoulderShape {
    seed: u64,
    offset: Vec3,
    axes: Vec3,
    /// Joint and chip planes: normal, offset, rounding.
    planes: Vec<(Vec3, f32, f32)>,
    /// Cracks: normal, offset, width, depth.
    cracks: Vec<(Vec3, f32, f32, f32)>,
    strata_axis: Vec3,
    strata_freq: f32,
    strata_depth: f32,
    ridge_amp: f32,
    flat_y: f32,
    warmth: f32,
    orange_lichen: bool,
    mossiness: f32,
}

impl BoulderShape {
    fn new(seed: u64, angular: f32) -> Self {
        let mut rng = Rng::new(seed);
        let axes = Vec3::new(
            rng.range(1.0, 1.5),
            rng.range(0.5, 0.9),
            rng.range(0.7, 1.1),
        );
        let support = |n: Vec3| (n * axes).length();
        let mut planes = Vec::new();
        // A few joints at irregular angles all round (the faces the rock split along), maybe
        // one across the top, then chips off the edges.
        let count = 3 + rng.range_i32(0, 3);
        let az0 = rng.range(0.0, TAU);
        for j in 0..count {
            let az = az0 + j as f32 * TAU / count as f32 + rng.range(-0.55, 0.55);
            let el = rng.range(-0.3, 0.6);
            let n = Vec3::new(az.cos() * el.cos(), el.sin(), az.sin() * el.cos());
            let k = rng.range(0.006, 0.02) + (1.0 - angular) * rng.range(0.015, 0.06);
            planes.push((n, support(n) * rng.range(0.55, 0.82), k));
        }
        if rng.chance(0.45 + 0.4 * angular) {
            let n = (Vec3::Y + rng.unit_vector() * 0.4).normalize();
            let k = rng.range(0.008, 0.02) + (1.0 - angular) * rng.range(0.02, 0.06);
            planes.push((n, support(n) * rng.range(0.7, 0.9), k));
        }
        for _ in 0..rng.range_i32(5, 11) {
            let mut n = rng.unit_vector();
            n.y = n.y.abs() * 0.9 - 0.15;
            let n = n.normalize();
            let k = rng.range(0.005, 0.02) + (1.0 - angular) * rng.range(0.01, 0.05);
            planes.push((n, support(n) * rng.range(0.74, 0.93), k));
        }
        let cracks = (0..rng.range_i32(0, 3))
            .map(|_| {
                let n = Vec3::new(
                    rng.range(-1.0, 1.0),
                    rng.range(-0.3, 0.3),
                    rng.range(-1.0, 1.0),
                )
                .normalize_or(Vec3::X);
                (
                    n,
                    rng.range(-0.35, 0.35),
                    rng.range(0.012, 0.028),
                    rng.range(0.03, 0.07),
                )
            })
            .collect();
        let strata_axis = (Vec3::Y + rng.unit_vector() * 0.25).normalize();
        Self {
            seed,
            offset: rng.unit_vector() * 10.0,
            axes,
            planes,
            cracks,
            strata_axis,
            strata_freq: rng.range(2.5, 5.0),
            strata_depth: rng.range(0.006, 0.02) * (1.0 - 0.5 * angular),
            ridge_amp: 0.045 + 0.06 * (1.0 - angular),
            flat_y: -axes.y * rng.range(0.45, 0.7),
            warmth: rng.range(-0.3, 0.3),
            orange_lichen: rng.chance(0.3),
            mossiness: rng.range(-0.15, 0.15) - 0.25 * angular,
        }
    }

    /// Surface point along unit direction `d`, and how much of a fresh fracture face it is.
    fn point(&self, d: Vec3) -> (Vec3, f32) {
        let q = d + self.offset;
        let e = 1.0 / (d / self.axes).length();
        let mut r = e
            * (1.0 + 0.14 * fbm3v(q * 0.9, self.seed, 3) + 0.04 * fbm3v(q * 2.6, self.seed + 5, 2));
        let r0 = r;
        let weather = 0.5 + 0.5 * n3(q * 1.1, self.seed + 9);
        for &(n, o, k) in &self.planes {
            let c = d.dot(n);
            if c > 0.02 {
                r = smin(r, o / c, k * (0.4 + 1.2 * weather));
            }
        }
        let fracture = smoothstep(0.005, 0.05, (r0 - r) / e);
        let ridge = ridged3(q * 2.1, self.seed + 1, 4);
        let chips = ridged3(q * 5.5, self.seed + 3, 2);
        let amp = self.ridge_amp * (1.0 - 0.7 * fracture) * (0.5 + weather);
        // Erosion pockets on weathered parts, shallow conchoidal ripples on fresh faces.
        let pocket = smoothstep(0.55, 0.9, n3(q * 5.5, self.seed + 4)) * (1.0 - fracture);
        let ripple = fracture * ridged3(q * 3.2, self.seed + 6, 2);
        r *= 1.0 + amp * (ridge - 0.55) + 0.028 * (chips - 0.5) - 0.014 * pocket
            + 0.012 * (ripple - 0.5)
            + 0.012 * fbm3v(q * 11.0, self.seed + 2, 2);
        for (i, &(n, o, w, depth)) in self.cracks.iter().enumerate() {
            let dist = d.dot(n) - o;
            let mask = smoothstep(-0.15, 0.35, n3(q * 1.6, self.seed + 20 + i as u64));
            r *= 1.0 - depth * mask * (-(dist / w).powi(2)).exp();
        }
        let mut p = d * r;
        let t = p.dot(self.strata_axis) * self.strata_freq + 0.35 * n3(q * 1.7, self.seed + 30);
        p *= 1.0 - self.strata_depth * smoothstep(0.7, 1.0, (t * TAU).cos());
        p.y = smax(p.y, self.flat_y, 0.08);
        (p, fracture)
    }
}

/// Build one boulder: `seed` picks the shape, `angular` (0–1) trades weathered, rounded and
/// mossy for fresh, sharp-edged and bare (talus blocks).
pub fn boulder_mesh(seed: u64, size: RockSize, angular: f32) -> BoulderMeshes {
    let shape = BoulderShape::new(seed, angular);
    let (near_sub, far_sub) = size.subdivisions();
    let (dirs, idx) = icosphere(near_sub);
    let raw: Vec<(Vec3, f32)> = dirs.iter().map(|d| shape.point(*d)).collect();
    // Normalize: base on y = 0, centered in x/z, scaled to the class's radius.
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for (p, _) in &raw {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }
    let half = (hi - lo) * 0.5;
    let scale = size.radius() / half.x.max(half.z);
    let shift = Vec3::new(-(lo.x + hi.x) * 0.5, -lo.y, -(lo.z + hi.z) * 0.5);
    let extent = Vec3::new(half.x * scale, (hi.y - lo.y) * scale, half.z * scale);
    let lift = size.radius() * 0.012;
    // Finer grain on smaller stones (the texture's cracks would look too big on a pebble).
    let uv = match size {
        RockSize::Large => 1.0,
        RockSize::Medium => 1.3,
        RockSize::Small => 1.8,
    };
    let (near, moss) = boulder_lod(
        &shape,
        &dirs,
        &idx,
        &raw,
        (shift, scale, uv),
        extent.y,
        lift,
    );
    let (fdirs, fidx) = icosphere(far_sub);
    let fraw: Vec<(Vec3, f32)> = fdirs.iter().map(|d| shape.point(*d)).collect();
    let (far, _) = boulder_lod(
        &shape,
        &fdirs,
        &fidx,
        &fraw,
        (shift, scale, uv),
        extent.y,
        lift,
    );
    BoulderMeshes {
        near,
        far,
        moss,
        extent,
    }
}

fn boulder_lod(
    shape: &BoulderShape,
    dirs: &[Vec3],
    idx: &[u32],
    raw: &[(Vec3, f32)],
    (shift, scale, uv_scale): (Vec3, f32, f32),
    height: f32,
    lift: f32,
) -> (Mesh, Mesh) {
    let pos: Vec<Vec3> = raw.iter().map(|(p, _)| (*p + shift) * scale).collect();
    let nrm = smooth_normals(&pos, idx);
    // Curvature at two scales from Laplacian smoothing: how far the neighborhood sits above
    // the tangent plane (hollows and cracks are dark, ridges a little lighter).
    let mut pairs: Vec<(u32, u32)> = Vec::with_capacity(idx.len() * 2);
    for t in idx.as_chunks::<3>().0 {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            pairs.push((a, b));
            pairs.push((b, a));
        }
    }
    let smooth_once = |src: &[Vec3]| {
        let mut sum = vec![Vec3::ZERO; src.len()];
        let mut cnt = vec![0.0f32; src.len()];
        for &(a, b) in &pairs {
            sum[a as usize] += src[b as usize];
            cnt[a as usize] += 1.0;
        }
        sum.iter()
            .zip(&cnt)
            .map(|(s, c)| *s / c.max(1.0))
            .collect::<Vec<_>>()
    };
    let edge = pairs
        .iter()
        .take(600)
        .map(|&(a, b)| (pos[a as usize] - pos[b as usize]).length())
        .sum::<f32>()
        / pairs.len().clamp(1, 600) as f32;
    let s1 = smooth_once(&pos);
    let mut wide = s1.clone();
    let wide_iters = (dirs.len() as f32).sqrt() as usize / 6 + 2;
    for _ in 0..wide_iters {
        wide = smooth_once(&wide);
    }
    let size = height.max(0.1);
    let mut col = Vec::with_capacity(pos.len());
    let mut cover = Vec::with_capacity(pos.len());
    let mut tint = Vec::with_capacity(pos.len());
    for i in 0..pos.len() {
        let (p, n, d) = (pos[i], nrm[i], dirs[i]);
        let small = (s1[i] - p).dot(n) / edge.max(1e-4);
        let large = (wide[i] - p).dot(n) / (size * 0.12);
        let concave = (small * 0.5).clamp(0.0, 1.0) + (large * 0.6).clamp(0.0, 1.0);
        let mut ao =
            1.0 - 0.65 * (small * 0.6).clamp(0.0, 1.0) - 0.5 * (large * 0.7).clamp(0.0, 1.0);
        ao *= 0.5 + 0.5 * smoothstep(-0.02 * size, 0.3 * size, p.y);
        let wear = 1.0 + 0.22 * (-small * 0.5).clamp(0.0, 1.0);
        let (c, m, t) = boulder_color(shape, d, p, n, raw[i].1, concave, size);
        col.push((c * ao * wear).extend(1.0));
        cover.push(m);
        tint.push(t * ao.sqrt());
    }
    let (mesh, src) = box_mapped(&pos, &nrm, &col, idx, |p, axis| {
        box_uv(p * uv_scale, axis) + Vec2::new(shape.offset.x, shape.offset.z) * 0.37
    });
    let moss = moss_layer(&mesh, &src, &cover, &tint, lift);
    (mesh, moss)
}

/// Vertex color of a boulder (rock tint × lichen × dirt, darkened under moss), its moss
/// coverage and the moss tint.
fn boulder_color(
    shape: &BoulderShape,
    d: Vec3,
    p: Vec3,
    n: Vec3,
    fracture: f32,
    concave: f32,
    height: f32,
) -> (Vec3, f32, Vec3) {
    let q = d + shape.offset;
    let seed = shape.seed;
    let warm = Vec3::new(0.5, 0.445, 0.375);
    let cool = Vec3::new(0.41, 0.415, 0.415);
    let hue = (0.5 + 0.5 * n3(q * 0.9, seed + 40) + shape.warmth).clamp(0.0, 1.0);
    let mut c = cool.lerp(warm, hue);
    c *= 0.74 + 0.4 * (0.5 + 0.5 * fbm3v(q * 2.5, seed + 41, 3));
    c *= 0.9 + 0.2 * (0.5 + 0.5 * fbm3v(q * 7.0, seed + 47, 2));
    // Fresh fracture faces are a cleaner, lighter shade than the weathered rind.
    c *= 1.0 + 0.14 * fracture;
    // Crustose lichen: pale blotches (sometimes orange) on tops and sides, and small dark
    // rosettes anywhere the rain reaches.
    let lichen = smoothstep(0.56, 0.68, 0.5 + 0.5 * fbm3v(q * 4.5, seed + 42, 3))
        * smoothstep(-0.3, 0.2, n.y)
        * (1.0 - fracture * 0.5);
    let lichen_c = if shape.orange_lichen && n3(q * 2.0, seed + 46) > 0.25 {
        Vec3::new(0.72, 0.46, 0.13)
    } else {
        Vec3::new(0.64, 0.65, 0.57)
    };
    c = c.lerp(lichen_c, lichen * 0.7);
    let dark = smoothstep(0.56, 0.66, 0.5 + 0.5 * fbm3v(q * 9.0, seed + 48, 2))
        * smoothstep(-0.5, 0.1, n.y)
        * (1.0 - fracture * 0.7);
    c = c.lerp(Vec3::new(0.09, 0.09, 0.08), dark * 0.6);
    // Soil splashed up from the ground.
    let ground = 1.0
        - smoothstep(
            0.0,
            0.22 * height,
            p.y + 0.06 * height * n3(q * 3.0, seed + 43),
        );
    c = c.lerp(Vec3::new(0.27, 0.2, 0.13), ground * 0.65);
    // Moss on top and in hollows.
    let patch = fbm3v(q * 1.8, seed + 44, 3);
    let moss = smoothstep(
        0.35,
        0.75,
        n.y * 0.8 + 0.5 * patch + 0.3 * concave + shape.mossiness - 0.1,
    ) * (1.0 - ground * 0.6);
    c = c.lerp(
        Vec3::new(0.1, 0.13, 0.05),
        smoothstep(0.1, 0.6, moss) * 0.85,
    );
    let tint = Vec3::new(0.33, 0.32, 0.3) * (0.8 + 0.4 * (0.5 + 0.5 * n3(q * 3.0, seed + 45)));
    (c, moss, tint)
}

/// One boulder in the library, ready to instance.
pub struct RockVariant {
    pub size: RockSize,
    pub near: Handle<Mesh>,
    pub far: Handle<Mesh>,
    pub moss: Handle<Mesh>,
    /// Half extents in x and z, and the height (m).
    pub extent: Vec3,
    /// Fresh, sharp talus block rather than a weathered boulder.
    pub angular: bool,
}

/// The shared rock materials and every boulder variant.
pub struct RockKit {
    pub mats: RockMaterials,
    pub variants: Vec<RockVariant>,
    /// The shared landscape's cliffs, building in the background until taken.
    cliff: Mutex<Option<JoinHandle<Cliff>>>,
}

/// How to instance a boulder.
#[derive(Debug, Clone, Copy, Default)]
pub struct RockLook {
    /// Use the wet material (in or next to water).
    pub wet: bool,
    /// Draw the moss skin when near.
    pub moss: bool,
}

/// Seeds and styles of the library's boulders: (size, angular).
const VARIANTS: [(RockSize, f32); 15] = [
    (RockSize::Large, 0.1),
    (RockSize::Large, 0.3),
    (RockSize::Large, 0.0),
    (RockSize::Large, 0.85),
    (RockSize::Large, 0.7),
    (RockSize::Medium, 0.2),
    (RockSize::Medium, 0.0),
    (RockSize::Medium, 0.4),
    (RockSize::Medium, 0.9),
    (RockSize::Medium, 0.75),
    (RockSize::Small, 0.3),
    (RockSize::Small, 0.1),
    (RockSize::Small, 0.6),
    (RockSize::Small, 0.9),
    (RockSize::Small, 0.5),
];

/// The rock kit and the cliffs being built on other threads (see [`start_kit`]).
pub struct KitJob(JoinHandle<(RockImages, Vec<BoulderMeshes>, JoinHandle<Cliff>)>);

/// Start generating the rock textures and every boulder variant on other threads, so they
/// overlap the rest of the world's setup; the shared landscape's cliffs follow once they are
/// done (they are needed later, and would only slow the textures down).
pub fn start_kit() -> KitJob {
    KitJob(std::thread::spawn(|| {
        let (images, boulders) = std::thread::scope(|s| {
            let jobs: Vec<_> = VARIANTS
                .iter()
                .enumerate()
                .map(|(i, &(size, angular))| {
                    s.spawn(move || boulder_mesh(4100 + i as u64 * 37, size, angular))
                })
                .collect();
            let images = rock_textures::generate(1024, 512);
            let boulders = jobs
                .into_iter()
                .map(|j| j.join().expect("boulder thread panicked"))
                .collect();
            (images, boulders)
        });
        let cliff = std::thread::spawn(|| cliff_meshes(crate::landscape::shared(), CLIFF_VOXEL));
        (images, boulders, cliff)
    }))
}

impl KitJob {
    /// Wait for the textures and boulders and add them as assets (the cliffs may still be
    /// building; [`RockKit::take_cliff`] waits for them).
    pub fn finish(
        self,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
        images: &mut Assets<Image>,
    ) -> RockKit {
        let (textures, boulders, cliff) = self.0.join().expect("rock kit thread panicked");
        let mats = rock_materials(materials, images, textures);
        let variants = boulders
            .into_iter()
            .zip(VARIANTS)
            .map(|(b, (size, angular))| RockVariant {
                size,
                near: meshes.add(b.near),
                far: meshes.add(b.far),
                moss: meshes.add(b.moss),
                extent: b.extent,
                angular: angular > 0.6,
            })
            .collect();
        RockKit {
            mats,
            variants,
            cliff: Mutex::new(Some(cliff)),
        }
    }
}

impl RockKit {
    /// The cliffs of `land`: the ones built in the background when it is the shared landscape,
    /// otherwise built now.
    pub fn take_cliff(&self, land: &Landscape) -> Cliff {
        let job = self.cliff.lock().ok().and_then(|mut c| c.take());
        match job {
            Some(job) if land.seed == crate::landscape::shared().seed => {
                job.join().expect("cliff thread panicked")
            }
            _ => cliff_meshes(land, CLIFF_VOXEL),
        }
    }

    /// A random variant of `size` (weathered ones unless `angular`).
    pub fn pick(&self, size: RockSize, angular: bool, rng: &mut Rng) -> &RockVariant {
        let options: Vec<&RockVariant> = self
            .variants
            .iter()
            .filter(|v| v.size == size && (v.angular == angular || rng.chance(0.25)))
            .collect();
        options[rng.range_i32(0, options.len() as i32) as usize]
    }

    /// Instance a boulder: near and far meshes swap by distance, the moss skin shows only near.
    /// Small stones cast no shadows.
    pub fn spawn(&self, commands: &mut Commands, v: &RockVariant, tf: Transform, look: RockLook) {
        let scale = tf.scale.max_element();
        let switch = v.size.switch() * scale.max(0.5);
        let mat = if look.wet {
            self.mats.wet
        } else {
            self.mats.rock
        };
        let mut renderer = MeshRenderer::new(v.near, mat);
        if v.size == RockSize::Small {
            renderer = renderer.without_shadows();
        }
        commands.spawn((
            tf,
            renderer,
            Lod {
                near: v.near,
                far: Some(v.far),
                switch,
                hide: f32::INFINITY,
                far_shown: false,
            },
        ));
        if look.moss && !look.wet {
            commands.spawn((
                tf,
                MeshRenderer::new(v.moss, self.mats.moss).without_shadows(),
                Lod::hide_beyond(v.moss, switch * 0.8),
            ));
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The cliff.

/// Height of the land without stream carving (with the planet's curvature). Over the plunge
/// pool's funnel this is where the rock top belongs.
///
/// Away from the streams it is the land's own height (lake basin, pads and roads included);
/// where a stream or pool may have cut in, it is rebuilt from the natural hills plus every
/// scarp's step, blending over a few meters.
pub fn uncarved_height(land: &Landscape, x: f64, z: f64) -> f64 {
    let p = DVec2::new(x, z);
    let fade = |d: f64, reach: f64| {
        let t = ((d - reach + 5.0) / 5.0).clamp(0.0, 1.0);
        1.0 - t * t * (3.0 - 2.0 * t)
    };
    let mut inside = 0.0f64;
    for st in &land.streams {
        if let Some((d, _)) = st.closest(p) {
            inside = inside.max(fade(d, st.width * 0.5 + 21.0));
        }
        if let Some((c, _)) = st.pool {
            inside = inside.max(fade((p - c).length(), 26.0));
        }
    }
    let own = land.height(x, z);
    if inside <= 0.0 {
        return own;
    }
    own + (rebuilt_height(land, p) - own) * inside
}

/// The natural hills plus every scarp's step, with curvature.
fn rebuilt_height(land: &Landscape, p: DVec2) -> f64 {
    let mut h = land.natural(p.x, p.y);
    for sc in &land.scarps {
        h += sc.rise(p, land.seed);
    }
    h - p.length_squared() / (2.0 * 6_360_000.0)
}

/// Rock strata: beds of alternating hard (thick, protruding) and soft (thin, recessed) rock.
struct Strata {
    bounds: Vec<f32>,
    hard: Vec<f32>,
    tint: Vec<Vec3>,
    /// Block length along the face and joint offset per bed (m).
    blocks: Vec<(f32, f32)>,
}

impl Strata {
    fn new(seed: u64) -> Self {
        let palette = [
            Vec3::new(0.5, 0.44, 0.36),
            Vec3::new(0.44, 0.43, 0.41),
            Vec3::new(0.52, 0.41, 0.31),
            Vec3::new(0.36, 0.34, 0.31),
            Vec3::new(0.56, 0.53, 0.47),
        ];
        let mut rng = Rng::new(seed);
        let (mut bounds, mut hard, mut tint, mut blocks) = (vec![], vec![], vec![], vec![]);
        let mut y = -12.0;
        let mut soft_next = false;
        while y < 45.0 {
            let is_hard = !soft_next || rng.chance(0.25);
            let th = if is_hard {
                rng.range(0.7, 2.1)
            } else {
                rng.range(0.25, 0.8)
            };
            bounds.push(y);
            hard.push(if is_hard {
                rng.range(0.6, 1.0)
            } else {
                rng.range(0.0, 0.3)
            });
            let c = palette[rng.range_i32(0, palette.len() as i32) as usize];
            tint.push(c * rng.range(0.92, 1.08) * if is_hard { 1.0 } else { 0.86 });
            blocks.push((rng.range(2.2, 6.5), rng.range(0.0, 20.0)));
            y += th;
            soft_next = is_hard;
        }
        bounds.push(y);
        Self {
            bounds,
            hard,
            tint,
            blocks,
        }
    }

    /// Bed index at height `yb`, position within the bed (0 bottom, 1 top) and its thickness.
    fn layer(&self, yb: f32) -> (usize, f32, f32) {
        let k = self
            .bounds
            .partition_point(|b| *b <= yb)
            .saturating_sub(1)
            .min(self.hard.len() - 1);
        let (b0, b1) = (self.bounds[k], self.bounds[k + 1]);
        (k, ((yb - b0) / (b1 - b0)).clamp(0.0, 1.0), b1 - b0)
    }
}

/// Where the stream (if any) falls over this scarp.
struct Falls {
    /// Along the scarp at the lip (m).
    u: f32,
    /// Half the stream's width (m).
    half_width: f32,
    /// Water surface points (world) from above the lip to past the pool, for wetness.
    water: Vec<DVec3>,
    pool: Option<(DVec2, f64)>,
    bottom_level: f64,
}

/// The cliff's signed distance (m, positive in the air) on a grid in the scarp's frame:
/// u along the scarp from `a`, s across it (positive toward the raised side), y up.
struct CliffField {
    scarp: Scarp,
    seed: u64,
    dir: DVec2,
    nrm: DVec2,
    u0: f32,
    s0: f32,
    y0: f32,
    h: f32,
    nu: usize,
    ns: usize,
    ny: usize,
    sdf: Vec<f32>,
    /// Per (u, s) column: top of the cap, top of the talus, rock allowed below (channel), ground.
    top: Vec<f32>,
    talus: Vec<f32>,
    chan: Vec<f32>,
    /// Per u column: lower ground, cliff height, face position at the foot.
    low: Vec<f32>,
    height: Vec<f32>,
    foot: Vec<f32>,
    strata: Strata,
    falls: Option<Falls>,
}

/// One piece of the cliff along the scarp, meshed relative to `origin`.
pub struct CliffChunk {
    pub origin: DVec3,
    /// Full detail rock (about 0.3 m voxels), dry part.
    pub near: Mesh,
    /// Full detail rock at the falls (wet material).
    pub wet: Mesh,
    /// Moss skin over the near mesh.
    pub moss: Mesh,
    /// The whole chunk at a third of the resolution.
    pub far: Mesh,
}

/// Where talus blocks and scree can sit at the foot of the cliff.
#[derive(Debug, Clone, Copy)]
pub struct TalusSpot {
    pub pos: DVec3,
    pub normal: Vec3,
    /// 0 at the cliff foot, 1 at the toe of the talus.
    pub reach: f32,
    pub wet: bool,
}

/// The cliff meshes, talus spots and a way to ask where its rock top is.
pub struct Cliff {
    pub chunks: Vec<CliffChunk>,
    pub talus: Vec<TalusSpot>,
    fields: Vec<CliffField>,
}

impl Cliff {
    /// Height of the cliff's rock surface at (x, z) where it rises above the terrain.
    pub fn rock_top(&self, land: &Landscape, x: f64, z: f64) -> Option<f64> {
        self.fields.iter().find_map(|f| {
            let (u, s) = f.scarp.frame(DVec2::new(x, z), f.seed);
            let top = f.top_at(u as f32, s as f32)? as f64;
            (top > land.height(x, z) + 0.05).then_some(top)
        })
    }

    /// Inside the cliff's footprint (where it builds its own rock and talus).
    pub fn covers(&self, x: f64, z: f64) -> bool {
        self.fields.iter().any(|f| {
            let (u, s) = f.scarp.frame(DVec2::new(x, z), f.seed);
            let (u, s) = (u as f32, s as f32);
            u > f.u0 + 1.0
                && u < f.u0 + (f.nu - 2) as f32 * f.h
                && s > f.s0 + 1.0
                && s < f.s0 + (f.ns - 2) as f32 * f.h
                && f.top_at(u, s).is_some()
        })
    }
}

/// Voxel size of the cliff meshes (m).
pub const CLIFF_VOXEL: f32 = 0.3;

/// Build the cliffs along every scarp of `land` with `voxel` m cells (see [`CLIFF_VOXEL`]).
pub fn cliff_meshes(land: &Landscape, voxel: f32) -> Cliff {
    let mut chunks = Vec::new();
    let mut talus = Vec::new();
    let mut fields = Vec::new();
    for (i, sc) in land.scarps.iter().enumerate() {
        let field = CliffField::build(land, sc, voxel, 9100 + i as u64 * 17);
        chunks.extend(field.mesh_all());
        talus.extend(field.talus_spots(land));
        fields.push(field);
    }
    Cliff {
        chunks,
        talus,
        fields,
    }
}

/// Run `f(index, item)` over `items` on all cores.
fn par_each<T: Send>(items: &mut [T], f: impl Fn(usize, &mut T) + Sync) {
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 8);
    let band = items.len().div_ceil(threads).max(1);
    std::thread::scope(|s| {
        for (b, chunk) in items.chunks_mut(band).enumerate() {
            let f = &f;
            s.spawn(move || {
                for (k, it) in chunk.iter_mut().enumerate() {
                    f(b * band + k, it);
                }
            });
        }
    });
}

impl CliffField {
    fn build(land: &Landscape, sc: &Scarp, h: f32, seed: u64) -> Self {
        let len = (sc.b - sc.a).length() as f32;
        let dir = (sc.b - sc.a).normalize();
        let nrm = DVec2::new(-dir.y, dir.x);
        let (u0, s0) = (-3.0f32, -13.0f32);
        let (u1, s1) = (len + 3.0, 22.0f32);
        let nu = ((u1 - u0) / h).ceil() as usize + 1;
        let ns = ((s1 - s0) / h).ceil() as usize + 1;

        // The stream that falls over this scarp, if any.
        let falls = land.streams.iter().find_map(|st| falls_over(land, sc, st));
        let u_falls = falls.as_ref().map_or(f32::MAX, |f| f.u);
        let near_falls = |u: f32| sc.notch_weight(u as f64) as f32;
        let half_width = falls.as_ref().map_or(0.0, |f| f.half_width);
        let in_channel =
            |u: f32| smoothstep(half_width + 2.0, half_width + 0.5, (u - u_falls).abs());

        let mut me = Self {
            scarp: *sc,
            seed: land.seed,
            dir,
            nrm,
            u0,
            s0,
            y0: 0.0,
            h,
            nu,
            ns,
            ny: 0,
            sdf: Vec::new(),
            top: Vec::new(),
            talus: Vec::new(),
            chan: Vec::new(),
            low: Vec::new(),
            height: Vec::new(),
            foot: Vec::new(),
            strata: Strata::new(seed),
            falls,
        };

        // Coarse (u, s) samples of the land: ground, uncarved ground and the stream's channel.
        let ch = 0.6f32;
        let cnu = ((u1 - u0) / ch).ceil() as usize + 2;
        let cns = ((s1 - s0) / ch).ceil() as usize + 2;
        let mut coarse = vec![[0.0f32; 4]; cnu * cns];
        par_each(&mut coarse, |idx, out| {
            let (ci, cj) = (idx / cns, idx % cns);
            let (u, s) = (u0 + ci as f32 * ch, s0 + cj as f32 * ch);
            let p = me.world(u as f64, s as f64);
            let g = land.height(p.x, p.y) as f32;
            let hu = uncarved_height(land, p.x, p.y) as f32;
            let mut c = 1.0e3f32;
            let mut funnel = 0.0f32;
            for st in &land.streams {
                if let Some((d, bed)) = st.closest(p) {
                    let d = d as f32;
                    if d < 14.0 {
                        c = c.min(bed as f32 - 0.25 + (d - 2.3).max(0.0) * 1.2);
                    }
                }
                if let Some((pc, pbed)) = st.pool {
                    let dp = (p - pc).length() as f32;
                    funnel = funnel.max(1.0 - smoothstep(17.5, 21.0, dp));
                    if dp < 16.0 {
                        c = c.min(
                            pbed as f32 - 0.4
                                + (dp - 5.2).max(0.0) * 2.6
                                + (s + 1.4).max(0.0) * 9.0,
                        );
                    }
                }
            }
            // The channel cuts the rock only where the stream or the pool has carved the
            // ground; beside them the ground's own step stays inside the cliff (cut away, it
            // showed through as a smooth slab beside the falls with a dark seam at the rock).
            let carved = (hu - g).max(0.0);
            c = c.max(g + 2.0 * (1.0 - smoothstep(0.05, 0.6, carved)));
            *out = [g, hu, c, funnel];
        });
        let coarse_at = |u: f32, s: f32, k: usize| {
            let fu = ((u - u0) / ch).clamp(0.0, (cnu - 1) as f32 - 1e-3);
            let fs = ((s - s0) / ch).clamp(0.0, (cns - 1) as f32 - 1e-3);
            let (i, j) = (fu as usize, fs as usize);
            let (tu, ts) = (fu - i as f32, fs - j as f32);
            let v = |i: usize, j: usize| coarse[i * cns + j][k];
            let a = v(i, j) + (v(i + 1, j) - v(i, j)) * tu;
            let b = v(i, j + 1) + (v(i + 1, j + 1) - v(i, j + 1)) * tu;
            a + (b - a) * ts
        };

        // Per column: lower ground, cliff height.
        me.low = (0..nu)
            .map(|i| coarse_at(u0 + i as f32 * h, -9.0, 1))
            .collect();
        me.height = (0..nu)
            .map(|i| (coarse_at(u0 + i as f32 * h, 6.0, 1) - me.low[i]).max(0.0))
            .collect();
        let y_lo = coarse
            .iter()
            .map(|c| c[0].min(c[1]))
            .fold(f32::MAX, f32::min)
            - 1.0;
        let y_hi = coarse.iter().map(|c| c[1]).fold(f32::MIN, f32::max) + 2.5;
        me.y0 = y_lo;
        me.ny = ((y_hi - y_lo) / h).ceil() as usize + 1;
        let ny = me.ny;

        // The face: where the rock starts across the scarp, per (u, y).
        let joints = {
            let mut rng = Rng::new(seed + 5);
            let mut v = Vec::new();
            let mut u = rng.range(3.0, 8.0);
            while u < len + 5.0 {
                v.push((u, rng.range(0.5, 1.1)));
                u += rng.range(7.0, 16.0);
            }
            v
        };
        let mut face = vec![0.0f32; nu * ny];
        {
            let me_ref = &me;
            par_each(&mut face, |idx, out| {
                let (i, k) = (idx / ny, idx % ny);
                let u = u0 + i as f32 * h;
                let y = me_ref.y0 + k as f32 * h;
                *out = me_ref.face_at(i, u, y, &joints, near_falls(u), in_channel(u));
            });
        }
        me.foot = (0..nu)
            .map(|i| {
                let y = me.low[i];
                let k = (((y - me.y0) / h).round() as usize).min(ny - 1);
                face[i * ny + k]
            })
            .collect();

        // Cap top, talus top and channel floor per (u, s).
        let mut cols = vec![[0.0f32; 3]; nu * ns];
        {
            let me_ref = &me;
            par_each(&mut cols, |idx, out| {
                let (i, j) = (idx / ns, idx % ns);
                let (u, s) = (u0 + i as f32 * h, s0 + j as f32 * h);
                let g = coarse_at(u, s, 0);
                // In front of the plateau's edge the rock top is the plateau's height (the face
                // decides where the rock ends), behind it the uncarved ground.
                let hu = coarse_at(u, s, 1).max(if s < 3.5 {
                    coarse_at(u, 3.5, 1)
                } else {
                    f32::MIN
                });
                let chan = coarse_at(u, s, 2);
                let funnel = coarse_at(u, s, 3);
                let tall = smoothstep(0.8, 2.5, me_ref.height[i]);
                // The cap covers the terrain's step and the funnel the plunge pool carved, then
                // dips under the plateau with a ragged edge.
                let carved = smoothstep(0.08, 0.5, coarse_at(u, s, 1) - g) * funnel;
                let lip = 1.0 - smoothstep(3.0, 6.5, s);
                let edge_noise = 0.3 * n2(u / 5.0, s / 5.0, seed + 7, 3);
                let cover = smoothstep(0.35, 0.65, lip.max(carved) + edge_noise)
                    * tall
                    * (1.0 - smoothstep(s1 - 6.0, s1 - 2.5, s));
                let top = hu + (-1.3 + 1.55 * cover);
                // Talus: an apron about a quarter of the cliff's height, 32° steep.
                let ht = (0.2 * me_ref.height[i] + 0.3)
                    * (0.75 + 0.5 * (0.5 + 0.5 * n2(u / 9.0, 0.3, seed + 8, 2)))
                    * (1.0 - 0.85 * near_falls(u))
                    * tall;
                let wt = ht / 0.62 + 0.8;
                let dist = me_ref.foot[i] - s;
                let prof = (1.0 - dist / wt).clamp(0.0, 1.0).powf(1.3);
                // Past its toe the apron dips well under the ground.
                let sink = 0.25 + 1.0 * (1.0 - smoothstep(0.0, 0.12, prof));
                let talus = g + ht * prof - sink + 0.2 * n2(u / 1.3, s / 1.3, seed + 9, 2);
                *out = [top, talus, chan];
            });
        }
        me.top = cols.iter().map(|c| c[0]).collect();
        me.talus = cols.iter().map(|c| c[1]).collect();
        me.chan = cols.iter().map(|c| c[2]).collect();

        // The distance field.
        let mut sdf = vec![0.0f32; nu * ns * ny];
        {
            let me_ref = &me;
            let face = &face;
            let slab = ns * ny;
            let mut slabs: Vec<&mut [f32]> = sdf.chunks_mut(slab).collect();
            par_each(&mut slabs, |i, out| {
                let u = u0 + i as f32 * h;
                for j in 0..ns {
                    let s = s0 + j as f32 * h;
                    let c = i * ns + j;
                    let (top, talus, chan) = (me_ref.top[c], me_ref.talus[c], me_ref.chan[c]);
                    for k in 0..ny {
                        let y = me_ref.y0 + k as f32 * h;
                        let f = face[i * ny + k] - s;
                        let cliff = smax(f, y - top, 0.5);
                        let tal = y - talus;
                        let mut v = smin(cliff, tal, 0.9);
                        v = smax(v, y - chan, 0.25);
                        if v.abs() < 1.6 {
                            let p = Vec3::new(u, s, y);
                            let rubble = smoothstep(0.3, -0.3, tal - cliff);
                            let band = smoothstep(1.6, 1.1, v.abs());
                            v += band
                                * ((0.07 + 0.21 * rubble) * n3(p / 1.4, seed + 11)
                                    + 0.05 * n3(p / 0.5, seed + 12));
                        }
                        out[j * ny + k] = v;
                    }
                }
            });
        }
        me.sdf = sdf;
        me
    }

    /// Face position (s) at column `i` (u) and height `y`.
    /// `falls` is how far into the falls' notch `u` is ([`Scarp::notch_weight`]), `channel`
    /// how far into the water's channel.
    fn face_at(
        &self,
        i: usize,
        u: f32,
        y: f32,
        joints: &[(f32, f32)],
        falls: f32,
        channel: f32,
    ) -> f32 {
        let seed = self.seed + 900;
        let (low, hgt) = (self.low[i], self.height[i].max(1.0));
        let t = (y - low) / hgt;
        // Leaning back a little, recessed where the water has worn the falls back.
        let mut f = -3.3 + 1.5 * t.clamp(0.0, 1.15) + 2.6 * falls;
        // Gently dipping, folded beds.
        let yb = y + 0.03 * u + 0.9 * n2(u / 40.0, 0.7, seed, 2);
        let (k, tau, th) = self.strata.layer(yb);
        let hard = self.strata.hard[k];
        let (blen, boff) = self.strata.blocks[k];
        let bu = (u + boff) / blen;
        let bf = bu - bu.floor();
        let hb = |n: i64| engine::core::math::hash01(k as i64, bu.floor() as i64, n, seed) - 0.5;
        // Every block juts out by its own amount and its face is a plane tilted its own way,
        // so the sun picks out each fractured block.
        let facet = hb(4) * (bf - 0.5) * blen * 0.22 + hb(5) * (tau - 0.5) * th * 0.45;
        let mut prot = (hard - 0.45) * 1.4 + (hb(3) * 0.5 + facet) * hard;
        // The caprock overhangs in places.
        let cap = (n2(u / 14.0, 2.5, seed + 1, 2) + 0.15).max(0.0) * 2.6;
        prot += cap * smoothstep(0.7, 0.86, t) * (1.0 - falls);
        // Rounded bed edges (grooves at the bedding planes) and joints between blocks.
        // Hard beds keep square edges, soft ones weather round.
        let edge = (2.0 * tau - 1.0).abs();
        let rounding = edge.powi(3) * (1.0 - hard) + edge.powi(8) * hard;
        prot -= rounding * (0.25 + 0.35 * hard) * (th / 1.5).min(1.0);
        let jd = bf.min(1.0 - bf) * blen;
        prot -= 0.4 * hard * (-(jd / 0.2).powi(2)).exp();
        for &(ju, depth) in joints {
            let du = u - ju;
            if du.abs() < 1.5 {
                prot -= depth * (-(du / 0.32).powi(2)).exp() * smoothstep(-0.1, 0.25, t);
            }
        }
        prot *= 1.0 - 0.6 * falls;
        f -= prot;
        f += 0.18 * n2(u / 2.2, y / 2.2, seed + 2, 3) + 0.05 * n2(u / 0.7, y / 0.7, seed + 3, 2);
        // Sharp vertical fractures and flutes (worn by runoff) in the hard beds.
        let flute = 1.0
            - n2(
                u / 1.3 + 0.25 * n2(y / 3.0, u / 7.0, seed + 4, 1),
                0.5,
                seed + 5,
                1,
            )
            .abs()
                * 2.5;
        f -= 0.12 * hard * flute.max(0.0).powi(2);
        // Never behind the terrain's own (coarsely meshed) step, which narrows and moves back
        // in the falls' notch as the face does (or the ground shows through beside the water).
        // Only the channel the water drops down stays open.
        let (start, end) = Scarp::step_span(falls as f64);
        let (start, width) = (start as f32, (end - start) as f32);
        let bound = start + width * (6.2 * t.clamp(0.0, 1.0) - 2.7) / 3.5 - 0.35;
        f = f + (smin(f, bound, 0.4) - f) * (1.0 - channel);
        f
    }

    /// World x/z of frame coordinates (u along, s across).
    fn world(&self, u: f64, s: f64) -> DVec2 {
        let base = self.scarp.a + self.dir * u;
        let (_, s_base) = self.scarp.frame(base, self.seed);
        base + self.nrm * (s - s_base)
    }

    fn idx(&self, i: usize, j: usize, k: usize) -> usize {
        (i * self.ns + j) * self.ny + k
    }

    /// Trilinear distance at frame point (u, s, y).
    fn sample(&self, u: f32, s: f32, y: f32) -> f32 {
        let fi = ((u - self.u0) / self.h).clamp(0.0, (self.nu - 1) as f32 - 1e-3);
        let fj = ((s - self.s0) / self.h).clamp(0.0, (self.ns - 1) as f32 - 1e-3);
        let fk = ((y - self.y0) / self.h).clamp(0.0, (self.ny - 1) as f32 - 1e-3);
        let (i, j, k) = (fi as usize, fj as usize, fk as usize);
        let (tu, ts, ty) = (fi - i as f32, fj - j as f32, fk - k as f32);
        let v = |a: usize, b: usize, c: usize| self.sdf[self.idx(i + a, j + b, k + c)];
        let l = |a: f32, b: f32, t: f32| a + (b - a) * t;
        l(
            l(
                l(v(0, 0, 0), v(1, 0, 0), tu),
                l(v(0, 1, 0), v(1, 1, 0), tu),
                ts,
            ),
            l(
                l(v(0, 0, 1), v(1, 0, 1), tu),
                l(v(0, 1, 1), v(1, 1, 1), tu),
                ts,
            ),
            ty,
        )
    }

    /// Distance gradient in frame space.
    fn gradient(&self, u: f32, s: f32, y: f32) -> Vec3 {
        let e = self.h * 0.75;
        Vec3::new(
            self.sample(u + e, s, y) - self.sample(u - e, s, y),
            self.sample(u, s + e, y) - self.sample(u, s - e, y),
            self.sample(u, s, y + e) - self.sample(u, s, y - e),
        ) / (2.0 * e)
    }

    /// World normal from a frame-space gradient (the frame is sheared by the scarp's wobble).
    fn world_normal(&self, u: f32, g: Vec3) -> Vec3 {
        let e = 0.5;
        let w = |u: f64| {
            let base = self.scarp.a + self.dir * u;
            -self.scarp.frame(base, self.seed).1
        };
        let dw = ((w(u as f64 + e) - w(u as f64 - e)) / (2.0 * e)) as f32;
        let du = Vec3::new(
            (self.dir.x + self.nrm.x * dw as f64) as f32,
            0.0,
            (self.dir.y + self.nrm.y * dw as f64) as f32,
        );
        let ds = Vec3::new(self.nrm.x as f32, 0.0, self.nrm.y as f32);
        let jac = Mat3::from_cols(du, ds, Vec3::Y);
        (jac.inverse().transpose() * g).normalize_or(Vec3::Y)
    }

    /// Top of the rock in the column at (u, s), if there is rock above the grid's floor.
    fn top_at(&self, u: f32, s: f32) -> Option<f32> {
        let i = ((u - self.u0) / self.h).round();
        let j = ((s - self.s0) / self.h).round();
        if i < 0.0 || j < 0.0 || i as usize >= self.nu || j as usize >= self.ns {
            return None;
        }
        let (i, j) = (i as usize, j as usize);
        for k in (0..self.ny - 1).rev() {
            let (a, b) = (self.sdf[self.idx(i, j, k)], self.sdf[self.idx(i, j, k + 1)]);
            if a <= 0.0 && b > 0.0 {
                return Some(self.y0 + (k as f32 + a / (a - b)) * self.h);
            }
        }
        None
    }

    /// Ambient occlusion from the distance field along the normal (frame-space direction).
    fn occlusion(&self, p: Vec3, dir: Vec3) -> f32 {
        let mut occ = 0.0;
        let mut w = 1.0;
        for k in 1..=5 {
            let d = k as f32 * 0.35;
            let q = p + dir * d;
            occ += w * (d - self.sample(q.x, q.y, q.z)).max(0.0);
            w *= 0.6;
        }
        (1.0 - 0.55 * occ).clamp(0.2, 1.0)
    }

    /// Mesh the whole field in chunks along the scarp.
    fn mesh_all(&self) -> Vec<CliffChunk> {
        let chunk_cells = (24.0 / self.h) as usize / 3 * 3;
        let cells = self.nu - 1;
        let ranges: Vec<(usize, usize)> = (0..cells)
            .step_by(chunk_cells)
            .map(|c0| (c0, (c0 + chunk_cells).min(cells)))
            .collect();
        let mut out: Vec<Option<CliffChunk>> = ranges.iter().map(|_| None).collect();
        par_each(&mut out, |n, slot| {
            let (c0, c1) = ranges[n];
            *slot = self.mesh_chunk(c0, c1);
        });
        out.into_iter().flatten().collect()
    }

    fn mesh_chunk(&self, c0: usize, c1: usize) -> Option<CliffChunk> {
        let um = self.u0 + (c0 + c1) as f32 * 0.5 * self.h;
        let o = self.world(um as f64, 0.0);
        let origin = DVec3::new(o.x, 0.0, o.y);
        let (near, near_attr) = self.nets(1, c0, c1, origin);
        if near.0.is_empty() {
            return None;
        }
        let (far, far_attr) = self.nets(3, c0 / 3, c1.div_ceil(3), origin);
        // Near: split wet triangles off, add the moss skin.
        let (pos, nrm, idx) = near;
        let col: Vec<Vec4> = near_attr.iter().map(|a| a.color).collect();
        let (mut dry_idx, mut wet_idx) = (Vec::new(), Vec::new());
        for t in idx.as_chunks::<3>().0 {
            let wet = t.iter().map(|&v| near_attr[v as usize].wet).sum::<f32>() / 3.0;
            if wet > 0.45 {
                wet_idx.extend_from_slice(t);
            } else {
                dry_idx.extend_from_slice(t);
            }
        }
        let uv = |p: Vec3, axis: usize| {
            let w = p + origin.as_vec3();
            let warp = Vec2::new(
                n2(w.x / 7.0, w.z / 7.0, self.seed + 70, 2),
                n2(w.z / 7.0, w.y / 7.0, self.seed + 71, 2),
            ) * 0.4;
            box_uv(w, axis) + warp
        };
        let (dry, dry_src) = box_mapped(&pos, &nrm, &col, &dry_idx, uv);
        let (wet, _) = box_mapped(&pos, &nrm, &col, &wet_idx, uv);
        let cover: Vec<f32> = near_attr.iter().map(|a| a.moss).collect();
        let tint: Vec<Vec3> = near_attr.iter().map(|a| a.moss_tint).collect();
        let moss = moss_layer(&dry, &dry_src, &cover, &tint, 0.025);
        let (fpos, fnrm, fidx) = far;
        let fcol: Vec<Vec4> = far_attr.iter().map(|a| a.color).collect();
        let (far, _) = box_mapped(&fpos, &fnrm, &fcol, &fidx, uv);
        Some(CliffChunk {
            origin,
            near: dry,
            wet,
            moss,
            far,
        })
    }

    /// Surface nets over cells with u index in [c0, c1) of the grid sampled every `st` voxels.
    /// Returns positions (relative to `origin`), normals, triangles and per-vertex shading.
    fn nets(&self, st: usize, c0: usize, c1: usize, origin: DVec3) -> (NetMesh, Vec<Shade>) {
        let (nu, ns, ny) = (
            (self.nu - 1) / st + 1,
            (self.ns - 1) / st + 1,
            (self.ny - 1) / st + 1,
        );
        let c1 = c1.min(nu - 1);
        let v = |i: usize, j: usize, k: usize| self.sdf[self.idx(i * st, j * st, k * st)];
        let lo = c0.saturating_sub(1);
        let (cs, cy) = (ns - 1, ny - 1);
        let cell_id = |i: usize, j: usize, k: usize| ((i - lo) * cs + j) * cy + k;
        let mut vert_of = vec![u32::MAX; (c1 - lo) * cs * cy];
        let mut grid_pos: Vec<Vec3> = Vec::new();
        const CORNERS: [(usize, usize, usize); 8] = [
            (0, 0, 0),
            (1, 0, 0),
            (0, 1, 0),
            (1, 1, 0),
            (0, 0, 1),
            (1, 0, 1),
            (0, 1, 1),
            (1, 1, 1),
        ];
        const EDGES: [(usize, usize); 12] = [
            (0, 1),
            (2, 3),
            (4, 5),
            (6, 7),
            (0, 2),
            (1, 3),
            (4, 6),
            (5, 7),
            (0, 4),
            (1, 5),
            (2, 6),
            (3, 7),
        ];
        for i in lo..c1 {
            for j in 0..cs {
                for k in 0..cy {
                    let vals = CORNERS.map(|(a, b, c)| v(i + a, j + b, k + c));
                    let inside = vals.iter().filter(|x| **x < 0.0).count();
                    if inside == 0 || inside == 8 {
                        continue;
                    }
                    let mut sum = Vec3::ZERO;
                    let mut n = 0.0;
                    for (a, b) in EDGES {
                        let (va, vb) = (vals[a], vals[b]);
                        if (va < 0.0) != (vb < 0.0) {
                            let t = va / (va - vb);
                            let pa = Vec3::new(
                                CORNERS[a].0 as f32,
                                CORNERS[a].1 as f32,
                                CORNERS[a].2 as f32,
                            );
                            let pb = Vec3::new(
                                CORNERS[b].0 as f32,
                                CORNERS[b].1 as f32,
                                CORNERS[b].2 as f32,
                            );
                            sum += pa + (pb - pa) * t;
                            n += 1.0;
                        }
                    }
                    vert_of[cell_id(i, j, k)] = grid_pos.len() as u32;
                    grid_pos.push(Vec3::new(i as f32, j as f32, k as f32) + sum / n);
                }
            }
        }
        // Quads around every sign-changing grid edge whose base sample is in [c0, c1).
        let mut quads: Vec<[u32; 4]> = Vec::new();
        for i in c0..c1 {
            for j in 0..ns {
                for k in 0..ny {
                    let here = v(i, j, k) < 0.0;
                    // Along u.
                    if i + 1 < nu && j >= 1 && k >= 1 && (v(i + 1, j, k) < 0.0) != here {
                        let ids = [(i, j - 1, k - 1), (i, j, k - 1), (i, j, k), (i, j - 1, k)];
                        push_quad(&mut quads, &vert_of, ids, cs, cy, &cell_id);
                    }
                    // Along s.
                    if j + 1 < ns && i >= 1 && k >= 1 && (v(i, j + 1, k) < 0.0) != here {
                        let ids = [(i - 1, j, k - 1), (i, j, k - 1), (i, j, k), (i - 1, j, k)];
                        push_quad(&mut quads, &vert_of, ids, cs, cy, &cell_id);
                    }
                    // Along y.
                    if k + 1 < ny && i >= 1 && j >= 1 && (v(i, j, k + 1) < 0.0) != here {
                        let ids = [(i - 1, j - 1, k), (i, j - 1, k), (i, j, k), (i - 1, j, k)];
                        push_quad(&mut quads, &vert_of, ids, cs, cy, &cell_id);
                    }
                }
            }
        }
        // Vertices to the world, with normals and shading.
        let fh = self.h * st as f32;
        let mut pos = Vec::with_capacity(grid_pos.len());
        let mut nrm = Vec::with_capacity(grid_pos.len());
        let mut shade = Vec::with_capacity(grid_pos.len());
        for g in &grid_pos {
            let (u, s, y) = (self.u0 + g.x * fh, self.s0 + g.y * fh, self.y0 + g.z * fh);
            let w = self.world(u as f64, s as f64);
            let world = DVec3::new(w.x, y as f64, w.y);
            let grad = self.gradient(u, s, y);
            let n = self.world_normal(u, grad);
            pos.push((world - origin).as_vec3());
            nrm.push(n);
            shade.push(self.shade(u, s, y, world, n, grad.normalize_or(Vec3::Z)));
        }
        let mut idx = Vec::with_capacity(quads.len() * 6);
        for q in quads {
            let p = q.map(|i| pos[i as usize]);
            // Split along the shorter diagonal, then face the triangles out of the rock.
            let tris = if (p[0] - p[2]).length_squared() <= (p[1] - p[3]).length_squared() {
                [[q[0], q[1], q[2]], [q[0], q[2], q[3]]]
            } else {
                [[q[0], q[1], q[3]], [q[1], q[2], q[3]]]
            };
            for t in tris {
                let [a, b, c] = t.map(|i| i as usize);
                let f = (pos[b] - pos[a]).cross(pos[c] - pos[a]);
                if f.length_squared() < 1e-12 {
                    continue;
                }
                let out = nrm[a] + nrm[b] + nrm[c];
                if f.dot(out) >= 0.0 {
                    idx.extend_from_slice(&[t[0], t[1], t[2]]);
                } else {
                    idx.extend_from_slice(&[t[0], t[2], t[1]]);
                }
            }
        }
        ((pos, nrm, idx), shade)
    }

    /// Vertex color, moss and wetness of the cliff at frame point (u, s, y).
    fn shade(&self, u: f32, s: f32, y: f32, world: DVec3, n: Vec3, gdir: Vec3) -> Shade {
        let seed = self.seed + 300;
        let i = (((u - self.u0) / self.h).round() as usize).min(self.nu - 1);
        let j = (((s - self.s0) / self.h).round() as usize).min(self.ns - 1);
        let yb = y + 0.03 * u + 0.9 * n2(u / 40.0, 0.7, self.seed + 900, 2);
        let (k, _, _) = self.strata.layer(yb);
        let mut c = self.strata.tint[k] * (0.9 + 0.2 * n2(u / 6.0, y / 3.0, seed, 3));
        let steep = 1.0 - n.y.abs();
        // Dark streaks running down the face.
        let streak = smoothstep(0.1, 0.6, n2(u * 0.8, y * 0.07 + 3.1, seed + 1, 3)) * steep;
        c *= 1.0 - 0.32 * streak;
        c = c.lerp(c * Vec3::new(0.8, 0.72, 0.62), streak * 0.5);
        // The talus is dirtier and greener toward its toe.
        let ta = self.talus[i * self.ns + j];
        let on_talus = smoothstep(0.5, -0.1, y - ta) * smoothstep(0.5, -0.5, s - self.foot[i]);
        c = c.lerp(Vec3::new(0.3, 0.27, 0.23), on_talus * 0.35);
        // Wet near the falls and the pool.
        let mut wet = 0.0f32;
        if let Some(f) = &self.falls {
            for w in &f.water {
                let dh = DVec2::new(world.x - w.x, world.z - w.z).length() as f32;
                let dy = (world.y - w.y) as f32;
                let near = (1.0 - smoothstep(3.6, 6.5, dh)) * (1.0 - smoothstep(0.6, 3.0, dy));
                wet = wet.max(near);
            }
            if let Some((pc, _)) = f.pool {
                let dp = DVec2::new(world.x - pc.x, world.z - pc.y).length() as f32;
                let spray = (1.0 - smoothstep(5.5, 10.0, dp))
                    * (1.0 - smoothstep(3.0, 7.0, (world.y - f.bottom_level) as f32));
                wet = wet.max(spray * 0.8);
            }
        }
        // Moss and grass-green on ledges, the top and damp walls by the water.
        let patch = fbm3v(world.as_vec3() / 2.5, seed + 2, 3);
        let ledge = smoothstep(0.45, 0.85, n.y + 0.3 * patch);
        let damp = wet * steep * smoothstep(-0.2, 0.3, patch) * 0.8;
        let moss = (ledge * smoothstep(-0.35, 0.2, patch) * (1.0 - on_talus * 0.6)).max(damp);
        c = c.lerp(Vec3::new(0.1, 0.13, 0.05), smoothstep(0.1, 0.6, moss) * 0.8);
        c *= 1.0 - 0.45 * wet;
        let ao = self.occlusion(Vec3::new(u, s, y), gdir);
        let tint = Vec3::new(0.32, 0.31, 0.29) * (0.8 + 0.4 * (0.5 + 0.5 * patch)) * ao.sqrt();
        Shade {
            color: (c * ao).extend(1.0),
            moss,
            moss_tint: tint,
            wet,
        }
    }

    /// Spots for talus blocks and scree on the apron at the foot of the face.
    fn talus_spots(&self, land: &Landscape) -> Vec<TalusSpot> {
        let mut rng = Rng::new(self.seed + 77);
        let mut out = Vec::new();
        let len = (self.scarp.b - self.scarp.a).length() as f32;
        let mut u = 2.0;
        while u < len - 2.0 {
            u += rng.range(0.25, 0.6);
            let i = (((u - self.u0) / self.h).round() as usize).min(self.nu - 1);
            let ht = self.height[i];
            if ht < 1.5 {
                continue;
            }
            let wt = (0.2 * ht + 0.3) / 0.62 + 0.8;
            let reach = rng.f32().powf(0.8);
            let s = self.foot[i] - 0.3 - reach * wt * 1.15;
            let Some(top) = self.top_at(u, s) else {
                continue;
            };
            let j = (((s - self.s0) / self.h).round() as usize).min(self.ns - 1);
            // Not in the stream's channel or the pool.
            if self.chan[i * self.ns + j] < top + 0.4 {
                continue;
            }
            let w = self.world(u as f64, s as f64);
            let ground = land.height(w.x, w.y) as f32;
            if top < ground - 0.3 {
                continue;
            }
            let grad = self.gradient(u, s, top);
            let wet = self.falls.as_ref().is_some_and(|f| (u - f.u).abs() < 9.0);
            out.push(TalusSpot {
                pos: DVec3::new(w.x, top.max(ground) as f64, w.y),
                normal: self.world_normal(u, grad),
                reach,
                wet,
            });
        }
        out
    }
}

/// Positions, normals and triangle indices of a surface-nets mesh.
type NetMesh = (Vec<Vec3>, Vec<Vec3>, Vec<u32>);

/// Per-vertex shading of the cliff.
#[derive(Debug, Clone, Copy)]
struct Shade {
    color: Vec4,
    moss: f32,
    moss_tint: Vec3,
    wet: f32,
}

fn push_quad(
    quads: &mut Vec<[u32; 4]>,
    vert_of: &[u32],
    cells: [(usize, usize, usize); 4],
    cs: usize,
    cy: usize,
    cell_id: &impl Fn(usize, usize, usize) -> usize,
) {
    let mut q = [0u32; 4];
    for (n, (i, j, k)) in cells.into_iter().enumerate() {
        if j >= cs || k >= cy {
            return;
        }
        let id = vert_of[cell_id(i, j, k)];
        if id == u32::MAX {
            return;
        }
        q[n] = id;
    }
    quads.push(q);
}

/// The falls of `st` over `sc`, if its waterfall is on this scarp.
fn falls_over(land: &Landscape, sc: &Scarp, st: &Stream) -> Option<Falls> {
    let (top, bottom) = st.falls()?;
    let (u, s) = sc.frame(st.path[top], land.seed);
    let len = (sc.b - sc.a).length();
    if !(0.0..=len).contains(&u) || s.abs() > 8.0 {
        return None;
    }
    let level = |i: usize| st.bed[i] + st.depth * 0.55;
    let water = (top.saturating_sub(14)..(bottom + 6).min(st.path.len()))
        .map(|i| DVec3::new(st.path[i].x, level(i), st.path[i].y))
        .collect();
    Some(Falls {
        u: u as f32,
        half_width: (st.width * 0.5) as f32,
        water,
        pool: st.pool,
        bottom_level: level(bottom),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boulders_are_closed_outward_facing_and_within_budget() {
        for (i, &(size, angular)) in VARIANTS.iter().enumerate() {
            let b = boulder_mesh(4100 + i as u64 * 37, size, angular);
            for m in [&b.near, &b.far] {
                assert!(m.indices.iter().all(|&k| (k as usize) < m.vertices.len()));
                assert!(
                    m.vertices
                        .iter()
                        .all(|v| v.position.iter().all(|c| c.is_finite()))
                );
                // Triangles face away from the rock's middle (its centroid).
                let c = m
                    .vertices
                    .iter()
                    .fold(Vec3::ZERO, |a, v| a + Vec3::from(v.position))
                    / m.vertices.len() as f32;
                let p = |k: u32| Vec3::from(m.vertices[k as usize].position);
                let outward = m
                    .indices
                    .chunks(3)
                    .filter(|t| {
                        let n = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
                        n.dot((p(t[0]) + p(t[1]) + p(t[2])) / 3.0 - c) > 0.0
                    })
                    .count();
                assert!(
                    outward * 100 > m.triangle_count() * 97,
                    "{size:?} faces inward"
                );
            }
            assert!(b.near.triangle_count() <= 20_480);
            assert!(b.far.triangle_count() * 8 <= b.near.triangle_count());
            assert!((b.extent.x.max(b.extent.z) - size.radius()).abs() < 1e-3);
            assert!(b.extent.y > 0.2 * size.radius());
        }
    }

    #[test]
    fn uncarved_height_rebuilds_the_scarp_and_fills_the_pool() {
        let land = crate::zones::landscape(7);
        let sc = land.scarps[0];
        let mut checked = 0;
        for k in 0..80 {
            let along = 5.0 + k as f64 * 1.8;
            for side in [-12.0, -6.0, -3.0, 0.0, 3.0, 6.0, 12.0, 18.0] {
                let p = sc.a + (sc.b - sc.a).normalize() * along + DVec2::new(0.0, -side);
                let a = land.height(p.x, p.y);
                let b = uncarved_height(&land, p.x, p.y);
                if land.stream_at(p.x, p.y).is_some_and(|(d, _)| d < 30.0) {
                    continue;
                }
                assert!((a - b).abs() < 1e-9, "at {p}: land {a}, uncarved {b}");
                // Where nothing else shapes the ground, the rebuilt step is the land's own
                // (past the lake's shore the basin only lifts ground below 0.9 m).
                let lake = (p - land.lake.center).length() > land.lake.radius * 1.7 && a > 1.0;
                if lake && land.pad_weight(p.x, p.y) == 0.0 && land.road_weight(p.x, p.y) == 0.0 {
                    let r = rebuilt_height(&land, p);
                    assert!((a - r).abs() < 0.01, "at {p}: land {a}, rebuilt {r}");
                    checked += 1;
                }
            }
        }
        assert!(checked > 20, "only {checked} points to check");
        // Over the plunge pool the uncarved height is well above the carved bed.
        let (pool, _) = land.streams[0].pool.unwrap();
        assert!(uncarved_height(&land, pool.x, pool.y) > land.height(pool.x, pool.y) + 2.0);
    }

    #[test]
    fn the_cliff_hides_the_ground_step_beside_the_falls() {
        // The ground's step narrows into the falls' notch as the face is worn back, and the
        // stream's channel cuts the rock; beside the water the rock must still cover the step
        // the stream hasn't carved, or it shows through as a smooth slab with a dark seam where
        // it meets the cliff.
        let land = crate::zones::landscape(7);
        let cliff = cliff_meshes(&land, CLIFF_VOXEL);
        let f = &cliff.fields[0];
        let falls = f.falls.as_ref().expect("the falls are on the first scarp");
        let mut worst = (0.0f32, 0.0f32, 0.0f32);
        for k in 0..=50 {
            let du = (falls.half_width + 2.5) + k as f32 * 0.25;
            for u in [falls.u - du, falls.u + du] {
                let i = (((u - f.u0) / f.h).round() as usize).min(f.nu - 1);
                let (low, height) = (f.low[i], f.height[i].max(1.0));
                for j in 0..=130 {
                    let s = -8.0 + j as f32 * 0.1;
                    let w = f.world(u as f64, s as f64);
                    let y = land.height(w.x, w.y);
                    let t = (y as f32 - low) / height;
                    let carved = uncarved_height(&land, w.x, w.y) - y > 0.05;
                    if (0.1..0.98).contains(&t) && !carved {
                        let air = f.sample(u, s, y as f32 + 0.05);
                        if air > worst.0 {
                            worst = (air, u - falls.u, t);
                        }
                    }
                }
            }
        }
        assert!(
            worst.0 < 0.1,
            "the ground stands {:.2} m in front of the rock {:.1} m along from the falls, {:.0}% up",
            worst.0,
            worst.1,
            worst.2 * 100.0
        );
    }

    #[test]
    fn cliff_leaves_the_falls_open_and_faces_out() {
        let land = crate::zones::landscape(7);
        let cliff = cliff_meshes(&land, 0.6);
        assert!(!cliff.chunks.is_empty());
        assert!(!cliff.talus.is_empty());
        let f = &cliff.fields[0];
        // The water's path over the lip and down to the pool is air.
        let st = &land.streams[0];
        let (top, bottom) = st.falls().unwrap();
        for i in top.saturating_sub(10)..bottom + 6 {
            let p = st.path[i];
            let level = st.bed[i] + st.depth * 0.55;
            let (u, s) = f.scarp.frame(p, land.seed);
            let d = f.sample(u as f32, s as f32, level as f32 + 0.2);
            assert!(d > 0.0, "rock in the water at path point {i} ({d})");
        }
        // Triangles face the way the distance field grows.
        for ch in &cliff.chunks {
            for m in [&ch.near, &ch.far] {
                let p = |k: u32| Vec3::from(m.vertices[k as usize].position);
                let mut agree = 0;
                for t in m.indices.chunks(3) {
                    let n = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
                    let vn = Vec3::from(m.vertices[t[0] as usize].normal);
                    if n.dot(vn) > 0.0 {
                        agree += 1;
                    }
                }
                assert!(agree * 100 >= m.triangle_count() * 98);
            }
        }
    }
}
