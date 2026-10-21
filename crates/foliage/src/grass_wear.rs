//! Grass trodden into paths: every `engine_render::GrassField` (the regions' own and any other,
//! such as a game's valley meadow) gets a `GrassWear` map of how worn its ground is, from
//! `engine_world::GroundWear`, so its blades lie flat, dry and thin out along footpaths and are
//! gone where the ground is worn bare.
//!
//! The wear map is finer than a field's own maps (half a meter, as the terrain's wear cells)
//! but covers only the rectangle of the field where the ground is worn. Finding that rectangle
//! means asking `GroundWear` all over the field, so the mapping runs on a worker thread, again
//! whenever the wear changes (at most every [`RESCAN_SECS`]) or a field is added or moved.

use std::collections::HashMap;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use engine_core::prelude::*;
use engine_render::{GrassField, GrassWear};
use engine_world::{GroundWear, WearQuery};

/// Meters between the wear map's points (the terrain keeps wear in half-meter cells).
pub const WEAR_CELL_M: f32 = 0.5;
/// Most wear-map points along one side; a wider worn area gets coarser points.
const MAX_POINTS: u32 = 1024;
/// Meters between the points searched for worn ground (a footpath is about a meter wide and
/// long, so a coarser search still finds it along its length).
const SEARCH_M: f32 = 1.0;
/// Below this the ground counts as untouched.
const WORN: f32 = 0.01;
/// Meters of margin kept around the worn points found.
const MARGIN_M: f32 = 2.0;
/// Shortest time between two mappings while paths change, seconds.
pub const RESCAN_SECS: f32 = 1.0;

/// Where a field lies, for mapping its wear away from the world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FieldPlace {
    /// World position of the field's -x -z corner.
    pub corner: DVec3,
    pub rotation: Quat,
    /// Size along its local x and z, meters.
    pub size: [f32; 2],
}

impl FieldPlace {
    pub fn of(field: &GrassField, at: &GlobalTransform) -> Self {
        let rotation = at.rotation();
        let half = DVec3::new(field.size[0] as f64 * 0.5, 0.0, field.size[1] as f64 * 0.5);
        Self {
            corner: at.translation() - rotation.as_dquat() * half,
            rotation,
            size: field.size,
        }
    }

    fn world(&self, x: f32, z: f32) -> DVec3 {
        self.corner + self.rotation.as_dquat() * DVec3::new(x as f64, 0.0, z as f64)
    }
}

/// The wear map of a field at `place`, or `None` where none of its ground is worn.
pub fn map_wear(wear: &dyn WearQuery, place: &FieldPlace) -> Option<GrassWear> {
    let [sx, sz] = place.size.map(|s| s.max(0.0));
    if sx <= 0.0 || sz <= 0.0 {
        return None;
    }
    // Search: the bounds of the worn points on a coarse grid, a band of rows per core.
    let [nx, nz] = [sx, sz].map(|s| (s / SEARCH_M).ceil() as u32 + 1);
    let step = [sx / (nx - 1).max(1) as f32, sz / (nz - 1).max(1) as f32];
    let threads = std::thread::available_parallelism().map_or(4, |c| c.get()) as u32;
    let rows_per = nz.div_ceil(threads.max(1));
    let bands: Vec<Option<[f32; 4]>> = std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..nz)
            .step_by(rows_per.max(1) as usize)
            .map(|first| {
                scope.spawn(move || {
                    let mut b: Option<[f32; 4]> = None;
                    for j in first..(first + rows_per).min(nz) {
                        let z = j as f32 * step[1];
                        for i in 0..nx {
                            let x = i as f32 * step[0];
                            if wear.wear(place.world(x, z)) > WORN {
                                let r = b.get_or_insert([x, z, x, z]);
                                *r = [r[0].min(x), r[1].min(z), r[2].max(x), r[3].max(z)];
                            }
                        }
                    }
                    b
                })
            })
            .collect();
        jobs.into_iter().map(|j| j.join().unwrap_or(None)).collect()
    });
    let found = bands.into_iter().flatten().reduce(|a, b| {
        [
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[2].max(b[2]),
            a[3].max(b[3]),
        ]
    })?;
    let lo = [
        (found[0] - MARGIN_M).max(0.0),
        (found[1] - MARGIN_M).max(0.0),
    ];
    let hi = [(found[2] + MARGIN_M).min(sx), (found[3] + MARGIN_M).min(sz)];
    let extent = (hi[0] - lo[0]).max(hi[1] - lo[1]);
    let cell = WEAR_CELL_M.max(extent / (MAX_POINTS - 1) as f32);
    let resolution = [0, 1].map(|a| ((hi[a] - lo[a]) / cell).ceil() as u32 + 1);
    let [rx, rz] = resolution;
    // The map itself, again a band of rows per core.
    let rows_per = rz.div_ceil(threads.max(1)).max(1) as usize;
    let mut values = vec![0.0f32; (rx * rz) as usize];
    std::thread::scope(|scope| {
        for (band, chunk) in values.chunks_mut(rows_per * rx as usize).enumerate() {
            scope.spawn(move || {
                for (k, out) in chunk.iter_mut().enumerate() {
                    let i = (k % rx as usize) as f32;
                    let j = (band * rows_per + k / rx as usize) as f32;
                    *out = wear.wear(place.world(lo[0] + i * cell, lo[1] + j * cell));
                }
            });
        }
    });
    Some(GrassWear {
        origin: lo,
        cell,
        resolution,
        wear: values,
    })
}

/// Each field, where it was mapped and its map.
type Mapped = Vec<(Entity, FieldPlace, Option<GrassWear>)>;

