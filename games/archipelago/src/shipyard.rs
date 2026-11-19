//! Building ships and aircraft.
//!
//! A Shipyard lays down ships on its slipway and an Airship mast builds aircraft at its
//! cup, one at a time, from an order list (`order`). Each craft is paid for when it is
//! ordered (coins and goods from the island's warehouse) and needs people of a tier on the
//! island to build it. While it is building its hull stands on the slip (or the envelope
//! hangs at the mast); when it is done it is launched and lies off the yard, off duty, until
//! it is sent somewhere or put on a route. Every craft of the company's costs upkeep.
//!
//! The yard's look is here too: two slips, one with a hull in frame, a timber gantry over
//! them, a sawpit, timber stacks and a sail loft (`model`).

use engine::prelude::*;

use crate::art::{Mat, Parts, hexc, shade, v3};
use crate::defs::{Good, Kind, Tier};
use crate::draw::{Art, spawn_model};
use crate::fittings::Fit;
use crate::fleet::{Doing, Roam, Vessel, mast_point, spawn_vessel};
use crate::globe::Frame;
use crate::grid;
use crate::looks::{ModelKey, Piece};
use crate::sim::{Building, Economy};
use crate::vessels::Craft;

/// What a craft costs to build and where.
#[derive(Clone, Copy, Debug)]
pub struct Plan {
    pub craft: Craft,
    /// Shipyard or Airship mast.
    pub at: Kind,
    pub coins: u32,
    pub cost: &'static [(Good, u32)],
    /// Game seconds on the slip.
    pub build_s: f32,
    /// People of this tier must live on the island.
    pub tier: Tier,
    /// Coins a minute it costs to keep, once launched.
    pub upkeep: f32,
    pub blurb: &'static str,
}

pub const PLANS: &[Plan] = &[
    Plan {
        craft: Craft::Schooner,
        at: Kind::Shipyard,
        coins: 800,
        cost: &[(Good::Planks, 24), (Good::Clothes, 8)],
        build_s: 150.0,
        tier: Tier::Fisherfolk,
        upkeep: 6.0,
        blurb: "Small and quick: two holds of 30. The ship to settle a new island with.",
    },
    Plan {
        craft: Craft::Barque,
        at: Kind::Shipyard,
        coins: 1_500,
        cost: &[(Good::Planks, 40), (Good::Clothes, 16)],
        build_s: 210.0,
        tier: Tier::Labourers,
        upkeep: 8.0,
        blurb: "Three holds of 40, slow under sail. The workhorse of the home routes.",
    },
    Plan {
        craft: Craft::PaddleSteamer,
        at: Kind::Shipyard,
        coins: 2_200,
        cost: &[(Good::Planks, 30), (Good::Bricks, 20)],
        build_s: 240.0,
        tier: Tier::Labourers,
        upkeep: 14.0,
        blurb: "Two holds of 25; she keeps her time in any wind, between near islands.",
    },
    Plan {
        craft: Craft::Steamer,
        at: Kind::Shipyard,
        coins: 3_500,
        cost: &[(Good::Planks, 30), (Good::Bricks, 30), (Good::Glass, 6)],
        build_s: 300.0,
        tier: Tier::Burghers,
        upkeep: 20.0,
        blurb: "Four holds of 50 and fast: the ocean crossings to the far regions.",
    },
    Plan {
        craft: Craft::Clipper,
        at: Kind::Shipyard,
        coins: 4_500,
        cost: &[(Good::Planks, 60), (Good::Clothes, 30)],
        build_s: 320.0,
        tier: Tier::Burghers,
        upkeep: 18.0,
        blurb: "Three holds of 45 and the fastest sails afloat.",
    },
    Plan {
        craft: Craft::Biplane,
        at: Kind::AirshipMast,
        coins: 1_200,
        cost: &[(Good::Planks, 8), (Good::Clothes, 6)],
        build_s: 120.0,
        tier: Tier::Labourers,
        upkeep: 8.0,
        blurb: "Carries nothing, sees far: send it over the fog to find islands.",
    },
    Plan {
        craft: Craft::Seaplane,
        at: Kind::AirshipMast,
        coins: 1_600,
        cost: &[(Good::Planks, 10), (Good::Clothes, 6), (Good::Oil, 6)],
        build_s: 150.0,
        tier: Tier::Labourers,
        upkeep: 10.0,
        blurb: "A scout that lands on the water: far sight over the ocean.",
    },
    Plan {
        craft: Craft::Airship,
        at: Kind::AirshipMast,
        coins: 3_000,
        cost: &[(Good::Planks, 20), (Good::Clothes, 30), (Good::Oil, 10)],
        build_s: 260.0,
        tier: Tier::Labourers,
        upkeep: 22.0,
        blurb: "Two holds of 25, straight over land and sea between masts.",
    },
    Plan {
        craft: Craft::Cutter,
        at: Kind::Shipyard,
        coins: 900,
        cost: &[(Good::Planks, 16), (Good::Clothes, 6)],
        build_s: 130.0,
        tier: Tier::Fisherfolk,
        upkeep: 5.0,
        blurb: "Quick and handy, one hold of 25: a scout, a dispatch boat, a guard for a lane.",
    },
    Plan {
        craft: Craft::Brig,
        at: Kind::Shipyard,
        coins: 1_800,
        cost: &[(Good::Planks, 36), (Good::Clothes, 14)],
        build_s: 200.0,
        tier: Tier::Labourers,
        upkeep: 9.0,
        blurb: "Three holds of 35 and three fitting slots: the trader that can carry guns.",
    },
    Plan {
        craft: Craft::Gunboat,
        at: Kind::Shipyard,
        coins: 2_800,
        cost: &[(Good::Planks, 20), (Good::Bricks, 30)],
        build_s: 240.0,
        tier: Tier::Labourers,
        upkeep: 16.0,
        blurb: "Low, grey and steam-driven, one hold: a heavy gun and a ram for the home waters.",
    },
    Plan {
        craft: Craft::Frigate,
        at: Kind::Shipyard,
        coins: 5_200,
        cost: &[(Good::Planks, 70), (Good::Clothes, 30), (Good::Bricks, 20)],
        build_s: 360.0,
        tier: Tier::Burghers,
        upkeep: 26.0,
        blurb: "Four fitting slots and the strongest hull afloat: the escort and the warship.",
    },
    Plan {
        craft: Craft::Liner,
        at: Kind::Shipyard,
        coins: 6_500,
        cost: &[(Good::Planks, 40), (Good::Bricks, 40), (Good::Glass, 12)],
        build_s: 400.0,
        tier: Tier::Burghers,
        upkeep: 30.0,
        blurb: "Three holds of 40, four fitting slots and the fastest crossings of the ocean.",
    },
    Plan {
        craft: Craft::FlyingBoat,
        at: Kind::AirshipMast,
        coins: 2_600,
        cost: &[(Good::Planks, 14), (Good::Clothes, 10), (Good::Oil, 10)],
        build_s: 220.0,
        tier: Tier::Burghers,
        upkeep: 16.0,
        blurb: "One hold of 20 and a long reach: lands on any water, sees far over the ocean.",
    },
];

