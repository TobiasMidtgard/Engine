//! Headless pictures of planets and terrain, for reviews and the project's screenshot folder.
//!
//! ```text
//! cargo run --release -p engine_terrain --example terrain_shots -- [out_dir] [WxH] [names…]
//! ```
//!
//! Writes one PNG per view (all views when no names are given). Needs a GPU or lavapipe.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_terrain::{
    FlatPad, OverlayPaint, PathWear, PlanetTerrain, TerrainPlugin, WorldKind, WorldRecipe,
    WornGround, demo,
};
use engine_world::{PrecipitationKind, SimulationFocus, SurfaceLayers, WeatherState};

#[derive(Clone, Copy)]
enum World3 {
    Earth,
    Moon,
    Mars,
    /// A planet of another star made from a recipe of this kind ([`exo_recipe`]).
    Exo(WorldKind),
}

/// A planet of each kind, as the space module's planets of the nearby stars make them.
fn exo_recipe(kind: WorldKind) -> WorldRecipe {
    let r = WorldRecipe {
        kind,
        seed: 11,
        radius_m: 6_000_000.0,
        ..Default::default()
    };
    match kind {
        WorldKind::Airless => WorldRecipe {
            temperature_k: 330.0,
            crater_density: 0.9,
            ..r
        },
        WorldKind::Lava => WorldRecipe {
            temperature_k: 1500.0,
            color: [0.09, 0.075, 0.07],
            // A 1575 K black body's luminance (engine_space::look::black_body_luminance).
            lava_glow_nits: 12_000.0,
            ..r
        },
        WorldKind::Desert => WorldRecipe {
            temperature_k: 600.0,
            pressure_bar: 0.05,
            crater_density: 0.5,
            color: [0.62, 0.42, 0.26],
            ..r
        },
        WorldKind::Terra => WorldRecipe {
            temperature_k: 290.0,
            pressure_bar: 1.0,
            sea_fraction: 0.5,
            star_kelvin: 5260.0,
            ..r
        },
        WorldKind::Ocean => WorldRecipe {
            temperature_k: 295.0,
            pressure_bar: 1.5,
            sea_fraction: 0.85,
            star_kelvin: 3050.0,
            ..r
        },
        WorldKind::ColdDesert => WorldRecipe {
            temperature_k: 220.0,
            pressure_bar: 0.6,
            crater_density: 0.1,
            color: [0.58, 0.44, 0.36],
            ..r
        },
        WorldKind::Ice => WorldRecipe {
            temperature_k: 160.0,
            pressure_bar: 0.0,
            color: [0.84, 0.87, 0.92],
            ..r
        },
        WorldKind::Hazy => WorldRecipe {
            temperature_k: 95.0,
            pressure_bar: 1.4,
            sea_fraction: 0.15,
            color: [0.78, 0.52, 0.24],
            ..r
        },
    }
}

#[derive(Clone, Copy, Default)]
enum Ground {
    #[default]
    Dry,
    Rain,
    Snow,
}

struct View {
    name: &'static str,
    world: World3,
    eye: DVec3,
    target: DVec3,
    /// Place `eye` this high above the ground below it instead of using `eye.y`.
    above_ground: Option<f64>,
    /// Likewise for `target`.
    target_above_ground: Option<f64>,
    /// Move the eye to this many meters inland of where the line from `eye` toward `target`
    /// first reaches the sea (then `above_ground` above the ground there), and the target as
    /// far beyond that point as it was from the eye.
    shore: Option<f64>,
    /// Sun azimuth (degrees from north toward east) and elevation (degrees).
    sun: (f64, f64),
    /// Far from home: the eye and target as (latitude, longitude, height above the ground),
    /// the target the planet's center when only the eye is given, and the sun overhead at
    /// (latitude, longitude) instead of `sun`.
    eye_ll: Option<[f64; 3]>,
    target_ll: Option<[f64; 3]>,
    sun_ll: Option<[f64; 2]>,
    /// The showcase's own auto exposure by default, so pictures look the way the game shows
    /// them; fixed for views of space, where the black sky would brighten the planet.
    exposure: Exposure,
    fov: f32,
    lab: bool,
    /// Hand-made, painted ground around home (roads, a plaza, fields, woods, a beach).
    painted: bool,
    ground: Ground,
    /// Paths worn into the ground before the picture (none with `NO_PATHS` set, for a
    /// before picture).
    paths: &'static [PathLine],
    /// The eye and target the paths' points are measured from, when not this view's own.
    paths_from: Option<(DVec3, DVec3)>,
    /// Places people walk between, `trips` times in all, laying their own trails (meters
    /// ahead of the eye, meters to its right, how busy). With `STRAIGHT` set everyone walks
    /// the straight line instead, for a before picture.
    places: &'static [(f64, f64, f32)],
    trips: u32,
    /// A town of street lamps around home, lit at night (no procedural towns and cities, so
    /// only its own lamps glow).
    town: bool,
    /// Someone walked along the dirt road and the grass beside it (footprints).
    steps: bool,
}

/// A path walked `passes` times along a line of (meters ahead of the eye, meters to its
/// right) points, each walker within `spread` meters of it.
struct PathLine {
    points: &'static [(f64, f64)],
    passes: u32,
    spread: f32,
}

/// A well-used path across the meadow, a fork off it walked less, and a faint shortcut.
const MEADOW_PATHS: &[PathLine] = &[
    PathLine {
        points: &[
            (2.0, 0.6),
            (25.0, -1.0),
            (60.0, 2.0),
            (110.0, 8.0),
            (180.0, 4.0),
            (300.0, -10.0),
        ],
        passes: 600,
        spread: 0.22,
    },
    PathLine {
        points: &[(60.0, 2.0), (85.0, -18.0), (130.0, -45.0), (200.0, -70.0)],
        passes: 140,
        spread: 0.25,
    },
    PathLine {
        points: &[(20.0, -0.5), (45.0, 14.0), (90.0, 36.0)],
        passes: 35,
        spread: 0.2,
    },
];

const AUTO: Exposure = Exposure::Auto {
    min_ev100: -3.0,
    max_ev100: 18.0,
    speed: 2.0,
    compensation: 0.0,
};

const BASE: View = View {
    name: "",
    world: World3::Earth,
    eye: DVec3::ZERO,
    target: DVec3::ZERO,
    above_ground: None,
    target_above_ground: None,
    shore: None,
    sun: (220.0, 35.0),
    eye_ll: None,
    target_ll: None,
    sun_ll: None,
    exposure: AUTO,
    fov: 60.0,
    lab: false,
    painted: false,
    ground: Ground::Dry,
    paths: &[],
    paths_from: None,
    places: &[],
    trips: 0,
    town: false,
    steps: false,
};

/// A farm by the meadow's edge and four places its people walk to (and between).
const MEADOW_PLACES: &[(f64, f64, f32)] = &[
    (5.0, 0.0, 4.0),
    (170.0, -55.0, 1.0),
    (185.0, 5.0, 1.0),
    (140.0, 70.0, 1.0),
    (60.0, 95.0, 1.0),
];
const MEADOW_FRAME: (DVec3, DVec3) = (
    DVec3::new(900.0, 0.0, 300.0),
    DVec3::new(1_300.0, 0.0, -400.0),
);

