//! The grown start: a harbour town on Haverholm, a farming island that feeds it, a northern
//! island of sheep and timber, and the routes that tie them, all laid out through the same
//! build calls a player makes. It searches the generated ground for each site, so it holds
//! up when the islands change shape.

use std::collections::{HashSet, VecDeque};

use engine::prelude::*;

use crate::build::{self, Changed, place, place_road};
use crate::defs::{CELL_M, Good, Kind, Tier};
use crate::fleet::{Roam, Route, Routes, Stop, mast_point, mooring, spawn_vessel};
use crate::grid::{self, Grid};
use crate::island::{self, islands};
use crate::sim::{Building, Economy, House};
use crate::vessels::Craft;

/// The middle of the home town, where the view starts.
#[derive(Resource, Clone, Copy, Debug)]
pub struct TownCentre(pub DVec3);

/// Find a free site for `kind` as near `target` as the ground allows (any rotation), place
/// it and join it to the nearest road. Returns the building.
pub fn place_near(
    world: &mut World,
    kind: Kind,
    target: DVec2,
    max_cells: i32,
    connect: bool,
) -> Option<Entity> {
    let t = grid::cell_of(target.x, target.y);
    let (w, d) = kind.def().size;
    let half = IVec2::new(w / 2, d / 2);
    for r in 0..max_cells {
        let mut ring: Vec<IVec2> = Vec::new();
        for dz in -r..=r {
            for dx in -r..=r {
                if dx.abs() == r || dz.abs() == r {
                    ring.push(IVec2::new(dx, dz));
                }
            }
        }
        for off in ring {
            for rot in 0..4u8 {
                let origin = t + off - half;
                if grid::check(world.resource::<Grid>(), kind, origin, rot).is_err() {
                    continue;
                }
                // Coast buildings face the sea; land buildings like a road at the front.
                let Ok(e) = place(world, kind, origin, rot, false) else {
                    continue;
                };
                if !connect || join_to_roads(world, e) {
                    return Some(e);
                }
                build::demolish(world, e);
            }
        }
    }
    None
}

/// Lay roads from a building's edge to the nearest road (or to `fallback` cells) over
/// buildable ground. False if no road is within reach.
pub fn join_to_roads(world: &mut World, e: Entity) -> bool {
    let Some(b) = world.get::<Building>(e).cloned() else {
        return false;
    };
    let ring = grid::ring(b.kind, b.origin, b.rot);
    let g = world.resource::<Grid>();
    if ring.iter().any(|c| g.is_road(*c)) {
        return true;
    }
    if g.roads.is_empty() {
        return true;
    }
    // BFS over free buildable cells to the nearest road.
    let mut seen: HashSet<IVec2> = HashSet::new();
    let mut from: std::collections::HashMap<IVec2, IVec2> = Default::default();
    let mut q = VecDeque::new();
    for c in ring {
        if g.is_open(c) && grid::cell_roadable(c) {
            seen.insert(c);
            q.push_back(c);
        }
    }
    let mut goal = None;
    while let Some(c) = q.pop_front() {
        if seen.len() > 40_000 {
            break;
        }
        for n in [IVec2::X, IVec2::NEG_X, IVec2::Y, IVec2::NEG_Y] {
            let nc = c + n;
            if g.is_road(nc) {
                goal = Some(c);
                break;
            }
            if !seen.contains(&nc) && g.is_open(nc) && grid::cell_roadable(nc) {
                seen.insert(nc);
                from.insert(nc, c);
                q.push_back(nc);
            }
        }
        if goal.is_some() {
            break;
        }
    }
    let Some(mut c) = goal else {
        return false;
    };
    let mut cells = vec![c];
    while let Some(p) = from.get(&c) {
        c = *p;
        cells.push(c);
    }
    for c in cells {
        let _ = place_road(world, c, false);
    }
    true
}

