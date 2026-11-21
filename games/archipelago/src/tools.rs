//! The hand on the map: what a click does (select, build, lay road, demolish), the ghost of
//! what is about to be built (with [`crate::placing`]'s grid, berths and reach), keys, and
//! what the bars' `isle.*` buttons do.
//!
//! Keys: R turns (Shift+R back), Z takes up the road (again: paving), X demolishes, C takes up
//! the building under the cursor (over open ground: the last one again), F4 to F9 open the
//! build bar's tabs (again: its next building, Shift: the one before), + and − change the
//! game's speed, Space pauses, Esc puts the tool down. Dragging with a building lays a row of
//! them; dragging a road finds its way round what stands in between. N switches blueprint
//! mode (Shift+N builds every blueprint the company can pay for, see [`crate::blueprint`]);
//! O takes up the copy tool for stamps (Shift+O the last stamp, see [`crate::stamps`]).

use std::collections::HashMap;

use engine::input::Mouse;
use engine::prelude::*;
use engine::render::Vertex;
use engine_ui::input::UiInputCapture;

use crate::art::Mat;
use crate::build;
use crate::camera;
use crate::defs::{CELL_M, Kind, Site, Tier};
use crate::draw::{Art, MapRow, Tiled};
use crate::grid::{self, Grid, Occ};
use crate::looks::{ModelKey, look};
use crate::sim::{Building, Economy, Producer};

/// What a left click does.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub enum Tool {
    #[default]
    Select,
    Build {
        kind: Kind,
        rot: u8,
    },
    Road,
    /// The road tool laying setts: over dirt, or a new paved road.
    Paving,
    Demolish,
    /// A drag round buildings and roads keeps them as a stamp.
    Copy,
    /// Stamp `index` of the book in the hand, turned `rot`.
    Stamp {
        index: usize,
        rot: u8,
    },
}

/// The player's hand between frames.
#[derive(Resource, Default)]
pub struct Hand {
    pub selected: Option<Entity>,
    /// Where a road drag began.
    road_from: Option<IVec2>,
    /// Where a drag with a building in the hand began (the cursor's cell): a row of them.
    row_from: Option<IVec2>,
    /// The right button's travel since it went down (a short one is a click: cancel).
    right_travel: f32,
    /// What the tool would do here, for the bar's hint line.
    pub hint: String,
    /// The last building or road taken up (C over open ground takes it up again).
    last: Option<Tool>,
    /// Where a drag with the copy tool began.
    copy_from: Option<IVec2>,
    /// The stamp last held (Shift+O takes it up again).
    last_stamp: Option<usize>,
}

/// The ghost's draw sets and the footprint tiles.
#[derive(Resource, Default)]
struct Ghost {
    sets: Tiled<(ModelKey, Mat)>,
    /// The same as blueprints' glass (blueprint mode, or not paid for yet).
    plans: Tiled<(ModelKey, Mat)>,
    /// The footprint tiles: allowed ones and blocked ones.
    marks: Tiled<bool>,
    tile: Option<MeshRenderer>,
}

/// Ready the hand.
pub fn setup(world: &mut World) {
    world.init_resource::<Hand>();
    world.init_resource::<Ghost>();
    crate::placing::warm_berths();
    crate::blueprint::setup(world);
    crate::stamps::setup(world);
}

fn set_tool(world: &mut World, tool: Tool) {
    *world.resource_mut::<Tool>() = tool;
    let mut hand = world.resource_mut::<Hand>();
    hand.road_from = None;
    hand.row_from = None;
    hand.copy_from = None;
    if matches!(tool, Tool::Build { .. } | Tool::Road | Tool::Paving) {
        hand.last = Some(tool);
    }
    if let Tool::Stamp { index, .. } = tool {
        hand.last_stamp = Some(index);
    }
    crate::hud::tool_changed(world);
}

/// Take up `kind` from the bar or a key (keeping the turn the hand had).
fn take_up(world: &mut World, kind: Kind) {
    let rot = match *world.resource::<Tool>() {
        Tool::Build { rot, .. } => rot,
        _ => 0,
    };
    set_tool(
        world,
        match kind {
            Kind::Road => Tool::Road,
            Kind::PavedRoad => Tool::Paving,
            _ => Tool::Build { kind, rot },
        },
    );
}

/// The build bar's tabs in the order shown (by tier, or by type), each with its buildings in
/// the order shown for the island in view: the keys follow what the bar shows.
fn tabs(world: &World) -> Vec<Vec<Kind>> {
    crate::hud::bar_tabs(world)
        .into_iter()
        .map(|(_, kinds)| kinds)
        .collect()
}

/// The key that opens the build bar's tab at position `i`: F4, F5 and so on.
pub fn tab_key(i: usize) -> String {
    format!("F{}", i + 4)
}

/// The keys that take up `kind`, as a label for its build tile: "Z" for the road, or its
/// tab's key with how many presses it takes ("F6 ×3": press F6 three times).
pub fn hotkey(world: &World, kind: Kind) -> Option<String> {
    match kind {
        Kind::Road => return Some("Z".into()),
        Kind::PavedRoad => return Some("Z ×2".into()),
        _ => {}
    }
    tabs(world)
        .iter()
        .enumerate()
        .take(TAB_KEYS.len())
        .find_map(|(i, list)| {
            let p = list.iter().position(|k| *k == kind)?;
            Some(if p == 0 {
                tab_key(i)
            } else {
                format!("{} ×{}", tab_key(i), p + 1)
            })
        })
}

/// The tab keys: F4 onward (F1 is the help page, F3 the performance panel).
const TAB_KEYS: [KeyCode; 6] = [
    KeyCode::F4,
    KeyCode::F5,
    KeyCode::F6,
    KeyCode::F7,
    KeyCode::F8,
    KeyCode::F9,
];

/// Game speeds the + and − keys step through.
const SPEEDS: [f32; 3] = [1.0, 2.0, 4.0];

/// A tab's key: its first building, or (pressed again) the next one; `back`: the one before.
fn tab_pressed(world: &mut World, i: usize, back: bool) {
    let Some(list) = tabs(world).into_iter().nth(i).filter(|l| !l.is_empty()) else {
        return;
    };
    let now = match *world.resource::<Tool>() {
        Tool::Build { kind, .. } => Some(kind),
        Tool::Road => Some(Kind::Road),
        Tool::Paving => Some(Kind::PavedRoad),
        _ => None,
    };
    let n = list.len();
    let next = match now.and_then(|k| list.iter().position(|x| *x == k)) {
        Some(p) if back => (p + n - 1) % n,
        Some(p) => (p + 1) % n,
        None => 0,
    };
    take_up(world, list[next]);
    crate::hud::open_tab(world, i);
}

