//! Editing trade routes and giving ships their orders: open and close routes, add, remove
//! and reorder stops, choose what each stop loads and unloads, and put ships on a route or take
//! them off. The `isle.route.*` actions and the routes screen both come through here.
//!
//! Routes are addressed by their index in [`Routes`] (as `Vessel::route` is); closing one moves
//! the later ones down by one. Stops are addressed by index within their route. A vessel keeps
//! sailing for the stop it was bound for when stops move around it, and turns for the next one
//! when its stop is taken away.

use engine::prelude::*;
use std::sync::Arc;

use crate::defs::{Good, Kind};
use crate::fleet::{Doing, Roam, Route, Routes, Stop, Vessel, building_by_id, mast_point, mooring};
use crate::sim::Building;

fn route_mut<'w>(world: &'w mut World, r: usize) -> Result<Mut<'w, Routes>, String> {
    let routes = world.resource_mut::<Routes>();
    if r >= routes.0.len() {
        return Err(format!("no route {r}"));
    }
    Ok(routes)
}

fn stop_kind(air: bool) -> Kind {
    if air {
        Kind::AirshipMast
    } else {
        Kind::Harbour
    }
}

/// Check that building `at` can be a stop on an `air` (or sea) route.
fn check_stop(world: &mut World, at: u32, air: bool) -> Result<(), String> {
    let b = building_by_id(world, at).ok_or(format!("no building {at}"))?;
    let want = stop_kind(air);
    if b.kind != want {
        return Err(format!(
            "{} is not a {}",
            b.kind.def().name,
            want.def().name
        ));
    }
    Ok(())
}

/// Open an empty route for ships (or, `air`, for aircraft). Returns its index.
pub fn open_route(world: &mut World, name: &str, air: bool) -> usize {
    let mut routes = world.resource_mut::<Routes>();
    let name = if name.trim().is_empty() {
        format!("Route {}", routes.0.len() + 1)
    } else {
        name.trim().to_string()
    };
    routes.0.push(Route {
        name,
        stops: Vec::new(),
        air,
    });
    routes.0.len() - 1
}

/// Close route `r`. Its vessels go off duty at the nearest harbour or mast.
pub fn close_route(world: &mut World, r: usize) -> Result<(), String> {
    route_mut(world, r)?.0.remove(r);
    let on: Vec<Entity> = vessels_on(world, r);
    for e in on {
        stand_down(world, e);
    }
    let mut q = world.query::<&mut Vessel>();
    for mut v in q.iter_mut(world) {
        if let Some(k) = v.route
            && k > r
        {
            v.route = Some(k - 1);
        }
    }
    Ok(())
}

pub fn rename_route(world: &mut World, r: usize, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a route needs a name".into());
    }
    route_mut(world, r)?.0[r].name = name.to_string();
    Ok(())
}

/// Add a stop at building `at` to route `r`, at `index` (the end when `None`). Returns the
/// stop's index.
pub fn add_stop(
    world: &mut World,
    r: usize,
    at: u32,
    index: Option<usize>,
    load: Vec<Good>,
    unload: Vec<Good>,
) -> Result<usize, String> {
    let air = route_mut(world, r)?.0[r].air;
    check_stop(world, at, air)?;
    let n = world.resource::<Routes>().0[r].stops.len();
    let i = index.unwrap_or(n).min(n);
    let mut order: Vec<Option<usize>> = (0..n).map(Some).collect();
    order.insert(i, None);
    world.resource_mut::<Routes>().0[r]
        .stops
        .insert(i, Stop { at, load, unload });
    reseat(world, r, &order);
    Ok(i)
}

/// Take stop `i` off route `r`.
pub fn remove_stop(world: &mut World, r: usize, i: usize) -> Result<(), String> {
    let n = route_mut(world, r)?.0[r].stops.len();
    if i >= n {
        return Err(format!("route {r} has no stop {i}"));
    }
    let mut order: Vec<Option<usize>> = (0..n).map(Some).collect();
    order.remove(i);
    world.resource_mut::<Routes>().0[r].stops.remove(i);
    reseat(world, r, &order);
    Ok(())
}

/// Move stop `from` of route `r` so that it becomes stop `to`.
pub fn move_stop(world: &mut World, r: usize, from: usize, to: usize) -> Result<(), String> {
    let n = route_mut(world, r)?.0[r].stops.len();
    if from >= n || to >= n {
        return Err(format!("route {r} has stops 0..{n}"));
    }
    let mut order: Vec<Option<usize>> = (0..n).map(Some).collect();
    let moved = order.remove(from);
    order.insert(to, moved);
    {
        let mut routes = world.resource_mut::<Routes>();
        let stops = &mut routes.0[r].stops;
        let s = stops.remove(from);
        stops.insert(to, s);
    }
    reseat(world, r, &order);
    Ok(())
}

