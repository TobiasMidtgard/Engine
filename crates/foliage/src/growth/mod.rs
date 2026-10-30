//! Growing plants from a graph of generators, over time.
//!
//! A [`PlantGraph`] describes a plant the way SpeedTree's generator stack or Unreal's
//! Procedural Vegetation Editor do, with time added as in Prusinkiewicz et al., "Animation of
//! plant development" (1993): stems ([`graph::Axis`]: a rosette crown, an upright shoot, a
//! runner) lay down nodes one plastochron apart, and every node grows the generators its
//! children name, turned by a phyllotaxis (137.5 degrees spiral, opposite, whorled) and
//! angled out of it: leaves (simple, trifoliate, palmate, pinnate, toothed), flowers that
//! swell, open and drop their petals, and fruit that swells from the flower and ripens. A
//! runner can grow a daughter plant, a branch can grow branches. Graphs are plain data, saved
//! and edited as JSON or RON (`foliage.grow` takes one).
//!
//! The graph is developed once into a [`Skeleton`] of every organ the plant will have, each
//! with the day it starts and the day it is dropped, and with random numbers keyed by its place
//! in the plant, so growing on never changes what is already there. The plant is then drawn at
//! any age ([`shape::grow_meshes`]): each organ grows along its own smooth curve and starts at
//! size zero where it attaches, so a film of it growing has no pops. Research and sources:
//! engine-design/foliage-research.md, section 1.
//!
//! [`GrowingPlant`] puts one in the world and moves its age on with the game clock (a time-lapse
//! when `days_per_second` is high), rebuilding its meshes as it grows.

pub mod develop;
pub mod graph;
pub mod shape;
pub mod tree;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_core::tasks::AsyncComputeTaskPool;
use engine_render::{Image, Mesh, MeshRenderer, NoVirtualGeometry, StandardMaterial};

pub use develop::Skeleton;
pub use graph::{
    Axis, AxisKind, Child, Flower, Fruit, Leaf, LeafKind, Node, Phyllotaxis, PlantGraph, Tree,
};
pub use shape::{GrownMeshes, grow_meshes};

use crate::assets::FoliageVisuals;
use crate::species::PlantLibrary;

/// The graphs that come with the engine: `strawberry`, `sapling`, `oak`.
pub fn presets() -> Vec<PlantGraph> {
    vec![strawberry(), sapling(), oak()]
}

/// A built-in graph by name.
pub fn preset(name: &str) -> Option<PlantGraph> {
    presets()
        .into_iter()
        .find(|g| g.name.eq_ignore_ascii_case(name))
}