/// C: take up the building (or road) under the cursor, turned as it stands; over open ground,
/// the last one taken up.
fn copy_under_cursor(world: &mut World) {
    let cell = camera::pick(world).map(|h| grid::cell_of(h.x, h.z));
    let tool = match cell.and_then(|c| building_at(world, c).map(|o| (c, o))) {
        Some((_, Occ::Building(e) | Occ::Field(e))) => {
            world.get::<Building>(e).map(|b| Tool::Build {
                kind: b.kind,
                rot: b.rot,
            })
        }
        Some((c, Occ::Road)) => Some(if world.resource::<Grid>().is_paved(c) {
            Tool::Paving
        } else {
            Tool::Road
        }),
        None => world.resource::<Hand>().last,
    };
    if let Some(t) = tool {
        set_tool(world, t);
    }
}

/// Put `tool` in the hand as if picked from the bar; a road drag begins `from` cells off the
/// cursor's (the tour's pictures of the hand).
/// (For the tour: `tool` in hand, a drag held from `from`.)
pub(crate) fn stage(world: &mut World, tool: Tool, from: Option<IVec2>) {
    set_tool(world, tool);
    world.resource_mut::<Hand>().road_from = from;
}

pub(crate) fn select(world: &mut World, e: Option<Entity>) {
    world.resource_mut::<Hand>().selected = e;
    crate::hud::show_card(world);
}

fn toast(world: &mut World, title: &str, text: &str) {
    crate::hud::complain(world, title, text);
}

/// Where `kind` turned `rot` goes for the cursor at `hit`: a quay on the free berth the
/// cursor is near (turned as the berth faces), anything else centred on the cursor's cell.
fn place_at(world: &World, kind: Kind, rot: u8, hit: DVec3) -> (IVec2, u8, bool) {
    if let Some(b) = crate::placing::snap(world, kind, DVec2::new(hit.x, hit.z)) {
        return (b.origin, b.rot, true);
    }
    let c = grid::cell_of(hit.x, hit.z);
    let (w, d) = kind.footprint(rot);
    (c - IVec2::new(w / 2, d / 2), rot, false)
}

/// Whether a drag with `kind` lays a row of them (not farms, which need room for fields,
/// nor quays, which go on berths).
fn rows_of(kind: Kind) -> bool {
    kind.def().site == Site::Land && !crate::fields::is_farm(kind)
}

/// Origins for a row of `kind` turned `rot` from the cell a drag began at to the cursor's:
/// side by side along the longer way of the drag (at most 40).
fn row(kind: Kind, rot: u8, from: IVec2, to: IVec2) -> Vec<IVec2> {
    let (w, d) = kind.footprint(rot);
    let o = from - IVec2::new(w / 2, d / 2);
    let span = to - from;
    let (step, n) = if span.x.abs() >= span.y.abs() {
        (
            IVec2::new(w * span.x.signum(), 0),
            (span.x.abs() + w / 2) / w,
        )
    } else {
        (
            IVec2::new(0, d * span.y.signum()),
            (span.y.abs() + d / 2) / d,
        )
    };
    (0..=n.min(39)).map(|k| o + step * k).collect()
}

fn building_at(world: &World, c: IVec2) -> Option<Occ> {
    world.resource::<Grid>().cells.get(&c).copied()
}

/// N: blueprint mode on or off.
fn toggle_blueprints(world: &mut World) {
    let on = {
        let mut bp = world.resource_mut::<crate::blueprint::Blueprints>();
        bp.on = !bp.on;
        bp.on
    };
    engine_ui::toast::notify(
        world,
        engine_ui::toast::ToastKind::Good,
        "",
        "Blueprints",
        if on {
            "Blueprint mode: what you lay now is a free plan, built when you order it \
             (a click on it, or Shift+N for all). N again to build for real."
        } else {
            "Blueprint mode off: what you lay now is built and paid for."
        },
        3.0,
    );
}

/// Shift+N: build every blueprint the company can pay for.
fn build_blueprints(world: &mut World) {
    let r = crate::blueprint::build_all(world);
    let text = match (r.built, r.waiting) {
        (0, 0) => "No blueprints are laid. N switches blueprint mode on.".to_string(),
        (b, 0) => format!("Built {b} from blueprints."),
        (b, w) => format!(
            "Built {b} from blueprints; {w} wait. {}",
            r.why.unwrap_or_default()
        ),
    };
    let kind = if r.built > 0 {
        engine_ui::toast::ToastKind::Good
    } else {
        engine_ui::toast::ToastKind::Bad
    };
    engine_ui::toast::notify(world, kind, "", "Blueprints", &text, 4.0);
}

/// Take up stamp `i` of the book (`None`: the last one held, or the newest).
fn take_up_stamp(world: &mut World, i: Option<usize>) {
    let n = world.resource::<crate::stamps::Book>().stamps.len();
    if n == 0 {
        toast(
            world,
            "Stamps",
            "The stamp book is empty: O takes up the copy tool, then drag round a piece of town.",
        );
        set_tool(world, Tool::Copy);
        return;
    }
    let i = i
        .or(world.resource::<Hand>().last_stamp)
        .unwrap_or(n - 1)
        .min(n - 1);
    let rot = match *world.resource::<Tool>() {
        Tool::Stamp { rot, .. } => rot,
        _ => 0,
    };
    set_tool(world, Tool::Stamp { index: i, rot });
}

