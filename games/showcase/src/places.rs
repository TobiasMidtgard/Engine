//! Named viewpoints: camera bookmarks (number keys), the screenshot tour and the
//! `showcase.goto` API all use this one list.

use crate::landscape;
use crate::zones::{ZONES, demo_views, moon_position, zone};
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
    /// Simulated seconds the tour waits here before the screenshot (a fight getting going).
    pub warmup_secs: f32,
    /// The tour runs the warm-up without drawing ([`crate::zones::View::quick_warmup`]).
    pub quick_warmup: bool,
    /// API calls (name, JSON args) made on arriving, e.g. `("ui.show", r#"{"screen":"shop"}"#)`.
    pub on_arrive: Vec<(String, String)>,
    /// API calls made when the camera moves on to another place.
    pub on_leave: Vec<(String, String)>,
}

impl Place {
    fn warmup(mut self, secs: f32) -> Self {
        self.warmup_secs = secs;
        self
    }

    /// Warm up without drawing (only for stops whose warm-up runs on the CPU).
    fn quick(mut self) -> Self {
        self.quick_warmup = true;
        self
    }

    /// Make an API call on arriving.
    fn arriving(mut self, name: &str, args: String) -> Self {
        self.on_arrive.push((name.to_string(), args));
        self
    }
}

/// The footpath villagers have worn from the end of the east road across the grass to the
/// lake shore (x, z): past the village's houses, round the cloth yard.
pub const LAKE_TRAIL: [[f64; 2]; 3] = [[82.0, -33.0], [95.0, -90.0], [172.0, -122.0]];

/// Whether (x, z) is within `margin` meters of [`LAKE_TRAIL`]'s line (trees, rocks and grass
/// keep off it, so the worn ground shows).
pub fn near_lake_trail(x: f64, z: f64, margin: f64) -> bool {
    let p = DVec2::new(x, z);
    LAKE_TRAIL.windows(2).any(|leg| {
        let (a, b) = (DVec2::from_array(leg[0]), DVec2::from_array(leg[1]));
        let t = ((p - a).dot(b - a) / (b - a).length_squared()).clamp(0.0, 1.0);
        p.distance(a + (b - a) * t) < margin
    })
}

/// Make `place` the current one: run the previous place's `on_leave` calls, then this one's
/// `on_arrive` calls. A call that fails (say, its module isn't loaded) only logs a warning.
pub fn visit(world: &mut World, place: &Place) {
    let previous = world
        .get_resource_mut::<CurrentPlace>()
        .and_then(|mut c| c.0.take());
    if let Some(prev) = previous {
        run_calls(world, &prev.on_leave);
    }
    run_calls(world, &place.on_arrive);
    world.insert_resource(CurrentPlace(Some(place.clone())));
}

/// Frames between moving the camera and running a place's calls: the focus points follow the
/// camera a frame later and the fields that follow them (weather on the ground, smoke, the
/// terrain's prints) recenter the frame after that, so calls made at once would write around
/// the old spot and be dropped when the grids move.
pub const ARRIVE_DELAY: u32 = 2;

/// A [`visit`] waiting for the camera's move to reach the focus points.
#[derive(Resource, Debug)]
pub struct PendingVisit {
    place: Place,
    frames: u32,
}

/// [`visit`] `place` [`ARRIVE_DELAY`] frames from now (call it right after moving the camera
/// there).
pub fn visit_soon(world: &mut World, place: &Place) {
    world.insert_resource(PendingVisit {
        place: place.clone(),
        frames: ARRIVE_DELAY,
    });
}

/// Count down and run a pending [`visit_soon`].
pub fn run_pending(world: &mut World) {
    let Some(mut pending) = world.get_resource_mut::<PendingVisit>() else {
        return;
    };
    if pending.frames > 0 {
        pending.frames -= 1;
        return;
    }
    if let Some(pending) = world.remove_resource::<PendingVisit>() {
        visit(world, &pending.place);
    }
}

/// Run the current place's `on_leave` calls and forget it.
pub fn leave(world: &mut World) {
    let previous = world
        .get_resource_mut::<CurrentPlace>()
        .and_then(|mut c| c.0.take());
    if let Some(prev) = previous {
        run_calls(world, &prev.on_leave);
    }
}