/// A grid of streets every `block` cells around `center`, out to `radius` meters, where the
/// ground allows.
fn street_grid(world: &mut World, center: DVec2, radius: f64, block: i32, yaw_cells: bool) {
    let c = grid::cell_of(center.x, center.y);
    let r = (radius / CELL_M) as i32;
    let _ = yaw_cells;
    let inside = |p: IVec2| (grid::cell_center(p) - center).length() < radius;
    for k in (-r..=r).step_by(block as usize) {
        for i in -r..=r {
            for p in [c + IVec2::new(i, k), c + IVec2::new(k, i)] {
                if inside(p) && grid::cell_roadable(p) && world.resource::<Grid>().is_free(p) {
                    let _ = place_road(world, p, false);
                }
            }
        }
    }
}

/// Take up the streets laid out ahead that nothing was built along: keep road cells that
/// front a building, and the way from each of them back to a harbour.
fn prune_roads(world: &mut World) {
    let roads = world.resource::<Grid>().roads.clone();
    let mut parent: std::collections::HashMap<IVec2, IVec2> = Default::default();
    let mut q = VecDeque::new();
    let mut buildings: Vec<Building> = world.query::<&Building>().iter(world).cloned().collect();
    buildings.sort_by_key(|b| b.id);
    for b in buildings.iter().filter(|b| b.kind == Kind::Harbour) {
        for c in grid::ring(b.kind, b.origin, b.rot) {
            if roads.contains(&c) && !parent.contains_key(&c) {
                parent.insert(c, c);
                q.push_back(c);
            }
        }
    }
    while let Some(c) = q.pop_front() {
        for n in [IVec2::X, IVec2::NEG_X, IVec2::Y, IVec2::NEG_Y] {
            let nc = c + n;
            if roads.contains(&nc) && !parent.contains_key(&nc) {
                parent.insert(nc, c);
                q.push_back(nc);
            }
        }
    }
    let mut keep: HashSet<IVec2> = HashSet::new();
    for b in &buildings {
        for c in grid::ring(b.kind, b.origin, b.rot) {
            if !roads.contains(&c) {
                continue;
            }
            keep.insert(c);
            let mut at = c;
            while let Some(&p) = parent.get(&at) {
                if p == at || !keep.insert(p) {
                    break;
                }
                at = p;
            }
        }
    }
    let gone: Vec<IVec2> = roads.difference(&keep).copied().collect();
    for c in &gone {
        build::remove_road(world, *c);
    }
    engine::core::log::info!(
        "archipelago start: took up {} unused road cells",
        gone.len()
    );
}

/// Houses along every road cell inside `radius` of `center`, facing the road.
fn fill_houses(
    world: &mut World,
    center: DVec2,
    radius: f64,
    max: usize,
    tier: Tier,
) -> Vec<Entity> {
    let mut roads: Vec<IVec2> = world
        .resource::<Grid>()
        .roads
        .iter()
        .copied()
        .filter(|c| (grid::cell_center(*c) - center).length() < radius)
        .collect();
    roads.sort_by_key(|c| {
        (
            ((grid::cell_center(*c) - center).length() * 10.0) as i64,
            c.x,
            c.y,
        )
    });
    let mut out = Vec::new();
    for rc in roads {
        // The four sides: the house's front row is the road cell's row.
        for rot in 0..4u8 {
            if out.len() >= max {
                return out;
            }
            let f = grid::front_dir(rot);
            // Footprint 3×3 whose front row (outside) contains rc: origin so that the cell in
            // front of its middle is rc.
            let mid_front = rc - f;
            let origin = match rot {
                0 => mid_front - IVec2::new(1, 2),
                1 => mid_front - IVec2::new(2, 1),
                2 => mid_front - IVec2::new(1, 0),
                _ => mid_front - IVec2::new(0, 1),
            };
            if grid::check(world.resource::<Grid>(), Kind::House, origin, rot).is_ok()
                && let Ok(e) = place(world, Kind::House, origin, rot, false)
            {
                if let Some(mut h) = world.get_mut::<House>(e) {
                    h.tier = tier;
                    h.residents = tier.capacity();
                }
                out.push(e);
            }
        }
    }
    out
}

