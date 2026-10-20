//! True displacement for virtual geometry: a mesh plus a height (a material's height map or a
//! procedural surface) becomes real, finely tessellated geometry when its clusters are built.
//!
//! ## How a mesh is displaced
//!
//! [`tessellate`] subdivides every triangle until its edges are at most
//! [`Displacement::edge_m`] long where the displaced surface needs it, and only
//! [`Displacement::max_edge_m`] where it is smooth (an edge is split when the displaced surface
//! at its midpoint or quarter points leaves the straight edge by more than
//! [`Displacement::tolerance_m`]). Every new point moves along the interpolated normal by
//! `amplitude_m * (height - midlevel)`. Then the cluster builder turns the dense result into its
//! level-of-detail hierarchy as usual, so far away a displaced stone costs what a plain one does.
//!
//! ## No cracks
//!
//! - Points on an edge of the base mesh are made once per edge (by position, whatever UV seams
//!   or hard normals split it into several vertices), so both triangles of every edge, and both
//!   sides of every seam, get the very same points.
//! - Vertices that share a position are displaced along one direction, the average of their
//!   normals, so hard edges (a box's corners) don't tear open.
//! - Whether an edge is split, and where its points go, depends only on its two end points.
//!   Two meshes that share a border (same positions, normals and texture coordinates, such as
//!   terrain chunks or pieces of one wall built in one frame) are therefore subdivided and
//!   displaced identically along it.
//!
//! ## Normals and memory
//!
//! Normals are recomputed from the displaced surface (area weighted over every vertex at a
//! position), so shading follows the real relief; the material's normal map adds what is
//! finer than the triangles. Memory stays bounded: the edge length grows when the surface would
//! need more than [`Displacement::max_triangles`] triangles, and no denser intermediate mesh is
//! ever made.

use std::collections::HashMap;
use std::sync::Arc;

use engine_core::prelude::*;

use crate::material::{StandardMaterial, TextureMapping};
use crate::mesh::{Mesh, Vertex};
use crate::texture::{Image, PixelFormat, f16_to_f32};

/// A height over a surface, for [`Displacement`]: 0 (deepest) .. 1 (highest).
pub trait HeightField: Send + Sync {
    /// Height at mesh-space point `p` (meters) on the base surface, whose unit normal is `n`
    /// and texture coordinate `uv`.
    fn height(&self, p: Vec3, n: Vec3, uv: Vec2) -> f32;

    /// Vertex color of a displaced vertex: `base` is the base surface's color there, `h` the
    /// height. Lets a source paint at the tessellation's resolution (lichen, dirt in pits).
    /// Unchanged by default.
    fn color(&self, p: Vec3, n: Vec3, h: f32, base: Vec4) -> Vec4 {
        let _ = (p, n, h);
        base
    }
}

/// A height function in Rust code for [`DisplacementSource::Custom`]. `key` names it for the
/// build cache: give a different key whenever the function's output changes.
#[derive(Clone)]
pub struct HeightFunction {
    pub key: u64,
    pub field: Arc<dyn HeightField>,
}

impl HeightFunction {
    /// Wrap a closure `|p, n, uv| height` (0..1).
    pub fn new(key: u64, f: impl Fn(Vec3, Vec3, Vec2) -> f32 + Send + Sync + 'static) -> Self {
        struct Closure<F>(F);
        impl<F: Fn(Vec3, Vec3, Vec2) -> f32 + Send + Sync> HeightField for Closure<F> {
            fn height(&self, p: Vec3, n: Vec3, uv: Vec2) -> f32 {
                (self.0)(p, n, uv)
            }
        }
        Self {
            key,
            field: Arc::new(Closure(f)),
        }
    }
}

impl std::fmt::Debug for HeightFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeightFunction")
            .field("key", &self.key)
            .finish()
    }
}

impl PartialEq for HeightFunction {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

/// Where a [`Displacement`]'s height comes from.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub enum DisplacementSource {
    /// The material's height map (`StandardMaterial::height_texture`, or the generated one of
    /// its preset), red channel, mapped the way the material maps it (mesh UVs × `uv_scale` /
    /// `texture_scale`, or triplanar in meters, in mesh space).
    #[default]
    MaterialHeight,
    /// Tool marks and fractures of dressed building stone (see [`crate::rock::StoneFinish`]).
    Stone(crate::rock::StoneFinish),
    /// A height function in Rust code (not serialized).
    #[serde(skip)]
    Custom(HeightFunction),
}

/// Real displaced geometry for a [`MeshRenderer`](crate::MeshRenderer): with this component the
/// mesh is tessellated and displaced by a height when its virtual geometry clusters are built
/// (on a background thread, cached on disk), so a low-poly stone and a scanned or procedural
/// height become chiseled geometry with real silhouettes, self-shadowing and contact shadows.
/// Level of detail comes from the cluster hierarchy, so it costs little from afar.
///
/// The entity takes the cluster path whatever its triangle count (it needs an opaque material
/// that isn't wind-animated, and `RenderSettings::virtual_geometry`). Until its clusters are
/// built it is drawn as usual, the base mesh with the material's parallax. Drawn displaced, the
/// material's parallax is turned off (the relief is now geometry) and its normal map is kept at
/// `normal_map_strength`, so it only adds detail finer than the triangles.
///
/// Positions are in the mesh's own space and meters, before the entity's scale.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Displacement {
    /// Height range, meters: a height of 1 lies `amplitude_m * (1 - midlevel)` above the base
    /// surface and 0 lies `amplitude_m * midlevel` below it. With
    /// [`DisplacementSource::MaterialHeight`], 0 takes the material's `parallax_depth`.
    pub amplitude_m: f32,
    /// Height that stays on the base surface (0.5: half out, half in; 1: carved inward only,
    /// like parallax mapping).
    pub midlevel: f32,
    /// Shortest edges the tessellation makes where the relief needs them, meters (2–5 mm on a
    /// 1 m stone).
    pub edge_m: f32,
    /// Longest edges it leaves where the displaced surface is flat, meters. Relief narrower
    /// than about a quarter of this can be missed.
    pub max_edge_m: f32,
    /// How far, meters, the displaced surface may stray from a straight edge before the edge
    /// is split.
    pub tolerance_m: f32,
    /// Most triangles the displaced mesh may have; edges grow longer to stay under it.
    pub max_triangles: u32,
    /// How much pits darken the vertex color, 0..1 (dirt and shade in the deepest relief).
    pub cavity_darkening: f32,
    /// Strength of the material's normal map on the displaced surface, 0..1 (the relief it
    /// shows is mostly geometry now).
    pub normal_map_strength: f32,
    pub source: DisplacementSource,
}

