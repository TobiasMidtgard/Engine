//! The player's screen: the top bar (coins, balance, people, the warehouse of the island in
//! view), the build bar by group, the selected building's card and the speed buttons; and the
//! hand on the map: the ghost of what is about to be built, roads dragged out cell by cell,
//! demolition and picking.

use std::collections::HashMap;
use std::sync::Arc;

use bevy_ecs::message::MessageCursor;
use engine::input::Mouse;
use engine::prelude::*;
use engine::render::{InstanceRow, MeshInstances, Vertex};
use engine_ui::data::UiData;
use engine_ui::el::El;
use engine_ui::input::{UiEvent, UiInputCapture};
use engine_ui::node::Screen;
use engine_ui::screens;
use engine_ui::style::{Pick, Position};

use crate::art::Mat;
use crate::build;
use crate::camera;
use crate::defs::{CELL_M, Category, DEFS, Good, Kind, Tier};
use crate::draw::Art;
use crate::grid::{self, Grid, Occ};
use crate::island;
use crate::looks::{ModelKey, look};
use crate::sim::{Building, Economy, House, Producer, population};

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
    Demolish,
}

/// HUD state between frames.
#[derive(Resource, Default)]
pub struct Hud {
    seen: MessageCursor<UiEvent>,
    category: usize,
    pub selected: Option<Entity>,
    /// Where a road drag began.
    road_from: Option<IVec2>,
    /// The right button's travel since it went down (a short one is a click: cancel).
    right_travel: f32,
    refresh: f32,
    /// The island the top bar shows.
    pub island: usize,
    pub hint: String,
    info_open: bool,
}

/// The ghost's draw sets and the footprint tiles.
#[derive(Resource, Default)]
struct Ghost {
    sets: HashMap<(ModelKey, Mat), Entity>,
    tiles: Option<(Entity, Entity)>,
}

const TOP: &str = "isle_top";
const BAR: &str = "isle_bar";
const INFO: &str = "isle_info";

fn kind_icon(k: Kind) -> &'static str {
    match k {
        Kind::Road => "isle_road",
        Kind::Harbour => "isle_anchor",
        Kind::House => "isle_house",
        Kind::Market => "coins",
        Kind::Chapel => "isle_chapel",
        Kind::AirshipMast => "isle_airship",
        _ => k.def().output.map_or("isle_workshop", Good::icon),
    }
}

fn kinds_in(c: Category) -> Vec<Kind> {
    DEFS.iter()
        .filter(|d| d.category == c)
        .map(|d| d.kind)
        .collect()
}

fn cost_text(k: Kind) -> String {
    let d = k.def();
    let mut parts = vec![format!("{} coins", d.coins)];
    for (g, n) in d.cost {
        parts.push(format!("{n} {}", g.name().to_lowercase()));
    }
    let mut s = parts.join(", ");
    if let Some((t, w)) = d.workers {
        s += &format!(". Needs {w} {}", t.name().to_lowercase());
    }
    match (d.input, d.output) {
        (Some(i), Some(o)) => {
            s += &format!(
                ". Turns {} into {}",
                i.name().to_lowercase(),
                o.name().to_lowercase()
            )
        }
        (None, Some(o)) => s += &format!(". Makes {}", o.name().to_lowercase()),
        _ => {}
    }
    if d.upkeep > 0.0 {
        s += &format!(". Upkeep {:.0}/min", d.upkeep);
    }
    format!("{}\n[color=#aab4c4]{s}.[/color]", d.blurb)
}

fn caption(text: &str) -> El {
    El::text(text)
        .class("caption")
        .text_style(|t| t.nowrap())
        .pick_ignore()
}

/// A good's icon and its warehouse count.
fn good_cell(g: Good) -> El {
    El::new()
        .class("row")
        .with_style(|s| s.gap(4.0).min_width(54.0))
        .tooltip(g.name(), "In the warehouse of the island in view: made and used per minute in the card on the right.")
        .child(El::icon(g.icon()).with_style(|s| s.size(24.0, 24.0).pick(Pick::Ignore)))
        .child(
            El::text(format!("{{isle.stock.{}}}", g.index()))
                .text_style(|t| t.semibold().nowrap().size(15.0))
                .pick_ignore(),
        )
}