/// What stop `i` of route `r` loads and unloads.
pub fn set_goods(
    world: &mut World,
    r: usize,
    i: usize,
    load: Vec<Good>,
    unload: Vec<Good>,
) -> Result<(), String> {
    let mut routes = route_mut(world, r)?;
    let stop = routes.0[r]
        .stops
        .get_mut(i)
        .ok_or(format!("route {r} has no stop {i}"))?;
    stop.load = load;
    stop.unload = unload;
    Ok(())
}

/// Put vessel `e` on route `r`: it heads for the route's nearest stop and goes round from
/// there.
pub fn assign(world: &mut World, e: Entity, r: usize) -> Result<(), String> {
    let route = world
        .resource::<Routes>()
        .0
        .get(r)
        .cloned()
        .ok_or(format!("no route {r}"))?;
    let v = world.get::<Vessel>(e).ok_or("no such vessel")?;
    if v.craft.flies() != route.air {
        return Err(if route.air {
            format!("{} is a ship; this route is for aircraft", v.name)
        } else {
            format!("{} flies; this route is for ships", v.name)
        });
    }
    let here = world
        .get::<Transform>(e)
        .map(|t| t.translation)
        .unwrap_or_default();
    // Start from the nearest stop.
    let mut best = (0usize, f64::MAX);
    for (i, s) in route.stops.iter().enumerate() {
        if let Some(b) = building_by_id(world, s.at) {
            let d = (berth(&b, route.air) - here).length();
            if d < best.1 {
                best = (i, d);
            }
        }
    }
    let mut v = world.get_mut::<Vessel>(e).ok_or("no such vessel")?;
    v.route = Some(r);
    v.roam = Roam::None;
    v.next_stop = best.0;
    replan(&mut v);
    Ok(())
}

/// Take vessel `e` off its route (or its own errands): it goes to the nearest harbour or mast
/// and waits there, cargo kept.
pub fn unassign(world: &mut World, e: Entity) -> Result<(), String> {
    if world.get::<Vessel>(e).is_none() {
        return Err("no such vessel".into());
    }
    stand_down(world, e);
    Ok(())
}

/// A harbour or mast was pulled down: take it off every route.
pub fn forget_building(world: &mut World, id: u32) {
    loop {
        let found = world
            .resource::<Routes>()
            .0
            .iter()
            .enumerate()
            .find_map(|(r, route)| route.stops.iter().position(|s| s.at == id).map(|i| (r, i)));
        let Some((r, i)) = found else {
            break;
        };
        let _ = remove_stop(world, r, i);
    }
}

/// Find a vessel by name (the first with it) or by entity bits.
pub fn find_vessel(world: &mut World, name: &str) -> Option<Entity> {
    let mut q = world.query::<(Entity, &Vessel)>();
    if let Some((e, _)) = q.iter(world).find(|(_, v)| v.name == name) {
        return Some(e);
    }
    let bits: u64 = name.parse().ok()?;
    let e = Entity::try_from_bits(bits)?;
    world.get::<Vessel>(e).map(|_| e)
}

// ------------------------------------------------------------------------------ helpers

fn vessels_on(world: &mut World, r: usize) -> Vec<Entity> {
    let mut q = world.query::<(Entity, &Vessel)>();
    q.iter(world)
        .filter(|(_, v)| v.route == Some(r))
        .map(|(e, _)| e)
        .collect()
}

/// Where a ship moors at a harbour or an aircraft meets a mast.
fn berth(b: &Building, air: bool) -> DVec3 {
    if air {
        mast_point(b)
    } else {
        let (p, _) = mooring(b);
        DVec3::new(p.x, 0.0, p.y)
    }
}

/// Plan the next leg now (from wherever it is) instead of finishing the current one.
fn replan(v: &mut Vessel) {
    if matches!(v.doing, Doing::Sailing | Doing::Lingering(_)) {
        v.doing = Doing::Lingering(0.0);
    }
}

/// After the stops of route `r` changed (`order[new] = Some(old)`, `None` for a new stop),
/// point each vessel on it at the same stop as before, or the one after a removed stop.
fn reseat(world: &mut World, r: usize, order: &[Option<usize>]) {
    let n = order.len();
    let mut q = world.query::<&mut Vessel>();
    for mut v in q.iter_mut(world) {
        if v.route != Some(r) {
            continue;
        }
        if n == 0 {
            v.next_stop = 0;
            continue;
        }
        let old = v.next_stop;
        match order.iter().position(|o| *o == Some(old)) {
            Some(i) => v.next_stop = i,
            None => {
                // Its stop is gone: make for the one that came after it.
                let after = order
                    .iter()
                    .position(|o| o.is_some_and(|k| k > old))
                    .unwrap_or(0);
                v.next_stop = after;
                replan(&mut v);
            }
        }
    }
}

