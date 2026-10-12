//! Procedural PBR texture sets for destructible surfaces: painted plaster, cast concrete,
//! brick, ashlar stone, wood, natural rock, dirt, gravel, painted steel, glass and ice.
//!
//! [`generate`] builds a [`TextureSet`] (albedo, tangent-space normal and ORM images) for one
//! [`SurfaceKind`]. Everything is computed from seeded, tile-periodic noise, so every set tiles
//! seamlessly and needs no files or downloads. Features have real-world sizes (a brick face is
//! 215 × 65 mm with 10 mm joints, form ties sit every 0.6 m) and each set covers
//! [`TextureSet::tile_m`] meters, so on meshes whose UVs are in meters (see
//! [`crate::meshing::box_uv`]) a material uses `uv_scale = 1 / tile_m`:
//!
//! ```ignore
//! let kind = SurfaceKind::Brick;
//! let set = textures::generate(kind, kind.default_size(), 7);
//! let material = materials.add(set.into_material(&mut images));
//! ```
//!
//! How the images are made: each kind is a function from a point on the tile (meters) to a
//! height (meters), linear albedo, roughness, metalness and cavity occlusion. Normals come
//! from that height field with the true texel size, so normal maps are physically scaled
//! (`normal_scale = 1` means "as built"); ambient occlusion is baked from the same height
//! field (horizon based) and also darkens the albedo a little in crevices, as hand-made game
//! textures do. Noise octaves finer than about three texels are faded out, so small sizes
//! blur instead of aliasing.
//!
//! Orientation: image rows run top to bottom and the image's *bottom* is world-down on a wall
//! (`box_uv` puts `v = -y`). Masonry courses are counted from the bottom of the image, so
//! with a wall whose bottom edge sits at `y = 0` (a multiple of the tile height) the courses
//! line up with the wall's cell grid.
//!
//! Generation is parallel over rows; a 512-pixel set takes a few tens of milliseconds in a
//! release build.

use engine_assets::Assets;
use engine_core::math::hash_u64;
use engine_render::{AlphaMode, Image, StandardMaterial};
use serde::{Deserialize, Serialize};

/// The kinds of surface [`generate`] can texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceKind {
    /// Painted drywall: near-white (tint it with vertex colors), orange-peel roller stipple,
    /// faint taped seams, screw spots, scuffs and scratches.
    Plaster,
    /// Cast-in-place concrete: plywood formwork seams and grain imprint, form-tie holes every
    /// 0.6 m, bug holes, aggregate speckle, water and rust stains, efflorescence.
    Concrete,
    /// Running-bond red brick (225 × 75 mm pitch: 215 × 65 mm bricks with 10 mm recessed
    /// mortar joints), chipped and pitted, per-brick color variation.
    Brick,
    /// Cut ashlar blocks (0.9 × 0.45 m pitch, 15 mm joints) with drafted margins and pitched
    /// faces, weathering streaks and lichen.
    Stone,
    /// Sawn timber with the grain along u: growth rings, knots, checks, weathering.
    Wood,
    /// Natural cliff rock: fractured facets, cracks, strata, mineral speckle, lichen.
    /// Reads from any box-projection axis.
    Rock,
    /// Bare soil: clods, pebbles, twigs, damp patches and drying cracks.
    Dirt,
    /// Painted steel plate: chipped paint over primer, bare scratched steel, rust.
    Metal,
    /// Loose gravel: packed pebbles over fines.
    Gravel,
    /// Window glass: smudges, dust and fine scratches (mostly in the roughness).
    Glass,
    /// Ice: clear and frosted areas, internal fractures and bubbles.
    Ice,
}

impl SurfaceKind {
    /// Every kind, in declaration order.
    pub const ALL: [SurfaceKind; 11] = [
        SurfaceKind::Plaster,
        SurfaceKind::Concrete,
        SurfaceKind::Brick,
        SurfaceKind::Stone,
        SurfaceKind::Wood,
        SurfaceKind::Rock,
        SurfaceKind::Dirt,
        SurfaceKind::Metal,
        SurfaceKind::Gravel,
        SurfaceKind::Glass,
        SurfaceKind::Ice,
    ];

    /// Lowercase name (`"brick"`), the same as the serde name.
    pub fn name(self) -> &'static str {
        match self {
            SurfaceKind::Plaster => "plaster",
            SurfaceKind::Concrete => "concrete",
            SurfaceKind::Brick => "brick",
            SurfaceKind::Stone => "stone",
            SurfaceKind::Wood => "wood",
            SurfaceKind::Rock => "rock",
            SurfaceKind::Dirt => "dirt",
            SurfaceKind::Metal => "metal",
            SurfaceKind::Gravel => "gravel",
            SurfaceKind::Glass => "glass",
            SurfaceKind::Ice => "ice",
        }
    }

    /// The kind called `name` (as [`SurfaceKind::name`] spells it), if any.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }

    /// Meters of surface one texture tile covers along u and v. Masonry tiles are whole
    /// numbers of bricks/blocks and an even number of courses, so the bond continues across
    /// tiles.
    pub fn tile_m(self) -> [f32; 2] {
        match self {
            SurfaceKind::Plaster => [1.2, 1.2],
            SurfaceKind::Concrete => [2.4, 1.2],
            SurfaceKind::Brick => [1.8, 0.9],
            SurfaceKind::Stone => [3.6, 1.8],
            SurfaceKind::Wood => [1.0, 0.5],
            SurfaceKind::Rock => [2.0, 2.0],
            SurfaceKind::Dirt => [2.0, 2.0],
            SurfaceKind::Metal => [1.0, 1.0],
            SurfaceKind::Gravel => [1.5, 1.5],
            SurfaceKind::Glass => [1.0, 1.0],
            SurfaceKind::Ice => [2.0, 2.0],
        }
    }

    /// Recommended image width in pixels (the height follows the tile's aspect ratio): about
    /// 500 to 1000 pixels per meter, enough for close-ups at 1.5 m.
    pub fn default_size(self) -> u32 {
        match self {
            SurfaceKind::Glass => 512,
            SurfaceKind::Concrete | SurfaceKind::Stone => 2048,
            _ => 1024,
        }
    }

    /// Recommended `StandardMaterial::normal_scale`. Normal maps are built from heights in
    /// meters, so 1 is physically right; a few kinds are toned a little for readability.
    pub fn normal_scale(self) -> f32 {
        1.0
    }

    /// Typical perceptual roughness of the kind (the ORM texture's green channel holds the
    /// per-texel value; this is its rough average, for untextured fallbacks).
    pub fn roughness(self) -> f32 {
        match self {
            SurfaceKind::Plaster => 0.7,
            SurfaceKind::Concrete => 0.85,
            SurfaceKind::Brick => 0.85,
            SurfaceKind::Stone => 0.85,
            SurfaceKind::Wood => 0.72,
            SurfaceKind::Rock => 0.82,
            SurfaceKind::Dirt => 0.95,
            SurfaceKind::Metal => 0.5,
            SurfaceKind::Gravel => 0.8,
            SurfaceKind::Glass => 0.08,
            SurfaceKind::Ice => 0.2,
        }
    }
}

/// A tileable PBR texture set (glTF conventions, see `engine_render::material`).
#[derive(Debug, Clone)]
pub struct TextureSet {
    /// Which surface this is.
    pub kind: SurfaceKind,
    /// Base color, sRGB, alpha 255.
    pub albedo: Image,
    /// Tangent-space normal map, linear, +Y = image up (OpenGL/glTF convention).
    pub normal: Image,
    /// Linear: R = ambient occlusion, G = perceptual roughness, B = metalness.
    pub orm: Image,
    /// Meters of surface the images cover along u and v.
    pub tile_m: [f32; 2],
}

impl TextureSet {
    /// `StandardMaterial::uv_scale` for meshes with UVs in meters: `1 / tile_m`.
    pub fn uv_scale(&self) -> [f32; 2] {
        [1.0 / self.tile_m[0], 1.0 / self.tile_m[1]]
    }

    /// Add the three images to `images` and return a material that uses them: white base
    /// color (vertex colors tint it), roughness and metallic factors 1 (the ORM texture holds
    /// the absolute values), `uv_scale` for meter UVs and the kind's recommended normal scale.
    /// Glass also gets alpha blending at 30 % opacity.
    pub fn into_material(self, images: &mut Assets<Image>) -> StandardMaterial {
        let uv = self.uv_scale();
        let kind = self.kind;
        let albedo = images.add(self.albedo);
        let normal = images.add(self.normal);
        let orm = images.add(self.orm);
        let mut m = StandardMaterial {
            base_color: [1.0; 4],
            perceptual_roughness: 1.0,
            metallic: 1.0,
            base_color_texture: Some(albedo),
            normal_texture: Some(normal),
            metallic_roughness_texture: Some(orm),
            occlusion_texture: Some(orm),
            uv_scale: uv,
            normal_scale: kind.normal_scale(),
            ..Default::default()
        };
        if kind == SurfaceKind::Glass {
            m.base_color[3] = 0.3;
            m.alpha_mode = AlphaMode::Blend;
        }
        m
    }
}

/// Generate the texture set for `kind`. `size` is the image width in pixels (along u, at
/// least 8); the height follows the tile's aspect ratio (`size × tile_v / tile_u`), so texels
/// are square. Use a power of two (see [`SurfaceKind::default_size`]). The same `seed` gives
/// the same images; different seeds give different bricks, stains, knots and so on.
pub fn generate(kind: SurfaceKind, size: u32, seed: u64) -> TextureSet {
    let [tu, tv] = kind.tile_m();
    let w = size.max(8);
    let h = ((w as f32 * tv / tu).round() as u32).max(4);
    render(kind, w, h, seed, [0, 0])
}

// ---------------------------------------------------------------------------------------------
// Pipeline

/// What a surface function returns for one point.
#[derive(Debug, Clone, Copy)]
struct Texel {
    /// Height above the nominal surface, meters.
    h: f32,
    /// Linear albedo.
    c: [f32; 3],
    /// Perceptual roughness.
    r: f32,
    /// Metalness.
    m: f32,
    /// Extra cavity occlusion, 1 = open.
    ao: f32,
}

impl Default for Texel {
    fn default() -> Self {
        Self {
            h: 0.0,
            c: [0.5; 3],
            r: 0.8,
            m: 0.0,
            ao: 1.0,
        }
    }
}

/// How the height field turns into occlusion.
struct Finish {
    /// Farthest a crevice wall can shade from, meters.
    ao_radius: f32,
    /// How strongly the baked horizon occlusion counts (0..1).
    ao_strength: f32,
    /// How much of the occlusion also darkens the albedo (0..1).
    albedo_ao: f32,
}

type SurfaceFn = fn(&Tile, u64, f32, f32) -> Texel;

fn surface(kind: SurfaceKind) -> (SurfaceFn, Finish) {
    let f = |ao_radius, ao_strength, albedo_ao| Finish {
        ao_radius,
        ao_strength,
        albedo_ao,
    };
    match kind {
        SurfaceKind::Plaster => (plaster, f(0.004, 0.6, 0.15)),
        SurfaceKind::Concrete => (concrete, f(0.02, 0.9, 0.3)),
        SurfaceKind::Brick => (brick, f(0.025, 1.0, 0.4)),
        SurfaceKind::Stone => (stone, f(0.06, 1.0, 0.35)),
        SurfaceKind::Wood => (wood, f(0.01, 0.9, 0.35)),
        SurfaceKind::Rock => (rock, f(0.08, 1.0, 0.4)),
        SurfaceKind::Dirt => (dirt, f(0.03, 1.0, 0.4)),
        SurfaceKind::Metal => (metal, f(0.006, 0.7, 0.2)),
        SurfaceKind::Gravel => (gravel, f(0.03, 1.0, 0.45)),
        SurfaceKind::Glass => (glass, f(0.002, 0.3, 0.0)),
        SurfaceKind::Ice => (ice, f(0.02, 0.6, 0.15)),
    }
}

