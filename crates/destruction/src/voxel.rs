//! Voxel cliffs, boulders and rock arches you can dig, blast and collapse.
//!
//! A [`VoxelVolume`] is a dense grid of density values (0 = air, 255 = solid) meshed with
//! surface nets, so craters are smooth bowls rather than stair steps. After every change the
//! module recomputes **structural support** the way Valheim does for building pieces, but per
//! voxel: support starts at the anchored faces (the ground) and drops a little per meter
//! climbed and a lot per meter reached sideways. Rock beyond its reach (an overhang whose base
//! was dug out, half an arch whose pillar was blasted) breaks off and falls as boulders.
//!
//! This is for destructible formations placed in a level. Digging the ground itself goes
//! through the terrain module's `TerrainEdit`; volumes here answer the same `TerrainEdit`
//! messages, so one dig tool works on both.
//!
//! Local space: `x` from `-width/2` to `width/2`, `y` from `0` up, `z` from `-depth/2` to
//! `depth/2`.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use engine_core::math::noise::{fbm2, fbm3, perlin2, perlin3};
use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::BoxFace;

use crate::material::{DestructibleMaterial, DestructionMaterials};
use crate::meshing::{MeshBuilder, box_uv, hash3, mix, scale};

/// Starting shape of a [`VoxelVolume`].
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoxelShape {
    /// Completely full box.
    Solid,
    /// A cliff face looking along −Z with ledges, a rough top and an overhang near the top
    /// that juts out `overhang` meters.
    #[default]
    Cliff,
    /// A rounded boulder.
    Boulder,
    /// Two pillars and a span: blast a pillar and half the span falls.
    Arch,
    /// Empty (fill it with `TerrainEdit` Add).
    Empty,
    /// A grassy mound that meets the ground at its rim, soil over rock (Valheim-style hill to
    /// dig into and build on).
    Hill,
}

/// A destructible voxel formation. Put it on an entity with a `Transform`; the module
/// generates, meshes and collides it, and keeps it up to date as it is dug and blasted.
/// Change any field to regenerate it.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct VoxelVolume {
    /// Voxels along local X, Y and Z.
    pub dims: [u32; 3],
    /// Edge of one voxel, meters.
    pub voxel_size: f32,
    pub material: DestructibleMaterial,
    /// Top layer material (soil on a cliff); `None` = no layer.
    pub topsoil: Option<DestructibleMaterial>,
    /// Top layer thickness, meters.
    pub topsoil_depth: f32,
    pub shape: VoxelShape,
    /// For `Cliff`: how far the top juts out over the face, meters.
    pub overhang: f32,
    /// Faces held by the world: `NegY` = resting on bedrock, `PosZ` = part of a hill.
    pub anchored: Vec<BoxFace>,
    /// Linear RGB of grass on upward-facing topsoil; `None` = bare.
    pub grass: Option<[f32; 3]>,
    pub seed: u64,
}

impl Default for VoxelVolume {
    fn default() -> Self {
        Self {
            dims: [48, 40, 28],
            voxel_size: 0.25,
            material: DestructibleMaterial::Rock,
            topsoil: Some(DestructibleMaterial::Dirt),
            topsoil_depth: 0.5,
            shape: VoxelShape::Cliff,
            overhang: 2.0,
            anchored: vec![BoxFace::NegY],
            grass: Some(crate::material::linear(0x4f6b2f)),
            seed: 3,
        }
    }
}

impl VoxelVolume {
    /// A cliff `width × height × depth` meters.
    pub fn cliff(width: f32, height: f32, depth: f32, voxel_size: f32) -> Self {
        let n = |m: f32| (m / voxel_size).round().max(2.0) as u32;
        Self {
            dims: [n(width), n(height), n(depth)],
            voxel_size,
            ..Self::default()
        }
    }

    /// A natural rock arch spanning `width` meters.
    pub fn arch(width: f32, height: f32, depth: f32, voxel_size: f32) -> Self {
        Self {
            shape: VoxelShape::Arch,
            topsoil: None,
            grass: None,
            ..Self::cliff(width, height, depth, voxel_size)
        }
    }

    /// A hill `width × height × depth` meters: soil (with grass) over rock.
    pub fn hill(width: f32, height: f32, depth: f32, voxel_size: f32) -> Self {
        Self {
            shape: VoxelShape::Hill,
            topsoil_depth: 1.4,
            overhang: 0.0,
            ..Self::cliff(width, height, depth, voxel_size)
        }
    }