/// Send a vessel off duty to the nearest harbour (ships) or mast (aircraft). Planes circle
/// it; ships and airships lie there.
fn stand_down(world: &mut World, e: Entity) {
    let Some(v) = world.get::<Vessel>(e).cloned() else {
        return;
    };
    let here = world
        .get::<Transform>(e)
        .map(|t| t.translation)
        .unwrap_or_default();
    let air = v.craft.flies();
    let want = stop_kind(air);
    let mut q = world.query::<&Building>();
    let near = q
        .iter(world)
        .filter(|b| b.kind == want)
        .map(|b| berth(b, air))
        .min_by(|a, b| {
            (*a - here)
                .length()
                .partial_cmp(&(*b - here).length())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    let roam = match near {
        Some(at) if air && v.craft != crate::vessels::Craft::Airship => {
            let alt = v.altitude as f64;
            let ring = (0..6)
                .map(|k| {
                    let a = k as f64 / 6.0 * std::f64::consts::TAU;
                    DVec3::new(at.x + 320.0 * a.cos(), alt, at.z + 320.0 * a.sin())
                })
                .collect();
            Roam::Loop {
                points: Arc::new(ring),
            }
        }
        Some(at) => Roam::Idle { at },
        None => Roam::None,
    };
    if let Some(mut v) = world.get_mut::<Vessel>(e) {
        v.route = None;
        v.roam = roam;
        v.next_stop = 0;
        replan(&mut v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::town::{build_start, tests::bare_world};

    fn ids(w: &mut World, kind: Kind) -> Vec<u32> {
        let mut q = w.query::<&Building>();
        let mut v: Vec<u32> = q.iter(w).filter(|b| b.kind == kind).map(|b| b.id).collect();
        v.sort();
        v
    }

    fn on(w: &mut World, r: usize) -> Vec<(Entity, usize)> {
        let mut q = w.query::<(Entity, &Vessel)>();
        q.iter(w)
            .filter(|(_, v)| v.route == Some(r))
            .map(|(e, v)| (e, v.next_stop))
            .collect()
    }

    #[test]
    fn routes_can_be_opened_edited_crewed_and_closed() {
        let mut w = bare_world();
        build_start(&mut w);
        let harbours = ids(&mut w, Kind::Harbour);
        let masts = ids(&mut w, Kind::AirshipMast);
        assert!(harbours.len() >= 3, "harbours {harbours:?}");
        let before = w.resource::<Routes>().0.len();

        // A new sea route with three stops; a mast is refused.
        let r = open_route(&mut w, "Test run", false);
        assert_eq!(r, before);
        for &h in &harbours[..3] {
            add_stop(&mut w, r, h, None, vec![Good::Wool], vec![]).unwrap();
        }
        if let Some(&m) = masts.first() {
            assert!(add_stop(&mut w, r, m, None, vec![], vec![]).is_err());
        }
        set_goods(&mut w, r, 2, vec![], vec![Good::Wool]).unwrap();
        assert_eq!(
            w.resource::<Routes>().0[r].stops[2].unload,
            vec![Good::Wool]
        );

        // Take a ship off route 0 and put it on the new one; an aircraft is refused.
        let (ship, _) = *on(&mut w, 0).first().expect("a ship on route 0");
        assign(&mut w, ship, r).unwrap();
        let bound = w.get::<Vessel>(ship).unwrap().next_stop;
        let target = w.resource::<Routes>().0[r].stops[bound].at;
        let flier = {
            let mut q = w.query::<(Entity, &Vessel)>();
            q.iter(&w).find(|(_, v)| v.craft.flies()).map(|(e, _)| e)
        };
        if let Some(f) = flier {
            assert!(assign(&mut w, f, r).is_err());
        }

        // Stops moving around it keep it bound for the same harbour.
        add_stop(&mut w, r, harbours[0], Some(0), vec![], vec![]).unwrap();
        move_stop(&mut w, r, 3, 0).unwrap();
        let v = w.get::<Vessel>(ship).unwrap();
        assert_eq!(w.resource::<Routes>().0[r].stops[v.next_stop].at, target);
        // Removing its stop sends it on to another.
        let i = w.get::<Vessel>(ship).unwrap().next_stop;
        remove_stop(&mut w, r, i).unwrap();
        let n = w.resource::<Routes>().0[r].stops.len();
        assert_eq!(n, 3);
        assert!(w.get::<Vessel>(ship).unwrap().next_stop < n);

        // Closing route 0 stands its ships down and moves the test route down by one.
        let crew0: Vec<Entity> = on(&mut w, 0).into_iter().map(|(e, _)| e).collect();
        close_route(&mut w, 0).unwrap();
        for e in crew0 {
            let v = w.get::<Vessel>(e).unwrap();
            assert_eq!(v.route, None);
            assert!(matches!(v.roam, Roam::Idle { .. } | Roam::Loop { .. }));
        }
        assert_eq!(w.get::<Vessel>(ship).unwrap().route, Some(r - 1));
        assert_eq!(w.resource::<Routes>().0[r - 1].name, "Test run");

        // Pulling a harbour down takes it off every route.
        forget_building(&mut w, harbours[0]);
        assert!(
            w.resource::<Routes>()
                .0
                .iter()
                .all(|route| route.stops.iter().all(|s| s.at != harbours[0]))
        );
        unassign(&mut w, ship).unwrap();
        assert_eq!(w.get::<Vessel>(ship).unwrap().route, None);
    }
}
