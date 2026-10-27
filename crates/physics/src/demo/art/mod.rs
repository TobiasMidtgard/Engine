//! Procedural art for the proving ground: meshes with metric UVs and bevels, and PBR texture
//! sets, built once per app and shared through keyed assets (`physics/…`), so a second proving
//! ground (or a rebuilt one) reuses them.
//!
//! Texture sets and the dummies' meshes are made on worker threads, as engines stream assets:
//! made on the frame, they held the yard's first frame for a second and a half. Until a set
//! arrives its materials are reserved (drawn plain), and a dummy's meshes are reserved (not
//! drawn). [`finish`] stores what the workers made; [`wait`] blocks until they are done.

// The modules carry a few parts and options the proving ground does not use (yet).
#![allow(dead_code)]

pub(crate) mod barrel_run;
pub(crate) mod catapult;
pub(crate) mod dummy;
pub(crate) mod galton;
pub(crate) mod jenga;
pub(crate) mod kit;
pub(crate) mod pendulums;
pub(crate) mod pool;
pub(crate) mod props;
pub(crate) mod site;
pub(crate) mod structures;
pub(crate) mod textures;
pub(crate) mod timber;

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, mpsc};

use parking_lot::Mutex;

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
    /// Polished chrome: the Newton's cradle's balls and frames.
    Chrome,
    /// Glossy lacquer of this color (sRGB hex): the pendulum wave's bobs, the cradle's base.
    Lacquer(u32),
    /// Billiard cloth of this color (sRGB hex).
    Felt(u32),
    /// Polished brass: the Galton board's pins.
    Brass,
    /// Clear glass: the Galton board's front pane.
    Glass,
    /// Sanded pale hardwood: the giant Jenga blocks.
    Oak,
    /// Worn yellow floor paint: the yard's walkway.
    LinePaint,
    /// Laid hemp rope: the bridge's railings; strand tints are vertex colors.
    Rope,
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
    Oak,
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
pub(crate) struct ArtCache {
    sets: HashMap<Tex, SetImages>,
    materials: HashMap<Mat, Handle<StandardMaterial>>,
    /// Texture sets being made on workers, and the materials reserved until they are.
    making: Vec<(Tex, Job<TextureSet>)>,
    waiting: Vec<(Mat, Handle<StandardMaterial>)>,
    /// Meshes being made on workers, for the handles reserved under their keys.
    meshing: Vec<Meshing>,
}

/// Mesh handles reserved under their keys, and the worker making their meshes.
type Meshing = (Vec<Handle<Mesh>>, Job<Vec<Mesh>>);

/// Something a worker makes: `None` until it is done; `Err` if it panicked.
type Slot<T> = Arc<Mutex<Option<std::thread::Result<T>>>>;

struct Job<T>(Slot<T>);

impl<T> Job<T> {
    fn done(&self) -> bool {
        self.0.lock().is_some()
    }

    /// What it made (waiting for it); `None` if it panicked.
    fn take(self) -> Option<T> {
        loop {
            if let Some(r) = self.0.lock().take() {
                return r.ok();
            }
            std::thread::yield_now();
        }
    }
}

type Work = Box<dyn FnOnce() + Send>;

/// The art workers: half the cores (at least one), so the frame and the rest of loading keep
/// theirs. `None` where threads can't start (the web).
fn workers() -> Option<&'static Mutex<mpsc::Sender<Work>>> {
    static POOL: OnceLock<Option<Mutex<mpsc::Sender<Work>>>> = OnceLock::new();
    POOL.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Work>();
        let rx = Arc::new(Mutex::new(rx));
        let n = std::thread::available_parallelism()
            .map_or(2, |n| n.get() / 2)
            .max(1);
        let mut started = 0;
        for i in 0..n {
            let rx = rx.clone();
            let spawned = std::thread::Builder::new()
                .name(format!("proving ground art {i}"))
                .spawn(move || {
                    loop {
                        let next = rx.lock().recv();
                        match next {
                            Ok(work) => work(),
                            Err(_) => return,
                        }
                    }
                });
            started += spawned.is_ok() as usize;
        }
        (started > 0).then(|| Mutex::new(tx))
    })
    .as_ref()
}

/// Run `make` on a worker, or here (`Err`) where there are none.
fn on_worker<T: Send + 'static>(make: impl FnOnce() -> T + Send + 'static) -> Result<Job<T>, T> {
    let Some(pool) = workers() else {
        return Err(make());
    };
    let slot: Slot<T> = Arc::new(Mutex::new(None));
    let theirs = slot.clone();
    let work: Work = Box::new(move || {
        let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(make));
        *theirs.lock() = Some(made);
    });
    match pool.lock().send(work) {
        Ok(()) => Ok(Job(slot)),
        // (The pool's threads are gone: nothing left to run it.)
        Err(mpsc::SendError(_)) => unreachable!("art workers never stop"),
    }
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
            Tex::Oak => textures::sanded_wood(res(512), 7),
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

