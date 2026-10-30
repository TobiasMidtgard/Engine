//! The showcase map: one zone per engine feature, laid out in a valley around a village plaza.
//!
//! Every zone sits on a flat pad of the landscape at `center` (its `y` is the pad height), so a
//! module's demo can build around `center` without caring about hills.
//!
//! **Module threads:** to show your module, edit only your zone's entry in [`ZONES`] and your
//! line in [`spawn_cell`], replacing the placeholder with
//! `engine_<module>::demo::spawn_demo(commands, z.center)`. Everything else in the showcase
//! (landscape, scenery, camera, tour) belongs to the Beautiful showcase world thread.
//!
//! In the exe each zone is a streamed cell ([`zone_cells`], `engine::stream`): it spawns when
//! the player comes within 100 m of it, so the start doesn't wait for zones far away, and its
//! own random sequence ([`cell_rng`]) keeps it the same whatever loaded first. A zone stays
//! loaded once in until its module can take it down again (an unload hook for what it keeps
//! outside its entities); then it joins [`UNLOADABLE`]. Tours, films and tools load every zone.
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
        name: "Market square",
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
        key: "script",
        name: "Script courtyard (Luau scripts, hot reload)",
        center: DVec3::new(70.0, 0.0, -105.0),
        radius: 22.0,
        module: "script",
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
        // The range is 45 × 52 m; flat out to its corners.
        radius: 36.0,
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

/// Zones whose demo stops short of the pad's edge, with the radius it fills (m). The valley's
/// woods grow on the rest of the pad, right up to the demo, so it sits inside a forest instead
/// of on a bare disc. The foliage demo's forest reaches about 90 m.
pub const WOODED_RIMS: &[(&str, f64)] = &[("forest", 94.0)];

/// Zones with natural ground, where the valley's grass and flowers grow too.
pub const GRASSY_ZONES: &[&str] = &[
    "market",
    "paddock",
    "yard",
    "gallery2d",
    "forest",
    "meadow",
    "parkour",
    "fortress",
    "weather",
    "footsteps",
];

/// Rectangles inside grassy zones where a module's demo lays its own ground, so no grass grows
/// there: zone key, then x min..max and z min..max relative to the zone center, meters.
pub const BARE_FOOTPRINTS: &[(&str, [f64; 2], [f64; 2])] = &[
    // The weather station's asphalt yard, sand pit and stone patio.
    ("weather", [-13.5, 13.0], [-8.0, 9.0]),
    // The animation paddock's sand floor (35 m round, fenced at 35.5 m), where people's feet
    // must show; the meadow frames it outside the fence.
    ("paddock", [-34.0, 34.0], [-34.0, 34.0]),
    // The siege fortress's gravel breach range.
    ("fortress", [-33.5, 26.5], [-14.0, -4.0]),
    // The siege fortress's breach house and the stair up its back wall.
    ("fortress", [11.2, 18.8], [17.7, 25.4]),
    // The market's cobbled square (grass still grows where its edge frays out).
    ("market", [-11.0, 11.0], [-11.0, 11.0]),
    // The footstep trail and its own meadow, which thins out about 7 m from the path, where
    // the valley's grass patches (reaching 2 m past this) take over.
    ("footsteps", [-30.0, 30.0], [-12.0, 12.0]),
    // The movement course's concrete: the runway, the vault lane, the ledge station, the
    // grapple pad, the zip line pad, the freight platform with its track bed, and the ring.
    ("parkour", [-28.0, 28.0], [7.0, 17.0]),
    ("parkour", [-28.5, -2.5], [-0.5, 4.5]),
    ("parkour", [11.0, 23.0], [-1.5, 5.5]),
    ("parkour", [3.0, 13.0], [18.0, 40.0]),
    ("parkour", [28.5, 39.0], [15.5, 26.0]),
    ("parkour", [-29.9, 2.4], [23.5, 32.6]),
    ("parkour", [-8.0, 8.0], [-22.0, -6.0]),
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

/// The lake zone's water: the lake over its basin, the stream from its spring over the falls,
/// and a jetty on the west shore facing the waterfall across the lake.
fn lake_water<'a>(z: &Zone, ground: &'a dyn Fn(f64, f64) -> f64) -> engine_water::demo::Valley<'a> {
    engine_water::demo::Valley {
        lake_center: z.center,
        lake_radius: z.radius as f32 * 1.3,
        streams: &[engine_water::demo::ValleyStream {
            path: &STREAM,
            width_m: 5.0,
            // Enough to fill the brink across: at 1 m³/s the falls split into two thin strands.
            flow_m3_s: 2.0,
        }],
        ground,
        jetty_bearing: Some(std::f64::consts::PI * 0.9),
    }
}

/// A tour stop for one of the water module's views; those it marks as pure CPU simulation warm
/// up without drawing.
fn water_view(v: &engine_water::demo::DemoView) -> View {
    let view = View::new(v.key, v.name, v.eye, v.target, v.hour).warmup(v.warmup_secs);
    if v.quick_warmup {
        view.quick_warmup()
    } else {
        view
    }
}

/// Where Physics' float test floats: out on the deep water south-east of the lake's middle,
/// clear of the jetty and the dinghy's lane on the west side.
fn float_test(lake: &Zone) -> DVec3 {
    lake.center + DVec3::new(20.0, crate::landscape::LAKE_LEVEL, -20.0)
}

/// Water over the stream's bed at the silk rinse, m (as the water sim runs it).
const RINSE_DEPTH: f64 = 0.47;

/// The yard's silk rinse: across the stream in the southern woods, 30 m above the falls, where
/// it runs about half a meter deep at 1.3 m/s between steep banks 5.7 m apart. Returns the middle of the pole's
/// span at the water's height (as the bed suggests), the way the stream runs, and the width
/// from bank to bank at the water.
fn rinse_site(land: &Landscape) -> (DVec3, DVec2, f64) {
    let (a, b) = (DVec2::from(STREAM[2]), DVec2::from(STREAM[3]));
    let down = (b - a).normalize();
    let west = DVec2::new(down.y, -down.x);
    let mid = a + down * 28.0;
    let h = |k: f64| {
        let p = mid + west * k;
        land.height(p.x, p.y)
    };
    let bed = (-60..=60)
        .map(|i| i as f64 * 0.1)
        .min_by(|x, y| h(*x).total_cmp(&h(*y)))
        .unwrap_or(0.0);
    let water = h(bed) + RINSE_DEPTH;
    // Out from the deepest point each way to where the bank rises out of the water.
    let bank = |dir: f64| {
        let mut k = bed;
        while k.abs() < 8.0 && h(k) < water + 0.03 {
            k += dir * 0.05;
        }
        k
    };
    let (east, west_bank) = (bank(-1.0), bank(1.0));
    let center = mid + west * ((east + west_bank) * 0.5);
    (
        DVec3::new(center.x, water, center.y),
        down,
        west_bank - east,
    )
}

