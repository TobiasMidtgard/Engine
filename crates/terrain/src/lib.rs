#![doc = include_str!("../README.md")]

pub mod chunk;
pub mod collider;
pub mod cube;
pub mod deform;
pub mod demo;
pub mod edits;
pub mod far_look;
pub mod forest;
pub mod ground;
pub mod landforms;
pub mod landmarks;
pub mod lights;
pub mod lod;
pub mod materials;
pub mod noise;
pub mod paint_map;
pub mod palette;
pub mod render;
pub mod rivers;
pub mod rocks;
pub mod shape;
pub mod wear;
pub mod worlds;

use std::collections::{HashMap, HashSet};
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
pub use far_look::{FarLook, FarLookMap};
pub use ground::GroundSurface;
pub use landmarks::{Landmark, LandmarkField, LandmarkKind, landmarks_near};
pub use paint_map::{PaintMap, paint_footprint};
pub use palette::TerrainPalette;
pub use shape::{
    Climate, FlatPad, HeightOverlay, OverlayPaintFn, PlanetShape, ShapeSample, dir_from_lat_lon,
    lat_lon_from_dir,
};
pub use wear::{PathWear, TrailWalk, WearSettings, WornGround};
pub use worlds::{WorldKind, WorldRecipe};

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
    /// Stones and boulders strewn over the ground near the focus (none by default).
    pub rocks: rocks::RockField,
    /// Arches, hoodoos, buttes, tors and ice spires standing on the ground (none by default).
    pub landmarks: landmarks::LandmarkField,
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
            rocks: rocks::RockField::default(),
            landmarks: landmarks::LandmarkField::earth(),
        }
    }

    /// A cratered, airless moon.
    pub fn moon(seed: u64) -> Self {
        Self {
            shape: PlanetShape::moon(seed),
            palette: TerrainPalette::moon(),
            ocean_surface: false,
            rocks: rocks::RockField::moon(),
            landmarks: landmarks::LandmarkField::default(),
            ..Self::earthlike(seed)
        }
    }

    /// A red desert planet.
    pub fn mars(seed: u64) -> Self {
        Self {
            shape: PlanetShape::mars(seed),
            palette: TerrainPalette::mars(),
            ocean_surface: false,
            rocks: rocks::RockField::mars(),
            landmarks: landmarks::LandmarkField::mars(),
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

    /// Terrain-local up at a terrain-local point.
    pub fn local_up(&self, local: DVec3) -> DVec3 {
        if self.source.map.is_sphere() {
            local.normalize_or(DVec3::Y)
        } else {
            DVec3::Y
        }
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
            .insert_resource(PathWear::new(WearSettings::default().cell_m))
            .init_resource::<WearSettings>()
            .init_resource::<WearProvider>()
            .init_resource::<collider::TerrainColliders>()
            .init_resource::<rocks::RockCells>()
            .init_resource::<landmarks::LandmarkCells>()
            .init_resource::<RecoverClock>()
            .init_resource::<lights::CityLights>()
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
            .register_resource::<WearSettings>(
                "Worn paths: cell size (m), how fast paths form (gain_scale) and heal \
                 (heal_scale, multiplies the healing half-lives), drawn window (cells).",
            )
            .add_systems(
                PostUpdate,
                (
                    sync_terrains,
                    lights::register_lamps,
                    apply_edits,
                    collider::update_colliders,
                    rocks::update_rocks,
                    landmarks::update_landmarks,
                )
                    .chain()
                    .in_set(UpdateSet::AfterPropagate),
            )
            .add_systems(PostUpdate, follow_world_shift.in_set(WorldShiftSet::Apply))
            .add_systems(
                PostUpdate,
                shift_wear_provider.in_set(WorldShiftSet::Rebuild),
            )
            .add_systems(
                FixedUpdate,
                (demo::walk, demo::swing_limbs)
                    .chain()
                    .in_set(SimSet::Environment),
            )
            // Prints and paths take the contacts of all of a frame's fixed steps at once.
            .add_systems(
                PostUpdate,
                (deform_from_contacts, wear_from_contacts)
                    .chain()
                    .before(UpdateSet::AfterPropagate),
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

#[allow(clippy::too_many_arguments)]
fn sync_terrains(
    mut terrains: ResMut<Terrains>,
    q: Query<PlanetPlacement>,
    materials: Res<SurfaceMaterials>,
    mut ground: ResMut<engine_world::Ground>,
    mut forest: ResMut<engine_world::ForestCover>,
    diagnostics: Option<ResMut<Diagnostics>>,
    focus: Option<Res<engine_world::FocusPoints>>,
    // Whether a provider is installed, and how many times one was (a diagnostic: the ground
    // is rebuilt only when a terrain is added, removed or moved).
    mut installed: Local<(bool, u32)>,
    // The terrains that answer the ground now: those near the focus.
    mut answering: Local<HashSet<Entity>>,
) {
    let mut changed = false;
    let mut seen = Vec::with_capacity(terrains.entries.len());
    let points: &[DVec3] = focus.as_ref().map_or(&[], |f| &f.points);
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
        let moved = t.position != pos || t.rotation != rot;
        t.position = pos;
        t.rotation = rot;
        // A planet answers the ground while the focus is within about its radius of it (a
        // little farther to stop answering), so the world's ground is the one underfoot and
        // far planets, which move every frame, leave the providers alone.
        let reach = if answering.contains(&e) { 1.25 } else { 1.0 };
        let near = points.is_empty()
            || !t.source.map.is_sphere()
            || points
                .iter()
                .any(|&p| t.altitude_hint(p) < t.source.map.radius * reach);
        let answers = t.config.ground && near;
        if answers != answering.contains(&e) {
            if answers {
                answering.insert(e);
            } else {
                answering.remove(&e);
            }
            changed = true;
        }
        changed |= answers && (moved || reset);
    }
    answering.retain(|e| seen.contains(e));
    let before = terrains.entries.len();
    terrains.entries.retain(|e, _| seen.contains(e));
    changed |= terrains.entries.len() != before;
    if !changed {
        return;
    }
    let list: Vec<_> = terrains
        .entries
        .iter()
        .filter(|(e, _)| answering.contains(e))
        .map(|(_, t)| t.ground_terrain())
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
    range: Option<Res<engine_world::SimulationRange>>,
) {
    if !settings.enabled || terrains.entries.is_empty() {
        contacts.clear();
        return;
    }
    let range = range.map(|r| *r).unwrap_or_default();
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
    // (Prints beyond the simulated range would never be seen up close.)
    for c in contacts
        .read()
        .filter(|c| range.is_active(&env.focus, c.position))
    {
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

/// Accumulates time for the low-rate path healing pass.
#[derive(Default)]
struct HealClock(f32);

/// Wears paths where feet, hooves and wheels land, and lets unused ones heal.
#[allow(clippy::too_many_arguments)]
fn wear_from_contacts(
    mut contacts: MessageReader<GroundContact>,
    mut wear: ResMut<PathWear>,
    settings: Res<WearSettings>,
    env: Env,
    terrains: Res<Terrains>,
    time: Res<Time>,
    weather: Res<WeatherState>,
    surface_layers: Res<SurfaceLayers>,
    mut clock: Local<HealClock>,
    mut provider: ResMut<WearProvider>,
    mut ground_wear: ResMut<engine_world::GroundWear>,
    range: Option<Res<engine_world::SimulationRange>>,
) {
    if !settings.enabled || terrains.entries.is_empty() {
        contacts.clear();
        return;
    }
    let range = range.map(|r| *r).unwrap_or_default();
    if (wear.cell_m - settings.cell_m).abs() > 1e-6 {
        *wear = PathWear::new(settings.cell_m.clamp(0.1, 4.0));
    }
    let gone: Vec<Entity> = wear
        .terrains
        .keys()
        .filter(|e| !terrains.entries.contains_key(e))
        .copied()
        .collect();
    for e in gone {
        wear.forget(e);
    }
    // (Paths are worn where people walk while the player is about; far off, footfalls rest
    // with everything else.)
    for c in contacts
        .read()
        .filter(|c| range.is_active(&env.focus, c.position))
    {
        let Some((entity, t)) = terrains.nearest(c.position) else {
            continue;
        };
        let local = t.to_local(c.position);
        if t.altitude_hint(c.position).abs() > 20_000.0 {
            continue;
        }
        let layers = surface_layers.sample(c.position);
        let ground = if layers.snow_m > 0.02 {
            WornGround::Snow
        } else {
            let surface = c
                .surface
                .or(env.ground(c.position).map(|g| g.surface))
                .unwrap_or_default();
            WornGround::of(env.material(surface))
        };
        wear.tread(
            entity,
            local,
            t.local_up(local),
            c.size,
            c.load_kg,
            c.kind,
            ground,
            layers.wetness,
            settings.gain_scale,
        );
    }
    clock.0 += time.delta_secs();
    if clock.0 >= 2.0 {
        let dt = clock.0;
        clock.0 = 0.0;
        // Snowfall: precipitation in mm/h of water ≈ 10× that in fresh snow.
        let snowfall = if weather.is_snowing() {
            weather.precipitation_mm_h * 0.01 / 3600.0
        } else {
            0.0
        };
        wear.heal(dt, snowfall, settings.heal_scale);
    }
    // What other modules read (`GroundWear`): a fresh snapshot a few times a second while
    // paths change.
    provider.since += time.delta_secs();
    if wear.revision != provider.revision && provider.since >= 0.5 {
        provider.since = 0.0;
        provider.revision = wear.revision;
        let snap = Arc::new(wear::WearSnapshot::of(&wear, |e| {
            terrains.entries.get(&e).map(|t| (t.position, t.rotation))
        }));
        *ground_wear = engine_world::GroundWear(snap.clone());
        provider.snapshot = Some(snap);
    }
}

/// The snapshot installed as `GroundWear`, and when it was taken.
#[derive(Resource, Default)]
struct WearProvider {
    snapshot: Option<Arc<wear::WearSnapshot>>,
    revision: u64,
    since: f32,
}

/// After the world re-anchors, `GroundWear` answers in the new frame at once.
fn shift_wear_provider(
    mut shifts: MessageReader<WorldShift>,
    mut provider: ResMut<WearProvider>,
    mut ground_wear: ResMut<engine_world::GroundWear>,
) {
    let Some(s) = shifts.read().copied().reduce(|a, b| a.then(&b)) else {
        return;
    };
    if let Some(snap) = &provider.snapshot {
        let moved = Arc::new(snap.shifted(&s));
        *ground_wear = engine_world::GroundWear(moved.clone());
        provider.snapshot = Some(moved);
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
        "terrain.wear",
        "How worn the ground is at a world point: 0 untouched .. 1 a well-worn path (grass \
         trodden to bare earth, snow packed to ice, a trough in sand).",
        r#"{"at": [0, 0, 0]}"#,
        |world, args| {
            let at = parse_vec3(args.get("at")).ok_or("Pass \"at\": [x, y, z] in meters.")?;
            let terrains = world.resource::<Terrains>();
            let (entity, t) = terrains
                .nearest(at)
                .ok_or("There is no terrain here. Spawn an entity with PlanetTerrain first.")?;
            let wear = world.resource::<PathWear>().wear_at(entity, t.to_local(at));
            Ok(serde_json::json!({ "wear": wear }))
        },
    )
    .register_api(
        "terrain.walk_path",
        "Wear a path at once, as if `passes` people of `load_kg` had walked from `from` to \
         `to` (world points; snapped to the ground), each keeping within `spread_m` of the \
         line. The ground under the line decides how it wears.",
        r#"{"from": [0, 0, 0], "to": [30, 0, 0], "passes": 300, "load_kg": 80, "spread_m": 0.3}"#,
        |world, args| {
            let from = parse_vec3(args.get("from")).ok_or("Pass \"from\": [x, y, z].")?;
            let to = parse_vec3(args.get("to")).ok_or("Pass \"to\": [x, y, z].")?;
            let passes = args.get("passes").and_then(|v| v.as_u64()).unwrap_or(300) as u32;
            let load = args.get("load_kg").and_then(|v| v.as_f64()).unwrap_or(80.0) as f32;
            let spread = args.get("spread_m").and_then(|v| v.as_f64()).unwrap_or(0.3) as f32;
            let terrains = world.resource::<Terrains>();
            let mats = world.resource::<SurfaceMaterials>();
            let (entity, t) = terrains
                .nearest(from)
                .ok_or("There is no terrain here. Spawn an entity with PlanetTerrain first.")?;
            let ground = t.ground_terrain();
            let ids = palette::SurfaceIds::new(mats);
            let snap = |p: DVec3| ground.surface(p, &ids);
            let (a, _, surface, _) = snap(from).ok_or("No ground under \"from\".")?;
            let (b, _, _, _) = snap(to).ok_or("No ground under \"to\".")?;
            let kind = WornGround::of(mats.get(surface));
            let (la, lb) = (t.to_local(a), t.to_local(b));
            let up = t.local_up((la + lb) * 0.5);
            let mut wear = world.resource_mut::<PathWear>();
            wear.walk_line(
                entity,
                la,
                lb,
                up,
                passes.min(100_000),
                load,
                kind,
                spread,
                1,
            );
            Ok(serde_json::json!({ "ground": format!("{kind:?}").to_lowercase() }))
        },
    )
    .register_api(
        "terrain.walk_between",
        "Let people walk between places `trips` times and lay their own trails: each takes \
         the way that feels shortest, cutting across open ground but keeping to a worn path \
         that runs their way, so shared trails that fork emerge (and unused ones heal). \
         `places` are world points, each with how busy it is; `attraction` 0..0.9 is how \
         much easier a path feels than open ground; `trips_per_day` sets how much time \
         passes between trips for healing.",
        r#"{"places": [[0, 0, 0, 3], [120, 0, 40, 1], [90, 0, -80, 1]], "trips": 3000, "load_kg": 80, "attraction": 0.5, "trips_per_day": 800}"#,
        |world, args| {
            let places: Vec<(DVec3, f32)> = args
                .get("places")
                .and_then(|v| v.as_array())
                .ok_or("Pass \"places\": [[x, y, z, busy], ...].")?
                .iter()
                .filter_map(|p| {
                    let p = p.as_array()?;
                    let at = DVec3::new(p.first()?.as_f64()?, p.get(1)?.as_f64()?, p.get(2)?.as_f64()?);
                    let busy = p.get(3).and_then(|b| b.as_f64()).unwrap_or(1.0) as f32;
                    Some((at, busy))
                })
                .collect();
            if places.len() < 2 {
                return Err("Pass at least two places.".into());
            }
            let trips = args.get("trips").and_then(|v| v.as_u64()).unwrap_or(3000) as u32;
            let load = args.get("load_kg").and_then(|v| v.as_f64()).unwrap_or(80.0) as f32;
            let defaults = TrailWalk::default();
            let walk = TrailWalk {
                attraction: args
                    .get("attraction")
                    .and_then(|v| v.as_f64())
                    .map_or(defaults.attraction, |v| v as f32),
                trips_per_day: args
                    .get("trips_per_day")
                    .and_then(|v| v.as_f64())
                    .map_or(800.0, |v| v as f32),
                ..defaults
            };
            let terrains = world.resource::<Terrains>();
            let mats = world.resource::<SurfaceMaterials>();
            let (entity, t) = terrains
                .nearest(places[0].0)
                .ok_or("There is no terrain here. Spawn an entity with PlanetTerrain first.")?;
            let ground = t.ground_terrain();
            let ids = palette::SurfaceIds::new(mats);
            let (first, _, surface, _) = ground
                .surface(places[0].0, &ids)
                .ok_or("No ground under the first place.")?;
            let kind = WornGround::of(mats.get(surface));
            let mut local = Vec::with_capacity(places.len());
            for &(p, busy) in &places {
                let (at, _, _, _) = ground.surface(p, &ids).ok_or("No ground under a place.")?;
                local.push((t.to_local(at), busy));
            }
            let up = t.local_up(t.to_local(first));
            let mut wear = world.resource_mut::<PathWear>();
            wear.walk_between(entity, &local, up, trips.min(20_000), load, kind, walk, 1);
            Ok(serde_json::json!({ "ground": format!("{kind:?}").to_lowercase() }))
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

    /// A town's lamps in a streamed cell: they keep glowing after the cell unloads, are
    /// replaced (not doubled) when it loads again, and go with its scene.
    #[test]
    fn a_streamed_towns_lamps_outlive_its_unloading_until_its_scene_goes() {
        use engine_world::stream::CellInfo;
        use engine_world::{CellId, CellMember, CellState, StreamedCells};
        let mut app = app_with_planet();
        app.world
            .resource_mut::<StreamedCells>()
            .cells
            .push(CellInfo {
                key: "town".into(),
                center: DVec3::ZERO,
                radius: 100.0,
                load_m: 256.0,
                keep_loaded: false,
                state: CellState::Loaded,
                load_secs: 0.0,
                build_secs: 0.0,
                entities: 0,
                ram_mb: None,
                assets: 0,
                assets_mb: 0.0,
                releases_assets: false,
                frame: None,
                retired: false,
            });
        let lamp = |lm: f32| engine_render::PointLight {
            intensity_lm: lm,
            range: 30.0,
            ..Default::default()
        };
        let spawn_town = |app: &mut App| {
            let root = app
                .world
                .spawn((Transform::from_xyz(40.0, 0.0, 0.0), CellMember(CellId(0))))
                .id();
            let light = app
                .world
                .spawn((
                    lamp(20_000.0),
                    Transform::from_xyz(0.0, 8.0, 0.0),
                    ChildOf(root),
                ))
                .id();
            (root, light)
        };
        // A lamp of the world's own (no cell), and the town's.
        app.world
            .spawn((lamp(5_000.0), Transform::from_xyz(-60.0, 8.0, 0.0)));
        let (root, light) = spawn_town(&mut app);
        app.update_n(2);
        let lumens = |app: &App| app.world.resource::<lights::CityLights>().lumens();
        assert_eq!(lumens(&app), 25_000.0);
        // The player walks off: the cell unloads, its lamps stay lit.
        app.world.despawn(light);
        app.world.despawn(root);
        app.update_n(2);
        assert_eq!(lumens(&app), 25_000.0);
        // Back again: the cell spawns its lamp anew, in place of the kept one.
        let (root, light) = spawn_town(&mut app);
        app.update_n(2);
        assert_eq!(lumens(&app), 25_000.0);
        app.world.despawn(light);
        app.world.despawn(root);
        app.update_n(2);
        assert_eq!(lumens(&app), 25_000.0);
        // Its scene is unloaded for good.
        app.world.resource_mut::<StreamedCells>().cells[0].retired = true;
        app.update();
        assert_eq!(lumens(&app), 5_000.0);
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
    fn height_alone_matches_the_full_sample() {
        let mut app = app_with_planet();
        app.update_n(2);
        let g = app.world.resource::<engine_world::Ground>().clone();
        let points: Vec<DVec3> = (0..400)
            .map(|i| {
                let (a, r) = (i as f64 * 0.7, 30.0 + i as f64 * 40.0);
                DVec3::new(a.cos() * r, 50.0 + (i % 7) as f64 * 30.0, a.sin() * r)
            })
            .collect();
        let t = std::time::Instant::now();
        let full: Vec<_> = points
            .iter()
            .map(|&p| g.sample(p).map(|s| s.height))
            .collect();
        let full_secs = t.elapsed().as_secs_f64();
        let t = std::time::Instant::now();
        let alone: Vec<_> = points.iter().map(|&p| g.height(p)).collect();
        let alone_secs = t.elapsed().as_secs_f64();
        assert_eq!(full, alone);
        assert!(alone.iter().all(Option::is_some));
        eprintln!(
            "ground per point: sample {:.1} us, height {:.1} us",
            full_secs * 1e6 / points.len() as f64,
            alone_secs * 1e6 / points.len() as f64
        );
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
    fn moving_planets_without_ground_leave_the_providers_alone() {
        let mut app = app_with_planet();
        let mut moon = PlanetTerrain::moon(3).with_radius(1_737_400.0);
        moon.ground = false;
        let moon = app
            .world
            .spawn((Transform::from_xyz(0.0, 0.0, 384_400_000.0), moon))
            .id();
        app.update_n(3);
        for i in 0..5 {
            app.world.get_mut::<Transform>(moon).unwrap().translation.x += 1_000.0 * (i + 1) as f64;
            app.update();
        }
        let d = app
            .world
            .resource::<engine_core::diagnostics::Diagnostics>();
        assert_eq!(d.get("terrain.ground_installs"), Some(1.0));
    }

    #[test]
    fn the_ground_underfoot_is_the_planet_you_are_on() {
        let mut app = app_with_planet();
        // A moon with ground of its own, far off and moving: Earth answers alone.
        let moon = PlanetTerrain::moon(3).with_radius(1_737_400.0);
        let moon = app
            .world
            .spawn((Transform::from_xyz(0.0, 0.0, 384_400_000.0), moon))
            .id();
        app.update_n(3);
        for i in 0..5 {
            app.world.get_mut::<Transform>(moon).unwrap().translation.x += 1_000.0 * (i + 1) as f64;
            app.update();
        }
        let installs = |app: &App| {
            app.world
                .resource::<engine_core::diagnostics::Diagnostics>()
                .get("terrain.ground_installs")
        };
        assert_eq!(installs(&app), Some(1.0));
        // Fly down to the Moon: its ground answers there.
        let at = app.world.get::<Transform>(moon).unwrap().translation;
        let mut q = app
            .world
            .query_filtered::<&mut Transform, With<engine_world::SimulationFocus>>();
        q.single_mut(&mut app.world).unwrap().translation = at + DVec3::Y * 1_740_000.0;
        app.update_n(2);
        assert_eq!(installs(&app), Some(2.0));
        let g = app.world.resource::<engine_world::Ground>().clone();
        let s = g.sample(at + DVec3::Y * 1_739_000.0).expect("moon ground");
        assert!(s.normal.y > 0.5, "{:?}", s.normal);
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

    /// Far patches show hand-made ground as the mean over each vertex's cell: covers changing
    /// within a vertex spacing average out instead of aliasing into blocks.
    #[test]
    fn far_patches_average_hand_made_paint() {
        let pad = FlatPad::default();
        let t = PlanetTerrain::earthlike(3)
            .with_radius(6_371_000.0)
            .with_pad(pad)
            .with_overlay(8_000.0, 3_000.0, |_, _| 5.0)
            .with_overlay_paint(|x, _| {
                Some(if x.rem_euclid(24.0) < 12.0 {
                    OverlayPaint::layer("sand", 1.0).tinted([0.0; 3], 1.0)
                } else {
                    OverlayPaint::layer("rock", 1.0).tinted([1.0; 3], 1.0)
                })
            });
        let tf = t.home_transform(pad);
        let src = ChunkSource {
            map: SurfaceMap::new(TerrainDomain::Sphere, t.shape.radius_m),
            shape: std::sync::Arc::new(t.shape.clone()),
            edits: std::sync::Arc::new(EditLayer::default()),
            ocean_surface: true,
        };
        let local =
            tf.rotation.as_dquat().inverse() * (DVec3::new(1_500.0, 5.0, 1_500.0) - tf.translation);
        let level = (0..24)
            .find(|&l| src.map.node_size(l) / chunk::QUADS as f64 <= 40.0)
            .unwrap();
        let key = src.map.node_at(local, level).unwrap();
        let c = chunk::build_chunk(&src, key);
        assert!(c.spacing > 10.0, "{}", c.spacing);
        let (sand, rock) = (
            paint_index("sand") as usize - 1,
            paint_index("rock") as usize - 1,
        );
        for v in &c.vertices[..chunk::GRID * chunk::GRID] {
            let (s, r) = (v.paint[sand] as f32 / 255.0, v.paint[rock] as f32 / 255.0);
            assert!((s + r - 1.0).abs() < 0.02, "{s} + {r}");
            assert!((0.2..=0.8).contains(&s), "sand {s}");
            // The tint is the cells' mean too: gray between the black and white stripes.
            let gray = (v.tint[0] as f32 / 255.0).powi(2);
            assert!((0.2..=0.8).contains(&gray), "tint {gray}");
            assert_eq!(v.tint[3], 255);
        }
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
    fn a_crowd_wears_a_path_across_the_meadow() {
        let mut app = app_with_planet();
        // Game time passes by the clock, not by how fast the machine runs (fixed steps, and
        // `GroundWear`'s snapshot at most every half second).
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual { step_secs: 0.05 });
        app.update();
        {
            let mut commands = app.world.commands();
            demo::spawn_path_crowd(
                &mut commands,
                DVec3::new(0.0, 0.0, 30.0),
                20.0,
                12,
                600.0,
                false,
            );
        }
        app.world.flush();
        app.update_n(12);
        let wear_at = |app: &mut App, p: DVec3| {
            let v = engine_core::registry::call_api(
                &mut app.world,
                "terrain.wear",
                serde_json::json!({ "at": p.to_array() }),
            )
            .unwrap();
            v["wear"].as_f64().unwrap()
        };
        let on = wear_at(&mut app, DVec3::new(0.0, 0.0, 30.0));
        let off = wear_at(&mut app, DVec3::new(0.0, 0.0, 34.0));
        assert!(on > 0.3, "the crowd's line wore only {on}");
        assert!(off < 0.01, "4 m to the side wore {off}");
        // Other modules see it through `GroundWear`.
        app.update_n(40);
        let shared = app.world.resource::<engine_world::GroundWear>().clone();
        let seen = shared.wear(DVec3::new(0.0, 0.0, 30.0)) as f64;
        assert!(
            (seen - on).abs() < 0.1,
            "GroundWear says {seen}, the map {on}"
        );
        assert!(shared.wear(DVec3::new(0.0, 0.0, 34.0)) < 0.01);
        // After the world re-anchors it answers in the new frame.
        app.world
            .write_message(WorldShift::translate(DVec3::new(100.0, 0.0, 0.0)));
        app.update();
        let shared = app.world.resource::<engine_world::GroundWear>().clone();
        let moved = shared.wear(DVec3::new(100.0, 0.0, 30.0)) as f64;
        assert!(moved > 0.2, "after a shift GroundWear says {moved}");
    }

    #[test]
    fn apis_are_registered() {
        let app = app_with_planet();
        let apis = app.world.resource::<ApiRegistry>();
        for name in [
            "terrain.height",
            "terrain.edit",
            "terrain.wear",
            "terrain.walk_path",
            "terrain.walk_between",
            "terrain.footprint",
            "terrain.stats",
        ] {
            assert!(apis.get(name).is_some(), "{name}");
        }
    }
}