/// Act on one of the bars' `isle.*` actions.
pub fn action(world: &mut World, a: &str) {
    if let Some(k) = a.strip_prefix("isle.build:").and_then(Kind::from_name) {
        take_up(world, k);
    } else if crate::helm::action(world, a) {
    } else if a == "isle.tool.paved" {
        // The road tool's other mode: dirt and paved in turn.
        let next = if *world.resource::<Tool>() == Tool::Paving {
            Tool::Road
        } else {
            Tool::Paving
        };
        set_tool(world, next);
    } else if a == "isle.tool:demolish" {
        set_tool(world, Tool::Demolish);
    } else if a == "isle.blueprint" {
        toggle_blueprints(world);
    } else if a == "isle.blueprint.build_all" {
        build_blueprints(world);
    } else if a == "isle.stamp.copy" {
        set_tool(world, Tool::Copy);
    } else if a == "isle.stamp.last" {
        take_up_stamp(world, None);
    } else if let Some(i) = a
        .strip_prefix("isle.stamp:")
        .and_then(|s| s.parse::<usize>().ok())
    {
        take_up_stamp(world, Some(i));
    } else if let Some(v) = a
        .strip_prefix("isle.speed:")
        .and_then(|s| s.parse::<f32>().ok())
    {
        world.resource_mut::<Economy>().speed = v;
    } else if a == "isle.deselect" {
        select(world, None);
    } else if a == "isle.sel.pause" {
        if let Some(e) = world.resource::<Hand>().selected
            && build::ours(world, e)
            && let Some(mut p) = world.get_mut::<Producer>(e)
        {
            p.paused = !p.paused;
        }
    } else if a == "isle.sel.demolish"
        && let Some(e) = world.resource::<Hand>().selected
        && build::ours(world, e)
    {
        build::demolish(world, e);
        select(world, None);
    } else if let Some((c, fit)) = a
        .strip_prefix("isle.yard.fitted:")
        .and_then(|r| r.split_once(':'))
        .and_then(|(c, f)| crate::vessels::Craft::from_name(c).map(|c| (c, f.to_string())))
    {
        // Order a craft with chosen fittings ("isle.yard.fitted:brig:swivels,hold").
        let Some(e) = world.resource::<Hand>().selected else {
            return;
        };
        let r = crate::fittings::parse_list(c, &fit)
            .and_then(|fit| crate::shipyard::order_fitted(world, e, c, fit));
        match r {
            Ok(()) => crate::hud::show_card(world),
            Err(msg) => toast(world, "Shipyard", &msg),
        }
    } else if let Some(c) = a
        .strip_prefix("isle.yard.order:")
        .and_then(crate::vessels::Craft::from_name)
    {
        // Order a craft at the selected shipyard or airship mast.
        let Some(e) = world.resource::<Hand>().selected else {
            return;
        };
        match crate::shipyard::order(world, e, c) {
            Ok(()) => crate::hud::show_card(world),
            Err(msg) => toast(world, "Shipyard", &msg),
        }
    } else if a == "isle.yard.cancel" {
        let Some(e) = world.resource::<Hand>().selected else {
            return;
        };
        match crate::shipyard::cancel(world, e) {
            Ok(_) => crate::hud::show_card(world),
            Err(msg) => toast(world, "Shipyard", &msg),
        }
    } else if a == "isle.house.up" || a == "isle.house.down" || a == "isle.island.up_all" {
        house_order(world, a);
    } else if a == "isle.ruin.rebuild" {
        // Rebuild the selected ruin for half its price.
        let Some(id) = world
            .resource::<Hand>()
            .selected
            .and_then(|e| world.get::<crate::sim::Building>(e).map(|b| b.id))
        else {
            return;
        };
        match crate::incidents::rebuild(world, id) {
            Ok(()) => crate::hud::show_card(world),
            Err(msg) => toast(world, "Rebuild", &msg),
        }
    } else if a == "isle.view.trade_lines" {
        crate::tradelines::set(world, None);
    } else if let Some(rest) = a.strip_prefix("isle.rival.") {
        match crate::rivals::action(world, rest) {
            Ok(said) => engine_ui::toast::notify(
                world,
                engine_ui::toast::ToastKind::Good,
                "",
                "Relations",
                &said,
                3.0,
            ),
            Err(msg) => toast(world, "Relations", &msg),
        }
    } else if let Some(rest) = a.strip_prefix("isle.route.") {
        if let Err(msg) = route_action(world, rest) {
            toast(world, "Trade route", &msg);
        }
    } else if let Some(rest) = a.strip_prefix("isle.item.") {
        if let Err(msg) = crate::items::action(world, rest) {
            toast(world, "Items", &msg);
        }
    } else if let Some(rest) = a.strip_prefix("isle.gear.")
        && let Err(msg) = crate::gear::action(world, rest)
    {
        toast(world, "Gear", &msg);
    }
}

/// Moving houses up or down a tier, the player's order (see `reports`): the selected house,
/// or every ready house on the selected building's island (the home island when nothing is
/// selected).
fn house_order(world: &mut World, a: &str) {
    let sel = world.resource::<Hand>().selected;
    let house = sel.filter(|e| world.get::<crate::sim::House>(*e).is_some());
    let result = match (a, house) {
        ("isle.house.up", Some(e)) => crate::reports::move_up(world, e)
            .map(|w| w.unwrap_or_else(|| "The house moved up a tier.".into())),
        ("isle.house.down", Some(e)) => crate::reports::move_down(world, e)
            .map(|()| "The house moved down a tier; people beyond its room moved out.".into()),
        ("isle.island.up_all", _) => {
            let i = sel
                .and_then(|e| world.get::<crate::sim::Building>(e).map(|b| b.island))
                .unwrap_or(0);
            let (moved, waiting) = crate::reports::move_up_all(world, i);
            let held: u32 = waiting.values().sum();
            Ok(match (moved, held) {
                (0, 0) => "No house is ready to move up.".into(),
                (m, 0) => format!("{m} houses moved up a tier."),
                (m, h) => format!(
                    "{m} houses moved up a tier; {h} ready ones wait so the works keep their hands."
                ),
            })
        }
        _ => Err("Select a house first.".into()),
    };
    match result {
        Ok(msg) => {
            crate::hud::show_card(world);
            toast(world, "Moving up", &msg);
        }
        Err(msg) => toast(world, "Moving up", &msg),
    }
}

/// `isle.route.<verb>:<args>` from the routes screen's buttons; see `orders`.
///
/// - `open:sea` / `open:air`, `close:<r>`
/// - `stop.add:<r>:<building id>`, `stop.remove:<r>:<i>`, `stop.move:<r>:<from>:<to>`
/// - `stop.load:<r>:<i>:<good>` / `stop.unload:<r>:<i>:<good>` (toggle that good)
/// - `assign:<r>:<vessel entity bits>`, `unassign:<vessel entity bits>`
fn route_action(world: &mut World, a: &str) -> Result<(), String> {
    use crate::orders;
    let (verb, args) = a.split_once(':').unwrap_or((a, ""));
    let parts: Vec<&str> = args.split(':').collect();
    let num = |k: usize| -> Result<usize, String> {
        parts
            .get(k)
            .and_then(|s| s.parse().ok())
            .ok_or(format!("isle.route.{verb}: argument {k} wants a number"))
    };
    let vessel = |world: &mut World, k: usize| -> Result<Entity, String> {
        parts
            .get(k)
            .and_then(|s| orders::find_vessel(world, s))
            .ok_or("no such vessel".to_string())
    };
    match verb {
        "open" => {
            orders::open_route(world, "", parts[0] == "air");
            Ok(())
        }
        "close" => orders::close_route(world, num(0)?),
        "stop.add" => {
            orders::add_stop(world, num(0)?, num(1)? as u32, None, vec![], vec![]).map(|_| ())
        }
        "stop.remove" => orders::remove_stop(world, num(0)?, num(1)?),
        "stop.move" => orders::move_stop(world, num(0)?, num(1)?, num(2)?),
        "stop.load" | "stop.unload" => {
            let (r, i) = (num(0)?, num(1)?);
            let g = parts
                .get(2)
                .and_then(|s| crate::defs::Good::from_name(s))
                .ok_or("unknown good")?;
            let stop = world
                .resource::<crate::fleet::Routes>()
                .0
                .get(r)
                .and_then(|route| route.stops.get(i))
                .cloned()
                .ok_or(format!("route {r} has no stop {i}"))?;
            let (mut load, mut unload) = (stop.load, stop.unload);
            let list = if verb == "stop.load" {
                &mut load
            } else {
                &mut unload
            };
            if let Some(k) = list.iter().position(|x| *x == g) {
                list.remove(k);
            } else {
                list.push(g);
            }
            orders::set_goods(world, r, i, load, unload)
        }
        "assign" => {
            let r = num(0)?;
            let e = vessel(world, 1)?;
            orders::assign(world, e, r)
        }
        "unassign" => {
            let e = vessel(world, 0)?;
            orders::unassign(world, e)
        }
        _ => Err(format!("unknown action isle.route.{verb}")),
    }
}

