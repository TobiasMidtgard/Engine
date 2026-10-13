//! The demo scenery: the race track (asphalt, curbs, run-off, barriers, grandstand, pits,
//! paddock with team transporters, start gantry, kicker, drift pad), the airfield (runway
//! with ICAO markings and lights, taxiways, aprons, hangar, fire station, control tower,
//! terminal, ground equipment, windsocks) and the spaceport (landing pad, flame grate,
//! lights, propellant tanks, service tower, water tower, hangars, blast walls, floodlights).
//!
//! Everything is procedural: textures are baked at startup ([`tex`]), meshes are built from
//! the same layout the demos use for their colliders, so drawn surfaces sit exactly on the
//! physics. Each zone becomes a few large merged meshes (one per material) plus instanced
//! props, all registered in `Assets` under `vehicles/scenery/…` keys.
//!
//! The demos in `demo.rs` call [`track::spawn`], [`airfield::spawn`] and
//! [`spaceport::spawn`]; each queues the build to run once the world is available, and
//! skips it when there is no renderer (no `Assets<Mesh>`). Their colliders
//! ([`track::solids`], [`airfield::solids`], [`spaceport::solids`], spawned by
//! [`solid::spawn`]) don't need a renderer.

pub mod airfield;
pub mod buildings;
pub mod font;
pub mod kit;
pub mod noise;
pub mod solid;
pub mod spaceport;
pub mod tex;
pub mod track;

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};

use engine_assets::{Assets, Handle};
use engine_core::ecs::schedule::Schedules;
use engine_core::math::{DVec3, Quat};
use engine_core::prelude::*;
use engine_render::{AlphaMode, Image, MaterialRef, Mesh, MeshRef, MeshRenderer, StandardMaterial};

/// The shared scenery materials.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Surface {
    /// Track asphalt (8 m tile).
    Asphalt,
    /// Fresh asphalt in repaired patches.
    AsphaltPatch,
    /// Grooved runway asphalt.
    Runway,
    /// Cast concrete (4 m tile).
    Concrete,
    /// Concrete pavement slabs, 5 m panels with joints.
    Slabs,
    /// Road paint (color from vertex colors) on track asphalt UVs.
    PaintAsphalt,
    /// Paint on runway UVs.
    PaintRunway,
    /// Paint on slab UVs.
    PaintSlabs,
    /// Paint on cast concrete UVs.
    PaintConcrete,
    /// Painted glyphs from the glyph atlas (UVs into the atlas).
    Glyphs,
    /// Curb paint (red and white from vertex colors).
    Curb,
    Gravel,
    Dirt,
    /// Mown grass (only for [`grass_ground`]; the game draws its own terrain).
    Grass,
    /// Galvanized steel (barriers, posts, fences).
    Galvanized,
    /// Painted steel and panels with grime; color from vertex colors.
    Painted,
    /// Ribbed steel cladding; color from vertex colors.
    Cladding,
    /// Tinted architectural glass.
    Glass,
    /// Tires, belts, rubber.
    Rubber,
    /// Plastic seats and fittings; color from vertex colors.
    Plastic,
    /// Chain-link mesh.
    Fence,
    /// Steel bar grating (flame grate, drain covers).
    Grate,
    /// Tire marks on asphalt and concrete.
    Skid,
    /// Tire marks on dirt (brown, softer).
    SkidDirt,
    /// Crack sealant and joint tar.
    Tar,
    /// Rocket soot.
    Soot,
    /// Sponsor boards and signs (UVs into the sign atlas).
    Signs,
    /// Dark interiors (open hangar and garage doors).
    Interior,
    /// Signal lenses (bright enough to read in daylight): warm white, red, green, amber,
    /// blue; an unlit red lens and an unlit clear one (headlamps with their reflector).
    LensWhite,
    LensRed,
    LensGreen,
    LensAmber,
    LensBlue,
    LensOff,
    LensClear,
    /// Area lighting: floodlight faces and ceiling strips, lit but not dazzling.
    Lamp,
    /// Monitors and displays.
    Screen,
}

/// Texture sizes. 2048² for the hero surfaces seen up close (track asphalt, sign text),
/// 1024² elsewhere. The runway and the pad slabs tile over hundreds of meters and are seen
/// from a cockpit's height at the closest, where 1024² reads the same at a quarter of the
/// memory (2048² sets cost 48 MB of pixels each, again on the GPU).
const HERO: u32 = 2048;
const STD: u32 = 1024;