fn top_bar() -> El {
    let stat = |icon: &str, bind: &str, tip: &str, tip_text: &str| {
        El::new()
            .class("row")
            .with_style(|s| s.gap(6.0))
            .tooltip(tip, tip_text)
            .child(El::icon(icon).with_style(|s| s.size(26.0, 26.0).pick(Pick::Ignore)))
            .child(
                El::text(bind)
                    .text_style(|t| t.bold().nowrap().size(17.0))
                    .pick_ignore(),
            )
    };
    let goods = El::new()
        .class("row")
        .with_style(|s| s.gap(6.0))
        .children(Good::ALL.into_iter().map(good_cell));
    let speed = El::new().class("row").with_style(|s| s.gap(4.0)).children(
        [("II", "0"), ("1×", "1"), ("2×", "2"), ("4×", "4")]
            .into_iter()
            .map(|(l, v)| {
                El::button(l, format!("isle.speed:{v}"))
                    .class("ghost")
                    .id(format!("isle.speed:{v}"))
                    .with_style(|s| s.padding([4.0, 10.0]).min_width(40.0))
            }),
    );
    let bar = El::new()
        .class("glass")
        .with_style(|s| {
            s.position(Position::Absolute)
                .left(16.0)
                .right(16.0)
                .top(12.0)
                .padding([8.0, 16.0])
                .gap(22.0)
                .align_items(engine_ui::style::Align::Center)
                .radius(14.0)
        })
        .child(stat(
            "coin",
            "{isle.coins}",
            "Coins",
            "Taxes from houses, minus every building's upkeep.",
        ))
        .child(
            El::text("{isle.balance}")
                .text_style(|t| t.semibold().nowrap().size(15.0))
                .tooltip("Balance", "Coins per minute after upkeep.")
                .id("isle.balance"),
        )
        .child(stat(
            "isle_people",
            "{isle.people}",
            "People",
            "Fisherfolk and labourers on every island.",
        ))
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(0.0))
                .child(
                    El::text("{isle.island}")
                        .text_style(|t| t.bold().nowrap().size(15.0))
                        .pick_ignore(),
                )
                .child(caption("{isle.island_people}")),
        )
        .child(goods)
        .child(El::new().with_style(|s| s.grow(1.0)))
        .child(caption("{isle.clock}"))
        .child(speed);
    El::new()
        .with_style(|s| s.fill_parent().pick(Pick::Ignore))
        .child(bar)
}

fn item_button(k: Kind, selected: bool) -> El {
    let d = k.def();
    El::new()
        .class("card")
        .id(format!("isle.build:{k:?}"))
        .on_click(format!(
            "isle.build:{}",
            serde_json::to_value(k)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default()
        ))
        .selected(selected)
        .tooltip(d.name, cost_text(k))
        .with_style(|s| {
            s.direction(engine_ui::style::Direction::Column)
                .align_items(engine_ui::style::Align::Center)
                .gap(4.0)
                .padding([8.0, 6.0])
                .width(84.0)
        })
        .child(El::icon(kind_icon(k)).with_style(|s| s.size(38.0, 38.0).pick(Pick::Ignore)))
        .child(
            El::text(d.name)
                .text_style(|t| t.semibold().size(12.0).center().nowrap())
                .pick_ignore(),
        )
}

fn bar_items(category: usize, tool: Tool) -> Vec<El> {
    let c = Category::ALL[category];
    let mut v: Vec<El> = kinds_in(c)
        .into_iter()
        .map(|k| {
            let sel = match tool {
                Tool::Build { kind, .. } => kind == k,
                Tool::Road => k == Kind::Road,
                _ => false,
            };
            item_button(k, sel)
        })
        .collect();
    if c == Category::Roads {
        v.push(
            El::new()
                .class("card")
                .on_click("isle.tool:demolish")
                .selected(tool == Tool::Demolish)
                .tooltip(
                    "Demolish",
                    "Click a building or a road to clear it. Nothing is refunded.",
                )
                .with_style(|s| {
                    s.direction(engine_ui::style::Direction::Column)
                        .align_items(engine_ui::style::Align::Center)
                        .gap(4.0)
                        .padding([8.0, 6.0])
                        .width(84.0)
                })
                .child(
                    El::icon("isle_demolish").with_style(|s| s.size(38.0, 38.0).pick(Pick::Ignore)),
                )
                .child(
                    El::text("Demolish")
                        .text_style(|t| t.semibold().size(12.0).center())
                        .pick_ignore(),
                ),
        );
    }
    v
}

