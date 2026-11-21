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

/// [`place_near`] with the building's front turned toward `face` (a landmark looks at its
/// square, an office at its quay).
pub fn place_facing(
    world: &mut World,
    kind: Kind,
    target: DVec2,
    max_cells: i32,
    face: DVec2,
) -> Option<Entity> {
    let t = grid::cell_of(target.x, target.y);
    let (w, d) = kind.def().size;
    let half = IVec2::new(w / 2, d / 2);
    let want = (face - target).normalize_or_zero();
    let mut rots: Vec<u8> = (0..4).collect();
    rots.sort_by(|a, b| {
        let dot = |r: u8| grid::front_dir(r).as_dvec2().dot(want);
        dot(*b)
            .partial_cmp(&dot(*a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for r in 0..max_cells {
        for dz in -r..=r {
            for dx in -r..=r {
                if dx.abs() != r && dz.abs() != r {
                    continue;
                }
                // The best turn only: a landmark with its back to the square is no use.
                let rot = rots[0];
                let origin = t + IVec2::new(dx, dz) - half;
                if grid::check(world.resource::<Grid>(), kind, origin, rot).is_err() {
                    continue;
                }
                let Ok(e) = place(world, kind, origin, rot, false) else {
                    continue;
                };
                if join_to_roads(world, e) {
                    return Some(e);
                }
                build::demolish(world, e);
            }
        }
    }
    None
}

/// Put new items of these names into a building's slots, in order.
pub fn stock_slots(world: &mut World, e: Option<Entity>, names: &[&str]) {
    let Some(e) = e else { return };
    for (slot, name) in names.iter().enumerate() {
        let Some(i) = crate::items::ITEMS.iter().position(|d| d.name == *name) else {
            engine::core::log::warn!("archipelago: no item called {name}");
            continue;
        };
        let held = world.resource_mut::<crate::items::Items>().mint(i);
        if let Some(mut inst) = world.get_mut::<crate::items::Institution>(e) {
            inst.slots[slot] = Some(held);
        }
    }
}

/// Lay roads from a building's edge to the nearest road (or to `fallback` cells) over
/// buildable ground. False if no road is within reach.
pub fn join_to_roads(world: &mut World, e: Entity) -> bool {
    // (About 270 m over open ground: further than that is not "near".)
    join_within(world, e, 14_000)
}

/// [`join_to_roads`] for a works out in the country (a woodcutter in the woods): a track of
/// up to half a kilometre.
pub fn join_far(world: &mut World, e: Entity) -> bool {
    join_within(world, e, 90_000)
}

/// [`join_to_roads`], giving up once `cap` cells have been looked at.
fn join_within(world: &mut World, e: Entity, cap: usize) -> bool {
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
        if seen.len() > cap {
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
    let mut laid: Vec<IVec2> = Vec::new();
    for k in (-r..=r).step_by(block as usize) {
        for i in -r..=r {
            for p in [c + IVec2::new(i, k), c + IVec2::new(k, i)] {
                if inside(p)
                    && grid::cell_roadable(p)
                    && world.resource::<Grid>().is_free(p)
                    && place_road(world, p, false).is_ok()
                {
                    laid.push(p);
                }
            }
        }
    }
    // Take up the bits a cliff or a beach cut off from the rest: a building that joined one
    // of them would have a road at its door that leads nowhere.
    let roads = world.resource::<Grid>().roads.clone();
    let Some(seed) = roads
        .iter()
        .copied()
        .min_by_key(|p| ((*p - c).as_i64vec2().length_squared(), p.x, p.y))
    else {
        return;
    };
    let mut seen: HashSet<IVec2> = HashSet::from([seed]);
    let mut q = VecDeque::from([seed]);
    while let Some(p) = q.pop_front() {
        for n in [IVec2::X, IVec2::NEG_X, IVec2::Y, IVec2::NEG_Y] {
            if roads.contains(&(p + n)) && seen.insert(p + n) {
                q.push_back(p + n);
            }
        }
    }
    for p in laid {
        if !seen.contains(&p) {
            build::remove_road(world, p);
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
                    // (A town that starts grown starts settled in, too.)
                    h.happiness = 55.0;
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
    let stale: Vec<(Entity, Kind, Tier, u64, usize)> = q
        .iter(world)
        .filter(|(_, _, h, s)| s.is_none_or(|s| s.0 != h.tier))
        .map(|(e, b, h, _)| (e, b.kind, h.tier, b.seed, b.island))
        .collect();
    if stale.is_empty() {
        return;
    }
    for (e, kind, tier, seed, isle) in stale {
        world.entity_mut(e).insert((
            ShownTier(tier),
            crate::draw::Look(std::sync::Arc::new(crate::looks::look(
                kind,
                tier,
                seed,
                islands()[isle].biome(),
            ))),
        ));
    }
    world.resource_mut::<crate::draw::Drawn>().dirty = true;
}

pub fn refresh_looks(world: &mut World) {
    let mut q = world.query::<(&Building, &House, &mut crate::draw::Look)>();
    for (b, h, mut look) in q.iter_mut(world) {
        *look = crate::draw::Look(std::sync::Arc::new(crate::looks::look(
            b.kind,
            h.tier,
            b.seed,
            islands()[b.island].biome(),
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

/// The route that crosses the ocean at the start.
const OCEAN_ROUTE: &str = "Sarrowmere packet";

/// The street from a harbour's back to `to`.
pub(crate) fn street_inland(world: &mut World, harbour: Entity, len_cells: i32) -> DVec2 {
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
fn woodcutters_near(world: &mut World, isle: usize, town: DVec2, n: usize) -> usize {
    by_the_woods(world, Kind::Woodcutter, isle, town, n)
}

/// `n` of `kind` at the edges of the woods nearest `town` on `isle`, at least 90 m apart.
/// Returns how many were placed.
fn by_the_woods(world: &mut World, kind: Kind, isle: usize, town: DVec2, n: usize) -> usize {
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
    // (Woods no road can reach, across a cliff say, are given up after a few tries.)
    let mut failed: Vec<DVec2> = Vec::new();
    for p in spots {
        if cut.len() >= n || failed.len() >= 6 {
            break;
        }
        if cut.iter().any(|q| (*q - p).length() < 90.0)
            || failed.iter().any(|q| (*q - p).length() < 120.0)
        {
            continue;
        }
        // (A forest track may run half a kilometre to the first of them.)
        match place_near(world, kind, p, 4, false) {
            Some(e) if join_within(world, e, 90_000) => cut.push(p),
            Some(e) => {
                build::demolish(world, e);
                failed.push(p);
            }
            None => failed.push(p),
        }
    }
    cut.len()
}

/// The harbours and masts of the far regions' settlements.
#[derive(Default)]
struct FarLands {
    sarrow: Option<Entity>,
    palm: Option<Entity>,
    amber: Option<Entity>,
    salt: Option<Entity>,
    frost: Option<Entity>,
    rock: Option<Entity>,
    sarrow_mast: Option<Entity>,
    amber_mast: Option<Entity>,
    frost_mast: Option<Entity>,
}

/// Where a far land's first town goes: the point on the shore for its harbour and the
/// middle of the town behind it.
///
/// Usually that is where the land's settlement meets the sea ([`island::settle_coast`]) and
/// a little inland of it. Where that shore is hemmed in by cliffs or hills, the flattest
/// ground by the sea within a kilometre is taken instead.
pub fn far_site(isle: usize) -> Option<(DVec2, DVec2)> {
    static SITES: std::sync::OnceLock<Vec<Option<(DVec2, DVec2)>>> = std::sync::OnceLock::new();
    SITES.get_or_init(|| {
        engine_ui::demo::art::par_map(islands().len(), |i| {
            (islands()[i].region() != 0)
                .then(|| find_far_site(i))
                .flatten()
        })
    })[isle]
}

fn find_far_site(isle: usize) -> Option<(DVec2, DVec2)> {
    let land = &islands()[isle];
    let coast = island::settle_coast(land)?;
    let spot = island::settle_spot(land)?;
    let dir = (spot - land.center).normalize();
    // Ground sampled every third cell over a box round the usual shore.
    const STEP: f64 = 12.0;
    let half = (1000.0 / STEP) as i32;
    let n = (half * 2 + 1) as usize;
    let at = |i: i32, j: i32| coast + DVec2::new(i as f64, j as f64) * STEP;
    let mut flat = vec![0u32; n * n];
    for j in -half..=half {
        for i in -half..=half {
            let p = at(i, j);
            let k = (j + half) as usize * n + (i + half) as usize;
            let c = grid::cell_of(p.x, p.y);
            flat[k] = (island::island_at(p.x, p.y) == Some(isle) && grid::cell_buildable(c)) as u32;
        }
    }
    // Sums of `flat` over boxes, by a summed-area table.
    let mut sum = vec![0u32; (n + 1) * (n + 1)];
    for j in 0..n {
        for i in 0..n {
            sum[(j + 1) * (n + 1) + i + 1] =
                flat[j * n + i] + sum[j * (n + 1) + i + 1] + sum[(j + 1) * (n + 1) + i]
                    - sum[j * (n + 1) + i];
        }
    }
    let reach = (130.0 / STEP) as i32;
    let flat_round = |i: i32, j: i32| {
        let (i0, i1) = (
            (i - reach + half).clamp(0, n as i32) as usize,
            (i + reach + 1 + half).clamp(0, n as i32) as usize,
        );
        let (j0, j1) = (
            (j - reach + half).clamp(0, n as i32) as usize,
            (j + reach + 1 + half).clamp(0, n as i32) as usize,
        );
        (sum[j1 * (n + 1) + i1] + sum[j0 * (n + 1) + i0]) as f64
            - (sum[j0 * (n + 1) + i1] + sum[j1 * (n + 1) + i0]) as f64
    };
    // Where a harbour could stand: shore cells where a quay fits in some facing.
    let empty = Grid::default();
    let (hw, hd) = Kind::Harbour.def().size;
    let mut quays: Vec<DVec2> = Vec::new();
    for j in -half..=half {
        for i in -half..=half {
            let p = at(i, j);
            if !(-3.0..=3.0).contains(&island::height(p.x, p.y))
                || island::nearest_island(p.x, p.y) != isle
            {
                continue;
            }
            let origin = grid::cell_of(p.x, p.y) - IVec2::new(hw / 2, hd / 2);
            if (0..4).any(|rot| grid::check(&empty, Kind::Harbour, origin, rot).is_ok()) {
                quays.push(p);
            }
        }
    }
    // A site's worth: flat ground all round, less the walk to the nearest quay.
    let worth = |p: DVec2| -> Option<(f64, DVec2)> {
        let quay = quays
            .iter()
            .copied()
            .min_by(|a, b| a.distance_squared(p).total_cmp(&b.distance_squared(p)))?;
        let walk = quay.distance(p);
        let (i, j) = (
            ((p.x - coast.x) / STEP).round() as i32,
            ((p.y - coast.y) / STEP).round() as i32,
        );
        (walk < 220.0).then(|| (flat_round(i, j) - walk * 0.4, quay))
    };
    // The usual site: a little inland of the shore.
    let usual = coast - dir * 60.0;
    let usual_worth = worth(usual).map_or(f64::MIN, |w| w.0);
    // The best: as good as can be had without going far from the usual shore.
    let mut best: Option<(f64, DVec2, DVec2)> = None;
    for j in (-half + reach..=half - reach).step_by(3) {
        for i in (-half + reach..=half - reach).step_by(3) {
            if flat[(j + half) as usize * n + (i + half) as usize] == 0 {
                continue;
            }
            let p = at(i, j);
            let Some((w, quay)) = worth(p) else {
                continue;
            };
            let score = w - (p - coast).length() * 0.12;
            if best.is_none_or(|b| score > b.0) {
                best = Some((score, quay, p));
            }
        }
    }
    match best {
        Some((score, quay, p)) if usual_worth < score * 0.6 => {
            engine::core::log::info!(
                "archipelago: {}'s town goes {:.0} m along the coast, to flatter ground",
                land.name,
                (p - usual).length()
            );
            Some((quay, p))
        }
        _ => Some((coast, usual)),
    }
}

/// The first town of a far land: a harbour on the shore ([`far_site`]), a grid of lanes, a
/// market and fisher houses. Returns the harbour and the middle.
fn far_town(
    world: &mut World,
    isle: usize,
    street: i32,
    radius: f64,
    houses: usize,
    burghers: usize,
) -> Option<(Entity, DVec2)> {
    let (coast, middle) = far_site(isle)?;
    let Some(h) = place_near(world, Kind::Harbour, coast, 40, false) else {
        engine::core::log::warn!("archipelago: no harbour site on {}", islands()[isle].name);
        return None;
    };
    let usual = island::settle_coast(&islands()[isle]) == Some(coast);
    let village = if usual {
        // The street runs straight inland from the quay.
        street_inland(world, h, street)
    } else {
        middle
    };
    street_grid(world, village, radius, 7, false);
    if !usual && !join_within(world, h, 90_000) {
        engine::core::log::warn!(
            "archipelago: no way from the quay to the town on {}",
            islands()[isle].name
        );
    }
    place_near(world, Kind::Market, village, 15, true);
    if burghers > 0 {
        // The merchants' quarter: a school, a tavern, a paved middle and the first big houses.
        place_near(
            world,
            Kind::School,
            village + DVec2::new(36.0, 30.0),
            15,
            true,
        );
        place_near(
            world,
            Kind::Tavern,
            village + DVec2::new(-36.0, 28.0),
            15,
            true,
        );
        // A patrician's house and two masters' among them.
        fill_houses(world, village, 45.0, 1, Tier::Patricians);
        fill_houses(world, village, 50.0, 2, Tier::Masters);
        fill_houses(world, village, 55.0, burghers, Tier::Burghers);
    }
    fill_houses(world, village, radius + 25.0, houses, Tier::Fisherfolk);
    if burghers > 0 {
        build::pave_round(world, village, 62.0);
    }
    Some((h, village))
}

/// `kind` (a coast building) along the shore either side of a harbour, `step` meters apart.
fn along_shore(world: &mut World, harbour: Entity, kind: Kind, n: usize, step: f64) {
    let Some(b) = world.get::<Building>(harbour).cloned() else {
        return;
    };
    let c = grid::footprint_center(b.kind, b.origin, b.rot);
    let fd = grid::front_dir(b.rot).as_dvec2();
    let along = DVec2::new(fd.y, -fd.x);
    let mut placed = 0;
    for k in [-1.0, 1.0, -2.0, 2.0, -3.0, 3.0, -4.0, 4.0] {
        if placed >= n {
            break;
        }
        if let Some(e) = place_near(world, kind, c + along * (step * k), 14, false) {
            join_to_roads(world, e);
            placed += 1;
        }
    }
}

/// Settle the far regions: on each continent a town that makes what only its climate
/// gives, on a nearby island an outpost that feeds it.
fn far_lands(world: &mut World) -> FarLands {
    let isles = islands();
    let mut out = FarLands::default();
    // Places round a town on its inland side: `a` radians off the way inland, `r` meters out.
    let ring = |isle: usize, town: DVec2| {
        let inland = (isles[isle].center - town).normalize();
        move |a: f64, r: f64| town + DVec2::from_angle(a).rotate(inland) * r
    };
    // --- Sarrowmere: stilt houses on the jungle coast, cane and rum.
    if let Some((h, town)) = far_town(world, 5, 14, 85.0, 44, 5) {
        out.sarrow = Some(h);
        let at = ring(5, town);
        place_near(world, Kind::Chapel, at(2.2, 70.0), 20, true);
        along_shore(world, h, Kind::Fishery, 2, 75.0);
        woodcutters_near(world, 5, town, 3);
        place_near(world, Kind::Sawmill, at(1.9, 120.0), 30, true);
        for (a, r) in [(-0.75, 250.0), (0.05, 330.0), (0.8, 260.0)] {
            place_near(world, Kind::CanePlantation, at(a, r), 40, true);
        }
        for (a, r) in [(-1.35, 150.0), (1.25, 165.0)] {
            place_near(world, Kind::Distillery, at(a, r), 30, true);
        }
        out.sarrow_mast = place_near(world, Kind::AirshipMast, at(-1.9, 150.0), 40, true);
    }
    if let Some((h, town)) = far_town(world, 6, 9, 50.0, 14, 0) {
        out.palm = Some(h);
        let at = ring(6, town);
        along_shore(world, h, Kind::Fishery, 1, 70.0);
        place_near(world, Kind::CanePlantation, at(0.2, 190.0), 40, true);
        woodcutters_near(world, 6, town, 1);
    }
    // --- The Amber Reach: mud-brick towns, dates under the palms, salt from the shore.
    if let Some((h, town)) = far_town(world, 9, 14, 85.0, 40, 5) {
        out.amber = Some(h);
        let at = ring(9, town);
        place_near(world, Kind::Chapel, at(2.2, 70.0), 20, true);
        along_shore(world, h, Kind::Fishery, 2, 75.0);
        for (a, r) in [(-0.8, 230.0), (0.75, 240.0)] {
            place_near(world, Kind::DateGrove, at(a, r), 40, true);
        }
        place_near(world, Kind::SheepFarm, at(0.0, 330.0), 40, true);
        place_near(world, Kind::Weaver, at(-1.5, 140.0), 30, true);
        out.amber_mast = place_near(world, Kind::AirshipMast, at(1.7, 160.0), 40, true);
    }
    if let Some((h, town)) = far_town(world, 10, 9, 50.0, 16, 0) {
        out.salt = Some(h);
        let at = ring(10, town);
        along_shore(world, h, Kind::Fishery, 1, 70.0);
        for (a, r) in [(-1.25, 170.0), (1.25, 170.0)] {
            place_near(world, Kind::SaltWorks, at(a, r), 40, true);
        }
        place_near(world, Kind::DateGrove, at(0.0, 210.0), 40, true);
    }
    // --- Frostholm: cabins under snow, furs from the spruce woods, oil from the ice edge.
    if let Some((h, town)) = far_town(world, 12, 14, 85.0, 38, 5) {
        out.frost = Some(h);
        let at = ring(12, town);
        place_near(world, Kind::Chapel, at(2.2, 70.0), 20, true);
        along_shore(world, h, Kind::Fishery, 2, 75.0);
        along_shore(world, h, Kind::OilWorks, 1, 150.0);
        // (Where the woods are up in the hills, out of a road's reach, planks come by sea.)
        if woodcutters_near(world, 12, town, 2) > 0 {
            place_near(world, Kind::Sawmill, at(1.8, 120.0), 30, true);
        }
        // Trappers out in the snow, clear of the town.
        for a in [0.6, 2.6, 4.4] {
            place_near(world, Kind::TrapperLodge, at(a, 190.0), 30, true);
        }
        out.frost_mast = place_near(world, Kind::AirshipMast, at(-1.8, 150.0), 40, true);
    }
    if let Some((h, _)) = far_town(world, 13, 9, 50.0, 14, 0) {
        out.rock = Some(h);
        along_shore(world, h, Kind::Fishery, 1, 70.0);
        along_shore(world, h, Kind::OilWorks, 2, 130.0);
    }
    out
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
    // The Town Hall looks across the market toward the sea; the Harbour Office stands
    // behind the quay.
    let hb = world.get::<Building>(h_harbour).cloned().unwrap();
    let hc = grid::footprint_center(hb.kind, hb.origin, hb.rot);
    let seaward = (hc - town).normalize_or_zero();
    let hall = place_facing(world, Kind::TownHall, town - seaward * 34.0, 12, town);
    let office = place_facing(
        world,
        Kind::HarbourOffice,
        hc - seaward * 36.0 + DVec2::new(seaward.y, -seaward.x) * 22.0,
        14,
        hc,
    );
    stock_slots(world, hall, &["Town crier", "Baker's round", "Town band"]);
    stock_slots(
        world,
        office,
        &["Stevedore gang", "Harbour pilot", "Noon gun"],
    );
    // The ship chandler beside the quay puts up lamp oil, spares, bunting and coal.
    place_near(
        world,
        Kind::Chandler,
        hc - seaward * 30.0 - DVec2::new(seaward.y, -seaward.x) * 26.0,
        14,
        true,
    );
    place_near(
        world,
        Kind::Chapel,
        town + DVec2::new(60.0, -40.0),
        25,
        true,
    );
    // What Burghers ask for, and Burghers round the market.
    place_near(
        world,
        Kind::School,
        town + DVec2::new(-58.0, -46.0),
        25,
        true,
    );
    place_near(
        world,
        Kind::Tavern,
        town + DVec2::new(-52.0, 44.0),
        25,
        true,
    );
    // The three that answer fires, riots and sickness.
    for (kind, off) in [
        (Kind::FireWatch, DVec2::new(84.0, 30.0)),
        (Kind::WatchHouse, DVec2::new(-92.0, -8.0)),
        (Kind::Infirmary, DVec2::new(10.0, 96.0)),
    ] {
        place_near(world, kind, town + off, 30, true);
    }
    // The richest at the heart: Patricians and Masters, with their theatre and bank.
    place_near(
        world,
        Kind::Theatre,
        town + DVec2::new(-70.0, 60.0),
        30,
        true,
    );
    place_near(world, Kind::Bank, town + DVec2::new(70.0, 64.0), 30, true);
    let core = fill_houses(world, town, 75.0, 8, Tier::Burghers);
    fill_houses(world, town, 90.0, 3, Tier::Patricians);
    fill_houses(world, town, 95.0, 4, Tier::Masters);
    let inner = fill_houses(world, town, 100.0, 16, Tier::Labourers);
    let outer = fill_houses(world, town, 190.0, 44, Tier::Fisherfolk);
    let _ = (core, inner, outer);
    let along = DVec2::new(
        grid::front_dir(hb.rot).y as f64,
        -grid::front_dir(hb.rot).x as f64,
    );
    let mut boats = 0;
    for k in [-1.0, 1.0, -2.0, 2.0, -3.0, 3.0] {
        if boats < 5
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
    place_near(world, Kind::Weaver, toward(2.65, 235.0), 30, true);
    place_near(world, Kind::SheepFarm, toward(2.9, 300.0), 40, true);
    place_near(world, Kind::SheepFarm, toward(3.4, 320.0), 40, true);
    place_near(world, Kind::ClayPit, toward(4.2, 260.0), 40, true);
    place_near(world, Kind::Brickworks, toward(4.5, 230.0), 30, true);
    let h_mast = place_near(world, Kind::AirshipMast, toward(5.4, 240.0), 40, true);
    // The manufactories: glass from sand, and what the packets land made into Burghers' goods.
    place_near(world, Kind::Glassworks, toward(1.5, 250.0), 35, true);
    place_near(world, Kind::Saltery, toward(0.25, 205.0), 30, true);
    place_near(world, Kind::Furrier, toward(2.0, 255.0), 30, true);
    place_near(world, Kind::LampMaker, toward(1.15, 205.0), 30, true);
    // And the Masters' works: iron, coffee, clocks, chocolate, gowns and jewellery.
    place_near(world, Kind::Ironworks, toward(4.9, 300.0), 40, true);
    place_near(world, Kind::Roastery, toward(0.4, 260.0), 35, true);
    place_near(world, Kind::Clockmaker, toward(1.3, 160.0), 30, true);
    place_near(world, Kind::Chocolatier, toward(0.8, 170.0), 30, true);
    place_near(world, Kind::Couturier, toward(1.9, 180.0), 30, true);
    place_near(world, Kind::Goldsmith, toward(2.3, 170.0), 30, true);
    // The Trading Union among the manufactories: glass, lamps, furs, a weaver and a sawmill
    // are in its reach.
    let union = place_facing(world, Kind::TradingUnion, toward(1.65, 205.0), 16, town);
    stock_slots(
        world,
        union,
        &["Flying-shuttle loom", "Wick-trimming jig", "Rush order"],
    );
    // What the company has on the shelf, and the pedlar lying off the quay with his stock.
    {
        let mut items = world.resource_mut::<crate::items::Items>();
        for name in [
            "Fishwives' stalls",
            "Quay lights",
            "Steam saw",
            "Works foreman",
            "Journeyman miller",
            "Barrel of lamp oil",
            "Fireworks",
            "Spare gears",
            "Copper sheathing",
            "Chronometer",
            "Captain Jory Pell (lucky)",
            "Crowd on all sail",
        ] {
            if let Some(i) = crate::items::ITEMS.iter().position(|d| d.name == name) {
                items.gain(i);
            }
        }
    }
    pedlar_calls(world, h_harbour);
    // `ARCH_FLEET_REVIEW=1`: one of every craft lying in a line off the home quay, for the
    // fleet picture (tour view `fleet`). They never sail.
    if std::env::var_os("ARCH_FLEET_REVIEW").is_some() {
        fleet_review(world, &hb);
    }
    // --- Brannsey: grain, flour, bread, hops and ale.
    let b_harbour = harbour_toward(world, 1, isles[0].center);
    if let Some(bh) = b_harbour {
        let village = street_inland(world, bh, 16);
        street_grid(world, village, 135.0, 7, false);
        place_near(world, Kind::Market, village, 15, true);
        place_near(
            world,
            Kind::Chapel,
            village + DVec2::new(-30.0, 30.0),
            20,
            true,
        );
        // (Fisherfolk work the grain and hop farms, Labourers the mills, ovens and brewery.)
        fill_houses(world, village, 140.0, 36, Tier::Labourers);
        fill_houses(world, village, 150.0, 18, Tier::Fisherfolk);
        // Farms out on the open land behind the village, where their fields have room.
        let inland = (isles[1].center - village).normalize();
        let out = |a: f64, r: f64| village + DVec2::from_angle(a).rotate(inland) * r;
        for (a, r) in [
            (-0.8, 240.0),
            (0.05, 300.0),
            (0.85, 250.0),
            (-0.4, 380.0),
            (0.5, 390.0),
        ] {
            place_near(world, Kind::GrainFarm, out(a, r), 40, true);
        }
        // (Brannsey bakes for all the home isles: four mills, four bakeries.)
        for (a, r) in [(-0.35, 175.0), (0.45, 185.0), (1.1, 170.0)] {
            place_near(world, Kind::Mill, out(a, r), 40, true);
        }
        for d in [DVec2::new(75.0, -35.0), DVec2::new(-20.0, -100.0)] {
            place_near(world, Kind::Bakery, village + d, 45, true);
        }
        place_near(world, Kind::Brewery, out(-1.0, 165.0), 40, true);
        place_near(world, Kind::HopFarm, out(0.0, 190.0), 40, true);
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
        // (A fifth mill: the master baker's ovens take a third more flour.)
        place_near(
            world,
            Kind::Mill,
            village + DVec2::new(70.0, -120.0),
            30,
            true,
        );
        // A second Trading Union among the bakeries: the master baker and the mash tun.
        let union = place_facing(
            world,
            Kind::TradingUnion,
            village + DVec2::new(10.0, -62.0),
            16,
            village,
        );
        stock_slots(
            world,
            union,
            &["Master baker Odalys Brandt", "Copper mash tun"],
        );
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
    // Brannsey has the flour: its confectioner bakes the dates the steamers bring.
    let brannsey_market = {
        let mut q = world.query::<&Building>();
        q.iter(world)
            .filter(|b| b.kind == Kind::Market && b.island == 1)
            .min_by_key(|b| b.id)
            .map(|b| grid::footprint_center(b.kind, b.origin, b.rot))
    };
    if let Some(m) = brannsey_market {
        place_near(
            world,
            Kind::Confectioner,
            m + DVec2::new(45.0, 35.0),
            25,
            true,
        );
        build::pave_round(world, m, 60.0);
    }
    // Haverholm's centre is paved; villages and the far towns keep their earth streets.
    build::pave_round(world, town, 118.0);
    // --- The far regions: a first town and an outpost in each (see `far_town`).
    // (ARCH_NO_FAR leaves their shores empty, for "before" pictures.)
    let far = if std::env::var_os("ARCH_NO_FAR").is_some() {
        FarLands::default()
    } else {
        far_lands(world)
    };
    refresh_looks(world);
    // Every settled island starts with some stock.
    let ids = |world: &mut World, e: Option<Entity>| {
        e.and_then(|e| world.get::<Building>(e).map(|b| (b.id, b.island)))
    };
    let far_harbours = [
        far.sarrow, far.palm, far.amber, far.salt, far.frost, far.rock,
    ];
    for (k, e) in [Some(h_harbour), b_harbour, n_harbour, s_harbour]
        .into_iter()
        .chain(far_harbours)
        .enumerate()
    {
        let Some((_, isle)) = ids(world, e) else {
            continue;
        };
        let mut econ = world.resource_mut::<Economy>();
        let books = &mut econ.islands[isle];
        books.capacity = books.harbours as f64 * crate::sim::HARBOUR_CAPACITY;
        for (g, n) in build::starting_stock() {
            books.add(g, n);
        }
        for g in [Good::Bread, Good::Ale, Good::Logs, Good::Wool, Good::Grain] {
            books.add(g, 25.0);
        }
        if k >= 4 {
            // Far from the weavers of home.
            books.add(Good::Clothes, 40.0);
            books.add(Good::Fish, 20.0);
            if matches!(isle, 5 | 9 | 12) {
                // What the merchants' quarter brought out with it.
                books.add(Good::SaltFish, 12.0);
                books.add(Good::Coats, 12.0);
            }
        } else {
            // The last packets left a little of everything from overseas.
            for g in Good::ALL.into_iter().filter(|g| g.overseas()) {
                // (Haverholm's manufactories work them up; the next packet is weeks out.)
                books.add(g, if k == 0 { 40.0 } else { 14.0 });
            }
            if k == 0 {
                // Haverholm's manufactories have been at work.
                for (g, n) in [
                    (Good::Glass, 14.0),
                    (Good::SaltFish, 14.0),
                    (Good::DateCakes, 10.0),
                    (Good::Coats, 12.0),
                    (Good::Lamps, 8.0),
                ] {
                    books.add(g, n);
                }
            }
        }
    }
    // --- Routes, each with the ships that sail it.
    type Line = (Route, &'static [(Craft, &'static str)]);
    let mut lines: Vec<Line> = Vec::new();
    let id = |world: &mut World, e: Option<Entity>| ids(world, e).map(|x| x.0);
    let h_id = id(world, Some(h_harbour));
    let b_id = id(world, b_harbour);
    let n_id = id(world, n_harbour);
    let s_id = id(world, s_harbour);
    let treats = [Good::Rum, Good::Salt, Good::Dates, Good::Furs, Good::Oil];
    let pair = |name: &str, a: (u32, &[Good]), b: (u32, &[Good]), air: bool| Route {
        name: name.into(),
        stops: vec![
            Stop {
                at: a.0,
                load: a.1.to_vec(),
                unload: b.1.to_vec(),
            },
            Stop {
                at: b.0,
                load: b.1.to_vec(),
                unload: a.1.to_vec(),
            },
        ],
        air,
        company: crate::sim::PLAYER,
    };
    if let (Some(h), Some(b)) = (h_id, b_id) {
        lines.push((
            pair(
                "Bread and ale run",
                (b, &[Good::Bread, Good::Ale, Good::DateCakes]),
                (h, &[Good::Planks, Good::Clothes, Good::Fish]),
                false,
            ),
            &[
                (Craft::Schooner, "Kittiwake"),
                (Craft::Schooner, "Maud Ellen"),
                (Craft::Schooner, "Good Intent"),
                (Craft::PaddleSteamer, "PS Haverholm Belle"),
            ],
        ));
    }
    if let (Some(h), Some(n), Some(b)) = (h_id, n_id, b_id) {
        // (The steamers also take what the packets land at Haverholm on to Brannsey.)
        let mut from_home = vec![Good::Bricks, Good::Bread];
        from_home.extend(treats);
        let mut to_brannsey = vec![Good::Bricks, Good::Planks];
        to_brannsey.extend(treats);
        lines.push((
            Route {
                name: "Northern steam line".into(),
                stops: vec![
                    Stop {
                        at: n,
                        load: vec![Good::Wool, Good::Planks, Good::Clothes],
                        unload: vec![Good::Bread, Good::Bricks],
                    },
                    Stop {
                        at: h,
                        load: from_home,
                        unload: vec![Good::Wool, Good::Planks, Good::Clothes],
                    },
                    Stop {
                        at: b,
                        load: vec![Good::Bread, Good::Ale],
                        unload: to_brannsey,
                    },
                ],
                air: false,
                company: crate::sim::PLAYER,
            },
            &[
                (Craft::Steamer, "SS Brannock"),
                (Craft::Steamer, "SS Northern Star"),
            ],
        ));
    }
    if let (Some(h), Some(s)) = (h_id, s_id) {
        lines.push((
            pair(
                "Skerry clay boat",
                (s, &[Good::Bricks, Good::Hops]),
                (h, &[Good::Fish, Good::Clothes, Good::Planks]),
                false,
            ),
            &[(Craft::Schooner, "Clay Pigeon")],
        ));
    }
    // The packets: out with cloth and brick, home with what only the far lands give.
    let out: &[Good] = &[Good::Clothes, Good::Bricks, Good::SaltFish, Good::Coats];
    if let (Some(h), Some(f)) = (h_id, id(world, far.sarrow)) {
        lines.push((
            pair(
                OCEAN_ROUTE,
                (f, &[Good::Rum, Good::Planks, Good::Logs]),
                (h, out),
                false,
            ),
            &[
                (Craft::Clipper, "Morning Star"),
                (Craft::Steamer, "SS Sarrow Trader"),
            ],
        ));
    }
    if let (Some(f), Some(k)) = (id(world, far.sarrow), id(world, far.palm)) {
        lines.push((
            pair(
                "Palm Key lugger",
                (k, &[Good::Cane, Good::Fish]),
                (f, &[Good::Clothes, Good::Planks]),
                false,
            ),
            &[(Craft::Schooner, "Pelican")],
        ));
    }
    if let (Some(h), Some(f)) = (h_id, id(world, far.amber)) {
        lines.push((
            pair(
                "Amber Reach packet",
                (f, &[Good::Dates, Good::Salt]),
                (
                    h,
                    &[Good::Planks, Good::Bricks, Good::SaltFish, Good::Coats],
                ),
                false,
            ),
            &[
                (Craft::Clipper, "Amber Queen"),
                (Craft::Steamer, "SS Saltwind"),
            ],
        ));
    }
    if let (Some(f), Some(k)) = (id(world, far.amber), id(world, far.salt)) {
        lines.push((
            pair(
                "Saltholm boat",
                (k, &[Good::Salt, Good::Fish]),
                (f, &[Good::Clothes, Good::Planks]),
                false,
            ),
            &[(Craft::Schooner, "Brine Maid")],
        ));
    }
    if let (Some(h), Some(f)) = (h_id, id(world, far.frost)) {
        lines.push((
            pair(
                "Frostholm packet",
                (f, &[Good::Furs, Good::Oil]),
                (h, out),
                false,
            ),
            &[
                (Craft::Steamer, "SS Ice Pilot"),
                (Craft::Clipper, "North Light"),
            ],
        ));
    }
    if let (Some(f), Some(k)) = (id(world, far.frost), id(world, far.rock)) {
        lines.push((
            pair(
                "Whalers' Rock tender",
                (k, &[Good::Oil, Good::Fish]),
                (f, &[Good::Clothes, Good::Planks]),
                false,
            ),
            &[(Craft::Schooner, "Guillemot")],
        ));
    }
    // Airships: over the home isles, and the long way to each far town.
    let hm = id(world, h_mast);
    if let (Some(h), Some(n)) = (hm, id(world, n_mast)) {
        lines.push((
            pair(
                "Northmoor air freight",
                (n, &[Good::Wool, Good::Clothes]),
                (h, &[Good::Bricks]),
                true,
            ),
            &[(Craft::Airship, "Albatross"), (Craft::Airship, "Petrel")],
        ));
    }
    /// A mast overseas, its route's name, what it sends home, and the airships on it.
    type AirLine = (
        Option<Entity>,
        &'static str,
        &'static [Good],
        &'static [(Craft, &'static str)],
    );
    let air: [AirLine; 3] = [
        (
            far.sarrow_mast,
            "Sarrowmere air freight",
            &[Good::Rum],
            &[(Craft::Airship, "Frigatebird")],
        ),
        (
            far.amber_mast,
            "Amber Reach air freight",
            &[Good::Dates, Good::Salt],
            &[(Craft::Airship, "Sandgrouse")],
        ),
        (
            far.frost_mast,
            "Frostholm air freight",
            &[Good::Furs, Good::Oil],
            &[(Craft::Airship, "Snow Goose")],
        ),
    ];
    for (mast, name, goods, ships) in air {
        if let (Some(h), Some(m)) = (hm, id(world, mast)) {
            lines.push((pair(name, (m, goods), (h, &[Good::Clothes]), true), ships));
        }
    }
    let routes: Vec<Route> = lines.iter().map(|l| l.0.clone()).collect();
    world.insert_resource(Routes(routes));
    // --- Ships on the routes, spread round them.
    for (r, (route, list)) in lines.iter().enumerate() {
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
    // The pedlar's first stock (no specialist the company already has).
    let owned = crate::items::specialists_owned(world);
    let mut items = world.resource_mut::<crate::items::Items>();
    items.restock(&owned);
    items.market_open = true;
    crate::fleet::spawn_traffic(world, true);
    // The company's ships go out with some gear aboard.
    crate::gear::fit_out(
        world,
        &[
            (
                "Morning Star",
                &["Captain Maren Holt (bold)", "Studding sails"],
            ),
            ("SS Brannock", &["Screw propeller"]),
            ("Albatross", &["Gas cell lining"]),
            ("Kittiwake", &["Ship's cat"]),
            ("PS Haverholm Belle", &["Full steam ahead"]),
        ],
    );
    crate::gear::apply(world);
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

/// The pedlar of items starts calling at `harbour` (the home quay of the sandbox, the first
/// harbour of a new game).
pub fn pedlar_calls(world: &mut World, harbour: Entity) {
    let Some(hb) = world.get::<Building>(harbour).cloned() else {
        return;
    };
    let (m, heading) = mooring(&hb);
    let out = grid::front_dir(hb.rot).as_dvec2();
    let side = DVec2::new(out.y, -out.x);
    let quay = m + out * 46.0 - side * 58.0;
    let away = quay + out * 1300.0 + side * 500.0;
    let e = spawn_vessel(
        world,
        Craft::Barque,
        "Pedlar's barque Magpie",
        DVec3::new(quay.x, 0.0, quay.y),
        heading,
        None,
        Roam::Pedlar { quay, away },
        7_707,
    );
    if let Some(mut v) = world.get_mut::<crate::fleet::Vessel>(e) {
        v.doing = crate::fleet::Doing::Moored(crate::fleet::PEDLAR_STAYS_S);
    }
}

/// Run the economy ahead (no carts, no drawing) so the start has full houses and stock.
pub fn prewarm(world: &mut World, seconds: f32) {
    let mut sys = bevy_ecs::system::SystemState::<(
        ResMut<Economy>,
        Query<(&Building, &mut House, Option<&crate::items::Boost>)>,
        Query<(
            Entity,
            &Building,
            &mut crate::sim::Producer,
            Option<&crate::sim::RoadLink>,
            Option<&crate::items::Boost>,
        )>,
        ResMut<crate::sim::Shipments>,
    )>::new(world);
    // Each route as (island, loads, unloads) per stop.
    let isle_of: std::collections::HashMap<u32, usize> = world
        .query::<&Building>()
        .iter(world)
        .map(|b| (b.id, b.island))
        .collect();
    type PaperStop = (usize, Vec<Good>, Vec<Good>);
    let routes: Vec<Vec<PaperStop>> = world
        .get_resource::<Routes>()
        .map(|r| {
            r.0.iter()
                .filter_map(|route| {
                    route
                        .stops
                        .iter()
                        .map(|s| {
                            isle_of
                                .get(&s.at)
                                .map(|i| (*i, s.load.clone(), s.unload.clone()))
                        })
                        .collect::<Option<Vec<_>>>()
                })
                .collect()
        })
        .unwrap_or_default();
    let mut holds: Vec<Vec<(Good, u32)>> = vec![Vec::new(); routes.len()];
    let mut t = 0.0;
    while t < seconds {
        let Ok((mut econ, mut houses, mut producers, mut ship)) = sys.get_mut(world) else {
            return;
        };
        crate::sim::step(1.0, &mut econ, &mut houses, &mut producers, &mut ship);
        // Deliver at once while warming up.
        let jobs = std::mem::take(&mut ship.carts);
        for (e, g, out) in jobs {
            if out && let Ok((_, b, _, _, _)) = producers.get(e) {
                let isle = b.island;
                econ.islands[isle].add(g, 1.0);
            }
        }
        t += 1.0;
        // The ships are not sailing yet: every minute and a half each route makes one round
        // on paper (an ocean route every third time), so islands start with what the
        // routes bring them.
        if t % 90.0 == 0.0 {
            let round = (t / 90.0) as u32;
            for (k, route) in routes.iter().enumerate() {
                let far = route.iter().any(|s| s.0 >= 4) && route.iter().any(|s| s.0 < 4);
                if far && !round.is_multiple_of(3) {
                    continue;
                }
                let cargo = &mut holds[k];
                // Twice round, so what the last stop loads reaches the first.
                for (isle, load, unload) in route.iter().chain(route.iter()) {
                    crate::fleet::exchange(&mut econ.islands[*isle], cargo, load, unload, 4, 24);
                }
            }
        }
    }
    sys.apply(world);
}

/// One of every craft in a review line off the home quay: the hulls side by side, bows the same
/// way, smallest nearest the shore; the airship and the biplane low over them.
fn fleet_review(world: &mut World, hb: &Building) {
    let (m, heading) = mooring(hb);
    let out = grid::front_dir(hb.rot).as_dvec2();
    let side = DVec2::new(out.y, -out.x);
    let base = m + out * 190.0 - side * 150.0;
    // Along the shore in line ahead, bows the same way, a gap of about 20 m between hulls.
    let line = [
        (Craft::Smack, 0.0, 0.0),
        (Craft::Schooner, 40.0, 0.0),
        (Craft::PaddleSteamer, 98.0, 0.0),
        (Craft::Barque, 162.0, 0.0),
        (Craft::Steamer, 232.0, 0.0),
        (Craft::Clipper, 312.0, 0.0),
        (Craft::Seaplane, 370.0, 0.0),
        (Craft::Biplane, 120.0, 46.0),
        (Craft::Airship, 215.0, 78.0),
        // The second line, further out: the round 8 craft.
        (Craft::Cutter, 10.0, 0.0),
        (Craft::Brig, 60.0, 0.0),
        (Craft::Gunboat, 122.0, 0.0),
        (Craft::Frigate, 190.0, 0.0),
        (Craft::Liner, 292.0, 0.0),
        (Craft::FlyingBoat, 180.0, 60.0),
    ];
    for (i, (craft, d, y)) in line.into_iter().enumerate() {
        let lift = if craft == Craft::Airship || craft == Craft::Biplane {
            -out * 40.0
        } else if i >= 9 {
            out * 110.0
        } else {
            DVec2::ZERO
        };
        let p = base + side * d + lift;
        let e = spawn_vessel(
            world,
            craft,
            &format!("Fleet review {}", craft.name()),
            DVec3::new(p.x, y, p.y),
            heading,
            None,
            Roam::None,
            9_100 + i as u64,
        );
        if let Some(mut v) = world.get_mut::<crate::fleet::Vessel>(e) {
            v.doing = crate::fleet::Doing::Moored(1.0e9);
        }
    }
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
        w.init_resource::<crate::globe::Frame>();
        w.init_resource::<crate::draw::Drawn>();
        w.init_resource::<crate::fleet::SeaChart>();
        w.init_resource::<Routes>();
        w.init_resource::<crate::items::Items>();
        w.init_resource::<crate::incidents::Incidents>();
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
        let mut tiers = vec![[0; Tier::COUNT]; islands().len()];
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
        assert_eq!(masts, 5, "airship masts");
        let routes = w.resource::<Routes>().0.clone();
        let names: Vec<&str> = routes.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(routes.len(), 13, "routes: {names:?}");
        // Every far land has its town and its outpost, in the style of its climate, and
        // makes what only it can.
        for (isle, kinds) in [
            (5, &[Kind::CanePlantation, Kind::Distillery][..]),
            (6, &[Kind::CanePlantation]),
            (9, &[Kind::DateGrove, Kind::Weaver]),
            (10, &[Kind::SaltWorks]),
            (12, &[Kind::TrapperLodge, Kind::OilWorks]),
            (13, &[Kind::OilWorks]),
        ] {
            let on: Vec<Kind> = w
                .query::<&Building>()
                .iter(&w)
                .filter(|b| b.island == isle)
                .map(|b| b.kind)
                .collect();
            let name = islands()[isle].name;
            assert!(on.contains(&Kind::Harbour), "no harbour on {name}");
            assert!(
                on.iter().filter(|k| **k == Kind::House).count() >= 8,
                "few houses on {name}"
            );
            for k in kinds {
                assert!(on.contains(k), "no {k:?} on {name}: {on:?}");
            }
        }
        // Brannsey bakes and brews for the home isles.
        for (kind, n) in [
            (Kind::GrainFarm, 5),
            (Kind::Mill, 4),
            (Kind::Bakery, 4),
            (Kind::HopFarm, 3),
            (Kind::Brewery, 2),
        ] {
            let have = w
                .query::<&Building>()
                .iter(&w)
                .filter(|b| b.island == 1 && b.kind == kind)
                .count();
            assert!(have >= n, "{have} {kind:?} on Brannsey");
        }
        // Haverholm's centre is paved and Burghers live on it, with what they ask for; the
        // villages keep earth streets.
        let (roads, paved) = {
            let g = w.resource::<Grid>();
            (g.roads.len(), g.paved.len())
        };
        assert!(
            paved > 100 && paved * 2 < roads,
            "{paved} of {roads} road cells paved"
        );
        assert!(tiers[0][Tier::Burghers.index()] >= 6, "{:?}", tiers[0]);
        for (isle, kinds) in [
            (
                0,
                &[
                    Kind::Glassworks,
                    Kind::Saltery,
                    Kind::Furrier,
                    Kind::LampMaker,
                    Kind::School,
                    Kind::Tavern,
                ][..],
            ),
            (1, &[Kind::Confectioner]),
        ] {
            for k in kinds {
                assert!(
                    w.query::<&Building>()
                        .iter(&w)
                        .any(|b| b.island == isle && b.kind == *k),
                    "no {k:?} on {}",
                    islands()[isle].name
                );
            }
        }
        build::refresh(&mut w);
        for (b, h) in w.query::<(&Building, &House)>().iter(&w) {
            if h.tier == Tier::Burghers {
                assert!(h.paved, "a Burghers' house off the paving on {}", b.island);
                assert!(
                    h.services[..4].iter().all(|s| *s),
                    "house {} lacks a service",
                    b.id
                );
            }
        }
        // The sea joins the two ends of every shipping route.
        for route in routes.iter().filter(|r| !r.air) {
            let ends: Vec<DVec2> = route
                .stops
                .iter()
                .map(|s| {
                    let b = w
                        .query::<&Building>()
                        .iter(&w)
                        .find(|b| b.id == s.at)
                        .cloned()
                        .unwrap();
                    crate::fleet::mooring(&b).0
                })
                .collect();
            for pair in ends.windows(2) {
                assert!(
                    w.resource_mut::<crate::fleet::SeaChart>()
                        .voyage(pair[0], pair[1])
                        .is_some(),
                    "no way by sea along {}",
                    route.name
                );
            }
        }
        // One of them crosses the ocean to the post on Sarrowmere, and the sea joins its ends.
        let ocean = routes
            .iter()
            .find(|r| r.name == OCEAN_ROUTE)
            .expect("ocean route");
        let ends: Vec<DVec2> = ocean
            .stops
            .iter()
            .map(|s| {
                let b = w
                    .query::<&Building>()
                    .iter(&w)
                    .find(|b| b.id == s.at)
                    .cloned()
                    .unwrap();
                crate::fleet::mooring(&b).0
            })
            .collect();
        assert_ne!(
            crate::globe::region_of(ends[0].x, ends[0].y),
            crate::globe::region_of(ends[1].x, ends[1].y)
        );
        let legs = w
            .resource_mut::<crate::fleet::SeaChart>()
            .voyage(ends[1], ends[0])
            .expect("a way over the ocean");
        assert_eq!(legs.len(), 2);
        let miles: f64 = legs
            .iter()
            .map(|l| l.windows(2).map(|w| (w[1] - w[0]).length()).sum::<f64>())
            .sum();
        assert!((25_000.0..60_000.0).contains(&miles), "{miles:.0} m");
        // The legs meet at the same spot on the planet, half way.
        let (a, b) = (*legs[0].last().unwrap(), legs[1][0]);
        assert!(crate::globe::distance(a, b) < 1.0);
        assert!(
            unlinked.is_empty(),
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
        // Every house pays the plain sum of its met needs: never more than its tier's full
        // income, and a full Fisherfolk house with fish, clothes and a market pays just that.
        let mut paid = 0.0;
        let mut full_fisher = 0;
        for (h, boost) in w.query::<(&House, Option<&crate::items::Boost>)>().iter(&w) {
            // (Items in reach are the only thing that adds to it.)
            assert!(boost.is_some() || h.income <= h.tier.full_income() + 1e-3);
            paid += h.income;
            if h.tier == Tier::Fisherfolk && h.residents > 9.9 && h.income > 1.99 {
                full_fisher += 1;
            }
        }
        assert!(full_fisher > 20, "{full_fisher} full Fisherfolk houses");
        let econ = w.resource::<Economy>();
        assert!(
            (paid as f64 - econ.taxes_per_min).abs() < econ.taxes_per_min * 0.25,
            "houses pay {paid:.0}, the books say {:.0}",
            econ.taxes_per_min
        );
        let pop: Vec<[f32; Tier::COUNT]> = econ.islands.iter().map(|b| b.population).collect();
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
