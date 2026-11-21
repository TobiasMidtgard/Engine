//! Stamps (his note 24): a piece of a town kept to lay again. O takes up the copy tool; a
//! drag round buildings and roads keeps them as a stamp in the stamp book and puts it in the
//! hand. Shift+O takes up the last stamp again, [ and ] go through the book, R turns the
//! stamp, a click lays it: the roads first, then each building that fits, built where the
//! company can pay for it and laid as a blueprint where it can't (all of it as blueprints in
//! blueprint mode). Shift+Delete tears the stamp in the hand out of the book.
//!
//! The book is kept between games in `stamps.json` (in `%APPDATA%\Brightsund` on Windows,
//! `~/.local/share/brightsund` elsewhere; `BRIGHTSUND_STAMPS` names another file). Scripted
//! runs keep theirs in memory only.

use std::path::PathBuf;

use engine::prelude::*;
use serde::{Deserialize, Serialize};

use crate::blueprint;
use crate::build;
use crate::defs::Kind;
use crate::grid::{self, Grid, Occ};
use crate::sim::Building;

/// The most stamps the book keeps (the oldest goes first).
pub const BOOK_SIZE: usize = 40;
/// The largest piece a stamp may hold, cells a side.
pub const MAX_SIDE: i32 = 64;

/// A kept piece of a town, from its corner.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stamp {
    pub name: String,
    /// Each building: its kind, origin and turn.
    pub buildings: Vec<(Kind, [i32; 2], u8)>,
    /// Each road cell, and whether it is paved.
    pub roads: Vec<([i32; 2], bool)>,
}

/// The stamp book.
#[derive(Resource, Default)]
pub struct Book {
    pub stamps: Vec<Stamp>,
    /// Where it is kept (`None`: in memory only).
    path: Option<PathBuf>,
}

/// Where the book is kept on this machine.
fn book_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("BRIGHTSUND_STAMPS") {
        return Some(PathBuf::from(p));
    }
    if cfg!(windows) {
        let base = std::env::var_os("APPDATA")?;
        return Some(PathBuf::from(base).join("Brightsund").join("stamps.json"));
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("brightsund").join("stamps.json"))
}

/// Open the book: from its file in a real game, empty in a scripted one.
pub fn setup(world: &mut World) {
    let scripted = world.resource::<EngineConfig>().headless
        || std::env::args().any(|a| a == "--tour" || a == "--play");
    let path = if scripted && std::env::var_os("BRIGHTSUND_STAMPS").is_none() {
        None
    } else {
        book_path()
    };
    let stamps = path
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<Vec<Stamp>>(&s).ok())
        .unwrap_or_default();
    world.insert_resource(Book { stamps, path });
}

fn save(book: &Book) {
    let Some(path) = &book.path else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match serde_json::to_string_pretty(&book.stamps) {
        Ok(s) => {
            if let Err(e) = std::fs::write(path, s) {
                engine::core::log::warn!("could not keep the stamp book in {path:?}: {e}");
            }
        }
        Err(e) => engine::core::log::warn!("could not write the stamp book: {e}"),
    }
}