/// The mapping in flight and what was last mapped.
#[derive(Default)]
pub struct WearMapping {
    job: Option<JoinHandle<Mapped>>,
    started: Option<Instant>,
    /// Wear changed since the last mapping began.
    stale: bool,
    /// Where each field was when last mapped.
    mapped: HashMap<Entity, FieldPlace>,
}

/// Keep every grass field's `GrassWear` in step with `GroundWear`.
pub fn wear_grass(
    mut commands: Commands,
    wear: Option<Res<GroundWear>>,
    mut state: Local<WearMapping>,
    fields: Query<(Entity, &GrassField, &GlobalTransform, Option<&GrassWear>)>,
) {
    let Some(wear) = wear else {
        return;
    };
    if state.job.as_ref().is_some_and(|j| j.is_finished())
        && let Some(job) = state.job.take()
    {
        for (e, place, mapped) in job.join().unwrap_or_default() {
            // A field that moved meanwhile is mapped again where it is now.
            let Ok((_, field, at, had)) = fields.get(e) else {
                continue;
            };
            if FieldPlace::of(field, at) != place {
                continue;
            }
            match mapped {
                Some(w) if had != Some(&w) => {
                    commands.entity(e).try_insert(w);
                }
                None if had.is_some() => {
                    commands.entity(e).try_remove::<GrassWear>();
                }
                _ => {}
            }
        }
    }
    if wear.is_changed() {
        state.stale = true;
    }
    state.mapped.retain(|e, _| fields.contains(*e));
    let moved = fields
        .iter()
        .any(|(e, f, at, _)| state.mapped.get(&e) != Some(&FieldPlace::of(f, at)));
    let due = state
        .started
        .is_none_or(|t| t.elapsed() >= Duration::from_secs_f32(RESCAN_SECS));
    if state.job.is_some() || !(state.stale || moved) || !due {
        return;
    }
    state.stale = false;
    state.started = Some(Instant::now());
    let places: Vec<(Entity, FieldPlace)> = fields
        .iter()
        .map(|(e, f, at, _)| (e, FieldPlace::of(f, at)))
        .collect();
    state.mapped = places.iter().copied().collect();
    let query: Arc<dyn WearQuery> = wear.0.clone();
    state.job = Some(std::thread::spawn(move || {
        places
            .into_iter()
            .map(|(e, place)| (e, place, map_wear(&*query, &place)))
            .collect()
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A footpath along x = 10, a meter wide.
    struct Path;
    impl WearQuery for Path {
        fn wear(&self, p: DVec3) -> f32 {
            (1.0 - (p.x - 10.0).abs() as f32 * 2.0).max(0.0)
        }
    }

    #[test]
    fn a_footpath_is_mapped_finely_where_it_runs() {
        let place = FieldPlace {
            corner: DVec3::new(-50.0, 3.0, -50.0),
            rotation: Quat::IDENTITY,
            size: [100.0, 100.0],
        };
        let w = map_wear(&Path, &place).expect("the path is found");
        assert!(w.cell <= WEAR_CELL_M + 1e-6, "cell {}", w.cell);
        // The path is at field-local x = 60, and the map hugs it.
        assert!(w.origin[0] > 55.0 && w.origin[0] < 60.0, "{:?}", w.origin);
        let width = (w.resolution[0] - 1) as f32 * w.cell;
        assert!(width < 8.0, "map {width} m wide");
        let at = |x: f32, z: f32| {
            let i = ((x - w.origin[0]) / w.cell).round() as u32;
            let j = ((z - w.origin[1]) / w.cell).round() as u32;
            w.wear[(j * w.resolution[0] + i) as usize]
        };
        assert!(at(60.0, 50.0) > 0.9);
        assert!(at(60.0 + 1.5, 50.0) < 0.01);
    }

    #[test]
    fn untouched_ground_has_no_map() {
        struct NoPath;
        impl WearQuery for NoPath {
            fn wear(&self, _: DVec3) -> f32 {
                0.0
            }
        }
        let place = FieldPlace {
            corner: DVec3::ZERO,
            rotation: Quat::IDENTITY,
            size: [40.0, 40.0],
        };
        assert!(map_wear(&NoPath, &place).is_none());
    }

    #[test]
    fn fields_on_worn_ground_get_a_wear_map() {
        let mut app = App::new();
        app.add_plugins(engine_world::WorldPlugin);
        app.add_systems(Update, wear_grass);
        app.world.insert_resource(GroundWear(Arc::new(Path)));
        let on = app
            .world
            .spawn((
                Transform::from_xyz(0.0, 0.0, 0.0),
                GrassField::flat(60.0, 60.0),
            ))
            .id();
        let off = app
            .world
            .spawn((
                Transform::from_xyz(200.0, 0.0, 0.0),
                GrassField::flat(60.0, 60.0),
            ))
            .id();
        let start = Instant::now();
        while app.world.get::<GrassWear>(on).is_none() && start.elapsed().as_secs() < 20 {
            app.update();
            std::thread::sleep(Duration::from_millis(5));
        }
        let w = app
            .world
            .get::<GrassWear>(on)
            .expect("the path's field is mapped");
        assert!(w.wear.iter().any(|&v| v > 0.9));
        assert!(app.world.get::<GrassWear>(off).is_none());
        // Healed: the map goes again.
        app.world.insert_resource(GroundWear::default());
        let start = Instant::now();
        while app.world.get::<GrassWear>(on).is_some() && start.elapsed().as_secs() < 20 {
            app.update();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.world.get::<GrassWear>(on).is_none());
    }
}
