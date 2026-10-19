//! Procedural PBR materials for the test area: tileable albedo, normal and
//! occlusion/roughness/metal textures generated at startup, so props and ground look like
//! real wood, rock, sand and paint up close without shipping image files.
//!
//! Every [`Surface`] gets three seamless textures built from a height field (millimeters) and
//! per-texel albedo, roughness and metalness:
//!
//! - **albedo**: sRGB RGBA8 (alpha only matters for [`Surface::Glass`]);
//! - **normal**: linear, tangent space, OpenGL convention (+Y = green = toward decreasing v),
//!   derived from the height field;
//! - **ORM**: R = ambient occlusion, G = perceptual roughness, B = metalness (glTF). The same
//!   image feeds `metallic_roughness_texture` and `occlusion_texture`.
//!
//! Mip chains are built here (in linear light for albedo); the normal map's mips are
//! renormalized and the lost normal variance is moved into the roughness mips (Toksvig /
//! vMF), so glossy surfaces don't sparkle or alias in the distance.
//!
//! ## UV conventions for meshes
//!
//! - UVs are in **meters**; each material's `uv_scale` is `1 / tile_size(surface)`.
//! - Wood grain, rope, bark plates and concrete drip streaks run along **v**.
//! - [`Surface::Planks`]: virtual boards are one third of the tile (0.1667 m) wide, centered
//!   on u = 0, 0.1667, 0.3333 m; nail rows cross every board at v = 0, 0.667, 1.333 m. Put
//!   u = 0 on a board's center line and v = 0 over a joist so nails line up, and offset each
//!   board by a whole number of board widths in u (or by a random v) so neighbours differ.
//! - [`Surface::EndGrain`]: the growth rings are centered on uv (0, 0): use the cut face's
//!   local x/y in meters from the log's axis (radius up to ~0.2 m; radial checks start there).
//! - Round things ([`Surface::Post`], [`Surface::Bark`], [`Surface::Rope`]): make u wrap the
//!   circumference in a whole number of tiles to avoid a seam. For [`Surface::Rope`] u must go
//!   once around per tile (`tile_size(Rope)[0]`, three strands) whatever the diameter; v is
//!   meters along the rope (lay length = one tile).
//! - [`Surface::Concrete`] (board-formed): the form boards run along u, drips run along v.
//! - Albedos are neutral for surfaces the props tint with vertex color (PaintedWood, Gelcoat,
//!   Plastic, PaintedMetal, Fabric, Rope, Ground); the vertex color multiplies the texture.

use std::cell::RefCell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_render::material::srgb_hex_to_linear;
use engine_render::{AlphaMode, Image, StandardMaterial};

/// What a surface is made of. Each has its own texture set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Surface {
    /// Weathered jetty planks: grain along the texture's v axis.
    Planks,
    /// Jetty posts: tarred, cracked round timber.
    Post,
    /// Boat planking painted over wood, chipped and worn along the edges. White paint, so a
    /// vertex color tints it.
    PaintedWood,
    /// Varnished hardwood (thwarts, oars, gunwales).
    Varnish,
    /// Glossy fiberglass gelcoat (motorboat hull and deck).
    Gelcoat,
    /// Non-slip deck: fine molded diamond pattern.
    NonSlip,
    /// Tree bark (logs).
    Bark,
    /// Sawn end grain with growth rings (log ends), rings centered on uv (0, 0).
    EndGrain,
    /// Granite boulders with lichen.
    Rock,
    /// Cast concrete (culvert headwall).
    Concrete,
    /// Corrugated galvanized steel (culvert pipe).
    Galvanized,
    /// Molded plastic (buoys).
    Plastic,
    /// Painted metal (outboard motor cowling, cleats).
    PaintedMetal,
    /// Rubber (fenders, seals).
    Rubber,
    /// Rope (tethers, mooring lines).
    Rope,
    /// Glass (windshield).
    Glass,
    /// Density test block: agglomerated cork.
    Cork,
    /// Density test block: clean planed pine.
    Pine,
    /// Density test block: lake ice.
    Ice,
    /// Smooth concrete test block.
    BlockConcrete,
    /// Skin for the swimmer and wader.
    Skin,
    /// Swimwear and clothing: neutral knit, tinted by vertex color.
    Fabric,
    /// Terrain detail: a near-neutral multiplier (clumps, pebbles, grass blades) for a mesh
    /// whose vertex color carries the macro color. UVs = world x/z in meters.
    Ground,
    /// [`Surface::PaintedWood`] with dark green paint (shares its normal and ORM maps).
    PaintedWoodGreen,
    /// [`Surface::PaintedWood`] with red paint (shares its normal and ORM maps).
    PaintedWoodRed,
    /// [`Surface::PaintedWood`] with blue paint (shares its normal and ORM maps).
    PaintedWoodBlue,
}

impl Surface {
    /// Every surface, in declaration order.
    pub const ALL: [Surface; 26] = [
        Surface::Planks,
        Surface::Post,
        Surface::PaintedWood,
        Surface::Varnish,
        Surface::Gelcoat,
        Surface::NonSlip,
        Surface::Bark,
        Surface::EndGrain,
        Surface::Rock,
        Surface::Concrete,
        Surface::Galvanized,
        Surface::Plastic,
        Surface::PaintedMetal,
        Surface::Rubber,
        Surface::Rope,
        Surface::Glass,
        Surface::Cork,
        Surface::Pine,
        Surface::Ice,
        Surface::BlockConcrete,
        Surface::Skin,
        Surface::Fabric,
        Surface::Ground,
        Surface::PaintedWoodGreen,
        Surface::PaintedWoodRed,
        Surface::PaintedWoodBlue,
    ];

    /// The surface whose textures this one is generated with (itself, except paint colors).
    fn source(self) -> Surface {
        match self {
            Surface::PaintedWoodGreen | Surface::PaintedWoodRed | Surface::PaintedWoodBlue => {
                Surface::PaintedWood
            }
            s => s,
        }
    }
}

/// One texture set per [`Surface`], built on first use.
#[derive(Resource, Default)]
pub struct DemoMaterials {
    sets: Vec<(Surface, StandardMaterial)>,
}

impl DemoMaterials {
    /// Generate every surface's textures into `images` at full quality (512² to 1024²
    /// texels per tile; about a second on four cores in a release build).
    pub fn build(images: &mut Assets<Image>) -> Self {
        Self::build_with_size(images, 512)
    }

    /// Like [`DemoMaterials::build`], with `size` texels for a standard tile instead of 512
    /// (the large surfaces get proportionally more). Small sizes (64) are for tests.
    pub fn build_with_size(images: &mut Assets<Image>, size: u32) -> Self {
        let size = size.clamp(8, 4096);
        let mut order: Vec<Surface> = Surface::ALL
            .iter()
            .copied()
            .filter(|s| s.source() == *s)
            .collect();
        // Longest jobs first so the threads finish together.
        order.sort_by_key(|s| std::cmp::Reverse(cost(*s)));
        let next = AtomicUsize::new(0);
        let done = Mutex::new(Vec::new());
        let threads = cores().min(order.len());
        // Cores left over when there are fewer surfaces left than workers: the remaining
        // surfaces split their texel loops across them.
        let idle = Arc::new(AtomicUsize::new(cores() - threads));
        std::thread::scope(|scope| {
            for _ in 0..threads {
                let idle = Arc::clone(&idle);
                let (next, done, order) = (&next, &done, &order);
                scope.spawn(move || {
                    SPARE_CORES.set(Some(Arc::clone(&idle)));
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&surface) = order.get(i) else { break };
                        let built = synthesize(surface, size);
                        done.lock().unwrap_or_else(|e| e.into_inner()).push(built);
                    }
                    idle.fetch_add(1, Ordering::Relaxed);
                    SPARE_CORES.set(None);
                });
            }
        });
        let mut done = done.into_inner().unwrap_or_else(|e| e.into_inner());
        done.sort_by_key(|b| b.surface as usize);
        let mut sets = Vec::new();
        for b in done {
            let albedo = images.add(b.albedo);
            let normal = images.add(b.normal);
            let orm = images.add(b.orm);
            sets.push((b.surface, textured(b.surface, albedo, normal, orm)));
            for (variant, image) in b.variants {
                let albedo = images.add(image);
                sets.push((variant, textured(variant, albedo, normal, orm)));
            }
        }
        sets.sort_by_key(|(s, _)| *s as usize);
        Self { sets }
    }

    /// The textured material for `surface` (or a plain one when it wasn't built).
    pub fn get(&self, surface: Surface) -> StandardMaterial {
        self.sets
            .iter()
            .find(|(s, _)| *s == surface)
            .map(|(_, m)| *m)
            .unwrap_or_else(|| plain(surface))
    }
}

/// Size in meters (u, v) of one texture tile of `surface`. Textured materials use
/// `uv_scale = 1 / tile_size`, so mesh UVs are plain meters.
pub fn tile_size(surface: Surface) -> [f32; 2] {
    spec(surface).tile
}

/// An untextured stand-in with the surface's color and roughness (tests, no GPU).
pub fn plain(surface: Surface) -> StandardMaterial {
    use Surface::*;
    let (rgb, rough, metal) = match surface {
        Planks => ([0.22, 0.20, 0.17], 0.85, 0.0),
        Post => ([0.07, 0.062, 0.055], 0.72, 0.0),
        PaintedWood => ([0.70, 0.68, 0.62], 0.45, 0.0),
        Varnish => ([0.27, 0.085, 0.022], 0.3, 0.0),
        Gelcoat => ([0.79, 0.78, 0.75], 0.15, 0.0),
        NonSlip => ([0.50, 0.50, 0.48], 0.7, 0.0),
        Bark => ([0.15, 0.13, 0.11], 0.88, 0.0),
        EndGrain => ([0.38, 0.25, 0.14], 0.85, 0.0),
        Rock => ([0.27, 0.26, 0.245], 0.8, 0.0),
        Concrete => ([0.33, 0.315, 0.29], 0.9, 0.0),
        Galvanized => ([0.58, 0.60, 0.61], 0.5, 0.8),
        Plastic => ([0.66, 0.67, 0.69], 0.42, 0.0),
        PaintedMetal => ([0.56, 0.565, 0.58], 0.34, 0.15),
        Rubber => ([0.035, 0.035, 0.035], 0.85, 0.0),
        Rope => ([0.58, 0.53, 0.43], 0.9, 0.0),
        Glass => ([0.07, 0.085, 0.085], 0.05, 0.0),
        Cork => ([0.30, 0.18, 0.09], 0.85, 0.0),
        Pine => ([0.62, 0.40, 0.16], 0.6, 0.0),
        Ice => ([0.45, 0.57, 0.65], 0.1, 0.0),
        BlockConcrete => ([0.40, 0.40, 0.39], 0.85, 0.0),
        Skin => ([0.55, 0.36, 0.27], 0.5, 0.0),
        Fabric => ([0.60, 0.60, 0.60], 0.7, 0.0),
        Ground => ([0.86, 0.86, 0.86], 0.92, 0.0),
        PaintedWoodGreen => ([0.03, 0.10, 0.05], 0.45, 0.0),
        PaintedWoodRed => ([0.30, 0.025, 0.018], 0.45, 0.0),
        PaintedWoodBlue => ([0.025, 0.065, 0.19], 0.45, 0.0),
    };
    let sp = spec(surface);
    let mut m = StandardMaterial::color(rgb[0], rgb[1], rgb[2])
        .with_roughness(rough)
        .with_metallic(metal)
        .with_subsurface(sp.subsurface);
    m.reflectance = sp.reflectance;
    m.double_sided = sp.double_sided;
    m.receives_wetness = sp.wet;
    if sp.blend {
        m.base_color[3] = 0.3;
        m.alpha_mode = AlphaMode::Blend;
    }
    m
}

/// The textured material: every factor is 1 and the textures carry the values.
fn textured(
    surface: Surface,
    albedo: Handle<Image>,
    normal: Handle<Image>,
    orm: Handle<Image>,
) -> StandardMaterial {
    let sp = spec(surface);
    let mut m = StandardMaterial {
        base_color: [1.0; 4],
        metallic: 1.0,
        perceptual_roughness: 1.0,
        reflectance: sp.reflectance,
        subsurface: sp.subsurface,
        double_sided: sp.double_sided,
        receives_wetness: sp.wet,
        ..Default::default()
    }
    .with_base_color_texture(albedo)
    .with_normal_texture(normal)
    .with_metallic_roughness_texture(orm)
    .with_occlusion_texture(orm)
    .with_uv_scale(1.0 / sp.tile[0], 1.0 / sp.tile[1]);
    if sp.blend {
        m.alpha_mode = AlphaMode::Blend;
    }
    // The ground runs up cliffs (the crag): projected along all three axes, it never
    // stretches down a face. (Its meshes share one transform, so it stays seamless.)
    if surface == Surface::Ground {
        m = m.with_triplanar();
    }
    m
}

// ---------------------------------------------------------------------------------------------
// Per-surface settings
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Spec {
    /// One tile in meters (u, v).
    tile: [f32; 2],
    /// Texels (u, v) at the standard size of 512.
    res: [u32; 2],
    /// Cavity occlusion from the height field: (blur radius in meters, strength).
    cavity: [(f32, f32); 2],
    /// Normal map strength (scales the height field's slopes).
    bump: f32,
    reflectance: f32,
    subsurface: f32,
    blend: bool,
    double_sided: bool,
    wet: bool,
}

fn spec(surface: Surface) -> Spec {
    use Surface::*;
    let base = Spec {
        tile: [0.5, 0.5],
        res: [512, 512],
        cavity: [(0.003, 0.5), (0.015, 0.4)],
        bump: 1.0,
        reflectance: 0.5,
        subsurface: 0.0,
        blend: false,
        double_sided: false,
        wet: true,
    };
    match surface {
        Planks => Spec {
            tile: [0.5, 2.0],
            res: [512, 2048],
            cavity: [(0.002, 0.6), (0.012, 0.5)],
            ..base
        },
        Post => Spec {
            tile: [0.5, 1.0],
            res: [512, 1024],
            cavity: [(0.003, 0.6), (0.015, 0.5)],
            ..base
        },
        PaintedWood | PaintedWoodGreen | PaintedWoodRed | PaintedWoodBlue => Spec {
            tile: [1.0, 1.0],
            res: [1024, 1024],
            cavity: [(0.002, 0.5), (0.01, 0.4)],
            ..base
        },
        Varnish => Spec {
            tile: [0.5, 1.0],
            res: [512, 1024],
            cavity: [(0.002, 0.2), (0.01, 0.1)],
            ..base
        },
        Gelcoat => Spec {
            tile: [1.0, 1.0],
            res: [1024, 1024],
            cavity: [(0.002, 0.1), (0.01, 0.0)],
            ..base
        },
        NonSlip => Spec {
            tile: [0.2, 0.2],
            cavity: [(0.0015, 0.6), (0.006, 0.3)],
            ..base
        },
        Bark => Spec {
            cavity: [(0.004, 0.25), (0.02, 0.2)],
            ..base
        },
        EndGrain => Spec {
            cavity: [(0.002, 0.5), (0.01, 0.4)],
            ..base
        },
        Rock => Spec {
            tile: [1.5, 1.5],
            res: [1024, 1024],
            cavity: [(0.006, 0.6), (0.04, 0.5)],
            ..base
        },
        Concrete => Spec {
            tile: [1.0, 1.0],
            res: [512, 512],
            cavity: [(0.003, 0.6), (0.015, 0.4)],
            ..base
        },
        Galvanized => Spec {
            cavity: [(0.002, 0.3), (0.01, 0.2)],
            ..base
        },
        Plastic | PaintedMetal => Spec {
            cavity: [(0.002, 0.2), (0.01, 0.1)],
            ..base
        },
        Rubber => Spec {
            cavity: [(0.002, 0.3), (0.01, 0.2)],
            ..base
        },
        Rope => Spec {
            tile: [0.06, 0.072],
            res: [256, 256],
            cavity: [(0.0015, 0.6), (0.004, 0.5)],
            subsurface: 0.1,
            ..base
        },
        Glass => Spec {
            tile: [1.0, 1.0],
            cavity: [(0.002, 0.0), (0.01, 0.0)],
            blend: true,
            double_sided: true,
            wet: false,
            ..base
        },
        Cork => Spec {
            tile: [0.25, 0.25],
            cavity: [(0.0015, 0.6), (0.006, 0.4)],
            ..base
        },
        Pine => Spec {
            cavity: [(0.002, 0.3), (0.01, 0.2)],
            ..base
        },
        Ice => Spec {
            cavity: [(0.002, 0.2), (0.01, 0.1)],
            reflectance: 0.35,
            subsurface: 0.3,
            wet: false,
            ..base
        },
        BlockConcrete => Spec {
            cavity: [(0.002, 0.5), (0.01, 0.3)],
            ..base
        },
        Skin => Spec {
            tile: [0.2, 0.2],
            cavity: [(0.001, 0.3), (0.005, 0.2)],
            reflectance: 0.42,
            subsurface: 0.45,
            ..base
        },
        Fabric => Spec {
            tile: [0.05, 0.05],
            cavity: [(0.0003, 0.5), (0.001, 0.3)],
            subsurface: 0.15,
            ..base
        },
        Ground => Spec {
            tile: [2.5, 2.5],
            res: [1024, 1024],
            cavity: [(0.008, 0.5), (0.04, 0.4)],
            ..base
        },
    }
}

/// Relative build cost, for scheduling the biggest jobs first.
fn cost(surface: Surface) -> u32 {
    use Surface::*;
    let sp = spec(surface);
    // Measured nanoseconds per texel (single core, generate + encode).
    let per_texel = match surface {
        Planks | Rock => 105,
        PaintedWood | Concrete => 90,
        Ground | Ice | Galvanized => 75,
        Post | Varnish | EndGrain | Bark => 68,
        Cork => 60,
        Gelcoat => 48,
        _ => 45,
    };
    sp.res[0] * sp.res[1] / 1024 * per_texel
}

// ---------------------------------------------------------------------------------------------
// Synthesis pipeline
// ---------------------------------------------------------------------------------------------

/// One texel of a surface before encoding.
#[derive(Clone, Copy)]
struct Px {
    /// Height in millimeters.
    h: f32,
    /// Linear albedo.
    c: Vec3,
    /// Opacity.
    a: f32,
    /// Perceptual roughness.
    r: f32,
    /// Metalness.
    m: f32,
    /// Baked occlusion (holes, cracks), multiplied with the cavity term from the heights.
    ao: f32,
    /// Extra slope (dh/du, dh/dv, dimensionless) on top of the height field: facets, flakes.
    tilt: Vec2,
    /// Top-coat paint coverage (painted wood color variants).
    paint: f32,
}

impl Default for Px {
    fn default() -> Self {
        Self {
            h: 0.0,
            c: Vec3::splat(0.5),
            a: 1.0,
            r: 0.5,
            m: 0.0,
            ao: 1.0,
            tilt: Vec2::ZERO,
            paint: 0.0,
        }
    }
}

/// The generated images of one surface (plus paint color variants sharing its maps).
struct Built {
    surface: Surface,
    albedo: Image,
    normal: Image,
    orm: Image,
    variants: Vec<(Surface, Image)>,
}

/// Texture-space helpers: pixel grid, tile size in meters, periodic noise sized in meters.
struct Canvas {
    w: usize,
    h: usize,
    tile: Vec2,
}

