//! Placing and removing buildings and roads, and what follows from a change: which buildings
//! reach a harbour by road, which houses have a market or chapel in reach, how many trees a
//! woodcutter has, and where the forest has to make way.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use engine::prelude::*;
use engine::world::SurfaceMaterials;
use engine::world::terrain::{EditOp, EditShape, TerrainEdit};

use crate::defs::{CELL_M, Good, Kind, Site, Tier};
use crate::draw::{Art, Drawn, Look, Spin, spawn_model};
use crate::grid::{self, Grid, Occ};
use crate::island;
use crate::looks::look;
use crate::sim::{Building, Economy, House, Producer, RoadLink};

/// Trees by cell, so building can clear them and woodcutters can count them.
#[derive(Resource, Default)]
pub struct Forest {
    pub by_cell: HashMap<IVec2, Vec<Entity>>,
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
    for &c in cells {
        let p = grid::cell_center(c);
        let y = grid::cell_height(c);
        world.write_message(TerrainEdit {
            center: DVec3::new(p.x, y, p.y),
            shape: EditShape::Box {
                half_extents: Vec3::new(CELL_M as f32 * 0.55, 3.0, CELL_M as f32 * 0.55),
                rotation: Quat::IDENTITY,
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

/// Roads along an L from `a` to `b` (x first), skipping cells that are already road; stops
/// at the first cell that can't take one.
pub fn place_road_line(world: &mut World, a: IVec2, b: IVec2, charge: bool) -> Result<u32, String> {
    let mut n = 0;
    let mut c = a;
    let step = |v: i32| v.signum();
    loop {
        place_road(world, c, charge)?;
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
    let isle = grid::check(world.resource::<Grid>(), kind, origin, rot)
        .map_err(|b| b.message().to_string())?;
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
        Look(Arc::new(look(kind, tier, seed))),
    ));
    if kind == Kind::House {
        e.insert(House::new(tier));
    }
    if def.output.is_some() {
        e.insert((Producer::default(), RoadLink::default()));
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
        Kind::GrainFarm | Kind::HopFarm => paint_cells(world, &cells, "dirt"),
        Kind::ClayPit => paint_cells(world, &cells, "mud"),
        Kind::Market => paint_cells(world, &cells, "gravel"),
        Kind::Harbour => {
            world.resource_mut::<Economy>().islands[isle].harbours += 1;
        }
        _ => {}
    }
    if def.site == Site::Land
        && kind != Kind::GrainFarm
        && kind != Kind::HopFarm
        && kind != Kind::SheepFarm
    {
        // A trodden yard round the building.
        paint_cells(world, &cells, "dirt");
    }
    // Moving parts.
    if kind == Kind::Mill
        && let Some(art) = world.get_resource::<Art>().cloned()
    {
        let c = grid::footprint_center(kind, origin, rot);
        let yaw = grid::yaw(rot);
        let hub = Vec3::new(c.x as f32, base_y as f32 + 11.6, c.y as f32)
            + Quat::from_rotation_y(yaw) * Vec3::new(0.0, 0.0, 2.6);
        let base = Transform::from_translation(hub.as_dvec3())
            .with_rotation(Quat::from_rotation_y(yaw) * Quat::from_rotation_x(-0.12));
        for part in spawn_model(world, &art, &art.sails.clone(), base, true) {
            world.entity_mut(part).insert(Spin {
                base,
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
    crate::fields::claim(world, entity);
    crate::effects::add_plumes(world, entity);
    if matches!(kind, Kind::Market | Kind::Harbour | Kind::Chapel) {
        crate::streets::mark_all(world);
    }
    world.resource_mut::<Drawn>().dirty = true;
    Ok(entity)
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
        let at = Transform::from_translation(DVec3::new(
            c.x + local.x as f64,
            base_y + 0.08,
            c.y + local.z as f64,
        ))
        .with_rotation(Quat::from_rotation_y(yaw + a + std::f32::consts::PI));
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

/// After a change: road links, house services, woodcutters' trees.
pub fn refresh(world: &mut World) {
    if !world.resource::<Changed>().any {
        return;
    }
    world.resource_mut::<Changed>().any = false;
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
        if b.kind.def().output.is_none() {
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
    // Services within reach.
    let services: Vec<(Kind, usize, DVec2, f64)> = all
        .iter()
        .filter(|(_, b)| matches!(b.kind, Kind::Market | Kind::Chapel))
        .map(|(_, b)| {
            (
                b.kind,
                b.island,
                grid::footprint_center(b.kind, b.origin, b.rot),
                b.kind.def().reach as f64 * CELL_M,
            )
        })
        .collect();
    let mut q = world.query::<(&Building, &mut House)>();
    for (b, mut h) in q.iter_mut(world) {
        let c = grid::footprint_center(b.kind, b.origin, b.rot);
        for (i, kind) in [Kind::Market, Kind::Chapel].into_iter().enumerate() {
            h.services[i] = services
                .iter()
                .any(|(k, isle, p, r)| *k == kind && *isle == b.island && (*p - c).length() <= *r);
        }
    }
    // Woodcutters: wooded cells in reach that nothing stands on (the trees planted there
    // follow the same woods field).
    let grid_cells: HashSet<IVec2> = world.resource::<Grid>().cells.keys().copied().collect();
    let mut q = world.query::<(&Building, &mut Producer, Option<&crate::fields::Fields>)>();
    for (b, mut p, fields) in q.iter_mut(world) {
        if b.kind != Kind::Woodcutter {
            p.site = crate::fields::share(b.kind, fields);
            continue;
        }
        let r = b.kind.def().reach;
        let (w, d) = b.kind.footprint(b.rot);
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
