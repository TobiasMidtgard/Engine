//! Scenery trees: static trees for painted landscapes (a valley's woods, the hills seen from a
//! tower), drawn with a species' looks but outside the simulation. They don't grow, seed,
//! compete, burn or fall, and get no collider (add your own). Each draws its near, far or
//! distant level of detail by its distance to the camera and follows the season (autumn
//! colors, leaf fall), and only a slice of them is looked at each frame, so tens of thousands
//! cost little.
//!
//! ```ignore
//! let tree = SceneryTree::new(&library, "pine", seed).unwrap();
//! commands.spawn((Transform::from_translation(foot).with_rotation(yaw), tree));
//! ```

use engine_core::prelude::*;
use engine_render::{Camera, MeshRenderer};

use crate::assets::FoliageVisuals;
use crate::plant::{EcologySettings, Genes, Plant};
use crate::season::FoliageSeason;
use crate::species::{PlantLibrary, SpeciesId};
use crate::visuals::{Part, PlantPart, apply, camera_position, desired_parts, plant_pose};

/// A static tree with a species' looks (see the module docs). Put it on an entity with a
/// `Transform` at the foot of the tree: the transform places, turns and scales the tree; its
/// lean, height and crown width come from `genes` on top. The level-of-detail distances count
/// `size` (see [`EcologySettings::tree_lod_distance_m`]) but not the transform's scale, so vary
/// tree sizes through `size` rather than the scale.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct SceneryTree {
    pub species: SpeciesId,
    /// How grown it is, 0..1 (below about a fifth it draws as a sapling).
    pub size: f32,
    pub genes: Genes,
}

impl Default for SceneryTree {
    fn default() -> Self {
        Self {
            species: SpeciesId::default(),
            size: 1.0,
            genes: Genes::default(),
        }
    }
}

impl SceneryTree {
    /// A grown tree of the named species with genes picked from `seed`, or `None` if the
    /// library has no such species.
    pub fn new(library: &PlantLibrary, species: &str, seed: u64) -> Option<Self> {
        Some(Self {
            species: library.id(species)?,
            size: 1.0,
            genes: Genes::random(&mut Rng::new(seed)),
        })
    }

    /// The plant it would be: grown to `size`, alive.
    fn plant(&self) -> Plant {
        Plant::new(self.species, self.size, self.genes)
    }
}

/// The drawn parts of a scenery tree (child entities: base, wood, foliage) and what they show.
#[derive(Component, Debug, Clone)]
pub struct SceneryTreeVisual {
    pub parts: [Entity; 3],
    shown: [Part; 3],
    /// Whether the parts cast sun shadows (only within the renderer's shadow reach).
    shadows: bool,
}

/// Scenery trees are looked at in this many slices, one slice a frame.
const SLICES: u32 = 8;

/// How far past the renderer's shadow distance a tree still casts its shadow, meters (its
/// crown and its shadow reach in from beyond it).
const SHADOW_MARGIN_M: f32 = 40.0;