/// Render `kind` at `w × h` pixels. `shift` starts the sampling `shift` pixels into the tile
/// (without wrapping the coordinates), which the tests use to prove the tile is periodic.
fn render(kind: SurfaceKind, w: u32, h: u32, seed: u64, shift: [u32; 2]) -> TextureSet {
    let [tu, tv] = kind.tile_m();
    let (wu, hu) = (w as usize, h as usize);
    let tile = Tile {
        w: tu,
        h: tv,
        texel: (tu / w as f32).max(tv / h as f32),
    };
    let (sample, finish) = surface(kind);
    let s = hash_u64(seed ^ (kind as u64 + 1).wrapping_mul(0x5851_F42D_4C95_7F2D));

    let mut texels = vec![Texel::default(); wu * hu];
    par_rows(&mut texels, wu, |r, row| {
        let y = tv * (1.0 - ((r as u32 + shift[1]) as f32 + 0.5) / h as f32);
        for (c, t) in row.iter_mut().enumerate() {
            let x = tu * ((c as u32 + shift[0]) as f32 + 0.5) / w as f32;
            *t = sample(&tile, s, x, y);
        }
    });

    let heights: Vec<f32> = texels.iter().map(|t| t.h).collect();
    let (dx_m, dy_m) = (tu / w as f32, tv / h as f32);
    let ao_steps = horizon_steps(finish.ao_radius / dx_m.max(dy_m));
    let mut out = vec![[0u8; 12]; wu * hu];
    par_rows(&mut out, wu, |r, row| {
        let at = |c: isize, rr: isize| {
            let c = c.rem_euclid(wu as isize) as usize;
            let rr = rr.rem_euclid(hu as isize) as usize;
            heights[rr * wu + c]
        };
        let ri = r as isize;
        for (c, px) in row.iter_mut().enumerate() {
            let ci = c as isize;
            // Sobel gradient; rows grow downward, so "up" is -row.
            let (tl, tc, tr) = (at(ci - 1, ri - 1), at(ci, ri - 1), at(ci + 1, ri - 1));
            let (ml, mr) = (at(ci - 1, ri), at(ci + 1, ri));
            let (bl, bc, br) = (at(ci - 1, ri + 1), at(ci, ri + 1), at(ci + 1, ri + 1));
            let dhdx = ((tr + 2.0 * mr + br) - (tl + 2.0 * ml + bl)) / (8.0 * dx_m);
            let dhdy = ((tl + 2.0 * tc + tr) - (bl + 2.0 * bc + br)) / (8.0 * dy_m);
            let (nx, ny, nz) = (-dhdx, -dhdy, 1.0f32);
            let len = (nx * nx + ny * ny + nz * nz).sqrt();
            let n = [nx / len, ny / len, nz / len];

            // Horizon-based occlusion: in 8 directions, the steepest rise within the radius.
            let h0 = heights[r * wu + c];
            let mut occ = 0.0;
            if !ao_steps.is_empty() {
                for (ddx, ddy) in DIRS8 {
                    let mut best = 0.0f32;
                    for &s in &ao_steps {
                        let hs = at(
                            ci + (ddx * s).round() as isize,
                            ri + (ddy * s).round() as isize,
                        );
                        let dist = s * dx_m;
                        best = best.max((hs - h0) / dist);
                    }
                    occ += best / (1.0 + best * best).sqrt();
                }
                occ /= 8.0;
            }
            let t = &texels[r * wu + c];
            let ao = (1.0 - occ * finish.ao_strength).clamp(0.0, 1.0) * t.ao.clamp(0.0, 1.0);
            let dark = 1.0 - finish.albedo_ao * (1.0 - ao);
            let mut o = [0u8; 12];
            for k in 0..3 {
                o[k] = linear_to_srgb8((t.c[k] * dark).clamp(0.01, 0.9));
                o[4 + k] = unorm8(n[k] * 0.5 + 0.5);
            }
            o[3] = 255;
            o[7] = 255;
            o[8] = unorm8(ao);
            o[9] = unorm8(t.r.clamp(0.02, 1.0));
            o[10] = unorm8(t.m.clamp(0.0, 1.0));
            o[11] = 255;
            *px = o;
        }
    });

    let mut albedo = Vec::with_capacity(wu * hu * 4);
    let mut normal = Vec::with_capacity(wu * hu * 4);
    let mut orm = Vec::with_capacity(wu * hu * 4);
    for o in &out {
        albedo.extend_from_slice(&o[0..4]);
        normal.extend_from_slice(&o[4..8]);
        orm.extend_from_slice(&o[8..12]);
    }
    TextureSet {
        kind,
        albedo: Image::new(w, h, albedo, true),
        normal: Image::new(w, h, normal, false),
        orm: Image::new(w, h, orm, false),
        tile_m: [tu, tv],
    }
}

const DIRS8: [(f32, f32); 8] = [
    (1.0, 0.0),
    (0.707_106_8, 0.707_106_8),
    (0.0, 1.0),
    (-0.707_106_8, 0.707_106_8),
    (-1.0, 0.0),
    (-0.707_106_8, -0.707_106_8),
    (0.0, -1.0),
    (0.707_106_8, -0.707_106_8),
];

/// Sample distances (texels) for horizon occlusion out to `radius` texels.
fn horizon_steps(radius: f32) -> Vec<f32> {
    let mut v = Vec::new();
    let mut s = 1.0f32;
    while s <= radius.min(64.0) {
        v.push(s);
        s = (s * 1.6).max(s + 1.0);
    }
    v
}

/// Run `f(row_index, row)` over the rows of `data` (`width` items each) on all cores.
fn par_rows<T: Send>(data: &mut [T], width: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    let rows = data.len() / width.max(1);
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(rows);
    if threads <= 1 {
        for (r, row) in data.chunks_mut(width).enumerate() {
            f(r, row);
        }
        return;
    }
    const BAND: usize = 4;
    let jobs = std::sync::Mutex::new(data.chunks_mut(width * BAND).enumerate());
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    let next = match jobs.lock() {
                        Ok(mut it) => it.next(),
                        Err(_) => None,
                    };
                    let Some((j, chunk)) = next else { break };
                    for (k, row) in chunk.chunks_mut(width).enumerate() {
                        f(j * BAND + k, row);
                    }
                }
            });
        }
    });
}

fn unorm8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

fn linear_to_srgb8(v: f32) -> u8 {
    static LUT: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    let lut = LUT.get_or_init(|| {
        (0..4096)
            .map(|i| {
                let l = i as f32 / 4095.0;
                let s = if l <= 0.003_130_8 {
                    l * 12.92
                } else {
                    1.055 * l.powf(1.0 / 2.4) - 0.055
                };
                unorm8(s)
            })
            .collect()
    });
    lut[(v.clamp(0.0, 1.0) * 4095.0 + 0.5) as usize]
}

// ---------------------------------------------------------------------------------------------
// Tile-periodic noise

/// A tile of `w × h` meters rendered with square texels of `texel` meters. All noise is
/// periodic over the tile.
#[derive(Debug, Clone, Copy)]
struct Tile {
    w: f32,
    h: f32,
    texel: f32,
}

/// Result of a Voronoi lookup.
#[derive(Debug, Clone, Copy)]
struct Voro {
    /// Distance to the nearest feature point, meters.
    f1: f32,
    /// Distance to the nearest cell border, meters.
    edge: f32,
    /// Hash of the nearest cell (wrapped, so the same cell on both sides of a tile seam).
    id: u64,
    /// The point minus the nearest feature point, meters.
    dx: f32,
    dy: f32,
}

impl Tile {
    /// Whole lattice periods across the tile for about `fx`, `fy` cycles per meter.
    fn periods(&self, fx: f32, fy: f32) -> (i32, i32) {
        (
            ((fx * self.w).round() as i32).max(1),
            ((fy * self.h).round() as i32).max(1),
        )
    }

    /// Gradient noise with about `fx` × `fy` cycles per meter, roughly in `[-1, 1]`.
    fn n2(&self, x: f32, y: f32, fx: f32, fy: f32, seed: u64) -> f32 {
        let (px, py) = self.periods(fx, fy);
        perlin(x * px as f32 / self.w, y * py as f32 / self.h, px, py, seed)
    }

    /// Isotropic [`Tile::n2`].
    fn n(&self, x: f32, y: f32, f: f32, seed: u64) -> f32 {
        self.n2(x, y, f, f, seed)
    }

    /// Fractal sum of [`Tile::n2`] (lacunarity 2), normalized to about `[-1, 1]`. Octaves
    /// finer than ~3 texels fade out.
    fn fbm2(&self, x: f32, y: f32, fx: f32, fy: f32, octaves: u32, gain: f32, seed: u64) -> f32 {
        let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
        let (mut fx, mut fy) = (fx, fy);
        for i in 0..octaves {
            norm += amp;
            let k = band(fx.max(fy) * self.texel);
            if k > 0.0 {
                sum += amp * k * self.n2(x, y, fx, fy, seed.wrapping_add(i as u64 * 0x9E37_79B9));
            }
            amp *= gain;
            fx *= 2.0;
            fy *= 2.0;
        }
        sum / norm
    }

    /// Isotropic [`Tile::fbm2`].
    fn fbm(&self, x: f32, y: f32, f: f32, octaves: u32, gain: f32, seed: u64) -> f32 {
        self.fbm2(x, y, f, f, octaves, gain, seed)
    }

    /// How much detail of `f` cycles per meter survives at this resolution (1 = all).
    fn keep(&self, f: f32) -> f32 {
        band(f * self.texel)
    }

    /// Call `visit(hash, dx, dy)` for the feature points of the 3 × 3 jittered-grid cells
    /// (about `f` cells per meter) around the point; `(dx, dy)` is the point minus the
    /// feature, meters. Features must not reach farther than one cell.
    fn scatter(&self, x: f32, y: f32, f: f32, seed: u64, mut visit: impl FnMut(u64, f32, f32)) {
        let (px, py) = self.periods(f, f);
        let (sx, sy) = (self.w / px as f32, self.h / py as f32);
        let (gx, gy) = (x / sx, y / sy);
        let (fx, fy) = (gx.floor(), gy.floor());
        let (ix, iy) = (fx as i32, fy as i32);
        let (fx, fy) = (gx - fx, gy - fy);
        for j in -1..=1 {
            for i in -1..=1 {
                let h = hash2((ix + i).rem_euclid(px), (iy + j).rem_euclid(py), seed);
                let dx = (fx - i as f32 - u01(h)) * sx;
                let dy = (fy - j as f32 - u01b(h)) * sy;
                visit(h, dx, dy);
            }
        }
    }

