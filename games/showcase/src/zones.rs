//! The showcase map: one zone per engine feature, laid out in a valley around a village plaza.
//!
//! Every zone sits on a flat pad of the landscape at `center` (its `y` is the pad height), so a
//! module's demo can build around `center` without caring about hills.
//!
//! **Module threads:** to show your module, edit only your zone's entry in [`ZONES`] and your
//! line in [`spawn_demos`], replacing the placeholder with
//! `engine_<module>::demo::spawn_demo(commands, z.center)`. Everything else in the showcase
//! (landscape, scenery, camera, tour) belongs to the Beautiful showcase world thread.
//! `TEST_CASES.md` lists every test case, which zone shows it and who owns it.

use crate::landscape::{CaveHill, Cover, Lake, Landscape, Pad, Road, Scarp, Stream};
use crate::placeholders as ph;
use crate::scenery::Library;
use engine::prelude::*;

/// A themed area of the map.
#[derive(Debug, Clone, Copy)]
pub struct Zone {
    pub key: &'static str,
    pub name: &'static str,
    /// Middle of the zone; `y` is the height of its flat pad.
    pub center: DVec3,
    /// Radius of the flat pad, meters.
    pub radius: f64,
    /// Which module's demo goes here.
    pub module: &'static str,
}

pub const ZONES: &[Zone] = &[
    Zone {
        key: "plaza",
        name: "Material plaza",
        center: DVec3::new(0.0, 0.0, 0.0),
        radius: 40.0,
        module: "render",
    },
    Zone {
        key: "market",
        name: "Market street (shop, dialogue, inventory, maps)",
        center: DVec3::new(75.0, 0.0, 70.0),
        radius: 30.0,
        module: "ui",
    },
    Zone {
        key: "paddock",
        name: "Animation paddock",
        center: DVec3::new(-75.0, 0.0, 70.0),
        radius: 40.0,
        module: "anim",
    },
    Zone {
        key: "physics",
        name: "Physics playground",
        center: DVec3::new(-125.0, 0.0, -35.0),
        radius: 45.0,
        module: "physics",
    },
    Zone {
        key: "yard",
        name: "Cloth and soft-body yard",
        center: DVec3::new(125.0, 0.0, -35.0),
        radius: 35.0,
        module: "softbody",
    },
    Zone {
        key: "magic",
        name: "Magic circle",
        center: DVec3::new(0.0, 0.0, -150.0),
        radius: 30.0,
        module: "vfx",
    },
    Zone {
        key: "gallery2d",
        name: "2D and 2.5D gallery",
        center: DVec3::new(-65.0, 0.0, -115.0),
        radius: 20.0,
        module: "render",
    },
    Zone {
        key: "forest",
        name: "Living forest",
        center: DVec3::new(-260.0, 0.0, -170.0),
        radius: 100.0,
        module: "foliage",
    },
    Zone {
        key: "meadow",
        name: "Wildlife meadow",
        // The lake keeps its shore 0.9 m above the water out this far.
        center: DVec3::new(375.0, 0.9, -250.0),
        radius: 70.0,
        module: "npc, anim",
    },
    Zone {
        key: "lake",
        name: "Lake",
        center: DVec3::new(240.0, 0.0, -175.0),
        radius: 85.0,
        module: "water",
    },
    Zone {
        key: "water",
        name: "Water test area (tarn, falls, stream, pond, boats)",
        center: DVec3::new(-330.0, 0.0, 30.0),
        radius: 95.0,
        module: "water",
    },
    Zone {
        key: "footsteps",
        name: "Footstep trail (mud, snow, sand, gravel, puddles)",
        center: DVec3::new(0.0, 0.0, 150.0),
        radius: 40.0,
        module: "audio",
    },
    Zone {
        key: "tracks",
        name: "Footprint lab (tracks in mud, snow and sand)",
        center: DVec3::new(70.0, 0.0, 205.0),
        radius: 30.0,
        module: "terrain",
    },
    Zone {
        key: "cave",
        name: "Mushroom cave",
        center: DVec3::new(0.0, 0.0, 290.0),
        radius: 35.0,
        module: "foliage",
    },
    Zone {
        key: "arena",
        name: "Combat arena",
        center: DVec3::new(-125.0, 0.0, 200.0),
        radius: 45.0,
        module: "npc",
    },
    Zone {
        key: "parkour",
        name: "Movement course (wall-run, slide, dash)",
        center: DVec3::new(-125.0, 0.0, 330.0),
        radius: 45.0,
        module: "movement",
    },
    Zone {
        key: "fortress",
        name: "Siege fortress",
        center: DVec3::new(-300.0, 10.0, 190.0),
        radius: 60.0,
        module: "destruction",
    },
    Zone {
        key: "voxel",
        name: "Voxel hill (dig and build)",
        center: DVec3::new(-420.0, 0.0, 320.0),
        radius: 45.0,
        module: "destruction, terrain",
    },
    Zone {
        key: "field",
        name: "Grass field (fire spread)",
        center: DVec3::new(165.0, 0.0, 185.0),
        radius: 60.0,
        module: "fire",
    },
    Zone {
        key: "range",
        name: "Smoke range (bullets, fans, explosions)",
        center: DVec3::new(285.0, 0.0, 55.0),
        radius: 30.0,
        module: "smoke",
    },
    Zone {
        key: "weather",
        name: "Weather station hill",
        center: DVec3::new(215.0, 28.0, 370.0),
        radius: 25.0,
        module: "weather",
    },
    Zone {
        key: "track",
        name: "Vehicle track",
        center: DVec3::new(0.0, 0.0, -500.0),
        radius: 160.0,
        module: "vehicles",
    },
    Zone {
        key: "airfield",
        name: "Airfield",
        center: DVec3::new(780.0, 0.0, -150.0),
        radius: 90.0,
        module: "vehicles",
    },
    Zone {
        key: "spaceport",
        name: "Spaceport",
        center: DVec3::new(470.0, 0.0, 280.0),
        radius: 120.0,
        module: "space, vehicles",
    },
];