fn views() -> Vec<View> {
    vec![
        View {
            name: "lab_prints",
            eye: DVec3::new(-21.0, 1.9, 9.0),
            target: DVec3::new(-13.0, 0.0, 1.0),
            sun: (250.0, 18.0),
            lab: true,
            ..BASE
        },
        View {
            name: "lab_sand",
            eye: DVec3::new(1.5, 1.7, 8.0),
            target: DVec3::new(5.5, 0.0, 1.0),
            sun: (250.0, 18.0),
            lab: true,
            ..BASE
        },
        View {
            name: "lab_tracks",
            eye: DVec3::new(2.0, 2.4, 13.0),
            target: DVec3::new(8.0, 0.0, 5.0),
            sun: (250.0, 20.0),
            lab: true,
            ..BASE
        },
        View {
            name: "lab_overview",
            eye: DVec3::new(-4.0, 22.0, 34.0),
            target: DVec3::new(0.0, 0.0, -6.0),
            sun: (230.0, 32.0),
            lab: true,
            ..BASE
        },
        View {
            name: "lab_rain",
            eye: DVec3::new(-21.0, 1.9, 9.0),
            target: DVec3::new(-13.0, 0.0, 1.0),
            sun: (250.0, 18.0),
            exposure: Exposure::Manual { ev100: 13.5 },
            lab: true,
            ground: Ground::Rain,
            ..BASE
        },
        View {
            name: "lab_snow",
            eye: DVec3::new(-4.0, 22.0, 34.0),
            target: DVec3::new(0.0, 0.0, -6.0),
            sun: (230.0, 25.0),
            exposure: Exposure::Manual { ev100: 15.0 },
            lab: true,
            ground: Ground::Snow,
            ..BASE
        },
        View {
            name: "painted_ground",
            eye: DVec3::new(-35.0, 38.0, 75.0),
            target: DVec3::new(15.0, 0.0, -45.0),
            sun: (230.0, 32.0),
            painted: true,
            ..BASE
        },
        View {
            name: "painted_close",
            eye: DVec3::new(-6.0, 2.2, 14.0),
            target: DVec3::new(6.0, 0.0, -14.0),
            sun: (235.0, 24.0),
            painted: true,
            ..BASE
        },
        // Face to the ground: a crouched look at the dirt road, the gravel ring round the
        // plaza, and the road seen standing, under a low sun (pebbles, clods and grit).
        View {
            name: "ground_macro_dirt",
            eye: DVec3::new(0.0, 0.0, 0.3),
            target: DVec3::new(0.7, 0.0, -0.5),
            above_ground: Some(0.45),
            target_above_ground: Some(0.0),
            sun: (200.0, 18.0),
            fov: 60.0,
            painted: true,
            ..BASE
        },
        View {
            name: "ground_macro_gravel",
            eye: DVec3::new(0.0, 0.0, -57.0),
            target: DVec3::new(0.6, 0.0, -58.2),
            above_ground: Some(0.5),
            target_above_ground: Some(0.0),
            sun: (200.0, 18.0),
            fov: 60.0,
            painted: true,
            ..BASE
        },
        View {
            name: "ground_road_eye",
            eye: DVec3::new(-3.0, 0.0, -0.8),
            target: DVec3::new(4.0, 0.0, 1.5),
            above_ground: Some(1.7),
            target_above_ground: Some(0.0),
            sun: (200.0, 18.0),
            painted: true,
            ..BASE
        },
        View {
            name: "ground_steps_eye",
            eye: DVec3::new(-3.0, 0.0, -0.8),
            target: DVec3::new(4.0, 0.0, 1.5),
            above_ground: Some(1.7),
            target_above_ground: Some(0.0),
            sun: (200.0, 18.0),
            painted: true,
            steps: true,
            ..BASE
        },
        View {
            name: "painted_snow",
            eye: DVec3::new(-35.0, 38.0, 75.0),
            target: DVec3::new(15.0, 0.0, -45.0),
            sun: (230.0, 32.0),
            painted: true,
            ground: Ground::Snow,
            exposure: Exposure::Manual { ev100: 15.0 },
            ..BASE
        },
        View {
            name: "snow_eye",
            eye: DVec3::new(25.0, 0.0, -12.0),
            target: DVec3::new(-15.0, -1.5, 22.0),
            above_ground: Some(1.7),
            sun: (230.0, 16.0),
            exposure: Exposure::Manual { ev100: 15.0 },
            painted: true,
            ground: Ground::Snow,
            ..BASE
        },
        View {
            name: "snow_valley",
            eye: DVec3::new(-40.0, 140.0, 330.0),
            target: DVec3::new(20.0, 0.0, -300.0),
            sun: (230.0, 25.0),
            exposure: Exposure::Manual { ev100: 15.0 },
            painted: true,
            ground: Ground::Snow,
            ..BASE
        },
        View {
            name: "painted_patchwork",
            eye: DVec3::new(-60.0, 32.0, -150.0),
            target: DVec3::new(20.0, 0.0, -260.0),
            sun: (230.0, 32.0),
            painted: true,
            ..BASE
        },
        // The same patchwork after rain: wet sand darkens and shines, soil turns to mud.
        View {
            name: "painted_wet",
            eye: DVec3::new(-60.0, 32.0, -150.0),
            target: DVec3::new(20.0, 0.0, -260.0),
            sun: (230.0, 32.0),
            painted: true,
            ground: Ground::Rain,
            ..BASE
        },
        View {
            name: "painted_border",
            eye: DVec3::new(-4.0, 2.2, -172.0),
            target: DVec3::new(8.0, 0.0, -200.0),
            sun: (235.0, 28.0),
            painted: true,
            ..BASE
        },
        View {
            name: "tinted_meadow",
            eye: DVec3::new(185.0, 0.0, 30.0),
            target: DVec3::new(330.0, 0.0, -230.0),
            above_ground: Some(1.7),
            sun: (230.0, 32.0),
            painted: true,
            ..BASE
        },
        View {
            name: "tinted_meadow_high",
            eye: DVec3::new(175.0, 24.0, 35.0),
            target: DVec3::new(300.0, 0.0, -150.0),
            sun: (230.0, 32.0),
            painted: true,
            ..BASE
        },
        View {
            name: "meadow_eye",
            eye: DVec3::new(900.0, 0.0, 300.0),
            target: DVec3::new(1_300.0, 0.0, -400.0),
            above_ground: Some(1.7),
            sun: (230.0, 32.0),
            ..BASE
        },
        View {
            name: "paths_meadow",
            eye: DVec3::new(900.0, 0.0, 300.0),
            target: DVec3::new(1_300.0, 0.0, -400.0),
            above_ground: Some(1.7),
            sun: (230.0, 32.0),
            paths: MEADOW_PATHS,
            ..BASE
        },
        View {
            name: "paths_meadow_high",
            eye: DVec3::new(900.0, 0.0, 300.0),
            target: DVec3::new(944.6, 0.0, 221.9),
            above_ground: Some(22.0),
            target_above_ground: Some(0.0),
            sun: (230.0, 32.0),
            paths: MEADOW_PATHS,
            ..BASE
        },
        // People walking between a farm and four places lay their own trails.
        View {
            name: "paths_desire",
            eye: DVec3::new(835.5, 0.0, 332.0),
            target: DVec3::new(962.0, 0.0, 222.0),
            above_ground: Some(95.0),
            target_above_ground: Some(0.0),
            sun: (230.0, 32.0),
            paths_from: Some(MEADOW_FRAME),
            places: MEADOW_PLACES,
            trips: 6000,
            ..BASE
        },
        View {
            name: "paths_desire_eye",
            eye: DVec3::new(905.0, 0.0, 291.3),
            target: DVec3::new(985.0, 0.0, 151.0),
            above_ground: Some(1.7),
            sun: (230.0, 32.0),
            paths_from: Some(MEADOW_FRAME),
            places: MEADOW_PLACES,
            trips: 6000,
            ..BASE
        },
        // The same paths from a hill a few hundred meters off, beyond the fine window.
        View {
            name: "paths_hill",
            eye: DVec3::new(620.0, 0.0, 790.0),
            target: DVec3::new(1_000.0, 0.0, 120.0),
            above_ground: Some(160.0),
            target_above_ground: Some(0.0),
            sun: (230.0, 32.0),
            paths: MEADOW_PATHS,
            paths_from: Some((
                DVec3::new(900.0, 0.0, 300.0),
                DVec3::new(1_300.0, 0.0, -400.0),
            )),
            ..BASE
        },
        View {
            name: "paths_rain",
            eye: DVec3::new(900.0, 0.0, 300.0),
            target: DVec3::new(1_300.0, 0.0, -400.0),
            above_ground: Some(1.7),
            sun: (230.0, 32.0),
            exposure: Exposure::Manual { ev100: 13.5 },
            ground: Ground::Rain,
            paths: MEADOW_PATHS,
            ..BASE
        },
        View {
            name: "paths_snow",
            eye: DVec3::new(25.0, 0.0, -12.0),
            target: DVec3::new(-15.0, -1.5, 22.0),
            above_ground: Some(1.7),
            sun: (230.0, 22.0),
            exposure: Exposure::Manual { ev100: 15.0 },
            painted: true,
            ground: Ground::Snow,
            paths: &[
                PathLine {
                    points: &[
                        (1.5, 0.5),
                        (15.0, 0.8),
                        (35.0, -2.0),
                        (55.0, 1.5),
                        (90.0, 0.0),
                    ],
                    passes: 260,
                    spread: 0.25,
                },
                PathLine {
                    points: &[(15.0, 0.8), (24.0, 9.0), (40.0, 20.0)],
                    passes: 40,
                    spread: 0.15,
                },
            ],
            ..BASE
        },
        View {
            name: "paths_beach",
            eye: DVec3::new(-7_000.0, 0.0, 12_500.0),
            target: DVec3::new(-17_000.0, 0.0, 20_000.0),
            shore: Some(60.0),
            above_ground: Some(4.0),
            target_above_ground: Some(-40.0),
            sun: (255.0, 25.0),
            paths: &[PathLine {
                points: &[(2.0, -3.0), (20.0, -1.0), (40.0, 2.0), (62.0, 4.0)],
                passes: 220,
                spread: 0.35,
            }],
            ..BASE
        },
        View {
            name: "earth_range",
            eye: DVec3::new(-600.0, 0.0, -1_500.0),
            target: DVec3::new(2_000.0, 1_200.0, -32_000.0),
            above_ground: Some(25.0),
            sun: (245.0, 16.0),
            ..BASE
        },
        View {
            name: "earth_hills",
            eye: DVec3::new(0.0, 0.0, 900.0),
            target: DVec3::new(-300.0, 0.0, -2_500.0),
            above_ground: Some(450.0),
            sun: (230.0, 35.0),
            ..BASE
        },
        View {
            name: "woods_aerial",
            eye: DVec3::new(380.0, 0.0, 470.0),
            target: DVec3::new(380.0, 0.0, 250.0),
            above_ground: Some(90.0),
            sun: (230.0, 35.0),
            ..BASE
        },
        // Woods up the foothills of the northern range: broadleaf below, dark conifers above.
        View {
            name: "woods_slopes",
            eye: DVec3::new(-2_500.0, 0.0, -11_000.0),
            target: DVec3::new(-1_500.0, 1_000.0, -21_000.0),
            above_ground: Some(350.0),
            sun: (200.0, 30.0),
            ..BASE
        },
        View {
            name: "woods_cold_aerial",
            eye: DVec3::new(-1_000.0, 0.0, -19_700.0),
            target: DVec3::new(-1_000.0, 0.0, -19_950.0),
            above_ground: Some(90.0),
            target_above_ground: Some(0.0),
            sun: (200.0, 35.0),
            ..BASE
        },
        View {
            name: "earth_coast",
            eye: DVec3::new(-7_000.0, 0.0, 12_500.0),
            target: DVec3::new(-17_000.0, -400.0, 20_000.0),
            above_ground: Some(250.0),
            sun: (255.0, 12.0),
            exposure: Exposure::Manual { ev100: 14.8 },
            ..BASE
        },
        // Down on the beach on the same coast, with the surf running in.
        View {
            name: "earth_beach",
            eye: DVec3::new(-7_000.0, 0.0, 12_500.0),
            target: DVec3::new(-17_000.0, 0.0, 20_000.0),
            shore: Some(60.0),
            above_ground: Some(4.0),
            target_above_ground: Some(-40.0),
            sun: (255.0, 25.0),
            fov: 60.0,
            ..BASE
        },
        View {
            name: "earth_mountains",
            eye: DVec3::new(-14_000.0, 0.0, -19_000.0),
            target: DVec3::new(6_000.0, 1_800.0, -33_000.0),
            above_ground: Some(1_200.0),
            sun: (250.0, 22.0),
            ..BASE
        },
        View {
            name: "range_dusk",
            eye: DVec3::new(2_000.0, 0.0, -11_000.0),
            target: DVec3::new(-3_000.0, 1_500.0, -20_000.0),
            above_ground: Some(300.0),
            sun: (285.0, 7.0),
            ..BASE
        },
        View {
            name: "earth_cliffs",
            eye: DVec3::new(-11_500.0, 0.0, -22_400.0),
            target: DVec3::new(-13_000.0, 1_900.0, -21_700.0),
            above_ground: Some(350.0),
            sun: (175.0, 28.0),
            ..BASE
        },
        View {
            name: "earth_cliff_beds",
            eye: DVec3::new(-12_300.0, 0.0, -22_050.0),
            target: DVec3::new(-13_000.0, 1_500.0, -21_750.0),
            above_ground: Some(120.0),
            sun: (175.0, 28.0),
            ..BASE
        },
        View {
            name: "earth_river",
            eye: DVec3::new(3_500.0, 0.0, -6_500.0),
            target: DVec3::new(9_800.0, -150.0, -6_200.0),
            above_ground: Some(150.0),
            sun: (230.0, 30.0),
            ..BASE
        },
        View {
            name: "earth_rivers_air",
            eye: DVec3::new(6_500.0, 0.0, -1_500.0),
            target: DVec3::new(9_000.0, 0.0, -5_000.0),
            above_ground: Some(1_200.0),
            sun: (230.0, 30.0),
            ..BASE
        },
        // A lowland river winding through its flood plain, from a few hundred meters up.
        View {
            name: "earth_meanders",
            eye: DVec3::new(-7_200.0, 0.0, 4_600.0),
            target: DVec3::new(-8_300.0, 0.0, 2_900.0),
            above_ground: Some(320.0),
            target_above_ground: Some(0.0),
            sun: (230.0, 35.0),
            ..BASE
        },
        View {
            name: "earth_high",
            eye: DVec3::new(-10_000.0, 45_000.0, 90_000.0),
            target: DVec3::new(0.0, 0.0, -30_000.0),
            sun: (230.0, 28.0),
            fov: 65.0,
            exposure: Exposure::Manual { ev100: 14.5 },
            ..BASE
        },
        View {
            name: "earth_leo",
            eye: DVec3::new(0.0, 420_000.0, 150_000.0),
            target: DVec3::new(0.0, -300_000.0, -1_500_000.0),
            sun: (200.0, 35.0),
            fov: 60.0,
            exposure: Exposure::Manual { ev100: 14.5 },
            ..BASE
        },
        View {
            name: "earth_orbit",
            eye: DVec3::new(0.0, 2_600_000.0, 7_000_000.0),
            target: DVec3::new(0.0, -6_371_000.0, 0.0),
            sun: (120.0, 40.0),
            fov: 50.0,
            exposure: Exposure::Manual { ev100: 14.5 },
            ..BASE
        },
        View {
            name: "earth_globe",
            world: World3::Earth,
            eye_ll: Some([25.0, 10.0, 14_000_000.0]),
            sun_ll: Some([10.0, -15.0]),
            fov: 55.0,
            ..BASE
        },
        // Night: towns and cities light the dark side.
        View {
            name: "earth_night_globe",
            world: World3::Earth,
            eye_ll: Some([32.0, 12.0, 9_000_000.0]),
            sun_ll: Some([-15.0, 130.0]),
            fov: 55.0,
            exposure: Exposure::Manual { ev100: -1.0 },
            ..BASE
        },
        View {
            name: "earth_night_leo",
            world: World3::Earth,
            eye_ll: Some([36.0, 6.0, 420_000.0]),
            target_ll: Some([42.0, 12.0, 0.0]),
            sun_ll: Some([-15.0, 160.0]),
            fov: 60.0,
            exposure: Exposure::Manual { ev100: -1.0 },
            ..BASE
        },
        View {
            name: "earth_night_high",
            world: World3::Earth,
            eye_ll: Some([39.0, 3.0, 9_000.0]),
            target_ll: Some([39.3, 3.5, 0.0]),
            sun_ll: Some([-15.0, 180.0]),
            fov: 60.0,
            exposure: Exposure::Manual { ev100: -1.5 },
            ..BASE
        },
        // City lights from the world's own lamps: a town of street lamps at night.
        View {
            name: "earth_town_plane",
            eye: DVec3::new(-2_600.0, 0.0, 2_200.0),
            target: DVec3::new(0.0, 0.0, -200.0),
            above_ground: Some(1_500.0),
            target_above_ground: Some(0.0),
            sun: (200.0, -25.0),
            exposure: Exposure::Manual { ev100: 0.0 },
            town: true,
            ..BASE
        },
        View {
            name: "earth_town_orbit",
            eye: DVec3::new(-150_000.0, 0.0, 150_000.0),
            above_ground: Some(400_000.0),
            target_above_ground: Some(0.0),
            sun: (200.0, -25.0),
            fov: 6.0,
            exposure: Exposure::Manual { ev100: 0.0 },
            town: true,
            ..BASE
        },
        View {
            name: "moon_surface",
            world: World3::Moon,
            eye: DVec3::new(3_500.0, 0.0, -2_500.0),
            target: DVec3::new(9_000.0, -200.0, -9_000.0),
            above_ground: Some(1.8),
            sun: (200.0, 12.0),
            exposure: Exposure::Manual { ev100: 15.0 },
            ..BASE
        },
        View {
            name: "moon_feet",
            world: World3::Moon,
            eye: DVec3::new(3_500.0, 0.0, -2_500.0),
            target: DVec3::new(3_503.0, 0.0, -2_504.0),
            above_ground: Some(1.7),
            target_above_ground: Some(0.0),
            sun: (120.0, 15.0),
            exposure: Exposure::Manual { ev100: 15.0 },
            ..BASE
        },
        View {
            name: "moon_low_orbit",
            world: World3::Moon,
            eye: DVec3::new(0.0, 60_000.0, 120_000.0),
            target: DVec3::new(0.0, -40_000.0, -150_000.0),
            sun: (260.0, 15.0),
            exposure: Exposure::Manual { ev100: 15.0 },
            fov: 70.0,
            ..BASE
        },
        View {
            name: "moon_orbit",
            world: World3::Moon,
            eye: DVec3::new(0.0, 900_000.0, 3_200_000.0),
            target: DVec3::new(0.0, -1_737_400.0, 0.0),
            sun: (100.0, 20.0),
            exposure: Exposure::Manual { ev100: 15.0 },
            fov: 50.0,
            ..BASE
        },
        View {
            name: "moon_full",
            world: World3::Moon,
            eye: DVec3::new(0.0, 900_000.0, 3_200_000.0),
            target: DVec3::new(0.0, -1_737_400.0, 0.0),
            sun: (190.0, 36.0),
            exposure: Exposure::Manual { ev100: 15.0 },
            fov: 50.0,
            ..BASE
        },
        // Planets of other stars, one of each kind, from the ground.
        View {
            name: "exo_lava",
            world: World3::Exo(WorldKind::Lava),
            eye_ll: Some([4.0, 20.0, 40.0]),
            target_ll: Some([4.6, 21.0, 0.0]),
            sun_ll: Some([10.0, 40.0]),
            exposure: Exposure::Manual { ev100: 14.0 },
            ..BASE
        },
        View {
            name: "exo_lava_night",
            world: World3::Exo(WorldKind::Lava),
            eye_ll: Some([4.0, 20.0, 400.0]),
            target_ll: Some([4.8, 21.2, 0.0]),
            sun_ll: Some([0.0, 200.0]),
            exposure: Exposure::Manual { ev100: 13.0 },
            ..BASE
        },
        View {
            name: "exo_airless",
            world: World3::Exo(WorldKind::Airless),
            eye_ll: Some([10.0, 30.0, 25.0]),
            target_ll: Some([10.3, 30.6, 0.0]),
            sun_ll: Some([0.0, 75.0]),
            exposure: Exposure::Manual { ev100: 14.0 },
            ..BASE
        },
        View {
            name: "exo_desert",
            world: World3::Exo(WorldKind::Desert),
            eye_ll: Some([12.0, 40.0, 30.0]),
            target_ll: Some([12.3, 40.6, 0.0]),
            sun_ll: Some([20.0, 80.0]),
            exposure: Exposure::Manual { ev100: 14.5 },
            ..BASE
        },
        View {
            name: "exo_terra",
            world: World3::Exo(WorldKind::Terra),
            eye_ll: Some([45.0, 10.0, 300.0]),
            target_ll: Some([45.3, 10.6, 0.0]),
            sun_ll: Some([10.0, 50.0]),
            ..BASE
        },
        View {
            name: "exo_ocean_red_dwarf",
            world: World3::Exo(WorldKind::Ocean),
            eye_ll: Some([8.0, 60.0, 200.0]),
            target_ll: Some([8.3, 60.6, 0.0]),
            sun_ll: Some([5.0, 100.0]),
            ..BASE
        },
        View {
            name: "exo_cold_desert",
            world: World3::Exo(WorldKind::ColdDesert),
            eye_ll: Some([55.0, 0.0, 60.0]),
            target_ll: Some([55.5, 1.0, 0.0]),
            sun_ll: Some([20.0, 40.0]),
            exposure: Exposure::Manual { ev100: 13.5 },
            ..BASE
        },
        View {
            name: "exo_ice",
            world: World3::Exo(WorldKind::Ice),
            eye_ll: Some([30.0, 50.0, 30.0]),
            target_ll: Some([30.3, 50.6, 0.0]),
            sun_ll: Some([10.0, 90.0]),
            exposure: Exposure::Manual { ev100: 14.0 },
            ..BASE
        },
        View {
            name: "exo_hazy",
            world: World3::Exo(WorldKind::Hazy),
            eye_ll: Some([10.0, 80.0, 200.0]),
            target_ll: Some([10.4, 80.8, 0.0]),
            sun_ll: Some([20.0, 120.0]),
            ..BASE
        },
        View {
            name: "mars_orbit",
            world: World3::Mars,
            eye: DVec3::new(0.0, 1_500_000.0, 6_000_000.0),
            target: DVec3::new(0.0, -3_389_500.0, 0.0),
            sun: (110.0, 30.0),
            exposure: Exposure::Manual { ev100: 14.5 },
            fov: 50.0,
            ..BASE
        },
        View {
            name: "mars_valles",
            world: World3::Mars,
            eye_ll: Some([-8.0, 282.0, 6_200_000.0]),
            sun_ll: Some([-5.0, 235.0]),
            fov: 45.0,
            exposure: Exposure::Manual { ev100: 14.5 },
            ..BASE
        },
        View {
            name: "mars_olympus",
            world: World3::Mars,
            eye_ll: Some([2.0, 224.0, 600_000.0]),
            target_ll: Some([19.0, 226.2, 0.0]),
            sun_ll: Some([15.0, 158.0]),
            fov: 50.0,
            exposure: Exposure::Manual { ev100: 14.5 },
            ..BASE
        },
        View {
            name: "mars_canyon",
            world: World3::Mars,
            eye_ll: Some([-13.15, 300.0, 400.0]),
            target_ll: Some([-13.45, 303.5, 1_500.0]),
            sun_ll: Some([5.0, 345.0]),
            fov: 70.0,
            exposure: Exposure::Manual { ev100: 14.6 },
            ..BASE
        },
        View {
            name: "mars_rim",
            world: World3::Mars,
            eye_ll: Some([-13.6, 297.0, 2_000.0]),
            target_ll: Some([-12.4, 297.6, 0.0]),
            sun_ll: Some([10.0, 330.0]),
            fov: 70.0,
            exposure: Exposure::Manual { ev100: 14.6 },
            ..BASE
        },
        // On the plains of Gale crater, where Curiosity drives, at a rover's mast height.
        View {
            name: "mars_ground",
            world: World3::Mars,
            eye_ll: Some([-4.6, 137.4, 2.0]),
            target_ll: Some([-4.57, 137.43, 0.0]),
            sun_ll: Some([-10.0, 165.0]),
            fov: 65.0,
            exposure: Exposure::Manual { ev100: 14.6 },
            ..BASE
        },
        View {
            name: "mars_pole",
            world: World3::Mars,
            eye_ll: Some([62.0, 20.0, 1_200_000.0]),
            target_ll: Some([85.0, 10.0, 0.0]),
            sun_ll: Some([25.0, 30.0]),
            fov: 50.0,
            exposure: Exposure::Manual { ev100: 14.5 },
            ..BASE
        },
    ]
}

