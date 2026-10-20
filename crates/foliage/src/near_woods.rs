//! Near woods: real trees where the terrain paints woods ([`engine_world::ForestCover`]) inside
//! the far woods' start, so walking out of a hand-planted valley into the painted forest
//! beyond finds trees all round instead of bare ground under the far woods' ring.
//!
//! The land within [`FarWoods::start_m`] of the camera is cut into square cells; each cell
//! plants [`SceneryTree`]s (spruce, pine, oak and birch, by the cover's conifer share) at the
//! spacing of a closed wood, as children of one cell entity, and drops them when the camera has
//! moved away. They draw, light and follow the seasons like any scenery tree. Ground already
//! holding placed scenery trees is left to them, and these trees don't count as placed, so
//! the far woods don't rebuild while they stream.

use std::collections::HashMap;

use engine_core::prelude::*;
use engine_render::Camera;
use engine_world::{ForestCover, Ground, WorldShift};

use crate::far_woods::{COVER_SETTLE_FRAMES, FarWoods, FarWoodsState, cell_cards};
use crate::plant::Genes;
use crate::scenery_tree::SceneryTree;
use crate::species::PlantLibrary;
use crate::visuals::camera_position;

/// How the near woods are planted.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NearWoods {
    pub enabled: bool,
    /// Side of a streamed cell, meters.
    pub cell_m: f32,
    /// Mean distance between trees in a closed wood, meters.
    pub spacing_m: f32,
    /// Most cells planted per frame.
    pub cells_per_frame: u32,
}

impl Default for NearWoods {
    fn default() -> Self {
        Self {
            enabled: true,
            cell_m: 64.0,
            spacing_m: 7.0,
            cells_per_frame: 8,
        }
    }
}

/// Marks a tree the near woods planted (and will drop).
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct NearWoodsTree;

/// A planted cell: the cover's version it was planted from.
#[derive(Component, Debug, Clone, Copy)]
pub struct NearWoodsCell {
    epoch: u64,
}

/// The cells by key, keyed from `origin` (moved by world shifts).
#[derive(Resource, Default)]
pub struct NearWoodsState {
    origin: DVec3,
    cells: HashMap<(i64, i64), Entity>,
}

/// Plant the cells within the far woods' start, nearest first, and drop those the camera has
/// left (past the start and one more cell). Cells planted from an older cover or placed-tree
/// ground replant once it has held still.
#[allow(clippy::too_many_arguments)]
pub fn stream_near_woods(
    mut commands: Commands,
    settings: Res<NearWoods>,
    far: Res<FarWoods>,
    far_state: Res<FarWoodsState>,
    library: Res<PlantLibrary>,
    cover: Res<ForestCover>,
    ground: Option<Res<Ground>>,
    cams: Query<(&Camera, &GlobalTransform)>,
    mut state: ResMut<NearWoodsState>,
    cells: Query<&NearWoodsCell>,
) {
    if !settings.enabled || settings.is_changed() {
        for (_, e) in std::mem::take(&mut state.cells) {
            commands.entity(e).despawn();
        }
        if !settings.enabled {
            return;
        }
    }
    let Some(cam) = camera_position(&cams) else {
        return;
    };
    let cell_m = settings.cell_m.max(8.0);
    let reach = far.start_m;
    let origin = state.origin;
    let corner = |key: (i64, i64)| {
        origin
            + DVec3::new(
                key.0 as f64 * cell_m as f64,
                0.0,
                key.1 as f64 * cell_m as f64,
            )
    };
    let flat_distance = |key: (i64, i64)| {
        let c = corner(key) + DVec3::new(cell_m as f64 * 0.5, 0.0, cell_m as f64 * 0.5);
        DVec2::new(c.x - cam.x, c.z - cam.z).length() as f32
    };
    // Drop the cells left behind.
    let gone: Vec<_> = state
        .cells
        .keys()
        .copied()
        .filter(|&k| flat_distance(k) > reach + cell_m)
        .collect();
    for k in gone {
        if let Some(e) = state.cells.remove(&k) {
            commands.entity(e).despawn();
        }
    }
    let settled = far_state.quiet_frames >= COVER_SETTLE_FRAMES;
    let (ci, cj) = (
        ((cam.x - origin.x) / cell_m as f64).floor() as i64,
        ((cam.z - origin.z) / cell_m as f64).floor() as i64,
    );
    let r = (reach / cell_m).ceil() as i64;
    let mut todo = Vec::new();
    for j in cj - r..=cj + r {
        for i in ci - r..=ci + r {
            let d = flat_distance((i, j));
            if d > reach {
                continue;
            }
            let stale = match state.cells.get(&(i, j)).and_then(|&e| cells.get(e).ok()) {
                None => !state.cells.contains_key(&(i, j)),
                Some(c) => settled && c.epoch != far_state.epoch,
            };
            if stale {
                todo.push((d, (i, j)));
            }
        }
    }
    todo.sort_by(|a, b| a.0.total_cmp(&b.0));
    let ground_y = |p: DVec3| match ground.as_deref() {
        Some(g) => g.height(DVec3::new(p.x, cam.y + 10_000.0, p.z)),
        None => Some(0.0),
    };
    let cover_at = |p: DVec3| cover.0.sample(p).map(|s| (s.density, s.conifer));
    let keep_out = |p: DVec3| {
        far_state.occupied.holds(far_state.origin, p)
            || far
                .keep_clear
                .iter()
                .any(|k| DVec2::new(p.x - k.center.x, p.z - k.center.z).length() < k.radius as f64)
    };
    for (_, key) in todo.into_iter().take(settings.cells_per_frame as usize) {
        if let Some(e) = state.cells.remove(&key) {
            commands.entity(e).despawn();
        }
        let at = corner(key);
        let seed =
            (key.0 as u64).wrapping_mul(0x632b_e59b) ^ (key.1 as u64).wrapping_mul(0x8551_93a1);
        let cards = cell_cards(
            at,
            cell_m,
            settings.spacing_m,
            seed,
            &cover_at,
            &ground_y,
            &keep_out,
        );
        let e = commands
            .spawn((
                Transform::from_translation(at),
                NearWoodsCell {
                    epoch: far_state.epoch,
                },
            ))
            .id();
        for (n, card) in cards.iter().enumerate() {
            let Some(species) = library.id(card.tree.species()) else {
                continue;
            };
            // Genes from where the tree stands, so a replanted cell grows the same wood.
            let mut rng = Rng::new(seed ^ (n as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15));
            let genes = Genes::random(&mut rng);
            let size = rng.range(0.7, 1.0);
            commands.spawn((
                Transform::from_translation(card.foot.as_dvec3())
                    .with_rotation(Quat::from_rotation_y(card.yaw * 2.0)),
                SceneryTree {
                    species,
                    size,
                    genes,
                },
                NearWoodsTree,
                ChildOf(e),
            ));
        }
        state.cells.insert(key, e);
    }
}

