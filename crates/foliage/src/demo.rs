//! Showcase content: the living forest and the glowing-mushroom cave.
//!
//! Both only spawn data (ecology regions, resource sources, ground cover); the plugin grows
//! every plant from it. Call them from a game's setup with a zone center:
//!
//! ```ignore
//! engine_foliage::demo::spawn_demo(&mut commands, forest_center);
//! engine_foliage::demo::spawn_cave_demo(&mut commands, cave_center);
//! ```
//!
//! **Forest** (radius ≈ 90 m, entered from +X): an old mixed forest grown for 110 simulated
//! years, then partly cut and partly burnt 5 years ago. A dirt trail ([`TRAIL`]) leads from the
//! +X edge through every feature below; [`forest_viewpoints`] stand on it.
//! - A clear-cut opening on the +X/+Z side: stumps (oak, birch and hazel resprouting),
//!   grey logs, young trees and grass coming back, and every 25 s a tree at its edge is felled
//!   so you can watch one fall.
//! - A burn scar on the −X/+Z side: charred snags over black ash, birch and grass recolonizing.
//! - A spring-fed pond (−10, +10): lush ferns, bluebells and birches around it.
//! - A dry, rocky ridge (−55, −40): pines win there; oaks and birches struggle.
//! - Growth runs 30× faster than the world default: a year every 20 seconds.
//!
//! **Cave** (≈ 28 × 32 m inside): no light, so only fungi grow. A mineral pool and a seep along
//! the back-left wall feed glowcaps and tall lantern caps; the rest of the floor stays bare.
//! The mushrooms glow brighter the better fed they are and light the cave around them.

use engine_core::prelude::*;
use engine_render::{BoxFace, MeshRenderer, Shape, SkyLightVolume, StandardMaterial};

use crate::felling::fell_tree;
use crate::plant::{Disturbance, EcologyRegion, Plant, PlantStage, RegionHistory, ResourceSource};
use crate::scenery::{PlacedRock, Pool, random_rock, rock_cluster, spawn_rocks};
use crate::species::PlantLibrary;

/// Forest clear-cut, local to the zone center (x, z, radius).
pub const CLEARING: [f32; 3] = [40.0, 28.0, 18.0];
/// Forest burn scar (x, z, radius).
pub const BURN: [f32; 3] = [-34.0, 52.0, 17.0];
/// Spring-fed pond (x, z, radius of open water).
pub const POND: [f32; 3] = [-10.0, 10.0, 4.5];
/// Dry ridge (x, z, radius).
pub const RIDGE: [f32; 3] = [-55.0, -40.0, 26.0];
/// Height of the pond's water above the ground, meters (its shore sinks to meet it).
pub const POND_WATER_M: f32 = 0.06;
/// Years since the forest was cut and burnt.
pub const YEARS_SINCE: f32 = 5.0;
/// The forest trail (local x, z): in from the +X edge, across the clearing, past the pond and
/// the burn scar, up onto the dry ridge. Nothing grows on it.
pub const TRAIL: [[f32; 2]; 13] = [
    [96.0, 41.0],
    [74.0, 36.0],
    [57.0, 31.0],
    [40.0, 27.0],
    [24.0, 22.0],
    [8.0, 16.0],
    [-3.0, 15.5],
    [-14.0, 24.0],
    [-25.0, 35.0],
    [-38.0, 22.0],
    [-46.0, 2.0],
    [-52.0, -18.0],
    [-56.0, -34.0],
];
/// Half the trail's width, meters.
pub const TRAIL_HALF_WIDTH: f32 = 1.6;

/// Distance from `p` to a polyline.
pub fn polyline_distance(p: Vec2, line: &[[f32; 2]]) -> f32 {
    line.windows(2)
        .map(|w| {
            let (a, b) = (Vec2::from(w[0]), Vec2::from(w[1]));
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
            (a + ab * t).distance(p)
        })
        .fold(f32::INFINITY, f32::min)
}

/// Circles along the trail for [`EcologyRegion::keep_clear`].
fn trail_clearance() -> Vec<[f32; 3]> {
    let mut out = Vec::new();
    for w in TRAIL.windows(2) {
        let (a, b) = (Vec2::from(w[0]), Vec2::from(w[1]));
        let n = (a.distance(b) / 1.5).ceil() as usize;
        for i in 0..n {
            let p = a.lerp(b, i as f32 / n as f32);
            out.push([p.x, p.y, TRAIL_HALF_WIDTH + 0.6]);
        }
    }
    out
}

