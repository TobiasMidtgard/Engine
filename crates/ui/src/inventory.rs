//! The inventory screen: equipment paper doll, a bag grid with drag and drop, category tabs,
//! a details panel comparing the highlighted item with what is worn, weight and gold.
//!
//! Open it with `ui.show {"screen": "inventory"}` (optionally `"owner": "Player"`, the name of
//! the entity with the [`Inventory`]). Drag items between slots to move or stack them, onto
//! an equipment slot to equip, onto the trash to drop; right click (or X) uses or equips.

use bevy_ecs::message::MessageCursor;
use bevy_ecs::prelude::*;
use serde_json::{Value, json};

use crate::color::Color;
use crate::el::{self, El};
use crate::input::UiEvent;
use crate::items::*;
use crate::node::*;
use crate::screens;
use crate::style::{Align, Corners, Fill, Pick, Position, Shadow, StylePatch, pct};
use crate::theme::Theme;
use crate::toast::{self, ToastKind};
use crate::widgets;

/// Category tabs: label and the categories each shows (empty = all).
pub const TABS: [(&str, &[ItemCategory]); 6] = [
    ("All", &[]),
    ("Weapons", &[ItemCategory::Weapon]),
    ("Armor", &[ItemCategory::Armor]),
    ("Consumables", &[ItemCategory::Consumable]),
    ("Materials", &[ItemCategory::Material, ItemCategory::Gadget]),
    ("Quest", &[ItemCategory::Quest]),
];

/// A slot in an inventory: a bag index or an equipment slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotRef {
    Bag(usize),
    Equip(EquipSlot),
}

impl SlotRef {
    pub fn payload(self) -> String {
        match self {
            SlotRef::Bag(i) => format!("inv:{i}"),
            SlotRef::Equip(s) => format!("equip:{}", s.key()),
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        if let Some(i) = s.strip_prefix("inv:") {
            return i.parse().ok().map(SlotRef::Bag);
        }
        s.strip_prefix("equip:")
            .and_then(EquipSlot::from_key)
            .map(SlotRef::Equip)
    }

    pub fn stack(self, inv: &Inventory) -> Option<&ItemStack> {
        match self {
            SlotRef::Bag(i) => inv.slots.get(i).and_then(Option::as_ref),
            SlotRef::Equip(s) => inv.equipment.get(&s),
        }
    }
}

/// State of an open inventory screen (on its root).
#[derive(Component, Debug, Clone)]
pub struct InventoryScreen {
    pub owner: Entity,
    pub tab: usize,
    pub selected: Option<SlotRef>,
    pub shown: Option<SlotRef>,
    pub dirty: bool,
    /// The last inventory drawn (rebuild when it changes).
    pub snapshot: Option<Inventory>,
}

/// The entity whose inventory a screen shows: `args.owner` by name, else the entity named
/// "Player", else the first with an [`Inventory`].
pub fn find_owner(world: &mut World, args: &Value) -> Result<Entity, String> {
    if let Some(name) = args.get("owner").and_then(Value::as_str) {
        let e = engine_core::registry::find_by_name(world, name)?;
        if world.get::<Inventory>(e).is_none() {
            return Err(format!("\"{name}\" has no Inventory."));
        }
        return Ok(e);
    }
    let mut q = world.query::<(Entity, &Inventory, Option<&Name>)>();
    let all: Vec<(Entity, bool)> = q
        .iter(world)
        .map(|(e, _, n)| (e, n.is_some_and(|n| n.as_str() == "Player")))
        .collect();
    all.iter()
        .find(|(_, p)| *p)
        .or(all.first())
        .map(|(e, _)| *e)
        .ok_or_else(|| {
            "No entity has an Inventory. Add one (entity.spawn with an Inventory component) or run the UI demo.".into()
        })
}

/// An item slot: rarity-tinted well, icon, stack count, tooltip.
pub fn item_slot(theme: &Theme, db: &ItemDatabase, stack: Option<&ItemStack>, size: f32) -> El {
    let p = &theme.palette;
    let mut el = El::new().class("slot").with_style(|s| s.size(size, size));
    let Some(st) = stack else {
        return el;
    };
    let Some(def) = db.get(&st.item) else {
        return el.child(El::text("?").pick_ignore());
    };
    let rc = theme.rarity(def.rarity as usize);
    let well = p.well;
    el = el
        .with_style(|s| {
            s.border_color(rc.fade(0.5)).background(Fill::Radial {
                inner: rc.fade(0.2),
                outer: well,
            })
        })
        .hover(
            StylePatch::default()
                .border_color(rc.lerp(Color::WHITE, 0.3))
                .background(Fill::Radial {
                    inner: rc.fade(0.34),
                    outer: well,
                })
                .shadow(Shadow::glow(rc.fade(0.35), 12.0)),
        )
        .child(
            El::icon(def.icon.clone())
                .with_style(|s| s.size(pct(100.0), pct(100.0)))
                .pick_ignore(),
        )
        .child(
            El::new()
                .with_style(|s| {
                    s.position(Position::Absolute)
                        .left(size * 0.22)
                        .right(size * 0.22)
                        .bottom(3.0)
                        .height(2.0)
                        .radius(Corners::all(1.0))
                        .bg(rc.fade(if def.rarity >= 2 { 0.9 } else { 0.45 }))
                })
                .pick_ignore(),
        );
    if st.count > 1 {
        el = el.child(
            El::text(st.count.to_string())
                .class("slot_count")
                .text_style(|t| t.shadow().tabular().nowrap())
                .pick_ignore(),
        );
    }
    let rarity = ["Common", "Uncommon", "Rare", "Epic", "Legendary"][def.rarity.min(4) as usize];
    el.tooltip_full(Tooltip {
        title: def.name.clone(),
        text: format!(
            "[color={}]{rarity} {}[/color]\n{}",
            rc.to_hex(),
            def.category.label().to_lowercase(),
            def.description
        ),
        title_color: Some(rc),
    })
}

fn rarity_name(r: u8) -> &'static str {
    ["Common", "Uncommon", "Rare", "Epic", "Legendary"][r.min(4) as usize]
}