impl Default for Displacement {
    fn default() -> Self {
        Self {
            amplitude_m: 0.0,
            midlevel: 0.5,
            edge_m: 0.004,
            max_edge_m: 0.016,
            tolerance_m: 0.0003,
            max_triangles: 1_500_000,
            cavity_darkening: 0.0,
            normal_map_strength: 0.5,
            source: DisplacementSource::MaterialHeight,
        }
    }
}

impl Displacement {
    /// Displace by the material's height map, `amplitude_m` deep (0: its `parallax_depth`),
    /// carving into the surface the way parallax mapping shows it.
    pub fn material_height(amplitude_m: f32) -> Self {
        Self {
            amplitude_m,
            midlevel: 1.0,
            ..Default::default()
        }
    }

    /// Displace by a height function in code, `amplitude_m` deep (heights 0..1, 0.5 stays on
    /// the surface). `key` names the function for the build cache.
    pub fn function(
        amplitude_m: f32,
        key: u64,
        f: impl Fn(Vec3, Vec3, Vec2) -> f32 + Send + Sync + 'static,
    ) -> Self {
        Self {
            amplitude_m,
            source: DisplacementSource::Custom(HeightFunction::new(key, f)),
            ..Default::default()
        }
    }

    /// Finest and coarsest edge lengths, meters.
    pub fn with_edges(mut self, edge_m: f32, max_edge_m: f32) -> Self {
        self.edge_m = edge_m;
        self.max_edge_m = max_edge_m;
        self
    }

    pub fn with_tolerance(mut self, tolerance_m: f32) -> Self {
        self.tolerance_m = tolerance_m;
        self
    }

    pub fn with_max_triangles(mut self, max_triangles: u32) -> Self {
        self.max_triangles = max_triangles;
        self
    }

    pub fn with_cavity_darkening(mut self, amount: f32) -> Self {
        self.cavity_darkening = amount;
        self
    }

    /// The material as drawn on the displaced surface: parallax off, normal map weakened.
    pub fn adjust_material(&self, material: &StandardMaterial) -> StandardMaterial {
        let mut m = *material;
        m.parallax_depth = 0.0;
        m.normal_scale *= self.normal_map_strength.clamp(0.0, 1.0);
        m
    }

    /// Amplitude actually used with `material` (its parallax depth when this is 0 and the
    /// height is the material's).
    pub fn amplitude_for(&self, material: &StandardMaterial) -> f32 {
        if self.amplitude_m <= 0.0 && self.source == DisplacementSource::MaterialHeight {
            material.parallax_depth.max(0.0)
        } else {
            self.amplitude_m.max(0.0)
        }
    }

    /// Hash of every setting that changes the displaced geometry (not the source's own data).
    pub(crate) fn settings_key(&self) -> u64 {
        let mut h = StableHasher::new(SETTINGS_SEED);
        for v in [
            self.amplitude_m,
            self.midlevel,
            self.edge_m,
            self.max_edge_m,
            self.tolerance_m,
            self.cavity_darkening,
        ] {
            h.write_u32(v.to_bits());
        }
        h.write_u32(self.max_triangles);
        match &self.source {
            DisplacementSource::MaterialHeight => h.write_u32(1),
            DisplacementSource::Stone(s) => {
                h.write_u32(2);
                h.write_u64(s.key());
            }
            DisplacementSource::Custom(f) => {
                h.write_u32(3);
                h.write_u64(f.key);
            }
        }
        h.finish()
    }
}

/// Seed of [`Displacement::settings_key`].
const SETTINGS_SEED: u64 = 0x5eed_d15b_1ace_3e47;

/// A small, fast, stable (same on every platform and Rust version) 64-bit hash for cache keys.
#[derive(Debug, Clone, Copy)]
pub struct StableHasher {
    a: u64,
    b: u64,
    len: u64,
}

impl StableHasher {
    pub fn new(seed: u64) -> Self {
        Self {
            a: seed ^ 0x243f_6a88_85a3_08d3,
            b: seed.rotate_left(32) ^ 0x1319_8a2e_0370_7344,
            len: 0,
        }
    }

    pub fn write_u64(&mut self, w: u64) {
        self.a = (self.a ^ w)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            .rotate_left(29);
        self.b = (self.b.wrapping_add(w))
            .wrapping_mul(0xc2b2_ae3d_27d4_eb4f)
            .rotate_left(31)
            ^ self.a;
        self.len += 8;
    }