/// A camera viewpoint of a showcase: world-space eye and target, and a good hour of day.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewpoint {
    /// Short unique key, like `forest_trail`.
    pub name: &'static str,
    /// What it shows, for a tour or a menu.
    pub title: &'static str,
    pub eye: DVec3,
    pub target: DVec3,
    pub hour: f32,
}

fn view(
    center: DVec3,
    (name, title): (&'static str, &'static str),
    eye: [f32; 3],
    target: [f32; 3],
    hour: f32,
) -> Viewpoint {
    let at = |v: [f32; 3]| center + DVec3::new(v[0] as f64, v[1] as f64, v[2] as f64);
    Viewpoint {
        name,
        title,
        eye: at(eye),
        target: at(target),
        hour,
    }
}

/// Where to look at the forest from (heights above the zone center; on uneven ground, add the
/// ground height). All but the first stand on the trail.
pub fn forest_viewpoints(center: DVec3) -> Vec<Viewpoint> {
    vec![
        view(
            center,
            ("forest_overview", "The living forest from above"),
            [112.0, 32.0, 92.0],
            [8.0, 0.0, 12.0],
            9.0,
        ),
        view(
            center,
            ("forest_trail", "The trail into the forest"),
            [76.0, 1.7, 36.5],
            [56.0, 2.5, 31.0],
            9.5,
        ),
        view(
            center,
            (
                "forest_clearing",
                "A clear-cut growing back; a tree falls every 25 s",
            ),
            [58.0, 2.0, 31.5],
            [36.0, 1.5, 25.0],
            9.5,
        ),
        view(
            center,
            ("forest_spring", "Ferns and birches by the spring-fed pond"),
            [-1.0, 1.7, 15.5],
            [-11.0, 0.3, 9.0],
            10.0,
        ),
        view(
            center,
            ("forest_burn_scar", "Charred snags and grass after a fire"),
            [-26.0, 2.0, 37.5],
            [-36.0, 3.0, 54.0],
            10.5,
        ),
        view(
            center,
            ("forest_dry_ridge", "Pines win on the dry, rocky ridge"),
            [-51.5, 1.9, -16.0],
            [-57.0, 2.0, -42.0],
            16.0,
        ),
    ]
}

/// Room in front of the forest's ground-level viewpoints, so no trunk takes root right in
/// front of the lens (shrubs and ferns still frame the view).
fn lens_clearance() -> Vec<[f32; 3]> {
    forest_viewpoints(DVec3::ZERO)
        .iter()
        .filter(|v| v.eye.y < 5.0)
        .map(|v| {
            let ahead = DVec2::new(v.target.x - v.eye.x, v.target.z - v.eye.z).normalize();
            let c = DVec2::new(v.eye.x, v.eye.z) + ahead * 3.0;
            [c.x as f32, c.y as f32, 3.5]
        })
        .collect()
}

/// Where to look at the mushroom cave from.
pub fn cave_viewpoints(center: DVec3) -> Vec<Viewpoint> {
    vec![
        view(
            center,
            ("cave_fungi", "Glowing fungi where mineral water reaches"),
            [0.0, 3.0, -6.0],
            [0.0, 0.5, 10.0],
            14.0,
        ),
        view(
            center,
            ("cave_pool", "Glowcaps around the mineral pool"),
            [CAVE_POOL[0] + 6.0, 1.4, CAVE_POOL[1] - 5.0],
            [CAVE_POOL[0] - 1.0, 0.4, CAVE_POOL[1] + 2.0],
            14.0,
        ),
    ]
}

fn local(center: DVec3, x: f32, z: f32) -> DVec3 {
    center + DVec3::new(x as f64, 0.0, z as f64)
}

/// The forest's ecology region (centered on the zone).
pub fn forest_region() -> EcologyRegion {
    EcologyRegion {
        half_extents: [86.0, 86.0],
        round: true,
        cell_m: 2.0,
        species: [
            "oak",
            "birch",
            "pine",
            "spruce",
            "hazel",
            "fern",
            "grass",
            "wildflowers",
            "bluebell",
        ]
        .map(String::from)
        .to_vec(),
        seed_rain: 150.0,
        sky_light: 1.0,
        moisture: 0.42,
        nutrients: 0.45,
        patchiness: 0.75,
        max_plants: [700, 600, 14000],
        step_years: 0.25,
        time_scale: 30.0,
        keep_clear: [vec![POND], trail_clearance()].concat(),
        no_trees: lens_clearance(),
        history: RegionHistory {
            grow_years: 110.0,
            initial_plants: [260, 120, 1500],
            disturbance: Disturbance::Many(vec![
                Disturbance::ClearCut {
                    center: [CLEARING[0], CLEARING[1]],
                    radius: CLEARING[2],
                },
                Disturbance::Burn {
                    center: [BURN[0], BURN[1]],
                    radius: BURN[2],
                },
            ]),
            years_since: YEARS_SINCE,
        },
        seed: 7,
        ..Default::default()
    }
}

