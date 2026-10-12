//! The shop screen: a merchant's wares as item cards, buy and sell tabs, a details panel with
//! quantity and total, and the player's bag along the bottom. Drag a card onto the bag to
//! buy it, or a bag item onto the merchant's wares to sell it; right click (or X) buys or
//! sells one at once.
//!
//! Open it with `ui.show {"screen": "shop", "vendor": "Merchant"}` (the name of an entity
//! with a [`Vendor`]; `owner` names the buyer, default the Player).

use bevy_ecs::prelude::*;
use serde_json::{Value, json};

use crate::color::Color;
use crate::el::{self, El};
use crate::input::UiEvent;
use crate::inventory::{self, group, item_details, item_slot};
use crate::items::*;
use crate::node::*;
use crate::screens;
use crate::style::{Align, Corners, Fill, Overflow, Pick, Shadow};
use crate::theme::Theme;
use crate::toast::{self, ToastKind};
use crate::widgets;

/// Buying from the vendor or selling to them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShopMode {
    Buy,
    Sell,
}

/// State of an open shop screen (on its root).
#[derive(Component, Debug, Clone)]
pub struct ShopScreen {
    pub owner: Entity,
    pub vendor: Entity,
    pub mode: ShopMode,
    /// Selected card (stock index when buying, bag index when selling).
    pub selected: Option<usize>,
    pub shown: Option<usize>,
    pub quantity: u32,
    pub snapshot: Option<(Inventory, Vendor)>,
}

fn find_vendor(world: &mut World, args: &Value) -> Result<Entity, String> {
    if let Some(name) = args.get("vendor").and_then(Value::as_str) {
        let e = engine_core::registry::find_by_name(world, name)?;
        if world.get::<Vendor>(e).is_none() {
            return Err(format!("\"{name}\" has no Vendor component."));
        }
        return Ok(e);
    }
    let mut q = world.query_filtered::<Entity, With<Vendor>>();
    q.iter(world)
        .next()
        .ok_or_else(|| "No entity has a Vendor. Pass \"vendor\" or run the UI demo.".into())
}

fn snapshot(world: &World, st: &ShopScreen) -> Option<(Inventory, Vendor)> {
    Some((
        world.get::<Inventory>(st.owner)?.clone(),
        world.get::<Vendor>(st.vendor)?.clone(),
    ))
}

/// The stack a card shows.
fn card_stack(st: &ShopScreen, inv: &Inventory, vendor: &Vendor, i: usize) -> Option<ItemStack> {
    match st.mode {
        ShopMode::Buy => vendor.stock.get(i).cloned(),
        ShopMode::Sell => inv.slots.get(i).cloned().flatten(),
    }
}

fn card(theme: &Theme, db: &ItemDatabase, st: &ShopScreen, inv: &Inventory, vendor: &Vendor, i: usize, stack: &ItemStack) -> El {
    let p = &theme.palette;
    let def = db.get(&stack.item);
    let (price, ok) = match (st.mode, def) {
        (ShopMode::Buy, Some(d)) => {
            let price = vendor.price(d);
            (price, inv.gold >= price)
        }
        (ShopMode::Sell, Some(d)) => (vendor.offer(d), d.category != ItemCategory::Quest),
        _ => (0, false),
    };
    let name = def.map(|d| d.name.clone()).unwrap_or(stack.item.clone());
    let rc = def.map(|d| theme.rarity(d.rarity as usize)).unwrap_or(p.text);
    let cat = def.map(|d| d.category.label()).unwrap_or("");
    let display = ItemStack::new(&stack.item, if st.mode == ShopMode::Buy { 1 } else { stack.count });
    let mut slot = item_slot(theme, db, Some(&display), 58.0);
    slot.tooltip = None;
    let count = match (st.mode, stack.count) {
        (ShopMode::Buy, 0) => "∞".to_string(),
        (_, n) => format!("×{n}"),
    };
    let mut el = El::new()
        .class("card")
        .id(format!("shop_item:{i}"))
        .on_click(format!("shop.select:{i}"))
        .drag(match st.mode {
            ShopMode::Buy => format!("shop:{i}"),
            ShopMode::Sell => format!("inv:{i}"),
        })
        .selected(st.selected == Some(i))
        .with_style(|s| s.row().align_items(Align::Center).gap(12.0).padding([8.0, 12.0, 8.0, 8.0]).width(370.0))
        .child(slot.pick_ignore())
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(3.0).grow(1.0).shrink(1.0))
                .child(
                    El::text(name)
                        .text_style(|t| t.semibold().color(rc.lerp(Color::WHITE, 0.25)))
                        .with_style(|s| s.font_size(15.0))
                        .pick_ignore(),
                )
                .child(
                    El::new()
                        .class("row")
                        .with_style(|s| s.gap(8.0))
                        .child(El::text(cat).class("caption").pick_ignore())
                        .child(El::text(count).class("caption").pick_ignore()),
                ),
        )
        .child(
            widgets::currency(&group(price), 17.0)
                .with_style(|s| if ok { s } else { s.color(p.bad) })
                .pick_ignore(),
        );
    if !ok {
        el = el.with_style(|s| s.opacity(0.62));
    }
    el
}

