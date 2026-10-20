#![doc = include_str!("../README.md")]

pub mod chunk;
pub mod collider;
pub mod cube;
pub mod deform;
pub mod demo;
pub mod edits;
pub mod forest;
pub mod ground;
pub mod landforms;
pub mod lod;
pub mod materials;
pub mod noise;
pub mod palette;
pub mod render;
pub mod rivers;
pub mod shape;

use std::collections::HashMap;
use std::sync::Arc;

use engine_core::math::{DQuat, DVec3};
use engine_core::prelude::*;
use engine_render::RenderAppExt;
use engine_world::{
    Deformation as Deform, EditOp, EditShape, Env, GroundContact, SurfaceLayers, SurfaceMaterial,
    SurfaceMaterials, TerrainChanged, TerrainEdit, WeatherState, WorldShift, WorldShiftSet,
};

pub use chunk::{ChunkMesh, ChunkSource, TerrainVertex};
pub use cube::{NodeKey, SurfaceMap, TerrainDomain, home_rotation};
pub use deform::{DeformationMap, DeformationSettings};
pub use edits::{EditLayer, LocalEdit, OverlayPaint, PAINT_NAMES, paint_index};
pub use ground::GroundSurface;
pub use palette::TerrainPalette;
pub use shape::{
    Climate, FlatPad, HeightOverlay, OverlayPaintFn, PlanetShape, ShapeSample, dir_from_lat_lon,
    lat_lon_from_dir,
};

/// A procedural terrain: a whole planet (or a flat square) with level of detail from orbit
/// down to half a meter, oceans, mountains, craters, flat building pads, painted materials and
/// edits. Put it on an entity whose `Transform` is the planet's center (and rotation).
///
/// ```ignore
/// // An Earth-like planet whose spot at 40°N 10°E is the world origin, flat for 800 m.
/// let pad = FlatPad { lat_deg: 40.0, lon_deg: 10.0, ..default() };
/// let terrain = PlanetTerrain::earthlike(7).with_pad(pad);
/// commands.spawn((terrain.home_transform(pad), terrain));
/// ```
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlanetTerrain {
    pub shape: PlanetShape,
    pub domain: TerrainDomain,
    pub palette: TerrainPalette,
    /// Level-of-detail multiplier: 1 is the default, 2 splits patches twice as far away.
    pub detail: f32,
    /// Vertex spacing of the finest patches near the camera, meters.
    pub finest_spacing_m: f64,
    /// Draw the sea as a surface at sea level. Turn off when a water module draws the ocean.
    pub ocean_surface: bool,
    pub cast_shadows: bool,
    /// Answer the world's `Ground` query (height, normal, surface material) from this terrain.
    pub ground: bool,
    /// Another query that names the surface material while this terrain answers height and
    /// normal (see [`Self::with_ground_surface`]). Not saved with the terrain.
    #[serde(skip)]
    pub ground_surface: Option<GroundSurface>,
    /// Keep physics heightfield colliders around the simulation focus.
    pub colliders: bool,
}

impl Default for PlanetTerrain {
    fn default() -> Self {
        Self::earthlike(1)
    }
}

impl PlanetTerrain {
    /// An Earth-sized planet with oceans, continents, mountains and snow.
    pub fn earthlike(seed: u64) -> Self {
        Self {
            shape: PlanetShape::earthlike(seed),
            domain: TerrainDomain::Sphere,
            palette: TerrainPalette::earth(),
            detail: 1.0,
            finest_spacing_m: 0.5,
            ocean_surface: true,
            cast_shadows: true,
            ground: true,
            ground_surface: None,
            colliders: true,
        }
    }

    /// A cratered, airless moon.
    pub fn moon(seed: u64) -> Self {
        Self {
            shape: PlanetShape::moon(seed),
            palette: TerrainPalette::moon(),
            ocean_surface: false,
            ..Self::earthlike(seed)
        }
    }

    /// A red desert planet.
    pub fn mars(seed: u64) -> Self {
        Self {
            shape: PlanetShape::mars(seed),
            palette: TerrainPalette::mars(),
            ocean_surface: false,
            ..Self::earthlike(seed)
        }
    }

    /// A flat square level `size_m` across with the relief of an Earth-like world.
    pub fn flat(size_m: f64, seed: u64) -> Self {
        Self {
            domain: TerrainDomain::Flat {
                size_m,
                lat_deg: 40.0,
                lon_deg: 0.0,
            },
            ..Self::earthlike(seed)
        }
    }

    pub fn with_pad(mut self, pad: FlatPad) -> Self {
        self.shape.pads.push(pad);
        self
    }

    pub fn with_radius(mut self, radius_m: f64) -> Self {
        self.shape.radius_m = radius_m;
        self
    }