/// The details panel for one stack: big icon, name, rarity and category, description,
/// stats compared with `compare` (what is worn in the same slot), value and weight.
pub fn item_details(
    theme: &Theme,
    db: &ItemDatabase,
    stack: Option<&ItemStack>,
    compare: Option<&ItemStack>,
    price: Option<(&str, u64, bool)>,
    actions: Vec<El>,
) -> El {
    let p = &theme.palette;
    let Some((st, def)) = stack.and_then(|s| db.get(&s.item).map(|d| (s, d))) else {
        return El::new()
            .class("col")
            .with_style(|s| s.grow(1.0).center().gap(12.0).padding(24.0))
            .child(
                El::icon("ui_info")
                    .with_style(|s| s.size(28.0, 28.0).color(p.text_faint))
                    .pick_ignore(),
            )
            .child(
                El::text("Hover or select an item to see its details.")
                    .class("dim")
                    .text_style(|t| t.center())
                    .with_style(|s| s.max_width(220.0)),
            );
    };
    let rc = theme.rarity(def.rarity as usize);
    let mut col = El::new().class("col").with_style(|s| s.gap(12.0).grow(1.0));
    // Hero icon with a rarity glow.
    col = col.child(
        El::new()
            .with_style(|s| {
                s.height(150.0)
                    .center()
                    .radius(Corners::all(14.0))
                    .background(Fill::Radial {
                        inner: rc.fade(0.32),
                        outer: p.well.fade(0.6),
                    })
                    .border(1.0, rc.fade(0.35))
            })
            .child(
                El::icon(def.icon.clone())
                    .with_style(|s| {
                        s.size(112.0, 112.0)
                            .shadow(Shadow::glow(rc.fade(0.4), 30.0))
                    })
                    .pick_ignore(),
            ),
    );
    col = col.child(
        El::new()
            .class("col")
            .with_style(|s| s.gap(6.0))
            .child(
                El::text(def.name.clone())
                    .class("heading")
                    .text_style(|t| t.color(rc.lerp(Color::WHITE, 0.15)))
                    .with_style(|s| s.font_size(22.0)),
            )
            .child(
                El::new()
                    .class("row")
                    .with_style(|s| s.gap(6.0))
                    .child(
                        El::text(rarity_name(def.rarity))
                            .class("chip")
                            .with_style(|s| s.color(rc).border_color(rc.fade(0.4))),
                    )
                    .child(El::text(def.category.label()).class("chip"))
                    .child_opt(def.slot.map(|s| El::text(s.label()).class("chip")))
                    .child_opt(
                        (st.count > 1).then(|| El::text(format!("×{}", st.count)).class("chip")),
                    ),
            ),
    );
    if !def.description.is_empty() {
        col = col.child(
            El::text(def.description.clone())
                .text_style(|t| t.italic())
                .with_style(|s| s.color(p.text_dim).font_size(15.0)),
        );
    }
    if !def.stats.is_empty() {
        let other = compare
            .filter(|c| c.item != st.item)
            .and_then(|c| db.get(&c.item));
        let mut stats = El::new().class("col").with_style(|s| s.gap(0.0));
        for (k, v) in &def.stats {
            let delta = other.map(|o| v - o.stats.get(k).copied().unwrap_or(0.0));
            let label = k
                .split('_')
                .map(|w| {
                    let mut c = w.chars();
                    c.next()
                        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join(" ");
            let value = if v.fract().abs() > 1e-3 {
                format!("{v:.2}")
            } else {
                format!("{v:.0}")
            };
            stats = stats.child(widgets::stat_row(&label, &value, delta));
        }
        if let Some(o) = other {
            stats = stats.child(
                El::text(format!("Compared with {}", o.name))
                    .class("caption")
                    .with_style(|s| s.margin([6.0, 0.0, 0.0, 0.0])),
            );
        }
        col = col.child(widgets::divider()).child(stats);
    }
    col = col.child(widgets::divider());
    let mut meta = El::new().class("row").with_style(|s| s.gap(16.0));
    if let Some((label, amount, ok)) = price {
        meta = meta
            .child(
                El::text(label)
                    .class("dim")
                    .with_style(|s| s.font_size(14.0)),
            )
            .child(
                widgets::currency(&group(amount), 18.0)
                    .with_style(|s| if ok { s } else { s.color(Color::hex(0xff6b6b)) }),
            );
    } else if def.value > 0 {
        meta = meta
            .child(
                El::text("Value")
                    .class("dim")
                    .with_style(|s| s.font_size(14.0)),
            )
            .child(widgets::currency(&group(def.value as u64), 18.0));
    }
    meta = meta.child(El::new().class("spacer")).child(
        El::text(format!("{:.1} kg", def.weight * st.count as f32))
            .class("dim")
            .with_style(|s| s.font_size(14.0)),
    );
    col = col.child(meta);
    if !actions.is_empty() {
        col = col.child(El::new().class("spacer")).child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(10.0))
                .children(actions),
        );
    }
    col
}