/// A point `above` meters over the ground at a latitude and longitude of the planet.
fn on_ground(tf: &Transform, planet: &PlanetTerrain, [lat, lon, above]: [f64; 3]) -> DVec3 {
    let dir = engine_terrain::shape::dir_from_lat_lon(lat, lon);
    let mut ground = planet.shape.height(dir);
    if planet.shape.ocean {
        ground = ground.max(0.0);
    }
    tf.transform_point(dir * (planet.shape.radius_m + ground + above))
}

fn painted_height(x: f64, z: f64) -> f64 {
    (x / 80.0).sin() * 1.2 + (z / 120.0).cos() * 0.8 - 0.8
}

/// A small hand-made layout, like a game level paints its valley.
fn painted_ground(x: f64, z: f64) -> Option<OverlayPaint> {
    let fade = |d: f64, inner: f64, outer: f64| {
        let t = ((outer - d) / (outer - inner)).clamp(0.0, 1.0);
        (t * t * (3.0 - 2.0 * t)) as f32
    };
    // A winding dirt road.
    let road = (z - 12.0 * (x / 45.0).sin()).abs();
    if road < 3.6 {
        return Some(OverlayPaint::layer("dirt", fade(road, 2.2, 3.6)));
    }
    // A stone plaza with a gravel ring around it.
    let plaza = x.hypot(z + 40.0);
    if plaza < 16.0 {
        return Some(OverlayPaint::layer("concrete", fade(plaza, 14.5, 16.0)));
    }
    if plaza < 20.0 {
        return Some(OverlayPaint::layer(
            "gravel",
            fade((plaza - 18.0).abs(), 1.4, 2.0),
        ));
    }
    // Fields: wheat, lavender and plowed strips.
    if (35.0..170.0).contains(&x) && (-150.0..-25.0).contains(&z) {
        let strip = ((x - 35.0) / 22.0).floor() as i32 % 3;
        return Some(match strip {
            0 => OverlayPaint::tint([0.36, 0.27, 0.09], 0.85),
            1 => OverlayPaint::tint([0.17, 0.11, 0.24], 0.8),
            _ => OverlayPaint::layer("dirt", 1.0).tinted([0.11, 0.075, 0.045], 0.6),
        });
    }
    // A patchwork of grounds at full weight meeting edge to edge, the way a level paints its
    // valley cover by cover: their borders blend instead of stepping along triangles.
    if (-170.0..170.0).contains(&x) && (-330.0..-170.0).contains(&z) {
        let (wx, wz) = (x + 9.0 * (z / 23.0).sin(), z + 9.0 * (x / 31.0).sin());
        let cell = ((wx / 40.0).floor() as i64 * 7 + (wz / 40.0).floor() as i64 * 3).rem_euclid(5);
        let layer = ["grass", "sand", "dirt", "gravel", "leaves"][cell as usize];
        return Some(OverlayPaint::layer(layer, 1.0));
    }
    // A meadow painted the way the showcase paints its valley: grass at full weight, tinted
    // toward a lush-to-dry green that drifts over tens of meters.
    if (170.0..420.0).contains(&x) && (-330.0..40.0).contains(&z) {
        let dry =
            ((x / 55.0).sin() * (z / 47.0).cos() * 0.5 + (x / 9.0 + z / 13.0).sin() * 0.2 + 0.45)
                .clamp(0.0, 1.0) as f32;
        let (lush, parched) = ([0.062, 0.15, 0.03], [0.16, 0.17, 0.055]);
        let tint = std::array::from_fn(|i| lush[i] + (parched[i] - lush[i]) * dry);
        return Some(OverlayPaint::layer("grass", 1.0).tinted(tint, 0.75));
    }
    // Woods: leaf litter.
    if (-170.0..-30.0).contains(&x) && (-150.0..-25.0).contains(&z) {
        return Some(OverlayPaint::layer("leaves", 1.0));
    }
    // A sandy shore.
    if z > 45.0 {
        return Some(OverlayPaint::layer("sand", fade(z, 52.0, 45.0)));
    }
    None
}