/// Tile sizes (meters) the surfaces' UVs are divided by.
pub const ASPHALT_TILE: f32 = 8.0;
pub const RUNWAY_TILE: f32 = 8.0;
pub const CONCRETE_TILE: f32 = 4.0;
pub const SLAB_PANEL: f32 = 5.0;
pub const GRAVEL_TILE: f32 = 2.0;
pub const DIRT_TILE: f32 = 4.0;
pub const METAL_TILE: f32 = 2.0;
pub const GRIME_TILE: f32 = 4.0;
pub const CLADDING_TILE: f32 = 1.0;
pub const FENCE_TILE: f32 = 0.5;
pub const GRATE_TILE: f32 = 1.0;
pub const GRASS_TILE: f32 = 3.0;
pub const SKID_LENGTH: f32 = 8.0;

/// Build context for one zone: spawns entities at `origin` and hands out shared materials.
pub struct Scene<'w> {
    pub world: &'w mut World,
    /// World position of the zone's local origin (mesh vertices are relative to it).
    pub origin: DVec3,
    /// Rotation from zone-local axes to world axes.
    pub rotation: Quat,
    /// Prefix for this zone's asset keys.
    pub key: String,
    materials: HashMap<Surface, Handle<StandardMaterial>>,
    /// Triangles spawned (instances counted once per instance).
    pub triangles: usize,
    pub entities: usize,
}

fn tex_key(name: &str, map: &str) -> String {
    format!("vehicles/scenery/tex/{name}/{map}")
}

/// Texel size of the preview maps a material starts with while its full maps bake.
const PREVIEW: u32 = 32;

type Handles = [Option<Handle<Image>>; 3];

struct BakeJob {
    handles: Handles,
    make: Box<dyn FnOnce() -> tex::Maps + Send>,
}

struct Baked {
    handles: Handles,
    maps: tex::Maps,
}

/// Bakes full-size scenery textures on a worker thread. Materials start with a small preview
/// of the same recipe and switch to the full maps when [`apply_baked`] (added to the app's
/// `First` schedule on first use) finds them done.
#[derive(Resource)]
pub struct TextureBaker {
    jobs: Mutex<Sender<BakeJob>>,
    done: Mutex<Receiver<Baked>>,
    /// Jobs sent and not applied yet.
    pub pending: usize,
}

impl TextureBaker {
    fn new() -> Self {
        let (job_tx, job_rx) = channel::<BakeJob>();
        let (done_tx, done_rx) = channel::<Baked>();
        let worker = std::thread::Builder::new()
            .name("scenery textures".into())
            .spawn(move || {
                for job in job_rx {
                    let maps = (job.make)();
                    let done = Baked {
                        handles: job.handles,
                        maps,
                    };
                    if done_tx.send(done).is_err() {
                        break;
                    }
                }
            });
        if worker.is_err() {
            engine_core::log::warn!("scenery: no texture worker; textures stay at preview size");
        }
        Self {
            jobs: Mutex::new(job_tx),
            done: Mutex::new(done_rx),
            pending: 0,
        }
    }
}

fn install_baker(world: &mut World) {
    if world.contains_resource::<TextureBaker>() {
        return;
    }
    world.insert_resource(TextureBaker::new());
    // Poll from a schedule that is not running right now (a running schedule is taken out
    // of `Schedules` while it runs).
    if let Some(mut schedules) = world.get_resource_mut::<Schedules>() {
        if schedules.get(First).is_some() {
            schedules.add_systems(First, apply_baked);
        } else if schedules.get(Last).is_some() {
            schedules.add_systems(Last, apply_baked);
        }
    }
}

fn store(world: &mut World, done: Baked) {
    let mut images = world.resource_mut::<Assets<Image>>();
    let [base, normal, orm] = done.handles;
    let tex::Maps {
        base: b,
        normal: n,
        orm: o,
    } = done.maps;
    if let Some(h) = base {
        images.set(h, b);
    }
    if let (Some(h), Some(n)) = (normal, n) {
        images.set(h, n);
    }
    if let (Some(h), Some(o)) = (orm, o) {
        images.set(h, o);
    }
}