/// The forest's resource sources, local to the zone center: the spring and the dry ridge.
pub fn forest_sources() -> Vec<(Vec2, ResourceSource)> {
    vec![
        (
            Vec2::new(POND[0], POND[1]),
            ResourceSource {
                radius_m: 18.0,
                water: 0.45,
                nutrients: 0.12,
                minerals: 0.0,
            },
        ),
        (
            Vec2::new(RIDGE[0], RIDGE[1]),
            ResourceSource {
                radius_m: RIDGE[2],
                water: -0.4,
                nutrients: -0.15,
                minerals: 0.0,
            },
        ),
    ]
}

/// The forest's rocks, local to the zone center: lichen-grey outcrops and loose stones on the
/// dry ridge, stones kicked to the trail's sides, rounded stones at the pond's edge, sooty
/// boulders in the burn scar and mossy boulders under the trees.
pub fn forest_rocks() -> Vec<PlacedRock> {
    let mut rng = Rng::new(0x51d6e);
    let mut out = Vec::new();
    let off_trail = |p: Vec2, r: f32| polyline_distance(p, &TRAIL) > TRAIL_HALF_WIDTH + r + 0.25;
    let pond = Vec2::new(POND[0], POND[1]);
    let ridge = Vec2::new(RIDGE[0], RIDGE[1]);
    let burn = Vec2::new(BURN[0], BURN[1]);
    let granite = [1.0, 0.98, 0.95];

    // The ridge: outcrops, then rocks and stones strewn between them.
    let mut outcrops: Vec<(Vec2, f32)> = Vec::new();
    for _ in 0..300 {
        if outcrops.len() >= 11 {
            break;
        }
        let p = ridge + rng.in_unit_disk() * RIDGE[2] * 0.9;
        let size = rng.range(1.1, 3.2);
        if !off_trail(p, size * 1.3)
            || outcrops
                .iter()
                .any(|(q, s)| q.distance(p) < (s + size) * 1.5)
        {
            continue;
        }
        outcrops.push((p, size));
        rock_cluster(&mut rng, p, size, 0.2, granite, &mut out);
    }
    for _ in 0..90 {
        let p = ridge + rng.in_unit_disk() * RIDGE[2];
        let w = if rng.chance(0.25) {
            rng.range(0.4, 0.9)
        } else {
            rng.range(0.1, 0.32)
        };
        if off_trail(p, w) {
            out.push(random_rock(&mut rng, p, w, false, 0.15, granite));
        }
    }
    // Stones kicked to the trail's sides.
    for w in TRAIL.windows(2) {
        let (a, b) = (Vec2::from(w[0]), Vec2::from(w[1]));
        let n = (a.distance(b) / 4.5).ceil() as usize;
        let side = (b - a).perp().normalize_or_zero();
        for i in 0..n {
            if !rng.chance(0.6) {
                continue;
            }
            let p = a.lerp(b, (i as f32 + rng.f32()) / n as f32);
            let width = rng.range(0.1, 0.42);
            let sign = if rng.chance(0.5) { 1.0 } else { -1.0 };
            let out_m = TRAIL_HALF_WIDTH + 0.35 + width * 0.5 + rng.range(0.0, 0.9);
            let q = p + side * sign * out_m;
            if q.distance(pond) < POND[2] + 1.0 || !off_trail(q, width * 0.5 - 0.3) {
                continue;
            }
            let rounded = rng.chance(0.2);
            out.push(random_rock(&mut rng, q, width, rounded, 0.4, granite));
        }
    }
    // Stones worn flat by feet, sunk into the trail.
    let mut embedded = Vec::new();
    for w in TRAIL.windows(2) {
        let (a, b) = (Vec2::from(w[0]), Vec2::from(w[1]));
        let n = (a.distance(b) / 2.5).ceil() as usize;
        let side = (b - a).perp().normalize_or_zero();
        for i in 0..n {
            if !rng.chance(0.55) {
                continue;
            }
            let p = a.lerp(b, (i as f32 + rng.f32()) / n as f32)
                + side * rng.range(-0.8, 0.8) * TRAIL_HALF_WIDTH;
            if p.distance(pond) < POND[2] + 1.0 {
                continue;
            }
            let width = rng.range(0.12, 0.38);
            let rounded = rng.chance(0.4);
            let mut r = random_rock(&mut rng, p, width, rounded, 0.0, [0.9, 0.86, 0.8]);
            r.size.y = width * rng.range(0.3, 0.45);
            r.rock.bury_m = r.size.y * rng.range(0.6, 0.8);
            r.rock.lichen = 0.05;
            embedded.push(r);
        }
    }
    // Round the pond: rounded stones at the water's edge and a mossy boulder or two.
    for _ in 0..22 {
        let a = rng.range(0.0, TAU);
        let p = pond + Vec2::from_angle(a) * (POND[2] + rng.range(-0.5, 1.4));
        let w = rng.range(0.14, 0.5);
        let rounded = rng.chance(0.6);
        if off_trail(p, w) {
            // Water-worn cobbles lie low and flat, pale where they dry above the water line.
            let mut stone = random_rock(&mut rng, p, w, rounded, 0.3, [1.08, 1.05, 0.99]);
            stone.size.y *= 0.6;
            stone.rock.bury_m *= 0.6;
            out.push(stone);
        }
    }
    for a in [0.6f32, 3.9] {
        let p = pond + Vec2::from_angle(a) * (POND[2] + 1.6);
        let size = rng.range(0.9, 1.4);
        if off_trail(p, 1.0) {
            rock_cluster(&mut rng, p, size, 0.9, granite, &mut out);
        }
    }
    // Sooty boulders in the burn scar.
    for _ in 0..4 {
        let p = burn + rng.in_unit_disk() * BURN[2] * 0.7;
        let size = rng.range(0.7, 1.4);
        if off_trail(p, 1.0) {
            rock_cluster(&mut rng, p, size, 0.0, [0.5, 0.47, 0.45], &mut out);
        }
    }
    // Mossy boulders under the trees.
    let mut placed = 0;
    for _ in 0..200 {
        if placed >= 16 {
            break;
        }
        let p = rng.in_unit_disk() * 80.0;
        let size = rng.range(0.6, 1.8);
        if p.distance(ridge) < RIDGE[2] + 4.0
            || p.distance(pond) < POND[2] + 4.0
            || p.distance(burn) < BURN[2] + 3.0
            || !off_trail(p, size * 1.4)
        {
            continue;
        }
        placed += 1;
        let mossy = [0.92, 0.95, 0.9];
        if rng.chance(0.5) {
            rock_cluster(&mut rng, p, size, 0.95, mossy, &mut out);
        } else {
            out.push(random_rock(&mut rng, p, size, false, 0.95, mossy));
        }
    }
    // Nothing broken off an outcrop lies on the trail itself; only the sunk stones do.
    out.retain(|r| polyline_distance(r.pos, &TRAIL) > TRAIL_HALF_WIDTH + r.radius() * 0.5);
    out.extend(embedded);
    out
}