/// Street lamps of a town about 2.5 km across around home: bright main roads every 400 m,
/// sodium-lit side streets every 80 m, a lamp every 40 m, thinning out toward a ragged edge.
/// With `NO_CITY=1` the terrain doesn't see them (a before picture): each is registered
/// below the lamps' threshold, with its color scaled so the renderer lights the same.
fn spawn_town(app: &mut App, place: &dyn Fn(DVec3, Option<f64>) -> DVec3) {
    let hidden = std::env::var_os("NO_CITY").is_some();
    let mut lamps = 0;
    for street in -15i32..=15 {
        let main = street % 5 == 0;
        for step in -30i32..=30 {
            for along_x in [true, false] {
                let (u, v) = (street as f64 * 80.0, step as f64 * 40.0);
                let (x, z) = if along_x { (v, u) } else { (u, v) };
                // A ragged edge: the town thins out from 900 to 1300 m.
                let r = x.hypot(z) * (1.0 + 0.15 * (x / 210.0).sin() * (z / 170.0).cos());
                let keep = ((1300.0 - r) / 400.0).clamp(0.0, 1.0);
                let hash = ((x * 12.9898 + z * 78.233).sin() * 43758.5453)
                    .fract()
                    .abs();
                if hash > keep {
                    continue;
                }
                let (lumens, color) = if main {
                    (20_000.0, [1.0, 0.95, 0.85])
                } else {
                    (9_000.0, [1.0, 0.72, 0.42])
                };
                let at = place(DVec3::new(x, 0.0, z), Some(8.0));
                let light = if hidden {
                    let k = lumens / 150.0;
                    PointLight {
                        color: [color[0] * k, color[1] * k, color[2] * k],
                        intensity_lm: 150.0,
                        range: 35.0,
                        radius: 0.2,
                    }
                } else {
                    PointLight {
                        color,
                        intensity_lm: lumens,
                        range: 35.0,
                        radius: 0.2,
                    }
                };
                app.world.spawn((light, Transform::from_translation(at)));
                lamps += 1;
            }
        }
    }
    eprintln!("town: {lamps} lamps");
}

