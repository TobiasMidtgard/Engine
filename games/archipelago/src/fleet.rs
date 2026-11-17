//! Ships and aircraft: trade routes between harbours and airship masts, sea lanes found with
//! A* over the sea bed, fishing smacks, free traders crossing the map, and mail planes.
//!
//! Ships ride the water module's waves (heave, pitch and roll from four samples round the
//! hull) and draw wakes through `WaterInteractor`. Everything moves in game time.

use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;

use engine::prelude::*;
use serde::Serialize;

use crate::defs::{CELL_M, Good, Kind};
use crate::draw::{Art, Body, Spin, spawn_model};
use crate::globe::{self, Frame};
use crate::grid;
use crate::island;
use crate::logistics::along;
use crate::sim::{Building, Economy};
use crate::vessels::Craft;

// ------------------------------------------------------------------------------ sea lanes

const NAV_CELL: f64 = 32.0;

/// Where ships may sail: for every region a grid of sea cells deep enough for a laden
/// freighter, and the ocean crossings between the regions.
#[derive(Resource)]
pub struct SeaChart {
    charts: Vec<Chart>,
    cache: HashMap<(IVec2, IVec2), Arc<Vec<DVec3>>>,
}

/// One region's waters, out to its reach.
struct Chart {
    /// Map point of the grid's middle (the region's sheet origin) and cells from it to an edge.
    origin: DVec2,
    half: i32,
    open: Vec<bool>,
    /// Extra cost of a cell (near shore), so lanes keep off the coasts.
    cost: Vec<f32>,
}

impl Chart {
    fn new(region: usize) -> Self {
        let origin = globe::sheet_origin(region);
        let half = (globe::regions()[region].reach_m / NAV_CELL).ceil() as i32;
        let n = (half * 2) as usize;
        let mut chart = Self {
            origin,
            half,
            open: Vec::new(),
            cost: Vec::new(),
        };
        let depth: Vec<f64> = engine_ui::demo::art::par_map(n, |z| {
            (0..n)
                .map(|x| {
                    let c = chart.center(IVec2::new(x as i32 - half, z as i32 - half));
                    island::height(c.x, c.y)
                })
                .collect::<Vec<_>>()
        })
        .into_iter()
        .flatten()
        .collect();
        let open: Vec<bool> = depth.iter().map(|&h| h < -4.5).collect();
        let mut cost = vec![0.0f32; n * n];
        for z in 0..n {
            for x in 0..n {
                let mut near = 0.0f32;
                for dz in -3i32..=3 {
                    for dx in -3i32..=3 {
                        let (xx, zz) = (x as i32 + dx, z as i32 + dz);
                        if xx < 0 || zz < 0 || xx >= n as i32 || zz >= n as i32 {
                            continue;
                        }
                        if !open[zz as usize * n + xx as usize] {
                            let d = ((dx * dx + dz * dz) as f32).sqrt();
                            near = near.max(4.0 - d);
                        }
                    }
                }
                cost[z * n + x] = near.max(0.0) * 1.5;
            }
        }
        chart.open = open;
        chart.cost = cost;
        chart
    }

    fn index(&self, c: IVec2) -> Option<usize> {
        let (x, z) = (c.x + self.half, c.y + self.half);
        let n = self.half * 2;
        (x >= 0 && z >= 0 && x < n && z < n).then(|| (z * n + x) as usize)
    }

    fn cell(&self, p: DVec2) -> IVec2 {
        let l = p - self.origin;
        IVec2::new(
            (l.x / NAV_CELL).floor() as i32,
            (l.y / NAV_CELL).floor() as i32,
        )
    }

    fn center(&self, c: IVec2) -> DVec2 {
        self.origin + DVec2::new((c.x as f64 + 0.5) * NAV_CELL, (c.y as f64 + 0.5) * NAV_CELL)
    }

    fn clear_line(&self, a: DVec2, b: DVec2) -> bool {
        let d = b - a;
        let steps = (d.length() / 8.0).ceil().max(1.0) as i32;
        (0..=steps).all(|i| {
            let p = a + d * (i as f64 / steps as f64);
            self.index(self.cell(p))
                .is_some_and(|i| self.open[i] && self.cost[i] < 3.0)
        })
    }