/// Follow a world shift: a translation moves the grid with the cells; a turn replants them.
pub fn follow_world_shift(
    mut commands: Commands,
    mut shifts: MessageReader<WorldShift>,
    mut state: ResMut<NearWoodsState>,
) {
    for s in shifts.read() {
        if s.is_translation_only() {
            state.origin = s.point(state.origin);
        } else {
            for (_, e) in std::mem::take(&mut state.cells) {
                commands.entity(e).despawn();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_assets::Assets;
    use engine_render::{Mesh, StandardMaterial};

    struct Everywhere;
    impl engine_world::ForestQuery for Everywhere {
        fn sample(&self, _p: DVec3) -> Option<engine_world::ForestSample> {
            Some(engine_world::ForestSample {
                density: 1.0,
                conifer: 1.0,
            })
        }
    }

    /// Conifer woods everywhere, near woods within 100 m of a camera at the origin.
    fn app() -> (App, Entity) {
        let mut app = App::new();
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual {
                step_secs: 1.0 / 60.0,
            });
        app.add_plugins(engine_world::WorldPlugin);
        app.init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Assets<engine_render::Image>>()
            .add_plugins(crate::FoliagePlugin);
        let all = PlantLibrary::default();
        let keep = ["pine", "spruce"].map(|n| all.get(all.id(n).unwrap()).clone());
        app.world.insert_resource(PlantLibrary {
            species: keep.to_vec(),
        });
        app.world.insert_resource(FarWoods {
            start_m: 100.0,
            end_m: 300.0,
            background_builds: 0,
            cells_per_frame: u32::MAX,
            ..Default::default()
        });
        app.world.insert_resource(NearWoods {
            cells_per_frame: u32::MAX,
            ..Default::default()
        });
        let cam = app
            .world
            .spawn((Camera::default(), Transform::from_translation(DVec3::ZERO)))
            .id();
        (app, cam)
    }

    fn planted(app: &mut App) -> Vec<DVec3> {
        let mut q = app
            .world
            .query_filtered::<&GlobalTransform, With<NearWoodsTree>>();
        q.iter(&app.world).map(|t| t.translation()).collect()
    }

    #[test]
    fn painted_woods_near_the_camera_get_real_trees() {
        let (mut app, cam) = app();
        app.update_n(3);
        assert!(planted(&mut app).is_empty(), "no woods painted");
        app.world
            .insert_resource(ForestCover(std::sync::Arc::new(Everywhere)));
        // The open cells replant once the new cover has held still.
        app.update_n(COVER_SETTLE_FRAMES as usize + 2);
        let trees = planted(&mut app);
        // About a closed wood's worth: π·100² m² at one tree per 7×7 m.
        assert!((300..1200).contains(&trees.len()), "{}", trees.len());
        for p in &trees {
            assert!(DVec2::new(p.x, p.z).length() < 100.0 + 50.0, "{p}");
        }
        // The camera walks 2 km on: the trees behind go, new ones stand round it.
        app.world.get_mut::<Transform>(cam).unwrap().translation = DVec3::new(2000.0, 0.0, 0.0);
        app.update_n(3);
        let trees = planted(&mut app);
        assert!(trees.len() >= 300);
        assert!(trees.iter().all(|p| p.x > 2000.0 - 150.0));
    }

    #[test]
    fn placed_trees_keep_their_ground() {
        let (mut app, _) = app();
        app.world
            .insert_resource(ForestCover(std::sync::Arc::new(Everywhere)));
        app.update_n(3);
        let open = planted(&mut app).len();
        // A placed grove 30–60 m out: the near woods replant round it.
        let library = app.world.resource::<PlantLibrary>().clone();
        let mut grove = Vec::new();
        for j in 0..5 {
            for i in 0..5 {
                let p = DVec3::new(30.0 + i as f64 * 7.0, 0.0, -15.0 + j as f64 * 7.0);
                grove.push(p);
                let tree = SceneryTree::new(&library, "pine", (i * 5 + j) as u64).unwrap();
                app.world.spawn((Transform::from_translation(p), tree));
            }
        }
        app.update_n(COVER_SETTLE_FRAMES as usize + 4);
        let trees = planted(&mut app);
        assert!(trees.len() < open, "{} vs {open}", trees.len());
        for p in &trees {
            let near = grove
                .iter()
                .map(|g| DVec2::new(p.x - g.x, p.z - g.z).length())
                .fold(f64::MAX, f64::min);
            assert!(
                near > 5.0,
                "a near woods tree {near:.1} m from a placed one"
            );
        }
    }
}
