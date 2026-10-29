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
use crate::shadow_standin::CastsShadow;
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
    pub(crate) fn plant(&self) -> Plant {
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
    screen: Option<Res<engine_world::LoadingBudget>>,
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
        Has<crate::near_woods::NearWoodsTree>,
    )>,
    mut parts: Query<
        (
            &mut MeshRenderer,
            &mut Transform,
            &mut CastsShadow,
            crate::lod_fade::Busy,
        ),
        With<PlantPart>,
    >,
    mut fades: ResMut<crate::lod_fade::LodFades>,
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
    // Timed from the first new one: the pass over the old ones isn't loading work.
    let mut started: Option<std::time::Instant> = None;
    let fresh_budget = loading.fresh_budget(screen.as_deref());
    let (mut fresh_seen, mut out_of_time, mut skipped) = (0u32, false, false);
    // Near woods trees planted while the camera moves on (a new cell coming into range) fade
    // in rather than appear; after a cut they are part of the new view.
    let fade_in = fades.settled();
    for (entity, tree, global, visual, streamed) in &mut trees {
        let fresh = visual.is_none();
        if fresh {
            if out_of_time {
                // Only one left without its parts is still loading.
                skipped = true;
                continue;
            }
            let since = *started.get_or_insert_with(std::time::Instant::now);
            fresh_seen += 1;
            out_of_time = fresh_seen.is_multiple_of(16) && since.elapsed() > fresh_budget;
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
        let pick = |d: f32| {
            if d > settings.scenery_tree_draw_distance_m {
                [None, None, None]
            } else {
                desired_parts(&plant, sp, vis, d, &settings, season.day_of_year)
            }
        };
        let want = match &visual {
            // A level shown stays a little past its switch distance (no flicker on the edge).
            Some(v) if !tree.is_changed() => {
                crate::lod_fade::with_hysteresis(v.shown, distance, pick)
            }
            _ => pick(distance),
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
                    let Ok((mut r, mut t, mut casts, busy)) = parts.get_mut(part) else {
                        continue;
                    };
                    if visual.shown[i] != want[i] {
                        // A tree that changed (felled, grown) switches at once.
                        let busy = busy || tree.is_changed();
                        crate::lod_fade::swap(
                            &mut commands,
                            &mut fades,
                            entity,
                            part,
                            pose,
                            &mut r,
                            visual.shown[i],
                            want[i],
                            busy,
                        );
                    }
                    if casts.0 != shadows {
                        casts.0 = shadows;
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
                    let mut part =
                        commands.spawn((pose, ChildOf(entity), r, PlantPart, CastsShadow(shadows)));
                    if streamed && fade_in && want[i].is_some() {
                        part.insert((
                            engine_render::DitherFade(-1.0),
                            crate::lod_fade::PartFade::appearing(),
                        ));
                    }
                    part.id()
                });
                commands.entity(entity).insert(SceneryTreeVisual {
                    parts,
                    shown: want,
                    shadows,
                });
            }
        }
    }
    loading.bypass_change_detection().trees_waiting = skipped;
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
        let trees = [10.0, 60.0, 150.0, 900.0, 9000.0].map(|d| app.world.spawn((at(d), pine)).id());
        // Spawned with the camera: the first frame waits for their world transforms, rather
        // than drawing every tree in full detail at the origin.
        app.update();
        assert!(app.world.get::<SceneryTreeVisual>(trees[4]).is_none());
        app.update_n(SLICES as usize + 2);
        let vis = app
            .world
            .resource::<FoliageVisuals>()
            .get(pine.species)
            .unwrap();
        let v = vis.variants[pine.genes.variant as usize % crate::assets::VARIANTS].clone();
        let [near, mid, far, distant, gone] = trees.map(|t| shown(&app, t));
        assert_eq!(near, [v.base, v.wood, v.foliage]);
        assert_eq!(mid, [v.base, v.wood_mid, v.foliage_mid]);
        assert!(v.foliage_mid.is_some());
        assert_eq!(far, [v.base, v.wood_lod, v.foliage_lod]);
        assert_eq!(distant, [None, v.wood_distant, v.foliage_distant]);
        assert!(v.foliage_distant.is_some());
        assert_eq!(gone, [None; 3]);
        // Only trees within the shadow reach cast shadows: what each tree's wood casts with,
        // itself or its stand-in.
        let casts = |app: &App, t: Entity| {
            let p = app.world.get::<SceneryTreeVisual>(t).unwrap().parts[1];
            let r = app.world.get::<MeshRenderer>(p).unwrap();
            if r.cast_shadows {
                return Some(r.mesh.clone());
            }
            let s = app.world.get::<crate::shadow_standin::StandIn>(p)?.0;
            let s = app.world.get::<MeshRenderer>(s).unwrap();
            (s.visible && s.cast_shadows).then(|| s.mesh.clone())
        };
        let handle = |m: Option<Handle<Mesh>>| Some(engine_render::MeshRef::Handle(m.unwrap()));
        // The near tree casts its middle level's shadow, through a stand-in.
        assert_eq!(casts(&app, trees[0]), handle(v.wood_mid));
        let p = app.world.get::<SceneryTreeVisual>(trees[0]).unwrap().parts[1];
        let s = app
            .world
            .get::<crate::shadow_standin::StandIn>(p)
            .unwrap()
            .0;
        assert!(app.world.get::<engine_render::ShadowOnly>(s).is_some());
        assert_eq!(casts(&app, trees[1]), handle(v.wood_mid));
        assert_eq!(casts(&app, trees[2]), handle(v.wood_lod));
        assert_eq!(casts(&app, trees[3]), None);
        // The camera walks up to the far tree: it draws in full detail within a few frames.
        app.world.get_mut::<Transform>(cam).unwrap().translation = DVec3::new(140.0, 0.0, 0.0);
        app.update_n(SLICES as usize + 1);
        assert_eq!(shown(&app, trees[2]), [v.base, v.wood, v.foliage]);
        assert_eq!(casts(&app, trees[2]), handle(v.wood_mid));
        // The tree left behind casts its far level itself, its stand-in gone.
        assert_eq!(casts(&app, trees[0]), handle(v.wood_lod));
        // Not plants: nothing grows them, and the ecology never sees them.
        let mut plants = app.world.query::<&Plant>();
        assert_eq!(plants.iter(&app.world).count(), 0);
    }

    #[test]
    fn walking_up_to_a_tree_cross_fades_its_detail() {
        use crate::lod_fade::FadeGhost;
        use engine_render::DitherFade;
        let mut app = app();
        let library = app.world.resource::<PlantLibrary>().clone();
        let pine = SceneryTree::new(&library, "pine", 7).unwrap();
        let cam = app
            .world
            .spawn((Camera::default(), Transform::from_translation(DVec3::ZERO)))
            .id();
        let tree = app
            .world
            .spawn((
                Transform::from_translation(DVec3::new(150.0, 0.0, 0.0)),
                pine,
            ))
            .id();
        app.update_n(SLICES as usize + 2);
        let before = shown(&app, tree);
        // Walk up at 1.5 m a frame (90 m/s at 60 fps: brisk, but no cut).
        let (mut ghosts_seen, mut dithered_seen) = (0, 0);
        for k in 1..=80 {
            app.world.get_mut::<Transform>(cam).unwrap().translation =
                DVec3::new(1.5 * k as f64, 0.0, 0.0);
            app.update();
            let mut ghosts = app.world.query_filtered::<(), With<FadeGhost>>();
            ghosts_seen += ghosts.iter(&app.world).count();
            let parts = app.world.get::<SceneryTreeVisual>(tree).unwrap().parts;
            dithered_seen += parts
                .iter()
                .filter(|&&p| app.world.get::<DitherFade>(p).is_some())
                .count();
        }
        assert_ne!(
            shown(&app, tree),
            before,
            "the tree should be in full detail by now"
        );
        assert!(
            ghosts_seen > 0 && dithered_seen > 0,
            "it switched without a fade"
        );
        // A second after, every fade is over: no ghosts, no dither left on the parts.
        app.update_n(60);
        let mut ghosts = app.world.query_filtered::<(), With<FadeGhost>>();
        assert_eq!(ghosts.iter(&app.world).count(), 0);
        let parts = app.world.get::<SceneryTreeVisual>(tree).unwrap().parts;
        assert!(
            parts
                .iter()
                .all(|&p| app.world.get::<DitherFade>(p).is_none())
        );
    }
}
