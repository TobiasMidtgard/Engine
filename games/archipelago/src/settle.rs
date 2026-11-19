//! A game from the beginning, and new islands won by ship.
//!
//! A new game starts with nothing ashore: one barque of the company's lies off Haverholm with
//! planks, fish and clothes in its hold, and every other island is unknown under the fog.
//! An island is the company's once it has a harbour. The first harbour of an island cannot be
//! built from the shore: a ship of the company's, off duty and with planks aboard, sails to the
//! spot, lands its stores and raises the harbour (`send_settlers`, then `land` when it is
//! there). Everything else on an island waits for that.
//!
//! The old start, a whole town on four islands and the far regions' settlements already at
//! work, stays as the sandbox start (`--sandbox`, and the tour's pictures).

use engine::prelude::*;

use crate::build;
use crate::defs::{Good, Kind};
use crate::fleet::{Doing, Roam, Vessel, mooring, spawn_vessel};
use crate::island::{self, islands};
use crate::sim::{Building, Economy, HARBOUR_CAPACITY};
use crate::vessels::Craft;

/// How a game begins.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StartMode {
    /// One ship and empty islands.
    #[default]
    Fresh,
    /// The finished town of earlier rounds: four settled home islands and the far regions'
    /// settlements.
    Sandbox,
}

/// Planks a ship must bring to raise an island's first harbour (its cost in goods).
pub fn harbour_planks() -> u32 {
    Kind::Harbour
        .def()
        .cost
        .iter()
        .find(|(g, _)| *g == Good::Planks)
        .map_or(0, |c| c.1)
}

/// The company's barque of a new game, and what it carries.
pub const FIRST_SHIP: &str = "Fair Venture";
pub const FIRST_CARGO: &[(Good, u32)] =
    &[(Good::Planks, 40), (Good::Fish, 30), (Good::Clothes, 20)];
/// The purse of a new game.
pub const FIRST_COINS: f64 = 12_000.0;

/// Whether the company has settled island `isle` (it has a harbour there).
pub fn claimed(world: &World, isle: usize) -> bool {
    world
        .resource::<Economy>()
        .islands
        .get(isle)
        .is_some_and(|b| b.harbours > 0)
}

/// The refusal for building on an island nobody has settled.
pub fn unclaimed_text(isle: usize) -> String {
    format!(
        "Nobody has settled {} yet. Build a harbour on its coast first: a ship of yours, off \
         duty and with {} planks aboard, sails there and raises it.",
        islands()[isle].name,
        harbour_planks()
    )
}

/// A new game: the barque off Haverholm, nothing ashore.
pub fn fresh(world: &mut World) {
    let home = &islands()[0];
    let coast = island::settle_coast(home).unwrap_or(home.center);
    let out = (coast - home.center).normalize_or_zero();
    let ty = island::height(coast.x, coast.y).max(0.0);
    world.insert_resource(crate::town::TownCentre(DVec3::new(coast.x, ty, coast.y)));
    world.resource_mut::<Economy>().coins = FIRST_COINS;
    // Lying to, a good way out, bows to the shore.
    let mut at = coast + out * 420.0;
    let mut k = 0;
    while island::height(at.x, at.y) > -8.0 && k < 20 {
        at += out * 40.0;
        k += 1;
    }
    let heading = (out.y).atan2(-out.x) as f32;
    let e = spawn_vessel(
        world,
        Craft::Barque,
        FIRST_SHIP,
        DVec3::new(at.x, 0.0, at.y),
        heading,
        None,
        Roam::Idle {
            at: DVec3::new(at.x, 0.0, at.y),
        },
        4_242,
    );
    if let Some(mut v) = world.get_mut::<Vessel>(e) {
        v.cargo = FIRST_CARGO.to_vec();
        v.doing = Doing::Moored(30.0);
    }
    crate::fleet::spawn_traffic(world, false);
    engine::core::log::info!(
        "archipelago: a new game; {FIRST_SHIP} lies off {} with {:?}",
        home.name,
        FIRST_CARGO
    );
}

/// Whether a vessel is one of the company's and free for an errand (not on a route, not on
/// one already).
pub fn free(v: &Vessel) -> bool {
    !v.craft.flies() && v.route.is_none() && matches!(v.roam, Roam::Idle { .. })
}

/// A harbour of the company's whose mooring is within `reach` meters of `p`.
fn harbour_near(world: &mut World, p: DVec2, reach: f64) -> Option<Building> {
    let mut q = world.query::<&Building>();
    q.iter(world)
        .filter(|b| b.kind == Kind::Harbour)
        .map(|b| (b.clone(), (mooring(b).0 - p).length()))
        .filter(|(_, d)| *d < reach)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(b, _)| b)
}