/// Footprints of someone who walked up the painted dirt road (its middle follows
/// `z = 12 sin(x / 45)`) and of someone on the grass beside it: a step every 0.7 m.
fn walk_steps(app: &mut App, place: &dyn Fn(DVec3, Option<f64>) -> DVec3) {
    for side in [0.0, 4.5] {
        let mut left = false;
        let mut x: f64 = -2.5;
        while x < 9.0 {
            let z = 12.0 * (x / 45.0).sin() + side;
            let ahead = DVec3::new(1.0, 0.0, (12.0 / 45.0) * (x / 45.0).cos()).normalize();
            let right = DVec3::new(-ahead.z, 0.0, ahead.x);
            let foot = DVec3::new(x, 0.0, z) + right * if left { -0.12 } else { 0.12 };
            app.world.write_message(engine_world::GroundContact {
                position: place(foot, Some(0.0)),
                normal: Vec3::Y,
                size: [0.11, 0.28],
                yaw: engine_world::GroundContact::yaw_of(ahead.as_vec3()),
                load_kg: 75.0,
                speed: 1.4,
                kind: engine_world::ContactKind::Foot,
                surface: None,
                entity: None,
            });
            left = !left;
            x += 0.7;
        }
    }
}

fn sun_dir(azimuth_deg: f64, elevation_deg: f64) -> DVec3 {
    let (az, el) = (azimuth_deg.to_radians(), elevation_deg.to_radians());
    // North is −Z, east is +X.
    DVec3::new(el.cos() * az.sin(), el.sin(), -el.cos() * az.cos())
}

