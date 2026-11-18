//! Ships' gear: every vessel's captain and gear sockets, what they add up to, and the place to
//! put shelf items aboard, take them off, or give an order. Opened with G or from the shelf.
//!
//! Data and rules are the game's (crate::gear); this is the UI thread's.

use engine::prelude::*;
use engine_ui::el::El;
use engine_ui::kit::brass::{self, Colors, SocketState};
use engine_ui::style::{Align, Direction, Enter, Pick, Position};

use crate::gear::{GearPanel, Kit};
use crate::items::{self, ItemView, Way};

use super::shelf::{details, socket_state, tip_text, ui_rank};

pub const SCREEN: &str = "isle_fleet";
const LIST_ID: &str = "isle.fleet.list";

/// The gear screen's state.
#[derive(Resource, Default)]
pub struct Fleet {
    pub open: bool,
    /// The socket picked out (vessel, slot): its item's details, or the items that fit it.
    picked: Option<(u32, usize)>,
    /// What the list last showed (rebuilt only when it changes).
    sig: String,
}

pub fn install(world: &mut World) {
    world.init_resource::<Fleet>();
}

/// What a vessel's gear adds up to, in words; empty when it adds nothing.
fn kit_line(k: &Kit) -> String {
    let pct = |x: f32| (x * 100.0).round();
    let mut v = Vec::new();
    if k.speed.abs() > 0.005 {
        v.push(format!("{:+.0}% speed", pct(k.speed)));
    }
    if k.hold > 0 {
        v.push(format!("+{} hold", k.hold));
    }
    if k.loads > 0.005 {
        v.push(format!("{:.0}% quicker loading", pct(k.loads)));
    }
    if k.longer > 0.005 {
        v.push(format!("{:+.0}% range", pct(k.longer)));
    }
    if k.sight > 0.005 {
        v.push(format!("+{:.0} sight", k.sight));
    }
    if k.luck > 0.005 {
        v.push(format!("+{:.0}% luck", pct(k.luck)));
    }
    if k.postage > 0.005 {
        v.push(format!("+{:.0} coins a voyage", k.postage));
    }
    if k.recharge > 0.005 {
        v.push(format!("{:.0}% faster recharge", pct(k.recharge)));
    }
    v.join(" · ")
}

fn flies(craft: &str) -> bool {
    crate::vessels::Craft::ALL
        .iter()
        .any(|c| c.name() == craft && c.flies())
}

fn state_id(vessel: u32, slot: usize) -> String {
    format!("isle.gsock.{vessel}.{slot}")
}