/// `1250` → `1,250`.
pub fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn equip_slot_el(
    theme: &Theme,
    db: &ItemDatabase,
    inv: &Inventory,
    slot: EquipSlot,
    selected: bool,
) -> El {
    let stack = inv.equipment.get(&slot);
    let r = SlotRef::Equip(slot);
    let mut el = item_slot(theme, db, stack, 60.0)
        .id(format!("equip_slot:{}", slot.key()))
        .on_click(format!("inv.select:{}", r.payload()))
        .drop_target(r.payload(), &["inv:", "equip:"])
        .selected(selected);
    if stack.is_some() {
        el = el.drag(r.payload());
    } else {
        el = el
            .child(
                El::icon(slot.placeholder_icon())
                    .with_style(|s| s.size(pct(100.0), pct(100.0)).opacity(0.16))
                    .pick_ignore(),
            )
            .tooltip(
                slot.label(),
                "[color=#8a94a8]Empty. Drag an item here to equip it.[/color]",
            );
    }
    El::new()
        .class("col")
        .with_style(|s| s.gap(4.0).align_items(Align::Center))
        .child(el)
        .child(
            El::text(slot.label())
                .class("label_caps")
                .text_style(|t| t.nowrap())
                .pick_ignore(),
        )
}

fn matches_tab(def: Option<&ItemDef>, tab: usize) -> bool {
    let cats = TABS.get(tab).map(|t| t.1).unwrap_or(&[]);
    cats.is_empty() || def.is_some_and(|d| cats.contains(&d.category))
}

fn hints() -> El {
    El::new()
        .class("hud_panel row")
        .with_style(|s| {
            s.gap(22.0)
                .padding([10.0, 18.0])
                .radius(Corners::all(999.0))
        })
        .child(widgets::key_hint("LMB", "A", "Select / drag"))
        .child(widgets::key_hint("RMB", "X", "Use or equip"))
        .child(widgets::key_hint2(["Q", "E"], ["LB", "RB"], "Category"))
        .child(widgets::key_hint("Esc", "B", "Close"))
}