fn nodes(list: Vec<(&str, Node)>) -> BTreeMap<String, Node> {
    list.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

/// A garden strawberry (Fragaria x ananassa) from a seedling: a rosette crown of trifoliate,
/// toothed leaves on long petioles, a leaf every week; trusses of white five-petalled flowers
/// from the fifth leaf on, each berry swelling from its flower's center and ripening green,
/// white, turning, red in about four weeks, the tip first; runners from midsummer that lie on
/// the ground and root a daughter plant at their second node. Old leaves redden and drop.
pub fn strawberry() -> PlantGraph {
    PlantGraph {
        name: "strawberry".into(),
        root: "crown".into(),
        life_days: 150.0,
        bark_species: None,
        nodes: nodes(vec![
            (
                "crown",
                Node::Axis(Axis {
                    kind: AxisKind::Rosette,
                    plastochron_days: 7.0,
                    max_nodes: 18,
                    internode_m: (0.0008, 0.0012),
                    internode_days: 4.0,
                    tip_radius_m: 0.004,
                    thickening_m: 0.0005,
                    gravitropism: 0.0,
                    wander_deg: 0.0,
                    color: [0.20, 0.22, 0.06],
                    wood_color: [0.22, 0.12, 0.06],
                    children: vec![
                        Child {
                            spawn: "leaf".into(),
                            angle_deg: 30.0,
                            phyllotaxis: Phyllotaxis::Spiral(137.5),
                            ..Default::default()
                        },
                        Child {
                            spawn: "truss".into(),
                            from_node: 5,
                            every: 3,
                            count: 3,
                            angle_deg: 22.0,
                            delay_days: 3.0,
                            phyllotaxis: Phyllotaxis::Spiral(137.5 + 60.0),
                            ..Default::default()
                        },
                        Child {
                            spawn: "runner".into(),
                            from_node: 8,
                            every: 4,
                            count: 2,
                            angle_deg: 70.0,
                            delay_days: 2.0,
                            phyllotaxis: Phyllotaxis::Spiral(137.5 + 150.0),
                            max_depth: 1,
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                }),
            ),
            (
                "leaf",
                Node::Leaf(Leaf {
                    kind: LeafKind::Trifoliate,
                    petiole_m: (0.07, 0.13),
                    petiole_radius_m: 0.0016,
                    blade_m: (0.045, 0.06),
                    width: 0.72,
                    widest: 0.55,
                    teeth: 9,
                    tooth_depth: 0.22,
                    droop: 0.32,
                    grow_days: 14.0,
                    unfold: 0.4,
                    senesce_days: 60.0,
                    fade_days: 9.0,
                    color: [0.045, 0.15, 0.025],
                    old_color: [0.42, 0.10, 0.03],
                }),
            ),
            (
                "truss",
                Node::Axis(Axis {
                    plastochron_days: 1.5,
                    max_nodes: 3,
                    internode_m: (0.035, 0.05),
                    internode_days: 6.0,
                    tip_radius_m: 0.0012,
                    thickening_m: 0.0004,
                    gravitropism: 1.2,
                    wander_deg: 8.0,
                    sag: 0.9,
                    color: [0.18, 0.28, 0.07],
                    apex: Some("flower".into()),
                    children: vec![Child {
                        spawn: "pedicel".into(),
                        from_node: 1,
                        count: 2,
                        angle_deg: 40.0,
                        delay_days: 2.0,
                        scale: 0.88,
                        phyllotaxis: Phyllotaxis::Alternate,
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
            ),
            (
                "pedicel",
                Node::Axis(Axis {
                    plastochron_days: 1.0,
                    max_nodes: 2,
                    internode_m: (0.025, 0.035),
                    internode_days: 5.0,
                    tip_radius_m: 0.001,
                    thickening_m: 0.0003,
                    gravitropism: 0.8,
                    wander_deg: 10.0,
                    sag: 0.7,
                    color: [0.18, 0.28, 0.07],
                    apex: Some("flower".into()),
                    ..Default::default()
                }),
            ),
            (
                "flower",
                Node::Flower(Flower {
                    petals: 5,
                    petal_m: (0.011, 0.0105),
                    petal_color: [0.88, 0.87, 0.82],
                    center_color: [0.72, 0.68, 0.12],
                    sepals: 10,
                    sepal_color: [0.09, 0.22, 0.04],
                    stamens: 20,
                    bud_days: 12.0,
                    open_days: 1.5,
                    bloom_days: 4.5,
                    cup: 0.2,
                    then: Some("berry".into()),
                }),
            ),
            (
                "berry",
                Node::Fruit(Fruit {
                    length_m: 0.034,
                    width_m: 0.03,
                    point: 0.45,
                    swell_days: 21.0,
                    colors: vec![
                        (0.0, [0.20, 0.34, 0.07]),
                        (0.55, [0.32, 0.44, 0.12]),
                        (0.68, [0.78, 0.78, 0.60]),
                        (0.8, [0.78, 0.30, 0.20]),
                        (0.92, [0.55, 0.03, 0.03]),
                        (1.05, [0.40, 0.01, 0.02]),
                    ],
                    ripe_days: 28.0,
                    seeds: 140,
                    seed_color: [0.50, 0.58, 0.14],
                    drop_days: 0.0,
                }),
            ),
            (
                "runner",
                Node::Axis(Axis {
                    kind: AxisKind::Stolon,
                    plastochron_days: 3.0,
                    max_nodes: 3,
                    internode_m: (0.12, 0.17),
                    taper_length: 0.0,
                    internode_days: 6.0,
                    tip_radius_m: 0.0012,
                    thickening_m: 0.0002,
                    gravitropism: -0.6,
                    plagiotropism: 2.0,
                    wander_deg: 6.0,
                    color: [0.17, 0.06, 0.035],
                    wood_color: [0.14, 0.07, 0.04],
                    children: vec![Child {
                        spawn: "crown".into(),
                        from_node: 2,
                        count: 1,
                        upright: true,
                        angle_deg: 0.0,
                        delay_days: 4.0,
                        scale: 0.8,
                        max_depth: 2,
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
            ),
        ]),
    }
}

/// A broadleaf sapling over three years: a leader laying down a node every twelve days, side
/// branches spiralling off it and twigs off those, each node with a toothed leaf that lives
/// half a year; wood thickens with what grows above it and darkens to bark, and the lowest
/// branches die and drop as the crown climbs.
pub fn sapling() -> PlantGraph {
    // Leaves come back every spring at every node (one leaf a node a year).
    let leaf_child = |angle: f32| Child {
        spawn: "leaf".into(),
        angle_deg: angle,
        phyllotaxis: Phyllotaxis::Alternate,
        repeat_days: 365.0,
        ..Default::default()
    };
    PlantGraph {
        name: "sapling".into(),
        root: "leader".into(),
        life_days: 1100.0,
        bark_species: Some("oak".into()),
        nodes: nodes(vec![
            (
                "leader",
                Node::Axis(Axis {
                    plastochron_days: 10.0,
                    max_nodes: 100,
                    internode_m: (0.05, 0.075),
                    taper_length: 0.35,
                    internode_days: 20.0,
                    tip_radius_m: 0.003,
                    thickening_m: 0.0012,
                    gravitropism: 0.5,
                    wander_deg: 6.0,
                    color: [0.20, 0.26, 0.07],
                    wood_color: OAK_BARK,
                    lignify_days: 50.0,
                    children: vec![
                        Child {
                            spawn: "branch".into(),
                            from_node: 4,
                            every: 2,
                            angle_deg: 55.0,
                            delay_days: 25.0,
                            scale: 0.85,
                            ..Default::default()
                        },
                        leaf_child(50.0),
                    ],
                    ..Default::default()
                }),
            ),
            (
                "branch",
                Node::Axis(Axis {
                    plastochron_days: 14.0,
                    max_nodes: 20,
                    internode_m: (0.05, 0.075),
                    taper_length: 0.5,
                    internode_days: 20.0,
                    tip_radius_m: 0.002,
                    thickening_m: 0.0009,
                    gravitropism: 0.3,
                    plagiotropism: 0.2,
                    wander_deg: 10.0,
                    sag: 0.25,
                    color: [0.20, 0.26, 0.07],
                    wood_color: OAK_BARK,
                    lignify_days: 70.0,
                    life_days: 950.0,
                    children: vec![
                        Child {
                            spawn: "twig".into(),
                            from_node: 1,
                            every: 1,
                            angle_deg: 48.0,
                            delay_days: 20.0,
                            scale: 0.8,
                            phyllotaxis: Phyllotaxis::Alternate,
                            ..Default::default()
                        },
                        leaf_child(55.0),
                    ],
                    ..Default::default()
                }),
            ),
            (
                "twig",
                Node::Axis(Axis {
                    plastochron_days: 12.0,
                    max_nodes: 8,
                    internode_m: (0.04, 0.06),
                    taper_length: 0.4,
                    internode_days: 16.0,
                    tip_radius_m: 0.0015,
                    thickening_m: 0.0007,
                    gravitropism: 0.25,
                    wander_deg: 12.0,
                    sag: 0.2,
                    color: [0.22, 0.27, 0.08],
                    wood_color: OAK_BARK,
                    lignify_days: 80.0,
                    life_days: 600.0,
                    children: vec![leaf_child(55.0)],
                    ..Default::default()
                }),
            ),
            (
                "leaf",
                Node::Leaf(Leaf {
                    kind: LeafKind::Simple,
                    petiole_m: (0.012, 0.02),
                    petiole_radius_m: 0.0009,
                    blade_m: (0.09, 0.13),
                    width: 0.55,
                    widest: 0.42,
                    teeth: 6,
                    tooth_depth: 0.14,
                    droop: 0.3,
                    grow_days: 16.0,
                    unfold: 0.3,
                    senesce_days: 190.0,
                    fade_days: 16.0,
                    color: [0.06, 0.17, 0.03],
                    old_color: [0.50, 0.32, 0.05],
                }),
            ),
        ]),
    }
}

/// The oak's bark color (linear; its bark texture is painted relative to it, so woody stems
/// of this color show the texture as painted).
const OAK_BARK: [f32; 3] = [0.144, 0.117, 0.091];

/// A pedunculate oak (Quercus robur) from an acorn, grown 25 years by the light it finds
/// ([`Tree`]): one spring flush a year of 30-50 cm on the leader (Colin et al. 2010), branches
/// from the buds near each year's tip, lower limbs shed as the crown shades them, and lobed
/// leaves on the new shoots from late April until they brown and fall in late October.
pub fn oak() -> PlantGraph {
    PlantGraph {
        name: "oak".into(),
        root: "tree".into(),
        life_days: 24.0 * 365.0 + 300.0,
        bark_species: Some("oak".into()),
        nodes: nodes(vec![
            (
                "tree",
                Node::Tree(Tree {
                    years: 25,
                    // An open-grown oak: limbs spread out low and wide, side light reaches
                    // the lower crown, and leafy spurs fill it (foliage-research.md, 10).
                    light_seeking: 0.12,
                    plagiotropism: 0.14,
                    shadow_spread: 0.5,
                    shed_light: 0.06,
                    leaves_per_node: 10,
                    leaf_years: 6,
                    spur_vigor: 0.2,
                    spur_light: 0.4,
                    bud_years: 6.0,
                    wander_deg: 12.0,
                    wood_color: OAK_BARK,
                    leaf: Some("leaf".into()),
                    ..Default::default()
                }),
            ),
            (
                "leaf",
                Node::Leaf(Leaf {
                    kind: LeafKind::Simple,
                    petiole_m: (0.004, 0.009),
                    petiole_radius_m: 0.0012,
                    blade_m: (0.11, 0.17),
                    width: 0.55,
                    widest: 0.6,
                    teeth: 5,
                    tooth_depth: 0.38,
                    droop: 0.25,
                    grow_days: 18.0,
                    unfold: 0.3,
                    senesce_days: 175.0,
                    fade_days: 20.0,
                    color: [0.05, 0.15, 0.03],
                    old_color: [0.42, 0.24, 0.06],
                }),
            ),
        ]),
    }
}

/// A plant growing from a graph, in the world. Its age moves on `days_per_second` days per
/// second of game time (0 holds it), and its meshes are rebuilt as it grows.
#[derive(Component, Debug, Clone, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct GrowingPlant {
    /// A built-in graph ([`presets`]) by name, used when `graph` is none.
    pub preset: String,
    /// The plant's own graph.
    pub graph: Option<PlantGraph>,
    pub seed: u64,
    pub age_days: f32,
    pub days_per_second: f32,
    /// Start again from a seed when it reaches the end of its life (a showcase loop).
    pub repeat: bool,
    /// Mesh detail: 1 full, lower for plants seen from further off.
    pub detail: f32,
}

impl Default for GrowingPlant {
    fn default() -> Self {
        Self {
            preset: "strawberry".into(),
            graph: None,
            seed: 1,
            age_days: 0.0,
            days_per_second: 0.0,
            repeat: false,
            detail: 1.0,
        }
    }
}

impl GrowingPlant {
    pub fn new(preset: &str, seed: u64) -> Self {
        Self {
            preset: preset.into(),
            seed,
            ..Default::default()
        }
    }

    pub fn aged(mut self, days: f32) -> Self {
        self.age_days = days;
        self
    }

    pub fn growing(mut self, days_per_second: f32) -> Self {
        self.days_per_second = days_per_second;
        self
    }

    /// The graph it grows from.
    pub fn resolve(&self) -> Result<PlantGraph, String> {
        let g = match &self.graph {
            Some(g) => g.clone(),
            None => preset(&self.preset).ok_or_else(|| {
                let names: Vec<String> = presets().into_iter().map(|g| g.name).collect();
                format!(
                    "Unknown growth preset \"{}\". Use one of: {}, or give a graph.",
                    self.preset,
                    names.join(", ")
                )
            })?,
        };
        g.validate()?;
        Ok(g)
    }
}

/// Engine meshes from what [`grow_meshes`] built (none for an empty part).
pub struct PlantParts {
    pub meshes: [Option<Mesh>; 4],
    pub triangles: usize,
    /// How long building them took, ms.
    pub ms: f32,
}

impl PlantParts {
    pub fn from_grown(g: GrownMeshes) -> Self {
        let triangles = g.triangles();
        let mesh = |b: crate::meshgen::MeshBuilder| (!b.is_empty()).then(|| b.finish());
        Self {
            meshes: [mesh(g.stems), mesh(g.leaves), mesh(g.petals), mesh(g.fruit)],
            triangles,
            ms: 0.0,
        }
    }
}

/// The plant's developed skeleton, its drawn parts and the rebuild in flight.
#[derive(Component)]
pub struct GrowthState {
    graph: Arc<PlantGraph>,
    skeleton: Arc<Skeleton>,
    /// Stems, leaves, petals, fruit (child entities).
    parts: [Entity; 4],
    meshes: [Handle<Mesh>; 4],
    /// Age the meshes show, and the age being built.
    shown_age: f32,
    building: Option<(f32, Arc<Mutex<Option<PlantParts>>>)>,
    /// How long the last build took, ms.
    pub last_build_ms: f32,
    /// Game time of the last rebuild, seconds.
    built_at: f64,
    source: (String, Option<PlantGraph>, u64),
}

/// The materials growing plants are drawn with (colors in the vertex colors).
#[derive(Resource, Clone)]
pub struct GrowthMaterials {
    pub stems: Handle<StandardMaterial>,
    pub leaves: Handle<StandardMaterial>,
    pub petals: Handle<StandardMaterial>,
    pub fruit: Handle<StandardMaterial>,
}

impl GrowthMaterials {
    fn new(materials: &mut Assets<StandardMaterial>, images: &mut Assets<Image>) -> Self {
        let leaf = crate::meshgen::small_textures::grown_leaf(512, 9);
        let white = |r: f32| {
            let mut m = StandardMaterial::color(1.0, 1.0, 1.0).with_roughness(r);
            m.double_sided = true;
            m
        };
        let mut stems = white(0.7).with_wind_sway(0.02).with_detail(0.3, 0.004);
        stems.double_sided = false;
        // Matte, and reflecting less than the default dielectric, as the trees' leaves: at 0.5
        // a low sun behind the bed turned every leaf facing it white.
        let mut leaves = white(0.8)
            .with_subsurface(0.5)
            .with_wind_sway(0.05)
            .with_base_color_texture(images.add(leaf.albedo))
            .with_normal_texture(images.add(leaf.normal));
        leaves.reflectance = 0.35;
        let petals = white(0.6).with_subsurface(0.6).with_wind_sway(0.04);
        let fruit = white(0.28).with_subsurface(0.3).with_wind_sway(0.02);
        Self {
            stems: materials.add(stems),
            leaves: materials.add(leaves),
            petals: materials.add(petals),
            fruit: materials.add(fruit),
        }
    }
}

/// Plants further than this from the camera keep the meshes they have (meters).
const HOLD_BEYOND_M: f64 = 80.0;
/// Plants nearer than this rebuild at most every `NEAR_REBUILD_S` seconds, further ones every
/// `FAR_REBUILD_S`: growth is slow next to a frame, and each rebuild uploads the meshes again.
const NEAR_M: f64 = 15.0;
const NEAR_REBUILD_S: f64 = 0.1;
const FAR_REBUILD_S: f64 = 0.5;
/// Rebuilds started in one frame at most, nearest first.
const BUILDS_PER_FRAME: usize = 2;

fn build(graph: &PlantGraph, sk: &Skeleton, age: f32, detail: f32) -> PlantParts {
    PlantParts::from_grown(grow_meshes(graph, sk, age, detail))
}

/// Develop new and changed growing plants, move their age on, and rebuild their meshes.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn grow_plants(
    mut commands: Commands,
    time: Res<Time>,
    library: Res<PlantLibrary>,
    visuals: Res<FoliageVisuals>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut mats: Local<Option<GrowthMaterials>>,
    loading: Res<crate::loading::FoliageLoading>,
    mut plants: Query<(
        Entity,
        &mut GrowingPlant,
        &GlobalTransform,
        Option<&mut GrowthState>,
    )>,
    cams: Query<(&engine_render::Camera, &GlobalTransform)>,
    mut renderers: Query<&mut MeshRenderer>,
) {
    if plants.is_empty() {
        return;
    }
    let mats = mats.get_or_insert_with(|| GrowthMaterials::new(&mut materials, &mut images));
    let dt = time.delta_secs();
    let now = time.elapsed_secs_f64();
    let camera = crate::visuals::camera_position(&cams);
    // Plants due a rebuild this frame: (distance, entity, age), the nearest started first.
    let mut due: Vec<(f64, Entity, f32)> = Vec::new();
    for (e, mut plant, at, state) in &mut plants {
        if plant.days_per_second != 0.0 && dt > 0.0 {
            let mut age = plant.age_days + plant.days_per_second * dt;
            if plant.repeat
                && let Some(s) = &state
                && age > s.skeleton.life_days
            {
                age = 0.0;
            }
            plant.bypass_change_detection().age_days = age;
        }
        let source = (plant.preset.clone(), plant.graph.clone(), plant.seed);
        let Some(mut state) = state.filter(|s| s.source == source) else {
            // New, or its graph or seed changed: develop it again.
            let graph = match plant.resolve() {
                Ok(g) => g,
                Err(err) => {
                    tracing::warn!("foliage: growing plant {e}: {err}");
                    commands.entity(e).remove::<GrowingPlant>();
                    continue;
                }
            };
            let skeleton = Skeleton::develop(&graph, plant.seed);
            let stems = graph
                .bark_species
                .as_deref()
                .and_then(|n| library.id(n))
                .and_then(|id| visuals.species.get(id.0 as usize)?.as_ref())
                .map_or(mats.stems, |v| v.bark);
            let materials = [stems, mats.leaves, mats.petals, mats.fruit];
            let handles: [Handle<Mesh>; 4] =
                std::array::from_fn(|_| meshes.add(Mesh::new(Vec::new(), Vec::new())));
            let parts: [Entity; 4] = std::array::from_fn(|i| {
                let mut r = MeshRenderer::new(handles[i], materials[i]);
                r.visible = false;
                commands
                    .spawn((Transform::IDENTITY, r, NoVirtualGeometry, ChildOf(e)))
                    .id()
            });
            if let Ok(mut old) = commands.get_entity(e) {
                old.insert(GrowthState {
                    graph: Arc::new(graph),
                    skeleton: Arc::new(skeleton),
                    parts,
                    meshes: handles,
                    shown_age: f32::NAN,
                    building: None,
                    last_build_ms: 0.0,
                    built_at: f64::NEG_INFINITY,
                    source,
                });
            }
            continue;
        };
        // A finished build goes in.
        if let Some((age, slot)) = &state.building
            && let Some(parts) = slot.lock().ok().and_then(|mut s| s.take())
        {
            let age = *age;
            state.last_build_ms = parts.ms;
            put(&mut state, parts, age, &mut meshes, &mut renderers);
            state.building = None;
        }
        let age = plant.age_days;
        if state.building.is_some() || state.shown_age == age {
            continue;
        }
        // Far plants grow on in their data; their meshes catch up when they come near.
        let distance = camera.map_or(0.0, |c| c.distance(at.translation()));
        let first = state.shown_age.is_nan();
        let wait = if distance > NEAR_M {
            FAR_REBUILD_S
        } else {
            NEAR_REBUILD_S
        };
        if !first && (distance > HOLD_BEYOND_M || now - state.built_at < wait) {
            continue;
        }
        // A plant never shown yet goes first.
        due.push((if first { -1.0 } else { distance }, e, age));
    }
    due.sort_by(|a, b| a.0.total_cmp(&b.0));
    for &(_, e, age) in due.iter().take(BUILDS_PER_FRAME) {
        let Ok((_, plant, _, Some(mut state))) = plants.get_mut(e) else {
            continue;
        };
        state.built_at = now;
        let (graph, skeleton, detail) = (state.graph.clone(), state.skeleton.clone(), plant.detail);
        let pool = AsyncComputeTaskPool::try_get();
        if !loading.background || pool.is_none() {
            let started = std::time::Instant::now();
            let parts = build(&graph, &skeleton, age, detail);
            state.last_build_ms = started.elapsed().as_secs_f32() * 1000.0;
            put(&mut state, parts, age, &mut meshes, &mut renderers);
        } else if let Some(pool) = pool {
            let slot = Arc::new(Mutex::new(None));
            let out = slot.clone();
            pool.spawn(async move {
                let started = std::time::Instant::now();
                let mut parts = build(&graph, &skeleton, age, detail);
                parts.ms = started.elapsed().as_secs_f32() * 1000.0;
                if let Ok(mut s) = out.lock() {
                    *s = Some(parts);
                }
            })
            .detach();
            state.building = Some((age, slot));
        }
    }
}

fn put(
    state: &mut GrowthState,
    parts: PlantParts,
    age: f32,
    meshes: &mut Assets<Mesh>,
    renderers: &mut Query<&mut MeshRenderer>,
) {
    for (i, mesh) in parts.meshes.into_iter().enumerate() {
        let visible = mesh.is_some();
        if let Some(mesh) = mesh {
            meshes.set(state.meshes[i], mesh);
        }
        if let Ok(mut r) = renderers.get_mut(state.parts[i])
            && r.visible != visible
        {
            r.visible = visible;
        }
    }
    state.shown_age = age;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_valid_and_round_trip_as_json() {
        for g in presets() {
            g.validate().unwrap();
            let json = serde_json::to_string(&g).unwrap();
            let back: PlantGraph = serde_json::from_str(&json).unwrap();
            assert_eq!(back, g);
        }
    }

    #[test]
    fn a_strawberry_leafs_flowers_fruits_and_sends_runners_in_order() {
        let g = strawberry();
        let sk = Skeleton::develop(&g, 7);
        let at = |t: f32| sk.census(t);
        // A seedling: a crown and its first leaves.
        let early = at(12.0);
        assert!(early.get("leaf").copied().unwrap_or(0) >= 1, "{early:?}");
        assert!(!early.contains_key("flower"), "{early:?}");
        // Flowers by early summer, berries after them, runners and daughters later.
        let first = |name: &str| {
            sk.organs
                .iter()
                .filter(|o| sk.names[o.node] == name)
                .map(|o| o.birth)
                .fold(f32::INFINITY, f32::min)
        };
        let (flower, berry, runner) = (first("flower"), first("berry"), first("runner"));
        assert!(
            flower < berry && berry < runner + 30.0,
            "{flower} {berry} {runner}"
        );
        assert!(
            sk.organs
                .iter()
                .filter(|o| sk.names[o.node] == "crown")
                .count()
                >= 2
        );
        // Old leaves drop: never more than ~10 alive at once on the mother crown.
        let leaves = at(140.0).get("leaf").copied().unwrap_or(0);
        assert!(leaves < 26, "{leaves} leaves alive");
    }

    #[test]
    fn growing_on_keeps_what_is_already_there() {
        // Developing for longer adds organs but never changes the ones already there.
        let mut short = strawberry();
        short.life_days = 60.0;
        let long = strawberry();
        let a = Skeleton::develop(&short, 3);
        let b = Skeleton::develop(&long, 3);
        for o in &a.organs {
            assert!(
                b.organs.iter().any(|p| p.seed == o.seed
                    && p.birth == o.birth
                    && p.internodes == o.internodes),
                "an organ of the shorter life changed"
            );
        }
    }

    /// Largest distance any vertex moves between two ages, of meshes with the same layout.
    fn moved(a: &GrownMeshes, b: &GrownMeshes) -> Option<f32> {
        let parts = |m: &GrownMeshes| {
            [&m.stems, &m.leaves, &m.petals, &m.fruit]
                .map(|b| b.vertices.iter().map(|v| v.position).collect::<Vec<_>>())
        };
        let (pa, pb) = (parts(a), parts(b));
        let mut worst = 0.0f32;
        for (x, y) in pa.iter().zip(&pb) {
            if x.len() != y.len() {
                return None;
            }
            for (p, q) in x.iter().zip(y) {
                worst = worst.max(Vec3::from(*p).distance(Vec3::from(*q)));
            }
        }
        Some(worst)
    }

    #[test]
    fn a_plant_grows_smoothly_from_day_to_day() {
        // Between two moments a few minutes apart nothing jumps: every vertex moves a little.
        let g = strawberry();
        let sk = Skeleton::develop(&g, 11);
        let mut compared = 0;
        let mut t = 1.0;
        while t < 140.0 {
            let a = grow_meshes(&g, &sk, t, 1.0);
            let b = grow_meshes(&g, &sk, t + 0.01, 1.0);
            if let Some(d) = moved(&a, &b) {
                assert!(
                    d < 0.004,
                    "day {t}: a vertex jumped {d} m in a quarter hour"
                );
                compared += 1;
            }
            t += 3.7;
        }
        assert!(compared > 20, "only {compared} comparable steps");
    }

    #[test]
    fn a_grown_strawberry_is_a_reasonable_size() {
        let g = strawberry();
        let sk = Skeleton::develop(&g, 5);
        let m = grow_meshes(&g, &sk, 110.0, 1.0);
        let tris = m.triangles();
        assert!((3_000..60_000).contains(&tris), "{tris} triangles");
        assert!(!m.fruit.is_empty(), "berries by day 110");
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for v in m.leaves.vertices.iter().chain(&m.stems.vertices) {
            lo = lo.min(Vec3::from(v.position));
            hi = hi.max(Vec3::from(v.position));
        }
        let size = hi - lo;
        // About a foot across (runners reach further) and a hand or two high.
        assert!(size.y > 0.08 && size.y < 0.4, "height {size}");
        assert!(size.x > 0.2 && size.x < 1.6, "width {size}");
    }

    #[test]
    fn a_sapling_climbs_and_thickens() {
        let g = sapling();
        let sk = Skeleton::develop(&g, 2);
        let height = |t: f32| {
            let m = grow_meshes(&g, &sk, t, 0.5);
            m.stems
                .vertices
                .iter()
                .map(|v| v.position[1])
                .fold(0.0, f32::max)
        };
        let (h1, h3) = (height(365.0), height(1095.0));
        assert!(
            h1 > 0.4 && h3 > h1 * 1.8,
            "{h1} m after a year, {h3} m after three"
        );
        assert!(h3 < 8.0, "{h3}");
        let m = grow_meshes(&g, &sk, 1095.0, 0.5);
        assert!(m.triangles() < 400_000, "{} triangles", m.triangles());
    }
}