pub fn plan(craft: Craft) -> Option<&'static Plan> {
    PLANS.iter().find(|p| p.craft == craft)
}

/// What craft of the company's cost a minute to keep (fishing smacks come with their
/// fishery).
pub fn upkeep_of(craft: Craft) -> f32 {
    plan(craft).map_or(0.0, |p| p.upkeep)
}

/// A yard's order list.
#[derive(Component, Clone, Debug, Default)]
pub struct Yard {
    /// What is on the slip first, then what waits.
    pub queue: Vec<Craft>,
    /// The fittings of each craft in `queue`, in the same order.
    pub fits: Vec<Fit>,
    /// Game seconds the first has been building.
    pub progress_s: f32,
    /// The hull on the slip (its drawn parts).
    pub hull: Vec<Entity>,
    pub launched: u32,
}

impl Yard {
    /// 0..1 through the craft on the slip.
    pub fn share(&self) -> f32 {
        self.queue
            .first()
            .and_then(|c| plan(*c))
            .map_or(0.0, |p| (self.progress_s / p.build_s).min(1.0))
    }
}

/// The most a yard takes on at once.
pub const MAX_QUEUE: usize = 4;

/// Order `craft` at yard `e` with its usual fittings: pay now, build when the slip is free.
pub fn order(world: &mut World, e: Entity, craft: Craft) -> Result<(), String> {
    order_fitted(world, e, craft, crate::fittings::usual(craft))
}

