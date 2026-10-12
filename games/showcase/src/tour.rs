//! `--tour`: fly to every place, wait for exposure to settle, save
//! `screenshots/tour/NN_<key>.png`, then quit. Used by CI-style visual checks and by AI agents
//! that want to see the whole map in one go.

use crate::camera;
use crate::day_cycle::DayCycle;
use crate::places::{places, Place};
use engine::prelude::*;

/// Frames to wait at each place before capturing (auto-exposure and the environment map
/// need a few frames to adapt).
const SETTLE_FRAMES: u32 = 4;

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
        .set_source(engine::core::time::TimeSource::Manual { step_secs: 0.05 });
    app.insert_resource(Tour::new(only));
    app.add_systems(Update, run.after(camera::fly));
}

fn run(world: &mut World) {
    let (place, frames, index, total, out_dir) = {
        let tour = world.resource::<Tour>();
        let Some(place) = tour.places.get(tour.index).cloned() else {
            // Done: give the last screenshot a frame to be written, then exit.
            world.write_message(AppExit::Success);
            return;
        };
        (place, tour.frames, tour.index, tour.places.len(), tour.out_dir.clone())
    };

    if frames == 0 {
        camera::go_to(world, &place);
        world.resource_mut::<DayCycle>().hour = place.hour;
        // Adapt exposure within a couple of frames instead of seconds.
        let mut cams = world.query::<&mut Camera>();
        for mut cam in cams.iter_mut(world) {
            if let Exposure::Auto { speed, .. } = &mut cam.exposure {
                *speed = 40.0;
            }
        }
        engine::core::log::info!("tour {}/{}: {}", index + 1, total, place.name);
    }

    let mut tour = world.resource_mut::<Tour>();
    if frames == SETTLE_FRAMES {
        let path = format!("{out_dir}/{:02}_{}.png", index + 1, place.key);
        tour.frames = 0;
        tour.index += 1;
        world.resource_mut::<Screenshots>().request(path);
    } else {
        tour.frames += 1;
    }
}
