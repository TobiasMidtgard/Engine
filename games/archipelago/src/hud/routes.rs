//! Trade routes: the harbour master's book. Each route is a parchment tile (its ships, then its
//! stops with what is loaded and unloaded there); the facing page is the sea chart with every
//! route drawn in its own ink along the lanes the ships sail, the stops numbered and the ships
//! where they are now. Clicking a route picks it out on the chart. Opened from the map table's
//! wheel medallion or with T.
//!
//! Part of the HUD (hud.rs), the UI thread's. Read only for now: changing routes needs the
//! game's actions (tools.rs).

use engine::prelude::*;
use engine_ui::Color;
use engine_ui::draw::DrawCtx;
use engine_ui::el::El;
use engine_ui::kit::brass::{self, Colors};
use engine_ui::node::Rect;
use engine_ui::style::{Align, Direction, Fill, Justify, Pick};
use engine_ui::text::{DISPLAY_FONT, TextSpec};

use super::map;
use crate::defs::Good;
use crate::fleet::{Routes, SeaChart, Vessel, mast_point, mooring};
use crate::island;
use crate::sim::Building;

pub const SCREEN: &str = "isle_routes";
const PAINTER: &str = "isle_routes_map";
const PAGE_W: f32 = 520.0;

/// Route inks, in order.
const INKS: [u32; 5] = [0x9a2f1c, 0x2c5a86, 0x3f7a2c, 0x9a6a12, 0x6a2f7a];

fn ink(i: usize) -> Color {
    Color::hex(INKS[i % INKS.len()])
}

/// The book's state: open, the route picked out, and each route's drawn lines.
#[derive(Resource, Default)]
pub struct RouteBook {
    pub open: bool,
    pub selected: Option<usize>,
    /// Per route: the polyline it sails (world X, Z) and its stops.
    lines: Vec<(Vec<DVec2>, Vec<DVec2>, bool)>,
}

pub fn install(world: &mut World) {
    world.init_resource::<RouteBook>();
    if let Some(mut p) = world.get_resource_mut::<engine_ui::Painters>() {
        p.add(PAINTER, paint);
    }
}

fn building(world: &mut World, id: u32) -> Option<Building> {
    let mut q = world.query::<&Building>();
    q.iter(world).find(|b| b.id == id).cloned()
}

/// Work out each route's line: sea lanes between moorings, or straight between masts.
fn trace(world: &mut World) -> Vec<(Vec<DVec2>, Vec<DVec2>, bool)> {
    let routes = world
        .get_resource::<Routes>()
        .map(|r| r.0.clone())
        .unwrap_or_default();
    let mut out = Vec::new();
    for r in routes {
        let stops: Vec<DVec2> = r
            .stops
            .iter()
            .filter_map(|s| building(world, s.at))
            .map(|b| {
                if r.air {
                    let p = mast_point(&b);
                    DVec2::new(p.x, p.z)
                } else {
                    mooring(&b).0
                }
            })
            .collect();
        let mut line = Vec::new();
        for k in 0..stops.len() {
            let (a, b) = (stops[k], stops[(k + 1) % stops.len()]);
            let lane = (!r.air)
                .then(|| {
                    world
                        .get_resource_mut::<SeaChart>()
                        .and_then(|mut c| c.lane(a, b))
                })
                .flatten();
            match lane {
                Some(p) => line.extend(p.iter().map(|q| DVec2::new(q.x, q.z))),
                None => line.extend([a, b]),
            }
        }
        out.push((line, stops, r.air));
    }
    out
}