pub fn zone(key: &str) -> &'static Zone {
    ZONES.iter().find(|z| z.key == key).expect("zone")
}

/// Zones with natural ground, where the valley's grass and flowers grow too.
pub const GRASSY_ZONES: &[&str] = &[
    "market",
    "paddock",
    "yard",
    "magic",
    "gallery2d",
    "forest",
    "meadow",
    "parkour",
    "fortress",
    "voxel",
    "weather",
];

/// The stream's course (x, z), spring first: through the southern woods, over the cliff south
/// of the lake (the waterfall) and into the lake.
pub const STREAM: [[f64; 2]; 7] = [
    [262.0, -440.0],
    [250.0, -400.0],
    [229.0, -360.0],
    [238.0, -322.0],
    [234.0, -290.0],
    [232.0, -268.0],
    [236.0, -235.0],
];

/// The runway's two ends (x, z): 1.2 km north–south through the airfield.
pub const RUNWAY: ([f64; 2], [f64; 2]) = ([780.0, -750.0], [780.0, 450.0]);

/// What each zone shows. Each zone is one line, owned by its module's thread: swap the
/// placeholder (`ph::…`) for your demo and leave the other lines alone.
pub fn spawn_demos(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    lib: &Library,
    land: &Landscape,
    rng: &mut Rng,
) {
    for z in ZONES {
        match z.key {
            "plaza" => ph::plaza(commands, lib, z),
            "market" => ph::market(commands, lib, z),
            "paddock" => ph::stage(commands, lib, z),
            "physics" => ph::physics(commands, lib, z),
            "yard" => engine_softbody::demo::spawn_demo(commands, z.center),
            "magic" => ph::magic(commands, lib, z),
            "gallery2d" => ph::gallery2d(commands, lib, z),
            "forest" => ph::forest(commands, lib, land, z, rng),
            "meadow" => engine_npc::demo::spawn_wildlife(commands, z.center, z.radius * 0.8),
            "lake" => ph::lake(commands, meshes, lib, land, z),
            "water" => engine_water::demo::spawn_demo(commands, z.center),
            "footsteps" => engine_audio::demo::spawn_demo(commands, z.center),
            "tracks" => ph::tracks(commands, z),
            "cave" => ph::cave(commands, z, rng),
            "arena" => engine_npc::demo::spawn_demo(commands, z.center),
            "parkour" => ph::parkour(commands, lib, z),
            "fortress" => engine_destruction::demo::spawn_demo(commands, z.center),
            "voxel" => ph::voxel(commands, meshes, z, rng),
            "field" => {
                engine_fire::demo::spawn_demo(commands, z.center);
                ph::field_edge(commands, lib, z, rng);
            }
            "range" => ph::range(commands, lib, z),
            "weather" => engine_weather::demo::spawn_demo(commands, z.center),
            "track" => engine_vehicles::demo::spawn_track(commands, z.center),
            "airfield" => {
                let (a, b) = RUNWAY;
                engine_vehicles::demo::spawn_airfield(
                    commands,
                    DVec3::new(a[0], z.center.y, a[1]),
                    DVec3::new(b[0], z.center.y, b[1]),
                );
                ph::airfield_buildings(commands, lib, z);
            }
            "spaceport" => {
                ph::spaceport(commands, lib, z);
                engine_vehicles::demo::spawn_spaceport(
                    commands,
                    z.center + DVec3::new(0.0, 0.4, 0.0),
                    30.0,
                );
            }
            _ => {}
        }
    }
}