/// Keep-clear circles under the rocks big enough to keep plants off them.
fn rock_footprints(rocks: &[PlacedRock]) -> Vec<[f32; 3]> {
    rocks
        .iter()
        .filter(|r| r.radius() > 0.25 && r.rock.bury_m < r.size.y * 0.9)
        .map(|r| [r.pos.x, r.pos.y, r.radius() * 0.85])
        .collect()
}

/// The living forest at `center` (flat ground or whatever `engine_world::Ground` says).
pub fn spawn_demo(commands: &mut Commands, center: DVec3) {
    let rocks = forest_rocks();
    let mut region = forest_region();
    // Nothing grows through a rock.
    region.keep_clear.extend(rock_footprints(&rocks));
    commands.spawn((Transform::from_translation(center), region));
    for (at, source) in forest_sources() {
        commands.spawn((
            Transform::from_translation(local(center, at.x, at.y)),
            source,
        ));
    }
    // The pond: dark still water sunk below a shore of wet mud and gravel (the ground
    // cover), stones and reeds round it.
    let mut water = StandardMaterial::hex(0x0b1a18).with_roughness(0.015);
    water.reflectance = 0.45;
    water.receives_wetness = false;
    commands.spawn((
        Transform::from_translation(
            local(center, POND[0], POND[1]) + DVec3::Y * (POND_WATER_M - 0.01) as f64,
        ),
        MeshRenderer::new(
            Shape::Cylinder {
                radius: POND[2] + 0.7,
                height: 0.02,
                segments: 72,
            },
            water,
        ),
    ));
    // The trees round the pond hide the low sky its still water would otherwise mirror at a
    // glance (the renderer's sky occlusion only sees cover from above): sky light dims toward
    // the water, fading back in at the edges and overhead.
    let reach = POND[2] + 2.5;
    commands.spawn((
        Transform::from_translation(
            local(center, POND[0], POND[1]) + DVec3::Y * POND_WATER_M as f64,
        ),
        SkyLightVolume::new([reach, 2.5, reach])
            .open(BoxFace::PosY)
            .open(BoxFace::PosX)
            .open(BoxFace::NegX)
            .open(BoxFace::PosZ)
            .open(BoxFace::NegZ)
            .with_sky_light(0.2)
            .with_fade(2.5),
    ));
    spawn_rocks(commands, center, rocks);
    // Leaf litter, needles, moss, ash and packed earth under the trees.
    commands.spawn((
        Transform::from_translation(center),
        GroundCover {
            half_extents: [92.0, 92.0],
            round: true,
            cell_m: 0.5,
            style: CoverStyle::ForestFloor,
            scorched: vec![[BURN[0], BURN[1], BURN[2] + 2.0, YEARS_SINCE]],
            trail: TRAIL.to_vec(),
            trail_half_width: TRAIL_HALF_WIDTH,
            pools: vec![Pool {
                center: [POND[0], POND[1]],
                radius: POND[2],
                water_m: POND_WATER_M,
                mineral: false,
            }],
            debris: 1.0,
            ..Default::default()
        },
    ));
    // A tree at the clearing's edge falls every so often.
    commands.spawn((
        Transform::from_translation(local(center, CLEARING[0], CLEARING[1])),
        DemoFelling {
            every_secs: 25.0,
            inner_m: CLEARING[2] - 2.0,
            outer_m: CLEARING[2] + 9.0,
            timer: 0.5,
            max_trees: 60,
            felled: 0,
        },
    ));
}