/// The runway's two ends (x, z): 1.2 km north–south through the airfield.
pub const RUNWAY: ([f64; 2], [f64; 2]) = ([780.0, -750.0], [780.0, 450.0]);

/// The zones whose demos are spawned, when not all of them (see [`demo_zones`]).
#[derive(Resource, Debug, Clone, PartialEq)]
pub struct DemoZones(pub Vec<String>);

/// Viewpoint names that belong to a zone of another name: the fire stops (`fire_front`) are the
/// field's, the smoke stops (`smoke_range`) the range's, and the sunset and night views look
/// across the plaza, so a tour of them shows its fountain and spheres (the village views load
/// it too). The overview and the tower look down on the whole valley, where only the lake's
/// water reads from that height (without it the lake is a bare basin), so they load just the
/// lake, as do the waterfall that feeds it and the start screen's view. The walk's stops load
/// where the pilot stands.
const ZONE_ALIASES: [(&str, &str); 16] = [
    ("fire", "field"),
    ("far_fortress", "fortress"),
    ("village", "plaza"),
    ("smoke", "range"),
    ("sunset", "plaza"),
    ("night", "plaza"),
    ("overview", "lake"),
    ("tower", "lake"),
    ("falls", "lake"),
    ("start_screen", "lake"),
    // (Before `walk`, whose prefix they share.)
    ("walk_yard", "market"),
    ("walk_path", "yard"),
    ("walk_shore", "lake"),
    ("walk_falls", "lake"),
    ("walk_meadow", "lake"),
    ("walk", "plaza"),
];

/// The zone a viewpoint belongs to: the zone with its key, or whose key and `_` start it
/// (`paddock_car` is the paddock's), or the zone a [`ZONE_ALIASES`] prefix names.
pub fn zone_of(place: &str) -> Option<&'static str> {
    let named = |key: &str| {
        place == key
            || place
                .strip_prefix(key)
                .is_some_and(|rest| rest.starts_with('_'))
    };
    ZONES
        .iter()
        .map(|z| z.key)
        .find(|key| named(key))
        .or_else(|| {
            ZONE_ALIASES
                .iter()
                .find(|(prefix, _)| named(prefix))
                .map(|(_, zone)| *zone)
        })
}

/// Which zones get their demos, from the `--zones` flag (`a,b`, `none` or `all`) or else from
/// the tour's or film's stops: when every stop belongs to a zone, only those zones load. Each
/// module's demo costs a few hundred MB to a GB, and all of them together nearly fill 16 GB
/// under a software renderer, so checking one module's stops shouldn't load the others.
/// `None` spawns every zone.
pub fn demo_zones(flag: Option<&str>, stops: Option<&str>) -> Option<DemoZones> {
    let list = |s: &str| -> Vec<String> {
        s.split(',')
            .map(str::trim)
            .filter(|k| !k.is_empty() && *k != "none")
            .map(String::from)
            .collect()
    };
    match flag {
        Some("all") => None,
        Some(keys) => Some(DemoZones(list(keys))),
        None => {
            let mut zones: Vec<String> = Vec::new();
            for stop in list(stops?) {
                let key = zone_of(&stop)?.to_string();
                if !zones.contains(&key) {
                    zones.push(key);
                }
            }
            (!zones.is_empty()).then_some(DemoZones(zones))
        }
    }
}

/// One streamed piece of the valley: a zone's demo, or a part of it that stands away from the
/// zone (the yard's dinghy on the lake, its silks in the stream). It loads as a whole when the
/// player comes near and is spawned by [`spawn_cell`].
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneCell {
    pub key: &'static str,
    /// Middle of what it spawns (world meters).
    pub center: DVec3,
    /// How far its content reaches from `center`, meters.
    pub radius: f64,
    /// Never unloaded once in: its module keeps state outside the entities it spawned (a single
    /// proving ground, the yard's hoist) and has no unload hook yet.
    pub keep_loaded: bool,
    /// A far-view proxy stands in for it while unloaded; not for cells whose content moves
    /// (the dinghy, the grazing herds) or is too small to see from afar (the silks).
    pub proxy: bool,
    /// Frees the meshes, materials and images its spawn made when it unloads
    /// (`StreamCell::release_assets`): only cells whose modules keep no handles to them past the
    /// unload (see [`RELEASES_ASSETS`]).
    pub release_assets: bool,
}

/// Zones whose content is only the entities they spawn, so they unload cleanly: the showcase's
/// own placeholders, and the modules' zones that `ENGINE_STREAM_AUDIT` found clean (out, in
/// and out again, 2026-10-28). The others join as their unload hooks land.
const UNLOADABLE: [&str; 12] = [
    "plaza",
    "gallery2d",
    "magic",
    "tracks",
    "parkour",
    "market",
    "arena",
    "voxel",
    "range",
    "footsteps",
    // Its unload hook frees what engine_water keeps; clean with its assets freed (2026-10-29).
    "water",
    // Its region's plants and the ground cover's litter go with it; clean with its assets
    // freed (2026-10-29).
    "cave",
];

/// Cells that free their assets on unload: the audit's reload found none of their meshes
/// missing. `SHOWCASE_RELEASE_ASSETS=all` opts every cell in, to audit which may join.
const RELEASES_ASSETS: [&str; 9] = [
    "tracks",
    "parkour",
    "gallery2d",
    "market",
    "arena",
    "voxel",
    "range",
    "water",
    "cave",
];

fn releases_assets(key: &str) -> bool {
    RELEASES_ASSETS.contains(&key)
        || std::env::var("SHOWCASE_RELEASE_ASSETS").is_ok_and(|v| v == "all")
}

/// A cell's module unload hook (`StreamCell::on_unload`), for what the module keeps outside
/// the entities the zone spawned.
pub fn unload_hook(key: &str) -> Option<fn(&mut World)> {
    match key {
        "water" => Some(engine_water::demo::unload_demo),
        _ => None,
    }
}

/// Cells with no far-view proxy ([`ZoneCell::proxy`]).
const NO_PROXY: [&str; 3] = ["yard_sailboat", "yard_rinse", "meadow"];