/// A tour stop a zone's demo brings: eye and target relative to the zone's center.
#[derive(Debug, Clone)]
pub struct View {
    pub key: &'static str,
    pub name: &'static str,
    pub eye: [f64; 3],
    pub target: [f64; 3],
    pub hour: f32,
    /// Simulated seconds to wait before the screenshot.
    pub warmup_secs: f32,
    /// API calls (name, JSON args) made on arriving and on leaving.
    pub on_arrive: Vec<(String, String)>,
    pub on_leave: Vec<(String, String)>,
}

#[allow(
    dead_code,
    reason = "builders for the module threads' demo_views lines"
)]
impl View {
    pub fn new(
        key: &'static str,
        name: &'static str,
        eye: [f64; 3],
        target: [f64; 3],
        hour: f32,
    ) -> Self {
        Self {
            key,
            name,
            eye,
            target,
            hour,
            warmup_secs: 0.0,
            on_arrive: Vec::new(),
            on_leave: Vec::new(),
        }
    }

    pub fn warmup(mut self, secs: f32) -> Self {
        self.warmup_secs = secs;
        self
    }

    pub fn on_arrive(mut self, api: &str, json_args: &str) -> Self {
        self.on_arrive.push((api.into(), json_args.into()));
        self
    }

    pub fn on_leave(mut self, api: &str, json_args: &str) -> Self {
        self.on_leave.push((api.into(), json_args.into()));
        self
    }
}