    pub fn write_u32(&mut self, w: u32) {
        self.write_u64(w as u64 | 0xa5a5_0000_0000_0000);
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) {
        let (words, rest) = bytes.as_chunks::<8>();
        for w in words {
            self.write_u64(u64::from_le_bytes(*w));
        }
        let mut tail = [0u8; 8];
        tail[..rest.len()].copy_from_slice(rest);
        self.write_u64(u64::from_le_bytes(tail) ^ ((rest.len() as u64) << 56));
    }

    /// 64-bit result.
    pub fn finish(&self) -> u64 {
        let (lo, hi) = self.finish128();
        lo ^ hi.rotate_left(17)
    }

    /// 128-bit result (two 64-bit halves), for cache file names.
    pub fn finish128(&self) -> (u64, u64) {
        let mix = |mut x: u64| {
            x ^= x >> 33;
            x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
            x ^= x >> 33;
            x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
            x ^ (x >> 33)
        };
        let a = mix(self.a ^ self.len);
        let b = mix(self.b.wrapping_add(a));
        (mix(a ^ b.rotate_left(23)), b)
    }
}

/// A height map on the CPU: one channel, bilinear, wrapping. Made from an [`Image`]'s red
/// channel at the resolution displacement can use.
#[derive(Debug, Clone, PartialEq)]
pub struct HeightImage {
    pub width: u32,
    pub height: u32,
    /// Row-major heights, 0..1.
    pub texels: Vec<f32>,
}

impl HeightImage {
    /// The red channel of `image` at the first mip level no larger than `max_size` texels per
    /// side (level 0 when it has no mips). None when its pixels were released.
    pub fn from_image(image: &Image, max_size: u32) -> Option<Self> {
        if image.is_released() || !image.is_valid() {
            return None;
        }
        let mut level = 0u32;
        while (level as usize) < image.mips.len() {
            let (w, h) = image.level_size(level);
            if w.max(h) <= max_size.max(1) {
                break;
            }
            level += 1;
        }
        let (w, h) = image.level_size(level);
        let data = if level == 0 {
            &image.data
        } else {
            &image.mips[level as usize - 1]
        };
        let bpp = image.format.bytes_per_pixel();
        if data.len() < w as usize * h as usize * bpp {
            return None;
        }
        let texels = (0..(w * h) as usize)
            .map(|i| match image.format {
                PixelFormat::Rgba8 => data[i * bpp] as f32 / 255.0,
                PixelFormat::Rgba16Float => {
                    f16_to_f32(u16::from_le_bytes([data[i * bpp], data[i * bpp + 1]]))
                        .clamp(0.0, 1.0)
                }
            })
            .collect();
        Some(Self {
            width: w,
            height: h,
            texels,
        })
    }

    /// Bilinear sample at texture coordinate `uv` (1 = one tile), wrapping.
    pub fn sample(&self, uv: Vec2) -> f32 {
        let (w, h) = (self.width as i64, self.height as i64);
        let x = uv.x as f64 * w as f64 - 0.5;
        let y = uv.y as f64 * h as f64 - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = ((x - x0) as f32, (y - y0) as f32);
        let at = |xi: i64, yi: i64| self.texels[(yi.rem_euclid(h) * w + xi.rem_euclid(w)) as usize];
        let (xi, yi) = (x0 as i64, y0 as i64);
        let top = at(xi, yi) + (at(xi + 1, yi) - at(xi, yi)) * fx;
        let bottom = at(xi, yi + 1) + (at(xi + 1, yi + 1) - at(xi, yi + 1)) * fx;
        top + (bottom - top) * fy
    }

    /// Hash of the heights (for the build cache).
    pub fn key(&self) -> u64 {
        let mut h = StableHasher::new(0x4e16_47a1);
        h.write_u32(self.width);
        h.write_u32(self.height);
        h.write_bytes(bytemuck::cast_slice(&self.texels));
        h.finish()
    }
}

/// Sharpness of the triplanar blend (`TRIPLANAR_SHARPNESS` in `material.rs`).
const TRIPLANAR_SHARPNESS: f32 = 6.0;

/// A material's height map laid on a mesh the way the material maps its textures.
#[derive(Debug, Clone)]
pub struct MaterialHeightField {
    pub image: Arc<HeightImage>,
    pub mapping: TextureMapping,
    /// `uv_scale` (UV mapping).
    pub uv_scale: Vec2,
    /// Tiles per meter (1 / `texture_scale`).
    pub tiles_per_m: f32,
}

impl MaterialHeightField {
    pub fn new(image: Arc<HeightImage>, material: &StandardMaterial) -> Self {
        Self {
            image,
            mapping: material.mapping,
            uv_scale: Vec2::from(material.uv_scale),
            tiles_per_m: 1.0 / material.texture_scale.max(1e-4),
        }
    }

    /// Hash of the image and its mapping (for the build cache).
    pub fn key(&self) -> u64 {
        let mut h = StableHasher::new(self.image.key());
        h.write_u32(self.mapping as u32);
        h.write_u32(self.uv_scale.x.to_bits());
        h.write_u32(self.uv_scale.y.to_bits());
        h.write_u32(self.tiles_per_m.to_bits());
        h.finish()
    }
}

