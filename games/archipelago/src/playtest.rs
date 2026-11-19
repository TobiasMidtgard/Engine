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
use crate::settle::StartMode;
use crate::sim::{Building, Economy, House, Producer, Status};
use crate::vessels::Craft;

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
    settled_seen: usize,
    launched_seen: usize,
    incidents_seen: usize,
    /// The run's seed: houses go up two or three at a time by it.
    seed: u64,
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
pub fn run(mut app: App, minutes: f64, speed: f32, shots: Option<&str>, seed: u64, out: &str) {
    let _ = std::fs::create_dir_all(out);
    if let Some(mut inc) = app.world.get_resource_mut::<crate::incidents::Incidents>() {
        inc.reseed(seed);
    }
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
    let mut log = Log {
        seed,
        ..Log::default()
    };
    let c0 = clock(&app.world);
    log.line(
        c0,
        format!("start-up took {startup_s:.1} s of wall time; playing at speed {speed}"),
    );
    let mut rows: Vec<Value> = Vec::new();
    let mut frames: Vec<f32> = Vec::new();
    let mut slow: Vec<Value> = Vec::new();
    // Each seed takes its turns at a different second of the half minute.
    let mut next_turn = c0 + (seed.wrapping_mul(7) % TURN_S as u64) as f64;
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
    let fresh = *world.resource::<StartMode>() == StartMode::Fresh;
    if fresh {
        colonise(world, log, now);
    }
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
            if fresh && crate::settle::claimed(world, i) {
                bootstrap(world, log, now, i);
            }
            continue;
        }
        supply(world, log, now, i);
        services(world, log, now, i);
        move_up(world, log, now, i);
        paving(world, log, now, i);
        safety(world, log, now, i);
        grow(world, log, now, i);
    }
    trade(world, log, now);
    restart_paused(world, log, now);
    incidents_seen(world, log);
}

/// A careful player presses "move up all" each turn: only houses whose tier can spare them
/// go, so the works keep their hands.
fn move_up(world: &mut World, log: &mut Log, now: f64, i: usize) {
    let (moved, waiting) = crate::reports::move_up_all(world, i);
    if moved > 0 {
        log.act("moved houses up");
        let held: u32 = waiting.values().sum();
        log.line(
            now,
            format!(
                "moved {moved} houses up on {}{}",
                islands()[i].name,
                if held > 0 {
                    format!(" ({held} ready ones wait: {waiting:?})")
                } else {
                    String::new()
                }
            ),
        );
    }
}

/// Public buildings where houses ask for one and have none in reach: a market, chapel, tavern
/// or school in the middle of the houses that lack it.
fn services(world: &mut World, log: &mut Log, now: f64, i: usize) {
    if log
        .last
        .get(&(i + 6000, None))
        .is_some_and(|t| now - t < 120.0)
    {
        return;
    }
    log.last.insert((i + 6000, None), now);
    let books = world.resource::<Economy>().islands[i].clone();
    let coins = world.resource::<Economy>().coins;
    for kind in crate::build::SERVICES {
        let mut lacking: Vec<DVec2> = Vec::new();
        for (b, h) in world.query::<(&Building, &House)>().iter(world) {
            if b.island != i || h.has(kind) {
                continue;
            }
            let asked = h.tier.wants().iter().enumerate().any(|(j, w)| {
                w.need == crate::defs::Need::Service(kind) && books.asks(h.tier, j, w)
            });
            if asked {
                lacking.push(crate::grid::footprint_center(b.kind, b.origin, b.rot));
            }
        }
        if lacking.len() < 4 {
            continue;
        }
        let d = kind.def();
        if coins < d.coins as f64 + 600.0
            || !d.cost.iter().all(|(g, n)| books.get(*g) >= *n as f64 + 2.0)
        {
            log.line(
                now,
                format!(
                    "{} houses on {} want a {} but it costs more than there is",
                    lacking.len(),
                    islands()[i].name,
                    d.name
                ),
            );
            continue;
        }
        let mid = lacking.iter().copied().sum::<DVec2>() / lacking.len() as f64;
        // The house nearest the middle, so the building lands among them.
        let at = lacking
            .iter()
            .copied()
            .min_by(|a, b| (*a - mid).length().total_cmp(&(*b - mid).length()))
            .unwrap_or(mid);
        build_near(
            world,
            log,
            now,
            kind,
            at,
            &format!("{} houses without one", lacking.len()),
        );
        // One a turn: let the reach settle before the next.
        return;
    }
}