/// The tier a house is drawn at.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub struct ShownTier(pub Tier);

/// Redraw houses whose people moved up (checked twice a second).
pub fn upgrade_looks(world: &mut World, mut timer: Local<f32>) {
    *timer -= world.resource::<Time>().real_delta_secs();
    if *timer > 0.0 {
        return;
    }
    *timer = 0.5;
    let mut q = world.query::<(Entity, &Building, &House, Option<&ShownTier>)>();
    let stale: Vec<(Entity, Kind, Tier, u64)> = q
        .iter(world)
        .filter(|(_, _, h, s)| s.is_none_or(|s| s.0 != h.tier))
        .map(|(e, b, h, _)| (e, b.kind, h.tier, b.seed))
        .collect();
    if stale.is_empty() {
        return;
    }
    for (e, kind, tier, seed) in stale {
        world.entity_mut(e).insert((
            ShownTier(tier),
            crate::draw::Look(std::sync::Arc::new(crate::looks::look(kind, tier, seed))),
        ));
    }
    world.resource_mut::<crate::draw::Drawn>().dirty = true;
}

pub fn refresh_looks(world: &mut World) {
    let mut q = world.query::<(&Building, &House, &mut crate::draw::Look)>();
    for (b, h, mut look) in q.iter_mut(world) {
        *look = crate::draw::Look(std::sync::Arc::new(crate::looks::look(
            b.kind, h.tier, b.seed,
        )));
    }
    world.resource_mut::<crate::draw::Drawn>().dirty = true;
}

/// A harbour on `isle`'s coast facing `toward` (world X, Z), with the town's first street
/// running inland from it. Returns the harbour.
fn harbour_toward(world: &mut World, isle: usize, toward: DVec2) -> Option<Entity> {
    let c = islands()[isle].center;
    let dir = (toward - c).normalize();
    // Walk out from the middle to the coast along `dir`.
    let mut p = c;
    for _ in 0..400 {
        if island::height(p.x, p.y) < 0.5 {
            break;
        }
        p += dir * 5.0;
    }
    place_near(world, Kind::Harbour, p, 40, false)
}

/// The street from a harbour's back to `to`.
fn street_inland(world: &mut World, harbour: Entity, len_cells: i32) -> DVec2 {
    let b = world.get::<Building>(harbour).cloned().unwrap();
    let back = -grid::front_dir(b.rot);
    let (w, d) = b.kind.footprint(b.rot);
    // Middle of the back edge, one cell out.
    let mid = b.origin + IVec2::new(w / 2, d / 2);
    let start = mid + back * ((if back.x != 0 { w } else { d }) / 2 + 1);
    let mut c = start;
    let mut laid = 0;
    for _ in 0..len_cells {
        if place_road(world, c, false).is_err() {
            break;
        }
        laid += 1;
        c += back;
    }
    let _ = laid;
    grid::cell_center(c - back)
}

/// Woodcutters at the edges of the woods nearest `town` on `isle`, at least 90 m apart.
fn woodcutters_near(world: &mut World, isle: usize, town: DVec2, n: usize) {
    let mut spots: Vec<DVec2> = Vec::new();
    for j in -40..=40 {
        for i in -40..=40 {
            let p = town + DVec2::new(i as f64, j as f64) * 16.0;
            if island::island_at(p.x, p.y) == Some(isle) && island::woods(p.x, p.y) > 0.55 {
                spots.push(p);
            }
        }
    }
    spots.sort_by_key(|p| ((*p - town).length() * 10.0) as i64);
    let mut cut: Vec<DVec2> = Vec::new();
    for p in spots {
        if cut.len() >= n {
            break;
        }
        if cut.iter().any(|q| (*q - p).length() < 90.0) {
            continue;
        }
        if place_near(world, Kind::Woodcutter, p, 4, true).is_some() {
            cut.push(p);
        }
    }
}