impl HeightField for MaterialHeightField {
    fn height(&self, p: Vec3, n: Vec3, uv: Vec2) -> f32 {
        match self.mapping {
            TextureMapping::Uv => self.image.sample(uv * self.uv_scale * self.tiles_per_m),
            // The mesh shader's triplanar projections (mesh.wgsl, `triplanar_projection`), in
            // mesh space.
            TextureMapping::Triplanar | TextureMapping::TriplanarWorld => {
                let q = p * self.tiles_per_m;
                let mut w = n.abs().powf(TRIPLANAR_SHARPNESS);
                w /= (w.x + w.y + w.z).max(1e-12);
                w = (w - Vec3::splat(0.02)).max(Vec3::ZERO);
                w /= (w.x + w.y + w.z).max(1e-6);
                let side = |c: f32| if c >= 0.0 { 1.0 } else { -1.0 };
                let mut sum = 0.0;
                if w.x > 0.0 {
                    sum += w.x * self.image.sample(Vec2::new(-side(n.x) * q.z, -q.y));
                }
                if w.y > 0.0 {
                    sum += w.y * self.image.sample(Vec2::new(side(n.y) * q.x, q.z));
                }
                if w.z > 0.0 {
                    sum += w.z * self.image.sample(Vec2::new(side(n.z) * q.x, -q.y));
                }
                sum
            }
        }
    }
}

/// Dyadic barycentric coordinates: a base edge can be halved this many times.
const DEPTH: u32 = 24;
const ONE: u32 = 1 << DEPTH;

/// Tessellation limits after the triangle budget is applied.
#[derive(Debug, Clone, Copy)]
struct Rules {
    edge: f32,
    max_edge: f32,
    tolerance: f32,
    amplitude: f32,
    midlevel: f32,
    cavity: f32,
}

/// Attributes of a point of the base surface.
#[derive(Debug, Clone, Copy)]
struct Attr {
    p: Vec3,
    /// Displacement direction (not normalized after interpolation).
    n: Vec3,
    uv: Vec2,
    color: Vec4,
}

impl Attr {
    fn blend(a: &Attr, b: &Attr, c: &Attr, w: Vec3) -> Attr {
        Attr {
            p: a.p * w.x + b.p * w.y + c.p * w.z,
            n: a.n * w.x + b.n * w.y + c.n * w.z,
            uv: a.uv * w.x + b.uv * w.y + c.uv * w.z,
            color: a.color * w.x + b.color * w.y + c.color * w.z,
        }
    }

    fn lerp(a: &Attr, b: &Attr, t: f32) -> Attr {
        Attr {
            p: a.p.lerp(b.p, t),
            n: a.n.lerp(b.n, t),
            uv: a.uv.lerp(b.uv, t),
            color: a.color.lerp(b.color, t),
        }
    }
}

/// A displaced point: where it was on the base surface, where it is, and its height.
#[derive(Debug, Clone, Copy)]
struct Sample {
    p: Vec3,
    d: Vec3,
    h: f32,
}

fn displace_point(a: &Attr, field: &dyn HeightField, rules: &Rules) -> Sample {
    let n = a.n.normalize_or_zero();
    let h = field.height(a.p, n, a.uv);
    let h = if h.is_finite() {
        h.clamp(0.0, 1.0)
    } else {
        rules.midlevel
    };
    Sample {
        p: a.p,
        d: a.p + n * (rules.amplitude * (h - rules.midlevel)),
        h,
    }
}

/// Whether the edge between `a` and `b` must be split; `eval(t)` displaces the point at `t`
/// along it. Returns the midpoint when it must. Depends only on the two end points (and the
/// field), so both triangles of an edge decide alike.
fn split_needed(
    a: &Sample,
    b: &Sample,
    rules: &Rules,
    eval: &mut dyn FnMut(f32) -> Sample,
) -> Option<Sample> {
    let base = a.p.distance(b.p);
    let len = base.max(a.d.distance(b.d));
    // Steep relief can keep a displaced edge long however short it gets on the base surface:
    // never go below half the finest edge there.
    if len <= rules.edge || base <= rules.edge * 0.5 {
        return None;
    }
    let mid = eval(0.5);
    if base > rules.max_edge {
        return Some(mid);
    }
    let off = |s: &Sample, t: f32| s.d.distance(a.d.lerp(b.d, t));
    if off(&mid, 0.5) > rules.tolerance {
        return Some(mid);
    }
    for t in [0.25, 0.75] {
        let s = eval(t);
        if off(&s, t) > rules.tolerance {
            return Some(mid);
        }
    }
    None
}

/// Displaced points along one base edge (welded by position), shared by every triangle (and
/// every side of a seam) that uses it.
struct EdgePoints {
    /// Sorted by `t` (dyadic, 0..ONE from the end with the lower position id).
    points: Vec<(u32, Sample)>,
}

impl EdgePoints {
    fn get(&self, t: u32) -> Option<&Sample> {
        self.points
            .binary_search_by_key(&t, |p| p.0)
            .ok()
            .map(|i| &self.points[i].1)
    }
}

