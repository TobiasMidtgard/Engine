//! Far views of streamed cells (hierarchical levels of detail, as Unreal's World Partition HLOD
//! and Unity's HLOD system): while a cell is out, one cheap mesh stands in for all of it, so a
//! zone on the far side of the valley is still there to see instead of a hole.
//!
//! The stand-in (a proxy) is captured from the cell itself. Once a cell has loaded and settled,
//! its still meshes ([`MeshRenderer`] without a moving body above them) are gathered on the main
//! thread and merged on a worker by vertex clustering (Rossignac and Borrel's simplification: snap
//! corners to a grid, merge what lands in one grid cell, drop the triangles that collapse),
//! coarsening the grid until it fits [`HlodSettings::triangles`]. Each corner's color is what
//! the loaded mesh shows from afar: its vertex color times its material's base color, its
//! texture's mean albedo (or its preset's) and part of its occlusion map's mean, averaged by area
//! over the corners merged into it; it goes into the proxy's vertices, so one plain material
//! draws every proxy in one go. Texture means are taken while the images still have their
//! pixels ([`ImageMeans`]): the renderer frees uploaded pixels after a couple of seconds, long
//! before most cells are captured. A proxy shows wherever its cell is out (up to
//! [`HlodSettings::draw_m`]); when the cell comes back its meshes fade in over the proxy with the
//! complementary dither ([`DitherFade::pair`]), and the proxy goes.
//!
//! A cell given a [`StreamCell::proxy_version`](crate::stream::StreamCell::proxy_version) keeps
//! its proxy on disk ([`HlodSettings::cache_dir`]), so the next run shows it before the cell was
//! ever visited; `ENGINE_HLOD_BAKE=1` loads every such cell once, in turn, to fill the cache
//! (a release step, or a tool).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Instant;

use engine_assets::{Assets, Handle};
use engine_core::math::{Mat4, Vec2, Vec3, Vec4};
use engine_core::prelude::*;
use engine_render::{
    DitherFade, Image, MaterialRef, MaxDrawDistance, Mesh, MeshRef, MeshRenderer, StandardMaterial,
    Vertex,
};
use engine_world::physics::{BodyKind, RigidBody, Velocity};
use engine_world::shift::{FollowsWorldShift, WorldShift};
use engine_world::{CellId, CellMember, CellState, StreamedCells};

/// How proxies are made and shown.
#[derive(Resource, Debug, Clone)]
pub struct HlodSettings {
    /// Make proxies at all.
    pub enabled: bool,
    /// Triangles a proxy may have at most.
    pub triangles: usize,
    /// Farthest a proxy draws, meters.
    pub draw_m: f32,
    /// Frames a loaded cell settles (and its fade-in ends) before it is captured.
    pub settle_frames: u32,
    /// Milliseconds a frame may spend walking a cell's meshes for its capture.
    pub gather_ms: f32,
    /// Where proxies of versioned cells are kept between runs (`ENGINE_HLOD_DIR`, else
    /// `hlod-cache` in the working directory).
    pub cache_dir: PathBuf,
    /// Load every versioned cell without a proxy once, in turn, to make one
    /// (`ENGINE_HLOD_BAKE=1`).
    pub bake: bool,
}

impl Default for HlodSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            triangles: 16_000,
            draw_m: 3000.0,
            settle_frames: 30,
            gather_ms: 2.0,
            cache_dir: std::env::var_os("ENGINE_HLOD_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("hlod-cache")),
            bake: std::env::var("ENGINE_HLOD_BAKE").is_ok_and(|v| v != "0"),
        }
    }
}

/// How a cell's proxy is kept: made at all, and the content version its disk copy is for
/// (`None`: in memory only).
#[derive(Debug, Clone, Copy)]
pub(crate) struct ProxyConfig {
    pub enabled: bool,
    pub version: Option<u64>,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            version: None,
        }
    }
}

/// On a proxy's entity: the cell it stands in for.
#[derive(Component, Debug, Clone, Copy)]
pub struct HlodProxy(pub CellId);

/// Each cell's proxy, in the order of [`StreamedCells::cells`].
/// Linear mean color of every image, taken while it still has its pixels, by slot: (the slot's
/// generation, the asset version it was taken from, the mean).
#[derive(Resource, Default)]
pub struct ImageMeans {
    means: HashMap<u32, (Option<u32>, u64, Vec4)>,
    /// `Assets::<Image>::latest_version` the last full pass saw.
    seen: u64,
    /// Where an unfinished pass goes on.
    cursor: usize,
}

impl ImageMeans {
    /// The mean of `h`'s image, if one was taken for this asset.
    pub fn get(&self, h: Handle<Image>) -> Option<Vec4> {
        let (generation, _, mean) = self.means.get(&h.index())?;
        (h.generation().is_none() || h.generation() == *generation).then_some(*mean)
    }

    /// Takes the means of new and changed images until `deadline`.
    fn refresh(&mut self, images: &Assets<Image>, deadline: Instant) {
        let latest = images.latest_version();
        if latest == self.seen && self.cursor == 0 {
            return;
        }
        for (n, (h, img)) in images.iter().enumerate().skip(self.cursor) {
            if Instant::now() > deadline {
                self.cursor = n;
                return;
            }
            let version = images.version(h);
            let fresh = self
                .means
                .get(&h.index())
                .is_some_and(|(g, v, _)| *g == h.generation() && *v == version);
            if !fresh && !img.is_released() {
                self.means
                    .insert(h.index(), (h.generation(), version, mean_of(img)));
            }
        }
        self.cursor = 0;
        self.seen = latest;
    }
}

