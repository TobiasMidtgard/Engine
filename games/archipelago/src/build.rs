//! Placing and removing buildings and roads, and what follows from a change: which buildings
//! reach a harbour by road, which houses have a market or chapel in reach, how many trees a
//! woodcutter has, and where the forest has to make way.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use engine::prelude::*;
use engine::world::SurfaceMaterials;
use engine::world::terrain::{EditOp, EditShape, TerrainEdit};

use crate::defs::{CELL_M, Good, Kind, MAX_SERVICES, PAVED_REACH_COST, PAVING_BRICKS, Site, Tier};
use crate::draw::{Art, Drawn, Look, Spin, spawn_model};
use crate::globe::{Biome, Frame};
use crate::grid::{self, Grid, Occ};
use crate::island;
use crate::looks::{h01, look};
use crate::sim::{Building, Economy, House, Producer, RoadLink};

/// Trees by cell, so building can clear them and woodcutters can count them.
#[derive(Resource, Default)]
pub struct Forest {
    pub by_cell: HashMap<IVec2, Vec<Entity>>,
    /// Cells cleared for building: nothing is planted there again.
    pub cleared: HashSet<IVec2>,
}

/// Something was built or removed: links, services and draws need a refresh.
#[derive(Resource, Default)]
pub struct Changed {
    pub any: bool,
    pub next_id: u32,
}

/// Look up a building by its stable id.
pub fn by_id(world: &mut World, id: u32) -> Option<Entity> {
    let mut q = world.query::<(Entity, &Building)>();
    q.iter(world).find(|(_, b)| b.id == id).map(|(e, _)| e)
}

fn pay(world: &mut World, kind: Kind, isle: usize) -> Result<(), String> {
    let def = kind.def();
    let econ = world.resource::<Economy>();
    if econ.coins < def.coins as f64 {
        return Err(format!(
            "{} costs {} coins; you have {:.0}.",
            def.name, def.coins, econ.coins
        ));
    }
    let books = &econ.islands[isle];
    for (g, n) in def.cost {
        if books.get(*g) + 1e-6 < *n as f64 {
            return Err(format!(
                "{} needs {} {} on {}; the warehouse has {:.0}.",
                def.name,
                n,
                g.name(),
                island::islands()[isle].name,
                books.get(*g)
            ));
        }
    }
    let mut econ = world.resource_mut::<Economy>();
    econ.coins -= def.coins as f64;
    for (g, n) in def.cost {
        econ.islands[isle].take(*g, *n as f64);
    }
    Ok(())
}