    /// Replace the ground within `radius_m` of the home spot (the first pad) with a hand-made
    /// height function, blending back into the planet over `blend_m` around it.
    ///
    /// `height(x, z)` returns the world y of the ground at world `(x, z)` in the home frame
    /// that [`home_transform`](Self::home_transform) sets up (or the space module's frame on
    /// the same spot). The ground passes exactly through `(x, height(x, z), z)`: the terrain
    /// converts it to heights on the sphere itself. A level built flat stays flat, standing
    /// up to `r² / 2R` above the curved planet (6 m at 9 km on Earth) until the blend; one whose
    /// heights already drop by `(x² + z²) / 2R` follows the sphere. Everything reads it:
    /// rendering, the `Ground` query, footprints and physics colliders. Call it after
    /// `with_pad`/`with_radius`.
    ///
    /// ```ignore
    /// let terrain = PlanetTerrain::earthlike(7)
    ///     .with_radius(6_371_000.0)
    ///     .with_pad(FlatPad::default())
    ///     .with_overlay(20_000.0, 6_000.0, |x, z| valley_height(x, z));
    /// ```
    pub fn with_overlay(
        mut self,
        radius_m: f64,
        blend_m: f64,
        height: impl Fn(f64, f64) -> f64 + Send + Sync + 'static,
    ) -> Self {
        let pad = self.shape.pads.first().copied().unwrap_or_default();
        let (rotation, translation, flat) = match self.domain {
            TerrainDomain::Sphere => {
                let tf = self.home_transform(pad);
                (tf.rotation.as_dquat().normalize(), tf.translation, false)
            }
            TerrainDomain::Flat {
                lat_deg, lon_deg, ..
            } => (
                engine_core::math::DQuat::from_rotation_arc(
                    engine_core::math::DVec3::Y,
                    dir_from_lat_lon(lat_deg, lon_deg),
                )
                .inverse(),
                engine_core::math::DVec3::new(0.0, -pad.height_m, 0.0),
                true,
            ),
        };
        // Sample the function once for the level-of-detail bounds.
        let reach = radius_m + blend_m;
        let (mut lo, mut hi) = (f64::MAX, f64::MIN);
        for j in 0..=64 {
            for i in 0..=64 {
                let y = height(
                    (i as f64 / 32.0 - 1.0) * reach,
                    (j as f64 / 32.0 - 1.0) * reach,
                );
                lo = lo.min(y);
                hi = hi.max(y);
            }
        }
        let margin = (hi - lo) * 0.1 + 50.0;
        self.shape.overlay = Some(HeightOverlay {
            height: std::sync::Arc::new(height),
            radius_m,
            blend_m,
            rotation,
            translation,
            flat,
            y_range: (lo - margin, hi + margin),
            paint: None,
        });
        self
    }

    /// Paint the hand-made ground of [`with_overlay`](Self::with_overlay): `paint(x, z)`
    /// returns the ground layer and tint at world `(x, z)` (the same frame as the height), or
    /// `None` for the planet's own ground there. Roads, plazas, beaches, fields and the forest
    /// floor: the renderer draws the layer's material, and footsteps, prints, rain and the
    /// `Ground` query treat the spot as that surface. Edits paint over it. Call it after
    /// `with_overlay`.
    ///
    /// ```ignore
    /// let terrain = PlanetTerrain::earthlike(7)
    ///     .with_pad(FlatPad::default())
    ///     .with_overlay(20_000.0, 6_000.0, |x, z| valley_height(x, z))
    ///     .with_overlay_paint(|x, z| match cover(x, z) {
    ///         Cover::Road => Some(OverlayPaint::layer("dirt", 1.0)),
    ///         Cover::Wheat => Some(OverlayPaint::tint([0.36, 0.27, 0.09], 0.8)),
    ///         _ => None,
    ///     });
    /// ```
    pub fn with_overlay_paint(
        mut self,
        paint: impl Fn(f64, f64) -> Option<OverlayPaint> + Send + Sync + 'static,
    ) -> Self {
        match self.shape.overlay.as_mut() {
            Some(overlay) => overlay.paint = Some(std::sync::Arc::new(paint)),
            None => engine_core::log::warn!("with_overlay_paint needs with_overlay first"),
        }
        self
    }

    /// Let another ground query name the surface material (grass, mud, snow...) while this
    /// terrain keeps answering height and normal, edits included: for hand-made ground whose
    /// materials live elsewhere. It is asked half a meter above the surface point. Where it
    /// returns `None`, under the sea and where an edit painted a layer, the terrain's own
    /// material answers.
    ///
    /// Never pass the world's `Ground`: with `ground` on, that is this terrain asking itself.
    ///
    /// ```ignore
    /// let earth = PlanetTerrain::earthlike(7)
    ///     .with_pad(pad)
    ///     .with_overlay(9_000.0, 6_000.0, valley_height)
    ///     .with_ground_surface(Arc::new(ValleyMaterials::new()));
    /// ```
    pub fn with_ground_surface(mut self, query: Arc<dyn engine_world::GroundQuery>) -> Self {
        self.ground_surface = Some(GroundSurface(query));
        self
    }

    /// The transform that puts `pad`'s center at the world origin with +Y up and north toward
    /// −Z, or as the pad's `heading_deg` turns it (planet center at about
    /// `(0, −radius − pad height, 0)`).
    ///
    /// `Transform` holds an `f32` rotation, which is only good to ~0.4 m at Earth's radius, so
    /// the translation is computed for the rotation as stored: the pad lands on the origin
    /// exactly and only the (harmless, ~1e-7 rad) tilt is rounded.
    pub fn home_transform(&self, pad: FlatPad) -> Transform {
        if !matches!(self.domain, TerrainDomain::Sphere) {
            return Transform::from_translation(DVec3::new(0.0, -pad.height_m, 0.0));
        }
        let rot = pad.home_rotation().as_quat();
        let stored = rot.as_dquat().normalize();
        let pad_point = pad.direction() * (self.shape.radius_m + pad.height_m);
        Transform::from_translation(-(stored * pad_point)).with_rotation(rot)
    }

    /// Lowest and highest heights the shape can produce (conservative), meters.
    pub fn height_range(&self) -> (f64, f64) {
        let s = &self.shape;
        let low = -(s.ocean_depth_m
            + s.plateau_height_m
            + s.hill_height_m
            + s.detail_height_m * 2.5
            + s.crater_max_radius_m * 0.25)
            - 50.0;
        // Pad ranges (see `FlatPad::mountains`) rise on top of everything else.
        let pad_range = s.pads.iter().map(|p| p.mountains).fold(0.0, f64::max) * 6_300.0;
        let mut high = s.plateau_height_m
            + s.mountain_height_m * 1.6
            + s.hill_height_m * 1.3
            + s.detail_height_m * 4.5
            + s.crater_max_radius_m * 0.05
            + pad_range
            + 50.0;
        let (forms_low, forms_high) = s.landforms.height_range();
        high += forms_high;
        let mut low = low + forms_low;
        if let Some(o) = &s.overlay {
            let base = if o.flat {
                -o.translation.y
            } else {
                -o.translation.y - s.radius_m
            };
            let reach = o.radius_m + o.blend_m;
            low = low.min(base + o.y_range.0);
            high = high.max(base + o.y_range.1 + reach * reach / (2.0 * s.radius_m));
        }
        (low, high)
    }
}