/// Tessellate `mesh` and displace it by `field` with `displacement`'s settings, `amplitude`
/// meters deep (already resolved: see [`Displacement::amplitude_for`]). Deterministic; uses up
/// to four threads. The result has smooth normals from the displaced surface.
pub fn tessellate(
    mesh: &Mesh,
    displacement: &Displacement,
    amplitude: f32,
    field: &dyn HeightField,
) -> Mesh {
    let tris: Vec<[u32; 3]> = mesh
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .copied()
        .filter(|t| t.iter().all(|&i| (i as usize) < mesh.vertices.len()))
        .collect();
    if tris.is_empty() {
        return Mesh::default();
    }

    // Weld positions; one displacement direction per position (the sum of its distinct
    // normals, so a seam's copies count once and a box corner points out diagonally).
    let mut by_position: HashMap<[u32; 3], u32> = HashMap::with_capacity(mesh.vertices.len());
    let pos_id: Vec<u32> = mesh
        .vertices
        .iter()
        .map(|v| {
            let n = by_position.len() as u32;
            *by_position.entry(bytemuck::cast(v.position)).or_insert(n)
        })
        .collect();
    let positions = by_position.len();
    drop(by_position);
    let mut dir = vec![Vec3::ZERO; positions];
    let mut seen_normal: std::collections::HashSet<(u32, [u32; 3])> =
        std::collections::HashSet::new();
    for (i, v) in mesh.vertices.iter().enumerate() {
        if seen_normal.insert((pos_id[i], bytemuck::cast(v.normal))) {
            dir[pos_id[i] as usize] += Vec3::from(v.normal);
        }
    }
    drop(seen_normal);
    // Positions without usable normals take the faces around them.
    let mut area = 0.0f32;
    let mut face_dir = vec![Vec3::ZERO; positions];
    for t in &tris {
        let [a, b, c] = t.map(|i| Vec3::from(mesh.vertices[i as usize].position));
        let n = (b - a).cross(c - a);
        area += n.length() * 0.5;
        for &i in t {
            face_dir[pos_id[i as usize] as usize] += n;
        }
    }
    for (d, f) in dir.iter_mut().zip(&face_dir) {
        *d = d.try_normalize().unwrap_or(f.normalize_or(Vec3::Y));
    }
    drop(face_dir);

    // Edges grow to keep the worst case (every edge at the finest length, relief adding half
    // the area again) within the budget.
    let budget = displacement.max_triangles.max(1000) as f32;
    let edge = displacement
        .edge_m
        .max(1e-5)
        .max((1.5 * area / (0.21 * budget)).sqrt());
    let rules = Rules {
        edge,
        max_edge: displacement.max_edge_m.max(edge),
        tolerance: displacement.tolerance_m.max(1e-7),
        amplitude,
        midlevel: displacement.midlevel,
        cavity: displacement.cavity_darkening.clamp(0.0, 1.0),
    };
    let attr = |v: u32| {
        let x = &mesh.vertices[v as usize];
        Attr {
            p: Vec3::from(x.position),
            n: dir[pos_id[v as usize] as usize],
            uv: Vec2::from(x.uv),
            color: Vec4::from(x.color),
        }
    };

    // Corners: one sample per position, from its first vertex.
    let mut corner_vertex = vec![u32::MAX; positions];
    for (i, &p) in pos_id.iter().enumerate() {
        if corner_vertex[p as usize] == u32::MAX {
            corner_vertex[p as usize] = i as u32;
        }
    }
    let corners: Vec<Sample> = corner_vertex
        .iter()
        .map(|&v| displace_point(&attr(v), field, &rules))
        .collect();

    // Base edges by position, with the vertices of their first use (from the lower position
    // id to the higher).
    let mut edge_index: HashMap<(u32, u32), u32> = HashMap::with_capacity(tris.len() * 3 / 2);
    let mut edge_ends: Vec<(u32, u32)> = Vec::with_capacity(tris.len() * 3 / 2);
    for t in &tris {
        for k in 0..3 {
            let (va, vb) = (t[k], t[(k + 1) % 3]);
            let (pa, pb) = (pos_id[va as usize], pos_id[vb as usize]);
            if pa == pb {
                continue;
            }
            edge_index
                .entry((pa.min(pb), pa.max(pb)))
                .or_insert_with(|| {
                    edge_ends.push(if pa < pb { (va, vb) } else { (vb, va) });
                    edge_ends.len() as u32 - 1
                });
        }
    }
    let edges: Vec<EdgePoints> = super::build::parallel_map(&edge_ends, |&(va, vb)| {
        let (x, y) = (attr(va), attr(vb));
        let ends = (
            corners[pos_id[va as usize] as usize],
            corners[pos_id[vb as usize] as usize],
        );
        let mut points = Vec::new();
        let mut stack = vec![(0u32, ends.0, ONE, ends.1)];
        while let Some((t0, s0, t1, s1)) = stack.pop() {
            if t1 - t0 < 2 {
                continue;
            }
            let mut eval = |f: f32| {
                let t = (t0 as f64 + (t1 - t0) as f64 * f as f64) / ONE as f64;
                displace_point(&Attr::lerp(&x, &y, t as f32), field, &rules)
            };
            if let Some(mid) = split_needed(&s0, &s1, &rules, &mut eval) {
                let tm = t0 + (t1 - t0) / 2;
                points.push((tm, mid));
                stack.push((t0, s0, tm, mid));
                stack.push((tm, mid, t1, s1));
            }
        }
        points.sort_unstable_by_key(|p| p.0);
        EdgePoints { points }
    });
    drop(edge_ends);

    // Every base triangle on its own (in parallel), with the shared edge and corner points.
    let ctx = TriangleContext {
        tris: &tris,
        pos_id: &pos_id,
        corners: &corners,
        edges: &edges,
        edge_index: &edge_index,
        rules,
        field,
        attr: &attr,
    };
    const CHUNK: usize = 256;
    let ranges: Vec<(usize, usize)> = (0..tris.len())
        .step_by(CHUNK)
        .map(|s| (s, (s + CHUNK).min(tris.len())))
        .collect();
    let parts = super::build::parallel_map(&ranges, |&(s, e)| {
        let mut out = Mesh::default();
        let mut scratch = TriangleScratch::default();
        for t in s..e {
            ctx.triangle(t, &mut scratch, &mut out);
        }
        out
    });
    let total_v: usize = parts.iter().map(|p| p.vertices.len()).sum();
    let total_i: usize = parts.iter().map(|p| p.indices.len()).sum();
    let mut out = Mesh {
        vertices: Vec::with_capacity(total_v),
        indices: Vec::with_capacity(total_i),
    };
    for p in parts {
        out.merge(&p);
    }
    smooth_normals(&mut out);
    out
}