    /// A lane from `a` to `b` through this region's waters, smoothed.
    fn lane(&self, a: DVec2, b: DVec2) -> Option<Vec<DVec2>> {
        let (ca, cb) = (self.cell(a), self.cell(b));
        let n = self.half * 2;
        let (ia, ib) = (self.index(ca)?, self.index(cb)?);
        #[derive(PartialEq)]
        struct Node(f32, usize);
        impl Eq for Node {}
        impl PartialOrd for Node {
            fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(o))
            }
        }
        impl Ord for Node {
            fn cmp(&self, o: &Self) -> std::cmp::Ordering {
                o.0.total_cmp(&self.0)
            }
        }
        let half = self.half;
        let h = |i: usize| {
            let (x, z) = ((i as i32 % n) - cb.x - half, (i as i32 / n) - cb.y - half);
            ((x * x + z * z) as f32).sqrt()
        };
        let mut g = vec![f32::INFINITY; (n * n) as usize];
        let mut from = vec![usize::MAX; (n * n) as usize];
        let mut heap = BinaryHeap::new();
        g[ia] = 0.0;
        heap.push(Node(h(ia), ia));
        let mut found = false;
        while let Some(Node(_, i)) = heap.pop() {
            if i == ib {
                found = true;
                break;
            }
            let (x, z) = (i as i32 % n, i as i32 / n);
            for (dx, dz, w) in [
                (1, 0, 1.0f32),
                (-1, 0, 1.0),
                (0, 1, 1.0),
                (0, -1, 1.0),
                (1, 1, 1.414),
                (1, -1, 1.414),
                (-1, 1, 1.414),
                (-1, -1, 1.414),
            ] {
                let (xx, zz) = (x + dx, z + dz);
                if xx < 0 || zz < 0 || xx >= n || zz >= n {
                    continue;
                }
                let j = (zz * n + xx) as usize;
                // The ends may sit in harbour shallows.
                if !self.open[j] && j != ib {
                    continue;
                }
                let ng = g[i] + w * (1.0 + self.cost[j]);
                if ng < g[j] {
                    g[j] = ng;
                    from[j] = i;
                    heap.push(Node(ng + h(j), j));
                }
            }
        }
        if !found {
            return None;
        }
        let mut cells = vec![ib];
        let mut i = ib;
        while i != ia {
            i = from[i];
            cells.push(i);
        }
        cells.reverse();
        let pts: Vec<DVec2> = cells
            .iter()
            .map(|&i| self.center(IVec2::new(i as i32 % n - half, i as i32 / n - half)))
            .collect();
        // String-pull.
        let mut out = vec![a];
        let mut k = 0;
        while k < pts.len() - 1 {
            let mut j = pts.len() - 1;
            while j > k + 1 && !self.clear_line(*out.last().unwrap(), pts[j]) {
                j -= 1;
            }
            out.push(pts[j]);
            k = j;
        }
        out.push(b);
        Some(out)
    }
}

/// How far apart the points of an ocean crossing are laid, meters (a straight course over
/// the planet is a curve on a chart).
const ARC_STEP_M: f64 = 150.0;

/// The straight course over the planet from map point `a` to map point `b` (which may be on
/// different sheets), as one leg per sheet: the first charted on `a`'s sheet as far as half
/// way, the second on `b`'s. Heights run evenly from `a.y` to `b.y`. On one sheet it is a
/// single leg.
pub fn course(a: DVec3, b: DVec3) -> Vec<Vec<DVec3>> {
    let (ka, kb) = (globe::region_of(a.x, a.z), globe::region_of(b.x, b.z));
    if ka == kb {
        return vec![vec![a, b]];
    }
    let (da, db) = (globe::dir(a.x, a.z), globe::dir(b.x, b.z));
    let angle = da.dot(db).clamp(-1.0, 1.0).acos();
    let n = ((angle * globe::RADIUS_M / ARC_STEP_M).ceil() as usize).max(2);
    let at = |i: usize, k: usize| {
        let t = i as f64 / n as f64;
        let d = (da * ((1.0 - t) * angle).sin() + db * (t * angle).sin()).normalize();
        let mut m = globe::map_on(k, d);
        m.y = a.y + (b.y - a.y) * t;
        m
    };
    let mid = n / 2;
    vec![
        (0..=mid).map(|i| at(i, ka)).collect(),
        (mid..=n).map(|i| at(i, kb)).collect(),
    ]
}

impl SeaChart {
    pub fn new() -> Self {
        Self {
            charts: (0..globe::regions().len()).map(Chart::new).collect(),
            cache: HashMap::new(),
        }
    }

    /// Where ships leave region `k`'s waters for region `to`: on its rim, on the straight
    /// course between the two.
    fn gate(k: usize, to: usize) -> DVec2 {
        let o = globe::sheet_origin(k);
        let there = globe::map_on(k, globe::regions()[to].anchor());
        let bearing = (DVec2::new(there.x, there.z) - o).normalize();
        o + bearing * (globe::regions()[k].reach_m * 0.95)
    }

    /// A sea lane from `a` to `b` (both at sea, on one sheet), smoothed; `None` if the sea
    /// doesn't join them.
    pub fn lane(&mut self, a: DVec2, b: DVec2) -> Option<Arc<Vec<DVec3>>> {
        let k = globe::region_of(a.x, a.y);
        if k != globe::region_of(b.x, b.y) {
            return None;
        }
        let chart = &self.charts[k];
        let key = (
            chart.cell(a),
            chart.cell(b) + IVec2::splat(100_000 * (k as i32 + 1)),
        );
        if let Some(p) = self.cache.get(&key) {
            return Some(p.clone());
        }
        let out = chart.lane(a, b)?;
        let path: Arc<Vec<DVec3>> =
            Arc::new(out.into_iter().map(|p| DVec3::new(p.x, 0.0, p.y)).collect());
        self.cache.insert(key, path.clone());
        Some(path)
    }