impl Canvas {
    /// Meters per texel (u, v).
    fn texel(&self) -> Vec2 {
        self.tile / Vec2::new(self.w as f32, self.h as f32)
    }

    /// Cells per tile for features about `size` meters (a whole number, so it tiles).
    fn cells(&self, size: Vec2) -> IVec2 {
        IVec2::new(
            ((self.tile.x / size.x).fround() as i32).max(1),
            ((self.tile.y / size.y).fround() as i32).max(1),
        )
    }

    /// Every texel's [`Px`] from `f(x, y, uv)` (texel centers; rows in parallel).
    fn fill(&self, f: impl Fn(usize, usize, Vec2) -> Px + Sync) -> Buf<Px> {
        let (w, h) = (self.w, self.h);
        let mut out = Buf::new(w * h);
        par_rows_in(&mut out, w, |y, row| {
            let v = (y as f32 + 0.5) / h as f32;
            for (x, out) in row.iter_mut().enumerate() {
                *out = f(x, y, Vec2::new((x as f32 + 0.5) / w as f32, v));
            }
        });
        out
    }

    /// A full-resolution scalar field.
    fn field(&self, f: impl Fn(Vec2) -> f32 + Sync) -> Buf<f32> {
        let (w, h) = (self.w, self.h);
        let mut out = Buf::new(w * h);
        par_rows_in(&mut out, w, |y, row| {
            let v = (y as f32 + 0.5) / h as f32;
            for (x, out) in row.iter_mut().enumerate() {
                *out = f(Vec2::new((x as f32 + 0.5) / w as f32, v));
            }
        });
        out
    }

    /// Seamless gradient noise (std ≈ 0.3) with features about `size` meters.
    fn noise(&self, uv: Vec2, size: Vec2, seed: u32) -> f32 {
        let n = self.cells(size);
        gnoise(uv * n.as_vec2(), n, seed)
    }

    /// Seamless fractal noise (std ≈ 0.3), `octaves` halving in size and amplitude.
    fn fbm(&self, uv: Vec2, size: Vec2, seed: u32, octaves: u32) -> f32 {
        let n = self.cells(size);
        fbm(uv * n.as_vec2(), n, seed, octaves, 0.5)
    }
}

fn synthesize(surface: Surface, size: u32) -> Built {
    let sp = spec(surface);
    let scale = size as f32 / 512.0;
    let w = ((sp.res[0] as f32 * scale).fround() as usize).max(4);
    let h = ((sp.res[1] as f32 * scale).fround() as usize).max(4);
    let c = Canvas {
        w,
        h,
        tile: Vec2::from(sp.tile),
    };
    // Small seeds, so `seed + k` never overflows.
    let seed = (surface as u32 + 1) * 7919;
    // Diagnostics: DEMO_MATERIALS_TIMING=1 prints each surface's start and durations.
    static EPOCH: OnceLock<std::time::Instant> = OnceLock::new();
    let timing = std::env::var_os("DEMO_MATERIALS_TIMING").is_some();
    let started = std::time::Instant::now();
    let epoch = *EPOCH.get_or_init(|| started);
    let px = match surface {
        Surface::Planks => planks(&c, seed),
        Surface::Post => post(&c, seed),
        Surface::PaintedWood
        | Surface::PaintedWoodGreen
        | Surface::PaintedWoodRed
        | Surface::PaintedWoodBlue => painted_wood(&c, seed),
        Surface::Varnish => varnish(&c, seed),
        Surface::Gelcoat => gelcoat(&c, seed),
        Surface::NonSlip => non_slip(&c, seed),
        Surface::Bark => bark(&c, seed),
        Surface::EndGrain => end_grain(&c, seed),
        Surface::Rock => rock(&c, seed),
        Surface::Concrete => concrete(&c, seed),
        Surface::Galvanized => galvanized(&c, seed),
        Surface::Plastic => plastic(&c, seed),
        Surface::PaintedMetal => painted_metal(&c, seed),
        Surface::Rubber => rubber(&c, seed),
        Surface::Rope => rope(&c, seed),
        Surface::Glass => glass(&c, seed),
        Surface::Cork => cork(&c, seed),
        Surface::Pine => pine(&c, seed),
        Surface::Ice => ice(&c, seed),
        Surface::BlockConcrete => block_concrete(&c, seed),
        Surface::Skin => skin(&c, seed),
        Surface::Fabric => fabric(&c, seed),
        Surface::Ground => ground(&c, seed),
    };
    let generated = started.elapsed().as_secs_f32();
    let mut built = finish(&c, &sp, surface, &px);
    if timing {
        eprintln!(
            "{surface:?} {w}x{h}: generate {:.0} ms, encode {:.0} ms (from {:.0} ms)",
            generated * 1000.0,
            (started.elapsed().as_secs_f32() - generated) * 1000.0,
            (started - epoch).as_secs_f32() * 1000.0,
        );
    }
    if surface == Surface::PaintedWood {
        let white = PAINT_WHITE;
        for (variant, color) in [
            (Surface::PaintedWoodGreen, hex(0x24563a)),
            (Surface::PaintedWoodRed, hex(0x93241c)),
            (Surface::PaintedWoodBlue, hex(0x23466f)),
        ] {
            let ratio = color / white;
            let recolored = |i: usize| (px[i].c * Vec3::ONE.lerp(ratio, px[i].paint), px[i].a);
            built
                .variants
                .push((variant, encode_albedo(w, h, recolored, seed ^ 0x77)));
        }
    }
    built
}

/// `h` rows of `w` values, `row(y, values)` filling row `y` (see [`par_rows_in`]).
fn par_rows<T: Clone + Default + Send>(
    w: usize,
    h: usize,
    row: impl Fn(usize, &mut [T]) + Sync,
) -> Vec<T> {
    let mut out = vec![T::default(); w * h];
    par_rows_in(&mut out, w, row);
    out
}

/// Logical cores available to this process.
fn cores() -> usize {
    static CORES: OnceLock<usize> = OnceLock::new();
    *CORES.get_or_init(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .clamp(1, 16)
    })
}

thread_local! {
    /// On a [`DemoMaterials::build`] worker: how many cores no worker is using.
    static SPARE_CORES: std::cell::Cell<Option<Arc<AtomicUsize>>> =
        const { std::cell::Cell::new(None) };
}

/// Fill `out` (rows of `w` values) by `row(y, values)`. Large images are cut into bands
/// computed on scoped threads when there are idle cores; every value depends only on its
/// row, so the result is the same however the work is split.
fn par_rows_in<T: Send>(out: &mut [T], w: usize, row: impl Fn(usize, &mut [T]) + Sync) {
    let w = w.max(1);
    let h = out.len() / w;
    let threads = if out.len() < 1 << 15 {
        1
    } else {
        let spare = SPARE_CORES.with(|c| {
            let a = c.take();
            let n = a.as_ref().map(|a| a.load(Ordering::Relaxed));
            c.set(a);
            n
        });
        // Outside a build every core is free.
        spare.map_or(cores(), |n| n + 1)
    };
    let bands = if threads <= 1 {
        1
    } else {
        (threads * 2).min(h)
    };
    if bands <= 1 {
        for (y, r) in out.chunks_mut(w).enumerate() {
            row(y, r);
        }
        return;
    }
    let per = h.div_ceil(bands);
    let row = &row;
    std::thread::scope(|scope| {
        for (b, band) in out.chunks_mut(w * per).enumerate() {
            scope.spawn(move || {
                for (k, r) in band.chunks_mut(w).enumerate() {
                    row(b * per + k, r);
                }
            });
        }
    });
}

/// Element types of [`Buf`], each with a per-thread list of spare buffers.
trait Pooled: Clone + Default + Send + 'static {
    fn spares() -> &'static std::thread::LocalKey<RefCell<Vec<Vec<Self>>>>;
}

macro_rules! pooled {
    ($($t:ty => $name:ident),* $(,)?) => {$(
        thread_local! {
            static $name: RefCell<Vec<Vec<$t>>> = const { RefCell::new(Vec::new()) };
        }
        impl Pooled for $t {
            fn spares() -> &'static std::thread::LocalKey<RefCell<Vec<Vec<Self>>>> {
                &$name
            }
        }
    )*};
}

pooled!(Px => SPARE_PX, f32 => SPARE_F32, Vec3 => SPARE_VEC3, Vec4 => SPARE_VEC4);

/// A scratch buffer that goes back to its thread's spares when dropped. The surfaces need
/// over a gigabyte of intermediate buffers in all; reusing them avoids a page fault per
/// 4 KiB, which costs more than the work done on the page on a virtual machine.
struct Buf<T: Pooled>(Vec<T>);

impl<T: Pooled> Buf<T> {
    /// `n` default values.
    fn new(n: usize) -> Self {
        let mut v = T::spares().with_borrow_mut(|spares| {
            // The smallest spare that fits, else the largest (it grows once).
            let fit = (0..spares.len())
                .filter(|&i| spares[i].capacity() >= n)
                .min_by_key(|&i| spares[i].capacity())
                .or_else(|| (0..spares.len()).max_by_key(|&i| spares[i].capacity()));
            fit.map(|i| spares.swap_remove(i)).unwrap_or_default()
        });
        v.clear();
        v.resize(n, T::default());
        Self(v)
    }

    fn collect(n: usize, values: impl Iterator<Item = T>) -> Self {
        let mut b = Self::new(0);
        b.0.reserve(n);
        b.0.extend(values);
        b
    }
}

impl<T: Pooled> Drop for Buf<T> {
    fn drop(&mut self) {
        let v = std::mem::take(&mut self.0);
        if v.capacity() > 0 {
            T::spares().with_borrow_mut(|spares| {
                spares.push(v);
                if spares.len() > 6 {
                    let smallest = (0..spares.len())
                        .min_by_key(|&i| spares[i].capacity())
                        .unwrap_or(0);
                    spares.swap_remove(smallest);
                }
            });
        }
    }
}

impl<T: Pooled> std::ops::Deref for Buf<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<T: Pooled> std::ops::DerefMut for Buf<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.0
    }
}

/// Heights → normals and cavity occlusion; encode the three images with their mip chains.
fn finish(c: &Canvas, sp: &Spec, surface: Surface, px: &[Px]) -> Built {
    let (w, h) = (c.w, c.h);
    let heights = Buf::collect(w * h, px.iter().map(|p| p.h));
    let texel_mm = c.texel() * 1000.0;
    let mut ao = Buf::collect(w * h, px.iter().map(|p| p.ao));
    for &(radius, strength) in &sp.cavity {
        if strength <= 0.0 {
            continue;
        }
        let radius_mm = radius * 1000.0;
        let rx = ((radius_mm / texel_mm.x).fround() as usize).max(1);
        let ry = ((radius_mm / texel_mm.y).fround() as usize).max(1);
        let blurred = blur(&heights, w, h, rx, ry);
        for ((a, b), hh) in ao.iter_mut().zip(blurred.iter()).zip(heights.iter()) {
            let depth = (b - hh) / radius_mm;
            *a *= 1.0 - strength * (depth * 1.5).clamp(0.0, 1.0);
        }
    }
    let mut normals = Buf::<Vec3>::new(w * h);
    par_rows_in(&mut normals, w, |y, row| {
        let up = &heights[prev_idx(y, h) * w..][..w];
        let mid = &heights[y * w..][..w];
        let down = &heights[next_idx(y, h) * w..][..w];
        let tilts = &px[y * w..][..w];
        for (x, (n, p)) in row.iter_mut().zip(tilts).enumerate() {
            let sx = (mid[next_idx(x, w)] - mid[prev_idx(x, w)]) / (2.0 * texel_mm.x) + p.tilt.x;
            let sy = (down[x] - up[x]) / (2.0 * texel_mm.y) + p.tilt.y;
            // Rows run toward +v; +Y of an OpenGL normal map points toward -v.
            *n = Vec3::new(-sx * sp.bump, sy * sp.bump, 1.0).normalize();
        }
    });
    drop(heights);
    let orm = Buf::collect(
        w * h,
        px.iter()
            .zip(ao.iter())
            .map(|(p, a)| Vec3::new(a.clamp(0.0, 1.0), p.r.clamp(0.02, 1.0), p.m.clamp(0.0, 1.0))),
    );
    drop(ao);
    let seed = surface as u32 * 31 + 7;
    let albedo = encode_albedo(w, h, |i| (px[i].c, px[i].a), seed);
    let (normal, orm) = encode_normal_orm(w, h, normals, orm, seed);
    Built {
        surface,
        albedo,
        normal,
        orm,
        variants: Vec::new(),
    }
}

/// Two passes of a wrapping box blur with radii (`rx`, `ry`) texels: close to a Gaussian.
fn blur(src: &[f32], w: usize, h: usize, rx: usize, ry: usize) -> Buf<f32> {
    let mut a = Buf::collect(src.len(), src.iter().copied());
    let mut tmp = Buf::new(w * h);
    for _ in 0..2 {
        box_rows(&a, &mut tmp, w, r_limit(rx, w));
        box_cols(&tmp, &mut a, w, h, r_limit(ry, h));
    }
    a
}

/// A box radius that fits in `n` texels.
fn r_limit(r: usize, n: usize) -> usize {
    r.min(n.saturating_sub(1) / 2)
}

fn box_rows(src: &[f32], dst: &mut [f32], w: usize, r: usize) {
    let inv = 1.0 / (2 * r + 1) as f32;
    for (row, out) in src.chunks_exact(w).zip(dst.chunks_exact_mut(w)) {
        let mut sum: f32 = (0..=2 * r).map(|k| row[(k + w - r) % w]).sum();
        let mut add = r + 1;
        let mut sub = w - r;
        for o in out.iter_mut() {
            *o = sum * inv;
            add = if add >= w { add - w } else { add };
            sub = if sub >= w { sub - w } else { sub };
            sum += row[add] - row[sub];
            add += 1;
            sub += 1;
        }
    }
}

fn box_cols(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize) {
    let inv = 1.0 / (2 * r + 1) as f32;
    let mut sums = Buf::collect(
        w,
        (0..w).map(|x| {
            (0..=2 * r)
                .map(|k| src[((k + h - r) % h) * w + x])
                .sum::<f32>()
        }),
    );
    for (y, out) in dst.chunks_exact_mut(w).enumerate() {
        let add = &src[((y + r + 1) % h) * w..][..w];
        let sub = &src[((y + h - r) % h) * w..][..w];
        for (((o, s), a), b) in out.iter_mut().zip(sums.iter_mut()).zip(add).zip(sub) {
            *o = *s * inv;
            *s += a - b;
        }
    }
}

/// Linear → sRGB (0..255) at 4096 steps, for [`srgb255`].
fn srgb_lut() -> &'static [f32; 4097] {
    static LUT: OnceLock<Box<[f32; 4097]>> = OnceLock::new();
    LUT.get_or_init(|| {
        Box::new(std::array::from_fn(|i| {
            let l = i as f32 / 4096.0;
            let s = if l <= 0.003_130_8 {
                l * 12.92
            } else {
                1.055 * l.powf(1.0 / 2.4) - 0.055
            };
            s * 255.0
        }))
    })
}

/// Linear `v` → sRGB 0..255 through `lut` ([`srgb_lut`]).
#[inline]
fn srgb255(lut: &[f32; 4097], v: f32) -> f32 {
    let t = v.clamp(0.0, 1.0) * 4096.0;
    let i = (t as u32).min(4095) as usize;
    let f = t - i as f32;
    lut[i] + (lut[i + 1] - lut[i]) * f
}

/// Quantizes to 8 bits with a deterministic ±0.5 LSB dither (no banding in smooth
/// gradients): a tiled white-noise table, offset per channel and seed.
struct Dither {
    noise: &'static [[f32; 64]; 64],
    offsets: [(usize, usize); 6],
}

impl Dither {
    fn new(seed: u32) -> Self {
        static NOISE: OnceLock<[[f32; 64]; 64]> = OnceLock::new();
        let noise = NOISE.get_or_init(|| {
            std::array::from_fn(|y| {
                std::array::from_fn(|x| unit(hash(x as i32, y as i32, 0x5EED)) - 0.5)
            })
        });
        let offsets = std::array::from_fn(|ch| {
            let k = hash(ch as i32, 0, seed);
            ((k & 63) as usize, ((k >> 6) & 63) as usize)
        });
        Self { noise, offsets }
    }

    /// Channel `ch`'s noise for texel row `y`, and the column offset into it.
    #[inline]
    fn row(&self, ch: usize, y: usize) -> (&[f32; 64], usize) {
        let (ox, oy) = self.offsets[ch];
        (&self.noise[(y + oy) & 63], ox)
    }
}

/// RGBA8 rows of `w`×`h` texels from `get(index)` (channels in 0..255): RGB dithered with
/// channels `first..first + 3` of `dither`, alpha truncated.
fn quantize_rows(
    w: usize,
    h: usize,
    dither: &Dither,
    first: usize,
    get: impl Fn(usize) -> [f32; 4] + Sync,
) -> Vec<u8> {
    par_rows(w * 4, h, |y, row| {
        let (n0, o0) = dither.row(first, y);
        let (n1, o1) = dither.row(first + 1, y);
        let (n2, o2) = dither.row(first + 2, y);
        let base = y * w;
        for (x, out) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let v = get(base + x);
            // `as u8` saturates: below 0 → 0, above 255 → 255.
            *out = [
                (v[0] + n0[(x + o0) & 63] + 0.5) as u8,
                (v[1] + n1[(x + o1) & 63] + 0.5) as u8,
                (v[2] + n2[(x + o2) & 63] + 0.5) as u8,
                v[3] as u8,
            ];
        }
    })
}

/// The next mip level of a `w`×`h` image whose texel `i` is `get(i)`: 2×2 averages (1×2 or
/// 2×1 once a side is down to one texel).
fn downsample<T>(w: usize, h: usize, get: impl Fn(usize) -> T + Sync) -> (Buf<T>, usize, usize)
where
    T: Pooled + std::ops::Add<Output = T> + std::ops::Mul<f32, Output = T>,
{
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = Buf::new(nw * nh);
    par_rows_in(&mut out, nw, |y, row| {
        let r0 = (y * 2).min(h - 1) * w;
        let r1 = (y * 2 + 1).min(h - 1) * w;
        for (x, o) in row.iter_mut().enumerate() {
            let (x0, x1) = ((x * 2).min(w - 1), (x * 2 + 1).min(w - 1));
            *o = (get(r0 + x0) + get(r0 + x1) + get(r1 + x0) + get(r1 + x1)) * 0.25;
        }
    });
    (out, nw, nh)
}

/// The sRGB albedo image of linear (color, alpha) `texel(index)`, with mips averaged in
/// linear light from the unquantized colors.
fn encode_albedo(
    w: usize,
    h: usize,
    texel: impl Fn(usize) -> (Vec3, f32) + Sync,
    seed: u32,
) -> Image {
    let lut = srgb_lut();
    let rgba = |c: Vec4| {
        [
            srgb255(lut, c.x),
            srgb255(lut, c.y),
            srgb255(lut, c.z),
            c.w.clamp(0.0, 1.0) * 255.0 + 0.5,
        ]
    };
    let dither = |level: u32| Dither::new(seed.wrapping_add(level * 977));
    let top = |i: usize| {
        let (c, a) = texel(i);
        c.extend(a)
    };
    let data = quantize_rows(w, h, &dither(0), 0, |i| rgba(top(i)));
    let mut image = Image::new(w as u32, h as u32, data, true);
    let mut cur: Option<Buf<Vec4>> = None;
    let (mut cw, mut ch) = (w, h);
    for level in 1..image.full_mip_count() {
        let (next, nw, nh) = match &cur {
            None => downsample(cw, ch, top),
            Some(prev) => downsample(cw, ch, |i| prev[i]),
        };
        let data = quantize_rows(nw, nh, &dither(level), 0, |i| rgba(next[i]));
        image.mips.push(data);
        cur = Some(next);
        (cw, ch) = (nw, nh);
    }
    image
}

