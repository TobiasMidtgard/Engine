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
use std::sync::{Arc, Mutex};

use engine_core::prelude::*;
use engine_core::tasks::AsyncComputeTaskPool;
use engine_render::Camera;
use engine_world::{ForestCover, ForestQuery, Ground, WorldShift};

use crate::far_woods::{
    COVER_SETTLE_FRAMES, FarCard, FarWoods, FarWoodsState, KeepClear, Occupied, cell_cards,
};
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
    /// Most cells being laid out at once.
    pub cells_per_frame: u32,
    /// Lay cells out on the async task pool (planted a frame or more later) rather than in the
    /// frame.
    pub background: bool,
}

impl Default for NearWoods {
    fn default() -> Self {
        Self {
            enabled: true,
            cell_m: 64.0,
            spacing_m: 7.0,
            cells_per_frame: 8,
            background: true,
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
    /// Cells being laid out off the frame.
    pending: HashMap<(i64, i64), Pending>,
    /// What the last pass saw (camera cell, cover version, settled) and whether it left cells
    /// to plant: while none of it changes there is nothing to do.
    seen: Option<((i64, i64), u64, bool)>,
    backlog: bool,
}

/// A cell being laid out: the cover's version, where its corner is now (moved by world
/// shifts meanwhile) and the cards once they are ready.
struct Pending {
    epoch: u64,
    corner: DVec3,
    cards: Arc<Mutex<Option<Vec<FarCard>>>>,
}

/// Everything a cell's layout needs, owned, so it can run off the frame.
struct CellJob {
    corner: DVec3,
    cell_m: f32,
    spacing: f32,
    seed: u64,
    cam_y: f64,
    cover: Arc<dyn ForestQuery>,
    ground: Option<Ground>,
    occupied: Arc<Occupied>,
    occupied_origin: DVec3,
    keep_clear: Vec<KeepClear>,
}

impl CellJob {
    fn run(self) -> Vec<FarCard> {
        let ground_y = |p: DVec3| match &self.ground {
            Some(g) => g.height(DVec3::new(p.x, self.cam_y + 10_000.0, p.z)),
            None => Some(0.0),
        };
        let cover_at = |p: DVec3| self.cover.sample(p).map(|s| (s.density, s.conifer));
        let keep_out = |p: DVec3| {
            self.occupied.holds(self.occupied_origin, p)
                || self.keep_clear.iter().any(|k| {
                    DVec2::new(p.x - k.center.x, p.z - k.center.z).length() < k.radius as f64
                })
        };
        cell_cards(
            self.corner,
            self.cell_m,
            self.spacing,
            self.seed,
            &cover_at,
            &ground_y,
            &keep_out,
        )
    }
}

fn seed_of(key: (i64, i64)) -> u64 {
    (key.0 as u64).wrapping_mul(0x632b_e59b) ^ (key.1 as u64).wrapping_mul(0x8551_93a1)
}

/// Plant the cells within the far woods' start, nearest first, and drop those the camera has
/// left (past the start and one more cell). Cells planted from an older cover or placed-tree
/// ground replant once it has held still. A cell's trees are laid out off the frame and
/// planted when ready.
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
    chopped: Option<Res<crate::hand_felling::ChoppedSpots>>,
) {
    if !settings.enabled || settings.is_changed() {
        for (_, e) in std::mem::take(&mut state.cells) {
            commands.entity(e).despawn();
        }
        state.pending.clear();
        state.seen = None;
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
    let settled = far_state.quiet_frames >= COVER_SETTLE_FRAMES;
    let (ci, cj) = (
        ((cam.x - origin.x) / cell_m as f64).floor() as i64,
        ((cam.z - origin.z) / cell_m as f64).floor() as i64,
    );
    // The camera still in the same cell, the cover unchanged and everything planted: nothing
    // to do this frame.
    let seen = Some(((ci, cj), far_state.epoch, settled));
    if state.seen == seen && !state.backlog && state.pending.is_empty() && !far.is_changed() {
        return;
    }
    state.seen = seen;
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
    // Drop the cells left behind, and the layouts no longer wanted.
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
    state
        .pending
        .retain(|&k, _| flat_distance(k) <= reach + cell_m);
    let r = (reach / cell_m).ceil() as i64;
    let mut todo = Vec::new();
    for j in cj - r..=cj + r {
        for i in ci - r..=ci + r {
            let d = flat_distance((i, j));
            if d > reach {
                continue;
            }
            let stale = match state.pending.get(&(i, j)) {
                Some(p) => settled && p.epoch != far_state.epoch,
                None => match state.cells.get(&(i, j)).and_then(|&e| cells.get(e).ok()) {
                    None => !state.cells.contains_key(&(i, j)),
                    Some(c) => settled && c.epoch != far_state.epoch,
                },
            };
            if stale {
                todo.push((d, (i, j)));
            }
        }
    }
    todo.sort_by(|a, b| a.0.total_cmp(&b.0));
    let room = (settings.cells_per_frame as usize).saturating_sub(state.pending.len());
    let take = room.min(todo.len());
    state.backlog = todo.len() > take;
    let pool = AsyncComputeTaskPool::try_get().filter(|_| settings.background);
    for (_, key) in todo.into_iter().take(take) {
        let job = CellJob {
            corner: corner(key),
            cell_m,
            spacing: settings.spacing_m,
            seed: seed_of(key),
            cam_y: cam.y,
            cover: cover.0.clone(),
            ground: ground.as_deref().cloned(),
            occupied: far_state.occupied.clone(),
            occupied_origin: far_state.origin,
            keep_clear: far.keep_clear.clone(),
        };
        let cards = Arc::new(Mutex::new(None));
        let out = cards.clone();
        let run = move || {
            let planned = job.run();
            if let Ok(mut s) = out.lock() {
                *s = Some(planned);
            }
        };
        match pool {
            Some(pool) => pool.spawn(async move { run() }).detach(),
            None => run(),
        }
        state.pending.insert(
            key,
            Pending {
                epoch: far_state.epoch,
                corner: corner(key),
                cards,
            },
        );
    }
    // Plant the cells whose layouts are ready.
    let ready: Vec<_> = state
        .pending
        .iter()
        .filter(|(_, p)| p.cards.lock().is_ok_and(|c| c.is_some()))
        .map(|(&k, _)| k)
        .collect();
    for key in ready {
        let p = state.pending.remove(&key).expect("pending cell");
        let cards = p
            .cards
            .lock()
            .ok()
            .and_then(|mut c| c.take())
            .unwrap_or_default();
        if let Some(e) = state.cells.remove(&key) {
            commands.entity(e).despawn();
        }
        let seed = seed_of(key);
        let e = commands
            .spawn((
                Transform::from_translation(p.corner),
                NearWoodsCell { epoch: p.epoch },
            ))
            .id();
        for (n, card) in cards.iter().enumerate() {
            let Some(species) = library.id(card.tree.species()) else {
                continue;
            };
            // Chopped by the player (now a plant or a stump of its own).
            if chopped
                .as_ref()
                .is_some_and(|c| c.contains(p.corner + card.foot.as_dvec3()))
            {
                continue;
            }
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
            for p in state.pending.values_mut() {
                p.corner = s.point(p.corner);
            }
        } else {
            for (_, e) in std::mem::take(&mut state.cells) {
                commands.entity(e).despawn();
            }
            state.pending.clear();
            state.seen = None;
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
            background: false,
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
    fn revisits_replant_without_leaving_cells_behind() {
        for background in [false, true] {
            let (mut app, cam) = app();
            app.world.resource_mut::<NearWoods>().background = background;
            app.world
                .insert_resource(ForestCover(std::sync::Arc::new(Everywhere)));
            let mut counts = Vec::new();
            for visit in 0..4 {
                let x = if visit % 2 == 0 { 0.0 } else { 2000.0 };
                app.world.get_mut::<Transform>(cam).unwrap().translation = DVec3::new(x, 0.0, 0.0);
                // Long enough for the cover to settle, then until every cell in reach is laid
                // out and planted (background layouts take as long as the machine needs).
                let start = std::time::Instant::now();
                for frame in 0.. {
                    app.update();
                    let state = app.world.resource::<NearWoodsState>();
                    let done = state.pending.is_empty() && !state.backlog;
                    if (frame >= COVER_SETTLE_FRAMES as usize + 40 && done)
                        || start.elapsed().as_secs() > 120
                    {
                        break;
                    }
                    if background {
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    }
                }
                let cells = app
                    .world
                    .query_filtered::<Entity, With<NearWoodsCell>>()
                    .iter(&app.world)
                    .count();
                counts.push((cells, planted(&mut app).len()));
            }
            assert_eq!(counts[0], counts[2], "background {background}: {counts:?}");
            assert_eq!(counts[1], counts[3], "background {background}: {counts:?}");
        }
    }

    #[test]
    fn cells_laid_out_off_the_frame_are_planted_when_ready() {
        let (mut app, _) = app();
        app.world.resource_mut::<NearWoods>().background = true;
        app.world
            .insert_resource(ForestCover(std::sync::Arc::new(Everywhere)));
        let start = std::time::Instant::now();
        while planted(&mut app).len() < 300 && start.elapsed().as_secs() < 20 {
            app.update();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(planted(&mut app).len() >= 300);
        assert!(app.world.resource::<NearWoodsState>().pending.is_empty());
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