    /// A boulder about `size` meters across.
    pub fn boulder(size: f32, voxel_size: f32) -> Self {
        Self {
            shape: VoxelShape::Boulder,
            topsoil: None,
            grass: None,
            ..Self::cliff(size, size * 0.8, size, voxel_size)
        }
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    pub fn with_material(mut self, material: DestructibleMaterial) -> Self {
        self.material = material;
        self
    }

    /// Size in meters.
    pub fn extent(&self) -> Vec3 {
        Vec3::new(
            self.dims[0] as f32,
            self.dims[1] as f32,
            self.dims[2] as f32,
        ) * self.voxel_size
    }
}

/// Voxels per mesh chunk edge.
pub const CHUNK: usize = 16;
/// Density at and above which a voxel counts as solid.
pub const SOLID: u8 = 128;

/// Runtime state of a [`VoxelVolume`] (managed by the module).
#[derive(Component, Debug, Clone, Default)]
pub struct VoxelState {
    pub dims: [usize; 3],
    pub size: f32,
    /// Local position of voxel (0, 0, 0)'s minimum corner.
    pub origin: Vec3,
    pub density: Vec<u8>,
    /// Material per voxel ([`DestructibleMaterial::index`]).
    pub material: Vec<u8>,
    /// 255 = weathered original surface, 0 = freshly broken.
    pub wear: Vec<u8>,
    /// Which faces are anchored: −X, +X, −Y, +Y, −Z, +Z.
    pub anchored: [bool; 6],
    pub chunks: [usize; 3],
    pub chunk_entities: Vec<Option<Entity>>,
    pub chunk_dirty: Vec<bool>,
    pub support_dirty: bool,
    pub initial_volume: f32,
    /// How well each voxel holds sideways, 0..255 around 128 (coherent noise, so rock breaks
    /// along a rough, natural surface instead of a straight cut). Empty = uniform.
    pub grain: Vec<u8>,
}

fn face_index(face: BoxFace) -> usize {
    match face {
        BoxFace::NegX => 0,
        BoxFace::PosX => 1,
        BoxFace::NegY => 2,
        BoxFace::PosY => 3,
        BoxFace::NegZ => 4,
        BoxFace::PosZ => 5,
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Signed distance-ish value (meters, positive inside) of a generated shape at local `p`.
fn shape_sdf(v: &VoxelVolume, p: Vec3) -> f32 {
    let e = v.extent();
    let (hw, h, hd) = (e.x * 0.5, e.y, e.z * 0.5);
    let seed = v.seed;
    match v.shape {
        VoxelShape::Solid => (hw - p.x.abs())
            .min(h - p.y)
            .min(hd - p.z.abs())
            .min(p.y + 1.0),
        VoxelShape::Empty => -1.0,
        VoxelShape::Hill => {
            // Height field: a broad dome with rolling bumps and a steeper rocky side (−Z),
            // falling to zero at the rim so it sits flush on the ground.
            let r = ((p.x / hw).powi(2) + (p.z / hd).powi(2)).sqrt();
            let rim = smoothstep(1.0, 0.55, r);
            let dome = (1.0 - r * r).max(0.0).powf(1.3);
            let bumps = 0.18 * fbm2(p.x as f64 * 0.09, p.z as f64 * 0.09, seed, 4, 2.0, 0.5)
                + 0.05 * fbm2(p.x as f64 * 0.5, p.z as f64 * 0.5, seed + 1, 3, 2.0, 0.5);
            let scarp = 0.25 * smoothstep(0.1, -0.6, p.z / hd) * smoothstep(0.9, 0.3, r);
            let top = h * (0.92 * dome + scarp + bumps * rim) * rim;
            (top - p.y).min(p.y + 1.0)
        }
        VoxelShape::Cliff => {
            // A rocky outcrop: tallest in the middle, sloping down into the ground at the
            // ends, with buttresses, ledges and an overhanging brow on its −Z face.
            let fade = (hw * 0.45).min(6.0);
            let ends = smoothstep(hw, hw - fade, p.x.abs());
            let top = h * (0.18 + 0.7 * ends.powf(0.7))
                + 1.6 * fbm2(p.x as f64 * 0.07, p.z as f64 * 0.1, seed, 3, 2.0, 0.5)
                + 0.5 * fbm2(p.x as f64 * 0.3, p.z as f64 * 0.3, seed + 7, 3, 2.0, 0.5);
            let strata = 0.25
                * ((p.y * 0.8 + 0.5 * perlin2(p.x as f64 * 0.15, 0.0, seed + 1)).fract() - 0.5);
            let buttress = 0.9 * (perlin2(p.x as f64 * 0.35, 0.0, seed + 5) * 1.6).clamp(-1.0, 1.0);
            let face = -hd
                + 1.6
                + 0.9 * fbm2(p.x as f64 * 0.25, p.y as f64 * 0.3, seed + 2, 4, 2.0, 0.5)
                + strata
                + buttress
                + 0.7 * smoothstep(1.8, 0.0, p.y)
                - v.overhang
                    * smoothstep(h * 0.42, h * 0.72, p.y)
                    * (1.0 - (p.x / hw).powi(2)).max(0.0)
                + 3.5 * (fade / 6.0) * (1.0 - ends);
            let back =
                hd - 0.6 - 0.5 * fbm2(p.x as f64 * 0.3, p.y as f64 * 0.3, seed + 3, 3, 2.0, 0.5);
            let rough = 0.3
                * fbm3(
                    p.x as f64 * 0.8,
                    p.y as f64 * 0.8,
                    p.z as f64 * 0.8,
                    seed + 4,
                    3,
                    2.0,
                    0.5,
                );
            (top - p.y)
                .min(p.z - face)
                .min(back - p.z)
                .min(hw - 0.3 - p.x.abs())
                + rough
        }
        VoxelShape::Boulder => {
            let c = Vec3::new(0.0, h * 0.42, 0.0);
            let r = Vec3::new(hw * 0.92, h * 0.5, hd * 0.92);
            let q = (p - c) / r;
            let n = 0.12
                * fbm3(
                    p.x as f64 * 0.8,
                    p.y as f64 * 0.8,
                    p.z as f64 * 0.8,
                    seed,
                    4,
                    2.0,
                    0.5,
                );
            (1.0 - q.length() + n) * r.min_element()
        }
        VoxelShape::Arch => {
            // A thick half-ring in the XY plane, flattened, with rough faces.
            let span = hw * 0.62;
            let c = Vec3::new(0.0, 0.0, 0.0);
            let d = Vec3::new(p.x - c.x, (p.y - c.y) * (span / (h * 0.82)), 0.0).length();
            let ring_thick = span * 0.3;
            let ring = ring_thick - (d - span).abs();
            let pillar_feet = (hw * 0.95 - p.x.abs()).min(p.y + 1.0);
            let depth = hd * 0.75 - p.z.abs();
            let n = 0.3
                * fbm3(
                    p.x as f64 * 0.6,
                    p.y as f64 * 0.6,
                    p.z as f64 * 0.6,
                    seed,
                    4,
                    2.0,
                    0.5,
                );
            ring.min(depth).min(pillar_feet).min(h - p.y) + n
        }
    }
}

impl VoxelState {
    /// Generate `volume`. Rock that would not be supported is dropped silently, so a level
    /// never collapses on load.
    pub fn new(volume: &VoxelVolume, materials: &DestructionMaterials) -> Self {
        let dims = volume.dims.map(|d| d.clamp(1, 512) as usize);
        let s = volume.voxel_size.max(0.02);
        let n = dims[0] * dims[1] * dims[2];
        let origin = Vec3::new(
            -(dims[0] as f32) * s * 0.5,
            0.0,
            -(dims[2] as f32) * s * 0.5,
        );
        let mut anchored = [false; 6];
        for f in &volume.anchored {
            anchored[face_index(*f)] = true;
        }
        let chunks = dims.map(|d| d.div_ceil(CHUNK));
        let chunk_count = chunks[0] * chunks[1] * chunks[2];
        let mut state = Self {
            dims,
            size: s,
            origin,
            density: vec![0; n],
            material: vec![volume.material.index() as u8; n],
            wear: vec![255; n],
            anchored,
            chunks,
            chunk_entities: vec![None; chunk_count],
            chunk_dirty: vec![true; chunk_count],
            support_dirty: false,
            initial_volume: 0.0,
            grain: vec![128; n],
        };
        for k in 0..dims[2] {
            for j in 0..dims[1] {
                for i in 0..dims[0] {
                    let p = state.center(i, j, k);
                    let sd = shape_sdf(volume, p);
                    let idx = state.idx(i, j, k);
                    state.density[idx] = ((0.5 + sd / s).clamp(0.0, 1.0) * 255.0).round() as u8;
                    if state.density[idx] >= SOLID {
                        let g = perlin3(
                            p.x as f64 * 0.7,
                            p.y as f64 * 0.7,
                            p.z as f64 * 0.7,
                            volume.seed + 11,
                        );
                        state.grain[idx] = (128.0 + g * 180.0).clamp(0.0, 255.0) as u8;
                    }
                }
            }
        }
        // A hill is soil over a bedrock base whose top undulates a meter or so: tunnels hold in
        // the rock, wide rooms dug into the soil above cave in.
        if volume.shape == VoxelShape::Hill {
            let soil = DestructibleMaterial::Dirt.index() as u8;
            for k in 0..dims[2] {
                for i in 0..dims[0] {
                    let c = state.center(i, 0, k);
                    let line = 1.8
                        + 1.3
                            * fbm2(
                                c.x as f64 * 0.11,
                                c.z as f64 * 0.11,
                                volume.seed + 41,
                                3,
                                2.0,
                                0.5,
                            );
                    for j in 0..dims[1] {
                        let idx = state.idx(i, j, k);
                        if state.center(i, j, k).y > line {
                            state.material[idx] = soil;
                        }
                    }
                }
            }
        }
        // Top layer: solid voxels within `topsoil_depth` of open air above.
        if let Some(top) = volume.topsoil {
            let layers = (volume.topsoil_depth / s).round() as usize;
            if layers > 0 {
                for k in 0..dims[2] {
                    for i in 0..dims[0] {
                        // Only the top surface: ledges down the face stay bare rock.
                        let mut depth = usize::MAX;
                        for j in (0..dims[1]).rev() {
                            let idx = state.idx(i, j, k);
                            if state.density[idx] >= SOLID {
                                depth = if depth == usize::MAX { 0 } else { depth + 1 };
                                if depth < layers {
                                    state.material[idx] = top.index() as u8;
                                }
                            } else if depth != usize::MAX {
                                break;
                            }
                        }
                    }
                }
            }
        }
        let _ = state.detach_unsupported(materials);
        state.chunk_dirty.iter_mut().for_each(|d| *d = true);
        state.initial_volume = state.volume();
        state
    }

    pub fn idx(&self, i: usize, j: usize, k: usize) -> usize {
        (k * self.dims[1] + j) * self.dims[0] + i
    }

    pub fn coords(&self, idx: usize) -> (usize, usize, usize) {
        let i = idx % self.dims[0];
        let j = (idx / self.dims[0]) % self.dims[1];
        let k = idx / (self.dims[0] * self.dims[1]);
        (i, j, k)
    }

    /// Local position of a voxel's center.
    pub fn center(&self, i: usize, j: usize, k: usize) -> Vec3 {
        self.origin + Vec3::new(i as f32 + 0.5, j as f32 + 0.5, k as f32 + 0.5) * self.size
    }

    /// Density at integer coordinates, with the outside rules (anchored floor continues down).
    pub fn sample(&self, i: i64, j: i64, k: i64) -> u8 {
        let [nx, ny, nz] = self.dims.map(|d| d as i64);
        if j < 0 && self.anchored[2] && (0..nx).contains(&i) && (0..nz).contains(&k) {
            return self.density[self.idx(i as usize, 0, k as usize)];
        }
        if !(0..nx).contains(&i) || !(0..ny).contains(&j) || !(0..nz).contains(&k) {
            return 0;
        }
        self.density[self.idx(i as usize, j as usize, k as usize)]
    }

    /// Local height of the rock's top under local `p`: where the density crosses solid
    /// between the highest solid voxel in `p`'s column at or below it and the voxel above
    /// (the surface the mesh draws). `p.y` itself when `p` is inside rock; `None` when the
    /// column holds no rock below `p` or `p` is outside the volume.
    pub fn surface_below(&self, p: Vec3) -> Option<f32> {
        let g = (p - self.origin) / self.size;
        let [nx, ny, nz] = self.dims.map(|d| d as i64);
        let (i, k) = (g.x.floor() as i64, g.z.floor() as i64);
        if !(0..nx).contains(&i) || !(0..nz).contains(&k) || g.y < 0.0 {
            return None;
        }
        let top = (g.y.floor() as i64).min(ny - 1);
        let (i, k) = (i as usize, k as usize);
        let d = |j: i64| self.density[self.idx(i, j as usize, k)] as f32;
        if top == g.y.floor() as i64 && d(top) >= SOLID as f32 {
            return Some(p.y);
        }
        let solid = SOLID as f32;
        for j in (0..=top).rev() {
            if d(j) >= solid {
                let above = if j + 1 < ny { d(j + 1) } else { 0.0 };
                let t = ((d(j) - solid) / (d(j) - above).max(1.0)).clamp(0.0, 1.0);
                return Some(self.center(i, j as usize, k).y + t * self.size);
            }
        }
        None
    }

    /// World bounds (min, max corners) of the volume placed by `gt`, grown by `margin` m.
    pub fn world_bounds(&self, gt: &GlobalTransform, margin: f64) -> (DVec3, DVec3) {
        let affine = gt.affine();
        let ext = Vec3::new(
            self.dims[0] as f32,
            self.dims[1] as f32,
            self.dims[2] as f32,
        ) * self.size;
        let (mut lo, mut hi) = (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY));
        for c in 0..8 {
            let corner = self.origin
                + ext * Vec3::new((c & 1) as f32, ((c >> 1) & 1) as f32, (c >> 2) as f32);
            let w = affine.transform_point3(corner.as_dvec3());
            lo = lo.min(w);
            hi = hi.max(w);
        }
        (lo - DVec3::splat(margin), hi + DVec3::splat(margin))
    }

    /// Local height where a point inside rock leaves it going straight up: where the density
    /// crosses solid above `p` in its column. `None` when `p` is not inside rock or the rock
    /// above it is thicker than `max_up` (inside a wall's side or under a ceiling, not just
    /// below a floor's top).
    pub fn exit_above(&self, p: Vec3, max_up: f32) -> Option<f32> {
        let g = (p - self.origin) / self.size;
        let [nx, ny, nz] = self.dims.map(|d| d as i64);
        let (i, mut j, k) = (g.x.floor() as i64, g.y.floor() as i64, g.z.floor() as i64);
        if !(0..nx).contains(&i) || !(0..ny).contains(&j) || !(0..nz).contains(&k) {
            return None;
        }
        let (i, k) = (i as usize, k as usize);
        let d = |j: i64| {
            if j < ny {
                self.density[self.idx(i, j as usize, k)] as f32
            } else {
                0.0
            }
        };
        let solid = SOLID as f32;
        if d(j) < solid {
            return None;
        }
        while d(j + 1) >= solid {
            j += 1;
            if self.center(i, j as usize, k).y - p.y > max_up {
                return None;
            }
        }
        let t = ((d(j) - solid) / (d(j) - d(j + 1)).max(1.0)).clamp(0.0, 1.0);
        let top = self.center(i, j as usize, k).y + t * self.size;
        (top - p.y <= max_up).then_some(top)
    }

    /// Density at local `p`, blended between the eight voxel centers around it (the surface the
    /// mesh draws is where this crosses [`SOLID`]).
    pub fn density_at(&self, p: Vec3) -> f32 {
        let g = (p - self.origin) / self.size - Vec3::splat(0.5);
        let b = g.floor();
        let f = g - b;
        let (i, j, k) = (b.x as i64, b.y as i64, b.z as i64);
        (0..8)
            .map(|c| {
                let (dx, dy, dz) = (c & 1, (c >> 1) & 1, c >> 2);
                let w = |d: i64, f: f32| if d == 1 { f } else { 1.0 - f };
                w(dx, f.x) * w(dy, f.y) * w(dz, f.z) * self.sample(i + dx, j + dy, k + dz) as f32
            })
            .sum()
    }

    /// Which way local `p`, inside rock, gets out of it soonest: toward the open voxels within
    /// two of it, the nearest by far the most (out of a wall's face, down out of a ceiling, up out
    /// of a floor; the open air just above a thin shelf outweighs the cave under it). `None`
    /// when rock fills all of them.
    pub fn outward(&self, p: Vec3) -> Option<Vec3> {
        let g = (p - self.origin) / self.size;
        let b = g.floor();
        let (i, j, k) = (b.x as i64, b.y as i64, b.z as i64);
        let mut sum = Vec3::ZERO;
        for dk in -2..=2 {
            for dj in -2..=2 {
                for di in -2..=2 {
                    if self.sample(i + di, j + dj, k + dk) >= SOLID {
                        continue;
                    }
                    let o = Vec3::new(
                        (i + di) as f32 + 0.5,
                        (j + dj) as f32 + 0.5,
                        (k + dk) as f32 + 0.5,
                    ) - g;
                    sum += o / o.length_squared().max(0.25).powi(2);
                }
            }
        }
        (sum.length_squared() > 1e-6).then(|| sum.normalize())
    }

    /// How far local `p` must move along `dir` to leave the rock: 0 outside it, `None` when
    /// that is more than `max`.
    pub fn exit_along(&self, p: Vec3, dir: Vec3, max: f32) -> Option<f32> {
        let solid = SOLID as f32;
        let step = self.size * 0.25;
        let mut t = 0.0;
        let mut d = self.density_at(p);
        while d >= solid {
            let next = self.density_at(p + dir * (t + step));
            if next < solid {
                // Where the blend crosses solid between the two samples.
                return Some(t + step * ((d - solid) / (d - next).max(1e-3)).clamp(0.0, 1.0));
            }
            t += step;
            if t > max {
                return None;
            }
            d = next;
        }
        Some(t)
    }

    pub fn solid_count(&self) -> usize {
        self.density.iter().filter(|d| **d >= SOLID).count()
    }

    /// Solid volume, m³.
    pub fn volume(&self) -> f32 {
        self.solid_count() as f32 * self.size.powi(3)
    }

    /// Fraction of the original rock left.
    pub fn integrity(&self) -> f32 {
        if self.initial_volume <= 0.0 {
            return 0.0;
        }
        (self.volume() / self.initial_volume).clamp(0.0, 1.0)
    }

    fn mark_chunk_dirty_around(&mut self, i: usize, j: usize, k: usize) {
        let c = |v: usize, n: usize, cn: usize| {
            let lo = v.saturating_sub(1) / CHUNK;
            let hi = ((v + 1).min(n - 1) / CHUNK).min(cn - 1);
            lo..=hi
        };
        for cz in c(k, self.dims[2], self.chunks[2]) {
            for cy in c(j, self.dims[1], self.chunks[1]) {
                for cx in c(i, self.dims[0], self.chunks[0]) {
                    let ci = (cz * self.chunks[1] + cy) * self.chunks[0] + cx;
                    self.chunk_dirty[ci] = true;
                }
            }
        }
    }

    /// Apply an edit described by a signed distance function (meters, positive inside the
    /// shape) over the local box `[lo, hi]`. Returns the solid volume removed (m³, negative
    /// when material was added).
    pub fn edit(&mut self, lo: Vec3, hi: Vec3, sdf: impl Fn(Vec3) -> f32, op: VoxelOp) -> f32 {
        let s = self.size;
        let to_index = |p: Vec3| ((p - self.origin) / s).floor();
        let a = to_index(lo - Vec3::splat(s));
        let b = to_index(hi + Vec3::splat(s));
        let clamp = |v: f32, n: usize| (v.max(0.0) as usize).min(n);
        let (i0, j0, k0) = (
            clamp(a.x, self.dims[0]),
            clamp(a.y, self.dims[1]),
            clamp(a.z, self.dims[2]),
        );
        let (i1, j1, k1) = (
            clamp(b.x + 1.0, self.dims[0]),
            clamp(b.y + 1.0, self.dims[1]),
            clamp(b.z + 1.0, self.dims[2]),
        );
        let before = self.solid_count_in(i0, j0, k0, i1, j1, k1);
        let mut touched = false;
        // A big smoothing brush works at a bigger scale: several passes of neighbor averaging.
        let passes = match op {
            VoxelOp::Smooth(_) => ((hi - lo).min_element() * 0.5 / (3.0 * s))
                .round()
                .clamp(1.0, 8.0) as usize,
            _ => 1,
        };
        for _ in 0..passes {
            for k in k0..k1 {
                for j in j0..j1 {
                    for i in i0..i1 {
                        let p = self.center(i, j, k);
                        let d = sdf(p);
                        // Occupancy of the shape in this voxel, 0..1.
                        let occ = (0.5 + d / s).clamp(0.0, 1.0);
                        if occ <= 0.0 && d < -s {
                            continue;
                        }
                        let idx = self.idx(i, j, k);
                        let old = self.density[idx];
                        match op {
                            VoxelOp::Remove => {
                                let new = (old as f32).min(255.0 * (1.0 - occ)).round() as u8;
                                if new != old {
                                    self.density[idx] = new;
                                    touched = true;
                                }
                                if d > -1.5 * s && old > 0 {
                                    self.wear[idx] = self.wear[idx]
                                        .min(((-d / (1.5 * s)).clamp(0.0, 1.0) * 255.0) as u8);
                                }
                            }
                            VoxelOp::Add(material) => {
                                let new = (old as f32).max(255.0 * occ).round() as u8;
                                if new != old {
                                    self.density[idx] = new;
                                    if occ > 0.3 {
                                        self.material[idx] = material.index() as u8;
                                        self.wear[idx] = 0;
                                    }
                                    touched = true;
                                }
                            }
                            VoxelOp::Smooth(strength) => {
                                if occ <= 0.0 {
                                    continue;
                                }
                                let mut sum = 0.0;
                                for (di, dj, dk) in [
                                    (-1, 0, 0),
                                    (1, 0, 0),
                                    (0, -1, 0),
                                    (0, 1, 0),
                                    (0, 0, -1),
                                    (0, 0, 1),
                                ] {
                                    sum += self.sample(i as i64 + di, j as i64 + dj, k as i64 + dk)
                                        as f32;
                                }
                                let avg = sum / 6.0;
                                let new = (old as f32
                                    + (avg - old as f32) * strength.clamp(0.0, 1.0) * occ)
                                    .round() as u8;
                                if new != old {
                                    self.density[idx] = new;
                                    touched = true;
                                }
                            }
                            VoxelOp::Paint(material) => {
                                if occ > 0.3 && old > 0 {
                                    self.material[idx] = material.index() as u8;
                                    touched = true;
                                }
                            }
                        }
                    }
                }
            }
        }
        if !touched {
            return 0.0;
        }
        let range = |a0: usize, a1: usize, n: usize, cn: usize| {
            (a0.saturating_sub(1) / CHUNK)..=(((a1 + 1).min(n) - 1) / CHUNK).min(cn - 1)
        };
        for cz in range(k0, k1, self.dims[2], self.chunks[2]) {
            for cy in range(j0, j1, self.dims[1], self.chunks[1]) {
                for cx in range(i0, i1, self.dims[0], self.chunks[0]) {
                    let ci = (cz * self.chunks[1] + cy) * self.chunks[0] + cx;
                    self.chunk_dirty[ci] = true;
                }
            }
        }
        self.support_dirty = true;
        let after = self.solid_count_in(i0, j0, k0, i1, j1, k1);
        (before as f32 - after as f32) * s.powi(3)
    }

    fn solid_count_in(
        &self,
        i0: usize,
        j0: usize,
        k0: usize,
        i1: usize,
        j1: usize,
        k1: usize,
    ) -> usize {
        let mut n = 0;
        for k in k0..k1 {
            for j in j0..j1 {
                for i in i0..i1 {
                    if self.density[self.idx(i, j, k)] >= SOLID {
                        n += 1;
                    }
                }
            }
        }
        n
    }

    /// Remove a rough sphere (a crater). Returns the volume removed, m³.
    pub fn crater(&mut self, center: Vec3, radius: f32, seed: u64) -> f32 {
        let r = radius.max(self.size * 0.3);
        let lo = center - Vec3::splat(r * 1.3);
        let hi = center + Vec3::splat(r * 1.3);
        self.edit(
            lo,
            hi,
            |p| {
                let n = fbm3(
                    p.x as f64 * 2.5,
                    p.y as f64 * 2.5,
                    p.z as f64 * 2.5,
                    seed,
                    2,
                    2.0,
                    0.5,
                );
                r * (1.0 + 0.3 * n) - p.distance(center)
            },
            VoxelOp::Remove,
        )
    }

    /// First solid voxel along a local ray: `(distance, point, normal)`.
    pub fn raycast(&self, origin: Vec3, dir: Vec3, max_dist: f32) -> Option<(f32, Vec3, Vec3)> {
        let dir = dir.normalize_or_zero();
        if dir == Vec3::ZERO {
            return None;
        }
        let s = self.size;
        let ext = Vec3::new(
            self.dims[0] as f32,
            self.dims[1] as f32,
            self.dims[2] as f32,
        ) * s;
        // Clip the ray to the volume box.
        let inv = dir.recip();
        let t0 = (self.origin - origin) * inv;
        let t1 = (self.origin + ext - origin) * inv;
        let tmin = t0.min(t1).max_element().max(0.0);
        let tmax = t0.max(t1).min_element().min(max_dist);
        if tmin > tmax {
            return None;
        }
        let start = origin + dir * (tmin + 1e-4);
        let g = (start - self.origin) / s;
        let mut cell = [g.x.floor() as i64, g.y.floor() as i64, g.z.floor() as i64];
        let step = [
            dir.x.signum() as i64,
            dir.y.signum() as i64,
            dir.z.signum() as i64,
        ];
        let next_boundary = |c: i64, st: i64| if st > 0 { (c + 1) as f32 } else { c as f32 };
        let gv = [g.x, g.y, g.z];
        let dv = [dir.x, dir.y, dir.z];
        let mut t_max = [0.0f32; 3];
        let mut t_delta = [0.0f32; 3];
        for a in 0..3 {
            if dv[a].abs() < 1e-9 {
                t_max[a] = f32::INFINITY;
                t_delta[a] = f32::INFINITY;
            } else {
                t_max[a] = (next_boundary(cell[a], step[a]) - gv[a]) / dv[a] * s;
                t_delta[a] = s / dv[a].abs();
            }
        }
        let mut normal = -dir;
        let mut t = 0.0;
        let limit = tmax - tmin;
        loop {
            let inside = (0..3).all(|a| cell[a] >= 0 && cell[a] < self.dims[a] as i64);
            if !inside || t > limit {
                return None;
            }
            let idx = self.idx(cell[0] as usize, cell[1] as usize, cell[2] as usize);
            if self.density[idx] >= SOLID {
                let dist = tmin + t;
                return Some((dist, origin + dir * dist, normal));
            }
            let a = if t_max[0] < t_max[1] {
                if t_max[0] < t_max[2] { 0 } else { 2 }
            } else if t_max[1] < t_max[2] {
                1
            } else {
                2
            };
            t = t_max[a];
            t_max[a] += t_delta[a];
            cell[a] += step[a];
            let mut n = Vec3::ZERO;
            n[a] = -step[a] as f32;
            normal = n;
        }
    }

    /// Recompute support and cut out every group of voxels that lost it. Returns the groups
    /// (voxel indices with their densities and materials), already removed from the volume.
    pub fn detach_unsupported(
        &mut self,
        materials: &DestructionMaterials,
    ) -> Vec<Vec<(usize, u8, u8)>> {
        self.support_dirty = false;
        let [nx, ny, nz] = self.dims;
        let n = nx * ny * nz;
        let s = self.size;
        let props: Vec<_> = DestructibleMaterial::ALL
            .iter()
            .map(|m| materials.get(*m))
            .collect();
        let mut best = vec![0.0f32; n];
        let mut heap = BinaryHeap::new();
        let solid = |st: &Self, idx: usize| st.density[idx] >= SOLID;
        for idx in 0..n {
            if !solid(self, idx) {
                continue;
            }
            let (i, j, k) = self.coords(idx);
            let on = [
                i == 0,
                i == nx - 1,
                j == 0,
                j == ny - 1,
                k == 0,
                k == nz - 1,
            ];
            if (0..6).any(|f| on[f] && self.anchored[f]) {
                let p = &props[self.material[idx] as usize % props.len()];
                best[idx] = p.max_support;
                heap.push(Support(p.max_support, idx));
            }
        }
        while let Some(Support(sup, idx)) = heap.pop() {
            if sup < best[idx] {
                continue;
            }
            let (i, j, k) = self.coords(idx);
            let mut relax = |nidx: usize, vertical: bool, heap: &mut BinaryHeap<Support>| {
                if self.density[nidx] < SOLID {
                    return;
                }
                let p = &props[self.material[nidx] as usize % props.len()];
                let loss = if vertical {
                    p.vertical_loss_per_m
                } else {
                    // Weaker and stronger seams: ±40 % sideways.
                    let grain = self
                        .grain
                        .get(nidx)
                        .map_or(1.0, |g| 1.4 - 0.8 * *g as f32 / 255.0);
                    p.horizontal_loss_per_m * grain
                } * s;
                // A material carries no more than it can hold (soft topsoil can't bridge rock).
                let cand = sup.min(p.max_support) - loss;
                if cand >= p.min_support && cand > best[nidx] {
                    best[nidx] = cand;
                    heap.push(Support(cand, nidx));
                }
            };
            if i > 0 {
                relax(idx - 1, false, &mut heap);
            }
            if i + 1 < nx {
                relax(idx + 1, false, &mut heap);
            }
            if j > 0 {
                relax(idx - nx, true, &mut heap);
            }
            if j + 1 < ny {
                relax(idx + nx, true, &mut heap);
            }
            if k > 0 {
                relax(idx - nx * ny, false, &mut heap);
            }
            if k + 1 < nz {
                relax(idx + nx * ny, false, &mut heap);
            }
        }
        let mut groups = Vec::new();
        let mut taken = vec![false; n];
        let mut stack = Vec::new();
        for start in 0..n {
            if taken[start] || !solid(self, start) || best[start] > 0.0 {
                continue;
            }
            let mut group = Vec::new();
            taken[start] = true;
            stack.push(start);
            while let Some(idx) = stack.pop() {
                group.push((idx, self.density[idx], self.material[idx]));
                let (i, j, k) = self.coords(idx);
                let mut push = |nidx: usize| {
                    if !taken[nidx] && self.density[nidx] >= SOLID && best[nidx] <= 0.0 {
                        taken[nidx] = true;
                        stack.push(nidx);
                    }
                };
                if i > 0 {
                    push(idx - 1);
                }
                if i + 1 < nx {
                    push(idx + 1);
                }
                if j > 0 {
                    push(idx - nx);
                }
                if j + 1 < ny {
                    push(idx + nx);
                }
                if k > 0 {
                    push(idx - nx * ny);
                }
                if k + 1 < nz {
                    push(idx + nx * ny);
                }
            }
            groups.push(group);
        }
        for group in &groups {
            for &(idx, _, _) in group {
                self.density[idx] = 0;
                let (i, j, k) = self.coords(idx);
                self.mark_chunk_dirty_around(i, j, k);
            }
        }
        groups
    }

    /// Sub-volume holding only `voxels` (taken from this volume), for a falling chunk.
    /// Returns the piece and the local position of its minimum corner.
    pub fn piece(&self, voxels: &[(usize, u8, u8)]) -> VoxelState {
        let (mut lo, mut hi) = ([usize::MAX; 3], [0usize; 3]);
        for &(idx, _, _) in voxels {
            let (i, j, k) = self.coords(idx);
            for (a, v) in [i, j, k].into_iter().enumerate() {
                lo[a] = lo[a].min(v);
                hi[a] = hi[a].max(v + 1);
            }
        }
        let dims = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
        let n = dims[0] * dims[1] * dims[2];
        let mut out = VoxelState {
            dims,
            size: self.size,
            origin: self.origin + Vec3::new(lo[0] as f32, lo[1] as f32, lo[2] as f32) * self.size,
            density: vec![0; n],
            material: vec![0; n],
            wear: vec![0; n],
            anchored: [false; 6],
            chunks: [1, 1, 1],
            chunk_entities: vec![None],
            chunk_dirty: vec![true],
            support_dirty: false,
            initial_volume: 0.0,
            grain: Vec::new(),
        };
        for &(idx, d, m) in voxels {
            let (i, j, k) = self.coords(idx);
            let o = out.idx(i - lo[0], j - lo[1], k - lo[2]);
            out.density[o] = d;
            out.material[o] = m;
            out.wear[o] = self.wear[idx];
        }
        out
    }
}

/// What an edit does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VoxelOp {
    Remove,
    Add(DestructibleMaterial),
    Smooth(f32),
    Paint(DestructibleMaterial),
}

#[derive(PartialEq)]
struct Support(f32, usize);
impl Eq for Support {}
impl PartialOrd for Support {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Support {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0).then(self.1.cmp(&other.1))
    }
}