    /// The whole way by sea from `a` to `b`, which may lie in different regions: one leg per
    /// sheet of the map (out through the home waters and half way over the ocean, then the
    /// other half and in through the far waters).
    pub fn voyage(&mut self, a: DVec2, b: DVec2) -> Option<Vec<Arc<Vec<DVec3>>>> {
        let (ka, kb) = (globe::region_of(a.x, a.y), globe::region_of(b.x, b.y));
        if ka == kb {
            return self.lane(a, b).map(|l| vec![l]);
        }
        let (ga, gb) = (Self::gate(ka, kb), Self::gate(kb, ka));
        let out = self.lane(a, ga)?;
        let inward = self.lane(gb, b)?;
        let sea = |p: DVec2| DVec3::new(p.x, 0.0, p.y);
        let mut legs = course(sea(ga), sea(gb));
        let mut first: Vec<DVec3> = out.as_ref().clone();
        first.extend(legs[0].drain(1..));
        let mut second = std::mem::take(&mut legs[1]);
        second.extend(inward.iter().skip(1).copied());
        Some(vec![Arc::new(first), Arc::new(second)])
    }
}

impl Default for SeaChart {
    fn default() -> Self {
        Self::new()
    }
}

// ------------------------------------------------------------------------------ routes

/// One stop of a route.
#[derive(Clone, Debug, Serialize)]
pub struct Stop {
    /// The harbour (or airship mast) building's id.
    pub at: u32,
    pub load: Vec<Good>,
    pub unload: Vec<Good>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Route {
    pub name: String,
    pub stops: Vec<Stop>,
    pub air: bool,
}

#[derive(Resource, Default, Debug, Serialize)]
pub struct Routes(pub Vec<Route>);

/// What a vessel is doing.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Doing {
    #[default]
    Sailing,
    /// Moored, loading and unloading, `s` seconds left.
    Moored(f32),
    /// Fishing or idling at sea, `s` seconds left.
    Lingering(f32),
}

