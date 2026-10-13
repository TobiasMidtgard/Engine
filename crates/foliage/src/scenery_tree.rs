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
/// lean, height and crown width come from `genes` on top.
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
}

/// Scenery trees are looked at in this many slices, one slice a frame.
const SLICES: u32 = 8;

/// Keep each scenery tree's parts showing the right level of detail for its distance and the
/// season. New and changed trees are handled at once, the rest a slice a frame (all of them
/// when the visuals, the settings or the season change).
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn sync_scenery_trees(
    mut commands: Commands,
    mut frame: Local<u32>,
    library: Res<PlantLibrary>,
    visuals: Res<FoliageVisuals>,
    settings: Res<EcologySettings>,
    season: Res<FoliageSeason>,
    cams: Query<(&Camera, &GlobalTransform)>,
    mut trees: Query<(
        Entity,
        Ref<SceneryTree>,
        &GlobalTransform,
        Option<&mut SceneryTreeVisual>,
    )>,
    mut parts: Query<(&mut MeshRenderer, &mut Transform), With<PlantPart>>,
) {
    *frame = frame.wrapping_add(1);
    let all = visuals.is_changed() || settings.is_changed() || season.is_changed();
    let slice = *frame % SLICES;
    let cam = camera_position(&cams);
    for (entity, tree, global, visual) in &mut trees {
        let fresh = visual.is_none();
        if !all && !fresh && !tree.is_changed() && entity.index().index() % SLICES != slice {
            continue;
        }
        let Some(vis) = visuals.get(tree.species) else {
            continue;
        };
        let sp = library.get(tree.species);
        let plant = tree.plant();
        let distance = cam.map_or(0.0, |c| c.distance(global.translation()) as f32);
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
                if visual.shown == want && !tree.is_changed() {
                    continue;
                }
                for (i, &part) in visual.parts.iter().enumerate() {
                    let Ok((mut r, mut t)) = parts.get_mut(part) else {
                        continue;
                    };
                    if visual.shown[i] != want[i] {
                        apply(&mut r, want[i]);
                    }
                    if *t != pose {
                        *t = pose;
                    }
                }
                visual.shown = want;
            }
            None => {
                let parts = std::array::from_fn(|i| {
                    let mut r = MeshRenderer::new(
                        engine_render::Shape::Cuboid { size: [0.01; 3] },
                        engine_render::StandardMaterial::default(),
                    );
                    apply(&mut r, want[i]);
                    commands.spawn((pose, ChildOf(entity), r, PlantPart)).id()
                });
                commands
                    .entity(entity)
                    .insert(SceneryTreeVisual { parts, shown: want });
            }
        }
    }
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
        // The camera walks up to the far tree: it draws in full detail within a few frames.
        app.world.get_mut::<Transform>(cam).unwrap().translation = DVec3::new(140.0, 0.0, 0.0);
        app.update_n(SLICES as usize + 1);
        assert_eq!(shown(&app, trees[1]), [v.base, v.wood, v.foliage]);
        // Not plants: nothing grows them, and the ecology never sees them.
        let mut plants = app.world.query::<&Plant>();
        assert_eq!(plants.iter(&app.world).count(), 0);
    }
}