/// Cave pool, local to the cave center (x, z, radius).
pub const CAVE_POOL: [f32; 3] = [0.0, 4.0, 5.0];
/// Height of the cave pool's water above the floor, meters.
pub const CAVE_WATER_M: f32 = 0.05;
/// Mineral seep along the back-left wall (x, z, radius).
pub const CAVE_SEEP: [f32; 3] = [-9.5, 11.5, 5.0];

/// The cave's ecology region (centered on the zone): only fungi, fed by minerals.
pub fn cave_region() -> EcologyRegion {
    EcologyRegion {
        half_extents: [13.5, 15.5],
        cell_m: 1.0,
        species: vec!["glowcap".into(), "lantern cap".into()],
        seed_rain: 30.0,
        sky_light: 0.03,
        moisture: 0.5,
        nutrients: 0.3,
        minerals: 0.0,
        patchiness: 0.6,
        max_plants: [0, 80, 1200],
        step_years: 0.1,
        time_scale: 20.0,
        keep_clear: vec![[CAVE_POOL[0], CAVE_POOL[1], CAVE_POOL[2] + 0.3]],
        history: RegionHistory {
            grow_years: 10.0,
            initial_plants: [0, 40, 120],
            ..Default::default()
        },
        seed: 11,
        ..Default::default()
    }
}

/// The cave's resource sources, local to the zone center: the mineral pool and the seep.
pub fn cave_sources() -> Vec<(Vec2, ResourceSource)> {
    vec![
        (
            Vec2::new(CAVE_POOL[0], CAVE_POOL[1]),
            ResourceSource {
                radius_m: 11.0,
                water: 0.35,
                nutrients: 0.05,
                minerals: 0.95,
            },
        ),
        (
            Vec2::new(CAVE_SEEP[0], CAVE_SEEP[1]),
            ResourceSource {
                radius_m: CAVE_SEEP[2],
                water: 0.25,
                nutrients: 0.0,
                minerals: 0.8,
            },
        ),
    ]
}

/// The mushroom cave's living part at `center` (inside a cave of about 28 × 32 m whose mouth
/// faces −Z): mineral pool, seep, cave floor and the fungi that grow where minerals reach.
pub fn spawn_cave_demo(commands: &mut Commands, center: DVec3) {
    let rocks = cave_rocks();
    let mut region = cave_region();
    region.keep_clear.extend(rock_footprints(&rocks));
    commands.spawn((Transform::from_translation(center), region));
    for (at, source) in cave_sources() {
        commands.spawn((
            Transform::from_translation(local(center, at.x, at.y)),
            source,
        ));
    }
    // The pool: dark mineral water with a faint glow of its own, held by a rim of crust.
    let mut pool = StandardMaterial::hex(0x061a1e)
        .with_roughness(0.03)
        .with_emissive([0.1, 0.9, 0.75], 0.12);
    pool.reflectance = 0.5;
    pool.receives_wetness = false;
    commands.spawn((
        Transform::from_translation(
            local(center, CAVE_POOL[0], CAVE_POOL[1]) + DVec3::Y * (CAVE_WATER_M - 0.01) as f64,
        ),
        MeshRenderer::new(
            Shape::Cylinder {
                radius: CAVE_POOL[2] + 0.5,
                height: 0.02,
                segments: 72,
            },
            pool,
        ),
    ));
    spawn_rocks(commands, center, rocks);
    commands.spawn((
        Transform::from_translation(center),
        GroundCover {
            half_extents: [14.2, 16.2],
            round: false,
            cell_m: 0.25,
            style: CoverStyle::CaveFloor,
            pools: vec![Pool {
                center: [CAVE_POOL[0], CAVE_POOL[1]],
                radius: CAVE_POOL[2],
                water_m: CAVE_WATER_M,
                mineral: true,
            }],
            debris: 0.6,
            ..Default::default()
        },
    ));
}