/// The streamed cells of the zones `only` names (every zone when `None`), each with its reach:
/// a module's demo spreads past its flat pad (fences, paths, runs), so half again the pad and
/// a margin, and the long ones as they are laid out.
pub fn zone_cells(land: &Landscape, only: Option<&DemoZones>) -> Vec<ZoneCell> {
    let mut cells = Vec::new();
    for z in ZONES {
        if only.is_some_and(|o| !o.0.iter().any(|k| k == z.key)) {
            continue;
        }
        let keep_loaded = !UNLOADABLE.contains(&z.key);
        let cell = |key: &'static str, center, radius| ZoneCell {
            key,
            center,
            radius,
            keep_loaded,
            proxy: !NO_PROXY.contains(&key),
            release_assets: releases_assets(key),
        };
        match z.key {
            "airfield" => {
                let (a, b) = RUNWAY;
                let (a, b) = (DVec2::from_array(a), DVec2::from_array(b));
                let mid = (a + b) * 0.5;
                cells.push(cell(
                    z.key,
                    DVec3::new(mid.x, z.center.y, mid.y),
                    a.distance(b) * 0.5 + z.radius,
                ));
            }
            "yard" => {
                cells.push(cell(z.key, z.center, z.radius * 1.5 + 10.0));
                cells.push(cell(
                    "yard_sailboat",
                    sailboat_home(),
                    SAILBOAT_REACH_M + 15.0,
                ));
                let (at, _, width) = rinse_site(land);
                cells.push(cell("yard_rinse", at, width + 15.0));
            }
            _ => cells.push(cell(z.key, z.center, z.radius * 1.5 + 10.0)),
        }
    }
    cells
}

/// Where the yard's sailing dinghy works: on the lake east of the jetty.
fn sailboat_home() -> DVec3 {
    zone("lake").center + DVec3::new(-30.0, crate::landscape::LAKE_LEVEL, 30.0)
}

/// How far the dinghy sails from its home, m.
const SAILBOAT_REACH_M: f64 = 22.0;

/// A cell's own random sequence, from the world's seed and the cell's key, so what a zone
/// draws doesn't depend on which zones loaded before it (FNV-1a of the key).
pub fn cell_rng(seed: u64, key: &str) -> Rng {
    Rng::new(seed ^ fnv(FNV_START, key.as_bytes()))
}

const FNV_START: u64 = 0xcbf2_9ce4_8422_2325;