/// Clicks and keys on the map.
pub fn input(world: &mut World) {
    if !world.contains_resource::<Hand>() {
        return;
    }
    let capture = world
        .get_resource::<UiInputCapture>()
        .cloned()
        .unwrap_or_default();
    let keys = world.resource::<ButtonInput<KeyCode>>().clone();
    let buttons = world.resource::<ButtonInput<MouseButton>>().clone();
    let delta = world.resource::<Mouse>().delta;
    let tool = *world.resource::<Tool>();
    if !capture.keyboard {
        let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        if keys.just_pressed(KeyCode::R) {
            let turn = |rot: u8| if shift { (rot + 3) % 4 } else { (rot + 1) % 4 };
            match tool {
                Tool::Build { kind, rot } => set_tool(
                    world,
                    Tool::Build {
                        kind,
                        rot: turn(rot),
                    },
                ),
                Tool::Stamp { index, rot } => set_tool(
                    world,
                    Tool::Stamp {
                        index,
                        rot: turn(rot),
                    },
                ),
                _ => {}
            }
        }
        // N: blueprint mode; Shift+N: build the blueprints.
        if keys.just_pressed(KeyCode::N) {
            if shift {
                build_blueprints(world);
            } else {
                toggle_blueprints(world);
            }
        }
        // O: the copy tool (again: put it down); Shift+O: the last stamp.
        if keys.just_pressed(KeyCode::O) {
            if shift {
                take_up_stamp(world, None);
            } else if tool == Tool::Copy {
                set_tool(world, Tool::Select);
            } else {
                set_tool(world, Tool::Copy);
            }
        }
        // [ and ]: the stamp book's pages.
        if let Tool::Stamp { index, .. } = tool {
            let n = world.resource::<crate::stamps::Book>().stamps.len().max(1);
            if keys.just_pressed(KeyCode::BracketLeft) {
                take_up_stamp(world, Some((index + n - 1) % n));
            }
            if keys.just_pressed(KeyCode::BracketRight) {
                take_up_stamp(world, Some((index + 1) % n));
            }
        }
        if keys.just_pressed(KeyCode::Escape) {
            if tool != Tool::Select {
                set_tool(world, Tool::Select);
            } else {
                select(world, None);
            }
        }
        if keys.just_pressed(KeyCode::Delete) {
            if let (true, Tool::Stamp { index, .. }) = (shift, tool) {
                // Shift+Delete with a stamp in the hand: tear it out of the book.
                crate::stamps::tear_out(world, index);
                toast(world, "Stamps", "Torn out of the stamp book.");
                set_tool(world, Tool::Select);
            } else {
                set_tool(world, Tool::Demolish);
            }
        }
        if keys.just_pressed(KeyCode::X) {
            let next = if tool == Tool::Demolish {
                Tool::Select
            } else {
                Tool::Demolish
            };
            set_tool(world, next);
        }
        if keys.just_pressed(KeyCode::Z) {
            set_tool(
                world,
                if tool == Tool::Road {
                    Tool::Paving
                } else {
                    Tool::Road
                },
            );
        }
        if keys.just_pressed(KeyCode::C) {
            copy_under_cursor(world);
        }
        for (i, k) in TAB_KEYS.into_iter().enumerate() {
            if keys.just_pressed(k) {
                tab_pressed(world, i, shift);
            }
        }
        // U: move the selected house up a tier; Shift+U: every ready house on its island.
        if keys.just_pressed(KeyCode::U) {
            action(
                world,
                if shift {
                    "isle.island.up_all"
                } else {
                    "isle.house.up"
                },
            );
        }
        let faster = keys.just_pressed(KeyCode::Equal) || keys.just_pressed(KeyCode::NumpadAdd);
        let slower =
            keys.just_pressed(KeyCode::Minus) || keys.just_pressed(KeyCode::NumpadSubtract);
        if faster || slower {
            let mut e = world.resource_mut::<Economy>();
            let now = e.speed;
            e.speed = if faster {
                SPEEDS.into_iter().find(|s| *s > now).unwrap_or(SPEEDS[2])
            } else {
                SPEEDS
                    .into_iter()
                    .rev()
                    .find(|s| *s < now)
                    .unwrap_or(SPEEDS[0])
            };
        }
        if keys.just_pressed(KeyCode::Space) {
            let mut e = world.resource_mut::<Economy>();
            e.speed = if e.speed > 0.0 { 0.0 } else { 1.0 };
        }
    }
    // A short right click cancels the tool; a drag pans (camera.rs).
    if buttons.just_pressed(MouseButton::Right) {
        world.resource_mut::<Hand>().right_travel = 0.0;
    }
    if buttons.pressed(MouseButton::Right) {
        world.resource_mut::<Hand>().right_travel += delta.length();
    }
    if buttons.just_released(MouseButton::Right)
        && world.resource::<Hand>().right_travel < 6.0
        && !capture.pointer
        && !crate::helm::right_click(world)
    {
        set_tool(world, Tool::Select);
    }
    if capture.pointer {
        return;
    }
    let tool = *world.resource::<Tool>();
    match tool {
        Tool::Select => {
            if crate::helm::input(world, &buttons) {
                return;
            }
            if buttons.just_pressed(MouseButton::Left) {
                let hit = camera::pick(world).map(|h| grid::cell_of(h.x, h.z));
                // A blueprint under the cursor: build it.
                if let Some(c) = hit
                    && building_at(world, c).is_none()
                    && build_blueprint_at(world, c)
                {
                    return;
                }
                let e = hit.and_then(|c| match building_at(world, c) {
                    Some(Occ::Building(e) | Occ::Field(e)) => Some(e),
                    _ => None,
                });
                select(world, e);
            }
        }
        Tool::Build { kind, .. } => {
            let hit = camera::pick(world);
            if buttons.just_pressed(MouseButton::Left) {
                world.resource_mut::<Hand>().row_from = hit.map(|h| grid::cell_of(h.x, h.z));
            }
            if buttons.just_released(MouseButton::Left)
                && world.resource::<Hand>().row_from.is_some()
            {
                let p = preview(world, tool, hit);
                world.resource_mut::<Hand>().row_from = None;
                let fits: Vec<(IVec2, u8)> = p
                    .buildings
                    .iter()
                    .filter(|b| b.3)
                    .map(|b| (b.1, b.2))
                    .collect();
                if fits.is_empty() {
                    toast(world, kind.def().name, &p.hint);
                }
                let plan_all = crate::blueprint::mode(world);
                let mut planned = None;
                for (origin, rot) in fits {
                    let pay = if plan_all {
                        Err(String::new())
                    } else {
                        grid::check(world.resource::<Grid>(), kind, origin, rot)
                            .map_err(|b| b.message().to_string())
                            .and_then(|isle| crate::blueprint::ready(world, kind, isle))
                    };
                    // What can't be paid for (or everything, in blueprint mode) is planned.
                    if let Err(why) = pay {
                        match crate::blueprint::lay(world, kind, origin, rot) {
                            Ok(()) => {
                                planned.get_or_insert(why);
                            }
                            Err(msg) => {
                                toast(world, kind.def().name, &msg);
                                break;
                            }
                        }
                        continue;
                    }
                    match build::place(world, kind, origin, rot, true) {
                        Ok(_) => crate::blueprint::forget(world, kind, origin, rot),
                        Err(msg) => {
                            toast(world, kind.def().name, &msg);
                            break;
                        }
                    }
                }
                if let Some(why) = planned.filter(|w| !w.is_empty()) {
                    engine_ui::toast::notify(
                        world,
                        engine_ui::toast::ToastKind::Good,
                        "",
                        "Laid as a blueprint",
                        &format!("{why} It waits as a blueprint: Shift+N builds it once paid for."),
                        4.0,
                    );
                }
            }
        }
        tool @ (Tool::Road | Tool::Paving) => {
            let hit = camera::pick(world);
            if buttons.just_pressed(MouseButton::Left) {
                world.resource_mut::<Hand>().road_from = hit.map(|h| grid::cell_of(h.x, h.z));
            }
            if buttons.just_released(MouseButton::Left)
                && world.resource::<Hand>().road_from.is_some()
            {
                let p = preview(world, tool, hit);
                world.resource_mut::<Hand>().road_from = None;
                let paved = tool == Tool::Paving;
                if p.plan && p.cells.iter().all(|(_, ok)| *ok) {
                    let cells: Vec<IVec2> = p.cells.iter().map(|c| c.0).collect();
                    crate::blueprint::lay_road(world, &cells, paved);
                } else if let Err(msg) = lay_road(world, &p, paved) {
                    toast(world, "Road", &msg);
                }
            }
        }
        Tool::Copy => {
            let hit = camera::pick(world).map(|h| grid::cell_of(h.x, h.z));
            if buttons.just_pressed(MouseButton::Left) {
                world.resource_mut::<Hand>().copy_from = hit;
            }
            if buttons.just_released(MouseButton::Left)
                && let (Some(a), Some(b)) = (world.resource::<Hand>().copy_from, hit)
            {
                world.resource_mut::<Hand>().copy_from = None;
                match crate::stamps::copy(world, a, b) {
                    Some(stamp) => {
                        let name = stamp.name.clone();
                        let index = crate::stamps::keep(world, stamp);
                        set_tool(world, Tool::Stamp { index, rot: 0 });
                        engine_ui::toast::notify(
                            world,
                            engine_ui::toast::ToastKind::Good,
                            "",
                            "Stamp kept",
                            &format!(
                                "{name}. R turns it, a click lays it, [ and ] page through \
                                 the stamp book."
                            ),
                            4.0,
                        );
                    }
                    None => toast(
                        world,
                        "Stamps",
                        "Nothing of the company's stands there: drag round buildings and roads.",
                    ),
                }
            }
        }
        Tool::Stamp { index, rot } => {
            if buttons.just_pressed(MouseButton::Left)
                && let Some(c) = camera::pick(world).map(|h| grid::cell_of(h.x, h.z))
            {
                let Some(stamp) = world
                    .resource::<crate::stamps::Book>()
                    .stamps
                    .get(index)
                    .cloned()
                else {
                    set_tool(world, Tool::Select);
                    return;
                };
                let r = crate::stamps::lay(world, &stamp, rot, c);
                let mut text = format!("{} built", r.built);
                if r.planned > 0 {
                    text += &format!(", {} laid as blueprints", r.planned);
                }
                if r.blocked > 0 {
                    text += &format!(", {} left out (no room)", r.blocked);
                }
                text += &format!(", {} road cells.", r.roads);
                if let Some(why) = r.why.filter(|_| r.planned > 0) {
                    text += &format!(" {why} Shift+N builds the blueprints once paid for.");
                }
                let kind = if r.built + r.planned > 0 {
                    engine_ui::toast::ToastKind::Good
                } else {
                    engine_ui::toast::ToastKind::Bad
                };
                engine_ui::toast::notify(world, kind, "", &stamp.name, &text, 4.0);
            }
        }
        Tool::Demolish => {
            if buttons.just_pressed(MouseButton::Left)
                && let Some(c) = camera::pick(world).map(|h| grid::cell_of(h.x, h.z))
            {
                // A blueprint goes first: clearing a plan costs nothing.
                if crate::blueprint::clear_at(world, c) {
                    return;
                }
                match building_at(world, c) {
                    Some(Occ::Building(e)) if build::ours(world, e) => {
                        if world.resource::<Hand>().selected == Some(e) {
                            select(world, None);
                        }
                        build::demolish(world, e);
                    }
                    Some(Occ::Road)
                        if crate::island::island_at(
                            grid::cell_center(c).x,
                            grid::cell_center(c).y,
                        )
                        .is_none_or(|i| {
                            world
                                .resource::<crate::sim::Economy>()
                                .owner(i)
                                .is_none_or(|o| o == crate::sim::acting(world))
                        }) =>
                    {
                        build::remove_road(world, c)
                    }
                    _ => {}
                }
            }
        }
    }
}

