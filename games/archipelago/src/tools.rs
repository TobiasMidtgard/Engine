//! The hand on the map: what a click does (select, build, lay road, demolish), the ghost of
//! what is about to be built, keys, and what the bars' `isle.*` buttons do.

use std::collections::HashMap;

use engine::input::Mouse;
use engine::prelude::*;
use engine::render::Vertex;
use engine_ui::input::UiInputCapture;

use crate::art::Mat;
use crate::build;
use crate::camera;
use crate::defs::{CELL_M, Kind, Tier};
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
}

/// The player's hand between frames.
#[derive(Resource, Default)]
pub struct Hand {
    pub selected: Option<Entity>,
    /// Where a road drag began.
    road_from: Option<IVec2>,
    /// The right button's travel since it went down (a short one is a click: cancel).
    right_travel: f32,
    /// What the tool would do here, for the bar's hint line.
    pub hint: String,
}

/// The ghost's draw sets and the footprint tiles.
#[derive(Resource, Default)]
struct Ghost {
    sets: Tiled<(ModelKey, Mat)>,
    /// The footprint tiles: allowed ones and blocked ones.
    marks: Tiled<bool>,
    tile: Option<MeshRenderer>,
}

/// Ready the hand.
pub fn setup(world: &mut World) {
    world.init_resource::<Hand>();
    world.init_resource::<Ghost>();
}

fn set_tool(world: &mut World, tool: Tool) {
    *world.resource_mut::<Tool>() = tool;
    world.resource_mut::<Hand>().road_from = None;
    crate::hud::tool_changed(world);
}

pub(crate) fn select(world: &mut World, e: Option<Entity>) {
    world.resource_mut::<Hand>().selected = e;
    crate::hud::show_card(world);
}

fn toast(world: &mut World, title: &str, text: &str) {
    crate::hud::complain(world, title, text);
}

/// Where the cursor's cell is and the origin a `kind` turned `rot` would have centred on it.
fn origin_under(world: &mut World, kind: Kind, rot: u8) -> Option<(IVec2, IVec2)> {
    let hit = camera::pick(world)?;
    let c = grid::cell_of(hit.x, hit.z);
    let (w, d) = kind.footprint(rot);
    Some((c, c - IVec2::new(w / 2, d / 2)))
}

fn building_at(world: &World, c: IVec2) -> Option<Occ> {
    world.resource::<Grid>().cells.get(&c).copied()
}