/// The cave's rocks, local to the zone center: rubble fallen along the walls (where the fungi
/// do not grow) and stones at the pool's edge.
pub fn cave_rocks() -> Vec<PlacedRock> {
    let mut rng = Rng::new(0xca7e);
    let stone = [0.62, 0.6, 0.58];
    let mut out = Vec::new();
    let seep = Vec2::new(CAVE_SEEP[0], CAVE_SEEP[1]);
    // Along the side and back walls (inner faces at x = ±14, z = +16).
    let wall = |rng: &mut Rng, p: Vec2, out: &mut Vec<PlacedRock>| {
        if p.distance(seep) < CAVE_SEEP[2] + 1.0 {
            return;
        }
        if rng.chance(0.45) {
            let size = rng.range(0.7, 1.5);
            rock_cluster(rng, p, size, 0.0, stone, out);
        } else {
            for _ in 0..rng.range_i32(1, 4) {
                let q = p + rng.in_unit_disk() * 1.2;
                let w = rng.range(0.2, 0.7);
                out.push(random_rock(rng, q, w, false, 0.0, stone));
            }
        }
    };
    let mut z = -15.0;
    while z < 15.0 {
        for x in [-13.2f32, 13.2] {
            if rng.chance(0.7) {
                let x = x + rng.range(-0.3, 0.4) * x.signum();
                wall(&mut rng, Vec2::new(x, z), &mut out);
            }
        }
        z += rng.range(2.2, 3.6);
    }
    let mut x = -12.0;
    while x < 12.0 {
        if rng.chance(0.7) {
            let z = 15.2 + rng.range(-0.3, 0.3);
            wall(&mut rng, Vec2::new(x, z), &mut out);
        }
        x += rng.range(2.4, 3.8);
    }
    // Inside the mouth, either side.
    for x in [-7.5f32, 7.5] {
        wall(&mut rng, Vec2::new(x, -15.0), &mut out);
    }
    // Stones at the pool's edge, half in the water.
    let pool = Vec2::new(CAVE_POOL[0], CAVE_POOL[1]);
    for _ in 0..9 {
        let p =
            pool + Vec2::from_angle(rng.range(0.0, TAU)) * (CAVE_POOL[2] + rng.range(-0.3, 0.4));
        let w = rng.range(0.15, 0.45);
        out.push(random_rock(&mut rng, p, w, true, 0.0, stone));
    }
    // Loose stones over the floor, away from the fungi.
    for _ in 0..40 {
        let p = Vec2::new(rng.range(-13.0, 13.0), rng.range(-15.0, 15.0));
        if p.distance(pool) < 10.0 || p.distance(seep) < CAVE_SEEP[2] + 0.5 {
            continue;
        }
        let w = rng.range(0.08, 0.3);
        out.push(random_rock(&mut rng, p, w, false, 0.0, stone));
    }
    out
}

// ---------------------------------------------------------------- ground cover

/// Look of a [`GroundCover`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CoverStyle {
    /// Leaf litter, humus and moss (mossier where the soil is wet), fading to grass at the edge.
    #[default]
    ForestFloor,
    /// Damp rock and mud with pale mineral crust where mineral water soaks the floor.
    CaveFloor,
}

/// A ground layer (forest floor, cave floor) laid over the ground, textured from the soil of
/// the ecology region beneath it and the plants on it: leaf litter under broadleaves, needles
/// under conifers, moss where wet, ash where burnt, packed earth on the trail, mud and gravel
/// round pools, mineral crust where mineral water soaks in. Debris (twigs, branches, cones,
/// leaves, pebbles) and reeds are scattered over it once the plants have grown; see
/// [`crate::scenery`].
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GroundCover {
    /// Half size along X and Z, meters.
    pub half_extents: [f32; 2],
    /// Elliptical, fading out at the edge.
    pub round: bool,
    /// Grid spacing, meters.
    pub cell_m: f32,
    pub style: CoverStyle,
    /// Burnt patches: local `[x, z, radius m, years ago]`.
    pub scorched: Vec<[f32; 4]>,
    /// A dirt trail through it: local `[x, z]` points.
    pub trail: Vec<[f32; 2]>,
    pub trail_half_width: f32,
    /// Pools of water: sunken shores of wet mud and gravel (or mineral crust) and reeds.
    pub pools: Vec<Pool>,
    /// How much debris (twigs, branches, cones, leaves, pebbles) to scatter, 0..1+.
    pub debris: f32,
    pub seed: u64,
}

