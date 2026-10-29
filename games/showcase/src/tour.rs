//! `--tour`: fly to every place, wait for exposure to settle, save
//! `screenshots/tour/NN_<key>.png`, then quit. Used by CI-style visual checks and by AI agents
//! that want to see the whole map in one go.

use crate::camera;
use crate::places::{Place, places};
use engine::prelude::*;
use engine::render::{RenderPaused, with_renderer};

/// Frames to wait at each place before capturing (metered auto-exposure reads back a frame or
/// two late, then adapts).
const SETTLE_FRAMES: u32 = 8;
/// Simulated seconds per tour frame.
const STEP_SECS: f32 = 0.05;
/// Before settling, a stop waits for meshes still being clustered in the background (the
/// displaced relief of stones, rocks and bark), while they keep finishing...
const BUILD_STALL_SECS: f32 = 90.0;
/// ...or, in the wait before a stop's arrival calls, while any finish this often: a timed
/// stop should do its long waiting there, where the clock hasn't started (a software renderer
/// can take minutes over one stone).
const ARRIVAL_STALL_SECS: f32 = 600.0;
/// ...but never longer than this, wall-clock seconds.
const BUILD_WAIT_SECS: f32 = 900.0;

#[derive(Resource, Debug)]
pub struct Tour {
    pub places: Vec<Place>,
    pub index: usize,
    pub frames: u32,
    pub out_dir: String,
    /// While a stop waits for background builds: when it started, the meshes ready and terrain
    /// patches pending, and when those last changed.
    waiting: Option<(std::time::Instant, (u64, u64), std::time::Instant)>,
}

impl Tour {
    pub fn new(only: Option<&str>) -> Self {
        let mut list = places();
        if let Some(only) = only {
            let keys: Vec<&str> = only.split(',').map(str::trim).collect();
            list.retain(|p| keys.contains(&p.key));
            list.extend(keys.iter().copied().filter_map(at));
        }
        Self {
            places: list,
            index: 0,
            frames: 0,
            out_dir: "screenshots/tour".into(),
            waiting: None,
        }
    }
}

/// An ad-hoc stop, `at:x:y:z:tx:ty:tz:hour` (eye, target and hour; the eye stays above the
/// ground), for looking at any spot without adding a place. These come after the named stops
/// and are saved as `NN_at.png`. Pair them with `--zones none` (or the zones they show): they
/// belong to no zone. Films take them too (`--film at:…,at:…` glides between them).
pub(crate) fn at(key: &str) -> Option<Place> {
    let nums: Vec<f64> = key
        .strip_prefix("at:")?
        .split(':')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    let [x, y, z, tx, ty, tz, hour] = nums[..] else {
        return None;
    };
    let ground = crate::landscape::shared().height(x, z);
    Some(Place {
        key: "at",
        name: "A spot given on the command line",
        position: DVec3::new(x, y.max(ground + 0.5), z),
        target: DVec3::new(tx, ty, tz),
        hour: hour as f32,
        warmup_secs: 0.0,
        quick_warmup: false,
        on_arrive: Vec::new(),
        on_leave: Vec::new(),
    })
}

/// Install the tour: deterministic time steps, fast-adapting exposure and far woods built at once.
pub fn install(app: &mut App, only: Option<&str>) {
    app.world
        .resource_mut::<Time>()
        .set_source(engine::core::time::TimeSource::Manual {
            step_secs: STEP_SECS as f64,
        });
    // The far woods build, and the near woods plant, every cell in view on the first frame
    // instead of a few a frame, which a tour's few frames per stop would never finish.
    if let Some(mut far) = app.world.get_resource_mut::<engine_foliage::FarWoods>() {
        far.background_builds = 0;
        far.cells_per_frame = u32::MAX;
    }
    if let Some(mut near) = app.world.get_resource_mut::<engine_foliage::NearWoods>() {
        near.cells_per_frame = u32::MAX;
    }
    app.insert_resource(Tour::new(only));
    app.add_systems(Update, run.after(camera::fly));
}

