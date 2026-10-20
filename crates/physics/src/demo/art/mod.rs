//! Procedural art for the proving ground: meshes with metric UVs and bevels, and PBR texture
//! sets, built once per app and shared through keyed assets (`physics/…`), so a second proving
//! ground (or a rebuilt one) reuses them.

// The modules carry a few parts and options the proving ground does not use (yet).
#![allow(dead_code)]

pub(crate) mod barrel_run;
pub(crate) mod dummy;
pub(crate) mod kit;
pub(crate) mod props;
pub(crate) mod structures;
pub(crate) mod textures;
pub(crate) mod timber;

use std::collections::HashMap;

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_render::{Image, Mesh, StandardMaterial};
use textures::TextureSet;

/// A mesh and the material it is drawn with.
pub(crate) type Parts = Vec<(Handle<Mesh>, Handle<StandardMaterial>)>;

/// The proving ground's materials.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Mat {
    /// The yard's slab, with its 6 m saw-cut joints.
    Slab,
    /// Cast and precast concrete: curbs, stairs, bins, plinths.
    Concrete,
    /// The curb's black-and-yellow band.
    Hazard,
    /// Safety-yellow painted steel: gantry, handrails, hubs, turntable rim.
    Yellow,
    /// Dark painted steel: crate brackets, seesaw, the paddle bin's pillow blocks.
    Dark,
    /// Bare forged steel: bolts, chain, wrecking ball, axles.
    Steel,
    /// Weathered timber: seesaw, paddles, bridge.
    Timber,
    /// Diamond plate: the turntable's deck.
    Diamond,
    /// Crate boards; the number picks one of two woods.
    CrateWood(u8),
    /// A painted steel drum of this color (sRGB hex).
    Drum(u32),
    /// Rubber of this color (sRGB hex).
    Rubber(u32),
    /// Dominoes: white and glossy, their colors are vertex colors.
    Domino,
    /// The crash test dummies' vinyl; their colors are vertex colors.
    Vinyl,
    /// The barrel run's asphalt.
    Asphalt,
    /// Raked sand: the barrel run's catch pit.
    Sand,
    /// Thermoplastic road paint: the barrel run's lane lines.
    Paint,
    /// Hot-dip galvanized steel: guardrails.
    Galvanized,
    /// Freshly broken granite: the rockfall's rocks.
    Granite,
}

/// Texture sets, generated once per app.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Tex {
    Slab,
    Concrete,
    Hazard,
    Yellow,
    Dark,
    Steel,
    Timber,
    Diamond,
    CrateWood(u8),
    Drum(u32),
    Rubber,
    Vinyl,
    Asphalt,
    Sand,
    Galvanized,
    Granite,
}

/// Generated texture sets' images.
#[derive(Clone, Copy)]
struct SetImages {
    albedo: Handle<Image>,
    normal: Handle<Image>,
    orm: Handle<Image>,
    tile_m: f32,
}

#[derive(Resource, Default)]
struct ArtCache {
    sets: HashMap<Tex, SetImages>,
    materials: HashMap<Mat, Handle<StandardMaterial>>,
}

/// Texture resolution: full size in optimized builds; debug builds (tests) make them a
/// quarter the size, which is 16× faster to generate.
fn res(full: u32) -> u32 {
    if cfg!(debug_assertions) {
        (full / 4).max(64)
    } else {
        full
    }
}

fn paint(hex: u32) -> [f32; 3] {
    kit::srgb(hex).truncate().to_array()
}

impl Tex {
    fn make(self) -> TextureSet {
        match self {
            Tex::Slab => textures::concrete_slab(res(1024)),
            Tex::Concrete => textures::cast_concrete(res(512)),
            Tex::Hazard => textures::hazard_stripes(res(512)),
            Tex::Yellow => textures::painted_steel(res(512), paint(0xd6a21e), 0.08),
            Tex::Dark => textures::painted_steel(res(512), paint(0x3d4247), 0.35),
            Tex::Steel => textures::forged_steel(res(512)),
            Tex::Timber => textures::weathered_timber(res(512)),
            Tex::Diamond => textures::diamond_plate(res(512)),
            Tex::CrateWood(seed) => textures::crate_wood_seeded(res(512), seed as u32),
            Tex::Drum(hex) => textures::painted_steel(res(256), paint(hex), 0.35),
            Tex::Rubber => textures::rubber(res(512)),
            Tex::Vinyl => textures::vinyl_skin(res(512)),
            Tex::Asphalt => textures::asphalt(res(1024)),
            Tex::Sand => textures::raked_sand(res(512)),
            Tex::Galvanized => textures::galvanized(res(512)),
            Tex::Granite => textures::granite(res(512)),
        }
    }
}

