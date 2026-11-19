//! The town economy explained: every number a house card, an island panel or the money panel
//! shows, already added up (`isle.house`, `isle.island.economy`, `isle.ledger`, `isle.work`),
//! and the player's moves on it (pause a need, move houses up or down, let an island's houses
//! move up on their own).
//! The interface only draws what these return.

use engine::prelude::*;
use serde_json::{Value, json};

use crate::build;
use crate::defs::{Cat, Good, Need, Tier};
use crate::island::islands;
use crate::items::Boost;
use crate::sim::{
    self, Appraisal, Building, Economy, Hit, House, IslandBooks, Mood, Producer, Status, Wait,
};

/// An island by index or by name.
pub fn island_arg(args: &Value) -> Result<usize, String> {
    let n = islands().len();
    match &args["island"] {
        Value::Number(i) => i
            .as_u64()
            .map(|i| i as usize)
            .filter(|i| *i < n)
            .ok_or_else(|| format!("no island {i}")),
        Value::String(s) => islands()
            .iter()
            .position(|i| i.name.eq_ignore_ascii_case(s))
            .ok_or_else(|| format!("no island called {s}")),
        _ => Err("island wants an index or a name".into()),
    }
}

fn house_arg(world: &mut World, args: &Value) -> Result<Entity, String> {
    let id = args["id"]
        .as_u64()
        .ok_or("id wants a house id (see isle.list)")?;
    let e = build::by_id(world, id as u32).ok_or("no building with that id")?;
    if world.get::<House>(e).is_none() {
        return Err("that building is not a house".into());
    }
    Ok(e)
}

fn need_name(n: Need) -> &'static str {
    match n {
        Need::Good(g) => g.name(),
        Need::Service(k) => k.def().name,
    }
}

fn low(s: &str) -> String {
    s.to_lowercase()
}

/// "bread or fish": the names of a category's needs that the island asks for.
fn names_of(tier: Tier, cat: Cat, a: &Appraisal) -> String {
    let names: Vec<String> = tier
        .wants()
        .iter()
        .enumerate()
        .filter(|(j, w)| w.cat == cat && a.lines[*j].asked)
        .map(|(_, w)| low(need_name(w.need)))
        .collect();
    names.join(" or ")
}