#[derive(Resource, Default)]
pub struct Proxies {
    slots: Vec<ProxySlot>,
    material: Option<Handle<StandardMaterial>>,
    /// Bake mode: the cell pinned in for its capture.
    baking: Option<usize>,
    /// Bake mode: cells pinned out after their capture (kept ones too), so memory stays at
    /// about one cell; unpinned when the bake ends.
    baked_out: Vec<String>,
    /// Bake mode: every versioned cell has its proxy.
    pub baked: bool,
}

#[derive(Default)]
struct ProxySlot {
    /// The cell's frame at registration, following world shifts: proxies are in its space.
    frame: Option<Transform>,
    mesh: Option<Handle<Mesh>>,
    triangles: usize,
    entity: Option<Entity>,
    settled: u32,
    /// A capture walking the cell's meshes, a slice a frame.
    gather: Option<Box<Gather>>,
    /// A worker merging the capture (behind a lock only so the resource is `Sync`).
    capturing: Option<Mutex<Receiver<Option<Mesh>>>>,
    /// Tried and found nothing to show (or the worker failed): don't capture again.
    empty: bool,
    loaded_from_disk: bool,
}

impl Proxies {
    /// Triangles of each cell's proxy (0 without one), in cell order.
    pub fn triangles(&self) -> Vec<usize> {
        self.slots.iter().map(|s| s.triangles).collect()
    }
}

impl FollowsWorldShift for Proxies {
    fn follow(&mut self, s: &WorldShift) {
        for slot in &mut self.slots {
            if let Some(frame) = slot.frame.as_mut() {
                *frame = s.transform(frame);
            }
        }
    }
}

pub(crate) fn register(app: &mut App) {
    use engine_world::shift::WorldShiftAppExt;
    app.init_resource::<HlodSettings>()
        .init_resource::<Proxies>()
        .init_resource::<ImageMeans>()
        .shift_resource::<Proxies>()
        .register_api(
            "stream.proxies",
            "The far-view proxies of streamed cells: each cell's key, its proxy's triangles \
             (0 = none yet) and whether it is showing.",
            "{}",
            |world, _| {
                let cells = world.resource::<StreamedCells>();
                let proxies = world.resource::<Proxies>();
                let rows: Vec<_> = cells
                    .cells
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        let slot = proxies.slots.get(i);
                        serde_json::json!({
                            "key": c.key,
                            "triangles": slot.map_or(0, |s| s.triangles),
                            "showing": slot.is_some_and(|s| s.entity.is_some()),
                        })
                    })
                    .collect();
                Ok(serde_json::json!({ "cells": rows, "baked": proxies.baked }))
            },
        );
}

/// Captures, shows and hides proxies (`Last`, after the cells' loads and fades).
///
/// `streaming`: some source decides what loads. Without one (tours, tools) every cell is in and
/// no proxy would ever show, so none is captured, unless baking.
pub(crate) fn update(
    world: &mut World,
    configs: &[ProxyConfig],
    fading: &[CellId],
    streaming: bool,
) {
    let settings = world.resource::<HlodSettings>().clone();
    // Without the renderer's assets (a server, a tool) there is nothing to draw proxies with.
    let drawable = world.contains_resource::<Assets<Mesh>>()
        && world.contains_resource::<Assets<StandardMaterial>>()
        && world.contains_resource::<Assets<Image>>();
    if !settings.enabled || !drawable {
        return;
    }
    {
        let deadline = Instant::now() + std::time::Duration::from_micros(500);
        let mut means = world.remove_resource::<ImageMeans>().unwrap_or_default();
        means.refresh(world.resource::<Assets<Image>>(), deadline);
        world.insert_resource(means);
    }
    let cells: Vec<(String, DVec3, CellState)> = world
        .resource::<StreamedCells>()
        .cells
        .iter()
        .map(|c| (c.key.clone(), c.center, c.state))
        .collect();
    {
        let mut proxies = world.resource_mut::<Proxies>();
        if proxies.slots.len() < cells.len() {
            proxies.slots.resize_with(cells.len(), Default::default);
        }
    }
    for (i, (key, center, state)) in cells.iter().enumerate() {
        let config = configs.get(i).copied().unwrap_or_default();
        if !config.enabled {
            continue;
        }
        let id = CellId(i as u32);
        let first = world.resource::<Proxies>().slots[i].frame.is_none();
        if first {
            world.resource_mut::<Proxies>().slots[i].frame =
                Some(Transform::from_translation(*center));
            if let Some(version) = config.version {
                load_from_disk(world, i, key, version, &settings);
            }
        }
        collect(world, i, key, config, &settings);
        let fading_in = fading.contains(&id);
        let slot = &world.resource::<Proxies>().slots[i];
        let (mesh, entity, empty) = (slot.mesh, slot.entity, slot.empty);
        match state {
            CellState::Loaded if !fading_in => {
                // The cell is in and shown: its proxy goes (the fade took it, or there was none).
                if let Some(e) = entity {
                    despawn(world, i, e);
                }
                if mesh.is_none() && !empty && (streaming || settings.bake) {
                    capture(world, i, id, key, &settings);
                }
            }
            CellState::Loaded => {}
            _ => {
                let slot = &mut world.resource_mut::<Proxies>().slots[i];
                slot.settled = 0;
                slot.gather = None;
                if let (Some(mesh), None) = (mesh, entity) {
                    show(world, i, id, mesh, &settings);
                    // A leaving cell still shows: the proxy comes in as it fades out.
                    if *state == CellState::Unloading {
                        fade_in(world, id, 0.0);
                    }
                }
            }
        }
    }
    bake(world, configs, &cells, &settings);
}

