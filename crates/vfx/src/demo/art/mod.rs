//! Detailed props and materials for the Spell court, made procedurally (nothing to download or
//! load from disk).
//!
//! A [`Prop`] is a list of meshes in the prop's own space (meters, +Y up, origin on the
//! ground), each tagged with the [`Surface`] it is made of. [`spawn_prop`] builds a prop the
//! first time its key is used, registers its meshes and the surfaces' materials once (every
//! copy shares them) and spawns one entity per part. Without a renderer (tests that only run
//! the simulation) nothing is spawned.
//!
//! The meshes and the surfaces' textures are made on a worker ([`engine_core::off_frame`]):
//! the prop's anchor entity stands at once, its parts appear under it once built (in a
//! streamed cell, before the cell counts as loaded). Each prop and surface is built once
//! however many copies wait for it.
//!
//! - [`materials`]: physically based materials per [`Surface`], with generated textures.
//! - [`meshes`]: mesh building blocks (lathe, beveled boxes, rocks, paving, sweeps, …).
//! - [`props`]: the Spell court's props, built from those blocks.

pub mod materials;
pub mod meshes;
pub mod props;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_render::{
    Image, MaterialRef, Mesh, MeshRef, MeshRenderer, RenderStats, StandardMaterial,
};

/// What a part of a prop is made of. Each surface maps to one shared material.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Surface {
    /// Worn flagstones of the court floor.
    Flagstone,
    /// Dressed, carved light granite: obelisks, portal, dais, fountain, wall.
    CarvedStone,
    /// Dark basalt for trims, plinths and inlays.
    DarkStone,
    /// Rough natural rock: pond rim, boulders, thrown stones, the golem.
    Rock,
    /// Weathered wood: plates, the anvil's stump, torch shafts.
    Wood,
    /// Forged, slightly rusty iron: anvil, braziers, bands.
    Iron,
    /// Aged bronze with green patina: fittings, rings, the fountain spout.
    Bronze,
    Mud,
    Snow,
    Sand,
    Dirt,
    /// Clear water surface (fountain basin, pond, puddles).
    Water,
    /// Shrub and ivy leaves.
    Leaves,
    /// Moss and grass tufts in cracks.
    Moss,
    /// Violet glowing rune inlays (portal).
    RuneArcane,
    /// Blue-white glowing rune inlays (obelisks).
    RuneStorm,
    /// Green glowing rune inlays (healing dais).
    RuneLife,
    /// Orange glowing inlays (caster, braziers' embers).
    RuneEmber,
    /// Faceted magic crystal (obelisk caps, the caster's focus).
    Crystal,
    /// Burning charcoal with glowing fissures (the braziers' fuel).
    Coals,
}

/// One mesh of a prop and what it is made of.
#[derive(Debug, Clone)]
pub struct Part {
    pub mesh: Mesh,
    pub surface: Surface,
}

/// A prop: meshes in the prop's own space.
#[derive(Debug, Clone, Default)]
pub struct Prop {
    pub parts: Vec<Part>,
}

impl Prop {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a part.
    pub fn with(mut self, surface: Surface, mesh: Mesh) -> Self {
        self.push(surface, mesh);
        self
    }

    /// Add a part, merging it into an existing part of the same surface (fewer draw calls).
    pub fn push(&mut self, surface: Surface, mesh: Mesh) {
        if let Some(p) = self.parts.iter_mut().find(|p| p.surface == surface) {
            p.mesh.merge(&mesh);
        } else {
            self.parts.push(Part { mesh, surface });
        }
    }

    pub fn triangle_count(&self) -> usize {
        self.parts.iter().map(|p| p.mesh.triangle_count()).sum()
    }
}

/// Meshes and materials already registered, so every copy of a prop shares them.
#[derive(Resource)]
pub struct ArtCache {
    props: HashMap<String, Vec<(Handle<Mesh>, Surface)>>,
    materials: HashMap<Surface, Handle<StandardMaterial>>,
    /// Glowing materials and the emission strength each was authored with (for dusk).
    glowing: Vec<(Handle<StandardMaterial>, f32)>,
    /// Factor on authored emission now in effect (see [`follow_exposure`]).
    glow: f32,
    /// Props and surfaces being built on workers.
    building: Building,
}

impl Default for ArtCache {
    fn default() -> Self {
        Self {
            props: HashMap::new(),
            materials: HashMap::new(),
            glowing: Vec::new(),
            glow: 1.0,
            building: Building::default(),
        }
    }
}

/// A value built once on a worker and taken once on the main thread; every job that wants it
/// shares the slot, and the first to wait on it builds it.
type Once<T> = Arc<OnceLock<Mutex<Option<T>>>>;
/// A built prop: its surfaces (read on the worker) and the prop, until it is registered.
type BuiltProp = (Vec<Surface>, Mutex<Option<Prop>>);
type BuiltSurface = (StandardMaterial, materials::textures::TextureImages);

/// Props and surfaces being built on workers, shared by the jobs that want them.
#[derive(Clone, Default)]
struct Building {
    props: Arc<Mutex<HashMap<String, Arc<OnceLock<BuiltProp>>>>>,
    surfaces: Arc<Mutex<HashMap<Surface, Once<BuiltSurface>>>>,
}