/// Act on one of the bars' `isle.*` actions.
pub fn action(world: &mut World, a: &str) {
    if let Some(k) = a.strip_prefix("isle.build:").and_then(Kind::from_name) {
        let rot = match *world.resource::<Tool>() {
            Tool::Build { rot, .. } => rot,
            _ => 0,
        };
        set_tool(
            world,
            match k {
                Kind::Road => Tool::Road,
                Kind::PavedRoad => Tool::Paving,
                _ => Tool::Build { kind: k, rot },
            },
        );
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
    } else if let Some(v) = a
        .strip_prefix("isle.speed:")
        .and_then(|s| s.parse::<f32>().ok())
    {
        world.resource_mut::<Economy>().speed = v;
    } else if a == "isle.deselect" {
        select(world, None);
    } else if a == "isle.sel.pause" {
        if let Some(e) = world.resource::<Hand>().selected
            && let Some(mut p) = world.get_mut::<Producer>(e)
        {
            p.paused = !p.paused;
        }
    } else if a == "isle.sel.demolish"
        && let Some(e) = world.resource::<Hand>().selected
    {
        build::demolish(world, e);
        select(world, None);
    } else if let Some(rest) = a.strip_prefix("isle.route.") {
        if let Err(msg) = route_action(world, rest) {
            toast(world, "Trade route", &msg);
        }
    } else if let Some(rest) = a.strip_prefix("isle.item.")
        && let Err(msg) = crate::items::action(world, rest)
    {
        toast(world, "Items", &msg);
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
        if keys.just_pressed(KeyCode::R)
            && let Tool::Build { kind, rot } = tool
        {
            set_tool(
                world,
                Tool::Build {
                    kind,
                    rot: (rot + 1) % 4,
                },
            );
        }
        if keys.just_pressed(KeyCode::Escape) {
            if tool != Tool::Select {
                set_tool(world, Tool::Select);
            } else {
                select(world, None);
            }
        }
        if keys.just_pressed(KeyCode::Delete) {
            set_tool(world, Tool::Demolish);
        }
        for (k, v) in [
            (KeyCode::Digit1, 1.0),
            (KeyCode::Digit2, 2.0),
            (KeyCode::Digit3, 4.0),
        ] {
            if keys.just_pressed(k) {
                world.resource_mut::<Economy>().speed = v;
            }
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
    {
        set_tool(world, Tool::Select);
    }
    if capture.pointer {
        return;
    }
    let tool = *world.resource::<Tool>();
    match tool {
        Tool::Select => {
            if buttons.just_pressed(MouseButton::Left) {
                let hit = camera::pick(world).map(|h| grid::cell_of(h.x, h.z));
                let e = hit.and_then(|c| match building_at(world, c) {
                    Some(Occ::Building(e) | Occ::Field(e)) => Some(e),
                    _ => None,
                });
                select(world, e);
            }
        }
        Tool::Build { kind, rot } => {
            if buttons.just_pressed(MouseButton::Left)
                && let Some((_, origin)) = origin_under(world, kind, rot)
            {
                match build::place(world, kind, origin, rot, true) {
                    Ok(_) => {}
                    Err(msg) => toast(world, kind.def().name, &msg),
                }
            }
        }
        tool @ (Tool::Road | Tool::Paving) => {
            let cell = camera::pick(world).map(|h| grid::cell_of(h.x, h.z));
            if buttons.just_pressed(MouseButton::Left) {
                world.resource_mut::<Hand>().road_from = cell;
            }
            if buttons.just_released(MouseButton::Left)
                && let (Some(a), Some(b)) = (world.resource_mut::<Hand>().road_from.take(), cell)
                && let Err(msg) = build::place_road_line(world, a, b, tool == Tool::Paving, true)
            {
                toast(world, "Road", &msg);
            }
        }
        Tool::Demolish => {
            if buttons.just_pressed(MouseButton::Left)
                && let Some(c) = camera::pick(world).map(|h| grid::cell_of(h.x, h.z))
            {
                match building_at(world, c) {
                    Some(Occ::Building(e)) => {
                        if world.resource::<Hand>().selected == Some(e) {
                            select(world, None);
                        }
                        build::demolish(world, e);
                    }
                    Some(Occ::Road) => build::remove_road(world, c),
                    Some(Occ::Field(_)) | None => {}
                }
            }
        }
    }
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

/// Cells the current tool would touch, and whether it can.
/// What a tool would do: the cells, whether it can, the building (kind, origin, turn) and a hint.
type Preview = (Vec<IVec2>, bool, Option<(Kind, IVec2, u8)>, String);

fn preview(world: &mut World, tool: Tool) -> Option<Preview> {
    match tool {
        Tool::Select => None,
        Tool::Build { kind, rot } => {
            let (_, origin) = origin_under(world, kind, rot)?;
            let ok = grid::check(world.resource::<Grid>(), kind, origin, rot);
            let msg = match ok {
                Ok(_) => format!(
                    "{}: click to build, R to turn, right click to stop.",
                    kind.def().name
                ),
                Err(b) => b.message().to_string(),
            };
            Some((
                grid::footprint(kind, origin, rot),
                ok.is_ok(),
                Some((kind, origin, rot)),
                msg,
            ))
        }
        tool @ (Tool::Road | Tool::Paving) => {
            let b = camera::pick(world).map(|h| grid::cell_of(h.x, h.z))?;
            let a = world.resource::<Hand>().road_from.unwrap_or(b);
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
            let g = world.resource::<Grid>();
            let ok = cells
                .iter()
                .all(|c| g.is_road(*c) || (g.is_open(*c) && grid::cell_roadable(*c)));
            let msg = if tool == Tool::Paving {
                let n = cells.iter().filter(|c| !g.is_paved(**c)).count();
                format!(
                    "Paved road: drag to pave {n} cells for {} coins and {:.0} bricks.",
                    n * Kind::PavedRoad.def().coins as usize,
                    (n as f64 * crate::defs::PAVING_BRICKS).ceil()
                )
            } else {
                let n = cells.iter().filter(|c| !g.is_road(**c)).count();
                format!(
                    "Dirt road: drag to lay {n} cells for {} coins.",
                    n * Kind::Road.def().coins as usize
                )
            };
            Some((cells, ok, None, msg))
        }
        Tool::Demolish => {
            let c = camera::pick(world).map(|h| grid::cell_of(h.x, h.z))?;
            let cells = match building_at(world, c) {
                Some(Occ::Building(e)) => world
                    .get::<Building>(e)
                    .map(|b| grid::footprint(b.kind, b.origin, b.rot))
                    .unwrap_or_default(),
                Some(Occ::Road) => vec![c],
                Some(Occ::Field(_)) | None => vec![],
            };
            Some((
                cells,
                false,
                None,
                "Demolish: click a building or a road.".into(),
            ))
        }
    }
}

/// Draw the ghost of what the tool would do.
pub fn ghost(world: &mut World) {
    if !world.contains_resource::<Ghost>() {
        return;
    }
    // A selected Trading Union, Town Hall or Harbour Office shows its reach.
    let holder = world
        .resource::<Hand>()
        .selected
        .filter(|e| world.get::<crate::items::Institution>(*e).is_some());
    if world.resource::<EngineConfig>().headless
        && world.resource::<Tool>() == &Tool::Select
        && holder.is_none()
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
    let tool = *world.resource::<Tool>();
    let p = preview(world, tool);
    let mut tiles: Vec<MapRow> = Vec::new();
    let mut groups: HashMap<(ModelKey, Mat), Vec<MapRow>> = HashMap::new();
    let mut hint = String::new();
    let mut ok_all = true;
    let p_building = p.as_ref().and_then(|x| x.2);
    if let Some((cells, ok, building, msg)) = p {
        ok_all = ok;
        hint = msg;
        let tint = if tool == Tool::Demolish {
            [1.0, 0.25, 0.2]
        } else if ok {
            [0.35, 1.0, 0.45]
        } else {
            [1.0, 0.3, 0.25]
        };
        for c in &cells {
            let p = grid::cell_center(*c);
            let y = grid::cell_range(*c).1.max(0.0) + 0.12;
            tiles.push(MapRow {
                at: DVec3::new(p.x, y, p.y),
                yaw: 0.0,
                scale: CELL_M as f32 * 0.94,
                tint,
            });
        }
        if let Some((kind, origin, rot)) = building {
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
    }
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
            let y = grid::cell_range(grid::cell_of(at.x, at.y)).1.max(0.0) + 0.16;
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
    // Allowed tiles and blocked ones are two sets (key: allowed).
    let good = ok_all && tool != Tool::Demolish;
    let mut marks: HashMap<bool, Vec<MapRow>> = HashMap::new();
    if !tiles.is_empty() {
        marks.insert(good, tiles);
    }
    marks.entry(true).or_default().extend(ring);
    marks.retain(|_, rows| !rows.is_empty());
    let tile = ghost.tile.clone();
    ghost.marks.set(world, marks, |_| tile.clone());
    ghost.sets.set(world, groups, |key| {
        crate::draw::model_renderer(&art, key, false)
    });
    *world.resource_mut::<Ghost>() = ghost;
    world.resource_mut::<Hand>().hint = hint;
}