/// How near one of the company's harbours a ship must lie to take stores aboard from it (off
/// the quay, or off the shipyard that launched her).
pub const PROVISION_M: f64 = 600.0;

pub fn planks_aboard(v: &Vessel) -> u32 {
    v.cargo
        .iter()
        .filter(|(g, _)| *g == Good::Planks)
        .map(|c| c.1)
        .sum()
}

/// A harbour the player placed on an island nobody has settled: send the nearest free ship
/// with the planks for it (one lying at a harbour of the company's takes the settlers' stores
/// aboard there first). The harbour goes up when it arrives.
pub fn send_settlers(world: &mut World, origin: IVec2, rot: u8, isle: usize) -> Result<(), String> {
    let name = islands()[isle].name;
    let need = harbour_planks();
    let coins = Kind::Harbour.def().coins as f64;
    if world.resource::<Economy>().coins < coins {
        return Err(format!(
            "A harbour costs {coins:.0} coins; you have {:.0}.",
            world.resource::<Economy>().coins
        ));
    }
    {
        let mut q = world.query::<&Vessel>();
        if let Some(v) = q
            .iter(world)
            .find(|v| matches!(v.roam, Roam::Found { isle: i, .. } if i == isle))
        {
            return Err(format!(
                "{} is already on its way to settle {name}.",
                v.name
            ));
        }
    }
    if street_from(world, origin, rot, 3).len() < 3 {
        return Err(format!(
            "The harbour needs dry, fairly flat land behind it on {name} for a road inland."
        ));
    }
    let site = grid_center(origin, rot);
    let mut ships: Vec<(Entity, Vessel)> = {
        let mut q = world.query::<(Entity, &Vessel)>();
        q.iter(world)
            .filter(|(_, v)| free(v))
            .map(|(e, v)| (e, v.clone()))
            .collect()
    };
    if ships.is_empty() {
        return Err(format!(
            "Settling {name} takes a ship of yours that is off duty. Build one at a shipyard, \
             or take one off its route."
        ));
    }
    ships.sort_by(|a, b| {
        let d = |v: &Vessel| (DVec2::new(v.at.x, v.at.z) - site).length();
        d(&a.1).total_cmp(&d(&b.1))
    });
    // The nearest with the planks aboard, or the nearest lying at a harbour with them ashore.
    let mut chosen = None;
    for (e, v) in &ships {
        if planks_aboard(v) >= need {
            chosen = Some(*e);
            break;
        }
        let here = DVec2::new(v.at.x, v.at.z);
        if let Some(h) = harbour_near(world, here, PROVISION_M)
            && provision(world, *e, h.island) >= need
        {
            chosen = Some(*e);
            break;
        }
    }
    // None has them: the nearest free ship calls first at the harbour with the most planks.
    let mut stores = None;
    let chosen = match chosen {
        Some(e) => e,
        None => {
            let best = {
                let econ = world.resource::<Economy>().clone();
                let mut q = world.query::<&Building>();
                q.iter(world)
                    .filter(|b| b.kind == Kind::Harbour)
                    .filter(|b| econ.islands[b.island].get(Good::Planks) >= (need + 12) as f64)
                    .max_by(|a, b| {
                        let p = |x: &Building| econ.islands[x.island].get(Good::Planks);
                        p(a).total_cmp(&p(b))
                    })
                    .cloned()
            };
            let Some(h) = best else {
                return Err(format!(
                    "Settling {name} takes {need} planks aboard a ship that is off duty, and no \
                     harbour of yours has planks to spare for her."
                ));
            };
            stores = Some((h.id, islands()[h.island].name));
            let at = mooring(&h).0;
            ships.sort_by(|a, b| {
                let d = |v: &Vessel| (DVec2::new(v.at.x, v.at.z) - at).length();
                d(&a.1).total_cmp(&d(&b.1))
            });
            ships[0].0
        }
    };
    let e = chosen;
    let ship = {
        let mut v = world.get_mut::<Vessel>(e).ok_or("no such vessel")?;
        v.roam = Roam::Found {
            isle,
            origin,
            rot,
            stores: stores.map(|s| s.0),
        };
        v.doing = Doing::Lingering(0.0);
        v.name.clone()
    };
    let text = match stores {
        Some((_, at)) => format!(
            "{ship} calls at {at} for the settlers' stores, then sails for {name}. The harbour \
             goes up when she is there."
        ),
        None => format!(
            "{ship} weighs anchor with the settlers' stores. The harbour goes up when she is there."
        ),
    };
    tell(world, &format!("Settling {name}"), &text);
    engine::core::log::info!("archipelago: {ship} sails to settle {name} at {origin}");
    Ok(())
}