    /// Voronoi cells with about `f` cells per meter; `jitter` 0..1 moves the points off the
    /// grid centers.
    fn voronoi(&self, x: f32, y: f32, f: f32, jitter: f32, seed: u64) -> Voro {
        let (px, py) = self.periods(f, f);
        let (sx, sy) = (self.w / px as f32, self.h / py as f32);
        let (gx, gy) = (x / sx, y / sy);
        let (fx, fy) = (gx.floor(), gy.floor());
        let (ix, iy) = (fx as i32, fy as i32);
        let (fx, fy) = (gx - fx, gy - fy);
        let feature = |i: i32, j: i32| {
            let h = hash2((ix + i).rem_euclid(px), (iy + j).rem_euclid(py), seed);
            let rx = (i as f32 + 0.5 + jitter * (u01(h) - 0.5) - fx) * sx;
            let ry = (j as f32 + 0.5 + jitter * (u01b(h) - 0.5) - fy) * sy;
            (rx, ry, h)
        };
        let (mut bd, mut bi, mut bj, mut brx, mut bry, mut bh) = (f32::MAX, 0, 0, 0.0, 0.0, 0);
        for j in -1..=1 {
            for i in -1..=1 {
                let (rx, ry, h) = feature(i, j);
                let d = rx * rx + ry * ry;
                if d < bd {
                    (bd, bi, bj, brx, bry, bh) = (d, i, j, rx, ry, h);
                }
            }
        }
        let mut edge = f32::MAX;
        for j in bj - 1..=bj + 1 {
            for i in bi - 1..=bi + 1 {
                if i == bi && j == bj {
                    continue;
                }
                let (rx, ry, _) = feature(i, j);
                let (ex, ey) = (rx - brx, ry - bry);
                let len = (ex * ex + ey * ey).sqrt();
                if len > 1e-9 {
                    edge = edge.min((0.5 * (brx + rx) * ex + 0.5 * (bry + ry) * ey) / len);
                }
            }
        }
        Voro {
            f1: bd.sqrt(),
            edge,
            id: bh,
            dx: -brx,
            dy: -bry,
        }
    }
}

const GRAD: [[f32; 2]; 16] = [
    [1.0, 0.0],
    [0.923_879_5, 0.382_683_4],
    [0.707_106_8, 0.707_106_8],
    [0.382_683_4, 0.923_879_5],
    [0.0, 1.0],
    [-0.382_683_4, 0.923_879_5],
    [-0.707_106_8, 0.707_106_8],
    [-0.923_879_5, 0.382_683_4],
    [-1.0, 0.0],
    [-0.923_879_5, -0.382_683_4],
    [-0.707_106_8, -0.707_106_8],
    [-0.382_683_4, -0.923_879_5],
    [0.0, -1.0],
    [0.382_683_4, -0.923_879_5],
    [0.707_106_8, -0.707_106_8],
    [0.923_879_5, -0.382_683_4],
];

/// Perlin gradient noise on a lattice that wraps every `px` × `py` cells.
fn perlin(x: f32, y: f32, px: i32, py: i32, seed: u64) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let ix = (x0 as i32).rem_euclid(px);
    let iy = (y0 as i32).rem_euclid(py);
    let ix1 = if ix + 1 == px { 0 } else { ix + 1 };
    let iy1 = if iy + 1 == py { 0 } else { iy + 1 };
    let g = |i: i32, j: i32, dx: f32, dy: f32| {
        let v = GRAD[(hash2(i, j, seed) >> 60) as usize];
        v[0] * dx + v[1] * dy
    };
    let n00 = g(ix, iy, fx, fy);
    let n10 = g(ix1, iy, fx - 1.0, fy);
    let n01 = g(ix, iy1, fx, fy - 1.0);
    let n11 = g(ix1, iy1, fx - 1.0, fy - 1.0);
    let u = fx * fx * fx * (fx * (fx * 6.0 - 15.0) + 10.0);
    let v = fy * fy * fy * (fy * (fy * 6.0 - 15.0) + 10.0);
    let a = n00 + (n10 - n00) * u;
    let b = n01 + (n11 - n01) * u;
    (a + (b - a) * v) * 1.42
}

fn hash2(i: i32, j: i32, seed: u64) -> u64 {
    hash_u64((i as u32 as u64) ^ ((j as u32 as u64) << 32) ^ seed)
}

/// First uniform `[0, 1)` from a hash.
fn u01(h: u64) -> f32 {
    (h >> 40) as f32 / 16_777_216.0
}

/// Second, independent uniform `[0, 1)` from the same hash.
fn u01b(h: u64) -> f32 {
    ((h >> 16) & 0xFF_FFFF) as f32 / 16_777_216.0
}

/// The `k`-th uniform `[0, 1)` derived from a hash.
fn rnd(h: u64, k: u64) -> f32 {
    u01(hash_u64(h ^ k.wrapping_mul(0xA24B_AED4_963E_E407)))
}

/// A sub-seed: `seed` salted with `k`.
fn sd(seed: u64, k: u64) -> u64 {
    seed ^ k.wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

/// Contrast kept for a feature of `cycles` per texel: 1 below ~1/5, 0 past ~1/2.5.
fn band(cycles: f32) -> f32 {
    1.0 - smooth(0.2, 0.4, cycles)
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)]
}

fn mul(a: [f32; 3], k: f32) -> [f32; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn mul3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}

/// Distance from `x` to the nearest multiple of `period`.
fn wrap_abs(x: f32, period: f32) -> f32 {
    let r = x.rem_euclid(period);
    r.min(period - r)
}

/// Inside distance of a rounded rectangle centered at the origin (positive inside), meters.
fn rrect_inside(px: f32, py: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = px.abs() - (hw - r);
    let qy = py.abs() - (hh - r);
    let outside = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r;
    -outside
}

/// Distance from `(dx, dy)` (relative to a segment's center) to a segment of half-length
/// `half` along angle `ang`; also returns the position along it (-1..1).
fn segment(dx: f32, dy: f32, ang: f32, half: f32) -> (f32, f32) {
    let (s, c) = ang.sin_cos();
    let along = dx * c + dy * s;
    let across = -dx * s + dy * c;
    let a = along.clamp(-half, half);
    ((along - a).hypot(across), along / half)
}

/// Cell of a running bond with `pitch` (u, v) meters: `(column, course, local x, local y)`,
/// courses counted from y = 0 upward, odd courses offset by half a unit, indices wrapped to
/// `cols × rows`.
fn bond(x: f32, y: f32, pitch: [f32; 2], cols: i32, rows: i32) -> (i32, i32, f32, f32) {
    let course = (y / pitch[1]).floor();
    let ly = y - course * pitch[1];
    let k = course as i32;
    let off = if k.rem_euclid(2) == 1 { pitch[0] * 0.5 } else { 0.0 };
    let bx = x + off;
    let col = (bx / pitch[0]).floor();
    let lx = bx - col * pitch[0];
    ((col as i32).rem_euclid(cols), k.rem_euclid(rows), lx, ly)
}

/// Lichen colonies on stone: coverage 0..1, color and a crusty height (meters).
fn lichen(t: &Tile, s: u64, x: f32, y: f32, freq: f32, density: f32) -> (f32, [f32; 3], f32) {
    let mut cover = 0.0f32;
    let mut color = [0.4, 0.4, 0.33];
    let edge_n = t.fbm(x, y, freq * 12.0, 3, 0.5, sd(s, 1));
    t.scatter(x, y, freq, sd(s, 2), |h, dx, dy| {
        if rnd(h, 1) > density {
            return;
        }
        let radius = (0.12 + 0.3 * rnd(h, 2)) / freq;
        let d = dx.hypot(dy) / radius + 0.35 * edge_n;
        let c = 1.0 - smooth(0.75, 1.0, d);
        if c > cover {
            cover = c;
            let pick = rnd(h, 3);
            color = if pick < 0.45 {
                [0.33, 0.36, 0.26] // grey-green crust
            } else if pick < 0.75 {
                [0.50, 0.50, 0.44] // pale grey-white
            } else if pick < 0.9 {
                [0.36, 0.38, 0.12] // yellow-green
            } else {
                [0.52, 0.24, 0.05] // orange (Xanthoria)
            };
            // Older centers are darker, growing rims lighter.
            color = mul(color, 0.8 + 0.3 * smooth(0.3, 0.9, d));
        }
    });
    let crust = t.voronoi(x, y, freq * 60.0, 0.9, sd(s, 3));
    let bump = (1.0 - smooth(0.0, 0.6 / (freq * 60.0), crust.f1)) * 0.0004;
    (cover, color, bump * cover)
}

// ---------------------------------------------------------------------------------------------
// Surfaces. Each maps a point (x along u, y up from the tile's bottom edge, meters) to a
// texel. Seeds are per-kind; `sd(s, k)` salts them per feature.

/// Painted drywall.
fn plaster(t: &Tile, s: u64, x: f32, y: f32) -> Texel {
    // Taped and mudded sheet joint at x = 0 (sheets are 1.2 m wide): a wide feathered band of
    // sanded compound, very slightly proud and smoother, with a different sheen ("flashing").
    let seam_d = wrap_abs(x, t.w) + 0.02 * t.n2(x, y, 3.0, 2.0, sd(s, 1));
    let seam = 1.0 - smooth(0.03, 0.17, seam_d);
    // Roller passes: 5 vertical lanes per sheet with different stipple strength.
    let lane_x = x + 0.03 * t.n2(x, y, 1.0, 3.0, sd(s, 2));
    let lane = ((lane_x / (t.w / 5.0)).floor() as i32).rem_euclid(5);
    let lane_k = 0.7 + 0.6 * rnd(hash2(lane, 0, sd(s, 3)), 1);
    // Orange-peel stipple: soft nubs on a jittered grid plus finer noise.
    let cell = 1.0 / 190.0;
    let v = t.voronoi(x, y, 190.0, 1.0, sd(s, 4));
    let nub = (1.0 - smooth(0.0, 0.75 * cell, v.f1)) * (0.6 + 0.8 * rnd(v.id, 1));
    let fine = t.fbm(x, y, 260.0, 2, 0.5, sd(s, 5));
    let keep = t.keep(190.0);
    let stip = (nub * keep * 0.8 + 0.5 * fine) * lane_k * (1.0 - 0.65 * seam);
    let broad = t.fbm(x, y, 35.0, 3, 0.5, sd(s, 6));
    let mut h = stip * 0.00009 + broad * 0.00006;
    // Long-wave unevenness of the board and the bulge of the joint compound.
    h += t.fbm(x, y, 2.5, 3, 0.5, sd(s, 7)) * 0.0005 + seam * seam * 0.0004;

    // Spackled screw heads on the studs (every 0.4 m) every 0.3 m up.
    let si = (x / 0.4).round();
    let sj = ((y - 0.15) / 0.3).round();
    let hs = hash2((si as i32).rem_euclid(3), (sj as i32).rem_euclid(4), sd(s, 8));
    let sdx = x - (si * 0.4 + (u01(hs) - 0.5) * 0.01);
    let sdy = y - (sj * 0.3 + 0.15 + (u01b(hs) - 0.5) * 0.03);
    let spot_r = sdx.hypot(sdy);
    let spot = 1.0 - smooth(0.006, 0.013, spot_r);
    h -= spot * 0.00012 * (1.0 - smooth(0.0, 0.004, spot_r)) - spot * 0.00003;

    // Scuffs (burnished, greyish) and scratches.
    let mut scuff = 0.0f32;
    let mut scratch = 0.0f32;
    let breakup = t.fbm(x, y, 60.0, 3, 0.6, sd(s, 9)) * 0.5 + 0.5;
    t.scatter(x, y, 3.0, sd(s, 10), |hh, dx, dy| {
        let kind = rnd(hh, 1);
        if kind < 0.3 {
            let ang = (rnd(hh, 2) - 0.5) * 0.6;
            let (d, a) = segment(dx, dy, ang, 0.02 + 0.06 * rnd(hh, 3));
            let width = 0.004 + 0.008 * rnd(hh, 4);
            let k = (1.0 - smooth(0.0, width, d)) * (1.0 - a.abs().powi(4)).max(0.0);
            scuff = scuff.max(k * smooth(0.25, 0.7, breakup) * (0.4 + 0.6 * rnd(hh, 5)));
        } else if kind < 0.75 {
            let ang = (rnd(hh, 6) - 0.5) * 1.2 + if rnd(hh, 7) < 0.25 { 1.57 } else { 0.0 };
            let (d, a) = segment(dx, dy, ang, 0.03 + 0.12 * rnd(hh, 8));
            let width = 0.0003 + 0.0004 * rnd(hh, 9);
            let fade = (1.0 - a.abs().powi(2)).max(0.0);
            scratch = scratch.max((1.0 - smooth(0.0, width, d)) * fade);
        }
    });
    h -= scratch * 0.00006;

    let paint = [0.84, 0.835, 0.82];
    let mottle = t.fbm(x, y, 4.0, 3, 0.5, sd(s, 11)) * 0.012;
    let mut c = mul(paint, 1.0 + mottle + 0.008 * stip + 0.006 * seam - 0.008 * spot);
    c = mix(c, mul3(c, [0.86, 0.85, 0.84]), scuff * 0.6);
    c = mul(c, 1.0 - 0.06 * scratch);
    let r = 0.72 + 0.04 * fine - 0.08 * seam - 0.05 * spot - 0.18 * scuff + 0.05 * scratch;
    Texel {
        h,
        c,
        r,
        m: 0.0,
        ao: 1.0,
    }
}