/// Order `craft` at yard `e` built with `fit`: the hull and its fittings are paid for now.
pub fn order_fitted(world: &mut World, e: Entity, craft: Craft, fit: Fit) -> Result<(), String> {
    let b = world
        .get::<Building>(e)
        .cloned()
        .ok_or("no such building")?;
    let p = plan(craft).ok_or(format!("{} can't be built", craft.name()))?;
    if b.kind != p.at {
        return Err(format!(
            "{} is built at a {}, not a {}.",
            craft.name(),
            p.at.def().name,
            b.kind.def().name
        ));
    }
    crate::fittings::check(craft, &fit)?;
    let (fit_coins, fit_cost) = crate::fittings::price(&fit);
    let mut cost: Vec<(Good, u32)> = p.cost.to_vec();
    for (g, n) in fit_cost {
        match cost.iter_mut().find(|x| x.0 == g) {
            Some(x) => x.1 += n,
            None => cost.push((g, n)),
        }
    }
    let coins = p.coins + fit_coins;
    let queued = world.get::<Yard>(e).map_or(0, |y| y.queue.len());
    if queued >= MAX_QUEUE {
        return Err(format!("The yard has {MAX_QUEUE} on its list already."));
    }
    let econ = world.resource::<Economy>();
    let books = &econ.islands[b.island];
    if books.population[p.tier.index()] < 1.0 {
        return Err(format!(
            "Building a {} takes {} on {}.",
            craft.name(),
            p.tier.name(),
            crate::island::islands()[b.island].name
        ));
    }
    if econ.coins < coins as f64 {
        return Err(format!(
            "A {} costs {} coins; you have {:.0}.",
            craft.name(),
            coins,
            econ.coins
        ));
    }
    for (g, n) in &cost {
        if books.get(*g) + 1e-6 < *n as f64 {
            return Err(format!(
                "A {} needs {n} {} on {}; the warehouse has {:.0}.",
                craft.name(),
                g.name(),
                crate::island::islands()[b.island].name,
                books.get(*g)
            ));
        }
    }
    {
        let mut econ = world.resource_mut::<Economy>();
        econ.coins -= coins as f64;
        for (g, n) in &cost {
            econ.islands[b.island].take(*g, *n as f64);
        }
    }
    if world.get::<Yard>(e).is_none() {
        world.entity_mut(e).insert(Yard::default());
    }
    let first = {
        let mut y = world.get_mut::<Yard>(e).unwrap();
        y.queue.push(craft);
        let n = y.queue.len();
        y.fits.resize(n - 1, Fit::default());
        y.fits.push(fit);
        n == 1
    };
    if first {
        lay_down(world, e);
    }
    Ok(())
}

/// Take the last order off yard `e`'s list and get its price back (all of it while it waits,
/// half once it is on the slip).
pub fn cancel(world: &mut World, e: Entity) -> Result<Craft, String> {
    let b = world
        .get::<Building>(e)
        .cloned()
        .ok_or("no such building")?;
    let (craft, fit, on_slip) = {
        let mut y = world.get_mut::<Yard>(e).ok_or("nothing on order")?;
        let craft = y.queue.pop().ok_or("nothing on order")?;
        let fit = if y.fits.len() > y.queue.len() {
            y.fits.pop().unwrap_or_default()
        } else {
            Fit::default()
        };
        let on_slip = y.queue.is_empty();
        if on_slip {
            y.progress_s = 0.0;
        }
        (craft, fit, on_slip)
    };
    let k = if on_slip { 0.5 } else { 1.0 };
    if let Some(p) = plan(craft) {
        let (fc, fg) = crate::fittings::price(&fit);
        let mut econ = world.resource_mut::<Economy>();
        econ.coins += (p.coins + fc) as f64 * k;
        for (g, n) in p.cost.iter().copied().chain(fg) {
            econ.islands[b.island].add(g, (n as f64 * k).floor());
        }
    }
    if on_slip {
        clear_slip(world, e);
    }
    Ok(craft)
}

/// Where a craft stands while it is built at `b`, and which way it faces.
fn slip(b: &Building, craft: Craft) -> (DVec3, f32) {
    if craft.flies() {
        let m = mast_point(b);
        let at = if craft == Craft::Airship {
            m + DVec3::new(-48.0, 4.0, 0.0)
        } else {
            // Planes stand on the ground beside the mast.
            DVec3::new(m.x + 14.0, b.base_y + 1.2, m.z + 10.0)
        };
        return (at, 0.0);
    }
    let c = grid::footprint_center(b.kind, b.origin, b.rot);
    let f = grid::front_dir(b.rot).as_dvec2();
    let side = DVec2::new(-f.y, f.x);
    // The left slip, the hull's middle a little seaward of the yard's.
    let p = c + side * SLIP_X as f64 + f * (craft.half_length() as f64 * 0.35 + 2.0);
    let heading = (-f.y).atan2(f.x) as f32;
    (DVec3::new(p.x, b.base_y + 2.4, p.y), heading)
}

/// Put the first craft of yard `e`'s list on the slip, to be seen building.
fn lay_down(world: &mut World, e: Entity) {
    clear_slip(world, e);
    let Some(b) = world.get::<Building>(e).cloned() else {
        return;
    };
    let Some(craft) = world.get::<Yard>(e).and_then(|y| y.queue.first().copied()) else {
        return;
    };
    let Some(art) = world.get_resource::<Art>().cloned() else {
        return;
    };
    let Some(model) = art.crafts.get(&craft).cloned() else {
        return;
    };
    let (at, heading) = slip(&b, craft);
    let t = world.resource::<Frame>().stand(at, heading);
    let parts = spawn_model(world, &art, &model, t, true);
    if let Some(mut y) = world.get_mut::<Yard>(e) {
        y.hull = parts;
    }
}

fn clear_slip(world: &mut World, e: Entity) {
    let parts = world
        .get_mut::<Yard>(e)
        .map(|mut y| std::mem::take(&mut y.hull))
        .unwrap_or_default();
    for p in parts {
        if world.get_entity(p).is_ok() {
            world.despawn(p);
        }
    }
}