/// Build the whole inventory screen content for `owner`.
fn build(world: &mut World, st: &InventoryScreen) -> El {
    let theme = world.resource::<Theme>().clone();
    let db = world.resource::<ItemDatabase>().clone();
    let inv = world
        .get::<Inventory>(st.owner)
        .cloned()
        .unwrap_or_default();
    let name = world
        .resource::<crate::data::UiData>()
        .get_str("player.name")
        .map(String::from)
        .or_else(|| world.get::<Name>(st.owner).map(|n| n.as_str().to_string()))
        .unwrap_or_else(|| "Traveler".into());
    let p = &theme.palette;
    let sel = st.selected;

    // Equipment panel.
    let left: Vec<EquipSlot> = vec![
        EquipSlot::Head,
        EquipSlot::Chest,
        EquipSlot::Hands,
        EquipSlot::Feet,
        EquipSlot::Back,
    ];
    let right: Vec<EquipSlot> = vec![
        EquipSlot::MainHand,
        EquipSlot::OffHand,
        EquipSlot::Ring,
        EquipSlot::Amulet,
    ];
    let slot_col = |slots: &[EquipSlot]| {
        El::new().class("col").with_style(|s| s.gap(10.0)).children(
            slots
                .iter()
                .map(|s| equip_slot_el(&theme, &db, &inv, *s, sel == Some(SlotRef::Equip(*s)))),
        )
    };
    let armor = inv.equipped_stat(&db, "armor");
    let damage = inv.equipped_stat(&db, "damage");
    let doll = El::new()
        .with_style(|s| {
            s.grow(1.0)
                .height(pct(100.0))
                .center()
                .radius(Corners::all(14.0))
                .background(Fill::Radial {
                    inner: p.info.fade(0.14),
                    outer: Color::TRANSPARENT,
                })
        })
        .child(
            El::icon("portrait:pilot")
                .with_style(|s| s.size(170.0, 200.0).opacity(0.95))
                .pick_ignore(),
        );
    let equipment = El::new()
        .class("panel")
        .id("inv_equipment")
        .with_style(|s| s.width(360.0))
        .child(widgets::header(None, "Equipment", &name, false))
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(8.0).align_items(Align::Start))
                .child(slot_col(&left))
                .child(doll)
                .child(slot_col(&right)),
        )
        .child(widgets::divider())
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(0.0))
                .child(widgets::stat_row("Armor", &format!("{armor:.0}"), None))
                .child(widgets::stat_row("Damage", &format!("{damage:.0}"), None))
                .child(widgets::stat_row(
                    "Speed",
                    &format!(
                        "{:.2}×",
                        inv.equipment
                            .values()
                            .filter_map(|s| db.get(&s.item))
                            .filter_map(|d| d.stats.get("speed"))
                            .product::<f32>()
                    ),
                    None,
                )),
        );

    // Bag panel.
    let cols = 8;
    let slot = 64.0;
    let gap = 8.0;
    let mut grid = El::new().id("inv_grid").with_style(|s| {
        s.row()
            .wrap(true)
            .gap(gap)
            .width(cols as f32 * slot + (cols - 1) as f32 * gap)
    });
    for (i, stack) in inv.slots.iter().enumerate() {
        let r = SlotRef::Bag(i);
        let def = stack.as_ref().and_then(|s| db.get(&s.item));
        let mut el = item_slot(&theme, &db, stack.as_ref(), slot)
            .id(format!("inv_slot:{i}"))
            .on_click(format!("inv.select:{}", r.payload()))
            .drop_target(r.payload(), &["inv:", "equip:"])
            .selected(sel == Some(r));
        if stack.is_some() {
            el = el.drag(r.payload());
        }
        if stack.is_some() && !matches_tab(def, st.tab) {
            el = el.with_style(|s| s.opacity(0.22));
        }
        grid = grid.child(el);
    }
    let weight = inv.weight(&db);
    let over = weight > inv.max_weight;
    let tab_labels: Vec<&str> = TABS.iter().map(|t| t.0).collect();
    let bag = El::new()
        .class("panel")
        .id("inv_bag")
        .child(widgets::header(
            Some("backpack"),
            "Inventory",
            &format!("{} of {} slots used", inv.used(), inv.slots.len()),
            true,
        ))
        .child(
            El::new()
                .class("row")
                .child(widgets::tabs(&tab_labels, st.tab, "inv.tab"))
                .child(El::new().class("spacer"))
                .child(
                    El::new()
                        .class("icon_button")
                        .id("inv_sort")
                        .on_click("inv.sort")
                        .tooltip("Sort", "Group by category and rarity, and merge stacks.")
                        .child(
                            El::icon("ui_sort")
                                .with_style(|s| s.size(16.0, 16.0))
                                .pick_ignore(),
                        ),
                ),
        )
        .child(grid)
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(18.0).margin([4.0, 0.0, 0.0, 0.0]))
                .child(
                    El::new()
                        .class("col")
                        .with_style(|s| s.gap(5.0).grow(1.0))
                        .child(
                            El::new()
                                .class("row")
                                .child(
                                    El::text("Weight")
                                        .class("caption")
                                        .with_style(|s| s.grow(1.0)),
                                )
                                .child(
                                    El::text(format!("{weight:.1} / {:.0} kg", inv.max_weight))
                                        .class("caption")
                                        .text_style(|t| t.tabular().nowrap())
                                        .with_style(|s| if over { s.color(p.bad) } else { s }),
                                ),
                        )
                        .child(
                            El::new()
                                .class("bar_track")
                                .with_style(|s| s.width(pct(100.0)).height(8.0))
                                .paint("bar")
                                .attach(
                                    "Bar",
                                    &widgets::Bar {
                                        value: (weight / inv.max_weight.max(0.01)).min(1.0),
                                        color: if over { p.bad } else { p.info.scale(0.8) },
                                        color2: if over { p.warn } else { p.info },
                                        ..Default::default()
                                    },
                                ),
                        ),
                )
                .child(
                    El::new()
                        .class("slot")
                        .id("inv_trash")
                        .drop_target("trash", &["inv:"])
                        .tooltip("Drop", "Drag an item here to drop it.")
                        .with_style(|s| s.size(44.0, 44.0).padding(11.0))
                        .hover(StylePatch::default().border_color(p.bad).color(p.bad))
                        .child(
                            El::icon("ui_trash")
                                .with_style(|s| s.size(pct(100.0), pct(100.0)))
                                .pick_ignore(),
                        ),
                )
                .child(widgets::currency(&group(inv.gold), 22.0).id("inv_gold")),
        );

    let details = El::new()
        .class("panel")
        .id("inv_details")
        .with_style(|s| s.width(360.0))
        .child(details(world, st));

    El::new()
        .with_style(|s| {
            s.fill_parent()
                .column()
                .center()
                .gap(18.0)
                .pick(Pick::Ignore)
        })
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(18.0).align_items(Align::Stretch))
                .child(equipment)
                .child(bag)
                .child(details),
        )
        .child(hints())
}