fn build(view: &View, size: (u32, u32)) -> App {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: 0.25 });
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = size.0;
        w.height = size.1;
    }
    app.add_plugins(engine_world::WorldPlugin)
        .add_plugins(RenderPlugin)
        .add_plugins(TerrainPlugin);

    let pad = FlatPad::default();
    let planet = match view.world {
        World3::Earth => PlanetTerrain::earthlike(7),
        World3::Moon => PlanetTerrain::moon(3),
        World3::Mars => PlanetTerrain::mars(5),
        World3::Exo(kind) => PlanetTerrain::world(&exo_recipe(kind)),
    }
    .with_pad(pad);
    // `NO_LIGHTS=1`: the night side without towns and cities, for a before picture.
    let mut planet = planet;
    if std::env::var_os("NO_LIGHTS").is_some() || view.town {
        planet.palette.night_lights = 0.0;
    }
    // `NO_ROCKS=1`: bare ground without stones and boulders, for a before picture.
    if std::env::var_os("NO_ROCKS").is_some() {
        planet.rocks.per_100m2 = 0.0;
    }
    let planet = if view.painted {
        planet
            .with_overlay(400.0, 300.0, painted_height)
            .with_overlay_paint(painted_ground)
    } else {
        planet
    };
    let tf = planet.home_transform(pad);
    let sun = match view.sun_ll {
        Some([lat, lon]) => {
            tf.rotation.as_dquat() * engine_terrain::shape::dir_from_lat_lon(lat, lon)
        }
        None => sun_dir(view.sun.0, view.sun.1),
    };
    {
        let mut atmo = app.world.resource_mut::<Atmosphere>();
        match view.world {
            World3::Earth => {
                atmo.planet_center = tf.translation;
                atmo.planet_radius = planet.shape.radius_m;
            }
            World3::Mars => {
                *atmo = Atmosphere {
                    planet_center: tf.translation,
                    planet_radius: planet.shape.radius_m,
                    ..Atmosphere::mars()
                };
            }
            World3::Moon => atmo.enabled = false,
            // Roughly the skies the space module gives them (it isn't loaded here).
            World3::Exo(kind) => {
                let base = match kind {
                    WorldKind::Terra | WorldKind::Ocean => Some(Atmosphere::earth()),
                    WorldKind::Desert | WorldKind::ColdDesert => Some(Atmosphere {
                        density_scale: 3.0,
                        ..Atmosphere::mars()
                    }),
                    WorldKind::Hazy => Some(Atmosphere {
                        density_scale: 6.0,
                        ..Atmosphere::mars()
                    }),
                    _ => None,
                };
                match base {
                    Some(a) => {
                        *atmo = Atmosphere {
                            planet_center: tf.translation,
                            planet_radius: planet.shape.radius_m,
                            ..a
                        }
                    }
                    None => atmo.enabled = false,
                }
            }
        }
    }
    app.world.spawn((tf, planet));
    app.world
        .spawn((Transform::default(), SimulationFocus::default()));
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(-sun, Vec3::Y),
    ));
    if view.lab {
        let mut commands = app.world.commands();
        demo::spawn_demo_with(
            &mut commands,
            DVec3::ZERO,
            demo::LabOptions {
                stand_in_bodies: true,
            },
        );
        app.world.flush();
    }
    if !view.lab {
        // Clear air for landscapes, so the pictures show the terrain rather than the haze.
        let mut w = app.world.resource_mut::<WeatherState>();
        w.fog = 0.0;
        w.cloud_cover = 0.15;
        // Clear-day visibility (~100 km) instead of the default ground haze. None where there
        // is no atmosphere: without one the haze measures height as world y, so it would pile
        // up on the far side of the globe.
        app.world.resource_mut::<FogSettings>().density = match view.world {
            // (Night views from space without it: the haze takes its sunlight where the camera
            // is, so from a sunlit orbit it glows over the night side too.)
            World3::Earth if view.name.starts_with("earth_night") => 0.0,
            World3::Earth => 0.00002,
            World3::Moon | World3::Mars | World3::Exo(_) => 0.0,
        };
    }
    match view.ground {
        Ground::Dry => {}
        Ground::Rain => {
            let mut w = app.world.resource_mut::<WeatherState>();
            w.precipitation_mm_h = 6.0;
            w.precipitation_kind = PrecipitationKind::Rain;
            w.cloud_cover = 0.6;
            let mut layers = app.world.resource_mut::<SurfaceLayers>();
            let (width, height) = (layers.grid.width, layers.grid.height);
            for y in 0..height {
                for x in 0..width {
                    let i = (y * width + x) as usize;
                    let c = &mut layers.grid.data[i];
                    c.wetness = 0.95;
                    // A few shallow puddles.
                    let (fx, fy) = (x as f32 * 0.37, y as f32 * 0.29);
                    let n = (fx.sin() * fy.cos() + (fx * 0.5 + 1.3).cos() * (fy * 0.7).sin()) * 0.5;
                    c.puddle_m = (n - 0.55).max(0.0) * 0.08;
                }
            }
        }
        Ground::Snow => {
            let mut w = app.world.resource_mut::<WeatherState>();
            w.temperature_c = -6.0;
            for c in &mut app.world.resource_mut::<SurfaceLayers>().grid.data {
                c.snow_m = 0.18;
            }
        }
    }
    app
}