impl Building {
    /// The prop `key`, built by `build` here unless another job built it first.
    fn prop(&self, key: &str, build: impl FnOnce() -> Prop) -> Arc<OnceLock<BuiltProp>> {
        let slot = self
            .props
            .lock()
            .unwrap()
            .entry(key.to_string())
            .or_default()
            .clone();
        slot.get_or_init(|| {
            let prop = build();
            let surfaces = prop.parts.iter().map(|p| p.surface).collect();
            (surfaces, Mutex::new(Some(prop)))
        });
        slot
    }

    /// `surface`'s material and textures, generated here unless another job did it first.
    fn surface(&self, surface: Surface) -> Once<BuiltSurface> {
        let slot = self
            .surfaces
            .lock()
            .unwrap()
            .entry(surface)
            .or_default()
            .clone();
        slot.get_or_init(|| Mutex::new(Some(materials::generate(surface, 0))));
        slot
    }

    /// The generated `surface`, if a worker made it and nobody has taken it yet.
    fn take_surface(&self, surface: Surface) -> Option<BuiltSurface> {
        let slot = self.surfaces.lock().unwrap().get(&surface)?.clone();
        slot.get()?.lock().unwrap().take()
    }
}

/// Whether the renderer's asset stores are here (art is only made for one).
fn has_renderer(world: &World) -> bool {
    world.contains_resource::<Assets<Mesh>>()
        && world.contains_resource::<Assets<StandardMaterial>>()
        && world.contains_resource::<Assets<Image>>()
}

/// Exposure that glowing art (rune inlays, crystals, burning coals) is authored for: dusk.
pub const GLOW_AUTHORED_EV100: f32 = 5.5;

/// How much brighter than authored glowing art is drawn at `ev100`: it follows the camera's
/// exposure as particle glow does (`weight` is [`VfxSettings::glow_exposure`]), so it keeps its
/// color at night and still reads by day.
///
/// [`VfxSettings::glow_exposure`]: crate::VfxSettings::glow_exposure
pub fn glow_factor(ev100: f32, weight: f32) -> f32 {
    use engine_render::frame::ev100_to_exposure;
    let at = |ev: f32| crate::feature::glow_scale(ev100_to_exposure(ev), weight);
    at(ev100) / at(GLOW_AUTHORED_EV100)
}

/// Each frame, scale the glowing materials' emission to the camera's exposure (last frame's,
/// from [`RenderStats`]). Without a renderer nothing changes.
pub fn follow_exposure(
    stats: Option<Res<RenderStats>>,
    settings: Res<crate::VfxSettings>,
    cache: Option<ResMut<ArtCache>>,
    materials: Option<ResMut<Assets<StandardMaterial>>>,
) {
    let (Some(stats), Some(mut cache), Some(mut materials)) = (stats, cache, materials) else {
        return;
    };
    if stats.width == 0 {
        return;
    }
    let k = glow_factor(stats.ev100, settings.glow_exposure);
    if (k / cache.glow - 1.0).abs() < 0.02 {
        return;
    }
    cache.glow = k;
    for (handle, authored) in &cache.glowing {
        if let Some(m) = materials.get_mut(*handle) {
            m.emissive_strength = authored * k;
        }
    }
}

/// Spawn the prop registered as `key` at `at`, building it with `build` the first time the key
/// is used. Returns nothing: the prop's anchor appears when the command runs, its parts once
/// they are built (see the module docs).
pub fn spawn_prop(
    commands: &mut Commands,
    key: impl Into<String>,
    at: Transform,
    build: impl FnOnce() -> Prop + Send + 'static,
) {
    let key = key.into();
    commands.queue(move |world: &mut World| {
        if !has_renderer(world) {
            return;
        }
        // Spawned now so the prop follows world shifts while its parts build.
        let anchor = world.spawn(at).id();
        place_under(world, key, anchor, build);
    });
}

/// Spawn prop `key`'s parts as children of `parent`, building what isn't registered yet on a
/// worker first. Does nothing without a renderer, or if `parent` is gone by then.
pub fn place_under(
    world: &mut World,
    key: String,
    parent: Entity,
    build: impl FnOnce() -> Prop + Send + 'static,
) {
    if !has_renderer(world) {
        return;
    }
    world.init_resource::<ArtCache>();
    let cache = world.resource::<ArtCache>();
    let known: Option<Vec<Surface>> = cache
        .props
        .get(&key)
        .map(|parts| parts.iter().map(|p| p.1).collect());
    let made: HashSet<Surface> = cache.materials.keys().copied().collect();
    if known
        .as_ref()
        .is_some_and(|s| s.iter().all(|s| made.contains(s)))
    {
        spawn_parts(world, &key, parent, None);
        return;
    }
    let building = cache.building.clone();
    let name = format!("vfx prop {key}");
    engine_core::off_frame::off_frame(
        world,
        name,
        move || {
            let prop = known.is_none().then(|| building.prop(&key, build));
            let surfaces = known.unwrap_or_else(|| prop.as_ref().unwrap().get().unwrap().0.clone());
            for s in surfaces.into_iter().filter(|s| !made.contains(s)) {
                building.surface(s);
            }
            (key, prop)
        },
        move |world, (key, prop)| {
            if world.get_entity(parent).is_ok() {
                spawn_parts(world, &key, parent, prop);
            }
        },
    );
}