/// The details panel's content for the shown (hovered/focused) or selected slot.
fn details(world: &World, st: &InventoryScreen) -> El {
    let theme = world.resource::<Theme>().clone();
    let db = world.resource::<ItemDatabase>().clone();
    let inv = world
        .get::<Inventory>(st.owner)
        .cloned()
        .unwrap_or_default();
    let shown = st.shown.or(st.selected);
    let stack = shown.and_then(|r| r.stack(&inv)).cloned();
    let compare = stack
        .as_ref()
        .and_then(|s| db.get(&s.item))
        .and_then(|d| d.slot)
        .and_then(|slot| inv.equipment.get(&slot))
        .cloned();
    let mut actions = Vec::new();
    if let (Some(s), Some(r)) = (&stack, shown)
        && let Some(def) = db.get(&s.item)
    {
        match (r, def.category, def.slot) {
            (SlotRef::Bag(_), ItemCategory::Consumable, _) => {
                actions.push(
                    El::button("Use", "inv.use")
                        .class("primary")
                        .id("inv_use")
                        .with_style(|s| s.grow(1.0)),
                );
            }
            (SlotRef::Bag(_), _, Some(_)) => {
                actions.push(
                    El::button("Equip", "inv.use")
                        .class("primary")
                        .id("inv_use")
                        .with_style(|s| s.grow(1.0)),
                );
            }
            (SlotRef::Equip(_), _, _) => {
                actions.push(
                    El::button("Unequip", "inv.use")
                        .id("inv_use")
                        .with_style(|s| s.grow(1.0)),
                );
            }
            _ => {}
        }
        if matches!(r, SlotRef::Bag(_)) && def.category != ItemCategory::Quest {
            actions.push(El::button("Drop", "inv.drop").class("ghost").id("inv_drop"));
        }
    }
    item_details(&theme, &db, stack.as_ref(), compare.as_ref(), None, actions)
}

/// Builder for `ui.show {"screen": "inventory"}`.
pub fn build_screen(world: &mut World, args: &Value) -> Result<Entity, String> {
    let owner = find_owner(world, args)?;
    let st = InventoryScreen {
        owner,
        tab: args.get("tab").and_then(Value::as_u64).unwrap_or(0) as usize,
        selected: None,
        shown: None,
        dirty: false,
        snapshot: world.get::<Inventory>(owner).cloned(),
    };
    let content = build(world, &st);
    let el = El::new()
        .class("screen")
        .child(El::new().with_style(|_| widgets::scrim(0.72)))
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
    let Some(st) = world.get::<InventoryScreen>(root).cloned() else {
        return;
    };
    let content = build(world, &st);
    let children: Vec<Entity> = world
        .get::<Children>(root)
        .map(|c| c.iter().collect())
        .unwrap_or_default();
    // Keep the scrim (first child); replace the content.
    for c in children.iter().skip(1) {
        if let Ok(e) = world.get_entity_mut(*c) {
            e.despawn();
        }
    }
    let _ = el::spawn(world, Some(root), &content);
    let snapshot = world.get::<Inventory>(st.owner).cloned();
    if let Some(mut s) = world.get_mut::<InventoryScreen>(root) {
        s.dirty = false;
        s.snapshot = snapshot;
    }
}

