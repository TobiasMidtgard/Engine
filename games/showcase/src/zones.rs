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

use crate::landscape::{CaveHill, Cover, Lake, Landscape, Pad, Road};
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
        radius: 24.0,
        module: "ui",
    },
    Zone {
        key: "stage",
        name: "Animation stage",
        center: DVec3::new(-75.0, 0.0, 70.0),
        radius: 26.0,
        module: "anim",
    },
    Zone {
        key: "physics",
        name: "Physics playground",
        center: DVec3::new(-125.0, 0.0, -35.0),
        radius: 35.0,
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
        center: DVec3::new(-400.0, 0.0, 10.0),
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
        key: "footsteps",
        name: "Footstep trail (mud, snow, sand, gravel, puddles)",
        center: DVec3::new(0.0, 0.0, 150.0),
        radius: 40.0,
        module: "audio, terrain",
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
        module: "npc, movement",
    },
    Zone {
        key: "parkour",
        name: "Movement course (wall-run, slide, dash)",
        center: DVec3::new(-125.0, 0.0, 330.0),
        radius: 40.0,
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
    "stage",
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

/// The runway's two ends (x, z): 1.2 km north–south through the airfield.
pub const RUNWAY: ([f64; 2], [f64; 2]) = ([780.0, -750.0], [780.0, 450.0]);

/// What each zone shows. Module threads swap their placeholder for their demo here.
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
            "stage" => ph::stage(commands, lib, z),
            "physics" => ph::physics(commands, lib, z),
            "yard" => ph::yard(commands, lib, z),
            "magic" => ph::magic(commands, lib, z),
            "gallery2d" => ph::gallery2d(commands, lib, z),
            "forest" => ph::forest(commands, lib, land, z, rng),
            "meadow" => ph::meadow(commands, lib, z, rng),
            "lake" => ph::lake(commands, lib, z),
            "footsteps" => ph::footsteps(commands, lib, z),
            "cave" => ph::cave(commands, meshes, lib, land, z, rng),
            "arena" => ph::arena(commands, lib, z),
            "parkour" => ph::parkour(commands, lib, z),
            "fortress" => ph::fortress(commands, lib, z),
            "voxel" => ph::voxel(commands, meshes, z, rng),
            "field" => ph::field(commands, lib, z, rng),
            "range" => ph::range(commands, lib, z),
            "weather" => ph::weather(commands, lib, z),
            "track" => ph::track(commands, lib, z),
            "airfield" => ph::airfield(commands, lib, z),
            "spaceport" => ph::spaceport(commands, lib, z),
            _ => {}
        }
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
        // North: plaza, footstep trail, cave.
        road(&[
            [0.0, 38.0],
            [0.0, 80.0],
            [0.0, 150.0],
            [0.0, 220.0],
            [0.0, 262.0],
        ]),
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
        // North-east: market, field, spaceport.
        road(&[[28.0, 27.0], [55.0, 52.0]]),
        road(&[[94.0, 88.0], [128.0, 132.0]]),
        road(&[[218.0, 212.0], [300.0, 248.0], [352.0, 268.0]]),
        // North-west: animation stage, arena, movement course.
        road(&[[-28.0, 27.0], [-55.0, 52.0]]),
        road(&[[-94.0, 88.0], [-115.0, 155.0]]),
        road(&[[-125.0, 245.0], [-125.0, 290.0]]),
        // West: physics playground, past the forest to the meadow.
        road(&[[-38.0, -12.0], [-90.0, -30.0]]),
        road(&[[-160.0, -30.0], [-230.0, -30.0], [-332.0, 8.0]]),
        // Arena to the fortress, fortress to the voxel hill.
        road(&[[-170.0, 205.0], [-240.0, 196.0]]),
        road(&[[-335.0, 238.0], [-392.0, 288.0]]),
        // Spaceport to the airfield.
        road(&[[585.0, 240.0], [690.0, 140.0], [736.0, 40.0]]),
        // Field to the weather station hill and to the smoke range.
        road(&[[190.0, 242.0], [205.0, 300.0], [213.0, 345.0]]),
        road(&[[218.0, 160.0], [258.0, 100.0], [272.0, 82.0]]),
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
        patches: footstep_patches(),
        surface_ids: surface_ids(),
        fields: Vec::new(),
    };
    land.plant_fields();
    land
}

/// Footstep surface ids for every [`Cover`] (from the default surface material table).
fn surface_ids() -> [engine::world::SurfaceId; crate::landscape::COVER_KINDS] {
    use Cover::*;
    let table = SurfaceMaterials::default();
    let mut ids = [engine::world::SurfaceId::default(); crate::landscape::COVER_KINDS];
    for c in [
        Grass, Forest, Dirt, Cobble, Sand, Rock, Snow, LakeBed, Mud, Gravel, Wood, Metal, Puddle,
    ] {
        ids[c as usize] = table.id(c.surface_name()).unwrap_or_default();
    }
    ids
}

/// The footstep trail: strips of different ground across the road north of the plaza, in walking
/// order from the plaza (the wooden and metal strips are boards laid on top, see placeholders).
pub const FOOTSTEP_STRIPS: &[(Cover, f64)] = &[
    (Cover::Grass, 8.0),
    (Cover::Mud, 9.0),
    (Cover::Puddle, 6.0),
    (Cover::Sand, 8.0),
    (Cover::Gravel, 8.0),
    (Cover::Snow, 10.0),
    (Cover::Wood, 7.0),
    (Cover::Metal, 6.0),
    (Cover::Dirt, 8.0),
];

/// Half width of the footstep trail (m).
pub const FOOTSTEP_HALF_WIDTH: f64 = 7.0;

/// Rectangles for [`FOOTSTEP_STRIPS`], starting at the south edge of the zone.
pub fn footstep_patches() -> Vec<(DVec2, DVec2, Cover)> {
    let z = zone("footsteps");
    let mut start = z.center.z - FOOTSTEP_STRIPS.iter().map(|s| s.1).sum::<f64>() * 0.5;
    let mut out = Vec::new();
    for (cover, len) in FOOTSTEP_STRIPS {
        out.push((
            DVec2::new(z.center.x - FOOTSTEP_HALF_WIDTH, start),
            DVec2::new(z.center.x + FOOTSTEP_HALF_WIDTH, start + len),
            *cover,
        ));
        start += len;
    }
    out
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