fn build_bar(category: usize, tool: Tool) -> El {
    let tabs = El::new().class("row").with_style(|s| s.gap(6.0)).children(
        Category::ALL.iter().enumerate().map(|(i, c)| {
            El::button(c.name(), format!("isle.cat:{i}"))
                .class("tab")
                .selected(i == category)
                .with_style(|s| s.padding([5.0, 12.0]))
        }),
    );
    let panel = El::new()
        .class("glass")
        .with_style(|s| {
            s.direction(engine_ui::style::Direction::Column)
                .gap(8.0)
                .padding([10.0, 14.0])
                .radius(16.0)
                .align_items(engine_ui::style::Align::Center)
        })
        .child(tabs)
        .child(
            El::new()
                .class("row")
                .id("isle.items")
                .with_style(|s| s.gap(8.0))
                .children(bar_items(category, tool)),
        );
    let hint = El::text("{isle.hint}")
        .text_style(|t| t.semibold().shadow().nowrap().size(15.0))
        .pick_ignore();
    El::new()
        .with_style(|s| s.fill_parent().pick(Pick::Ignore))
        .child(
            El::new()
                .with_style(|s| {
                    s.position(Position::Absolute)
                        .left(0.0)
                        .right(0.0)
                        .bottom(14.0)
                        .direction(engine_ui::style::Direction::Column)
                        .align_items(engine_ui::style::Align::Center)
                        .gap(10.0)
                        .pick(Pick::Ignore)
                })
                .child(hint)
                .child(panel),
        )
}

fn info_card(icon: &str) -> El {
    let line = |i: usize| {
        El::text(format!("{{isle.sel.l{i}}}"))
            .text_style(|t| t.nowrap().size(14.0))
            .pick_ignore()
    };
    let card = El::new()
        .class("panel")
        .with_style(|s| {
            s.position(Position::Absolute)
                .right(16.0)
                .top(84.0)
                .width(330.0)
                .padding(16.0)
                .gap(6.0)
                .direction(engine_ui::style::Direction::Column)
        })
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(10.0).align_items(engine_ui::style::Align::Center))
                .child(El::icon(icon).with_style(|s| s.size(34.0, 34.0).pick(Pick::Ignore)))
                .child(
                    El::new()
                        .class("col")
                        .with_style(|s| s.grow(1.0))
                        .child(El::text("{isle.sel.title}").class("heading").pick_ignore())
                        .child(caption("{isle.sel.sub}")),
                )
                .child(engine_ui::widgets::close_button().on_click("isle.deselect")),
        )
        .child(engine_ui::widgets::divider())
        .children((0..6).map(line))
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(8.0).margin([6.0, 0.0, 0.0, 0.0]))
                .child(El::button("Pause", "isle.sel.pause").class("ghost"))
                .child(El::button("Demolish", "isle.sel.demolish").class("ghost danger")),
        );
    El::new()
        .with_style(|s| s.fill_parent().pick(Pick::Ignore))
        .child(card)
}

fn hud_screen(key: &str, layer: i32) -> Screen {
    Screen {
        key: key.into(),
        modal: false,
        layer,
        closable: false,
        ..Default::default()
    }
}

/// Add the icons, open the bars.
pub fn setup(world: &mut World) {
    if let Some(mut lib) = world.get_resource_mut::<engine_ui::icons::IconLibrary>() {
        crate::icons::install(&mut lib);
    }
    world.init_resource::<Hud>();
    world.init_resource::<Ghost>();
    update_data_now(world);
    if let Err(e) = screens::spawn_screen(world, hud_screen(TOP, 0), &top_bar()) {
        engine::core::log::warn!("archipelago top bar: {e}");
    }
    let tool = *world.resource::<Tool>();
    if let Err(e) = screens::spawn_screen(world, hud_screen(BAR, 0), &build_bar(1, tool)) {
        engine::core::log::warn!("archipelago build bar: {e}");
    }
    world.resource_mut::<Hud>().category = 1;
}

fn refresh_items(world: &mut World) {
    let (cat, tool) = (world.resource::<Hud>().category, *world.resource::<Tool>());
    if let Some(e) = engine_ui::input::find_id(world, "isle.items") {
        let _ = engine_ui::el::replace_children(world, e, &bar_items(cat, tool));
    }
}

fn set_tool(world: &mut World, tool: Tool) {
    *world.resource_mut::<Tool>() = tool;
    world.resource_mut::<Hud>().road_from = None;
    refresh_items(world);
}