fn refresh_details(world: &mut World, root: Entity) {
    let Some(st) = world.get::<InventoryScreen>(root).cloned() else {
        return;
    };
    if let Some(panel) = screens::find_in(world, root, "inv_details") {
        let content = details(world, &st);
        let _ = el::replace_children(world, panel, &[content]);
    }
}

/// Use, equip or unequip what is in `r`.
pub fn use_slot(world: &mut World, owner: Entity, r: SlotRef) -> Result<String, String> {
    let db = world.resource::<ItemDatabase>().clone();
    let mut inv = world
        .get::<Inventory>(owner)
        .cloned()
        .ok_or("No inventory.")?;
    let stack = r.stack(&inv).cloned().ok_or("That slot is empty.")?;
    let def = db.get(&stack.item).cloned().ok_or("Unknown item.")?;
    let msg = match r {
        SlotRef::Equip(slot) => {
            inv.unequip(&db, slot, None)?;
            format!("Unequipped {}", def.name)
        }
        SlotRef::Bag(i) => {
            if def.category == ItemCategory::Consumable {
                inv.take(i, 1);
                if let Some(heal) = def.stats.get("heal") {
                    let mut data = world.resource_mut::<crate::data::UiData>();
                    if let (Some(h), Some(m)) = (
                        data.get_f64("player.health"),
                        data.get_f64("player.health_max"),
                    ) {
                        data.set("player.health", (h + *heal as f64).min(m));
                    }
                }
                world.write_message(UiEvent::Action {
                    action: format!("inventory.used:{}", stack.item),
                    id: None,
                    alt: false,
                    entity: None,
                });
                format!("Used {}", def.name)
            } else if def.slot.is_some() {
                inv.equip(&db, i)?;
                format!("Equipped {}", def.name)
            } else {
                return Err(format!("{} can't be used.", def.name));
            }
        }
    };
    *world.get_mut::<Inventory>(owner).unwrap() = inv;
    Ok(msg)
}

/// Handle input for open inventory screens and refresh them when their inventory changes.
pub fn inventory_screen_system(world: &mut World, mut seen: Local<MessageCursor<UiEvent>>) {
    let events: Vec<UiEvent> = crate::input::read_events(world, &mut seen);
    let mut q = world.query::<(Entity, &InventoryScreen, &Screen)>();
    let screens: Vec<(Entity, InventoryScreen, String)> = q
        .iter(world)
        .filter(|(_, _, s)| !s.closing)
        .map(|(e, st, s)| (e, st.clone(), s.key.clone()))
        .collect();
    for (root, mut st, key) in screens {
        if world.get::<Inventory>(st.owner).is_none() {
            screens::close_screen(world, root, false);
            continue;
        }
        let db = world.resource::<ItemDatabase>().clone();
        let mut details_only = false;
        let mut dirty = false;
        for ev in &events {
            match ev {
                UiEvent::Action {
                    action,
                    alt,
                    entity,
                    ..
                } => {
                    if entity.is_some_and(|e| !screens::is_descendant(world, e, root)) {
                        continue;
                    }
                    if let Some(t) = action.strip_prefix("inv.tab:") {
                        st.tab = t.parse().unwrap_or(0);
                        dirty = true;
                    } else if let Some(r) =
                        action.strip_prefix("inv.select:").and_then(SlotRef::parse)
                    {
                        if *alt {
                            match use_slot(world, st.owner, r) {
                                Ok(m) => toast::notify(world, ToastKind::Good, "", "", &m, 2.2),
                                Err(e) => toast::notify(world, ToastKind::Bad, "", "", &e, 2.5),
                            }
                        } else {
                            st.selected = if st.selected == Some(r) {
                                None
                            } else {
                                Some(r)
                            };
                            st.shown = Some(r);
                        }
                        dirty = true;
                    } else if action == "inv.sort" {
                        if let Some(mut inv) = world.get_mut::<Inventory>(st.owner) {
                            inv.sort(&db);
                        }
                        st.selected = None;
                        dirty = true;
                    } else if action == "inv.use" {
                        if let Some(r) = st.shown.or(st.selected) {
                            match use_slot(world, st.owner, r) {
                                Ok(m) => toast::notify(world, ToastKind::Good, "", "", &m, 2.2),
                                Err(e) => toast::notify(world, ToastKind::Bad, "", "", &e, 2.5),
                            }
                        }
                        dirty = true;
                    } else if action == "inv.drop"
                        && let Some(SlotRef::Bag(i)) = st.shown.or(st.selected)
                    {
                        drop_item(world, st.owner, i);
                        st.selected = None;
                        st.shown = None;
                        dirty = true;
                    }
                }
                UiEvent::Tab { screen, delta } if *screen == key => {
                    st.tab = (st.tab as i32 + delta).rem_euclid(TABS.len() as i32) as usize;
                    dirty = true;
                }
                UiEvent::Highlighted { id, .. } => {
                    let r = id.as_deref().and_then(|id| {
                        id.strip_prefix("inv_slot:")
                            .and_then(|i| i.parse().ok())
                            .map(SlotRef::Bag)
                            .or_else(|| {
                                id.strip_prefix("equip_slot:")
                                    .and_then(EquipSlot::from_key)
                                    .map(SlotRef::Equip)
                            })
                    });
                    if let Some(r) = r
                        && st.shown != Some(r)
                    {
                        st.shown = Some(r);
                        details_only = true;
                    }
                }
                UiEvent::Dropped {
                    payload,
                    slot,
                    target,
                    ..
                } => {
                    if target.is_some_and(|t| !screens::is_descendant(world, t, root)) {
                        continue;
                    }
                    let Some(from) = SlotRef::parse(payload) else {
                        continue;
                    };
                    if slot == "trash" {
                        if let SlotRef::Bag(i) = from {
                            drop_item(world, st.owner, i);
                        }
                        dirty = true;
                        continue;
                    }
                    let Some(to) = SlotRef::parse(slot) else {
                        continue;
                    };
                    match move_between(world, st.owner, from, to) {
                        Ok(Some(m)) => toast::notify(world, ToastKind::Good, "", "", &m, 2.0),
                        Ok(None) => {}
                        Err(e) => toast::notify(world, ToastKind::Bad, "", "", &e, 2.5),
                    }
                    st.selected = Some(to);
                    st.shown = Some(to);
                    dirty = true;
                }
                _ => {}
            }
        }
        // The inventory changed from outside (the game, the API).
        if world.get::<Inventory>(st.owner).cloned() != st.snapshot {
            dirty = true;
        }
        if let Some(mut s) = world.get_mut::<InventoryScreen>(root) {
            *s = st;
        }
        if dirty {
            rebuild(world, root);
        } else if details_only {
            refresh_details(world, root);
        }
    }
}