/// Global terrain settings.
#[derive(Resource, Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TerrainSettings {
    /// Split a patch when the camera is closer than this many patch sizes (× each terrain's
    /// `detail`).
    pub split_factor: f64,
    /// Patches a planet that fills the view may draw before detail stops growing: a view
    /// that needs fewer (a planet seen from orbit) splits them finer until it reaches this,
    /// up to four times the detail, so mountains, canyons and craters stay sharp from space.
    /// Planets covering less of the view get a share of it (0 = never boost).
    pub patch_budget: usize,
    /// Patches built per frame while moving (they build in parallel).
    pub builds_per_frame: usize,
    /// When more patches than this are missing (after a teleport), build them all at once.
    pub catch_up_threshold: usize,
    /// GPU patch slots (32 KB each).
    pub max_patches: usize,
    /// Physics collider tile size (meters) and resolution (samples per side).
    pub collider_tile_m: f64,
    pub collider_samples: u32,
    /// Draw patch outlines (debug).
    pub wireframe_lod: bool,
    /// Slide vertices toward the coarser patch's shape before it takes over, so detail fades
    /// in and out instead of popping.
    pub geomorph: bool,
    /// Texels per side of the ground material textures (grass, leaves, rock, snow…), a power
    /// of two. They bake once per process on a background thread; 0 draws plain colors.
    pub material_resolution: u32,
    /// Use the CC0 photo scans in `<asset root>/fetched/terrain` for the materials that have
    /// one (`cargo run -p fetch-assets -- terrain`); the rest stay procedural.
    pub scanned_materials: bool,
}

impl TerrainSettings {
    /// Folder the ground material scans are read from, if scans are on: `fetched/terrain`
    /// under the [`AssetServer`](engine_assets::AssetServer) root (or `assets/` without one).
    pub fn scan_folder(&self, world: &World) -> Option<std::path::PathBuf> {
        self.scanned_materials.then(|| {
            world
                .get_resource::<engine_assets::AssetServer>()
                .map(|a| a.root())
                .unwrap_or_else(|| "assets".into())
                .join("fetched/terrain")
        })
    }
}

impl Default for TerrainSettings {
    fn default() -> Self {
        Self {
            split_factor: 3.2,
            patch_budget: 700,
            builds_per_frame: 24,
            catch_up_threshold: 48,
            max_patches: 6144,
            collider_tile_m: 64.0,
            collider_samples: 65,
            wireframe_lod: false,
            geomorph: true,
            material_resolution: 1024,
            scanned_materials: true,
        }
    }
}

/// Runtime state of one terrain (shared between systems and the renderer).
#[derive(Debug, Clone)]
pub struct TerrainRuntime {
    pub source: ChunkSource,
    /// The master edit list (`source.edits` is a snapshot of it).
    pub edits: EditLayer,
    pub config: PlanetTerrain,
    pub position: DVec3,
    pub rotation: DQuat,
    /// Bumped when the shape or palette changes (every patch is rebuilt).
    pub version: u64,
    /// Terrain-local spheres changed by edits, oldest first. Consumers (renderer, colliders)
    /// keep their own read position.
    pub edit_log: Vec<(DVec3, f64)>,
}

impl TerrainRuntime {
    fn new(config: &PlanetTerrain, edits: EditLayer, version: u64) -> Self {
        let map = SurfaceMap::new(config.domain, config.shape.radius_m);
        Self {
            source: ChunkSource {
                shape: Arc::new(config.shape.clone()),
                edits: Arc::new(edits.clone()),
                map,
                ocean_surface: config.ocean_surface,
            },
            edits,
            config: config.clone(),
            position: DVec3::ZERO,
            rotation: DQuat::IDENTITY,
            version,
            edit_log: Vec::new(),
        }
    }

    /// This terrain as the ground query sees it.
    pub fn ground_terrain(&self) -> ground::GroundTerrain {
        ground::GroundTerrain {
            source: self.source.clone(),
            palette: self.config.palette.clone(),
            position: self.position,
            rotation: self.rotation,
            surface_override: self.config.ground_surface.clone(),
        }
    }

    /// World point → terrain-local.
    pub fn to_local(&self, p: DVec3) -> DVec3 {
        self.rotation.inverse() * (p - self.position)
    }

    /// Distance from a world point to this terrain's sea-level surface (signed, meters).
    pub fn altitude_hint(&self, p: DVec3) -> f64 {
        let l = self.to_local(p);
        if self.source.map.is_sphere() {
            l.length() - self.source.map.radius
        } else {
            l.y
        }
    }
}

/// Every terrain in the world, by entity.
#[derive(Resource, Debug, Default)]
pub struct Terrains {
    pub entries: HashMap<Entity, TerrainRuntime>,
    next_version: u64,
}

impl Terrains {
    /// The terrain whose surface is nearest to a world point.
    pub fn nearest(&self, p: DVec3) -> Option<(Entity, &TerrainRuntime)> {
        self.entries
            .iter()
            .min_by(|a, b| {
                a.1.altitude_hint(p)
                    .abs()
                    .total_cmp(&b.1.altitude_hint(p).abs())
            })
            .map(|(e, t)| (*e, t))
    }

    /// Full-detail surface sample under a world point: (world surface point, world normal,
    /// terrain height above sea level).
    pub fn surface_at(
        &self,
        p: DVec3,
        materials: &SurfaceMaterials,
    ) -> Option<(DVec3, Vec3, engine_world::SurfaceId, f64)> {
        let (_, t) = self.nearest(p)?;
        t.ground_terrain()
            .surface(p, &palette::SurfaceIds::new(materials))
    }
}