/// Swap in every texture the worker finished (a system; also safe to call by hand).
pub fn apply_baked(world: &mut World) {
    let finished: Vec<Baked> = match world.get_resource::<TextureBaker>() {
        Some(baker) if baker.pending > 0 => match baker.done.lock() {
            Ok(rx) => rx.try_iter().collect(),
            Err(_) => Vec::new(),
        },
        _ => return,
    };
    let n = finished.len();
    for done in finished {
        store(world, done);
    }
    if let Some(mut baker) = world.get_resource_mut::<TextureBaker>() {
        baker.pending = baker.pending.saturating_sub(n);
    }
}

/// Block until every queued scenery texture is baked and in place (tests and screenshots).
/// Returns the seconds spent waiting.
pub fn finish_textures(world: &mut World) -> f64 {
    let started = std::time::Instant::now();
    loop {
        let next = match world.get_resource::<TextureBaker>() {
            Some(baker) if baker.pending > 0 => match baker.done.lock() {
                Ok(rx) => rx.recv().ok(),
                Err(_) => None,
            },
            _ => None,
        };
        let Some(done) = next else { break };
        store(world, done);
        if let Some(mut baker) = world.get_resource_mut::<TextureBaker>() {
            baker.pending = baker.pending.saturating_sub(1);
        }
    }
    started.elapsed().as_secs_f64()
}

/// Register the maps of one texture under `vehicles/scenery/tex/<name>/…` and return (base,
/// normal, orm) handles. A [`PREVIEW`]-sized bake goes in at once and the `full`-sized one
/// is queued on the [`TextureBaker`]; `make(px)` bakes the recipe at `px` texels.
fn textures(
    world: &mut World,
    name: &str,
    full: u32,
    make: impl Fn(u32) -> tex::Maps + Send + 'static,
) -> (Handle<Image>, Option<Handle<Image>>, Option<Handle<Image>>) {
    {
        let images = world.resource::<Assets<Image>>();
        if let Some(base) = images.by_key(&tex_key(name, "base")) {
            return (
                base,
                images.by_key(&tex_key(name, "normal")),
                images.by_key(&tex_key(name, "orm")),
            );
        }
    }
    install_baker(world);
    let preview = make(PREVIEW.min(full));
    let mut images = world.resource_mut::<Assets<Image>>();
    let base = images.insert_keyed(&tex_key(name, "base"), preview.base);
    let normal = preview
        .normal
        .map(|n| images.insert_keyed(&tex_key(name, "normal"), n));
    let orm = preview
        .orm
        .map(|o| images.insert_keyed(&tex_key(name, "orm"), o));
    if full > PREVIEW {
        let job = BakeJob {
            handles: [Some(base), normal, orm],
            make: Box::new(move || make(full)),
        };
        let mut baker = world.resource_mut::<TextureBaker>();
        let sent = baker
            .jobs
            .lock()
            .map(|tx| tx.send(job).is_ok())
            .unwrap_or(false);
        if sent {
            baker.pending += 1;
        }
    }
    (base, normal, orm)
}

fn pbr(
    base: Handle<Image>,
    normal: Option<Handle<Image>>,
    orm: Option<Handle<Image>>,
) -> StandardMaterial {
    StandardMaterial {
        base_color: [1.0; 4],
        perceptual_roughness: 1.0,
        metallic: 0.0,
        base_color_texture: Some(base),
        normal_texture: normal,
        metallic_roughness_texture: orm,
        occlusion_texture: orm,
        ..Default::default()
    }
}

fn tiled(mut m: StandardMaterial, tile: f32) -> StandardMaterial {
    m.uv_scale = [1.0 / tile, 1.0 / tile];
    m
}

fn lens(color: [f32; 3], strength: f32) -> StandardMaterial {
    let mut m = StandardMaterial::color(color[0] * 0.3, color[1] * 0.3, color[2] * 0.3)
        .with_roughness(0.08)
        .with_emissive(color, strength);
    m.reflectance = 0.7;
    m.receives_wetness = false;
    m
}

impl<'w> Scene<'w> {
    /// A build context, or `None` when the world has no renderer assets (simulation-only
    /// apps and tests skip the drawing).
    pub fn new(world: &'w mut World, origin: DVec3, zone: &str) -> Option<Self> {
        if !world.contains_resource::<Assets<Mesh>>()
            || !world.contains_resource::<Assets<Image>>()
            || !world.contains_resource::<Assets<StandardMaterial>>()
        {
            return None;
        }
        Some(Self {
            world,
            origin,
            rotation: Quat::IDENTITY,
            key: format!(
                "vehicles/scenery/{zone}@{:.0},{:.0},{:.0}",
                origin.x, origin.y, origin.z
            ),
            materials: HashMap::new(),
            triangles: 0,
            entities: 0,
        })
    }