fn run(world: &mut World) {
    let (place, frames, index, total, out_dir) = {
        let tour = world.resource::<Tour>();
        let Some(place) = tour.places.get(tour.index).cloned() else {
            // Done: give the last screenshot a frame to be written, then exit.
            unpark(world);
            crate::places::leave(world);
            world.write_message(AppExit::Success);
            return;
        };
        (
            place,
            tour.frames,
            tour.index,
            tour.places.len(),
            tour.out_dir.clone(),
        )
    };

    // The place's calls run a few frames after the builds in view finish; exposure settles at
    // the end of the warm-up that follows them.
    let capture_at = crate::places::ARRIVE_DELAY
        + 2
        + SETTLE_FRAMES
        + (place.warmup_secs / STEP_SECS).ceil() as u32;
    if frames == 0 {
        camera::go_to(world, &place);
        world.resource_mut::<engine_weather::Sky>().hour = place.hour;
        // Adapt exposure within a couple of frames instead of seconds.
        let mut cams = world.query::<&mut Camera>();
        for mut cam in cams.iter_mut(world) {
            if let Exposure::Auto { speed, .. } = &mut cam.exposure {
                *speed = 40.0;
            }
        }
        engine::core::log::info!("tour {}/{}: {}", index + 1, total, place.name);
    }

    // The place's calls wait for the meshes in view to finish building, so the frames run
    // meanwhile don't eat into the warm-up: a timed moment (a vault, a leap onto a moving car)
    // is caught when the warm-up says, however long the builds take.
    if frames == 1 {
        if wait_for_builds(world, ARRIVAL_STALL_SECS) {
            return;
        }
        crate::places::visit_soon(world, &place);
    }

    // A quick warm-up runs without drawing, from after the arrival calls until the frames
    // that settle the picture.
    let settle_from = capture_at - SETTLE_FRAMES;
    if place.quick_warmup && frames == crate::places::ARRIVE_DELAY + 3 && frames < settle_from {
        park(world);
    }
    // Anything the calls brought into view gets the same wait before the picture.
    if frames == settle_from {
        unpark(world);
        if wait_for_builds(world, BUILD_STALL_SECS) {
            return;
        }
    }

    if frames == capture_at {
        let d = world.resource::<Diagnostics>();
        engine::core::log::info!(
            "  exposure EV100 {:.1} (metered {:.1})",
            d.get("render.ev100").unwrap_or(f64::NAN),
            d.get("render.ev100_metered").unwrap_or(f64::NAN)
        );
    }
    let mut tour = world.resource_mut::<Tour>();
    if frames == capture_at {
        let path = format!("{out_dir}/{:02}_{}.png", index + 1, place.key);
        tour.frames = 0;
        tour.index += 1;
        world.resource_mut::<Screenshots>().request(path);
    } else {
        tour.frames += 1;
    }
}

/// Hold the stop (true) while virtual geometry is still building meshes in view, or the
/// terrain its patches (on worker threads; it draws their parents meanwhile), so the picture
/// shows the finished relief, not the smooth shapes drawn until it is ready.
fn wait_for_builds(world: &mut World, stall_secs: f32) -> bool {
    let Some(state) = with_renderer(world, |r, _| {
        let describe = r.describe();
        describe["features"]
            .as_array()
            .and_then(|f| f.iter().find(|f| f["name"] == "meshes"))
            .map(|f| f["state"]["virtual_geometry"].clone())
            .unwrap_or_default()
    }) else {
        return false;
    };
    let (patches_building, patches_missing) = world
        .get_resource::<engine::core::diagnostics::Diagnostics>()
        .map_or((0.0, 0.0), |d| {
            (
                d.get("terrain.patches_building").unwrap_or(0.0),
                d.get("terrain.patches_missing").unwrap_or(0.0),
            )
        });
    let patches = (patches_building + patches_missing) as u64;
    if patches_building > 0.0 {
        // Let the workers get on with them before drawing another frame.
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    // Foliage plants its regions and uploads its trees on workers, over frames.
    let foliage = world
        .get_resource::<engine_foliage::FoliageLoading>()
        .is_some_and(|f| !f.is_ready()) as u64;
    let building = state["meshes_building"].as_u64().unwrap_or(0) + patches + foliage;
    // Progress on either keeps the wait from counting as stalled.
    let ready = (
        state["meshes_ready"].as_u64().unwrap_or(0),
        patches + foliage,
    );
    let now = std::time::Instant::now();
    let mut tour = world.resource_mut::<Tour>();
    let Some((since, last_ready, changed)) = tour.waiting else {
        // One frame first: a renderer parked through a quick warm-up has only just seen the
        // meshes in view.
        tour.waiting = Some((now, ready, now));
        return true;
    };
    // Fewer patches pending than ever before is progress; a count going up and down (the
    // terrain trading patches as the view settles) is not, or the wait would never stall.
    let ready = (ready.0, ready.1.min(last_ready.1));
    let changed = if ready != last_ready { now } else { changed };
    let waited = now.duration_since(since).as_secs_f32();
    let stalled = now.duration_since(changed).as_secs_f32();
    if building == 0 || waited > BUILD_WAIT_SECS || stalled > stall_secs {
        if waited > 1.0 {
            engine::core::log::info!(
                "  waited {waited:.0} s for meshes' clusters, terrain patches and foliage ({building} not ready)"
            );
        }
        tour.waiting = None;
        return false;
    }
    tour.waiting = Some((since, ready, changed));
    true
}

/// Pause drawing, so frames run the simulation only (as the module galleries do between
/// pictures).
fn park(world: &mut World) {
    world.insert_resource(RenderPaused);
}

fn unpark(world: &mut World) {
    world.remove_resource::<RenderPaused>();
}