/// Normals from the faces around every position (vertices at one position share one normal).
pub(crate) fn smooth_normals(mesh: &mut Mesh) {
    let mut by_position: HashMap<[u32; 3], u32> = HashMap::with_capacity(mesh.vertices.len());
    let ids: Vec<u32> = mesh
        .vertices
        .iter()
        .map(|v| {
            let n = by_position.len() as u32;
            *by_position.entry(bytemuck::cast(v.position)).or_insert(n)
        })
        .collect();
    let mut normals = vec![Vec3::ZERO; by_position.len()];
    drop(by_position);
    for t in mesh.indices.as_chunks::<3>().0 {
        let [a, b, c] = t.map(|i| Vec3::from(mesh.vertices[i as usize].position));
        let n = (b - a).cross(c - a);
        for &i in t {
            normals[ids[i as usize] as usize] += n;
        }
    }
    for (v, id) in mesh.vertices.iter_mut().zip(ids) {
        let n = normals[id as usize];
        v.normal = n.normalize_or(Vec3::from(v.normal)).to_array();
    }
}

struct TriangleContext<'a, F: Fn(u32) -> Attr> {
    tris: &'a [[u32; 3]],
    pos_id: &'a [u32],
    corners: &'a [Sample],
    edges: &'a [EdgePoints],
    edge_index: &'a HashMap<(u32, u32), u32>,
    rules: Rules,
    field: &'a dyn HeightField,
    attr: &'a F,
}

/// Per-triangle maps, reused.
#[derive(Default)]
struct TriangleScratch {
    samples: HashMap<u64, Sample>,
    splits: HashMap<(u64, u64), Option<u64>>,
    index: HashMap<u64, u32>,
    stack: Vec<[u64; 3]>,
}

/// A dyadic barycentric point: the weights of corners 0 and 1 (corner 2 has the rest).
fn bary_key(w0: u32, w1: u32) -> u64 {
    ((w0 as u64) << 32) | w1 as u64
}

fn bary_of(key: u64) -> (u32, u32, u32) {
    let w0 = (key >> 32) as u32;
    let w1 = key as u32;
    (w0, w1, ONE - w0 - w1)
}

fn bary_mid(a: u64, b: u64) -> Option<u64> {
    let (a0, a1, _) = bary_of(a);
    let (b0, b1, _) = bary_of(b);
    let (s0, s1) = (a0 + b0, a1 + b1);
    let s2 = 2 * ONE - s0 - s1;
    (s0.is_multiple_of(2) && s1.is_multiple_of(2) && s2.is_multiple_of(2))
        .then(|| bary_key(s0 / 2, s1 / 2))
}

/// The base edge (0: corners 0-1, 1: 1-2, 2: 2-0) a point lies on, for a point that is on one
/// (corners lie on two; this picks one).
fn edge_of(b: (u32, u32, u32)) -> Option<usize> {
    if b.2 == 0 {
        Some(0)
    } else if b.0 == 0 {
        Some(1)
    } else if b.1 == 0 {
        Some(2)
    } else {
        None
    }
}