/// Extra tour stops for each zone's demo, placed right after the zone's own stop. Like
/// [`spawn_demos`], a module thread edits only its own zone's line.
pub fn demo_views(z: &Zone) -> Vec<View> {
    match z.key {
        "field" => engine_fire::demo::VIEWS
            .iter()
            .map(|v| View::new(v.key, v.name, v.eye, v.target, v.hour))
            .collect(),
        "yard" => vec![
            View::new(
                "yard_banners",
                "Banners in the wind",
                [0.0, 3.0, 2.0],
                [0.0, 7.5, -18.0],
                14.5,
            )
            .warmup(3.0),
            View::new(
                "yard_curtains",
                "Curtains: shot full of holes, and burning",
                [-12.0, 3.4, 15.0],
                [-12.0, 3.2, 6.0],
                14.5,
            )
            .warmup(13.0),
            View::new(
                "yard_jelly",
                "Jelly cubes and the wrecking ball",
                [1.5, 3.2, 7.5],
                [0.5, 1.8, -1.0],
                14.5,
            )
            .warmup(4.4),
            View::new(
                "yard_bridge",
                "A walker crossing the rope bridge",
                [8.0, 6.5, 19.0],
                [11.0, 2.5, 10.0],
                14.5,
            )
            .warmup(8.0),
            View::new(
                "yard_cut_bridge",
                "A rope bridge cut and falling",
                [-2.0, 5.0, -2.0],
                [-2.0, 2.8, -11.0],
                14.5,
            )
            .warmup(7.0),
        ],
        "arena" => vec![
            View::new(
                "arena_duel",
                "Duel: a master against a veteran (block, parry, feint)",
                [10.0, 2.4, -30.5],
                [12.0, 1.0, -23.0],
                15.0,
            )
            .warmup(10.0),
            View::new(
                "arena_cover",
                "Squads in cover, flanking and suppressing",
                [-15.0, 4.5, -4.0],
                [2.0, 0.8, 9.0],
                15.0,
            )
            .warmup(12.0),
        ],
        "meadow" => vec![
            View::new(
                "meadow_herd",
                "Deer herd grazing near the shore",
                [-14.0, 3.0, -10.0],
                [-5.0, 0.8, 5.0],
                15.0,
            )
            .warmup(6.0),
        ],
        "weather" => {
            let set = |preset: &str| format!(r#"{{"preset": "{preset}", "transition_s": 0}}"#);
            vec![
                View::new(
                    "weather_station",
                    "The weather station",
                    [9.0, 3.5, 13.0],
                    [0.0, 1.2, 0.0],
                    15.0,
                )
                .warmup(1.0),
                View::new(
                    "weather_rain",
                    "A downpour soaking the station yard",
                    [6.0, 1.8, 9.0],
                    [0.0, 0.4, -2.0],
                    13.0,
                )
                .warmup(3.0)
                .on_arrive("weather.set", &set("storm"))
                .on_arrive("weather.advance", r#"{"seconds": 900}"#)
                .on_arrive("weather.ground", r#"{"wetness": 1.0, "puddle_m": 0.03}"#)
                .on_leave("weather.set", &set("partly_cloudy"))
                .on_leave("weather.ground", r#"{"dry": true}"#),
                View::new(
                    "weather_snow",
                    "Fresh snow on the station",
                    [9.0, 3.0, 12.0],
                    [0.0, 0.8, 0.0],
                    12.0,
                )
                .warmup(3.0)
                .on_arrive("weather.set", &set("snow"))
                .on_arrive("weather.ground", r#"{"wetness": 0.3, "snow_m": 0.15}"#)
                .on_leave("weather.set", &set("partly_cloudy"))
                .on_leave("weather.ground", r#"{"dry": true}"#),
                // The wind comes from the west-southwest (+X, a little -Z): look upwind at the
                // front rolling in.
                View::new(
                    "weather_storm",
                    "A thunderstorm rolling in",
                    [-6.0, 3.0, 4.0],
                    [200.0, 60.0, -70.0],
                    15.0,
                )
                .warmup(2.0)
                .on_arrive(
                    "weather.set",
                    r#"{"preset": "storm", "transition_s": 900, "wind_from_deg": 250}"#,
                )
                .on_arrive("weather.advance", r#"{"seconds": 640}"#)
                .on_leave("weather.set", &set("partly_cloudy")),
            ]
        }
        _ => Vec::new(),
    }
}

/// The valley: pads for every zone and the runway, roads between them, the lake and the cave
/// hill.
pub fn landscape(seed: u64) -> Landscape {
    // The lake shapes its own basin (see `Lake`), so it gets no flat pad.
    let mut pads: Vec<Pad> = ZONES
        .iter()
        .filter(|z| z.key != "lake")
        .map(|z| {
            let mut pad = Pad::disc(z.center, z.radius);
            // Raised zones get a wide, gentle hill instead of a steep bank.
            if z.center.y > 1.0 {
                pad.falloff = 70.0;
            }
            pad
        })
        .collect();
    let (a, b) = RUNWAY;
    pads.push(Pad {
        a: DVec2::from_array(a),
        b: DVec2::from_array(b),
        radius: 45.0,
        falloff: 40.0,
        height: 0.0,
    });
    let road = |pts: &[[f64; 2]]| Road::new(pts, 7.0, false);
    let roads = vec![
        // North: plaza to the footstep trail, and on from it to the cave (the trail's loop is
        // the path through its zone).
        road(&[[0.0, 38.0], [0.0, 80.0], [0.0, 128.0]]),
        road(&[[0.0, 172.0], [0.0, 220.0], [0.0, 262.0]]),
        // South: plaza, magic circle, track.
        road(&[
            [0.0, -38.0],
            [0.0, -110.0],
            [0.0, -190.0],
            [-6.0, -300.0],
            [0.0, -400.0],
        ]),
        // East: yard, lake shore.
        road(&[[38.0, -12.0], [90.0, -30.0]]),
        road(&[[158.0, -48.0], [185.0, -78.0], [200.0, -95.0]]),
        // Footstep trail across to the footprint lab.
        road(&[[38.0, 160.0], [52.0, 185.0]]),
        // North-east: market, field, spaceport.
        road(&[[28.0, 27.0], [55.0, 52.0]]),
        road(&[[94.0, 88.0], [128.0, 132.0]]),
        road(&[[218.0, 212.0], [300.0, 248.0], [352.0, 268.0]]),
        // North-west: animation paddock, arena, movement course.
        road(&[[-28.0, 27.0], [-55.0, 52.0]]),
        road(&[[-94.0, 88.0], [-115.0, 155.0]]),
        road(&[[-125.0, 245.0], [-125.0, 290.0]]),
        // West: physics playground, on to the edge of the forest.
        road(&[[-38.0, -12.0], [-90.0, -30.0]]),
        road(&[[-170.0, -30.0], [-230.0, -40.0], [-285.0, -62.0]]),
        // Arena to the fortress, fortress to the voxel hill.
        road(&[[-170.0, 205.0], [-240.0, 196.0]]),
        road(&[[-335.0, 238.0], [-392.0, 288.0]]),
        // Spaceport to the airfield.
        road(&[[585.0, 240.0], [690.0, 140.0], [736.0, 40.0]]),
        // Field to the weather station hill and to the smoke range.
        road(&[[190.0, 242.0], [205.0, 300.0], [213.0, 345.0]]),
        road(&[[218.0, 160.0], [258.0, 100.0], [272.0, 82.0]]),
        // Smoke range down past the lake to the wildlife meadow.
        road(&[
            [285.0, 25.0],
            [330.0, -80.0],
            [368.0, -150.0],
            [374.0, -182.0],
        ]),
        // Magic circle to the 2D gallery.
        road(&[[-28.0, -142.0], [-48.0, -126.0]]),
    ];
    let lake = zone("lake");
    let cave = zone("cave");
    let mut land = Landscape {
        seed,
        pads,
        roads,
        lake: Lake {
            center: DVec2::new(lake.center.x, lake.center.z),
            radius: lake.radius * 1.05,
            depth: 8.0,
        },
        cave: CaveHill {
            center: DVec2::new(cave.center.x, cave.center.z + 30.0),
            // The hole reaches past the cave's inner walls by more than a terrain cell, so the
            // steep terrain triangles at its edge stay hidden inside the walls. Its front edge
            // is the front wall's outer face (19 m in front of the zone center).
            mouth: DVec2::new(cave.center.x, cave.center.z + 1.5),
            hole: DVec2::new(20.0, 20.5),
            hill_height: 24.0,
            hill_radius: 90.0,
        },
        woods: vec![
            (DVec2::new(-260.0, -170.0), 150.0),
            (DVec2::new(-520.0, -380.0), 260.0),
            (DVec2::new(330.0, -470.0), 200.0),
            (DVec2::new(-230.0, 470.0), 170.0),
            (DVec2::new(420.0, 620.0), 240.0),
            (DVec2::new(640.0, -620.0), 240.0),
            (DVec2::new(-700.0, 260.0), 220.0),
            (DVec2::new(80.0, 640.0), 200.0),
            (DVec2::new(-900.0, -100.0), 300.0),
            (DVec2::new(1050.0, 300.0), 300.0),
        ],
        patches: ground_patches(),
        painters: vec![footstep_trail],
        fields: Vec::new(),
        // A cliff south of the lake, with the land behind it raised 9 m.
        scarps: vec![Scarp {
            a: DVec2::new(300.0, -300.0),
            b: DVec2::new(150.0, -300.0),
            height: 9.0,
            reach: 400.0,
        }],
        // From a spring in the southern woods, over the cliff as a waterfall, into the lake.
        streams: vec![Stream::new(&STREAM, 5.0, 1.4)],
    };
    land.lay_streams();
    land.plant_fields();
    land
}

/// The footprint lab's ground: 10 m wide strips running 32 m north–south.
pub const TRACK_STRIPS: [Cover; 4] = [Cover::Mud, Cover::Snow, Cover::Sand, Cover::Grass];

/// Ground patches: the footprint lab's strips.
pub fn ground_patches() -> Vec<(DVec2, DVec2, Cover)> {
    let mut out = Vec::new();
    let t = zone("tracks").center;
    for (i, cover) in TRACK_STRIPS.iter().enumerate() {
        let x0 = t.x - 20.0 + i as f64 * 10.0;
        out.push((
            DVec2::new(x0, t.z - 16.0),
            DVec2::new(x0 + 10.0, t.z + 16.0),
            *cover,
        ));
    }
    out
}

/// The audio module's footstep trail, painted into the ground so the player, NPCs and rain find
/// the same surfaces as the trail's walkers.
fn footstep_trail(x: f64, z: f64) -> Option<Cover> {
    let c = zone("footsteps").center;
    let offset = DVec3::new(x - c.x, 0.0, z - c.z);
    // The trail is about 54 × 22 m; skip the lookup everywhere else.
    if offset.x.abs() > 28.0 || offset.z.abs() > 10.0 {
        return None;
    }
    Some(match engine_audio::demo::surface_under(offset)? {
        "mud" => Cover::Mud,
        "snow" => Cover::Snow,
        "ice" => Cover::Ice,
        "gravel" => Cover::Gravel,
        "wood" => Cover::Wood,
        "metal" => Cover::Metal,
        "concrete" => Cover::Concrete,
        "sand" => Cover::Sand,
        "leaves" => Cover::Forest,
        "water" => Cover::Puddle,
        _ => Cover::Grass,
    })
}

/// The Moon at its real size and distance: 1 737 km radius, 384 400 km away.
pub fn moon(commands: &mut Commands) {
    commands.spawn((
        Transform::from_translation(moon_position()),
        MeshRenderer::new(
            Shape::Sphere {
                radius: 1_737_400.0,
                segments: 96,
            },
            StandardMaterial::hex(0x9a9690).with_roughness(0.95),
        ),
    ));
}

/// The Moon's position (for camera bookmarks).
pub fn moon_position() -> DVec3 {
    DVec3::new(-0.45, 0.35, -0.82).normalize() * 384_400_000.0
}
