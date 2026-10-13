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
    /// Short grass seen from above or at a slant: dense blades in tufts with soil and dead
    /// straw between them. Sits beside [`SurfaceKind::Dirt`] on grassy hill tops.
    Grass,
}

impl SurfaceKind {
    /// Every kind, in declaration order.
    pub const ALL: [SurfaceKind; 12] = [
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
        SurfaceKind::Grass,
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
            SurfaceKind::Grass => "grass",
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
            SurfaceKind::Grass => [1.5, 1.5],
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
    /// meters, so 1 is physically right; grass is softened (its blades overlap in steps that
    /// read too crisp on a flat ground plane).
    pub fn normal_scale(self) -> f32 {
        match self {
            SurfaceKind::Grass => 0.8,
            _ => 1.0,
        }
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
            SurfaceKind::Grass => 0.85,
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
        SurfaceKind::Grass => (grass, f(0.015, 0.9, 0.5)),
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
    // Horizon taps per direction: (column, row) offsets and 1 / distance in meters.
    let ao_taps: Vec<Vec<(isize, isize, f32)>> = DIRS8
        .iter()
        .map(|&(ddx, ddy)| {
            ao_steps
                .iter()
                .map(|&s| {
                    let off = |d: f32| (d * s).round() as isize;
                    (off(ddx), off(ddy), 1.0 / (s * dx_m))
                })
                .collect()
        })
        .collect();
    let (wi, hi) = (wu as isize, hu as isize);
    let mut out = vec![[0u8; 12]; wu * hu];
    par_rows(&mut out, wu, |r, row| {
        let at = |c: isize, rr: isize| {
            let c = if (0..wi).contains(&c) {
                c
            } else {
                c.rem_euclid(wi)
            };
            let rr = if (0..hi).contains(&rr) {
                rr
            } else {
                rr.rem_euclid(hi)
            };
            heights[rr as usize * wu + c as usize]
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
                for taps in &ao_taps {
                    let mut best = 0.0f32;
                    for &(oc, or, inv_dist) in taps {
                        best = best.max((at(ci + oc, ri + or) - h0) * inv_dist);
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

const SQRT_HALF: f32 = std::f32::consts::FRAC_1_SQRT_2;

const DIRS8: [(f32, f32); 8] = [
    (1.0, 0.0),
    (SQRT_HALF, SQRT_HALF),
    (0.0, 1.0),
    (-SQRT_HALF, SQRT_HALF),
    (-1.0, 0.0),
    (-SQRT_HALF, -SQRT_HALF),
    (0.0, -1.0),
    (SQRT_HALF, -SQRT_HALF),
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
            ((fx * self.w + 0.5) as i32).max(1),
            ((fy * self.h + 0.5) as i32).max(1),
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

    /// Ridged fractal noise: sharp crests where the noise crosses zero, about `0..1`.
    fn ridged(&self, x: f32, y: f32, f: f32, octaves: u32, gain: f32, seed: u64) -> f32 {
        let (mut sum, mut amp, mut norm, mut f) = (0.0, 1.0, 0.0, f);
        for i in 0..octaves {
            norm += amp;
            let k = band(f * self.texel);
            if k > 0.0 {
                let r = 1.0
                    - self
                        .n(x, y, f, seed.wrapping_add(i as u64 * 0x9E37_79B9))
                        .abs();
                sum += amp * k * r * r;
            }
            amp *= gain;
            f *= 2.0;
        }
        sum / norm
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
        let (ix, iy) = (floor_i(gx), floor_i(gy));
        let (fx, fy) = (gx - ix as f32, gy - iy as f32);
        for j in -1..=1 {
            for i in -1..=1 {
                let h = hash2(wrap(ix + i, px), wrap(iy + j, py), seed);
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
        let (ix, iy) = (floor_i(gx), floor_i(gy));
        let (fx, fy) = (gx - ix as f32, gy - iy as f32);
        let feature = |i: i32, j: i32| {
            let h = hash2(wrap(ix + i, px), wrap(iy + j, py), seed);
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
    [SQRT_HALF, SQRT_HALF],
    [0.382_683_4, 0.923_879_5],
    [0.0, 1.0],
    [-0.382_683_4, 0.923_879_5],
    [-SQRT_HALF, SQRT_HALF],
    [-0.923_879_5, 0.382_683_4],
    [-1.0, 0.0],
    [-0.923_879_5, -0.382_683_4],
    [-SQRT_HALF, -SQRT_HALF],
    [-0.382_683_4, -0.923_879_5],
    [0.0, -1.0],
    [0.382_683_4, -0.923_879_5],
    [SQRT_HALF, -SQRT_HALF],
    [0.923_879_5, -0.382_683_4],
];

/// Perlin gradient noise on a lattice that wraps every `px` × `py` cells.
fn perlin(x: f32, y: f32, px: i32, py: i32, seed: u64) -> f32 {
    let (x0, y0) = (floor_i(x), floor_i(y));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let ix = wrap(x0, px);
    let iy = wrap(y0, py);
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

/// `hypot` without the libm call (no overflow care needed at these magnitudes).
trait Hyp {
    fn hyp(self, other: f32) -> f32;
}

impl Hyp for f32 {
    #[inline]
    fn hyp(self, other: f32) -> f32 {
        (self * self + other * other).sqrt()
    }
}

/// `x.floor()` as an integer, without a libm call (for values well inside the i32 range).
#[inline]
fn floor_i(x: f32) -> i32 {
    let i = x as i32;
    if (i as f32) > x { i - 1 } else { i }
}

/// `i` wrapped into `0..n`; skips the division in the common in-range case.
#[inline]
fn wrap(i: i32, n: i32) -> i32 {
    if (0..n).contains(&i) {
        i
    } else {
        i.rem_euclid(n)
    }
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
    [
        lerp(a[0], b[0], t),
        lerp(a[1], b[1], t),
        lerp(a[2], b[2], t),
    ]
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
    let outside = qx.max(0.0).hyp(qy.max(0.0)) + qx.max(qy).min(0.0) - r;
    -outside
}

/// Distance from `(dx, dy)` (relative to a segment's center) to a segment of half-length
/// `half` along angle `ang`; also returns the position along it (-1..1).
fn segment(dx: f32, dy: f32, ang: f32, half: f32) -> (f32, f32) {
    let (s, c) = ang.sin_cos();
    let along = dx * c + dy * s;
    let across = -dx * s + dy * c;
    let a = along.clamp(-half, half);
    ((along - a).hyp(across), along / half)
}

/// Cell of a running bond with `pitch` (u, v) meters: `(column, course, local x, local y)`,
/// courses counted from y = 0 upward, odd courses offset by half a unit, indices wrapped to
/// `cols × rows`.
fn bond(x: f32, y: f32, pitch: [f32; 2], cols: i32, rows: i32) -> (i32, i32, f32, f32) {
    let course = (y / pitch[1]).floor();
    let ly = y - course * pitch[1];
    let k = course as i32;
    let off = if k.rem_euclid(2) == 1 {
        pitch[0] * 0.5
    } else {
        0.0
    };
    let bx = x + off;
    let col = (bx / pitch[0]).floor();
    let lx = bx - col * pitch[0];
    ((col as i32).rem_euclid(cols), k.rem_euclid(rows), lx, ly)
}

/// Lichen colonies: coverage 0..1, color and crust height (meters). About `freq` colony
/// sites per meter, `density` of them occupied.
fn lichen(t: &Tile, s: u64, x: f32, y: f32, freq: f32, density: f32) -> (f32, [f32; 3], f32) {
    let mut cover = 0.0f32;
    let mut color = [0.27, 0.3, 0.25];
    // Ragged rims; the noise moves them by at most half a radius, so it is only needed near
    // a colony.
    let mut edge_n = None;
    t.scatter(x, y, freq, sd(s, 2), |h, dx, dy| {
        if rnd(h, 1) > density {
            return;
        }
        let radius = (0.1 + 0.32 * rnd(h, 2).powi(2)) / freq;
        let dn = dx.hyp(dy) / radius;
        if dn >= 1.5 {
            return;
        }
        let d =
            dn + 0.5 * *edge_n.get_or_insert_with(|| t.fbm(x, y, freq * 6.0, 4, 0.55, sd(s, 1)));
        let c = 1.0 - smooth(0.6, 1.0, d);
        if c > cover {
            cover = c;
            let pick = rnd(h, 3);
            color = if pick < 0.5 {
                [0.27, 0.3, 0.25] // grey-green crust
            } else if pick < 0.82 {
                [0.32, 0.32, 0.3] // pale grey
            } else if pick < 0.97 {
                [0.33, 0.34, 0.2] // yellow-green
            } else {
                [0.3, 0.2, 0.11] // orange
            };
            color = mul(color, 0.85 + 0.3 * rnd(h, 4));
        }
    });
    // Crusts grow as lobes: the cover breaks into granules, most toward the rim.
    let lobe_f = freq * 45.0;
    if cover <= 0.0 || t.keep(lobe_f) <= 0.0 {
        return (cover, color, 0.0);
    }
    let lobes = t.voronoi(x, y, lobe_f, 1.0, sd(s, 3));
    let grain = 1.0 - smooth(0.25 / lobe_f, 0.6 / lobe_f, lobes.f1);
    let granular = t.keep(lobe_f) * (1.0 - cover * cover).max(0.3);
    let cover = cover * lerp(1.0, grain, granular);
    (cover, color, grain * 0.0003 * cover)
}

/// Rounded arris profile: 0 inside, dropping over `radius` toward the edge (`d` = inside
/// distance), then steeply down outside it.
fn arris(d: f32, radius: f32) -> f32 {
    if d >= radius {
        0.0
    } else if d > 0.0 {
        -(radius
            - (radius * radius - (radius - d) * (radius - d))
                .max(0.0)
                .sqrt())
    } else {
        -radius + d * 4.0
    }
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
    // Roller stipple ("orange peel"): soft nubs a few millimeters across, pocks between them
    // and a broader peel undulation.
    let cell = 1.0 / 170.0;
    let nub = if t.keep(170.0) > 0.0 {
        let v = t.voronoi(x, y, 170.0, 1.0, sd(s, 4));
        (1.0 - smooth(0.0, 0.8 * cell, v.f1)) * (0.5 + rnd(v.id, 1)) * t.keep(170.0)
    } else {
        0.0
    };
    let pock = t.fbm(x, y, 300.0, 2, 0.5, sd(s, 5));
    let peel = t.fbm(x, y, 40.0, 3, 0.5, sd(s, 6));
    let stip = (nub * 0.8 + 0.4 * pock) * lane_k * (1.0 - 0.7 * seam);
    let mut h = stip * 0.00036 + peel * 0.00016 * (1.0 - 0.5 * seam);
    // Long-wave unevenness of the board and the bulge of the joint compound.
    h += t.fbm(x, y, 2.5, 3, 0.5, sd(s, 7)) * 0.0006 + seam * seam * 0.0005;

    // Spackled screw heads on the studs (every 0.4 m) every 0.3 m up.
    let si = (x / 0.4).round();
    let sj = ((y - 0.15) / 0.3).round();
    let hs = hash2(
        (si as i32).rem_euclid(3),
        (sj as i32).rem_euclid(4),
        sd(s, 8),
    );
    let sdx = x - (si * 0.4 + (u01(hs) - 0.5) * 0.01);
    let sdy = y - (sj * 0.3 + 0.15 + (u01b(hs) - 0.5) * 0.03);
    let spot_r = sdx.hyp(sdy);
    let spot = 1.0 - smooth(0.006, 0.014, spot_r);
    h -= spot * 0.00025 * (1.0 - smooth(0.0, 0.005, spot_r)) - spot * 0.00005;

    // Dings: small dents from knocks.
    let mut ding = 0.0f32;
    t.scatter(x, y, 2.5, sd(s, 12), |hh, dx, dy| {
        if rnd(hh, 1) < 0.35 {
            let rad = 0.003 + 0.008 * rnd(hh, 2);
            let e = (dx / rad).hyp(dy / (rad * (0.5 + 0.5 * rnd(hh, 3))));
            if e < 1.0 {
                ding = ding.max((1.0 - e * e) * (0.0002 + 0.0007 * rnd(hh, 4)));
            }
        }
    });
    h -= ding;

    // Scuffs (burnished, greyish) and scratches.
    let mut scuff = 0.0f32;
    let mut scratch = 0.0f32;
    let breakup = t.fbm(x, y, 60.0, 3, 0.6, sd(s, 9)) * 0.5 + 0.5;
    t.scatter(x, y, 3.0, sd(s, 10), |hh, dx, dy| {
        let kind = rnd(hh, 1);
        if kind < 0.4 {
            let ang = (rnd(hh, 2) - 0.5) * 0.6;
            let (d, a) = segment(dx, dy, ang, 0.02 + 0.06 * rnd(hh, 3));
            let width = 0.004 + 0.01 * rnd(hh, 4);
            let k = (1.0 - smooth(0.0, width, d)) * (1.0 - a.abs().powi(4)).max(0.0);
            scuff = scuff.max(k * smooth(0.25, 0.7, breakup) * (0.4 + 0.6 * rnd(hh, 5)));
        } else if kind < 0.8 {
            let ang = (rnd(hh, 6) - 0.5) * 1.2 + if rnd(hh, 7) < 0.25 { 1.57 } else { 0.0 };
            let (d, a) = segment(dx, dy, ang, 0.03 + 0.12 * rnd(hh, 8));
            let width = 0.0003 + 0.0005 * rnd(hh, 9);
            let fade = (1.0 - a.abs().powi(2)).max(0.0);
            scratch = scratch.max((1.0 - smooth(0.0, width, d)) * fade);
        }
    });
    h -= scratch * 0.0001;

    let paint = [0.84, 0.835, 0.82];
    let mottle = t.fbm(x, y, 4.0, 3, 0.5, sd(s, 11)) * 0.015;
    let mut c = mul(
        paint,
        1.0 + mottle + 0.01 * stip + 0.006 * seam - 0.01 * spot,
    );
    c = mix(c, mul3(c, [0.8, 0.79, 0.78]), scuff * 0.6);
    c = mul(c, 1.0 - 0.08 * scratch);
    let r = 0.72 + 0.04 * pock - 0.08 * seam - 0.05 * spot - 0.2 * scuff + 0.05 * scratch;
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
    let (u, v) = (x.rem_euclid(t.w), y.rem_euclid(t.h));
    let seam = wrap_abs(x, t.w).min(wrap_abs(y, t.h));
    // Each panel leans a little, so neighbors meet with a ~1 mm lip at the seams.
    let lip = (u / t.w - 0.5) * 0.0012 - (v / t.h - 0.5) * 0.0008;
    // Fin of paste squeezed between panels, broken in places.
    let fin_w = 0.0015 + 0.001 * t.n(x, y, 30.0, sd(s, 1));
    let fin =
        (1.0 - smooth(fin_w * 0.4, fin_w, seam)) * smooth(-0.4, 0.3, t.n(x, y, 8.0, sd(s, 2)));
    // Grout leaking at the joint leaves a darker, honeycombed band.
    let bleed_w = 0.006 + 0.014 * (t.fbm(x, y, 6.0, 3, 0.5, sd(s, 3)) * 0.5 + 0.5);
    let bleed = 1.0 - smooth(0.0, bleed_w, seam);
    let honey = bleed * smooth(0.1, 0.5, t.fbm(x, y, 25.0, 3, 0.5, sd(s, 4)));

    // Plywood face grain printed into the surface (along u), with patch plugs.
    let gw = t.fbm2(x, y, 1.5, 3.0, 2, 0.5, sd(s, 5)) * 0.03;
    let grain = t.fbm2(x, y + gw, 1.5, 45.0, 5, 0.55, sd(s, 6));
    let grain_fine = t.n2(x, y + gw, 4.0, 220.0, sd(s, 7)) * t.keep(220.0);
    let mut plug = 0.0f32;
    t.scatter(x, y, 1.6, sd(s, 8), |h, dx, dy| {
        if rnd(h, 1) < 0.35 {
            let e = (dx / 0.032).hyp(dy / 0.012);
            plug = plug.max(1.0 - smooth(0.0, 0.08, (e - 1.0).abs()));
        }
    });
    let mut h = lip + grain * 0.0003 + grain_fine * 0.00008 - plug * 0.0003;
    h += t.fbm(x, y, 1.2, 4, 0.5, sd(s, 9)) * 0.001;
    h += fin * 0.0009 - honey * 0.002 * (0.5 + 0.5 * t.n(x, y, 120.0, sd(s, 10)));

    // Form ties every 0.6 m (centers at 0.3 + 0.6 i, 0.3 + 0.6 j): open cones with a rusty
    // rod end, cones filled with grout, or plastic caps.
    let gx = ((x - 0.3) / 0.6).round();
    let gy = ((y - 0.3) / 0.6).round();
    let tx = x - (0.3 + gx * 0.6);
    let ty = y - (0.3 + gy * 0.6);
    let th = hash2(
        (gx as i32).rem_euclid(4),
        (gy as i32).rem_euclid(2),
        sd(s, 11),
    );
    let tr = tx.hyp(ty);
    let style = rnd(th, 1);
    let cone_r = 0.016;
    let (mut hole, mut rod, mut patch, mut cap) = (0.0, 0.0, 0.0, 0.0);
    if style < 0.55 {
        hole = 1.0 - smooth(cone_r - 0.001, cone_r, tr);
        let depth = ((cone_r - tr) * 1.6).clamp(0.0, 0.022);
        rod = 1.0 - smooth(0.003, 0.004, tr);
        h -= depth * hole - rod * 0.008;
    } else if style < 0.8 {
        let pr = 0.019 + 0.003 * t.n(x, y, 140.0, sd(s, 12));
        patch = 1.0 - smooth(pr - 0.0015, pr, tr);
        h += patch * (-0.0018 + 0.0004 * t.n(x, y, 300.0, sd(s, 13)));
    } else {
        cap = 1.0 - smooth(cone_r - 0.0006, cone_r, tr);
        h -= cap * (0.003 + 0.0006 * (1.0 - smooth(0.0, 0.006, tr)));
    }
    // Ring left by the cone's washer.
    let ring = 1.0 - smooth(0.0, 0.0012, (tr - 0.024).abs());
    h -= ring * 0.0003;
    // Rust bleeding down from the open tie above (streaks are shorter than the tie spacing).
    let mut rust = 0.0;
    let gy_above = ((y - 0.3) / 0.6).ceil();
    let th_above = hash2(
        (gx as i32).rem_euclid(4),
        (gy_above as i32).rem_euclid(2),
        sd(s, 11),
    );
    if rnd(th_above, 1) < 0.55 {
        let below = 0.3 + gy_above * 0.6 - y;
        let len = 0.12 + 0.35 * rnd(th_above, 2);
        if below > 0.01 && below < len {
            let wiggle = 0.004 * t.n2(x, y, 20.0, 4.0, sd(s, 14));
            let width = 0.004 + below * 0.05;
            let across = (tx + wiggle).abs();
            let streaks = 0.5 + 0.5 * t.n2(x, y, 140.0, 6.0, sd(s, 15));
            rust = (1.0 - smooth(0.0, width, across))
                * (1.0 - below / len)
                * streaks
                * (0.35 + 0.65 * rnd(th_above, 3));
        }
    }

    // Bug holes: air voids against the form, a few millimeters across, in clusters and more
    // toward the top of the pour where air collects.
    let rise = v / t.h;
    let density = (0.05 + 0.3 * smooth(-0.1, 0.6, t.n(x, y, 2.5, sd(s, 16)))) * (0.4 + 0.9 * rise);
    let mut bug = 0.0f32;
    t.scatter(x, y, 50.0, sd(s, 17), |hh, dx, dy| {
        if rnd(hh, 1) < density {
            let rad = 0.0008 + 0.006 * rnd(hh, 2).powi(3);
            let d = dx.hyp(dy);
            if d < rad {
                let k = 1.0 - (d / rad).powi(2);
                bug = bug.max(k.sqrt() * rad * 0.9);
            }
        }
    });
    // Pinholes: sub-millimeter pores everywhere.
    let mut pin = 0.0f32;
    if t.keep(220.0) > 0.0 {
        t.scatter(x, y, 220.0, sd(s, 26), |hh, dx, dy| {
            if rnd(hh, 1) < 0.3 {
                let rad = 0.0003 + 0.0006 * rnd(hh, 2);
                let d = dx.hyp(dy);
                if d < rad {
                    pin = pin.max((1.0 - (d / rad).powi(2)).sqrt() * rad);
                }
            }
        });
    }
    h -= bug + pin * t.keep(220.0);

    // Color: cement paste with hydration mottling, faint aggregate, water run-off from the
    // top seam, lime streaks, rust, grout and caps.
    let cloud = t.fbm(x, y, 0.9, 6, 0.55, sd(s, 18));
    let blotch = smooth(0.15, 0.55, t.fbm(x, y, 2.2, 4, 0.5, sd(s, 19)));
    let agg_v = if t.keep(240.0) > 0.0 {
        (rnd(t.voronoi(x, y, 240.0, 1.0, sd(s, 20)).id, 1) - 0.5) * t.keep(240.0)
    } else {
        0.0
    };
    let speck = t.n(x, y, 700.0, sd(s, 21)) * t.keep(700.0);
    let blotch2 = smooth(0.25, 0.6, t.fbm(x, y, 4.5, 3, 0.5, sd(s, 27)));
    let mottle = 1.0 + 0.3 * (smooth(-0.4, 0.4, cloud) - 0.5) + 0.08 * cloud;
    let mut c = mul(
        [0.28, 0.28, 0.27],
        mottle
            * (1.0 + 0.08 * agg_v + 0.04 * speck + 0.1 * grain + 0.04 * grain_fine)
            * (1.0 - 0.2 * blotch)
            * (1.0 - 0.1 * blotch2),
    );
    let below_top = t.h - v;
    let run_len = 0.15 + 0.9 * (t.fbm2(x, 0.0, 6.0, 1.0, 3, 0.5, sd(s, 22)) * 0.5 + 0.5);
    let streak_n = t.fbm2(x, y, 9.0, 0.8, 4, 0.5, sd(s, 23));
    let streak_mask = smooth(0.0, 0.5, t.n2(x, y, 3.0, 0.8, sd(s, 28)));
    let run = (1.0 - smooth(run_len * 0.5, run_len, below_top))
        * smooth(0.0, 0.5, streak_n)
        * streak_mask;
    c = mul(c, 1.0 - 0.25 * run);
    let lime_n = t.fbm2(x, y, 24.0, 1.0, 3, 0.5, sd(s, 24));
    let lime = smooth(0.3, 0.65, lime_n)
        * (1.0 - smooth(0.03, 0.12 + 0.2 * rnd(th_above, 4), below_top))
        * smooth(0.0, 0.03, below_top)
        * smooth(0.1, 0.5, t.n(x, y, 2.0, sd(s, 25)));
    c = mix(c, [0.55, 0.55, 0.53], 0.45 * lime);
    c = mul(c, 1.0 - 0.2 * bleed - 0.12 * honey);
    c = mix(c, [0.35, 0.34, 0.32], patch * 0.85);
    c = mix(c, [0.05, 0.05, 0.05], cap * 0.9);
    c = mix(c, [0.24, 0.1, 0.04], 0.6 * rust);
    c = mix(c, [0.12, 0.06, 0.03], rod * 0.9);
    c = mul(c, 1.0 - 0.2 * hole);
    let r = 0.84 + 0.04 * speck + 0.04 * agg_v - 0.12 * run + 0.06 * honey + 0.04 * patch
        - 0.06 * fin
        - 0.35 * cap;
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
    let d0 = rrect_inside(px, py, hw, hh, 0.002) + t.fbm(x, y, 120.0, 3, 0.5, sd(s, 2)) * 0.0008;

    // Damage: a few bites out of the arrises, and broken corners.
    let chip_n = t.fbm(x, y, 40.0, 4, 0.5, sd(s, 3)) * 0.5 + 0.5;
    let edge_chip = smooth(0.62, 0.8, chip_n) * (0.3 + 1.2 * rv(5).powi(2)) * 0.012;
    let mut corner_chip = 0.0f32;
    for (k, (sx, sy)) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)]
        .into_iter()
        .enumerate()
    {
        let roll = rv(20 + k as u64);
        if roll < 0.22 {
            let size = 0.005 + 0.012 * (roll / 0.22);
            let r = (px - sx * hw).hyp(py - sy * hh);
            corner_chip = corner_chip.max(size - r + t.fbm(x, y, 150.0, 3, 0.5, sd(s, 18)) * 0.004);
        }
    }
    let chip_n2 = t.n(x, y, 200.0, sd(s, 4)) * 0.0015;
    let chip = edge_chip.max(corner_chip * 1.4 + chip_n2);

    // Face: sand-struck texture, slight tilt and warp, creases, pits from burnt inclusions.
    let tilt = (rv(7) - 0.5) * 0.0016 * px / hw + (rv(8) - 0.5) * 0.001 * py / hh;
    let warp = t.n(x, y, 20.0, sd(s, 5)) * 0.0004;
    let sand = t.fbm(x, y, 260.0, 3, 0.55, sd(s, 6)) * 0.0003;
    let crease_n = t.fbm(x, y, 14.0, 3, 0.5, sd(s, 7));
    let crease = (1.0 - smooth(0.0, 0.03, crease_n.abs())) * smooth(0.7, 0.95, rv(9));
    let mut pit = 0.0f32;
    let pit_rate = 0.15 + 0.35 * rv(10);
    t.scatter(x, y, 140.0, sd(s, 8), |hh2, dx, dy| {
        if rnd(hh2, 1) < pit_rate {
            let rad = 0.0006 + 0.0016 * rnd(hh2, 2);
            let d = dx.hyp(dy);
            if d < rad {
                pit = pit.max((1.0 - (d / rad).powi(2)).sqrt() * rad * 0.7);
            }
        }
    });
    let face = tilt + warp + sand - pit - crease * 0.0003;
    // Arris rounded over ~3 mm; chips scoop down toward the edge.
    let chip_floor = -(chip - d0).max(0.0) * 0.7 + t.n(x, y, 260.0, sd(s, 9)) * 0.0005;
    let chipped = smooth(0.0, 0.0012, chip - d0);
    let brick_h = face + arris(d0, 0.003).min(chip_floor);

    // Mortar: recessed ~7 mm with a slightly concave struck profile, sandy.
    let joint_fill = t.n2(x, y, 6.0, 6.0, sd(s, 10)) * 0.0012;
    let mortar_sand = t.fbm(x, y, 450.0, 2, 0.5, sd(s, 11)) * 0.0005;
    let concave = smooth(0.0, jw, -d0) * 0.0012;
    let mortar_h = -0.0062 + joint_fill + mortar_sand - concave;
    let h = brick_h.max(mortar_h);
    let is_brick = smooth(-0.0002, 0.0004, brick_h - mortar_h);

    // Brick color: mostly reds with oranges, browns, a few light and a few dark overfired.
    let pick = rv(12);
    let (base, mut rough) = if pick < 0.4 {
        ([0.31, 0.09, 0.05], 0.85)
    } else if pick < 0.62 {
        ([0.37, 0.125, 0.065], 0.86)
    } else if pick < 0.8 {
        ([0.24, 0.085, 0.05], 0.84)
    } else if pick < 0.93 {
        ([0.42, 0.18, 0.1], 0.87)
    } else {
        ([0.12, 0.045, 0.04], 0.62)
    };
    let base = mul(
        mul3(base, [1.0, 0.92 + 0.16 * rv(13), 1.0]),
        0.88 + 0.24 * rv(11),
    );
    // Kiln flashing: one end darker on some bricks.
    let flash_dir = if rv(14) < 0.5 { -1.0 } else { 1.0 };
    let flash = smooth(0.1, 1.0, flash_dir * px / hw) * smooth(0.55, 1.0, rv(15)) * 0.35;
    let mottle = t.fbm(x, y, 22.0, 4, 0.5, sd(s, 12));
    let mut c = mul(
        base,
        (1.0 + 0.14 * mottle + 0.5 * sand / 0.0003 * 0.08) * (1.0 - flash),
    );
    // Dark manganese specks and light sand grains.
    let speck_k = t.keep(260.0);
    if speck_k > 0.0 {
        let sp = t.voronoi(x, y, 260.0, 1.0, sd(s, 13));
        let spr = rnd(sp.id, 1);
        if spr < 0.06 {
            c = mul(
                c,
                1.0 - 0.55 * speck_k * (1.0 - smooth(0.0005, 0.0012, sp.f1)),
            );
        } else if spr > 0.88 {
            c = mul(c, 1.0 + 0.22 * speck_k);
        }
    }
    c = mul(c, 1.0 - 0.08 * crease);
    // Fresh clay in chips, lime haze near the joints on some bricks.
    c = mix(c, mul(base, 1.3), chipped * 0.55);
    let haze = (1.0 - smooth(0.0, 0.006, d0)) * smooth(0.6, 0.9, rv(16)) * 0.3;
    c = mix(c, [0.36, 0.34, 0.3], haze);
    rough = lerp(rough, 0.92, chipped * 0.7);

    // Mortar color, darker and dirtier in the deep parts.
    let m_n = t.fbm(x, y, 40.0, 3, 0.5, sd(s, 14));
    let m_speck = t.n(x, y, 600.0, sd(s, 15)) * t.keep(600.0);
    let mortar_c = mul(
        [0.27, 0.255, 0.23],
        0.86 + 0.12 * m_n + 0.1 * m_speck + 25.0 * (mortar_h + 0.0062),
    );
    c = mix(mortar_c, c, is_brick);
    rough = lerp(0.94, rough + 0.04 * mottle, is_brick);

    // Weathering over the whole wall: soot and rain streaks, efflorescence low on bricks.
    let streak = t.fbm2(x, y, 7.0, 0.7, 4, 0.5, sd(s, 16));
    c = mul(c, 1.0 - 0.18 * smooth(0.0, 0.6, streak));
    let eff = smooth(0.4, 0.8, t.fbm(x, y, 8.0, 3, 0.5, sd(s, 17)))
        * (1.0 - smooth(0.0, 0.02, py + hh))
        * is_brick;
    c = mix(c, [0.5, 0.49, 0.46], eff * 0.35);
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

/// Weathered ashlar: 4 blocks × 4 courses per 3.6 × 1.8 m tile, pillowed and pitched faces,
/// tooled strokes, spalls, recessed lime mortar, streaks and lichen.
fn stone(t: &Tile, s: u64, x: f32, y: f32) -> Texel {
    let (col, course, lx, ly) = bond(x, y, STONE_PITCH, 4, 4);
    let id = hash2(col, course, sd(s, 1));
    let rv = |k| rnd(id, k);
    let jw = STONE_JOINT * 0.5;
    let px = lx - STONE_PITCH[0] * 0.5 - (rv(1) - 0.5) * 0.004;
    let py = ly - STONE_PITCH[1] * 0.5 - (rv(2) - 0.5) * 0.003;
    let hw = STONE_PITCH[0] * 0.5 - jw - rv(3) * 0.002;
    let hh = STONE_PITCH[1] * 0.5 - jw - rv(4) * 0.0015;
    // Weathered outline: the arrises wander by a few millimeters.
    let outline = t.fbm(x, y, 22.0, 4, 0.55, sd(s, 2)) * 0.004;
    let d0 = rrect_inside(px, py, hw, hh, 0.015) + outline;

    // Spalls: some arrises and corners have lost chunks.
    let spall_n = t.fbm(x, y, 9.0, 4, 0.5, sd(s, 3)) * 0.5 + 0.5;
    let (ex, ey) = (hw - px.abs(), hh - py.abs());
    let corner = 1.0 - smooth(0.0, 0.12, ex.min(0.12) + ey.min(0.12));
    let spall = (smooth(0.52, 0.78, spall_n) * (0.3 + 0.9 * rv(5))
        + corner * smooth(0.3, 1.0, rv(6)))
        * 0.04;

    // Pillowed face: weathering rounds the block over a few centimeters; the middle is
    // gently pitched with rock-faced relief.
    let dpos = d0.max(0.0);
    let pillow = 0.012 * (1.0 - (-dpos / 0.045).exp());
    let crown = (0.002 + 0.01 * rv(7)) * (dpos / hh).min(1.0).powf(0.7);
    let relief_k = 0.4 + rv(8);
    let relief = t.fbm(x, y, 5.0, 7, 0.5, sd(s, 4)) * 0.006 * relief_k;
    let ridges = t.ridged(x, y, 9.0, 4, 0.5, sd(s, 5)) * 0.004 * relief_k;
    // Boasted finish: parallel chisel strokes at a per-block angle on about half the blocks.
    let ang = (rv(9) - 0.5) * 1.2 + 0.5;
    let (sa, ca) = ang.sin_cos();
    let along = px * ca + py * sa;
    let tooled = smooth(0.4, 0.6, rv(10)) * t.keep(1.0 / 0.008);
    let strokes = ((along / 0.008 + 0.6 * t.n(x, y, 25.0, sd(s, 6))) * std::f32::consts::TAU).sin()
        * 0.0003
        * tooled
        * (0.5 + 0.5 * t.n(x, y, 12.0, sd(s, 7)));
    // Weathering pits (alveoli), heavier on some blocks.
    let pit_rate = (0.02 + 0.4 * rv(11).powi(3)) * smooth(-0.1, 0.5, t.n(x, y, 4.0, sd(s, 21)));
    let mut pit = 0.0f32;
    t.scatter(x, y, 70.0, sd(s, 8), |h, dx, dy| {
        if rnd(h, 1) < pit_rate {
            let rad = 0.0015 + 0.0045 * rnd(h, 2).powi(2);
            let d = dx.hyp(dy);
            if d < rad {
                pit = pit.max((1.0 - (d / rad).powi(2)).sqrt() * rad * 0.35);
            }
        }
    });
    let grain = t.fbm(x, y, 220.0, 2, 0.5, sd(s, 9)) * 0.00025;
    let face = pillow + crown + relief + ridges + strokes + grain - pit;
    let spall_floor = -(spall - d0).max(0.0) * 0.5 + t.fbm(x, y, 60.0, 3, 0.5, sd(s, 10)) * 0.002;
    let spalled = smooth(0.0, 0.003, spall - d0);
    let block_h = face + arris(d0, 0.012).min(spall_floor);

    // Lime mortar recessed ~1 cm, eroded into deeper pockets in places.
    let erode = smooth(0.2, 0.65, t.fbm(x, y, 7.0, 4, 0.5, sd(s, 11)));
    let mortar_h = -0.016 - 0.014 * erode + t.fbm(x, y, 260.0, 3, 0.5, sd(s, 12)) * 0.0008;
    let h = block_h.max(mortar_h);
    let is_block = smooth(-0.0005, 0.001, block_h - mortar_h);

    // Color: weathered limestone and sandstone, per block.
    let pick = rv(12);
    let base = if pick < 0.35 {
        [0.27, 0.255, 0.23]
    } else if pick < 0.6 {
        [0.30, 0.275, 0.235]
    } else if pick < 0.8 {
        [0.23, 0.225, 0.215]
    } else if pick < 0.93 {
        [0.32, 0.28, 0.22]
    } else {
        [0.18, 0.17, 0.155]
    };
    let base = mul(base, 0.88 + 0.24 * rv(13));
    let mottle = t.fbm(x, y, 4.0, 6, 0.55, sd(s, 13));
    let fine = t.n(x, y, 350.0, sd(s, 14)) * t.keep(350.0);
    let mut c = mul(base, 1.0 + 0.18 * mottle + 0.06 * fine);
    // Exposed crowns bleach; rounded edges, pits and recesses gather grime.
    c = mul(
        c,
        0.8 + 0.28 * smooth(0.002, 0.016, pillow + crown + relief),
    );
    c = mul(c, 1.0 - 0.3 * smooth(0.0002, 0.0015, pit));
    // Rain washes dirt from the joints down the faces.
    let streak = t.fbm2(x, y, 5.0, 0.5, 5, 0.5, sd(s, 15));
    let under_joint = 1.0 - smooth(0.0, 0.18, hh - py);
    c = mul(
        c,
        1.0 - 0.25 * smooth(0.0, 0.6, streak) - 0.18 * under_joint * smooth(-0.2, 0.4, streak),
    );
    // A green algae film low on the blocks where damp sits above the bed joints.
    let damp = (1.0 - smooth(0.0, 0.1, py + hh)) * smooth(0.0, 0.5, t.n(x, y, 3.0, sd(s, 16)));
    c = mix(c, mul3(c, [0.7, 0.88, 0.55]), damp * 0.6);
    c = mix(c, mul(base, 1.15), spalled * 0.5);
    c = mul(
        c,
        1.0 - 0.15 * (1.0 - smooth(0.0, 0.05, d0)) * (1.0 - spalled),
    );
    let mut rough = 0.86 + 0.05 * mottle + 0.04 * spalled;

    // Mortar: pale lime, darker in pockets, moss deep in some joints.
    let m_n = t.fbm(x, y, 25.0, 3, 0.5, sd(s, 17));
    let mut mortar_c = mul(
        [0.34, 0.32, 0.28],
        (0.85 + 0.12 * m_n) * (1.0 - 0.45 * erode),
    );
    let moss = smooth(0.2, 0.6, t.fbm(x, y, 5.0, 4, 0.5, sd(s, 18))) * erode;
    mortar_c = mix(mortar_c, [0.045, 0.065, 0.02], moss * 0.85);
    c = mix(mortar_c, c, is_block);
    rough = lerp(0.95, rough, is_block);

    // Lichen colonies on the faces.
    let (cover, lc, lh) = lichen(t, sd(s, 19), x, y, 6.0, 0.25);
    let cover = cover * is_block * smooth(-0.3, 0.2, t.n(x, y, 1.0, sd(s, 20)));
    c = mix(c, lc, cover * 0.4);
    rough = lerp(rough, 0.95, cover);
    Texel {
        h: h + lh * is_block,
        c,
        r: rough,
        m: 0.0,
        ao: 1.0,
    }
}

/// Sawn softwood, grain along u: flat-sawn cathedral figure, uneven growth rings, knots,
/// checks, saw marks and weathering.
fn wood(t: &Tile, s: u64, x: f32, y: f32) -> Texel {
    // Across-grain coordinate, bent by slow warps and around knots.
    let mut yw = y
        + 0.03 * t.n2(x, y, 1.0, 2.0, sd(s, 1))
        + 0.003 * t.n2(x, y, 3.0, 8.0, sd(s, 2))
        + 0.0007 * t.n2(x, y, 10.0, 30.0, sd(s, 3));
    let mut knot = 0.0f32;
    let mut knot_e = 9.0f32;
    t.scatter(x, y, 1.6, sd(s, 4), |h, dx, dy| {
        if rnd(h, 1) < 0.35 {
            let kr = 0.011 + 0.02 * rnd(h, 2).powi(2);
            let e = (dx / (kr * 1.3)).hyp(dy / kr);
            // Rings flow smoothly around the knot.
            let side = dy / (dy * dy + kr * kr).sqrt();
            yw += side * kr * 1.8 * (-(e / 2.2).powi(2)).exp();
            let k = 1.0 - smooth(0.9, 1.05, e);
            if e < knot_e {
                knot = k;
                knot_e = e;
            }
        }
    });
    // The face cuts the log's rings at a slant: distance from the pith, with the pith
    // line `depth` behind the face and folded every half tile, gives nested cathedral
    // arches along the grain (folding keeps it periodic; rings run parallel at the folds).
    let fold = t.h * 0.5;
    let across = (yw.rem_euclid(fold) - 0.5 * fold).abs();
    // The pith line drifts slowly toward and away from the face along the board.
    let wave = std::f32::consts::TAU * (x / t.w + y / t.h);
    let depth = 0.035
        + 0.006 * (wave + 1.5 * t.n2(x, y, 0.6, 1.0, sd(s, 5))).sin()
        + 0.004 * t.n2(x, y, 1.5, 1.0, sd(s, 18));
    let phase = across.hyp(depth) / 0.0085 + 0.8 * t.n2(x, y, 0.8, 3.0, sd(s, 6));
    let f = phase.rem_euclid(1.0);
    let ring = hash2(phase.floor() as i32, 0, sd(s, 7));
    // Latewood: a darker band with a soft start and a sharp end; ring widths and darkness
    // vary year to year.
    let lw = 0.4 + 0.3 * rnd(ring, 1);
    let ring_dark = 0.55 + 0.6 * rnd(ring, 2);
    let late = smooth(lw, lw + 0.22, f) * (1.0 - smooth(0.93, 0.985, f)) * ring_dark;
    let late = lerp(0.3, late.min(1.0), t.keep(150.0));
    // Heartwood/sapwood bands and fiber streaks along the grain.
    let band = t.fbm2(x, yw, 0.8, 8.0, 4, 0.5, sd(s, 8));
    let streak = t.fbm2(x, yw, 1.2, 40.0, 4, 0.6, sd(s, 9));
    let fiber = t.fbm2(x, yw, 10.0, 300.0, 3, 0.6, sd(s, 10));
    // Checks: long thin drying cracks along the grain.
    let check_n = t.n2(x, yw, 1.5, 24.0, sd(s, 11));
    let check_mask = smooth(0.3, 0.7, t.n2(x, y, 1.0, 2.0, sd(s, 12)));
    let check = (1.0 - smooth(0.0, 0.03, check_n.abs())) * check_mask;
    // Saw marks across the board, faint.
    let saw_cycles = (t.w / 0.0065).round();
    let saw = (x * std::f32::consts::TAU * saw_cycles / t.w
        + 3.0 * t.n2(x, y, 3.0, 4.0, sd(s, 13)))
    .sin()
        * smooth(0.0, 0.6, t.n2(x, y, 2.0, 3.0, sd(s, 14)))
        * t.keep(160.0);
    // Weathering: silver-grey, streaky along the grain, raising the latewood.
    let weather = smooth(-0.1, 0.6, t.fbm2(x, y, 0.8, 16.0, 4, 0.5, sd(s, 15)));

    let mut h = (late - 0.5) * 0.0004 * (0.4 + weather)
        + fiber * 0.00008
        + streak * 0.00015
        + saw * 0.00006
        - check * 0.002
        + t.n2(x, y, 3.0, 2.0, sd(s, 16)) * 0.0004;
    let early_c = [0.33, 0.2, 0.105];
    let late_c = [0.13, 0.068, 0.032];
    let mut c = mul(
        mix(early_c, late_c, late),
        (1.0 + 0.35 * band) * (1.0 + 0.14 * streak + 0.07 * fiber),
    );
    let grey = mix([0.22, 0.2, 0.175], [0.1, 0.09, 0.075], late);
    c = mix(c, mul(grey, 1.0 + 0.1 * streak), weather * 0.5);
    let grime = smooth(0.3, 0.8, t.fbm2(x, y, 2.0, 12.0, 4, 0.5, sd(s, 17)));
    c = mul(c, 1.0 - 0.3 * grime);
    let mut rough = lerp(0.68, 0.58, late) + 0.2 * weather + 0.05 * grime;
    // Knots: dark, tight rings, a darker rim and a resin halo; slightly sunken center.
    if knot > 0.0 {
        let kf = (knot_e * 4.0).rem_euclid(1.0);
        let kc = mul([0.09, 0.045, 0.022], 0.8 + 0.4 * smooth(0.3, 0.9, kf));
        let rim = 1.0 - smooth(0.0, 0.12, (knot_e - 0.9).abs());
        c = mix(c, kc, knot * 0.9);
        c = mul(c, 1.0 - 0.3 * rim);
        h += knot * 0.0002 - knot * (1.0 - smooth(0.0, 0.5, knot_e)) * 0.0006;
        rough = lerp(rough, 0.5, knot);
    }
    let halo = (1.0 - smooth(1.0, 2.6, knot_e)) * (1.0 - knot);
    c = mix(c, mul3(c, [0.78, 0.6, 0.45]), halo * 0.5);
    c = mix(c, [0.04, 0.025, 0.015], check * 0.85);
    rough = lerp(rough, 0.95, check);
    Texel {
        h,
        c,
        r: rough,
        m: 0.0,
        ao: 1.0,
    }
}

/// Natural cliff rock: crags on a few bedding planes, flaking ledges, faults and open
/// cracks, mineral grains, stains, rain streaks and lichen.
fn rock(t: &Tile, s: u64, x: f32, y: f32) -> Texel {
    let wx = x + 0.2 * t.fbm(x, y, 0.5, 3, 0.5, sd(s, 1));
    let wy = y + 0.12 * t.fbm(x, y, 0.5, 3, 0.5, sd(s, 2));
    // Two bedding planes per meter, each bed set back or forward a little.
    let bed_t = (wy + 0.04 * t.n(x, y, 2.0, sd(s, 6))) * 2.0;
    let bed_f = bed_t - bed_t.floor();
    let bed_id = hash2((bed_t.floor() as i32).rem_euclid(4), 0, sd(s, 7));
    let d_bed = bed_f.min(1.0 - bed_f) * 0.5;
    let bed_groove = smooth(-0.2, 0.3, t.n(x, y, 1.3, sd(s, 28)));
    let bed_h =
        (rnd(bed_id, 1) - 0.5) * 0.012 - (1.0 - smooth(0.0, 0.03, d_bed)) * 0.01 * bed_groove;
    // Faults: meandering, mostly vertical lines where the face steps.
    // Distance to the fault from the field's value over its gradient.
    let fault_at = |x: f32, y: f32| {
        t.fbm2(x, y, 1.0, 0.35, 3, 0.5, sd(s, 4)) + 0.006 * t.n(x, y, 14.0, sd(s, 26))
    };
    let fault_n = fault_at(wx, wy);
    // The field changes by at most ~25 per meter, so past 0.25 the fault is > 1 cm away.
    let fault_d = if fault_n.abs() > 0.25 {
        fault_n.signum()
    } else {
        let e = 0.004;
        let g = ((fault_at(wx + e, wy) - fault_n) / e)
            .hyp((fault_at(wx, wy + e) - fault_n) / e)
            .max(0.05);
        fault_n / g
    };
    let fault_h =
        smooth(-0.003, 0.003, fault_d) * (0.008 + 0.014 * (t.n(x, y, 1.0, sd(s, 27)) * 0.5 + 0.5));
    // Flaking: thin layers spalled off in irregular ledges.
    let flake_n = t.fbm(wx, wy, 2.5, 4, 0.5, sd(s, 5)) * 4.0 + 0.3 * t.n(x, y, 12.0, sd(s, 25));
    let flake = (flake_n.floor() + smooth(0.55, 0.95, flake_n - flake_n.floor())) * 0.004;
    // Crags and broad forms over everything; fine grit on top.
    let forms = t.fbm(wx, wy, 0.8, 4, 0.5, sd(s, 8)) * 0.07;
    let crag = t.ridged(wx, wy, 1.6, 7, 0.5, sd(s, 9));
    let detail = t.fbm(x, y, 20.0, 5, 0.5, sd(s, 10)) * 0.0025;
    let mut h = bed_h + fault_h + flake + forms + crag * 0.08 + detail;
    // Cracks: parts of the faults and bed joints open up; hairlines follow noise zero
    // crossings.
    let cw = 0.002 + 0.005 * (t.n(x, y, 5.0, sd(s, 11)) * 0.5 + 0.5);
    let open_f = smooth(-0.1, 0.3, t.n(x, y, 1.2, sd(s, 12)));
    let open_b = smooth(0.25, 0.55, t.n(x, y, 2.5, sd(s, 13)));
    let crack = ((1.0 - smooth(cw * 0.3, cw, fault_d.abs())) * open_f)
        .max((1.0 - smooth(cw * 0.2, cw * 0.6, d_bed)) * open_b * bed_groove * 0.8);
    let hair_n = t.fbm(wx, wy, 4.0, 3, 0.5, sd(s, 14));
    let hair = (1.0 - smooth(0.0, 0.02, hair_n.abs()))
        * smooth(0.25, 0.6, t.n(x, y, 1.5, sd(s, 15)))
        * t.keep(250.0);
    h -= crack * 0.035 + hair * 0.004;

    // Color: cool grey to warm buff, varying by bed and region; mineral grains.
    let tone = t.fbm(x, y, 0.6, 4, 0.5, sd(s, 16));
    let mut c = mix(
        [0.15, 0.15, 0.145],
        [0.22, 0.195, 0.165],
        smooth(-0.35, 0.45, tone),
    );
    c = mul(c, 0.94 + 0.12 * rnd(bed_id, 2));
    c = mul(c, 1.0 + 0.16 * t.fbm(x, y, 5.0, 5, 0.55, sd(s, 17)));
    let gk = t.keep(220.0);
    let g = if gk > 0.0 {
        rnd(t.voronoi(x, y, 220.0, 1.0, sd(s, 18)).id, 1)
    } else {
        0.5
    };
    let mut rough = 0.84 + 0.06 * t.n(x, y, 30.0, sd(s, 19));
    if g < 0.14 {
        c = mul(c, 1.0 - 0.5 * gk);
    } else if g > 0.9 {
        c = mul(c, 1.0 + 0.35 * gk);
    } else if g > 0.84 {
        rough -= 0.2 * gk;
    }
    // Iron staining washed down the face in broad patches.
    let iron = smooth(-0.1, 0.6, t.fbm2(x, y, 0.9, 0.5, 4, 0.55, sd(s, 20)));
    c = mix(c, mul3(c, [1.25, 0.97, 0.72]), iron * 0.55);
    // Crests bleach; fresh flake scars are a little paler.
    let fresh = smooth(0.55, 0.95, flake_n - flake_n.floor());
    c = mul(
        c,
        (0.86 + 0.28 * smooth(0.3, 0.8, crag)) * (1.0 + 0.04 * fresh),
    );
    // Dark rain streaks run down from cracks and ledges.
    let streak = t.fbm2(x, y, 5.0, 0.45, 4, 0.5, sd(s, 21));
    let wash =
        smooth(0.0, 0.55, streak) * (0.5 + 0.5 * smooth(-0.2, 0.4, t.n(x, y, 0.7, sd(s, 22))));
    c = mul(c, 1.0 - 0.38 * wash);
    rough += 0.04 * wash;
    c = mix(c, [0.045, 0.04, 0.035], (crack * 0.9 + hair * 0.5).min(1.0));
    let (cover, lc, lh) = lichen(t, sd(s, 23), x, y, 4.0, 0.4);
    let cover = cover * smooth(-0.2, 0.3, t.n(x, y, 0.8, sd(s, 24))) * (1.0 - crack);
    c = mix(c, mul(lc, 0.85), cover * 0.6);
    rough = lerp(rough, 0.95, cover.max(crack));
    Texel {
        h: h + lh,
        c,
        r: rough,
        m: 0.0,
        ao: 1.0 - 0.4 * crack,
    }
}

/// Short grass seen from above: dense, long blades leaning with the wind and out of their
/// tufts, over soil and dead straw.
fn grass(t: &Tile, s: u64, x: f32, y: f32) -> Texel {
    // Soil and thatch underneath.
    let soil_n = t.fbm(x, y, 20.0, 4, 0.5, sd(s, 1));
    let ground = -0.016 + soil_n * 0.002 + t.fbm(x, y, 2.0, 4, 0.5, sd(s, 2)) * 0.006;
    let mut top = ground;
    let thatch = smooth(-0.3, 0.3, t.fbm(x, y, 4.0, 3, 0.5, sd(s, 8)));
    let mut c = mul(
        mix([0.06, 0.047, 0.032], [0.045, 0.05, 0.024], thatch),
        1.0 + 0.25 * soil_n,
    );
    let mut r = 0.95;
    t.scatter(x, y, 30.0, sd(s, 3), |hh, dx, dy| {
        if dx * dx + dy * dy > 0.02 * 0.02 || rnd(hh, 1) > 0.8 {
            return;
        }
        let (d, a) = segment(dx, dy, rnd(hh, 2) * 3.2, 0.006 + 0.012 * rnd(hh, 3));
        let w = 0.0006 + 0.0006 * rnd(hh, 4);
        if d < w {
            let hb = -0.015 + 0.002 * rnd(hh, 5) + (1.0 - (d / w).powi(2)).sqrt() * w * 0.4;
            if hb > top {
                top = hb;
                c = mul([0.22, 0.18, 0.1], 0.6 + 0.5 * rnd(hh, 6) - 0.2 * a.abs());
                r = 0.85;
            }
        }
    });
    // Tufts thin out a little between them; blades lean out of the tuft and with the wind.
    let tuft = t.voronoi(x, y, 6.0, 1.0, sd(s, 4));
    let patchy = smooth(-0.4, 0.4, t.fbm(x, y, 3.0, 3, 0.5, sd(s, 5)));
    let density = (1.0 - smooth(0.3 / 6.0, 0.75 / 6.0, tuft.f1) * 0.25) * (0.8 + 0.2 * patchy);
    let tl_len = tuft.f1.max(1e-6);
    let wind = 4.0 * t.n(x, y, 0.7, sd(s, 6));
    let (lx, ly) = (
        tuft.dx / tl_len * 0.8 + 0.7 * wind.cos(),
        tuft.dy / tl_len * 0.8 + 0.7 * wind.sin(),
    );
    let dryness = smooth(0.0, 0.6, t.fbm(x, y, 1.2, 4, 0.5, sd(s, 7)));
    // Each tuft has its own shade and a touch more or less yellow.
    let tuft_tint = 0.85 + 0.3 * rnd(tuft.id, 1);
    let tuft_yellow = 0.25 * rnd(tuft.id, 2);
    for layer in 0..8u64 {
        t.scatter(x, y, 30.0, sd(s, 10 + layer), |hh, dx, dy| {
            let r1 = rnd(hh, 1);
            // Blades reach at most one 33 mm scatter cell (length, curl and width included).
            let half = 0.012 + 0.016 * (r1 * 7.0).fract();
            let reach = half + 0.0045;
            if dx * dx + dy * dy > reach * reach || r1 > density {
                return;
            }
            // Blade direction: the local lean plus a random turn.
            let g = GRAD[(hh >> 12) as usize & 15];
            let (bx, by) = (lx + g[0], ly + g[1]);
            let bl = bx.hyp(by).max(1e-6);
            let (bx, by) = (bx / bl, by / bl);
            let along = dx * bx + dy * by;
            if along.abs() > half {
                return;
            }
            // Slight curl; taper from the root (tl = 0) to the tip (tl = 1).
            let r2 = rnd(hh, 2);
            let curl = (r2 - 0.5) * 20.0;
            let across = -dx * by + dy * bx + curl * along * along;
            let tl = (along / half + 1.0) * 0.5;
            let w = (0.0019 + 0.0017 * (r2 * 13.0).fract()) * (1.0 - 0.85 * tl * tl);
            if across.abs() >= w {
                return;
            }
            let r3 = rnd(hh, 3);
            let cross = (1.0 - (across / w).powi(2)).sqrt();
            let base_h = -0.014 + 0.014 * r3;
            let hb = base_h + 0.006 * (1.0 - 2.0 * (tl - 0.6) * (tl - 0.6)) + cross * w * 0.25;
            if hb <= top {
                return;
            }
            top = hb;
            let pick = (r3 * 11.0).fract();
            let straw = 0.03 + 0.15 * dryness;
            let mut bc = if pick < straw {
                [0.3, 0.24, 0.13]
            } else if pick < straw + 0.3 {
                [0.075, 0.15, 0.03]
            } else if pick < straw + 0.75 {
                [0.11, 0.2, 0.042]
            } else {
                [0.16, 0.23, 0.055]
            };
            if pick >= straw {
                bc = mix(bc, [0.2, 0.2, 0.07], dryness * 0.3 + tuft_yellow);
                bc = mul(bc, tuft_tint);
            }
            // Lower blades sit in shaded clutter; tips and midribs are lighter.
            let lift = smooth(-0.014, 0.006, hb);
            bc = mul(
                bc,
                (0.55 + 0.45 * lift) * (0.85 + 0.3 * (r2 * 29.0).fract()) * (1.0 + 0.15 * tl),
            );
            bc = mul(bc, 1.0 + 0.12 * (1.0 - smooth(0.0, 0.4, across.abs() / w)));
            c = bc;
            r = 0.78 + 0.1 * (r3 * 37.0).fract();
        });
    }
    Texel {
        h: top,
        c,
        r,
        m: 0.0,
        ao: 1.0,
    }
}

/// Bare soil: crumbly clods, pebbles, twigs, damp patches and drying cracks.
fn dirt(t: &Tile, s: u64, x: f32, y: f32) -> Texel {
    let wx = x + 0.02 * t.n(x, y, 8.0, sd(s, 1));
    let wy = y + 0.02 * t.n(x, y, 8.0, sd(s, 2));
    // Damp patches: darker, smoother, flatter (kept subtle so the tile does not show).
    let damp = smooth(-0.1, 0.45, t.fbm(x, y, 2.5, 4, 0.5, sd(s, 3))) * 0.6;
    // Clods a few centimeters across on top of long undulations, crumbs on the clods.
    let lumps = t.voronoi(wx, wy, 18.0, 1.0, sd(s, 4));
    let lump_r = (0.5 + 0.45 * rnd(lumps.id, 2)) / 18.0;
    let lump = (1.0 - (lumps.f1 / lump_r).powi(2)).max(0.0).powf(0.6)
        * smooth(0.25, 0.4, rnd(lumps.id, 3));
    let small = t.voronoi(wx, wy, 45.0, 1.0, sd(s, 16));
    let small_r = (0.4 + 0.5 * rnd(small.id, 2)) / 45.0;
    let pebble_clod = (1.0 - (small.f1 / small_r).powi(2)).max(0.0).powf(0.4);
    let lump_h = (lump * (0.003 + 0.006 * rnd(lumps.id, 1)))
        .max(pebble_clod * (0.001 + 0.003 * rnd(small.id, 1)));
    let crumbs = t.voronoi(x, y, 90.0, 1.0, sd(s, 5));
    let crumb = (1.0 - (crumbs.f1 * 90.0 / 0.75).powi(2)).max(0.0).sqrt() * t.keep(90.0);
    let undulate = t.fbm(x, y, 1.5, 5, 0.5, sd(s, 6)) * 0.02;
    let rough_h = t.fbm(x, y, 30.0, 5, 0.5, sd(s, 7)) * 0.002;
    let mut h = undulate + (lump_h + crumb * 0.0012) * (1.0 - 0.6 * damp) + rough_h;
    // Drying cracks on the dry ground.
    let cr = t.voronoi(x, y, 9.0, 0.9, sd(s, 8));
    let crack_w = 0.0015 + 0.0015 * (t.n(x, y, 20.0, sd(s, 9)) * 0.5 + 0.5);
    let crack = (1.0 - smooth(crack_w * 0.3, crack_w, cr.edge))
        * (1.0 - damp)
        * smooth(0.15, 0.5, t.n(x, y, 1.5, sd(s, 10)));
    h -= crack * 0.004;
    // Pebbles and grit, half buried.
    let ground = h;
    let mut stone = 0.0f32;
    let mut stone_c = [0.0; 3];
    let mut stone_top = f32::MIN;
    for (freq, prob, rmin, rmax, salt) in [
        (22.0, 0.25, 0.004, 0.011, 11),
        (70.0, 0.15, 0.0015, 0.0035, 12),
    ] {
        t.scatter(x, y, freq, sd(s, salt), |hh, dx, dy| {
            if rnd(hh, 1) > prob {
                return;
            }
            let rad = rmin + (rmax - rmin) * rnd(hh, 2);
            let e = (dx / rad).hyp(dy / (rad * (0.6 + 0.4 * rnd(hh, 3))));
            if e < 1.0 {
                let top = (1.0 - e * e).sqrt() * rad * 0.6 - rad * 0.2;
                if top > stone_top {
                    stone_top = top;
                    stone = 1.0 - smooth(0.85, 1.0, e);
                    let pick = rnd(hh, 4);
                    stone_c = if pick < 0.55 {
                        [0.12, 0.105, 0.09]
                    } else if pick < 0.9 {
                        [0.15, 0.12, 0.09]
                    } else {
                        [0.2, 0.19, 0.175]
                    };
                    stone_c = mul(stone_c, 0.7 + 0.5 * rnd(hh, 5));
                }
            }
        });
    }
    let is_stone = if stone_top > f32::MIN && ground + stone_top > h {
        h = ground + stone_top;
        stone
    } else {
        0.0
    };
    // Twigs and rootlets.
    let mut twig = 0.0f32;
    t.scatter(x, y, 5.0, sd(s, 13), |hh, dx, dy| {
        if rnd(hh, 1) < 0.5 {
            let (d, a) = segment(dx, dy, rnd(hh, 2) * 3.2, 0.02 + 0.06 * rnd(hh, 3));
            let w = 0.0008 + 0.0012 * rnd(hh, 4);
            let bend = 1.0 - a.abs().powi(3);
            twig = twig.max((1.0 - smooth(w * 0.5, w, d)) * bend);
        }
    });
    h += twig * 0.001;

    let organic = t.fbm(x, y, 3.0, 5, 0.5, sd(s, 14));
    let fine = t.n(x, y, 500.0, sd(s, 15)) * t.keep(500.0);
    let dry = [0.13, 0.1, 0.07];
    let wet = [0.055, 0.042, 0.03];
    let mut c = mul(
        mix(dry, wet, damp * 0.85),
        1.0 + 0.2 * organic + 0.08 * fine,
    );
    // Clod tops dry out lighter; the gaps between them stay dark.
    c = mul(c, 0.8 + 0.3 * lump * (1.0 - damp));
    c = mix(c, [0.045, 0.035, 0.025], crack * 0.7);
    c = mix(c, stone_c, is_stone);
    c = mix(c, [0.16, 0.11, 0.07], twig * 0.85);
    let rough = lerp(0.97, 0.78, damp) - 0.12 * is_stone;
    Texel {
        h,
        c,
        r: rough,
        m: 0.0,
        ao: 1.0,
    }
}

/// Painted steel plate with worn, chipped paint, primer, scratches and rust.
fn metal(t: &Tile, s: u64, x: f32, y: f32) -> Texel {
    // Wear zones (where things rub and knock) hold clusters of small chips.
    let wear_at = |x: f32, y: f32, octaves: u32| {
        t.fbm(x, y, 22.0, octaves, 0.5, sd(s, 2)) * 0.5
            + 0.5
            + 0.3 * t.fbm(x, y, 1.3, 3, 0.5, sd(s, 1))
    };
    let wear = wear_at(x, y, 4);
    let bare = smooth(0.74, 0.745, wear);
    let primer = smooth(0.705, 0.71, wear) * (1.0 - bare);
    // Scratches through to bare steel.
    let mut scratch = 0.0f32;
    t.scatter(x, y, 9.0, sd(s, 3), |h, dx, dy| {
        if rnd(h, 1) < 0.7 {
            let ang = (rnd(h, 2) - 0.5) * 0.7 + if rnd(h, 3) < 0.3 { 1.2 } else { 0.0 };
            let (d, a) = segment(dx, dy, ang, 0.015 + 0.07 * rnd(h, 4));
            let w = 0.00015 + 0.0004 * rnd(h, 5);
            let fade = (1.0 - a.abs().powi(2)).max(0.0);
            scratch = scratch.max((1.0 - smooth(w * 0.5, w, d)) * fade);
        }
    });
    let steel = bare.max(scratch);
    // Rust grows in the bare spots and bleeds down in thin runs.
    let rust_n = t.fbm(x, y, 25.0, 4, 0.5, sd(s, 4)) * 0.5 + 0.5;
    let rust = bare * smooth(0.3, 0.6, rust_n);
    let mut bleed = 0.0f32;
    for (k, up) in [0.015f32, 0.045, 0.09].into_iter().enumerate() {
        let wob = 0.004 * t.n2(x, y, 25.0, 3.0, sd(s, 5 + k as u64));
        let above = smooth(0.74, 0.76, wear_at(x + wob, y + up, 3));
        bleed = bleed.max(above * (1.0 - up / 0.12));
    }
    let runs = smooth(-0.1, 0.5, t.n2(x, y, 90.0, 3.0, sd(s, 9)));
    let bleed = bleed * runs * (1.0 - bare);

    // Height: topcoat and primer (~0.1 mm each) over the plate, orange peel, shallow dents.
    let peel = t.fbm(x, y, 120.0, 3, 0.5, sd(s, 10)) * t.keep(120.0);
    let dent = t.fbm(x, y, 4.0, 3, 0.5, sd(s, 11));
    let paint_h = (1.0 - bare) * 0.0001 + (1.0 - bare - primer).max(0.0) * 0.0001;
    let h = paint_h + peel * 0.00003 * (1.0 - bare) + dent * 0.0012 - scratch * 0.00006
        + rust * (rust_n - 0.4) * 0.0004;

    // Paint: neutral dark grey so vertex colors can tint it; grime streaks.
    let grime = t.fbm2(x, y, 6.0, 1.5, 4, 0.5, sd(s, 12));
    let paint = mul(
        [0.15, 0.16, 0.16],
        1.0 + 0.08 * t.fbm(x, y, 8.0, 3, 0.5, sd(s, 13)),
    );
    let mut c = mul(paint, 1.0 - 0.2 * smooth(0.0, 0.6, grime));
    c = mix(c, [0.2, 0.06, 0.035], primer);
    c = mix(c, [0.2, 0.07, 0.025], bleed * 0.6);
    let brushed = t.n2(x, y, 12.0, 900.0, sd(s, 14)) * t.keep(900.0);
    let steel_c = mul([0.56, 0.57, 0.58], 0.9 + 0.08 * brushed);
    c = mix(c, steel_c, steel);
    let rust_c = mix([0.25, 0.09, 0.03], [0.12, 0.045, 0.02], rust_n);
    c = mix(c, rust_c, rust);
    let mut r = 0.55 + 0.08 * peel + 0.15 * smooth(0.0, 0.6, grime);
    r = lerp(r, 0.62, primer);
    r = lerp(r, 0.3 + 0.06 * brushed, steel);
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
            let ang = rnd(hh, 3) * std::f32::consts::TAU;
            let (sn, cs) = ang.sin_cos();
            let u = dx * cs + dy * sn;
            let v = -dx * sn + dy * cs;
            let e2 = (u / a).powi(2) + (v / b).powi(2);
            if e2 >= 1.0 {
                return;
            }
            let lumpy = 1.0 + 0.12 * t.n(x, y, freq * 3.0, sd(hh, 5));
            let height = b * (0.55 + 0.3 * rnd(hh, 4)) * (1.0 - e2).sqrt() * lumpy - b * 0.25
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
            let e = (dx / 0.007).hyp(dy / 0.0095);
            prints = prints.max(1.0 - smooth(0.6, 1.0, e));
        }
    });
    let mut scratch = 0.0f32;
    t.scatter(x, y, 7.0, sd(s, 4), |h, dx, dy| {
        if rnd(h, 1) < 0.3 {
            let (d, a) = segment(
                dx,
                dy,
                rnd(h, 2) * std::f32::consts::PI,
                0.01 + 0.05 * rnd(h, 3),
            );
            let fade = (1.0 - a.abs().powi(2)).max(0.0);
            scratch = scratch.max((1.0 - smooth(0.0001, 0.0003, d)) * fade);
        }
    });
    let dust = smooth(0.55, 0.8, t.n(x, y, 300.0, sd(s, 5)) * 0.5 + 0.5) * t.keep(300.0);
    let c = mix(
        [0.8, 0.86, 0.84],
        [0.6, 0.58, 0.54],
        dust * 0.4 + smudge * 0.06,
    );
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
    let fracture =
        (1.0 - smooth(0.0, 0.012, inner.edge)) * smooth(-0.2, 0.3, t.n(x, y, 2.0, sd(s, 8)));
    let mut bubbles = 0.0f32;
    t.scatter(x, y, 70.0, sd(s, 9), |h, dx, dy| {
        if rnd(h, 1) < 0.25 {
            let rad = 0.0005 + 0.0015 * rnd(h, 2);
            bubbles = bubbles.max(1.0 - smooth(rad * 0.6, rad, dx.hyp(dy)));
        }
    });
    let h = melt * 0.004 + grain * 0.0003 * frost - crack * 0.0006;
    let clear = [0.32, 0.53, 0.68];
    let mut c = mix(clear, [0.78, 0.86, 0.9], frost * 0.85);
    c = mul(c, 1.0 + 0.06 * melt);
    c = mix(
        c,
        [0.75, 0.84, 0.9],
        (fracture * 0.6 + bubbles * 0.5) * (1.0 - frost * 0.5),
    );
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
            .as_chunks::<4>()
            .0
            .iter()
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
            for (name, img) in [
                ("albedo", &set.albedo),
                ("normal", &set.normal),
                ("orm", &set.orm),
            ] {
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
                assert!(
                    seam_c <= worst_cols + 2.0,
                    "{kind:?} {name} u seam {seam_c} vs {worst_cols}"
                );
                assert!(
                    seam_r <= worst_rows + 2.0,
                    "{kind:?} {name} v seam {seam_r} vs {worst_rows}"
                );
            }
            assert_periodic(&set, 100);
        }
    }

    /// Rendering from half a tile in must give the same image rolled by half (the surface is
    /// periodic and every filter wraps): at most one texel in `per` may differ by more than
    /// 3 levels (float rounding moves a few).
    fn assert_periodic(set: &TextureSet, per: u32) {
        let (w, h) = (set.albedo.width, set.albedo.height);
        let shifted = render(set.kind, w, h, 3, [w / 2, h / 2]);
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
            assert!(
                bad * per <= w * h,
                "{:?}: {bad} of {} texels differ",
                set.kind,
                w * h
            );
        }
    }

    /// Full-detail periodicity, where the finest features exist (slow in debug builds:
    /// `cargo test --release -p engine_destruction --lib textures -- --ignored`).
    #[test]
    #[ignore]
    fn every_kind_is_periodic_at_full_detail() {
        for kind in SurfaceKind::ALL {
            assert_periodic(&generate(kind, 512, 3), 1000);
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
            assert_eq!(
                metal > 0.5,
                kind == SurfaceKind::Metal,
                "{kind:?} metalness"
            );
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
                    assert!(
                        mortar < -0.004,
                        "course {course} brick {k}: joint h {mortar}"
                    );
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
            assert!(
                grey(set.albedo.pixel(col_at(joint_x), row)),
                "course {course} joint"
            );
            assert!(
                !grey(set.albedo.pixel(col_at(brick_x), row)),
                "course {course} brick"
            );
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