/// The proxy of cell `id` fades out as its meshes fade in, `t` of the way (1 = gone).
pub(crate) fn fade_out(world: &mut World, id: CellId, t: f32) {
    let Some(entity) = world
        .resource::<Proxies>()
        .slots
        .get(id.0 as usize)
        .and_then(|s| s.entity)
    else {
        return;
    };
    if t >= 1.0 {
        despawn(world, id.0 as usize, entity);
    } else if let Ok(mut e) = world.get_entity_mut(entity) {
        e.insert(DitherFade::pair(t).0);
    }
}

/// The proxy of cell `id` fades in as its meshes fade out, `t` of the way (1 = all in).
pub(crate) fn fade_in(world: &mut World, id: CellId, t: f32) {
    let Some(entity) = world
        .resource::<Proxies>()
        .slots
        .get(id.0 as usize)
        .and_then(|s| s.entity)
    else {
        return;
    };
    let Ok(mut e) = world.get_entity_mut(entity) else {
        return;
    };
    if t >= 1.0 {
        e.remove::<DitherFade>();
    } else {
        e.insert(DitherFade::pair(t).1);
    }
}

fn despawn(world: &mut World, i: usize, entity: Entity) {
    if let Ok(e) = world.get_entity_mut(entity) {
        e.despawn();
    }
    world.resource_mut::<Proxies>().slots[i].entity = None;
}

fn show(world: &mut World, i: usize, id: CellId, mesh: Handle<Mesh>, settings: &HlodSettings) {
    let material = proxy_material(world);
    let Some(frame) = world.resource::<Proxies>().slots[i].frame else {
        return;
    };
    let entity = world
        .spawn((
            Name::new("hlod proxy"),
            frame,
            GlobalTransform::from(frame),
            MeshRenderer {
                mesh: MeshRef::Handle(mesh),
                material: MaterialRef::Handle(material),
                cast_shadows: true,
                visible: true,
            },
            MaxDrawDistance(settings.draw_m),
            HlodProxy(id),
        ))
        .id();
    world.resource_mut::<Proxies>().slots[i].entity = Some(entity);
}

/// One plain material for every proxy; the color is in the vertices.
fn proxy_material(world: &mut World) -> Handle<StandardMaterial> {
    if let Some(m) = world.resource::<Proxies>().material {
        return m;
    }
    let material = StandardMaterial {
        base_color: [1.0; 4],
        perceptual_roughness: 0.85,
        metallic: 0.0,
        ..Default::default()
    };
    let handle = world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(material);
    world.resource_mut::<Proxies>().material = Some(handle);
    handle
}

/// One still mesh of a cell: corners, their colors and triangles shared by its instances.
struct Source {
    positions: Vec<Vec3>,
    colors: Vec<Vec3>,
    indices: Vec<u32>,
}

/// One instance: which source, where in the proxy's space, which color.
struct Instance {
    source: usize,
    matrix: Mat4,
    color: Vec4,
}

/// Gathers the settled cell's still meshes and merges them on a worker.
fn capture(world: &mut World, i: usize, id: CellId, key: &str, settings: &HlodSettings) {
    {
        let slot = &mut world.resource_mut::<Proxies>().slots[i];
        if slot.capturing.is_some() {
            return;
        }
        slot.settled += 1;
        if slot.settled < settings.settle_frames {
            return;
        }
    }
    let Some(frame) = world.resource::<Proxies>().slots[i].frame else {
        return;
    };
    let mut gather = match world.resource_mut::<Proxies>().slots[i].gather.take() {
        Some(g) => g,
        None => {
            let roots: Vec<Entity> = world
                .query::<(Entity, &CellMember)>()
                .iter(world)
                .filter(|(_, m)| m.0 == id)
                .map(|(e, _)| e)
                .collect();
            Box::new(Gather::new(roots, frame))
        }
    };
    let deadline = Instant::now() + std::time::Duration::from_secs_f32(settings.gather_ms / 1000.0);
    if !gather.step(world, deadline) {
        world.resource_mut::<Proxies>().slots[i].gather = Some(gather);
        return;
    }
    let Gather {
        sources, instances, ..
    } = *gather;
    let budget = settings.triangles;
    let key = key.to_string();
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name(format!("hlod {key}"))
        .spawn(move || {
            let t = Instant::now();
            let source_tris: usize = instances
                .iter()
                .map(|n| sources[n.source].indices.len() / 3)
                .sum();
            let mesh = cluster(&sources, &instances, budget);
            if let Some((mesh, grid)) = &mesh {
                engine_core::log::info!(
                    "hlod: {key} proxy of {} triangles (grid {grid:.2} m) from {} meshes, {} \
                     triangles, in {:.2} s",
                    mesh.indices.len() / 3,
                    instances.len(),
                    source_tris,
                    t.elapsed().as_secs_f32()
                );
            }
            let _ = tx.send(mesh.map(|(m, _)| m));
        });
    let slot = &mut world.resource_mut::<Proxies>().slots[i];
    match spawned {
        Ok(_) => slot.capturing = Some(Mutex::new(rx)),
        Err(_) => slot.empty = true,
    }
}