/// One vessel: its picture, name and what its gear adds, then the captain's socket and the
/// gear sockets; under it, when one of its sockets is picked, the item or the items that fit.
fn row(p: &GearPanel, picked: Option<usize>, shelf: &[ItemView]) -> El {
    let kit = kit_line(&p.kit);
    let sockets = p.slots.iter().map(|gs| {
        let captain = gs.kind == "captain";
        let size = if captain { 50.0 } else { 42.0 };
        let action = format!("isle.fleet.pick:{}:{}", p.vessel, gs.slot);
        let socket = match &gs.item {
            Some(iv) => brass::socket_with(
                Some((iv.icon, ui_rank(iv.rank))),
                size,
                &action,
                &socket_state(iv),
                &state_id(p.vessel, gs.slot),
            )
            .tooltip(iv.name, tip_text(iv)),
            None => brass::socket(None, size, &action).tooltip(
                if captain {
                    "No captain"
                } else {
                    "Empty gear slot"
                },
                if captain {
                    "Click to sign on a captain from the shelf."
                } else {
                    "Click to fit gear from the shelf."
                },
            ),
        };
        let lit = picked == Some(gs.slot);
        El::new()
            .class("col")
            .with_style(move |s| {
                s.gap(2.0)
                    .align_items(Align::Center)
                    .padding([3.0, 3.0])
                    .radius(8.0)
                    .background(engine_ui::style::Fill::Solid(if lit {
                        Colors::brass().with_alpha(0.25)
                    } else {
                        Colors::brass().with_alpha(0.0)
                    }))
            })
            .child(socket)
            .child_opt(captain.then(|| {
                El::text("Captain")
                    .class("ink_label")
                    .text_style(|t| t.nowrap())
                    .pick_ignore()
            }))
    });
    let mut col = El::new()
        .class("col")
        .with_style(|s| s.gap(6.0).padding([8.0, 6.0]))
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(10.0).align_items(Align::Center))
                .child(brass::badge(
                    if flies(p.craft) {
                        "isle_airship"
                    } else {
                        "isle_anchor"
                    },
                    34.0,
                ))
                .child(
                    El::new()
                        .class("col")
                        .with_style(|s| s.gap(0.0).width(200.0).shrink(0.0))
                        .child(
                            El::text(p.name.clone())
                                .class("ink")
                                .text_style(|t| t.size(16.0).bold().nowrap())
                                .pick_ignore(),
                        )
                        .child(brass::label(p.craft, true))
                        .child_opt((!kit.is_empty()).then(|| {
                            El::text(kit)
                                .class("ink")
                                .text_style(|t| t.size(13.0).bold())
                                .with_style(|s| s.color(Colors::good_ink()))
                                .pick_ignore()
                        })),
                )
                .child(
                    El::new()
                        .class("row")
                        .with_style(|s| s.gap(4.0).align_items(Align::Start).shrink(0.0))
                        .children(sockets),
                ),
        );
    if let Some(slot) = picked {
        let item = p.slots.get(slot).and_then(|g| g.item.as_ref());
        col = col.child(match item {
            Some(iv) => {
                let pressable = matches!(iv.way, Way::Order { .. } | Way::Consumable { .. })
                    && iv.later.is_none();
                El::new()
                    .class("col")
                    .with_style(|s| s.gap(6.0).padding([4.0, 8.0, 4.0, 44.0]))
                    .child(details(iv))
                    .child(
                        El::new()
                            .class("row")
                            .with_style(|s| s.gap(10.0))
                            .child_opt(pressable.then(|| {
                                El::button(
                                    if iv.ready {
                                        "Give the order"
                                    } else if iv.acting {
                                        "Under way"
                                    } else {
                                        "Recharging"
                                    },
                                    format!("isle.gear.use:{}:{slot}", p.vessel),
                                )
                                .class("brass_button")
                                .disabled(!iv.ready)
                            }))
                            .child(
                                El::button(
                                    "Take ashore",
                                    format!("isle.gear.unsocket:{}:{slot}", p.vessel),
                                )
                                .class("brass_button_dark"),
                            ),
                    )
            }
            None => {
                let fits: Vec<&ItemView> = p
                    .fits
                    .iter()
                    .filter(|(s, _)| *s == slot)
                    .filter_map(|(_, uid)| shelf.iter().find(|iv| iv.uid == *uid))
                    .collect();
                let mut pick = El::new()
                    .class("col")
                    .with_style(|s| s.gap(6.0).padding([4.0, 8.0, 4.0, 44.0]));
                pick = pick.child(brass::label(
                    if fits.is_empty() {
                        "Nothing on the shelf fits this slot"
                    } else if slot == 0 {
                        "Sign on a captain"
                    } else {
                        "Fit gear from the shelf"
                    },
                    true,
                ));
                pick.child(
                    El::new()
                        .class("row")
                        .with_style(|s| s.gap(6.0).wrap(true))
                        .children(fits.into_iter().map(|iv| {
                            El::new()
                                .class("col")
                                .with_style(|s| s.gap(2.0).width(70.0).align_items(Align::Center))
                                .child(
                                    brass::socket_with(
                                        Some((iv.icon, ui_rank(iv.rank))),
                                        44.0,
                                        &format!("isle.gear.socket:{}:{slot}:{}", p.vessel, iv.uid),
                                        &socket_state(iv),
                                        "",
                                    )
                                    .tooltip(iv.name, tip_text(iv)),
                                )
                                .child(
                                    El::text(iv.name)
                                        .class("ink")
                                        .text_style(|t| t.size(12.0).center())
                                        .pick_ignore(),
                                )
                        })),
                )
            }
        });
    }
    col
}

fn rule() -> El {
    El::new().with_style(|s| {
        s.height(1.0)
            .width(engine_ui::style::pct(100.0))
            .background(engine_ui::style::Fill::Solid(
                Colors::brass_dark().with_alpha(0.35),
            ))
            .pick(Pick::Ignore)
    })
}

/// The vessels, those with anything aboard first.
fn panels(world: &mut World) -> Vec<GearPanel> {
    let mut v = crate::gear::fleet(world);
    v.sort_by_key(|p| !p.slots.iter().any(|g| g.item.is_some()));
    v
}

fn body(world: &mut World) -> Vec<El> {
    let picked = world.resource::<Fleet>().picked;
    let shelf = items::shelf_view(world);
    let list = panels(world);
    if list.is_empty() {
        return vec![
            El::text("No vessels yet. Build a harbour and a shipyard.")
                .class("ink")
                .text_style(|t| t.size(14.0).italic())
                .with_style(|s| s.color(Colors::ink_dim()))
                .pick_ignore(),
        ];
    }
    let mut out = Vec::new();
    for (i, p) in list.iter().enumerate() {
        if i > 0 {
            out.push(rule());
        }
        let mine = picked.filter(|(v, _)| *v == p.vessel).map(|(_, s)| s);
        out.push(row(p, mine, &shelf));
    }
    out
}

fn sig(world: &mut World) -> String {
    let picked = world.resource::<Fleet>().picked;
    let mut list = panels(world);
    for p in &mut list {
        for g in &mut p.slots {
            if let Some(iv) = &mut g.item {
                iv.left_s = 0.0;
                iv.cool_s = 0.0;
                iv.fill = 0.0;
            }
        }
    }
    format!(
        "{}|{}|{picked:?}",
        serde_json::to_string(&list).unwrap_or_default(),
        serde_json::to_string(&items::shelf_view(world)).unwrap_or_default()
    )
}