/// Take the settlers' stores aboard `e` from island `isle`'s warehouse: planks for the hold,
/// then fish and clothes for the first houses. Returns the planks aboard after.
pub fn provision(world: &mut World, e: Entity, isle: usize) -> u32 {
    let Some(v) = world.get::<Vessel>(e).cloned() else {
        return 0;
    };
    let (slots, per) = v.craft.hold();
    let slots = (slots + v.kit.hold) as usize;
    let mut cargo = v.cargo.clone();
    {
        let mut econ = world.resource_mut::<Economy>();
        let books = &mut econ.islands[isle];
        for g in [Good::Planks, Good::Fish, Good::Clothes] {
            let have = cargo.iter().position(|c| c.0 == g);
            if have.is_none() && cargo.len() >= slots {
                break;
            }
            let aboard = have.map_or(0, |i| cargo[i].1);
            let room = per.saturating_sub(aboard);
            // Leave the island a little to go on with.
            let spare = (books.get(g) - 6.0).floor().max(0.0) as u32;
            let n = room.min(spare);
            if n == 0 {
                continue;
            }
            books.take(g, n as f64);
            match have {
                Some(i) => cargo[i].1 += n,
                None => cargo.push((g, n)),
            }
        }
    }
    let planks = cargo
        .iter()
        .filter(|(g, _)| *g == Good::Planks)
        .map(|c| c.1)
        .sum();
    if let Some(mut v) = world.get_mut::<Vessel>(e) {
        v.cargo = cargo;
    }
    planks
}

/// The best way inland from a harbour at `origin`: the ring cell (and the run of cells
/// straight on from it, away from the sea) that gives the longest dry road, up to `len`.
fn street_from(world: &World, origin: IVec2, rot: u8, len: i32) -> Vec<IVec2> {
    use crate::grid;
    let g = world.resource::<grid::Grid>();
    let back = -grid::front_dir(rot);
    let mut best: Vec<IVec2> = Vec::new();
    for start in grid::ring(Kind::Harbour, origin, rot) {
        let mut run = Vec::new();
        let mut c = start;
        while (run.len() as i32) < len && g.is_open(c) && grid::cell_roadable(c) {
            run.push(c);
            c += back;
        }
        if run.len() > best.len() {
            best = run;
        }
    }
    best
}

/// Lay [`street_from`] (free: it comes with the settlers).
fn first_street(world: &mut World, origin: IVec2, rot: u8, len: i32) {
    for c in street_from(world, origin, rot, len) {
        let _ = build::place_road(world, c, false);
    }
}

/// The middle of a harbour's footprint at `origin`.
fn grid_center(origin: IVec2, rot: u8) -> DVec2 {
    crate::grid::footprint_center(Kind::Harbour, origin, rot)
}

/// Where a ship bound to raise a harbour at `origin` lies to: the harbour's own mooring.
pub fn landing(origin: IVec2, rot: u8, isle: usize) -> DVec2 {
    let b = Building {
        kind: Kind::Harbour,
        origin,
        rot,
        island: isle,
        base_y: 0.0,
        seed: 0,
        id: 0,
    };
    mooring(&b).0
}