/// Takes a finished capture: the proxy mesh goes into the assets (and to disk when versioned).
fn collect(world: &mut World, i: usize, key: &str, config: ProxyConfig, settings: &HlodSettings) {
    let polled = {
        let slot = &mut world.resource_mut::<Proxies>().slots[i];
        let Some(rx) = slot.capturing.as_mut() else {
            return;
        };
        match rx
            .get_mut()
            .map_or(Err(TryRecvError::Disconnected), |rx| rx.try_recv())
        {
            Ok(mesh) => mesh,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => None,
        }
    };
    world.resource_mut::<Proxies>().slots[i].capturing = None;
    let Some(mesh) = polled else {
        world.resource_mut::<Proxies>().slots[i].empty = true;
        return;
    };
    if let Some(version) = config.version {
        let path = cache_path(settings, key, version);
        if let Err(e) = write_mesh(&path, &mesh) {
            engine_core::log::warn!("hlod: can't keep {key}'s proxy at {}: {e}", path.display());
        }
    }
    store(world, i, mesh);
}

fn store(world: &mut World, i: usize, mesh: Mesh) {
    let triangles = mesh.indices.len() / 3;
    let handle = world.resource_mut::<Assets<Mesh>>().add(mesh);
    let slot = &mut world.resource_mut::<Proxies>().slots[i];
    slot.mesh = Some(handle);
    slot.triangles = triangles;
}

fn load_from_disk(world: &mut World, i: usize, key: &str, version: u64, settings: &HlodSettings) {
    let path = cache_path(settings, key, version);
    let Ok(mesh) = read_mesh(&path) else {
        return;
    };
    store(world, i, mesh);
    world.resource_mut::<Proxies>().slots[i].loaded_from_disk = true;
}

/// Bake mode: pins each versioned cell without a proxy in until it has one, one at a time.
fn bake(
    world: &mut World,
    configs: &[ProxyConfig],
    cells: &[(String, DVec3, CellState)],
    settings: &HlodSettings,
) {
    if !settings.bake || world.resource::<Proxies>().baked {
        return;
    }
    let done = |world: &World, i: usize| {
        let slot = &world.resource::<Proxies>().slots[i];
        slot.mesh.is_some() || slot.empty
    };
    if let Some(i) = world.resource::<Proxies>().baking {
        if !done(world, i) {
            return;
        }
        world
            .resource_mut::<StreamedCells>()
            .pins
            .insert(cells[i].0.clone(), false);
        let mut proxies = world.resource_mut::<Proxies>();
        proxies.baking = None;
        proxies.baked_out.push(cells[i].0.clone());
    }
    let next = (0..cells.len()).find(|&i| {
        let c = configs.get(i).copied().unwrap_or_default();
        c.enabled && c.version.is_some() && !done(world, i)
    });
    match next {
        Some(i) => {
            engine_core::log::info!("hlod bake: {}", cells[i].0);
            world
                .resource_mut::<StreamedCells>()
                .pins
                .insert(cells[i].0.clone(), true);
            world.resource_mut::<Proxies>().baking = Some(i);
        }
        None => {
            // Done once the last ones are out too.
            let leaving = world.resource::<Proxies>().baked_out.iter().any(|key| {
                cells
                    .iter()
                    .any(|(k, _, state)| k == key && *state != CellState::Unloaded)
            });
            if leaving {
                return;
            }
            let out = std::mem::take(&mut world.resource_mut::<Proxies>().baked_out);
            let mut streamed = world.resource_mut::<StreamedCells>();
            for key in out {
                streamed.pins.remove(&key);
            }
            world.resource_mut::<Proxies>().baked = true;
            engine_core::log::info!("hlod bake: done");
        }
    }
}

/// Walks a settled cell's trees for its still meshes, in the proxy's space with their colors,
/// a slice of entities a frame ([`HlodSettings::gather_ms`]) so a big zone doesn't hitch.
struct Gather {
    frame: Transform,
    stack: Vec<Entity>,
    sources: Vec<Source>,
    by_mesh: HashMap<String, usize>,
    instances: Vec<Instance>,
}

impl Gather {
    fn new(roots: Vec<Entity>, frame: Transform) -> Self {
        Self {
            frame,
            stack: roots,
            sources: Vec::new(),
            by_mesh: HashMap::new(),
            instances: Vec::new(),
        }
    }

