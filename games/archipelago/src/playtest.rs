//! `--play <game minutes>`: a scripted player runs the game headless (drawing paused) from the
//! start for that long, and writes what it saw to `screenshots/archipelago/playtest/`:
//! `timeline.json` (a row every game minute), `events.log` (every action and what came of it)
//! and `frames.json` (how long each update took, and the slow ones).
//!
//! The player does what a careful person would: presses orders when they are ready, puts
//! shelf items into empty slots and gear aboard, buys from the pedlar when rich, builds the
//! works for any good an island runs short of (and the works for its inputs), builds houses
//! while the basic needs are met, and opens a route when one island makes what another lacks.
//! Every refusal the game gives is logged, so the log reads as a list of what got in the way.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::time::Instant;

use engine::prelude::*;
use engine::render::RenderPaused;
use serde_json::{Value, json};

use crate::defs::{Category, DEFS, Good, Kind, Tier};
use crate::fleet::{Doing, Vessel};
use crate::island::islands;
use crate::items::{Host, ITEMS, Institution, Items};
use crate::sim::{Building, Economy, House, Producer, Status};

/// Seconds of game time between the player's turns.
const TURN_S: f64 = 30.0;

#[derive(Default)]
struct Log {
    text: String,
    /// Refusals by message, with how often.
    refused: HashMap<String, u32>,
    built: HashMap<&'static str, u32>,
    actions: HashMap<&'static str, u32>,
    /// When the player last acted on (island, good), or grew island (island, None).
    last: HashMap<(usize, Option<Good>), f64>,
}

impl Log {
    fn line(&mut self, clock: f64, s: impl AsRef<str>) {
        let _ = writeln!(self.text, "[{}] {}", clock_text(clock), s.as_ref());
    }
    fn refuse(&mut self, clock: f64, what: &str, why: &str) {
        *self.refused.entry(format!("{what}: {why}")).or_default() += 1;
        self.line(clock, format!("REFUSED {what}: {why}"));
    }
    fn act(&mut self, a: &'static str) {
        *self.actions.entry(a).or_default() += 1;
    }
}

fn clock_text(s: f64) -> String {
    let m = (s / 60.0).floor() as i64;
    format!("{:02}:{:02}:{:02}", m / 60, m % 60, (s % 60.0) as i64)
}

fn clock(world: &World) -> f64 {
    world.resource::<Economy>().clock_s
}