fn select(world: &mut World, e: Option<Entity>) {
    world.resource_mut::<Hud>().selected = e;
    if world.resource::<Hud>().info_open {
        screens::hide(world, INFO, true);
        world.resource_mut::<Hud>().info_open = false;
    }
    let Some(kind) = e.and_then(|e| world.get::<Building>(e)).map(|b| b.kind) else {
        return;
    };
    update_data_now(world);
    if screens::spawn_screen(world, hud_screen(INFO, 1), &info_card(kind_icon(kind))).is_ok() {
        world.resource_mut::<Hud>().info_open = true;
    }
}

fn toast(world: &mut World, title: &str, text: &str) {
    engine_ui::toast::notify(
        world,
        engine_ui::toast::ToastKind::Bad,
        "",
        title,
        text,
        3.0,
    );
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

/// Act on one UI action.
fn action(world: &mut World, a: &str) {
    if let Some(i) = a
        .strip_prefix("isle.cat:")
        .and_then(|s| s.parse::<usize>().ok())
    {
        world.resource_mut::<Hud>().category = i.min(Category::ALL.len() - 1);
        refresh_items(world);
    } else if let Some(k) = a.strip_prefix("isle.build:").and_then(Kind::from_name) {
        let rot = match *world.resource::<Tool>() {
            Tool::Build { rot, .. } => rot,
            _ => 0,
        };
        set_tool(
            world,
            if k == Kind::Road {
                Tool::Road
            } else {
                Tool::Build { kind: k, rot }
            },
        );
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
        if let Some(e) = world.resource::<Hud>().selected
            && let Some(mut p) = world.get_mut::<Producer>(e)
        {
            p.paused = !p.paused;
        }
    } else if a == "isle.sel.demolish"
        && let Some(e) = world.resource::<Hud>().selected
    {
        build::demolish(world, e);
        select(world, None);
    }
}

/// Clicks, keys and the bars' buttons.
pub fn input(world: &mut World) {
    if !world.contains_resource::<Hud>() {
        return;
    }
    let mut seen = std::mem::take(&mut world.resource_mut::<Hud>().seen);
    let events = engine_ui::input::read_events(world, &mut seen);
    world.resource_mut::<Hud>().seen = seen;
    for ev in events {
        if let UiEvent::Action { action: a, .. } = ev {
            action(world, &a);
        }
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
        world.resource_mut::<Hud>().right_travel = 0.0;
    }
    if buttons.pressed(MouseButton::Right) {
        world.resource_mut::<Hud>().right_travel += delta.length();
    }
    if buttons.just_released(MouseButton::Right)
        && world.resource::<Hud>().right_travel < 6.0
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
                    Some(Occ::Building(e)) => Some(e),
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
        Tool::Road => {
            let cell = camera::pick(world).map(|h| grid::cell_of(h.x, h.z));
            if buttons.just_pressed(MouseButton::Left) {
                world.resource_mut::<Hud>().road_from = cell;
            }
            if buttons.just_released(MouseButton::Left)
                && let (Some(a), Some(b)) = (world.resource_mut::<Hud>().road_from.take(), cell)
                && let Err(msg) = build::place_road_line(world, a, b, true)
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
                        if world.resource::<Hud>().selected == Some(e) {
                            select(world, None);
                        }
                        build::demolish(world, e);
                    }
                    Some(Occ::Road) => build::remove_road(world, c),
                    None => {}
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
        Tool::Road => {
            let b = camera::pick(world).map(|h| grid::cell_of(h.x, h.z))?;
            let a = world.resource::<Hud>().road_from.unwrap_or(b);
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
                .all(|c| g.is_road(*c) || (g.is_free(*c) && grid::cell_roadable(*c)));
            let n = cells.iter().filter(|c| !g.is_road(**c)).count();
            let msg = format!(
                "Road: drag to lay {n} cells for {} coins.",
                n * Kind::Road.def().coins as usize
            );
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
                None => vec![],
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
    if !world.contains_resource::<Ghost>()
        || world.resource::<EngineConfig>().headless && world.resource::<Tool>() == &Tool::Select
    {
        return;
    }
    let Some(art) = world.get_resource::<Art>().cloned() else {
        return;
    };
    if world.resource::<Ghost>().tiles.is_none() {
        let mesh = world.resource_mut::<Assets<Mesh>>().add(tile_mesh());
        let mat = ghost_material(world);
        let mut r = MeshRenderer::new(mesh, mat);
        r.cast_shadows = false;
        let ok = world
            .spawn((
                Name::new("Ghost tiles"),
                Transform::default(),
                r.clone(),
                MeshInstances::new(Vec::new()),
            ))
            .id();
        let bad = world
            .spawn((
                Name::new("Ghost tiles (blocked)"),
                Transform::default(),
                r,
                MeshInstances::new(Vec::new()),
            ))
            .id();
        world.resource_mut::<Ghost>().tiles = Some((ok, bad));
    }
    let tool = *world.resource::<Tool>();
    let p = preview(world, tool);
    let mut tiles: Vec<InstanceRow> = Vec::new();
    let mut groups: HashMap<(ModelKey, Mat), Vec<InstanceRow>> = HashMap::new();
    let mut hint = String::new();
    let mut ok_all = true;
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
            tiles.push(InstanceRow {
                position: Vec3::new(p.x as f32, y as f32, p.y as f32),
                yaw: 0.0,
                scale: CELL_M as f32 * 0.94,
                tint,
            });
        }
        if let Some((kind, origin, rot)) = building {
            let c = grid::footprint_center(kind, origin, rot);
            let base = Vec3::new(
                c.x as f32,
                grid::base_height(kind, origin, rot) as f32,
                c.y as f32,
            );
            let yaw = grid::yaw(rot);
            let q = Quat::from_rotation_y(yaw);
            for piece in look(kind, Tier::Fisherfolk, 7) {
                let Some(model) = art.models.get(piece.model) else {
                    continue;
                };
                for (_, mat) in model.iter() {
                    groups
                        .entry((piece.model, *mat))
                        .or_default()
                        .push(InstanceRow {
                            position: base + q * piece.pos,
                            yaw: yaw + piece.yaw,
                            scale: piece.scale,
                            tint: piece.tint,
                        });
                }
            }
        }
    }
    let (ok_e, bad_e) = world.resource::<Ghost>().tiles.unwrap();
    let (a, b) = if ok_all && tool != Tool::Demolish {
        (tiles, Vec::new())
    } else {
        (Vec::new(), tiles)
    };
    if let Some(mut i) = world.get_mut::<MeshInstances>(ok_e) {
        *i = MeshInstances::new(a);
    }
    if let Some(mut i) = world.get_mut::<MeshInstances>(bad_e) {
        *i = MeshInstances::new(b);
    }
    let mut ghost = std::mem::take(&mut *world.resource_mut::<Ghost>());
    for (key, e) in &ghost.sets {
        if !groups.contains_key(key)
            && let Some(mut inst) = world.get_mut::<MeshInstances>(*e)
        {
            inst.rows = Arc::from(Vec::new());
        }
    }
    for (key, rows) in groups {
        if let Some(&e) = ghost.sets.get(&key) {
            if let Some(mut inst) = world.get_mut::<MeshInstances>(e) {
                *inst = MeshInstances::new(rows);
            }
            continue;
        }
        let Some((mesh, mat)) = art.models[key.0].iter().find(|(_, m)| *m == key.1) else {
            continue;
        };
        let mut r = MeshRenderer::new(*mesh, art.mats.get(*mat));
        r.cast_shadows = false;
        let e = world
            .spawn((
                Name::new("Ghost"),
                Transform::default(),
                r,
                MeshInstances::new(rows),
            ))
            .id();
        ghost.sets.insert(key, e);
    }
    *world.resource_mut::<Ghost>() = ghost;
    world.resource_mut::<Hud>().hint = hint;
}

