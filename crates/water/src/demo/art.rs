//! The demos' art, made on workers while the game loads.
//!
//! As Unreal's async loading thread and Unity's background loading do, the costly, world-free
//! part (procedural textures, the tarn's ground, the crag, boulders, the swimmer and waders:
//! a few seconds of one core) starts on the async compute pool once a game says it will
//! spawn a water demo ([`WaterDemoArt::prefetch`]), and the frame thread only puts the
//! finished pieces in. [`WaterLoading`](crate::WaterLoading) counts
//! the pieces still being made, so a loading screen can wait for them before it wakes the
//! water zones. A demo spawned sooner takes each piece as soon as it is done (waiting for the
//! rest), or makes it there and then when nothing started it (tests, tools).

use std::sync::OnceLock;

use bevy_tasks::Task;
use bevy_tasks::futures::check_ready;
use engine_assets::Assets;
use engine_core::prelude::*;
use engine_core::tasks::AsyncComputeTaskPool;
use engine_render::{Image, Mesh};

use super::materials::{Built, DemoMaterials, synthesize_all};
use super::props::{self, Part};
use crate::sim::WaterLoading;

static TARN: OnceLock<Mesh> = OnceLock::new();
static CRAG: OnceLock<(Mesh, Mesh)> = OnceLock::new();
static ROCKS: OnceLock<Vec<Vec<Part>>> = OnceLock::new();
static SWIMMER: OnceLock<Vec<Part>> = OnceLock::new();
static WADER: OnceLock<Vec<Part>> = OnceLock::new();

/// The test area's hill and tarn bed (0.5 m spacing).
pub(crate) fn tarn() -> Mesh {
    TARN.get_or_init(|| super::tarn_mesh(0.5)).clone()
}

/// The test area's crag crevice: (turf, bare rock).
pub(crate) fn crag() -> (Mesh, Mesh) {
    CRAG.get_or_init(|| super::crag_mesh(0.5)).clone()
}

/// The test area's boulder shapes.
pub(crate) fn rocks() -> Vec<Vec<Part>> {
    ROCKS.get_or_init(|| super::rock_shapes(true)).clone()
}

pub(crate) fn swimmer() -> Vec<Part> {
    SWIMMER.get_or_init(props::swimmer).clone()
}

pub(crate) fn wader() -> Vec<Part> {
    WADER.get_or_init(props::wader).clone()
}

/// The demo art still being made on workers.
#[derive(Resource, Default)]
pub struct DemoArtLoading {
    textures: Option<Task<Vec<Built>>>,
    meshes: Vec<Task<()>>,
}

impl DemoArtLoading {
    /// Pieces not finished yet.
    pub fn pending(&self) -> usize {
        self.textures.is_some() as usize + self.meshes.len()
    }
}

/// Asks for the water demos' art ahead of time: a game that will spawn [`spawn_demo`] or
/// [`spawn_valley`] (a showcase with its water or lake zone) inserts it while it loads, and
/// the art is made on workers from the next frame, without seconds of worker time and memory
/// in apps that never show it. [`WaterLoading::is_ready`](crate::WaterLoading::is_ready)
/// then waits for it.
///
/// [`spawn_demo`]: super::spawn_demo
/// [`spawn_valley`]: super::valley::spawn_valley
#[derive(Resource, Debug, Default)]
pub struct WaterDemoArt {
    started: bool,
}

impl WaterDemoArt {
    /// Start making the demo art on workers (once; later calls do nothing).
    pub fn prefetch(world: &mut World) {
        if !world.contains_resource::<WaterDemoArt>() {
            world.insert_resource(WaterDemoArt::default());
        }
    }
}

/// Start the demo art on the async compute pool once [`WaterDemoArt::prefetch`] asked for it
/// (in an app with a mesh and an image store).
pub fn start_demo_art_system(world: &mut World) {
    if !world
        .get_resource::<WaterDemoArt>()
        .is_some_and(|a| !a.started)
        || !world.contains_resource::<Assets<Mesh>>()
        || !world.contains_resource::<Assets<Image>>()
    {
        return;
    }
    world.resource_mut::<WaterDemoArt>().started = true;
    if world.contains_resource::<DemoMaterials>() {
        return;
    }
    engine_core::tasks::init_task_pools();
    let pool = AsyncComputeTaskPool::get();
    let mut loading = DemoArtLoading {
        textures: Some(pool.spawn(async { synthesize_all(512, 1) })),
        meshes: Vec::new(),
    };
    // Longest first, so the pool's few threads finish together.
    let jobs: [fn(); 5] = [
        // (One core: the compute pool's threads are the frame's.)
        || {
            ROCKS.get_or_init(|| super::rock_shapes(false));
        },
        || {
            CRAG.get_or_init(|| super::crag_mesh(0.5));
        },
        || {
            SWIMMER.get_or_init(props::swimmer);
        },
        || {
            WADER.get_or_init(props::wader);
        },
        || {
            TARN.get_or_init(|| super::tarn_mesh(0.5));
        },
    ];
    for job in jobs {
        loading.meshes.push(pool.spawn(async move { job() }));
    }
    let pending = loading.pending();
    world.insert_resource(loading);
    world.resource_mut::<WaterLoading>().art = pending;
}

/// Put finished art in: the textures become [`DemoMaterials`] (the meshes wait in their
/// caches for the demos). Keeps [`WaterLoading::art`] up to date.
pub fn finish_demo_art_system(world: &mut World) {
    let Some(mut loading) = world.get_resource_mut::<DemoArtLoading>() else {
        return;
    };
    loading.meshes.retain_mut(|t| check_ready(t).is_none());
    let textures = match loading.textures.as_mut().map(check_ready) {
        Some(Some(built)) => {
            loading.textures = None;
            Some(built)
        }
        _ => None,
    };
    let pending = loading.pending();
    if pending == 0 {
        world.remove_resource::<DemoArtLoading>();
    }
    if let Some(built) = textures
        && !world.contains_resource::<DemoMaterials>()
    {
        let materials =
            DemoMaterials::from_built(&mut world.resource_mut::<Assets<Image>>(), built);
        world.insert_resource(materials);
    }
    if world.resource::<WaterLoading>().art != pending {
        world.resource_mut::<WaterLoading>().art = pending;
    }
}

/// The demo materials, for a prop being drawn now: the finished ones, the ones being made
/// (waiting for them), or made here when nothing started them.
pub(crate) fn materials(world: &mut World) -> &DemoMaterials {
    if !world.contains_resource::<DemoMaterials>() {
        if !world.contains_resource::<Assets<Image>>() {
            world.insert_resource(DemoMaterials::default());
            return world.resource::<DemoMaterials>();
        }
        let pending = world
            .get_resource_mut::<DemoArtLoading>()
            .and_then(|mut l| l.textures.take());
        let built = match pending {
            Some(task) => bevy_tasks::block_on(task),
            None => synthesize_all(512, 0),
        };
        let materials =
            DemoMaterials::from_built(&mut world.resource_mut::<Assets<Image>>(), built);
        world.insert_resource(materials);
    }
    world.resource::<DemoMaterials>()
}