/// Build the whole start. Called once, after the art exists.
pub fn build_start(world: &mut World) {
    let started = std::time::Instant::now();
    world.init_resource::<build::PaintHold>();
    let isles = islands();
    // --- Haverholm: the harbour town.
    let home = 0;
    let Some(h_harbour) = harbour_toward(world, home, isles[1].center) else {
        engine::core::log::warn!("archipelago: no harbour site on Haverholm");
        return;
    };
    let town = street_inland(world, h_harbour, 30);
    let ty = island::height(town.x, town.y).max(0.0);
    world.insert_resource(TownCentre(DVec3::new(town.x, ty, town.y)));
    street_grid(world, town, 170.0, 7, false);
    place_near(world, Kind::Market, town, 20, true);
    place_near(
        world,
        Kind::Chapel,
        town + DVec2::new(60.0, -40.0),
        25,
        true,
    );
    let inner = fill_houses(world, town, 95.0, 16, Tier::Labourers);
    let outer = fill_houses(world, town, 190.0, 44, Tier::Fisherfolk);
    let _ = (inner, outer);
    let hb = world.get::<Building>(h_harbour).cloned().unwrap();
    let hc = grid::footprint_center(hb.kind, hb.origin, hb.rot);
    let along = DVec2::new(
        grid::front_dir(hb.rot).y as f64,
        -grid::front_dir(hb.rot).x as f64,
    );
    let mut boats = 0;
    for k in [-1.0, 1.0, -2.0, 2.0, -3.0, 3.0] {
        if boats < 3
            && let Some(e) = place_near(world, Kind::Fishery, hc + along * (80.0 * k), 25, false)
        {
            join_to_roads(world, e);
            boats += 1;
        }
    }
    woodcutters_near(world, home, town, 4);
    // Spread round the inland side of the town: `a` from 0 to 2π sweeps 200° about the way
    // inland.
    let inland = isles[home].center - town;
    let base = inland.y.atan2(inland.x);
    let toward = |a: f64, r: f64| {
        let t = base + (a / std::f64::consts::TAU - 0.5) * 200f64.to_radians();
        town + DVec2::new(t.cos(), t.sin()) * r
    };
    place_near(world, Kind::Sawmill, toward(0.6, 210.0), 30, true);
    place_near(world, Kind::Sawmill, toward(0.9, 230.0), 30, true);
    place_near(world, Kind::Weaver, toward(2.4, 215.0), 30, true);
    place_near(world, Kind::SheepFarm, toward(2.9, 300.0), 40, true);
    place_near(world, Kind::SheepFarm, toward(3.4, 320.0), 40, true);
    place_near(world, Kind::ClayPit, toward(4.2, 260.0), 40, true);
    place_near(world, Kind::Brickworks, toward(4.5, 230.0), 30, true);
    let h_mast = place_near(world, Kind::AirshipMast, toward(5.4, 240.0), 40, true);
    // --- Brannsey: grain, flour, bread, hops and ale.
    let b_harbour = harbour_toward(world, 1, isles[0].center);
    if let Some(bh) = b_harbour {
        let village = street_inland(world, bh, 16);
        street_grid(world, village, 90.0, 7, false);
        place_near(world, Kind::Market, village, 15, true);
        place_near(
            world,
            Kind::Chapel,
            village + DVec2::new(-30.0, 30.0),
            20,
            true,
        );
        fill_houses(world, village, 100.0, 20, Tier::Labourers);
        fill_houses(world, village, 130.0, 4, Tier::Fisherfolk);
        // Farms out on the open land behind the village, where their fields have room.
        let inland = (isles[1].center - village).normalize();
        let out = |a: f64, r: f64| village + DVec2::from_angle(a).rotate(inland) * r;
        for (a, r) in [(-0.8, 240.0), (0.05, 300.0), (0.85, 250.0)] {
            place_near(world, Kind::GrainFarm, out(a, r), 40, true);
        }
        place_near(
            world,
            Kind::Mill,
            village + DVec2::new(110.0, 40.0),
            30,
            true,
        );
        place_near(
            world,
            Kind::Mill,
            village + DVec2::new(-60.0, 120.0),
            30,
            true,
        );
        place_near(
            world,
            Kind::Bakery,
            village + DVec2::new(40.0, -60.0),
            20,
            true,
        );
        place_near(
            world,
            Kind::Bakery,
            village + DVec2::new(-50.0, -70.0),
            20,
            true,
        );
        for (a, r) in [(-1.35, 190.0), (1.4, 200.0)] {
            place_near(world, Kind::HopFarm, out(a, r), 40, true);
        }
        place_near(
            world,
            Kind::Brewery,
            village + DVec2::new(-90.0, -20.0),
            25,
            true,
        );
        woodcutters_near(world, 1, village, 1);
    }
    // --- Northmoor: sheep, timber and an airship mast.
    let n_harbour = harbour_toward(world, 3, isles[0].center);
    let mut n_mast = None;
    if let Some(nh) = n_harbour {
        let village = street_inland(world, nh, 14);
        street_grid(world, village, 80.0, 7, false);
        place_near(world, Kind::Market, village, 15, true);
        fill_houses(world, village, 90.0, 18, Tier::Fisherfolk);
        place_near(
            world,
            Kind::SheepFarm,
            village + DVec2::new(140.0, 60.0),
            40,
            true,
        );
        place_near(
            world,
            Kind::SheepFarm,
            village + DVec2::new(-140.0, 80.0),
            40,
            true,
        );
        place_near(
            world,
            Kind::Weaver,
            village + DVec2::new(60.0, 90.0),
            25,
            true,
        );
        for p in [DVec2::new(200.0, -120.0), DVec2::new(-220.0, -60.0)] {
            place_near(world, Kind::Woodcutter, village + p, 30, true);
        }
        place_near(
            world,
            Kind::Sawmill,
            village + DVec2::new(-90.0, -50.0),
            25,
            true,
        );
        if let Some(nhb) = world.get::<Building>(nh).cloned() {
            let c = grid::footprint_center(nhb.kind, nhb.origin, nhb.rot);
            let fd = grid::front_dir(nhb.rot).as_dvec2();
            let along = DVec2::new(fd.y, -fd.x);
            place_near(world, Kind::Fishery, c + along * 60.0, 30, false)
                .map(|e| join_to_roads(world, e));
        }
        n_mast = place_near(
            world,
            Kind::AirshipMast,
            village + DVec2::new(30.0, -110.0),
            40,
            true,
        );
    }
    // --- Skerryvale: a clay and hops outpost.
    let s_harbour = harbour_toward(world, 2, isles[0].center);
    if let Some(sh) = s_harbour {
        let village = street_inland(world, sh, 10);
        street_grid(world, village, 55.0, 7, false);
        fill_houses(world, village, 60.0, 10, Tier::Labourers);
        place_near(
            world,
            Kind::Market,
            village + DVec2::new(20.0, 20.0),
            15,
            true,
        );
        place_near(
            world,
            Kind::Chapel,
            village + DVec2::new(-20.0, 20.0),
            15,
            true,
        );
        place_near(
            world,
            Kind::ClayPit,
            village + DVec2::new(90.0, 30.0),
            30,
            true,
        );
        place_near(
            world,
            Kind::Brickworks,
            village + DVec2::new(50.0, -40.0),
            25,
            true,
        );
        place_near(
            world,
            Kind::HopFarm,
            village + DVec2::new(-100.0, 40.0),
            35,
            true,
        );
    }
    refresh_looks(world);
    // Every settled island starts with some stock.
    let ids = |world: &mut World, e: Option<Entity>| {
        e.and_then(|e| world.get::<Building>(e).map(|b| (b.id, b.island)))
    };
    for e in [Some(h_harbour), b_harbour, n_harbour, s_harbour]
        .into_iter()
        .flatten()
    {
        if let Some((_, isle)) = ids(world, Some(e)) {
            let mut econ = world.resource_mut::<Economy>();
            let books = &mut econ.islands[isle];
            books.capacity = books.harbours as f64 * crate::sim::HARBOUR_CAPACITY;
            for (g, n) in build::starting_stock() {
                books.add(g, n);
            }
            for g in [Good::Bread, Good::Ale, Good::Logs, Good::Wool, Good::Grain] {
                books.add(g, 25.0);
            }
        }
    }
    // --- Routes.
    let mut routes = Vec::new();
    let h_id = ids(world, Some(h_harbour)).map(|x| x.0);
    let b_id = ids(world, b_harbour).map(|x| x.0);
    let n_id = ids(world, n_harbour).map(|x| x.0);
    let s_id = ids(world, s_harbour).map(|x| x.0);
    if let (Some(h), Some(b)) = (h_id, b_id) {
        routes.push(Route {
            name: "Bread and ale run".into(),
            stops: vec![
                Stop {
                    at: b,
                    load: vec![Good::Bread, Good::Ale],
                    unload: vec![Good::Planks, Good::Clothes, Good::Fish],
                },
                Stop {
                    at: h,
                    load: vec![Good::Planks, Good::Clothes, Good::Fish],
                    unload: vec![Good::Bread, Good::Ale],
                },
            ],
            air: false,
        });
    }
    if let (Some(h), Some(n), Some(b)) = (h_id, n_id, b_id) {
        routes.push(Route {
            name: "Northern steam line".into(),
            stops: vec![
                Stop {
                    at: n,
                    load: vec![Good::Wool, Good::Planks, Good::Clothes],
                    unload: vec![Good::Bread, Good::Bricks],
                },
                Stop {
                    at: h,
                    load: vec![Good::Bricks, Good::Bread],
                    unload: vec![Good::Wool, Good::Planks, Good::Clothes],
                },
                Stop {
                    at: b,
                    load: vec![Good::Bread, Good::Ale, Good::Grain],
                    unload: vec![Good::Bricks, Good::Planks],
                },
            ],
            air: false,
        });
    }
    if let (Some(h), Some(s)) = (h_id, s_id) {
        routes.push(Route {
            name: "Skerry clay boat".into(),
            stops: vec![
                Stop {
                    at: s,
                    load: vec![Good::Bricks, Good::Hops],
                    unload: vec![Good::Fish, Good::Clothes, Good::Planks],
                },
                Stop {
                    at: h,
                    load: vec![Good::Fish, Good::Clothes, Good::Planks],
                    unload: vec![Good::Bricks, Good::Hops],
                },
            ],
            air: false,
        });
    }
    let mast_id = |world: &mut World, e: Option<Entity>| {
        e.and_then(|e| world.get::<Building>(e).map(|b| b.id))
    };
    let hm = mast_id(world, h_mast);
    let nm = mast_id(world, n_mast);
    if let (Some(h), Some(n)) = (hm, nm) {
        routes.push(Route {
            name: "Northmoor air freight".into(),
            stops: vec![
                Stop {
                    at: n,
                    load: vec![Good::Wool, Good::Clothes],
                    unload: vec![Good::Bricks],
                },
                Stop {
                    at: h,
                    load: vec![Good::Bricks],
                    unload: vec![Good::Wool, Good::Clothes],
                },
            ],
            air: true,
        });
    }
    world.insert_resource(Routes(routes.clone()));
    // --- Ships on the routes, spread round them.
    let crafts: [&[(Craft, &str)]; 4] = [
        &[
            (Craft::Schooner, "Kittiwake"),
            (Craft::Schooner, "Maud Ellen"),
            (Craft::Schooner, "Good Intent"),
            (Craft::PaddleSteamer, "PS Haverholm Belle"),
        ],
        &[
            (Craft::Steamer, "SS Brannock"),
            (Craft::Steamer, "SS Northern Star"),
        ],
        &[(Craft::Schooner, "Clay Pigeon")],
        &[(Craft::Airship, "Albatross"), (Craft::Airship, "Petrel")],
    ];
    for (r, route) in routes.iter().enumerate() {
        let list = if route.air {
            crafts[3]
        } else {
            crafts[r.min(2)]
        };
        for (k, (craft, name)) in list.iter().enumerate() {
            let stop = &route.stops[k % route.stops.len()];
            let Some(e) = build::by_id(world, stop.at) else {
                continue;
            };
            let b = world.get::<Building>(e).cloned().unwrap();
            let (at, heading) = if craft.flies() {
                let m = mast_point(&b);
                (m + DVec3::new(-48.0, 4.0, 0.0), 0.0)
            } else {
                let (p, h) = mooring(&b);
                let side = DVec2::new(h.cos() as f64, -h.sin() as f64) * (k as f64 * 40.0 - 20.0);
                (DVec3::new(p.x + side.x, 0.0, p.y + side.y), h)
            };
            let e = spawn_vessel(
                world,
                *craft,
                name,
                at,
                heading,
                Some(r),
                Roam::None,
                50 + r as u64 * 10 + k as u64,
            );
            if let Some(mut v) = world.get_mut::<crate::fleet::Vessel>(e) {
                v.next_stop = (k + 1) % route.stops.len();
                v.doing = crate::fleet::Doing::Moored(3.0 + k as f32 * 9.0);
            }
        }
    }
    crate::fleet::spawn_traffic(world);
    prune_roads(world);
    build::flush_paint(world);
    world.resource_mut::<Changed>().any = true;
    build::refresh(world);
    let n = world.query::<&Building>().iter(world).count();
    engine::core::log::info!(
        "archipelago start: {n} buildings, {} road cells in {:.2} s",
        world.resource::<Grid>().roads.len(),
        started.elapsed().as_secs_f32()
    );
}