fn hints(mode: ShopMode) -> El {
    El::new()
        .class("hud_panel row")
        .with_style(|s| s.gap(22.0).padding([10.0, 18.0]).radius(Corners::all(999.0)))
        .child(widgets::key_hint("LMB", "A", "Select"))
        .child(widgets::key_hint(
            "RMB",
            "X",
            if mode == ShopMode::Buy { "Buy one" } else { "Sell one" },
        ))
        .child(widgets::key_hint("Drag", "", if mode == ShopMode::Buy { "Drop on your bag to buy" } else { "Drop on the wares to sell" }))
        .child(widgets::key_hint2(["Q", "E"], ["LB", "RB"], "Buy / Sell"))
        .child(widgets::key_hint("Esc", "B", "Leave"))
}

fn build(world: &World, st: &ShopScreen) -> El {
    let theme = world.resource::<Theme>().clone();
    let db = world.resource::<ItemDatabase>().clone();
    let inv = world.get::<Inventory>(st.owner).cloned().unwrap_or_default();
    let vendor = world.get::<Vendor>(st.vendor).cloned().unwrap_or_default();
    let p = &theme.palette;

    let portrait = El::new()
        .with_style(|s| {
            s.size(96.0, 96.0)
                .radius(Corners::all(48.0))
                .overflow(Overflow::Hidden)
                .background(Fill::Radial {
                    inner: Color::hex(0xa0602a).fade(0.6),
                    outer: Color::hex(0x2a160c),
                })
                .border(2.0, p.accent.fade(0.7))
                .shadow(Shadow::glow(p.accent.fade(0.3), 16.0))
                .padding([6.0, 0.0, 0.0, 0.0])
                .center()
        })
        .child(El::icon(vendor.portrait.clone()).with_style(|s| s.size(100.0, 125.0).margin([26.0, 0.0, 0.0, 0.0])).pick_ignore());
    let head = El::new()
        .class("row")
        .with_style(|s| s.gap(18.0))
        .child(portrait)
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(4.0).grow(1.0).shrink(1.0))
                .child(El::text(vendor.name.clone()).class("title"))
                .child(
                    El::text(format!("“{}”", vendor.greeting))
                        .text_style(|t| t.italic())
                        .with_style(|s| s.color(p.text_dim).font_size(15.0)),
                ),
        )
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(2.0).align_items(Align::End))
                .child(El::text("Merchant's purse").class("label_caps").text_style(|t| t.nowrap()))
                .child(widgets::currency(&group(vendor.gold), 16.0)),
        )
        .child(widgets::close_button());

    // Cards.
    let mut grid = El::new()
        .id("shop_grid")
        .drop_target("vendor", &["inv:"])
        .with_style(|s| s.row().wrap(true).gap(10.0).align_content(Align::Start));
    let entries: Vec<(usize, ItemStack)> = match st.mode {
        ShopMode::Buy => vendor.stock.iter().cloned().enumerate().collect(),
        ShopMode::Sell => inv
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.clone().map(|s| (i, s)))
            .collect(),
    };
    for (i, stack) in &entries {
        grid = grid.child(card(&theme, &db, st, &inv, &vendor, *i, stack));
    }
    if entries.is_empty() {
        grid = grid.child(El::text(if st.mode == ShopMode::Buy { "Sold out." } else { "Nothing to sell." }).class("dim"));
    }
    let wares = El::new()
        .class("panel")
        .id("shop_wares")
        .with_style(|s| s.width(820.0))
        .child(head)
        .child(
            El::new()
                .class("row")
                .child(widgets::tabs(&["Buy", "Sell"], (st.mode == ShopMode::Sell) as usize, "shop.mode"))
                .child(El::new().class("spacer"))
                .child(
                    El::text(match st.mode {
                        ShopMode::Buy => format!("{} wares", vendor.stock.len()),
                        ShopMode::Sell => format!("Brannoc pays {:.0}% of value", vendor.buy_rate * 100.0),
                    })
                    .class("caption"),
                ),
        )
        .child(
            El::new()
                .class("scroll")
                .id("shop_scroll")
                .with_style(|s| s.grow(1.0).padding([2.0, 6.0, 2.0, 2.0]))
                .child(grid),
        );

    let details = El::new()
        .class("panel")
        .id("shop_details")
        .with_style(|s| s.width(380.0))
        .child(details(world, st));

    // The player's bag strip (drop target for purchases, drag source for sales).
    let mut strip = El::new().class("row").with_style(|s| s.gap(6.0));
    for (i, stack) in inv.slots.iter().enumerate().take(14) {
        let mut el = item_slot(&theme, &db, stack.as_ref(), 50.0)
            .id(format!("shop_bag:{i}"))
            .drop_target("bag", &["shop:"]);
        if stack.is_some() {
            el = el.drag(format!("inv:{i}"));
        }
        strip = strip.child(el);
    }
    let bag = El::new()
        .class("panel row")
        .id("shop_bag")
        .drop_target("bag", &["shop:"])
        .with_style(|s| s.padding([14.0, 18.0]).gap(18.0).width(1218.0))
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(4.0))
                .child(El::text("Your bag").class("heading").with_style(|s| s.font_size(16.0)))
                .child(El::text(format!("{} / {} slots", inv.used(), inv.slots.len())).class("caption")),
        )
        .child(strip)
        .child(El::new().class("spacer"))
        .child(widgets::currency(&group(inv.gold), 24.0).id("shop_gold"));

    El::new()
        .with_style(|s| s.fill_parent().column().center().gap(16.0).pick(Pick::Ignore))
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(18.0).align_items(Align::Stretch).height(620.0))
                .child(wares)
                .child(details),
        )
        .child(bag)
        .child(hints(st.mode))
}

