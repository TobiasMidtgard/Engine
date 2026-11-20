//! Fires, riots and sickness.
//!
//! Every building has a small chance a minute of an incident, set by what it is and how it
//! lives: bakeries, distilleries and thatch burn more readily than brick; houses riot when
//! needs they had go unmet; sickness comes with crowding and with ships, less where the door
//! is on paving. An incident grows, and past half way it spreads to its neighbours, until a
//! crew puts it down. Left alone a fire burns the building out (a charred ruin to rebuild or
//! clear), a riot burns itself out after a while with the street paying nothing, and a
//! sickness empties houses. When a quarter of an island's houses are sick its harbours fly
//! the yellow flag and ships neither load nor land there.
//!
//! Three public buildings answer them, each with a reach and crews that drive the streets:
//! the Fire Watch (a hand pump cart), the Watch House (constables) and the Infirmary (the
//! doctor's gig). In reach the chance of an incident is a quarter, and a crew sets out as
//! soon as one starts. Items in the Trading Union, Town Hall and Harbour Office answer some
//! at once (fire buckets, the night watch, the herb woman) or put them down when ordered
//! (curfew, quarantine notice, strike breaker).
//!
//! The economy reads only [`Hit`] on houses and works. The interface reads [`alerts`].

use engine::prelude::*;
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::art::{Mat, Parts, hexc, shade, v3};
use crate::build::Changed;
use crate::defs::{CELL_M, Good, Kind, Tier};
use crate::draw::{Art, Body, Drawn, Look, spawn_model};
use crate::globe::Frame;
use crate::grid::{self, Grid};
use crate::island::{self, islands};
use crate::items::Cue;
use crate::looks::{ModelKey, Piece};
use crate::sim::{Building, Economy, Hit, House, Producer};

/// What went wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Trouble {
    Fire,
    Riot,
    Sickness,
}

impl Trouble {
    pub const ALL: [Trouble; 3] = [Trouble::Fire, Trouble::Riot, Trouble::Sickness];

    pub fn index(self) -> usize {
        self as usize
    }

    /// The public building that answers it.
    pub fn answered_by(self) -> Kind {
        match self {
            Trouble::Fire => Kind::FireWatch,
            Trouble::Riot => Kind::WatchHouse,
            Trouble::Sickness => Kind::Infirmary,
        }
    }

    fn hit(self) -> Hit {
        match self {
            Trouble::Fire => Hit::Fire,
            Trouble::Riot => Hit::Riot,
            Trouble::Sickness => Hit::Sick,
        }
    }

    fn cue(self) -> Cue {
        match self {
            Trouble::Fire => Cue::Fire,
            Trouble::Riot => Cue::Riot,
            Trouble::Sickness => Cue::Sickness,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Trouble::Fire => "Fire",
            Trouble::Riot => "Riot",
            Trouble::Sickness => "Sickness",
        }
    }

    /// Seconds from a spark to its worst, with nobody fighting it.
    fn grow_s(self) -> f32 {
        match self {
            Trouble::Fire => 45.0,
            Trouble::Riot => 60.0,
            Trouble::Sickness => 90.0,
        }
    }

    /// Seconds at its worst before it ends on its own (a fire: before the building is lost).
    fn worst_s(self) -> f32 {
        match self {
            Trouble::Fire => 30.0,
            Trouble::Riot => 120.0,
            Trouble::Sickness => 150.0,
        }
    }

    /// Seconds a crew takes to put down the worst of it.
    fn quell_s(self) -> f32 {
        match self {
            Trouble::Fire => 14.0,
            Trouble::Riot => 18.0,
            Trouble::Sickness => 24.0,
        }
    }

    /// How far it reaches its neighbours, in cells.
    fn spread_cells(self) -> f64 {
        match self {
            Trouble::Fire => 3.5,
            Trouble::Riot => 4.5,
            Trouble::Sickness => 5.0,
        }
    }
}

/// Islands with fewer people than this have no incidents (a new settlement is let be).
pub const QUIET_BELOW: f32 = 150.0;
/// The chance an incident starts in reach of the building that answers it.
pub const COVERED: f32 = 0.25;
/// Most incidents at once on an island.
const MOST: usize = 24;
/// Most incidents of one kind on an island that still spread to neighbours.
const SPREAD_MOST: usize = 6;
/// Crews each answering building sends out at once.
pub const CREWS: u32 = 2;
/// Crew speed, m/s of game time (half again on paving).
const CREW_SPEED: f64 = 9.0;
/// Seconds between rolls for new incidents.
const ROLL_S: f32 = 1.0;
/// Share of an island's houses sick that closes its harbours.
pub const YELLOW_FLAG: f32 = 0.25;

/// An incident at a building.
#[derive(Component, Clone, Debug)]
pub struct Incident {
    pub what: Trouble,
    /// 0..1: how bad it is.
    pub level: f32,
    /// Seconds it has been at its worst.
    pub worst_s: f32,
    /// Game clock when it started.
    pub since_s: f64,
    /// A crew is on the way (or at work).
    pub crew: Option<Entity>,
    /// A crew is at work here.
    pub fought: bool,
    /// Seconds to its next try at spreading.
    pub spread_in: f32,
    /// The flames, flags, crowd and so on it shows.
    pub props: Vec<Entity>,
    /// For the interface.
    pub alert: u32,
}

/// A burnt-out building: its old look is kept for when it is rebuilt.
#[derive(Component, Clone, Debug)]
pub struct Ruin {
    pub was: Arc<Vec<Piece>>,
    #[allow(dead_code)]
    pub since_s: f64,
    pub smoke: Option<Entity>,
}

/// A crew on its way to an incident, at work there, or going home.
#[derive(Component, Clone, Debug)]
pub struct Crew {
    #[allow(dead_code)]
    pub what: Trouble,
    /// The building it came from and the one it is going to.
    pub home: Entity,
    pub to: Entity,
    pub path: Arc<Vec<DVec3>>,
    pub lengths: Arc<Vec<f64>>,
    pub s: f64,
    pub back: bool,
}

/// One line for the interface's alert strip ("go there").
#[derive(Clone, Debug, Serialize)]
pub struct Alert {
    pub id: u32,
    pub what: Trouble,
    /// The building's [`Building::id`].
    pub building: u32,
    pub kind: Kind,
    pub island: usize,
    pub island_name: &'static str,
    /// Map point to go to.
    pub at: [f64; 3],
    pub level: f32,
    pub since_s: f64,
    /// A crew is on the way or at work.
    pub crew: bool,
    pub text: String,
}

/// Totals for the playtest and the ledger.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Tally {
    pub started: [u32; 3],
    /// Of those, how many spread from a neighbour.
    pub spread: [u32; 3],
    pub put_out: [u32; 3],
    pub by_items: [u32; 3],
    pub burnt: u32,
    pub ended: [u32; 3],
    pub rebuilt: u32,
}

#[derive(Resource)]
pub struct Incidents {
    pub on: bool,
    /// New incidents start by chance (off in tests that start their own).
    pub rolls: bool,
    rng: u64,
    roll_in: f32,
    next_alert: u32,
    pub tally: Tally,
    /// What each answering building covers, refreshed when anything is built.
    cover: HashMap<Entity, [bool; 3]>,
    cover_dirty: bool,
    /// Islands whose harbours fly the yellow flag.
    pub flagged: HashSet<usize>,
    /// Ships landed per island, last seen (sickness comes with them).
    landed_seen: Vec<f64>,
    arrivals: Vec<f32>,
    /// Events for the playtest log: (clock, island, text).
    pub log: Vec<(f64, usize, String)>,
}

impl Default for Incidents {
    fn default() -> Self {
        Self {
            on: true,
            rolls: true,
            rng: 0x5eed_f1e5_0000_0001,
            roll_in: ROLL_S,
            next_alert: 1,
            tally: Tally::default(),
            cover: HashMap::new(),
            cover_dirty: true,
            flagged: HashSet::new(),
            landed_seen: Vec::new(),
            arrivals: Vec::new(),
            log: Vec::new(),
        }
    }
}

impl Incidents {
    /// Start the dice from `seed` (the playtest's `--seed`).
    pub fn reseed(&mut self, seed: u64) {
        self.rng = 0x5eed_f1e5_0000_0001 ^ seed.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        if self.rng == 0 {
            self.rng = 1;
        }
    }