/// A yard is being pulled down: take its hull away (orders are lost).
pub fn forget(world: &mut World, e: Entity) {
    clear_slip(world, e);
}

/// Names for new craft, taken in turn.
const NAMES: &[&str] = &[
    "Kittiwake II",
    "Sea Lark",
    "Good Intent",
    "Morning Tide",
    "Haverholm Lass",
    "Grey Petrel",
    "Brave Endeavour",
    "Marram",
    "Shearwater",
    "Northern Light",
    "Hope of Brannsey",
    "Whimbrel",
    "Saltmarsh",
    "Steadfast",
    "Little Auk",
    "Merrow",
];

/// Build: move every yard's work on, and launch what is finished. Also keeps the fleet's
/// upkeep in the books.
pub fn tick(world: &mut World) {
    let dt = world.resource::<Time>().delta_secs() * world.resource::<Economy>().speed;
    // What the company's craft cost a minute.
    let fleet: f32 = {
        let mut q = world.query::<&Vessel>();
        q.iter(world)
            .filter(|v| v.owned())
            .map(|v| upkeep_of(v.craft) + crate::fittings::upkeep(&v.fit))
            .sum()
    };
    world.resource_mut::<Economy>().fleet_upkeep = fleet;
    if dt <= 0.0 {
        return;
    }
    let mut done = Vec::new();
    {
        let mut q = world.query::<(Entity, &mut Yard)>();
        for (e, mut y) in q.iter_mut(world) {
            let Some(p) = y.queue.first().and_then(|c| plan(*c)) else {
                continue;
            };
            y.progress_s += dt;
            if y.progress_s >= p.build_s {
                done.push(e);
            }
        }
    }
    for e in done {
        launch(world, e);
    }
}

/// The first craft of yard `e` is finished: it goes into the water and lies off the yard.
pub fn launch(world: &mut World, e: Entity) {
    let Some(b) = world.get::<Building>(e).cloned() else {
        return;
    };
    let (craft, fit, n) = {
        let Some(mut y) = world.get_mut::<Yard>(e) else {
            return;
        };
        if y.queue.is_empty() {
            return;
        }
        let craft = y.queue.remove(0);
        let fit = if y.fits.is_empty() {
            Fit::default()
        } else {
            y.fits.remove(0)
        };
        y.progress_s = 0.0;
        y.launched += 1;
        (craft, fit, y.launched)
    };
    clear_slip(world, e);
    let name = {
        let taken: Vec<String> = world
            .query::<&Vessel>()
            .iter(world)
            .map(|v| v.name.clone())
            .collect();
        let k = (b.id as usize * 7 + n as usize) % NAMES.len();
        (0..NAMES.len())
            .map(|i| NAMES[(k + i) % NAMES.len()].to_string())
            .find(|s| !taken.contains(s))
            .unwrap_or(format!("{} {}", craft.name(), b.id * 10 + n))
    };
    let (at, heading) = if craft.flies() {
        let m = mast_point(&b);
        (m + DVec3::new(-48.0, 4.0, 0.0), 0.0)
    } else {
        // Off the yard's front, clear of the slip.
        let c = grid::footprint_center(b.kind, b.origin, b.rot);
        let f = grid::front_dir(b.rot).as_dvec2();
        let side = DVec2::new(-f.y, f.x);
        let mut p = c + f * (30.0 + craft.half_length() as f64) + side * 10.0;
        let mut k = 0;
        while crate::island::height(p.x, p.y) > -4.0 && k < 20 {
            p += f * 10.0;
            k += 1;
        }
        let heading = (-side.y).atan2(side.x) as f32;
        (DVec3::new(p.x, 0.0, p.y), heading)
    };
    let seed = 5_000 + b.id as u64 * 31 + n as u64;
    let v = spawn_vessel(
        world,
        craft,
        &name,
        at,
        heading,
        None,
        Roam::Idle { at },
        seed,
    );
    if let Some(mut vv) = world.get_mut::<Vessel>(v) {
        vv.doing = Doing::Moored(20.0);
        vv.fit = fit;
    }
    // Its fittings go into its kit, and those that show go on the hull.
    world.resource_mut::<crate::build::Changed>().any = true;
    crate::fittings::dress(world, v);
    if world.get::<Yard>(e).is_some_and(|y| !y.queue.is_empty()) {
        lay_down(world, e);
    }
    let now = world.resource::<Economy>().clock_s;
    world
        .resource_mut::<Launched>()
        .0
        .push((craft, name.clone(), now));
    let isle = crate::island::islands()[b.island].name;
    if world.get_resource::<engine_ui::theme::Theme>().is_some() {
        engine_ui::toast::notify(
            world,
            engine_ui::toast::ToastKind::Good,
            "",
            &format!("{name} launched"),
            &format!(
                "A new {} lies off the yard on {isle}, waiting for orders.",
                craft.name().to_lowercase()
            ),
            7.0,
        );
    }
    engine::core::log::info!("archipelago: {} {name} launched on {isle}", craft.name());
}