/// FNV-1a over `bytes`, continuing from `h`.
fn fnv(h: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(h, |h, b| {
        (h ^ *b as u64).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Bump when the showcase changes what its zones spawn, so far-view proxies kept on disk from
/// an older build aren't shown for them.
const ZONE_CONTENT_REVISION: u64 = 2;

/// The version a cell's far-view proxy is kept on disk under (`engine::hlod`): the world's
/// seed, the cell, the commit a release was built from (CI sets `GITHUB_SHA`; a local build
/// says "local", so it keeps its proxies until a zone is visited again) and
/// [`ZONE_CONTENT_REVISION`].
pub fn proxy_version(seed: u64, key: &str) -> u64 {
    let build = option_env!("GITHUB_SHA").unwrap_or("local");
    let h = fnv(FNV_START, &seed.to_le_bytes());
    let h = fnv(h, key.as_bytes());
    let h = fnv(h, build.as_bytes());
    fnv(h, &ZONE_CONTENT_REVISION.to_le_bytes())
}

/// What each cell shows ([`zone_cells`]). Each zone is one line, owned by its module's thread:
/// swap the placeholder (`ph::…`) for your demo and leave the other lines alone.
pub fn spawn_cell(commands: &mut Commands, lib: &Library, land: &Landscape, seed: u64, key: &str) {
    let rng = &mut cell_rng(seed, key);
    // The yard's outlying parts belong to the yard.
    let zone_key = if key.starts_with("yard_") {
        "yard"
    } else {
        key
    };
    let Some(z) = ZONES.iter().find(|z| z.key == zone_key) else {
        return;
    };
    match key {
        "plaza" => crate::plaza::spawn(commands, lib, z),
        "market" => engine_ui::demo::spawn_demo(commands, z.center),
        "paddock" => engine_anim::demo::spawn_demo(commands, z.center),
        "physics" => engine_physics::demo::spawn_demo(commands, z.center),
        "yard" => {
            engine_softbody::demo::spawn_demo(commands, z.center);
            // Bare earth under the burning curtain's burner, so its flame lights the
            // curtain and not the meadow.
            commands.spawn((
                Name::new("Curtain burner (firebreak)"),
                Transform::from_translation(z.center + engine_softbody::demo::CURTAIN_BURNER),
                engine_fire::FuelPatch::firebreak(engine_fire::PatchShape::Disc { radius: 4.0 }),
            ));
        }
        "yard_sailboat" => {
            // The sailing dinghy works the lake east of the jetty, leaving a wake as wide
            // as its hull.
            let boat = engine_softbody::demo::spawn_sailboat(
                commands,
                sailboat_home(),
                SAILBOAT_REACH_M as f32,
            );
            commands
                .entity(boat)
                .insert(engine_water::WaterInteractor::new(0.8).with_wake(1.0));
        }
        "yard_rinse" => {
            // Dyed silks rinsed in the stream above the falls (they tie on once the
            // lake zone's stream is running), the pole's ends against the banks.
            let (at, down, width) = rinse_site(land);
            engine_softbody::demo::rinse::spawn_rinse(
                commands,
                at,
                Vec3::new(down.x as f32, 0.0, down.y as f32),
                width as f32 + 0.1,
            );
        }
        "magic" => engine_vfx::demo::spawn_demo(commands, z.center),
        "script" => engine_script::demo::spawn_demo(commands, z.center),
        "gallery2d" => ph::gallery2d(commands, lib, z),
        "forest" => engine_foliage::demo::spawn_demo(commands, z.center),
        "meadow" => engine_npc::demo::spawn_wildlife(commands, z.center, z.radius * 0.8),
        "lake" => {
            engine_water::demo::spawn_valley(commands, &lake_water(z, &|x, z| land.height(x, z)));
            engine_physics::demo::spawn_floaters(commands, float_test(z));
        }
        "water" => engine_water::demo::spawn_demo(commands, z.center),
        "footsteps" => engine_audio::demo::spawn_demo(commands, z.center),
        "tracks" => engine_terrain::demo::spawn_demo_with(
            commands,
            z.center,
            engine_terrain::demo::LabOptions {
                stand_in_bodies: true,
            },
        ),
        "cave" => engine_foliage::demo::spawn_cave_demo(commands, z.center),
        "arena" => engine_npc::demo::spawn_demo(commands, z.center),
        "parkour" => engine_movement::demo::spawn_demo(commands, z.center),
        "fortress" => {
            engine_destruction::demo::spawn_demo(commands, z.center);
            // The burning storehouse has straw on its floor (it goes up fast and grows back
            // for the next loop) and stands in a bare earth yard ringed by green grass, so
            // its fire stays a house fire: no grass fire runs off across the valley, and
            // embers on the wind die where they land (most come down well inside 60 m).
            let barn = z.center + DVec3::from_array(engine_destruction::barn::BARN);
            commands.spawn((
                Name::new("Storehouse straw floor"),
                Transform::from_translation(barn),
                engine_fire::FuelPatch::grass(engine_fire::PatchShape::Rect {
                    half_extents: [2.3, 1.8],
                })
                .with_load(0.6)
                .with_moisture(0.05)
                .with_regrow(90.0)
                .with_priority(20),
            ));
            commands.spawn((
                Name::new("Storehouse yard (firebreak)"),
                Transform::from_translation(barn),
                engine_fire::FuelPatch::firebreak(engine_fire::PatchShape::Disc { radius: 8.0 }),
            ));
            commands.spawn((
                Name::new("Storehouse green grass"),
                Transform::from_translation(barn),
                engine_fire::FuelPatch::grass(engine_fire::PatchShape::Disc { radius: 60.0 })
                    .with_moisture(1.0)
                    .with_priority(5),
            ));
        }
        "voxel" => engine_destruction::hill::spawn_voxel_hill(commands, z.center),
        "field" => {
            engine_fire::demo::spawn_demo(commands, z.center);
            ph::field_edge(commands, lib, z, rng);
            // Air over the meadow, so the fire's smoke rises as one simulated plume. It
            // reaches farther downwind (the valley's wind blows west and a little north) so
            // the plume drifts out of it well clear of the fire.
            commands.spawn((
                Name::new("Fire demo: smoke"),
                Transform::from_translation(z.center + DVec3::new(-14.0, 0.0, 4.0)),
                engine_smoke::SmokeVolume {
                    size: [100.0, 40.0, 60.0],
                    cell_m: 1.25,
                },
            ));
            // Finer air around the campfire (fire::demo puts it 22 m west, 20.5 m south of
            // the center), so its thin wisp isn't smeared into the meadow's coarse cells.
            commands.spawn((
                Name::new("Fire demo: campfire smoke"),
                Transform::from_translation(z.center + DVec3::new(-25.0, -1.0, -19.5)),
                engine_smoke::SmokeVolume {
                    size: [16.0, 13.0, 10.0],
                    cell_m: 0.3,
                },
            ));
        }
        "range" => engine_smoke::demo::spawn_demo(commands, z.center),
        "weather" => engine_weather::demo::spawn_demo(commands, z.center),
        "track" => engine_vehicles::demo::spawn_track(commands, z.center),
        "airfield" => {
            let (a, b) = RUNWAY;
            engine_vehicles::demo::spawn_airfield(
                commands,
                DVec3::new(a[0], z.center.y, a[1]),
                DVec3::new(b[0], z.center.y, b[1]),
            );
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
    /// Run the warm-up without drawing (see [`View::quick_warmup`]).
    pub quick_warmup: bool,
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
            quick_warmup: false,
            on_arrive: Vec::new(),
            on_leave: Vec::new(),
        }
    }

    pub fn warmup(mut self, secs: f32) -> Self {
        self.warmup_secs = secs;
        self
    }

    /// The tour parks the renderer through this stop's warm-up and draws only the frames that
    /// settle the picture: a software renderer spends seconds on every frame, the simulation
    /// milliseconds. Only for stops whose look comes from CPU simulations (fire, smoke, water,
    /// physics, animation): GPU-simulated effects (particles, GPU cloth) stand still while the
    /// renderer is parked.
    pub fn quick_warmup(mut self) -> Self {
        self.quick_warmup = true;
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
/// [`spawn_cell`], a module thread edits only its own zone's line.
pub fn demo_views(z: &Zone) -> Vec<View> {
    match z.key {
        "track" | "airfield" | "spaceport" => engine_vehicles::demo::views(z.key, z.center)
            .into_iter()
            .map(|v| {
                let view = View::new(v.key, v.name, v.eye, v.target, v.hour).warmup(v.warmup_secs);
                let view = v
                    .on_arrive
                    .iter()
                    .fold(view, |view, (api, args)| view.on_arrive(api, args));
                v.on_leave
                    .iter()
                    .fold(view, |view, (api, args)| view.on_leave(api, args))
            })
            .collect(),
        "field" => engine_fire::demo::VIEWS
            .iter()
            .map(|v| {
                // Each stop starts a fresh fire its own way along in clear air, so it catches
                // its moment whichever stop came before. Fire, smoke and embers are CPU
                // simulations, so the warm-up runs undrawn.
                View::new(v.key, v.name, v.eye, v.target, v.hour)
                    .on_arrive("smoke.clear", "{}")
                    .on_arrive("fire.demo", &format!(r#"{{"at_secs": {}}}"#, v.at_secs))
                    .warmup(v.warmup_secs)
                    .quick_warmup()
            })
            .collect(),
        "parkour" => {
            // The duel runs from its setup when the camera arrives (movement.duel), so each
            // stop catches its moment; leaving puts the duel back on its three-round cycle.
            let duel = |round: &str| format!(r#"{{"round": "{round}"}}"#);
            let cycle = r#"{"round": null}"#;
            vec![
                View::new(
                    "parkour_course",
                    "Movement course: slide bar, wall-run walls and the tower",
                    [-34.0, 7.0, 26.0],
                    [2.0, 2.5, 11.0],
                    16.0,
                )
                .warmup(2.0),
                View::new(
                    "parkour_blink",
                    "An elite cuts down an untrained player in a blink",
                    [-4.0, 1.9, -11.5],
                    [0.0, 1.15, -10.0],
                    16.0,
                )
                .warmup(0.7)
                .on_arrive("movement.duel", &duel("Blink"))
                .on_leave("movement.duel", cycle),
                View::new(
                    "parkour_focus",
                    "Trained: the same attack in slow motion, sidestepped",
                    [-2.0, 2.4, -5.0],
                    [0.3, 1.1, -13.0],
                    16.0,
                )
                .warmup(1.0)
                .on_arrive("movement.duel", &duel("Focus"))
                .on_leave("movement.duel", cycle),
                View::new(
                    "parkour_counter",
                    "The counter-strike while the elite recovers",
                    [-4.0, 2.3, -7.5],
                    [1.0, 1.0, -10.5],
                    16.0,
                )
                .warmup(1.55)
                .on_arrive("movement.duel", &duel("Focus"))
                .on_leave("movement.duel", cycle),
                View::new(
                    "parkour_pack",
                    "Grunts fall one by one in slow motion",
                    [8.0, 4.0, -4.0],
                    [0.0, 1.0, -15.0],
                    16.0,
                )
                .warmup(1.75)
                .on_arrive("movement.duel", &duel("Pack"))
                .on_leave("movement.duel", cycle),
                // The ledge station: the climber restarts its loop on arrival and is caught
                // going around the corner at the end of the block's top. The Blink round has
                // no slow motion, so the climber keeps its pace.
                View::new(
                    "parkour_ledge",
                    "Ledge hang: catch a ledge too high to climb, shimmy along it and around \
                     its corner, climb up",
                    [24.6, 3.3, 6.2],
                    [19.7, 2.6, 1.7],
                    16.0,
                )
                .warmup(4.4)
                .on_arrive("movement.duel", &duel("Blink"))
                .on_arrive("movement.climber", "{}")
                .on_leave("movement.duel", cycle),
                // The grapple station: the grappler restarts its loop on arrival and is caught
                // mid-reel, the cable taut from its fist to the anchor plate.
                View::new(
                    "parkour_grapple",
                    "Grapple hook: fire at the top of a 9 m tower, reel in, climb over",
                    [15.5, 2.4, 20.5],
                    [8.0, 6.0, 26.5],
                    16.0,
                )
                .warmup(1.9)
                .on_arrive("movement.duel", &duel("Blink"))
                .on_arrive("movement.grappler", "{}")
                .on_leave("movement.duel", cycle),
                // The zip line: the grappler restarts its loop on arrival and is caught past the
                // middle of the line, coming at the camera with the cable running back up to
                // the gantry on the tower.
                View::new(
                    "parkour_zip",
                    "Zip line: jump into the cable off the tower roof and ride it down to the gate",
                    [25.9, 4.4, 17.5],
                    [20.5, 7.3, 26.8],
                    16.0,
                )
                // Past the middle of the line about 6.6 s into the loop.
                .warmup(6.15)
                .on_arrive("movement.duel", &duel("Blink"))
                .on_arrive("movement.grappler", "{}")
                .on_leave("movement.duel", cycle),
                // The tower ladder: the climber restarts its loop on arrival and is caught
                // halfway up the caged ladder, hand over hand, the roof railing above it.
                View::new(
                    "parkour_ladder",
                    "Ladder: climb the tower's caged ladder hand over hand, slide back down",
                    [29.0, 2.1, 17.6],
                    [23.0, 3.4, 14.4],
                    16.0,
                )
                // Halfway up about 2.9 s into the loop.
                .warmup(2.45)
                .on_arrive("movement.duel", &duel("Blink"))
                .on_arrive("movement.ladder", "{}")
                .on_leave("movement.duel", cycle),
                // The vault lane: the vaulter restarts its run on arrival and is caught going
                // over the concrete barrier at a sprint, its left hand planted on the top.
                View::new(
                    "parkour_vault",
                    "Vault: sprint over a railing, a concrete barrier and strapped cases without slowing",
                    [-14.0, 1.3, 5.4],
                    [-16.2, 1.15, 2.0],
                    16.0,
                )
                // Over the barrier about 2.57 s into the run.
                .warmup(1.92)
                .on_arrive("movement.duel", &duel("Blink"))
                .on_arrive("movement.vaulter", "{}")
                .on_leave("movement.duel", cycle),
                // The freight station: the rider restarts its loop on arrival, with the car
                // about to pull out, and is caught leaping onto the car's side.
                View::new(
                    "parkour_freight",
                    "Ride a moving freight car: leap onto its side, wall-run along it, climb aboard",
                    [-20.0, 3.6, 21.0],
                    [-15.5, 3.0, 28.5],
                    16.0,
                )
                .warmup(1.65)
                .on_arrive("movement.duel", &duel("Blink"))
                .on_arrive("movement.rider", "{}")
                .on_leave("movement.duel", cycle),
                // Play mode (P): a pilot in front of the camera, locked onto the elite.
                View::new(
                    "parkour_play",
                    "Play mode: your pilot dash-strikes the locked-on elite (press P to play)",
                    [-8.0, 2.0, -6.0],
                    [-2.0, 1.0, -17.0],
                    16.0,
                )
                // The capture lands mid-strike (dash 0.2 s, strike from 0.3 s).
                .warmup(0.0)
                .on_arrive("movement.duel", &duel("Focus"))
                .on_arrive("movement.play", r#"{"on": true}"#)
                .on_arrive("movement.lock_on", r#"{"target": "Elite"}"#)
                .on_arrive("movement.attack", r#"{"entity": "Pilot", "kind": "dash"}"#)
                .on_leave("movement.play", r#"{"on": false}"#)
                .on_leave("movement.duel", cycle),
            ]
        }
        // Each stop restarts the range, so fans, grenades and bullets are at the same moment.
        "range" => engine_smoke::demo::VIEWS
            .iter()
            .map(|v| {
                View::new(v.key, v.name, v.eye, v.target, v.hour)
                    .on_arrive("smoke.demo", r#"{"restart":true}"#)
                    .warmup(v.warmup_secs)
            })
            .collect(),
        "physics" => engine_physics::demo::VIEWS
            .iter()
            .map(|v| {
                let mut view =
                    View::new(v.key, v.name, v.eye, v.target, v.hour).warmup(v.warmup_secs);
                // Long warm-ups (Jenga plays 22 s before its fall) run without drawing.
                if v.warmup_secs > 3.0 {
                    view = view.quick_warmup();
                }
                match v.station {
                    Some(s) => view.on_arrive(
                        "physics.proving_ground.replay",
                        &format!(r#"{{"station": "{}"}}"#, s.key()),
                    ),
                    None => view,
                }
            })
            .collect(),
        "market" => {
            let (eye, target) = engine_ui::demo::market::ui_view();
            let ui = |key, name, screen: &'static str| {
                View::new(key, name, eye.to_array(), target.to_array(), 15.0)
                    .warmup(1.5)
                    .on_arrive("ui.show", screen)
                    .on_leave("ui.hide_all", "{}")
            };
            vec![
                ui(
                    "market_hud",
                    "HUD: vitals, compass, minimap, markers",
                    r#"{"screen": "hud"}"#,
                ),
                ui(
                    "market_dialogue",
                    "A conversation with the trader",
                    r#"{"screen": "dialogue", "tree": "merchant_intro", "node": "offer"}"#,
                )
                .warmup(6.0),
                // Each screen opens with something picked, so its details panel is filled.
                ui(
                    "market_shop",
                    "The trader's shop",
                    r#"{"screen": "shop", "select": "staff"}"#,
                ),
                ui(
                    "market_inventory",
                    "Inventory and equipment",
                    r#"{"screen": "inventory", "select": "greatsword"}"#,
                ),
                ui(
                    "market_journal",
                    "Quest journal",
                    r#"{"screen": "journal", "select": "silent_relay"}"#,
                ),
                ui(
                    "market_skills",
                    "Skill tree",
                    r#"{"screen": "skills", "select": "c_cleave"}"#,
                ),
                ui(
                    "market_crafting",
                    "Crafting bench",
                    r#"{"screen": "crafting", "select": "r_elixir"}"#,
                ),
                ui(
                    "market_saves",
                    "Save and load",
                    r#"{"screen": "saves", "select": "save_3"}"#,
                ),
                ui("market_title", "Title screen", r#"{"screen": "title"}"#),
                ui(
                    "market_loading",
                    "Loading screen",
                    r#"{"screen": "loading", "planet": "veyra", "place": "Hollow Reach", "simulate_s": 9.0}"#,
                ),
                // Over the HUD, so the slow-motion gauge shows the world slowing.
                View::new(
                    "market_wheel",
                    "Quick-select wheel (slows time)",
                    eye.to_array(),
                    target.to_array(),
                    15.0,
                )
                .warmup(1.5)
                .on_arrive("ui.show", r#"{"screen": "hud", "instant": true}"#)
                .on_arrive("ui.show", r#"{"screen": "wheel"}"#)
                .on_leave("ui.hide_all", "{}"),
                ui(
                    "market_map_galaxy",
                    "Galaxy map",
                    r#"{"screen": "map", "zoom": 3.6, "select": {"type": "system", "id": "veyra_system"}}"#,
                ),
                ui(
                    "market_map_system",
                    "Star system map",
                    r#"{"screen": "map", "zoom": 2.6, "select": {"type": "planet", "id": "veyra"}}"#,
                ),
                ui(
                    "market_map_globe",
                    "Planet globe",
                    r#"{"screen": "map", "zoom": 1.4, "select": {"type": "poi", "id": "relay_tower"}}"#,
                ),
                ui(
                    "market_map_surface",
                    "Surface map",
                    r#"{"screen": "map", "zoom": 0.5, "select": {"type": "poi", "id": "hollow_reach"}}"#,
                ),
                ui("market_pause", "Pause menu", r#"{"screen": "pause"}"#),
                ui(
                    "market_settings",
                    "Settings",
                    r#"{"screen": "settings", "tab": "video"}"#,
                ),
            ]
        }
        "lake" => {
            let land = crate::landscape::shared();
            let mut views: Vec<View> =
                engine_water::demo::valley_views(&lake_water(z, &|x, z| land.height(x, z)))
                    .iter()
                    .map(water_view)
                    .collect();
            // The jetty in a gale: from above the bank, past the boats and out over the open
            // lake, where whitecaps and spray streaks run downwind.
            if let Some(jetty) = views.iter().find(|v| v.key == "lake_jetty") {
                let eye = DVec3::from(jetty.eye) + DVec3::Y * 1.5;
                let water_y = jetty.target[1] - 0.3;
                let out = DVec3::from(jetty.target) - eye;
                let far = eye + DVec3::new(out.x, 0.0, out.z) * 4.0;
                let gale = View::new(
                    "lake_gale",
                    "The lake in a gale: whitecaps, spray streaks and boats riding it out",
                    eye.to_array(),
                    [far.x, water_y, far.z],
                    15.0,
                )
                .warmup(6.0)
                .quick_warmup()
                .on_arrive("weather.set", r#"{"preset": "storm", "transition_s": 0}"#)
                // The wind takes a few minutes to rise to the storm's (the rain doesn't).
                .on_arrive("weather.advance", r#"{"seconds": 180}"#)
                .on_leave(
                    "weather.set",
                    r#"{"preset": "partly_cloudy", "transition_s": 0}"#,
                );
                views.push(gale);
            }
            // The float test from the south, 8 m off and 3 m up, a few seconds after a drop
            // (its cycle runs while the camera is near, so it drops as the camera arrives).
            let at = (float_test(z) - z.center).to_array();
            views.push(
                View::new(
                    "lake_floats",
                    "Float test: eight things settle on the lake as the real ones do",
                    [at[0] - 2.0, at[1] + 3.0, at[2] - 8.0],
                    [at[0], at[1] + 0.3, at[2]],
                    16.5,
                )
                .warmup(8.0),
            );
            views
        }
        "water" => engine_water::demo::VIEWS.iter().map(water_view).collect(),
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
            // The unveiling starts over on arrival: the crane lowers the silk over the statue,
            // leaves it on and has just lifted it clear of the head when the picture is taken.
            // The camera looks east past the statue's right side, toward the village, with
            // the curtains (and the burning one's smoke) out of the frame.
            View::new(
                "yard_statue",
                "Unveiling: silk lifted off a marble statue",
                [-9.6, 1.85, -6.3],
                [-14.0, 2.7, -4.0],
                15.0,
            )
            .on_arrive("softbody.unveil", "{}")
            .warmup(18.4)
            .quick_warmup(),
            // On the lake: the dinghy's mooring is 85 m east and 110 m south of the yard. It
            // starts its lane over on arrival, so it is out on the water in view.
            View::new(
                "yard_sailboat",
                "A gaff dinghy sailing on the wind in its sail",
                [70.0, 4.5, -95.0],
                [85.0, 1.8, -110.0],
                15.5,
            )
            .on_arrive("softbody.sailboat_home", "{}")
            .warmup(12.0),
            // From the stream's west bank, upstream of the silk rinse, with the afternoon sun
            // behind: down through the clear water onto the silks streaming away.
            // The stream is the lake zone's: render with `--tour yard_rinse --zones yard,lake`.
            {
                let land = crate::landscape::shared();
                let (at, down, width) = rinse_site(land);
                let west = DVec2::new(down.y, -down.x);
                let c = DVec2::new(at.x, at.z);
                let eye = c + west * (width * 0.5 + 1.0) - down * 1.2;
                let eye_y = (at.y + 2.6).max(land.height(eye.x, eye.y) + 1.7);
                let rel = |p: DVec2, y: f64| [p.x - z.center.x, y, p.y - z.center.z];
                View::new(
                    "yard_rinse",
                    "Dyed silks rinsed in a mountain stream",
                    rel(eye, eye_y),
                    rel(c + down * 3.4, at.y - 0.2),
                    15.0,
                )
                .warmup(6.0)
                .quick_warmup()
            },
        ],
        "fortress" => destruction_views(engine_destruction::demo::VIEWS),
        "voxel" => destruction_views(engine_destruction::hill::VIEWS),
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
        "forest" => engine_foliage::demo::forest_viewpoints(DVec3::ZERO)
            .iter()
            .map(|v| {
                View::new(
                    v.name,
                    v.title,
                    v.eye.to_array(),
                    v.target.to_array(),
                    v.hour,
                )
            })
            .collect(),
        "cave" => engine_foliage::demo::cave_viewpoints(DVec3::ZERO)
            .iter()
            .map(|v| {
                View::new(
                    v.name,
                    v.title,
                    v.eye.to_array(),
                    v.target.to_array(),
                    v.hour,
                )
            })
            .collect(),
        // Each stop restarts the paddock and waits for its moment (the tour settles 0.4 s
        // before its warm-up).
        "paddock" => engine_anim::demo::VIEWS
            .iter()
            .map(|v| {
                // The paddock's moments are CPU animation: warm up without drawing.
                View::new(v.key, v.name, v.eye, v.target, v.hour)
                    .warmup((v.at - 0.4).max(0.0))
                    .quick_warmup()
                    .on_arrive("anim.demo_restart", "{}")
            })
            .collect(),
        "magic" => engine_vfx::demo::VIEWS
            .iter()
            .map(|v| View::new(v.key, v.name, v.eye, v.target, v.hour).warmup(v.warmup))
            .collect(),
        "script" => engine_script::demo::VIEWS
            .iter()
            .map(|v| {
                let view = View::new(v.key, v.name, v.eye, v.target, v.hour).warmup(v.warmup_secs);
                let view = v
                    .on_arrive
                    .iter()
                    .fold(view, |view, (api, args)| view.on_arrive(api, args));
                v.on_leave
                    .iter()
                    .fold(view, |view, (api, args)| view.on_leave(api, args))
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The destruction demos' viewpoints: each restarts its demo on arrival so its moment plays,
/// warming up without drawing.
fn destruction_views(views: &[engine_destruction::DemoView]) -> Vec<View> {
    views
        .iter()
        .map(|v| {
            let mut view = View::new(v.key, v.name, v.eye, v.target, v.hour)
                .warmup(v.warmup_secs)
                .on_arrive(engine_destruction::RESTART_API, "{}");
            // Breaking runs on the CPU; only the storehouse fire needs drawing to burn.
            if v.key != "fortress_fire" {
                view = view.quick_warmup();
            }
            // Someone climbs the breach house ladders and up through the hatches once they open.
            if v.key == "fortress_hatches" {
                view.on_arrive("movement.ladder_guest", "{}")
            } else {
                view
            }
        })
        .collect()
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
        patches: Vec::new(),
        painters: vec![footstep_trail, footprint_lab],
        fields: Vec::new(),
        // A cliff south of the lake, with the land behind it raised 9 m.
        scarps: vec![Scarp {
            a: DVec2::new(300.0, -300.0),
            b: DVec2::new(150.0, -300.0),
            height: 9.0,
            reach: 400.0,
            notch: None,
        }],
        // From a spring in the southern woods, over the cliff as a waterfall, into the lake.
        streams: vec![Stream::new(&STREAM, 5.0, 1.4)],
    };
    land.lay_streams();
    land.plant_fields();
    land
}

/// The footprint lab's strips from −X to +X (the last is the valley's own grass).
pub const TRACK_STRIPS: [Option<Cover>; 4] =
    [Some(Cover::Mud), Some(Cover::Snow), Some(Cover::Sand), None];

/// The footprint lab's ground, the same shape the terrain module's lab paints: each strip a
/// row of seven 5.2 m circles 5 m apart along Z, 10 m between strips. Footsteps and the
/// ground paint then agree with the terrain's own strips.
fn footprint_lab(x: f64, z: f64) -> Option<Cover> {
    let c = zone("tracks").center;
    let (dx, dz) = (x - c.x, z - c.z);
    if dx.abs() > 21.0 || dz.abs() > 21.0 {
        return None;
    }
    TRACK_STRIPS.iter().enumerate().find_map(|(i, cover)| {
        let sx = dx + 15.0 - i as f64 * 10.0;
        let inside = (0..7).any(|k| {
            let sz = dz + 15.0 - k as f64 * 5.0;
            sx * sx + sz * sz < 5.2 * 5.2
        });
        cover.filter(|_| inside)
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_silk_rinse_streams_out_in_the_running_stream() {
        use engine::core::time::TimeSource;
        use engine::world::{Ground, SurfaceMaterials, Water};
        let land = crate::landscape::shared();
        let mut app = App::new();
        app.world.resource_mut::<EngineConfig>().headless = true;
        app.world
            .resource_mut::<Time>()
            .set_source(TimeSource::Manual {
                step_secs: 1.0 / 30.0,
            });
        app.add_plugins((
            engine::world::WorldPlugin,
            engine_water::WaterPlugin,
            engine_softbody::SoftBodyPlugin,
        ));
        let ground =
            crate::landscape::LandscapeGround::new(land.clone(), &mut SurfaceMaterials::default());
        app.insert_resource(Ground(std::sync::Arc::new(ground)));
        let (at, down, width) = rinse_site(land);
        assert!(
            (4.0..7.0).contains(&width),
            "the stream is {width:.2} m wide there"
        );
        app.add_systems(Startup, move |mut commands: Commands| {
            engine_water::demo::spawn_valley(
                &mut commands,
                &lake_water(zone("lake"), &|x, z| land.height(x, z)),
            );
            engine_softbody::demo::rinse::spawn_rinse(
                &mut commands,
                at,
                Vec3::new(down.x as f32, 0.0, down.y as f32),
                width as f32 + 0.1,
            );
        });
        app.update_n(30 * 10);

        // The stream runs deep and steady there, down this reach, as high as the bed suggests.
        let w = app.world.resource::<Water>().0.sample(at + DVec3::Y * 10.0);
        let w = w.expect("the stream at the rinse");
        let flow = DVec2::new(w.velocity.x as f64, w.velocity.z as f64);
        assert!(w.depth > 0.3, "{w:?}");
        assert!(flow.dot(down) > 0.4, "{w:?}");
        assert!(
            (w.surface_height - at.y).abs() < 0.15,
            "{w:?} vs {:.2}",
            at.y
        );
        // Five silks tied on, wet, streaming downstream from the pole.
        let mut q = app.world.query::<&engine_softbody::SoftBodySim>();
        let silks: Vec<_> = q.iter(&app.world).collect();
        assert_eq!(silks.len(), 5);
        for b in silks {
            let reach = (0..b.particle_count())
                .map(|i| {
                    let p = b.world_position(i);
                    assert!(p.is_finite());
                    let along = DVec2::new(p.x - at.x, p.z - at.z).dot(down);
                    // Past the knot, in the water: not riding up the banks.
                    assert!(
                        along < 1.0 || p.y < w.surface_height + 0.15,
                        "a silk rides {:.2} m above the water",
                        p.y - w.surface_height
                    );
                    along
                })
                .fold(f64::MIN, f64::max);
            let wet = b.wet.iter().sum::<f32>() / b.wet.len() as f32;
            assert!(reach > 5.0, "a silk reaches only {reach:.2} m downstream");
            assert!(wet > 0.5, "a silk is only {wet:.2} wet");
        }
    }

    #[test]
    fn the_float_test_has_open_deep_water() {
        let lake = zone("lake");
        let at = float_test(lake);
        let land = crate::landscape::shared();
        // 14 × 8 m at least 2 m deep, with a meter to spare.
        for i in -8..=8 {
            for k in -5..=5 {
                let (x, z) = (at.x + i as f64, at.z + k as f64);
                assert!(
                    land.lake_depth(x, z) > 2.0,
                    "{:.1} m deep at ({x}, {z})",
                    land.lake_depth(x, z)
                );
            }
        }
        // Clear of the dinghy's mooring and its 22 m lane.
        let dinghy = lake.center + DVec3::new(-30.0, 0.0, 30.0);
        assert!((at - dinghy).length() > 22.0 + 10.0);
    }

    #[test]
    fn every_zone_streams_as_a_cell_that_holds_it() {
        let land = landscape(1);
        let cells = zone_cells(&land, None);
        for z in ZONES {
            let cell = cells.iter().find(|c| c.key == z.key).expect(z.key);
            assert!(
                cell.center.distance(z.center) + z.radius <= cell.radius + 1e-6,
                "{}'s pad reaches past its cell",
                z.key
            );
        }
        // The yard's dinghy and silks stand away from it, by the lake and in the stream.
        for part in ["yard_sailboat", "yard_rinse"] {
            assert!(cells.iter().any(|c| c.key == part), "{part}");
        }
        let keys: std::collections::HashSet<_> = cells.iter().map(|c| c.key).collect();
        assert_eq!(keys.len(), cells.len(), "two cells share a key");
        // Only the yard (with its parts) for `--zones yard`.
        let yard = zone_cells(&land, Some(&DemoZones(vec!["yard".into()])));
        assert_eq!(yard.len(), 3);
        assert!(yard.iter().all(|c| c.key.starts_with("yard")));
    }

    #[test]
    fn a_zone_draws_the_same_whatever_loaded_before_it() {
        let draw = |key| {
            let mut rng = cell_rng(7, key);
            (0..4).map(|_| rng.next_u64()).collect::<Vec<_>>()
        };
        assert_eq!(draw("field"), draw("field"));
        assert_ne!(draw("field"), draw("plaza"));
    }

    #[test]
    fn each_zone_keeps_its_far_view_under_its_own_version() {
        assert_eq!(proxy_version(7, "fortress"), proxy_version(7, "fortress"));
        assert_ne!(proxy_version(7, "fortress"), proxy_version(7, "plaza"));
        assert_ne!(proxy_version(7, "fortress"), proxy_version(8, "fortress"));
        let cells = zone_cells(&landscape(1), None);
        let proxied = |key| cells.iter().find(|c| c.key == key).unwrap().proxy;
        assert!(proxied("fortress") && proxied("plaza"));
        assert!(!proxied("yard_sailboat") && !proxied("meadow"));
    }

    #[test]
    fn tours_of_zone_stops_load_only_those_zones() {
        assert_eq!(zone_of("paddock_car"), Some("paddock"));
        assert_eq!(zone_of("track_cars"), Some("track"));
        assert_eq!(zone_of("tracks"), Some("tracks"));
        assert_eq!(zone_of("overview"), Some("lake"));
        assert_eq!(zone_of("orbit"), None);
        assert_eq!(zone_of("fire_front"), Some("field"));
        assert_eq!(zone_of("smoke_fans"), Some("range"));
        assert_eq!(zone_of("firefly"), None);
        assert_eq!(zone_of("sunset"), Some("plaza"));
        assert_eq!(zone_of("night"), Some("plaza"));
        assert_eq!(zone_of("walk"), Some("plaza"));
        assert_eq!(zone_of("walk_falls"), Some("lake"));
        let only = |keys: &[&str]| Some(DemoZones(keys.iter().map(|k| k.to_string()).collect()));
        assert_eq!(
            demo_zones(None, Some("paddock_car,yard,paddock")),
            only(&["paddock", "yard"])
        );
        // A stop that sees more than one zone, or the whole tour, loads every zone.
        assert_eq!(demo_zones(None, Some("paddock,orbit")), None);
        assert_eq!(demo_zones(None, None), None);
        // The flag wins.
        assert_eq!(demo_zones(Some("all"), Some("paddock")), None);
        assert_eq!(demo_zones(Some("none"), Some("overview")), only(&[]));
        assert_eq!(
            demo_zones(Some("lake,water"), None),
            only(&["lake", "water"])
        );
    }

    #[test]
    fn every_demo_view_belongs_to_its_zone() {
        // Otherwise a tour of that view loads every zone, which a software renderer can't fit.
        let strays: Vec<String> = ZONES
            .iter()
            .flat_map(|z| demo_views(z).into_iter().map(move |v| (z.key, v.key)))
            .filter(|&(zone, key)| zone_of(key) != Some(zone))
            .map(|(zone, key)| format!("{key} (in {zone})"))
            .collect();
        assert!(strays.is_empty(), "views outside their zone: {strays:?}");
    }
}