    /// Walks on until `deadline`; true once every tree is done.
    fn step(&mut self, world: &World, deadline: Instant) -> bool {
        let meshes = world.resource::<Assets<Mesh>>();
        let materials = world.resource::<Assets<StandardMaterial>>();
        let images = world.resource::<Assets<Image>>();
        let means = world.get_resource::<ImageMeans>();
        let inverse = self.frame.rotation.inverse();
        // Skips what moves (a body that isn't static, or anything with a velocity) together
        // with everything under it.
        while let Some(e) = self.stack.pop() {
            if Instant::now() > deadline {
                self.stack.push(e);
                return false;
            }
            let Ok(entity) = world.get_entity(e) else {
                continue;
            };
            let moves = entity
                .get::<RigidBody>()
                .is_some_and(|b| b.kind != BodyKind::Static)
                || entity.contains::<Velocity>();
            if moves {
                continue;
            }
            if let Some(children) = entity.get::<Children>() {
                self.stack.extend(children.iter());
            }
            let (Some(renderer), Some(global)) = (
                entity.get::<MeshRenderer>(),
                entity.get::<GlobalTransform>(),
            ) else {
                continue;
            };
            // Hidden ones, and ones a game fades out itself, stay out.
            let faded = entity
                .get::<DitherFade>()
                .is_some_and(|d| d.hidden() >= 0.5)
                && !entity.contains::<crate::stream::StreamFade>();
            if !renderer.visible || faded {
                continue;
            }
            let mesh_key = match &renderer.mesh {
                MeshRef::Handle(h) => format!("h{}", h.index()),
                MeshRef::Shape(s) => s.key(),
                MeshRef::Asset(k) => format!("a{k}"),
            };
            let source = match self.by_mesh.get(&mesh_key) {
                Some(&s) => s,
                None => {
                    // Copied once per mesh, corners only; instances share it.
                    let take = |m: &Mesh| Source {
                        positions: m.vertices.iter().map(|v| Vec3::from(v.position)).collect(),
                        colors: m
                            .vertices
                            .iter()
                            .map(|v| Vec3::new(v.color[0], v.color[1], v.color[2]))
                            .collect(),
                        indices: m.indices.clone(),
                    };
                    let source = match &renderer.mesh {
                        MeshRef::Handle(h) => meshes.get(*h).map(take),
                        MeshRef::Shape(s) => Some(take(&s.mesh())),
                        MeshRef::Asset(k) => meshes.by_key(k).and_then(|h| meshes.get(h)).map(take),
                    };
                    let Some(source) = source else {
                        continue;
                    };
                    self.sources.push(source);
                    self.by_mesh.insert(mesh_key, self.sources.len() - 1);
                    self.sources.len() - 1
                }
            };
            let material = match &renderer.material {
                MaterialRef::Handle(h) => materials.get(*h),
                MaterialRef::Inline(m) => Some(m),
                MaterialRef::Asset(k) => materials.by_key(k).and_then(|h| materials.get(h)),
            };
            let translation = inverse * (global.translation() - self.frame.translation).as_vec3();
            let matrix = Mat4::from_scale_rotation_translation(
                global.scale(),
                inverse * global.rotation(),
                translation,
            );
            self.instances.push(Instance {
                source,
                matrix,
                color: material.map_or(Vec4::splat(0.5), |m| albedo(m, images, means)),
            });
        }
        true
    }
}

/// Share of a surface's light the occlusion map darkens: it shades ambient light, not the sun,
/// and outdoors in daylight ambient is roughly this part of what a wall reflects.
const AMBIENT_SHARE: f32 = 0.4;

/// A material's mean color from afar: its base color times its texture's mean (or its preset's;
/// an untextured material's is its base color alone), darkened by its occlusion map's mean on
/// the ambient share of the light.
fn albedo(m: &StandardMaterial, images: &Assets<Image>, means: Option<&ImageMeans>) -> Vec4 {
    let base = Vec4::from(m.base_color);
    let mean = |h: Handle<Image>| {
        images
            .get(h)
            .filter(|img| !img.is_released())
            .map(mean_of)
            .or_else(|| means.and_then(|m| m.get(h)))
    };
    let tint = match (m.base_color_texture, m.preset) {
        (Some(h), preset) => mean(h)
            .or(preset.map(|p| Vec3::from(p.mean_albedo()).extend(1.0)))
            // Pixels gone before anyone saw them: a typical surface, not a white one.
            .unwrap_or(Vec4::splat(0.45)),
        (None, Some(p)) => Vec3::from(p.mean_albedo()).extend(1.0),
        (None, None) => Vec4::ONE,
    };
    let occlusion = m.occlusion_texture.and_then(mean).map_or(1.0, |o| o.x);
    let mut c = base * tint * (1.0 - AMBIENT_SHARE * (1.0 - occlusion.clamp(0.0, 1.0)));
    c.w = 1.0;
    c
}

/// Mean color of an image from up to 32×32 samples, linear.
fn mean_of(img: &Image) -> Vec4 {
    let n = 32u32;
    let mut sum = Vec3::ZERO;
    for y in 0..n {
        for x in 0..n {
            let p = img.pixel(x * img.width.max(1) / n, y * img.height.max(1) / n);
            let c = Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32) / 255.0;
            sum += if img.srgb { c.powf(2.2) } else { c };
        }
    }
    (sum / (n * n) as f32).extend(1.0)
}