/// The next need of a tier to unlock on an island: (need, people it takes).
pub fn next_unlock(tier: Tier, books: &IslandBooks) -> Option<(Need, f32)> {
    tier.wants()
        .iter()
        .enumerate()
        .filter(|(j, _)| books.unlocked[tier.index()] & (1 << j) == 0)
        .map(|(_, w)| (w.need, w.unlock))
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

/// One line on why a house is waiting, or that it is ready.
pub fn wait_text(h: &House, a: &Appraisal, books: &IslandBooks) -> String {
    let tier = h.tier;
    match h.wait {
        Wait::None => "Ready to move up".into(),
        Wait::Top => format!("{} are the top tier", tier.name()),
        Wait::Trouble => match h.hit {
            Hit::Fire => "On fire".into(),
            Hit::Riot => "A riot in the house".into(),
            Hit::Sick => "Sickness in the house".into(),
            _ => "Burnt down".into(),
        },
        Wait::NotFull => {
            // The asked-for need that would bring the most people.
            let gap = tier
                .wants()
                .iter()
                .enumerate()
                .filter(|(j, w)| w.basic() && a.lines[*j].asked && a.lines[*j].met < 0.95)
                .max_by(|(i, x), (j, y)| {
                    (x.people * (1.0 - a.lines[*i].met))
                        .total_cmp(&(y.people * (1.0 - a.lines[*j].met)))
                })
                .map(|(_, w)| low(need_name(w.need)));
            let head = format!("{:.0} of {:.0} people", h.residents.floor(), a.size);
            match gap {
                Some(n) => format!("{head}: needs {n} to fill"),
                None => format!("{head}: filling"),
            }
        }
        Wait::Short(c) => {
            if !a.asked[c.index()] {
                let next = tier
                    .wants()
                    .iter()
                    .filter(|w| w.cat == c)
                    .map(|w| (w.need, w.unlock))
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                match next {
                    Some((n, at)) => format!(
                        "{}: nothing asked yet ({} from {at:.0} {})",
                        c.name(),
                        low(need_name(n)),
                        tier.name()
                    ),
                    None => format!("{}: nothing to meet it", c.name()),
                }
            } else {
                format!(
                    "{} {:.1} of {:.0}: more {}",
                    c.name(),
                    a.points[c.index()],
                    tier.threshold(c),
                    names_of(tier, c, a)
                )
            }
        }
        Wait::Angry => format!("Too unhappy to move up (happiness {:.0})", h.happiness),
        Wait::NeedsPaving => {
            let next = tier.next().map_or("They", |(t, _)| t.name());
            format!("{next} want a paved street at the door")
        }
        Wait::NoMaterials => {
            let cost = tier.next().map_or(&[][..], |(_, c)| c);
            let keep = sim::BUILD_RESERVE;
            cost.iter()
                .find(|(g, c)| books.get(*g) < *c as f64 + keep)
                .map(|(g, c)| {
                    format!(
                        "Moving up takes {c:.0} {}; the warehouse has {:.0} and keeps {keep:.0} for building",
                        low(g.name()),
                        books.get(*g).floor()
                    )
                })
                .unwrap_or_else(|| "Waiting for building goods".into())
        }
        Wait::WouldEmptyWorks => format!(
            "Ready, but the {}' works need these {:.0} people",
            tier.name(),
            h.residents
        ),
    }
}

/// Everything about one house, added up: `isle.house`.
pub fn house(world: &mut World, e: Entity) -> Result<Value, String> {
    let (Some(b), Some(h)) = (
        world.get::<Building>(e).cloned(),
        world.get::<House>(e).cloned(),
    ) else {
        return Err("not a house".into());
    };
    let boost = world.get::<Boost>(e).cloned();
    let econ = world.resource::<Economy>();
    let books = &econ.islands[b.island];
    let a = sim::appraise(&h, books, boost.as_ref());
    let tier = h.tier;
    let base = tier.capacity();
    // What the needs pay scales with how full the house is (see `sim::step`).
    let scale = (h.residents / base).min(a.size / base);
    let hit_pay = match h.hit {
        Hit::Clear => 1.0,
        Hit::Sick => 0.35,
        _ => 0.0,
    };
    let needs: Vec<Value> = tier
        .wants()
        .iter()
        .enumerate()
        .map(|(j, w)| {
            let l = a.lines[j];
            let unlocked = books.unlocked[tier.index()] & (1 << j) != 0;
            let paused = matches!(w.need, Need::Good(g) if books.paused[g.index()]);
            json!({
                "need": need_name(w.need),
                "category": w.cat.name(),
                "asked": l.asked,
                "locked_until": if unlocked { Value::Null } else { json!(w.unlock) },
                "paused": paused,
                "supply": l.met,
                "points": w.points * l.met,
                "people": l.people,
                "coins": l.coins * scale * hit_pay,
                "joy": l.joy,
            })
        })
        .collect();
    let categories: Vec<Value> = Cat::BASIC
        .iter()
        .map(|c| {
            let th = tier.threshold(*c);
            json!({
                "category": c.name(),
                "points": a.points[c.index()],
                "threshold": th,
                "asked": a.asked[c.index()],
                "met": a.asked[c.index()] && a.points[c.index()] >= th - 1e-4,
            })
        })
        .collect();
    let mut pulls = vec![json!({"why": "Every house starts at", "points": 40.0})];
    if a.joy > 0.0 {
        pulls.push(json!({"why": "Joy of what it has", "points": a.joy}));
    }
    for c in Cat::BASIC {
        let th = tier.threshold(c);
        let p = a.points[c.index()];
        if a.asked[c.index()] && p < th * 0.995 {
            pulls.push(
                json!({"why": format!("{} short", c.name()), "points": -sim::SHORT_GLOOM * (1.0 - p / th)}),
            );
        }
    }
    if a.smoke > 0.0 {
        pulls.push(json!({"why": "Smoke from a works nearby", "points": -a.smoke}));
    }
    if a.trouble > 0.0 {
        let why = if h.hit == Hit::Clear {
            "Trouble in the street"
        } else {
            "Trouble in the house"
        };
        pulls.push(json!({"why": why, "points": -a.trouble}));
    }
    if a.crowding > 0.0 {
        pulls.push(json!({"why": format!("A big town of {}", tier.name()), "points": -a.crowding}));
    }
    let items = boost
        .as_ref()
        .map_or(0.0, |x| x.flat_pay() + x.tier_pay(tier))
        * scale
        * hit_pay;
    Ok(json!({
        "id": b.id,
        "island": islands()[b.island].name,
        "tier": tier.name(),
        "residents": h.residents,
        "size": a.size,
        "fills_toward": a.residents,
        "happiness": h.happiness,
        "target": h.target,
        "mood": h.mood().name(),
        "pulls": pulls,
        "needs": needs,
        "categories": categories,
        "rent": h.rent,
        "goodwill": h.goodwill,
        "items": items,
        "income": h.income,
        "ready": h.ready,
        "ready_for_s": h.content_s,
        "wait": h.wait,
        "wait_text": wait_text(&h, &a, books),
        "next": tier.next().map(|(t, cost)| json!({
            "tier": t.name(),
            "cost": cost.iter().map(|(g, n)| json!([g.name(), n])).collect::<Vec<_>>(),
        })),
    }))
}

/// Per tier on an island: people, houses, moods, ready houses, waiting reasons, the next
/// need to unlock: `isle.island.economy`.
pub fn island(world: &mut World, i: usize) -> Value {
    let mut waits: Vec<std::collections::BTreeMap<String, u32>> =
        vec![Default::default(); Tier::COUNT];
    let mut happiness = [0.0f32; Tier::COUNT];
    let mut houses = [0u32; Tier::COUNT];
    let mut income = [0.0f32; Tier::COUNT];
    for (b, h) in world.query::<(&Building, &House)>().iter(world) {
        if b.island != i {
            continue;
        }
        let t = h.tier.index();
        houses[t] += 1;
        happiness[t] += h.happiness;
        income[t] += h.income;
        let key = match serde_json::to_value(h.wait) {
            Ok(Value::String(s)) => s,
            Ok(v) => v.to_string(),
            Err(_) => String::new(),
        };
        *waits[t].entry(key).or_default() += 1;
    }
    let econ = world.resource::<Economy>();
    let books = &econ.islands[i];
    let tiers: Vec<Value> = Tier::ALL
        .iter()
        .map(|t| {
            let k = t.index();
            let moods: serde_json::Map<String, Value> = Mood::ALL
                .iter()
                .map(|m| (m.name().to_string(), json!(books.moods[k][m.index()])))
                .collect();
            json!({
                "tier": t.name(),
                "people": books.population[k],
                "houses": houses[k],
                "happiness": if houses[k] > 0 { happiness[k] / houses[k] as f32 } else { 0.0 },
                "moods": moods,
                "ready": books.ready[k],
                "waiting": waits[k],
                "income": income[k],
                "next_unlock": next_unlock(*t, books).map(|(n, at)| format!("{} from {at:.0} {}", need_name(n), t.name())),
            })
        })
        .collect();
    let paused: Vec<&str> = Good::ALL
        .iter()
        .filter(|g| books.paused[g.index()])
        .map(|g| g.name())
        .collect();
    json!({
        "island": islands()[i].name,
        "auto_up": books.auto_up,
        "paused_needs": paused,
        "tiers": tiers,
    })
}

/// Where the money comes from and goes: `isle.ledger`.
pub fn ledger(world: &mut World) -> Value {
    // Income by tier: rent, goodwill and each need.
    let mut rent = [0.0f32; Tier::COUNT];
    let mut goodwill = [0.0f32; Tier::COUNT];
    let mut total = [0.0f32; Tier::COUNT];
    let mut by_need: Vec<std::collections::BTreeMap<&'static str, f32>> =
        vec![Default::default(); Tier::COUNT];
    let houses: Vec<(Building, House, Option<Boost>)> = world
        .query::<(&Building, &House, Option<&Boost>)>()
        .iter(world)
        .map(|(b, h, x)| (b.clone(), h.clone(), x.cloned()))
        .collect();
    let econ = world.resource::<Economy>().clone();
    for (b, h, boost) in &houses {
        let t = h.tier.index();
        rent[t] += h.rent;
        goodwill[t] += h.goodwill;
        total[t] += h.income;
        let a = sim::appraise(h, &econ.islands[b.island], boost.as_ref());
        let base = h.tier.capacity();
        let scale = (h.residents / base).min(a.size / base)
            * match h.hit {
                Hit::Clear => 1.0,
                Hit::Sick => 0.35,
                _ => 0.0,
            };
        for (j, w) in h.tier.wants().iter().enumerate() {
            let c = a.lines[j].coins * scale;
            if c > 0.0 {
                *by_need[t].entry(need_name(w.need)).or_default() += c;
            }
        }
    }
    // Costs by kind and state.
    let mut costs: std::collections::BTreeMap<(String, &'static str), (u32, f32)> =
        Default::default();
    for (b, p, boost) in world
        .query::<(&Building, &Producer, Option<&Boost>)>()
        .iter(world)
    {
        if p.hit == Hit::Ruin {
            continue;
        }
        let def = b.kind.def();
        let (cut, thrift) = boost.map_or((0.0, 0.0), |x| (x.upkeep_cut, x.thrift));
        let full = (def.upkeep * (1.0 - cut) - thrift).max(0.0);
        let (state, share) = if p.hit != Hit::Clear {
            ("trouble", 1.0)
        } else if p.paused {
            ("paused", p.upkeep_share())
        } else if p.idle_s >= sim::LET_GO_S {
            ("idle", p.upkeep_share())
        } else {
            ("running", p.upkeep_share())
        };
        let e = costs.entry((def.name.to_string(), state)).or_default();
        e.0 += 1;
        e.1 += full * share;
    }
    let costs: Vec<Value> = costs
        .into_iter()
        .map(|((kind, state), (n, c))| json!({"kind": kind, "state": state, "count": n, "per_min": c}))
        .collect();
    let drains: Vec<Value> = econ
        .drains
        .iter()
        .map(|d| {
            json!({
                "text": drain_line(d),
                "kind": d.kind, "island": islands()[d.island].name, "count": d.count, "idle": d.idle, "per_min": d.per_min,
            })
        })
        .collect();
    json!({
        "coins": econ.coins,
        "taxes_per_min": econ.taxes_per_min,
        "upkeep_per_min": econ.upkeep_per_min,
        "balance_per_min": econ.balance_per_min,
        "in_debt_s": econ.debt_s,
        "auto_paused": econ.auto_paused,
        "income": Tier::ALL.iter().map(|t| {
            let k = t.index();
            json!({"tier": t.name(), "total": total[k], "rent": rent[k], "goodwill": goodwill[k], "needs": by_need[k]})
        }).collect::<Vec<_>>(),
        "costs": costs,
        "public_upkeep": econ.public_upkeep,
        "fleet_upkeep": econ.fleet_upkeep,
        "drains": drains,
        "chains": chains(),
    })
}

/// What each chain costs and pays at half load, from the tables (see `ledger`).
fn chains() -> Vec<Value> {
    let mut v = Vec::new();
    for t in Tier::ALL {
        for w in t.wants() {
            let Need::Good(g) = w.need else { continue };
            let (Some(c), Some((houses, pays))) =
                (crate::ledger::chain(g), crate::ledger::half_load(t, g))
            else {
                continue;
            };
            v.push(json!({
                "tier": t.name(), "good": c.good.name(), "upkeep": c.upkeep, "per_min": c.per_min,
                "works": c.works.iter().map(|(k, n)| json!([k.def().name, n])).collect::<Vec<_>>(),
                "feeds_houses": houses, "pays_at_half_load": pays,
            }));
        }
    }
    v
}

/// Per island and tier: people, jobs, jobs let go, share filled, works short: `isle.work`.
pub fn work(world: &mut World, only: Option<usize>) -> Value {
    let mut short: Vec<[Vec<String>; Tier::COUNT]> = vec![Default::default(); islands().len()];
    for (b, p, boost) in world
        .query::<(&Building, &Producer, Option<&Boost>)>()
        .iter(world)
    {
        let Some((t, _)) = b.kind.def().workers else {
            continue;
        };
        let t = boost.and_then(|x| x.workers).unwrap_or(t);
        if p.status == Status::NoWorkers {
            short[b.island][t.index()].push(format!("{} {}", b.kind.def().name, b.id));
        }
    }
    let econ = world.resource::<Economy>();
    let v: Vec<Value> = econ
        .islands
        .iter()
        .enumerate()
        .filter(|(i, b)| {
            only.is_none_or(|o| o == *i) && (only.is_some() || sim::population(b) > 0.0)
        })
        .map(|(i, b)| {
            json!({
                "island": islands()[i].name,
                "tiers": Tier::ALL.iter().map(|t| {
                    let k = t.index();
                    json!({
                        "tier": t.name(),
                        "people": b.population[k],
                        "jobs": b.jobs[k],
                        "let_go": b.let_go[k],
                        "filled": b.workforce[k],
                        "filled_first": b.staff_first[k],
                        "filled_rest": b.staff_rest[k],
                        "short": short[i][k],
                    })
                }).collect::<Vec<_>>(),
            })
        })
        .collect();
    Value::Array(v)
}

/// Stop or resume handing out a good on an island.
pub fn pause_need(world: &mut World, i: usize, g: Good, paused: bool) {
    world.resource_mut::<Economy>().islands[i].paused[g.index()] = paused;
}

/// Hold an island: no house there moves up on its own.
/// Let ready houses on an island move up on their own (off by default), keeping enough
/// people for the works of the tier they leave.
pub fn auto_up(world: &mut World, i: usize, on: bool) {
    world.resource_mut::<Economy>().islands[i].auto_up = on;
}

/// The player moves a house up now: it must be ready; pays the materials. It may leave the
/// tier below short of workers: the answer says so (`Ok(Some(warning))`).
pub fn move_up(world: &mut World, e: Entity) -> Result<Option<String>, String> {
    let (Some(b), Some(h)) = (
        world.get::<Building>(e).cloned(),
        world.get::<House>(e).cloned(),
    ) else {
        return Err("not a house".into());
    };
    let free = world.get::<Boost>(e).is_some_and(|x| x.rise);
    if !h.ready {
        let econ = world.resource::<Economy>();
        let a = sim::appraise(&h, &econ.islands[b.island], world.get::<Boost>(e));
        return Err(format!(
            "Not ready: {}",
            wait_text(&h, &a, &econ.islands[b.island])
        ));
    }
    let why = sim::up_wait(
        &h,
        &world.resource::<Economy>().islands[b.island],
        free,
        true,
    );
    if why != Wait::None {
        let mut shown = h.clone();
        shown.wait = why;
        let econ = world.resource::<Economy>();
        let a = sim::appraise(&h, &econ.islands[b.island], world.get::<Boost>(e));
        return Err(wait_text(&shown, &a, &econ.islands[b.island]));
    }
    let mut econ = world.resource_mut::<Economy>();
    let books = &mut econ.islands[b.island];
    let left = h.tier;
    let mut h2 = h;
    sim::move_up(&mut h2, books, free);
    let t = left.index();
    let (pop, jobs) = (books.population[t], books.jobs[t] + books.let_go[t]);
    if let Some(mut h) = world.get_mut::<House>(e) {
        *h = h2;
    }
    Ok((pop < jobs).then(|| {
        format!(
            "{} on {} are now {:.0} short of the {:.0} their works ask for",
            left.name(),
            islands()[b.island].name,
            jobs - pop,
            jobs
        )
    }))
}

/// The player moves every ready house on an island up, longest ready first, as long as the
/// tier each leaves keeps the people its works ask for and the warehouse has the materials.
/// Returns (moved, still waiting with their reasons).
pub fn move_up_all(world: &mut World, i: usize) -> (u32, std::collections::BTreeMap<String, u32>) {
    let mut ready: Vec<(Entity, f32)> = world
        .query::<(Entity, &Building, &House)>()
        .iter(world)
        .filter(|(_, b, h)| b.island == i && h.ready && h.tier.next().is_some())
        .map(|(e, _, h)| (e, h.content_s))
        .collect();
    ready.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut moved = 0;
    let mut waiting: std::collections::BTreeMap<String, u32> = Default::default();
    for (e, _) in ready {
        let Some(h) = world.get::<House>(e).cloned() else {
            continue;
        };
        let free = world.get::<Boost>(e).is_some_and(|x| x.rise);
        let mut econ = world.resource_mut::<Economy>();
        let books = &mut econ.islands[i];
        // (The guard and the building reserve, as for houses that move up on their own.)
        let why = sim::up_wait(&h, books, free, false);
        if why == Wait::None {
            let mut h2 = h;
            sim::move_up(&mut h2, books, free);
            if let Some(mut h) = world.get_mut::<House>(e) {
                *h = h2;
            }
            moved += 1;
        } else {
            let key = match serde_json::to_value(why) {
                Ok(Value::String(s)) => s,
                Ok(v) => v.to_string(),
                Err(_) => String::new(),
            };
            *waiting.entry(key).or_default() += 1;
        }
    }
    (moved, waiting)
}

/// The player moves a house down a tier to get its workers back.
pub fn move_down(world: &mut World, e: Entity) -> Result<(), String> {
    let moved = world
        .get_mut::<House>(e)
        .map(|mut h| sim::move_down(&mut h))
        .unwrap_or(false);
    if !moved {
        return Err("Fisherfolk are the first tier".into());
    }
    Ok(())
}

/// What the money alerts have said, so they do not repeat every frame.
#[derive(Resource, Default)]
pub struct MoneyAlerts {
    /// Debt seconds when the last debt alert went out (0: none in this spell of debt).
    pub said_at: f64,
    pub auto_paused: u32,
}

/// The three drains as one line: "3 weavers idle on Little Tarn: 9 a minute; ...".
pub fn drains_text(econ: &Economy) -> String {
    econ.drains
        .iter()
        .map(drain_line)
        .collect::<Vec<_>>()
        .join("; ")
}

/// "bakery" -> "bakeries", "windmill" -> "windmills".
fn plural(name: &str, n: u32) -> String {
    let name = low(name);
    match name.strip_suffix('y') {
        // (Names that end in s already, such as oil works or brickworks, stay as they are.)
        _ if n == 1 || name.ends_with('s') => name,
        Some(stem) if !stem.ends_with(['a', 'e', 'o', 'u']) => format!("{stem}ies"),
        _ => format!("{name}s"),
    }
}

fn drain_line(d: &sim::Drain) -> String {
    format!(
        "{} {} {} on {}: {:.0} a minute",
        d.count,
        plural(d.kind.def().name, d.count),
        if d.idle { "idle" } else { "running" },
        islands()[d.island].name,
        d.per_min
    )
}

/// Tells the player when the company is in debt and what costs the most, every five game
/// minutes while it lasts, and when works pause themselves to save coins.
pub fn alerts(world: &mut World) {
    world.init_resource::<MoneyAlerts>();
    let econ = world.resource::<Economy>();
    let (debt, auto) = (econ.debt_s, econ.auto_paused);
    let text = drains_text(econ);
    let said = world.resource::<MoneyAlerts>().said_at;
    let mut toasts = Vec::new();
    if debt <= 0.0 {
        world.resource_mut::<MoneyAlerts>().said_at = 0.0;
    } else if said == 0.0 || debt - said >= 300.0 {
        world.resource_mut::<MoneyAlerts>().said_at = debt.max(1e-3);
        let soon = if debt < sim::DEBT_PAUSE_S {
            " Idle works pause themselves after two minutes."
        } else {
            ""
        };
        toasts.push((
            engine_ui::toast::ToastKind::Warn,
            "In debt",
            format!("Biggest costs: {text}.{soon}"),
        ));
    }
    let before = world.resource::<MoneyAlerts>().auto_paused;
    if auto > before {
        world.resource_mut::<MoneyAlerts>().auto_paused = auto;
        toasts.push((
            engine_ui::toast::ToastKind::Info,
            "Works paused",
            format!(
                "{} idle works paused themselves to save coins. Resume them from their cards.",
                auto - before
            ),
        ));
    }
    for (kind, title, text) in toasts {
        engine_ui::toast::notify(world, kind, "", title, &text, 8.0);
    }
}

pub fn register(app: &mut App) {
    app.register_api(
        "isle.house",
        "One house, every number added up: each need (asked, supply, points, people, coins, joy, locked until), each category (points of threshold), rent, goodwill, happiness and what pulls it, ready or why it waits.",
        r#"{"id": 12}"#,
        |world, args| {
            let e = house_arg(world, &args)?;
            house(world, e)
        },
    )
    .register_api(
        "isle.island.economy",
        "Per tier on an island: people, houses, happiness by mood, ready houses, waiting reasons counted, the next need to unlock; whether houses move up on their own, and paused needs.",
        r#"{"island": 0}"#,
        |world, args| {
            let i = island_arg(&args)?;
            Ok(island(world, i))
        },
    )
    .register_api(
        "isle.ledger",
        "Income by tier (rent, goodwill, each need), costs by building kind and state (running, idle, paused), and the three biggest drains.",
        "{}",
        |world, _| Ok(ledger(world)),
    )
    .register_api(
        "isle.work",
        "Per island and tier: people, jobs, jobs idle works let go, share filled, works short of hands.",
        r#"{"island": 0}"#,
        |world, args| {
            let only = if args["island"].is_null() { None } else { Some(island_arg(&args)?) };
            Ok(work(world, only))
        },
    )
    .register_api(
        "isle.need.pause",
        "Stop (or with paused: false, resume) handing out a good on an island: nobody uses it and it gives nothing.",
        r#"{"island": 0, "good": "ale", "paused": true}"#,
        |world, args| {
            let i = island_arg(&args)?;
            let g = args["good"].as_str().and_then(Good::from_name).ok_or("unknown good")?;
            pause_need(world, i, g, args["paused"].as_bool().unwrap_or(true));
            Ok(json!({}))
        },
    )
    .register_api(
        "isle.island.auto_up",
        "Let ready houses on an island move up on their own (off by default), keeping the people the works of the tier they leave ask for.",
        r#"{"island": 0, "on": true}"#,
        |world, args| {
            let i = island_arg(&args)?;
            auto_up(world, i, args["on"].as_bool().unwrap_or(true));
            Ok(json!({}))
        },
    )
    .register_api(
        "isle.island.up_all",
        "Move every ready house on an island up, longest ready first, while the tier each leaves keeps the people its works ask for. Returns how many moved and why the rest wait.",
        r#"{"island": 0}"#,
        |world, args| {
            let i = island_arg(&args)?;
            let (moved, waiting) = move_up_all(world, i);
            Ok(json!({"moved": moved, "waiting": waiting}))
        },
    )
    .register_api(
        "isle.house.up",
        "Move one ready house up a tier now and pay the materials. It may take workers the tier below needs: a warning says so.",
        r#"{"id": 12}"#,
        |world, args| {
            let e = house_arg(world, &args)?;
            let warning = move_up(world, e)?;
            Ok(json!({"warning": warning}))
        },
    )
    .register_api(
        "isle.house.down",
        "Move a house down a tier: people beyond the smaller house move out, nothing is refunded.",
        r#"{"id": 12}"#,
        |world, args| {
            let e = house_arg(world, &args)?;
            move_down(world, e)?;
            Ok(json!({}))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::town::tests::bare_world;

    /// Target 9: every number on a house card adds up to what the house pays.
    #[test]
    fn a_house_card_adds_up() {
        let mut w = bare_world();
        crate::town::build_start(&mut w);
        crate::town::prewarm(&mut w, 300.0);
        let ids: Vec<u32> = w
            .query::<(&Building, &House)>()
            .iter(&w)
            .map(|(b, _)| b.id)
            .collect();
        assert!(ids.len() > 10);
        for id in ids {
            let e = build::by_id(&mut w, id).unwrap();
            let v = house(&mut w, e).unwrap();
            let needs: f64 = v["needs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|n| n["coins"].as_f64().unwrap())
                .sum();
            let sum = needs
                + v["rent"].as_f64().unwrap()
                + v["goodwill"].as_f64().unwrap()
                + v["items"].as_f64().unwrap();
            let income = v["income"].as_f64().unwrap();
            assert!(
                (sum - income).abs() < 1e-3,
                "house {id}: parts {sum} against {income}: {v:#}"
            );
            let pulls: f64 = v["pulls"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p["points"].as_f64().unwrap())
                .sum();
            let target = v["target"].as_f64().unwrap();
            assert!(
                (pulls.clamp(0.0, 100.0) - target).abs() < 1e-3,
                "house {id}: pulls {pulls} against {target}"
            );
            assert!(!v["wait_text"].as_str().unwrap().is_empty());
        }
    }

    /// Writes a few house cards and the island book as JSON, for the picture page.
    #[test]
    #[ignore]
    fn dump_cards() {
        let mut w = bare_world();
        crate::town::build_start(&mut w);
        crate::town::prewarm(&mut w, 600.0);
        let mut picked: Vec<Entity> = Vec::new();
        for t in Tier::ALL {
            let mut hs: Vec<(Entity, f32, bool)> = w
                .query::<(Entity, &House)>()
                .iter(&w)
                .filter(|(_, h)| h.tier == t)
                .map(|(e, h)| (e, h.happiness, h.ready))
                .collect();
            hs.sort_by(|a, b| a.1.total_cmp(&b.1));
            if let Some(x) = hs.last() {
                picked.push(x.0);
            }
            if let Some(x) = hs.iter().find(|x| !x.2) {
                picked.push(x.0);
            }
        }
        let cards: Vec<Value> = picked
            .into_iter()
            .map(|e| house(&mut w, e).unwrap())
            .collect();
        let out = json!({"houses": cards, "island": island(&mut w, 0), "ledger": ledger(&mut w)});
        let path = std::env::var("CARDS_OUT").unwrap_or("cards.json".into());
        std::fs::write(path, serde_json::to_string_pretty(&out).unwrap()).unwrap();
    }

    /// Moving a house down and up again through the calls.
    #[test]
    fn the_player_moves_houses_and_turns_auto_up_on() {
        let mut w = bare_world();
        crate::town::build_start(&mut w);
        crate::town::prewarm(&mut w, 120.0);
        let (e, tier) = w
            .query::<(Entity, &House)>()
            .iter(&w)
            .find(|(_, h)| h.tier == Tier::Labourers)
            .map(|(e, h)| (e, h.tier))
            .expect("a Labourers' house in the start");
        move_down(&mut w, e).unwrap();
        let h = w.get::<House>(e).unwrap();
        assert_eq!(Some(h.tier), tier.prev());
        assert!(h.residents <= Tier::Fisherfolk.capacity());
        assert!(move_down(&mut w, e).is_err());
        // Not ready yet: the reason comes back.
        let err = move_up(&mut w, e).unwrap_err();
        assert!(err.starts_with("Not ready"), "{err}");
        auto_up(&mut w, 0, true);
        assert!(w.resource::<Economy>().islands[0].auto_up);
        let v = island(&mut w, 0);
        assert_eq!(v["auto_up"], json!(true));
    }

    /// Houses never move up unless the player says so; "move up all" keeps the workers of
    /// the tier below.
    #[test]
    fn houses_move_up_only_when_the_player_says_and_keep_the_workers() {
        let mut w = bare_world();
        crate::town::build_start(&mut w);
        crate::town::prewarm(&mut w, 60.0);
        let tiers = |w: &mut World| {
            let mut n = [0u32; Tier::COUNT];
            for h in w.query::<&House>().iter(w) {
                n[h.tier.index()] += 1;
            }
            n
        };
        let before = tiers(&mut w);
        crate::town::prewarm(&mut w, 600.0);
        assert_eq!(tiers(&mut w), before, "houses moved up on their own");
        let mut moved = 0;
        for i in 0..w.resource::<Economy>().islands.len() {
            let before = w.resource::<Economy>().islands[i].population;
            moved += move_up_all(&mut w, i).0;
            let b = &w.resource::<Economy>().islands[i];
            for (t, was) in before.iter().enumerate() {
                let jobs = b.jobs[t] + b.let_go[t];
                // A tier only gives up people it can spare.
                assert!(
                    b.population[t] >= was - 1e-3 || b.population[t] >= jobs - 1e-3,
                    "island {i} tier {t}: {} people left for {jobs} jobs",
                    b.population[t]
                );
            }
        }
        assert!(moved > 0, "nothing was ready to move up");
        assert!(tiers(&mut w) != before);
    }
}