/// The settlers are there: raise the harbour, land the stores, and the island is the
/// company's. On failure (the spot was taken, the purse is empty) the ship waits there.
pub fn land(world: &mut World, v: &mut Vessel) {
    let Roam::Found {
        isle, origin, rot, ..
    } = v.roam.clone()
    else {
        return;
    };
    let name = islands()[isle].name;
    let here = v.path.last().copied().unwrap_or(v.at);
    v.roam = Roam::Idle { at: here };
    v.doing = Doing::Moored(20.0);
    let need = harbour_planks();
    let coins = Kind::Harbour.def().coins as f64;
    let fail = |world: &mut World, why: String| {
        crate::hud::complain(world, &format!("Settling {name}"), &why);
        engine::core::log::info!("archipelago: settling {name} failed: {why}");
    };
    if planks_aboard(v) < need {
        return fail(
            world,
            format!("{} has fewer than {need} planks aboard.", v.name),
        );
    }
    if world.resource::<Economy>().coins < coins {
        return fail(world, format!("A harbour costs {coins:.0} coins."));
    }
    let e = match build::place(world, Kind::Harbour, origin, rot, false) {
        Ok(e) => e,
        Err(why) => return fail(world, format!("The harbour can't go up there: {why}")),
    };
    world.resource_mut::<Economy>().coins -= coins;
    // A street inland from the warehouse, for the first works and houses to join.
    first_street(world, origin, rot, 12);
    let mut left = need;
    for (g, n) in v.cargo.iter_mut() {
        if *g == Good::Planks {
            let k = (*n).min(left);
            *n -= k;
            left -= k;
        }
    }
    // Everything else aboard goes into the new warehouse.
    {
        let mut econ = world.resource_mut::<Economy>();
        let books = &mut econ.islands[isle];
        books.capacity = books.capacity.max(books.harbours as f64 * HARBOUR_CAPACITY);
        for (g, n) in v.cargo.drain(..) {
            books.add(g, n as f64);
        }
    }
    if let Some(b) = world.get::<Building>(e).cloned() {
        let (m, _) = mooring(&b);
        v.roam = Roam::Idle {
            at: DVec3::new(m.x, 0.0, m.y),
        };
    }
    let first = world
        .query::<&Building>()
        .iter(world)
        .filter(|b| b.kind == Kind::Harbour)
        .count()
        == 1;
    if first {
        crate::town::pedlar_calls(world, e);
    }
    let now = world.resource::<Economy>().clock_s;
    world.resource_mut::<Settled>().0.push((isle, now));
    tell(
        world,
        &format!("{name} settled"),
        &format!(
            "{} has raised the harbour and landed her stores. The island is the company's.",
            v.name
        ),
    );
    engine::core::log::info!(
        "archipelago: {name} settled by {} at {:.0} s",
        v.name,
        world.resource::<Economy>().clock_s
    );
}

/// A settlers' ship calling for its stores is at the harbour: take them aboard and sail on.
pub fn take_stores(world: &mut World, v: &mut Vessel) {
    let Roam::Found {
        isle,
        origin,
        rot,
        stores: Some(id),
    } = v.roam.clone()
    else {
        return;
    };
    v.roam = Roam::Found {
        isle,
        origin,
        rot,
        stores: None,
    };
    v.doing = Doing::Moored(12.0);
    let Some(b) = crate::fleet::building_by_id(world, id) else {
        return;
    };
    // (`provision` works on the entity: put the vessel's state back for it, then read it.)
    let e = {
        let mut q = world.query::<(Entity, &Vessel)>();
        q.iter(world)
            .find(|(_, x)| x.name == v.name)
            .map(|(e, _)| e)
    };
    if let Some(e) = e {
        if let Some(mut slot) = world.get_mut::<Vessel>(e) {
            slot.cargo = v.cargo.clone();
        }
        provision(world, e, b.island);
        if let Some(slot) = world.get::<Vessel>(e) {
            v.cargo = slot.cargo.clone();
        }
    }
}

/// Islands settled in this game and when (game seconds), for the playtest and the ledger.
#[derive(Resource, Default, Debug)]
pub struct Settled(pub Vec<(usize, f64)>);

/// Send a free ship of the company's to a point at sea (to look round under the fog).
pub fn sail_to(world: &mut World, e: Entity, to: DVec2) -> Result<(), String> {
    let mut v = world.get_mut::<Vessel>(e).ok_or("no such vessel")?;
    if v.route.is_some() {
        return Err(format!("{} is on a route; take her off it first.", v.name));
    }
    if !matches!(v.roam, Roam::Idle { .. } | Roam::Found { .. }) {
        return Err(format!("{} is not one of the company's.", v.name));
    }
    let y = if v.craft.flies() {
        v.altitude as f64
    } else {
        0.0
    };
    v.roam = Roam::Idle {
        at: DVec3::new(to.x, y, to.y),
    };
    v.doing = Doing::Lingering(0.0);
    Ok(())
}

fn tell(world: &mut World, title: &str, text: &str) {
    if world.get_resource::<engine_ui::theme::Theme>().is_none() {
        return;
    }
    engine_ui::toast::notify(
        world,
        engine_ui::toast::ToastKind::Info,
        "",
        title,
        text,
        7.0,
    );
}