impl Default for GroundCover {
    fn default() -> Self {
        Self {
            half_extents: [20.0, 20.0],
            round: true,
            cell_m: 1.0,
            style: CoverStyle::ForestFloor,
            scorched: Vec::new(),
            trail: Vec::new(),
            trail_half_width: 1.5,
            pools: Vec::new(),
            debris: 1.0,
            seed: 3,
        }
    }
}

// ---------------------------------------------------------------- live felling

/// Fells the nearest mature tree in a ring around this entity every `every_secs` (world time),
/// toward the center: a showcase of falling trees and the regrowth that follows.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DemoFelling {
    pub every_secs: f32,
    /// Ring of trees to fell, meters from this entity.
    pub inner_m: f32,
    pub outer_m: f32,
    /// Seconds until the next tree falls.
    pub timer: f32,
    /// Stop after this many.
    pub max_trees: u32,
    pub felled: u32,
}

impl Default for DemoFelling {
    fn default() -> Self {
        Self {
            every_secs: 30.0,
            inner_m: 10.0,
            outer_m: 20.0,
            timer: 5.0,
            max_trees: 20,
            felled: 0,
        }
    }
}

/// Run [`DemoFelling`] entities.
pub fn run_demo_felling(world: &mut World) {
    let dt = world.resource::<Time>().delta_secs();
    if dt <= 0.0 {
        return;
    }
    let mut q = world.query::<(Entity, &mut DemoFelling, &Transform)>();
    let mut due = Vec::new();
    for (e, mut d, t) in q.iter_mut(world) {
        if d.felled >= d.max_trees {
            continue;
        }
        d.timer -= dt;
        if d.timer <= 0.0 {
            d.timer = d.every_secs;
            due.push((e, t.translation, d.inner_m as f64, d.outer_m as f64));
        }
    }
    if due.is_empty() {
        return;
    }
    let Some(library) = world.get_resource::<PlantLibrary>().cloned() else {
        return;
    };
    for (e, center, inner, outer) in due {
        let mut pq = world.query::<(Entity, &Plant, &Transform)>();
        let pick = pq
            .iter(world)
            .filter(|(_, p, _)| {
                p.stage == PlantStage::Alive && p.size > 0.5 && library.get(p.species).is_tree()
            })
            .map(|(pe, _, t)| {
                (
                    pe,
                    t.translation,
                    DVec2::new(t.translation.x - center.x, t.translation.z - center.z).length(),
                )
            })
            .filter(|(_, _, d)| *d >= inner && *d <= outer)
            .min_by(|a, b| a.2.total_cmp(&b.2));
        if let Some((tree, p, _)) = pick {
            let dir = Vec3::new((center.x - p.x) as f32, 0.0, (center.z - p.z) as f32)
                .normalize_or(Vec3::X);
            if fell_tree(world, tree, dir).is_some()
                && let Some(mut d) = world.get_mut::<DemoFelling>(e)
            {
                d.felled += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::ecology::play_history;
    use crate::sim::{FlatEnv, RegionSim, SimPlant, SimSource, Soil};

    fn grown_forest() -> (Vec<SimPlant>, usize) {
        grow(forest_region(), forest_sources())
    }

    fn grow(region: EcologyRegion, sources: Vec<(Vec2, ResourceSource)>) -> (Vec<SimPlant>, usize) {
        let library = PlantLibrary::default();
        let env = FlatEnv::default();
        let sources: Vec<SimSource> = sources
            .into_iter()
            .map(|(at, s)| SimSource {
                pos: DVec3::new(at.x as f64, 0.0, at.y as f64),
                radius: s.radius_m,
                water: s.water,
                nutrients: s.nutrients,
                minerals: s.minerals,
            })
            .collect();
        let soil = Soil::build(&region, DVec3::ZERO, &sources, &env);
        let mut sim = RegionSim::new(soil, region.seed);
        let rain: Vec<_> = region
            .species
            .iter()
            .filter_map(|n| library.id(n))
            .collect();
        let (plants, logs) = play_history(&mut sim, &region, DVec3::ZERO, &library, &rain, &env);
        (plants, logs.len())
    }

    fn census(plants: &[SimPlant], within: Option<[f32; 3]>) -> BTreeMap<String, usize> {
        let library = PlantLibrary::default();
        let mut out = BTreeMap::new();
        for p in plants {
            if let Some([x, z, r]) = within
                && DVec2::new(p.pos.x - x as f64, p.pos.z - z as f64).length() > r as f64
            {
                continue;
            }
            let key = format!(
                "{} {:?}",
                library.get(p.plant.species).name,
                std::mem::discriminant(&p.plant.stage)
            );
            let key = key.split(' ').next().unwrap().to_string()
                + match p.plant.stage {
                    PlantStage::Alive if p.plant.size < 0.25 => " young",
                    PlantStage::Alive => "",
                    PlantStage::Stump { .. } => " stump",
                    PlantStage::Dead { .. } => " dead",
                    PlantStage::Burnt { .. } => " burnt",
                };
            *out.entry(key).or_insert(0) += 1;
        }
        out
    }

    #[test]
    fn showcase_forest_is_mixed_and_regrowing() {
        let (plants, logs) = grown_forest();
        let all = census(&plants, None);
        let clearing = census(&plants, Some(CLEARING));
        let burn = census(&plants, Some(BURN));
        let spring = census(&plants, Some([POND[0], POND[1], 14.0]));
        let ridge = census(&plants, Some(RIDGE));
        eprintln!(
            "all: {all:?}\nclearing: {clearing:?}\nburn: {burn:?}\nspring: {spring:?}\nridge: {ridge:?}\nlogs: {logs}"
        );
        let n = |c: &BTreeMap<String, usize>, k: &str| c.get(k).copied().unwrap_or(0);
        let sum = |c: &BTreeMap<String, usize>, keys: &[&str]| {
            keys.iter().map(|k| n(c, k)).sum::<usize>()
        };

        // A mixed forest: no tree species takes over.
        let canopy = sum(&all, &["oak", "birch", "pine", "spruce"]);
        for tree in ["oak", "pine", "spruce"] {
            assert!(n(&all, tree) * 10 > canopy, "too few {tree}: {all:?}");
            assert!(n(&all, tree) * 2 < canopy, "{tree} took over: {all:?}");
        }
        assert!(sum(&all, &["fern", "grass"]) > 2000, "{all:?}");
        // The clear-cut: stumps of pines and spruces (oaks, birches and hazels resprouted),
        // young trees and plenty of grass.
        assert!(
            sum(&clearing, &["pine stump", "spruce stump"]) >= 3,
            "{clearing:?}"
        );
        assert!(
            sum(
                &clearing,
                &[
                    "oak young",
                    "birch young",
                    "pine young",
                    "spruce young",
                    "birch",
                    "hazel"
                ]
            ) >= 5,
            "{clearing:?}"
        );
        assert!(n(&clearing, "grass") > 100, "{clearing:?}");
        assert!(logs >= 10);
        // The burn: charred snags, grass and ferns coming back.
        let snags = burn
            .iter()
            .filter(|(k, _)| k.ends_with("burnt"))
            .map(|(_, v)| v)
            .sum::<usize>();
        assert!(snags >= 10, "{burn:?}");
        assert!(n(&burn, "grass") > 50, "{burn:?}");
        // Ferns love the spring; pines win on the dry ridge.
        assert!(n(&spring, "fern") > 50, "{spring:?}");
        assert!(
            n(&ridge, "pine") > n(&ridge, "birch") + n(&ridge, "oak"),
            "{ridge:?}"
        );
    }

    #[test]
    fn showcase_cave_glows_where_minerals_reach() {
        let (plants, _) = grow(cave_region(), cave_sources());
        let all = census(&plants, None);
        eprintln!("cave: {all:?}");
        let n = |k: &str| all.get(k).copied().unwrap_or(0);
        assert!(n("glowcap") > 80, "{all:?}");
        assert!(n("lantern") >= 4, "{all:?}");
        // Fungi only where the pool or the seep feeds the floor.
        let fed = |p: &SimPlant| {
            let d = |c: [f32; 3]| DVec2::new(p.pos.x - c[0] as f64, p.pos.z - c[1] as f64).length();
            d(CAVE_POOL) < 11.0 || d(CAVE_SEEP) < CAVE_SEEP[2] as f64
        };
        let alive: Vec<_> = plants.iter().filter(|p| p.plant.stage.is_alive()).collect();
        let stray = alive.iter().filter(|p| !fed(p)).count();
        assert!(
            stray * 20 < alive.len(),
            "{stray} of {} fungi grow on bare rock",
            alive.len()
        );
    }
}