fn drop_item(world: &mut World, owner: Entity, i: usize) {
    let db = world.resource::<ItemDatabase>().clone();
    let Some(stack) = world
        .get::<Inventory>(owner)
        .and_then(|inv| inv.slots.get(i).cloned().flatten())
    else {
        return;
    };
    if db
        .get(&stack.item)
        .is_some_and(|d| d.category == ItemCategory::Quest)
    {
        toast::notify(
            world,
            ToastKind::Warn,
            "",
            "",
            "Quest items can't be dropped.",
            2.5,
        );
        return;
    }
    if let Some(mut inv) = world.get_mut::<Inventory>(owner) {
        inv.slots[i] = None;
    }
    let name = db
        .get(&stack.item)
        .map(|d| d.name.clone())
        .unwrap_or(stack.item.clone());
    world.write_message(UiEvent::Action {
        action: format!("inventory.dropped:{}:{}", stack.item, stack.count),
        id: None,
        alt: false,
        entity: None,
    });
    toast::notify(
        world,
        ToastKind::Info,
        "ui_trash",
        "",
        &format!("Dropped {name}"),
        2.0,
    );
}

/// Move an item between two slots (bag ↔ bag, bag → equipment, equipment → bag).
pub fn move_between(
    world: &mut World,
    owner: Entity,
    from: SlotRef,
    to: SlotRef,
) -> Result<Option<String>, String> {
    let db = world.resource::<ItemDatabase>().clone();
    let mut inv = world
        .get::<Inventory>(owner)
        .cloned()
        .ok_or("No inventory.")?;
    let msg = match (from, to) {
        (SlotRef::Bag(a), SlotRef::Bag(b)) => {
            inv.move_slot(&db, a, b);
            None
        }
        (SlotRef::Bag(a), SlotRef::Equip(slot)) => {
            let item = inv.slots[a].clone().ok_or("That slot is empty.")?;
            let def = db.get(&item.item).ok_or("Unknown item.")?;
            if def.slot != Some(slot) {
                return Err(match def.slot {
                    Some(s) => format!(
                        "{} goes in the {} slot.",
                        def.name,
                        s.label().to_lowercase()
                    ),
                    None => format!("{} can't be equipped.", def.name),
                });
            }
            inv.equip(&db, a)?;
            Some(format!("Equipped {}", def.name))
        }
        (SlotRef::Equip(slot), SlotRef::Bag(b)) => {
            let name = inv
                .equipment
                .get(&slot)
                .and_then(|s| db.get(&s.item))
                .map(|d| d.name.clone())
                .unwrap_or_default();
            match inv.slots.get(b) {
                Some(None) => inv.unequip(&db, slot, Some(b))?,
                Some(Some(other)) => {
                    // Swap with an item that fits this slot, otherwise just unequip.
                    let fits = db.get(&other.item).and_then(|d| d.slot) == Some(slot);
                    if fits {
                        inv.equip(&db, b)?;
                    } else {
                        inv.unequip(&db, slot, None)?;
                    }
                }
                None => return Err("No such slot.".into()),
            }
            Some(format!("Unequipped {name}"))
        }
        (SlotRef::Equip(_), SlotRef::Equip(_)) => None,
    };
    *world.get_mut::<Inventory>(owner).unwrap() = inv;
    Ok(msg)
}