fn details(world: &World, st: &ShopScreen) -> El {
    let theme = world.resource::<Theme>().clone();
    let db = world.resource::<ItemDatabase>().clone();
    let inv = world.get::<Inventory>(st.owner).cloned().unwrap_or_default();
    let vendor = world.get::<Vendor>(st.vendor).cloned().unwrap_or_default();
    let shown = st.shown.or(st.selected);
    let stack = shown.and_then(|i| card_stack(st, &inv, &vendor, i));
    let def = stack.as_ref().and_then(|s| db.get(&s.item)).cloned();
    let compare = def
        .as_ref()
        .and_then(|d| d.slot)
        .and_then(|slot| inv.equipment.get(&slot))
        .cloned();
    let mut actions = Vec::new();
    let mut price = None;
    if let (Some(d), Some(s)) = (&def, &stack) {
        let unit = match st.mode {
            ShopMode::Buy => vendor.price(d),
            ShopMode::Sell => vendor.offer(d),
        };
        let max = match st.mode {
            ShopMode::Buy if s.count > 0 => s.count,
            ShopMode::Buy => 99,
            ShopMode::Sell => s.count,
        };
        let qty = st.quantity.clamp(1, max.max(1));
        let total = unit * qty as u64;
        let ok = match st.mode {
            ShopMode::Buy => inv.gold >= total,
            ShopMode::Sell => d.category != ItemCategory::Quest && vendor.gold >= total,
        };
        price = Some((if st.mode == ShopMode::Buy { "Price" } else { "Offer" }, unit, ok || st.mode == ShopMode::Sell));
        let stepper = El::new()
            .class("row")
            .with_style(|s| s.gap(6.0))
            .child(
                El::new()
                    .class("icon_button")
                    .id("shop_less")
                    .on_click("shop.qty:-1")
                    .disabled(qty <= 1)
                    .child(El::icon("ui_minus").with_style(|s| s.size(14.0, 14.0)).pick_ignore()),
            )
            .child(
                El::text(qty.to_string())
                    .text_style(|t| t.tabular().center().semibold())
                    .with_style(|s| s.width(34.0).font_size(17.0)),
            )
            .child(
                El::new()
                    .class("icon_button")
                    .id("shop_more")
                    .on_click("shop.qty:1")
                    .disabled(qty >= max)
                    .child(El::icon("ui_plus").with_style(|s| s.size(14.0, 14.0)).pick_ignore()),
            );
        let verb = if st.mode == ShopMode::Buy { "Buy" } else { "Sell" };
        actions.push(stepper);
        actions.push(
            El::new()
                .class("button primary")
                .id("shop_confirm")
                .on_click("shop.confirm")
                .disabled(!ok)
                .with_style(|s| s.grow(1.0))
                .child(El::text(format!("{verb} for")).pick_ignore())
                .child(widgets::currency(&group(total), 16.0).with_style(|s| s.color(Color::hex(0x23180a))).pick_ignore()),
        );
    }
    let mut el = item_details(&theme, &db, stack.as_ref(), compare.as_ref(), price, actions);
    if let (Some(d), ShopMode::Buy) = (&def, st.mode)
        && inv.gold < vendor.price(d)
    {
        el = el.child(
            El::text(format!("You need {} more gold.", group(vendor.price(d) - inv.gold)))
                .with_style(|s| s.color(theme.palette.bad).font_size(14.0)),
        );
    }
    el
}