/// The place the camera was last sent to with [`visit`].
#[derive(Resource, Debug, Default)]
pub struct CurrentPlace(pub Option<Place>);

fn run_calls(world: &mut World, calls: &[(String, String)]) {
    for (name, args) in calls {
        let args = serde_json::from_str(args).unwrap_or(serde_json::Value::Null);
        if let Err(e) = engine::core::registry::call_api(world, name, args) {
            engine::core::log::warn!("{name}: {e}");
        }
    }
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

/// Behind the walk's start of leg `i`, looking at the stop it heads for (the pilot's camera
/// takes over once the walk starts).
fn walk_view(i: usize) -> (DVec3, DVec3) {
    let (from, to) = crate::walk::leg(i);
    let back = (from - to).with_y(0.0).normalize_or(DVec3::Z);
    (from + back * 7.0 + DVec3::Y * 2.5, to)
}

/// Every viewpoint, in tour order.
pub fn places() -> Vec<Place> {
    let place = |key, name, (position, target): (DVec3, DVec3), hour| Place {
        key,
        name,
        position,
        target,
        hour,
        warmup_secs: 0.0,
        quick_warmup: false,
        on_arrive: Vec::new(),
        on_leave: Vec::new(),
    };
    let tower = zone("spaceport").center + DVec3::new(70.0, 0.0, 60.0);
    let moon = moon_position();
    let moon_dir = moon.normalize();
    let sun = engine_weather::Climate::default()
        .sun_direction(10.0, engine_weather::Sky::default().day_of_year)
        .as_dvec3();
    let moon_side = (sun * 0.7 - moon_dir * 0.7).normalize();
    let mut list = vec![
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
            "paddock",
            "Animation paddock",
            near("paddock", [0.0, 16.0, 32.0], [0.0, 0.0, 0.0]),
            15.0,
        ),
        place(
            "physics",
            "Physics playground",
            near("physics", [0.0, 24.0, 44.0], [0.0, 2.0, -3.0]),
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
            // From above its edge, looking down over the whole wood (Foliage's framing).
            near("forest", [112.0, 32.0, 92.0], [8.0, 0.0, 12.0]),
            9.0,
        ),
        // Water's own test area: a tarn on a hill spilling over falls into a pond. The stream
        // needs a few seconds to run and the pond to fill.
        place(
            "water",
            "Water test area: tarn, falls, stream and pond",
            near("water", [62.0, 18.0, 66.0], [0.0, 4.0, 0.0]),
            16.5,
        )
        // The water runs on the CPU (only the mist puffs at the falls' foot stand still).
        .warmup(8.0)
        .quick(),
        place(
            "meadow",
            "Wildlife meadow by the lake, in the morning",
            near("meadow", [60.0, 3.0, 10.0], [-120.0, 0.0, 60.0]),
            7.8,
        ),
        place(
            "lake",
            "Lake",
            near("lake", [-80.0, 6.0, 60.0], [10.0, 0.0, -10.0]),
            16.5,
        ),
        place("falls", "Waterfall into the lake", falls_view(), 15.0),
        place(
            "start_screen",
            "The exe's start screen over the valley",
            crate::walk::start_view(0.0),
            17.0,
        )
        .arriving("showcase.start_screen", "{}".into()),
        place(
            "walk",
            "Walk the valley: the pilot on the plaza, the market's marker ahead",
            walk_view(0),
            10.5,
        )
        .arriving("showcase.walk", r#"{"stop": 1}"#.into()),
        place(
            "walk_falls",
            "Walk the valley: on the lake shore, heading for the waterfall",
            walk_view(4),
            15.0,
        )
        .arriving("showcase.walk", r#"{"stop": 5}"#.into()),
        lake_trail(place(
            "lake_trail",
            "A footpath worn bare across the grass to the lake",
            {
                let [a, b, c] = LAKE_TRAIL.map(DVec2::from_array);
                let eye = a - (b - a).normalize() * 14.0;
                let mid = (b + (b + c) * 0.5) * 0.5;
                (
                    above_ground(DVec3::new(eye.x, 20.0, eye.y), 18.0),
                    DVec3::new(mid.x, 0.0, mid.y),
                )
            },
            10.0,
        )),
        place(
            "footsteps",
            "Footstep trail: grass, mud, snow, ice, gravel, boards, grate, underpass, sand",
            near("footsteps", [-32.0, 7.0, -19.0], [2.0, 0.0, 1.0]),
            14.0,
        ),
        place(
            "tracks",
            "Footprint lab: tracks in mud, snow and sand",
            near("tracks", [14.0, 6.0, -30.0], [0.0, 0.0, 0.0]),
            10.0,
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
        // The fight needs a few seconds to get going.
        place(
            "arena",
            "Combat arena",
            near("arena", [0.0, 11.0, -27.0], [0.0, 0.0, 3.0]),
            12.0,
        )
        // The fighters think and move on the CPU (physics raycasts, no renderer reads).
        .warmup(10.0)
        .quick(),
        place(
            "parkour",
            "Movement course",
            near("parkour", [32.0, 10.0, -32.0], [-5.0, 3.0, 0.0]),
            11.5,
        ),
        place(
            "fortress",
            "Siege fortress on its hill",
            near("fortress", [-34.0, 26.0, -62.0], [0.0, 3.0, 2.0]),
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
            // Across the plaza between its planter trees, to the lit fountain.
            (DVec3::new(-30.0, 8.0, 6.0), DVec3::new(8.0, 0.5, -2.0)),
            22.5,
        ),
    ];
    // Each zone demo's own viewpoints, right after the zone's stop (or at the end).
    for z in ZONES {
        let views = demo_views(z);
        if views.is_empty() {
            continue;
        }
        let at = list
            .iter()
            .rposition(|p| p.key == z.key || p.key.starts_with(&format!("{}_", z.key)))
            .map_or(list.len(), |i| i + 1);
        for (k, v) in views.into_iter().enumerate() {
            let position = above_ground(z.center + DVec3::from_array(v.eye), 0.3);
            list.insert(
                at + k,
                Place {
                    key: v.key,
                    name: v.name,
                    position,
                    target: z.center + DVec3::from_array(v.target),
                    hour: v.hour,
                    warmup_secs: v.warmup_secs,
                    quick_warmup: v.quick_warmup,
                    on_arrive: v.on_arrive,
                    on_leave: v.on_leave,
                },
            );
        }
    }
    list
}

/// Wear [`LAKE_TRAIL`] into the ground on arriving: years of villagers walking to the lake,
/// so the ground decides how it shows (bare earth through turf, a rut through mud).
fn lake_trail(place: Place) -> Place {
    LAKE_TRAIL.windows(2).fold(place, |place, leg| {
        let ([ax, az], [bx, bz]) = (leg[0], leg[1]);
        place.arriving(
            "terrain.walk_path",
            format!(
                r#"{{"from": [{ax}, 0, {az}], "to": [{bx}, 0, {bz}], "passes": 10000, "load_kg": 75, "spread_m": 0.4}}"#
            ),
        )
    })
}

/// Below the waterfall, looking up at it across the plunge pool.
fn falls_view() -> (DVec3, DVec3) {
    let land = landscape::shared();
    let Some(st) = land.streams.first() else {
        return near("lake", [0.0, 20.0, 0.0], [0.0, 0.0, 0.0]);
    };
    let Some((top, bottom)) = st.falls() else {
        return near("lake", [0.0, 20.0, 0.0], [0.0, 0.0, 0.0]);
    };
    let lip = st.path[top];
    let foot = st.path[bottom];
    let downstream = (foot - lip).normalize_or(DVec2::Y);
    let side = DVec2::new(-downstream.y, downstream.x);
    let eye = foot + downstream * 26.0 + side * 9.0;
    let ground = land.height(eye.x, eye.y).max(crate::landscape::LAKE_LEVEL);
    (
        DVec3::new(eye.x, ground + 3.0, eye.y),
        DVec3::new(lip.x, st.bed[top] - 1.5, lip.y),
    )
}

pub fn find(key: &str) -> Option<Place> {
    places().into_iter().find(|p| p.key == key)
}