/// Adds terrains: rendering, the ground query, edits, footprints and physics colliders.
pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        if !app.world.contains_resource::<SurfaceMaterials>() {
            app.init_resource::<SurfaceMaterials>();
        }
        {
            // Ground layers the terrain paints that the default table lacks: Moon/Mars dust
            // and the ash left by blasts and fires.
            let mut mats = app.world.resource_mut::<SurfaceMaterials>();
            for (name, friction, density, absorbency, depth_m) in [
                ("regolith", 0.75, 1500.0, 0.1, 0.035),
                ("ash", 0.6, 700.0, 0.6, 0.03),
            ] {
                if mats.id(name).is_none() {
                    mats.add(SurfaceMaterial {
                        name: name.into(),
                        friction,
                        density,
                        absorbency,
                        deformation: Deform::Granular { depth_m },
                        footstep_sound: format!("footstep.{name}"),
                        impact_effect: format!("impact.{name}"),
                        ..Default::default()
                    });
                }
            }
        }
        let settings = DeformationSettings::default();
        app.init_resource::<Terrains>()
            .init_resource::<TerrainSettings>()
            .insert_resource(DeformationMap::new(settings.cell_m, settings.size))
            .insert_resource(settings)
            .init_resource::<collider::TerrainColliders>()
            .init_resource::<RecoverClock>()
            .register_component::<PlanetTerrain>(
                "Procedural planet or flat terrain on this entity (shape, palette, level of \
                 detail, ocean, ground query, colliders). Lengths in meters.",
            )
            .register_component::<demo::DemoWalker>(
                "Demo walker that follows a looped path and leaves footprints (speed m/s, \
                 step length m, load kg, contact kind).",
            )
            .register_resource::<TerrainSettings>(
                "Terrain level of detail, patch build budget, collider tiles.",
            )
            .register_resource::<DeformationSettings>(
                "Footprints and tracks: grid cell size (m), rim height, recovery rates.",
            )
            .add_systems(
                PostUpdate,
                (sync_terrains, apply_edits, collider::update_colliders)
                    .chain()
                    .in_set(UpdateSet::AfterPropagate),
            )
            .add_systems(PostUpdate, follow_world_shift.in_set(WorldShiftSet::Apply))
            .add_systems(
                FixedUpdate,
                (demo::walk, demo::swing_limbs, deform_from_contacts)
                    .chain()
                    .in_set(SimSet::Environment),
            )
            .add_render_feature(render::TerrainFeature::default());
        register_apis(app);
    }
}

/// Keeps [`Terrains`] in sync with `PlanetTerrain` components and installs the ground query.
/// A planet entity: its terrain and where it is.
type PlanetPlacement = (
    Entity,
    Ref<'static, PlanetTerrain>,
    &'static GlobalTransform,
    &'static Transform,
    Has<ChildOf>,
);

fn sync_terrains(
    mut terrains: ResMut<Terrains>,
    q: Query<PlanetPlacement>,
    materials: Res<SurfaceMaterials>,
    mut ground: ResMut<engine_world::Ground>,
    mut forest: ResMut<engine_world::ForestCover>,
    diagnostics: Option<ResMut<Diagnostics>>,
    // Whether a provider is installed, and how many times one was (a diagnostic: the ground
    // is rebuilt only when a terrain is added, removed or moved).
    mut installed: Local<(bool, u32)>,
) {
    let mut changed = false;
    let mut seen = Vec::with_capacity(terrains.entries.len());
    for (e, config, gt, tf, child) in &q {
        seen.push(e);
        let reset = match terrains.entries.get(&e) {
            None => true,
            Some(t) => config.is_changed() && t.config != *config,
        };
        if reset {
            terrains.next_version += 1;
            let version = terrains.next_version;
            let (edits, log) = terrains
                .entries
                .get(&e)
                .map(|t| (t.edits.clone(), t.edit_log.clone()))
                .unwrap_or_default();
            let mut runtime = TerrainRuntime::new(&config, edits, version);
            runtime.edit_log = log;
            terrains.entries.insert(e, runtime);
            changed = true;
        }
        let t = terrains.entries.get_mut(&e).expect("terrain entry");
        // Normalized in f64: an f32 quaternion is unit only to ~1e-7, which would scale a
        // planet's radius by meters. A root planet reads its own `Transform`: the rotation
        // extracted from the global matrix differs by ~3e-8, a third of a meter at Earth's
        // radius, and would shift the ground against everything placed in the home frame.
        let (pos, rot) = if child {
            (gt.translation(), gt.rotation().as_dquat().normalize())
        } else {
            (tf.translation, tf.rotation.as_dquat().normalize())
        };
        if t.position != pos || t.rotation != rot {
            t.position = pos;
            t.rotation = rot;
            changed = true;
        }
    }
    let before = terrains.entries.len();
    terrains.entries.retain(|e, _| seen.contains(e));
    changed |= terrains.entries.len() != before;
    if !changed {
        return;
    }
    let list: Vec<_> = terrains
        .entries
        .values()
        .filter(|t| t.config.ground)
        .map(TerrainRuntime::ground_terrain)
        .collect();
    if list.is_empty() {
        if installed.0 {
            *ground = engine_world::Ground::default();
            *forest = engine_world::ForestCover::default();
            installed.0 = false;
        }
    } else {
        let provider = ground::provider(list, &materials);
        *forest = engine_world::ForestCover(provider.clone());
        *ground = engine_world::Ground(provider);
        installed.0 = true;
        installed.1 += 1;
        if let Some(mut d) = diagnostics {
            d.set("terrain.ground_installs", installed.1 as f64);
        }
    }
}

/// Applies [`TerrainEdit`] messages to the nearest terrain and reports [`TerrainChanged`].
/// Edits that arrived before any terrain existed (a level spawning its terrain and its edits
/// in the same frame), with how many frames they have waited.
#[derive(Default)]
struct PendingEdits(Vec<(TerrainEdit, u32)>);

/// Frames an edit waits for a terrain before it is dropped.
const EDIT_WAIT_FRAMES: u32 = 600;