/// A click on a blueprint with no tool in hand: build it (a planned road: all of it joined to
/// that cell). Whether there was one.
fn build_blueprint_at(world: &mut World, c: IVec2) -> bool {
    if let Some(p) = crate::blueprint::print_at(world, c) {
        if let Err(msg) = crate::blueprint::build_print(world, p) {
            toast(world, p.kind.def().name, &msg);
        }
        return true;
    }
    if crate::blueprint::road_at(world, c) {
        if let Err(msg) = crate::blueprint::build_road_at(world, c) {
            toast(world, "Road", &msg);
        }
        return true;
    }
    false
}

/// Lay the road a drag's preview found, cell by cell (stopping at the first that can't be
/// paid for or built).
fn lay_road(world: &mut World, p: &Preview, paved: bool) -> Result<(), String> {
    if !p.cells.iter().all(|(_, ok)| *ok) {
        return Err(p.hint.clone());
    }
    for &(c, _) in &p.cells {
        let at = grid::cell_center(c);
        if let Some(i) = crate::island::island_at(at.x, at.y)
            && !crate::settle::claimed(world, i)
        {
            return Err(crate::settle::unclaimed_text_in(world, i));
        }
        if paved {
            build::place_paving(world, c, true)?;
        } else {
            build::place_road(world, c, true)?;
        }
    }
    Ok(())
}

fn ghost_material(world: &mut World) -> Handle<StandardMaterial> {
    world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: [1.0, 1.0, 1.0, 0.42],
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            double_sided: true,
            ..Default::default()
        })
}

fn tile_mesh() -> Mesh {
    let n = Vec3::Y;
    let v = |x: f32, z: f32| Vertex {
        position: [x, 0.0, z],
        normal: n.into(),
        uv: [x + 0.5, z + 0.5],
        color: [1.0; 4],
    };
    Mesh::new(
        vec![v(-0.5, -0.5), v(0.5, -0.5), v(0.5, 0.5), v(-0.5, 0.5)],
        vec![0, 2, 1, 0, 3, 2],
    )
}