/// Builder for `ui.show {"screen": "shop"}`.
pub fn build_screen(world: &mut World, args: &Value) -> Result<Entity, String> {
    let owner = inventory::find_owner(world, args)?;
    let vendor = find_vendor(world, args)?;
    let mut st = ShopScreen {
        owner,
        vendor,
        mode: if args.get("mode").and_then(Value::as_str) == Some("sell") {
            ShopMode::Sell
        } else {
            ShopMode::Buy
        },
        selected: None,
        shown: None,
        quantity: 1,
        snapshot: None,
    };
    st.snapshot = snapshot(world, &st);
    let content = build(world, &st);
    let el = El::new()
        .class("screen")
        .child(El::new().with_style(|_| widgets::scrim(0.7)))
        .child(content);
    let sp = screens::spawn_screen(
        world,
        Screen {
            modal: true,
            layer: 20,
            ..Default::default()
        },
        &el,
    )?;
    let root = sp.root.ok_or("empty")?;
    world.entity_mut(root).insert(st);
    Ok(root)
}

fn rebuild(world: &mut World, root: Entity) {
    let Some(st) = world.get::<ShopScreen>(root).cloned() else {
        return;
    };
    // Keep the scroll position across rebuilds.
    let scroll = screens::find_in(world, root, "shop_scroll").and_then(|e| world.get::<ScrollState>(e).copied());
    let content = build(world, &st);
    let children: Vec<Entity> = world.get::<Children>(root).map(|c| c.iter().collect()).unwrap_or_default();
    for c in children.iter().skip(1) {
        if let Ok(e) = world.get_entity_mut(*c) {
            e.despawn();
        }
    }
    let _ = el::spawn(world, Some(root), &content);
    if let (Some(s), Some(e)) = (scroll, screens::find_in(world, root, "shop_scroll")) {
        world.entity_mut(e).insert(s);
    }
    let snap = snapshot(world, &st);
    if let Some(mut s) = world.get_mut::<ShopScreen>(root) {
        s.snapshot = snap;
    }
}

fn refresh_details(world: &mut World, root: Entity) {
    let Some(st) = world.get::<ShopScreen>(root).cloned() else {
        return;
    };
    if let Some(panel) = screens::find_in(world, root, "shop_details") {
        let content = details(world, &st);
        let _ = el::replace_children(world, panel, &[content]);
    }
}