/// Merges the instances into one mesh of at most `budget` triangles by vertex clustering:
/// the grid starts fine (a 512th of the content's size) and coarsens until the result fits.
/// Returns the mesh (flat-shaded, colors in the vertices) and the grid size, or None when
/// nothing is left.
fn cluster(sources: &[Source], instances: &[Instance], budget: usize) -> Option<(Mesh, f32)> {
    let world_pos = |n: &Instance, v: u32| {
        n.matrix
            .transform_point3(sources[n.source].positions[v as usize])
    };
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    for n in instances {
        for &p in &sources[n.source].positions {
            let w = n.matrix.transform_point3(p);
            lo = lo.min(w);
            hi = hi.max(w);
        }
    }
    if lo.x > hi.x {
        return None;
    }
    let mut grid = ((hi - lo).max_element() / 512.0).max(0.05);
    for _ in 0..24 {
        let mut ids: HashMap<(i32, i32, i32), u32> = HashMap::new();
        let mut sum: Vec<(Vec3, f32, Vec3, f32)> = Vec::new();
        let mut tris: HashMap<[u32; 3], ()> = HashMap::new();
        let mut order: Vec<[u32; 3]> = Vec::new();
        let mut over = false;
        'all: for n in instances {
            let s = &sources[n.source];
            for t in s.indices.as_chunks::<3>().0 {
                let p = [world_pos(n, t[0]), world_pos(n, t[1]), world_pos(n, t[2])];
                let area = (p[1] - p[0]).cross(p[2] - p[0]).length() * 0.5;
                let mut c = [0u32; 3];
                for k in 0..3 {
                    let cell = (p[k] / grid).floor();
                    let key = (cell.x as i32, cell.y as i32, cell.z as i32);
                    let next = sum.len() as u32;
                    let id = *ids.entry(key).or_insert(next);
                    if id == next {
                        sum.push((Vec3::ZERO, 0.0, Vec3::ZERO, 0.0));
                    }
                    let e = &mut sum[id as usize];
                    e.0 += p[k];
                    e.1 += 1.0;
                    let color = n.color.truncate() * s.colors[t[k] as usize];
                    e.2 += color * area.max(1e-6);
                    e.3 += area.max(1e-6);
                    c[k] = id;
                }
                if c[0] == c[1] || c[1] == c[2] || c[0] == c[2] {
                    continue;
                }
                // The same corners in the same turn are one triangle; front and back stay two.
                let r = (0..3).min_by_key(|&k| c[k]).unwrap_or(0);
                let canon = [c[r], c[(r + 1) % 3], c[(r + 2) % 3]];
                if tris.insert(canon, ()).is_none() {
                    order.push(canon);
                    if order.len() > budget {
                        over = true;
                        break 'all;
                    }
                }
            }
        }
        if over {
            grid *= 1.5;
            continue;
        }
        if order.is_empty() {
            return None;
        }
        let corner = |id: u32| {
            let e = &sum[id as usize];
            (e.0 / e.1, e.2 / e.3)
        };
        let mut vertices = Vec::with_capacity(order.len() * 3);
        for t in &order {
            let [(a, ca), (b, cb), (c, cc)] = [corner(t[0]), corner(t[1]), corner(t[2])];
            let normal = (b - a).cross(c - a).normalize_or(Vec3::Y);
            for (p, col) in [(a, ca), (b, cb), (c, cc)] {
                vertices.push(Vertex::new(p, normal, Vec2::ZERO).with_color(col.extend(1.0)));
            }
        }
        let indices = (0..vertices.len() as u32).collect();
        return Some((Mesh::new(vertices, indices), grid));
    }
    None
}

fn cache_path(settings: &HlodSettings, key: &str, version: u64) -> PathBuf {
    let safe: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    settings
        .cache_dir
        .join(format!("{safe}-{version:016x}.hlod"))
}

/// Bumped whenever proxies come out differently, so caches of the old look are made again.
const MAGIC: &[u8; 8] = b"HLOD0002";

fn write_mesh(path: &std::path::Path, mesh: &Mesh) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut bytes = Vec::with_capacity(16 + mesh.vertices.len() * 48 + mesh.indices.len() * 4);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&(mesh.vertices.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(mesh.indices.len() as u32).to_le_bytes());
    bytes.extend_from_slice(bytemuck::cast_slice(&mesh.vertices));
    bytes.extend_from_slice(bytemuck::cast_slice(&mesh.indices));
    // Written whole, then renamed: a run killed half way never leaves a torn proxy.
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path)
}