/// What a tool would do where the cursor is.
#[derive(Default)]
struct Preview {
    /// Cells it would touch, each with whether it can be built on.
    cells: Vec<(IVec2, bool)>,
    /// Buildings that would stand: kind, origin, turn, and whether it fits.
    buildings: Vec<(Kind, IVec2, u8, bool)>,
    /// A farm's plot: each cell and whether a field will lie there.
    fields: Vec<(IVec2, bool)>,
    /// What a click does, or why it can't.
    hint: String,
    /// It would be laid as a blueprint (blueprint mode, or not paid for yet).
    plan: bool,
}

/// Whether `kind` may go on island `isle` now: the company must hold it, except for a first
/// harbour on an island nobody holds (settlers sail there).
fn may_build_on(world: &World, kind: Kind, isle: usize) -> Result<(), String> {
    if crate::settle::claimed(world, isle)
        || (kind == Kind::Harbour && world.resource::<Economy>().owner(isle).is_none())
    {
        Ok(())
    } else {
        Err(crate::settle::unclaimed_text_in(world, isle))
    }
}

fn preview(world: &mut World, tool: Tool, hit: Option<DVec3>) -> Preview {
    let mut p = Preview::default();
    let Some(hit) = hit else {
        return p;
    };
    let cursor = grid::cell_of(hit.x, hit.z);
    match tool {
        Tool::Select => {
            if let Some(b) = crate::blueprint::print_at(world, cursor) {
                p.hint = format!(
                    "Blueprint of a {}: click to build it ({}), X then click to clear it.",
                    b.kind.def().name.to_lowercase(),
                    crate::blueprint::cost_text(b.kind)
                );
            } else if crate::blueprint::road_at(world, cursor) {
                p.hint = "Planned road: click to build it, X then click to clear a cell.".into();
            }
        }
        Tool::Build { kind, rot } => {
            let (origin, rot, berth) = place_at(world, kind, rot, hit);
            let origins = match world.resource::<Hand>().row_from {
                Some(from) if rows_of(kind) => row(kind, rot, from, cursor),
                _ => vec![origin],
            };
            let def = kind.def();
            let plan_all = crate::blueprint::mode(world);
            let mut first_err = None;
            let mut first_isle = None;
            for o in &origins {
                let fit = if plan_all {
                    crate::blueprint::check(world, kind, *o, rot)
                } else {
                    grid::check(world.resource::<Grid>(), kind, *o, rot)
                        .map_err(|b| b.message().to_string())
                        .and_then(|isle| may_build_on(world, kind, isle).map(|_| isle))
                        .and_then(|isle| {
                            crate::blueprint::clear_for(world, kind, *o, rot).map(|_| isle)
                        })
                };
                let ok = fit.is_ok();
                match fit {
                    Ok(isle) => {
                        first_isle.get_or_insert(isle);
                    }
                    Err(e) => {
                        first_err.get_or_insert(e);
                    }
                }
                p.cells
                    .extend(grid::footprint(kind, *o, rot).into_iter().map(|c| (c, ok)));
                p.buildings.push((kind, *o, rot, ok));
            }
            let n = origins.len();
            let fit = p.buildings.iter().filter(|b| b.3).count();
            // Not paid for yet: a click lays it as a blueprint (said before the click).
            let short = match first_isle {
                Some(isle) if !plan_all => {
                    let purse = world
                        .resource::<Economy>()
                        .coins_of(crate::sim::acting(world));
                    crate::blueprint::ready(world, kind, isle)
                        .err()
                        .or_else(|| {
                            (purse < (fit as u32 * def.coins) as f64).then(|| {
                                format!(
                                    "{} × {fit} cost {} coins; you have {purse:.0}.",
                                    def.name,
                                    fit as u32 * def.coins
                                )
                            })
                        })
                }
                _ => None,
            };
            p.plan = plan_all || short.is_some();
            if crate::fields::is_farm(kind) {
                let c = grid::footprint_center(kind, origin, rot);
                if let Some(isle) = crate::island::island_at(c.x, c.y) {
                    p.fields =
                        crate::fields::preview(world.resource::<Grid>(), kind, origin, rot, isle);
                }
            }
            p.hint = if n > 1 {
                let blocked = if fit < n {
                    format!(" ({} blocked)", n - fit)
                } else {
                    String::new()
                };
                format!(
                    "{} × {fit}{blocked}: release to build them, {} coins in all.",
                    def.name,
                    fit as u32 * def.coins
                )
            } else if let Some(e) = first_err {
                if def.site == Site::Coast && !berth {
                    format!("{e} Amber frames mark the berths where it fits.")
                } else {
                    e
                }
            } else if !p.fields.is_empty() {
                let sown = p.fields.iter().filter(|f| f.1).count();
                let share = crate::fields::share_of(kind, sown);
                format!(
                    "{}: {sown} fields fit, {:.0} % of full output. Click to build, R to turn.",
                    def.name,
                    share * 100.0
                )
            } else {
                // The building's key, so the hand teaches it.
                let name = match hotkey(world, kind) {
                    Some(k) => format!("{} [{k}]", def.name),
                    None => def.name.to_string(),
                };
                if berth {
                    format!("{name}: click to build at this berth.")
                } else if rows_of(kind) {
                    format!(
                        "{name}: click to build, drag for a row, R to turn, right click to stop."
                    )
                } else {
                    format!("{name}: click to build, R to turn, right click to stop.")
                }
            };
            if fit > 0 {
                if plan_all {
                    p.hint = format!(
                        "Blueprint mode · {}",
                        p.hint
                            .replace("click to build", "click to lay a free blueprint")
                            .replace("release to build them", "release to plan them")
                            .replace(" coins in all", " coins when built")
                            .replace(", right click to stop", "")
                    ) + " N: back to building.";
                } else if let Some(why) = short {
                    p.hint = format!(
                        "{why} A click lays it as a blueprint instead, built once paid for \
                         (Shift+N)."
                    );
                }
            }
        }
        tool @ (Tool::Road | Tool::Paving) => {
            let b = cursor;
            let a = world.resource::<Hand>().road_from.unwrap_or(b);
            let kept = crate::blueprint::kept(world);
            let plan_all = crate::blueprint::mode(world);
            let purse = world
                .resource::<Economy>()
                .coins_of(crate::sim::acting(world));
            let g = world.resource::<Grid>();
            let (cells, ok) = match crate::placing::road_path_around(g, &kept, a, b) {
                Some(path) => (path, true),
                None => (l_line(a, b), false),
            };
            p.cells = cells
                .iter()
                .map(|c| (*c, ok && (g.is_road(*c) || g.is_open(*c))))
                .collect();
            let new = cells
                .iter()
                .filter(|c| {
                    if tool == Tool::Paving {
                        !g.is_paved(**c)
                    } else {
                        !g.is_road(**c)
                    }
                })
                .count();
            let price = new as f64
                * if tool == Tool::Paving {
                    Kind::PavedRoad.def().coins
                } else {
                    Kind::Road.def().coins
                } as f64;
            p.plan = ok && (plan_all || price > purse);
            p.hint = if !ok {
                "No way through for a road there: something stands in the way, or the ground \
                 is too steep or wet."
                    .into()
            } else if plan_all {
                format!(
                    "Blueprint mode · planned road: drag to plan {new} cells, free until built \
                     ({price:.0} coins). N: back to building."
                )
            } else if price > purse {
                format!(
                    "{new} road cells cost {price:.0} coins; you have {purse:.0}. Release to \
                     plan the road as a blueprint, built once paid for (Shift+N)."
                )
            } else if tool == Tool::Paving {
                let n = cells.iter().filter(|c| !g.is_paved(**c)).count();
                format!(
                    "Paved road: drag to pave {n} cells for {} coins and {:.0} bricks.",
                    n * Kind::PavedRoad.def().coins as usize,
                    (n as f64 * crate::defs::PAVING_BRICKS).ceil()
                )
            } else {
                let n = cells.iter().filter(|c| !g.is_road(**c)).count();
                format!(
                    "Dirt road: drag to lay {n} cells for {} coins; Z again for paving.",
                    n * Kind::Road.def().coins as usize
                )
            };
        }
        Tool::Copy => {
            let a = world.resource::<Hand>().copy_from;
            match a {
                None => {
                    p.cells = vec![(cursor, true)];
                    p.hint = "Copy: drag round a piece of the town to keep it as a stamp. O puts \
                              the tool down."
                        .into();
                }
                Some(a) => {
                    let lo = a.min(cursor);
                    let hi = a
                        .max(cursor)
                        .min(lo + IVec2::splat(crate::stamps::MAX_SIDE - 1));
                    for x in lo.x..=hi.x {
                        for z in lo.y..=hi.y {
                            if x == lo.x || x == hi.x || z == lo.y || z == hi.y {
                                p.cells.push((IVec2::new(x, z), true));
                            }
                        }
                    }
                    let (b, r) = crate::stamps::copy(world, a, cursor)
                        .map_or((0, 0), |s| (s.buildings.len(), s.roads.len()));
                    p.hint = format!(
                        "Copy: release to keep {b} buildings and {r} road cells as a stamp."
                    );
                }
            }
        }
        Tool::Stamp { index, rot } => {
            let book = world.resource::<crate::stamps::Book>();
            let n = book.stamps.len();
            let Some(stamp) = book.stamps.get(index).cloned() else {
                p.hint = "The stamp book is empty: O takes up the copy tool.".into();
                return p;
            };
            let (bs, rs) = crate::stamps::members(&stamp, rot, cursor);
            let mut coins = 0;
            for (kind, o, r) in &bs {
                let ok = crate::stamps::fits(world, *kind, *o, *r).is_ok();
                if ok {
                    coins += kind.def().coins;
                }
                p.cells
                    .extend(grid::footprint(*kind, *o, *r).into_iter().map(|c| (c, ok)));
                p.buildings.push((*kind, *o, *r, ok));
            }
            {
                let g = world.resource::<Grid>();
                for (c, _) in &rs {
                    p.cells.push((
                        *c,
                        g.is_road(*c) || g.is_open(*c) && grid::cell_roadable(*c),
                    ));
                }
            }
            let fit = p.buildings.iter().filter(|b| b.3).count();
            let purse = world
                .resource::<Economy>()
                .coins_of(crate::sim::acting(world));
            p.plan = crate::blueprint::mode(world) || coins as f64 > purse;
            let pay = if crate::blueprint::mode(world) {
                " Blueprint mode: all of it as blueprints.".to_string()
            } else if coins as f64 > purse {
                format!(" {coins} coins, you have {purse:.0}: the rest as blueprints.")
            } else {
                format!(" {coins} coins.")
            };
            p.hint = format!(
                "{} (stamp {} of {n}): {fit} of {} fit.{pay} Click lays it, R turns it, [ ] \
                 pages, Shift+Delete tears it out.",
                stamp.name,
                index + 1,
                bs.len()
            );
        }
        Tool::Demolish => {
            let cells = match building_at(world, cursor) {
                Some(Occ::Building(e)) => world
                    .get::<Building>(e)
                    .map(|b| grid::footprint(b.kind, b.origin, b.rot))
                    .unwrap_or_default(),
                Some(Occ::Road) => vec![cursor],
                Some(Occ::Field(_)) | None => vec![],
            };
            p.cells = cells.into_iter().map(|c| (c, false)).collect();
            p.hint = "Demolish: click a building or a road.".into();
            if let Some(b) = crate::blueprint::print_at(world, cursor) {
                p.cells = b.cells().into_iter().map(|c| (c, false)).collect();
                p.hint = format!(
                    "Click to clear the blueprint of a {}.",
                    b.kind.def().name.to_lowercase()
                );
            } else if crate::blueprint::road_at(world, cursor) {
                p.cells = vec![(cursor, false)];
                p.hint = "Click to clear this planned road cell.".into();
            }
        }
    }
    p
}