    /// The shared material for `s` (baking its textures the first time).
    pub fn material(&mut self, s: Surface) -> Handle<StandardMaterial> {
        if let Some(h) = self.materials.get(&s) {
            return *h;
        }
        let key = format!("vehicles/scenery/mat/{s:?}");
        if let Some(h) = self
            .world
            .resource::<Assets<StandardMaterial>>()
            .by_key(&key)
        {
            self.materials.insert(s, h);
            return h;
        }
        let m = self.make_material(s);
        let h = self
            .world
            .resource_mut::<Assets<StandardMaterial>>()
            .insert_keyed(&key, m);
        self.materials.insert(s, h);
        h
    }

    fn make_material(&mut self, s: Surface) -> StandardMaterial {
        let w = &mut *self.world;
        let asphalt = |w: &mut World| {
            textures(w, "asphalt", HERO, |px| {
                tex::asphalt(px, ASPHALT_TILE, None, 11)
            })
        };
        let runway = |w: &mut World| {
            textures(w, "runway", STD, |px| {
                tex::asphalt(px, RUNWAY_TILE, Some(0.038), 12)
            })
        };
        let concrete = |w: &mut World| {
            textures(w, "concrete", STD, |px| {
                tex::concrete(px, CONCRETE_TILE, 13)
            })
        };
        let slabs = |w: &mut World| textures(w, "slabs", STD, |px| tex::slabs(px, SLAB_PANEL, 14));
        let paint = |w: &mut World, tile: f32, name: &str| {
            textures(w, name, STD, move |px| tex::paint(px, tile, 0.18, 15))
        };
        let grime = |w: &mut World| textures(w, "grime", 512, |px| tex::grime(px, GRIME_TILE, 16));
        match s {
            Surface::Asphalt => {
                let (b, n, o) = asphalt(w);
                let mut m = tiled(pbr(b, n, o), ASPHALT_TILE);
                m.reflectance = 0.45;
                m
            }
            Surface::AsphaltPatch => {
                let (b, n, o) = asphalt(w);
                let mut m = tiled(pbr(b, n, o), ASPHALT_TILE);
                m.base_color = [0.62, 0.62, 0.64, 1.0];
                m.perceptual_roughness = 0.92;
                m.normal_scale = 0.7;
                m
            }
            Surface::Runway => {
                let (b, n, o) = runway(w);
                tiled(pbr(b, n, o), RUNWAY_TILE)
            }
            Surface::Concrete => {
                let (b, n, o) = concrete(w);
                tiled(pbr(b, n, o), CONCRETE_TILE)
            }
            Surface::Slabs => {
                let (b, n, o) = slabs(w);
                tiled(pbr(b, n, o), SLAB_PANEL * 2.0)
            }
            Surface::PaintAsphalt
            | Surface::PaintRunway
            | Surface::PaintSlabs
            | Surface::PaintConcrete => {
                let (tile, normal) = match s {
                    Surface::PaintAsphalt => (ASPHALT_TILE, asphalt(w).1),
                    Surface::PaintRunway => (RUNWAY_TILE, runway(w).1),
                    Surface::PaintSlabs => (SLAB_PANEL * 2.0, slabs(w).1),
                    _ => (CONCRETE_TILE, concrete(w).1),
                };
                let (b, _, o) = paint(w, tile, &format!("paint{}", tile as u32));
                let mut m = tiled(pbr(b, normal, o), tile);
                m.occlusion_texture = None;
                m.normal_scale = 0.8;
                m.alpha_mode = AlphaMode::Mask(0.5);
                m
            }
            Surface::Glyphs => {
                let (b, _, o) = textures(w, "glyphs", 2048, |px| {
                    tex::glyph_atlas((px / 16).max(2), 0.95, 17)
                });
                let mut m = pbr(b, None, o);
                m.occlusion_texture = None;
                m.alpha_mode = AlphaMode::Mask(0.5);
                m
            }
            Surface::Curb => {
                let (b, n, o) = textures(w, "curb", STD, |px| tex::curb_paint(px, 4.0, 18));
                tiled(pbr(b, n, o), 4.0)
            }
            Surface::Gravel => {
                let (b, n, o) = textures(w, "gravel", STD, |px| tex::gravel(px, GRAVEL_TILE, 19));
                let mut m = tiled(pbr(b, n, o), GRAVEL_TILE);
                m.normal_scale = 1.3;
                m
            }
            Surface::Grass => {
                let (b, n, o) = textures(w, "grass", STD, |px| tex::grass(px, GRASS_TILE, 27));
                let mut m = tiled(pbr(b, n, o), GRASS_TILE);
                m.normal_scale = 1.2;
                m.reflectance = 0.3;
                m
            }
            Surface::Dirt => {
                let (b, n, o) = textures(w, "dirt", STD, |px| tex::dirt(px, DIRT_TILE, 20));
                tiled(pbr(b, n, o), DIRT_TILE)
            }
            Surface::Galvanized => {
                let (b, n, o) = textures(w, "galvanized", 512, |px| {
                    tex::galvanized(px, METAL_TILE, 21)
                });
                let mut m = tiled(pbr(b, n, o), METAL_TILE);
                m.metallic = 1.0;
                m.double_sided = true;
                m
            }
            Surface::Painted => {
                let (b, n, o) = grime(w);
                let mut m = tiled(pbr(b, n, o), GRIME_TILE);
                m.normal_scale = 0.6;
                m
            }
            Surface::Cladding => {
                let (b, n, o) = textures(w, "cladding", STD, |px| tex::cladding(px, 0.25, 4, 22));
                tiled(pbr(b, n, o), CLADDING_TILE)
            }
            Surface::Glass => {
                let mut m = StandardMaterial::hex(0x1b2a33).with_roughness(0.04);
                m.base_color[3] = 0.42;
                m.alpha_mode = AlphaMode::Blend;
                m.reflectance = 0.6;
                m.double_sided = true;
                m.receives_wetness = false;
                m
            }
            Surface::Rubber => StandardMaterial::hex(0x1c1c1e).with_roughness(0.85),
            Surface::Plastic => {
                let mut m = StandardMaterial::color(1.0, 1.0, 1.0).with_roughness(0.42);
                m.reflectance = 0.5;
                m
            }
            Surface::Fence => {
                let (b, n, o) = textures(w, "fence", 512, |px| tex::chain_link(px, FENCE_TILE));
                let mut m = tiled(pbr(b, n, o), FENCE_TILE);
                m.metallic = 1.0;
                m.occlusion_texture = None;
                m.alpha_mode = AlphaMode::Blend;
                m.double_sided = true;
                m
            }
            Surface::Grate => {
                let (b, n, o) = textures(w, "grate", STD, |px| tex::grating(px, GRATE_TILE, 26));
                let mut m = tiled(pbr(b, n, o), GRATE_TILE);
                m.metallic = 1.0;
                m.double_sided = true;
                m
            }
            Surface::Skid | Surface::SkidDirt => {
                let (b, _, _) = textures(w, "skid", 1024, |px| {
                    tex::skid((px / 8).max(4), px, SKID_LENGTH, 23)
                });
                let mut m = pbr(b, None, None);
                m.uv_scale = [1.0, 1.0 / SKID_LENGTH];
                m.alpha_mode = AlphaMode::Blend;
                m.perceptual_roughness = 0.7;
                if s == Surface::SkidDirt {
                    m.base_color = [3.5, 2.6, 1.9, 0.75];
                }
                m
            }
            Surface::Tar => {
                let mut m = StandardMaterial::hex(0x0b0b0c).with_roughness(0.45);
                m.base_color[3] = 0.9;
                m.alpha_mode = AlphaMode::Blend;
                m
            }
            Surface::Soot => {
                let (b, _, _) = textures(w, "soot", STD, |px| tex::soot(px, 24));
                let mut m = pbr(b, None, None);
                m.alpha_mode = AlphaMode::Blend;
                m.perceptual_roughness = 0.95;
                m
            }
            Surface::Signs => {
                let (b, _, o) = textures(w, "signs", HERO, |px| tex::sign_atlas(px, 25));
                let mut m = pbr(b, None, o);
                m.occlusion_texture = None;
                m
            }
            Surface::Interior => StandardMaterial::hex(0x0c0d0e).with_roughness(0.9),
            Surface::LensWhite => lens([1.0, 0.92, 0.78], 60_000.0),
            Surface::LensRed => lens([1.0, 0.06, 0.03], 90_000.0),
            Surface::LensGreen => lens([0.1, 1.0, 0.35], 50_000.0),
            Surface::LensAmber => lens([1.0, 0.55, 0.12], 60_000.0),
            Surface::LensBlue => lens([0.15, 0.35, 1.0], 70_000.0),
            Surface::LensOff => {
                let mut m = StandardMaterial::hex(0x2a0806).with_roughness(0.06);
                m.reflectance = 0.8;
                m
            }
            Surface::LensClear => {
                let mut m = StandardMaterial::hex(0xc8ccd0).with_roughness(0.12);
                m.metallic = 1.0;
                m
            }
            Surface::Lamp => lens([1.0, 0.95, 0.86], 8_000.0),
            Surface::Screen => {
                let mut m = lens([0.4, 0.6, 0.95], 350.0);
                m.base_color = [0.02, 0.025, 0.03, 1.0];
                m
            }
        }
    }