/// Every craft launched this game, with the game second it went into the water.
#[derive(Resource, Default, Debug)]
pub struct Launched(pub Vec<(Craft, String, f64)>);

// ------------------------------------------------------------------------------ the look

/// Across the yard: the left slip (where the ordered craft is built) and the right one.
const SLIP_X: f32 = -7.0;

#[cfg(test)]
pub const MODELS: &[ModelKey] = &["shipyard", "hull_in_frame"];

pub fn build(key: ModelKey) -> Option<Parts> {
    Some(match key {
        "shipyard" => model(),
        "hull_in_frame" => hull_in_frame(),
        _ => return None,
    })
}

/// The pieces of a shipyard: the yard itself and a hull in frame on the right slip.
pub fn look(seed: u64, out: &mut Vec<Piece>) {
    out.push(Piece::at("shipyard", Vec3::ZERO));
    out.push(Piece::at("hull_in_frame", v3(-SLIP_X, 0.6, -3.0)));
    let r = |k: u64| crate::looks::h01(seed, 600 + k);
    for i in 0..3u64 {
        out.push(Piece::at("log_pile", v3(-15.0 + i as f32 * 2.6, 0.0, -12.5)).yaw(r(i) * 0.3));
    }
    out.push(Piece::at("plank_stack", v3(13.0, 0.0, 1.0)).yaw(1.57));
    out.push(Piece::at("plank_stack", v3(13.0, 0.0, 4.4)).yaw(1.57));
    for i in 0..4u64 {
        out.push(Piece::at("barrel", v3(14.6, 0.0, 8.0 + i as f32 * 0.85)));
    }
    out.push(Piece::at("hand_cart", v3(10.5, 0.0, 9.5)).yaw(0.6));
    out.push(Piece::at("lamp_post", v3(-16.4, 0.0, 12.0)));
    out.push(Piece::at("lamp_post", v3(16.4, 0.0, 12.0)));
}

const TAR: u32 = 0x2e2a26;
const OAK: u32 = 0x8a6a48;
const PALE_OAK: u32 = 0xb08a5e;
const STONE: u32 = 0xc9c2b2;

/// A sloping slab (a slipway): top from `(z0, y0)` to `(z1, y1)` between `x0` and `x1`,
/// with sides down to `bottom`.
#[allow(clippy::too_many_arguments)]
fn ramp(
    p: &mut Parts,
    m: Mat,
    x0: f32,
    x1: f32,
    z0: f32,
    y0: f32,
    z1: f32,
    y1: f32,
    bottom: f32,
    col: Vec4,
) {
    p.quad(
        m,
        v3(x0, y0, z0),
        v3(x0, y1, z1),
        v3(x1, y1, z1),
        v3(x1, y0, z0),
        col,
    );
    let side = shade(col, 0.8);
    p.quad(
        m,
        v3(x0, bottom, z0),
        v3(x0, bottom, z1),
        v3(x0, y1, z1),
        v3(x0, y0, z0),
        side,
    );
    p.quad(
        m,
        v3(x1, y0, z0),
        v3(x1, y1, z1),
        v3(x1, bottom, z1),
        v3(x1, bottom, z0),
        side,
    );
    p.quad(
        m,
        v3(x0, bottom, z0),
        v3(x0, y0, z0),
        v3(x1, y0, z0),
        v3(x1, bottom, z0),
        side,
    );
}