/// Register the `inventory.*` API.
pub fn register_api(app: &mut engine_core::prelude::App) {
    app.register_api(
        "inventory.give",
        "Give items to an inventory (default: the Player's). Returns how many didn't fit.",
        r#"{"item": "potion_health", "count": 3, "owner": "Player"}"#,
        |world, args| {
            let owner = find_owner(world, &args)?;
            let item = args
                .get("item")
                .and_then(Value::as_str)
                .ok_or("Pass \"item\" (an ItemDatabase id).")?;
            let count = args.get("count").and_then(Value::as_u64).unwrap_or(1) as u32;
            let db = world.resource::<ItemDatabase>().clone();
            if db.get(item).is_none() {
                let ids: Vec<&String> = db.items.keys().take(12).collect();
                return Err(format!(
                    "There is no item \"{item}\". Some ids: {ids:?}. Add new ones to ItemDatabase."
                ));
            }
            let left = world
                .get_mut::<Inventory>(owner)
                .unwrap()
                .add(&db, item, count);
            Ok(json!({"added": count - left, "left_over": left}))
        },
    );
    app.register_api(
        "inventory.list",
        "What an inventory holds: bag slots, equipment and gold.",
        r#"{"owner": "Player"}"#,
        |world, args| {
            let owner = find_owner(world, &args)?;
            serde_json::to_value(world.get::<Inventory>(owner).unwrap()).map_err(|e| e.to_string())
        },
    );
    app.register_api(
        "inventory.use",
        "Use, equip or unequip a slot: {\"slot\": \"inv:3\"} or {\"slot\": \"equip:main_hand\"}.",
        r#"{"slot": "inv:0"}"#,
        |world, args| {
            let owner = find_owner(world, &args)?;
            let slot = args
                .get("slot")
                .and_then(Value::as_str)
                .ok_or("Pass \"slot\", e.g. \"inv:0\".")?;
            let r = SlotRef::parse(slot).ok_or("Slots look like \"inv:3\" or \"equip:head\".")?;
            Ok(json!({"result": use_slot(world, owner, r)?}))
        },
    );
    app.register_api(
        "inventory.move",
        "Move an item as a drag and drop would: {\"from\": \"inv:0\", \"to\": \"equip:main_hand\"}.",
        r#"{"from": "inv:0", "to": "inv:5"}"#,
        |world, args| {
            let owner = find_owner(world, &args)?;
            let parse = |k: &str| {
                args.get(k)
                    .and_then(Value::as_str)
                    .and_then(SlotRef::parse)
                    .ok_or_else(|| format!("Pass \"{k}\" like \"inv:3\" or \"equip:head\"."))
            };
            let (from, to) = (parse("from")?, parse("to")?);
            Ok(json!({"result": move_between(world, owner, from, to)?}))
        },
    );
    app.register_api(
        "inventory.sort",
        "Sort an inventory by category and rarity.",
        r#"{"owner": "Player"}"#,
        |world, args| {
            let owner = find_owner(world, &args)?;
            let db = world.resource::<ItemDatabase>().clone();
            world.get_mut::<Inventory>(owner).unwrap().sort(&db);
            Ok(json!({"ok": true}))
        },
    );
    app.register_api(
        "inventory.items",
        "Every item id in the ItemDatabase with its name, category, rarity and price.",
        "{}",
        |world, _| {
            let db = world.resource::<ItemDatabase>();
            Ok(Value::Array(
                db.items
                    .iter()
                    .map(|(id, d)| json!({"id": id, "name": d.name, "category": d.category, "rarity": d.rarity, "value": d.value}))
                    .collect(),
            ))
        },
    );
}