/// Run the scripted game for `minutes` of game time at `speed`.
pub fn run(mut app: App, minutes: f64, speed: f32, shots: Option<&str>) {
    let out = "screenshots/archipelago/playtest";
    let _ = std::fs::create_dir_all(out);
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(engine::core::time::TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.world.insert_resource(RenderPaused);
    let started = Instant::now();
    app.update();
    let startup_s = started.elapsed().as_secs_f64();
    app.world.resource_mut::<Economy>().speed = speed;
    let mut log = Log::default();
    let c0 = clock(&app.world);
    log.line(
        c0,
        format!("start-up took {startup_s:.1} s of wall time; playing at speed {speed}"),
    );
    let mut rows: Vec<Value> = Vec::new();
    let mut frames: Vec<f32> = Vec::new();
    let mut slow: Vec<Value> = Vec::new();
    let mut next_turn = c0;
    let mut next_row = c0;
    let mut stuck: HashMap<u32, (Status, f64)> = HashMap::new();
    let mut moored: HashMap<String, f64> = HashMap::new();
    let mut tiers_seen = [0usize; Tier::COUNT];
    let end = c0 + minutes * 60.0;
    let wall = Instant::now();
    loop {
        let t = Instant::now();
        app.update();
        let ms = t.elapsed().as_secs_f32() * 1000.0;
        frames.push(ms);
        let now = clock(&app.world);
        if ms > 60.0 {
            slow.push(json!({"clock": clock_text(now), "ms": ms}));
        }
        if now >= next_turn {
            let t = Instant::now();
            turn(&mut app.world, &mut log);
            let turn_ms = t.elapsed().as_secs_f32() * 1000.0;
            if turn_ms > 200.0 {
                log.line(
                    now,
                    format!("(the player's turn took {turn_ms:.0} ms of wall time)"),
                );
            }
            watch(
                &mut app.world,
                &mut log,
                &mut stuck,
                &mut moored,
                &mut tiers_seen,
            );
            next_turn += TURN_S;
        }
        if now >= next_row {
            rows.push(row(&mut app.world, &frames));
            next_row += 60.0;
        }
        if now >= end || wall.elapsed().as_secs() > 5 * 3600 {
            break;
        }
    }
    let now = clock(&app.world);
    log.line(
        now,
        format!(
            "played {} of game time in {:.0} s of wall time ({} updates)",
            clock_text(now - c0),
            wall.elapsed().as_secs_f64(),
            frames.len()
        ),
    );
    summary(&mut app.world, &mut log, &stuck, &moored);
    let mut sorted = frames.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let pct = |p: f32| sorted[((sorted.len() as f32 - 1.0) * p) as usize];
    let frames_json = json!({
        "updates": frames.len(),
        "startup_s": startup_s,
        "p50_ms": pct(0.5), "p90_ms": pct(0.9), "p99_ms": pct(0.99), "max_ms": pct(1.0),
        "slow": slow,
    });
    let _ = std::fs::write(
        format!("{out}/frames.json"),
        serde_json::to_string_pretty(&frames_json).unwrap_or_default(),
    );
    let _ = std::fs::write(
        format!("{out}/timeline.json"),
        serde_json::to_string_pretty(&rows).unwrap_or_default(),
    );
    let _ = std::fs::write(format!("{out}/events.log"), &log.text);
    engine::core::log::info!("playtest: done, see {out}/");
    if let Some(keys) = shots {
        app.world.remove_resource::<RenderPaused>();
        app.world.resource_mut::<Economy>().speed = 1.0;
        crate::tour::shoot(&mut app, Some(keys), out);
    }
}

// ------------------------------------------------------------------------------ the player

fn turn(world: &mut World, log: &mut Log) {
    let now = clock(world);
    press_orders(world, log, now);
    fill_slots(world, log, now);
    shop(world, log, now);
    let n = world.resource::<Economy>().islands.len();
    for i in 0..n {
        if world.resource::<Economy>().islands[i]
            .population
            .iter()
            .sum::<f32>()
            < 1.0
        {
            continue;
        }
        supply(world, log, now, i);
        grow(world, log, now, i);
    }
    trade(world, log, now);
}

fn press_orders(world: &mut World, log: &mut Log, now: f64) {
    for e in crate::items::holders(world) {
        let (Some(b), Some(inst)) = (
            world.get::<Building>(e).cloned(),
            world.get::<Institution>(e).cloned(),
        ) else {
            continue;
        };
        for (slot, h) in inst.slots.iter().enumerate() {
            let Some(h) = h else { continue };
            if h.ready() {
                match crate::items::use_item(world, b.id, slot) {
                    Ok(name) => {
                        log.act("order pressed");
                        log.line(now, format!("pressed {name} in {}", b.kind.def().name));
                    }
                    Err(e) => log.refuse(now, "press an order", &e),
                }
            }
        }
    }
    for p in crate::gear::fleet(world) {
        for s in &p.slots {
            if s.item.as_ref().is_some_and(|i| i.ready) {
                match crate::gear::use_item(world, p.vessel, s.slot) {
                    Ok(name) => {
                        log.act("ship order pressed");
                        log.line(now, format!("pressed {name} aboard {}", p.name));
                    }
                    Err(e) => log.refuse(now, "press a ship order", &e),
                }
            }
        }
    }
}

fn fill_slots(world: &mut World, log: &mut Log, now: f64) {
    let shelf = world.resource::<Items>().shelf.clone();
    for h in shelf {
        let d = h.def();
        if d.later > 0 {
            continue;
        }
        if d.host == Host::Ship {
            let fleet = crate::gear::fleet(world);
            let mut done = false;
            for p in &fleet {
                for s in &p.slots {
                    if s.item.is_some() || done {
                        continue;
                    }
                    if crate::gear::socket(world, p.vessel, s.slot, h.uid).is_ok() {
                        log.act("gear fitted");
                        log.line(now, format!("put {} aboard {}", d.name, p.name));
                        done = true;
                    }
                }
            }
            if !done {
                log.refuse(now, "fit gear", &format!("no vessel takes {}", d.name));
            }
            continue;
        }
        let mut done = false;
        for e in crate::items::holders(world) {
            let Some(b) = world.get::<Building>(e).cloned() else {
                continue;
            };
            if b.kind != d.host.kind() {
                continue;
            }
            let open = crate::items::open_slots(world.resource::<Economy>(), b.island);
            let inst = world.get::<Institution>(e).cloned().unwrap_or_default();
            for slot in 0..open.min(inst.slots.len()) {
                if inst.slots[slot].is_some() || done {
                    continue;
                }
                match crate::items::socket(world, b.id, slot, h.uid) {
                    Ok(()) => {
                        log.act("item slotted");
                        log.line(now, format!("put {} into {}", d.name, b.kind.def().name));
                        done = true;
                    }
                    Err(e) => log.refuse(now, "slot an item", &e),
                }
            }
            if done {
                break;
            }
        }
        if !done && world.resource::<Items>().shelf.len() >= 18 {
            // A full shelf: sell the least worth.
            if crate::items::action(world, &format!("sell:{}", h.uid)).is_ok() {
                log.act("item sold");
                log.line(now, format!("sold {} (shelf full)", d.name));
            }
        }
    }
}

fn shop(world: &mut World, log: &mut Log, now: f64) {
    let (offers, coins) = {
        let items = world.resource::<Items>();
        (items.offers.clone(), world.resource::<Economy>().coins)
    };
    if let Some((i, o)) = offers
        .iter()
        .enumerate()
        .filter(|(_, o)| ITEMS[o.item].later == 0)
        .min_by_key(|(_, o)| o.price)
        && coins > o.price as f64 * 4.0
    {
        match crate::items::buy(world, i) {
            Ok(name) => {
                log.act("bought from the pedlar");
                log.line(now, format!("bought {name} for {} coins", o.price));
            }
            Err(e) => log.refuse(now, "buy from the pedlar", &e),
        }
    }
}

/// Where to build on island `i`: its market, else its harbour.
fn heart(world: &mut World, i: usize) -> Option<DVec2> {
    if i == 0
        && let Some(t) = world.get_resource::<crate::town::TownCentre>()
    {
        return Some(DVec2::new(t.0.x, t.0.z));
    }
    let mut q = world.query::<&Building>();
    let mut pick = |k: Kind| {
        q.iter(world)
            .find(|b| b.island == i && b.kind == k)
            .map(|b| crate::grid::footprint_center(b.kind, b.origin, b.rot))
    };
    pick(Kind::Market).or_else(|| pick(Kind::Harbour))
}

/// Build `kind` near `at` on island `i`, paying for it, and join it to the roads.
fn build_near(
    world: &mut World,
    log: &mut Log,
    now: f64,
    kind: Kind,
    at: DVec2,
    why: &str,
) -> bool {
    let t = crate::grid::cell_of(at.x, at.y);
    let (w, d) = kind.def().size;
    let half = IVec2::new(w / 2, d / 2);
    let mut tried = 0;
    for r in 4i32..60 {
        for dz in -r..=r {
            for dx in -r..=r {
                if dx.abs() != r && dz.abs() != r {
                    continue;
                }
                let origin = t + IVec2::new(dx, dz) - half;
                for rot in 0..4u8 {
                    if crate::grid::check(world.resource::<crate::grid::Grid>(), kind, origin, rot)
                        .is_err()
                    {
                        continue;
                    }
                    tried += 1;
                    match crate::build::place(world, kind, origin, rot, true) {
                        Ok(e) => {
                            if crate::town::join_to_roads(world, e) {
                                *log.built.entry(kind.def().name).or_default() += 1;
                                log.line(now, format!("built a {} ({why})", kind.def().name));
                                return true;
                            }
                            crate::build::demolish(world, e);
                            if tried > 12 {
                                log.refuse(
                                    now,
                                    &format!("build a {}", kind.def().name),
                                    "no spot that joins the roads",
                                );
                                return false;
                            }
                        }
                        Err(e) => {
                            log.refuse(now, &format!("build a {}", kind.def().name), &e);
                            return false;
                        }
                    }
                }
            }
        }
    }
    log.refuse(
        now,
        &format!("build a {}", kind.def().name),
        "no free ground within 240 m",
    );
    false
}

/// Kinds that make `g` and may stand on island `i`.
fn makers(g: Good, i: usize) -> Vec<Kind> {
    DEFS.iter()
        .filter(|d| d.output == Some(g) && d.category != Category::Public)
        .filter(|d| !d.needs_fertility || islands()[i].fertility.allows(d.kind))
        .map(|d| d.kind)
        .collect()
}

fn supply(world: &mut World, log: &mut Log, now: f64, i: usize) {
    let books = world.resource::<Economy>().islands[i].clone();
    let Some(at) = heart(world, i) else { return };
    let mut builds = 0;
    for g in Good::ALL {
        let gi = g.index();
        let short =
            books.used_per_min[gi] > books.made_per_min[gi] * 1.05 + 0.2 && books.stock[gi] < 25.0;
        if !short || builds >= 2 {
            continue;
        }
        // Give the last works time to start (a careful player waits a few minutes).
        if log.last.get(&(i, Some(g))).is_some_and(|t| now - t < 360.0) {
            continue;
        }
        // Brought by a route already: wait for the ship.
        let isle_of: HashMap<u32, usize> = world
            .query::<&Building>()
            .iter(world)
            .map(|b| (b.id, b.island))
            .collect();
        let routed = world.resource::<crate::fleet::Routes>().0.iter().any(|r| {
            r.stops
                .iter()
                .any(|s| s.unload.contains(&g) && isle_of.get(&s.at) == Some(&i))
        });
        if routed && books.stock[gi] > 0.5 {
            continue;
        }
        log.last.insert((i, Some(g)), now);
        let ks = makers(g, i);
        let Some(&k) = ks.first() else {
            // Made elsewhere: a route has to bring it.
            continue;
        };
        log.line(
            now,
            format!(
                "{} is short of {} (makes {:.1}, uses {:.1} a minute, {:.0} in store)",
                islands()[i].name,
                g.name(),
                books.made_per_min[gi],
                books.used_per_min[gi],
                books.stock[gi]
            ),
        );
        if build_near(
            world,
            log,
            now,
            k,
            at,
            &format!("{} short of {}", islands()[i].name, g.name()),
        ) {
            builds += 1;
            // And what it needs, if the island makes none.
            for input in [k.def().input, k.def().input2].into_iter().flatten() {
                let made = world.resource::<Economy>().islands[i].made_per_min[input.index()];
                if made < 0.1
                    && let Some(&m) = makers(input, i).first()
                {
                    build_near(
                        world,
                        log,
                        now,
                        m,
                        at,
                        &format!("input {} for it", input.name()),
                    );
                }
            }
        }
    }
}

fn grow(world: &mut World, log: &mut Log, now: f64, i: usize) {
    let (coins, books) = {
        let e = world.resource::<Economy>();
        (e.coins, e.islands[i].clone())
    };
    // Basic needs of the first tier met, and money in the bank: more houses.
    let fed = Tier::Fisherfolk.wants().iter().all(|w| match w.need {
        crate::defs::Need::Good(g) if w.people > 0.0 => books.supply[g.index()] > 0.9,
        _ => true,
    });
    let short_hands = (0..Tier::COUNT).any(|t| books.jobs[t] > 0.0 && books.workforce[t] < 0.9);
    if log.last.get(&(i, None)).is_some_and(|t| now - t < 120.0) {
        return;
    }
    if coins > 3000.0 && (fed || short_hands) {
        log.last.insert((i, None), now);
        let Some(at) = heart(world, i) else { return };
        for _ in 0..2 {
            if !build_near(
                world,
                log,
                now,
                Kind::House,
                at,
                if short_hands {
                    "short of hands"
                } else {
                    "growth"
                },
            ) {
                break;
            }
        }
    }
}

fn trade(world: &mut World, log: &mut Log, now: f64) {
    // One check every five minutes is enough.
    if (now / TURN_S) as i64 % 10 != 0 {
        return;
    }
    let econ = world.resource::<Economy>().clone();
    let routes = world.resource::<crate::fleet::Routes>().0.clone();
    let carried = |g: Good| {
        routes.iter().any(|r| {
            r.stops
                .iter()
                .any(|s| s.load.contains(&g) || s.unload.contains(&g))
        })
    };
    for (to, b) in econ.islands.iter().enumerate() {
        if b.population.iter().sum::<f32>() < 1.0 {
            continue;
        }
        for g in Good::ALL {
            let gi = g.index();
            if b.supply[gi] > 0.8 || b.used_per_min[gi] < 0.2 || !makers(g, to).is_empty() {
                continue;
            }
            let from = econ
                .islands
                .iter()
                .enumerate()
                .filter(|(j, x)| *j != to && x.stock[gi] > 30.0)
                .max_by(|a, c| a.1.stock[gi].total_cmp(&c.1.stock[gi]))
                .map(|(j, _)| j);
            let Some(from) = from else {
                log.line(
                    now,
                    format!(
                        "{} lacks {} and no island has any to spare",
                        islands()[to].name,
                        g.name()
                    ),
                );
                continue;
            };
            if carried(g) {
                log.line(
                    now,
                    format!(
                        "{} lacks {} although a route carries it ({:.0} in store on {})",
                        islands()[to].name,
                        g.name(),
                        econ.islands[from].stock[gi],
                        islands()[from].name
                    ),
                );
                continue;
            }
            let harbour = |w: &mut World, i: usize| {
                w.query::<&Building>()
                    .iter(w)
                    .find(|b| b.island == i && b.kind == Kind::Harbour)
                    .map(|b| b.id)
            };
            let (Some(a), Some(z)) = (harbour(world, from), harbour(world, to)) else {
                log.refuse(now, "open a route", "an island has no harbour");
                continue;
            };
            let r = engine::core::registry::call_api(
                world,
                "isle.route",
                json!({
                    "name": format!("{} run", g.name()),
                    "craft": "schooner",
                    "vessel": format!("Playtest {}", g.name()),
                    "stops": [
                        {"at": a, "load": [g.name()], "unload": []},
                        {"at": z, "load": [], "unload": [g.name()]},
                    ]
                }),
            );
            match r {
                Ok(_) => {
                    log.act("route opened");
                    log.line(
                        now,
                        format!(
                            "opened a {} route {} to {} (the API gave a new schooner for free)",
                            g.name(),
                            islands()[from].name,
                            islands()[to].name
                        ),
                    );
                }
                Err(e) => log.refuse(now, "open a route", &e),
            }
            return;
        }
    }
}

// ------------------------------------------------------------------------------ watching

fn watch(
    world: &mut World,
    log: &mut Log,
    stuck: &mut HashMap<u32, (Status, f64)>,
    moored: &mut HashMap<String, f64>,
    tiers_seen: &mut [usize; Tier::COUNT],
) {
    let now = clock(world);
    let econ = world.resource::<Economy>().clone();
    if !econ.coins.is_finite() {
        log.line(now, "BUG coins are not a number");
    }
    if econ.coins < 0.0 {
        log.line(now, format!("coins below zero: {:.0}", econ.coins));
    }
    for (i, b) in econ.islands.iter().enumerate() {
        for g in Good::ALL {
            let s = b.stock[g.index()];
            if s < -1e-3 || !s.is_finite() {
                log.line(
                    now,
                    format!("BUG {} stock of {} is {s}", islands()[i].name, g.name()),
                );
            }
            if s > b.capacity + b.extra_room + 1.0 {
                log.line(
                    now,
                    format!(
                        "BUG {} holds {s:.0} {} over its room {:.0}",
                        islands()[i].name,
                        g.name(),
                        b.capacity + b.extra_room
                    ),
                );
            }
        }
    }
    // Works stuck in one state for more than five minutes.
    let mut q = world.query::<(&Building, &Producer)>();
    let works: Vec<(Building, Producer)> =
        q.iter(world).map(|(b, p)| (b.clone(), p.clone())).collect();
    for (b, p) in &works {
        let e = stuck.entry(b.id).or_insert((p.status, now));
        if e.0 != p.status {
            *e = (p.status, now);
        } else if p.status != Status::Working
            && p.status != Status::Paused
            && now - e.1 > 300.0
            && (now - e.1) % 600.0 < TURN_S
        {
            log.line(
                now,
                format!(
                    "{} #{} on {} has been '{}' for {:.0} min",
                    b.kind.def().name,
                    b.id,
                    islands()[b.island].name,
                    p.status.text(),
                    (now - e.1) / 60.0
                ),
            );
        }
    }
    // Ships on a route moored or sitting still for long.
    let mut q = world.query::<&Vessel>();
    let vs: Vec<Vessel> = q.iter(world).cloned().collect();
    for v in &vs {
        let idle = v.route.is_none() || matches!(v.doing, Doing::Moored(_));
        if idle {
            let since = *moored.entry(v.name.clone()).or_insert(now);
            if v.route.is_some() && now - since > 600.0 && (now - since) % 600.0 < TURN_S {
                log.line(
                    now,
                    format!(
                        "{} on route {:?} has lain moored {:.0} min",
                        v.name,
                        v.route,
                        (now - since) / 60.0
                    ),
                );
            }
        } else {
            moored.remove(&v.name);
        }
    }
    // Tier milestones.
    let mut q = world.query::<&House>();
    let mut tiers = [0usize; Tier::COUNT];
    for h in q.iter(world) {
        tiers[h.tier.index()] += 1;
    }
    for t in Tier::ALL {
        let n = tiers[t.index()];
        if n > 0 && tiers_seen[t.index()] == 0 {
            log.line(now, format!("first {} house", t.name()));
        }
        if n / 10 > tiers_seen[t.index()] / 10 {
            log.line(now, format!("{n} {} houses", t.name()));
        }
        tiers_seen[t.index()] = n;
    }
}

fn row(world: &mut World, frames: &[f32]) -> Value {
    let now = clock(world);
    let econ = world.resource::<Economy>().clone();
    let mut q = world.query::<(&Building, &Producer)>();
    let mut status: HashMap<&'static str, u32> = HashMap::new();
    for (_, p) in q.iter(world) {
        *status.entry(p.status.text()).or_default() += 1;
    }
    let mut q = world.query::<&House>();
    let mut tiers = [0usize; Tier::COUNT];
    let mut content = 0;
    for h in q.iter(world) {
        tiers[h.tier.index()] += 1;
        if h.content_s > 0.0 {
            content += 1;
        }
    }
    let tail = &frames[frames.len().saturating_sub(450)..];
    let avg = tail.iter().sum::<f32>() / tail.len().max(1) as f32;
    let max = tail.iter().cloned().fold(0.0, f32::max);
    let carts = world.query::<&crate::logistics::Cart>().iter(world).count();
    let vessels = world.query::<&Vessel>().iter(world).count();
    let items = world.resource::<Items>();
    let isles: Vec<Value> = econ
        .islands
        .iter()
        .enumerate()
        .filter(|(_, b)| b.population.iter().sum::<f32>() > 0.0)
        .map(|(i, b)| {
            let short: Vec<&str> = Good::ALL
                .iter()
                .filter(|g| b.used_per_min[g.index()] > 0.1 && b.supply[g.index()] < 0.8)
                .map(|g| g.name())
                .collect();
            json!({
                "name": islands()[i].name,
                "people": b.population,
                "workforce": b.workforce,
                "short": short,
                "planks": b.get(Good::Planks).round(), "bricks": b.get(Good::Bricks).round(),
            })
        })
        .collect();
    json!({
        "clock": clock_text(now),
        "coins": econ.coins.round(),
        "balance_per_min": econ.balance_per_min.round(),
        "taxes_per_min": econ.taxes_per_min.round(),
        "upkeep_per_min": econ.upkeep_per_min.round(),
        "houses": tiers, "content_houses": content,
        "works": status,
        "carts": carts, "vessels": vessels,
        "shelf": items.shelf.len(),
        "update_ms_avg": avg, "update_ms_max": max,
        "islands": isles,
    })
}

fn summary(
    world: &mut World,
    log: &mut Log,
    stuck: &HashMap<u32, (Status, f64)>,
    moored: &HashMap<String, f64>,
) {
    let now = clock(world);
    let mut s = String::from("\n==== SUMMARY ====\n");
    let mut refused: Vec<_> = log.refused.iter().collect();
    refused.sort_by(|a, b| b.1.cmp(a.1));
    s += "Refusals (count):\n";
    for (k, n) in refused.iter().take(40) {
        let _ = writeln!(s, "  {n:5} {k}");
    }
    s += "Built:\n";
    for (k, n) in &log.built {
        let _ = writeln!(s, "  {n:5} {k}");
    }
    s += "Actions:\n";
    for (k, n) in &log.actions {
        let _ = writeln!(s, "  {n:5} {k}");
    }
    s += "Works not working at the end (status, minutes):\n";
    let mut q = world.query::<&Building>();
    let names: HashMap<u32, Building> = q.iter(world).map(|b| (b.id, b.clone())).collect();
    let mut bad: Vec<_> = stuck
        .iter()
        .filter(|(_, (st, _))| *st != Status::Working)
        .filter_map(|(id, (st, t))| names.get(id).map(|b| (b, *st, (now - t) / 60.0)))
        .collect();
    bad.sort_by(|a, b| b.2.total_cmp(&a.2));
    for (b, st, m) in bad.iter().take(40) {
        let _ = writeln!(
            s,
            "  {:30} #{:4} {:12} {:28} {m:.0} min",
            b.kind.def().name,
            b.id,
            islands()[b.island].name,
            st.text()
        );
    }
    s += "Vessels idle at the end (minutes):\n";
    for (k, t) in moored {
        let _ = writeln!(s, "  {k:32} {:.0}", (now - t) / 60.0);
    }
    let state =
        engine::core::registry::call_api(world, "isle.state", json!({})).unwrap_or_default();
    let _ = writeln!(
        s,
        "Final state: {}",
        serde_json::to_string_pretty(&state).unwrap_or_default()
    );
    log.text += &s;
}