/// A name for a stamp from what it holds: "6 houses, a market square and 2 more" (a stamp
/// of roads alone: "40 road cells").
fn name_of(buildings: &[(Kind, [i32; 2], u8)], roads: usize) -> String {
    let mut counts: Vec<(Kind, usize)> = Vec::new();
    for (k, _, _) in buildings {
        match counts.iter_mut().find(|c| c.0 == *k) {
            Some(c) => c.1 += 1,
            None => counts.push((*k, 1)),
        }
    }
    counts.sort_by_key(|c| std::cmp::Reverse(c.1));
    let mut parts: Vec<String> = counts
        .iter()
        .take(2)
        .map(|(k, n)| {
            let name = k.def().name.to_lowercase();
            if *n == 1 {
                let an = name.starts_with(['a', 'e', 'i', 'o', 'u']);
                format!("{} {name}", if an { "an" } else { "a" })
            } else {
                format!("{n} {name}s")
            }
        })
        .collect();
    let more: usize = counts.iter().skip(2).map(|c| c.1).sum();
    if more > 0 {
        parts.push(format!("{more} more"));
    }
    let name = match parts.len() {
        0 if roads > 0 => format!("{roads} road cells"),
        0 => "an empty stamp".into(),
        1 => parts[0].clone(),
        _ => {
            let last = parts.pop().unwrap_or_default();
            format!("{} and {last}", parts.join(", "))
        }
    };
    capitalise(&name)
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

/// The company's buildings and the roads inside the rectangle from `a` to `b` (corners
/// included) as a stamp: buildings wholly inside, roads by cell.
pub fn copy(world: &mut World, a: IVec2, b: IVec2) -> Option<Stamp> {
    let lo = a.min(b);
    let hi = a.max(b).min(lo + IVec2::splat(MAX_SIDE - 1));
    let inside = |c: IVec2| c.x >= lo.x && c.y >= lo.y && c.x <= hi.x && c.y <= hi.y;
    let mut buildings: Vec<(Kind, [i32; 2], u8)> = world
        .query::<(Entity, &Building)>()
        .iter(world)
        .filter(|(e, b)| {
            build::ours(world, *e)
                && grid::footprint(b.kind, b.origin, b.rot)
                    .iter()
                    .all(|c| inside(*c))
        })
        .map(|(_, b)| {
            let o = b.origin - lo;
            (b.kind, [o.x, o.y], b.rot)
        })
        .collect();
    buildings.sort_by_key(|b| (b.1[1], b.1[0]));
    let g = world.resource::<Grid>();
    let mut roads: Vec<([i32; 2], bool)> = g
        .roads
        .iter()
        .filter(|c| inside(**c) && matches!(g.cells.get(c), Some(Occ::Road)))
        .map(|c| {
            let o = *c - lo;
            ([o.x, o.y], g.is_paved(*c))
        })
        .collect();
    roads.sort();
    if buildings.is_empty() && roads.is_empty() {
        return None;
    }
    Some(Stamp {
        name: name_of(&buildings, roads.len()),
        buildings,
        roads,
    })
}

/// Keep `s` in the book (the oldest goes when it is full): its place in the book.
pub fn keep(world: &mut World, s: Stamp) -> usize {
    let mut book = world.resource_mut::<Book>();
    book.stamps.push(s);
    while book.stamps.len() > BOOK_SIZE {
        book.stamps.remove(0);
    }
    save(&book);
    book.stamps.len() - 1
}

/// Tear stamp `i` out of the book.
pub fn tear_out(world: &mut World, i: usize) {
    let mut book = world.resource_mut::<Book>();
    if i < book.stamps.len() {
        book.stamps.remove(i);
        save(&book);
    }
}

/// A building a stamp lays: its kind, origin and turn.
pub type Member = (Kind, IVec2, u8);

/// A cell of a stamp turned `rot` quarter turns, the way buildings turn (the front of turn
/// 0 to the front of turn 1).
fn turn(c: IVec2, rot: u8) -> IVec2 {
    (0..rot % 4).fold(c, |c, _| IVec2::new(c.y, -c.x))
}

/// What stamp `s` turned `rot` lays with its middle on cell `at`: each building (kind,
/// origin, turn) and each road cell.
pub fn members(s: &Stamp, rot: u8, at: IVec2) -> (Vec<Member>, Vec<(IVec2, bool)>) {
    let mut bs: Vec<Member> = s
        .buildings
        .iter()
        .map(|(k, o, r)| {
            let cells = grid::footprint(*k, IVec2::new(o[0], o[1]), *r);
            let origin = cells
                .iter()
                .map(|c| turn(*c, rot))
                .reduce(|a, b| a.min(b))
                .unwrap_or_default();
            (*k, origin, (r + rot) % 4)
        })
        .collect();
    let mut rs: Vec<(IVec2, bool)> = s
        .roads
        .iter()
        .map(|(c, p)| (turn(IVec2::new(c[0], c[1]), rot), *p))
        .collect();
    // Its middle on the cursor.
    let all = bs
        .iter()
        .flat_map(|(k, o, r)| grid::footprint(*k, *o, *r))
        .chain(rs.iter().map(|r| r.0));
    let (lo, hi) = all.fold((IVec2::MAX, IVec2::MIN), |(lo, hi), c| {
        (lo.min(c), hi.max(c))
    });
    if lo.x > hi.x {
        return (bs, rs);
    }
    let shift = at - (lo + hi) / 2;
    for b in &mut bs {
        b.1 += shift;
    }
    for r in &mut rs {
        r.0 += shift;
    }
    (bs, rs)
}

/// What laying a stamp did.
#[derive(Debug, Default, PartialEq)]
pub struct Laid {
    pub built: usize,
    pub planned: usize,
    pub blocked: usize,
    pub roads: usize,
    /// Why the first building laid as a blueprint is one.
    pub why: Option<String>,
}

/// Whether a stamp's building fits as a plan or a building would (the stamp's own roads
/// lie beside its buildings, never under them).
pub fn fits(world: &World, kind: Kind, origin: IVec2, rot: u8) -> Result<usize, String> {
    blueprint::check(world, kind, origin, rot)
}

/// Lay stamp `s` turned `rot` with its middle on `at`.
pub fn lay(world: &mut World, s: &Stamp, rot: u8, at: IVec2) -> Laid {
    let (bs, rs) = members(s, rot, at);
    let plan_all = blueprint::mode(world);
    let mut out = Laid::default();
    // The roads first, so the buildings find them.
    let mut planned_roads: Vec<(IVec2, bool)> = Vec::new();
    for (c, paved) in rs {
        let (open, done) = {
            let g = world.resource::<Grid>();
            (
                g.is_open(c) && grid::cell_roadable(c) || g.is_road(c),
                if paved { g.is_paved(c) } else { g.is_road(c) },
            )
        };
        if done || !open || blueprint::print_at(world, c).is_some() {
            continue;
        }
        let at = grid::cell_center(c);
        if !crate::island::island_at(at.x, at.y).is_some_and(|i| crate::settle::claimed(world, i)) {
            continue;
        }
        let laid = !plan_all
            && if paved {
                build::place_paving(world, c, true).is_ok()
            } else {
                build::place_road(world, c, true).is_ok()
            };
        if laid {
            out.roads += 1;
        } else {
            planned_roads.push((c, paved));
        }
    }
    for paved in [false, true] {
        let cells: Vec<IVec2> = planned_roads
            .iter()
            .filter(|r| r.1 == paved)
            .map(|r| r.0)
            .collect();
        blueprint::lay_road(world, &cells, paved);
    }
    for (kind, origin, rot) in bs {
        let isle = match fits(world, kind, origin, rot) {
            Ok(i) => i,
            Err(_) => {
                out.blocked += 1;
                continue;
            }
        };
        let pay = if plan_all {
            Err("Blueprint mode is on.".to_string())
        } else {
            crate::blueprint::ready(world, kind, isle)
        };
        let built = match pay {
            Ok(()) => match build::place(world, kind, origin, rot, true) {
                Ok(_) => true,
                Err(e) => {
                    out.why.get_or_insert(e);
                    false
                }
            },
            Err(e) => {
                out.why.get_or_insert(e);
                false
            }
        };
        if built {
            out.built += 1;
        } else if blueprint::lay(world, kind, origin, rot).is_ok() {
            out.planned += 1;
        } else {
            out.blocked += 1;
        }
    }
    out
}

/// (For the tour: the company's town round `centre` kept as a stamp, `side` cells a side.)
pub(crate) fn tour_copy(world: &mut World, centre: IVec2, side: i32) -> Option<usize> {
    let h = IVec2::splat(side / 2);
    let s = copy(world, centre - h, centre + h)?;
    Some(keep(world, s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stamp_turns_like_its_buildings_and_lays_its_middle_on_the_cursor() {
        let s = Stamp {
            name: "t".into(),
            buildings: vec![(Kind::House, [0, 0], 0), (Kind::Market, [5, 0], 1)],
            roads: vec![([0, 3], false), ([1, 3], false), ([2, 3], true)],
        };
        let at = IVec2::new(100, 200);
        for rot in 0..4u8 {
            let (bs, rs) = members(&s, rot, at);
            // Each building turned with the stamp.
            assert_eq!(bs[0].2, rot % 4);
            assert_eq!(bs[1].2, (1 + rot) % 4);
            // Nothing overlaps after the turn.
            let mut cells: Vec<IVec2> = bs
                .iter()
                .flat_map(|(k, o, r)| grid::footprint(*k, *o, *r))
                .chain(rs.iter().map(|r| r.0))
                .collect();
            let n = cells.len();
            cells.sort_by_key(|c| (c.x, c.y));
            cells.dedup();
            assert_eq!(cells.len(), n, "turn {rot} overlaps");
            // Its middle near the cursor.
            let (lo, hi) = cells.iter().fold((IVec2::MAX, IVec2::MIN), |(lo, hi), c| {
                (lo.min(*c), hi.max(*c))
            });
            assert!(((lo + hi) / 2 - at).abs().max_element() <= 1);
            // The house's front still faces the road it faced (turn 0: +Z, towards z = 3).
            let front = grid::front_row(bs[0].0, bs[0].1, bs[0].2);
            assert!(
                front.iter().any(|c| rs.iter().any(|r| r.0 == *c)),
                "turn {rot}: the house no longer faces its road"
            );
        }
    }

    #[test]
    fn a_copied_block_lays_again_built_where_paid_and_planned_where_not() {
        let mut w = crate::town::tests::bare_world();
        crate::town::build_start(&mut w);
        w.init_resource::<blueprint::Blueprints>();
        w.insert_resource(Book::default());
        // The town's middle: the first market.
        let m = w
            .query::<&Building>()
            .iter(&w)
            .find(|b| b.kind == Kind::Market && b.island == 0)
            .map(|b| b.origin)
            .expect("a market in the start town");
        let s = copy(&mut w, m - IVec2::splat(8), m + IVec2::splat(8)).expect("a stamp");
        assert!(s.buildings.len() >= 3, "{s:?}");
        assert!(!s.roads.is_empty());
        let i = keep(&mut w, s.clone());
        assert_eq!(w.resource::<Book>().stamps[i], s);
        // Laid on open ground: with no money everything waits as blueprints.
        let home = crate::island::home_town_spot();
        let c0 = grid::cell_of(home.x, home.y);
        let me = crate::sim::acting(&w);
        *w.resource_mut::<crate::sim::Economy>().coins_mut(me) = -1.0;
        let spot = (0..120i32)
            .flat_map(|r| {
                [
                    IVec2::new(r, 0),
                    IVec2::new(-r, 0),
                    IVec2::new(0, r),
                    IVec2::new(0, -r),
                ]
            })
            .map(|o| c0 + o * 4)
            .find(|at| {
                let (bs, _) = members(&s, 0, *at);
                bs.iter().all(|(k, o, r)| fits(&w, *k, *o, *r).is_ok())
            })
            .expect("room for the stamp");
        let laid = lay(&mut w, &s, 0, spot);
        assert_eq!(laid.built, 0, "{laid:?}");
        assert_eq!(laid.planned, s.buildings.len(), "{laid:?}");
        assert!(laid.why.is_some());
        let bp = w.resource::<blueprint::Blueprints>();
        assert_eq!(bp.prints.len(), s.buildings.len());
        assert!(!bp.roads.is_empty(), "the roads wait too");
        // Paid for, Shift+N builds them all.
        *w.resource_mut::<crate::sim::Economy>().coins_mut(me) = 1.0e6;
        for isle in &mut w.resource_mut::<crate::sim::Economy>().islands {
            for g in crate::defs::Good::ALL {
                isle.add(g, 500.0);
            }
        }
        let r = blueprint::build_all(&mut w);
        // All but the institutions the company's renown allows only one of.
        let limited = s
            .buildings
            .iter()
            .filter(|b| crate::items::Host::of(b.0).is_some())
            .count();
        assert!(r.waiting <= limited, "{r:?}");
        assert!(r.built >= s.buildings.len() - limited, "{r:?}");
    }

    #[test]
    fn stamps_are_named_by_what_they_hold() {
        let b = vec![
            (Kind::House, [0, 0], 0),
            (Kind::House, [3, 0], 0),
            (Kind::Market, [6, 0], 0),
        ];
        assert_eq!(name_of(&b, 12), "2 houses and a market square");
        let more = [
            b.clone(),
            vec![(Kind::Chapel, [9, 0], 0), (Kind::Infirmary, [12, 0], 0)],
        ]
        .concat();
        assert_eq!(name_of(&more, 12), "2 houses, a market square and 2 more");
        assert_eq!(name_of(&b[2..], 0), "A market square");
        assert_eq!(name_of(&[], 3), "3 road cells");
    }
}