/// Burghers want a paved street at the door: pave the road beside a ready house that waits
/// for it.
fn paving(world: &mut World, log: &mut Log, now: f64, i: usize) {
    if log
        .last
        .get(&(i + 7000, None))
        .is_some_and(|t| now - t < 120.0)
    {
        return;
    }
    log.last.insert((i + 7000, None), now);
    let roads = world.resource::<crate::grid::Grid>().roads.clone();
    let paved = world.resource::<crate::grid::Grid>().paved.clone();
    let doors: Vec<IVec2> = world
        .query::<(&Building, &House)>()
        .iter(world)
        .filter(|(b, h)| b.island == i && h.wait == crate::sim::Wait::NeedsPaving)
        .filter_map(|(b, _)| {
            crate::grid::ring(b.kind, b.origin, b.rot)
                .into_iter()
                .find(|c| roads.contains(c) && !paved.contains(c))
        })
        .take(6)
        .collect();
    let mut n = 0;
    for c in doors {
        if crate::build::place_road_line(world, c, c, true, true).is_ok() {
            n += 1;
        }
    }
    if n > 0 {
        log.act("paved a street");
        log.line(
            now,
            format!(
                "paved {n} cells for houses that want to be Burghers on {}",
                islands()[i].name
            ),
        );
    }
}

/// Works that paused themselves in debt start again once the purse has recovered.
fn restart_paused(world: &mut World, log: &mut Log, now: f64) {
    let econ = world.resource::<Economy>();
    if econ.coins < 1500.0 || econ.balance_per_min <= 0.0 {
        return;
    }
    let mut n = 0;
    for mut p in world.query::<&mut Producer>().iter_mut(world) {
        if p.auto_paused {
            p.paused = false;
            p.auto_paused = false;
            p.idle_s = 0.0;
            n += 1;
        }
    }
    if n > 0 {
        log.line(
            now,
            format!("Coins back: restarted {n} works that paused in debt"),
        );
    }
}

/// Copy new fires, riots and sicknesses into the log.
fn incidents_seen(world: &mut World, log: &mut Log) {
    let Some(inc) = world.get_resource::<crate::incidents::Incidents>() else {
        return;
    };
    let new: Vec<(f64, String)> = inc.log[log.incidents_seen.min(inc.log.len())..]
        .iter()
        .map(|(t, _, s)| (*t, s.clone()))
        .collect();
    log.incidents_seen = inc.log.len();
    for (t, s) in new {
        log.line(t, format!("INCIDENT {s}"));
    }
}