#[allow(clippy::too_many_arguments)]
fn apply_edits(
    mut edits: MessageReader<TerrainEdit>,
    mut terrains: ResMut<Terrains>,
    materials: Res<SurfaceMaterials>,
    mut changed: MessageWriter<TerrainChanged>,
    mut ground: ResMut<engine_world::Ground>,
    mut forest: ResMut<engine_world::ForestCover>,
    mut pending: Local<PendingEdits>,
    mut prints: ResMut<DeformationMap>,
) {
    let mut any = false;
    let waiting = std::mem::take(&mut pending.0);
    let incoming = waiting
        .into_iter()
        .chain(edits.read().map(|e| (*e, 0)))
        .collect::<Vec<_>>();
    for (edit, waited) in incoming {
        let Some((entity, _)) = terrains.nearest(edit.center) else {
            if waited < EDIT_WAIT_FRAMES {
                pending.0.push((edit, waited + 1));
            }
            continue;
        };
        let t = terrains.entries.get_mut(&entity).expect("terrain");
        let inv = t.rotation.inverse();
        let shape = match edit.shape {
            EditShape::Box {
                half_extents,
                rotation,
            } => EditShape::Box {
                half_extents,
                rotation: inv.as_quat() * rotation,
            },
            EditShape::Capsule { to, radius } => EditShape::Capsule {
                to: (inv * to.as_dvec3()).as_vec3(),
                radius,
            },
            s => s,
        };
        let paint = edit
            .surface
            .map(|id| paint_index(&materials.get(id).name))
            .unwrap_or(match edit.op {
                EditOp::Remove => 0,
                _ => 0,
            });
        let local = LocalEdit {
            center: inv * (edit.center - t.position),
            shape,
            op: edit.op,
            paint,
        };
        let reach = local.reach();
        t.edits.add(local);
        t.source.edits = Arc::new(t.edits.clone());
        t.edit_log.push((local.center, reach + 2.0));
        changed.write(TerrainChanged {
            center: edit.center,
            radius: reach as f32,
        });
        // Digging, building up or smoothing takes the prints there with it; paint keeps them.
        if edit.op != EditOp::Paint {
            prints.erase(edit.center, reach as f32);
        }
        any = true;
    }
    if any {
        let list: Vec<_> = terrains
            .entries
            .values()
            .filter(|t| t.config.ground)
            .map(TerrainRuntime::ground_terrain)
            .collect();
        if !list.is_empty() {
            let provider = ground::provider(list, &materials);
            *forest = engine_world::ForestCover(provider.clone());
            *ground = engine_world::Ground(provider);
        }
    }
}

/// Accumulates time for the low-rate print recovery pass.
#[derive(Resource, Default)]
struct RecoverClock(f32);

/// Footprints and lab walkers stay on the ground when the world re-anchors (planets and
/// their colliders are placed from their transforms, which engine_world moves).
fn follow_world_shift(
    mut shifts: MessageReader<WorldShift>,
    mut map: ResMut<DeformationMap>,
    mut walkers: Query<&mut demo::DemoWalker>,
) {
    let Some(s) = shifts.read().copied().reduce(|a, b| a.then(&b)) else {
        return;
    };
    map.follow_shift(&s);
    for mut w in &mut walkers {
        w.center = s.point(w.center);
    }
}

/// Stamps ground contacts into the deformation map and lets old prints recover.
#[allow(clippy::too_many_arguments)]
fn deform_from_contacts(
    mut contacts: MessageReader<GroundContact>,
    mut map: ResMut<DeformationMap>,
    settings: Res<DeformationSettings>,
    env: Env,
    terrains: Res<Terrains>,
    time: Res<Time>,
    mut clock: ResMut<RecoverClock>,
    weather: Res<WeatherState>,
    surface_layers: Res<SurfaceLayers>,
) {
    if !settings.enabled || terrains.entries.is_empty() {
        contacts.clear();
        return;
    }
    if (map.cell - settings.cell_m).abs() > 1e-6 || map.size != settings.size {
        *map = DeformationMap::new(settings.cell_m, settings.size.clamp(64, 4096));
    }
    let now = time.elapsed_secs_f64();
    let grass_recover = env
        .materials
        .id("grass")
        .map(|id| match env.material(id).deformation {
            Deform::Flatten { recover_secs } => recover_secs,
            _ => 20.0,
        })
        .unwrap_or(20.0);
    if !env.focus.points.is_empty() {
        map.follow(
            env.focus.primary(),
            now,
            grass_recover,
            settings.sand_half_life_s,
        );
    }
    for c in contacts.read() {
        let ground = env.ground(c.position);
        let surface = c.surface.or(ground.map(|g| g.surface)).unwrap_or_default();
        let mat = env.material(surface);
        let layers = surface_layers.sample(c.position);
        let mut snow = layers.snow_m;
        if snow <= 0.02 && mat.name == "snow" {
            snow = settings.default_snow_m;
        }
        let (kind, depth, rim, flatten) =
            deform::response(mat.deformation, c.load_kg, layers.wetness, snow, &settings);
        if depth > 0.0 || flatten > 0.0 {
            map.record(c, kind, depth, rim, flatten, now);
        }
    }
    clock.0 += time.delta_secs();
    if clock.0 >= 0.25 {
        let dt = clock.0;
        clock.0 = 0.0;
        // Snowfall: precipitation in mm/h of water ≈ 10× that in fresh snow.
        let snowfall = if weather.is_snowing() {
            weather.precipitation_mm_h * 0.01 / 3600.0
        } else {
            0.0
        };
        map.recover(dt, snowfall, grass_recover, settings.sand_half_life_s);
    }
}

fn parse_vec3(v: Option<&serde_json::Value>) -> Option<DVec3> {
    let a = v?.as_array()?;
    Some(DVec3::new(
        a.first()?.as_f64()?,
        a.get(1)?.as_f64()?,
        a.get(2)?.as_f64()?,
    ))
}