    /// Spawn `mesh` (vertices relative to the zone origin) with material `s`.
    pub fn add(&mut self, name: &str, mesh: Mesh, s: Surface, shadows: bool) -> Option<Entity> {
        if mesh.indices.is_empty() {
            return None;
        }
        let key = format!("{}/{name}", self.key);
        self.triangles += mesh.triangle_count();
        let handle = self
            .world
            .resource_mut::<Assets<Mesh>>()
            .insert_keyed(&key, mesh);
        let material = self.material(s);
        let t = Transform::from_translation(self.origin).with_rotation(self.rotation);
        Some(self.spawn_handle(name, handle, material, t, shadows))
    }

    /// Spawn many copies of one shared mesh (registered under `vehicles/scenery/prop/<key>`)
    /// at `placements` (zone-local transforms).
    pub fn instances(
        &mut self,
        key: &str,
        make: impl FnOnce() -> Mesh,
        s: Surface,
        shadows: bool,
        placements: &[Transform],
    ) {
        if placements.is_empty() {
            return;
        }
        let handle = self
            .world
            .resource_mut::<Assets<Mesh>>()
            .get_or_insert_keyed(&format!("vehicles/scenery/prop/{key}"), make);
        let tris = self
            .world
            .resource::<Assets<Mesh>>()
            .get(handle)
            .map_or(0, |m| m.triangle_count());
        let material = self.material(s);
        for t in placements {
            let mut world_t = *t;
            world_t.translation = self.to_world(t.translation);
            world_t.rotation = self.rotation * t.rotation;
            self.spawn_handle(key, handle, material, world_t, shadows);
            self.triangles += tris;
        }
    }

