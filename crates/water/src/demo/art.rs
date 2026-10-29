//! The demos' art, made on workers while the game loads.
//!
//! As Unreal's async loading thread and Unity's background loading do, the costly, world-free
//! part (procedural textures, the tarn's ground, the crag, boulders, the swimmer and waders,
//! boats, the jetty, buoys, logs and the streams' beds: a few seconds of one core) starts on the async compute pool once a game says it will
//! spawn a water demo ([`WaterDemoArt::prefetch`]), and the frame thread only puts the
//! finished pieces in. [`WaterLoading`](crate::WaterLoading) counts
//! the pieces still being made, so a loading screen can wait for them before it wakes the
//! water zones. A demo spawned sooner takes each piece as soon as it is done (waiting for the
//! rest), or makes it there and then when nothing started it (tests, tools).

use std::sync::{Mutex, OnceLock};

use bevy_tasks::Task;
use bevy_tasks::futures::check_ready;
use engine_assets::Assets;
use engine_core::math::Mat4;
use engine_core::prelude::*;
use engine_core::tasks::AsyncComputeTaskPool;
use engine_render::{Image, Mesh};
use engine_world::physics::Collider;

use super::materials::{Built, DemoMaterials, synthesize_all};
use super::props::{self, Part};
use super::scenery::{Piece, build_jetty};
use super::{Boulder, crag, tarn_height};
use crate::boat::{HullCut, Interior, hull_collider};
use crate::flow::FlowRegion;
use crate::sim::WaterLoading;

static TARN: OnceLock<Mesh> = OnceLock::new();
static CRAG: OnceLock<(Mesh, Mesh)> = OnceLock::new();
static ROCKS: OnceLock<Vec<Vec<Part>>> = OnceLock::new();
static SWIMMER: OnceLock<Vec<Part>> = OnceLock::new();
static WADER: OnceLock<Vec<Part>> = OnceLock::new();
static ROWBOAT: OnceLock<(Vec<Part>, Collider, Interior)> = OnceLock::new();
static MOTORBOAT: OnceLock<(Vec<Part>, Collider, Interior)> = OnceLock::new();
static BOULDERS: OnceLock<Vec<Boulder>> = OnceLock::new();
static CULVERT: OnceLock<Vec<Part>> = OnceLock::new();
static BUOYS: OnceLock<Vec<Vec<Part>>> = OnceLock::new();
static LOGS: OnceLock<Vec<Vec<Part>>> = OnceLock::new();
static STREAM_BED: OnceLock<Vec<f32>> = OnceLock::new();
static CASCADE: OnceLock<(Transform, FlowRegion)> = OnceLock::new();
/// A jetty's plan ([`jetty_plan`](super::scenery::jetty_plan)).
type Plan = Vec<(Piece, Mat4)>;
/// Jetties made so far, by their plans (the test area's, the valley's).
static JETTIES: Mutex<Vec<(Plan, Vec<Part>)>> = Mutex::new(Vec::new());

/// Buoy seeds kept made ([`buoy`]): the test area's five, the valley's three.
const BUOY_SEEDS: u64 = 5;
/// Logs in the test area's stream ([`stream_log`]).
const LOGS_IN_STREAM: u64 = 3;

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
    if let Some(rocks) = ROCKS.get() {
        return rocks.clone();
    }
    // Built outside the lock: while the wide build waits for its pieces this thread runs other
    // pool tasks, and one may be another app's demo asking for the rocks (tests run several
    // apps in one process), which would wait on the lock this thread holds forever.
    let built = super::rock_shapes(true);
    ROCKS.get_or_init(|| built).clone()
}

/// The test area's boulders, placed and stretched.
pub(crate) fn boulders() -> Vec<Boulder> {
    if let Some(b) = BOULDERS.get() {
        return b.clone();
    }
    let built = super::boulder_layout(&rocks());
    BOULDERS.get_or_init(|| built).clone()
}

pub(crate) fn culvert() -> Vec<Part> {
    CULVERT.get_or_init(props::culvert).clone()
}

/// A mooring buoy's model.
pub(crate) fn buoy(seed: u64) -> Vec<Part> {
    if seed >= BUOY_SEEDS {
        return props::buoy(seed);
    }
    BUOYS.get_or_init(|| (0..BUOY_SEEDS).map(props::buoy).collect())[seed as usize].clone()
}