fn main() {
    engine_core::log::init("warn");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = std::path::PathBuf::from(
        args.first()
            .cloned()
            .unwrap_or_else(|| "terrain_shots".into()),
    );
    let size = args
        .get(1)
        .and_then(|s| s.split_once('x'))
        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
        .unwrap_or((1280, 720));
    let only: Vec<&str> = args.iter().skip(2).map(String::as_str).collect();
    std::fs::create_dir_all(&out).expect("output folder");

    for view in views() {
        if !only.is_empty() && !only.contains(&view.name) {
            continue;
        }
        let started = std::time::Instant::now();
        let mut app = build(&view, size);
        app.update();
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU: {}", d.0);
            return;
        }
        let ground = app.world.resource::<engine_world::Ground>().clone();
        let place = |mut p: DVec3, above: Option<f64>| {
            if let Some(h) = above {
                // On a planet the ground is found along the local vertical, so ask again from
                // just above it: from high up the answer lies a little to the side.
                let mut y = 50_000.0;
                for _ in 0..3 {
                    y = ground.height(DVec3::new(p.x, y + 10.0, p.z)).unwrap_or(0.0);
                }
                p.y = y + h;
            }
            p
        };
        let (mut from, mut toward) = (view.eye, view.target);
        if let Some(back) = view.shore {
            let mut q = app.world.query::<(&Transform, &PlanetTerrain)>();
            let (tf, planet) = q.iter(&app.world).next().expect("planet");
            let d = DVec3::new(toward.x - from.x, 0.0, toward.z - from.z);
            let (len, d) = (d.length(), d.normalize());
            let inv = tf.rotation.as_dquat().inverse();
            let sea_at = |s: f64| {
                let p = from + d * s;
                let dir = (inv * (DVec3::new(p.x, 0.0, p.z) - tf.translation)).normalize();
                planet.shape.height(dir) < 0.0
            };
            if let Some(s) = (0..20_000).map(|i| i as f64 * 5.0).find(|&s| sea_at(s)) {
                let shore = from + d * s;
                from = shore - d * back;
                toward = shore + d * (len - back).max(50.0);
            }
        }
        let mut eye = place(from, view.above_ground);
        let mut target = place(toward, view.target_above_ground);
        let mut up = Vec3::Y;
        if let Some(mut e) = view.eye_ll {
            let mut q = app.world.query::<(&Transform, &PlanetTerrain)>();
            let (tf, planet) = q.iter(&app.world).next().expect("planet");
            let mut t_ll = view.target_ll;
            // A made-up world with seas: move the view to the nearest land.
            if matches!(view.world, World3::Exo(_)) && planet.shape.ocean {
                let land = |lat: f64, lon: f64| {
                    planet
                        .shape
                        .height(engine_terrain::shape::dir_from_lat_lon(lat, lon))
                        > 40.0
                };
                let shift = (0..40)
                    .flat_map(|r| {
                        (0..(8 * r).max(1)).map(move |k| {
                            let a = k as f64 / (8 * r).max(1) as f64 * std::f64::consts::TAU;
                            (r as f64 * 0.5 * a.cos(), r as f64 * 0.5 * a.sin())
                        })
                    })
                    .find(|&(dl, dn)| land(e[0] + dl, e[1] + dn));
                if let Some((dl, dn)) = shift {
                    e[0] += dl;
                    e[1] += dn;
                    t_ll = t_ll.map(|t| [t[0] + dl, t[1] + dn, t[2]]);
                }
            }
            eye = on_ground(tf, planet, e);
            target = t_ll.map_or(tf.translation, |t| on_ground(tf, planet, t));
            // Level with the ground below, or north up from orbit.
            up = match t_ll {
                Some(_) => (eye - tf.translation).normalize().as_vec3(),
                None => (tf.rotation.as_dquat() * DVec3::Y).as_vec3(),
            };
        }
        if view.town {
            spawn_town(&mut app, &place);
        }
        if view.steps {
            walk_steps(&mut app, &place);
        }
        if (!view.paths.is_empty() || !view.places.is_empty())
            && std::env::var_os("NO_PATHS").is_none()
        {
            let (from, toward) = view.paths_from.map_or((eye, target), |(e, t)| {
                (place(e, Some(1.7)), place(t, Some(0.0)))
            });
            wear_paths(&mut app, &view, from, toward, &place);
        }
        // Rocks and colliders gather around the focus: put it at the eye.
        let mut focus = app
            .world
            .query_filtered::<&mut Transform, With<SimulationFocus>>();
        if let Ok(mut f) = focus.single_mut(&mut app.world) {
            f.translation = eye;
        }
        app.world.spawn((
            Camera {
                exposure: view.exposure,
                ..Camera::perspective(view.fov)
            },
            Transform::from_translation(eye).looking_at(target, up),
        ));
        // Let the walkers lay their trails and the terrain finish streaming, with the ground
        // materials baked.
        let settings = app
            .world
            .get_resource::<engine_terrain::TerrainSettings>()
            .cloned()
            .unwrap_or_default();
        let (resolution, scans) = (
            settings.material_resolution,
            settings.scan_folder(&app.world),
        );
        while resolution > 0
            && engine_terrain::materials::shared(resolution, scans.as_deref()).is_none()
        {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        app.update_n(12);
        // The far shadow map's patches stream in a few a frame after the view's own, and
        // planets seen from afar ease into finer detail.
        let mut last_boost = 0.0;
        for _ in 0..300 {
            let d = app
                .world
                .resource::<engine_core::diagnostics::Diagnostics>();
            let built = d.get("terrain.patches_built").unwrap_or(0.0);
            let boost = d.get("terrain.detail_boost").unwrap_or(1.0);
            // (Patches build on worker threads: wait for those on their way, too.)
            let pending = d.get("terrain.patches_building").unwrap_or(0.0)
                + d.get("terrain.patches_missing").unwrap_or(0.0)
                + d.get("terrain.rock_builds").unwrap_or(0.0);
            if built == 0.0 && pending == 0.0 && (boost - last_boost).abs() < 0.005 {
                break;
            }
            last_boost = boost;
            // Let the workers finish what they have before drawing another frame.
            if d.get("terrain.patches_building").unwrap_or(0.0) > 0.0 {
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            app.update();
        }
        let path = out.join(format!("{}.png", view.name));
        match capture_now(&mut app.world, &path) {
            Ok(_) => {
                let d = app
                    .world
                    .resource::<engine_core::diagnostics::Diagnostics>();
                println!(
                    "{:<16} {:>6.1}s  patches drawn {:>5}  far shadow {:>5}  resident {:>5}  detail ×{:.2}  -> {}",
                    view.name,
                    started.elapsed().as_secs_f32(),
                    d.get("terrain.patches_drawn").unwrap_or(0.0),
                    d.get("terrain.far_shadow_patches").unwrap_or(0.0),
                    d.get("terrain.patches_resident").unwrap_or(0.0),
                    d.get("terrain.detail_boost").unwrap_or(1.0),
                    path.display()
                );
            }
            Err(e) => eprintln!("{}: {e}", view.name),
        }
        if std::env::var_os("FOREST_MASK").is_some() {
            let mask = out.join(format!("{}_forest.pgm", view.name));
            forest_mask(&app, &ground, (eye, target, up), view.fov, size, &mask);
        }
    }
}

/// Wears the view's paths into the ground (as if hundreds of people had walked them).
fn wear_paths(
    app: &mut App,
    view: &View,
    eye: DVec3,
    target: DVec3,
    place: &dyn Fn(DVec3, Option<f64>) -> DVec3,
) {
    let fwd = DVec3::new(target.x - eye.x, 0.0, target.z - eye.z).normalize();
    let right = fwd.cross(DVec3::Y);
    let world = &mut app.world;
    let mats = world.resource::<engine_world::SurfaceMaterials>().clone();
    let terrains = world.resource::<engine_terrain::Terrains>();
    let Some((entity, t)) = terrains.nearest(eye) else {
        return;
    };
    let mut lines = Vec::new();
    for (k, line) in view.paths.iter().enumerate() {
        let pts: Vec<DVec3> = line
            .points
            .iter()
            .map(|&(a, r)| place(eye + fwd * a + right * r, Some(0.0)))
            .collect();
        for (i, pair) in pts.windows(2).enumerate() {
            let kind = match view.ground {
                Ground::Snow => WornGround::Snow,
                _ => terrains
                    .surface_at(pair[0], &mats)
                    .map_or(WornGround::Turf, |(_, _, s, _)| WornGround::of(mats.get(s))),
            };
            let (a, b) = (t.to_local(pair[0]), t.to_local(pair[1]));
            let up = t.local_up((a + b) * 0.5);
            lines.push((
                a,
                b,
                up,
                line.passes,
                line.spread,
                kind,
                (k * 31 + i) as u32,
            ));
        }
    }
    let spots: Vec<DVec3> = view
        .places
        .iter()
        .map(|&(a, r, _)| place(eye + fwd * a + right * r, Some(0.0)))
        .collect();
    let places: Vec<(DVec3, f32)> = spots
        .iter()
        .zip(view.places)
        .map(|(&p, &(_, _, busy))| (t.to_local(p), busy))
        .collect();
    let place_kind = spots.first().map_or(WornGround::Turf, |&p| {
        terrains
            .surface_at(p, &mats)
            .map_or(WornGround::Turf, |(_, _, s, _)| WornGround::of(mats.get(s)))
    });
    let up = places.first().map_or(DVec3::Y, |p| t.local_up(p.0));
    let mut wear = world.resource_mut::<PathWear>();
    for (a, b, up, passes, spread, kind, seed) in lines {
        wear.walk_line(entity, a, b, up, passes, 75.0, kind, spread, seed);
    }
    if places.len() < 2 {
        return;
    }
    if std::env::var_os("STRAIGHT").is_some() {
        // Everyone on the straight line: each pair's share of the trips.
        let total: f32 = places.iter().map(|p| p.1).sum();
        for i in 0..places.len() {
            for j in i + 1..places.len() {
                let share = places[i].1 * places[j].1
                    / (total * total - places.iter().map(|p| p.1 * p.1).sum::<f32>())
                    * 2.0;
                let passes = (view.trips as f32 * share).round() as u32;
                wear.walk_line(
                    entity,
                    places[i].0,
                    places[j].0,
                    up,
                    passes,
                    75.0,
                    place_kind,
                    0.25,
                    (i * 7 + j) as u32,
                );
            }
        }
    } else {
        wear.walk_between(
            entity,
            &places,
            up,
            view.trips,
            75.0,
            place_kind,
            // A busy farm: hundreds of trips a day.
            engine_terrain::TrailWalk {
                trips_per_day: 800.0,
                ..Default::default()
            },
            5,
        );
    }
}

/// With `FOREST_MASK` set: what [`engine_world::ForestCover`] reports at the ground under every
/// fourth pixel of the view, as a grayscale image (canopy cover 0..255), to lay over the shot.
fn forest_mask(
    app: &App,
    ground: &engine_world::Ground,
    (eye, target, up): (DVec3, DVec3, Vec3),
    fov_y_deg: f32,
    (w, h): (u32, u32),
    path: &std::path::Path,
) {
    let woods = app.world.resource::<engine_world::ForestCover>().clone();
    let forward = (target - eye).normalize();
    let right = forward.cross(up.as_dvec3()).normalize();
    let cam_up = right.cross(forward);
    let tan = ((fov_y_deg as f64).to_radians() * 0.5).tan();
    let aspect = w as f64 / h as f64;
    let (mw, mh) = (w / 4, h / 4);
    let below = |p: DVec3| ground.height(p).is_some_and(|g| p.y < g);
    let mut px = Vec::with_capacity((mw * mh) as usize);
    for j in 0..mh {
        for i in 0..mw {
            let x = (2.0 * (i as f64 + 0.5) / mw as f64 - 1.0) * tan * aspect;
            let y = (1.0 - 2.0 * (j as f64 + 0.5) / mh as f64) * tan;
            let dir = (forward + right * x + cam_up * y).normalize();
            let (mut a, mut t) = (0.0, 1.0);
            let mut hit = None;
            while t < 40_000.0 {
                if below(eye + dir * t) {
                    let mut b = t;
                    for _ in 0..10 {
                        let m = 0.5 * (a + b);
                        if below(eye + dir * m) { b = m } else { a = m }
                    }
                    hit = Some(eye + dir * b);
                    break;
                }
                a = t;
                t += (t * 0.01).max(2.0);
            }
            let d = hit.and_then(|p| woods.sample(p)).map_or(0.0, |s| s.density);
            px.push((d * 255.0).round() as u8);
        }
    }
    let mut file = format!("P5\n{mw} {mh}\n255\n").into_bytes();
    file.extend(px);
    std::fs::write(path, file).expect("forest mask");
}