/// Store a texture set's images.
fn store_set(world: &mut World, tex: Tex, set: TextureSet) {
    let mut images = world.resource_mut::<Assets<Image>>();
    let s = SetImages {
        albedo: images.add(set.albedo),
        normal: images.add(set.normal),
        orm: images.add(set.orm),
        tile_m: set.tile_m,
    };
    world.resource_mut::<ArtCache>().sets.insert(tex, s);
}

/// A material whose texture set drives color, normals, roughness, metalness and occlusion;
/// `tint` (sRGB hex) multiplies the color.
fn textured(s: SetImages, tint: u32) -> StandardMaterial {
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

/// How `m` is made: plain, or from a texture set with a tint (sRGB hex) and subsurface.
enum Recipe {
    Plain(StandardMaterial),
    Textured(Tex, u32, f32),
}

fn recipe(m: Mat) -> Recipe {
    let white = 0xffffff;
    let tex = |t: Tex| Recipe::Textured(t, white, 0.0);
    match m {
        Mat::Slab => tex(Tex::Slab),
        Mat::Concrete => tex(Tex::Concrete),
        Mat::Hazard => tex(Tex::Hazard),
        Mat::Yellow => tex(Tex::Yellow),
        Mat::Dark => tex(Tex::Dark),
        Mat::Steel => tex(Tex::Steel),
        Mat::Timber => tex(Tex::Timber),
        Mat::Diamond => tex(Tex::Diamond),
        // The second wood is a little darker and warmer.
        Mat::CrateWood(k) => Recipe::Textured(
            Tex::CrateWood(k % 2),
            if k % 2 == 0 { white } else { 0xe8d2bc },
            0.0,
        ),
        Mat::Drum(hex) => tex(Tex::Drum(hex)),
        Mat::Rubber(hex) => Recipe::Textured(Tex::Rubber, hex, 0.0),
        Mat::Domino => Recipe::Plain(StandardMaterial::hex(white).with_roughness(0.3)),
        Mat::Vinyl => Recipe::Textured(Tex::Vinyl, white, 0.1),
        Mat::Asphalt => tex(Tex::Asphalt),
        Mat::Sand => tex(Tex::Sand),
        Mat::Paint => Recipe::Plain(StandardMaterial::hex(0xe9e6dc).with_roughness(0.55)),
        Mat::Galvanized => tex(Tex::Galvanized),
        Mat::Granite => tex(Tex::Granite),
        Mat::Chrome => Recipe::Plain(
            StandardMaterial::hex(0xe4e6e9)
                .with_metallic(1.0)
                .with_roughness(0.07),
        ),
        Mat::Lacquer(hex) => Recipe::Plain(StandardMaterial::hex(hex).with_roughness(0.2)),
        Mat::Felt(hex) => Recipe::Plain(StandardMaterial::hex(hex).with_roughness(0.93)),
        Mat::Brass => Recipe::Plain(
            StandardMaterial::hex(0xd6b25e)
                .with_metallic(1.0)
                .with_roughness(0.25),
        ),
        Mat::Oak => tex(Tex::Oak),
        Mat::LinePaint => Recipe::Plain(StandardMaterial::hex(0xe3b22a).with_roughness(0.7)),
        Mat::Rope => Recipe::Plain(StandardMaterial::hex(0xb39566).with_roughness(0.95)),
        Mat::Glass => Recipe::Plain(
            StandardMaterial::hex(0xe8f1f4)
                .with_roughness(0.04)
                .with_alpha(0.12),
        ),
    }
}

/// `m` made from its recipe, if its texture set (if any) is here.
fn made(world: &World, m: Mat) -> Option<StandardMaterial> {
    match recipe(m) {
        Recipe::Plain(made) => Some(made),
        Recipe::Textured(tex, tint, subsurface) => {
            let s = *world.resource::<ArtCache>().sets.get(&tex)?;
            let made = textured(s, tint);
            Some(if subsurface > 0.0 {
                made.with_subsurface(subsurface)
            } else {
                made
            })
        }
    }
}

/// The material for `m`, made on first use. One whose texture set isn't made yet is reserved
/// (drawn plain) while a worker makes the set.
pub(crate) fn material(world: &mut World, m: Mat) -> Handle<StandardMaterial> {
    if let Some(h) = world.get_resource_or_init::<ArtCache>().materials.get(&m) {
        return *h;
    }
    if let Some(made) = made(world, m) {
        let h = world.resource_mut::<Assets<StandardMaterial>>().add(made);
        world.resource_mut::<ArtCache>().materials.insert(m, h);
        return h;
    }
    let Recipe::Textured(tex, ..) = recipe(m) else {
        unreachable!("plain materials are always made")
    };
    let h = world
        .resource_mut::<Assets<StandardMaterial>>()
        .reserve_keyed(&format!("physics/material/{m:?}"));
    let mut cache = world.resource_mut::<ArtCache>();
    cache.materials.insert(m, h);
    cache.waiting.push((m, h));
    if !cache.making.iter().any(|(t, _)| *t == tex) {
        match on_worker(move || tex.make()) {
            Ok(job) => cache.making.push((tex, job)),
            Err(set) => {
                store_set(world, tex, set);
                finish(world);
            }
        }
    }
    h
}

/// Wait for the workers and store everything they made (for screenshots and tools).
pub fn wait(world: &mut World) {
    let Some(mut cache) = world.get_resource_mut::<ArtCache>() else {
        return;
    };
    let making = std::mem::take(&mut cache.making);
    let meshing = std::mem::take(&mut cache.meshing);
    store_made(world, making, meshing);
    finish(world);
}

/// Store finished texture sets and meshes (a set whose worker panicked is made here).
#[allow(clippy::type_complexity)]
fn store_made(
    world: &mut World,
    sets: Vec<(Tex, Job<TextureSet>)>,
    meshes: Vec<(Vec<Handle<Mesh>>, Job<Vec<Mesh>>)>,
) {
    for (tex, job) in sets {
        let set = job.take().unwrap_or_else(|| tex.make());
        store_set(world, tex, set);
    }
    for (handles, job) in meshes {
        let made = job.take().unwrap_or_default();
        let mut assets = world.resource_mut::<Assets<Mesh>>();
        for (h, m) in handles.into_iter().zip(made) {
            assets.set(h, m);
        }
    }
}

/// Store what the workers have made: texture sets (and fill the materials waiting for them)
/// and meshes. Runs every frame.
pub(crate) fn finish(world: &mut World) {
    let Some(mut cache) = world.get_resource_mut::<ArtCache>() else {
        return;
    };
    if cache.making.is_empty() && cache.waiting.is_empty() && cache.meshing.is_empty() {
        return;
    }
    let (sets, making): (Vec<_>, Vec<_>) = std::mem::take(&mut cache.making)
        .into_iter()
        .partition(|(_, j)| j.done());
    cache.making = making;
    let (meshes, meshing): (Vec<_>, Vec<_>) = std::mem::take(&mut cache.meshing)
        .into_iter()
        .partition(|(_, j)| j.done());
    cache.meshing = meshing;
    store_made(world, sets, meshes);
    let waiting = std::mem::take(&mut world.resource_mut::<ArtCache>().waiting);
    let mut still = Vec::new();
    for (m, h) in waiting {
        match made(world, m) {
            Some(made) => world
                .resource_mut::<Assets<StandardMaterial>>()
                .set(h, made),
            None => still.push((m, h)),
        }
    }
    world.resource_mut::<ArtCache>().waiting = still;
}

/// Is there art for [`finish`] to store or materials waiting for it? (`finish` takes the
/// whole world, so it runs only then.)
pub(crate) fn busy(cache: Option<Res<ArtCache>>) -> bool {
    cache.is_some_and(|c| !c.making.is_empty() || !c.waiting.is_empty() || !c.meshing.is_empty())
}

/// True while workers are still making the proving ground's art.
pub fn pending(world: &World) -> bool {
    world
        .get_resource::<ArtCache>()
        .is_some_and(|c| !c.making.is_empty() || !c.waiting.is_empty() || !c.meshing.is_empty())
}

/// `N` meshes made on a worker (cached under `key`), each drawn with its material from `mats`
/// once they arrive. None of them may be empty.
pub(crate) fn parts_later<const N: usize>(
    world: &mut World,
    key: &str,
    mats: [Mat; N],
    make: impl FnOnce() -> [Mesh; N] + Send + 'static,
) -> Parts {
    let keys: [String; N] = std::array::from_fn(|i| format!("physics/{key}#{i}"));
    let cached: Option<Vec<_>> = {
        let meshes = world.resource::<Assets<Mesh>>();
        keys.iter().map(|k| meshes.by_key(k)).collect()
    };
    let handles = match cached {
        Some(h) => h,
        None => {
            let handles: Vec<Handle<Mesh>> = {
                let mut meshes = world.resource_mut::<Assets<Mesh>>();
                keys.iter().map(|k| meshes.reserve_keyed(k)).collect()
            };
            match on_worker(move || Vec::from(make())) {
                Ok(job) => world
                    .get_resource_or_init::<ArtCache>()
                    .meshing
                    .push((handles.clone(), job)),
                Err(made) => {
                    let mut meshes = world.resource_mut::<Assets<Mesh>>();
                    for (h, m) in handles.iter().zip(made) {
                        meshes.set(*h, m);
                    }
                }
            }
            handles
        }
    };
    handles
        .into_iter()
        .zip(mats)
        .map(|(h, m)| (h, material(world, m)))
        .collect()
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