/// Buy (or sell) `qty` of card `i`. Returns a message for a toast.
pub fn trade(world: &mut World, st: &ShopScreen, i: usize, qty: u32) -> Result<String, String> {
    let db = world.resource::<ItemDatabase>().clone();
    let mut inv = world.get::<Inventory>(st.owner).cloned().ok_or("No inventory.")?;
    let mut vendor = world.get::<Vendor>(st.vendor).cloned().ok_or("No vendor.")?;
    let item = card_stack(st, &inv, &vendor, i).ok_or("That item is gone.")?;
    let name = db.get(&item.item).map(|d| d.name.clone()).unwrap_or(item.item.clone());
    let msg = match st.mode {
        ShopMode::Buy => {
            let cost = buy(&db, &mut vendor, &mut inv, i, qty)?;
            format!("Bought {}{name} for {} gold", if qty > 1 { format!("{qty} × ") } else { String::new() }, group(cost))
        }
        ShopMode::Sell => {
            let got = sell(&db, &mut vendor, &mut inv, i, qty)?;
            format!("Sold {}{name} for {} gold", if qty > 1 { format!("{qty} × ") } else { String::new() }, group(got))
        }
    };
    *world.get_mut::<Inventory>(st.owner).unwrap() = inv;
    *world.get_mut::<Vendor>(st.vendor).unwrap() = vendor;
    let gold = world.get::<Inventory>(st.owner).map(|i| i.gold).unwrap_or(0);
    world.resource_mut::<crate::data::UiData>().set("player.gold", gold);
    world.write_message(UiEvent::Action {
        action: format!(
            "shop.{}:{}:{qty}",
            if st.mode == ShopMode::Buy { "bought" } else { "sold" },
            item.item
        ),
        id: None,
        alt: false,
        entity: None,
    });
    Ok(msg)
}

/// Handle input for open shop screens and refresh them when the inventory or stock changes.
pub fn shop_screen_system(world: &mut World) {
    let events: Vec<UiEvent> = world
        .resource::<bevy_ecs::message::Messages<UiEvent>>()
        .iter_current_update_messages()
        .cloned()
        .collect();
    let mut q = world.query::<(Entity, &ShopScreen, &Screen)>();
    let open: Vec<(Entity, ShopScreen, String)> = q
        .iter(world)
        .filter(|(_, _, s)| !s.closing)
        .map(|(e, st, s)| (e, st.clone(), s.key.clone()))
        .collect();
    for (root, mut st, key) in open {
        if world.get::<Inventory>(st.owner).is_none() || world.get::<Vendor>(st.vendor).is_none() {
            screens::close_screen(world, root, false);
            continue;
        }
        let mut dirty = false;
        let mut details_only = false;
        let mut result: Option<Result<String, String>> = None;
        for ev in &events {
            match ev {
                UiEvent::Action { action, alt, entity, .. } => {
                    if entity.is_some_and(|e| !screens::is_descendant(world, e, root)) {
                        continue;
                    }
                    if let Some(m) = action.strip_prefix("shop.mode:") {
                        st.mode = if m == "1" { ShopMode::Sell } else { ShopMode::Buy };
                        st.selected = None;
                        st.shown = None;
                        st.quantity = 1;
                        dirty = true;
                    } else if let Some(i) = action.strip_prefix("shop.select:").and_then(|i| i.parse::<usize>().ok()) {
                        if *alt {
                            result = Some(trade(world, &st, i, 1));
                        } else {
                            if st.selected != Some(i) {
                                st.quantity = 1;
                            }
                            st.selected = Some(i);
                            st.shown = Some(i);
                        }
                        dirty = true;
                    } else if let Some(d) = action.strip_prefix("shop.qty:").and_then(|d| d.parse::<i32>().ok()) {
                        st.quantity = (st.quantity as i32 + d).clamp(1, 99) as u32;
                        details_only = true;
                    } else if action == "shop.confirm"
                        && let Some(i) = st.shown.or(st.selected)
                    {
                        result = Some(trade(world, &st, i, st.quantity.max(1)));
                        st.quantity = 1;
                        dirty = true;
                    }
                }
                UiEvent::Tab { screen, .. } if *screen == key => {
                    st.mode = if st.mode == ShopMode::Buy { ShopMode::Sell } else { ShopMode::Buy };
                    st.selected = None;
                    st.shown = None;
                    dirty = true;
                }
                UiEvent::Highlighted { id, .. } => {
                    if let Some(i) = id
                        .as_deref()
                        .and_then(|id| id.strip_prefix("shop_item:"))
                        .and_then(|i| i.parse::<usize>().ok())
                        && st.shown != Some(i)
                    {
                        st.shown = Some(i);
                        if st.selected != Some(i) {
                            st.quantity = 1;
                        }
                        details_only = true;
                    }
                }
                UiEvent::Dropped { payload, slot, target, .. } => {
                    if target.is_some_and(|t| !screens::is_descendant(world, t, root)) {
                        continue;
                    }
                    if slot == "bag"
                        && let Some(i) = payload.strip_prefix("shop:").and_then(|i| i.parse().ok())
                    {
                        let buy_st = ShopScreen { mode: ShopMode::Buy, ..st.clone() };
                        result = Some(trade(world, &buy_st, i, 1));
                        dirty = true;
                    } else if slot == "vendor"
                        && let Some(i) = payload.strip_prefix("inv:").and_then(|i| i.parse().ok())
                    {
                        let sell_st = ShopScreen { mode: ShopMode::Sell, ..st.clone() };
                        let count = world
                            .get::<Inventory>(st.owner)
                            .and_then(|inv| inv.slots.get(i).cloned().flatten())
                            .map(|s: ItemStack| s.count)
                            .unwrap_or(1);
                        result = Some(trade(world, &sell_st, i, count));
                        dirty = true;
                    }
                }
                _ => {}
            }
        }
        match result {
            Some(Ok(m)) => toast::notify(world, ToastKind::Loot, "coins", "", &m, 2.6),
            Some(Err(e)) => toast::notify(world, ToastKind::Bad, "", "", &e, 2.6),
            None => {}
        }
        if snapshot(world, &st) != st.snapshot {
            dirty = true;
        }
        if let Some(mut s) = world.get_mut::<ShopScreen>(root) {
            *s = st;
        }
        if dirty {
            rebuild(world, root);
        } else if details_only {
            refresh_details(world, root);
        }
    }
}