/// `isle.yard.*`, `isle.ship.*` and `isle.claims`: building craft and settling, for scripts,
/// the playtest and the interface.
pub fn register(app: &mut App) {
    use serde_json::{Value, json};
    fn yard_of(world: &mut World, args: &Value) -> Result<Entity, String> {
        let id = args["yard"].as_u64().ok_or("yard wants a building id")? as u32;
        build::by_id(world, id).ok_or(format!("no building {id}"))
    }
    fn vessel_of(world: &mut World, args: &Value) -> Result<Entity, String> {
        let key = match &args["vessel"] {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            _ => return Err("vessel wants a name or an id".into()),
        };
        crate::orders::find_vessel(world, &key).ok_or(format!("no vessel {key}"))
    }
    app.register_api(
        "isle.yards",
        "Every shipyard and airship mast: what is on its slip and how far, what waits, and what it can build.",
        "{}",
        |world, _| {
            use crate::shipyard::{PLANS, Yard};
            let mut q = world.query::<(&Building, Option<&Yard>)>();
            let yards: Vec<Value> = q
                .iter(world)
                .filter(|(b, _)| matches!(b.kind, Kind::Shipyard | Kind::AirshipMast))
                .map(|(b, y)| {
                    let plans: Vec<&str> = PLANS.iter().filter(|p| p.at == b.kind).map(|p| p.craft.name()).collect();
                    json!({
                        "id": b.id,
                        "kind": b.kind.def().name,
                        "island": b.island,
                        "queue": y.map(|y| y.queue.iter().map(|c| c.name()).collect::<Vec<_>>()).unwrap_or_default(),
                        "progress": y.map_or(0.0, |y| y.share()),
                        "builds": plans,
                    })
                })
                .collect();
            let plans: Vec<Value> = PLANS
                .iter()
                .map(|p| {
                    json!({
                        "craft": p.craft.name(), "at": p.at.def().name, "coins": p.coins,
                        "cost": p.cost.iter().map(|(g, n)| json!([g.name(), n])).collect::<Vec<_>>(),
                        "build_s": p.build_s, "tier": p.tier.name(), "upkeep": p.upkeep, "blurb": p.blurb,
                    })
                })
                .collect();
            Ok(json!({"yards": yards, "plans": plans}))
        },
    )
    .register_api(
        "isle.yard.order",
        "Order a craft at a shipyard (ships) or airship mast (aircraft); it is paid for now and launched when built.",
        r#"{"yard": 12, "craft": "schooner"}"#,
        |world, args| {
            let e = yard_of(world, &args)?;
            let craft = args["craft"].as_str().and_then(Craft::from_name).ok_or("unknown craft")?;
            crate::shipyard::order(world, e, craft)?;
            Ok(json!({"queue": world.get::<crate::shipyard::Yard>(e).map(|y| y.queue.len())}))
        },
    )
    .register_api(
        "isle.yard.cancel",
        "Take the last order off a yard's list: all its price back while it waits, half once it is on the slip.",
        r#"{"yard": 12}"#,
        |world, args| {
            let e = yard_of(world, &args)?;
            let c = crate::shipyard::cancel(world, e)?;
            Ok(json!({"cancelled": c.name()}))
        },
    )
    .register_api(
        "isle.ship.goto",
        "Send a craft of yours that is off duty to a point (map meters), to look round under the fog.",
        r#"{"vessel": "Fair Venture", "to": [1200.0, -800.0]}"#,
        |world, args| {
            let e = vessel_of(world, &args)?;
            let to = DVec2::new(
                args["to"][0].as_f64().ok_or("to wants [x, z] in meters")?,
                args["to"][1].as_f64().ok_or("to wants [x, z] in meters")?,
            );
            sail_to(world, e, to)?;
            Ok(json!({"ok": true}))
        },
    )
    .register_api(
        "isle.ship.provision",
        "Take the settlers' stores (planks, fish, clothes) aboard a craft lying at one of your harbours.",
        r#"{"vessel": "Fair Venture"}"#,
        |world, args| {
            let e = vessel_of(world, &args)?;
            let v = world.get::<Vessel>(e).cloned().ok_or("no such vessel")?;
            let h = harbour_near(world, DVec2::new(v.at.x, v.at.z), PROVISION_M).ok_or(format!("{} is not lying at a harbour of yours", v.name))?;
            let planks = provision(world, e, h.island);
            let cargo = world.get::<Vessel>(e).map(|v| v.cargo.iter().map(|(g, n)| json!([g.name(), n])).collect::<Vec<_>>());
            Ok(json!({"planks": planks, "cargo": cargo}))
        },
    )
    .register_api(
        "isle.claims",
        "Which islands are the company's, when each was settled (game seconds), and the craft launched so far.",
        "{}",
        |world, _| {
            let isles: Vec<Value> = islands()
                .iter()
                .enumerate()
                .map(|(i, isle)| json!({"index": i, "name": isle.name, "region": isle.region(), "claimed": claimed(world, i), "center": [isle.center.x, isle.center.y], "settle_coast": island::settle_coast(isle).map(|p| [p.x, p.y])}))
                .collect();
            let settled: Vec<Value> = world.resource::<Settled>().0.iter().map(|(i, t)| json!({"island": islands()[*i].name, "at_s": t})).collect();
            let launched: Vec<Value> = world.resource::<crate::shipyard::Launched>().0.iter().map(|(c, n, t)| json!({"craft": c.name(), "name": n, "at_s": t})).collect();
            Ok(json!({"islands": isles, "settled": settled, "launched": launched, "fleet_upkeep": world.resource::<Economy>().fleet_upkeep}))
        },
    );
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::sim::{Producer, RoadLink};
    use crate::town::tests::bare_world;

    /// A harbour site on island `i` near its landfall.
    pub(crate) fn site(w: &World, i: usize) -> (IVec2, u8) {
        let p = island::settle_coast(&islands()[i]).unwrap();
        let t = crate::grid::cell_of(p.x, p.y);
        for r in 0i32..60 {
            for dz in -r..=r {
                for dx in -r..=r {
                    if dx.abs() != r && dz.abs() != r {
                        continue;
                    }
                    for rot in 0..4u8 {
                        let o = t + IVec2::new(dx, dz);
                        if crate::grid::check(
                            w.resource::<crate::grid::Grid>(),
                            Kind::Harbour,
                            o,
                            rot,
                        ) == Ok(i)
                        {
                            return (o, rot);
                        }
                    }
                }
            }
        }
        panic!("no harbour site on island {i}");
    }

    pub(crate) fn settle_home(w: &mut World) -> Entity {
        fresh(w);
        let (o, rot) = site(w, 0);
        // Nothing but a harbour may go up before the island is settled.
        assert!(build::place(w, Kind::Market, o + IVec2::new(0, -12), 0, true).is_err());
        build::place(w, Kind::Harbour, o, rot, true).expect("settlers sent");
        let e = crate::orders::find_vessel(w, FIRST_SHIP).unwrap();
        let mut v = w.get::<Vessel>(e).cloned().unwrap();
        assert!(matches!(v.roam, Roam::Found { isle: 0, .. }));
        land(w, &mut v);
        *w.get_mut::<Vessel>(e).unwrap() = v;
        e
    }

    #[test]
    fn a_new_game_settles_by_ship_and_its_first_works_reach_the_harbour() {
        let mut w = bare_world();
        w.init_resource::<Settled>();
        assert!(!claimed(&w, 0));
        let ship = settle_home(&mut w);
        assert!(claimed(&w, 0));
        let econ = w.resource::<Economy>();
        assert!(econ.coins < FIRST_COINS, "the harbour was paid for");
        // The stores went ashore: 40 planks less the harbour's 8, fish and clothes.
        assert_eq!(econ.islands[0].get(Good::Planks), 32.0);
        assert_eq!(econ.islands[0].get(Good::Fish), 30.0);
        assert!(w.get::<Vessel>(ship).unwrap().cargo.is_empty());
        assert!(
            !w.resource::<crate::grid::Grid>().roads.is_empty(),
            "a street inland"
        );
        // A woodcutter near the town joins the street and so the harbour.
        let centre = w.resource::<crate::town::TownCentre>().0;
        let mut placed = None;
        'out: for r in 6i32..40 {
            for dx in -r..=r {
                for dz in [-r, r] {
                    let c = crate::grid::cell_of(centre.x, centre.z) + IVec2::new(dx, dz);
                    if let Ok(e) = build::place(&mut w, Kind::Woodcutter, c, 0, true) {
                        if crate::town::join_to_roads(&mut w, e) {
                            placed = Some(e);
                            break 'out;
                        }
                        build::demolish(&mut w, e);
                    }
                }
            }
        }
        let e = placed.expect("a woodcutter joined to the roads");
        w.resource_mut::<build::Changed>().any = true;
        build::refresh(&mut w);
        let linked = w.get::<RoadLink>(e).is_some_and(|l| l.path.is_some());
        let status = w.get::<Producer>(e).map(|p| p.status);
        assert!(linked, "woodcutter has no road to the harbour ({status:?})");
    }
}