/// Cells along an L from `a` to `b` (x first), at most 400.
fn l_line(a: IVec2, b: IVec2) -> Vec<IVec2> {
    let mut cells = vec![a];
    let mut c = a;
    while c != b && cells.len() < 400 {
        if c.x != b.x {
            c.x += (b.x - c.x).signum();
        } else {
            c.y += (b.y - c.y).signum();
        }
        cells.push(c);
    }
    cells
}

/// Draw the ghost of what the tool would do.
pub fn ghost(world: &mut World) {
    if !world.contains_resource::<Ghost>() {
        return;
    }
    crate::blueprint::draw(world);
    let tool = *world.resource::<Tool>();
    let selected = world.resource::<Hand>().selected;
    // A selected Trading Union, Town Hall or Harbour Office shows its reach.
    let holder = selected.filter(|e| world.get::<crate::items::Institution>(*e).is_some());
    // So does a selected market, chapel, school, tavern, fire watch, watch house or infirmary.
    let serves = selected
        .and_then(|e| world.get::<Building>(e))
        .is_some_and(|b| crate::placing::has_reach(b.kind));
    if world.resource::<EngineConfig>().headless
        && tool == Tool::Select
        && holder.is_none()
        && !serves
        && world.resource::<Ghost>().marks.anchors.is_empty()
    {
        return;
    }
    let Some(art) = world.get_resource::<Art>().cloned() else {
        return;
    };
    if world.resource::<Ghost>().tile.is_none() {
        let mesh = world.resource_mut::<Assets<Mesh>>().add(tile_mesh());
        let mat = ghost_material(world);
        let mut r = MeshRenderer::new(mesh, mat);
        r.cast_shadows = false;
        world.resource_mut::<Ghost>().tile = Some(r);
    }
    let plans = world
        .get_resource::<crate::blueprint::Blueprints>()
        .is_some_and(|b| !b.prints.is_empty() || !b.roads.is_empty());
    let hit = if tool == Tool::Select && !plans {
        None
    } else {
        camera::pick(world)
    };
    let p = preview(world, tool, hit);
    let lift = crate::placing::lift(world);
    let mut tiles: Vec<MapRow> = Vec::new();
    let mut groups: HashMap<(ModelKey, Mat), Vec<MapRow>> = HashMap::new();
    let tile_at = |c: IVec2, tint: [f32; 3]| {
        let at = grid::cell_center(c);
        let y = grid::cell_range(c).1.max(0.0) + 0.12 + lift;
        MapRow {
            at: DVec3::new(at.x, y, at.y),
            yaw: 0.0,
            scale: CELL_M as f32 * 0.94,
            tint,
        }
    };
    for &(c, ok) in &p.cells {
        let tint = if tool == Tool::Demolish || !ok {
            [1.0, 0.3, 0.25]
        } else if p.plan {
            crate::blueprint::BLUE
        } else {
            [0.35, 1.0, 0.45]
        };
        tiles.push(tile_at(c, tint));
    }
    // A farm's plot: pale where fields will lie, red where they can't.
    for &(c, ok) in &p.fields {
        tiles.push(tile_at(
            c,
            if ok {
                [0.92, 1.0, 0.62]
            } else {
                [0.85, 0.32, 0.28]
            },
        ));
    }
    for &(kind, origin, rot, _) in &p.buildings {
        let c = grid::footprint_center(kind, origin, rot);
        let base = DVec3::new(c.x, grid::base_height(kind, origin, rot), c.y);
        let yaw = grid::yaw(rot);
        let q = Quat::from_rotation_y(yaw);
        let biome = crate::island::island_at(c.x, c.y)
            .map_or(crate::globe::Biome::Temperate, |i| {
                crate::island::islands()[i].biome()
            });
        for piece in look(kind, Tier::Fisherfolk, 7, biome) {
            let Some(model) = art.models.get(piece.model) else {
                continue;
            };
            for (_, mat) in model.iter() {
                groups.entry((piece.model, *mat)).or_default().push(MapRow {
                    at: base + (q * piece.pos).as_dvec3(),
                    yaw: yaw + piece.yaw,
                    scale: piece.scale,
                    tint: piece.tint,
                });
            }
        }
    }
    let p_building = p.buildings.first().map(|b| (b.0, b.1, b.2));
    // The reach of an item building: the one in the hand, or the one selected.
    let reach = match (tool, &p_building, holder) {
        (Tool::Build { kind, .. }, Some((_, origin, rot)), _)
            if crate::items::Host::of(kind).is_some() =>
        {
            let c = grid::footprint_center(kind, *origin, *rot);
            crate::island::island_at(c.x, c.y).map(|isle| {
                let r = kind.def().reach as f64 * CELL_M;
                let host = crate::items::Host::of(kind).unwrap();
                let inside: Vec<(Building, u32)> = world
                    .query::<&Building>()
                    .iter(world)
                    .filter(|b| {
                        b.island == isle
                            && host.acts_on(b.kind)
                            && (grid::footprint_center(b.kind, b.origin, b.rot) - c).length() <= r
                    })
                    .map(|b| (b.clone(), 0))
                    .collect();
                (host, c, r, inside)
            })
        }
        (Tool::Select, _, Some(e)) => world.get::<Building>(e).cloned().and_then(|b| {
            let host = crate::items::Host::of(b.kind)?;
            let inside = crate::items::in_reach(world, e)
                .into_iter()
                .map(|(_, b, n)| (b, n))
                .collect();
            Some((
                host,
                grid::footprint_center(b.kind, b.origin, b.rot),
                b.kind.def().reach as f64 * CELL_M,
                inside,
            ))
        }),
        _ => None,
    };
    let mut ring: Vec<MapRow> = Vec::new();
    if let Some((host, c, r, inside)) = reach {
        let col = crate::art::hexc(host.colour());
        let tint = [col.x, col.y, col.z];
        // A dashed ring on the ground.
        let n = (std::f64::consts::TAU * r / 4.4) as usize;
        for i in 0..n {
            let a = i as f64 / n as f64 * std::f64::consts::TAU;
            let at = c + DVec2::new(a.cos(), a.sin()) * r;
            let y = grid::cell_range(grid::cell_of(at.x, at.y)).1.max(0.0) + 0.16 + lift;
            ring.push(MapRow {
                at: DVec3::new(at.x, y, at.y),
                yaw: -(a as f32),
                scale: 2.3,
                tint,
            });
        }
        // A glow under everything it acts on, brighter where an item is at work.
        for (b, items) in inside {
            let bc = grid::footprint_center(b.kind, b.origin, b.rot);
            let (w, d) = b.kind.footprint(b.rot);
            let k = if items > 0 { 1.0 } else { 0.45 };
            ring.push(MapRow {
                at: DVec3::new(bc.x, b.base_y + 0.14, bc.y),
                yaw: 0.0,
                scale: w.max(d) as f32 * CELL_M as f32 + 3.0,
                tint: [tint[0] * k, tint[1] * k, tint[2] * k],
            });
        }
    }
    let mut ghost = std::mem::take(&mut *world.resource_mut::<Ghost>());
    let mut marks: HashMap<bool, Vec<MapRow>> = HashMap::new();
    marks.insert(true, tiles);
    marks.insert(false, ring);
    marks.retain(|_, rows| !rows.is_empty());
    let tile = ghost.tile.clone();
    ghost.marks.set(world, marks, |_| tile.clone());
    let (real, glass) = if p.plan {
        let tint = crate::blueprint::glass_tint();
        for rows in groups.values_mut() {
            for r in rows.iter_mut() {
                r.tint = tint;
            }
        }
        (HashMap::new(), groups)
    } else {
        (groups, HashMap::new())
    };
    ghost.sets.set(world, real, |key| {
        crate::draw::model_renderer(&art, key, false)
    });
    let glass_mat = crate::blueprint::glass(world);
    ghost.plans.set(world, glass, |key| {
        crate::blueprint::glass_renderer(&art, key, &glass_mat)
    });
    *world.resource_mut::<Ghost>() = ghost;
    world.resource_mut::<Hand>().hint = p.hint;
    crate::placing::draw(
        world,
        crate::placing::Held {
            tool,
            hit,
            building: p_building,
            selected: if tool == Tool::Select { selected } else { None },
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_lays_a_row_side_by_side() {
        let from = IVec2::new(10, 10);
        let r = row(Kind::House, 0, from, from + IVec2::new(7, 1));
        assert_eq!(r.len(), 3, "{r:?}");
        assert_eq!(r[1] - r[0], IVec2::new(3, 0));
        let r = row(Kind::Sawmill, 1, from, from + IVec2::new(0, -9));
        assert_eq!(r[1] - r[0], IVec2::new(0, -3));
        assert_eq!(row(Kind::House, 0, from, from).len(), 1);
    }

    #[test]
    fn every_building_on_the_bar_has_a_key() {
        let world = World::new();
        let shown: Vec<Kind> = tabs(&world).into_iter().flatten().collect();
        assert!(shown.len() > 20, "{shown:?}");
        for k in shown {
            assert!(hotkey(&world, k).is_some(), "{k:?}");
        }
        assert_eq!(hotkey(&world, Kind::Road).as_deref(), Some("Z"));
    }
}