fn el(world: &mut World) -> El {
    let n = crate::gear::fleet(world).len();
    let left = if world.resource::<super::shelf::Shelf>().open {
        478.0
    } else {
        22.0
    };
    let page = brass::parchment()
        .with_style(move |s| {
            s.position(Position::Absolute)
                .left(left)
                .top(150.0)
                .width(560.0)
                .padding([18.0, 22.0])
                .gap(10.0)
                .direction(Direction::Column)
                .enter(Enter::slide(0.38, -70.0))
        })
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(12.0).align_items(Align::Center))
                .child(brass::badge("isle_anchor", 52.0))
                .child(
                    El::new()
                        .class("col")
                        .with_style(|s| s.gap(0.0).grow(1.0))
                        .child(El::text("Ships' gear").class("ink_title").pick_ignore())
                        .child(brass::label(
                            &format!(
                                "{n} vessels · a captain and gear aboard each · click a socket"
                            ),
                            true,
                        )),
                )
                .child(
                    brass::medallion("isle_close", 32.0, "isle.fleet")
                        .tooltip("", "Close [color=#6a5339](G)[/color]"),
                ),
        )
        .child(brass::divider(516.0))
        .child(
            El::new()
                .class("scroll")
                .id(LIST_ID)
                .with_style(|s| s.direction(Direction::Column).gap(2.0).max_height(760.0))
                .children(body(world)),
        );
    El::new()
        .with_style(|s| s.fill_parent().pick(Pick::Ignore))
        .child(page)
}

/// Open or close the gear screen.
pub fn toggle(world: &mut World) {
    if world.resource::<Fleet>().open {
        engine_ui::screens::hide(world, SCREEN, true);
        let mut f = world.resource_mut::<Fleet>();
        f.open = false;
        f.picked = None;
        return;
    }
    let screen = engine_ui::node::Screen {
        key: SCREEN.into(),
        modal: false,
        layer: 2,
        closable: false,
        ..Default::default()
    };
    let el = el(world);
    let s = sig(world);
    if engine_ui::screens::spawn_screen(world, screen, &el).is_ok() {
        let mut f = world.resource_mut::<Fleet>();
        f.open = true;
        f.sig = s;
    }
}

/// Rebuild the list when what it shows changed; move the sockets' clocks every tick.
pub fn refresh(world: &mut World) {
    if !world.resource::<Fleet>().open {
        return;
    }
    let s = sig(world);
    if world.resource::<Fleet>().sig != s {
        let els = body(world);
        if let Some(e) = engine_ui::input::find_id(world, LIST_ID) {
            let _ = engine_ui::el::replace_children(world, e, &els);
            world.resource_mut::<Fleet>().sig = s;
        }
    }
    for p in crate::gear::fleet(world) {
        for g in &p.slots {
            if let Some(iv) = &g.item
                && let Some(e) = engine_ui::input::find_id(world, &state_id(p.vessel, g.slot))
                && let Some(mut st) = world.get_mut::<SocketState>(e)
            {
                let new = socket_state(iv);
                if *st != new {
                    *st = new;
                }
            }
        }
    }
}

/// `isle.fleet*` buttons. Returns false for anything else.
pub fn action(world: &mut World, a: &str) -> bool {
    if a == "isle.fleet" {
        toggle(world);
    } else if let Some(rest) = a.strip_prefix("isle.fleet.pick:") {
        let n: Vec<u32> = rest.split(':').filter_map(|s| s.parse().ok()).collect();
        if let [v, s] = n[..] {
            let mut f = world.resource_mut::<Fleet>();
            let pick = Some((v, s as usize));
            f.picked = if f.picked == pick { None } else { pick };
        }
        refresh(world);
    } else {
        return false;
    }
    true
}

/// After an `isle.gear.*` action: show the result at once.
pub fn after_gear_action(world: &mut World, a: &str) {
    // A fitted item: keep its socket picked, now showing the item.
    if let Some(rest) = a.strip_prefix("isle.gear.socket:") {
        let n: Vec<u32> = rest.split(':').filter_map(|s| s.parse().ok()).collect();
        if let [v, s, _] = n[..] {
            world.resource_mut::<Fleet>().picked = Some((v, s as usize));
        }
    }
    refresh(world);
}

/// `--hud-fleet=k`: open the gear screen with socket `k` picked on the first vessel that has
/// an order or a consumable there (or the first vessel).
pub fn demo(world: &mut World, k: usize) {
    if !world.resource::<Fleet>().open {
        toggle(world);
    }
    let list = panels(world);
    let pressable = |p: &&GearPanel| {
        p.slots
            .get(k)
            .and_then(|g| g.item.as_ref())
            .is_some_and(|iv| matches!(iv.way, Way::Order { .. } | Way::Consumable { .. }))
    };
    if let Some(p) = list.iter().find(pressable).or(list.first()) {
        world.resource_mut::<Fleet>().picked = Some((p.vessel, k));
        // `--hud-fleet-use`: give that order, to show it under way.
        if std::env::args().any(|a| a == "--hud-fleet-use") {
            crate::tools::action(world, &format!("isle.gear.use:{}:{k}", p.vessel));
        }
    }
    refresh(world);
}