/// Log `k` of those riding the test area's stream.
pub(crate) fn stream_log(k: u64) -> Vec<Part> {
    if k >= LOGS_IN_STREAM {
        return props::log(1.7, 0.17, k);
    }
    LOGS.get_or_init(|| {
        (0..LOGS_IN_STREAM)
            .map(|k| props::log(1.7, 0.17, k))
            .collect()
    })[k as usize]
        .clone()
}

/// The bed under the test area's outlet stream.
pub(crate) fn stream_bed() -> Vec<f32> {
    STREAM_BED.get_or_init(super::stream_bed_heights).clone()
}

/// The crag's cascade, for the test area centered at the origin.
pub(crate) fn cascade() -> (Transform, FlowRegion) {
    CASCADE
        .get_or_init(|| crag::cascade(DVec3::ZERO, tarn_height))
        .clone()
}

/// The jetty `plan` lays out: made once per plan.
pub(crate) fn jetty(plan: Plan) -> Vec<Part> {
    let made = JETTIES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, parts)) = made.iter().find(|(p, _)| *p == plan) {
        return parts.clone();
    }
    // Made outside the lock.
    drop(made);
    let parts = build_jetty(&plan);
    let mut made = JETTIES.lock().unwrap_or_else(|e| e.into_inner());
    if made.len() < 4 && !made.iter().any(|(p, _)| *p == plan) {
        made.push((plan, parts.clone()));
    }
    parts
}

pub(crate) fn swimmer() -> Vec<Part> {
    SWIMMER.get_or_init(props::swimmer).clone()
}

pub(crate) fn wader() -> Vec<Part> {
    WADER.get_or_init(props::wader).clone()
}

/// The rowboat's model and its collider.
pub(crate) fn rowboat() -> (Vec<Part>, Collider, Interior) {
    ROWBOAT.get_or_init(make_rowboat).clone()
}

/// The motorboat's model and its collider.
pub(crate) fn motorboat() -> (Vec<Part>, Collider, Interior) {
    MOTORBOAT.get_or_init(make_motorboat).clone()
}

fn make_rowboat() -> (Vec<Part>, Collider, Interior) {
    let parts = props::rowboat();
    let (hull, inside) = hull_collider(
        &parts,
        HullCut {
            floor_y: -0.16,
            transom_x: -1.82,
            wall: 0.05,
        },
    );
    (parts, hull, inside)
}

fn make_motorboat() -> (Vec<Part>, Collider, Interior) {
    let parts = props::motorboat();
    let (hull, inside) = hull_collider(
        &parts,
        HullCut {
            floor_y: -0.05,
            transom_x: -2.62,
            wall: 0.07,
        },
    );
    (parts, hull, inside)
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
/// (in an app with a mesh and an image store). A plain system that queues the start, so it
/// doesn't hold the frame to itself every frame as a `&mut World` system would.
pub fn start_demo_art_system(mut commands: Commands, art: Option<ResMut<WaterDemoArt>>) {
    let Some(mut art) = art.filter(|a| !a.started) else {
        return;
    };
    art.started = true;
    commands.queue(start_demo_art);
}

fn start_demo_art(world: &mut World) {
    if !world.contains_resource::<Assets<Mesh>>() || !world.contains_resource::<Assets<Image>>() {
        // Asked again next frame.
        world.resource_mut::<WaterDemoArt>().started = false;
        return;
    }
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
    let jobs: [fn(); 8] = [
        // (One core: the compute pool's threads are the frame's.)
        || {
            let rocks = ROCKS.get_or_init(|| super::rock_shapes(false));
            BOULDERS.get_or_init(|| super::boulder_layout(rocks));
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
        || {
            ROWBOAT.get_or_init(make_rowboat);
        },
        || {
            MOTORBOAT.get_or_init(make_motorboat);
        },
        // The smaller props and the streams' beds.
        || {
            culvert();
            buoy(0);
            stream_log(0);
            stream_bed();
            cascade();
            super::with_demo_jetty(DVec3::ZERO, |j| jetty(super::scenery::jetty_plan(j)));
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
pub fn finish_demo_art_system(
    mut commands: Commands,
    loading: Option<ResMut<DemoArtLoading>>,
    mut status: ResMut<WaterLoading>,
) {
    let Some(mut loading) = loading else {
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
        commands.remove_resource::<DemoArtLoading>();
    }
    if let Some(built) = textures {
        commands.queue(move |world: &mut World| {
            if !world.contains_resource::<DemoMaterials>() {
                let materials =
                    DemoMaterials::from_built(&mut world.resource_mut::<Assets<Image>>(), built);
                world.insert_resource(materials);
            }
        });
    }
    if status.art != pending {
        status.art = pending;
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