/// Colors for meshing voxels.
#[derive(Debug, Clone)]
pub struct VoxelLook {
    /// (weathered, fresh) linear RGB per material index.
    pub colors: Vec<([f32; 3], [f32; 3])>,
    pub grass: Option<[f32; 3]>,
    pub topsoil: Option<u8>,
    pub seed: u64,
    /// Split meshes into one layer per surface (material, or [`GRASS_LAYER`]) so each layer
    /// can use its own textured material. Off: one mesh in the volume's material.
    pub layered: bool,
    /// The volume's own material index (the layer an unlayered mesh belongs to).
    pub base: u8,
}

/// Layer id of grassy topsoil in [`mesh_chunk_layers`] (other layers are material indices).
pub const GRASS_LAYER: u8 = u8::MAX;

impl VoxelLook {
    pub fn new(volume: &VoxelVolume, materials: &DestructionMaterials) -> Self {
        Self {
            colors: DestructibleMaterial::ALL
                .iter()
                .map(|m| {
                    let p = materials.get(*m);
                    (p.exterior, p.interior)
                })
                .collect(),
            grass: volume.grass,
            topsoil: volume.topsoil.map(|m| m.index() as u8),
            seed: volume.seed,
            layered: false,
            base: volume.material.index() as u8,
        }
    }