    fn spawn_handle(
        &mut self,
        name: &str,
        mesh: Handle<Mesh>,
        material: Handle<StandardMaterial>,
        transform: Transform,
        shadows: bool,
    ) -> Entity {
        let mut r = MeshRenderer::new(MeshRef::Handle(mesh), MaterialRef::Handle(material));
        r.cast_shadows = shadows;
        self.entities += 1;
        self.world
            .spawn((transform, r, Name::new(format!("Scenery: {name}"))))
            .id()
    }

    /// World position of a zone-local point.
    pub fn to_world(&self, local: DVec3) -> DVec3 {
        self.origin + self.rotation.as_dquat() * local
    }

    /// The same context with zone-local axes rotated by `rotation` into the world.
    pub fn rotated(mut self, rotation: Quat) -> Self {
        self.rotation = rotation;
        self
    }

    /// Spawn a point light at a zone-local position.
    pub fn light(&mut self, at: DVec3, light: engine_render::PointLight) {
        let t = Transform::from_translation(self.to_world(at));
        self.world.spawn((t, light, Name::new("Scenery: light")));
        self.entities += 1;
    }

    /// Record build statistics under `vehicles.scenery.<zone>.*` in `Diagnostics` (if
    /// present) and the log.
    pub fn finish(self, zone: &str, started: std::time::Instant) -> BuildStats {
        let stats = BuildStats {
            seconds: started.elapsed().as_secs_f64(),
            triangles: self.triangles,
            entities: self.entities,
        };
        if let Some(mut d) = self
            .world
            .get_resource_mut::<engine_core::diagnostics::Diagnostics>()
        {
            d.set(
                &format!("vehicles.scenery.{zone}.ms"),
                stats.seconds * 1000.0,
            );
            d.set(
                &format!("vehicles.scenery.{zone}.triangles"),
                stats.triangles as f64,
            );
        }
        let mut s = self
            .world
            .get_resource_or_insert_with::<SceneryStats>(SceneryStats::default);
        s.zones.push((zone.to_string(), stats));
        stats
    }
}