fn set_images(world: &mut World, tex: Tex) -> SetImages {
    if let Some(s) = world.get_resource_or_init::<ArtCache>().sets.get(&tex) {
        return *s;
    }
    let set = tex.make();
    let mut images = world.resource_mut::<Assets<Image>>();
    let s = SetImages {
        albedo: images.add(set.albedo),
        normal: images.add(set.normal),
        orm: images.add(set.orm),
        tile_m: set.tile_m,
    };
    world.resource_mut::<ArtCache>().sets.insert(tex, s);
    s
}

/// A material whose texture set drives color, normals, roughness, metalness and occlusion;
/// `tint` (sRGB hex) multiplies the color.
fn textured(world: &mut World, tex: Tex, tint: u32) -> StandardMaterial {
    let s = set_images(world, tex);
    let mut m = StandardMaterial::hex(tint)
        .with_roughness(1.0)
        .with_metallic(1.0)
        .with_base_color_texture(s.albedo)
        .with_normal_texture(s.normal);
    m.metallic_roughness_texture = Some(s.orm);
    m.occlusion_texture = Some(s.orm);
    m.uv_scale = [1.0 / s.tile_m; 2];
    m
}

/// The material for `m`, made (with its textures) on first use.
pub(crate) fn material(world: &mut World, m: Mat) -> Handle<StandardMaterial> {
    if let Some(h) = world.get_resource_or_init::<ArtCache>().materials.get(&m) {
        return *h;
    }
    let white = 0xffffff;
    let made = match m {
        Mat::Slab => textured(world, Tex::Slab, white),
        Mat::Concrete => textured(world, Tex::Concrete, white),
        Mat::Hazard => textured(world, Tex::Hazard, white),
        Mat::Yellow => textured(world, Tex::Yellow, white),
        Mat::Dark => textured(world, Tex::Dark, white),
        Mat::Steel => textured(world, Tex::Steel, white),
        Mat::Timber => textured(world, Tex::Timber, white),
        Mat::Diamond => textured(world, Tex::Diamond, white),
        // The second wood is a little darker and warmer.
        Mat::CrateWood(k) => textured(
            world,
            Tex::CrateWood(k % 2),
            if k % 2 == 0 { white } else { 0xe8d2bc },
        ),
        Mat::Drum(hex) => textured(world, Tex::Drum(hex), white),
        Mat::Rubber(hex) => textured(world, Tex::Rubber, hex),
        Mat::Domino => StandardMaterial::hex(white).with_roughness(0.3),
        Mat::Vinyl => textured(world, Tex::Vinyl, white).with_subsurface(0.1),
        Mat::Asphalt => textured(world, Tex::Asphalt, white),
        Mat::Sand => textured(world, Tex::Sand, white),
        Mat::Paint => StandardMaterial::hex(0xe9e6dc).with_roughness(0.55),
        Mat::Galvanized => textured(world, Tex::Galvanized, white),
        Mat::Granite => textured(world, Tex::Granite, white),
    };
    let h = world.resource_mut::<Assets<StandardMaterial>>().add(made);
    world.resource_mut::<ArtCache>().materials.insert(m, h);
    h
}

/// `N` meshes cached under `key` (`make` builds them on first use), each drawn with its
/// material from `mats`. Empty meshes are left out.
pub(crate) fn parts<const N: usize>(
    world: &mut World,
    key: &str,
    mats: [Mat; N],
    make: impl FnOnce() -> [Mesh; N],
) -> Parts {
    let keys: [String; N] = std::array::from_fn(|i| format!("physics/{key}#{i}"));
    let cached: Option<Vec<_>> = {
        let meshes = world.resource::<Assets<Mesh>>();
        keys.iter().map(|k| meshes.by_key(k)).collect()
    };
    let handles = match cached {
        Some(h) => h,
        None => {
            let built = make();
            let mut meshes = world.resource_mut::<Assets<Mesh>>();
            keys.iter()
                .zip(built)
                .map(|(k, m)| meshes.insert_keyed(k, m))
                .collect()
        }
    };
    let mut out = Parts::new();
    for (h, m) in handles.into_iter().zip(mats) {
        let empty = world
            .resource::<Assets<Mesh>>()
            .get(h)
            .is_none_or(|m| m.triangle_count() == 0);
        if !empty {
            out.push((h, material(world, m)));
        }
    }
    out
}