/// Cast-in-place concrete from plywood forms (one 2.4 × 1.2 m panel per tile).
fn concrete(t: &Tile, s: u64, x: f32, y: f32) -> Texel {
    // Panel seams at the tile edges: a thin irregular fin of paste that leaked between
    // panels, with honeycombed paste loss beside it.
    let seam = wrap_abs(x, t.w).min(wrap_abs(y, t.h));
    let fin_n = t.n(x, y, 35.0, sd(s, 1));
    let fin_w = 0.0012 + 0.0007 * fin_n;
    let fin = (1.0 - smooth(fin_w * 0.4, fin_w, seam)) * (0.55 + 0.45 * t.n(x, y, 9.0, sd(s, 2)));
    let near_seam = 1.0 - smooth(0.003, 0.035, seam);
    let honey = near_seam * smooth(0.15, 0.55, t.fbm(x, y, 30.0, 3, 0.5, sd(s, 3)));

    // Plywood face grain printed into the surface, along u, slightly wavy.
    let gw = t.fbm2(x, y, 1.5, 3.0, 2, 0.5, sd(s, 4)) * 0.025;
    let grain = t.fbm2(x, y + gw, 1.5, 60.0, 4, 0.55, sd(s, 5));
    let grain_fine = t.n2(x, y + gw, 5.0, 240.0, sd(s, 6)) * t.keep(240.0);
    let mut h = grain * 0.00014 + grain_fine * 0.00005;
    // Long-wave flatness and slight pillowing of the panel.
    h += t.fbm(x, y, 1.5, 3, 0.5, sd(s, 7)) * 0.0008;
    h += fin * 0.0007 - honey * 0.0018 * (0.5 + 0.5 * t.n(x, y, 120.0, sd(s, 8)));

    // Form ties every 0.6 m (centers at 0.3 + 0.6 i, 0.3 + 0.6 j). Most holes are open cones
    // with a rusty rod end; some are patched with grout.
    let gx = ((x - 0.3) / 0.6).round();
    let gy = ((y - 0.3) / 0.6).round();
    let tx = x - (0.3 + gx * 0.6);
    let ty = y - (0.3 + gy * 0.6);
    let th = hash2((gx as i32).rem_euclid(4), (gy as i32).rem_euclid(2), sd(s, 9));
    let tr = tx.hypot(ty);
    let patched = rnd(th, 1) < 0.3;
    let mut patch = 0.0;
    let mut rod = 0.0;
    let mut hole = 0.0;
    if patched {
        let pr = 0.018 + 0.003 * t.n(x, y, 140.0, sd(s, 10));
        patch = 1.0 - smooth(pr - 0.0012, pr, tr);
        h += patch * (-0.0012 + 0.0003 * t.n(x, y, 300.0, sd(s, 11)));
    } else {
        let cone_r = 0.0125;
        hole = 1.0 - smooth(cone_r - 0.0008, cone_r, tr);
        let depth = ((cone_r - tr) * 1.7).clamp(0.0, 0.016);
        rod = 1.0 - smooth(0.0028, 0.0036, tr);
        h -= depth * hole - rod * 0.004;
        // Ring left by the cone's washer.
        let ring = 1.0 - smooth(0.0, 0.0012, (tr - 0.021).abs());
        h -= ring * 0.00025;
    }
    // Rust bleeding down from the open tie above (streaks are shorter than the tie spacing).
    let mut rust = 0.0;
    let gy_above = ((y - 0.3) / 0.6).ceil();
    let th_above = hash2(
        (gx as i32).rem_euclid(4),
        (gy_above as i32).rem_euclid(2),
        sd(s, 9),
    );
    if rnd(th_above, 1) >= 0.3 {
        let th = th_above;
        let below = 0.3 + gy_above * 0.6 - y;
        let len = 0.12 + 0.35 * rnd(th, 2);
        if below > 0.0 && below < len {
            let wiggle = 0.004 * t.n2(x, y, 20.0, 4.0, sd(s, 12));
            let width = 0.004 + below * 0.05;
            let across = (tx + wiggle).abs();
            let streaks = 0.5 + 0.5 * t.n2(x, y, 140.0, 6.0, sd(s, 13));
            rust = (1.0 - smooth(0.0, width, across))
                * (1.0 - below / len)
                * streaks
                * (0.35 + 0.65 * rnd(th, 3));
        }
    }

    // Bug holes: air voids against the form, a few millimeters across, in clusters.
    let density = 0.08 + 0.3 * smooth(-0.1, 0.6, t.n(x, y, 2.5, sd(s, 14)));
    let mut bug = 0.0f32;
    t.scatter(x, y, 55.0, sd(s, 15), |hh, dx, dy| {
        if rnd(hh, 1) < density {
            let rad = 0.0008 + 0.0042 * rnd(hh, 2).powi(2);
            let d = dx.hypot(dy);
            if d < rad {
                let k = 1.0 - (d / rad).powi(2);
                bug = bug.max(k.sqrt() * rad * 0.9);
            }
        }
    });
    h -= bug;

    // Color: cement paste with aggregate showing faintly, cloudy hydration differences,
    // water streaks, efflorescence below the top seam, rust from ties, grout patches.
    let cloud = t.fbm(x, y, 1.2, 5, 0.55, sd(s, 16));
    let agg = t.voronoi(x, y, 240.0, 1.0, sd(s, 17));
    let agg_v = (rnd(agg.id, 1) - 0.5) * t.keep(240.0);
    let speck = t.n(x, y, 700.0, sd(s, 18)) * t.keep(700.0);
    let mut c = mul(
        [0.31, 0.305, 0.29],
        1.0 + 0.11 * cloud + 0.09 * agg_v + 0.04 * speck + 0.03 * grain,
    );
    let streak = t.fbm2(x, y, 10.0, 0.85, 4, 0.5, sd(s, 19));
    let stain = smooth(0.05, 0.5, streak) * smooth(-0.3, 0.4, t.n(x, y, 0.9, sd(s, 20)));
    c = mul(c, 1.0 - 0.25 * stain);
    let below_top = t.h - y;
    let eff_n = t.fbm2(x, y, 6.0, 2.0, 3, 0.5, sd(s, 21));
    let eff = (1.0 - smooth(0.05, 0.1 + 0.15 * (eff_n * 0.5 + 0.5), below_top))
        * smooth(0.0, 0.06, below_top)
        * smooth(-0.1, 0.4, eff_n);
    c = mix(c, [0.56, 0.56, 0.54], 0.5 * eff);
    c = mix(c, [0.36, 0.35, 0.33], patch * 0.85);
    c = mul(c, 1.0 - 0.18 * honey);
    c = mix(c, [0.24, 0.1, 0.04], 0.6 * rust);
    c = mix(c, [0.14, 0.07, 0.035], rod * 0.9);
    c = mul(c, 1.0 - 0.15 * hole);
    let r = 0.86 + 0.04 * speck + 0.04 * agg_v - 0.1 * stain + 0.06 * honey + 0.04 * patch
        - 0.06 * fin;
    Texel {
        h,
        c,
        r,
        m: 0.0,
        ao: 1.0 - 0.3 * hole,
    }
}

/// Brick pitch along u and v, meters (215 × 65 mm brick + 10 mm joint).
const BRICK_PITCH: [f32; 2] = [0.225, 0.075];
/// Brick mortar joint width, meters.
const BRICK_JOINT: f32 = 0.01;