/// The yard (its own space: middle of the footprint at floor height, the sea toward +Z).
/// 36 m across and 32 m deep: two slips running down into the water, a timber gantry over
/// both, a sail loft on the right, a sawpit and the steam box on the left.
pub fn model() -> Parts {
    let mut p = Parts::default();
    let stone = hexc(STONE);
    let oak = hexc(OAK);
    let pale = hexc(PALE_OAK);
    let tar = hexc(TAR);
    // The yard floor: rammed earth and setts on the land half.
    p.cuboid(
        Mat::Cobble,
        v3(-18.0, -3.0, -16.0),
        v3(18.0, 0.0, -2.0),
        shade(stone, 0.85),
        false,
    );
    p.cuboid(
        Mat::Cobble,
        v3(-18.0, -3.0, -2.0),
        v3(-11.5, 0.0, 16.0),
        shade(stone, 0.85),
        false,
    );
    p.cuboid(
        Mat::Cobble,
        v3(11.5, -3.0, -2.0),
        v3(18.0, 0.0, 16.0),
        shade(stone, 0.85),
        false,
    );
    // Two slipways of dressed stone down into the sea, and their groundways.
    for x in [SLIP_X, -SLIP_X] {
        // Gently down across the yard, then steeply into the sea.
        ramp(
            &mut p,
            Mat::Stone,
            x - 4.2,
            x + 4.2,
            -14.0,
            0.05,
            4.0,
            -0.6,
            -10.0,
            stone,
        );
        ramp(
            &mut p,
            Mat::Stone,
            x - 4.2,
            x + 4.2,
            4.0,
            -0.6,
            26.0,
            -7.0,
            -10.0,
            stone,
        );
        for dx in [-1.6, 1.6] {
            let a = v3(x + dx, 0.3, -13.5);
            let m = v3(x + dx, -0.35, 4.0);
            let b = v3(x + dx, -6.6, 25.5);
            p.beam(Mat::Timber, a, m, 0.45, shade(oak, 0.75));
            p.beam(Mat::Timber, m, b, 0.45, shade(oak, 0.75));
        }
    }
    // Quay walls between and beside the slips.
    p.cuboid(
        Mat::Stone,
        v3(-2.6, -8.0, -2.0),
        v3(2.6, 0.0, 16.0),
        stone,
        false,
    );
    p.cuboid(
        Mat::Planks,
        v3(-2.6, -0.2, -2.0),
        v3(2.6, 0.05, 16.0),
        pale,
        true,
    );
    for x in [-11.5f32, 11.5] {
        p.cuboid(
            Mat::Stone,
            v3(x - 0.3, -8.0, -2.0),
            v3(x + 0.3, 0.0, 16.0),
            shade(stone, 0.9),
            false,
        );
    }
    // The gantry: tall posts in pairs either side of each slip, tied across and along, with a
    // travelling beam and a block over each slip.
    let top = 15.0;
    for x in [SLIP_X, -SLIP_X] {
        for z in [-12.0f32, -4.0, 4.0, 12.0] {
            for dx in [-4.6f32, 4.6] {
                p.beam(
                    Mat::Timber,
                    v3(x + dx, 0.0, z),
                    v3(x + dx, top, z),
                    0.4,
                    oak,
                );
            }
            p.beam(
                Mat::Timber,
                v3(x - 4.8, top, z),
                v3(x + 4.8, top, z),
                0.4,
                oak,
            );
            // Knee braces.
            for s in [-1.0f32, 1.0] {
                p.beam(
                    Mat::Timber,
                    v3(x + 4.6 * s, top - 3.0, z),
                    v3(x + 2.4 * s, top, z),
                    0.22,
                    shade(oak, 0.9),
                );
            }
        }
        for dx in [-4.6f32, 4.6] {
            p.beam(
                Mat::Timber,
                v3(x + dx, top, -12.2),
                v3(x + dx, top, 12.2),
                0.4,
                oak,
            );
            p.beam(
                Mat::Timber,
                v3(x + dx, top - 6.0, -12.2),
                v3(x + dx, top - 6.0, 12.2),
                0.25,
                shade(oak, 0.9),
            );
        }
        // The travelling beam, a crab and its falls.
        p.beam(
            Mat::Timber,
            v3(x - 4.8, top + 0.6, 1.5),
            v3(x + 4.8, top + 0.6, 1.5),
            0.5,
            shade(oak, 0.8),
        );
        p.box_at(
            Mat::Iron,
            v3(x + 1.0, top + 0.1, 1.5),
            v3(0.5, 0.4, 0.6),
            0.0,
            tar,
        );
        p.rod(
            Mat::Canvas,
            v3(x + 1.0, top - 0.3, 1.5),
            v3(x + 1.0, 7.0, 1.5),
            0.04,
            4,
            hexc(0xb89c6a),
        );
        p.box_at(
            Mat::Planks,
            v3(x + 1.0, 6.8, 1.5),
            v3(0.35, 0.3, 0.35),
            0.0,
            oak,
        );
        // Staging along both sides: planks on the gantry's lower ties.
        for dx in [-4.0f32, 4.0] {
            p.cuboid(
                Mat::Planks,
                v3(x + dx - 0.6, top - 6.0, -12.0),
                v3(x + dx + 0.6, top - 5.85, 12.0),
                pale,
                true,
            );
        }
    }
    // A roof over the land end of both slips, where the frames are cut and fitted.
    p.gable_roof(
        Mat::Slate,
        Mat::Planks,
        v3(0.0, top, -10.0),
        29.0,
        8.0,
        3.2,
        0.6,
        false,
        Vec4::ONE,
        oak,
    );
    // The sail loft and office on the right: brick below, boarded loft above, a gable door.
    let (x0, x1, z0, z1, wall) = (11.8f32, 17.6, -15.6, -4.0, 7.0);
    p.cuboid(
        Mat::Brick,
        v3(x0, 0.0, z0),
        v3(x1, 3.6, z1),
        hexc(0x9a4a36),
        false,
    );
    p.cuboid(
        Mat::Planks,
        v3(x0 - 0.1, 3.6, z0 - 0.1),
        v3(x1 + 0.1, wall, z1 + 0.1),
        hexc(0x5f4a38),
        false,
    );
    p.gable_roof(
        Mat::Slate,
        Mat::Planks,
        v3((x0 + x1) * 0.5, wall, (z0 + z1) * 0.5),
        z1 - z0,
        x1 - x0,
        3.0,
        0.4,
        true,
        Vec4::ONE,
        hexc(0x5f4a38),
    );
    p.door(v3(x0, 0.0, -8.0), -Vec3::X, 1.6, 2.6, hexc(0x24476b));
    p.windows(
        v3(x0 - 0.02, 5.2, (z0 + z1) * 0.5),
        -Vec3::X,
        4,
        0.9,
        1.2,
        1.6,
        Vec4::ONE,
    );
    p.windows(
        v3(x0 - 0.02, 2.0, -12.5),
        -Vec3::X,
        2,
        0.9,
        1.3,
        2.0,
        Vec4::ONE,
    );
    // Sailcloth drying on a line in front of it.
    for i in 0..3 {
        let z = -2.0 + i as f32 * 2.2;
        p.rod(
            Mat::Timber,
            v3(14.0, 0.0, z),
            v3(14.0, 3.4, z),
            0.07,
            4,
            oak,
        );
    }
    p.quad(
        Mat::Canvas,
        v3(14.0, 3.3, -2.0),
        v3(14.0, 3.3, 2.4),
        v3(14.05, 1.0, 2.4),
        v3(14.05, 1.0, -2.0),
        hexc(0xe8e0cc),
    );
    // The sawpit on the left: a trestle over a pit, a log on it and the steam box's chimney.
    p.cuboid(
        Mat::Planks,
        v3(-17.0, 0.0, -6.0),
        v3(-13.0, 0.15, -1.0),
        shade(oak, 0.6),
        true,
    );
    for z in [-5.5f32, -1.5] {
        for x in [-16.4f32, -13.6] {
            p.beam(Mat::Timber, v3(x, 0.0, z), v3(x, 1.4, z), 0.2, oak);
        }
        p.beam(Mat::Timber, v3(-16.6, 1.4, z), v3(-13.4, 1.4, z), 0.25, oak);
    }
    p.rod(
        Mat::Timber,
        v3(-15.0, 1.75, -6.4),
        v3(-15.0, 1.75, -0.6),
        0.35,
        8,
        pale,
    );
    p.cuboid(
        Mat::Brick,
        v3(-17.4, 0.0, 3.0),
        v3(-13.4, 2.2, 7.0),
        hexc(0x8d4434),
        false,
    );
    p.cuboid(
        Mat::Planks,
        v3(-17.0, 2.2, 3.4),
        v3(-13.8, 3.0, 6.6),
        shade(oak, 0.7),
        false,
    );
    p.cylinder(
        Mat::Brick,
        v3(-16.6, 2.2, 6.2),
        0.45,
        0.4,
        5.0,
        8,
        hexc(0x7a3a2c),
        true,
    );
    p.finish()
}