    /// The color a textured layer already has (its material's weathered color, or the grass
    /// color), which vertex colors are divided by.
    pub fn reference(&self, layer: u8) -> [f32; 3] {
        if layer == GRASS_LAYER {
            return self.grass.unwrap_or([1.0; 3]);
        }
        self.colors.get(layer as usize).map_or([1.0; 3], |c| c.0)
    }

    /// Mesh in layers (see [`VoxelLook::layered`]).
    pub fn with_layers(mut self, layered: bool) -> Self {
        self.layered = layered;
        self
    }
}

const CORNERS: [[i64; 3]; 8] = [
    [0, 0, 0],
    [1, 0, 0],
    [0, 1, 0],
    [1, 1, 0],
    [0, 0, 1],
    [1, 0, 1],
    [0, 1, 1],
    [1, 1, 1],
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

/// Surface-nets mesh of the edges whose base sample lies in `[lo, hi)` (sample coordinates).
/// Vertices are in the state's local space minus `offset`. With `look.layered` (textured
/// surfaces) it is ready for the volume's own textured material, as a falling piece needs.
pub fn mesh_region(
    state: &VoxelState,
    look: &VoxelLook,
    lo: [i64; 3],
    hi: [i64; 3],
    offset: Vec3,
) -> MeshBuilder {
    let tagged = mesh_region_tagged(state, look, lo, hi);
    let mut mb = if look.layered {
        retexture(&tagged, look, false)
            .pop()
            .map(|l| l.1)
            .unwrap_or_default()
    } else {
        tagged.mesh
    };
    // Offset last, so texture coordinates stay in volume space (a falling boulder keeps its
    // grain).
    mb.translate(-offset);
    mb
}

/// [`mesh_region`] split into layers when `look.layered` (else one layer, the volume's
/// material): each triangle goes to the surface most of its corners show.
pub fn mesh_region_layers(
    state: &VoxelState,
    look: &VoxelLook,
    lo: [i64; 3],
    hi: [i64; 3],
    offset: Vec3,
) -> Vec<(u8, MeshBuilder)> {
    let tagged = mesh_region_tagged(state, look, lo, hi);
    let mut layers = if look.layered {
        retexture(&tagged, look, true)
    } else {
        vec![(look.base, tagged.mesh)]
    };
    for (_, m) in &mut layers {
        m.translate(-offset);
    }
    layers
}

/// Prepare a voxel mesh for textured materials. Each triangle is mapped along its own face
/// normal's main axis (per-vertex box mapping would smear texture across triangles whose
/// corners pick different axes), and vertex colors become relative to their layer's own color,
/// since the texture now carries the albedo. With `split`, grass is cut out along the lawn's
/// edge (where the corners' grass cover crosses one half), so the grass layer ends in a smooth
/// line rather than in whole triangles; the bare rest goes to the material two of its corners
/// share (else the first's). Without `split`, everything is one layer in the volume's
/// material. Layers come out sorted by tag.
pub fn retexture(tagged: &Tagged, look: &VoxelLook, split: bool) -> Vec<(u8, MeshBuilder)> {
    let (mb, tags, lawn) = (&tagged.mesh, &tagged.tags, &tagged.lawn);
    let mut out: Vec<(u8, MeshBuilder, HashMap<(u32, u32, u8), u32>)> = Vec::new();
    let pos = |i: u32| Vec3::from(mb.vertices[i as usize].position);
    let grassy = |i: u32| lawn[i as usize] > 0.5;
    for t in mb.indices.as_chunks::<3>().0 {
        let face = (pos(t[1]) - pos(t[0])).cross(pos(t[2]) - pos(t[0]));
        let axis = face.abs().max_position();
        let dir = {
            let mut d = Vec3::ZERO;
            d[axis] = face[axis].signum();
            d
        };
        let code = (axis * 2) as u8 + (face[axis] < 0.0) as u8;
        // Corners as (a, b): a vertex (b = a) or the lawn's edge between two vertices.
        let mut polygons: Vec<(u8, Vec<(u32, u32)>)> = Vec::new();
        let corners: Vec<(u32, u32)> = t.iter().map(|&i| (i, i)).collect();
        let g = t.map(grassy);
        if !split {
            polygons.push((look.base, corners));
        } else if g.iter().all(|g| *g) {
            polygons.push((GRASS_LAYER, corners));
        } else if g.iter().all(|g| !*g) {
            polygons.push((majority(t.map(|i| tags[i as usize])), corners));
        } else {
            let (mut grass, mut bare) = (Vec::new(), Vec::new());
            for k in 0..3 {
                let (p, q) = (t[k], t[(k + 1) % 3]);
                if g[k] {
                    grass.push((p, p));
                } else {
                    bare.push((p, p));
                }
                if g[k] != g[(k + 1) % 3] {
                    let cut = (p.min(q), p.max(q));
                    grass.push(cut);
                    bare.push(cut);
                }
            }
            // The bare part wears its first bare corner's material.
            let tag = t
                .iter()
                .zip(g)
                .find(|(_, g)| !g)
                .map_or(look.base, |(i, _)| tags[*i as usize]);
            polygons.push((GRASS_LAYER, grass));
            polygons.push((tag, bare));
        }
        for (tag, poly) in polygons {
            let k = match out.iter().position(|o| o.0 == tag) {
                Some(k) => k,
                None => {
                    out.push((tag, MeshBuilder::new(), HashMap::new()));
                    out.len() - 1
                }
            };
            let (_, layer, remap) = &mut out[k];
            let ids: Vec<u32> = poly
                .iter()
                .map(|&(a, b)| {
                    *remap.entry((a, b, code)).or_insert_with(|| {
                        let v = corner(tagged, a, b);
                        let mut vert = mb.vertices[a as usize];
                        vert.position = v.position.to_array();
                        vert.normal = v.normal.to_array();
                        vert.uv = box_uv(v.position, dir).to_array();
                        // Grass shades grass; every other layer shows its own bare color.
                        let (color, reference) = if tag == GRASS_LAYER {
                            (v.color, look.reference(GRASS_LAYER))
                        } else {
                            let own = if split { tag } else { v.material };
                            (v.bare, look.reference(own))
                        };
                        for ch in 0..3 {
                            vert.color[ch] =
                                (color[ch] / reference[ch].max(1e-3)).clamp(0.12, 1.35);
                        }
                        layer.vertices.push(vert);
                        layer.vertices.len() as u32 - 1
                    })
                })
                .collect();
            for k in 1..ids.len() - 1 {
                layer.triangle(ids[0], ids[k], ids[k + 1]);
            }
        }
    }
    out.sort_by_key(|o| o.0);
    out.into_iter().map(|(t, m, _)| (t, m)).collect()
}

/// The material most corners show (two that agree, else the first).
fn majority([a, b, c]: [u8; 3]) -> u8 {
    if b == c { b } else { a }
}

/// A corner of a retextured triangle.
struct Corner {
    position: Vec3,
    normal: Vec3,
    color: [f32; 3],
    bare: [f32; 3],
    material: u8,
}

/// Vertex `a` (when `b == a`) or the point on edge `a`–`b` where grass cover crosses one half.
/// Neighboring triangles name a shared edge the same way round, so their cuts meet exactly.
fn corner(tagged: &Tagged, a: u32, b: u32) -> Corner {
    let at = |i: u32| {
        let v = &tagged.mesh.vertices[i as usize];
        Corner {
            position: Vec3::from(v.position),
            normal: Vec3::from(v.normal),
            color: [v.color[0], v.color[1], v.color[2]],
            bare: tagged.bare[i as usize],
            material: tagged.tags[i as usize],
        }
    };
    let (p, q) = (at(a), at(b));
    if a == b {
        return p;
    }
    let (la, lb) = (tagged.lawn[a as usize], tagged.lawn[b as usize]);
    let t = ((0.5 - la) / (lb - la)).clamp(0.0, 1.0);
    let lerp3 = |x: [f32; 3], y: [f32; 3]| std::array::from_fn(|k| x[k] + (y[k] - x[k]) * t);
    Corner {
        position: p.position.lerp(q.position, t),
        normal: p.normal.lerp(q.normal, t).normalize_or(p.normal),
        color: lerp3(p.color, q.color),
        bare: lerp3(p.bare, q.bare),
        material: if la <= 0.5 { p.material } else { q.material },
    }
}

fn mesh_region_tagged(state: &VoxelState, look: &VoxelLook, lo: [i64; 3], hi: [i64; 3]) -> Tagged {
    let s = state.size;
    let mut mb = MeshBuilder::new();
    let mut tags: Vec<u8> = Vec::new();
    let mut lawn: Vec<f32> = Vec::new();
    let mut bare_colors: Vec<[f32; 3]> = Vec::new();
    let mut cache: HashMap<[i64; 3], Option<u32>> = HashMap::new();
    let f = |p: [i64; 3]| state.sample(p[0], p[1], p[2]) as f32 - 127.5;
    let floor_anchored = state.anchored[2];
    // Sky light reaching a vertex (1 open, 0 buried), from rays through the density field:
    // tunnels, caves and overhangs go dark, which the renderer's screen-space occlusion can't
    // see from outside. `q` is in sample coordinates. Rays that hit far away count less.
    let dirs = sky_directions();
    let sky = |q: Vec3, n: Vec3| -> f32 {
        let (t1, t2) = n.any_orthonormal_pair();
        let mut open = 0.0;
        for d in &dirs {
            let dir = t1 * d.x + t2 * d.y + n * d.z;
            let hit = SKY_STEPS.iter().position(|m| {
                let p = (q + dir * (m / s)).round();
                f([p.x as i64, p.y as i64, p.z as i64]) >= 0.0
            });
            open += hit.map_or(1.0, |k| k as f32 / SKY_STEPS.len() as f32);
        }
        open / dirs.len() as f32
    };

    let mut make_vertex = |c: [i64; 3], mb: &mut MeshBuilder| -> Option<u32> {
        if let Some(v) = cache.get(&c) {
            return *v;
        }
        let vals: [f32; 8] = CORNERS.map(|o| f([c[0] + o[0], c[1] + o[1], c[2] + o[2]]));
        let mut sum = Vec3::ZERO;
        let mut count = 0.0;
        for (a, b) in EDGES {
            let (fa, fb) = (vals[a], vals[b]);
            if (fa >= 0.0) != (fb >= 0.0) {
                let t = fa / (fa - fb);
                let pa = Vec3::new(
                    CORNERS[a][0] as f32,
                    CORNERS[a][1] as f32,
                    CORNERS[a][2] as f32,
                );
                let pb = Vec3::new(
                    CORNERS[b][0] as f32,
                    CORNERS[b][1] as f32,
                    CORNERS[b][2] as f32,
                );
                sum += pa + (pb - pa) * t;
                count += 1.0;
            }
        }
        if count == 0.0 {
            cache.insert(c, None);
            return None;
        }
        let local = sum / count;
        let pos = state.origin
            + (Vec3::new(c[0] as f32, c[1] as f32, c[2] as f32) + local + Vec3::splat(0.5)) * s;
        let g = Vec3::new(
            (vals[1] - vals[0]) + (vals[3] - vals[2]) + (vals[5] - vals[4]) + (vals[7] - vals[6]),
            (vals[2] - vals[0]) + (vals[3] - vals[1]) + (vals[6] - vals[4]) + (vals[7] - vals[5]),
            (vals[4] - vals[0]) + (vals[5] - vals[1]) + (vals[6] - vals[2]) + (vals[7] - vals[3]),
        );
        let smooth_normal = (-g).normalize_or(Vec3::Y);
        // The slope over a 4-voxel block: on a voxel slope the local normal flips between
        // step tops and risers, which would grow grass in stripes along the steps.
        let broad = {
            let mut g = Vec3::ZERO;
            for i in -1..=2 {
                for j in -1..=2 {
                    for k in -1..=2 {
                        let v = f([c[0] + i, c[1] + j, c[2] + k]);
                        g += Vec3::new(i as f32 - 0.5, j as f32 - 0.5, k as f32 - 0.5) * v;
                    }
                }
            }
            (-g).normalize_or(smooth_normal)
        };
        // Fine relief the voxel grid can't hold: chisel marks, knobs and cracks, as a
        // displacement along the normal that depends only on position (seamless between
        // chunks and on the pieces that break off).
        let relief = |p: Vec3| -> f32 {
            let q = |k: f32| {
                (
                    p.x as f64 * k as f64,
                    p.y as f64 * k as f64,
                    p.z as f64 * k as f64,
                )
            };
            let (x1, y1, z1) = q(0.9);
            let (x2, y2, z2) = q(3.1);
            0.09 * fbm3(x1, y1, z1, look.seed + 31, 3, 2.0, 0.5)
                + 0.03 * perlin3(x2, y2, z2, look.seed + 32)
        };
        let h = relief(pos);
        let pos = pos + smooth_normal * h * (s / 0.25).min(1.5);
        let normal = {
            let e = 0.04;
            let grad = Vec3::new(
                relief(pos + Vec3::X * e) - relief(pos - Vec3::X * e),
                relief(pos + Vec3::Y * e) - relief(pos - Vec3::Y * e),
                relief(pos + Vec3::Z * e) - relief(pos - Vec3::Z * e),
            ) / (2.0 * e);
            let tangential = grad - smooth_normal * grad.dot(smooth_normal);
            (smooth_normal - tangential * 1.3).normalize_or(smooth_normal)
        };
        // Color from the most solid corner.
        let (mut best, mut best_v) = (0usize, f32::NEG_INFINITY);
        for (k, v) in vals.iter().enumerate() {
            if *v > best_v {
                best_v = *v;
                best = k;
            }
        }
        let bc = [
            (c[0] + CORNERS[best][0]).clamp(0, state.dims[0] as i64 - 1) as usize,
            (c[1] + CORNERS[best][1]).clamp(0, state.dims[1] as i64 - 1) as usize,
            (c[2] + CORNERS[best][2]).clamp(0, state.dims[2] as i64 - 1) as usize,
        ];
        let vidx = state.idx(bc[0], bc[1], bc[2]);
        let mat = state.material[vidx];
        let wear = state.wear[vidx] as f32 / 255.0;
        let (outside, inside) = look.colors[mat as usize % look.colors.len()];
        let wp = pos;
        let strata = 0.86
            + 0.12
                * perlin2(
                    wp.y as f64 * 1.3
                        + 0.5 * perlin2(wp.x as f64 * 0.15, wp.z as f64 * 0.15, look.seed) as f64,
                    0.0,
                    look.seed + 9,
                )
            + 0.07
                * perlin2(
                    wp.x as f64 * 2.2 + wp.z as f64 * 1.3,
                    wp.y as f64 * 0.25,
                    look.seed + 4,
                )
            + 0.08 * (hash3(bc[0] as i64, bc[1] as i64, bc[2] as i64, look.seed) - 0.5);
        let bare = mix(scale(inside, 0.97), scale(outside, strata), wear);
        let mut color = bare;
        // Grass by the broad slope (not the relief's) plus noise, so it grows in clean
        // patches rather than flickering where a slope sits near the limit.
        let (x, z) = (pos.x as f64, pos.z as f64);
        let patch = 0.16 * perlin2(x * 0.18, z * 0.18, look.seed + 7)
            + 0.06 * perlin2(x * 0.9, z * 0.9, look.seed + 8);
        let slope = broad.y + patch;
        let mut cover = 0.0;
        if let (Some(grass), Some(top)) = (look.grass, look.topsoil)
            && mat == top
            && slope > 0.45
        {
            cover = ((slope - 0.45) / 0.35).clamp(0.0, 1.0) * wear;
            let var = 0.85 + 0.3 * hash3(bc[0] as i64, 0, bc[2] as i64, look.seed + 5);
            color = mix(color, scale(grass, var), cover);
        }
        // Crevices facing down are a little darker, and buried places much darker.
        let open = sky(
            Vec3::new(c[0] as f32, c[1] as f32, c[2] as f32) + local,
            broad,
        );
        let shade = (0.82 + 0.18 * (normal.y * 0.5 + 0.5)) * (0.1 + 0.9 * open * open);
        let id = mb.vertex(pos, normal, scale(color, shade));
        tags.push(mat);
        lawn.push(cover);
        bare_colors.push(scale(bare, shade));
        cache.insert(c, Some(id));
        Some(id)
    };

    for axis in 0..3 {
        let (b, c) = ((axis + 1) % 3, (axis + 2) % 3);
        for z in lo[2]..hi[2] {
            for y in lo[1]..hi[1] {
                for x in lo[0]..hi[0] {
                    let p0 = [x, y, z];
                    if floor_anchored && axis != 1 && y < 0 {
                        continue;
                    }
                    let mut p1 = p0;
                    p1[axis] += 1;
                    let (f0, f1) = (f(p0), f(p1));
                    if (f0 >= 0.0) == (f1 >= 0.0) {
                        continue;
                    }
                    let mut cubes = [p0; 4];
                    cubes[1][b] -= 1;
                    cubes[2][b] -= 1;
                    cubes[2][c] -= 1;
                    cubes[3][c] -= 1;
                    let mut ids = [0u32; 4];
                    let mut ok = true;
                    for (k, cube) in cubes.iter().enumerate() {
                        match make_vertex(*cube, &mut mb) {
                            Some(id) => ids[k] = id,
                            None => ok = false,
                        }
                    }
                    if !ok {
                        continue;
                    }
                    let mut out = Vec3::ZERO;
                    out[axis] = if f0 >= 0.0 { 1.0 } else { -1.0 };
                    let pos = |id: u32| Vec3::from(mb.vertices[id as usize].position);
                    let n = (pos(ids[1]) - pos(ids[0])).cross(pos(ids[2]) - pos(ids[0]));
                    if n.dot(out) < 0.0 {
                        ids.reverse();
                    }
                    // Split along the shorter diagonal.
                    if pos(ids[0]).distance_squared(pos(ids[2]))
                        <= pos(ids[1]).distance_squared(pos(ids[3]))
                    {
                        mb.triangle(ids[0], ids[1], ids[2]);
                        mb.triangle(ids[0], ids[2], ids[3]);
                    } else {
                        mb.triangle(ids[0], ids[1], ids[3]);
                        mb.triangle(ids[1], ids[2], ids[3]);
                    }
                }
            }
        }
    }
    Tagged {
        mesh: mb,
        tags,
        lawn,
        bare: bare_colors,
    }
}

/// A voxel mesh before texturing: each vertex's material, grass cover (0..1, grass above one
/// half) and color without grass.
pub struct Tagged {
    pub mesh: MeshBuilder,
    pub tags: Vec<u8>,
    pub lawn: Vec<f32>,
    pub bare: Vec<[f32; 3]>,
}

/// How far sky rays look, meters.
const SKY_STEPS: [f32; 6] = [0.45, 0.9, 1.6, 2.6, 4.0, 6.0];

/// Cosine-weighted ray directions around +Z (a golden-angle spiral).
fn sky_directions() -> Vec<Vec3> {
    const N: usize = 14;
    (0..N)
        .map(|i| {
            let u = (i as f32 + 0.5) / N as f32;
            let (r, a) = (u.sqrt(), i as f32 * 2.399_963);
            Vec3::new(r * a.cos(), r * a.sin(), (1.0 - u).sqrt())
        })
        .collect()
}

/// Mesh of one chunk of a volume.
pub fn mesh_chunk(state: &VoxelState, look: &VoxelLook, chunk: [usize; 3]) -> MeshBuilder {
    let (lo, hi) = chunk_bounds(state, chunk);
    mesh_region(state, look, lo, hi, Vec3::ZERO)
}

/// Mesh of one chunk of a volume in layers (see [`mesh_region_layers`]).
pub fn mesh_chunk_layers(
    state: &VoxelState,
    look: &VoxelLook,
    chunk: [usize; 3],
) -> Vec<(u8, MeshBuilder)> {
    let (lo, hi) = chunk_bounds(state, chunk);
    mesh_region_layers(state, look, lo, hi, Vec3::ZERO)
}

fn chunk_bounds(state: &VoxelState, chunk: [usize; 3]) -> ([i64; 3], [i64; 3]) {
    let mut lo = [0i64; 3];
    let mut hi = [0i64; 3];
    for a in 0..3 {
        lo[a] = (chunk[a] * CHUNK) as i64;
        hi[a] = (((chunk[a] + 1) * CHUNK).min(state.dims[a])) as i64;
        if chunk[a] == 0 {
            lo[a] = -1;
        }
    }
    (lo, hi)
}

/// Mesh of a whole (small) volume, e.g. a falling piece, offset by `-center`.
pub fn mesh_all(state: &VoxelState, look: &VoxelLook, center: Vec3) -> MeshBuilder {
    let hi = state.dims.map(|d| d as i64);
    mesh_region(state, look, [-1, -1, -1], hi, center)
}

/// World height of the rock surface a point resting on rock touches, for things that land on
/// rock as on the ground: the rock top under it, or, when it is inside rock, where it leaves
/// the rock going up, at most `max_up` above it (deeper, it is inside a wall or under a
/// ceiling: no contact).
pub fn rock_contact(
    volumes: &[(&GlobalTransform, &VoxelState)],
    p: DVec3,
    max_up: f32,
) -> Option<f64> {
    volumes
        .iter()
        .filter_map(|(gt, state)| {
            let affine = gt.affine();
            let local = affine.inverse().transform_point3(p).as_vec3();
            let h = match state.exit_above(local, f32::INFINITY) {
                Some(top) if top - local.y <= max_up => top,
                Some(_) => return None,
                None => state.surface_below(local)?,
            };
            Some(
                affine
                    .transform_point3(Vec3::new(local.x, h, local.z).as_dvec3())
                    .y,
            )
        })
        .reduce(f64::max)
}

/// How deep `p` is inside rock among `volumes`: how far it must move to leave it, up out of
/// a floor or out of the rock's open side (a wall's face, a ceiling's underside); 0 outside.
pub fn rock_depth(volumes: &[(&GlobalTransform, &VoxelState)], p: DVec3) -> f32 {
    volumes
        .iter()
        .filter_map(|(gt, state)| {
            let affine = gt.affine();
            let local = affine.inverse().transform_point3(p).as_vec3();
            let up = state.exit_above(local, f32::INFINITY).map(|top| {
                let world = affine.transform_point3(Vec3::new(local.x, top, local.z).as_dvec3());
                (world.y - p.y) as f32
            });
            let reach = up.map_or(DEPTH_REACH_M, |a| a.min(DEPTH_REACH_M));
            let out = state
                .outward(local)
                .and_then(|dir| state.exit_along(local, dir, reach));
            match (up, out) {
                (_, Some(b)) => Some(up.map_or(b, |a| a.min(b))),
                (Some(a), None) => Some(a),
                // In rock with no way out within reach (deep in a volume's anchored bedrock).
                (None, None) => (state.density_at(local) >= SOLID as f32).then_some(reach),
            }
        })
        .fold(0.0, f32::max)
}

/// How far [`rock_depth`] looks along the rock's outward direction, m: deeper counts as this.
const DEPTH_REACH_M: f32 = 2.0;

/// Whether `p` is inside rock among `volumes`.
pub fn in_rock(volumes: &[(&GlobalTransform, &VoxelState)], p: DVec3) -> bool {
    volumes.iter().any(|(gt, state)| {
        let local = gt.affine().inverse().transform_point3(p).as_vec3();
        state.density_at(local) >= SOLID as f32
    })
}

/// Rock's outward direction above which a point inside rock is in a floor (it rises out of
/// it) rather than in a wall's side or a ceiling (it is pushed out sideways or down).
const FLOOR_UP: f32 = 0.5;

/// How rock pushes a point inside a wall's side or a ceiling out of it: the rock's outward
/// direction (world) and how far along it the point is inside. `None` outside rock, in a floor
/// (the rock opens upward, or the way up is no longer than the way out: it rises out of that,
/// see [`rock_contact`]), or deeper than `reach`.
pub fn rock_side(
    volumes: &[(&GlobalTransform, &VoxelState)],
    p: DVec3,
    reach: f32,
) -> Option<(Vec3, f32)> {
    volumes
        .iter()
        .filter_map(|(gt, state)| {
            let affine = gt.affine();
            let local = affine.inverse().transform_point3(p).as_vec3();
            if state.density_at(local) < SOLID as f32 {
                return None;
            }
            let dir = state.outward(local)?;
            if dir.y >= FLOOR_UP {
                return None;
            }
            let depth = state.exit_along(local, dir, reach)?;
            // Just under a floor's top (a thin shelf, a ledge's rim): it rises out of that.
            if state
                .exit_above(local, depth)
                .is_some_and(|top| top - local.y <= depth)
            {
                return None;
            }
            let world = affine.transform_vector3(dir.as_dvec3()).as_vec3();
            Some((world.normalize_or(dir), depth))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
}

/// World height of the highest rock top under `p` among `volumes` (see
/// [`VoxelState::surface_below`]), for things that stand on rock as on the ground.
pub fn rock_below(volumes: &[(&GlobalTransform, &VoxelState)], p: DVec3) -> Option<f64> {
    volumes
        .iter()
        .filter_map(|(gt, state)| {
            let affine = gt.affine();
            let local = affine.inverse().transform_point3(p).as_vec3();
            let h = state.surface_below(local)?;
            Some(
                affine
                    .transform_point3(Vec3::new(local.x, h, local.z).as_dvec3())
                    .y,
            )
        })
        .reduce(f64::max)
}

/// Local rotation-aware box SDF helper for `TerrainEdit` boxes (positive inside).
pub fn box_sdf(p: Vec3, center: Vec3, half: Vec3, rotation: Quat) -> f32 {
    let q = rotation.inverse() * (p - center);
    let d = q.abs() - half;
    -(d.max(Vec3::ZERO).length() + d.max_element().min(0.0))
}

/// Capsule SDF from `a` to `b` (positive inside).
pub fn capsule_sdf(p: Vec3, a: Vec3, b: Vec3, r: f32) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
    r - p.distance(a + ab * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mats() -> DestructionMaterials {
        DestructionMaterials::default()
    }

    #[test]
    fn cliff_generates_stable_and_meshes() {
        let v = VoxelVolume::cliff(10.0, 8.0, 6.0, 0.3);
        let mut s = VoxelState::new(&v, &mats());
        assert!(s.solid_count() > 1000, "{}", s.solid_count());
        assert!(
            s.detach_unsupported(&mats()).is_empty(),
            "a fresh cliff stands"
        );
        let look = VoxelLook::new(&v, &mats());
        let mut tris = 0;
        for cz in 0..s.chunks[2] {
            for cy in 0..s.chunks[1] {
                for cx in 0..s.chunks[0] {
                    let m = mesh_chunk(&s, &look, [cx, cy, cz]);
                    for vtx in &m.vertices {
                        assert!(vtx.position.iter().all(|c| c.is_finite()));
                    }
                    tris += m.indices.len() / 3;
                }
            }
        }
        assert!(tris > 500, "{tris} triangles");
    }

    #[test]
    fn layered_hill_meshes_split_into_rock_soil_and_grass() {
        let v = VoxelVolume::hill(12.0, 4.0, 10.0, 0.3);
        let s = VoxelState::new(&v, &mats());
        let plain = VoxelLook::new(&v, &mats());
        let layered = plain.clone().with_layers(true);
        let area = |m: &MeshBuilder| -> f32 {
            let p = |i: u32| Vec3::from(m.vertices[i as usize].position);
            m.indices
                .as_chunks::<3>()
                .0
                .iter()
                .map(|t| (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0])).length() * 0.5)
                .sum()
        };
        let (mut whole, mut split) = (0.0, 0.0);
        let mut seen = std::collections::BTreeSet::new();
        for ci in 0..s.chunk_dirty.len() {
            let c = [
                ci % s.chunks[0],
                (ci / s.chunks[0]) % s.chunks[1],
                ci / (s.chunks[0] * s.chunks[1]),
            ];
            whole += area(&mesh_chunk(&s, &plain, c));
            for (tag, m) in mesh_chunk_layers(&s, &layered, c) {
                seen.insert(tag);
                split += area(&m);
            }
        }
        assert!(
            (whole - split).abs() < whole * 1e-4,
            "cutting the lawn's edge keeps the whole surface ({whole} vs {split} m²)"
        );
        assert!(seen.contains(&GRASS_LAYER), "a grassy top ({seen:?})");
        assert!(
            seen.contains(&(DestructibleMaterial::Dirt.index() as u8)),
            "bare soil on the flanks ({seen:?})"
        );
    }

    #[test]
    fn digging_removes_rock_and_undercutting_collapses_it() {
        let v = VoxelVolume {
            dims: [24, 16, 8],
            voxel_size: 0.5,
            shape: VoxelShape::Solid,
            topsoil: None,
            ..VoxelVolume::default()
        };
        let mut s = VoxelState::new(&v, &mats());
        let before = s.volume();
        let removed = s.crater(Vec3::new(0.0, 4.0, -2.0), 1.0, 1);
        assert!(removed > 0.5, "{removed}");
        assert!(s.volume() < before);
        // Cut a slot all the way through near the bottom except for one pillar on the left:
        // the right part is now held only sideways through the top and is far too long.
        let ext = v.extent();
        s.edit(
            Vec3::new(-ext.x * 0.3, 0.0, -ext.z),
            Vec3::new(ext.x, 1.6, ext.z),
            |p| {
                box_sdf(
                    p,
                    Vec3::new(ext.x * 0.35, 0.75, 0.0),
                    Vec3::new(ext.x * 0.65, 0.8, ext.z),
                    Quat::IDENTITY,
                )
            },
            VoxelOp::Remove,
        );
        let groups = s.detach_unsupported(&mats());
        assert!(!groups.is_empty(), "the undercut block falls");
        let fallen: usize = groups.iter().map(|g| g.len()).sum();
        assert!(fallen > 50, "{fallen} voxels fell");
        let piece = s.piece(&groups[0]);
        let look = VoxelLook::new(&v, &mats());
        assert!(!mesh_all(&piece, &look, Vec3::ZERO).is_empty());
    }

    #[test]
    fn arch_loses_its_span_when_a_pillar_is_blasted() {
        let v = VoxelVolume::arch(12.0, 7.0, 3.0, 0.3);
        let mut s = VoxelState::new(&v, &mats());
        assert!(s.solid_count() > 500);
        assert!(s.detach_unsupported(&mats()).is_empty());
        // Remove the left pillar's foot.
        let ext = v.extent();
        let foot = Vec3::new(-ext.x * 0.38, 1.0, 0.0);
        s.edit(
            foot - Vec3::new(2.5, 1.5, 3.0),
            foot + Vec3::new(2.5, 1.5, 3.0),
            |p| box_sdf(p, foot, Vec3::new(2.5, 1.5, 3.0), Quat::IDENTITY),
            VoxelOp::Remove,
        );
        let groups = s.detach_unsupported(&mats());
        let fallen: usize = groups.iter().map(|g| g.len()).sum();
        assert!(fallen > 100, "only {fallen} voxels fell");
    }

    #[test]
    fn the_surface_below_is_the_rock_top_under_a_point() {
        let v = VoxelVolume {
            dims: [10, 10, 10],
            voxel_size: 0.5,
            shape: VoxelShape::Solid,
            topsoil: None,
            ..VoxelVolume::default()
        };
        let s = VoxelState::new(&v, &mats());
        let top = s.surface_below(Vec3::new(0.1, 8.0, 0.1)).unwrap();
        assert!((top - 5.0).abs() < 0.5, "{top}");
        assert_eq!(s.surface_below(Vec3::new(0.1, 2.0, 0.1)), Some(2.0));
        assert_eq!(s.surface_below(Vec3::new(9.0, 8.0, 0.1)), None);
        // Just under the top, a point leaves the rock at the top; deep inside, not within reach.
        let exit = s.exit_above(Vec3::new(0.1, top - 0.2, 0.1), 0.5).unwrap();
        assert!((exit - top).abs() < 1e-4, "{exit} vs {top}");
        assert_eq!(s.exit_above(Vec3::new(0.1, 2.0, 0.1), 0.5), None);
        assert_eq!(s.exit_above(Vec3::new(0.1, 8.0, 0.1), 0.5), None);
    }

    #[test]
    fn rock_pushes_out_of_its_nearest_face() {
        // A 5 m cube of rock: x from −2.5 to 2.5, y from 0 to 5.
        let v = VoxelVolume {
            dims: [10, 10, 10],
            voxel_size: 0.5,
            shape: VoxelShape::Solid,
            topsoil: None,
            ..VoxelVolume::default()
        };
        let s = VoxelState::new(&v, &mats());
        // Just inside the +X face, halfway up: out sideways, about 0.2 m.
        let p = Vec3::new(2.3, 2.5, 0.1);
        let dir = s.outward(p).unwrap();
        assert!(dir.x > 0.9, "{dir}");
        let depth = s.exit_along(p, dir, 1.0).unwrap();
        assert!((depth - 0.2).abs() < 0.1, "{depth}");
        // Just under the top: up.
        assert!(s.outward(Vec3::new(0.1, 4.8, 0.1)).unwrap().y > 0.9);
        // Deep inside, no open voxel nearby; outside, no depth.
        assert_eq!(s.outward(Vec3::new(0.1, 2.5, 0.1)), None);
        assert_eq!(
            s.exit_along(Vec3::new(3.0, 2.5, 0.1), Vec3::X, 1.0),
            Some(0.0)
        );
        assert_eq!(s.exit_along(p - Vec3::X, Vec3::X, 1.0), None);
        // The same block hollowed out under a roof one voxel thick: anywhere just under its top
        // a point rises out of it (a floor), it isn't pushed down through it.
        let mut s = s;
        for j in 0..9 {
            for k in 1..9 {
                for i in 1..9 {
                    let idx = s.idx(i, j, k);
                    s.density[idx] = 0;
                }
            }
        }
        let gts = GlobalTransform::default();
        let volumes = [(&gts, &s)];
        for i in 0..24 {
            for depth in [0.004, 0.02, 0.04] {
                let p = Vec3::new(-1.9 + i as f32 * 0.165, 5.0 - depth, 0.07 * i as f32 - 0.8);
                if s.density_at(p) >= SOLID as f32 {
                    assert_eq!(rock_side(&volumes, p.as_dvec3(), 0.6), None, "{p}");
                }
            }
        }
    }

    #[test]
    fn raycast_hits_the_face() {
        let v = VoxelVolume {
            dims: [10, 10, 10],
            voxel_size: 0.5,
            shape: VoxelShape::Solid,
            topsoil: None,
            ..VoxelVolume::default()
        };
        let s = VoxelState::new(&v, &mats());
        let hit = s
            .raycast(Vec3::new(0.1, 2.0, -10.0), Vec3::Z, 50.0)
            .unwrap();
        assert!((hit.1.z + 2.5).abs() < 0.3, "{:?}", hit.1);
        assert!(hit.2.z < -0.9);
        assert!(
            s.raycast(Vec3::new(0.0, 20.0, -10.0), Vec3::Z, 50.0)
                .is_none()
        );
    }
}