/// Running-bond brick: 8 bricks × 12 courses per 1.8 × 0.9 m tile.
fn brick(t: &Tile, s: u64, x: f32, y: f32) -> Texel {
    let (col, course, lx, ly) = bond(x, y, BRICK_PITCH, 8, 12);
    let id = hash2(col, course, sd(s, 1));
    let rv = |k| rnd(id, k);
    // The brick rectangle, with laying imprecision of about a millimeter.
    let jw = BRICK_JOINT * 0.5;
    let px = lx - BRICK_PITCH[0] * 0.5 - (rv(1) - 0.5) * 0.002;
    let py = ly - BRICK_PITCH[1] * 0.5 - (rv(2) - 0.5) * 0.0014;
    let hw = BRICK_PITCH[0] * 0.5 - jw - rv(3) * 0.0008;
    let hh = BRICK_PITCH[1] * 0.5 - jw - rv(4) * 0.0006;
    let d0 = rrect_inside(px, py, hw, hh, 0.002) + t.fbm(x, y, 120.0, 3, 0.5, sd(s, 2)) * 0.0009;

    // Chips: some bricks lose chunks of their arrises and corners.
    let chippy = 0.25 + 1.1 * rv(5).powi(2);
    let corner = 1.0 - smooth(0.0, 0.03, (hw - px.abs()).min(0.03) + (hh - py.abs()).min(0.03));
    let chip_n = t.fbm(x, y, 55.0, 3, 0.5, sd(s, 3)) * 0.5 + 0.5;
    let chip = (smooth(0.5, 0.75, chip_n) * chippy + corner * 0.6 * rv(6)) * 0.011;

    // Face: slight tilt and warp, sandy grain, pits from burnt-out inclusions.
    let tilt = (rv(7) - 0.5) * 0.0016 * px / hw + (rv(8) - 0.5) * 0.001 * py / hh;
    let warp = t.n(x, y, 20.0, sd(s, 4)) * 0.0004;
    let sand = t.fbm(x, y, 450.0, 2, 0.5, sd(s, 5)) * 0.00012;
    let mut pit = 0.0f32;
    let pit_rate = 0.15 + 0.35 * rv(9);
    t.scatter(x, y, 140.0, sd(s, 6), |hh2, dx, dy| {
        if rnd(hh2, 1) < pit_rate {
            let rad = 0.0006 + 0.0014 * rnd(hh2, 2);
            let d = dx.hypot(dy);
            if d < rad {
                pit = pit.max((1.0 - (d / rad).powi(2)).sqrt() * rad * 0.7);
            }
        }
    });
    let face = tilt + warp + sand - pit;
    // Arris: rounded over ~3 mm, chip scoops slope down to the edge.
    let br = 0.003;
    let bevel = if d0 >= br {
        0.0
    } else if d0 > 0.0 {
        -(br - (br * br - (br - d0) * (br - d0)).max(0.0).sqrt())
    } else {
        -br + d0 * 4.0
    };
    let chip_floor = -(chip - d0).max(0.0) * 0.75 + t.n(x, y, 260.0, sd(s, 7)) * 0.0004;
    let chipped = smooth(0.0, 0.0012, chip - d0);
    let brick_h = face + bevel.min(chip_floor);

    // Mortar: recessed ~6 mm, sandy, some joints fuller than others.
    let joint_fill = t.n2(x, y, 6.0, 6.0, sd(s, 8)) * 0.0015;
    let mortar_sand = t.fbm(x, y, 500.0, 2, 0.5, sd(s, 9)) * 0.0005;
    let mortar_h = -0.0062 + joint_fill + mortar_sand;
    let h = brick_h.max(mortar_h);
    let is_brick = smooth(-0.0002, 0.0004, brick_h - mortar_h);

    // Brick color: reds, oranges and browns with a few dark overfired ones.
    let pick = rv(10);
    let (mut base, mut rough) = if pick < 0.38 {
        ([0.30, 0.085, 0.048], 0.84)
    } else if pick < 0.6 {
        ([0.40, 0.14, 0.07], 0.86)
    } else if pick < 0.78 {
        ([0.21, 0.08, 0.05], 0.83)
    } else if pick < 0.9 {
        ([0.45, 0.2, 0.12], 0.87)
    } else {
        ([0.085, 0.042, 0.036], 0.6)
    };
    base = mul(base, 0.8 + 0.4 * rv(11));
    base[1] *= 0.9 + 0.2 * rv(12);
    // Kiln flashing: one end darker on some bricks.
    let flash_dir = if rv(13) < 0.5 { -1.0 } else { 1.0 };
    let flash = smooth(0.2, 1.0, flash_dir * px / hw) * smooth(0.6, 1.0, rv(14)) * 0.35;
    let mottle = t.fbm(x, y, 28.0, 4, 0.5, sd(s, 10));
    let mut c = mul(base, (1.0 + 0.16 * mottle) * (1.0 - flash));
    // Dark manganese specks and light sand grains.
    let sp = t.voronoi(x, y, 260.0, 1.0, sd(s, 11));
    let speck_k = t.keep(260.0);
    if rnd(sp.id, 1) < 0.07 {
        c = mul(c, 1.0 - 0.55 * speck_k * (1.0 - smooth(0.0005, 0.0012, sp.f1)));
    } else if rnd(sp.id, 1) > 0.9 {
        c = mul(c, 1.0 + 0.25 * speck_k);
    }
    // Fresh clay in chips, lime haze near the joints on some bricks.
    c = mix(c, mul(base, 1.25), chipped * 0.6);
    let haze = (1.0 - smooth(0.0, 0.006, d0)) * smooth(0.6, 0.9, rv(15)) * 0.35;
    c = mix(c, [0.4, 0.37, 0.33], haze);
    rough = lerp(rough, 0.9, chipped * 0.7);

    // Mortar color, darker and dirtier in the deep parts.
    let m_n = t.fbm(x, y, 40.0, 3, 0.5, sd(s, 12));
    let m_speck = t.n(x, y, 600.0, sd(s, 13)) * t.keep(600.0);
    let mortar_c = mul(
        [0.37, 0.35, 0.31],
        0.88 + 0.12 * m_n + 0.08 * m_speck + 30.0 * (mortar_h + 0.0062),
    );
    c = mix(mortar_c, c, is_brick);
    rough = lerp(0.93, rough + 0.04 * m_n, is_brick);

    // Weathering over the whole wall: soot and rain streaks, efflorescence low on bricks.
    let streak = t.fbm2(x, y, 7.0, 0.7, 4, 0.5, sd(s, 14));
    c = mul(c, 1.0 - 0.2 * smooth(0.0, 0.6, streak));
    let eff = smooth(0.35, 0.8, t.fbm(x, y, 8.0, 3, 0.5, sd(s, 15)))
        * (1.0 - smooth(0.0, 0.02, py + hh))
        * is_brick;
    c = mix(c, [0.55, 0.54, 0.5], eff * 0.4);
    Texel {
        h,
        c,
        r: rough,
        m: 0.0,
        ao: 1.0,
    }
}

/// Ashlar block pitch along u and v, meters.
const STONE_PITCH: [f32; 2] = [0.9, 0.45];
/// Ashlar joint width, meters.
const STONE_JOINT: f32 = 0.015;

/// Cut ashlar: 4 blocks × 4 courses per 3.6 × 1.8 m tile, drafted margins, pitched faces.
fn stone(t: &Tile, s: u64, x: f32, y: f32) -> Texel {
    let (col, course, lx, ly) = bond(x, y, STONE_PITCH, 4, 4);
    let id = hash2(col, course, sd(s, 1));
    let rv = |k| rnd(id, k);
    let jw = STONE_JOINT * 0.5;
    let px = lx - STONE_PITCH[0] * 0.5 - (rv(1) - 0.5) * 0.004;
    let py = ly - STONE_PITCH[1] * 0.5 - (rv(2) - 0.5) * 0.003;
    let hw = STONE_PITCH[0] * 0.5 - jw - rv(3) * 0.002;
    let hh = STONE_PITCH[1] * 0.5 - jw - rv(4) * 0.0015;
    let edge_n = t.fbm(x, y, 40.0, 4, 0.55, sd(s, 2));
    let d0 = rrect_inside(px, py, hw, hh, 0.006) + edge_n * 0.002;

    // Chipped arrises and broken corners.
    let chip_n = t.fbm(x, y, 18.0, 4, 0.55, sd(s, 3)) * 0.5 + 0.5;
    let corner = 1.0 - smooth(0.0, 0.08, (hw - px.abs()).min(0.08) + (hh - py.abs()).min(0.08));
    let chip = (smooth(0.55, 0.8, chip_n) * (0.4 + rv(5)) + corner * rv(6) * 0.8) * 0.03;

    // Drafted margin (tooled band along the edges) around a pitched, rock-faced field.
    let margin = 0.03 + 0.015 * rv(7);
    let bulge = 0.006 + 0.016 * rv(8);
    let into_field = (d0 - margin).max(0.0);
    let field = smooth(0.0, 0.006, d0 - margin);
    let crown = bulge * (1.0 - (-into_field / 0.07).exp());
    let facets = t.voronoi(x, y, 14.0, 0.9, sd(s, 4));
    let facet_h = (rnd(facets.id, 1) - 0.5) * 0.4 * facets.dx
        + (rnd(facets.id, 2) - 0.5) * 0.4 * facets.dy
        + (rnd(facets.id, 3) - 0.5) * 0.004;
    let rock = t.fbm(x, y, 12.0, 6, 0.5, sd(s, 5)) * 0.004;
    let pitched = (crown + (facet_h + rock) * smooth(0.0, 0.04, into_field)) * field;
    // Fine chisel strokes across the margin, perpendicular to the nearest edge.
    let along = if hw - px.abs() < hh - py.abs() { y } else { x };
    let strokes = (along * std::f32::consts::TAU / 0.004).sin()
        * (0.5 + 0.5 * t.n(x, y, 30.0, sd(s, 6)))
        * t.keep(250.0);
    let margin_h = strokes * 0.00015 * (1.0 - field);
    let grain = t.fbm(x, y, 150.0, 3, 0.5, sd(s, 7)) * 0.0003;
    let face = pitched + margin_h + grain + (rv(9) - 0.5) * 0.004;
    // Worn arris: rounded over ~6 mm; chip scoops.
    let br = 0.006;
    let bevel = if d0 >= br {
        0.0
    } else if d0 > 0.0 {
        -(br - (br * br - (br - d0) * (br - d0)).max(0.0).sqrt())
    } else {
        -br + d0 * 3.0
    };
    let chip_floor = -(chip - d0).max(0.0) * 0.6 + t.fbm(x, y, 90.0, 3, 0.5, sd(s, 8)) * 0.0015;
    let chipped = smooth(0.0, 0.002, chip - d0);
    let block_h = face + bevel.min(chip_floor);

    // Lime mortar recessed ~12 mm, eroded into pockets in places.
    let erode = smooth(0.25, 0.6, t.fbm(x, y, 9.0, 4, 0.5, sd(s, 9)));
    let mortar_h = -0.011 - 0.014 * erode + t.fbm(x, y, 300.0, 3, 0.5, sd(s, 10)) * 0.0008;
    let h = block_h.max(mortar_h);
    let is_block = smooth(-0.0005, 0.001, block_h - mortar_h);

    // Color: weathered limestone/sandstone, per block.
    let pick = rv(10);
    let base = if pick < 0.35 {
        [0.30, 0.285, 0.255]
    } else if pick < 0.6 {
        [0.34, 0.30, 0.24]
    } else if pick < 0.8 {
        [0.26, 0.255, 0.245]
    } else if pick < 0.93 {
        [0.38, 0.31, 0.22]
    } else {
        [0.2, 0.19, 0.175]
    };
    let base = mul(base, 0.88 + 0.24 * rv(11));
    let mottle = t.fbm(x, y, 6.0, 5, 0.55, sd(s, 11));
    let speck = t.n(x, y, 400.0, sd(s, 12)) * t.keep(400.0);
    let mut c = mul(base, 1.0 + 0.14 * mottle + 0.06 * speck);
    // Exposed bosses bleach, recesses collect grime.
    c = mul(c, 0.86 + 0.28 * smooth(-0.004, bulge, face));
    // Iron spots, rare.
    let iron = t.voronoi(x, y, 30.0, 1.0, sd(s, 13));
    if rnd(iron.id, 1) < 0.08 {
        let k = 1.0 - smooth(0.004, 0.012 + 0.01 * rnd(iron.id, 2), iron.f1);
        c = mix(c, [0.3, 0.15, 0.06], k * 0.5);
    }
    // Rain streaks running down, heavier below joints.
    let streak = t.fbm2(x, y, 6.0, 0.55, 5, 0.5, sd(s, 14));
    let under_joint = 1.0 - smooth(0.0, 0.25, hh - py);
    c = mul(
        c,
        1.0 - 0.3 * smooth(0.0, 0.6, streak) - 0.12 * under_joint * smooth(-0.2, 0.3, streak),
    );
    c = mix(c, mul(base, 1.12), chipped * 0.5);
    let mut rough = 0.84 + 0.06 * mottle - 0.08 * (1.0 - field) * is_block + 0.04 * chipped;

    // Mortar: pale lime, darker in pockets, moss deep in some joints.
    let m_n = t.fbm(x, y, 25.0, 3, 0.5, sd(s, 15));
    let mut mortar_c = mul([0.42, 0.40, 0.35], 0.85 + 0.12 * m_n - 0.4 * erode);
    let moss = smooth(0.2, 0.6, t.fbm(x, y, 5.0, 4, 0.5, sd(s, 16))) * erode;
    mortar_c = mix(mortar_c, [0.05, 0.075, 0.025], moss * 0.85);
    c = mix(mortar_c, c, is_block);
    rough = lerp(0.95, rough, is_block);

    // Lichen colonies on the faces.
    let (cover, lc, lh) = lichen(t, sd(s, 17), x, y, 4.0, 0.35);
    let cover = cover * is_block * smooth(-0.3, 0.2, t.n(x, y, 1.0, sd(s, 18)));
    c = mix(c, lc, cover * 0.9);
    rough = lerp(rough, 0.95, cover);
    Texel {
        h: h + lh * is_block,
        c,
        r: rough,
        m: 0.0,
        ao: 1.0,
    }
}