/// A hull in frame: a keel on its blocks, stem and sternpost, and the ribs, some planked.
pub fn hull_in_frame() -> Parts {
    let mut p = Parts::default();
    let oak = hexc(OAK);
    let pale = hexc(PALE_OAK);
    let (len, beam, depth) = (26.0f32, 3.4, 4.2);
    // Keel blocks and keel (along Z, bow to +Z).
    let mut z = -len * 0.5 + 1.0;
    while z < len * 0.5 - 1.0 {
        p.box_at(
            Mat::Planks,
            v3(0.0, -0.5, z),
            v3(0.5, 0.5, 0.4),
            0.0,
            shade(oak, 0.7),
        );
        z += 2.5;
    }
    p.beam(
        Mat::Timber,
        v3(0.0, 0.2, -len * 0.5),
        v3(0.0, 0.2, len * 0.5),
        0.5,
        oak,
    );
    p.beam(
        Mat::Timber,
        v3(0.0, 0.2, len * 0.5),
        v3(0.0, depth + 0.8, len * 0.5 + 2.2),
        0.4,
        oak,
    );
    p.beam(
        Mat::Timber,
        v3(0.0, 0.2, -len * 0.5),
        v3(0.0, depth + 0.6, -len * 0.5 - 0.6),
        0.4,
        oak,
    );
    // Ribs: half-width by station, round in the bilge.
    let n = 15;
    for i in 0..n {
        let t = i as f32 / (n - 1) as f32;
        let z = -len * 0.5 + 1.2 + t * (len - 2.4);
        let fullness = (1.0 - (2.0 * t - 1.0).powi(2) * 0.85).max(0.15).sqrt();
        let w = beam * fullness;
        for s in [-1.0f32, 1.0] {
            let pts = [
                v3(0.0, 0.3, z),
                v3(s * w * 0.65, 0.6, z),
                v3(s * w * 0.95, 1.6, z),
                v3(s * w, 3.0, z),
                v3(s * w * 0.97, depth, z),
            ];
            for k in 0..pts.len() - 1 {
                p.beam(Mat::Timber, pts[k], pts[k + 1], 0.22, pale);
            }
        }
        // Planking on the after half, up to the turn of the bilge.
        if t < 0.45 {
            for s in [-1.0f32, 1.0] {
                p.quad(
                    Mat::Planks,
                    v3(0.0, 0.3, z - 0.95),
                    v3(s * w * 0.95, 1.6, z - 0.95),
                    v3(s * w * 0.95, 1.6, z + 0.95),
                    v3(0.0, 0.3, z + 0.95),
                    shade(pale, 0.85),
                );
            }
        }
    }
    // The sheer battens.
    for s in [-1.0f32, 1.0] {
        p.beam(
            Mat::Timber,
            v3(s * beam * 0.4, depth, -len * 0.5 + 1.2),
            v3(s * beam * 0.4, depth, len * 0.5 - 1.2),
            0.18,
            oak,
        );
    }
    p.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::town::tests::bare_world;

    /// A shipyard on Haverholm's coast near the harbour, free.
    fn yard(w: &mut World) -> Entity {
        let centre = w.resource::<crate::town::TownCentre>().0;
        let t = grid::cell_of(centre.x, centre.z);
        for r in 0i32..80 {
            for dz in -r..=r {
                for dx in -r..=r {
                    if dx.abs() != r && dz.abs() != r {
                        continue;
                    }
                    for rot in 0..4u8 {
                        let o = t + IVec2::new(dx, dz);
                        if grid::check(w.resource::<grid::Grid>(), Kind::Shipyard, o, rot) == Ok(0)
                            && let Ok(e) = crate::build::place(w, Kind::Shipyard, o, rot, false)
                        {
                            return e;
                        }
                    }
                }
            }
        }
        panic!("no shipyard site on Haverholm");
    }

    #[test]
    fn a_shipyard_builds_paid_ships_for_islands_with_the_people_and_launches_them_off_duty() {
        let mut w = bare_world();
        w.init_resource::<crate::settle::Settled>();
        w.init_resource::<Launched>();
        w.init_resource::<Time>();
        crate::settle::tests::settle_home(&mut w);
        let e = yard(&mut w);
        // Nobody lives there yet: no shipwrights.
        assert!(order(&mut w, e, Craft::Schooner).is_err());
        {
            let mut econ = w.resource_mut::<Economy>();
            econ.islands[0].population[Tier::Fisherfolk.index()] = 60.0;
            econ.islands[0].add(Good::Clothes, 20.0);
        }
        // A barque wants Labourers; an airship wants a mast.
        assert!(
            order(&mut w, e, Craft::Barque)
                .unwrap_err()
                .contains("Labourers")
        );
        assert!(order(&mut w, e, Craft::Airship).is_err());
        let (coins, planks) = {
            let econ = w.resource::<Economy>();
            (econ.coins, econ.islands[0].get(Good::Planks))
        };
        order(&mut w, e, Craft::Schooner).expect("a schooner on order");
        {
            let econ = w.resource::<Economy>();
            assert_eq!(econ.coins, coins - 800.0);
            assert_eq!(econ.islands[0].get(Good::Planks), planks - 24.0);
        }
        // Cancelled on the slip: half back; ordered again.
        cancel(&mut w, e).unwrap();
        assert_eq!(w.resource::<Economy>().coins, coins - 400.0);
        w.resource_mut::<Economy>().islands[0].add(Good::Planks, 40.0);
        w.resource_mut::<Economy>().islands[0].add(Good::Clothes, 20.0);
        order(&mut w, e, Craft::Schooner).unwrap();
        let before = w.query::<&Vessel>().iter(&w).count();
        w.get_mut::<Yard>(e).unwrap().progress_s = 1e6;
        launch(&mut w, e);
        assert_eq!(w.query::<&Vessel>().iter(&w).count(), before + 1);
        assert!(w.get::<Yard>(e).unwrap().queue.is_empty());
        let (name, at_sea) = {
            let l = w.resource::<Launched>().0.clone();
            assert_eq!(l.len(), 1);
            let v = w
                .query::<&Vessel>()
                .iter(&w)
                .find(|v| v.name == l[0].1)
                .cloned()
                .unwrap();
            assert!(v.owned() && v.route.is_none() && crate::settle::free(&v));
            (v.name.clone(), crate::island::height(v.at.x, v.at.z) < -2.0)
        };
        assert!(at_sea, "{name} was launched onto dry land");
        // The fleet's upkeep: the barque of the start and the new schooner.
        tick(&mut w);
        assert_eq!(w.resource::<Economy>().fleet_upkeep, 8.0 + 6.0);
    }
}