impl<F: Fn(u32) -> Attr> TriangleContext<'_, F> {
    /// The base edge (0: corners 0-1, 1: 1-2, 2: 2-0) both points lie on, if any.
    fn shared_edge(a: (u32, u32, u32), b: (u32, u32, u32)) -> Option<usize> {
        if a.2 == 0 && b.2 == 0 {
            Some(0)
        } else if a.0 == 0 && b.0 == 0 {
            Some(1)
        } else if a.1 == 0 && b.1 == 0 {
            Some(2)
        } else {
            None
        }
    }

    /// The shared points of base edge `k` of triangle `t`, and whether the edge runs from its
    /// corner `k` (forward) in the shared points' direction.
    fn edge_points(&self, t: usize, k: usize) -> Option<(&EdgePoints, bool)> {
        let tri = &self.tris[t];
        let (pa, pb) = (
            self.pos_id[tri[k] as usize],
            self.pos_id[tri[(k + 1) % 3] as usize],
        );
        let i = *self.edge_index.get(&(pa.min(pb), pa.max(pb)))?;
        Some((&self.edges[i as usize], pa < pb))
    }

    /// Parameter along base edge `k` (from its lower position id) of a point on it.
    fn edge_t(b: (u32, u32, u32), k: usize, forward: bool) -> u32 {
        // Weight of the edge's second corner.
        let w = [b.1, b.2, b.0][k];
        if forward { w } else { ONE - w }
    }

    fn blend(&self, t: usize, w: Vec3) -> Attr {
        let tri = &self.tris[t];
        Attr::blend(
            &(self.attr)(tri[0]),
            &(self.attr)(tri[1]),
            &(self.attr)(tri[2]),
            w,
        )
    }

    fn sample(&self, t: usize, key: u64, s: &mut TriangleScratch) -> Sample {
        if let Some(x) = s.samples.get(&key) {
            return *x;
        }
        let b = bary_of(key);
        let tri = &self.tris[t];
        let corner = [b.0, b.1, b.2].iter().position(|&w| w == ONE);
        let value = if let Some(c) = corner {
            self.corners[self.pos_id[tri[c] as usize] as usize]
        } else {
            edge_of(b)
                .and_then(|k| {
                    let (points, forward) = self.edge_points(t, k)?;
                    points.get(Self::edge_t(b, k, forward)).copied()
                })
                .unwrap_or_else(|| {
                    let w = Vec3::new(b.0 as f32, b.1 as f32, b.2 as f32) / ONE as f32;
                    displace_point(&self.blend(t, w), self.field, &self.rules)
                })
        };
        s.samples.insert(key, value);
        value
    }

    /// The midpoint where edge `a`-`b` of a piece of triangle `t` is split, if it is.
    fn split(&self, t: usize, a: u64, b: u64, s: &mut TriangleScratch) -> Option<u64> {
        let mid = bary_mid(a, b)?;
        if let Some(k) = Self::shared_edge(bary_of(a), bary_of(b)) {
            // On a base edge: split exactly where its shared points are.
            let (points, forward) = self.edge_points(t, k)?;
            let tm = Self::edge_t(bary_of(mid), k, forward);
            return points.get(tm).is_some().then_some(mid);
        }
        let key = (a.min(b), a.max(b));
        if let Some(&r) = s.splits.get(&key) {
            return r;
        }
        // Decide from the end points in a fixed order, so both pieces sharing the edge agree.
        let (sl, sh) = (self.sample(t, key.0, s), self.sample(t, key.1, s));
        let (bl, bh) = (bary_of(key.0), bary_of(key.1));
        let mid_sample = self.sample(t, mid, s);
        let mut eval = |f: f32| {
            if f == 0.5 {
                return mid_sample;
            }
            let w = |x: u32, y: u32| x as f32 + (y as f32 - x as f32) * f;
            let wv = Vec3::new(w(bl.0, bh.0), w(bl.1, bh.1), w(bl.2, bh.2)) / ONE as f32;
            displace_point(&self.blend(t, wv), self.field, &self.rules)
        };
        let r = split_needed(&sl, &sh, &self.rules, &mut eval).map(|_| mid);
        s.splits.insert(key, r);
        r
    }

    fn vertex(&self, t: usize, key: u64, s: &mut TriangleScratch, out: &mut Mesh) -> u32 {
        if let Some(&i) = s.index.get(&key) {
            return i;
        }
        let sample = self.sample(t, key, s);
        let b = bary_of(key);
        let a = self.blend(
            t,
            Vec3::new(b.0 as f32, b.1 as f32, b.2 as f32) / ONE as f32,
        );
        let mut color = self
            .field
            .color(a.p, a.n.normalize_or(Vec3::Y), sample.h, a.color);
        if self.rules.cavity > 0.0 {
            let mid = self.rules.midlevel.max(1e-3);
            let depth = ((mid - sample.h) / mid).clamp(0.0, 1.0);
            let shade = 1.0 - self.rules.cavity * depth.powf(1.5);
            color = Vec4::new(color.x * shade, color.y * shade, color.z * shade, color.w);
        }
        let i = out.vertices.len() as u32;
        out.vertices.push(Vertex {
            position: sample.d.to_array(),
            normal: a.n.normalize_or(Vec3::Y).to_array(),
            uv: a.uv.to_array(),
            color: color.to_array(),
        });
        s.index.insert(key, i);
        i
    }

    fn triangle(&self, t: usize, s: &mut TriangleScratch, out: &mut Mesh) {
        s.samples.clear();
        s.splits.clear();
        s.index.clear();
        s.stack.clear();
        let ids = self.tris[t].map(|v| self.pos_id[v as usize]);
        if ids[0] == ids[1] || ids[1] == ids[2] || ids[0] == ids[2] {
            return;
        }
        s.stack
            .push([bary_key(ONE, 0), bary_key(0, ONE), bary_key(0, 0)]);
        while let Some([a, b, c]) = s.stack.pop() {
            let m = [
                self.split(t, a, b, s),
                self.split(t, b, c, s),
                self.split(t, c, a, s),
            ];
            match m {
                [None, None, None] => {
                    let idx = [a, b, c].map(|k| self.vertex(t, k, s, out));
                    out.indices.extend_from_slice(&idx);
                }
                [Some(ab), Some(bc), Some(ca)] => {
                    s.stack
                        .extend([[a, ab, ca], [ab, b, bc], [ca, bc, c], [ab, bc, ca]]);
                }
                [Some(ab), None, None] => s.stack.extend([[a, ab, c], [ab, b, c]]),
                [None, Some(bc), None] => s.stack.extend([[a, b, bc], [a, bc, c]]),
                [None, None, Some(ca)] => s.stack.extend([[a, b, ca], [ca, b, c]]),
                [Some(ab), Some(bc), None] => {
                    s.stack.push([ab, b, bc]);
                    let quad = self.split_quad(t, [a, ab, bc, c], s);
                    s.stack.extend(quad);
                }
                [None, Some(bc), Some(ca)] => {
                    s.stack.push([bc, c, ca]);
                    let quad = self.split_quad(t, [a, b, bc, ca], s);
                    s.stack.extend(quad);
                }
                [Some(ab), None, Some(ca)] => {
                    s.stack.push([a, ab, ca]);
                    let quad = self.split_quad(t, [ab, b, c, ca], s);
                    s.stack.extend(quad);
                }
            }
        }
    }

    /// Two triangles of the quad `q` (counter-clockwise), cut along its shorter diagonal.
    fn split_quad(&self, t: usize, q: [u64; 4], s: &mut TriangleScratch) -> [[u64; 3]; 2] {
        let d = q.map(|k| self.sample(t, k, s).d);
        if d[0].distance_squared(d[2]) <= d[1].distance_squared(d[3]) {
            [[q[0], q[1], q[2]], [q[0], q[2], q[3]]]
        } else {
            [[q[0], q[1], q[3]], [q[1], q[2], q[3]]]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{cuboid, plane};

    fn waves(p: Vec3) -> f32 {
        let k = std::f32::consts::TAU / 0.1;
        0.5 + 0.5 * (p.x * k).sin() * (p.z * k).cos()
    }

    fn wave_displacement() -> Displacement {
        Displacement::function(0.01, 7, |p, _, _| waves(p)).with_edges(0.004, 0.02)
    }

    fn run(base: &Mesh, d: &Displacement) -> Mesh {
        let DisplacementSource::Custom(f) = &d.source else {
            unreachable!()
        };
        tessellate(base, d, d.amplitude_m, f.field.as_ref())
    }

    /// Directed edges by welded position that are not matched by the opposite direction: 0
    /// for a closed surface.
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
    fn vertices_sit_on_the_displaced_surface() {
        let m = run(&plane(Vec2::splat(0.5), 1), &wave_displacement());
        assert!(
            m.triangle_count() > 2000,
            "{} triangles",
            m.triangle_count()
        );
        for v in &m.vertices {
            let p = Vec3::from(v.position);
            let expected = 0.01 * (waves(p) - 0.5);
            assert!((p.y - expected).abs() < 1e-6, "{p} vs {expected}");
        }
        // Between the vertices the surface stays close to the true one.
        let mut worst = 0.0f32;
        for t in m.indices.as_chunks::<3>().0 {
            let c = t
                .iter()
                .map(|&i| Vec3::from(m.vertices[i as usize].position))
                .sum::<Vec3>()
                / 3.0;
            worst = worst.max((c.y - 0.01 * (waves(c) - 0.5)).abs());
        }
        assert!(worst < 0.0006, "worst deviation {worst} m");
        // Normals follow the waves.
        let tilted = m.vertices.iter().filter(|v| v.normal[1] < 0.99).count();
        assert!(tilted * 4 > m.vertices.len(), "normals stayed flat");
    }

    #[test]
    fn adaptive_tessellation_spends_triangles_where_the_relief_is() {
        // Waves on one half, flat on the other.
        let fine = |p: Vec3| 0.5 + 0.5 * (p.x * 160.0).sin() * (p.z * 140.0).cos();
        let d = Displacement::function(
            0.01,
            8,
            move |p, _, _| if p.x < 0.0 { fine(p) } else { 0.5 },
        )
        .with_edges(0.004, 0.02);
        let m = run(&plane(Vec2::splat(0.4), 4), &d);
        let (mut rough, mut flat) = (0, 0);
        for t in m.indices.as_chunks::<3>().0 {
            let x: f32 = t.iter().map(|&i| m.vertices[i as usize].position[0]).sum();
            if x < 0.0 {
                rough += 1;
            } else {
                flat += 1;
            }
        }
        assert!(rough > flat * 5, "rough half {rough}, flat half {flat}");
    }

    #[test]
    fn closed_meshes_with_hard_edges_stay_closed() {
        // A cuboid has separate vertices per face (hard edges and UV seams).
        let base = cuboid(Vec3::new(0.3, 0.2, 0.25));
        let d = Displacement::function(0.01, 9, |p, _, _| {
            0.5 + 0.5 * (p.x * 61.0).sin() * (p.y * 47.0 + p.z * 29.0).cos()
        })
        .with_edges(0.006, 0.02);
        let m = run(&base, &d);
        assert!(m.triangle_count() > 3000);
        assert_eq!(open_edges(&m), 0, "the displaced box has cracks");
        let again = run(&base, &d);
        assert_eq!(m.vertices, again.vertices, "not deterministic");
        assert_eq!(m.indices, again.indices, "not deterministic");
    }

    #[test]
    fn neighboring_meshes_share_their_border() {
        // Two tiles side by side, built separately, with matching UVs on the shared edge.
        let tile = |x0: f32| {
            let mut m = plane(Vec2::new(0.3, 0.3), 3);
            for v in &mut m.vertices {
                v.position[0] += x0;
                v.uv = [v.position[0], v.position[2]];
            }
            m
        };
        let d = wave_displacement();
        let a = run(&tile(-0.15), &d);
        let b = run(&tile(0.15), &d);
        let border = |m: &Mesh| {
            let mut pts: Vec<[u32; 3]> = m
                .vertices
                .iter()
                .filter(|v| v.position[0].abs() < 1e-6)
                .map(|v| bytemuck::cast(v.position))
                .collect();
            pts.sort_unstable();
            pts.dedup();
            pts
        };
        let (ba, bb) = (border(&a), border(&b));
        assert!(ba.len() > 12, "{} border points", ba.len());
        assert_eq!(ba, bb, "the tiles' borders differ");
        // Together they are one surface: the shared border closes up.
        let mut both = a.clone();
        both.merge(&b);
        let shared = (ba.len() - 1) * 2;
        assert_eq!(
            open_edges(&both),
            open_edges(&a) + open_edges(&b) - 2 * shared
        );
    }

    #[test]
    fn the_triangle_budget_holds() {
        let d = wave_displacement()
            .with_edges(0.001, 0.004)
            .with_max_triangles(50_000);
        let m = run(&plane(Vec2::splat(2.0), 2), &d);
        assert!(
            m.triangle_count() <= 50_000 && m.triangle_count() > 10_000,
            "{} triangles",
            m.triangle_count()
        );
    }

    #[test]
    fn height_images_sample_bilinearly_and_wrap() {
        let img = Image::from_fn(4, 4, false, |x, _| [(x * 85) as u8, 0, 0, 255]);
        let h = HeightImage::from_image(&img, 1024).unwrap();
        assert!(h.sample(Vec2::new(0.125, 0.5)).abs() < 1e-6);
        assert!((h.sample(Vec2::new(0.375, 0.5)) - 1.0 / 3.0).abs() < 1e-3);
        // Halfway between the last texel and the first (wrapping).
        assert!((h.sample(Vec2::new(1.0, 0.5)) - 0.5).abs() < 1e-3);
        assert_eq!(
            h.sample(Vec2::new(0.375, 0.5)),
            h.sample(Vec2::new(2.375, -3.5))
        );
    }
}