/// A ship or aircraft and its orders.
#[derive(Component, Clone, Debug)]
pub struct Vessel {
    pub craft: Craft,
    pub name: String,
    /// Route index (trade) and which stop it is bound for.
    pub route: Option<usize>,
    pub next_stop: usize,
    /// Where it is: a map point (`y` above the sea), on the sheet its lane is charted on.
    pub at: DVec3,
    /// The lane it follows now.
    pub path: Arc<Vec<DVec3>>,
    pub lengths: Arc<Vec<f64>>,
    pub s: f64,
    /// The legs of its voyage still to come (an ocean crossing changes sheets half way).
    pub onward: Vec<Arc<Vec<DVec3>>>,
    pub doing: Doing,
    pub cargo: Vec<(Good, u32)>,
    /// Smoothed heading (radians, yaw of +X) and speed.
    pub heading: f32,
    pub speed: f32,
    /// Bank angle (aircraft), radians.
    pub bank: f32,
    /// Free traders and smacks: their own little orders.
    pub roam: Roam,
    pub seed: u64,
    /// Height above the sea to fly at (aircraft).
    pub altitude: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Roam {
    #[default]
    None,
    /// A smack working the water off its fishery's quay.
    Fishing { home: DVec2, ground: DVec2 },
    /// A barque crossing between far points of the map.
    Crossing { a: DVec2, b: DVec2 },
    /// A plane flying a loop through these points.
    Loop { points: Arc<Vec<DVec3>> },
    /// Off duty: lying at a harbour's mooring or an airship mast until given a route.
    Idle { at: DVec3 },
}

fn lengths(path: &[DVec3]) -> Arc<Vec<f64>> {
    let mut out = vec![0.0];
    let mut acc = 0.0;
    for w in path.windows(2) {
        acc += (w[1] - w[0]).length();
        out.push(acc);
    }
    Arc::new(out)
}

/// Where a ship moors at a harbour (at sea off the quay) and which way it lies.
pub fn mooring(b: &Building) -> (DVec2, f32) {
    let c = grid::footprint_center(b.kind, b.origin, b.rot);
    let f = grid::front_dir(b.rot).as_dvec2();
    let (_, d) = b.kind.footprint(b.rot);
    let reach = match b.kind {
        Kind::Harbour => 0.5 * 4.0 * CELL_M + 7.0,
        _ => 0.5 * d.max(3) as f64 * CELL_M + 4.0,
    };
    let p = c + f * reach;
    // Lie along the quay.
    let along = DVec2::new(f.y, -f.x);
    ((p), (-along.y).atan2(along.x) as f32)
}

/// Where an airship hangs at a mast: its nose at the cup, so its middle is 48 m out.
pub fn mast_point(b: &Building) -> DVec3 {
    let c = grid::footprint_center(b.kind, b.origin, b.rot);
    DVec3::new(c.x, b.base_y + 37.5, c.y)
}

pub(crate) fn building_by_id(world: &mut World, id: u32) -> Option<Building> {
    let mut q = world.query::<&Building>();
    q.iter(world).find(|b| b.id == id).cloned()
}

/// Spawn a vessel at `at` facing `heading` (its route's first leg is planned on the next
/// step).
#[allow(clippy::too_many_arguments)]
pub fn spawn_vessel(
    world: &mut World,
    craft: Craft,
    name: &str,
    at: DVec3,
    heading: f32,
    route: Option<usize>,
    roam: Roam,
    seed: u64,
) -> Entity {
    let t = world.resource::<Frame>().stand(at, heading);
    let mut parts = Vec::new();
    if let Some(art) = world.get_resource::<Art>().cloned() {
        parts = spawn_model(world, &art, &art.crafts[&craft].clone(), t, true);
    }
    let altitude = match craft {
        Craft::Airship => 170.0 + (seed % 5) as f32 * 12.0,
        Craft::Biplane => 260.0 + (seed % 7) as f32 * 20.0,
        Craft::Seaplane => 140.0 + (seed % 5) as f32 * 15.0,
        _ => 0.0,
    };
    let v = Vessel {
        craft,
        name: name.into(),
        route,
        next_stop: 0,
        at,
        path: Arc::new(vec![at, at]),
        lengths: Arc::new(vec![0.0, 0.0]),
        s: 0.0,
        onward: Vec::new(),
        doing: Doing::Moored(2.0 + (seed % 7) as f32),
        cargo: Vec::new(),
        heading,
        speed: 0.0,
        bank: 0.0,
        roam,
        seed,
        altitude,
    };
    let mut e = world.spawn((
        Name::new(format!("{}: {name}", craft.name())),
        t,
        v,
        Body { parts },
    ));
    if !craft.flies() {
        let beam = match craft {
            Craft::Steamer => 4.8,
            Craft::Barque => 4.5,
            Craft::Schooner => 3.5,
            Craft::Clipper => 5.2,
            Craft::PaddleSteamer => 6.6,
            _ => 1.8,
        };
        e.insert(engine_water::WaterInteractor::new(beam).with_length(craft.half_length()));
    }
    let id = e.id();
    crate::effects::add_riders(world, id, craft);
    id
}

/// Plan the next leg for vessels that finished one, and handle moorings.
pub fn plan_legs(world: &mut World) {
    let dt = world.resource::<Time>().delta_secs() * world.resource::<Economy>().speed;
    let mut todo: Vec<Entity> = Vec::new();
    {
        let mut q = world.query::<(Entity, &mut Vessel)>();
        for (e, mut v) in q.iter_mut(world) {
            match v.doing {
                Doing::Moored(ref mut s) | Doing::Lingering(ref mut s) => {
                    *s -= dt;
                    if *s <= 0.0 {
                        todo.push(e);
                    }
                }
                Doing::Sailing => {
                    if v.s >= *v.lengths.last().unwrap_or(&0.0) - 0.5 {
                        todo.push(e);
                    }
                }
            }
        }
    }
    for e in todo {
        let Some(mut v) = world.get::<Vessel>(e).cloned() else {
            continue;
        };
        let here = v.at;
        match v.doing.clone() {
            Doing::Sailing if !v.onward.is_empty() => {
                // The next sheet of the map: the same spot and course on the planet.
                let path = v.onward.remove(0);
                let start = path[0];
                v.heading = globe::carry_yaw(here, v.heading, start);
                v.at = DVec3::new(start.x, here.y, start.z);
                v.lengths = lengths(&path);
                v.path = path;
                v.s = 0.0;
            }
            Doing::Sailing => arrive(world, &mut v),
            _ => depart(world, &mut v, here),
        }
        if let Some(mut slot) = world.get_mut::<Vessel>(e) {
            *slot = v;
        }
    }
}

/// Reached the end of a leg.
fn arrive(world: &mut World, v: &mut Vessel) {
    match (&v.roam, v.route) {
        (_, Some(r)) => {
            let Some(route) = world
                .resource::<Routes>()
                .0
                .get(r)
                .filter(|r| !r.stops.is_empty())
                .cloned()
            else {
                v.doing = Doing::Lingering(5.0);
                return;
            };
            let stop = &route.stops[v.next_stop % route.stops.len()];
            if let Some(b) = building_by_id(world, stop.at) {
                trade(world, v, &b, stop);
            }
            v.next_stop = (v.next_stop + 1) % route.stops.len().max(1);
            v.doing = Doing::Moored(if v.craft.flies() { 14.0 } else { 18.0 });
        }
        (Roam::Fishing { home, .. }, None) => {
            let here = v.path.last().copied().unwrap_or_default();
            let at_home = (DVec2::new(here.x, here.z) - *home).length() < 30.0;
            v.doing = if at_home {
                Doing::Moored(20.0)
            } else {
                Doing::Lingering(25.0 + (v.seed % 13) as f32)
            };
        }
        (Roam::Idle { .. }, None) => v.doing = Doing::Moored(30.0),
        _ => v.doing = Doing::Lingering(if v.craft.flies() { 0.01 } else { 4.0 }),
    }
}

fn trade(world: &mut World, v: &mut Vessel, b: &Building, stop: &Stop) {
    let (slots, per) = v.craft.hold();
    let mut econ = world.resource_mut::<Economy>();
    exchange(
        &mut econ.islands[b.island],
        &mut v.cargo,
        &stop.load,
        &stop.unload,
        slots,
        per,
    );
}

/// What a warehouse takes of a good from a ship: above this the rest stays aboard for the
/// next call, so one island does not hoard what the whole route lives on.
pub const WAREHOUSE_WANTS: f64 = 40.0;
/// What a ship leaves an island of each good it loads.
pub const ISLAND_KEEPS: f64 = 8.0;

/// A ship's call at a quay: land what the stop takes (up to [`WAREHOUSE_WANTS`] in stock),
/// then load what it gives (down to [`ISLAND_KEEPS`]), `per` to a slot of the hold.
pub fn exchange(
    books: &mut crate::sim::IslandBooks,
    cargo: &mut Vec<(Good, u32)>,
    load: &[Good],
    unload: &[Good],
    slots: u32,
    per: u32,
) {
    for (g, n) in cargo.iter_mut() {
        if unload.contains(g) {
            let room = (WAREHOUSE_WANTS - books.get(*g)).floor().max(0.0) as u32;
            let k = (*n).min(room);
            books.add(*g, k as f64);
            *n -= k;
        }
    }
    cargo.retain(|(_, n)| *n > 0);
    for g in load {
        // (Never load what was just landed: a good both ways on one stop stays ashore.)
        if unload.contains(g) {
            continue;
        }
        let aboard = cargo.iter().position(|c| c.0 == *g);
        if aboard.is_none() && cargo.len() as u32 >= slots {
            continue;
        }
        let room = per - aboard.map_or(0, |i| cargo[i].1.min(per));
        let n = (books.get(*g) - ISLAND_KEEPS)
            .floor()
            .clamp(0.0, room as f64) as u32;
        if n == 0 {
            continue;
        }
        books.take(*g, n as f64);
        match aboard {
            Some(i) => cargo[i].1 += n,
            None => cargo.push((*g, n)),
        }
    }
}

/// Leave a mooring (or a fishing ground) for the next place.
fn depart(world: &mut World, v: &mut Vessel, here: DVec3) {
    let from = DVec2::new(here.x, here.z);
    let target: Option<DVec3> = match (&v.roam, v.route) {
        (_, Some(r)) => {
            let route = world.resource::<Routes>().0.get(r).cloned();
            let stop = route
                .as_ref()
                .and_then(|r| r.stops.get(v.next_stop % r.stops.len().max(1)).cloned());
            stop.and_then(|s| building_by_id(world, s.at)).map(|b| {
                if v.craft.flies() {
                    let m = mast_point(&b);
                    DVec3::new(m.x, m.y, m.z)
                } else {
                    let (p, _) = mooring(&b);
                    DVec3::new(p.x, 0.0, p.y)
                }
            })
        }
        (Roam::Fishing { home, ground }, None) => {
            let at_home = (from - *home).length() < 30.0;
            let jitter = DVec2::new(
                ((v.seed * 7 + v.s as u64) % 120) as f64 - 60.0,
                ((v.seed * 13 + v.s as u64) % 120) as f64 - 60.0,
            );
            Some(if at_home {
                DVec3::new(ground.x + jitter.x, 0.0, ground.y + jitter.y)
            } else {
                DVec3::new(home.x, 0.0, home.y)
            })
        }
        (Roam::Crossing { a, b }, None) => {
            // (The two ends may be on different sheets: compare over the planet.)
            let far = |p: &DVec2| globe::distance(here, DVec3::new(p.x, 0.0, p.y));
            let to = if far(a) < far(b) { *b } else { *a };
            Some(DVec3::new(to.x, 0.0, to.y))
        }
        (Roam::Loop { points }, None) => {
            let i = v.next_stop % points.len().max(1);
            v.next_stop += 1;
            points.get(i).copied()
        }
        (Roam::Idle { at }, None) => {
            if (DVec2::new(at.x, at.z) - from).length() < 30.0 {
                v.doing = Doing::Moored(30.0);
                return;
            }
            Some(*at)
        }
        (Roam::None, None) => None,
    };
    let Some(to) = target else {
        v.doing = Doing::Lingering(10.0);
        return;
    };
    let mut legs: Vec<Arc<Vec<DVec3>>> = if v.craft.flies() {
        let cruise = v.altitude as f64;
        // Climb out, cruise (straight over the planet), let down to the mast.
        let mut way = course(
            DVec3::new(here.x, cruise, here.z),
            DVec3::new(to.x, cruise, to.z),
        );
        let len: f64 = way
            .iter()
            .map(|l| lengths(l).last().copied().unwrap_or(0.0))
            .sum();
        let climb = (len * 0.2).min(500.0);
        let first = &mut way[0];
        let out = (first[1] - first[0]).normalize_or_zero();
        first[0] += out * climb;
        first.insert(0, here);
        let last = way.last_mut().unwrap();
        let n = last.len();
        let back = (last[n - 2] - last[n - 1]).normalize_or_zero();
        let over = last[n - 1];
        last[n - 1] = over + back * climb;
        if v.route.is_some() || matches!(v.roam, Roam::Idle { .. }) {
            // Nose to the mast: its middle ends 48 m short, just above the cup.
            last.push(DVec3::new(
                to.x + back.x * 48.0,
                to.y + 4.0,
                to.z + back.z * 48.0,
            ));
        } else {
            last.push(over);
        }
        way.into_iter().map(Arc::new).collect()
    } else {
        let chart = world.get_resource_mut::<SeaChart>();
        let way = chart.and_then(|mut c| c.voyage(from, DVec2::new(to.x, to.z)));
        match way {
            Some(l) => l,
            None => course(here, to).into_iter().map(Arc::new).collect(),
        }
    };
    let path = legs.remove(0);
    v.onward = legs;
    v.lengths = lengths(&path);
    v.path = path;
    v.s = 0.0;
    v.doing = Doing::Sailing;
}

/// Height of the sea's swell at a map point, meters: what hulls ride on. (The planet's sea
/// is the terrain's; its waves are drawn, not simulated.)
fn swell(x: f64, z: f64, t: f64) -> f64 {
    0.22 * (x * 0.045 + z * 0.012 + t * 0.55).sin()
        + 0.13 * (z * 0.081 - x * 0.027 - t * 0.83).sin()
        + 0.06 * ((x + z) * 0.19 + t * 1.3).sin()
}

/// Move every vessel along its lane: steer toward a point ahead, ride the waves, bank.
pub fn sail(world: &mut World) {
    let dt = world.resource::<Time>().delta_secs();
    let speed_k = world.resource::<Economy>().speed;
    let gdt = dt * speed_k;
    let t_now = world.resource::<Time>().elapsed_secs();
    let frame = *world.resource::<Frame>();
    let mut q = world.query::<(&mut Vessel, &mut Transform)>();
    for (mut v, mut t) in q.iter_mut(world) {
        let total = *v.lengths.last().unwrap_or(&0.0);
        let sailing = v.doing == Doing::Sailing;
        let target_speed = if sailing {
            // Ease off over the last stretch.
            let left = (total - v.s).max(0.0) as f32;
            v.craft.speed() * (left / (v.craft.half_length() * 4.0 + 20.0)).clamp(0.25, 1.0)
        } else {
            0.0
        };
        let accel = if v.craft.flies() { 2.0 } else { 0.8 };
        v.speed += (target_speed - v.speed).clamp(-accel * gdt * 2.0, accel * gdt);
        if sailing {
            v.s = (v.s + (v.speed * gdt) as f64).min(total);
        }
        let look = v.s + v.craft.half_length() as f64 * 2.5 + 15.0;
        let (p, _) = along(&v.path, &v.lengths, v.s);
        let (ahead, dir) = along(&v.path, &v.lengths, look.min(total));
        let mut to = ahead - p;
        if to.length_squared() < 1.0 {
            to = dir;
        }
        let want = if to.length_squared() > 1e-6 {
            (-to.z).atan2(to.x) as f32
        } else {
            v.heading
        };
        let mut dh = want - v.heading;
        while dh > std::f32::consts::PI {
            dh -= std::f32::consts::TAU;
        }
        while dh < -std::f32::consts::PI {
            dh += std::f32::consts::TAU;
        }
        let turn = if v.craft.flies() { 0.35 } else { 0.12 } * (0.3 + v.speed / v.craft.speed());
        let step = dh.clamp(-turn * gdt, turn * gdt);
        if sailing || v.speed > 0.2 {
            v.heading += step;
        }
        let turn_rate = if gdt > 0.0 { step / gdt } else { 0.0 };
        let fwd = Vec3::new(v.heading.cos(), 0.0, -v.heading.sin());
        // Level on the planet where it is, bows along its heading.
        let yaw = frame.facing(p.x, p.z, v.heading);
        if v.craft.flies() {
            let bank_want = (-turn_rate * v.speed / 9.81).atan().clamp(-0.6, 0.6);
            v.bank += (bank_want - v.bank) * (gdt * 1.5).min(1.0);
            let pos = DVec3::new(p.x, p.y, p.z);
            let bob = if v.craft == Craft::Airship {
                (t_now * 0.3 + v.seed as f32).sin() as f64 * 0.6
            } else {
                0.0
            };
            v.at = pos + DVec3::Y * bob;
            t.translation = frame.at(v.at);
            let pitch = if total > 0.0 {
                (dir.y as f32).asin()
            } else {
                0.0
            };
            t.rotation = yaw
                * Quat::from_rotation_z(pitch)
                * Quat::from_rotation_x(if v.craft == Craft::Airship {
                    v.bank * 0.15
                } else {
                    v.bank
                });
        } else {
            // Float on the waves.
            let side = Vec3::new(fwd.z, 0.0, -fwd.x);
            let hl = v.craft.half_length() * 0.7;
            let sample =
                |o: Vec3| -> f64 { swell(p.x + o.x as f64, p.z + o.z as f64, t_now as f64) };
            let (hb, hs) = (sample(fwd * hl), sample(-fwd * hl));
            let (hp, hst) = (sample(side * 2.5), sample(-side * 2.5));
            let mid = (hb + hs + hp + hst) * 0.25;
            let pitch = ((hb - hs) / (2.0 * hl as f64)).atan() as f32;
            let roll = ((hp - hst) / 5.0).atan() as f32 * 0.6;
            let heel = if v.craft.sails() {
                0.06 * (v.speed / v.craft.speed())
            } else {
                0.0
            };
            v.at = DVec3::new(p.x, mid, p.z);
            t.translation = frame.at(v.at);
            t.rotation = yaw
                * Quat::from_rotation_z(pitch)
                * Quat::from_rotation_x(roll + heel - turn_rate * 2.0);
        }
    }
}

/// Add each harbour's fishing smack the first time it is seen.
pub fn launch_smacks(world: &mut World) {
    let mut new = Vec::new();
    {
        let mut q = world.query::<(Entity, &Building)>();
        for (e, b) in q.iter(world) {
            if b.kind == Kind::Fishery && world.get::<HasSmack>(e).is_none() {
                new.push((e, b.clone()));
            }
        }
    }
    for (e, b) in new {
        world.entity_mut(e).insert(HasSmack);
        let (home, heading) = mooring(&b);
        let f = grid::front_dir(b.rot).as_dvec2();
        let mut ground = home + f * 260.0;
        // Fish where it's deep enough.
        let mut k = 0;
        while island::height(ground.x, ground.y) > -8.0 && k < 10 {
            ground += f * 60.0;
            k += 1;
        }
        spawn_vessel(
            world,
            Craft::Smack,
            &format!("Smack {}", b.id),
            DVec3::new(home.x, 0.0, home.y),
            heading,
            None,
            Roam::Fishing { home, ground },
            b.seed,
        );
    }
}

#[derive(Component)]
pub struct HasSmack;

/// Free traders and mail planes that come with the map.
pub fn spawn_traffic(world: &mut World) {
    let crossings = [
        (DVec2::new(-5200.0, 400.0), DVec2::new(5200.0, -300.0)),
        (DVec2::new(600.0, -5200.0), DVec2::new(-400.0, 5200.0)),
        (DVec2::new(-4600.0, -4200.0), DVec2::new(4400.0, 4300.0)),
        (DVec2::new(4800.0, -3800.0), DVec2::new(-4600.0, 3400.0)),
    ];
    for (i, (a, b)) in crossings.iter().enumerate() {
        let t = 0.2 + 0.18 * i as f64;
        let at = *a + (*b - *a) * t;
        let d = *b - *a;
        // Barques of the free traders, and clippers racing through on the long run.
        let (craft, name) = if i % 2 == 1 {
            (
                Craft::Clipper,
                ["Cutty Wren", "Sea Witch of Tarn"][i / 2].to_string(),
            )
        } else {
            (Craft::Barque, format!("Free trader {}", i / 2 + 1))
        };
        let e = spawn_vessel(
            world,
            craft,
            &name,
            DVec3::new(at.x, 0.0, at.y),
            (-d.y).atan2(d.x) as f32,
            None,
            Roam::Crossing { a: *a, b: *b },
            900 + i as u64,
        );
        // Start under way.
        if let Some(mut v) = world.get_mut::<Vessel>(e) {
            v.doing = Doing::Lingering(0.1);
            v.speed = v.craft.speed();
        }
    }
    // The mail planes and seaplanes fly the home isles.
    let isles: Vec<&island::Island> = island::islands()
        .iter()
        .filter(|i| i.region() == 0)
        .collect();
    for i in 0..3 {
        let pts: Vec<DVec3> = (0..isles.len())
            .map(|k| {
                let isle = &isles[(k * (i + 1) + i) % isles.len()];
                DVec3::new(isle.center.x, 0.0, isle.center.y)
            })
            .collect();
        let start = pts[0] + DVec3::new(0.0, 280.0, 0.0);
        let e = spawn_vessel(
            world,
            Craft::Biplane,
            &format!("Mail {}", i + 1),
            start,
            i as f32 * 2.0,
            None,
            Roam::Loop {
                points: Arc::new(pts),
            },
            700 + i as u64,
        );
        if let Some(mut v) = world.get_mut::<Vessel>(e) {
            v.doing = Doing::Lingering(0.1);
            v.speed = v.craft.speed();
        }
    }
    // Seaplanes hopping low between the home island, Brannsey and Little Tarn.
    for (i, order) in [[0usize, 1, 4], [0, 4, 3]].into_iter().enumerate() {
        let pts: Vec<DVec3> = order
            .iter()
            .map(|&k| DVec3::new(isles[k].center.x, 0.0, isles[k].center.y))
            .collect();
        let start = pts[i % pts.len()] + DVec3::new(300.0, 150.0, -200.0);
        let e = spawn_vessel(
            world,
            Craft::Seaplane,
            ["Kestrel", "Tern"][i],
            start,
            1.0 + i as f32 * 2.5,
            None,
            Roam::Loop {
                points: Arc::new(pts),
            },
            740 + i as u64,
        );
        if let Some(mut v) = world.get_mut::<Vessel>(e) {
            v.doing = Doing::Lingering(0.1);
            v.speed = v.craft.speed();
        }
    }
    far_traffic(world);
}

/// The far regions' own traffic: free traders working each coast, and a mail plane round
/// its settlements and islands.
fn far_traffic(world: &mut World) {
    for k in 1..globe::regions().len() {
        let region = &globe::regions()[k];
        let o = globe::sheet_origin(k);
        let r = region.reach_m * 0.8;
        let crossings = [
            (DVec2::new(-1.0, 0.12), DVec2::new(1.0, -0.14)),
            (DVec2::new(0.12, -1.0), DVec2::new(-0.1, 1.0)),
        ];
        for (i, (a, b)) in crossings.into_iter().enumerate() {
            let (a, b) = (o + a * r, o + b * r);
            // Near one end, well out at sea.
            let at = a + (b - a) * (0.03 + 0.03 * i as f64);
            let d = b - a;
            let (craft, name) = if i == 0 {
                (Craft::Barque, format!("{} coaster", region.name))
            } else {
                (Craft::Steamer, format!("SS {} Mail", region.name))
            };
            let e = spawn_vessel(
                world,
                craft,
                &name,
                DVec3::new(at.x, 0.0, at.y),
                (-d.y).atan2(d.x) as f32,
                None,
                Roam::Crossing { a, b },
                1000 + (k * 10 + i) as u64,
            );
            if let Some(mut v) = world.get_mut::<Vessel>(e) {
                v.doing = Doing::Lingering(0.1);
                v.speed = v.craft.speed();
            }
        }
        // Round the coasts (not over the continents' mountains).
        let pts: Vec<DVec3> = island::islands()
            .iter()
            .filter(|i| i.region() == k)
            .map(|i| island::settle_coast(i).unwrap_or(i.center))
            .map(|p| DVec3::new(p.x, 0.0, p.y))
            .collect();
        if pts.len() < 2 {
            continue;
        }
        let craft = if k == 1 {
            Craft::Seaplane
        } else {
            Craft::Biplane
        };
        let start = pts[0] + DVec3::new(400.0, 260.0, 300.0);
        let e = spawn_vessel(
            world,
            craft,
            &format!("{} mail plane", region.name),
            start,
            k as f32,
            None,
            Roam::Loop {
                points: Arc::new(pts),
            },
            1100 + k as u64,
        );
        if let Some(mut v) = world.get_mut::<Vessel>(e) {
            v.doing = Doing::Lingering(0.1);
            v.speed = v.craft.speed();
        }
    }
}

/// Give each new airship its four turning propellers and each paddle steamer its wheels.
pub fn spin_props(world: &mut World) {
    let mut add = Vec::new();
    let mut q = world.query_filtered::<(Entity, &Vessel), Without<PropsSet>>();
    for (e, v) in q.iter(world) {
        if matches!(v.craft, Craft::Airship | Craft::PaddleSteamer) {
            add.push((e, v.craft));
        }
    }
    let Some(art) = world.get_resource::<Art>().cloned() else {
        return;
    };
    for (e, craft) in add {
        if craft == Craft::PaddleSteamer {
            let base = Transform::from_translation(DVec3::new(-0.0, 1.3, 0.0));
            for p in spawn_model(world, &art, &art.paddles.clone(), base, true) {
                world.entity_mut(p).insert(Spin {
                    base,
                    at: DVec3::ZERO,
                    axis: Vec3::Z,
                    rate: -1.7,
                    angle: 0.0,
                    on: Some(e),
                    driven_by: None,
                });
            }
            world.entity_mut(e).insert(PropsSet);
            continue;
        }
        for (x, z) in [(-14.0, 7.5), (-14.0, -7.5), (5.0, 8.5), (5.0, -8.5)] {
            let base = Transform::from_translation(DVec3::new(x - 2.75, -10.5 * 0.65, z));
            for p in spawn_model(world, &art, &art.props.clone(), base, false) {
                world.entity_mut(p).insert(Spin {
                    base,
                    at: DVec3::ZERO,
                    axis: Vec3::X,
                    rate: 9.0,
                    angle: 0.0,
                    on: Some(e),
                    driven_by: None,
                });
            }
        }
        world.entity_mut(e).insert(PropsSet);
    }
}

#[derive(Component)]
pub struct PropsSet;