fn fmt_int(x: f64) -> String {
    engine_ui::data::group_thousands(format!("{:.0}", x))
}

/// The calendar: a game minute is a month, from spring 1820.
fn clock(s: f64) -> String {
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let m = (s / 60.0) as u64 + 3;
    format!("{} {}", MONTHS[(m % 12) as usize], 1820 + m / 12)
}

/// Fill the bars' values now.
fn update_data_now(world: &mut World) {
    let focus = {
        let mut q = world.query::<&camera::StrategyCam>();
        q.iter(world).next().map(|c| c.focus)
    };
    let isle = focus.map_or(0, |f| {
        island::island_at(f.x, f.z).unwrap_or_else(|| island::nearest_island(f.x, f.z))
    });
    let econ = world.resource::<Economy>().clone();
    let hint = world
        .get_resource::<Hud>()
        .map(|h| h.hint.clone())
        .unwrap_or_default();
    let selected = world.get_resource::<Hud>().and_then(|h| h.selected);
    let mut lines: Vec<String> = vec![String::new(); 6];
    let (mut title, mut sub) = (String::new(), String::new());
    if let Some(e) = selected
        && let Some(b) = world.get::<Building>(e).cloned()
    {
        let d = b.kind.def();
        title = d.name.to_string();
        sub = format!(
            "{} · {}",
            island::islands()[b.island].name,
            d.category.name()
        );
        if let Some(h) = world.get::<House>(e) {
            let (needs, basic) = h.tier.needs();
            sub = format!(
                "{} house · {}",
                h.tier.name(),
                island::islands()[b.island].name
            );
            lines[0] = format!("Residents: {:.1} of {:.0}", h.residents, h.tier.capacity());
            let books = &econ.islands[b.island];
            let need_list: Vec<String> = needs[..basic]
                .iter()
                .map(|g| format!("{} {:.0}%", g.name(), books.supply[g.index()] * 100.0))
                .collect();
            lines[1] = format!("Needs: {}", need_list.join(", "));
            let svc: Vec<String> = h
                .tier
                .services()
                .iter()
                .enumerate()
                .map(|(i, k)| format!("{} {}", k.def().name, if h.services[i] { "✓" } else { "✗" }))
                .collect();
            lines[2] = format!("Services: {}", svc.join(", "));
            lines[3] = format!("Taxes: {:.1} coins/min", h.residents * h.tier.tax() / 1.0);
            lines[4] = match h.tier.next() {
                Some((next, cost)) => {
                    let c: Vec<String> = cost
                        .iter()
                        .map(|(g, n)| format!("{n} {}", g.name().to_lowercase()))
                        .collect();
                    format!(
                        "Moves up to {} when full and content ({})",
                        next.name(),
                        c.join(", ")
                    )
                }
                None => "Top tier for now".into(),
            };
            lines[5] = format!("Content for {:.0} s", h.content_s);
        } else if let Some(p) = world.get::<Producer>(e) {
            lines[0] = format!("Status: {}", p.status.text());
            lines[1] = format!("Output: {:.0}%", p.efficiency * 100.0);
            lines[2] = match (d.input, d.output) {
                (Some(i), Some(o)) => {
                    format!("{} → {}, one every {:.0} s", i.name(), o.name(), d.cycle_s)
                }
                (None, Some(o)) => format!("{}, one every {:.0} s", o.name(), d.cycle_s),
                _ => String::new(),
            };
            lines[3] = d.workers.map_or(String::new(), |(t, n)| {
                format!("Workers: {n} {}", t.name().to_lowercase())
            });
            lines[4] = format!("Made so far: {}", p.made);
            lines[5] = format!("Upkeep: {:.0} coins/min", d.upkeep);
        } else {
            lines[0] = d.blurb.to_string();
            if b.kind == Kind::Harbour {
                let books = &econ.islands[b.island];
                lines[1] = format!("Warehouse room: {:.0} of each good", books.capacity);
            }
        }
    }
    let total: f32 = econ.islands.iter().map(population).sum();
    let books = &econ.islands[isle];
    {
        let mut data = world.resource_mut::<UiData>();
        data.set("isle.coins", fmt_int(econ.coins));
        let b = econ.balance_per_min;
        data.set(
            "isle.balance",
            format!("{}{:.0} / min", if b >= 0.0 { "+" } else { "−" }, b.abs()),
        );
        data.set("isle.people", fmt_int(total as f64));
        data.set("isle.island", island::islands()[isle].name);
        data.set(
            "isle.island_people",
            format!(
                "{:.0} fisherfolk · {:.0} labourers",
                books.population[0], books.population[1]
            ),
        );
        for g in Good::ALL {
            data.set(
                &format!("isle.stock.{}", g.index()),
                format!("{:.0}", books.get(g)),
            );
        }
        data.set("isle.clock", clock(econ.clock_s));
        data.set("isle.hint", hint);
        data.set("isle.sel.title", title);
        data.set("isle.sel.sub", sub);
        for (i, l) in lines.into_iter().enumerate() {
            data.set(&format!("isle.sel.l{i}"), l);
        }
    }
    if let Some(mut h) = world.get_resource_mut::<Hud>() {
        h.island = isle;
    }
}

/// Refresh the bars a few times a second.
pub fn update_data(world: &mut World) {
    if !world.contains_resource::<Hud>() {
        return;
    }
    let dt = world.resource::<Time>().real_delta_secs();
    let due = {
        let mut h = world.resource_mut::<Hud>();
        h.refresh -= dt;
        h.refresh <= 0.0
    };
    if due {
        world.resource_mut::<Hud>().refresh = 0.2;
        update_data_now(world);
    }
}