/// Whether meshes of this material cast sun shadows (flat ground layers and decals don't).
pub fn casts_shadow(s: Surface) -> bool {
    !matches!(
        s,
        Surface::Asphalt
            | Surface::AsphaltPatch
            | Surface::Runway
            | Surface::Slabs
            | Surface::PaintAsphalt
            | Surface::PaintRunway
            | Surface::PaintSlabs
            | Surface::PaintConcrete
            | Surface::Glyphs
            | Surface::Skid
            | Surface::SkidDirt
            | Surface::Tar
            | Surface::Soot
            | Surface::Gravel
            | Surface::Dirt
            | Surface::Grass
            | Surface::Curb
    )
}

/// A mown grass ground `2 * half` meters across with its top at `center`, for apps and tests
/// that have no terrain of their own (the showcase draws its own). Lusher and drier patches
/// come from vertex colors over the tiled texture. Does nothing without a renderer.
pub fn grass_ground(world: &mut World, center: DVec3, half: f32) {
    let Some(mut scene) = Scene::new(world, center, "ground") else {
        return;
    };
    let mut b = kit::Builder::new();
    let n = ((half * 2.0) / 6.0).ceil().max(1.0) as u32;
    let step = half * 2.0 / n as f32;
    let base = b.mesh.vertices.len() as u32;
    let o = center.as_vec3();
    for j in 0..=n {
        for i in 0..=n {
            let (x, z) = (-half + i as f32 * step, -half + j as f32 * step);
            let (wx, wz) = (x + o.x, z + o.z);
            let k = 1.0
                + 0.3 * noise::world_fbm(wx, wz, 45.0, 4, 3)
                + 0.12 * noise::world_fbm(wx, wz, 9.0, 3, 4);
            let dry = noise::smoothstep(0.1, 0.6, noise::world_fbm(wx, wz, 80.0, 3, 5));
            let c = engine_core::math::Vec4::new(
                k * (1.0 + 0.4 * dry),
                k * (1.0 + 0.14 * dry),
                k * (1.0 - 0.1 * dry),
                1.0,
            );
            b.vertex_c(
                engine_core::math::Vec3::new(x, 0.0, z),
                engine_core::math::Vec3::Y,
                engine_core::math::Vec2::new(wx, wz),
                c,
            );
        }
    }
    let row = n + 1;
    for j in 0..n {
        for i in 0..n {
            let k = base + j * row + i;
            b.quad_idx(k, k + 1, k + row + 1, k + row);
        }
    }
    scene.add("grass", b.finish(), Surface::Grass, false);
}

/// One mesh builder per material; spawned as one entity per material.
#[derive(Default)]
pub struct Parts {
    list: Vec<(Surface, kit::Builder)>,
}

impl Parts {
    /// The builder for material `s`.
    pub fn get(&mut self, s: Surface) -> &mut kit::Builder {
        let i = match self.list.iter().position(|(k, _)| *k == s) {
            Some(i) => i,
            None => {
                self.list.push((s, kit::Builder::new()));
                self.list.len() - 1
            }
        };
        &mut self.list[i].1
    }

    /// Triangles so far.
    pub fn triangles(&self) -> usize {
        self.list.iter().map(|(_, b)| b.triangles()).sum()
    }

    /// Spawn every non-empty builder as `<prefix>/<material>`.
    pub fn spawn(self, scene: &mut Scene, prefix: &str) {
        for (s, b) in self.list {
            let name = format!("{prefix}/{s:?}");
            scene.add(&name, b.finish(), s, casts_shadow(s));
        }
    }
}

/// What one zone's scenery build produced.
#[derive(Clone, Copy, Debug, Default)]
pub struct BuildStats {
    /// Wall time of the build (textures baked on first use included), seconds.
    pub seconds: f64,
    pub triangles: usize,
    pub entities: usize,
}

/// Every scenery build so far (zone name and stats), for tests and diagnostics.
#[derive(Resource, Default, Debug)]
pub struct SceneryStats {
    pub zones: Vec<(String, BuildStats)>,
}