/// A stop: the island's name over what is loaded (green) and unloaded (red) there.
fn stop_chip(name: &str, load: &[Good], unload: &[Good]) -> El {
    let goods = |mark: &str, tip: &str, list: &[Good]| {
        El::new()
            .class("row")
            .with_style(|s| s.gap(2.0).align_items(Align::Center))
            .tooltip("", tip)
            .child(El::icon(mark).with_style(|s| {
                s.size(11.0, 11.0)
                    .margin([0.0, 3.0, 0.0, 0.0])
                    .pick(Pick::Ignore)
            }))
            .children(list.iter().map(|g| {
                El::icon(g.icon())
                    .tooltip(g.name(), "")
                    .with_style(|s| s.size(20.0, 20.0))
            }))
    };
    El::new()
        .class("col")
        .with_style(|s| {
            s.gap(1.0)
                .padding([4.0, 8.0])
                .radius(3.0)
                .background(Fill::Solid(Color::hexa(0x5a3a1418)))
        })
        .child(
            El::text(name)
                .class("ink")
                .text_style(|t| t.bold().size(14.0).nowrap())
                .pick_ignore(),
        )
        .child(goods("isle_load", "Loaded here", load))
        .child(goods("isle_unload", "Unloaded here", unload))
}

/// One route as a tile: its ink, name, ships, and stops in order.
fn route_tile(world: &mut World, i: usize, selected: bool) -> Option<El> {
    let r = world.get_resource::<Routes>()?.0.get(i)?.clone();
    let mut crafts: Vec<(&'static str, usize)> = Vec::new();
    let mut cargo: Vec<(Good, u32)> = Vec::new();
    {
        let mut q = world.query::<&Vessel>();
        for v in q.iter(world).filter(|v| v.route == Some(i)) {
            match crafts.iter_mut().find(|c| c.0 == v.craft.name()) {
                Some(c) => c.1 += 1,
                None => crafts.push((v.craft.name(), 1)),
            }
            for (g, n) in &v.cargo {
                match cargo.iter_mut().find(|c| c.0 == *g) {
                    Some(c) => c.1 += n,
                    None => cargo.push((*g, *n)),
                }
            }
        }
    }
    let fleet = if crafts.is_empty() {
        "no ships yet".to_string()
    } else {
        crafts
            .iter()
            .map(|(n, k)| {
                if *k == 1 {
                    format!("1 {}", n.to_lowercase())
                } else {
                    format!("{k} {}s", n.to_lowercase())
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let aboard = if cargo.is_empty() {
        "holds empty".to_string()
    } else {
        cargo
            .iter()
            .map(|(g, n)| format!("{n} {}", g.name().to_lowercase()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut chain = El::new()
        .class("row")
        .with_style(|s| s.gap(6.0).align_items(Align::Center));
    for (k, s) in r.stops.iter().enumerate() {
        let name = building(world, s.at)
            .map(|b| island::islands()[b.island].name)
            .unwrap_or("?");
        if k > 0 {
            chain = chain.child(
                El::icon("isle_arrow").with_style(|s| s.size(24.0, 14.0).pick(Pick::Ignore)),
            );
        }
        chain = chain.child(stop_chip(name, &s.load, &s.unload));
    }
    chain = chain.child(
        El::text("and back")
            .class("ink_label")
            .text_style(|t| t.nowrap())
            .pick_ignore(),
    );
    let color = ink(i);
    Some(
        El::new()
            .class("brass_tile")
            .id(format!("isle.route:{i}"))
            .on_click(format!("isle.route:{i}"))
            .selected(selected)
            .with_style(move |s| {
                s.direction(Direction::Column)
                    .gap(6.0)
                    .padding([10.0, 14.0, 12.0, 14.0])
                    .width(500.0)
                    .enter(
                        engine_ui::style::Enter::rise(0.32, 18.0)
                            .delay(0.05 * i as f32)
                            .ease(engine_ui::style::Ease::Back),
                    )
            })
            .child(
                El::new()
                    .class("row")
                    .with_style(|s| s.gap(10.0).align_items(Align::Center))
                    .child(El::new().with_style(move |s| {
                        s.size(6.0, 34.0)
                            .radius(3.0)
                            .background(Fill::Solid(color))
                            .pick(Pick::Ignore)
                    }))
                    .child(
                        El::new()
                            .class("col")
                            .with_style(|s| s.gap(0.0).grow(1.0))
                            .child(
                                El::text(r.name.clone())
                                    .class("ink_title")
                                    .text_style(|t| t.size(19.0).nowrap())
                                    .pick_ignore(),
                            )
                            .child(
                                El::text(format!(
                                    "{} · {fleet}",
                                    if r.air { "by air" } else { "by sea" }
                                ))
                                .class("ink_label")
                                .text_style(|t| t.nowrap())
                                .pick_ignore(),
                            ),
                    )
                    .child(
                        El::text(aboard)
                            .class("ink")
                            .text_style(|t| t.size(13.0).italic())
                            .with_style(|s| s.color(Colors::ink_dim()).max_width(170.0))
                            .pick_ignore(),
                    ),
            )
            .child(chain),
    )
}

/// The book: route tiles on the plate, the chart on a parchment page.
fn el(world: &mut World) -> El {
    let n = world.get_resource::<Routes>().map_or(0, |r| r.0.len());
    let sel = world.resource::<RouteBook>().selected;
    let tiles: Vec<El> = (0..n)
        .filter_map(|i| route_tile(world, i, sel == Some(i)))
        .collect();
    let header = El::new()
        .class("row")
        .with_style(|s| s.gap(18.0).align_items(Align::Center))
        .child(brass::divider(70.0))
        .child(
            El::text("Trade routes")
                .class("brass_title")
                .text_style(|t| t.nowrap().shadow().size(30.0))
                .pick_ignore(),
        )
        .child(brass::divider(70.0))
        .child(El::new().with_style(|s| s.grow(1.0)))
        .child(
            El::text(format!("{n} routes · click one to follow it on the chart"))
                .class("brass_label")
                .text_style(|t| t.nowrap())
                .pick_ignore(),
        )
        .child(
            brass::medallion("isle_close", 36.0, "isle.routes")
                .tooltip("", "Close [color=#c9b07a](T or Esc)[/color]"),
        );
    let list = El::new()
        .class("col")
        .with_style(|s| s.gap(14.0).padding([4.0, 6.0]))
        .children(tiles);
    let page = brass::parchment()
        .with_style(|s| s.padding(14.0).direction(Direction::Column))
        .child(El::new().paint(PAINTER).with_style(|s| {
            s.size(PAGE_W, PAGE_W * map::PH as f32 / map::PW as f32)
                .border(1.0, Color::hex(0x2a1a0a))
                .radius(2.0)
                .pick(Pick::Ignore)
        }));
    let book = brass::plate()
        .with_style(|s| {
            s.direction(Direction::Column)
                .gap(14.0)
                .padding([16.0, 24.0, 22.0, 24.0])
                .enter(engine_ui::style::Enter::pop(0.35, 0.94))
        })
        .child(header)
        .child(
            El::new()
                .class("row")
                .with_style(|s| {
                    s.gap(22.0)
                        .align_items(Align::Start)
                        .justify(Justify::Center)
                })
                .child(list)
                .child(page),
        );
    El::new()
        .with_style(|s| {
            s.fill_parent()
                .background(Fill::Solid(Color::hexa(0x0a0f1650)))
                .align_items(Align::Center)
                .justify(Justify::Center)
                .padding([96.0, 0.0, 0.0, 0.0])
                .pick(Pick::Block)
        })
        .child(book)
}

/// Open the book, or close it.
pub fn toggle(world: &mut World) {
    if world.resource::<RouteBook>().open {
        engine_ui::screens::hide(world, SCREEN, true);
        world.resource_mut::<RouteBook>().open = false;
        return;
    }
    let lines = trace(world);
    world.resource_mut::<RouteBook>().lines = lines;
    let screen = engine_ui::node::Screen {
        key: SCREEN.into(),
        modal: false,
        layer: 2,
        closable: false,
        ..Default::default()
    };
    let el = el(world);
    if engine_ui::screens::spawn_screen(world, screen, &el).is_ok() {
        world.resource_mut::<RouteBook>().open = true;
    }
}

/// Pick a route out on the chart (again to let go).
pub fn select(world: &mut World, i: usize) {
    let sel = {
        let mut b = world.resource_mut::<RouteBook>();
        b.selected = if b.selected == Some(i) { None } else { Some(i) };
        b.selected
    };
    let n = world.get_resource::<Routes>().map_or(0, |r| r.0.len());
    for k in 0..n {
        if let Some(e) = engine_ui::input::find_id(world, &format!("isle.route:{k}"))
            && let Some(mut it) = world.get_mut::<engine_ui::node::Interaction>(e)
        {
            it.selected = sel == Some(k);
        }
    }
}

fn paint(world: &mut World, _e: Entity, ctx: &mut DrawCtx) {
    let r = ctx.rect;
    let px = ctx.px;
    let at = |p: DVec2| {
        let uv = map::to_uv(p.x, p.y);
        Vec2::new(r.x + uv.x * r.w, r.y + uv.y * r.h)
    };
    ctx.icon(map::PICTURE, r, Color::WHITE);
    let book = world.resource::<RouteBook>();
    let sel = book.selected;
    let lines = book.lines.clone();
    // Others first, the picked route on top.
    let mut order: Vec<usize> = (0..lines.len()).collect();
    if let Some(s) = sel {
        order.retain(|k| *k != s);
        order.push(s);
    }
    for &k in &order {
        let (line, _, air) = &lines[k];
        let on = sel.is_none_or(|s| s == k);
        let c = ink(k).with_alpha(if on { 1.0 } else { 0.3 });
        let w = if sel == Some(k) { 3.6 } else { 2.4 } * px;
        for seg in line.windows(2) {
            let (a, b) = (at(seg[0]), at(seg[1]));
            if on {
                ctx.line(a, b, w + 2.4 * px, Color::hexa(0xf6ead0b0), 0.0);
            }
            ctx.line(a, b, w, c, if *air { 7.0 * px } else { 0.0 });
        }
    }
    // Numbered stops.
    for &k in &order {
        let (_, stops, _) = &lines[k];
        let on = sel.is_none_or(|s| s == k);
        if !on {
            continue;
        }
        for (n, p) in stops.iter().enumerate() {
            let q = at(*p) + Vec2::new(0.0, -(k as f32) * 2.0 * px);
            let s = 17.0 * px;
            ctx.rect(
                Rect::new(q.x - s * 0.5, q.y - s * 0.5, s, s),
                Color::hex(0xf6ead0),
                s * 0.5,
                2.0 * px,
                ink(k),
            );
            let spec = TextSpec {
                text: (n + 1).to_string(),
                size: 11.0 * px,
                weight: 700,
                color: ink(k),
                family: Some(DISPLAY_FONT.into()),
                wrap: false,
                ..Default::default()
            };
            ctx.text_spec(&spec, q, Vec2::splat(0.5));
        }
    }
    // Ships where they are, in their route's ink.
    let mut q = world.query::<(&Vessel, &Transform)>();
    let ships: Vec<(Vec2, f32, bool, Option<usize>)> = q
        .iter(world)
        .map(|(v, t)| {
            (
                at(DVec2::new(t.translation.x, t.translation.z)),
                std::f32::consts::FRAC_PI_2 - v.heading,
                v.craft == crate::vessels::Craft::Airship,
                v.route,
            )
        })
        .collect();
    for (p, angle, air, route) in ships {
        let on = route.is_some_and(|k| sel.is_none_or(|s| s == k));
        if let Some(k) = route
            && on
        {
            ctx.glow(p, 12.0 * px, ink(k).with_alpha(0.55), 1.8);
        }
        let s = if air { 17.0 } else { 14.0 } * px;
        ctx.icon_rotated(
            if air {
                "isle_mark_airship"
            } else {
                "isle_mark_ship"
            },
            Rect::new(p.x - s * 0.5, p.y - s * 0.5, s, s),
            Color::WHITE.with_alpha(if on || sel.is_none() { 1.0 } else { 0.45 }),
            angle,
        );
    }
    let s = 64.0 * px;
    ctx.icon(
        "isle_compass",
        Rect::new(r.x + r.w - s - 10.0 * px, r.y + 10.0 * px, s, s),
        Color::WHITE,
    );
}