fn read_mesh(path: &std::path::Path) -> std::io::Result<Mesh> {
    let bytes = std::fs::read(path)?;
    let bad = || std::io::Error::new(std::io::ErrorKind::InvalidData, "not a proxy");
    if bytes.len() < 16 || &bytes[..8] != MAGIC {
        return Err(bad());
    }
    let nv = u32::from_le_bytes(bytes[8..12].try_into().map_err(|_| bad())?) as usize;
    let ni = u32::from_le_bytes(bytes[12..16].try_into().map_err(|_| bad())?) as usize;
    let vsize = std::mem::size_of::<Vertex>();
    let (v0, i0) = (16, 16 + nv * vsize);
    if bytes.len() != i0 + ni * 4 {
        return Err(bad());
    }
    let vertices: Vec<Vertex> = bytes[v0..i0]
        .chunks_exact(vsize)
        .map(bytemuck::pod_read_unaligned)
        .collect();
    let indices: Vec<u32> = bytes[i0..]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect();
    if indices.iter().any(|&i| i as usize >= nv) {
        return Err(bad());
    }
    Ok(Mesh::new(vertices, indices))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::{StreamCell, StreamCellsExt, StreamingBudget, StreamingPlugin};
    use engine_render::Shape;
    use engine_world::StreamingSource;

    fn app(dir: &std::path::Path) -> App {
        let mut app = App::new();
        app.add_plugins((engine_world::WorldPlugin, StreamingPlugin));
        app.init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Assets<Image>>();
        let mut settings = app.world.resource_mut::<HlodSettings>();
        settings.settle_frames = 2;
        settings.cache_dir = dir.to_path_buf();
        settings.bake = false;
        let mut budget = app.world.resource_mut::<StreamingBudget>();
        budget.fade_frames = 1;
        budget.fade_s = 0.05;
        app
    }

    fn place(app: &mut App, e: Entity, x: f64) {
        let t = Transform::from_translation(DVec3::new(x, 0.0, 0.0));
        app.world
            .entity_mut(e)
            .insert((t, GlobalTransform::from(t)));
    }

    /// A hut of two boxes and a cart (a dynamic body) far off to one side.
    fn hut(world: &mut World) {
        let red = StandardMaterial {
            base_color: [0.8, 0.1, 0.1, 1.0],
            ..Default::default()
        };
        for x in [-4.0, 4.0] {
            let t = Transform::from_xyz(1000.0 + x, 2.0, 0.0);
            world.spawn((
                t,
                GlobalTransform::from(t),
                MeshRenderer::new(
                    Shape::Cuboid {
                        size: [3.0, 4.0, 3.0],
                    },
                    red,
                ),
            ));
        }
        let t = Transform::from_xyz(1000.0, 0.5, 40.0);
        world.spawn((
            t,
            GlobalTransform::from(t),
            MeshRenderer::new(
                Shape::Cuboid { size: [1.0; 3] },
                StandardMaterial::default(),
            ),
            RigidBody::default(),
        ));
    }

    fn until(app: &mut App, mut done: impl FnMut(&mut App) -> bool) {
        for _ in 0..400 {
            app.update();
            if done(app) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("never happened");
    }

    fn proxies(app: &mut App) -> Vec<Entity> {
        app.world
            .query_filtered::<Entity, With<HlodProxy>>()
            .iter(&app.world)
            .collect()
    }

    #[test]
    fn a_cell_leaves_a_proxy_that_gives_way_when_it_returns() {
        let dir = tempdir("swap");
        let mut app = app(&dir);
        app.stream_cell(
            StreamCell::new("hut", DVec3::new(1000.0, 0.0, 0.0), 10.0, hut).load_within(50.0),
        );
        let player = app
            .world
            .spawn(StreamingSource {
                lookahead_s: 0.0,
                ..Default::default()
            })
            .id();
        place(&mut app, player, 1000.0);
        until(&mut app, |app| {
            app.world.resource::<Proxies>().triangles()[0] > 0
        });
        let mesh = {
            let proxies = app.world.resource::<Proxies>();
            proxies.slots[0].mesh.unwrap()
        };
        let mesh = app
            .world
            .resource::<Assets<Mesh>>()
            .get(mesh)
            .unwrap()
            .clone();
        // The two boxes, in the cell's space and red; the cart moves, so it isn't there.
        let aabb = mesh.aabb();
        assert!(aabb.min.x < -4.0 && aabb.max.x > 4.0, "{aabb:?}");
        assert!(aabb.max.z < 5.0, "the cart stayed out: {aabb:?}");
        assert!(
            mesh.vertices
                .iter()
                .all(|v| v.color[0] > 0.7 && v.color[1] < 0.2)
        );
        assert!(proxies(&mut app).is_empty(), "no proxy while the hut is in");

        // Away, the hut goes and its proxy stands in.
        place(&mut app, player, 0.0);
        app.update();
        let shown = proxies(&mut app);
        assert_eq!(shown.len(), 1);
        let at = app.world.get::<Transform>(shown[0]).unwrap().translation;
        assert_eq!(at, DVec3::new(1000.0, 0.0, 0.0));

        // Back, the hut fades in over it and the proxy goes.
        place(&mut app, player, 1000.0);
        app.update();
        assert_eq!(
            proxies(&mut app).len(),
            1,
            "kept while the hut loads hidden"
        );
        until(&mut app, |app| proxies(app).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_leaving_cell_fades_out_over_its_proxy_then_despawns() {
        let dir = tempdir("leave");
        let mut app = app(&dir);
        app.world.resource_mut::<StreamingBudget>().fade_s = 0.3;
        app.stream_cell(
            StreamCell::new("hut", DVec3::new(1000.0, 0.0, 0.0), 10.0, hut).load_within(50.0),
        );
        let player = app.world.spawn(StreamingSource::default()).id();
        place(&mut app, player, 1000.0);
        until(&mut app, |app| {
            app.world.resource::<Proxies>().triangles()[0] > 0
        });
        let fades = |app: &mut App| -> Vec<f32> {
            app.world
                .query_filtered::<&DitherFade, Without<HlodProxy>>()
                .iter(&app.world)
                .map(|d| d.0)
                .collect()
        };
        place(&mut app, player, 0.0);
        app.update();
        let state = |app: &App| app.world.resource::<StreamedCells>().cells[0].state;
        assert_eq!(state(&app), CellState::Unloading);
        let proxy = proxies(&mut app)[0];
        let p = app.world.get::<DitherFade>(proxy).copied().unwrap().0;
        let cell = fades(&mut app);
        assert_eq!(cell.len(), 3, "the boxes and the cart fade out: {cell:?}");
        // Complementary patterns: what the cell hides, the proxy shows.
        assert!(
            p < 0.0 && cell.iter().all(|&c| (c - (1.0 + p)).abs() < 0.2),
            "{p} {cell:?}"
        );
        until(&mut app, |app| state(app) == CellState::Unloaded);
        assert!(fades(&mut app).is_empty(), "the hut is gone");
        assert_eq!(
            app.world.get::<DitherFade>(proxy),
            None,
            "the proxy is all in"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_versioned_proxy_is_there_before_its_cell_ever_loads() {
        let dir = tempdir("disk");
        let cell = || {
            StreamCell::new("hut", DVec3::new(1000.0, 0.0, 0.0), 10.0, hut)
                .load_within(50.0)
                .proxy_version(7)
        };
        {
            // A bake: every versioned cell loads once, in turn, and leaves its proxy on disk.
            let mut app = app(&dir);
            app.world.resource_mut::<HlodSettings>().bake = true;
            // Kept cells too: each goes out after its capture, so a bake holds one at a time.
            app.stream_cell(cell().keep_loaded());
            let player = app.world.spawn(StreamingSource::default()).id();
            place(&mut app, player, 0.0);
            until(&mut app, |app| app.world.resource::<Proxies>().baked);
            assert_eq!(
                app.world.resource::<StreamedCells>().cells[0].state,
                CellState::Unloaded
            );
            assert!(app.world.resource::<StreamedCells>().pins.is_empty());
        }
        assert!(dir.join(format!("hut-{:016x}.hlod", 7)).exists());

        let mut app = app(&dir);
        app.stream_cell(cell());
        let player = app.world.spawn(StreamingSource::default()).id();
        place(&mut app, player, 0.0);
        app.update();
        assert_eq!(
            proxies(&mut app).len(),
            1,
            "from disk, the hut never loaded"
        );
        assert_eq!(
            app.world.resource::<StreamedCells>().cells[0].state,
            CellState::Unloaded
        );

        // Another version is another cell's content: no proxy until it loads.
        let mut app = self::app(&dir);
        app.stream_cell(cell().proxy_version(8));
        let player = app.world.spawn(StreamingSource::default()).id();
        place(&mut app, player, 0.0);
        app.update();
        assert!(proxies(&mut app).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn clustering_fits_the_budget_and_keeps_the_shape() {
        // A 200 m wavy sheet of 80 000 triangles.
        let n = 200u32;
        let positions: Vec<Vec3> = (0..=n)
            .flat_map(|z| {
                (0..=n).map(move |x| Vec3::new(x as f32, (x as f32 * 0.1).sin() * 3.0, z as f32))
            })
            .collect();
        let mut indices = Vec::new();
        for z in 0..n {
            for x in 0..n {
                let i = z * (n + 1) + x;
                indices.extend([i, i + n + 1, i + 1, i + 1, i + n + 1, i + n + 2]);
            }
        }
        let colors = vec![Vec3::ONE; positions.len()];
        let sources = [Source {
            positions,
            colors,
            indices,
        }];
        let instances = [Instance {
            source: 0,
            matrix: Mat4::IDENTITY,
            color: Vec4::new(0.2, 0.5, 0.1, 1.0),
        }];
        let (mesh, grid) = cluster(&sources, &instances, 4000).unwrap();
        let tris = mesh.indices.len() / 3;
        assert!(tris <= 4000 && tris > 1000, "{tris}");
        let aabb = mesh.aabb();
        assert!(
            aabb.min.x < grid && aabb.max.x > 200.0 - grid,
            "{aabb:?} grid {grid}"
        );
        assert!(aabb.max.y > 2.0, "the waves are still there: {aabb:?}");
        assert!(mesh.vertices.iter().all(|v| v.normal[1] > 0.0), "faces up");
    }

    #[test]
    fn proxy_colors_follow_vertex_colors_textures_and_occlusion() {
        let mut images = Assets::<Image>::default();
        // Linear 0.5 grey once decoded (sRGB 186 ≈ 0.5^(1/2.2) × 255).
        let grey = images.add(Image::solid([186, 186, 186, 255], true));
        let ao = images.add(Image::solid([128, 255, 255, 255], false));
        let mut means = ImageMeans::default();
        means.refresh(&images, Instant::now() + std::time::Duration::from_secs(1));
        // The renderer frees the pixels later; the means stay.
        for h in [grey, ao] {
            images.get_mut(h).unwrap().data.clear();
        }
        let wall = StandardMaterial {
            base_color: [1.0, 0.8, 0.6, 1.0],
            base_color_texture: Some(grey),
            occlusion_texture: Some(ao),
            ..Default::default()
        };
        let c = albedo(&wall, &images, Some(&means));
        let occlusion = 1.0 - AMBIENT_SHARE * (1.0 - 128.0 / 255.0);
        for (got, want) in [(c.x, 1.0), (c.y, 0.8), (c.z, 0.6)] {
            let want = want * 0.5 * occlusion;
            assert!((got - want).abs() < 0.02, "{c} vs {want}");
        }
        // Pixels gone before anyone saw them: a typical surface, not white.
        let unseen = images.add(Image::solid([255; 4], true));
        images.get_mut(unseen).unwrap().data.clear();
        let lost = StandardMaterial {
            base_color_texture: Some(unseen),
            ..Default::default()
        };
        assert!(albedo(&lost, &images, Some(&means)).x < 0.5);

        // A quad tinted dark on one side: the proxy keeps the tint.
        let positions = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 10.0),
            Vec3::new(0.0, 0.0, 10.0),
        ];
        let colors = vec![Vec3::splat(0.2), Vec3::ONE, Vec3::ONE, Vec3::splat(0.2)];
        let sources = [Source {
            positions,
            colors,
            indices: vec![0, 2, 1, 0, 3, 2],
        }];
        let instances = [Instance {
            source: 0,
            matrix: Mat4::IDENTITY,
            color: Vec4::new(0.5, 0.5, 0.5, 1.0),
        }];
        let (mesh, _) = cluster(&sources, &instances, 100).unwrap();
        for v in &mesh.vertices {
            let want = if v.position[0] < 5.0 { 0.1 } else { 0.5 };
            assert!((v.color[0] - want).abs() < 1e-4, "{v:?}");
        }
    }

    #[test]
    fn a_torn_cache_file_is_ignored() {
        let dir = tempdir("torn");
        let path = dir.join("x.hlod");
        let mesh = Mesh::new(
            vec![Vertex::new(Vec3::ZERO, Vec3::Y, Vec2::ZERO); 3],
            vec![0, 1, 2],
        );
        write_mesh(&path, &mesh).unwrap();
        assert_eq!(read_mesh(&path).unwrap().indices, vec![0, 1, 2]);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() - 2]).unwrap();
        assert!(read_mesh(&path).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    fn tempdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("engine-hlod-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