/// One level of the normal and ORM images: normals `n` (renormalized) and ORM `orm(index)`.
fn quantize_normal_orm(
    w: usize,
    h: usize,
    n: &[Vec3],
    orm: impl Fn(usize) -> Vec3 + Sync,
    dither: &Dither,
) -> (Vec<u8>, Vec<u8>) {
    let nd = quantize_rows(w, h, dither, 0, |i| {
        let u = n[i].normalize_or(Vec3::Z) * 127.5 + 127.5;
        [u.x, u.y, u.z, 255.0]
    });
    let od = quantize_rows(w, h, dither, 3, |i| {
        let v = orm(i) * 255.0;
        [v.x, v.y, v.z, 255.0]
    });
    (nd, od)
}

/// Encode the normal and ORM images with matching mip chains: mip normals are renormalized
/// averages and the variance they lose is added to the roughness (vMF / Toksvig).
fn encode_normal_orm(
    w: usize,
    h: usize,
    normals: Buf<Vec3>,
    mut orm: Buf<Vec3>,
    seed: u32,
) -> (Image, Image) {
    let dither = |level: u32| Dither::new(seed.wrapping_add(level * 977));
    let (nd, od) = quantize_normal_orm(w, h, &normals, |i| orm[i], &dither(0));
    let mut nimg = Image::new(w as u32, h as u32, nd, false);
    let mut oimg = Image::new(w as u32, h as u32, od, false);
    // Averages carried down the chain: unnormalized normal, AO, GGX alpha², metal.
    for o in orm.iter_mut() {
        o.y = o.y.powi(4);
    }
    let (mut n_cur, mut o_cur) = (normals, orm);
    let (mut cw, mut ch) = (w, h);
    for level in 1..nimg.full_mip_count() {
        let (n_next, nw, nh) = downsample(cw, ch, |i| n_cur[i]);
        let (o_next, _, _) = downsample(cw, ch, |i| o_cur[i]);
        // Roughness from the averaged alpha² plus the normal spread (vMF lobe of the mean
        // normal's length r: kappa = (3r - r³) / (1 - r²), alpha² += 2 / kappa).
        let level_orm = |i: usize| {
            let o = o_next[i];
            let r = n_next[i].length().min(0.9999);
            let kappa = (3.0 * r - r * r * r) / (1.0 - r * r);
            let a2 = (o.y + 2.0 / kappa.max(1e-3)).min(1.0);
            Vec3::new(o.x, a2.sqrt().sqrt(), o.z)
        };
        let (nd, od) = quantize_normal_orm(nw, nh, &n_next, level_orm, &dither(level));
        nimg.mips.push(nd);
        oimg.mips.push(od);
        n_cur = n_next;
        o_cur = o_next;
        (cw, ch) = (nw, nh);
    }
    (nimg, oimg)
}

// ---------------------------------------------------------------------------------------------
// Seamless noise
// ---------------------------------------------------------------------------------------------

const GRADS: [[f32; 2]; 16] = [
    [0.980_785, 0.195_09],
    [0.831_47, 0.555_57],
    [0.555_57, 0.831_47],
    [0.195_09, 0.980_785],
    [-0.195_09, 0.980_785],
    [-0.555_57, 0.831_47],
    [-0.831_47, 0.555_57],
    [-0.980_785, 0.195_09],
    [-0.980_785, -0.195_09],
    [-0.831_47, -0.555_57],
    [-0.555_57, -0.831_47],
    [-0.195_09, -0.980_785],
    [0.195_09, -0.980_785],
    [0.555_57, -0.831_47],
    [0.831_47, -0.555_57],
    [0.980_785, -0.195_09],
];

#[inline]
fn hash(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = seed.wrapping_mul(0x9E37_79B9)
        ^ (x as u32).wrapping_mul(0x85EB_CA6B)
        ^ (y as u32).wrapping_mul(0xC2B2_AE35).rotate_left(15);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^= h >> 16;
    h
}

/// Another independent hash derived from `h`.
#[inline]
fn rehash(h: u32, k: u32) -> u32 {
    hash(h as i32, k as i32, 0x68E3_1DA4)
}

/// Hash → [0, 1).
#[inline]
fn unit(h: u32) -> f32 {
    (h >> 8) as f32 * (1.0 / 16_777_216.0)
}

#[inline]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Gradient noise with period `per` (in lattice cells) on each axis; std ≈ 0.3, |n| < 1.
#[inline]
fn gnoise(p: Vec2, per: IVec2, seed: u32) -> f32 {
    let fx = p.x.ffloor();
    let fy = p.y.ffloor();
    let tx = p.x - fx;
    let ty = p.y - fy;
    let x0 = wrap_idx(fx as i32, per.x);
    let y0 = wrap_idx(fy as i32, per.y);
    let x1 = if x0 + 1 >= per.x { 0 } else { x0 + 1 };
    let y1 = if y0 + 1 >= per.y { 0 } else { y0 + 1 };
    let g = |x: i32, y: i32, dx: f32, dy: f32| {
        let g = GRADS[(hash(x, y, seed) & 15) as usize];
        g[0] * dx + g[1] * dy
    };
    let a = g(x0, y0, tx, ty);
    let b = g(x1, y0, tx - 1.0, ty);
    let c = g(x0, y1, tx, ty - 1.0);
    let d = g(x1, y1, tx - 1.0, ty - 1.0);
    let u = fade(tx);
    let v = fade(ty);
    let ab = a + (b - a) * u;
    let cd = c + (d - c) * u;
    (ab + (cd - ab) * v) * 1.4
}

/// Fractal sum of [`gnoise`], normalized to the std of one octave.
fn fbm(p: Vec2, per: IVec2, seed: u32, octaves: u32, gain: f32) -> f32 {
    let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
    let mut q = p;
    let mut pp = per;
    for i in 0..octaves {
        sum += amp * gnoise(q, pp, seed.wrapping_add(i * 0x9E37));
        norm += amp * amp;
        amp *= gain;
        q *= 2.0;
        pp *= 2;
    }
    sum / f32::sqrt(norm)
}

/// `floor`, `round` and `fract` without a libm call (baseline x86-64 has no SSE4.1 rounding
/// instructions, and these run several times per texel); exact for |x| < 2³¹, which covers
/// every lattice, texel and color value here.
trait FastRound {
    fn ffloor(self) -> Self;
    fn fround(self) -> Self;
    fn ffract(self) -> Self;
}

impl FastRound for f32 {
    #[inline]
    fn ffloor(self) -> f32 {
        let t = self as i32 as f32;
        if t > self { t - 1.0 } else { t }
    }

    #[inline]
    fn fround(self) -> f32 {
        (self + 0.5).ffloor()
    }

    #[inline]
    fn ffract(self) -> f32 {
        self - self.ffloor()
    }
}

impl FastRound for Vec2 {
    #[inline]
    fn ffloor(self) -> Vec2 {
        Vec2::new(self.x.ffloor(), self.y.ffloor())
    }

    #[inline]
    fn fround(self) -> Vec2 {
        Vec2::new(self.x.fround(), self.y.fround())
    }

    #[inline]
    fn ffract(self) -> Vec2 {
        self - self.ffloor()
    }
}