    fn unit(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Whether an island's harbours fly the yellow flag.
pub fn flagged(world: &World, isle: usize) -> bool {
    world
        .get_resource::<Incidents>()
        .is_some_and(|i| i.flagged.contains(&isle))
}

fn centre(b: &Building) -> DVec2 {
    grid::footprint_center(b.kind, b.origin, b.rot)
}

/// The chance a minute of each trouble starting at a building, before cover.
fn risk(b: &Building, house: Option<&House>, crowd: u32, paved: bool, ships: f32) -> [f32; 3] {
    let mut fire = match b.kind {
        Kind::Bakery
        | Kind::Brewery
        | Kind::Distillery
        | Kind::Glassworks
        | Kind::OilWorks
        | Kind::LampMaker
        | Kind::Brickworks
        | Kind::Chandler
        | Kind::Confectioner => 0.004,
        Kind::Sawmill | Kind::Woodcutter | Kind::Shipyard | Kind::Tavern | Kind::Mill => 0.003,
        Kind::House => 0.0,
        k if k.def().output.is_some() => 0.0012,
        _ => 0.0008,
    };
    let (mut riot, mut sick) = (0.0, 0.0);
    if let Some(h) = house {
        fire = match h.tier {
            Tier::Fisherfolk => 0.0009,
            Tier::Labourers => 0.0006,
            Tier::Burghers => 0.0004,
        };
        // Unhappy houses riot: the further below 40, the likelier; fisherfolk grumble less.
        let unhappy = ((40.0 - h.happiness) / 20.0).max(0.0);
        riot = 0.0025
            * unhappy
            * match h.tier {
                Tier::Fisherfolk => 0.3,
                _ => 1.0,
            };
        sick = 0.0003 + 0.00012 * crowd.saturating_sub(4) as f32 + 0.0004 * ships.min(3.0);
        if paved {
            sick *= 0.6;
        }
    } else if b.kind.def().workers.is_some_and(|w| w.1 >= 20) {
        sick = 0.0002;
    }
    [fire, riot, sick]
}

/// Mark the cover stale (anything built or pulled down).
pub fn touch(world: &mut World) {
    if let Some(mut i) = world.get_resource_mut::<Incidents>() {
        i.cover_dirty = true;
    }
}

/// Road cells from `from`'s door within `budget` steps (paved cells half), with the cell
/// each was reached from.
fn reach_roads(
    from: &Building,
    roads: &HashSet<IVec2>,
    paved: &HashSet<IVec2>,
    budget: f32,
) -> HashMap<IVec2, (f32, IVec2)> {
    let mut best: HashMap<IVec2, (f32, IVec2)> = HashMap::new();
    let mut queue = VecDeque::new();
    for c in grid::ring(from.kind, from.origin, from.rot) {
        if roads.contains(&c) {
            best.insert(c, (0.0, c));
            queue.push_back(c);
        }
    }
    while let Some(c) = queue.pop_front() {
        let d = best[&c].0;
        for n in [IVec2::X, IVec2::NEG_X, IVec2::Y, IVec2::NEG_Y] {
            let nc = c + n;
            if !roads.contains(&nc) {
                continue;
            }
            let nd = d + if paved.contains(&nc) { 0.5 } else { 1.0 };
            if nd <= budget && best.get(&nc).is_none_or(|old| nd < old.0) {
                best.insert(nc, (nd, c));
                queue.push_back(nc);
            }
        }
    }
    best
}

/// What the answering building `e` covers: as the crow flies within its reach, or along
/// the streets a third further.
fn covers(station: &Building, b: &Building, by_road: &HashMap<IVec2, (f32, IVec2)>) -> bool {
    if station.island != b.island {
        return false;
    }
    let r = station.kind.def().reach as f64 * CELL_M;
    (centre(station) - centre(b)).length() <= r
        || grid::ring(b.kind, b.origin, b.rot)
            .iter()
            .any(|c| by_road.contains_key(c))
}

/// A road path from `station`'s door to `b`'s, as map points, if the streets join them
/// within twice the station's reach.
fn road_path(world: &World, station: &Building, b: &Building) -> Option<Vec<DVec3>> {
    let g = world.resource::<Grid>();
    let budget = station.kind.def().reach as f32 * 2.5;
    let best = reach_roads(station, &g.roads, &g.paved, budget);
    let end = grid::ring(b.kind, b.origin, b.rot)
        .into_iter()
        .filter_map(|c| best.get(&c).map(|d| (d.0, c)))
        .min_by(|a, b| a.0.total_cmp(&b.0))?
        .1;
    let pt = |c: IVec2| {
        let p = grid::cell_center(c);
        DVec3::new(p.x, grid::cell_height(c) + 0.05, p.y)
    };
    let mut cells = vec![end];
    let mut c = end;
    for _ in 0..4000 {
        let prev = best[&c].1;
        if prev == c {
            break;
        }
        cells.push(prev);
        c = prev;
    }
    cells.reverse();
    let s = centre(station);
    let mut out = vec![DVec3::new(s.x, station.base_y, s.y)];
    out.extend(cells.into_iter().map(pt));
    let t = centre(b);
    out.push(DVec3::new(t.x, b.base_y, t.y));
    Some(out)
}

fn lengths(path: &[DVec3]) -> Vec<f64> {
    let mut out = vec![0.0];
    let mut acc = 0.0;
    for w in path.windows(2) {
        acc += (w[1] - w[0]).length();
        out.push(acc);
    }
    out
}

/// Every frame: roll for new incidents, grow and spread them, send crews, end them.
pub fn tick(world: &mut World) {
    let dt = world.resource::<Time>().delta_secs() * world.resource::<Economy>().speed;
    step(world, dt);
}

/// One step of `dt` game seconds (the playtest and tests call this directly).
pub fn step(world: &mut World, dt: f32) {
    if dt <= 0.0 || !world.get_resource::<Incidents>().is_some_and(|i| i.on) {
        return;
    }
    if world.resource::<Changed>().any {
        touch(world);
    }
    refresh_cover(world);
    count_arrivals(world, dt);
    orders(world);
    drive_crews(world, dt);
    grow(world, dt);
    let roll = {
        let mut inc = world.resource_mut::<Incidents>();
        inc.roll_in -= dt;
        if inc.roll_in <= 0.0 {
            inc.roll_in += ROLL_S;
            true
        } else {
            false
        }
    };
    if roll {
        if world.resource::<Incidents>().rolls {
            roll_new(world, ROLL_S);
        }
        send_crews(world);
        flags(world);
    }
}

fn refresh_cover(world: &mut World) {
    if !world.resource::<Incidents>().cover_dirty {
        return;
    }
    let (roads, paved) = {
        let g = world.resource::<Grid>();
        (g.roads.clone(), g.paved.clone())
    };
    let mut q = world.query_filtered::<(Entity, &Building), Without<Ruin>>();
    let all: Vec<(Entity, Building)> = q.iter(world).map(|(e, b)| (e, b.clone())).collect();
    let mut cover: HashMap<Entity, [bool; 3]> = HashMap::new();
    for (_, st) in all
        .iter()
        .filter(|(_, b)| matches!(b.kind, Kind::FireWatch | Kind::WatchHouse | Kind::Infirmary))
    {
        let t = Trouble::ALL
            .into_iter()
            .find(|t| t.answered_by() == st.kind)
            .unwrap_or(Trouble::Fire);
        let budget = st.kind.def().reach as f32 * 1.35;
        let by_road = reach_roads(st, &roads, &paved, budget);
        for (e, b) in &all {
            if covers(st, b, &by_road) {
                cover.entry(*e).or_default()[t.index()] = true;
            }
        }
    }
    let mut inc = world.resource_mut::<Incidents>();
    inc.cover = cover;
    inc.cover_dirty = false;
}

/// Ships landing goods bring sickness to the harbour's streets.
fn count_arrivals(world: &mut World, dt: f32) {
    let landed = world.resource::<crate::items::Items>().landed.clone();
    let mut inc = world.resource_mut::<Incidents>();
    if inc.landed_seen.len() < landed.len() {
        inc.landed_seen.resize(landed.len(), -1.0);
        inc.arrivals.resize(landed.len(), 0.0);
    }
    for (i, l) in landed.iter().enumerate() {
        let seen = inc.landed_seen[i];
        let new = if seen < 0.0 { 0.0 } else { (l - seen).max(0.0) };
        inc.landed_seen[i] = *l;
        // Loads landed in the last few minutes, about one per 30 goods.
        let a = &mut inc.arrivals[i];
        *a = (*a + new as f32 / 30.0) * (1.0 - dt / 240.0).max(0.0);
    }
}

fn roll_new(world: &mut World, dt: f32) {
    let pops: Vec<f32> = world
        .resource::<Economy>()
        .islands
        .iter()
        .map(crate::sim::population)
        .collect();
    let paved = world.resource::<Grid>().paved.clone();
    let mut q = world.query_filtered::<(
        Entity,
        &Building,
        Option<&House>,
        Option<&Producer>,
    ), (Without<Incident>, Without<Ruin>)>();
    let all: Vec<(Entity, Building, Option<House>, bool)> = q
        .iter(world)
        .map(|(e, b, h, p)| (e, b.clone(), h.cloned(), p.is_some()))
        .collect();
    // Houses near each house, for crowding.
    let mut houses_at: HashMap<IVec2, u32> = HashMap::new();
    for (_, b, h, _) in &all {
        if h.is_some() {
            *houses_at
                .entry(b.origin.div_euclid(IVec2::splat(4)))
                .or_default() += 1;
        }
    }
    let harbours: Vec<(usize, DVec2)> = all
        .iter()
        .filter(|(_, b, ..)| b.kind == Kind::Harbour)
        .map(|(_, b, ..)| (b.island, centre(b)))
        .collect();
    let mut active = vec![0usize; pops.len()];
    for b in world
        .query_filtered::<&Building, With<Incident>>()
        .iter(world)
    {
        active[b.island] += 1;
    }
    let mut starts: Vec<(Entity, Building, Trouble)> = Vec::new();
    {
        let mut inc = world.resource_mut::<Incidents>();
        for (e, b, h, _) in &all {
            if pops[b.island] < QUIET_BELOW
                || active[b.island] >= MOST
                || matches!(b.kind, Kind::Road | Kind::PavedRoad)
            {
                continue;
            }
            let crowd = houses_at
                .get(&b.origin.div_euclid(IVec2::splat(4)))
                .copied()
                .unwrap_or(0);
            let door_paved = grid::ring(b.kind, b.origin, b.rot)
                .iter()
                .any(|c| paved.contains(c));
            let near_quay = harbours
                .iter()
                .any(|(i, p)| *i == b.island && (*p - centre(b)).length() < 60.0);
            let ships = if near_quay {
                inc.arrivals.get(b.island).copied().unwrap_or(0.0)
            } else {
                0.0
            };
            let r = risk(b, h.as_ref(), crowd, door_paved, ships);
            let cov = inc.cover.get(e).copied().unwrap_or_default();
            for t in Trouble::ALL {
                let mut p = r[t.index()] * dt / 60.0;
                if cov[t.index()] {
                    p *= COVERED;
                }
                if p > 0.0 && inc.unit() < p {
                    starts.push((*e, b.clone(), t));
                    active[b.island] += 1;
                    break;
                }
            }
        }
    }
    for (e, b, t) in starts {
        start(world, e, &b, t, false);
    }
}

/// Start `what` at building `e` (from a roll, or spread from a neighbour). An item in reach
/// may answer it at once.
pub fn start(world: &mut World, e: Entity, b: &Building, what: Trouble, spread: bool) -> bool {
    if world.get::<Incident>(e).is_some() || world.get::<Ruin>(e).is_some() {
        return false;
    }
    let clock = world.resource::<Economy>().clock_s;
    let isle_name = islands()[b.island].name;
    let kind_name = b.kind.def().name;
    if let Some(item) = crate::items::answer(world, what.cue(), b) {
        let mut inc = world.resource_mut::<Incidents>();
        inc.tally.by_items[what.index()] += 1;
        inc.log.push((
            clock,
            b.island,
            format!(
                "{} at a {kind_name} on {isle_name}: {item} dealt with it",
                what.word()
            ),
        ));
        if world.get_resource::<Art>().is_some() {
            let icon = match what {
                Trouble::Fire => "isle_alert_fire",
                Trouble::Riot => "isle_alert_riot",
                Trouble::Sickness => "isle_alert_sick",
            };
            let text = format!(
                "{item} dealt with a {} at a {} on {isle_name}.",
                low(what.word()),
                low(kind_name)
            );
            engine_ui::toast::notify(
                world,
                engine_ui::toast::ToastKind::Good,
                icon,
                what.word(),
                &text,
                5.0,
            );
        }
        return false;
    }
    let alert = {
        let mut inc = world.resource_mut::<Incidents>();
        inc.tally.started[what.index()] += 1;
        if spread {
            inc.tally.spread[what.index()] += 1;
        }
        let id = inc.next_alert;
        inc.next_alert += 1;
        inc.log.push((
            clock,
            b.island,
            format!(
                "{} at a {kind_name} on {isle_name}{}",
                what.word(),
                if spread { " (spread)" } else { "" }
            ),
        ));
        id
    };
    let props = show(world, e, b, what);
    world.entity_mut(e).insert(Incident {
        what,
        level: 0.1,
        worst_s: 0.0,
        since_s: clock,
        crew: None,
        fought: false,
        spread_in: 8.0,
        props,
        alert,
    });
    set_hit(world, e, what.hit());
    if !spread {
        let text = match what {
            Trouble::Fire => format!("A {} is burning on {isle_name}.", low(kind_name)),
            Trouble::Riot => format!("A riot has broken out on {isle_name}."),
            Trouble::Sickness => format!("Sickness on {isle_name}."),
        };
        let kind = match what {
            Trouble::Fire => engine_ui::toast::ToastKind::Bad,
            _ => engine_ui::toast::ToastKind::Warn,
        };
        if world.get_resource::<Art>().is_some() {
            let icon = match what {
                Trouble::Fire => "isle_alert_fire",
                Trouble::Riot => "isle_alert_riot",
                Trouble::Sickness => "isle_alert_sick",
            };
            engine_ui::toast::notify(world, kind, icon, what.word(), &text, 6.0);
        }
    }
    true
}

fn low(s: &str) -> String {
    s.to_lowercase()
}

fn set_hit(world: &mut World, e: Entity, hit: Hit) {
    if let Some(mut h) = world.get_mut::<House>(e) {
        h.hit = hit;
    }
    if let Some(mut p) = world.get_mut::<Producer>(e) {
        p.hit = hit;
    }
}

/// Grow, fight, spread and end incidents.
fn grow(world: &mut World, dt: f32) {
    let mut q = world.query::<(Entity, &Building, &mut Incident)>();
    let mut ended: Vec<(Entity, bool)> = Vec::new();
    let mut spread: Vec<(Building, Trouble)> = Vec::new();
    let quarantine: Vec<(usize, DVec2, f64)> = crate::items::acting_orders(world)
        .into_iter()
        .filter(|o| o.0 == "Quarantine notice")
        .map(|o| (o.1, o.2, o.3))
        .collect();
    for (e, b, mut inc) in q.iter_mut(world) {
        let t = inc.what;
        if inc.fought {
            inc.level -= dt / t.quell_s();
            if inc.level <= 0.0 {
                ended.push((e, true));
            }
            continue;
        }
        if inc.level < 1.0 {
            inc.level = (inc.level + dt / t.grow_s()).min(1.0);
        } else {
            inc.worst_s += dt;
            if inc.worst_s >= t.worst_s() {
                ended.push((e, false));
                continue;
            }
        }
        inc.spread_in -= dt;
        if inc.level >= 0.6 && inc.spread_in <= 0.0 {
            inc.spread_in = 15.0;
            let held = t == Trouble::Sickness
                && quarantine
                    .iter()
                    .any(|(i, p, r)| *i == b.island && (*p - centre(b)).length() <= *r);
            if !held {
                spread.push((b.clone(), t));
            }
        }
    }
    // Spread to a neighbour or two.
    for (b, t) in spread {
        let c = centre(&b);
        let reach = t.spread_cells() * CELL_M
            + b.kind.def().size.0.max(b.kind.def().size.1) as f64 * CELL_M * 0.5;
        let mut q = world.query_filtered::<(Entity, &Building, Option<&House>), (Without<Incident>, Without<Ruin>)>();
        let near: Vec<(Entity, Building)> = q
            .iter(world)
            .filter(|(_, n, h)| {
                n.island == b.island
                    && !matches!(n.kind, Kind::Road | Kind::PavedRoad)
                    && (t == Trouble::Fire || h.is_some())
                    && (centre(n) - c).length()
                        <= reach
                            + n.kind.def().size.0.max(n.kind.def().size.1) as f64 * CELL_M * 0.5
            })
            .map(|(e, n, _)| (e, n.clone()))
            .collect();
        // One neighbour at a time: left alone a fire takes three or four houses with it,
        // a riot or a sickness a street.
        let chance = match t {
            Trouble::Fire => 0.10,
            Trouble::Riot => 0.04,
            Trouble::Sickness => 0.05,
        };
        // A street at most: past SPREAD_MOST of one kind on an island it stops spreading.
        let same = world
            .query::<(&Building, &Incident)>()
            .iter(world)
            .filter(|(n, i)| n.island == b.island && i.what == t)
            .count();
        if same >= SPREAD_MOST {
            continue;
        }
        let (roll, pick) = {
            let mut inc = world.resource_mut::<Incidents>();
            (inc.unit(), inc.unit())
        };
        if roll < chance && !near.is_empty() {
            let (e, n) = &near[(pick * near.len() as f32) as usize % near.len()];
            start(world, *e, n, t, true);
        }
    }
    for (e, fought) in ended {
        end(world, e, fought);
    }
    // Works next to a riot down tools.
    strikes(world);
}

/// A riot stops the works next to it.
fn strikes(world: &mut World) {
    let riots: Vec<(usize, DVec2)> = world
        .query::<(&Building, &Incident)>()
        .iter(world)
        .filter(|(_, i)| i.what == Trouble::Riot)
        .map(|(b, _)| (b.island, centre(b)))
        .collect();
    let mut q =
        world.query_filtered::<(&Building, &mut Producer), (Without<Incident>, Without<Ruin>)>();
    for (b, mut p) in q.iter_mut(world) {
        let near = riots
            .iter()
            .any(|(i, c)| *i == b.island && (*c - centre(b)).length() < 6.0 * CELL_M);
        let want = if near { Hit::Riot } else { Hit::Clear };
        if p.hit != want {
            p.hit = want;
        }
    }
}

/// End the incident at `e`: put down by a crew (`fought`), or run its course.
fn end(world: &mut World, e: Entity, fought: bool) {
    let Some(inc) = world.get::<Incident>(e).cloned() else {
        return;
    };
    let Some(b) = world.get::<Building>(e).cloned() else {
        return;
    };
    for p in &inc.props {
        despawn_tree(world, *p);
    }
    world.entity_mut(e).remove::<Incident>();
    let clock = world.resource::<Economy>().clock_s;
    let name = islands()[b.island].name;
    let kind = b.kind.def().name;
    {
        let mut t = world.resource_mut::<Incidents>();
        if fought {
            t.tally.put_out[inc.what.index()] += 1;
        } else if inc.what != Trouble::Fire {
            t.tally.ended[inc.what.index()] += 1;
        }
    }
    if !fought && inc.what == Trouble::Fire {
        burn_out(world, e, &b);
        world.resource_mut::<Incidents>().log.push((
            clock,
            b.island,
            format!("A {} on {name} burnt down", low(kind)),
        ));
        if world.get_resource::<Art>().is_some() {
            engine_ui::toast::notify(
                world,
                engine_ui::toast::ToastKind::Bad,
                "isle_ruin",
                "Burnt down",
                &format!(
                    "A {} on {name} is a ruin. Rebuild it or clear it.",
                    low(kind)
                ),
                6.0,
            );
        }
        return;
    }
    if !fought && inc.what == Trouble::Sickness {
        // The sickness took some with it.
        if let Some(mut h) = world.get_mut::<House>(e) {
            h.residents *= 0.6;
        }
    }
    set_hit(world, e, Hit::Clear);
}

fn despawn_tree(world: &mut World, e: Entity) {
    if let Some(body) = world.get::<Body>(e).cloned() {
        for p in body.parts {
            if world.get_entity(p).is_ok() {
                world.despawn(p);
            }
        }
    }
    if world.get_entity(e).is_ok() {
        world.despawn(e);
    }
}

/// The building at `e` burns out: it stands as a charred shell.
fn burn_out(world: &mut World, e: Entity, b: &Building) {
    let clock = world.resource::<Economy>().clock_s;
    world.resource_mut::<Incidents>().tally.burnt += 1;
    let was = world
        .get::<Look>(e)
        .map(|l| l.0.clone())
        .unwrap_or_else(|| Arc::new(Vec::new()));
    // The roof has fallen in: what stands is a blackened shell of broken walls with the
    // rubble inside, an 8 m square scaled to the lot (two or three along a long one).
    let (w, d) = b.kind.def().size;
    let (long, short, along_x) = if w >= d { (w, d, true) } else { (d, w, false) };
    let n = ((long as f32 / short as f32).round() as u32).clamp(1, 3);
    let side = short as f32 * CELL_M as f32;
    let step = long as f32 * CELL_M as f32 / n as f32;
    let charred: Vec<Piece> = (0..n)
        .map(|k| {
            let off = (k as f32 + 0.5) * step - long as f32 * CELL_M as f32 * 0.5;
            let at = if along_x {
                v3(off, 0.0, 0.0)
            } else {
                v3(0.0, 0.0, off)
            };
            let mut p = Piece::at("ruin_shell", at);
            p.scale = (side.min(step) * 0.85 / 8.0).clamp(0.4, 2.0);
            p.yaw = if along_x {
                0.0
            } else {
                std::f32::consts::FRAC_PI_2
            };
            p
        })
        .chain(ruin_pieces(b))
        .collect();
    let smoke = spawn_effect(world, b, "isle.embers", 0.0);
    world.entity_mut(e).insert((
        Look(Arc::new(charred)),
        Ruin {
            was,
            since_s: clock,
            smoke,
        },
    ));
    set_hit(world, e, Hit::Ruin);
    if let Some(mut h) = world.get_mut::<House>(e) {
        h.residents = 0.0;
    }
    if let Some(mut d) = world.get_resource_mut::<Drawn>() {
        d.dirty = true;
    }
    world.resource_mut::<Changed>().any = true;
}

/// Rubble and fallen beams on a ruin's lot.
fn ruin_pieces(b: &Building) -> Vec<Piece> {
    let (w, d) = b.kind.def().size;
    let (hw, hd) = (
        w as f32 * CELL_M as f32 * 0.5,
        d as f32 * CELL_M as f32 * 0.5,
    );
    let mut out = Vec::new();
    let n = ((w * d) as f32 / 6.0).ceil().clamp(1.0, 8.0) as u64;
    for k in 0..n {
        let r = |j: u64| crate::looks::h01(b.seed, 1700 + k * 8 + j);
        let at = v3((r(0) - 0.5) * hw * 1.4, 0.0, (r(1) - 0.5) * hd * 1.4);
        let mut p = Piece::at("rubble", at);
        p.yaw = r(2) * std::f32::consts::TAU;
        p.scale = 0.7 + 0.6 * r(3);
        out.push(p);
    }
    out
}

/// What the cost of rebuilding a ruin is: half the building's.
pub fn rebuild_cost(kind: Kind) -> (u32, Vec<(Good, u32)>) {
    let d = kind.def();
    (
        d.coins / 2,
        d.cost.iter().map(|(g, n)| (*g, n.div_ceil(2))).collect(),
    )
}

/// Rebuild the ruin with building id `id` for half its cost.
pub fn rebuild(world: &mut World, id: u32) -> Result<(), String> {
    let e = crate::build::by_id(world, id).ok_or("No such building.")?;
    let ruin = world.get::<Ruin>(e).cloned().ok_or("That is not a ruin.")?;
    let b = world
        .get::<Building>(e)
        .cloned()
        .ok_or("No such building.")?;
    let (coins, goods) = rebuild_cost(b.kind);
    let owner = world.resource::<Economy>().islands[b.island].owner;
    if owner != crate::sim::acting(world) {
        return Err(format!(
            "That ruin is {}'s to rebuild.",
            crate::rivals::company_name(world, owner)
        ));
    }
    {
        let econ = world.resource::<Economy>();
        if econ.coins_of(owner) < coins as f64 {
            return Err(format!("Rebuilding costs {coins} coins."));
        }
        let books = &econ.islands[b.island];
        if let Some((g, n)) = goods.iter().find(|(g, n)| books.get(*g) < *n as f64) {
            return Err(format!("Rebuilding needs {n} {}.", low(g.name())));
        }
    }
    {
        let mut econ = world.resource_mut::<Economy>();
        *econ.coins_mut(owner) -= coins as f64;
        for (g, n) in &goods {
            econ.islands[b.island].take(*g, *n as f64);
        }
    }
    if let Some(s) = ruin.smoke {
        despawn_tree(world, s);
    }
    world.entity_mut(e).remove::<Ruin>().insert(Look(ruin.was));
    set_hit(world, e, Hit::Clear);
    if let Some(mut h) = world.get_mut::<House>(e) {
        h.residents = 2.0;
    }
    world.resource_mut::<Incidents>().tally.rebuilt += 1;
    if let Some(mut d) = world.get_resource_mut::<Drawn>() {
        d.dirty = true;
    }
    world.resource_mut::<Changed>().any = true;
    Ok(())
}

/// Orders that put incidents down: Curfew and Strike breaker end riots in reach.
fn orders(world: &mut World) {
    let acting = crate::items::acting_orders(world);
    let calm: Vec<(usize, DVec2, f64)> = acting
        .iter()
        .filter(|o| matches!(o.0, "Curfew" | "Strike breaker"))
        .map(|o| (o.1, o.2, o.3))
        .collect();
    if calm.is_empty() {
        return;
    }
    let riots: Vec<Entity> = world
        .query::<(Entity, &Building, &Incident)>()
        .iter(world)
        .filter(|(_, b, i)| {
            i.what == Trouble::Riot
                && calm
                    .iter()
                    .any(|(isle, p, r)| *isle == b.island && (*p - centre(b)).length() <= *r)
        })
        .map(|(e, ..)| e)
        .collect();
    for e in riots {
        world.resource_mut::<Incidents>().tally.by_items[Trouble::Riot.index()] += 1;
        end(world, e, true);
    }
}

/// Send free crews from the answering buildings to incidents in their reach.
fn send_crews(world: &mut World) {
    let mut q = world.query::<(Entity, &Building, &Incident)>();
    let waiting: Vec<(Entity, Building, Trouble)> = q
        .iter(world)
        .filter(|(_, _, i)| i.crew.is_none())
        .map(|(e, b, i)| (e, b.clone(), i.what))
        .collect();
    if waiting.is_empty() {
        return;
    }
    let mut busy: HashMap<Entity, u32> = HashMap::new();
    for c in world.query::<&Crew>().iter(world) {
        *busy.entry(c.home).or_default() += 1;
    }
    let mut q = world.query_filtered::<(Entity, &Building), Without<Ruin>>();
    let stations: Vec<(Entity, Building)> = q
        .iter(world)
        .filter(|(_, b)| matches!(b.kind, Kind::FireWatch | Kind::WatchHouse | Kind::Infirmary))
        .map(|(e, b)| (e, b.clone()))
        .collect();
    for (e, b, t) in waiting {
        let mut best: Option<(f64, Entity, Building)> = None;
        for (se, s) in &stations {
            if s.kind != t.answered_by()
                || s.island != b.island
                || busy.get(se).copied().unwrap_or(0) >= CREWS
            {
                continue;
            }
            let d = (centre(s) - centre(&b)).length();
            if d <= s.kind.def().reach as f64 * CELL_M * 2.0
                && best.as_ref().is_none_or(|x| d < x.0)
            {
                best = Some((d, *se, s.clone()));
            }
        }
        let Some((_, se, s)) = best else { continue };
        let Some(path) = road_path(world, &s, &b) else {
            continue;
        };
        *busy.entry(se).or_default() += 1;
        let lengths = Arc::new(lengths(&path));
        let crew = Crew {
            what: t,
            home: se,
            to: e,
            path: Arc::new(path),
            lengths,
            s: 0.0,
            back: false,
        };
        let start = crew.path[0];
        let stand = world
            .get_resource::<Frame>()
            .map_or(Transform::default(), |f| f.stand(start, 0.0));
        let parts = match world.get_resource::<Art>().cloned() {
            Some(art) => art
                .models
                .get(crew_model(t))
                .map(|m| spawn_model(world, &art, m, stand, true))
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let c = world
            .spawn((
                Name::new(format!("{} crew", t.word())),
                stand,
                crew,
                Body { parts },
            ))
            .id();
        if let Some(mut i) = world.get_mut::<Incident>(e) {
            i.crew = Some(c);
        }
    }
}

fn crew_model(t: Trouble) -> ModelKey {
    match t {
        Trouble::Fire => "pump_cart",
        Trouble::Riot => "constables",
        Trouble::Sickness => "doctor_gig",
    }
}

/// Drive crews along the streets; set to work on arrival; home again after.
fn drive_crews(world: &mut World, dt: f32) {
    let frame = world.get_resource::<Frame>().copied();
    let paved = std::mem::take(&mut world.resource_mut::<Grid>().paved);
    let mut arrived: Vec<(Entity, Entity, bool)> = Vec::new();
    let mut q = world.query::<(Entity, &mut Crew, &mut Transform)>();
    for (e, mut c, mut t) in q.iter_mut(world) {
        let total = *c.lengths.last().unwrap_or(&0.0);
        let s = if c.back { total - c.s } else { c.s };
        let (here, _) = crate::logistics::along(&c.path, &c.lengths, s);
        let fast = paved.contains(&grid::cell_of(here.x, here.z));
        c.s += CREW_SPEED * if fast { 1.5 } else { 1.0 } * dt as f64;
        if c.s >= total {
            arrived.push((e, c.to, c.back));
            continue;
        }
        let s = if c.back { total - c.s } else { c.s };
        let (p, mut d) = crate::logistics::along(&c.path, &c.lengths, s);
        if c.back {
            d = -d;
        }
        let ground = island::height(p.x, p.z).max(p.y - 0.5);
        let yaw = if d.length_squared() > 0.0 {
            (-d.z).atan2(d.x) as f32
        } else {
            0.0
        };
        if let Some(f) = frame {
            *t = f.stand(DVec3::new(p.x, ground + 0.02, p.z), yaw);
        }
    }
    world.resource_mut::<Grid>().paved = paved;
    for (e, to, back) in arrived {
        if back {
            despawn_tree(world, e);
            continue;
        }
        match world.get_mut::<Incident>(to) {
            Some(mut i) if i.crew == Some(e) => {
                i.fought = true;
                // The crew stands by until it is out, then drives home.
                if let Some(mut c) = world.get_mut::<Crew>(e) {
                    c.s = c.lengths.last().copied().unwrap_or(0.0);
                }
                park(world, e);
            }
            _ => turn_home(world, e),
        }
    }
    // Crews whose incident is over go home.
    let mut q = world.query::<(Entity, &Crew)>();
    let parked: Vec<(Entity, Entity)> = q
        .iter(world)
        .filter(|(_, c)| !c.back && c.s >= c.lengths.last().copied().unwrap_or(0.0))
        .map(|(e, c)| (e, c.to))
        .collect();
    for (e, to) in parked {
        let done = world.get::<Incident>(to).is_none_or(|i| i.crew != Some(e));
        if done {
            turn_home(world, e);
        }
    }
}

/// A crew at work: it stops at the end of its path.
fn park(world: &mut World, e: Entity) {
    if let Some(mut c) = world.get_mut::<Crew>(e) {
        let total = c.lengths.last().copied().unwrap_or(0.0);
        c.s = total + 1e-3;
    }
}

fn turn_home(world: &mut World, e: Entity) {
    if let Some(mut c) = world.get_mut::<Crew>(e) {
        c.back = true;
        c.s = 0.0;
    }
}

/// Yellow flags: islands with a quarter of their houses sick.
fn flags(world: &mut World) {
    let n = world.resource::<Economy>().islands.len();
    let mut houses = vec![0u32; n];
    let mut sick = vec![0u32; n];
    for (b, h) in world.query::<(&Building, &House)>().iter(world) {
        houses[b.island] += 1;
        if h.hit == Hit::Sick {
            sick[b.island] += 1;
        }
    }
    let flagged: HashSet<usize> = (0..n)
        .filter(|i| houses[*i] >= 8 && sick[*i] as f32 >= houses[*i] as f32 * YELLOW_FLAG)
        .collect();
    let clock = world.resource::<Economy>().clock_s;
    let old = world.resource::<Incidents>().flagged.clone();
    for i in flagged.difference(&old) {
        let name = islands()[*i].name;
        world.resource_mut::<Incidents>().log.push((
            clock,
            *i,
            format!("{name} flies the yellow flag"),
        ));
        if world.get_resource::<Art>().is_some() {
            engine_ui::toast::notify(
                world,
                engine_ui::toast::ToastKind::Warn,
                "isle_alert_sick",
                "Yellow flag",
                &format!("So many are sick on {name} that ships will not call there."),
                6.0,
            );
        }
    }
    world.resource_mut::<Incidents>().flagged = flagged;
}

/// Flames, smoke, a crowd with torches or a yellow flag at building `b`.
fn show(world: &mut World, e: Entity, b: &Building, what: Trouble) -> Vec<Entity> {
    let mut out = Vec::new();
    match what {
        Trouble::Fire => {
            // The blaze's flames start 3.5 m above where it stands: put them at the roof,
            // so they lick out of it rather than burn hidden inside the walls.
            let roof = roof_m(world, e, b);
            out.extend(spawn_effect(world, b, "isle.blaze", roof - 3.5));
        }
        Trouble::Riot => {
            out.extend(spawn_effect(world, b, "isle.torches", 0.0));
            out.extend(spawn_props(world, b, "rioters", 1.0));
        }
        Trouble::Sickness => {
            out.extend(spawn_props(world, b, "sick_flag", 0.0));
            // And one flying from the roof, seen from every side.
            let roof = roof_m(world, e, b) - 1.5;
            out.extend(spawn_on_roof(world, b, "sick_flag", roof, 1.4));
        }
    }
    out
}

/// About how high building `e`'s roof stands over its floor, meters.
fn roof_m(world: &World, e: Entity, b: &Building) -> f64 {
    match world.get::<House>(e).map(|h| h.tier) {
        Some(Tier::Fisherfolk) => 6.0,
        Some(Tier::Labourers) => 9.0,
        Some(Tier::Burghers) => 11.5,
        None => {
            let (w, d) = b.kind.def().size;
            (6.0 + (w.max(d) as f64) * 0.8).min(11.0)
        }
    }
}

/// A model over the middle of building `b`, `up` meters above its floor, scaled `scale`.
fn spawn_on_roof(
    world: &mut World,
    b: &Building,
    key: ModelKey,
    up: f64,
    scale: f32,
) -> Option<Entity> {
    let art = world.get_resource::<Art>()?.clone();
    let model = art.models.get(key)?.clone();
    let c = centre(b);
    let mut stand = world
        .get_resource::<Frame>()?
        .stand(DVec3::new(c.x, b.base_y + up, c.y), grid::yaw(b.rot));
    stand.scale = Vec3::splat(scale);
    let parts = spawn_model(world, &art, &model, stand, true);
    Some(world.spawn((Name::new(key), stand, Body { parts })).id())
}

/// A particle effect over the middle of building `b`, `up` meters above its floor.
fn spawn_effect(world: &mut World, b: &Building, name: &str, up: f64) -> Option<Entity> {
    world.get_resource::<Art>()?;
    let c = centre(b);
    let at = DVec3::new(c.x, b.base_y + up, c.y);
    let stand = world.get_resource::<Frame>()?.stand(at, 0.0);
    let fx = engine_vfx::ParticleEffect::new(name);
    Some(world.spawn((Name::new(name.to_string()), stand, fx)).id())
}

/// A model in front of building `b`'s door (`out` cells out from its front row).
fn spawn_props(world: &mut World, b: &Building, key: ModelKey, out: f64) -> Option<Entity> {
    let art = world.get_resource::<Art>()?.clone();
    let model = art.models.get(key)?.clone();
    let c = centre(b);
    let (_, d) = b.kind.footprint(0);
    let front = grid::front_dir(b.rot).as_dvec2();
    let p = c + front * (d as f64 * 0.5 + out) * CELL_M * 0.5;
    let ground = island::height(p.x, p.y).max(b.base_y - 0.3);
    let stand = world
        .get_resource::<Frame>()?
        .stand(DVec3::new(p.x, ground, p.y), grid::yaw(b.rot));
    let parts = spawn_model(world, &art, &model, stand, true);
    Some(world.spawn((Name::new(key), stand, Body { parts })).id())
}

/// Burn building `e` out at once (the tour's ruin picture).
pub fn burn_now(world: &mut World, e: Entity) {
    let Some(b) = world.get::<Building>(e).cloned() else {
        return;
    };
    if world.get::<Ruin>(e).is_some() {
        return;
    }
    if let Some(inc) = world.get::<Incident>(e).cloned() {
        for p in inc.props {
            despawn_tree(world, p);
        }
        world.entity_mut(e).remove::<Incident>();
    }
    burn_out(world, e, &b);
}

/// A building is pulled down: its incident, crew and ruin go with it.
pub fn forget(world: &mut World, e: Entity) {
    if let Some(inc) = world.get::<Incident>(e).cloned() {
        for p in inc.props {
            despawn_tree(world, p);
        }
        if let Some(c) = inc.crew {
            turn_home(world, c);
        }
    }
    if let Some(s) = world.get::<Ruin>(e).and_then(|r| r.smoke) {
        despawn_tree(world, s);
    }
    touch(world);
}

/// The incidents now, for the interface's alert strip.
pub fn alerts(world: &mut World) -> Vec<Alert> {
    let mut q = world.query::<(&Building, &Incident)>();
    let mut out: Vec<Alert> = q
        .iter(world)
        .map(|(b, i)| {
            let c = centre(b);
            let isle = islands()[b.island].name;
            let kind = b.kind.def().name;
            Alert {
                id: i.alert,
                what: i.what,
                building: b.id,
                kind: b.kind,
                island: b.island,
                island_name: isle,
                at: [c.x, b.base_y, c.y],
                level: i.level,
                since_s: i.since_s,
                crew: i.crew.is_some(),
                text: match (i.what, i.fought, i.crew.is_some()) {
                    (Trouble::Fire, true, _) => {
                        format!("Fire at a {} on {isle}: the pump is at work", low(kind))
                    }
                    (Trouble::Fire, _, true) => {
                        format!("Fire at a {} on {isle}: the pump is on its way", low(kind))
                    }
                    (Trouble::Fire, ..) => {
                        format!("Fire at a {} on {isle}: no Fire Watch in reach", low(kind))
                    }
                    (Trouble::Riot, true, _) => format!("Riot on {isle}: the constables are there"),
                    (Trouble::Riot, _, true) => {
                        format!("Riot on {isle}: the constables are on their way")
                    }
                    (Trouble::Riot, ..) => format!("Riot on {isle}: no Watch House in reach"),
                    (Trouble::Sickness, true, _) => {
                        format!("Sickness on {isle}: the doctor is with them")
                    }
                    (Trouble::Sickness, _, true) => {
                        format!("Sickness on {isle}: the doctor is on her way")
                    }
                    (Trouble::Sickness, ..) => format!("Sickness on {isle}: no Infirmary in reach"),
                },
            }
        })
        .collect();
    out.sort_by_key(|a| a.id);
    out
}

/// The ruins standing now: (building id, kind, island, map point).
pub fn ruins(world: &mut World) -> Vec<(u32, Kind, usize, [f64; 3])> {
    let mut q = world.query_filtered::<&Building, With<Ruin>>();
    let mut out: Vec<_> = q
        .iter(world)
        .map(|b| {
            let c = centre(b);
            (b.id, b.kind, b.island, [c.x, b.base_y, c.y])
        })
        .collect();
    out.sort_by_key(|r| r.0);
    out
}

// ---------------------------------------------------------------------------------------
// Looks: the three answering buildings, their crews, and what an incident shows.

#[allow(dead_code)]
pub const MODELS: &[ModelKey] = &[
    "fire_watch",
    "watch_house",
    "infirmary",
    "pump_cart",
    "constables",
    "doctor_gig",
    "rioters",
    "sick_flag",
    "rubble",
    "ruin_shell",
];

pub fn build(key: ModelKey) -> Option<Parts> {
    Some(match key {
        "fire_watch" => fire_watch(),
        "watch_house" => watch_house(),
        "infirmary" => infirmary(),
        "pump_cart" => pump_cart(),
        "constables" => constables(),
        "doctor_gig" => doctor_gig(),
        "rioters" => rioters(),
        "sick_flag" => sick_flag(),
        "rubble" => rubble(),
        "ruin_shell" => ruin_shell(),
        _ => return None,
    })
}

pub fn look(kind: Kind, out: &mut Vec<Piece>) {
    out.push(Piece::at(
        match kind {
            Kind::FireWatch => "fire_watch",
            Kind::WatchHouse => "watch_house",
            _ => "infirmary",
        },
        Vec3::ZERO,
    ));
}

/// A plain block house with a pitched roof, `w` across x and `d` deep, walls `h` high.
#[allow(clippy::too_many_arguments)]
fn block(p: &mut Parts, wall: Mat, col: Vec4, at: Vec3, w: f32, d: f32, h: f32, roof: Vec4) {
    p.cuboid(
        wall,
        at + v3(-w * 0.5, 0.0, -d * 0.5),
        at + v3(w * 0.5, h, d * 0.5),
        col,
        false,
    );
    p.gable_roof(
        Mat::Slate,
        wall,
        at + v3(0.0, h, 0.0),
        w,
        d,
        d * 0.36,
        0.35,
        false,
        roof,
        col,
    );
}

/// Windows on the front and back of a block: `n` each, ground and upper floor.
fn front_windows(p: &mut Parts, at: Vec3, d: f32, h: f32, n: u32, gap: f32) {
    for (z, nz) in [(d * 0.5 + 0.02, 1.0f32), (-d * 0.5 - 0.02, -1.0)] {
        for y in [h * 0.3, h * 0.72] {
            p.windows(
                at + v3(0.0, y, z),
                v3(0.0, 0.0, nz),
                n,
                0.9,
                1.3,
                gap,
                hexc(0xe8e0d0),
            );
        }
    }
}

/// Fire Watch: a brick engine house with two tall red doors, a hose tower with a bell and a
/// hand pump cart standing out front.
fn fire_watch() -> Parts {
    let mut p = Parts::default();
    let brick = hexc(0xa0523a);
    let slate = hexc(0x4a5058);
    block(
        &mut p,
        Mat::Brick,
        brick,
        v3(-1.5, 0.0, 0.5),
        10.0,
        9.0,
        5.2,
        slate,
    );
    // Two arched engine doors, painted fire-engine red.
    for x in [-4.0f32, 1.0] {
        p.cuboid(
            Mat::Paint,
            v3(x - 1.4, 0.0, 5.02),
            v3(x + 1.4, 3.6, 5.12),
            hexc(0xb81e1e),
            false,
        );
        p.cuboid(
            Mat::Stone,
            v3(x - 1.6, 3.6, 5.0),
            v3(x + 1.6, 4.0, 5.16),
            hexc(0xd8ccb0),
            false,
        );
    }
    p.windows(
        v3(-1.5, 4.4, 5.04),
        Vec3::Z,
        3,
        0.8,
        0.7,
        2.4,
        hexc(0xe8e0d0),
    );
    p.windows(
        v3(-1.5, 2.6, -4.02),
        Vec3::NEG_Z,
        3,
        0.9,
        1.2,
        2.6,
        hexc(0xe8e0d0),
    );
    // Hose tower with a louvred top and a bell.
    let t = v3(5.6, 0.0, -1.5);
    p.cuboid(
        Mat::Brick,
        t + v3(-1.6, 0.0, -1.6),
        t + v3(1.6, 12.0, 1.6),
        brick,
        false,
    );
    p.cuboid(
        Mat::Timber,
        t + v3(-1.75, 12.0, -1.75),
        t + v3(1.75, 14.0, 1.75),
        hexc(0x5a3a28),
        false,
    );
    p.hip_roof(
        Mat::Slate,
        t + v3(0.0, 14.0, 0.0),
        3.9,
        3.9,
        2.2,
        0.2,
        slate,
    );
    p.cylinder(
        Mat::Copper,
        t + v3(0.0, 12.4, 0.0),
        0.55,
        0.25,
        0.9,
        10,
        hexc(0xc89a40),
        true,
    );
    // The pump cart in the yard.
    let mut cart = pump_cart();
    cart.ground_occlusion(0.0, 0.0);
    p.add_parts(&cart, Mat4::from_translation(v3(-2.0, 0.0, 8.5)));
    p.ground_occlusion(1.0, 2.0);
    p.finish()
}

/// Watch House: a stone lock-up with a blue lamp over the door, barred windows and a
/// lantern-hung porch.
fn watch_house() -> Parts {
    let mut p = Parts::default();
    let stone = hexc(0xb8b0a0);
    block(
        &mut p,
        Mat::Plaster,
        stone,
        v3(0.0, 0.0, 0.0),
        11.0,
        8.0,
        6.0,
        hexc(0x3c4048),
    );
    front_windows(&mut p, v3(0.0, 0.0, 0.0), 8.0, 6.0, 2, 7.0);
    // Bars over the ground-floor windows.
    for x in [-3.5f32, 3.5] {
        for k in 0..4 {
            let bx = x - 0.6 + k as f32 * 0.4;
            p.beam(
                Mat::Iron,
                v3(bx, 1.2, 4.08),
                v3(bx, 2.6, 4.08),
                0.06,
                hexc(0x222428),
            );
        }
    }
    // Porch and the blue lamp.
    p.cuboid(
        Mat::Paint,
        v3(-0.9, 0.0, 4.02),
        v3(0.9, 2.6, 4.1),
        hexc(0x1c2a48),
        false,
    );
    p.cuboid(
        Mat::Timber,
        v3(-1.8, 2.8, 4.0),
        v3(1.8, 3.0, 6.0),
        hexc(0x4a3628),
        false,
    );
    for x in [-1.6f32, 1.6] {
        p.beam(
            Mat::Timber,
            v3(x, 0.0, 5.8),
            v3(x, 2.8, 5.8),
            0.18,
            hexc(0x4a3628),
        );
    }
    p.cuboid(
        Mat::Lamp,
        v3(-0.3, 3.1, 4.3),
        v3(0.3, 3.7, 4.9),
        hexc(0x4080ff),
        false,
    );
    // A walled yard behind.
    p.cuboid(
        Mat::Plaster,
        v3(-5.5, 0.0, -8.0),
        v3(5.5, 2.2, -7.6),
        stone,
        false,
    );
    p.cuboid(
        Mat::Plaster,
        v3(-5.5, 0.0, -8.0),
        v3(-5.1, 2.2, -4.0),
        stone,
        false,
    );
    p.cuboid(
        Mat::Plaster,
        v3(5.1, 0.0, -8.0),
        v3(5.5, 2.2, -4.0),
        stone,
        false,
    );
    p.ground_occlusion(1.0, 2.0);
    p.finish()
}

/// Infirmary: a long whitewashed hall with tall windows, a red-cross flag (no: a green
/// one, for the herb garden) and the doctor's gig by the door.
fn infirmary() -> Parts {
    let mut p = Parts::default();
    let white = hexc(0xeae4d6);
    block(
        &mut p,
        Mat::Plaster,
        white,
        v3(0.0, 0.0, -1.0),
        16.0,
        8.0,
        5.5,
        hexc(0x6a4a3a),
    );
    for x in [-5.0f32, 5.0] {
        front_windows(&mut p, v3(x, 0.0, -1.0), 8.0, 5.5, 2, 2.2);
    }
    // A porch with columns and a pediment.
    p.cuboid(
        Mat::Plaster,
        v3(-2.2, 0.0, 3.0),
        v3(2.2, 0.35, 5.0),
        white,
        false,
    );
    for x in [-1.8f32, 1.8] {
        p.cylinder(
            Mat::Plaster,
            v3(x, 0.35, 4.6),
            0.22,
            0.2,
            3.6,
            8,
            white,
            true,
        );
    }
    p.gable_roof(
        Mat::Slate,
        Mat::Plaster,
        v3(0.0, 3.95, 4.0),
        2.2,
        4.6,
        1.2,
        0.2,
        true,
        hexc(0x6a4a3a),
        white,
    );
    // Flagpole with a green flag.
    p.rod(
        Mat::Timber,
        v3(7.0, 5.5, -1.0),
        v3(7.0, 10.5, -1.0),
        0.08,
        6,
        hexc(0xe0e0e0),
    );
    p.cuboid(
        Mat::Canvas,
        v3(7.05, 9.0, -1.04),
        v3(9.0, 10.3, -0.96),
        hexc(0x3a9a4a),
        false,
    );
    // The herb garden: low beds in rows.
    for k in 0..4 {
        let x = -6.0 + k as f32 * 3.0;
        p.cuboid(
            Mat::Crop,
            v3(x, 0.0, -9.0),
            v3(x + 2.2, 0.4, -6.0),
            hexc(0x4f8a3a),
            false,
        );
    }
    let mut gig = doctor_gig();
    gig.ground_occlusion(0.0, 0.0);
    p.add_parts(&gig, Mat4::from_translation(v3(5.0, 0.0, 5.5)));
    p.ground_occlusion(1.0, 2.0);
    p.finish()
}

/// A horse in harness, nose along +x, standing at `at`.
fn horse(p: &mut Parts, at: Vec3, coat: Vec4) {
    let mut body = Parts::default();
    body.lathe_x(
        Mat::Wool,
        &[(-0.8, 0.05), (-0.6, 0.35), (0.4, 0.38), (0.8, 0.05)],
        10,
        |_, _| coat,
    );
    p.add_parts(&body, Mat4::from_translation(at + v3(0.0, 1.25, 0.0)));
    p.beam(
        Mat::Wool,
        at + v3(0.6, 1.35, 0.0),
        at + v3(1.0, 2.0, 0.0),
        0.28,
        coat,
    );
    p.box_at(
        Mat::Wool,
        at + v3(1.15, 1.95, 0.0),
        v3(0.3, 0.15, 0.13),
        0.0,
        coat,
    );
    for (x, z) in [(-0.5, -0.18), (-0.5, 0.18), (0.5, -0.18), (0.5, 0.18)] {
        p.beam(
            Mat::Wool,
            at + v3(x, 0.0, z),
            at + v3(x, 1.0, z),
            0.12,
            shade(coat, 0.8),
        );
    }
}

fn wheel(p: &mut Parts, at: Vec3, r: f32, col: Vec4) {
    let mut w = Parts::default();
    w.cylinder(Mat::Timber, v3(0.0, 0.0, -0.05), r, r, 0.1, 12, col, true);
    p.add_parts(
        &w,
        Mat4::from_rotation_translation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2), at),
    );
}

/// The hand pump cart: a red tank on two wheels, a pump beam and a hose reel, drawn by a
/// grey horse.
fn pump_cart() -> Parts {
    let mut p = Parts::default();
    let red = hexc(0xb02020);
    p.cuboid(
        Mat::Paint,
        v3(-1.4, 0.8, -0.65),
        v3(0.6, 1.7, 0.65),
        red,
        true,
    );
    p.beam(
        Mat::Copper,
        v3(-1.2, 1.9, 0.0),
        v3(0.4, 1.9, 0.0),
        0.12,
        hexc(0xc89a40),
    );
    p.beam(
        Mat::Timber,
        v3(-0.4, 1.7, 0.0),
        v3(-0.4, 2.1, 0.0),
        0.1,
        hexc(0x4a3628),
    );
    let mut reel = Parts::default();
    reel.cylinder(
        Mat::Canvas,
        v3(0.0, 0.0, -0.5),
        0.35,
        0.35,
        1.0,
        10,
        hexc(0xc8b890),
        true,
    );
    p.add_parts(
        &reel,
        Mat4::from_rotation_translation(
            Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
            v3(-1.6, 1.2, 0.0),
        ),
    );
    for z in [-0.75f32, 0.75] {
        wheel(&mut p, v3(-0.4, 0.6, z), 0.6, hexc(0x3a2a20));
    }
    p.beam(
        Mat::Timber,
        v3(0.6, 0.85, -0.35),
        v3(2.4, 1.0, -0.35),
        0.07,
        hexc(0x4a3628),
    );
    p.beam(
        Mat::Timber,
        v3(0.6, 0.85, 0.35),
        v3(2.4, 1.0, 0.35),
        0.07,
        hexc(0x4a3628),
    );
    horse(&mut p, v3(2.6, 0.0, 0.0), hexc(0x8a8680));
    // Two firemen in brass helmets on the box.
    for z in [-0.35f32, 0.35] {
        p.cuboid(
            Mat::Wool,
            v3(-0.9, 1.7, z - 0.18),
            v3(-0.6, 2.4, z + 0.18),
            hexc(0x1c2440),
            false,
        );
        p.cylinder(
            Mat::Copper,
            v3(-0.75, 2.4, z),
            0.16,
            0.12,
            0.25,
            8,
            hexc(0xd8a840),
            true,
        );
    }
    p.finish()
}

/// A figure of `h` meters: coat, legs and a head, standing at `at`.
pub(crate) fn figure(p: &mut Parts, at: Vec3, coat: Vec4, hat: Vec4, h: f32) {
    let k = h / 1.75;
    for z in [-0.11f32, 0.11] {
        p.beam(
            Mat::Wool,
            at + v3(0.0, 0.0, z),
            at + v3(0.0, 0.85 * k, z),
            0.14 * k,
            hexc(0x2a2a30),
        );
    }
    p.cuboid(
        Mat::Wool,
        at + v3(-0.15 * k, 0.85 * k, -0.24 * k),
        at + v3(0.15 * k, 1.45 * k, 0.24 * k),
        coat,
        false,
    );
    p.cylinder(
        Mat::Plaster,
        at + v3(0.0, 1.45 * k, 0.0),
        0.12 * k,
        0.12 * k,
        0.24 * k,
        8,
        hexc(0xd8b090),
        true,
    );
    p.cylinder(
        Mat::Wool,
        at + v3(0.0, 1.68 * k, 0.0),
        0.16 * k,
        0.14 * k,
        0.12 * k,
        8,
        hat,
        true,
    );
}

/// Two constables in blue with tall hats, walking side by side.
fn constables() -> Parts {
    let mut p = Parts::default();
    for z in [-0.45f32, 0.45] {
        figure(
            &mut p,
            v3(0.0, 0.0, z),
            hexc(0x1c2a58),
            hexc(0x14182a),
            1.85,
        );
        p.beam(
            Mat::Timber,
            v3(0.1, 0.9, z + 0.3),
            v3(0.35, 1.3, z + 0.3),
            0.05,
            hexc(0x3a2a1a),
        );
    }
    p.finish()
}

/// The doctor's gig: a light two-wheeled trap with a black hood, and a bay pony.
fn doctor_gig() -> Parts {
    let mut p = Parts::default();
    let black = hexc(0x1e1e22);
    p.cuboid(
        Mat::Paint,
        v3(-1.0, 0.9, -0.6),
        v3(0.4, 1.3, 0.6),
        black,
        true,
    );
    // The hood.
    p.cuboid(
        Mat::Canvas,
        v3(-1.0, 1.3, -0.62),
        v3(-0.6, 2.2, 0.62),
        black,
        false,
    );
    p.cuboid(
        Mat::Canvas,
        v3(-1.0, 2.1, -0.62),
        v3(-0.1, 2.25, 0.62),
        black,
        false,
    );
    for z in [-0.72f32, 0.72] {
        wheel(&mut p, v3(-0.3, 0.75, z), 0.75, hexc(0x8a1a1a));
    }
    p.beam(
        Mat::Timber,
        v3(0.4, 1.0, -0.3),
        v3(2.2, 1.0, -0.3),
        0.06,
        black,
    );
    p.beam(
        Mat::Timber,
        v3(0.4, 1.0, 0.3),
        v3(2.2, 1.0, 0.3),
        0.06,
        black,
    );
    horse(&mut p, v3(2.4, 0.0, 0.0), hexc(0x6a3a1e));
    // The doctor with her bag.
    figure(
        &mut p,
        v3(-0.4, 1.3, 0.0),
        hexc(0x2a2a2a),
        hexc(0x101012),
        1.2,
    );
    p.cuboid(
        Mat::Wool,
        v3(-0.2, 1.3, 0.3),
        v3(0.1, 1.5, 0.5),
        hexc(0x5a3418),
        false,
    );
    p.finish()
}

/// A crowd with banners, torches and an overturned cart for a barricade.
fn rioters() -> Parts {
    let mut p = Parts::default();
    let coats = [0x6a4a30, 0x4a5a3a, 0x5a3a3a, 0x3a4a5a, 0x7a6a4a, 0x4a3a2a];
    for k in 0..12 {
        let a = k as f32 * 2.399;
        let r = 1.2 + (k % 4) as f32 * 0.9;
        let at = v3(a.cos() * r, 0.0, a.sin() * r * 1.4);
        figure(
            &mut p,
            at,
            hexc(coats[k % coats.len()]),
            hexc(0x2a241e),
            1.7,
        );
        if k % 3 == 0 {
            // A torch held up.
            p.beam(
                Mat::Timber,
                at + v3(0.1, 1.2, 0.3),
                at + v3(0.2, 2.3, 0.35),
                0.05,
                hexc(0x3a2a1a),
            );
            p.cuboid(
                Mat::Lamp,
                at + v3(0.12, 2.25, 0.28),
                at + v3(0.28, 2.5, 0.44),
                hexc(0xffa030),
                false,
            );
        }
        if k % 4 == 1 {
            // A banner on two poles.
            p.beam(
                Mat::Timber,
                at + v3(0.0, 0.0, -0.7),
                at + v3(0.0, 2.8, -0.7),
                0.05,
                hexc(0x3a2a1a),
            );
            p.beam(
                Mat::Timber,
                at + v3(0.0, 0.0, 0.7),
                at + v3(0.0, 2.8, 0.7),
                0.05,
                hexc(0x3a2a1a),
            );
            p.cuboid(
                Mat::Canvas,
                at + v3(-0.02, 2.0, -0.7),
                at + v3(0.02, 2.7, 0.7),
                hexc(0xc02a20),
                false,
            );
        }
    }
    // The barricade: an overturned cart, barrels and a heap of crates across the street.
    let wood = hexc(0x6a4a2a);
    p.box_at(Mat::Planks, v3(4.5, 0.6, 0.0), v3(0.7, 0.6, 1.6), 0.3, wood);
    wheel(&mut p, v3(4.0, 1.4, -1.0), 0.6, shade(wood, 0.7));
    for (x, z) in [(5.2f32, 1.8f32), (5.0, -1.9), (3.6, 2.4)] {
        p.cylinder(
            Mat::Planks,
            v3(x, 0.0, z),
            0.4,
            0.4,
            0.9,
            10,
            shade(wood, 0.9),
            true,
        );
    }
    p.box_at(
        Mat::Planks,
        v3(4.2, 0.4, 2.8),
        v3(0.5, 0.4, 0.5),
        0.6,
        shade(wood, 1.1),
    );
    p.finish()
}

/// A yellow flag on a pole by the door: sickness in the house.
fn sick_flag() -> Parts {
    let mut p = Parts::default();
    p.rod(
        Mat::Timber,
        v3(0.0, 0.0, 0.0),
        v3(0.0, 4.2, 0.0),
        0.06,
        6,
        hexc(0x5a4030),
    );
    p.cuboid(
        Mat::Canvas,
        v3(0.05, 3.0, -0.03),
        v3(1.5, 4.0, 0.03),
        hexc(0xf0d020),
        false,
    );
    // A cross of boards over the door way.
    p.beam(
        Mat::Planks,
        v3(-0.8, 0.2, 0.4),
        v3(0.8, 2.0, 0.4),
        0.14,
        hexc(0x8a6a40),
    );
    p.beam(
        Mat::Planks,
        v3(0.8, 0.2, 0.4),
        v3(-0.8, 2.0, 0.4),
        0.14,
        hexc(0x8a6a40),
    );
    p.finish()
}

/// The burnt-out shell of a building, 8 m square: broken brick walls blackened by smoke,
/// empty window holes, charred joists fallen across the inside, no roof.
fn ruin_shell() -> Parts {
    let mut p = Parts::default();
    let soot = hexc(0x2c2622);
    let brick = hexc(0x6a3a2a);
    let char_wood = hexc(0x16120f);
    // Each wall: segments of uneven height with gaps for the door and windows.
    let walls: [(Vec3, Vec3); 4] = [
        (v3(-4.0, 0.0, 4.0), v3(4.0, 0.0, 4.0)),
        (v3(4.0, 0.0, 4.0), v3(4.0, 0.0, -4.0)),
        (v3(4.0, 0.0, -4.0), v3(-4.0, 0.0, -4.0)),
        (v3(-4.0, 0.0, -4.0), v3(-4.0, 0.0, 4.0)),
    ];
    let heights = [
        [4.6, 3.9, 0.0, 2.8, 5.2, 4.1, 1.6, 3.4],
        [3.1, 2.2, 4.8, 4.4, 0.9, 3.6, 4.9, 2.4],
        [1.4, 2.9, 3.3, 0.0, 4.2, 5.0, 3.8, 2.1],
        [5.1, 4.4, 2.6, 1.2, 3.0, 0.0, 2.7, 4.0],
    ];
    for (wi, (a, b)) in walls.iter().enumerate() {
        let seg = 8;
        for (k, &h) in heights[wi].iter().enumerate() {
            if h <= 0.01 {
                continue;
            }
            let t0 = k as f32 / seg as f32;
            let t1 = (k + 1) as f32 / seg as f32;
            let lo = a.lerp(*b, t0);
            let hi = a.lerp(*b, t1);
            let mid = (lo + hi) * 0.5;
            let along = hi - lo;
            let yaw = along.z.atan2(along.x);
            let half = v3(along.length() * 0.5 + 0.02, h * 0.5, 0.22);
            // Sooty above, some brick showing low down where the smoke did not reach.
            let col = if k % 3 == 1 { brick } else { soot };
            p.box_at(Mat::Brick, mid + v3(0.0, h * 0.5, 0.0), half, -yaw, col);
            // An empty window hole: a black opening on the taller segments.
            if h > 3.4 && k % 2 == 0 {
                p.box_at(
                    Mat::Stone,
                    mid + v3(0.0, 2.3, 0.0),
                    v3(along.length() * 0.28, 0.55, 0.24),
                    -yaw,
                    hexc(0x0a0908),
                );
            }
        }
    }
    // Joists fallen in across the walls, a rafter or two still leaning.
    for (a, b) in [
        (v3(-3.8, 3.6, 3.6), v3(3.2, 0.4, -2.0)),
        (v3(3.8, 4.1, 2.0), v3(-2.5, 0.3, 1.0)),
        (v3(-3.6, 0.3, -2.8), v3(2.8, 2.2, -3.8)),
        (v3(0.5, 0.2, 3.2), v3(-1.0, 3.8, -3.6)),
    ] {
        p.beam(Mat::Timber, a, b, 0.26, char_wood);
    }
    // Heaps of rubble inside.
    for k in 0..9 {
        let a = k as f32 * 2.4;
        let r = 0.6 + (k % 4) as f32 * 0.8;
        p.box_at(
            Mat::Stone,
            v3(a.cos() * r, 0.3, a.sin() * r),
            v3(0.7, 0.3 + (k % 3) as f32 * 0.15, 0.55),
            a,
            shade(soot, 0.8 + (k % 3) as f32 * 0.12),
        );
    }
    p.finish()
}

/// A heap of blackened rubble with charred beams sticking out.
fn rubble() -> Parts {
    let mut p = Parts::default();
    let soot = hexc(0x2a2522);
    for k in 0..7 {
        let a = k as f32 * 2.1;
        let r = 0.4 + (k % 3) as f32 * 0.6;
        p.box_at(
            Mat::Stone,
            v3(a.cos() * r, 0.25, a.sin() * r),
            v3(0.5, 0.25 + (k % 2) as f32 * 0.15, 0.4),
            a,
            shade(soot, 0.8 + (k % 3) as f32 * 0.15),
        );
    }
    for (a, b) in [
        (v3(-1.2, 0.0, -0.5), v3(0.8, 1.6, 0.4)),
        (v3(0.9, 0.0, -1.0), v3(-0.4, 2.2, 0.6)),
        (v3(-0.3, 0.0, 1.2), v3(0.5, 1.1, -0.8)),
    ] {
        p.beam(Mat::Timber, a, b, 0.22, hexc(0x1a1614));
    }
    p.finish()
}

/// `isle.alerts`, `isle.incidents`, `isle.ruin.rebuild`: incidents for scripts, the playtest
/// and the interface.
pub fn register(app: &mut App) {
    use serde_json::{Value, json};
    app.register_api(
        "isle.alerts",
        "Fires, riots and sickness going on now, newest last: what, where (building id, island, map point), how bad, whether a crew is on it.",
        "{}",
        |world, _| {
            let alerts = alerts(world);
            Ok(serde_json::to_value(alerts).unwrap_or(Value::Null))
        },
    )
    .register_api(
        "isle.incidents",
        "Totals of fires, riots and sickness so far, the ruins standing, islands under the yellow flag, and the last incidents logged.",
        "{}",
        |world, _| {
            let ruins: Vec<Value> = ruins(world)
                .into_iter()
                .map(|(id, k, i, at)| json!({"building": id, "kind": k.def().name, "island": i, "at": at, "rebuild": rebuild_cost(k).0}))
                .collect();
            let inc = world.resource::<Incidents>();
            let log: Vec<Value> = inc
                .log
                .iter()
                .rev()
                .take(40)
                .map(|(t, i, s)| json!({"at_s": t, "island": i, "text": s}))
                .collect();
            let mut flagged: Vec<usize> = inc.flagged.iter().copied().collect();
            flagged.sort();
            Ok(json!({"tally": inc.tally, "ruins": ruins, "yellow_flag": flagged, "log": log}))
        },
    )
    .register_api(
        "isle.ruin.rebuild",
        "Rebuild a burnt-out building for half its price in coins and goods.",
        r#"{"building": 31}"#,
        |world, args| {
            let id = args["building"].as_u64().ok_or("building wants an id")? as u32;
            rebuild(world, id)?;
            Ok(json!({"ok": true}))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::town::tests::bare_world;

    fn first(w: &mut World, kind: Kind, isle: usize) -> (Entity, Building) {
        let mut q = w.query::<(Entity, &Building)>();
        q.iter(w)
            .filter(|(_, b)| b.kind == kind && b.island == isle)
            .min_by_key(|(_, b)| b.id)
            .map(|(e, b)| (e, b.clone()))
            .expect("a building of that kind")
    }

    fn run(w: &mut World, s: u32) {
        for _ in 0..s {
            w.resource_mut::<Economy>().clock_s += 1.0;
            step(w, 1.0);
        }
    }

    #[test]
    fn fires_burn_out_unless_the_fire_watch_comes_and_riots_and_sickness_end() {
        let mut w = bare_world();
        crate::town::build_start(&mut w);
        crate::build::refresh(&mut w);
        crate::town::prewarm(&mut w, 300.0);
        // No rolls in this test: only the incidents it starts.
        w.resource_mut::<Incidents>().rolls = false;
        // The sandbox start has its stations: take them down first.
        let stations: Vec<Entity> = w
            .query::<(Entity, &Building)>()
            .iter(&w)
            .filter(|(_, b)| matches!(b.kind, Kind::FireWatch | Kind::WatchHouse | Kind::Infirmary))
            .map(|(e, _)| e)
            .collect();
        for e in stations {
            crate::build::demolish(&mut w, e);
        }
        crate::build::refresh(&mut w);
        // A bakery with no Fire Watch near burns down.
        let (bakery, b) = first(&mut w, Kind::Sawmill, 0);
        assert!(start(&mut w, bakery, &b, Trouble::Fire, false));
        assert_eq!(w.get::<Producer>(bakery).unwrap().hit, Hit::Fire);
        run(&mut w, 90);
        assert!(
            w.get::<Ruin>(bakery).is_some(),
            "the bakery should be a ruin"
        );
        assert_eq!(w.get::<Producer>(bakery).unwrap().hit, Hit::Ruin);
        assert!(w.resource::<Incidents>().tally.burnt >= 1);
        // Rebuilt for half its price.
        w.resource_mut::<Economy>().coins = 50_000.0;
        for g in Good::ALL {
            w.resource_mut::<Economy>().islands[0].add(g, 50.0);
        }
        rebuild(&mut w, b.id).unwrap();
        assert!(w.get::<Ruin>(bakery).is_none());
        assert_eq!(w.get::<Producer>(bakery).unwrap().hit, Hit::Clear);
        // A Fire Watch by the market: its pump drives out and puts a house fire out.
        let (_, market) = first(&mut w, Kind::Market, 0);
        let at = centre(&market);
        let watch = crate::town::place_near(&mut w, Kind::FireWatch, at, 30, true)
            .expect("room for a fire watch");
        crate::build::refresh(&mut w);
        let wb = w.get::<Building>(watch).unwrap().clone();
        let mut q = w.query::<(Entity, &Building, &House)>();
        let (house, hb) = q
            .iter(&w)
            .filter(|(_, b, _)| b.island == 0)
            .min_by_key(|(_, b, _)| ((centre(b) - centre(&wb)).length() * 10.0) as i64)
            .map(|(e, b, _)| (e, b.clone()))
            .unwrap();
        // The fire watch makes fires a quarter as likely in its reach.
        run(&mut w, 1);
        assert!(
            w.resource::<Incidents>()
                .cover
                .get(&house)
                .is_some_and(|c| c[0])
        );
        assert!(start(&mut w, house, &hb, Trouble::Fire, false));
        run(&mut w, 120);
        assert!(
            w.get::<Ruin>(house).is_none(),
            "the pump should have saved the house"
        );
        assert!(
            w.resource::<Incidents>().tally.put_out[0] >= 1,
            "{:?}",
            w.resource::<Incidents>().tally
        );
        // A riot and a sickness with nobody to answer them run their course.
        let mut q = w.query::<(Entity, &Building, &House)>();
        let mut far: Vec<(Entity, Building)> = q
            .iter(&w)
            .filter(|(_, b, _)| b.island == 0)
            .map(|(e, b, _)| (e, b.clone()))
            .collect();
        far.sort_by_key(|(_, b)| -((centre(b) - centre(&wb)).length() * 10.0) as i64);
        assert!(start(&mut w, far[0].0, &far[0].1, Trouble::Riot, false));
        assert!(start(&mut w, far[1].0, &far[1].1, Trouble::Sickness, false));
        assert_eq!(w.get::<House>(far[0].0).unwrap().hit, Hit::Riot);
        assert!(alerts(&mut w).len() >= 2);
        run(&mut w, 400);
        assert_eq!(w.get::<House>(far[0].0).unwrap().hit, Hit::Clear);
        assert_eq!(w.get::<House>(far[1].0).unwrap().hit, Hit::Clear);
        let t = w.resource::<Incidents>().tally.clone();
        assert!(t.ended[1] >= 1 && t.ended[2] >= 1, "{t:?}");
    }

    #[test]
    fn a_big_town_has_an_incident_every_few_minutes() {
        let mut w = bare_world();
        crate::town::build_start(&mut w);
        crate::build::refresh(&mut w);
        crate::town::prewarm(&mut w, 300.0);
        run(&mut w, 1800);
        let t = w.resource::<Incidents>().tally.clone();
        let n: u32 = t.started.iter().sum();
        eprintln!("30 minutes: {t:?}");
        assert!(
            (3..=40).contains(&n),
            "{n} incidents in half an hour: {t:?}"
        );
    }

    #[test]
    fn models_build_within_budget() {
        for k in MODELS {
            let t = build(k).unwrap().triangles();
            assert!(t > 20 && t < 20_000, "{k}: {t}");
        }
    }
}