/// A careful player puts up a Fire Watch once the town is big enough to burn, a Watch House
/// and an Infirmary as it grows, one more of each for every 500 people, and rebuilds ruins.
fn safety(world: &mut World, log: &mut Log, now: f64, i: usize) {
    if log
        .last
        .get(&(i + 5000, None))
        .is_some_and(|t| now - t < 240.0)
    {
        return;
    }
    log.last.insert((i + 5000, None), now);
    let Some(at) = heart(world, i) else { return };
    let books = world.resource::<Economy>().islands[i].clone();
    let pop: f32 = books.population.iter().sum();
    let count = |world: &mut World, k: Kind| {
        world
            .query::<&Building>()
            .iter(world)
            .filter(|b| b.island == i && b.kind == k)
            .count()
    };
    let want = |from: f32| {
        if pop < from {
            0
        } else {
            1 + ((pop - from) / 500.0) as usize
        }
    };
    let coins = world.resource::<Economy>().coins;
    for (kind, from, why) in [
        (Kind::FireWatch, crate::incidents::QUIET_BELOW, "fires"),
        (Kind::WatchHouse, 250.0, "riots"),
        (Kind::Infirmary, 500.0, "sickness"),
    ] {
        let d = kind.def();
        let can = coins > d.coins as f64 + 400.0
            && d.cost.iter().all(|(g, n)| books.get(*g) >= *n as f64 + 4.0);
        if can && count(world, kind) < want(from) {
            build_for(world, log, now, kind, i, at, why);
            break;
        }
    }
    // Ruins: rebuilt when there is money.
    for (id, kind, isle, _) in crate::incidents::ruins(world) {
        if isle != i || world.resource::<Economy>().coins < 1200.0 {
            continue;
        }
        match crate::incidents::rebuild(world, id) {
            Ok(()) => {
                log.act("rebuild a ruin");
                log.line(
                    now,
                    format!(
                        "rebuilt a burnt {} on {}",
                        kind.def().name,
                        islands()[i].name
                    ),
                );
            }
            Err(e) => log.refuse(now, "rebuild", &e),
        }
    }
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
                // (Counted, not written out each turn: it stays true until a fitting craft.)
                *log.refused
                    .entry(format!("fit gear: no vessel takes {}", d.name))
                    .or_default() += 1;
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
        && coins > 8_000.0
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
                            let joined = if kind == Kind::Woodcutter {
                                crate::town::join_far(world, e)
                            } else {
                                crate::town::join_to_roads(world, e)
                            };
                            if joined {
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

/// Where to put a woodcutter on island `i`: the nearest thick wood to `at` that no woodcutter
/// cuts yet.
fn woods_near(world: &mut World, i: usize, at: DVec2) -> Option<DVec2> {
    let cutters: Vec<DVec2> = world
        .query::<&Building>()
        .iter(world)
        .filter(|b| b.kind == Kind::Woodcutter)
        .map(|b| crate::grid::footprint_center(b.kind, b.origin, b.rot))
        .collect();
    let mut best: Option<(DVec2, f64)> = None;
    for j in -40..=40 {
        for k in -40..=40 {
            let p = at + DVec2::new(k as f64, j as f64) * 16.0;
            if crate::island::island_at(p.x, p.y) != Some(i)
                || crate::island::woods(p.x, p.y) < 0.55
                || cutters.iter().any(|c| (*c - p).length() < 90.0)
            {
                continue;
            }
            let d = (p - at).length();
            if best.is_none_or(|b| d < b.1) {
                best = Some((p, d));
            }
        }
    }
    best.map(|b| b.0)
}

/// Build `kind` for island `i` near `at`, or by the woods for a woodcutter.
fn build_for(
    world: &mut World,
    log: &mut Log,
    now: f64,
    kind: Kind,
    i: usize,
    at: DVec2,
    why: &str,
) -> bool {
    let at = if kind == Kind::Woodcutter {
        woods_near(world, i, at).unwrap_or(at)
    } else {
        at
    };
    build_near(world, log, now, kind, at, why)
}

/// Kinds that make `g` and may stand on island `i`, with every input made there too (a chain
/// that cannot run there is no use: the good comes by ship instead).
fn makers(g: Good, i: usize) -> Vec<Kind> {
    makers_within(g, i, 4)
}

fn makers_within(g: Good, i: usize, depth: u32) -> Vec<Kind> {
    if depth == 0 {
        return Vec::new();
    }
    DEFS.iter()
        .filter(|d| d.output == Some(g) && d.category != Category::Public)
        .filter(|d| !d.needs_fertility || islands()[i].fertility.allows(d.kind))
        .filter(|d| {
            d.input
                .into_iter()
                .chain(d.input2)
                .all(|x| !makers_within(x, i, depth - 1).is_empty())
        })
        .map(|d| d.kind)
        .collect()
}

/// Timber to build with: a woodcutter at work and a sawmill whenever planks run low.
fn materials(world: &mut World, log: &mut Log, now: f64, i: usize, at: DVec2) {
    let planks = world.resource::<Economy>().islands[i].get(Good::Planks);
    if planks > 15.0
        || log
            .last
            .get(&(i + 3000, None))
            .is_some_and(|t| now - t < 240.0)
    {
        return;
    }
    log.last.insert((i + 3000, None), now);
    let works: Vec<(Kind, Status)> = world
        .query::<(&Building, &Producer)>()
        .iter(world)
        .filter(|(b, _)| b.island == i)
        .map(|(b, p)| (b.kind, p.status))
        .collect();
    let working = |k: Kind| {
        works
            .iter()
            .filter(|(kk, st)| {
                *kk == k && matches!(st, Status::Working | Status::NoInput | Status::Full)
            })
            .count()
    };
    // Short of hands: more works would stand idle; houses come first.
    if works.iter().any(|(_, st)| *st == Status::NoWorkers) {
        return;
    }
    let (cutters, mills) = (working(Kind::Woodcutter), working(Kind::Sawmill));
    if cutters <= mills {
        build_for(world, log, now, Kind::Woodcutter, i, at, "planks run low");
    }
    if mills == 0 || cutters > mills + 1 {
        build_for(world, log, now, Kind::Sawmill, i, at, "planks run low");
    }
}

/// Bricks once there are Labourers to make them (where the ground has clay), and the chapel
/// Labourers ask for.
fn bricks_and_chapel(world: &mut World, log: &mut Log, now: f64, i: usize, at: DVec2) {
    let books = world.resource::<Economy>().islands[i].clone();
    if books.population[Tier::Labourers.index()] < 40.0
        || log
            .last
            .get(&(i + 4000, None))
            .is_some_and(|t| now - t < 300.0)
    {
        return;
    }
    log.last.insert((i + 4000, None), now);
    let count = |world: &mut World, k: Kind| {
        world
            .query::<&Building>()
            .iter(world)
            .filter(|b| b.island == i && b.kind == k)
            .count()
    };
    if islands()[i].fertility.clay && books.get(Good::Bricks) < 20.0 {
        if count(world, Kind::ClayPit) <= count(world, Kind::Brickworks) {
            build_for(world, log, now, Kind::ClayPit, i, at, "bricks");
        }
        if count(world, Kind::Brickworks) < count(world, Kind::ClayPit) {
            build_for(world, log, now, Kind::Brickworks, i, at, "bricks");
        }
    }
    if count(world, Kind::Chapel) == 0 && books.get(Good::Bricks) >= 6.0 {
        build_for(
            world,
            log,
            now,
            Kind::Chapel,
            i,
            at,
            "Labourers want a chapel",
        );
    }
}

fn supply(world: &mut World, log: &mut Log, now: f64, i: usize) {
    let Some(at) = heart(world, i) else { return };
    materials(world, log, now, i, at);
    bricks_and_chapel(world, log, now, i, at);
    let books = world.resource::<Economy>().islands[i].clone();
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
        if build_for(
            world,
            log,
            now,
            k,
            i,
            at,
            &format!("{} short of {}", islands()[i].name, g.name()),
        ) {
            builds += 1;
            // And what it needs: one maker of each input to each works that uses it.
            for input in [k.def().input, k.def().input2].into_iter().flatten() {
                let Some(&m) = makers(input, i).first() else {
                    continue;
                };
                let count = |world: &mut World, kk: Kind| {
                    world
                        .query::<&Building>()
                        .iter(world)
                        .filter(|b| b.island == i && b.kind == kk)
                        .count()
                };
                if count(world, m) < count(world, k) {
                    build_for(
                        world,
                        log,
                        now,
                        m,
                        i,
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
    // Houses still filling toward what their needs allow: wait for them before building more.
    let (mut held, mut toward) = (0.0f32, 0.0f32);
    for (b, h, boost) in world
        .query::<(&Building, &House, Option<&crate::items::Boost>)>()
        .iter(world)
    {
        if b.island == i {
            held += h.residents;
            toward += crate::sim::appraise(h, &books, boost).residents;
        }
    }
    if toward > 0.0 && held < toward * 0.85 {
        return;
    }
    if log.last.get(&(i, None)).is_some_and(|t| now - t < 120.0) {
        return;
    }
    // (Houses cost timber, not coins: more people are how a short purse fills again.)
    if coins > 200.0 && (fed || short_hands) {
        log.last.insert((i, None), now);
        let Some(at) = heart(world, i) else { return };
        for _ in 0..(2 + log.seed % 2) {
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
                // Make it for export on a settled island whose ground allows the chain.
                let maker = econ
                    .islands
                    .iter()
                    .enumerate()
                    .filter(|(j, x)| *j != to && x.population.iter().sum::<f32>() >= 50.0)
                    .find_map(|(j, _)| makers(g, j).first().map(|k| (j, *k)));
                if let Some((j, k)) = maker
                    && log
                        .last
                        .get(&(j + 8000, Some(g)))
                        .is_none_or(|t| now - t > 900.0)
                    && let Some(at) = heart(world, j)
                {
                    log.last.insert((j + 8000, Some(g)), now);
                    let why = format!("{} to ship to {}", g.name(), islands()[to].name);
                    if build_for(world, log, now, k, j, at, &why) {
                        for input in [k.def().input, k.def().input2].into_iter().flatten() {
                            if let Some(&m) = makers(input, j).first() {
                                build_for(
                                    world,
                                    log,
                                    now,
                                    m,
                                    j,
                                    at,
                                    &format!("input {} for it", input.name()),
                                );
                            }
                        }
                    }
                }
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
            // A new game: the route needs a ship of ours that is off duty.
            let craft = if *world.resource::<StartMode>() == StartMode::Fresh {
                let mut q = world.query::<&Vessel>();
                match q
                    .iter(world)
                    .find(|v| crate::settle::free(v))
                    .map(|v| v.craft)
                {
                    Some(c) => c,
                    None => {
                        log.refuse(now, "open a route", "no ship of ours is off duty");
                        return;
                    }
                }
            } else {
                Craft::Schooner
            };
            let r = engine::core::registry::call_api(
                world,
                "isle.route",
                json!({
                    "name": format!("{} run", g.name()),
                    "craft": craft.name(),
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
                            "opened a {} route {} to {}",
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

// ------------------------------------------------------------------------------ a new game

/// Free ships of ours (off duty, at sea or in port, not aircraft).
fn free_ships(world: &mut World) -> Vec<Vessel> {
    let mut q = world.query::<&Vessel>();
    q.iter(world)
        .filter(|v| crate::settle::free(v))
        .cloned()
        .collect()
}

/// The first works of a new settlement, in order: what Fisherfolk need and the timber to
/// build with.
const FIRST_WORKS: &[(Kind, usize)] = &[
    (Kind::Market, 1),
    (Kind::Woodcutter, 1),
    (Kind::Sawmill, 1),
    (Kind::Fishery, 1),
    (Kind::House, 8),
    (Kind::SheepFarm, 1),
    (Kind::Weaver, 1),
];

/// A settled island with nobody living there yet: lay out its first works and houses.
fn bootstrap(world: &mut World, log: &mut Log, now: f64, i: usize) {
    if log.last.get(&(i, None)).is_some_and(|t| now - t < 45.0) {
        return;
    }
    log.last.insert((i, None), now);
    let Some(at) = heart(world, i) else { return };
    let have = |world: &mut World, k: Kind| {
        world
            .query::<&Building>()
            .iter(world)
            .filter(|b| b.island == i && b.kind == k)
            .count()
    };
    let mut built = 0;
    for &(k, n) in FIRST_WORKS {
        if built >= 4 {
            break;
        }
        if k.def().needs_fertility && !islands()[i].fertility.allows(k) {
            continue;
        }
        while have(world, k) < n && built < 4 {
            if !build_for(world, log, now, k, i, at, "a new settlement") {
                break;
            }
            built += 1;
        }
    }
}

/// Where island `i`'s first harbour should go: its settlement's coast, or the shore facing
/// the home island.
fn landfall(i: usize) -> DVec2 {
    let isle = &islands()[i];
    if let Some(p) = crate::island::settle_coast(isle) {
        return p;
    }
    let home = islands()[0].center;
    let dir = if isle.region() == 0 {
        (home - isle.center).normalize_or_zero()
    } else {
        DVec2::X
    };
    let mut p = isle.center;
    for _ in 0..800 {
        if crate::island::height(p.x, p.y) < 0.5 {
            break;
        }
        p += dir * 5.0;
    }
    p
}

/// Send settlers to island `i`: find a harbour site near its landfall and place it (the game
/// sends the ship). Returns whether one went.
fn found(world: &mut World, log: &mut Log, now: f64, i: usize) -> bool {
    let name = islands()[i].name;
    if log
        .last
        .get(&(i + 1000, None))
        .is_some_and(|t| now - t < 300.0)
    {
        return false;
    }
    log.last.insert((i + 1000, None), now);
    let t = crate::grid::cell_of(landfall(i).x, landfall(i).y);
    for r in 0i32..60 {
        for dz in -r..=r {
            for dx in -r..=r {
                if dx.abs() != r && dz.abs() != r {
                    continue;
                }
                let origin = t + IVec2::new(dx, dz);
                for rot in 0..4u8 {
                    let ok = crate::grid::check(
                        world.resource::<crate::grid::Grid>(),
                        Kind::Harbour,
                        origin,
                        rot,
                    );
                    if ok != Ok(i) {
                        continue;
                    }
                    return match crate::build::place(world, Kind::Harbour, origin, rot, true) {
                        Ok(_) => {
                            log.act("settlers sent");
                            log.line(now, format!("sent settlers to {name}"));
                            true
                        }
                        // A spot with no way inland: the next one.
                        Err(e) if e.contains("road inland") => continue,
                        Err(e) => {
                            log.refuse(now, &format!("settle {name}"), &e);
                            false
                        }
                    };
                }
            }
        }
    }
    log.refuse(
        now,
        &format!("settle {name}"),
        "no harbour site near its landfall",
    );
    false
}

/// A new game: settle Haverholm first, then build a shipyard and ships, and settle the next
/// island whenever a ship is free and home can spare the stores.
fn colonise(world: &mut World, log: &mut Log, now: f64) {
    let n = islands().len();
    let claimed: Vec<bool> = (0..n).map(|i| crate::settle::claimed(world, i)).collect();
    let under_way = world
        .query::<&Vessel>()
        .iter(world)
        .any(|v| matches!(v.roam, crate::fleet::Roam::Found { .. }));
    if !claimed[0] {
        if !under_way {
            found(world, log, now, 0);
        }
        return;
    }
    shipbuilding(world, log, now);
    if under_way {
        return;
    }
    let (coins, home) = {
        let e = world.resource::<Economy>();
        (e.coins, e.islands[0].clone())
    };
    let settled = claimed.iter().filter(|c| **c).count();
    // Each new island asks for a bigger home first.
    let need_people = (150.0 + 150.0 * (settled - 1) as f32).min(900.0);
    if home.population.iter().sum::<f32>() < need_people
        || coins < 3_000.0
        || home.get(Good::Planks) < 60.0
        || free_ships(world).is_empty()
    {
        return;
    }
    // The nearest island of the home region not yet settled, then the far regions in turn.
    let target = (0..n).filter(|&i| !claimed[i]).min_by(|&a, &b| {
        let key = |i: usize| {
            let isle = &islands()[i];
            (
                isle.region(),
                (isle.center - islands()[0].center).length() as i64,
            )
        };
        key(a).cmp(&key(b))
    });
    if let Some(i) = target {
        found(world, log, now, i);
    }
}

/// The yards: a shipyard on the home island once it has people to spare, and a ship ordered
/// whenever none is free and none is building.
fn shipbuilding(world: &mut World, log: &mut Log, now: f64) {
    let coins = world.resource::<Economy>().coins;
    let yards: Vec<(Entity, Building)> = world
        .query::<(Entity, &Building)>()
        .iter(world)
        .filter(|(_, b)| b.kind == Kind::Shipyard)
        .map(|(e, b)| (e, b.clone()))
        .collect();
    if yards.is_empty() {
        let people: f32 = world.resource::<Economy>().islands[0]
            .population
            .iter()
            .sum();
        if people >= 120.0
            && coins > 2_500.0
            && !log.last.get(&(2000, None)).is_some_and(|t| now - t < 300.0)
        {
            log.last.insert((2000, None), now);
            if let Some(at) = heart(world, 0) {
                build_near(world, log, now, Kind::Shipyard, at, "ships to settle with");
            }
        }
        return;
    }
    let building = yards.iter().any(|(e, _)| {
        world
            .get::<crate::shipyard::Yard>(*e)
            .is_some_and(|y| !y.queue.is_empty())
    });
    if building || !free_ships(world).is_empty() || coins < 2_500.0 {
        return;
    }
    if log.last.get(&(2001, None)).is_some_and(|t| now - t < 120.0) {
        return;
    }
    log.last.insert((2001, None), now);
    let (e, b) = &yards[0];
    let labourers = world.resource::<Economy>().islands[b.island].population[1] >= 1.0;
    let craft = if labourers && coins > 6_000.0 {
        Craft::Barque
    } else {
        Craft::Schooner
    };
    match crate::shipyard::order(world, *e, craft) {
        Ok(()) => {
            log.act("ship ordered");
            log.line(now, format!("ordered a {} at the shipyard", craft.name()));
        }
        Err(e) => log.refuse(now, &format!("order a {}", craft.name()), &e),
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
    // Islands settled and craft launched since the last look.
    let settled = world.resource::<crate::settle::Settled>().0.clone();
    for (i, t) in settled.iter().skip(log.settled_seen) {
        log.line(*t, format!("SETTLED {}", islands()[*i].name));
    }
    log.settled_seen = settled.len();
    let launched = world.resource::<crate::shipyard::Launched>().0.clone();
    for (c, n, t) in launched.iter().skip(log.launched_seen) {
        log.line(*t, format!("LAUNCHED {} {n}", c.name()));
    }
    log.launched_seen = launched.len();
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
    let mut happy = 0;
    let mut stuck = 0;
    let mut moods = [[0u32; crate::sim::Mood::COUNT]; Tier::COUNT];
    let mut waits: HashMap<String, u32> = HashMap::new();
    let mut happiness = 0.0;
    for h in q.iter(world) {
        tiers[h.tier.index()] += 1;
        if h.ready {
            content += 1;
            // Ready, nothing stopping it, yet not moved up in a minute: a lock.
            if h.wait == crate::sim::Wait::None && h.content_s > 60.0 {
                stuck += 1;
            }
        }
        if h.is_content() {
            happy += 1;
        }
        happiness += h.happiness;
        moods[h.tier.index()][h.mood().index()] += 1;
        let w = serde_json::to_value(h.wait)
            .ok()
            .map(|v| match v {
                Value::String(s) => s,
                v => v.to_string(),
            })
            .unwrap_or_default();
        *waits.entry(w).or_default() += 1;
    }
    let houses_n: usize = tiers.iter().sum();
    let happiness = happiness / houses_n.max(1) as f32;
    let tail = &frames[frames.len().saturating_sub(450)..];
    let avg = tail.iter().sum::<f32>() / tail.len().max(1) as f32;
    let max = tail.iter().cloned().fold(0.0, f32::max);
    let carts = world.query::<&crate::logistics::Cart>().iter(world).count();
    let vessels = world.query::<&Vessel>().iter(world).count();
    let burning = world
        .query::<&crate::incidents::Incident>()
        .iter(world)
        .count();
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
    let people: Vec<f32> = (0..Tier::COUNT)
        .map(|t| econ.islands.iter().map(|b| b.population[t]).sum())
        .collect();
    let jobs: Vec<f32> = (0..Tier::COUNT)
        .map(|t| econ.islands.iter().map(|b| b.jobs[t]).sum())
        .collect();
    let idle = status
        .iter()
        .filter(|(k, _)| **k != Status::Working.text() && **k != Status::NoWorkers.text())
        .map(|(_, n)| *n)
        .sum::<u32>();
    json!({
        "clock": clock_text(now),
        "minute": (now / 60.0).round(),
        "people": people, "jobs": jobs, "idle_works": idle,
        "public_upkeep": econ.public_upkeep,
        "coins": econ.coins.round(),
        "balance_per_min": econ.balance_per_min.round(),
        "taxes_per_min": econ.taxes_per_min.round(),
        "upkeep_per_min": econ.upkeep_per_min.round(),
        "houses": tiers, "content_houses": content, "happy_houses": happy, "waiting_without_reason": stuck,
        "moods": moods, "waits": waits, "happiness": happiness,
        "debt_s": econ.debt_s, "auto_paused": econ.auto_paused,
        "drains": econ.drains,
        "let_go": (0..Tier::COUNT).map(|t| econ.islands.iter().map(|b| b.let_go[t]).sum::<f32>()).collect::<Vec<_>>(),
        "works": status,
        "carts": carts, "vessels": vessels,
        "settled": world.resource::<crate::settle::Settled>().0.len(),
        "launched": world.resource::<crate::shipyard::Launched>().0.len(),
        "fleet_upkeep": econ.fleet_upkeep,
        "incidents": world.get_resource::<crate::incidents::Incidents>().map(|i| serde_json::to_value(&i.tally).unwrap_or_default()),
        "burning": burning,
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