/// Smoothstep that also works with `e0 > e1` (falling edge).
#[inline]
fn ss(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Wrap a uv difference into [-0.5, 0.5).
#[inline]
fn wrap(d: Vec2) -> Vec2 {
    d - d.fround()
}

#[inline]
fn wrap1(d: f32) -> f32 {
    d - d.fround()
}

/// Wrap any lattice index into `0..n` (cheap when it is already inside).
#[inline]
fn wrap_idx(i: i32, n: i32) -> i32 {
    if (i as u32) < n as u32 {
        i
    } else {
        i.rem_euclid(n)
    }
}

/// The next index after `i` in `0..n`, wrapping.
#[inline]
fn next_idx(i: usize, n: usize) -> usize {
    if i + 1 >= n { i + 1 - n } else { i + 1 }
}

/// The index before `i` in `0..n`, wrapping.
#[inline]
fn prev_idx(i: usize, n: usize) -> usize {
    if i == 0 { n - 1 } else { i - 1 }
}

/// Wrap a lattice index that is at most one period outside `0..n`.
#[inline]
fn wrap_i(i: i32, n: i32) -> i32 {
    if i < 0 {
        i + n
    } else if i >= n {
        i - n
    } else {
        i
    }
}

/// sRGB hex color → linear (through a table: cheap enough for per-texel use).
#[inline]
fn hex(rgb: u32) -> Vec3 {
    static LUT: OnceLock<[f32; 256]> = OnceLock::new();
    let lut = LUT.get_or_init(|| std::array::from_fn(|i| srgb_hex_to_linear(i as u32)[2]));
    Vec3::new(
        lut[(rgb >> 16) as usize & 0xff],
        lut[(rgb >> 8) as usize & 0xff],
        lut[rgb as usize & 0xff],
    )
}

/// Inverse of the standard normal CDF (Tukey lambda approximation, ~1%), through a table.
fn probit(p: f32) -> f32 {
    static LUT: OnceLock<Vec<f32>> = OnceLock::new();
    let lut = LUT.get_or_init(|| {
        (0..=1024)
            .map(|i| {
                let p = (i as f32 / 1024.0).clamp(1e-4, 1.0 - 1e-4);
                4.91 * (p.powf(0.14) - (1.0 - p).powf(0.14))
            })
            .collect()
    });
    let t = p.clamp(0.0, 1.0) * 1024.0;
    let i = (t as usize).min(1023);
    mix(lut[i], lut[i + 1], t - i as f32)
}

impl Canvas {
    /// Like [`Canvas::fbm`] with a chosen gain (0.6+ gives ragged detail).
    fn fbm_g(&self, uv: Vec2, size: Vec2, seed: u32, octaves: u32, gain: f32) -> f32 {
        let n = self.cells(size);
        fbm(uv * n.as_vec2(), n, seed, octaves, gain)
    }

    /// The noise behind a [`Mask`]: domain-warped ragged fractal noise (std ≈ 0.3).
    fn grunge(&self, uv: Vec2, size: Vec2, seed: u32) -> f32 {
        let warp = Vec2::new(
            self.fbm(uv, size, seed + 101, 3),
            self.fbm(uv, size, seed + 102, 3),
        ) * (size / self.tile)
            * 1.3;
        self.fbm_g(uv + warp, size, seed, 5, 0.6)
    }

    /// Ragged patch mask (stains, lichen, oxidation) with features about `size` meters,
    /// covering about `cover` of the area with edges `soft` wide (noise units).
    fn mask(&self, div: usize, size: Vec2, seed: u32, cover: f32, soft: f32) -> Mask {
        let t = 0.3 * probit(1.0 - cover);
        Mask {
            noise: self.grid(div, |uv| self.grunge(uv, size, seed)),
            lo: t - soft,
            hi: t + soft,
        }
    }

    /// A slow field evaluated every `div` texels.
    fn grid(&self, div: usize, f: impl Fn(Vec2) -> f32 + Sync) -> Grid {
        Grid::new(self, div, f)
    }

    /// Jittered Voronoi sites about `size` meters apart.
    fn sites(&self, size: Vec2, seed: u32, jitter: f32) -> Sites {
        Sites::new(self.cells(size), seed, jitter)
    }
}

// ---------------------------------------------------------------------------------------------
// Fast lookups: coarse fields and precomputed lattices
// ---------------------------------------------------------------------------------------------

/// A smooth field sampled on a coarse periodic grid and read back by interpolation.
struct Grid {
    w: usize,
    h: usize,
    /// Row stride of `data`: the grid with one wrapped column/row before it and two after,
    /// so lookups never wrap.
    pw: usize,
    data: Buf<f32>,
}

impl Grid {
    fn new(c: &Canvas, div: usize, f: impl Fn(Vec2) -> f32 + Sync) -> Self {
        let w = (c.w / div).max(1);
        let h = (c.h / div).max(1);
        let mut core = Buf::new(w * h);
        par_rows_in(&mut core, w, |y, row| {
            let v = (y as f32 + 0.5) / h as f32;
            for (x, out) in row.iter_mut().enumerate() {
                *out = f(Vec2::new((x as f32 + 0.5) / w as f32, v));
            }
        });
        let (pw, ph) = (w + 3, h + 3);
        let mut data = Buf::new(pw * ph);
        for (py, row) in data.chunks_exact_mut(pw).enumerate() {
            let src = &core[((py + h - 1) % h) * w..][..w];
            for (px, out) in row.iter_mut().enumerate() {
                *out = src[(px + w - 1) % w];
            }
        }
        Self { w, h, pw, data }
    }

    /// Padded index of the cell at or before `uv` (texel centers), and the fractions.
    #[inline]
    fn base(&self, uv: Vec2) -> (usize, f32, f32) {
        let gx = uv.x * self.w as f32 - 0.5;
        let gy = uv.y * self.h as f32 - 0.5;
        let fx = gx.ffloor();
        let fy = gy.ffloor();
        let x = wrap_idx(fx as i32, self.w as i32) as usize;
        let y = wrap_idx(fy as i32, self.h as i32) as usize;
        ((y + 1) * self.pw + x + 1, gx - fx, gy - fy)
    }

    /// Bilinear (colors and masks).
    #[inline]
    fn at(&self, uv: Vec2) -> f32 {
        let (i, fx, fy) = self.base(uv);
        let r0: [f32; 2] = self.data[i..i + 2].try_into().unwrap_or_default();
        let r1: [f32; 2] = self.data[i + self.pw..i + self.pw + 2]
            .try_into()
            .unwrap_or_default();
        mix(mix(r0[0], r0[1], fx), mix(r1[0], r1[1], fx), fy)
    }

    /// Catmull-Rom (heights and ring positions: continuous slopes).
    fn smooth(&self, uv: Vec2) -> f32 {
        let (i, fx, fy) = self.base(uv);
        let weights = |t: f32| {
            let t2 = t * t;
            let t3 = t2 * t;
            [
                -0.5 * t3 + t2 - 0.5 * t,
                1.5 * t3 - 2.5 * t2 + 1.0,
                -1.5 * t3 + 2.0 * t2 + 0.5 * t,
                0.5 * t3 - 0.5 * t2,
            ]
        };
        let wx = weights(fx);
        let wy = weights(fy);
        let start = i - self.pw - 1;
        let mut sum = 0.0;
        for (k, wyk) in wy.into_iter().enumerate() {
            let o = start + k * self.pw;
            let r: [f32; 4] = self.data[o..o + 4].try_into().unwrap_or_default();
            sum += wyk * (wx[0] * r[0] + wx[1] * r[1] + wx[2] * r[2] + wx[3] * r[3]);
        }
        sum
    }
}

/// A ragged mask: its noise on a coarse grid, thresholded at each lookup so the edges stay
/// sharp however coarse the grid.
struct Mask {
    noise: Grid,
    lo: f32,
    hi: f32,
}

impl Mask {
    #[inline]
    fn at(&self, uv: Vec2) -> f32 {
        ss(self.lo, self.hi, self.noise.at(uv))
    }
}

/// The nearest Voronoi site to a point.
#[derive(Clone, Copy)]
struct Cell {
    /// Hash of the site's cell (stable per cell, use for per-cell randomness).
    id: u32,
    /// Distance to the site (cell units).
    f1: f32,
    /// Distance to the nearest cell border (cell units; only when asked for).
    edge: f32,
    /// Point minus site (cell units).
    rel: Vec2,
}

/// Jittered sites, one per cell of a periodic lattice, for Voronoi patterns.
struct Sites {
    n: IVec2,
    pts: Vec<(Vec2, u32)>,
}

impl Sites {
    fn new(n: IVec2, seed: u32, jitter: f32) -> Self {
        let mut pts = Vec::with_capacity((n.x * n.y) as usize);
        for y in 0..n.y {
            for x in 0..n.x {
                let h = hash(x, y, seed);
                let o = Vec2::new(
                    0.5 + jitter * (unit(h) - 0.5),
                    0.5 + jitter * (unit(rehash(h, 1)) - 0.5),
                );
                pts.push((o, h));
            }
        }
        Self { n, pts }
    }

    /// The site nearest to `uv`, optionally offset by `warp` lattice cells.
    fn at(&self, uv: Vec2, warp: Vec2, edge: bool) -> Cell {
        self.nearest(uv * self.n.as_vec2() + warp, edge)
    }

    /// The site nearest to `p` (lattice units, wraps).
    fn nearest(&self, p: Vec2, edge: bool) -> Cell {
        let b = p.ffloor();
        let f = p - b;
        let (nx, ny) = (self.n.x, self.n.y);
        let bx = wrap_idx(b.x as i32, nx);
        let by = wrap_idx(b.y as i32, ny);
        let cols = [wrap_i(bx - 1, nx), bx, wrap_i(bx + 1, nx)];
        let mut pts = [Vec2::ZERO; 9];
        let mut best = 0;
        let mut best_id = 0;
        let mut d1 = f32::MAX;
        for (j, dy) in [-1.0f32, 0.0, 1.0].into_iter().enumerate() {
            let row = &self.pts[(wrap_i(by + j as i32 - 1, ny) * nx) as usize..][..nx as usize];
            for (i, dx) in [-1.0f32, 0.0, 1.0].into_iter().enumerate() {
                let (o, id) = row[cols[i] as usize];
                let d = Vec2::new(dx, dy) + o - f;
                let k = j * 3 + i;
                pts[k] = d;
                let dd = d.length_squared();
                if dd < d1 {
                    d1 = dd;
                    best = k;
                    best_id = id;
                }
            }
        }
        let mut e = f32::MAX;
        if edge {
            let mr = pts[best];
            for (k, r) in pts.iter().enumerate() {
                if k == best {
                    continue;
                }
                let diff = *r - mr;
                let l2 = diff.length_squared();
                if l2 > 1e-8 {
                    e = e.min((0.5 * (mr + *r)).dot(diff) / l2.sqrt());
                }
            }
        }
        Cell {
            id: best_id,
            f1: d1.sqrt(),
            edge: e,
            rel: -pts[best],
        }
    }
}

/// A sparse round feature (pore, pebble, bubble, lichen colony).
#[derive(Clone, Copy)]
struct Spot {
    /// Distance from the center / radius (`f32::MAX` when there is none nearby).
    d: f32,
    /// Radius in meters.
    r: f32,
    id: u32,
    /// Point minus center, meters.
    rel: Vec2,
}

/// Sparse round spots: at most one per lattice cell, precomputed and binned per cell.
struct SpotField {
    n: IVec2,
    cm: Vec2,
    /// `spots[start[cell]..start[cell + 1]]`: the spots that reach into the cell, as
    /// (center relative to the cell's corner in lattice units, radius in meters, id).
    start: Vec<u32>,
    spots: Vec<(Vec2, f32, u32)>,
}

impl SpotField {
    /// Spots are only reported out to this many radii (callers fade them out before).
    const REACH: f32 = 1.75;

    /// Cells about `cell` meters; a cell at uv holds a spot with probability `fill(uv)`, of a
    /// radius in `radius` (meters, at most about a cell).
    fn new(
        c: &Canvas,
        cell: Vec2,
        seed: u32,
        radius: (f32, f32),
        fill: impl Fn(Vec2) -> f32,
    ) -> Self {
        let n = c.cells(cell);
        let cm = c.tile / n.as_vec2();
        let mut cells = Vec::with_capacity((n.x * n.y) as usize);
        for y in 0..n.y {
            for x in 0..n.x {
                let h = hash(x, y, seed);
                let at = (Vec2::new(x as f32, y as f32) + 0.5) / n.as_vec2();
                let center = Vec2::new(
                    0.15 + 0.7 * unit(rehash(h, 1)),
                    0.15 + 0.7 * unit(rehash(h, 2)),
                );
                let r = mix(radius.0, radius.1, unit(rehash(h, 3)));
                let present = unit(h) < fill(at);
                cells.push((center, if present { r } else { 0.0 }, h));
            }
        }
        let mut start = Vec::with_capacity(cells.len() + 1);
        let mut spots = Vec::new();
        for y in 0..n.y {
            for x in 0..n.x {
                start.push(spots.len() as u32);
                for j in -1..=1i32 {
                    let cy = wrap_i(y + j, n.y);
                    for i in -1..=1i32 {
                        let cx = wrap_i(x + i, n.x);
                        let (o, r, id) = cells[(cy * n.x + cx) as usize];
                        if r <= 0.0 {
                            continue;
                        }
                        let center = Vec2::new(i as f32, j as f32) + o;
                        let reach = Self::REACH * r / cm;
                        let near = center.clamp(Vec2::ZERO, Vec2::ONE);
                        if (center - near).abs().cmple(reach).all() {
                            spots.push((center, r, id));
                        }
                    }
                }
            }
        }
        start.push(spots.len() as u32);
        Self {
            n,
            cm,
            start,
            spots,
        }
    }

    fn at(&self, uv: Vec2) -> Spot {
        let p = uv * self.n.as_vec2();
        let b = p.ffloor();
        let f = p - b;
        let cell =
            (wrap_idx(b.y as i32, self.n.y) * self.n.x + wrap_idx(b.x as i32, self.n.x)) as usize;
        let mut best = Spot {
            d: f32::MAX,
            r: 1.0,
            id: 0,
            rel: Vec2::ZERO,
        };
        let (s0, s1) = (self.start[cell] as usize, self.start[cell + 1] as usize);
        for &(center, r, id) in &self.spots[s0..s1] {
            let rel = (f - center) * self.cm;
            let d = rel.length() / r;
            if d < best.d && d < Self::REACH {
                best = Spot { d, r, id, rel };
            }
        }
        best
    }
}

/// Settings for thin tapered line segments (scratches, grass blades).
struct Strokes {
    /// Grid cell size in meters; a stroke is at most two cells long.
    cell: Vec2,
    /// Candidate strokes per cell (each kept with the density's probability).
    per_cell: u32,
    /// Length range in meters.
    len: (f32, f32),
    /// Width range in meters.
    width: (f32, f32),
    /// Mean direction (radians from +u) and the ± spread around it.
    angle: f32,
    spread: f32,
    seed: u32,
}

/// The stroke covering a point most.
#[derive(Clone, Copy, Default)]
struct Stroke {
    cover: f32,
    /// Across the stroke, -1..1 inside.
    across: f32,
    id: u32,
}

#[derive(Clone, Copy)]
struct Seg {
    /// Center relative to a cell's corner (lattice units).
    c: Vec2,
    dir: Vec2,
    half: f32,
    hw: f32,
    id: u32,
}

/// Precomputed strokes, binned per lattice cell.
struct StrokeField {
    n: IVec2,
    cm: Vec2,
    /// `segs[start[cell]..start[cell + 1]]`: the strokes that reach into the cell.
    start: Vec<u32>,
    segs: Vec<Seg>,
    aa: f32,
}

impl StrokeField {
    fn new(c: &Canvas, s: &Strokes, density: impl Fn(Vec2) -> f32) -> Self {
        Self::with_dir(c, s, |uv| (density(uv), None))
    }

    /// Like [`StrokeField::new`], with the density and, optionally, the mean direction
    /// (radians from +u, overriding `s.angle`) given per cell.
    fn with_dir(c: &Canvas, s: &Strokes, field: impl Fn(Vec2) -> (f32, Option<f32>)) -> Self {
        let n = c.cells(s.cell);
        let cm = c.tile / n.as_vec2();
        let aa = c.texel().min_element() * 0.6;
        let mut cells = Vec::with_capacity((n.x * n.y) as usize);
        for y in 0..n.y {
            for x in 0..n.x {
                let base = hash(x, y, s.seed);
                let at = (Vec2::new(x as f32, y as f32) + 0.5) / n.as_vec2();
                let (dens, dir) = field(at);
                let mean = dir.unwrap_or(s.angle);
                let mut segs = Vec::new();
                for k in 0..s.per_cell {
                    let h = rehash(base, k * 8 + 1);
                    if unit(h) >= dens {
                        continue;
                    }
                    let len = mix(s.len.0, s.len.1, unit(rehash(h, 4)));
                    segs.push(Seg {
                        c: Vec2::new(unit(rehash(h, 2)), unit(rehash(h, 3))),
                        dir: Vec2::from_angle(mean + (unit(rehash(h, 5)) - 0.5) * 2.0 * s.spread),
                        half: (0.5 * len).min(cm.min_element()),
                        hw: 0.5 * mix(s.width.0, s.width.1, unit(rehash(h, 6))),
                        id: h,
                    });
                }
                cells.push(segs);
            }
        }
        let mut start = Vec::with_capacity(cells.len() + 1);
        let mut segs = Vec::new();
        for y in 0..n.y {
            for x in 0..n.x {
                start.push(segs.len() as u32);
                for j in -1..=1i32 {
                    let cy = wrap_i(y + j, n.y);
                    for i in -1..=1i32 {
                        let cx = wrap_i(x + i, n.x);
                        for sg in &cells[(cy * n.x + cx) as usize] {
                            let center = Vec2::new(i as f32, j as f32) + sg.c;
                            let reach = (sg.dir.abs() * sg.half + sg.hw + aa) / cm;
                            let near = center.clamp(Vec2::ZERO, Vec2::ONE);
                            if (center - near).abs().cmple(reach).all() {
                                segs.push(Seg { c: center, ..*sg });
                            }
                        }
                    }
                }
            }
        }
        start.push(segs.len() as u32);
        Self {
            n,
            cm,
            start,
            segs,
            aa,
        }
    }

    fn at(&self, uv: Vec2) -> Stroke {
        let p = uv * self.n.as_vec2();
        let b = p.ffloor();
        let f = p - b;
        let cell =
            (wrap_idx(b.y as i32, self.n.y) * self.n.x + wrap_idx(b.x as i32, self.n.x)) as usize;
        let mut best = Stroke::default();
        let (s0, s1) = (self.start[cell] as usize, self.start[cell + 1] as usize);
        for s in &self.segs[s0..s1] {
            let rel = (f - s.c) * self.cm;
            let along = rel.dot(s.dir);
            if along.abs() >= s.half {
                continue;
            }
            let t = along / s.half;
            let hw = s.hw * (1.0 - t * t).sqrt();
            let across = s.dir.perp_dot(rel);
            let cover = 1.0 - ss(hw - self.aa, hw + self.aa, across.abs());
            if cover > best.cover {
                best = Stroke {
                    cover,
                    across: across / hw.max(1e-6),
                    id: s.id,
                };
            }
        }
        best
    }
}

/// A periodic profile (one growth ring) that can be box-filtered analytically through its
/// running integral, so fine stripes don't alias.
struct Profile {
    cum: Vec<f32>,
    total: f32,
}

impl Profile {
    const N: usize = 256;

    fn new(f: impl Fn(f32) -> f32) -> Self {
        let mut cum = vec![0.0; Self::N + 1];
        for i in 0..Self::N {
            let t = (i as f32 + 0.5) / Self::N as f32;
            cum[i + 1] = cum[i] + f(t) / Self::N as f32;
        }
        let total = cum[Self::N];
        Self { cum, total }
    }

    fn integral(&self, x: f32) -> f32 {
        let k = x.ffloor();
        let t = (x - k) * Self::N as f32;
        let i = (t as usize).min(Self::N - 1);
        let f = t - i as f32;
        k * self.total + self.cum[i] + (self.cum[i + 1] - self.cum[i]) * f
    }

    /// The profile averaged over phases `x ± w/2`.
    fn filtered(&self, x: f32, w: f32) -> f32 {
        let w = w.max(0.02);
        (self.integral(x + 0.5 * w) - self.integral(x - 0.5 * w)) / w
    }
}

/// Phase change across one texel of a ring-phase field (the filter width), ignoring jumps
/// at board joints.
fn ring_width(phase: &[f32], w: usize, h: usize, x: usize, y: usize) -> f32 {
    let p = phase[y * w + x];
    let one_sided = |a: f32, b: f32| if a.abs() < b.abs() { a } else { b };
    let dx = one_sided(
        phase[y * w + next_idx(x, w)] - p,
        p - phase[y * w + prev_idx(x, w)],
    );
    let dy = one_sided(
        phase[next_idx(y, h) * w + x] - p,
        p - phase[prev_idx(y, h) * w + x],
    );
    (dx * dx + dy * dy).sqrt()
}

// ---------------------------------------------------------------------------------------------
// Wood
// ---------------------------------------------------------------------------------------------

/// Growth rings: ring phase (years) as a function of the distance from the pith.
struct Rings {
    step: f32,
    table: Vec<f32>,
}

impl Rings {
    /// Rings out to `max_r` meters, about `width` meters each: wider near the pith, and
    /// varying from year to year like real climate does.
    fn new(rng: &mut Rng, max_r: f32, width: f32) -> Self {
        let step = 0.0001;
        let count = (max_r / step) as usize + 2;
        let mut table = Vec::with_capacity(count);
        let (mut phase, mut next, mut w, mut climate) = (0.0, 0.0, width, 0.5);
        for i in 0..count {
            let r = i as f32 * step;
            if r >= next {
                climate = 0.55 * climate + 0.45 * rng.f32();
                w = width * (1.0 + 0.6 * ss(0.05, 0.0, r)) * (0.35 + 1.3 * climate);
                next += w;
            }
            table.push(phase);
            phase += step / w;
        }
        Self { step, table }
    }

    fn phase(&self, r: f32) -> f32 {
        let t = (r.max(0.0) / self.step).min((self.table.len() - 2) as f32);
        let i = t as usize;
        self.table[i] + (self.table[i + 1] - self.table[i]) * (t - i as f32)
    }
}

/// One board: a plane `depth` from the pith of a log whose axis runs along the board.
struct Board {
    /// Pith offset across the board (meters from its center line).
    pith: f32,
    depth: f32,
    /// The log's waviness/taper along the board: depth amplitude (m) and cycles per tile.
    amp: f32,
    cycles: f32,
    shift: f32,
    r_off: f32,
    tone: f32,
    warm: f32,
}

/// Boards side by side across a tile, centered on across = k / n: a pith under the board
/// gives cathedral arches (flat sawn), a pith off to the side straight lines (rift sawn).
struct Boards {
    n: usize,
    width: f32,
    list: Vec<Board>,
    rings: Rings,
}

impl Boards {
    fn new(rng: &mut Rng, n: usize, width: f32, ring: f32) -> Self {
        let list = (0..n)
            .map(|_| {
                let flat = rng.chance(0.65);
                let side = if rng.chance(0.5) { 1.0 } else { -1.0 };
                Board {
                    pith: if flat {
                        rng.range(-0.3, 0.3) * width
                    } else {
                        side * rng.range(0.8, 2.0) * width
                    },
                    depth: rng.range(0.025, 0.1),
                    amp: rng.range(0.006, 0.018),
                    cycles: rng.range_i32(1, 3) as f32,
                    shift: rng.f32(),
                    r_off: rng.range(0.0, 0.03),
                    tone: rng.range(-1.0, 1.0),
                    warm: rng.f32(),
                }
            })
            .collect();
        let rings = Rings::new(rng, 0.7, ring);
        Self {
            n,
            width,
            list,
            rings,
        }
    }

    /// The board under `across` (tile units) and the meters from its center line.
    fn locate(&self, across: f32) -> (usize, f32) {
        let f = across * self.n as f32 + 0.5;
        let b = f.ffloor();
        (
            (b as i32).rem_euclid(self.n as i32) as usize,
            (f - b - 0.5) * self.width,
        )
    }

    /// Ring phase `x` meters from board `i`'s center line, `along` tile units down it, with
    /// the rings pushed out by `extra` meters (wobble, knots).
    fn phase(&self, i: usize, x: f32, along: f32, extra: f32) -> f32 {
        let b = &self.list[i];
        let d = b.depth + b.amp * (TAU * (along * b.cycles + b.shift)).sin();
        let r = ((x - b.pith).powi(2) + d * d).sqrt() + b.r_off + extra;
        self.rings.phase(r)
    }
}

/// A knot: the rings swirl around it.
struct Knot {
    at: Vec2,
    /// Radius in meters.
    radius: f32,
    /// Elongation along v (a branch cut at an angle).
    stretch: f32,
    /// How far the rings are pushed out around it (meters).
    swirl: f32,
}

impl Knot {
    fn random(rng: &mut Rng, radius: (f32, f32), ring: f32) -> Self {
        let sign = if rng.chance(0.5) { 1.0 } else { -1.0 };
        Self {
            at: Vec2::new(rng.f32(), rng.f32()),
            radius: rng.range(radius.0, radius.1),
            stretch: rng.range(1.2, 2.2),
            swirl: sign * rng.range(1.5, 3.0) * ring,
        }
    }

    /// Point minus center, meters, with the elongation removed.
    fn local(&self, c: &Canvas, uv: Vec2) -> Vec2 {
        let d = wrap(uv - self.at) * c.tile;
        Vec2::new(d.x, d.y / self.stretch)
    }

    fn push(&self, c: &Canvas, uv: Vec2) -> f32 {
        let q = self.local(c, uv);
        self.swirl * (-q.length_squared() / (self.radius * self.radius * 5.0)).exp()
    }
}

/// For each texel row of `c`, the indices of the `items` whose v-extent (center v in tile
/// units, half length in meters) covers it: per-texel loops then skip the rest.
fn rows_touched(c: &Canvas, items: impl Iterator<Item = (f32, f32)>) -> Vec<Vec<u16>> {
    let mut rows = vec![Vec::new(); c.h];
    for (k, (v, half)) in items.enumerate() {
        for (y, row) in rows.iter_mut().enumerate() {
            let rv = (y as f32 + 0.5) / c.h as f32;
            if (wrap1(rv - v) * c.tile.y).abs() <= half {
                row.push(k as u16);
            }
        }
    }
    rows
}

/// A drying check along the grain (v).
struct Crack {
    at: Vec2,
    len: f32,
    width: f32,
    depth: f32,
    seed: u32,
}

impl Crack {
    fn random(rng: &mut Rng, len: (f32, f32), width: (f32, f32), depth: (f32, f32)) -> Self {
        Self {
            at: Vec2::new(rng.f32(), rng.f32()),
            len: rng.range(len.0, len.1),
            width: rng.range(width.0, width.1),
            depth: rng.range(depth.0, depth.1),
            seed: rng.next_u32() & 0xff_ffff,
        }
    }

    /// (coverage, closeness 0..1 within a few widths).
    fn sample(&self, c: &Canvas, uv: Vec2) -> (f32, f32) {
        let dv = wrap1(uv.y - self.at.y) * c.tile.y;
        let half = self.len * 0.5;
        if dv.abs() >= half {
            return (0.0, 0.0);
        }
        // |wobble| < 1, so this can only skip texels that the test below would.
        if (wrap1(uv.x - self.at.x) * c.tile.x).abs() > 0.0215 {
            return (0.0, 0.0);
        }
        let t = dv / half;
        let per = ((c.tile.y / 0.04).fround() as i32).max(1);
        let wobble = gnoise(
            Vec2::new(uv.y * per as f32, 0.5),
            IVec2::new(per, 64),
            self.seed,
        );
        let du = wrap1(uv.x - self.at.x - wobble * 0.0015 / c.tile.x) * c.tile.x;
        if du.abs() > 0.02 {
            return (0.0, 0.0);
        }
        let taper = (1.0 - t * t).max(0.0).powf(0.6);
        let hw = 0.5 * self.width * taper * (0.75 + 0.5 * wobble.abs());
        let aa = c.texel().x * 0.7;
        let cover = 1.0 - ss(hw - aa, hw + aa, du.abs());
        let near = (1.0 - du.abs() / (hw * 5.0 + 3.0 * aa)).max(0.0) * taper;
        (cover, near)
    }
}

// ---------------------------------------------------------------------------------------------
// Surfaces
// ---------------------------------------------------------------------------------------------

fn planks(c: &Canvas, seed: u32) -> Buf<Px> {
    let mut rng = Rng::new(seed as u64);
    let n = ((c.tile.x / 0.16).fround() as usize).max(1);
    let ring = 0.0045;
    let boards = Boards::new(&mut rng, n, c.tile.x / n as f32, ring);
    let knots: Vec<Knot> = (0..3)
        .map(|_| Knot::random(&mut rng, (0.006, 0.013), ring))
        .collect();
    let cracks: Vec<Crack> = (0..14)
        .map(|_| Crack::random(&mut rng, (0.08, 0.6), (0.0005, 0.0026), (1.5, 4.0)))
        .collect();
    // Nails: a pair across each board, rows every ~0.6 m starting at v = 0.
    let rows = ((c.tile.y / 0.6).fround() as usize).max(1);
    let mut nails = Vec::new();
    for r in 0..rows {
        for b in 0..n {
            for side in [-1.0f32, 1.0] {
                let u = b as f32 / n as f32 + (side * 0.045 + rng.range(-0.005, 0.005)) / c.tile.x;
                let v = r as f32 / rows as f32 + rng.range(-0.008, 0.008) / c.tile.y;
                let present = !rng.chance(0.1);
                let head = rng.chance(0.75);
                let radius = rng.range(0.0028, 0.0036);
                if present {
                    nails.push((Vec2::new(u, v), head, radius));
                }
            }
        }
    }
    let crack_rows = rows_touched(c, cracks.iter().map(|cr| (cr.at.y, 0.5 * cr.len)));
    let nail_rows = rows_touched(c, nails.iter().map(|n| (n.0.y, 0.045)));
    let wobble = c.grid(4, |uv| {
        0.0022 * c.fbm(uv, Vec2::new(0.04, 0.3), seed + 20, 3)
            + 0.0007 * c.fbm(uv, Vec2::new(0.012, 0.06), seed + 21, 2)
    });
    let phase = c.field(|uv| {
        let (b, x) = boards.locate(uv.x);
        let extra = wobble.smooth(uv) + knots.iter().map(|k| k.push(c, uv)).sum::<f32>();
        boards.phase(b, x, uv.y, extra)
    });
    let tone = c.grid(4, |uv| c.fbm(uv, Vec2::new(0.1, 0.5), seed + 3, 4));
    let warm = c.mask(4, Vec2::new(0.05, 0.4), seed + 4, 0.35, 0.1);
    let mildew = c.mask(4, Vec2::new(0.01, 0.15), seed + 5, 0.18, 0.08);
    let cup = c.grid(4, |uv| c.fbm(uv, Vec2::new(0.15, 0.8), seed + 6, 3));
    let prof = Profile::new(|t| ss(0.3, 0.92, t).powf(1.6));
    let silver = hex(0xa29e94);
    let grey = hex(0x77736a);
    let brown = hex(0x7f6a55);
    let crack_col = hex(0x1d1a17);
    let knot_col = hex(0x3b2a1e);
    let texel = c.texel();
    c.fill(|x, y, uv| {
        let i = y * c.w + x;
        let (bi, bx) = boards.locate(uv.x);
        let board = &boards.list[bi];
        let lw = prof.filtered(phase[i], ring_width(&phase, c.w, c.h, x, y));
        let fiber = c.noise(uv, Vec2::new(texel.x * 4.0, 0.02), seed + 1);
        let fine = c.noise(uv, Vec2::new(texel.x * 2.0, 0.008), seed + 2);
        let pit = ss(
            0.5,
            0.8,
            c.noise(uv, Vec2::new(texel.x * 2.0, 0.005), seed + 7),
        );
        let t = tone.at(uv) + 0.2 * board.tone;
        let mut col = grey.lerp(silver, (0.5 + 1.3 * t).clamp(0.0, 1.0));
        col = col.lerp(brown, warm.at(uv) * (0.25 + 0.5 * board.warm));
        col *= 1.0 - 0.36 * lw;
        col *= 1.0 - 0.45 * mildew.at(uv);
        col *= 1.0 + 0.12 * fiber + 0.07 * fine;
        col *= 1.0 - 0.45 * pit;
        let edge = ss(0.062, 0.083, bx.abs());
        col *= 1.0 - 0.15 * edge;
        let mut h =
            0.5 * cup.smooth(uv) + 0.35 * lw + 0.08 * fiber + 0.035 * fine - 0.3 * pit - 0.3 * edge;
        let mut rough = 0.84 + 0.04 * t - 0.07 * lw + 0.05 * pit;
        let mut ao = 1.0;
        let mut metal = 0.0;
        for k in &knots {
            let q = k.local(c, uv);
            let r = q.length() / k.radius;
            if r < 1.3 {
                let inside = 1.0 - ss(0.85, 1.0, r);
                let rings = (q.length() * 1000.0 / 1.6 + 0.6 * fiber).ffract();
                let kc = knot_col * (0.75 + 0.35 * ss(0.2, 0.8, rings));
                col = col.lerp(kc, inside);
                col *= 1.0 - 0.5 * (-((r - 1.0) / 0.1).powi(2)).exp();
                h += 0.2 * inside;
                rough += (0.72 - rough) * inside;
                let ang = q.y.atan2(q.x);
                let check = (1.0 - ss(0.0, 0.06, (ang * 1.5 + k.swirl * 300.0).sin().abs() * r))
                    * ss(0.15, 0.4, r)
                    * inside;
                col *= 1.0 - 0.7 * check;
                h -= 1.0 * check;
            }
        }
        for &k in &crack_rows[y] {
            let cr = &cracks[k as usize];
            let (cover, near) = cr.sample(c, uv);
            if near <= 0.0 {
                continue;
            }
            col *= 1.0 - 0.3 * near;
            h -= 0.3 * near + cr.depth * cover;
            col = col.lerp(crack_col, cover);
            ao *= 1.0 - 0.8 * cover;
            rough += (0.95 - rough) * cover;
        }
        for &k in &nail_rows[y] {
            let (at, head, radius) = nails[k as usize];
            let d = wrap(uv - at) * c.tile;
            if d.x.abs() > 0.012 || d.y.abs() > 0.045 {
                continue;
            }
            // Iron-tannin stain wicking along the grain.
            let e = Vec2::new(d.x / 0.0075, d.y / 0.035).length();
            let stain = (1.0 - e).max(0.0).powf(0.6) * (0.75 + 0.5 * fiber);
            col *= 1.0 - 0.6 * stain.min(1.0);
            let r = d.length() / radius;
            h -= 0.35 * ss(2.6, 1.0, r);
            let aa = texel.x / radius;
            let cover = 1.0 - ss(1.0 - aa, 1.0 + aa, r);
            if cover <= 0.0 {
                continue;
            }
            if head {
                let rust = c.noise(uv, Vec2::splat(0.0015), seed + 9);
                let head_col = hex(0x5d5852).lerp(hex(0x5e3a20), ss(-0.25, 0.35, rust));
                col = col.lerp(head_col * (1.0 - 0.3 * r * r), cover);
                metal += 0.55 * ss(0.35, -0.25, rust) * cover;
                rough += (0.6 - rough) * cover;
                h -= 0.15 * cover;
            } else {
                col = col.lerp(Vec3::splat(0.012), cover);
                h -= 4.0 * cover * (1.0 - r * r).max(0.0);
                ao *= 1.0 - 0.9 * cover;
            }
        }
        Px {
            h,
            c: col,
            r: rough.clamp(0.55, 0.97),
            m: metal,
            ao,
            ..Px::default()
        }
    })
}

fn post(c: &Canvas, seed: u32) -> Buf<Px> {
    let mut rng = Rng::new(seed as u64);
    let cracks: Vec<Crack> = (0..10)
        .map(|_| Crack::random(&mut rng, (0.15, 0.85), (0.002, 0.007), (4.0, 10.0)))
        .collect();
    let knots: Vec<(Vec2, f32)> = (0..2)
        .map(|_| (Vec2::new(rng.f32(), rng.f32()), rng.range(0.012, 0.022)))
        .collect();
    let crack_rows = rows_touched(c, cracks.iter().map(|cr| (cr.at.y, 0.5 * cr.len)));
    let texel = c.texel();
    let thick = hex(0x2a2622);
    let weathered = hex(0x4c453d);
    let wood = hex(0x7a7268);
    let facet = c.grid(4, |uv| c.fbm(uv, Vec2::new(0.05, 0.45), seed, 3));
    let tar = c.mask(4, Vec2::new(0.02, 0.3), seed + 3, 0.4, 0.22);
    let worn = c.mask(4, Vec2::new(0.01, 0.25), seed + 4, 0.3, 0.2);
    let grime = c.grid(4, |uv| c.fbm(uv, Vec2::new(0.15, 0.3), seed + 5, 3));
    c.fill(|_, y, uv| {
        let fiber = c.noise(uv, Vec2::new(texel.x * 3.0, 0.04), seed + 1);
        let fiber2 = c.noise(uv, Vec2::new(texel.x * 8.0, 0.12), seed + 2);
        let f = facet.smooth(uv);
        // Creosote soaked in along the grain: darker streaks, never flat patches; the
        // raised fibers wear through to grey wood.
        let pooled = (tar.at(uv) * (0.75 + 0.5 * fiber2)).clamp(0.0, 1.0);
        let bare = ss(0.45, 0.85, worn.at(uv) + 0.5 * fiber + 0.3 * fiber2) * (1.0 - 0.7 * pooled);
        let mut col = weathered.lerp(thick, 0.85 * pooled).lerp(wood, 0.8 * bare);
        col *= 1.0 + 0.16 * fiber + 0.1 * fiber2 + 0.12 * grime.at(uv);
        let mut h = 1.2 * f + 0.1 * fiber + 0.15 * fiber2 + 0.25 * pooled - 0.12 * bare;
        let mut rough = 0.72 - 0.28 * pooled + 0.12 * bare;
        let mut ao = 1.0;
        for &(at, radius) in &knots {
            let d = wrap(uv - at) * c.tile;
            let r = Vec2::new(d.x, d.y / 1.3).length() / radius;
            if r < 1.4 {
                let inside = 1.0 - ss(0.8, 1.05, r);
                col = col.lerp(thick * 0.7, inside);
                h += 1.5 * inside * (1.0 - r * r).max(0.0)
                    - 0.6 * (-((r - 1.0) / 0.12).powi(2)).exp();
                rough += (0.5 - rough) * inside;
            }
        }
        for &k in &crack_rows[y] {
            let cr = &cracks[k as usize];
            let (cover, near) = cr.sample(c, uv);
            if near <= 0.0 {
                continue;
            }
            h -= 0.8 * near + cr.depth * cover;
            col *= 1.0 - 0.35 * near;
            col = col.lerp(Vec3::splat(0.012), cover);
            ao *= 1.0 - 0.85 * cover;
            rough += (0.9 - rough) * cover;
        }
        Px {
            h,
            c: col,
            r: rough.clamp(0.35, 0.95),
            ao,
            ..Px::default()
        }
    })
}

/// Top coat color of [`Surface::PaintedWood`] (linear); the color variants scale it.
const PAINT_WHITE: Vec3 = Vec3::new(0.74, 0.725, 0.68);

fn painted_wood(c: &Canvas, seed: u32) -> Buf<Px> {
    let mut rng = Rng::new(seed as u64);
    let n = ((c.tile.x / 0.16).fround() as usize).max(1);
    let boards = Boards::new(&mut rng, n, c.tile.x / n as f32, 0.004);
    let wobble = c.grid(4, |uv| {
        0.002 * c.fbm(uv, Vec2::new(0.04, 0.3), seed + 20, 3)
    });
    let phase = c.field(|uv| {
        let (b, x) = boards.locate(uv.x);
        boards.phase(b, x, uv.y, wobble.smooth(uv))
    });
    let prof = Profile::new(|t| ss(0.3, 0.92, t).powf(1.5));
    let wear = c.grid(4, |uv| c.fbm(uv, Vec2::new(0.2, 0.3), seed + 3, 4));
    let chalk = c.grid(4, |uv| c.fbm(uv, Vec2::new(0.1, 0.2), seed + 7, 4));
    let dirt = c.mask(4, Vec2::new(0.03, 0.25), seed + 8, 0.3, 0.15);
    let crazing = c.grid(4, |uv| c.fbm(uv, Vec2::new(0.05, 0.2), seed + 9, 3));
    let texel = c.texel();
    let wood_light = hex(0x8e877c);
    let wood_dark = hex(0x5a5148);
    let primer = hex(0x8a4b35);
    c.fill(|x, y, uv| {
        let i = y * c.w + x;
        let (_, bx) = boards.locate(uv.x);
        let lw = prof.filtered(phase[i], ring_width(&phase, c.w, c.h, x, y));
        let fiber = c.noise(uv, Vec2::new(texel.x * 3.0, 0.03), seed + 1);
        let wood_h = 0.25 * lw + 0.05 * fiber;
        let wood = wood_light.lerp(wood_dark, 0.35 + 0.5 * lw) * (1.0 + 0.12 * fiber);

        // Chips: wear zones and board edges × chip shapes stretched along the grain, lifting
        // along latewood.
        let chip = c.fbm_g(uv, Vec2::new(0.016, 0.045), seed + 4, 5, 0.6);
        let edge = ss(0.055, 0.083, bx.abs());
        let f = 0.85 * chip + 0.75 * wear.at(uv) + 0.18 * lw + 0.4 * edge;
        let (t_top, t_under, ew) = (0.44, 0.54, 0.01);
        let top_gone = ss(t_top - ew, t_top + ew, f);
        let under_gone = ss(t_under - ew, t_under + ew, f);
        let lip = (-((f - t_top) / 0.018).powi(2)).exp();

        // Paint: brush streaks along v, chalky and dirty patches, hairline cracks along grain.
        let brush = 0.6 * c.noise(uv, Vec2::new(texel.x * 4.0, 0.08), seed + 5)
            + 0.4 * c.noise(uv, Vec2::new(0.004, 0.25), seed + 6);
        let ch = chalk.at(uv);
        let dt = dirt.at(uv);
        let hairline =
            ss(0.93, 0.99, prof.filtered(phase[i] + 0.25, 0.05)) * ss(0.1, 0.35, crazing.at(uv));
        let mut paint = PAINT_WHITE * (1.0 + 0.04 * ch + 0.025 * brush);
        paint *= 1.0 - 0.2 * dt - 0.3 * hairline - 0.08 * edge;
        let under = primer * (0.9 + 0.2 * ch);
        let mut col = wood.lerp(under, 1.0 - under_gone);
        col = col.lerp(paint, 1.0 - top_gone);
        col *= 1.0 - 0.35 * lip * top_gone.max(0.3);
        let h = wood_h * (1.0 - 0.4 * (1.0 - under_gone))
            + 0.12 * (1.0 - under_gone)
            + 0.15 * (1.0 - top_gone)
            + 0.025 * brush * (1.0 - top_gone)
            + 0.03 * lip
            - 0.05 * hairline;
        let paint_rough = 0.36 + 0.14 * ss(-0.2, 0.5, ch) + 0.12 * dt;
        let rough = mix(paint_rough, mix(0.6, 0.85, under_gone), top_gone);
        Px {
            h,
            c: col,
            r: rough,
            ao: 1.0 - 0.25 * under_gone * (1.0 - lw),
            paint: 1.0 - top_gone,
            ..Px::default()
        }
    })
}

fn varnish(c: &Canvas, seed: u32) -> Buf<Px> {
    let mut rng = Rng::new(seed as u64);
    let n = ((c.tile.x / 0.25).fround() as usize).max(1);
    let boards = Boards::new(&mut rng, n, c.tile.x / n as f32, 0.0026);
    let wobble = c.grid(4, |uv| {
        0.0012 * c.fbm(uv, Vec2::new(0.03, 0.25), seed + 20, 3)
    });
    let phase = c.field(|uv| {
        let (b, x) = boards.locate(uv.x);
        boards.phase(b, x, uv.y, wobble.smooth(uv))
    });
    let prof = Profile::new(|t| ss(0.45, 0.95, t).powf(1.3));
    let texel = c.texel();
    let early = hex(0xa5602f);
    let late = hex(0x6b3618);
    let ribbons = (c.tile.x / 0.035).fround();
    let ribbon_warp = c.grid(4, |uv| c.fbm(uv, Vec2::new(0.05, 0.3), seed + 2, 2));
    let tone = c.grid(4, |uv| c.fbm(uv, Vec2::new(0.12, 0.5), seed + 4, 3));
    let rough_var = c.grid(4, |uv| c.fbm(uv, Vec2::splat(0.08), seed + 7, 3));
    let scratches = StrokeField::new(
        c,
        &Strokes {
            cell: Vec2::splat(0.05),
            per_cell: 1,
            len: (0.015, 0.08),
            width: (0.00025, 0.0006),
            angle: 0.0,
            spread: PI,
            seed: seed + 9,
        },
        |_| 0.5,
    );
    c.fill(|x, y, uv| {
        let i = y * c.w + x;
        let lw = prof.filtered(phase[i], ring_width(&phase, c.w, c.h, x, y));
        let ribbon = (TAU * (uv.x * ribbons + 0.7 * ribbon_warp.at(uv))).sin();
        let pores = ss(
            0.4,
            0.7,
            c.noise(uv, Vec2::new(texel.x * 2.0, 0.005), seed + 3),
        );
        let mut col = early.lerp(late, lw * 0.85);
        col *= (1.0 + 0.12 * ribbon) * (1.0 - 0.4 * pores) * (1.0 + 0.2 * tone.at(uv));
        let s = scratches.at(uv);
        let peel = c.fbm(uv, Vec2::splat(0.004), seed + 5, 2);
        let brush = c.noise(uv, Vec2::new(0.006, 0.15), seed + 6);
        col *= 1.0 + 0.15 * s.cover;
        let h = 0.012 * lw + 0.008 * peel + 0.01 * brush - 0.008 * s.cover - 0.006 * pores;
        let rough = 0.27 + 0.04 * rough_var.at(uv) + 0.18 * s.cover;
        Px {
            h,
            c: col,
            r: rough,
            ..Px::default()
        }
    })
}

fn gelcoat(c: &Canvas, seed: u32) -> Buf<Px> {
    let scratches = StrokeField::new(
        c,
        &Strokes {
            cell: Vec2::splat(0.05),
            per_cell: 1,
            len: (0.02, 0.1),
            width: (0.0003, 0.0008),
            angle: 0.0,
            spread: 0.5,
            seed: seed + 1,
        },
        |_| 0.45,
    );
    let base = hex(0xe6e4dc);
    let weave_u = (c.tile.x / 0.008).fround();
    let weave_v = (c.tile.y / 0.008).fround();
    let yellow = c.grid(8, |uv| c.fbm(uv, Vec2::splat(0.3), seed + 2, 3));
    let scuffs = c.mask(4, Vec2::new(0.05, 0.03), seed + 4, 0.08, 0.1);
    c.fill(|_, _, uv| {
        let mut col =
            base * Vec3::ONE.lerp(Vec3::new(1.0, 0.985, 0.93), ss(0.0, 0.6, yellow.at(uv)));
        let peel = c.fbm(uv, Vec2::splat(0.0035), seed + 3, 2);
        let weave = (TAU * uv.x * weave_u).sin() * (TAU * uv.y * weave_v).sin();
        let streak = c.noise(uv, Vec2::new(0.01, 0.003), seed + 5);
        let scuff = scuffs.at(uv) * (0.6 + 0.4 * streak);
        let s = scratches.at(uv);
        col *= 1.0 - 0.12 * scuff;
        col = col.lerp(Vec3::splat(0.8), 0.5 * s.cover);
        let h = 0.008 * peel + 0.0025 * weave - 0.01 * s.cover - 0.004 * scuff;
        let rough = 0.13 + 0.02 * peel + 0.22 * scuff + 0.25 * s.cover;
        Px {
            h,
            c: col,
            r: rough,
            ..Px::default()
        }
    })
}

fn non_slip(c: &Canvas, seed: u32) -> Buf<Px> {
    let n = c.cells(Vec2::splat(0.004)).as_vec2();
    let base = hex(0xbcbcb6);
    let wear = c.grid(4, |uv| c.fbm(uv, Vec2::splat(0.04), seed + 1, 4));
    let dirt = c.mask(4, Vec2::splat(0.02), seed + 2, 0.4, 0.15);
    c.fill(|_, _, uv| {
        let p = uv * n;
        let f = p - p.ffloor() - 0.5;
        let l1 = f.x.abs() + f.y.abs();
        let diamond = ss(0.42, 0.3, l1);
        let grit = c.noise(uv, Vec2::splat(0.0009), seed);
        let worn_tops = diamond * ss(-0.1, 0.4, wear.at(uv));
        let mut col = base * (1.0 + 0.04 * grit);
        col *= 1.0 - (1.0 - diamond) * (0.2 + 0.3 * dirt.at(uv));
        col *= 1.0 + 0.05 * worn_tops;
        let h = 0.35 * diamond + 0.012 * grit;
        let rough = 0.72 - 0.18 * worn_tops + 0.08 * (1.0 - diamond);
        Px {
            h,
            c: col,
            r: rough,
            ..Px::default()
        }
    })
}

/// Weathered pine/spruce bark: grey-brown plates of flaky scales split by soft, darker
/// brown furrows (rounded valleys, not black lines), low contrast from a few meters.
fn bark(c: &Canvas, seed: u32) -> Buf<Px> {
    let plates = c.sites(Vec2::new(0.05, 0.17), seed + 2, 0.9);
    let scales = c.sites(Vec2::new(0.024, 0.016), seed + 3, 0.9);
    let wx = c.grid(2, |uv| c.fbm(uv, Vec2::new(0.06, 0.2), seed, 3));
    let wy = c.grid(2, |uv| c.fbm(uv, Vec2::new(0.06, 0.2), seed + 1, 3));
    let silvered = c.mask(4, Vec2::new(0.08, 0.2), seed + 7, 0.4, 0.25);
    let grime = c.grid(4, |uv| c.fbm(uv, Vec2::splat(0.1), seed + 8, 3));
    let plate = hex(0x716b64);
    let silver = hex(0x8c8780);
    let inner = hex(0x6f5a4c);
    let furrow = hex(0x4a4139);
    let texel = c.texel();
    c.fill(|_, _, uv| {
        let warp = Vec2::new(0.7 * wx.smooth(uv), 0.35 * wy.smooth(uv));
        let jag = c.noise(uv, Vec2::new(0.006, 0.015), seed + 4);
        let cell = plates.at(uv, warp, true);
        let e = cell.edge + 0.02 * jag;
        // 0 on the plate, 1 at the bottom of the furrow: a wide rounded valley, darker
        // only toward its bottom.
        let valley = ss(0.3, 0.0, e);
        let dark = ss(0.16, 0.0, e);
        let id = cell.id;
        let tilt = Vec2::new(unit(rehash(id, 1)) - 0.5, unit(rehash(id, 2)) - 0.5);
        // Flaky scales on the plates: shallow steps, a few showing a browner inner layer.
        let sc = scales.at(uv, warp * 2.0, true);
        let step = unit(rehash(sc.id, 1));
        let scale_edge = ss(0.08, 0.0, sc.edge);
        let fiber = c.noise(uv, Vec2::new(texel.x * 3.0, 0.02), seed + 5);
        let plate_h = 2.0 * tilt.dot(cell.rel)
            + 1.0 * step
            + 0.5 * c.fbm(uv, Vec2::new(0.006, 0.01), seed + 6, 2)
            + 0.15 * fiber
            - 0.4 * scale_edge;
        let h = plate_h * (1.0 - valley) - 6.0 * valley + 0.5 * fiber * valley;
        let fresh = ss(0.88, 0.97, step) * (1.0 - valley) * 0.45;
        let tone = 0.92 + 0.16 * unit(rehash(id, 4));
        let mut col = plate * tone * (0.94 + 0.12 * unit(rehash(sc.id, 2)));
        col = col.lerp(silver, 0.55 * silvered.at(uv) * (1.0 - valley));
        col = col.lerp(inner, fresh);
        col *= 1.0 - 0.12 * scale_edge;
        col = col.lerp(furrow, 0.7 * dark);
        col *= (1.0 + 0.07 * fiber) * (1.0 + 0.08 * grime.at(uv));
        Px {
            h,
            c: col,
            r: 0.87 + 0.05 * dark - 0.04 * fresh,
            ao: 1.0 - 0.15 * dark,
            ..Px::default()
        }
    })
}

fn end_grain(c: &Canvas, seed: u32) -> Buf<Px> {
    let mut rng = Rng::new(seed as u64);
    let rings = Rings::new(&mut rng, 0.45, 0.0035);
    let mut cracks: Vec<(f32, f32, f32)> = (0..7)
        .map(|_| {
            (
                rng.range(0.0, TAU),
                rng.range(0.05, 0.14),
                rng.range(0.002, 0.005),
            )
        })
        .collect();
    for _ in 0..2 {
        cracks.push((rng.range(0.0, TAU), -rng.range(0.02, 0.05), 0.0012));
    }
    let prof = Profile::new(|t| ss(0.4, 0.95, t).powf(1.3));
    let texel = c.texel().x;
    let heart = hex(0xa0704a);
    let sap = hex(0xc4a47c);
    let saw_center = Vec2::new(0.55, -0.4);
    let dirt = c.grid(4, |uv| c.fbm(uv, Vec2::splat(0.05), seed + 4, 4));
    let saw_jitter = c.grid(4, |uv| c.noise(uv, Vec2::splat(0.05), seed + 5));
    c.fill(|_, _, uv| {
        let p = wrap(uv) * c.tile;
        let r = p.length();
        let dir = if r > 1e-6 { p / r } else { Vec2::X };
        let ang = dir.y.atan2(dir.x);
        let wob = gnoise(dir * 2.5 + Vec2::splat(32.0), IVec2::splat(4096), seed)
            + 0.5 * gnoise(dir * 6.0 + Vec2::splat(64.0), IVec2::splat(4096), seed + 1);
        let re = r * (1.0 + 0.05 * wob) + 0.0012 * c.noise(uv, Vec2::splat(0.015), seed + 2);
        let phase = rings.phase(re);
        let dphase = (rings.phase(re + texel) - phase).abs();
        let lw = prof.filtered(phase, dphase);
        let fiber = c.noise(uv, Vec2::splat(texel * 2.5), seed + 3);
        let heartwood = ss(0.12, 0.09, re);
        let mut col = sap.lerp(heart, heartwood);
        col *= Vec3::ONE - Vec3::new(0.55, 0.62, 0.7) * lw;
        col *= 1.0 + 0.08 * fiber;
        col = col.lerp(hex(0x6e675e), 0.15 + 0.1 * dirt.at(uv));
        col *= 1.0 - 0.6 * ss(0.004, 0.0015, r);
        // Chainsaw arcs.
        let saw = (p - saw_center).length() * 1000.0 / 6.0 + 0.8 * saw_jitter.at(uv);
        let saw_f = saw - saw.ffloor();
        let saw_h = ss(0.0, 0.6, saw_f) * ss(1.0, 0.75, saw_f);
        let mut h = 0.25 * saw_h - 0.06 * lw + 0.03 * fiber;
        col *= 1.0 + 0.08 * saw_h;
        let mut rough = 0.82 + 0.06 * saw_h - 0.05 * lw;
        let mut ao = 1.0;
        for &(a0, r_in, wmax) in &cracks {
            // r_in > 0: check from the bark inward; < 0: pith check out to |r_in|.
            let (from, to, w) = if r_in > 0.0 {
                (r_in, 0.24, wmax * ss(r_in, 0.2, r))
            } else {
                (0.0, -r_in, wmax * ss(-r_in, 0.0, r))
            };
            if r < from || r > to {
                continue;
            }
            let bend = 0.05 * gnoise(Vec2::new(r * 18.0, a0 * 3.0), IVec2::splat(4096), seed + 7);
            let da = (ang - a0 - bend + PI).rem_euclid(TAU) - PI;
            let dist = da.abs() * r;
            let cover = 1.0 - ss(0.5 * w - texel * 0.7, 0.5 * w + texel * 0.7, dist);
            let near = (1.0 - dist / (2.5 * w + 2.0 * texel)).max(0.0);
            col *= 1.0 - 0.3 * near;
            if cover > 0.0 {
                col = col.lerp(Vec3::splat(0.015), cover);
                h -= 5.0 * cover;
                ao *= 1.0 - 0.85 * cover;
                rough += (0.95 - rough) * cover;
            }
        }
        Px {
            h,
            c: col,
            r: rough,
            ao,
            ..Px::default()
        }
    })
}

fn rock(c: &Canvas, seed: u32) -> Buf<Px> {
    let feldspar = hex(0xb9b2aa);
    let pink = hex(0xc4ab9c);
    let quartz = hex(0x86868a);
    let biotite = hex(0x1c1c1d);
    let patina = hex(0x8d8a84);
    let iron = hex(0x8e7158);
    let texel = c.texel();
    let grains = c.sites(Vec2::splat(0.007), seed + 2, 0.95);
    let big = c.grid(4, |uv| c.fbm(uv, Vec2::splat(0.3), seed, 5));
    let mid = c.grid(2, |uv| c.fbm(uv, Vec2::splat(0.03), seed + 1, 3));
    let weather = c.grid(4, |uv| c.fbm(uv, Vec2::splat(0.1), seed + 3, 4));
    let stain = c.mask(4, Vec2::splat(0.12), seed + 4, 0.25, 0.12);
    let streak = c.mask(4, Vec2::new(0.04, 0.35), seed + 5, 0.2, 0.12);
    // Joints: zero crossings of warped noise, kept only where a mask allows.
    let joint = c.grid(2, |uv| {
        let w = Vec2::new(
            c.fbm(uv, Vec2::splat(0.15), seed + 6, 3),
            c.fbm(uv, Vec2::splat(0.15), seed + 7, 3),
        ) * 0.08;
        c.fbm(uv + w, Vec2::splat(0.45), seed + 8, 2)
    });
    let joint_mask = c.mask(4, Vec2::splat(0.3), seed + 9, 0.18, 0.1);
    // Lichen: rosette colonies plus merged older patches.
    let zone = c.mask(8, Vec2::splat(0.3), seed + 16, 0.4, 0.25);
    let colonies = SpotField::new(c, Vec2::splat(0.06), seed + 10, (0.01, 0.035), |uv| {
        0.4 * zone.at(uv)
    });
    let merged = c.mask(4, Vec2::splat(0.1), seed + 11, 0.1, 0.06);
    let lobes = c.grid(2, |uv| c.fbm(uv, Vec2::splat(0.008), seed + 12, 2));
    let areoles = c.sites(Vec2::splat(0.0025), seed + 13, 0.9);
    let species_col = [
        hex(0xa9b296),
        hex(0xb7b98a),
        hex(0xc27a2c),
        hex(0x9fa79a),
        hex(0x3b3c35),
    ];
    c.fill(|_, _, uv| {
        let g = grains.at(uv, Vec2::ZERO, false);
        let m = unit(g.id);
        let shade = unit(rehash(g.id, 1));
        let inner = c.noise(uv, Vec2::splat(texel.x * 2.0), seed + 14);
        let (mineral, relief, mrough) = if m < 0.13 {
            let speck = ss(0.55, 0.35, g.f1);
            (feldspar.lerp(biotite, speck), -0.15 * speck, 0.8)
        } else if m < 0.45 {
            (quartz * (0.8 + 0.4 * shade), 0.25, 0.62)
        } else if m < 0.62 {
            (pink * (0.9 + 0.2 * shade), 0.1, 0.8)
        } else {
            (feldspar * (0.88 + 0.24 * shade), 0.1, 0.8)
        };
        let mineral = mineral * (1.0 + 0.08 * inner);
        let wt = (0.55 + 0.6 * weather.at(uv)).clamp(0.3, 0.85);
        let mut col = mineral.lerp(patina, wt);
        col = col.lerp(iron, stain.at(uv) * 0.45);
        let bg = big.smooth(uv);
        col *= 1.0 + 0.3 * bg;
        col *= 1.0 - 0.3 * streak.at(uv);
        let mut h = 10.0 * bg + 1.5 * mid.smooth(uv) + relief + 0.03 * inner;
        let mut rough = mrough + 0.05 * mid.at(uv);
        let mut ao = 1.0;
        let jm = joint_mask.at(uv);
        let jv = if jm > 0.0 { joint.smooth(uv) } else { 1.0 };
        if jv.abs() < 0.08 {
            // Distance to the zero line (value / slope), so flat stretches of the noise
            // near zero don't open into blobs.
            let (du, dv) = (
                Vec2::new(1.0 / c.w as f32, 0.0),
                Vec2::new(0.0, 1.0 / c.h as f32),
            );
            let slope = Vec2::new(
                (joint.smooth(uv + du) - joint.smooth(uv - du)) / (2.0 * texel.x),
                (joint.smooth(uv + dv) - joint.smooth(uv - dv)) / (2.0 * texel.y),
            );
            let dist_mm = jv.abs() / slope.length().max(1.0) * 1000.0;
            let half_mm = 0.7 + 1.5 * jm;
            let crack = ss(half_mm, half_mm * 0.2, dist_mm) * jm;
            if crack > 0.0 {
                h -= 8.0 * crack;
                col = col.lerp(hex(0x34312c), ss(0.2, 0.9, crack) * 0.8);
                ao *= 1.0 - 0.6 * crack;
                rough += (0.92 - rough) * crack;
            }
        }
        let colony = colonies.at(uv);
        let lob = lobes.at(uv);
        let dc = colony.d + 0.35 * lob;
        let in_colony = ss(1.0, 0.9, dc);
        let lichen = in_colony.max(ss(0.0, 0.3, merged.at(uv) + 0.3 * lob));
        if lichen > 0.0 {
            let pick = if in_colony > 0.0 {
                let k = unit(rehash(colony.id, 7));
                match k {
                    k if k < 0.5 => 0,
                    k if k < 0.7 => 1,
                    k if k < 0.82 => 2,
                    k if k < 0.95 => 3,
                    _ => 4,
                }
            } else {
                0
            };
            let rim = if in_colony > 0.0 {
                ss(0.55, 0.95, dc)
            } else {
                0.5
            };
            let ar = areoles.at(uv, Vec2::ZERO, true);
            let crackle = ss(0.1, 0.0, ar.edge) * (1.0 - rim);
            let bumps = c.noise(uv, Vec2::splat(texel.x * 2.5), seed + 15);
            let lc = if pick == 4 {
                // Dark crustose thallus broken into areoles: the cracks show the pale rock.
                let gaps = ss(0.3, 0.08, ar.edge) * (1.0 - 0.6 * rim);
                (species_col[4] * (0.9 + 0.15 * bumps)).lerp(col, 0.7 * gaps)
            } else {
                species_col[pick.min(4)]
                    * (0.85 + 0.25 * rim + 0.1 * bumps)
                    * (1.0 - 0.35 * crackle)
            };
            col = col.lerp(lc, lichen * 0.93);
            h += lichen * (0.4 + 0.1 * bumps - 0.25 * crackle);
            rough += (0.92 - rough) * lichen;
        }
        Px {
            h,
            c: col,
            r: rough,
            ao,
            ..Px::default()
        }
    })
}

fn concrete(c: &Canvas, seed: u32) -> Buf<Px> {
    let mut rng = Rng::new(seed as u64);
    // Form boards run along u, stacked down v.
    let n = ((c.tile.y / 0.14).fround() as usize).max(1);
    let boards = Boards::new(&mut rng, n, c.tile.y / n as f32, 0.004);
    let wobble = c.grid(4, |uv| {
        0.002 * c.fbm(uv, Vec2::new(0.3, 0.04), seed + 20, 3)
    });
    let phase = c.field(|uv| {
        let (b, x) = boards.locate(uv.y);
        boards.phase(b, x, uv.x, wobble.smooth(uv))
    });
    let prof = Profile::new(|t| ss(0.4, 0.92, t));
    let base = hex(0x9a958c);
    let texel = c.texel();
    let cluster = c.mask(8, Vec2::splat(0.15), seed + 4, 0.4, 0.2);
    let pores = SpotField::new(c, Vec2::splat(0.01), seed + 5, (0.0006, 0.003), |uv| {
        0.05 + 0.35 * cluster.at(uv)
    });
    let wear = c.mask(4, Vec2::splat(0.12), seed + 6, 0.3, 0.15);
    let agg = c.sites(Vec2::splat(0.005), seed + 7, 0.9);
    let mottle = c.grid(4, |uv| c.fbm(uv, Vec2::splat(0.12), seed + 3, 4));
    let drips = c.mask(4, Vec2::new(0.025, 0.5), seed + 8, 0.3, 0.12);
    let algae = c.mask(4, Vec2::new(0.08, 0.3), seed + 9, 0.15, 0.1);
    let efflo = c.mask(4, Vec2::new(0.03, 0.4), seed + 10, 0.06, 0.08);
    let fin_break = c.grid(4, |uv| c.noise(uv, Vec2::splat(0.05), seed + 11));
    // A form-tie hole per tile, plugged with grout, with a rust run below it.
    let tie = Vec2::new(0.5, 0.5 + 0.5 / n as f32);
    c.fill(|x, y, uv| {
        let i = y * c.w + x;
        let (bi, bx) = boards.locate(uv.y);
        let board = &boards.list[bi];
        let lw = prof.filtered(phase[i], ring_width(&phase, c.w, c.h, x, y));
        let imprint = 0.35 + 0.65 * board.warm;
        let offset = 0.6 * board.tone;
        let seam_m = boards.width * 0.5 - bx.abs();
        let fin = (-(seam_m / 0.0012).powi(2)).exp() * ss(-0.2, 0.2, fin_break.at(uv));
        let sand = c.noise(uv, Vec2::splat(texel.x * 2.0), seed + 2);
        let pore = pores.at(uv);
        let pa = texel.x / pore.r;
        let pore_in = ss(1.0 + pa, 1.0 - pa, pore.d);
        let wr = wear.at(uv);
        let a = agg.at(uv, Vec2::ZERO, false);
        let agg_light = wr * ss(0.5, 0.3, a.f1) * (0.4 + 0.6 * unit(a.id));
        let mut col = base * (1.0 + 0.07 * board.tone + 0.12 * mottle.at(uv) + 0.05 * sand);
        col *= 1.0 - 0.07 * lw * imprint;
        col = col.lerp(hex(0xc8c4bc), agg_light * 0.6);
        col *= 1.0 - 0.3 * drips.at(uv);
        col = col.lerp(hex(0x3a3d30), 0.5 * algae.at(uv));
        col = col.lerp(hex(0xd0cdc5), 0.45 * efflo.at(uv));
        col *= 1.0 - 0.15 * fin;
        col *= 1.0 - 0.25 * pore_in;
        let mut h = offset + 0.3 * imprint * lw + 0.8 * fin + 0.05 * sand + 0.15 * agg_light
            - 1.8 * pore_in * (1.0 - pore.d * pore.d).max(0.0);
        let mut ao = 1.0 - 0.5 * pore_in;
        // Tie hole.
        let d = wrap(uv - tie) * c.tile;
        let r = d.length() / 0.013;
        if r < 1.6 {
            let plug = ss(1.02, 0.95, r);
            h = mix(h, h - 3.0 + 1.5 * r * r, plug);
            col = col.lerp(hex(0x8f8c86) * (0.95 + 0.05 * sand), plug);
            col *= 1.0 - 0.35 * (-((r - 1.0) / 0.08).powi(2)).exp();
            ao *= 1.0 - 0.3 * plug;
        }
        if d.x.abs() < 0.012 && d.y > 0.0 && d.y < 0.35 {
            let run = (1.0 - d.x.abs() / 0.012) * ss(0.35, 0.02, d.y) * (0.6 + 0.4 * sand);
            col = col.lerp(hex(0x7a5236) * 0.8, run.max(0.0) * 0.5);
        }
        let rough = 0.9 - 0.05 * agg_light + 0.03 * mottle.at(uv) - 0.06 * algae.at(uv);
        Px {
            h,
            c: col,
            r: rough,
            ao,
            ..Px::default()
        }
    })
}

fn galvanized(c: &Canvas, seed: u32) -> Buf<Px> {
    let crystals = c.sites(Vec2::splat(0.014), seed + 2, 1.0);
    let wx = c.grid(4, |uv| c.noise(uv, Vec2::splat(0.03), seed));
    let wy = c.grid(4, |uv| c.noise(uv, Vec2::splat(0.03), seed + 1));
    let dull = c.mask(4, Vec2::splat(0.06), seed + 7, 0.4, 0.2);
    let white = c.mask(4, Vec2::splat(0.012), seed + 8, 0.07, 0.08);
    let streaks = c.mask(4, Vec2::new(0.03, 0.3), seed + 9, 0.2, 0.15);
    c.fill(|_, _, uv| {
        let warp = Vec2::new(wx.at(uv), wy.at(uv)) * 0.4;
        let cell = crystals.at(uv, warp, true);
        let id = cell.id;
        let tilt = Vec2::new(unit(rehash(id, 1)) - 0.5, unit(rehash(id, 2)) - 0.5) * 0.08;
        let bright = 0.6 + 0.08 * unit(rehash(id, 3));
        let cell_rough = 0.3 + 0.15 * unit(rehash(id, 4));
        let dir = Vec2::from_angle(unit(rehash(id, 5)) * TAU);
        let feather = gnoise(
            Vec2::new(cell.rel.dot(dir) * 16.0, dir.perp_dot(cell.rel) * 2.0) + Vec2::splat(64.0),
            IVec2::splat(4096),
            seed + 6,
        );
        let boundary = ss(0.035, 0.0, cell.edge);
        let ox = 0.35 + 0.5 * dull.at(uv);
        let wh = white.at(uv);
        let dark = streaks.at(uv);
        let metal_col = Vec3::splat(bright * (1.0 + 0.08 * feather)) * Vec3::new(0.98, 1.0, 1.02);
        let ox_col = hex(0x9d9f9e).lerp(hex(0xcfcec8), wh);
        let mut col = metal_col.lerp(ox_col, ox * 0.6 + 0.4 * wh);
        col *= 1.0 - 0.1 * boundary - 0.25 * dark;
        let metal = 1.0 - 0.55 * ox - 0.4 * wh;
        let rough = mix(cell_rough + 0.05 * feather, 0.7, ox * 0.7 + 0.3 * wh) + 0.06 * dark;
        let h = -0.01 * boundary + 0.004 * feather + 0.04 * wh;
        Px {
            h,
            c: col,
            r: rough,
            m: metal,
            tilt: tilt * (1.0 - 0.6 * ox),
            ..Px::default()
        }
    })
}

fn plastic(c: &Canvas, seed: u32) -> Buf<Px> {
    let scratches = StrokeField::new(
        c,
        &Strokes {
            cell: Vec2::splat(0.04),
            per_cell: 2,
            len: (0.01, 0.06),
            width: (0.0003, 0.0009),
            angle: 0.3,
            spread: 1.2,
            seed: seed + 1,
        },
        |_| 0.4,
    );
    let base = hex(0xdcdddf);
    let flow = c.grid(4, |uv| c.fbm(uv, Vec2::new(0.2, 0.05), seed + 2, 3));
    let grime = c.mask(4, Vec2::splat(0.03), seed + 3, 0.12, 0.12);
    let chalk = c.grid(4, |uv| c.fbm(uv, Vec2::splat(0.15), seed + 4, 3));
    c.fill(|_, _, uv| {
        let grain = c.fbm(uv, Vec2::splat(0.0015), seed, 2);
        let s = scratches.at(uv);
        let gr = grime.at(uv);
        let mut col = base * (1.0 + 0.03 * flow.at(uv)) * (1.0 - 0.15 * gr);
        col = col.lerp(Vec3::splat(0.78), 0.4 * s.cover);
        let h = 0.006 * grain + 0.003 * flow.at(uv) - 0.01 * s.cover;
        let rough = 0.37 + 0.1 * ss(-0.2, 0.5, chalk.at(uv)) + 0.1 * gr + 0.2 * s.cover;
        Px {
            h,
            c: col,
            r: rough,
            ..Px::default()
        }
    })
}

fn painted_metal(c: &Canvas, seed: u32) -> Buf<Px> {
    let scratches = StrokeField::new(
        c,
        &Strokes {
            cell: Vec2::splat(0.05),
            per_cell: 1,
            len: (0.01, 0.05),
            width: (0.0003, 0.0007),
            angle: 0.0,
            spread: PI,
            seed: seed + 1,
        },
        |_| 0.3,
    );
    let chips = SpotField::new(c, Vec2::splat(0.03), seed + 3, (0.0007, 0.0025), |_| 0.12);
    let base = hex(0xc6c7ca);
    let bare = hex(0xb4b6ba);
    let grime = c.mask(4, Vec2::splat(0.04), seed + 5, 0.2, 0.15);
    c.fill(|x, y, uv| {
        let peel = c.fbm(uv, Vec2::splat(0.003), seed, 2);
        let fh = hash(x as i32, y as i32, seed + 2);
        let flake = unit(fh) > 0.8;
        let chip_d = chips.at(uv).d + 0.25 * c.noise(uv, Vec2::splat(0.001), seed + 4);
        let chipped = ss(1.0, 0.85, chip_d);
        let s = scratches.at(uv);
        let bare_amt = chipped.max(s.cover * 0.8);
        let gr = grime.at(uv);
        let mut col = base * (1.0 - 0.12 * gr);
        let mut metal = 0.1;
        let mut tilt = Vec2::ZERO;
        if flake {
            col *= 1.12;
            metal = 0.5;
            tilt = Vec2::new(unit(rehash(fh, 1)) - 0.5, unit(rehash(fh, 2)) - 0.5) * 0.2;
        }
        col = col.lerp(bare, bare_amt);
        metal += (1.0 - metal) * bare_amt;
        let rough = mix(0.32 + 0.03 * peel + 0.08 * gr, 0.38, bare_amt);
        let h = 0.006 * peel - 0.06 * chipped - 0.008 * s.cover;
        Px {
            h,
            c: col,
            r: rough,
            m: metal,
            tilt: tilt * (1.0 - bare_amt),
            ..Px::default()
        }
    })
}

fn rubber(c: &Canvas, seed: u32) -> Buf<Px> {
    let scuffs = StrokeField::new(
        c,
        &Strokes {
            cell: Vec2::splat(0.05),
            per_cell: 2,
            len: (0.02, 0.08),
            width: (0.001, 0.004),
            angle: 0.0,
            spread: 0.4,
            seed: seed + 1,
        },
        |_| 0.4,
    );
    let bloom = c.mask(4, Vec2::splat(0.04), seed + 2, 0.25, 0.2);
    c.fill(|_, _, uv| {
        let grain = c.fbm(uv, Vec2::splat(0.002), seed, 2);
        let b = bloom.at(uv);
        let s = scuffs.at(uv);
        let mut col = Vec3::splat(0.032) * (1.0 + 0.1 * grain);
        col = col.lerp(Vec3::splat(0.065), 0.5 * b);
        col = col.lerp(Vec3::splat(0.055), 0.6 * s.cover);
        let h = 0.012 * grain - 0.015 * s.cover;
        let rough = 0.84 + 0.06 * b - 0.2 * s.cover;
        Px {
            h,
            c: col,
            r: rough,
            ..Px::default()
        }
    })
}

fn rope(c: &Canvas, seed: u32) -> Buf<Px> {
    let base = hex(0xc9bfa9);
    let texel = c.texel();
    let dirt = c.grid(2, |uv| c.fbm(uv, Vec2::new(0.06, 0.07), seed + 2, 3));
    c.fill(|_, _, uv| {
        // Strands: lines of constant 3u + 3v (three strands around, one lay length per tile).
        let s = 3.0 * uv.x + 3.0 * uv.y;
        let k = s.ffloor();
        let t = s - k;
        let strand = (k as i32).rem_euclid(3) as u32;
        let across = 2.0 * t - 1.0;
        let bulge = (1.0 - across * across).max(0.0).sqrt();
        // Yarns lie roughly along the rope on each strand's surface.
        let ys = uv.x * 21.0 + 0.5 * t;
        let yt = ys - ys.ffloor();
        let yarn = (1.0 - (2.0 * yt - 1.0).powi(2)).max(0.0).sqrt();
        let fiber = c.noise(uv, Vec2::new(texel.x * 2.5, texel.y * 6.0), seed);
        let fuzz = c.noise(uv, Vec2::splat(texel.x * 2.0), seed + 1);
        let tone = 0.95 + 0.05 * strand as f32;
        let mut col = base * tone * (1.0 + 0.1 * fiber + 0.06 * fuzz);
        col *= 0.75 + 0.25 * yarn;
        col *= 1.0 - 0.25 * ss(0.0, 0.5, dirt.at(uv));
        let h = 2.6 * bulge + 0.3 * yarn * bulge + 0.06 * fiber + 0.04 * fuzz;
        let rough = 0.86 + 0.06 * fuzz;
        Px {
            h,
            c: col,
            r: rough,
            ao: 0.45 + 0.55 * bulge.sqrt(),
            ..Px::default()
        }
    })
}

fn glass(c: &Canvas, seed: u32) -> Buf<Px> {
    let smudge = c.mask(4, Vec2::new(0.03, 0.015), seed, 0.15, 0.12);
    let dust = c.mask(4, Vec2::splat(0.1), seed + 1, 0.15, 0.2);
    let drops = SpotField::new(c, Vec2::splat(0.03), seed + 2, (0.002, 0.008), |_| 0.3);
    let wave = c.grid(8, |uv| c.fbm(uv, Vec2::splat(0.25), seed + 3, 2));
    c.fill(|_, _, uv| {
        let sm = smudge.at(uv);
        let du = dust.at(uv);
        let ring = (-((drops.at(uv).d - 1.0) / 0.12).powi(2)).exp();
        let film = ring * 0.8 + du * 0.5;
        let col = Vec3::new(0.06, 0.075, 0.075).lerp(Vec3::splat(0.35), film * 0.5);
        let a = 0.24 + 0.2 * film;
        let rough = 0.04 + 0.16 * sm + 0.3 * ring + 0.2 * du;
        Px {
            h: 0.05 * wave.smooth(uv),
            c: col,
            a,
            r: rough,
            ..Px::default()
        }
    })
}

fn cork(c: &Canvas, seed: u32) -> Buf<Px> {
    let tones = [
        hex(0xa57a52),
        hex(0x94693f),
        hex(0x7d5532),
        hex(0xb08660),
        hex(0x8a5e3a),
        hex(0x5b3a22),
    ];
    let texel = c.texel();
    let granules = c.sites(Vec2::splat(0.0045), seed + 2, 1.0);
    let fines = c.sites(Vec2::splat(0.002), seed + 3, 1.0);
    let wx = c.grid(2, |uv| c.noise(uv, Vec2::splat(0.007), seed));
    let wy = c.grid(2, |uv| c.noise(uv, Vec2::splat(0.007), seed + 1));
    let voids = SpotField::new(c, Vec2::splat(0.004), seed + 4, (0.0003, 0.0009), |_| 0.45);
    c.fill(|_, _, uv| {
        let warp = Vec2::new(wx.at(uv), wy.at(uv)) * 0.6;
        let g = granules.at(uv, warp, true);
        let f = fines.at(uv, warp * 2.2, false);
        // Small crumbs fill in between some granules.
        let small = g.f1 > 0.55 && unit(rehash(f.id, 9)) < 0.5;
        let (id, f1) = if small { (f.id, f.f1) } else { (g.id, g.f1) };
        let pick = (unit(rehash(id, 1)) * tones.len() as f32) as usize;
        let mut col = tones[pick.min(tones.len() - 1)] * (0.9 + 0.2 * unit(rehash(id, 2)));
        let cells = c.noise(uv, Vec2::splat(texel.x * 2.0), seed + 5);
        col *= 1.0 + 0.15 * cells;
        let seam = ss(0.06, 0.0, g.edge) * if small { 0.3 } else { 1.0 };
        let gap = seam * ss(-0.1, 0.25, c.noise(uv, Vec2::splat(0.003), seed + 6));
        let v = voids.at(uv);
        let void_in = ss(1.0, 0.7, v.d);
        col *= 1.0 - 0.35 * seam;
        col = col.lerp(hex(0x24170e), (gap * 0.7).max(void_in * 0.85));
        let tilt = Vec2::new(unit(rehash(id, 3)) - 0.5, unit(rehash(id, 4)) - 0.5);
        let rel = if small { f.rel } else { g.rel };
        let h = 0.3 * (1.0 - f1 * f1).max(0.0)
            + 0.25 * tilt.dot(rel)
            + 0.2 * unit(rehash(id, 5))
            + 0.05 * cells
            - 0.6 * gap
            - 0.5 * void_in;
        Px {
            h,
            c: col,
            r: 0.84 + 0.06 * gap,
            ao: 1.0 - 0.4 * gap.max(void_in),
            ..Px::default()
        }
    })
}

fn pine(c: &Canvas, seed: u32) -> Buf<Px> {
    let mut rng = Rng::new(seed as u64);
    let ring = 0.0055;
    let mut boards = Boards::new(&mut rng, 1, c.tile.x, ring);
    boards.list[0].pith = rng.range(-0.05, 0.05);
    boards.list[0].depth = rng.range(0.02, 0.035);
    boards.list[0].amp = 0.012;
    let knots = [
        Knot {
            at: Vec2::new(0.3, 0.4),
            radius: 0.012,
            stretch: 1.3,
            swirl: 2.5 * ring,
        },
        Knot::random(&mut rng, (0.003, 0.005), ring),
    ];
    let wobble = c.grid(4, |uv| {
        0.0015 * c.fbm(uv, Vec2::new(0.04, 0.25), seed + 20, 3)
    });
    let phase = c.field(|uv| {
        let (b, x) = boards.locate(uv.x);
        let extra = wobble.smooth(uv) + knots.iter().map(|k| k.push(c, uv)).sum::<f32>();
        boards.phase(b, x, uv.y, extra)
    });
    let prof = Profile::new(|t| ss(0.5, 0.93, t).powf(1.4));
    let texel = c.texel();
    let early = hex(0xdcb987);
    let late = hex(0xb57a3f);
    let knot_col = hex(0x7a3e1a);
    let tone = c.grid(4, |uv| c.fbm(uv, Vec2::new(0.15, 0.4), seed + 2, 3));
    c.fill(|x, y, uv| {
        let i = y * c.w + x;
        let lw = prof.filtered(phase[i], ring_width(&phase, c.w, c.h, x, y));
        let fiber = c.noise(uv, Vec2::new(texel.x * 2.5, 0.02), seed + 1);
        let mut col = early.lerp(late, lw) * (1.0 + 0.06 * fiber + 0.08 * tone.at(uv));
        let mut h = 0.03 * lw + 0.01 * fiber;
        let mut rough = 0.6 - 0.08 * lw;
        for k in &knots {
            let q = k.local(c, uv);
            let r = q.length() / k.radius;
            if r < 1.2 {
                let inside = 1.0 - ss(0.88, 1.0, r);
                let rings = (q.length() * 1000.0 / 1.4 + 0.5 * fiber).ffract();
                let kc = knot_col * (0.7 + 0.4 * ss(0.1, 0.9, rings)) * (0.75 + 0.25 * r);
                col = col.lerp(kc, inside);
                col *= 1.0 - 0.35 * (-((r - 0.97) / 0.06).powi(2)).exp();
                h += 0.02 * inside;
                rough += (0.48 - rough) * inside;
            }
        }
        Px {
            h,
            c: col,
            r: rough,
            ..Px::default()
        }
    })
}

fn ice(c: &Canvas, seed: u32) -> Buf<Px> {
    let deep = hex(0x7d9fb4);
    let cloudy = hex(0xd2dee4);
    let texel = c.texel();
    let cloud = c.mask(4, Vec2::splat(0.06), seed, 0.35, 0.3);
    let column = c.mask(4, Vec2::new(0.006, 0.05), seed + 1, 0.2, 0.15);
    let bubbles = SpotField::new(c, Vec2::splat(0.006), seed + 2, (0.0004, 0.0016), |uv| {
        0.15 + 0.5 * cloud.at(uv)
    });
    let cracks = c.sites(Vec2::splat(0.09), seed + 3, 0.9);
    let cw = c.grid(4, |uv| c.fbm(uv, Vec2::splat(0.04), seed + 6, 2));
    let cmask = c.mask(4, Vec2::splat(0.12), seed + 4, 0.5, 0.2);
    let frost = c.mask(4, Vec2::splat(0.04), seed + 5, 0.12, 0.15);
    let ripple = c.grid(2, |uv| c.fbm(uv, Vec2::splat(0.025), seed + 7, 3));
    c.fill(|_, _, uv| {
        let cl = cloud.at(uv);
        let b = bubbles.at(uv);
        let aa = texel.x / b.r;
        let b_in = ss(1.0 + aa, 1.0 - aa, b.d);
        let b_rim = (-((b.d - 0.85) / 0.2).powi(2)).exp();
        let cr = cracks.at(uv, Vec2::splat(cw.at(uv) * 0.4), true);
        let cm = cmask.at(uv);
        let crack = ss(0.012, 0.0, cr.edge) * cm;
        let halo = ss(0.06, 0.0, cr.edge) * cm;
        let fr = frost.at(uv);
        let mut col = deep.lerp(cloudy, cl * 0.55 + column.at(uv) * 0.3);
        col = col.lerp(cloudy, halo * 0.35);
        col = col.lerp(Vec3::splat(0.8), crack * 0.8);
        col = col.lerp(Vec3::splat(0.82), b_in * 0.85);
        col *= 1.0 - 0.25 * b_rim * (1.0 - b_in);
        col = col.lerp(Vec3::splat(0.75), fr * 0.5);
        let rp = ripple.smooth(uv);
        let h = 0.05 * rp - 0.05 * crack + 0.02 * fr;
        let rough = 0.07 + 0.03 * rp.abs() + 0.3 * fr;
        Px {
            h,
            c: col,
            r: rough,
            ..Px::default()
        }
    })
}

fn block_concrete(c: &Canvas, seed: u32) -> Buf<Px> {
    let base = hex(0xaaa9a4);
    let texel = c.texel();
    let mottle = c.grid(4, |uv| c.fbm(uv, Vec2::splat(0.1), seed, 5));
    let oil = c.mask(4, Vec2::new(0.15, 0.06), seed + 2, 0.15, 0.15);
    let pores = SpotField::new(c, Vec2::splat(0.012), seed + 3, (0.0005, 0.002), |_| 0.18);
    c.fill(|_, _, uv| {
        let m = mottle.at(uv);
        let sand = c.noise(uv, Vec2::splat(texel.x * 2.0), seed + 1);
        let o = oil.at(uv);
        let pore = pores.at(uv);
        let pc = ss(1.0 + texel.x / pore.r, 1.0 - texel.x / pore.r, pore.d);
        let mut col = base * (1.0 + 0.08 * m + 0.05 * sand) * (1.0 - 0.1 * o);
        col *= 1.0 - 0.3 * pc;
        let h = 0.04 * sand + 0.08 * m - 1.2 * pc * (1.0 - pore.d * pore.d).max(0.0);
        Px {
            h,
            c: col,
            r: 0.84 + 0.04 * m - 0.1 * o,
            ao: 1.0 - 0.5 * pc,
            ..Px::default()
        }
    })
}

fn skin(c: &Canvas, seed: u32) -> Buf<Px> {
    let base = hex(0xc4a08e);
    let flush = hex(0xc8907f);
    let texel = c.texel();
    let (a, b) = ((c.tile.x / 0.0028).fround(), (c.tile.y / 0.0034).fround());
    let mottle = c.grid(2, |uv| c.fbm_g(uv, Vec2::splat(0.02), seed, 4, 0.6));
    let warp = c.grid(2, |uv| c.fbm(uv, Vec2::splat(0.01), seed + 2, 2));
    let oil = c.grid(4, |uv| c.fbm(uv, Vec2::splat(0.05), seed + 5, 3));
    let freckles = SpotField::new(c, Vec2::splat(0.008), seed + 1, (0.0005, 0.0013), |_| 0.15);
    c.fill(|_, _, uv| {
        let mut col = base.lerp(flush, ss(0.0, 0.6, mottle.at(uv)) * 0.3);
        col *= Vec3::ONE.lerp(Vec3::new(0.88, 0.83, 0.8), ss(1.1, 0.4, freckles.at(uv).d));
        let wp = 0.4 * warp.at(uv);
        let l1 = (TAU * (uv.x * a + uv.y * b + wp)).sin();
        let l2 = (TAU * (uv.x * a - uv.y * b - wp)).sin();
        let lines = ss(0.7, 1.0, l1) + ss(0.7, 1.0, l2);
        let pore = ss(0.35, 0.6, c.noise(uv, Vec2::splat(texel.x * 2.0), seed + 3));
        let soft = c.fbm(uv, Vec2::splat(0.01), seed + 4, 3);
        let h = -0.012 * lines - 0.01 * pore + 0.03 * soft;
        let rough = 0.5 + 0.1 * oil.at(uv) + 0.05 * pore;
        Px {
            h,
            c: col * (1.0 - 0.03 * pore),
            r: rough,
            ..Px::default()
        }
    })
}

fn fabric(c: &Canvas, seed: u32) -> Buf<Px> {
    let wales = (c.tile.x / 0.0011).fround();
    let courses = (c.tile.y / 0.0009).fround();
    let base = hex(0xd8d8d6);
    let texel = c.texel();
    c.fill(|_, _, uv| {
        let p = Vec2::new(uv.x * wales, uv.y * courses);
        let fx = p.x - p.x.ffloor();
        let mut best = 0.0f32;
        let mut along_best = 0.0;
        let mut row_id = 0;
        for dy in -1..=1 {
            let row = p.y.ffloor() + dy as f32;
            let fy = p.y - row;
            for (cx, slant) in [(0.29f32, 0.38f32), (0.71, -0.38)] {
                let d = Vec2::new(fx - cx, fy - 0.55);
                let axis = Vec2::new(slant, 1.0).normalize();
                let along = d.dot(axis);
                let across = axis.perp_dot(d);
                let e = (along / 0.6).powi(2) + (across / 0.21).powi(2);
                if e < 1.0 {
                    let hh = (1.0 - e).sqrt();
                    if hh > best {
                        best = hh;
                        along_best = along;
                        row_id = row as i32;
                    }
                }
            }
        }
        let twist = (TAU * (along_best * 5.0 + fx * 2.0)).sin();
        let heather = unit(hash(
            row_id.rem_euclid(courses as i32),
            p.x.ffloor() as i32,
            seed,
        ));
        let fuzz = c.noise(uv, Vec2::splat(texel.x * 2.0), seed + 1);
        let mut col = base * (0.96 + 0.08 * heather) * (1.0 + 0.04 * fuzz);
        col *= 0.65 + 0.35 * best.sqrt();
        let h = 0.25 * best.sqrt() + 0.02 * twist * best + 0.01 * fuzz;
        Px {
            h,
            c: col,
            r: 0.72 - 0.12 * best + 0.03 * fuzz,
            ao: 0.6 + 0.4 * best.sqrt(),
            ..Px::default()
        }
    })
}

fn ground(c: &Canvas, seed: u32) -> Buf<Px> {
    let tufts = c.mask(8, Vec2::splat(0.25), seed + 6, 0.45, 0.25);
    // Grass clumps: blades lean out from each clump's center; sparse loose blades between.
    let clumps = SpotField::new(c, Vec2::splat(0.15), seed + 9, (0.05, 0.1), |uv| {
        0.3 + 0.6 * tufts.at(uv)
    });
    let blades = StrokeField::with_dir(
        c,
        &Strokes {
            cell: Vec2::splat(0.035),
            per_cell: 5,
            len: (0.03, 0.07),
            width: (0.002, 0.0038),
            angle: 0.0,
            spread: 0.8,
            seed: seed + 1,
        },
        |uv| {
            let sp = clumps.at(uv);
            let inside = ss(1.4, 0.5, sp.d);
            // Out of a clump: any direction (the cell's own).
            let loose = unit(hash((uv.x * 4096.0) as i32, (uv.y * 4096.0) as i32, seed)) * TAU;
            let dir = if inside > 0.0 && sp.rel.length_squared() > 1e-8 {
                sp.rel.to_angle()
            } else {
                loose
            };
            (0.05 + 0.9 * inside, Some(dir))
        },
    );
    let loose = StrokeField::new(
        c,
        &Strokes {
            cell: Vec2::splat(0.04),
            per_cell: 3,
            len: (0.03, 0.08),
            width: (0.0015, 0.003),
            angle: 0.0,
            spread: PI,
            seed: seed + 10,
        },
        |uv| 0.3 + 0.3 * tufts.at(uv),
    );
    let pebbles = SpotField::new(c, Vec2::splat(0.035), seed + 5, (0.003, 0.009), |uv| {
        0.35 * (1.0 - tufts.at(uv))
    });
    let clods = c.sites(Vec2::splat(0.03), seed + 3, 0.9);
    let crumbs = c.sites(Vec2::splat(0.007), seed + 4, 0.9);
    let hollow = c.grid(4, |uv| c.fbm(uv, Vec2::splat(0.3), seed + 2, 4));
    let damp = c.mask(4, Vec2::splat(0.15), seed + 7, 0.3, 0.2);
    let pebble_shape = c.grid(2, |uv| c.noise(uv, Vec2::splat(0.006), seed + 8));
    let texel = c.texel().x;
    c.fill(|_, _, uv| {
        let hl = hollow.smooth(uv);
        let cl = clods.at(uv, Vec2::ZERO, true);
        let clod_h = (1.0 - cl.f1 * cl.f1).max(0.0) * (0.5 + unit(rehash(cl.id, 1)));
        let cr = crumbs.at(uv, Vec2::ZERO, false);
        let crumb_h = (1.0 - cr.f1 * cr.f1).max(0.0) * unit(rehash(cr.id, 1));
        let crack = ss(0.06, 0.0, cl.edge);
        let pb = pebbles.at(uv);
        let pd = pb.d + 0.25 * pebble_shape.at(uv);
        let pa = texel / pb.r;
        let p_in = ss(1.0 + pa, 1.0 - pa, pd);
        let p_dome = (1.0 - pd * pd).max(0.0).sqrt();
        let clump = clumps.at(uv);
        // Soil under a clump is shaded and covered in thatch.
        let under = ss(1.1, 0.4, clump.d);
        let mut h = 14.0 * hl + 1.6 * clod_h + 0.6 * crumb_h - 0.4 * crack
            + 3.0 * p_dome * p_in
            + 1.2 * under;
        let dm = damp.at(uv);
        let mut col = Vec3::splat(0.86 * (1.0 + 0.05 * crumb_h + 0.04 * clod_h));
        col *= 1.0 - 0.12 * ss(0.0, -0.45, hl) - 0.1 * dm - 0.05 * crack - 0.1 * under;
        let p_tone = 0.92 + 0.12 * unit(rehash(pb.id, 4));
        col = col.lerp(Vec3::new(1.0, 0.99, 0.97) * p_tone, p_in);
        let mut rough = 0.93 - 0.14 * p_in - 0.05 * dm;
        let mut ao = 1.0 - 0.25 * under;
        for (s, lift) in [(loose.at(uv), 0.35), (blades.at(uv), 0.8)] {
            if s.cover <= 0.0 {
                continue;
            }
            let prof = (1.0 - s.across * s.across).max(0.0).sqrt();
            let dry = unit(rehash(s.id, 9));
            let tint = if dry > 0.75 {
                Vec3::new(1.0, 0.975, 0.9)
            } else {
                Vec3::splat(0.8 + 0.2 * dry)
            };
            h = mix(h, h.max(2.0 * lift * under + lift) + 0.6 * prof, s.cover);
            col = col.lerp(tint * (0.92 + 0.08 * prof), s.cover);
            rough = mix(rough, 0.86, s.cover);
            ao = mix(ao, 1.0, 0.6 * s.cover);
        }
        Px {
            h,
            c: col,
            r: rough,
            ao,
            ..Px::default()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(images: &Assets<Image>, h: Option<Handle<Image>>) -> &Image {
        images.get(h.expect("texture")).expect("image")
    }

    #[test]
    fn every_surface_is_textured() {
        let mut images = Assets::<Image>::default();
        let mats = DemoMaterials::build_with_size(&mut images, 64);
        for s in Surface::ALL {
            let m = mats.get(s);
            for t in [
                m.base_color_texture,
                m.normal_texture,
                m.metallic_roughness_texture,
                m.occlusion_texture,
            ] {
                let img = bytes(&images, t);
                assert!(img.is_valid(), "{s:?}");
                assert_eq!(img.mips.len() + 1, img.full_mip_count() as usize, "{s:?}");
            }
            assert!(bytes(&images, m.base_color_texture).srgb, "{s:?}");
            assert!(!bytes(&images, m.normal_texture).srgb, "{s:?}");
            let tile = tile_size(s);
            assert!((m.uv_scale[0] * tile[0] - 1.0).abs() < 1e-5, "{s:?}");
            assert_eq!(m.normal_texture, mats.get(s.source()).normal_texture);
        }
    }

    /// Mean absolute difference between two rows/columns of an image.
    fn line_diff(img: &Image, a: u32, b: u32, vertical: bool) -> f32 {
        let n = if vertical { img.height } else { img.width };
        let mut sum = 0.0;
        for k in 0..n {
            let (pa, pb) = if vertical {
                (img.pixel(a, k), img.pixel(b, k))
            } else {
                (img.pixel(k, a), img.pixel(k, b))
            };
            for c in 0..3 {
                sum += (pa[c] as f32 - pb[c] as f32).abs();
            }
        }
        sum / (n as f32 * 3.0)
    }

    /// Median absolute difference between two rows/columns of an image.
    fn line_median(img: &Image, a: u32, b: u32, vertical: bool) -> f32 {
        let n = if vertical { img.height } else { img.width };
        let mut d: Vec<u8> = (0..n)
            .flat_map(|k| {
                let (pa, pb) = if vertical {
                    (img.pixel(a, k), img.pixel(b, k))
                } else {
                    (img.pixel(k, a), img.pixel(k, b))
                };
                (0..3).map(move |c| pa[c].abs_diff(pb[c]))
            })
            .collect();
        d.sort_unstable();
        d[d.len() / 2] as f32
    }

    #[test]
    fn textures_tile_seamlessly() {
        let mut images = Assets::<Image>::default();
        let mats = DemoMaterials::build_with_size(&mut images, 128);
        for s in Surface::ALL {
            let m = mats.get(s);
            for t in [m.base_color_texture, m.normal_texture, m.occlusion_texture] {
                let img = bytes(&images, t);
                for vertical in [true, false] {
                    let n = if vertical { img.width } else { img.height };
                    let seam = line_diff(img, n - 1, 0, vertical);
                    let inner: f32 = (0..n - 1)
                        .map(|k| line_diff(img, k, k + 1, vertical))
                        .sum::<f32>()
                        / (n - 1) as f32;
                    // A small feature on the seam (a nail at v = 0) raises the mean there but
                    // not the median; a broken seam raises both.
                    let seam_median = line_median(img, n - 1, 0, vertical);
                    let inner_median: f32 = (0..n - 1)
                        .map(|k| line_median(img, k, k + 1, vertical))
                        .sum::<f32>()
                        / (n - 1) as f32;
                    assert!(
                        seam <= inner * 1.6 + 1.5 || seam_median <= inner_median * 1.6 + 1.0,
                        "{s:?}: seam {seam:.2} vs neighbours {inner:.2} (vertical {vertical})"
                    );
                }
            }
        }
    }

    #[test]
    fn noise_is_periodic() {
        let per = IVec2::new(5, 3);
        for i in 0..50 {
            let p = Vec2::new(i as f32 * 0.37, i as f32 * 0.21);
            let a = fbm(p, per, 9, 4, 0.5);
            let b = fbm(p + Vec2::new(5.0, 3.0), per, 9, 4, 0.5);
            assert!((a - b).abs() < 1e-4, "{a} {b}");
            let sites = Sites::new(per, 3, 0.9);
            let ca = sites.nearest(p, true);
            let cb = sites.nearest(p + Vec2::new(10.0, -6.0), true);
            assert_eq!(ca.id, cb.id);
            assert!((ca.edge - cb.edge).abs() < 1e-3);
        }
    }

    #[test]
    fn normals_decode_to_unit_length() {
        let mut images = Assets::<Image>::default();
        let mats = DemoMaterials::build_with_size(&mut images, 64);
        for s in Surface::ALL {
            let img = bytes(&images, mats.get(s).normal_texture);
            let levels = std::iter::once(&img.data).chain(img.mips.iter());
            for data in levels {
                for px in data.chunks(4) {
                    let n = Vec3::new(px[0] as f32, px[1] as f32, px[2] as f32) / 127.5 - 1.0;
                    assert!((n.length() - 1.0).abs() < 0.02, "{s:?}: {n}");
                    assert!(n.z > 0.0, "{s:?}: {n}");
                }
            }
        }
    }

    #[test]
    fn deterministic() {
        let mut a = Assets::<Image>::default();
        let mut b = Assets::<Image>::default();
        let ma = DemoMaterials::build_with_size(&mut a, 64);
        let mb = DemoMaterials::build_with_size(&mut b, 64);
        for s in Surface::ALL {
            let (x, y) = (ma.get(s), mb.get(s));
            for (tx, ty) in x.textures().iter().zip(y.textures().iter()) {
                if let (Some(tx), Some(ty)) = (tx, ty) {
                    let (ix, iy) = (a.get(*tx).unwrap(), b.get(*ty).unwrap());
                    assert!(ix.data == iy.data && ix.mips == iy.mips, "{s:?}");
                }
            }
        }
    }

    #[test]
    fn albedo_is_physically_plausible() {
        let mut images = Assets::<Image>::default();
        let mats = DemoMaterials::build_with_size(&mut images, 64);
        let lut = |v: u8| {
            let s = v as f32 / 255.0;
            if s <= 0.04045 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        for s in Surface::ALL {
            let img = bytes(&images, mats.get(s).base_color_texture);
            let mut sum = 0.0;
            let mut max: f32 = 0.0;
            for px in img.data.chunks(4) {
                let l = lut(px[0]).max(lut(px[1])).max(lut(px[2]));
                sum += l;
                max = max.max(l);
            }
            let mean = sum / (img.data.len() / 4) as f32;
            let limit = if s == Surface::Ground { 1.0 } else { 0.9 };
            assert!(max <= limit, "{s:?}: brightest texel {max}");
            assert!(mean > 0.02, "{s:?}: mean {mean}");
        }
    }

    #[test]
    fn plain_colors_are_distinct_and_sensible() {
        for (i, a) in Surface::ALL.iter().enumerate() {
            let ma = plain(*a);
            assert!(!ma.has_textures());
            let ca = Vec3::from_slice(&ma.base_color[..3]);
            assert!(ca.max_element() <= 0.9 && ca.min_element() >= 0.01, "{a:?}");
            assert!((0.04..=1.0).contains(&ma.perceptual_roughness), "{a:?}");
            for b in &Surface::ALL[i + 1..] {
                let cb = Vec3::from_slice(&plain(*b).base_color[..3]);
                assert!(
                    (ca - cb).abs().max_element() > 0.015,
                    "{a:?} and {b:?} look the same"
                );
            }
        }
    }

    /// Writes flat swatches (albedo, normal, ORM and a CPU-lit view) of every surface to
    /// `$MATERIAL_DUMP_DIR` for looking at the textures without a GPU.
    #[test]
    #[ignore]
    fn dump_swatches() {
        let dir = std::env::var("MATERIAL_DUMP_DIR").unwrap_or_else(|_| "target/materials".into());
        let size: u32 = std::env::var("MATERIAL_DUMP_SIZE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(512);
        let only = std::env::var("MATERIAL_DUMP_ONLY").ok();
        std::fs::create_dir_all(&dir).unwrap();
        for s in Surface::ALL {
            if s.source() != s {
                continue;
            }
            let name = format!("{s:?}");
            if only
                .as_ref()
                .is_some_and(|o| !o.split(',').any(|k| k == name))
            {
                continue;
            }
            let started = std::time::Instant::now();
            let b = synthesize(s, size);
            eprintln!("{name}: {:.0} ms", started.elapsed().as_secs_f32() * 1000.0);
            let (w, h) = (b.albedo.width, b.albedo.height);
            let save = |suffix: &str, data: Vec<u8>| {
                image::RgbaImage::from_raw(w, h, data)
                    .unwrap()
                    .save(format!("{dir}/{name}_{suffix}.png"))
                    .unwrap();
            };
            save("albedo", b.albedo.data.clone());
            save("normal", b.normal.data.clone());
            save("orm", b.orm.data.clone());
            let lin = |v: u8| ((v as f32 / 255.0 + 0.055) / 1.055).powf(2.4);
            let l = Vec3::new(-0.55, 0.45, 0.6).normalize();
            let hv = (l + Vec3::Z).normalize();
            let mut lit = Vec::with_capacity(b.albedo.data.len());
            for i in 0..(w * h) as usize {
                let a = &b.albedo.data[i * 4..i * 4 + 4];
                let n = &b.normal.data[i * 4..i * 4 + 4];
                let o = &b.orm.data[i * 4..i * 4 + 4];
                let alb = Vec3::new(lin(a[0]), lin(a[1]), lin(a[2]));
                let n =
                    (Vec3::new(n[0] as f32, n[1] as f32, n[2] as f32) / 127.5 - 1.0).normalize();
                let (ao, r, m) = (
                    o[0] as f32 / 255.0,
                    o[1] as f32 / 255.0,
                    o[2] as f32 / 255.0,
                );
                let alpha = (r * r).max(0.002);
                let nh = n.dot(hv).max(0.0);
                let nl = n.dot(l).max(0.0);
                let d = alpha * alpha / (PI * (nh * nh * (alpha * alpha - 1.0) + 1.0).powi(2));
                let f0 = Vec3::splat(0.04).lerp(alb, m);
                let diffuse = alb * (1.0 - m);
                let col = (diffuse / PI * 3.0 + f0 * d * 0.25 * 3.0) * nl * 1.6
                    + (diffuse * 0.35 + f0 * 0.3) * ao;
                for ch in 0..3 {
                    let x = col[ch];
                    lit.push((srgb255(srgb_lut(), x / (1.0 + x))).fround() as u8);
                }
                lit.push(255);
            }
            save("lit", lit);
        }
        let mut images = Assets::<Image>::default();
        let started = std::time::Instant::now();
        DemoMaterials::build_with_size(&mut images, size);
        eprintln!(
            "build_with_size({size}): {:.2} s",
            started.elapsed().as_secs_f32()
        );
        // Noise statistics (calibration).
        let per = IVec2::splat(64);
        let (mut s1, mut s2, mut f1, mut f2) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
        let n = 100_000;
        for i in 0..n {
            let p = Vec2::new((i % 317) as f32 * 0.193, (i / 317) as f32 * 0.171);
            let a = gnoise(p, per, 1) as f64;
            let b = fbm(p, per, 1, 5, 0.5) as f64;
            s1 += a;
            s2 += a * a;
            f1 += b;
            f2 += b * b;
        }
        let n = n as f64;
        eprintln!(
            "gnoise std {:.3}, fbm std {:.3}",
            (s2 / n - (s1 / n).powi(2)).sqrt(),
            (f2 / n - (f1 / n).powi(2)).sqrt()
        );
    }
}
