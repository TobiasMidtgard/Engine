//! Named viewpoints: camera bookmarks (number keys), the screenshot tour and the
//! `showcase.goto` API all use this one list.

use crate::landscape;
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

/// Eye and target relative to a zone's center. The eye never ends up below the ground.
fn near(key: &str, eye: [f64; 3], look: [f64; 3]) -> (DVec3, DVec3) {
    let c = zone(key).center;
    (
        above_ground(c + DVec3::from_array(eye), 1.7),
        c + DVec3::from_array(look),
    )
}

fn above_ground(p: DVec3, clearance: f64) -> DVec3 {
    let ground = landscape::shared().height(p.x, p.z);
    DVec3::new(p.x, p.y.max(ground + clearance), p.z)
}

/// Every viewpoint, in tour order.
pub fn places() -> Vec<Place> {
    let place = |key, name, (position, target): (DVec3, DVec3), hour| Place {
        key,
        name,
        position,
        target,
        hour,
    };
    let tower = zone("spaceport").center + DVec3::new(70.0, 0.0, 60.0);
    let moon = moon_position();
    let moon_dir = moon.normalize();
    let sun = crate::day_cycle::sun_direction(10.0).as_dvec3();
    let moon_side = (sun * 0.7 - moon_dir * 0.7).normalize();
    vec![
        place(
            "overview",
            "The valley",
            (
                DVec3::new(250.0, 330.0, -1050.0),
                DVec3::new(-60.0, 0.0, 120.0),
            ),
            9.5,
        ),
        place(
            "plaza",
            "Material plaza and the village",
            near("plaza", [10.0, 4.5, 16.0], [0.0, 1.8, -1.0]),
            11.0,
        ),
        place(
            "market",
            "Market street: shop, dialogue and maps",
            near("market", [12.0, 3.2, -16.0], [0.0, 1.5, 3.0]),
            10.5,
        ),
        place(
            "stage",
            "Animation stage",
            near("stage", [6.0, 4.0, -26.0], [0.0, 2.5, 2.0]),
            15.0,
        ),
        place(
            "physics",
            "Physics playground",
            near("physics", [22.0, 7.0, 24.0], [0.0, 1.0, -4.0]),
            13.5,
        ),
        place(
            "yard",
            "Cloth and soft-body yard",
            near("yard", [14.0, 8.0, 32.0], [0.0, 3.0, -4.0]),
            15.5,
        ),
        place(
            "magic",
            "Magic circle at dusk",
            near("magic", [22.0, 4.0, 20.0], [0.0, 2.0, 0.0]),
            18.35,
        ),
        place(
            "gallery2d",
            "2D and 2.5D gallery",
            near("gallery2d", [0.0, 5.0, 18.0], [0.0, 4.0, -3.0]),
            12.5,
        ),
        place(
            "forest",
            "Living forest",
            near("forest", [14.0, 2.5, 12.0], [-10.0, 4.0, -6.0]),
            9.0,
        ),
        place(
            "meadow",
            "Wildlife meadow in the morning",
            near("meadow", [34.0, 3.0, 30.0], [0.0, 1.0, 0.0]),
            7.8,
        ),
        place(
            "lake",
            "Lake",
            near("lake", [-80.0, 6.0, 60.0], [10.0, 0.0, -10.0]),
            16.5,
        ),
        place(
            "footsteps",
            "Footstep trail: mud, puddle, sand, gravel, snow, wood, metal",
            near("footsteps", [9.0, 3.0, -44.0], [0.0, 0.0, 0.0]),
            14.0,
        ),
        place(
            "cave",
            "Mushroom cave",
            near("cave", [0.0, 3.0, -6.0], [0.0, 0.5, 10.0]),
            14.0,
        ),
        place(
            "cave_mouth",
            "The cave in its hill",
            near("cave", [30.0, 6.0, -60.0], [0.0, 6.0, 0.0]),
            15.5,
        ),
        place(
            "arena",
            "Combat arena",
            near("arena", [0.0, 14.0, -40.0], [0.0, 0.0, 0.0]),
            12.0,
        ),
        place(
            "parkour",
            "Movement course",
            near("parkour", [32.0, 10.0, -32.0], [-5.0, 3.0, 0.0]),
            11.5,
        ),
        place(
            "fortress",
            "Siege fortress on its hill",
            near("fortress", [16.0, 6.0, -52.0], [0.0, 6.0, 0.0]),
            15.0,
        ),
        place(
            "voxel",
            "Voxel hill",
            near("voxel", [30.0, 12.0, -32.0], [0.0, 4.0, 0.0]),
            13.0,
        ),
        place(
            "field",
            "Grass field",
            near("field", [-40.0, 3.0, -45.0], [0.0, 1.0, 0.0]),
            16.0,
        ),
        place(
            "range",
            "Smoke range",
            near("range", [0.0, 4.0, -34.0], [0.0, 1.5, 10.0]),
            12.0,
        ),
        place(
            "weather",
            "From the weather station hill",
            near("weather", [-8.0, 3.0, -14.0], [-215.0, -20.0, -370.0]),
            16.2,
        ),
        place(
            "track",
            "Vehicle track",
            near("track", [60.0, 60.0, 160.0], [0.0, 0.0, 0.0]),
            10.0,
        ),
        place(
            "airfield",
            "Airfield",
            near("airfield", [-160.0, 25.0, -70.0], [-40.0, 0.0, 0.0]),
            10.5,
        ),
        place(
            "spaceport",
            "Spaceport",
            near("spaceport", [-60.0, 12.0, -70.0], [30.0, 40.0, 30.0]),
            13.0,
        ),
        place(
            "tower",
            "Top of the 600 m tower",
            (
                tower + DVec3::new(40.0, 640.0, 40.0),
                DVec3::new(0.0, 0.0, 0.0),
            ),
            9.0,
        ),
        place(
            "stratosphere",
            "30 km up",
            (
                DVec3::new(0.0, 30_000.0, 0.0),
                DVec3::new(0.0, 0.0, -150_000.0),
            ),
            8.0,
        ),
        place(
            "orbit",
            "Low orbit, 420 km",
            (
                DVec3::new(0.0, 420_000.0, 0.0),
                DVec3::new(0.0, 110_000.0, -950_000.0),
            ),
            10.0,
        ),
        // From the Moon's sunlit side (a gibbous Moon), 5000 km out.
        place(
            "moon",
            "The Moon, 384 400 km away",
            (moon + moon_side * 5_000_000.0, moon),
            10.0,
        ),
        place(
            "sunset",
            "Sunset over the plaza",
            (DVec3::new(-40.0, 5.0, 20.0), DVec3::new(60.0, 6.0, -20.0)),
            17.8,
        ),
        place(
            "night",
            "The village at night",
            (DVec3::new(-26.0, 6.0, 30.0), DVec3::new(10.0, 2.0, -10.0)),
            22.5,
        ),
    ]
}

pub fn find(key: &str) -> Option<Place> {
    places().into_iter().find(|p| p.key == key)
}