/// Keep each scenery tree's parts showing the right level of detail for its distance and the
/// season. New and changed trees are handled at once, the rest a slice a frame (all of them
/// when the visuals, the settings or the season change). Nothing is picked without an active
/// camera, and a new tree waits one frame for its world transform, so trees spawned with the
/// camera don't all start in full detail at the origin. Only trees within the renderer's
/// shadow reach cast shadows.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn sync_scenery_trees(
    mut commands: Commands,
    mut frame: Local<u32>,
    mut loading: ResMut<crate::loading::FoliageLoading>,
    library: Res<PlantLibrary>,
    visuals: Res<FoliageVisuals>,
    settings: Res<EcologySettings>,
    season: Res<FoliageSeason>,
    render: Option<Res<engine_render::RenderSettings>>,
    cams: Query<(&Camera, &GlobalTransform)>,
    mut trees: Query<(
        Entity,
        Ref<SceneryTree>,
        Ref<GlobalTransform>,
        Option<&mut SceneryTreeVisual>,
    )>,
    mut parts: Query<(&mut MeshRenderer, &mut Transform), With<PlantPart>>,
) {
    *frame = frame.wrapping_add(1);
    let render_changed = render.as_ref().is_some_and(|r| r.is_changed());
    let all =
        visuals.is_changed() || settings.is_changed() || season.is_changed() || render_changed;
    let slice = *frame % SLICES;
    let Some(cam) = camera_position(&cams) else {
        return;
    };
    let shadow_reach = render.as_ref().map_or(350.0, |r| r.shadow_distance) + SHADOW_MARGIN_M;
    // While loading, new ones get their parts a few ms a frame (their spawns cost about as
    // much again when the commands apply).
    let started = std::time::Instant::now();
    let fresh_budget = loading.fresh_budget();
    let (mut fresh_seen, mut out_of_time) = (0u32, false);
    for (entity, tree, global, visual) in &mut trees {
        let fresh = visual.is_none();
        if fresh {
            if out_of_time {
                continue;
            }
            fresh_seen += 1;
            out_of_time = fresh_seen.is_multiple_of(16) && started.elapsed() > fresh_budget;
        }
        // Not propagated yet: its world transform still says the origin.
        if fresh && global.is_added() {
            continue;
        }
        if !all && !fresh && !tree.is_changed() && entity.index().index() % SLICES != slice {
            continue;
        }
        let Some(vis) = visuals.get(tree.species) else {
            continue;
        };
        let sp = library.get(tree.species);
        let plant = tree.plant();
        let distance = cam.distance(global.translation()) as f32;
        let shadows = distance <= shadow_reach;
        let want = if distance > settings.scenery_tree_draw_distance_m {
            [None; 3]
        } else {
            desired_parts(&plant, sp, vis, distance, &settings, season.day_of_year)
        };
        // The parts carry the tree's own lean and size under the entity's transform.
        let (rotation, scale) = plant_pose(&plant);
        let pose = Transform::from_rotation(rotation).with_scale(scale);
        match visual {
            Some(mut visual) => {
                if visual.shown == want && visual.shadows == shadows && !tree.is_changed() {
                    continue;
                }
                for (i, &part) in visual.parts.iter().enumerate() {
                    let Ok((mut r, mut t)) = parts.get_mut(part) else {
                        continue;
                    };
                    if visual.shown[i] != want[i] {
                        apply(&mut r, want[i]);
                    }
                    if r.cast_shadows != shadows {
                        r.cast_shadows = shadows;
                    }
                    if *t != pose {
                        *t = pose;
                    }
                }
                visual.shown = want;
                visual.shadows = shadows;
            }
            None => {
                let parts = std::array::from_fn(|i| {
                    let mut r = MeshRenderer::new(
                        engine_render::Shape::Cuboid { size: [0.01; 3] },
                        engine_render::StandardMaterial::default(),
                    );
                    apply(&mut r, want[i]);
                    r.cast_shadows = shadows;
                    commands.spawn((pose, ChildOf(entity), r, PlantPart)).id()
                });
                commands.entity(entity).insert(SceneryTreeVisual {
                    parts,
                    shown: want,
                    shadows,
                });
            }
        }
    }
    loading.bypass_change_detection().trees_waiting = out_of_time;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FoliagePlugin;
    use engine_assets::{Assets, Handle};
    use engine_render::Mesh;

    /// An app drawing pines only (building every species' visuals would take a while).
    fn app() -> App {
        let mut app = App::new();
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual {
                step_secs: 1.0 / 60.0,
            });
        app.add_plugins(engine_world::WorldPlugin);
        app.init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<engine_render::StandardMaterial>>()
            .init_resource::<Assets<engine_render::Image>>()
            .add_plugins(FoliagePlugin);
        app.world.insert_resource(crate::FoliageLoading::in_frame());
        let all = PlantLibrary::default();
        let pine = all.get(all.id("pine").unwrap()).clone();
        app.world.insert_resource(PlantLibrary {
            species: vec![pine],
        });
        app
    }

    /// The meshes a scenery tree's parts show.
    fn shown(app: &App, tree: Entity) -> [Option<Handle<Mesh>>; 3] {
        let parts = app.world.get::<SceneryTreeVisual>(tree).unwrap().parts;
        parts.map(|p| {
            let r = app.world.get::<MeshRenderer>(p).unwrap();
            match (r.visible, &r.mesh) {
                (true, engine_render::MeshRef::Handle(h)) => Some(*h),
                _ => None,
            }
        })
    }

    #[test]
    fn scenery_trees_switch_detail_with_distance_and_stay_out_of_the_simulation() {
        let mut app = app();
        let library = app.world.resource::<PlantLibrary>().clone();
        let pine = SceneryTree::new(&library, "pine", 7).unwrap();
        assert!(SceneryTree::new(&library, "baobab", 7).is_none());
        let cam = app
            .world
            .spawn((Camera::default(), Transform::from_translation(DVec3::ZERO)))
            .id();
        let at = |d: f64| Transform::from_translation(DVec3::new(d, 0.0, 0.0));
        let trees = [10.0, 150.0, 900.0, 9000.0].map(|d| app.world.spawn((at(d), pine)).id());
        // Spawned with the camera: the first frame waits for their world transforms, rather
        // than drawing every tree in full detail at the origin.
        app.update();
        assert!(app.world.get::<SceneryTreeVisual>(trees[3]).is_none());
        app.update_n(SLICES as usize + 2);
        let vis = app
            .world
            .resource::<FoliageVisuals>()
            .get(pine.species)
            .unwrap();
        let v = vis.variants[pine.genes.variant as usize % crate::assets::VARIANTS].clone();
        let [near, far, distant, gone] = trees.map(|t| shown(&app, t));
        assert_eq!(near, [v.base, v.wood, v.foliage]);
        assert_eq!(far, [v.base, v.wood_lod, v.foliage_lod]);
        assert_eq!(distant, [None, v.wood_distant, v.foliage_distant]);
        assert!(v.foliage_distant.is_some());
        assert_eq!(gone, [None; 3]);
        // Only trees within the shadow reach cast shadows.
        let casts = |t: Entity| {
            let p = app.world.get::<SceneryTreeVisual>(t).unwrap().parts[1];
            app.world.get::<MeshRenderer>(p).unwrap().cast_shadows
        };
        assert!(casts(trees[0]) && casts(trees[1]) && !casts(trees[2]));
        // The camera walks up to the far tree: it draws in full detail within a few frames.
        app.world.get_mut::<Transform>(cam).unwrap().translation = DVec3::new(140.0, 0.0, 0.0);
        app.update_n(SLICES as usize + 1);
        assert_eq!(shown(&app, trees[1]), [v.base, v.wood, v.foliage]);
        // Not plants: nothing grows them, and the ecology never sees them.
        let mut plants = app.world.query::<&Plant>();
        assert_eq!(plants.iter(&app.world).count(), 0);
    }
}