/// Run the economy ahead (no carts, no drawing) so the start has full houses and stock.
pub fn prewarm(world: &mut World, seconds: f32) {
    let mut sys = bevy_ecs::system::SystemState::<(
        ResMut<Economy>,
        Query<(&Building, &mut House)>,
        Query<(
            Entity,
            &Building,
            &mut crate::sim::Producer,
            Option<&crate::sim::RoadLink>,
        )>,
        ResMut<crate::sim::Shipments>,
    )>::new(world);
    let mut t = 0.0;
    while t < seconds {
        let Ok((mut econ, mut houses, mut producers, mut ship)) = sys.get_mut(world) else {
            return;
        };
        crate::sim::step(1.0, &mut econ, &mut houses, &mut producers, &mut ship);
        // Deliver at once while warming up.
        let jobs = std::mem::take(&mut ship.carts);
        for (e, g, out) in jobs {
            if out && let Ok((_, b, _, _)) = producers.get(e) {
                let isle = b.island;
                econ.islands[isle].add(g, 1.0);
            }
        }
        t += 1.0;
    }
    sys.apply(world);
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::sim::{Producer, RoadLink, Status};

    /// The game's resources without a renderer, terrain or UI.
    pub fn bare_world() -> World {
        let mut w = World::new();
        w.init_resource::<Grid>();
        w.init_resource::<Economy>();
        w.init_resource::<crate::sim::Shipments>();
        w.init_resource::<Changed>();
        w.init_resource::<build::Forest>();
        w.init_resource::<crate::draw::Drawn>();
        w.init_resource::<crate::fleet::SeaChart>();
        w.init_resource::<Routes>();
        w.init_resource::<engine::world::SurfaceMaterials>();
        w.init_resource::<bevy_ecs::message::Messages<engine::world::terrain::TerrainEdit>>();
        w
    }

    #[test]
    fn the_start_connects_its_workshops_and_runs() {
        let mut w = bare_world();
        build_start(&mut w);
        let mut q = w.query::<(&Building, &Producer, &RoadLink)>();
        let total = q.iter(&w).count();
        let linked = q.iter(&w).filter(|(_, _, l)| l.path.is_some()).count();
        let unlinked: Vec<String> = q
            .iter(&w)
            .filter(|(_, _, l)| l.path.is_none())
            .map(|(b, _, _)| format!("{:?}@{}", b.kind, islands()[b.island].name))
            .collect();
        let mut kinds: Vec<String> = q
            .iter(&w)
            .map(|(b, _, _)| format!("{:?}@{}", b.kind, b.island))
            .collect();
        kinds.sort();
        assert!(total >= 30, "only {total} workshops: {kinds:?}");
        let mut tiers = [[0; 2]; 5];
        for (b, h) in w.query::<(&Building, &House)>().iter(&w) {
            tiers[b.island][h.tier.index()] += 1;
        }
        eprintln!("houses by island and tier at the start: {tiers:?}");
        // Every farm got most of the fields it wants.
        let mut farms = Vec::new();
        for (b, f) in w
            .query::<(&Building, Option<&crate::fields::Fields>)>()
            .iter(&w)
        {
            if crate::fields::is_farm(b.kind) {
                let have = f.map_or(0, |f| f.0.len());
                farms.push(format!(
                    "{:?}@{}: {have}/{}",
                    b.kind,
                    b.island,
                    crate::fields::wanted(b.kind)
                ));
                assert!(
                    have * 10 >= crate::fields::wanted(b.kind) * 6,
                    "{:?}@{} has only {have} field cells",
                    b.kind,
                    b.island
                );
            }
        }
        eprintln!("farm fields: {farms:?}");
        let plumes = w.query::<&crate::effects::Plume>().iter(&w).count();
        let riders = w.query::<&crate::effects::Riding>().iter(&w).count();
        let houses = w.query::<&House>().iter(&w).count();
        eprintln!("plumes {plumes} (houses {houses}), riders {riders}");
        assert!(plumes < houses + 20, "{plumes} plumes for {houses} houses");
        let masts = w
            .query::<&Building>()
            .iter(&w)
            .filter(|b| b.kind == Kind::AirshipMast)
            .count();
        assert_eq!(masts, 2, "airship masts");
        assert_eq!(w.resource::<Routes>().0.len(), 4, "routes");
        assert!(
            linked * 10 >= total * 9,
            "{linked} of {total} linked; not: {unlinked:?}"
        );
        prewarm(&mut w, 600.0);
        let mut q = w.query::<(&Building, &Producer)>();
        let working = q
            .iter(&w)
            .filter(|(_, p)| p.status == Status::Working)
            .count();
        let idle: Vec<String> = q
            .iter(&w)
            .filter(|(_, p)| p.status != Status::Working)
            .map(|(b, p)| format!("{:?}@{}: {}", b.kind, b.island, p.status.text()))
            .collect();
        let econ = w.resource::<Economy>();
        let pop: Vec<[f32; 2]> = econ.islands.iter().map(|b| b.population).collect();
        assert!(
            working * 4 >= total * 3,
            "{working} of {total} working: {idle:?}; people {pop:?}"
        );
        eprintln!(
            "{working} of {total} working; idle {idle:?}; people {pop:?}; coins {:.0}, balance {:.0}/min (taxes {:.0}, upkeep {:.0})",
            econ.coins, econ.balance_per_min, econ.taxes_per_min, econ.upkeep_per_min
        );
        for b in &econ.islands {
            eprintln!(
                "  stock {:?} supply {:?}",
                b.stock.iter().map(|x| x.round()).collect::<Vec<_>>(),
                b.supply
                    .iter()
                    .map(|x| (x * 100.0).round())
                    .collect::<Vec<_>>()
            );
        }
        let econ = w.resource::<Economy>();
        assert!(econ.islands[0].population.iter().sum::<f32>() > 200.0);
    }
}