/// Register `key` (from its worker-built `prop`, when it isn't yet) and spawn its parts under
/// `parent`.
fn spawn_parts(
    world: &mut World,
    key: &str,
    parent: Entity,
    prop: Option<Arc<OnceLock<BuiltProp>>>,
) {
    let take = || {
        prop.and_then(|p| p.get()?.1.lock().unwrap().take())
            .expect("a prop is registered by the first job that takes it")
    };
    for (mesh, material) in register(world, key, take) {
        world.spawn((
            Transform::default(),
            MeshRenderer::new(MeshRef::Handle(mesh), MaterialRef::Handle(material)),
            ChildOf(parent),
        ));
    }
}

/// Register a prop's meshes and materials (once per key) and return them.
pub fn register(
    world: &mut World,
    key: &str,
    build: impl FnOnce() -> Prop,
) -> Vec<(Handle<Mesh>, Handle<StandardMaterial>)> {
    world.init_resource::<ArtCache>();
    world.resource_scope(|world, mut cache: Mut<ArtCache>| {
        if !cache.props.contains_key(key) {
            let prop = build();
            let mut meshes = world.resource_mut::<Assets<Mesh>>();
            let parts = prop
                .parts
                .into_iter()
                .map(|p| (meshes.add(p.mesh), p.surface))
                .collect();
            cache.props.insert(key.to_string(), parts);
        }
        let parts = cache.props[key].clone();
        parts
            .into_iter()
            .map(|(mesh, surface)| {
                let material = match cache.materials.get(&surface) {
                    Some(m) => *m,
                    None => {
                        let glow = cache.glow;
                        // Made on a worker when one built it (see `place_under`).
                        let (material, set) = cache
                            .building
                            .take_surface(surface)
                            .unwrap_or_else(|| materials::generate(surface, 0));
                        let (m, authored) =
                            world.resource_scope(|world, mut images: Mut<Assets<Image>>| {
                                let mut made = materials::attach(material, set, &mut images);
                                let authored = made.emissive_strength;
                                made.emissive_strength *= glow;
                                let m = world.resource_mut::<Assets<StandardMaterial>>().add(made);
                                (m, authored)
                            });
                        if authored > 0.0 {
                            cache.glowing.push((m, authored));
                        }
                        cache.materials.insert(surface, m);
                        m
                    }
                };
                (mesh, material)
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_assets::AssetAppExt;
    use engine_render::Shape;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn meshes(app: &mut App) -> Vec<Entity> {
        app.world
            .query_filtered::<Entity, With<MeshRenderer>>()
            .iter(&app.world)
            .collect()
    }

    #[test]
    fn copies_of_a_prop_wait_for_one_build_on_a_worker_then_stand_under_their_anchors() {
        let mut app = App::new();
        app.init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .init_asset::<Image>();
        // The rock's material is made already, so the test builds no textures.
        let rock = app
            .world
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        app.world.init_resource::<ArtCache>();
        app.world
            .resource_mut::<ArtCache>()
            .materials
            .insert(Surface::Rock, rock);
        let builds = Arc::new(AtomicUsize::new(0));
        let spawn = |app: &mut App, x: f64| {
            let b = builds.clone();
            let mut commands = app.world.commands();
            spawn_prop(
                &mut commands,
                "test.rock",
                Transform::from_translation(DVec3::new(x, 0.0, 0.0)),
                move || {
                    b.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    Prop::new().with(Surface::Rock, Shape::Cuboid { size: [1.0; 3] }.mesh())
                },
            );
            app.world.flush();
        };
        for x in [0.0, 10.0, 20.0] {
            spawn(&mut app, x);
        }
        // The anchors stand at once; the parts wait for the build.
        assert!(meshes(&mut app).is_empty());
        engine_core::off_frame::finish(&mut app.world, None);
        let parts = meshes(&mut app);
        assert_eq!(parts.len(), 3);
        assert_eq!(
            builds.load(Ordering::SeqCst),
            1,
            "built once for every copy"
        );
        assert_eq!(app.world.resource::<Assets<Mesh>>().len(), 1);
        let mut xs: Vec<f64> = parts
            .iter()
            .map(|&e| {
                let parent = app.world.get::<ChildOf>(e).unwrap().parent();
                app.world.get::<Transform>(parent).unwrap().translation.x
            })
            .collect();
        xs.sort_by(f64::total_cmp);
        assert_eq!(xs, [0.0, 10.0, 20.0]);
        // A known prop with its materials made is placed at once.
        spawn(&mut app, 30.0);
        assert_eq!(meshes(&mut app).len(), 4);
        assert_eq!(builds.load(Ordering::SeqCst), 1);
    }
}
