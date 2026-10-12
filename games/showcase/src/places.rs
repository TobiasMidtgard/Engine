//! Named viewpoints: camera bookmarks (number keys), the screenshot tour and the
//! `showcase.goto` API all use this one list.

use crate::zones::{moon_position, zone};
use engine::prelude::*;

/// A camera viewpoint with a time of day.
#[derive(Debug, Clone)]
pub struct Place {
    pub key: &'static str,
    pub name: &'static str,
    pub position: DVec3,
    pub target: DVec3,
    /// Hour to show it at (the tour sets it; bookmarks keep the current time).
    pub hour: f32,
}

fn near(key: &str, eye: [f64; 3], look: [f64; 3]) -> (DVec3, DVec3) {
    let c = zone(key).center;
    (c + DVec3::from_array(eye), c + DVec3::from_array(look))
}

/// Every viewpoint, in tour order.
pub fn places() -> Vec<Place> {
    let place = |key, name, (position, target): (DVec3, DVec3), hour| Place { key, name, position, target, hour };
    let tower = zone("spaceport").center + DVec3::new(70.0, 0.0, 60.0);
    let moon = moon_position();
    let moon_dir = moon.normalize();
    vec![
        place("overview", "The whole map", (DVec3::new(520.0, 260.0, 520.0), DVec3::new(20.0, 0.0, -60.0)), 10.0),
        place("plaza", "Material plaza", near("plaza", [10.0, 4.5, 16.0], [0.0, 1.8, -1.0]), 11.0),
        place("forest", "Living forest", near("forest", [95.0, 6.0, 60.0], [0.0, 4.0, 0.0]), 9.0),
        place("lake", "Lake", near("lake", [75.0, 8.0, 70.0], [-20.0, 0.0, -10.0]), 16.5),
        place("cave", "Mushroom cave", near("cave", [0.0, 3.5, -12.0], [0.0, 0.5, 6.0]), 14.0),
        place("fortress", "Siege fortress", near("fortress", [14.0, 6.0, -38.0], [0.0, 5.0, 0.0]), 15.0),
        place("arena", "Combat arena", near("arena", [0.0, 14.0, -40.0], [0.0, 0.0, 0.0]), 12.0),
        place("field", "Grass field", near("field", [-40.0, 3.0, -45.0], [0.0, 1.0, 0.0]), 17.3),
        place("track", "Vehicle track", near("track", [60.0, 60.0, 160.0], [0.0, 0.0, 0.0]), 10.0),
        place("spaceport", "Spaceport", near("spaceport", [-60.0, 12.0, -70.0], [30.0, 40.0, 30.0]), 13.0),
        place("tower", "Top of the 600 m tower", (tower + DVec3::new(40.0, 640.0, 40.0), DVec3::new(0.0, 0.0, 0.0)), 9.0),
        place("stratosphere", "30 km up", (DVec3::new(0.0, 30_000.0, 0.0), DVec3::new(0.0, 0.0, -150_000.0)), 8.0),
        place("orbit", "Low orbit, 420 km", (DVec3::new(0.0, 420_000.0, 0.0), DVec3::new(0.0, -6_360_000.0, -2_500_000.0)), 10.0),
        place("moon", "The Moon, 384 400 km away", (moon - moon_dir * 5_000_000.0, moon), 10.0),
        place("sunset", "Sunset over the plaza", (DVec3::new(-40.0, 5.0, 20.0), DVec3::new(60.0, 6.0, -20.0)), 17.8),
        place("night", "Spaceport at night", near("spaceport", [-50.0, 8.0, -50.0], [0.0, 2.0, 0.0]), 23.0),
    ]
}

pub fn find(key: &str) -> Option<Place> {
    places().into_iter().find(|p| p.key == key)
}
