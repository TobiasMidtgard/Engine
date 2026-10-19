//! `--tour`: fly to every place, wait for exposure to settle, save
//! `screenshots/tour/NN_<key>.png`, then quit. Used by CI-style visual checks and by AI agents
//! that want to see the whole map in one go.

use crate::camera;
use crate::places::{Place, places};
use engine::prelude::*;
use engine::render::{RenderDisabled, Renderer};

/// Frames to wait at each place before capturing (metered auto-exposure reads back a frame or
/// two late, then adapts).
const SETTLE_FRAMES: u32 = 8;
/// Simulated seconds per tour frame.
const STEP_SECS: f32 = 0.05;

/// The renderer, parked while a stop with [`Place::quick_warmup`] warms up.
#[derive(Resource)]
struct Parked(Renderer);

#[derive(Resource, Debug)]
pub struct Tour {
    pub places: Vec<Place>,
    pub index: usize,
    pub frames: u32,
    pub out_dir: String,
}

impl Tour {
    pub fn new(only: Option<&str>) -> Self {
        let mut list = places();
        if let Some(only) = only {
            let keys: Vec<&str> = only.split(',').collect();
            list.retain(|p| keys.contains(&p.key));
        }
        Self {
            places: list,
            index: 0,
            frames: 0,
            out_dir: "screenshots/tour".into(),
        }
    }
}

/// Install the tour: deterministic time steps and fast-adapting exposure.
pub fn install(app: &mut App, only: Option<&str>) {
    app.world
        .resource_mut::<Time>()
        .set_source(engine::core::time::TimeSource::Manual {
            step_secs: STEP_SECS as f64,
        });
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

    // The place's calls run a few frames after the move; exposure settles at the end of the
    // warm-up that follows them.
    let capture_at = crate::places::ARRIVE_DELAY
        + 1
        + SETTLE_FRAMES
        + (place.warmup_secs / STEP_SECS).ceil() as u32;
    if frames == 0 {
        camera::go_to(world, &place);
        crate::places::visit_soon(world, &place);
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

    // A quick warm-up runs without drawing, from after the arrival calls until the frames
    // that settle the picture.
    let settle_from = capture_at - SETTLE_FRAMES;
    if place.quick_warmup && frames == crate::places::ARRIVE_DELAY + 2 && frames < settle_from {
        park(world);
    }
    if frames == settle_from {
        unpark(world);
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

/// Take the renderer out of the world, so frames run the simulation only (as the module
/// galleries do between pictures).
fn park(world: &mut World) {
    if let Some(renderer) = world.remove_resource::<Renderer>() {
        world.insert_resource(Parked(renderer));
        world.insert_resource(RenderDisabled("parked through a quick warm-up".into()));
    }
}

fn unpark(world: &mut World) {
    if let Some(Parked(renderer)) = world.remove_resource::<Parked>() {
        world.remove_resource::<RenderDisabled>();
        world.insert_resource(renderer);
    }
}