/// Sawn timber, grain along u.
fn wood(t: &Tile, s: u64, x: f32, y: f32) -> Texel {
    // Ring coordinate across the grain: y bent by warps of several scales; where the warp
    // folds, flat-sawn "cathedral" arches appear.
    let mut ring_y = y
        + 0.035 * t.n2(x, y, 1.0, 2.0, sd(s, 1))
        + 0.022 * t.n2(x, y, 2.0, 6.0, sd(s, 2))
        + 0.003 * t.n2(x, y, 8.0, 20.0, sd(s, 3));
    // Knots: the grain flows around them.
    let mut knot = 0.0f32;
    let mut knot_r = 0.0f32;
    t.scatter(x, y, 1.8, sd(s, 4), |h, dx, dy| {
        if rnd(h, 1) < 0.55 {
            let kr = 0.006 + 0.014 * rnd(h, 2);
            let e = (dx / (kr * 1.5)).hypot(dy / kr);
            let sign = if dy > 0.0 { 1.0 } else { -1.0 };
            ring_y += sign * kr * 2.2 * (-(e / 2.5).powi(2)).exp();
            let k = 1.0 - smooth(0.85, 1.05, e);
            if k > knot {
                knot = k;
                knot_r = e;
            }
        }
    });
    let spacing = 0.0034 * (1.0 + 0.35 * t.n2(x, y, 1.0, 3.0, sd(s, 5)));
    let f = (ring_y / spacing).rem_euclid(1.0);
    let contrast = t.keep(1.0 / spacing);
    let late = (smooth(0.5, 0.88, f) * (1.0 - smooth(0.92, 1.0, f)) - 0.3) * contrast + 0.3;
    // Fibers and pores: fine streaks along the grain.
    let fiber = t.fbm2(x, ring_y, 9.0, 380.0, 3, 0.6, sd(s, 6));
    // Checks: long thin drying cracks along the grain.
    let check_n = t.n2(x, y, 2.0, 30.0, sd(s, 7));
    let check_mask = smooth(0.2, 0.6, t.n2(x, y, 1.5, 2.0, sd(s, 8)));
    let check = (1.0 - smooth(0.0, 0.035, check_n.abs())) * check_mask;
    // Saw marks across the board, faint.
    let saw = (x * std::f32::consts::TAU / 0.0065 + 3.0 * t.n2(x, y, 3.0, 4.0, sd(s, 9))).sin()
        * smooth(0.0, 0.6, t.n2(x, y, 2.0, 3.0, sd(s, 10)))
        * t.keep(160.0);
    // Weathering: grey, raised latewood.
    let weather = smooth(-0.35, 0.45, t.fbm(x, y, 2.5, 4, 0.5, sd(s, 11)));

    let mut h = (late - 0.5) * (0.00012 + 0.0003 * weather)
        + fiber * 0.00004
        + saw * 0.00006
        - check * 0.0016
        + t.n2(x, y, 3.0, 2.0, sd(s, 12)) * 0.0004;
    let early_c = [0.40, 0.235, 0.12];
    let late_c = [0.17, 0.085, 0.04];
    let tone = 0.85 + 0.3 * t.n2(x, y, 1.5, 2.0, sd(s, 13));
    let mut c = mul(mix(early_c, late_c, late), tone * (1.0 + 0.1 * fiber));
    let grey = mix([0.27, 0.25, 0.22], [0.15, 0.14, 0.12], late);
    c = mix(c, grey, weather * 0.65);
    let mut rough = lerp(0.68, 0.58, late) + 0.18 * weather;
    // Knot: dark, its own tight rings, a radial crack, slightly proud.
    if knot > 0.0 {
        let kf = (knot_r * 5.0).rem_euclid(1.0);
        let kc = mul([0.12, 0.06, 0.03], 0.8 + 0.4 * smooth(0.3, 0.9, kf));
        c = mix(c, kc, knot * 0.9);
        h += knot * 0.0002 - knot * (1.0 - smooth(0.0, 0.08, knot_r)) * 0.0008;
        rough = lerp(rough, 0.55, knot);
    }
    c = mix(c, [0.05, 0.03, 0.02], check * 0.85);
    rough = lerp(rough, 0.95, check);
    Texel {
        h,
        c,
        r: rough,
        m: 0.0,
        ao: 1.0,
    }
}

/// Natural cliff rock (granite/limestone): facets, cracks, strata, speckle, lichen.
fn rock(t: &Tile, s: u64, x: f32, y: f32) -> Texel {
    let wx = x + 0.12 * t.fbm(x, y, 1.0, 3, 0.5, sd(s, 1));
    let wy = y + 0.12 * t.fbm(x, y, 1.0, 3, 0.5, sd(s, 2));
    // Big fracture facets: each Voronoi cell is a tilted plane, so cell borders become
    // ledges and steps.
    let big = t.voronoi(wx, wy, 2.2, 0.9, sd(s, 3));
    let plane = |v: &Voro, slope: f32, step: f32| {
        (rnd(v.id, 1) - 0.5) * slope * v.dx
            + (rnd(v.id, 2) - 0.5) * slope * v.dy
            + (rnd(v.id, 3) - 0.5) * step
    };
    let mid = t.voronoi(wx, wy, 7.0, 0.9, sd(s, 4));
    let small = t.voronoi(wx, wy, 22.0, 0.9, sd(s, 5));
    let mut h = plane(&big, 0.5, 0.05) + plane(&mid, 0.35, 0.012) + plane(&small, 0.2, 0.003);
    h += t.fbm(x, y, 5.0, 7, 0.5, sd(s, 6)) * 0.012;
    // Strata: bedding planes every ~0.28 m, gently warped.
    let bed_y = y + 0.05 * t.n2(x, y, 0.5, 1.0, sd(s, 7));
    let bed_f = (bed_y * 3.5).rem_euclid(1.0);
    let bed = 1.0 - smooth(0.0, 0.012, (bed_f - 0.5).abs() * (1.0 / 3.5));
    h -= bed * 0.006;
    h += smooth(0.45, 0.55, bed_f) * 0.004;
    // Cracks along the big fractures (not all) and fine hairlines.
    let crack_mask = smooth(-0.15, 0.25, t.n(x, y, 1.5, sd(s, 8)));
    let cw = 0.002 + 0.003 * (t.n(x, y, 6.0, sd(s, 9)) * 0.5 + 0.5);
    let crack = (1.0 - smooth(cw * 0.3, cw, big.edge)) * crack_mask;
    let hair_v = t.voronoi(wx, wy, 9.0, 1.0, sd(s, 10));
    let hair = (1.0 - smooth(0.0, 0.0012, hair_v.edge))
        * smooth(0.1, 0.5, t.n(x, y, 3.0, sd(s, 11)))
        * t.keep(400.0);
    h -= crack * 0.02 + hair * 0.003;

    // Color: grey granite/limestone, per facet tone, strata bands, mineral grains.
    let band_v = t.n2(x, bed_y, 0.3, 4.5, sd(s, 12)) + 0.5 * t.n2(x, bed_y, 0.6, 11.0, sd(s, 13));
    let mut c = mul([0.27, 0.26, 0.24], 0.9 + 0.2 * rnd(big.id, 4));
    c = mul3(c, [1.0 + 0.08 * band_v, 1.0 + 0.04 * band_v, 1.0 - 0.02 * band_v]);
    c = mul(c, 1.0 + 0.12 * t.fbm(x, y, 3.0, 5, 0.55, sd(s, 14)));
    let grain = t.voronoi(x, y, 180.0, 1.0, sd(s, 15));
    let gk = t.keep(180.0);
    let g = rnd(grain.id, 1);
    let mut rough = 0.8 + 0.08 * t.n(x, y, 30.0, sd(s, 16));
    if g < 0.14 {
        c = mul(c, 1.0 - 0.6 * gk); // mica / hornblende
    } else if g > 0.86 {
        c = mul(c, 1.0 + 0.4 * gk); // feldspar
    } else if g > 0.78 {
        c = mul3(c, [1.0 + 0.1 * gk, 1.0 + 0.1 * gk, 1.0 + 0.12 * gk]); // quartz
        rough -= 0.25 * gk;
    }
    c = mul(c, 1.0 - 0.25 * bed);
    // Weathering: dark streaks below ledges, dirt in cracks, pale exposed edges.
    let streak = t.fbm2(x, y, 5.0, 0.6, 5, 0.5, sd(s, 17));
    c = mul(c, 1.0 - 0.28 * smooth(0.05, 0.6, streak));
    c = mix(c, [0.08, 0.065, 0.05], crack * 0.8 + hair * 0.4);
    c = mul(c, 1.0 + 0.1 * smooth(0.0002, 0.003, big.edge.min(0.003)) - 0.1);
    let (cover, lc, lh) = lichen(t, sd(s, 18), x, y, 2.5, 0.4);
    let cover = cover * smooth(-0.2, 0.3, t.n(x, y, 0.8, sd(s, 19))) * (1.0 - crack);
    c = mix(c, lc, cover * 0.85);
    rough = lerp(rough, 0.95, cover.max(crack));
    Texel {
        h: h + lh,
        c,
        r: rough,
        m: 0.0,
        ao: 1.0 - 0.4 * crack,
    }
}