/// Register the `shop.*` API.
pub fn register_api(app: &mut engine_core::prelude::App) {
    app.register_api(
        "shop.buy",
        "Buy from a vendor as the player would: {\"item\": \"potion_health\", \"count\": 2} (vendor/owner by name, default the first Vendor and the Player).",
        r#"{"vendor": "Merchant", "item": "potion_health", "count": 2}"#,
        |world, args| {
            let owner = inventory::find_owner(world, &args)?;
            let vendor = find_vendor(world, &args)?;
            let item = args.get("item").and_then(Value::as_str).ok_or("Pass \"item\".")?;
            let count = args.get("count").and_then(Value::as_u64).unwrap_or(1) as u32;
            let i = world
                .get::<Vendor>(vendor)
                .unwrap()
                .stock
                .iter()
                .position(|s| s.item == item)
                .ok_or_else(|| format!("The vendor doesn't sell \"{item}\"."))?;
            let st = ShopScreen { owner, vendor, mode: ShopMode::Buy, selected: None, shown: None, quantity: 1, snapshot: None };
            Ok(json!({"result": trade(world, &st, i, count)?}))
        },
    );
    app.register_api(
        "shop.sell",
        "Sell from the player's bag: {\"slot\": 3, \"count\": 1} or {\"item\": \"herb\"}.",
        r#"{"vendor": "Merchant", "item": "herb", "count": 5}"#,
        |world, args| {
            let owner = inventory::find_owner(world, &args)?;
            let vendor = find_vendor(world, &args)?;
            let inv = world.get::<Inventory>(owner).unwrap();
            let i = match (args.get("slot").and_then(Value::as_u64), args.get("item").and_then(Value::as_str)) {
                (Some(i), _) => i as usize,
                (None, Some(item)) => inv
                    .slots
                    .iter()
                    .position(|s| s.as_ref().is_some_and(|s| s.item == item))
                    .ok_or_else(|| format!("The bag has no \"{item}\"."))?,
                _ => return Err("Pass \"slot\" or \"item\".".into()),
            };
            let count = args.get("count").and_then(Value::as_u64).unwrap_or(1) as u32;
            let st = ShopScreen { owner, vendor, mode: ShopMode::Sell, selected: None, shown: None, quantity: 1, snapshot: None };
            Ok(json!({"result": trade(world, &st, i, count)?}))
        },
    );
    app.register_api(
        "shop.stock",
        "A vendor's wares with prices.",
        r#"{"vendor": "Merchant"}"#,
        |world, args| {
            let vendor = find_vendor(world, &args)?;
            let db = world.resource::<ItemDatabase>();
            let v = world.get::<Vendor>(vendor).unwrap();
            Ok(json!({
                "name": v.name,
                "gold": v.gold,
                "stock": v.stock.iter().map(|s| json!({
                    "item": s.item,
                    "count": if s.count == 0 { Value::String("unlimited".into()) } else { json!(s.count) },
                    "price": db.get(&s.item).map(|d| v.price(d)),
                })).collect::<Vec<_>>()
            }))
        },
    );
}