fn register_apis(app: &mut App) {
    app.register_api(
        "terrain.height",
        "Ground under a world point: surface height, normal, material, height above sea level \
         and how far above the ground the point is.",
        r#"{"at": [0, 10, 0]}"#,
        |world, args| {
            let at = parse_vec3(args.get("at")).ok_or("Pass \"at\": [x, y, z] in meters.")?;
            let terrains = world.resource::<Terrains>();
            let mats = world.resource::<SurfaceMaterials>();
            let (point, normal, surface, height) = terrains
                .surface_at(at, mats)
                .ok_or("There is no terrain here. Spawn an entity with PlanetTerrain first.")?;
            let up = normal.as_dvec3();
            Ok(serde_json::json!({
                "surface_point": point.to_array(),
                "height": point.y,
                "normal": normal.to_array(),
                "material": mats.get(surface).name,
                "above_sea_level_m": height,
                "above_ground_m": (at - point).dot(up),
            }))
        },
    )
    .register_api(
        "terrain.edit",
        "Change the terrain: dig (\"remove\"), build up (\"add\"), \"smooth\" or \"paint\" a \
         material, with a sphere (radius) or a trench (\"to\" offset). Lengths in meters.",
        r#"{"at": [0, 0, 0], "op": "remove", "radius": 3.0, "to": null, "material": "mud"}"#,
        |world, args| {
            let at = parse_vec3(args.get("at")).ok_or("Pass \"at\": [x, y, z] in meters.")?;
            let radius = args.get("radius").and_then(|v| v.as_f64()).unwrap_or(2.0) as f32;
            let op = match args.get("op").and_then(|v| v.as_str()).unwrap_or("remove") {
                "remove" | "dig" => EditOp::Remove,
                "add" | "build" => EditOp::Add,
                "smooth" => EditOp::Smooth { strength: 1.0 },
                "paint" => EditOp::Paint,
                other => {
                    return Err(format!(
                        "Unknown op \"{other}\". Use remove, add, smooth or paint."
                    ));
                }
            };
            let shape = match parse_vec3(args.get("to")) {
                Some(to) => EditShape::Capsule {
                    to: to.as_vec3(),
                    radius,
                },
                None => EditShape::Sphere { radius },
            };
            let surface = match args.get("material").and_then(|v| v.as_str()) {
                Some(name) => Some(world.resource::<SurfaceMaterials>().id(name).ok_or(
                    format!("Unknown material \"{name}\" (see SurfaceMaterials)."),
                )?),
                None => None,
            };
            world.write_message(TerrainEdit {
                center: at,
                shape,
                op,
                surface,
                source: None,
            });
            Ok(serde_json::json!({ "queued": true }))
        },
    )
    .register_api(
        "terrain.footprint",
        "Press a print into the ground as if something stepped there (kind: foot, paw, hoof, \
         wheel, track, body; yaw in radians; load in kg).",
        r#"{"at": [0, 0, 0], "kind": "foot", "yaw": 0.0, "load_kg": 80}"#,
        |world, args| {
            let at = parse_vec3(args.get("at")).ok_or("Pass \"at\": [x, y, z] in meters.")?;
            let kind = match args.get("kind").and_then(|v| v.as_str()).unwrap_or("foot") {
                "foot" => engine_world::ContactKind::Foot,
                "paw" => engine_world::ContactKind::Paw,
                "hoof" => engine_world::ContactKind::Hoof,
                "wheel" => engine_world::ContactKind::Wheel,
                "track" => engine_world::ContactKind::Track,
                "body" => engine_world::ContactKind::Body,
                other => return Err(format!("Unknown kind \"{other}\".")),
            };
            world.write_message(GroundContact {
                position: at,
                normal: Vec3::Y,
                size: [0.0, 0.0],
                yaw: args.get("yaw").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                load_kg: args.get("load_kg").and_then(|v| v.as_f64()).unwrap_or(80.0) as f32,
                speed: 1.0,
                kind,
                surface: None,
                entity: None,
            });
            Ok(serde_json::json!({ "queued": true }))
        },
    )
    .register_api(
        "terrain.stats",
        "Terrain patches drawn and built, footprint grid state.",
        "{}",
        |world, _| {
            let d = world.resource::<Diagnostics>();
            let map = world.resource::<DeformationMap>();
            let terrains = world.resource::<Terrains>();
            Ok(serde_json::json!({
                "terrains": terrains.entries.len(),
                "patches_drawn": d.get("terrain.patches_drawn"),
                "patches_resident": d.get("terrain.patches_resident"),
                "patches_built_last_frame": d.get("terrain.patches_built"),
                "prints_stamped": map.stamps,
                "print_grid_m": map.extent(),
            }))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_planet() -> App {
        let mut app = App::new();
        app.add_plugins(engine_world::WorldPlugin)
            .add_plugins(TerrainPlugin);
        let pad = FlatPad::default();
        let t = PlanetTerrain::earthlike(3).with_pad(pad);
        let tf = t.home_transform(pad);
        app.world.spawn((tf, t));
        app.world.spawn((
            Transform::from_xyz(0.0, 2.0, 0.0),
            engine_world::SimulationFocus::default(),
        ));
        app
    }

    #[test]
    fn ground_query_reports_the_pad() {
        let mut app = app_with_planet();
        app.update_n(2);
        let g = app.world.resource::<engine_world::Ground>().clone();
        let s = g.sample(DVec3::new(10.0, 5.0, -20.0)).expect("ground");
        assert!(s.height.abs() < 0.05, "pad height {}", s.height);
        assert!(s.normal.y > 0.999);
        let mats = app.world.resource::<SurfaceMaterials>();
        assert_eq!(mats.get(s.surface).name, "grass");
        // Far from the pad the ground has relief.
        let far = g.sample(DVec3::new(9_000.0, 0.0, 12_000.0)).unwrap();
        assert!(far.height.is_finite());
    }

    #[test]
    fn the_ground_is_installed_once_while_the_terrain_stays_put() {
        let mut app = app_with_planet();
        app.update_n(10);
        let d = app
            .world
            .resource::<engine_core::diagnostics::Diagnostics>();
        assert_eq!(d.get("terrain.ground_installs"), Some(1.0));
    }

    #[test]
    fn forest_cover_reports_woods_and_open_land() {
        let mut app = app_with_planet();
        app.update_n(2);
        let woods = app.world.resource::<engine_world::ForestCover>().clone();
        let (mut wooded, mut open) = (0, 0);
        for i in 0..20 {
            for j in 0..20 {
                let p = DVec3::new(
                    i as f64 * 2_000.0 - 20_000.0,
                    0.0,
                    j as f64 * 2_000.0 - 20_000.0,
                );
                match woods.sample(p) {
                    Some(s) => {
                        assert!(s.density > 0.0 && s.density <= 1.0, "{s:?}");
                        assert!((0.0..=1.0).contains(&s.conifer), "{s:?}");
                        if s.density > 0.5 {
                            wooded += 1;
                        }
                    }
                    None => open += 1,
                }
            }
        }
        assert!(wooded > 20 && open > 20, "wooded {wooded}, open {open}");
    }

    /// Cost of a forest lookup (`cargo test --release -p engine_terrain forest_cover_cost --
    /// --ignored --nocapture`).
    #[test]
    #[ignore]
    fn forest_cover_cost() {
        let mut app = app_with_planet();
        app.update_n(2);
        let woods = app.world.resource::<engine_world::ForestCover>().clone();
        let n = 20_000;
        let t = std::time::Instant::now();
        let mut wooded = 0;
        for i in 0..n {
            let p = DVec3::new(
                (i % 200) as f64 * 25.0 - 2_500.0,
                0.0,
                (i / 200) as f64 * 25.0 - 2_500.0,
            );
            wooded += woods.sample(p).is_some() as u32;
        }
        let us = t.elapsed().as_secs_f64() * 1e6 / n as f64;
        println!("forest lookup {us:.1} µs ({wooded} of {n} wooded)");
    }

    #[test]
    fn overlay_ground_passes_through_its_points() {
        // Also with the planet turned under the level (north toward +Z).
        for heading_deg in [0.0, 180.0] {
            overlay_passes_through_its_points(heading_deg);
        }
    }

    fn overlay_passes_through_its_points(heading_deg: f64) {
        let valley = |x: f64, z: f64| 12.0 + 0.02 * x + (z / 150.0).sin() * 6.0;
        let mut app = App::new();
        app.add_plugins(engine_world::WorldPlugin)
            .add_plugins(TerrainPlugin);
        let pad = FlatPad {
            heading_deg,
            ..FlatPad::default()
        };
        let t = PlanetTerrain::earthlike(3)
            .with_radius(6_371_000.0)
            .with_pad(pad)
            .with_overlay(8_000.0, 3_000.0, valley);
        // North is where the heading turned it.
        let rot = t.home_transform(pad).rotation.as_dquat();
        let r = 6_371_060.0;
        let to_pole = rot * (dir_from_lat_lon(40.01, 0.0) * r - dir_from_lat_lon(40.0, 0.0) * r);
        let north_z = if heading_deg == 0.0 { -1.0 } else { 1.0 };
        assert!(
            to_pole.z * north_z > 1_000.0 && to_pole.x.abs() < 1.0,
            "{to_pole:?}"
        );
        let tf = t.home_transform(pad);
        app.world.spawn((tf, t));
        app.world.spawn((
            Transform::from_xyz(0.0, 2.0, 0.0),
            engine_world::SimulationFocus::default(),
        ));
        app.update_n(2);
        let g = app.world.resource::<engine_world::Ground>().clone();
        // Exact (to a centimeter) despite the planet's curvature, near and far.
        for (x, z) in [
            (0.0, 0.0),
            (40.0, -25.0),
            (-3_000.0, 1_500.0),
            (6_000.0, 5_000.0),
        ] {
            let h = g.height(DVec3::new(x, 500.0, z)).unwrap();
            assert!(
                (h - valley(x, z)).abs() < 0.01,
                "{heading_deg}°: ({x}, {z}): {h} vs {}",
                valley(x, z)
            );
        }
        // Back to the planet well outside the blend (the plain is not tilted there).
        let far = g.height(DVec3::new(0.0, 500.0, -14_000.0)).unwrap();
        assert!((far - valley(0.0, -14_000.0)).abs() > 1.0);
    }

    #[test]
    fn overlay_paint_shows_and_reports_its_ground() {
        let mut app = App::new();
        app.add_plugins(engine_world::WorldPlugin)
            .add_plugins(TerrainPlugin);
        let pad = FlatPad::default();
        let t = PlanetTerrain::earthlike(3)
            .with_radius(6_371_000.0)
            .with_pad(pad)
            .with_overlay(8_000.0, 3_000.0, |_, _| 5.0)
            .with_overlay_paint(|x, z| {
                if x < 0.0 {
                    Some(OverlayPaint::layer("sand", 1.0))
                } else if z < 0.0 {
                    Some(OverlayPaint::tint([0.36, 0.27, 0.09], 0.8))
                } else {
                    None
                }
            });
        let shape = t.shape.clone();
        let tf = t.home_transform(pad);
        app.world.spawn((tf, t));
        app.world.spawn((
            Transform::from_xyz(0.0, 2.0, 0.0),
            engine_world::SimulationFocus::default(),
        ));
        app.update_n(2);
        let g = app.world.resource::<engine_world::Ground>().clone();
        let mats = app.world.resource::<SurfaceMaterials>();
        let surface = |x: f64, z: f64| {
            let s = g.sample(DVec3::new(x, 50.0, z)).expect("ground");
            mats.get(s.surface).name.clone()
        };
        // The painted layer is what footsteps find; a tint alone keeps the planet's ground.
        assert_eq!(surface(-30.0, 20.0), "sand");
        assert_eq!(surface(30.0, -20.0), "grass");
        assert_eq!(surface(30.0, 20.0), "grass");

        // The vertices carry the layer and the tint, fading out with the overlay's blend.
        let src = ChunkSource {
            map: SurfaceMap::new(TerrainDomain::Sphere, shape.radius_m),
            shape: std::sync::Arc::new(shape),
            edits: std::sync::Arc::new(EditLayer::default()),
            ocean_surface: true,
        };
        let inv = tf.rotation.as_dquat().inverse();
        let at = |x: f64, z: f64| {
            let local = inv * (DVec3::new(x, 5.0, z) - tf.translation);
            let (dir, up, base) = src.map.locate(local).unwrap();
            src.sample(base, up, dir, 0.0)
        };
        let sand = at(-30.0, 20.0);
        assert_eq!(sand.paint.0, paint_index("sand"));
        assert!((sand.paint.1 - 1.0).abs() < 1e-6);
        let field = at(30.0, -20.0);
        assert_eq!(field.paint.1, 0.0);
        assert!((field.tint[3] - 0.8).abs() < 1e-6 && (field.tint[0] - 0.36).abs() < 1e-6);
        assert_eq!(at(30.0, 20.0).tint[3], 0.0);
        let edge = at(-9_500.0, 0.0);
        assert!(edge.paint.1 > 0.0 && edge.paint.1 < 1.0, "{}", edge.paint.1);
        assert_eq!(at(-12_000.0, 0.0).paint.1, 0.0);
    }

    #[test]
    fn edits_dig_and_report() {
        let mut app = app_with_planet();
        app.update();
        app.world.write_message(TerrainEdit {
            center: DVec3::new(30.0, 0.0, 30.0),
            shape: EditShape::Sphere { radius: 4.0 },
            op: EditOp::Remove,
            surface: None,
            source: None,
        });
        app.update_n(2);
        let g = app.world.resource::<engine_world::Ground>().clone();
        let h = g.height(DVec3::new(30.0, 5.0, 30.0)).unwrap();
        assert!(h < -3.0, "crater floor {h}");
        let t = app.world.resource::<Terrains>();
        assert_eq!(t.entries.values().next().unwrap().edits.len(), 1);
    }

    /// An override names the ground; height, normal and edits stay the terrain's, and a layer
    /// an edit painted wins over the override.
    #[test]
    fn ground_surface_override_names_the_ground() {
        struct EastSand(engine_world::SurfaceId);
        impl engine_world::GroundQuery for EastSand {
            fn sample(&self, p: DVec3) -> Option<engine_world::GroundSample> {
                (p.x > 0.0).then_some(engine_world::GroundSample {
                    height: 1_000.0,
                    normal: Vec3::X,
                    surface: self.0,
                })
            }
        }
        let mut app = App::new();
        app.add_plugins(engine_world::WorldPlugin)
            .add_plugins(TerrainPlugin);
        let mats = app.world.resource::<SurfaceMaterials>();
        let (sand, grass, mud) = (mats.id("sand").unwrap(), mats.id("grass"), mats.id("mud"));
        let pad = FlatPad::default();
        let t = PlanetTerrain::earthlike(3)
            .with_pad(pad)
            .with_ground_surface(Arc::new(EastSand(sand)));
        app.world.spawn((t.home_transform(pad), t));
        app.world.spawn((
            Transform::from_xyz(0.0, 2.0, 0.0),
            engine_world::SimulationFocus::default(),
        ));
        app.update_n(2);
        let g = app.world.resource::<engine_world::Ground>().clone();
        let east = g.sample(DVec3::new(10.0, 5.0, -20.0)).expect("ground");
        assert!(east.height.abs() < 0.05, "pad height {}", east.height);
        assert!(east.normal.y > 0.999);
        assert_eq!(east.surface, sand);
        let west = g.sample(DVec3::new(-10.0, 5.0, -20.0)).expect("ground");
        assert_eq!(Some(west.surface), grass);

        app.world.write_message(TerrainEdit {
            center: DVec3::new(20.0, 0.0, 20.0),
            shape: EditShape::Sphere { radius: 6.0 },
            op: EditOp::Paint,
            surface: mud,
            source: None,
        });
        app.world.write_message(TerrainEdit {
            center: DVec3::new(30.0, 0.0, -30.0),
            shape: EditShape::Sphere { radius: 4.0 },
            op: EditOp::Remove,
            surface: None,
            source: None,
        });
        app.update_n(2);
        let g = app.world.resource::<engine_world::Ground>().clone();
        let painted = g.sample(DVec3::new(20.0, 5.0, 20.0)).unwrap();
        assert_eq!(Some(painted.surface), mud);
        let pit = g.sample(DVec3::new(30.0, 5.0, -30.0)).unwrap();
        assert!(pit.height < -3.0, "pit floor {}", pit.height);
    }

    #[test]
    fn contacts_leave_prints_in_painted_mud() {
        let mut app = app_with_planet();
        app.update();
        let mud = app.world.resource::<SurfaceMaterials>().id("mud");
        app.world.write_message(TerrainEdit {
            center: DVec3::new(5.0, 0.0, 0.0),
            shape: EditShape::Sphere { radius: 6.0 },
            op: EditOp::Paint,
            surface: mud,
            source: None,
        });
        app.update_n(2);
        app.world.write_message(GroundContact {
            position: DVec3::new(5.0, 0.0, 0.0),
            normal: Vec3::Y,
            size: [0.0, 0.0],
            yaw: 0.0,
            load_kg: 80.0,
            speed: 1.0,
            kind: engine_world::ContactKind::Foot,
            surface: None,
            entity: None,
        });
        // Fixed steps need time to pass.
        for _ in 0..10 {
            app.world
                .resource_mut::<Time>()
                .set_source(engine_core::time::TimeSource::Manual { step_secs: 0.05 });
            app.update();
        }
        let map = app.world.resource::<DeformationMap>();
        assert!(map.stamps >= 1, "no prints");
        let (d, _) = map.sample(DVec3::new(5.0, 0.0, -0.05));
        assert!(d > 0.02, "mud print depth {d}");
    }

    #[test]
    fn apis_are_registered() {
        let app = app_with_planet();
        let apis = app.world.resource::<ApiRegistry>();
        for name in [
            "terrain.height",
            "terrain.edit",
            "terrain.footprint",
            "terrain.stats",
        ] {
            assert!(apis.get(name).is_some(), "{name}");
        }
    }
}