/// While this exists, ground paint waits here (the start lays out and takes up a lot before
/// it settles; [`flush_paint`] then paints what stayed).
#[derive(Resource, Default)]
pub struct PaintHold(pub Vec<(IVec2, &'static str)>);

/// Paint what [`PaintHold`] kept, for cells something still stands on.
pub fn flush_paint(world: &mut World) {
    let Some(hold) = world.remove_resource::<PaintHold>() else {
        return;
    };
    let mut last: HashMap<IVec2, &'static str> = HashMap::new();
    for (c, s) in hold.0 {
        last.insert(c, s);
    }
    let mut by_surface: HashMap<&'static str, Vec<IVec2>> = HashMap::new();
    {
        let g = world.resource::<Grid>();
        for (c, s) in last {
            if g.cells.contains_key(&c) {
                by_surface.entry(s).or_default().push(c);
            }
        }
    }
    for (s, mut cells) in by_surface {
        cells.sort_by_key(|c| (c.x, c.y));
        paint_cells(world, &cells, s);
    }
}

pub(crate) fn paint_cells(world: &mut World, cells: &[IVec2], surface: &'static str) {
    if let Some(mut hold) = world.get_resource_mut::<PaintHold>() {
        hold.0.extend(cells.iter().map(|c| (*c, surface)));
        return;
    }
    let id = world.resource::<SurfaceMaterials>().id_or_default(surface);
    let frame = world.get_resource::<Frame>().copied().unwrap_or_default();
    for &c in cells {
        let p = grid::cell_center(c);
        let y = grid::cell_height(c);
        world.write_message(TerrainEdit {
            center: frame.at(DVec3::new(p.x, y, p.y)),
            shape: EditShape::Box {
                half_extents: Vec3::new(CELL_M as f32 * 0.55, 3.0, CELL_M as f32 * 0.55),
                rotation: frame.facing(p.x, p.y, 0.0),
            },
            op: EditOp::Paint,
            surface: Some(id),
            source: None,
        });
    }
}

pub(crate) fn clear_trees(world: &mut World, cells: &[IVec2]) {
    let mut gone = Vec::new();
    {
        let mut forest = world.resource_mut::<Forest>();
        for c in cells {
            forest.cleared.insert(*c);
            if let Some(v) = forest.by_cell.remove(c) {
                gone.extend(v);
            }
        }
    }
    for e in gone {
        world.despawn(e);
    }
}

/// Lay a road on a cell.
pub fn place_road(world: &mut World, c: IVec2, charge: bool) -> Result<(), String> {
    if world.resource::<Grid>().is_road(c) {
        return Ok(());
    }
    if !world.resource::<Grid>().is_open(c) {
        return Err("Something already stands there.".into());
    }
    if !grid::cell_roadable(c) {
        return Err("Roads need dry, fairly flat land.".into());
    }
    if charge
        && let Some(i) = crate::island::island_at(grid::cell_center(c).x, grid::cell_center(c).y)
        && !crate::settle::claimed(world, i)
    {
        return Err(crate::settle::unclaimed_text(i));
    }
    if charge {
        let mut econ = world.resource_mut::<Economy>();
        let cost = Kind::Road.def().coins as f64;
        if econ.coins < cost {
            return Err("Not enough coins for a road.".into());
        }
        econ.coins -= cost;
    }
    crate::fields::release_cell(world, c);
    {
        let mut g = world.resource_mut::<Grid>();
        g.cells.insert(c, Occ::Road);
        g.roads.insert(c);
    }
    clear_trees(world, &[c]);
    paint_cells(world, &[c], "dirt");
    crate::streets::mark(world, c);
    world.resource_mut::<Changed>().any = true;
    Ok(())
}

/// Pave a cell: over a dirt road, or a new road where there is none.
pub fn place_paving(world: &mut World, c: IVec2, charge: bool) -> Result<(), String> {
    if world.resource::<Grid>().is_paved(c) {
        return Ok(());
    }
    if !world.resource::<Grid>().is_road(c) {
        // The price of paving covers the road bed under it.
        place_road(world, c, false)?;
    }
    if charge {
        let p = grid::cell_center(c);
        let isle = island::island_at(p.x, p.y);
        let mut econ = world.resource_mut::<Economy>();
        let cost = Kind::PavedRoad.def().coins as f64;
        if econ.coins < cost {
            return Err("Not enough coins for paving.".into());
        }
        if let Some(books) = isle.and_then(|i| econ.islands.get_mut(i))
            && !books.take(Good::Bricks, PAVING_BRICKS)
        {
            return Err("Paving needs bricks in this island's warehouse.".into());
        }
        econ.coins -= cost;
    }
    world.resource_mut::<Grid>().paved.insert(c);
    crate::streets::mark(world, c);
    world.resource_mut::<Changed>().any = true;
    Ok(())
}

/// Pave every road cell within `radius` meters of `center` (the starting towns, free).
pub fn pave_round(world: &mut World, center: DVec2, radius: f64) {
    let cells: Vec<IVec2> = world
        .resource::<Grid>()
        .roads
        .iter()
        .copied()
        .filter(|c| (grid::cell_center(*c) - center).length() <= radius)
        .collect();
    for c in cells {
        let _ = place_paving(world, c, false);
    }
}

/// Roads along an L from `a` to `b` (x first), skipping cells that are already road; stops
/// at the first cell that can't take one. `paved`: lay setts (over dirt too).
pub fn place_road_line(
    world: &mut World,
    a: IVec2,
    b: IVec2,
    paved: bool,
    charge: bool,
) -> Result<u32, String> {
    let mut n = 0;
    let mut c = a;
    let step = |v: i32| v.signum();
    loop {
        if paved {
            place_paving(world, c, charge)?;
        } else {
            place_road(world, c, charge)?;
        }
        n += 1;
        if c == b {
            break;
        }
        if c.x != b.x {
            c.x += step(b.x - c.x);
        } else {
            c.y += step(b.y - c.y);
        }
    }
    Ok(n)
}

/// Place a building. `charge`: take its coins and materials (the starting town is free).
pub fn place(
    world: &mut World,
    kind: Kind,
    origin: IVec2,
    rot: u8,
    charge: bool,
) -> Result<Entity, String> {
    if kind == Kind::Road {
        place_road(world, origin, charge)?;
        return Ok(Entity::PLACEHOLDER);
    }
    if kind == Kind::PavedRoad {
        place_paving(world, origin, charge)?;
        return Ok(Entity::PLACEHOLDER);
    }
    let isle = grid::check(world.resource::<Grid>(), kind, origin, rot)
        .map_err(|b| b.message().to_string())?;
    // An island nobody has settled: its first harbour comes by ship; nothing else before it.
    if charge && !crate::settle::claimed(world, isle) {
        if kind == Kind::Harbour {
            crate::settle::send_settlers(world, origin, rot, isle)?;
            return Ok(Entity::PLACEHOLDER);
        }
        return Err(crate::settle::unclaimed_text(isle));
    }
    if charge && crate::items::Host::of(kind).is_some() {
        // The company's renown says how many of each it may keep.
        let allowed = crate::items::allowed(world.resource::<Economy>());
        let have = world
            .query::<&Building>()
            .iter(world)
            .filter(|b| b.kind == kind)
            .count();
        if have >= allowed {
            return Err(format!(
                "The company's renown allows {allowed} of these; more people in higher \
                 tiers raise it."
            ));
        }
    }
    if charge {
        pay(world, kind, isle)?;
    }
    let base_y = grid::base_height(kind, origin, rot);
    let id = {
        let mut ch = world.resource_mut::<Changed>();
        ch.next_id += 1;
        ch.any = true;
        ch.next_id
    };
    let seed = (origin.x as u64).wrapping_mul(73_856_093)
        ^ (origin.y as u64).wrapping_mul(19_349_663)
        ^ (kind as u64) << 40;
    let tier = Tier::Fisherfolk;
    let def = kind.def();
    let mut e = world.spawn((
        Name::new(format!("{} {}", def.name, id)),
        Building {
            kind,
            origin,
            rot,
            island: isle,
            base_y,
            seed,
            id,
        },
        Look(Arc::new(look(
            kind,
            tier,
            seed,
            island::islands()[isle].biome(),
        ))),
    ));
    if kind == Kind::House {
        e.insert(House::new(tier));
    }
    if def.works() {
        e.insert((Producer::default(), RoadLink::default()));
    }
    if crate::items::Host::of(kind).is_some() {
        e.insert(crate::items::Institution::default());
    }
    let entity = e.id();
    let cells = grid::footprint(kind, origin, rot);
    {
        let mut g = world.resource_mut::<Grid>();
        for &c in &cells {
            g.cells.insert(c, Occ::Building(entity));
        }
    }
    clear_trees(world, &cells);
    match kind {
        Kind::GrainFarm | Kind::HopFarm | Kind::CanePlantation | Kind::DateGrove => {
            paint_cells(world, &cells, "dirt")
        }
        Kind::SaltWorks => paint_cells(world, &cells, "sand"),
        Kind::ClayPit => paint_cells(world, &cells, "mud"),
        Kind::Market | Kind::TownHall | Kind::TradingUnion | Kind::HarbourOffice => {
            paint_cells(world, &cells, "gravel")
        }
        Kind::Harbour => {
            world.resource_mut::<Economy>().islands[isle].harbours += 1;
        }
        _ => {}
    }
    if def.site == Site::Land
        && !crate::fields::is_farm(kind)
        && kind != Kind::House
        && crate::items::Host::of(kind).is_none()
    {
        // A trodden yard round the building (houses keep their grass: they have gardens).
        paint_cells(world, &cells, "dirt");
    }
    // Moving parts.
    if kind == Kind::Mill
        && let Some(art) = world.get_resource::<Art>().cloned()
    {
        let c = grid::footprint_center(kind, origin, rot);
        let yaw = grid::yaw(rot);
        let hub = DVec3::new(c.x, base_y + 11.6, c.y)
            + (Quat::from_rotation_y(yaw) * Vec3::new(0.0, 0.0, 2.6)).as_dvec3();
        let base =
            Transform::from_rotation(Quat::from_rotation_y(yaw) * Quat::from_rotation_x(-0.12));
        let stand = world.resource::<Frame>().stand(hub, 0.0);
        let first = Transform {
            rotation: stand.rotation * base.rotation,
            ..stand
        };
        for part in spawn_model(world, &art, &art.sails.clone(), first, true) {
            world.entity_mut(part).insert(Spin {
                base,
                at: hub,
                axis: Vec3::Z,
                rate: 0.9,
                angle: 0.0,
                on: None,
                driven_by: Some(entity),
            });
            world
                .entity_mut(entity)
                .entry::<MovingParts>()
                .or_default()
                .get_mut()
                .0
                .push(part);
        }
    }
    if kind == Kind::Market {
        spawn_stalls(world, entity, origin, rot, base_y);
    }
    plant_garden(world, entity);
    crate::fields::claim(world, entity);
    crate::effects::add_plumes(world, entity);
    crate::incidents::touch(world);
    if matches!(kind, Kind::Market | Kind::Harbour | Kind::Chapel) {
        crate::streets::mark_all(world);
    }
    world.resource_mut::<Drawn>().dirty = true;
    Ok(entity)
}

/// The trees that grow on a building's lot, by the land's climate: where in the building's
/// space, which species, how grown (0..1).
pub fn garden_trees(kind: Kind, seed: u64, biome: Biome) -> Vec<(Vec3, &'static str, f32)> {
    let r = |k: u64| h01(seed, 900 + k);
    let (small, big): (&[&'static str], &[&'static str]) = match biome {
        Biome::Temperate => (
            &["rowan", "hazel", "birch", "goat willow", "alder"],
            &["oak", "beech", "aspen", "birch"],
        ),
        Biome::Tropical => (&["coconut palm"], &["coconut palm", "tropical canopy"]),
        Biome::Arid => (&["juniper", "umbrella thorn"], &["umbrella thorn"]),
        Biome::Arctic => (
            &["mountain birch", "juniper"],
            &["black spruce", "mountain birch"],
        ),
    };
    let pick =
        |list: &[&'static str], k: u64| list[(r(k) * list.len() as f32) as usize % list.len()];
    let mut out = Vec::new();
    match kind {
        Kind::House => {
            // Beside the house on the side without the shed, and in a back corner (clear of
            // both the cottage and the terrace house that may replace it).
            let free = if crate::looks::variant(seed) == 2 {
                -1.0
            } else {
                1.0
            };
            if r(0) < 0.5 {
                out.push((
                    Vec3::new(5.1 * free, 0.0, -1.6 + 1.2 * r(1)),
                    pick(small, 2),
                    0.42 + 0.2 * r(3),
                ));
            }
            if r(4) < 0.35 {
                out.push((
                    Vec3::new(-4.9 * free, 0.0, -4.7),
                    pick(small, 5),
                    0.36 + 0.2 * r(6),
                ));
            }
        }
        Kind::Chapel => {
            // The churchyard.
            for (k, (x, z)) in [(-6.1, 4.8), (6.1, -3.4), (6.2, 6.4), (-6.2, -6.0)]
                .into_iter()
                .enumerate()
            {
                let k = k as u64 * 4;
                if r(10 + k) < 0.65 {
                    out.push((
                        Vec3::new(x, 0.0, z),
                        pick(big, 11 + k),
                        0.4 + 0.2 * r(12 + k),
                    ));
                }
            }
        }
        Kind::SheepFarm | Kind::GrainFarm | Kind::HopFarm => {
            // One old tree in the farmyard.
            let (w, d) = kind.def().size;
            let yard = Vec3::new(
                -(w as f32) * CELL_M as f32 * 0.5 + 6.0,
                0.0,
                d as f32 * CELL_M as f32 * 0.5 - 6.0,
            );
            out.push((
                yard + Vec3::new(5.4, 0.0, -2.2),
                pick(big, 30),
                0.7 + 0.25 * r(31),
            ));
        }
        Kind::Weaver | Kind::Bakery | Kind::Brewery if r(40) < 0.6 => {
            let (w, d) = kind.def().size;
            out.push((
                Vec3::new(
                    (w as f32 * CELL_M as f32 * 0.5 - 1.0) * if r(41) < 0.5 { -1.0 } else { 1.0 },
                    0.0,
                    -(d as f32) * CELL_M as f32 * 0.5 + 1.2,
                ),
                pick(small, 42),
                0.45 + 0.2 * r(43),
            ));
        }
        _ => {}
    }
    out
}

/// Plant a building's garden trees (they go when the building goes).
fn plant_garden(world: &mut World, owner: Entity) {
    let Some(library) = world
        .get_resource::<engine_foliage::PlantLibrary>()
        .cloned()
    else {
        return;
    };
    let Some(b) = world.get::<Building>(owner).cloned() else {
        return;
    };
    let biome = island::islands()[b.island].biome();
    let c = grid::footprint_center(b.kind, b.origin, b.rot);
    let yaw = grid::yaw(b.rot);
    let frame = *world.resource::<Frame>();
    let mut parts = Vec::new();
    for (k, (at, species, size)) in garden_trees(b.kind, b.seed, biome).into_iter().enumerate() {
        let p = Quat::from_rotation_y(yaw) * at;
        let (x, z) = (c.x + p.x as f64, c.y + p.z as f64);
        let ground = island::height(x, z);
        if ground < 0.4 {
            continue;
        }
        let Some(mut tree) =
            engine_foliage::SceneryTree::new(&library, species, b.seed ^ (k as u64 + 1) << 48)
        else {
            continue;
        };
        tree.size = size;
        let turn = h01(b.seed, 950 + k as u64) * std::f32::consts::TAU;
        parts.push(
            world
                .spawn((frame.stand(DVec3::new(x, ground - 0.1, z), turn), tree))
                .id(),
        );
    }
    if !parts.is_empty() {
        world
            .entity_mut(owner)
            .entry::<MovingParts>()
            .or_default()
            .get_mut()
            .0
            .extend(parts);
    }
}

/// Extra entities a building owns (sails, stalls, lights).
#[derive(Component, Default, Clone, Debug)]
pub struct MovingParts(pub Vec<Entity>);

/// Remove a building (or a road cell).
pub fn demolish(world: &mut World, e: Entity) {
    let Some(b) = world.get::<Building>(e).cloned() else {
        return;
    };
    crate::fields::release_all(world, e);
    crate::items::vacate(world, e);
    crate::shipyard::forget(world, e);
    crate::incidents::forget(world, e);
    if matches!(b.kind, Kind::Market | Kind::Harbour | Kind::Chapel) {
        crate::streets::mark_all(world);
    }
    {
        let mut g = world.resource_mut::<Grid>();
        for c in grid::footprint(b.kind, b.origin, b.rot) {
            g.cells.remove(&c);
        }
    }
    if matches!(b.kind, Kind::Harbour | Kind::AirshipMast) {
        crate::orders::forget_building(world, b.id);
    }
    if b.kind == Kind::Harbour {
        let h = &mut world.resource_mut::<Economy>().islands[b.island].harbours;
        *h = h.saturating_sub(1);
    }
    if let Some(parts) = world.get::<MovingParts>(e).cloned() {
        for p in parts.0 {
            world.despawn(p);
        }
    }
    world.despawn(e);
    world.resource_mut::<Changed>().any = true;
    world.resource_mut::<Drawn>().dirty = true;
}

pub fn remove_road(world: &mut World, c: IVec2) {
    let removed = {
        let mut g = world.resource_mut::<Grid>();
        let r = g.roads.remove(&c);
        if r {
            g.cells.remove(&c);
            g.paved.remove(&c);
        }
        r
    };
    if removed {
        paint_cells(world, &[c], "grass");
        crate::streets::mark(world, c);
        world.resource_mut::<Changed>().any = true;
    }
}

fn spawn_stalls(world: &mut World, owner: Entity, origin: IVec2, rot: u8, base_y: f64) {
    use engine_ui::demo::art::stall::{Goods, build_stall, place_stall};
    let Some(mats) = world.get_resource::<StallKit>().map(|k| k.0) else {
        return;
    };
    let c = grid::footprint_center(Kind::Market, origin, rot);
    let yaw = grid::yaw(rot);
    let mut parts = Vec::new();
    for (k, (x, z, a, goods)) in [
        (-5.0f32, -5.0f32, 0.785f32, Goods::Produce),
        (5.0, -5.0, -0.785, Goods::Cloth),
        (-5.0, 5.0, 2.356, Goods::Potter),
        (5.0, 5.0, -2.356, Goods::Smith),
    ]
    .into_iter()
    .enumerate()
    {
        let local = Quat::from_rotation_y(yaw) * Vec3::new(x, 0.0, z);
        let at = world.resource::<Frame>().stand(
            DVec3::new(c.x + local.x as f64, base_y + 0.08, c.y + local.z as f64),
            yaw + a + std::f32::consts::PI,
        );
        parts.extend(place_stall(
            world,
            &mats,
            at,
            build_stall(goods, origin.x as u64 * 31 + k as u64),
        ));
    }
    world
        .entity_mut(owner)
        .entry::<MovingParts>()
        .or_default()
        .get_mut()
        .0
        .extend(parts);
}

/// Materials for market stalls (the UI module's market art).
#[derive(Resource, Clone, Copy)]
pub struct StallKit(pub engine_ui::demo::art::stall::StallMats);

/// The public buildings houses ask for, in the order of `House::services`.
pub const SERVICES: [Kind; MAX_SERVICES] = [Kind::Market, Kind::Chapel, Kind::School, Kind::Tavern];

/// How much further than its radius a public building reaches along a dirt road.
const STREET_REACH: f32 = 1.25;

/// The road cells a public building serves along the streets from its door: its reach in
/// cells times [`STREET_REACH`], a paved cell costing [`PAVED_REACH_COST`] of a dirt one.
fn street_reach(
    b: &Building,
    roads: &HashSet<IVec2>,
    paved: &HashSet<IVec2>,
    out: &mut HashSet<IVec2>,
) {
    let budget = b.kind.def().reach as f32 * STREET_REACH;
    let cost = |c: &IVec2| {
        if paved.contains(c) {
            PAVED_REACH_COST
        } else {
            1.0
        }
    };
    let mut best: HashMap<IVec2, f32> = HashMap::new();
    let mut queue = VecDeque::new();
    for c in grid::ring(b.kind, b.origin, b.rot) {
        if roads.contains(&c) {
            best.insert(c, 0.0);
            queue.push_back(c);
        }
    }
    // Costs are 0.5 or 1, so a plain queue with re-relaxing settles fast.
    while let Some(c) = queue.pop_front() {
        let d = best[&c];
        for n in [IVec2::X, IVec2::NEG_X, IVec2::Y, IVec2::NEG_Y] {
            let nc = c + n;
            if !roads.contains(&nc) {
                continue;
            }
            let nd = d + cost(&nc);
            if nd <= budget && best.get(&nc).is_none_or(|old| nd < *old) {
                best.insert(nc, nd);
                queue.push_back(nc);
            }
        }
    }
    out.extend(best.into_keys());
}

/// After a change: road links, house services, woodcutters' trees.
pub fn refresh(world: &mut World) {
    if !world.resource::<Changed>().any {
        return;
    }
    world.resource_mut::<Changed>().any = false;
    // Items first: what they add up to for each building, which the services below read.
    crate::items::apply(world);
    crate::gear::apply(world);
    let roads = world.resource::<Grid>().roads.clone();
    // Distance over roads from every harbour, island by island (one BFS for all).
    let mut dist: HashMap<IVec2, (u32, IVec2)> = HashMap::new();
    let mut queue = VecDeque::new();
    let mut harbours: Vec<(Entity, Building)> = Vec::new();
    let mut all: Vec<(Entity, Building)> = Vec::new();
    {
        let mut q = world.query::<(Entity, &Building)>();
        for (e, b) in q.iter(world) {
            all.push((e, b.clone()));
            if b.kind == Kind::Harbour {
                harbours.push((e, b.clone()));
            }
        }
    }
    for (_, h) in &harbours {
        for c in grid::ring(h.kind, h.origin, h.rot) {
            if roads.contains(&c) && !dist.contains_key(&c) {
                dist.insert(c, (0, c));
                queue.push_back(c);
            }
        }
    }
    while let Some(c) = queue.pop_front() {
        let d = dist[&c].0;
        for n in [IVec2::X, IVec2::NEG_X, IVec2::Y, IVec2::NEG_Y] {
            let nc = c + n;
            if roads.contains(&nc) && !dist.contains_key(&nc) {
                dist.insert(nc, (d + 1, c));
                queue.push_back(nc);
            }
        }
    }
    let world_pt = |c: IVec2| {
        let p = grid::cell_center(c);
        DVec3::new(p.x, grid::cell_height(c) + 0.05, p.y)
    };
    let mut links: Vec<(Entity, Option<Arc<Vec<DVec3>>>)> = Vec::new();
    for (e, b) in &all {
        if !b.kind.def().works() {
            continue;
        }
        let start = grid::ring(b.kind, b.origin, b.rot)
            .into_iter()
            .filter_map(|c| dist.get(&c).map(|d| (d.0, c)))
            .min_by_key(|(d, c)| (*d, c.x, c.y));
        let path = start.and_then(|(_, mut c)| {
            let center = grid::footprint_center(b.kind, b.origin, b.rot);
            let mut pts = vec![DVec3::new(center.x, b.base_y, center.y)];
            let mut guard = 0;
            loop {
                pts.push(world_pt(c));
                let (d, prev) = dist[&c];
                if d == 0 || guard > 4000 {
                    break;
                }
                c = prev;
                guard += 1;
            }
            // Into the harbour nearest the last road cell.
            let end = harbours
                .iter()
                .min_by_key(|(_, h)| {
                    let hc = grid::footprint_center(h.kind, h.origin, h.rot);
                    ((hc - grid::cell_center(c)).length() * 10.0) as i64
                })
                .map(|(_, h)| h.clone())?;
            if end.island != b.island {
                return None;
            }
            let hc = grid::footprint_center(end.kind, end.origin, end.rot);
            let back = grid::front_dir(end.rot).as_dvec2() * -2.0 * CELL_M * 0.5;
            pts.push(DVec3::new(hc.x + back.x, end.base_y, hc.y + back.y));
            Some(Arc::new(pts))
        });
        links.push((*e, path));
    }
    for (e, path) in links {
        if let Some(mut l) = world.get_mut::<RoadLink>(e) {
            l.path = path;
        }
    }
    // Services within reach: as the crow flies, or further along paved streets.
    let paved = world.resource::<Grid>().paved.clone();
    let services: Vec<(Kind, usize, DVec2, f64)> = all
        .iter()
        .filter(|(e, b)| {
            SERVICES.contains(&b.kind) && world.get::<crate::incidents::Ruin>(*e).is_none()
        })
        .map(|(_, b)| {
            (
                b.kind,
                b.island,
                grid::footprint_center(b.kind, b.origin, b.rot),
                b.kind.def().reach as f64 * CELL_M,
            )
        })
        .collect();
    let mut by_road: [HashSet<IVec2>; MAX_SERVICES] = Default::default();
    for (_, b) in all.iter().filter(|(e, b)| {
        SERVICES.contains(&b.kind) && world.get::<crate::incidents::Ruin>(*e).is_none()
    }) {
        let i = SERVICES.iter().position(|k| *k == b.kind).unwrap_or(0);
        street_reach(b, &roads, &paved, &mut by_road[i]);
    }
    // What the buildings that make nothing cost to run.
    world.resource_mut::<Economy>().public_upkeep = all
        .iter()
        .filter(|(_, b)| !b.kind.def().works())
        .map(|(_, b)| b.kind.def().upkeep)
        .sum();
    // Smoky works, and how far (centre to centre) their smoke spoils a street.
    let smoky: Vec<(usize, DVec2)> = all
        .iter()
        .filter(|(_, b)| b.kind.smoky())
        .map(|(_, b)| (b.island, grid::footprint_center(b.kind, b.origin, b.rot)))
        .collect();
    let smoke_m = (crate::defs::SMOKE_CELLS + 2.0) as f64 * CELL_M;
    let mut q = world.query::<(&Building, &mut House, Option<&crate::items::Boost>)>();
    for (b, mut h, boost) in q.iter_mut(world) {
        let c = grid::footprint_center(b.kind, b.origin, b.rot);
        let door = grid::ring(b.kind, b.origin, b.rot);
        for (i, kind) in SERVICES.into_iter().enumerate() {
            h.services[i] = services
                .iter()
                .any(|(k, isle, p, r)| *k == kind && *isle == b.island && (*p - c).length() <= *r)
                || door.iter().any(|c| by_road[i].contains(c))
                // An item can stand in for the building.
                || boost.is_some_and(|x| x.serves.contains(&kind));
        }
        h.paved = door.iter().any(|c| paved.contains(c));
        // Smoke from a works next door spoils the street.
        h.smoke = smoky
            .iter()
            .any(|(isle, p)| *isle == b.island && (*p - c).length() <= smoke_m);
    }
    // Woodcutters: wooded cells in reach that nothing stands on (the trees planted there
    // follow the same woods field).
    let grid_cells: HashSet<IVec2> = world.resource::<Grid>().cells.keys().copied().collect();
    let mut q = world.query::<(&Building, &mut Producer, Option<&crate::fields::Fields>)>();
    for (b, mut p, fields) in q.iter_mut(world) {
        if !matches!(b.kind, Kind::Woodcutter | Kind::TrapperLodge) {
            p.site = crate::fields::share(b.kind, fields);
            continue;
        }
        let r = b.kind.def().reach;
        let (w, d) = b.kind.footprint(b.rot);
        if b.kind == Kind::TrapperLodge {
            // Trappers want wild country: land in reach with nothing built on it.
            let (mut wild, mut all) = (0.0f32, 0.0f32);
            for z in -r..d + r {
                for x in -r..w + r {
                    let c = b.origin + IVec2::new(x, z);
                    let m = grid::cell_center(c);
                    all += 1.0;
                    if !grid_cells.contains(&c)
                        && island::island_at(m.x, m.y) == Some(b.island)
                        && grid::cell_height(c) > 0.9
                    {
                        wild += 1.0;
                    }
                }
            }
            p.site = (wild / all.max(1.0) / 0.7).min(1.0);
            continue;
        }
        let mut n = 0.0;
        for z in -r..d + r {
            for x in -r..w + r {
                let c = b.origin + IVec2::new(x, z);
                if grid_cells.contains(&c) {
                    continue;
                }
                let m = grid::cell_center(c);
                n += island::woods(m.x, m.y);
            }
        }
        p.site = (n as f32 / 60.0).min(1.0);
    }
}

/// Goods an island starts with when settled.
pub fn starting_stock() -> Vec<(Good, f64)> {
    vec![
        (Good::Planks, 60.0),
        (Good::Fish, 20.0),
        (Good::Clothes, 10.0),
        (Good::Bricks, 30.0),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paving_carries_a_market_further_along_the_street() {
        let market = Building {
            kind: Kind::Market,
            origin: IVec2::ZERO,
            rot: 0,
            island: 0,
            base_y: 0.0,
            seed: 1,
            id: 1,
        };
        // One straight street from the market's front.
        let roads: HashSet<IVec2> = (5..120).map(|z| IVec2::new(2, z)).collect();
        assert!(grid::ring(market.kind, market.origin, market.rot).contains(&IVec2::new(2, 5)));
        let mut dirt = HashSet::new();
        street_reach(&market, &roads, &HashSet::new(), &mut dirt);
        let mut paved = HashSet::new();
        street_reach(&market, &roads, &roads, &mut paved);
        let reach = Kind::Market.def().reach as f32 * STREET_REACH;
        assert_eq!(dirt.len(), reach as usize + 1);
        assert_eq!(paved.len(), (reach / PAVED_REACH_COST) as usize + 1);
        // Half paved: the paved half costs half.
        let half: HashSet<IVec2> = (5..15).map(|z| IVec2::new(2, z)).collect();
        let mut mixed = HashSet::new();
        street_reach(&market, &roads, &half, &mut mixed);
        assert!(dirt.len() < mixed.len() && mixed.len() < paved.len());
    }
}