/// Bare soil.
fn dirt(t: &Tile, s: u64, x: f32, y: f32) -> Texel {
    // Clods and footprint-scale unevenness.
    let clod = t.fbm(x, y, 6.0, 7, 0.5, sd(s, 1));
    let crumbs = t.voronoi(x, y, 90.0, 1.0, sd(s, 2));
    let crumb_h = (1.0 - smooth(0.0, 0.6 / 90.0, crumbs.f1)) * (0.4 + 0.6 * rnd(crumbs.id, 1));
    let mut h = clod * 0.012 + crumb_h * 0.0015 * t.keep(90.0);
    // Damp patches: darker, smoother, flatter.
    let damp = smooth(-0.1, 0.45, t.fbm(x, y, 1.2, 4, 0.5, sd(s, 3)));
    // Drying cracks on the dry ground.
    let cr = t.voronoi(x, y, 9.0, 0.9, sd(s, 4));
    let crack_w = 0.0015 + 0.0015 * (t.n(x, y, 20.0, sd(s, 5)) * 0.5 + 0.5);
    let crack = (1.0 - smooth(crack_w * 0.3, crack_w, cr.edge))
        * (1.0 - damp)
        * smooth(0.0, 0.4, t.n(x, y, 2.0, sd(s, 6)));
    h -= crack * 0.004;
    // Pebbles and grit.
    let mut stone = 0.0f32;
    let mut stone_c = [0.0; 3];
    let mut stone_h = -1.0f32;
    for (freq, prob, rmin, rmax, salt) in [
        (22.0, 0.3, 0.004, 0.012, 7),
        (70.0, 0.25, 0.0015, 0.0035, 8),
    ] {
        t.scatter(x, y, freq, sd(s, salt), |hh, dx, dy| {
            if rnd(hh, 1) > prob {
                return;
            }
            let rad = rmin + (rmax - rmin) * rnd(hh, 2);
            let e = (dx / rad).hypot(dy / (rad * (0.6 + 0.4 * rnd(hh, 3))));
            if e < 1.0 {
                let top = (1.0 - e * e).sqrt() * rad * 0.6 - rad * 0.2;
                if top > stone_h {
                    stone_h = top;
                    stone = 1.0 - smooth(0.85, 1.0, e);
                    let g = rnd(hh, 4);
                    stone_c = mul([0.3, 0.27, 0.23], 0.5 + 0.8 * g);
                }
            }
        });
    }
    let h_ground = h;
    if stone_h > -1.0 {
        h = h.max(h_ground + stone_h);
    }
    // Twigs and rootlets.
    let mut twig = 0.0f32;
    t.scatter(x, y, 5.0, sd(s, 9), |hh, dx, dy| {
        if rnd(hh, 1) < 0.6 {
            let (d, a) = segment(dx, dy, rnd(hh, 2) * 3.2, 0.02 + 0.06 * rnd(hh, 3));
            let w = 0.0008 + 0.0015 * rnd(hh, 4);
            let bend = 1.0 - a.abs().powi(3);
            twig = twig.max((1.0 - smooth(w * 0.5, w, d)) * bend);
        }
    });
    h += twig * 0.0012;

    let organic = t.fbm(x, y, 3.0, 5, 0.5, sd(s, 10));
    let fine = t.n(x, y, 500.0, sd(s, 11)) * t.keep(500.0);
    let dry = [0.16, 0.12, 0.085];
    let wet = [0.07, 0.05, 0.035];
    let mut c = mul(mix(dry, wet, damp * 0.8), 1.0 + 0.18 * organic + 0.1 * fine);
    c = mul(c, 0.85 + 0.25 * smooth(-0.01, 0.01, clod * 0.012));
    c = mix(c, [0.06, 0.045, 0.03], crack * 0.7);
    let is_stone = if stone_h > -1.0 && h > h_ground + 1e-5 {
        stone
    } else {
        0.0
    };
    c = mix(c, stone_c, is_stone);
    c = mix(c, [0.24, 0.17, 0.1], twig * 0.85);
    let rough = lerp(0.97, 0.78, damp) - 0.15 * is_stone;
    Texel {
        h,
        c,
        r: rough,
        m: 0.0,
        ao: 1.0,
    }
}

/// Painted steel plate with chipped paint, primer, scratches and rust.
fn metal(t: &Tile, s: u64, x: f32, y: f32) -> Texel {
    let loss_at = |x: f32, y: f32| {
        t.fbm(x, y, 3.0, 6, 0.55, sd(s, 1)) * 0.5 + 0.5 + 0.06 * t.n(x, y, 40.0, sd(s, 2))
    };
    let wear = loss_at(x, y);
    let bare = smooth(0.66, 0.665, wear);
    let primer = smooth(0.635, 0.64, wear) * (1.0 - bare);
    // Scratches through to bare steel.
    let mut scratch = 0.0f32;
    t.scatter(x, y, 9.0, sd(s, 3), |h, dx, dy| {
        if rnd(h, 1) < 0.6 {
            let ang = (rnd(h, 2) - 0.5) * 0.7 + if rnd(h, 3) < 0.3 { 1.2 } else { 0.0 };
            let (d, a) = segment(dx, dy, ang, 0.015 + 0.06 * rnd(h, 4));
            let w = 0.00015 + 0.0004 * rnd(h, 5);
            let fade = (1.0 - a.abs().powi(2)).max(0.0);
            scratch = scratch.max((1.0 - smooth(w * 0.5, w, d)) * fade);
        }
    });
    let steel = bare.max(scratch);
    // Rust grows in the bare spots and bleeds downward.
    let rust_n = t.fbm(x, y, 25.0, 4, 0.5, sd(s, 4)) * 0.5 + 0.5;
    let rust = bare * smooth(0.35, 0.65, rust_n);
    let mut bleed = 0.0f32;
    for (k, up) in [0.015f32, 0.04, 0.08].into_iter().enumerate() {
        let wob = 0.006 * t.n2(x, y, 25.0, 3.0, sd(s, 5 + k as u64));
        let above = smooth(0.66, 0.68, loss_at(x + wob, y + up));
        bleed = bleed.max(above * (1.0 - up / 0.1));
    }
    let bleed = bleed * smooth(-0.2, 0.4, t.n2(x, y, 60.0, 4.0, sd(s, 8))) * (1.0 - bare);

    // Height: paint layers (~0.15 mm) over the plate, slight orange peel, shallow dents.
    let peel = t.fbm(x, y, 250.0, 2, 0.5, sd(s, 9)) * t.keep(250.0);
    let dent = t.fbm(x, y, 4.0, 3, 0.5, sd(s, 10));
    let paint_h = (1.0 - bare) * 0.00008 + (1.0 - bare - primer).max(0.0) * 0.00008;
    let h = paint_h + peel * 0.00002 * (1.0 - bare) + dent * 0.0012 - scratch * 0.00006
        + rust * (rust_n - 0.4) * 0.0003;

    // Paint: neutral grey so vertex colors can tint it.
    let grime = t.fbm2(x, y, 6.0, 1.5, 4, 0.5, sd(s, 11));
    let paint = mul([0.32, 0.33, 0.33], 1.0 + 0.06 * t.fbm(x, y, 8.0, 3, 0.5, sd(s, 12)));
    let mut c = mul(paint, 1.0 - 0.18 * smooth(0.0, 0.6, grime));
    c = mix(c, [0.2, 0.06, 0.035], primer);
    c = mix(c, [0.22, 0.08, 0.03], bleed * 0.5);
    let brushed = t.n2(x, y, 12.0, 900.0, sd(s, 13)) * t.keep(900.0);
    let steel_c = mul([0.56, 0.57, 0.58], 0.9 + 0.08 * brushed);
    c = mix(c, steel_c, steel);
    let rust_c = mix([0.25, 0.09, 0.03], [0.13, 0.05, 0.02], rust_n);
    c = mix(c, rust_c, rust);
    let mut r = 0.42 + 0.06 * peel + 0.15 * smooth(0.0, 0.6, grime);
    r = lerp(r, 0.62, primer);
    r = lerp(r, 0.32 + 0.06 * brushed, steel);
    r = lerp(r, 0.88, rust.max(bleed * 0.5));
    let m = steel * (1.0 - rust);
    Texel {
        h,
        c,
        r,
        m,
        ao: 1.0,
    }
}

/// Loose gravel over fines.
fn gravel(t: &Tile, s: u64, x: f32, y: f32) -> Texel {
    let fines = t.fbm(x, y, 60.0, 3, 0.5, sd(s, 1));
    let base_h = -0.006 + fines * 0.001 + t.fbm(x, y, 3.0, 3, 0.5, sd(s, 2)) * 0.004;
    let mut h = base_h;
    let mut c = mul([0.15, 0.13, 0.11], 1.0 + 0.2 * fines);
    let mut rough = 0.95;
    let mut top = f32::MIN;
    for (freq, size, salt) in [(26.0, 1.0, 3u64), (48.0, 0.8, 4u64)] {
        t.scatter(x, y, freq, sd(s, salt), |hh, dx, dy| {
            let cell = 1.0 / freq;
            let a = cell * size * (0.42 + 0.22 * rnd(hh, 1));
            let b = a * (0.55 + 0.4 * rnd(hh, 2));
            let ang = rnd(hh, 3) * 6.283;
            let (sn, cs) = ang.sin_cos();
            let u = dx * cs + dy * sn;
            let v = -dx * sn + dy * cs;
            let e2 = (u / a).powi(2) + (v / b).powi(2);
            if e2 >= 1.0 {
                return;
            }
            let lumpy = 1.0 + 0.12 * t.n(x, y, freq * 3.0, sd(hh, 5));
            let height = b * (0.55 + 0.3 * rnd(hh, 4)) * (1.0 - e2).sqrt() * lumpy
                - b * 0.25
                + (rnd(hh, 6) - 0.5) * 0.004;
            if height > top {
                top = height;
                let pick = rnd(hh, 7);
                let tone = if pick < 0.35 {
                    [0.3, 0.29, 0.27]
                } else if pick < 0.55 {
                    [0.42, 0.40, 0.37]
                } else if pick < 0.72 {
                    [0.38, 0.32, 0.25]
                } else if pick < 0.84 {
                    [0.22, 0.16, 0.12]
                } else if pick < 0.94 {
                    [0.12, 0.12, 0.12]
                } else {
                    [0.6, 0.58, 0.55]
                };
                let mottle = t.n(x, y, freq * 4.0, sd(hh, 8));
                c = mul(tone, (0.85 + 0.3 * rnd(hh, 9)) * (1.0 + 0.12 * mottle));
                rough = 0.55 + 0.3 * rnd(hh, 10);
            }
        });
    }
    if top > base_h {
        h = top;
    }
    Texel {
        h,
        c,
        r: rough,
        m: 0.0,
        ao: 1.0,
    }
}

/// Window glass: almost flat; smudges, dust and fine scratches live in roughness.
fn glass(t: &Tile, s: u64, x: f32, y: f32) -> Texel {
    let wave = t.fbm(x, y, 3.0, 3, 0.5, sd(s, 1)) * 0.00004;
    let smudge = smooth(0.15, 0.55, t.fbm(x, y, 4.0, 5, 0.55, sd(s, 2)));
    let mut prints = 0.0f32;
    t.scatter(x, y, 6.0, sd(s, 3), |h, dx, dy| {
        if rnd(h, 1) < 0.25 {
            let e = (dx / 0.007).hypot(dy / 0.0095);
            prints = prints.max(1.0 - smooth(0.6, 1.0, e));
        }
    });
    let mut scratch = 0.0f32;
    t.scatter(x, y, 7.0, sd(s, 4), |h, dx, dy| {
        if rnd(h, 1) < 0.3 {
            let (d, a) = segment(dx, dy, rnd(h, 2) * 3.14, 0.01 + 0.05 * rnd(h, 3));
            let fade = (1.0 - a.abs().powi(2)).max(0.0);
            scratch = scratch.max((1.0 - smooth(0.0001, 0.0003, d)) * fade);
        }
    });
    let dust = smooth(0.55, 0.8, t.n(x, y, 300.0, sd(s, 5)) * 0.5 + 0.5) * t.keep(300.0);
    let c = mix([0.8, 0.86, 0.84], [0.6, 0.58, 0.54], dust * 0.4 + smudge * 0.06);
    let r = 0.04 + 0.14 * smudge + 0.18 * prints + 0.3 * scratch + 0.2 * dust;
    Texel {
        h: wave - scratch * 0.00002,
        c,
        r,
        m: 0.0,
        ao: 1.0,
    }
}

/// Ice: clear and frosted patches, internal fractures, bubbles.
fn ice(t: &Tile, s: u64, x: f32, y: f32) -> Texel {
    let melt = t.fbm(x, y, 2.0, 5, 0.5, sd(s, 1));
    let frost = smooth(0.0, 0.4, t.fbm(x, y, 1.5, 5, 0.55, sd(s, 2)));
    let grain = t.fbm(x, y, 180.0, 3, 0.5, sd(s, 3));
    // Surface cracks between ice crystals, and deeper fracture planes seen through the ice.
    let cells = t.voronoi(x, y, 5.0, 0.9, sd(s, 4));
    let crack = 1.0 - smooth(0.0003, 0.0015, cells.edge);
    let wx = x + 0.08 * t.n(x, y, 1.5, sd(s, 5));
    let wy = y + 0.08 * t.n(x, y, 1.5, sd(s, 6));
    let inner = t.voronoi(wx, wy, 3.0, 1.0, sd(s, 7));
    let fracture = (1.0 - smooth(0.0, 0.012, inner.edge)) * smooth(-0.2, 0.3, t.n(x, y, 2.0, sd(s, 8)));
    let mut bubbles = 0.0f32;
    t.scatter(x, y, 70.0, sd(s, 9), |h, dx, dy| {
        if rnd(h, 1) < 0.25 {
            let rad = 0.0005 + 0.0015 * rnd(h, 2);
            bubbles = bubbles.max(1.0 - smooth(rad * 0.6, rad, dx.hypot(dy)));
        }
    });
    let h = melt * 0.004 + grain * 0.0003 * frost - crack * 0.0006;
    let clear = [0.32, 0.53, 0.68];
    let mut c = mix(clear, [0.78, 0.86, 0.9], frost * 0.85);
    c = mul(c, 1.0 + 0.06 * melt);
    c = mix(c, [0.75, 0.84, 0.9], (fracture * 0.6 + bubbles * 0.5) * (1.0 - frost * 0.5));
    c = mix(c, [0.85, 0.9, 0.92], crack * 0.5);
    let r = lerp(0.06, 0.55, frost) + 0.1 * grain * frost + 0.2 * crack;
    Texel {
        h,
        c,
        r,
        m: 0.0,
        ao: 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(img: &Image) -> Vec<[f32; 4]> {
        img.data
            .chunks_exact(4)
            .map(|p| {
                [
                    p[0] as f32 / 255.0,
                    p[1] as f32 / 255.0,
                    p[2] as f32 / 255.0,
                    p[3] as f32 / 255.0,
                ]
            })
            .collect()
    }

    fn srgb_to_linear(v: f32) -> f32 {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }

    /// Mean absolute difference (0..255 levels) between two columns or rows of an image.
    fn line_diff(img: &Image, a: u32, b: u32, column: bool) -> f32 {
        let n = if column { img.height } else { img.width };
        let mut sum = 0.0;
        for i in 0..n {
            let (pa, pb) = if column {
                (img.pixel(a, i), img.pixel(b, i))
            } else {
                (img.pixel(i, a), img.pixel(i, b))
            };
            for k in 0..3 {
                sum += (pa[k] as f32 - pb[k] as f32).abs();
            }
        }
        sum / (n as f32 * 3.0)
    }

    #[test]
    fn every_kind_tiles_seamlessly() {
        for kind in SurfaceKind::ALL {
            let set = generate(kind, 64, 3);
            let (w, h) = (set.albedo.width, set.albedo.height);
            for (name, img) in [("albedo", &set.albedo), ("normal", &set.normal), ("orm", &set.orm)] {
                // The seam between the last and first column (row) must look like any other
                // pair of neighbors.
                let worst_cols = (0..w - 1)
                    .map(|c| line_diff(img, c, c + 1, true))
                    .fold(0.0, f32::max);
                let worst_rows = (0..h - 1)
                    .map(|r| line_diff(img, r, r + 1, false))
                    .fold(0.0, f32::max);
                let seam_c = line_diff(img, w - 1, 0, true);
                let seam_r = line_diff(img, h - 1, 0, false);
                assert!(seam_c <= worst_cols + 2.0, "{kind:?} {name} u seam {seam_c} vs {worst_cols}");
                assert!(seam_r <= worst_rows + 2.0, "{kind:?} {name} v seam {seam_r} vs {worst_rows}");
            }
            // Rendering from half a tile in must give the same image rolled by half: the
            // surface is periodic and every filter wraps.
            let shifted = render(kind, w, h, 3, [w / 2, h / 2]);
            for (a, b) in [
                (&set.albedo, &shifted.albedo),
                (&set.normal, &shifted.normal),
                (&set.orm, &shifted.orm),
            ] {
                let mut bad = 0;
                for y in 0..h {
                    for x in 0..w {
                        let pa = a.pixel((x + w / 2) % w, (y + h / 2) % h);
                        let pb = b.pixel(x, y);
                        if (0..4).any(|k| (pa[k] as i32 - pb[k] as i32).abs() > 3) {
                            bad += 1;
                        }
                    }
                }
                assert!(bad * 100 <= w * h, "{kind:?}: {bad} of {} texels differ", w * h);
            }
        }
    }

    #[test]
    fn normals_are_unit_and_face_out() {
        for kind in SurfaceKind::ALL {
            let set = generate(kind, 64, 5);
            let px = decode(&set.normal);
            let mut mean_z = 0.0;
            for p in &px {
                let n = [p[0] * 2.0 - 1.0, p[1] * 2.0 - 1.0, p[2] * 2.0 - 1.0];
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                assert!((len - 1.0).abs() < 0.03, "{kind:?}: |n| = {len}");
                assert!(n[2] > 0.0, "{kind:?}: normal faces into the surface");
                mean_z += n[2];
            }
            mean_z /= px.len() as f32;
            assert!(mean_z > 0.85, "{kind:?}: mean z {mean_z}");
        }
    }

    #[test]
    fn albedo_is_physically_plausible() {
        for kind in SurfaceKind::ALL {
            let set = generate(kind, 64, 9);
            assert!(set.albedo.srgb && !set.normal.srgb && !set.orm.srgb);
            let mut mean = [0.0f32; 3];
            let px = decode(&set.albedo);
            for p in &px {
                let l = [0, 1, 2].map(|k| srgb_to_linear(p[k]));
                let lum = 0.2126 * l[0] + 0.7152 * l[1] + 0.0722 * l[2];
                assert!((0.012..=0.9).contains(&lum), "{kind:?}: luminance {lum}");
                for k in 0..3 {
                    mean[k] += l[k] / px.len() as f32;
                }
            }
            if kind == SurfaceKind::Plaster {
                let lum = 0.2126 * mean[0] + 0.7152 * mean[1] + 0.0722 * mean[2];
                assert!((0.76..=0.87).contains(&lum), "plaster mean {lum}");
                let spread = mean.iter().fold(0.0f32, |a, b| a.max(*b))
                    - mean.iter().fold(1.0f32, |a, b| a.min(*b));
                assert!(spread < 0.04, "plaster is not neutral: {mean:?}");
            }
            let orm = decode(&set.orm);
            let metal = orm.iter().map(|p| p[2]).fold(0.0, f32::max);
            assert_eq!(metal > 0.5, kind == SurfaceKind::Metal, "{kind:?} metalness");
        }
    }

    #[test]
    fn masonry_joints_land_on_the_grid() {
        let tile = |k: SurfaceKind| {
            let [w, h] = k.tile_m();
            Tile { w, h, texel: 0.001 }
        };
        // Brick: joint at u = 0 on even courses (counted from the bottom), at half a brick
        // on odd ones, and along every course line.
        let t = tile(SurfaceKind::Brick);
        for seed in [1, 2, 3] {
            for course in 0..12 {
                let y = (course as f32 + 0.5) * BRICK_PITCH[1];
                let (joint, solid) = if course % 2 == 0 {
                    (0.0, BRICK_PITCH[0] * 0.5)
                } else {
                    (BRICK_PITCH[0] * 0.5, BRICK_PITCH[0])
                };
                for k in 0..8 {
                    let x0 = k as f32 * BRICK_PITCH[0];
                    let mortar = brick(&t, seed, x0 + joint, y).h;
                    let face = brick(&t, seed, x0 + solid, y).h;
                    assert!(mortar < -0.004, "course {course} brick {k}: joint h {mortar}");
                    assert!(face > -0.0025, "course {course} brick {k}: face h {face}");
                }
                let bed = brick(&t, seed, 0.06, course as f32 * BRICK_PITCH[1]).h;
                assert!(bed < -0.004, "bed joint below course {course}: {bed}");
            }
        }
        // Ashlar: the same layout at 0.9 × 0.45 m.
        let t = tile(SurfaceKind::Stone);
        for seed in [1, 2] {
            for course in 0..4 {
                let y = (course as f32 + 0.5) * STONE_PITCH[1];
                let (joint, solid) = if course % 2 == 0 {
                    (0.0, STONE_PITCH[0] * 0.5)
                } else {
                    (STONE_PITCH[0] * 0.5, STONE_PITCH[0])
                };
                for k in 0..4 {
                    let x0 = k as f32 * STONE_PITCH[0];
                    let mortar = stone(&t, seed, x0 + joint, y).h;
                    let face = stone(&t, seed, x0 + solid, y).h;
                    assert!(mortar < -0.009, "stone course {course}: joint h {mortar}");
                    assert!(face > -0.004, "stone course {course}: face h {face}");
                }
            }
        }
        // In the image: the bottom course is even (joint at u = 0), the next one odd.
        let set = generate(SurfaceKind::Brick, 256, 4);
        let (w, h) = (set.albedo.width, set.albedo.height);
        let texel = 1.8 / w as f32;
        let row_at = |y: f32| h - 1 - (y / texel) as u32;
        let col_at = |x: f32| (x / texel) as u32;
        let grey = |p: [u8; 4]| (p[0] as f32) < p[1] as f32 * 1.25 + 8.0;
        for (course, joint_x, brick_x) in [(0, 0.0, 0.1125), (1, 0.1125, 0.225)] {
            let row = row_at((course as f32 + 0.5) * 0.075);
            assert!(grey(set.albedo.pixel(col_at(joint_x), row)), "course {course} joint");
            assert!(!grey(set.albedo.pixel(col_at(brick_x), row)), "course {course} brick");
        }
    }

    #[test]
    fn sizes_seeds_and_names() {
        let a = generate(SurfaceKind::Wood, 32, 1);
        assert_eq!((a.albedo.width, a.albedo.height), (32, 16));
        assert_eq!(a.uv_scale(), [1.0, 2.0]);
        let b = generate(SurfaceKind::Wood, 32, 1);
        let c = generate(SurfaceKind::Wood, 32, 2);
        assert!(a.albedo.data == b.albedo.data && a.normal.data == b.normal.data);
        assert!(a.albedo.data != c.albedo.data);
        for k in SurfaceKind::ALL {
            assert_eq!(SurfaceKind::from_name(k.name()), Some(k));
            assert_eq!(serde_json::to_value(k).unwrap(), k.name());
        }
        let mut images = Assets::<Image>::default();
        let m = a.into_material(&mut images);
        assert_eq!(images.len(), 3);
        assert_eq!(m.uv_scale, [1.0, 2.0]);
        assert_eq!(m.metallic_roughness_texture, m.occlusion_texture);
    }
}
