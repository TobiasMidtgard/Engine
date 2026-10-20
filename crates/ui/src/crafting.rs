//! The crafting bench: recipes by station (alchemy, smithing, tinkering) turned into items
//! from what is in the bag. Pick a recipe, see what it makes and what it needs, choose how
//! many, and craft: the result builds up in a ring of light and lands in the bag.
//!
//! ```ignore
//! ui::show(world, "crafting", &json!({"station": "alchemy", "select": "r_potion_mana"}))?;
//! ```
//!
//! Recipes live in the [`RecipeBook`] resource (`craft.recipes` reads or replaces it,
//! `craft.learn` teaches one, `craft.make` crafts at once). Ingredients come out of the
//! owner's [`Inventory`] only when a craft finishes, and each finished craft sends
//! `craft.made:<recipe id>`.

use bevy_ecs::message::MessageCursor;
use bevy_ecs::prelude::*;
use engine_core::prelude::*;
use engine_input::{ButtonInput, GamepadButton, KeyCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::color::Color;
use crate::draw::{DrawCtx, Painters, flag};
use crate::el::{self, El};
use crate::input::UiEvent;
use crate::inventory::{self, item_slot};
use crate::items::*;
use crate::node::*;
use crate::screens;
use crate::style::{Align, Corners, Fill, Pick, Shadow, pct};
use crate::theme::{Palette, Theme};
use crate::toast::{self, ToastKind};
use crate::widgets;

// -------------------------------------------------------------------------------- recipes

/// Where a recipe is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Station {
    #[default]
    Alchemy,
    Smithing,
    Tinkering,
}

impl Station {
    pub const ALL: [Station; 3] = [Station::Alchemy, Station::Smithing, Station::Tinkering];

    pub fn label(self) -> &'static str {
        match self {
            Station::Alchemy => "Alchemy",
            Station::Smithing => "Smithing",
            Station::Tinkering => "Tinkering",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Station::Alchemy => "alchemy",
            Station::Smithing => "smithing",
            Station::Tinkering => "tinkering",
        }
    }

    pub fn from_key(k: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.key() == k)
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    /// The light of the bench: a green brew, forge orange, tinker blue.
    pub fn color(self) -> Color {
        match self {
            Station::Alchemy => Color::hex(0x5fd39a),
            Station::Smithing => Color::hex(0xf08a3c),
            Station::Tinkering => Color::hex(0x4cb8f0),
        }
    }
}

/// `count` of `item`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ingredient {
    pub item: String,
    pub count: u32,
}

/// One recipe: what goes in, what comes out, where and how long.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Recipe {
    pub id: String,
    /// The item made.
    pub result: String,
    /// How many one craft makes.
    pub count: u32,
    pub station: Station,
    pub ingredients: Vec<Ingredient>,
    /// Seconds for one craft.
    pub time: f32,
    /// Unknown recipes show as a silhouette until learned.
    pub known: bool,
    /// Where an unknown recipe is learned.
    pub hint: String,
}

impl Default for Recipe {
    fn default() -> Self {
        Self {
            id: String::new(),
            result: String::new(),
            count: 1,
            station: Station::Alchemy,
            ingredients: Vec::new(),
            time: 1.2,
            known: true,
            hint: String::new(),
        }
    }
}

impl Recipe {
    /// How many times this can be made from `inv`.
    pub fn can_make(&self, inv: &Inventory) -> u32 {
        if !self.known {
            return 0;
        }
        self.ingredients
            .iter()
            .map(|i| inv.count_of(&i.item) / i.count.max(1))
            .min()
            .unwrap_or(0)
    }

    /// The first ingredient short in `inv`, as "Missing 1 Oak Log".
    pub fn missing(&self, inv: &Inventory, db: &ItemDatabase) -> Option<String> {
        let i = self
            .ingredients
            .iter()
            .find(|i| inv.count_of(&i.item) < i.count)?;
        let name = db.get(&i.item).map_or(i.item.as_str(), |d| d.name.as_str());
        Some(format!(
            "Missing {} {name}",
            i.count - inv.count_of(&i.item)
        ))
    }
}

/// Every recipe in the game.
#[derive(Resource, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RecipeBook {
    pub recipes: Vec<Recipe>,
}

impl RecipeBook {
    pub fn get(&self, id: &str) -> Option<&Recipe> {
        self.recipes.iter().find(|r| r.id == id)
    }

    pub fn at(&self, station: Station) -> impl Iterator<Item = &Recipe> {
        self.recipes.iter().filter(move |r| r.station == station)
    }
}

/// Take `n` of `item` out of the bag, smallest stacks first.
fn remove_items(inv: &mut Inventory, item: &str, mut n: u32) {
    let mut slots: Vec<usize> = inv
        .slots
        .iter()
        .enumerate()
        .filter(|(_, s)| s.as_ref().is_some_and(|s| s.item == item))
        .map(|(i, _)| i)
        .collect();
    slots.sort_by_key(|&i| inv.slots[i].as_ref().map_or(0, |s| s.count));
    for i in slots {
        if n == 0 {
            break;
        }
        if let Some(s) = inv.take(i, n) {
            n -= s.count;
        }
    }
}

/// Craft `qty` of recipe `id` for `owner` now. Returns a message for a toast.
pub fn craft(world: &mut World, owner: Entity, id: &str, qty: u32) -> Result<String, String> {
    let db = world.resource::<ItemDatabase>().clone();
    let r = world
        .get_resource::<RecipeBook>()
        .and_then(|b| b.get(id))
        .cloned()
        .ok_or_else(|| format!("There is no recipe \"{id}\"."))?;
    if !r.known {
        return Err("You don't know that recipe yet.".into());
    }
    let mut inv = world
        .get::<Inventory>(owner)
        .cloned()
        .ok_or("That character has no inventory.")?;
    let qty = qty.max(1);
    if r.can_make(&inv) < qty {
        return Err(r
            .missing(&inv, &db)
            .unwrap_or_else(|| "Not enough ingredients.".into()));
    }
    for i in &r.ingredients {
        remove_items(&mut inv, &i.item, i.count * qty);
    }
    let made = r.count * qty;
    if inv.add(&db, &r.result, made) > 0 {
        return Err("Your bag is full.".into());
    }
    world.entity_mut(owner).insert(inv);
    crate::input::dispatch(world, &format!("craft.made:{id}"));
    let name = db
        .get(&r.result)
        .map_or(r.result.clone(), |d| d.name.clone());
    Ok(if made > 1 {
        format!("[b]{name}[/b] ×{made}")
    } else {
        format!("[b]{name}[/b]")
    })
}

/// Teach recipe `id`. Returns false when it was already known (or doesn't exist).
pub fn learn(world: &mut World, id: &str) -> bool {
    let Some(mut book) = world.get_resource_mut::<RecipeBook>() else {
        return false;
    };
    let Some(r) = book.recipes.iter_mut().find(|r| r.id == id) else {
        return false;
    };
    if r.known {
        return false;
    }
    r.known = true;
    let result = r.result.clone();
    let db = world.resource::<ItemDatabase>();
    let (name, icon) = db
        .get(&result)
        .map_or((result.clone(), String::new()), |d| {
            (d.name.clone(), d.icon.clone())
        });
    toast::notify(
        world,
        ToastKind::Good,
        &icon,
        "Recipe learned",
        &format!("[b]{name}[/b]"),
        2.6,
    );
    true
}

// -------------------------------------------------------------------------------- result painter

/// The result showcase (on the node with `paint: "craft_result"`).
#[derive(Component, Debug, Clone, Default, PartialEq)]
pub struct CraftView {
    pub icon: String,
    pub color: Color,
    pub station: Color,
    pub known: bool,
    /// 0-1 while a craft is under way.
    pub progress: Option<f32>,
    /// Seconds since the last craft finished (a burst of light), counted down from 1.
    pub flash: f32,
    /// Real seconds, for the slow turn of the outer ring.
    pub time: f32,
}

fn paint_result(world: &mut World, e: Entity, ctx: &mut DrawCtx) {
    let Some(v) = world.get::<CraftView>(e).cloned() else {
        return;
    };
    let px = ctx.px;
    let rect = ctx.rect;
    let c = rect.center();
    let r = rect.w.min(rect.h) * 0.5;
    let tau = std::f32::consts::TAU;
    let col = if v.known {
        v.color
    } else {
        Color::hex(0x5a6478)
    };
    let busy = v.progress.is_some();
    let pulse = 0.5 + 0.5 * (v.time * 2.4).sin();

    // The bench's light pooled behind the item, then the item's rarity light on top.
    ctx.glow(c, r * 1.25, v.station.with_alpha(0.10), 1.4);
    ctx.glow(
        c,
        r * 0.95,
        col.with_alpha(if busy { 0.22 + 0.10 * pulse } else { 0.16 }),
        1.6,
    );
    // A turning dashed ring and a fine inner ring.
    ctx.ring(
        c,
        Vec2::splat(r * 0.94),
        1.2 * px,
        col.with_alpha(0.35),
        v.time * 0.12,
        tau,
        48.0,
    );
    ctx.ring(
        c,
        Vec2::splat(r * 0.86),
        1.0 * px,
        Color::WHITE.with_alpha(0.08),
        0.0,
        tau,
        0.0,
    );
    // The well: a dark disc lit from the middle in the rarity color.
    let d = r * 0.74;
    let well = Rect::new(c.x - d, c.y - d, d * 2.0, d * 2.0);
    ctx.shadow(well, d, 18.0 * px, Color::hexa(0x000000a0));
    ctx.rect_fill(
        well,
        Fill::Radial {
            inner: col.lerp(Color::hexa(0x0b111cff), 0.62),
            outer: Color::hexa(0x080c14ff),
        },
        [d; 4],
        1.5 * px,
        col.with_alpha(0.55),
    );
    // Progress: a bright arc filling clockwise with a spark at its head.
    if let Some(t) = v.progress {
        let rr = Vec2::splat(r * 0.80);
        ctx.ring(c, rr, 6.0 * px, Color::hexa(0x04070dd0), 0.0, tau, 0.0);
        ctx.ring(c, rr, 9.0 * px, col.with_alpha(0.18), 0.0, tau * t, 0.0);
        ctx.ring(
            c,
            rr,
            3.5 * px,
            col.lerp(Color::WHITE, 0.35),
            0.0,
            tau * t,
            0.0,
        );
        let a = tau * t;
        let head = c + Vec2::new(a.sin(), -a.cos()) * rr.x;
        ctx.glow(
            head,
            18.0 * px,
            col.lerp(Color::WHITE, 0.5).with_alpha(0.8),
            2.0,
        );
    }
    // The item, a dark silhouette while unknown.
    let is = d * 1.2;
    let n0 = ctx.list.instances.len();
    let bob = if busy {
        (v.time * 5.0).sin() * 2.0 * px
    } else {
        0.0
    };
    ctx.icon(
        &v.icon,
        Rect::new(c.x - is * 0.5, c.y - is * 0.5 + bob, is, is),
        if v.known {
            Color::WHITE
        } else {
            Color::hexa(0x0a0e16e0)
        },
    );
    if !v.known {
        for inst in &mut ctx.list.instances[n0..] {
            inst.kind[1] |= flag::DESATURATE;
        }
        let l = 34.0 * px;
        ctx.icon(
            "ui_lock",
            Rect::new(c.x - l * 0.5, c.y - l * 0.5, l, l),
            Color::WHITE.with_alpha(0.5),
        );
    }
    // A burst when a craft lands.
    if v.flash > 0.0 {
        let k = 1.0 - v.flash;
        ctx.glow(c, r * (0.6 + k * 0.7), col.with_alpha(0.55 * v.flash), 1.2);
        ctx.ring(
            c,
            Vec2::splat(r * (0.74 + k * 0.4)),
            (4.0 - 3.0 * k) * px,
            col.lerp(Color::WHITE, 0.6).with_alpha(v.flash),
            0.0,
            tau,
            0.0,
        );
    }
}

pub fn add_painters(p: &mut Painters) {
    p.add("craft_result", paint_result);
}

// -------------------------------------------------------------------------------- screen

/// A craft under way.
#[derive(Debug, Clone, PartialEq)]
pub struct CraftJob {
    pub recipe: String,
    pub qty: u32,
    pub elapsed: f32,
    pub total: f32,
}

/// The open crafting screen (on its root).
#[derive(Component, Debug, Clone)]
pub struct CraftingScreen {
    pub owner: Entity,
    pub station: Station,
    pub selected: Option<String>,
    pub quantity: u32,
    pub job: Option<CraftJob>,
    pub flash: f32,
    pub snapshot: Option<(Inventory, RecipeBook)>,
}

fn snapshot(world: &World, st: &CraftingScreen) -> Option<(Inventory, RecipeBook)> {
    Some((
        world.get::<Inventory>(st.owner)?.clone(),
        world.get_resource::<RecipeBook>()?.clone(),
    ))
}

/// How long `qty` crafts take: each after the first goes faster, and never more than 6 s.
fn job_time(r: &Recipe, qty: u32) -> f32 {
    (r.time * (1.0 + 0.45 * (qty.max(1) - 1) as f32)).min(6.0)
}

fn rarity_name(r: u8) -> &'static str {
    ["Common", "Uncommon", "Rare", "Epic", "Legendary"][r.min(4) as usize]
}

fn recipe_row(theme: &Theme, db: &ItemDatabase, inv: &Inventory, r: &Recipe, selected: bool) -> El {
    let p = &theme.palette;
    let def = db.get(&r.result);
    let rc = def.map_or(p.text, |d| theme.rarity(d.rarity as usize));
    let n = r.can_make(inv);
    let mut slot = if r.known {
        item_slot(theme, db, Some(&ItemStack::new(&r.result, r.count)), 54.0)
    } else {
        El::new()
            .class("slot")
            .with_style(|s| s.size(54.0, 54.0))
            .child(
                El::icon(def.map_or("ui_lock".to_string(), |d| d.icon.clone()))
                    .icon_tint(Color::hexa(0x0a0e16d8))
                    .with_style(|s| s.size(pct(100.0), pct(100.0))),
            )
    };
    slot.tooltip = None;
    let name = if r.known {
        def.map_or(r.result.clone(), |d| d.name.clone())
    } else {
        "Unknown recipe".to_string()
    };
    let (status, status_color) = if !r.known {
        (r.hint.clone(), p.text_faint)
    } else if n > 0 {
        (format!("Can make {n}"), p.good)
    } else {
        (
            r.missing(inv, db).unwrap_or_default(),
            p.bad.lerp(p.text_dim, 0.25),
        )
    };
    // On the lit (selected) card, text needs more light to stay readable.
    let status_color = if selected {
        status_color.lerp(Color::WHITE, 0.6)
    } else {
        status_color
    };
    let mut row = El::new()
        .class("card")
        .id(format!("recipe:{}", r.id))
        .on_click(format!("craft.select:{}", r.id))
        .selected(selected)
        .focusable(true)
        .with_style(|s| {
            s.row()
                .align_items(Align::Center)
                .gap(12.0)
                .padding([7.0, 12.0, 7.0, 7.0])
                .width(pct(100.0))
        })
        .child(slot.pick_ignore())
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(3.0).grow(1.0).shrink(1.0))
                .child(
                    El::text(name)
                        .text_style(|t| t.semibold().nowrap())
                        .with_style(|s| {
                            s.font_size(15.0).color(if selected {
                                Color::WHITE
                            } else if r.known {
                                rc.lerp(Color::WHITE, 0.25)
                            } else {
                                p.text_dim
                            })
                        })
                        .pick_ignore(),
                )
                .child(
                    El::text(status)
                        .text_style(|t| t.nowrap())
                        .with_style(|s| s.font_size(13.0).color(status_color))
                        .pick_ignore(),
                ),
        );
    if n > 0 {
        row = row.child(
            El::new()
                .with_style(|s| {
                    s.size(8.0, 8.0)
                        .radius(Corners::all(4.0))
                        .bg(p.good)
                        .shadow(Shadow::glow(p.good.fade(0.6), 8.0))
                })
                .pick_ignore(),
        );
    } else if !r.known {
        row = row.with_style(|s| s.opacity(0.72));
    }
    row
}

fn ingredient_card(
    theme: &Theme,
    db: &ItemDatabase,
    inv: &Inventory,
    i: &Ingredient,
    qty: u32,
) -> El {
    let p = &theme.palette;
    let need = i.count * qty.max(1);
    let have = inv.count_of(&i.item);
    let ok = have >= need;
    let name = db.get(&i.item).map_or(i.item.clone(), |d| d.name.clone());
    let frac = (have as f32 / need.max(1) as f32).min(1.0);
    let bar_col = if ok { p.good } else { p.bad };
    El::new()
        .class("col")
        .id(format!("ingredient:{}", i.item))
        .with_style(|s| {
            s.width(138.0)
                .gap(6.0)
                .padding([10.0, 10.0])
                .align_items(Align::Center)
                .radius(Corners::all(12.0))
                .bg(Color::WHITE.with_alpha(0.035))
                .border(1.0, if ok { p.line } else { p.bad.with_alpha(0.55) })
        })
        .child(item_slot(
            theme,
            db,
            Some(&ItemStack::new(&i.item, 1)),
            60.0,
        ))
        .child(
            El::text(name)
                .text_style(|t| t.semibold().nowrap().center())
                .with_style(|s| s.font_size(13.0).width(pct(100.0)))
                .pick_ignore(),
        )
        .child(
            El::text(format!(
                "[color={}]{have}[/color] / {need}",
                if ok { p.good } else { p.bad }.to_hex()
            ))
            .text_style(|t| t.tabular().nowrap().center().semibold())
            .with_style(|s| s.font_size(14.0).color(p.text_dim))
            .pick_ignore(),
        )
        .child(
            El::new()
                .with_style(|s| {
                    s.width(pct(100.0))
                        .height(4.0)
                        .radius(Corners::all(2.0))
                        .bg(Color::WHITE.with_alpha(0.08))
                })
                .child(El::new().with_style(|s| {
                    s.width(pct(frac * 100.0))
                        .height(pct(100.0))
                        .radius(Corners::all(2.0))
                        .bg(bar_col.lerp(Color::WHITE, 0.15))
                }))
                .pick_ignore(),
        )
}

fn details(world: &World, st: &CraftingScreen) -> El {
    let theme = world.resource::<Theme>().clone();
    let p = theme.palette.clone();
    let db = world.resource::<ItemDatabase>().clone();
    let inv = world
        .get::<Inventory>(st.owner)
        .cloned()
        .unwrap_or_default();
    let book = world
        .get_resource::<RecipeBook>()
        .cloned()
        .unwrap_or_default();
    let mut panel = El::new()
        .class("panel col")
        .id("craft_details")
        .with_style(|s| s.grow(1.0).gap(14.0));
    let Some(r) = st.selected.as_deref().and_then(|id| book.get(id)).cloned() else {
        return panel.child(El::text("Pick a recipe.").class("dim"));
    };
    let def = db.get(&r.result).cloned().unwrap_or_default();
    let rc = theme.rarity(def.rarity as usize);
    let max = r.can_make(&inv);
    let qty = st.quantity.clamp(1, max.max(1));

    // The result: the showcase on the left, words on the right.
    let mut words = El::new()
        .class("col")
        .with_style(|s| s.gap(6.0).grow(1.0).shrink(1.0))
        .child(
            El::text(if r.known {
                format!(
                    "{}  ·  {}",
                    rarity_name(def.rarity).to_uppercase(),
                    def.category.label().to_uppercase()
                )
            } else {
                format!("{}  ·  NOT YET LEARNED", r.station.label().to_uppercase())
            })
            .text_style(|t| t.bold().nowrap())
            .with_style(|s| {
                s.font_size(12.0)
                    .color(if r.known { rc } else { p.text_faint })
            }),
        )
        .child(
            El::text(if r.known {
                def.name.clone()
            } else {
                "Unknown recipe".into()
            })
            .class("title")
            .text_style(|t| t.shadow()),
        )
        .child(
            El::text(if r.known {
                def.description.clone()
            } else {
                r.hint.clone()
            })
            .class("body")
            .with_style(|s| s.color(p.text_dim)),
        );
    if r.known {
        for (k, val) in &def.stats {
            let mut label = k.replace('_', " ");
            if let Some(f) = label.get_mut(0..1) {
                f.make_ascii_uppercase();
            }
            words = words.child(widgets::stat_row(&label, &format!("{val}"), None));
        }
        let each = if r.count > 1 {
            format!("Makes {} per craft", r.count)
        } else {
            "Makes 1 per craft".into()
        };
        words = words.child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(16.0).margin([4.0, 0.0, 0.0, 0.0]))
                .child(El::text(each).class("caption"))
                .child(El::text(format!("{:.1} s each", r.time)).class("caption"))
                .child(
                    El::text(format!("In your bag: {}", inv.count_of(&r.result))).class("caption"),
                ),
        );
    }
    let canvas = El::new()
        .id("craft_result")
        .paint("craft_result")
        .with_style(|s| s.size(280.0, 280.0).shrink(0.0).pick(Pick::Ignore));
    panel = panel.child(
        El::new()
            .class("row")
            .with_style(|s| s.gap(22.0).align_items(Align::Center))
            .child(canvas)
            .child(words),
    );

    // What goes in.
    panel = panel.child(widgets::divider()).child(
        El::new()
            .class("row")
            .child(El::text("INGREDIENTS").class("label_caps"))
            .child(El::new().class("spacer"))
            .child(
                El::text(if r.known && max > 0 {
                    format!("Enough for {max}")
                } else {
                    String::new()
                })
                .class("caption"),
            ),
    );
    let mut ings = El::new()
        .class("row")
        .with_style(|s| s.gap(10.0).wrap(true));
    for i in &r.ingredients {
        ings = ings.child(ingredient_card(&theme, &db, &inv, i, qty));
    }
    panel = panel.child(ings).child(El::new().class("spacer"));

    // How many, and the button.
    let busy = st.job.is_some();
    let stepper = El::new()
        .class("row")
        .with_style(|s| s.gap(6.0))
        .child(
            El::new()
                .class("icon_button")
                .id("craft_less")
                .on_click("craft.qty:-1")
                .disabled(qty <= 1 || busy)
                .child(
                    El::icon("ui_minus")
                        .with_style(|s| s.size(14.0, 14.0))
                        .pick_ignore(),
                ),
        )
        .child(
            El::text(qty.to_string())
                .id("craft_qty")
                .text_style(|t| t.tabular().center().semibold())
                .with_style(|s| s.width(34.0).font_size(17.0)),
        )
        .child(
            El::new()
                .class("icon_button")
                .id("craft_more")
                .on_click("craft.qty:1")
                .disabled(qty >= max || busy)
                .child(
                    El::icon("ui_plus")
                        .with_style(|s| s.size(14.0, 14.0))
                        .pick_ignore(),
                ),
        )
        .child(
            El::button("Max", "craft.qty_max")
                .class("ghost")
                .id("craft_max")
                .disabled(max <= 1 || qty >= max || busy),
        );
    let blocker = if !r.known {
        Some("Learn this recipe first".to_string())
    } else if max == 0 {
        r.missing(&inv, &db)
    } else {
        None
    };
    let label = if let Some(j) = &st.job {
        format!(
            "Crafting…  {:.0}%",
            (j.elapsed / j.total.max(1e-3) * 100.0).min(100.0)
        )
    } else if qty > 1 {
        format!("Craft ×{qty}")
    } else {
        "Craft".to_string()
    };
    // While crafting the button stays lit and shows the percentage, but does nothing.
    let go = El::button(label, if busy { "craft.busy" } else { "craft.go" })
        .class(if blocker.is_none() { "primary" } else { "" })
        .id("craft_go")
        .disabled(blocker.is_some())
        .with_style(|s| s.width(300.0).height(50.0));
    panel
        .child_opt(blocker.map(|why| {
            El::text(why)
                .text_style(|t| t.nowrap())
                .with_style(|s| s.font_size(13.0).color(p.warn))
        }))
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(16.0).align_items(Align::Center))
                .child(stepper)
                .child(El::new().class("spacer"))
                .child(go),
        )
}

fn hints() -> El {
    El::new()
        .class("hud_panel row")
        .with_style(|s| {
            s.gap(22.0)
                .padding([10.0, 18.0])
                .radius(Corners::all(999.0))
        })
        .child(widgets::key_hint("LMB", "A", "Select"))
        .child(widgets::key_hint("Space", "X", "Craft"))
        .child(widgets::key_hint2(["Q", "E"], ["LB", "RB"], "Station"))
        .child(widgets::key_hint("Esc", "B", "Close"))
}

fn build(world: &World, st: &CraftingScreen) -> El {
    let theme = world.resource::<Theme>().clone();
    let p: Palette = theme.palette.clone();
    let db = world.resource::<ItemDatabase>().clone();
    let inv = world
        .get::<Inventory>(st.owner)
        .cloned()
        .unwrap_or_default();
    let book = world
        .get_resource::<RecipeBook>()
        .cloned()
        .unwrap_or_default();
    let known = book.recipes.iter().filter(|r| r.known).count();
    let tab_labels: Vec<String> = Station::ALL
        .iter()
        .map(|s| {
            let n = book.at(*s).filter(|r| r.can_make(&inv) > 0).count();
            if n > 0 {
                format!("{}  {n}", s.label())
            } else {
                s.label().to_string()
            }
        })
        .collect();
    let labels: Vec<&str> = tab_labels.iter().map(String::as_str).collect();
    let top = El::new()
        .class("row")
        .id("craft_top")
        .with_style(|s| s.gap(16.0).width(1290.0).padding([0.0, 4.0]))
        .child(
            El::new().class("header_icon").child(
                El::icon("hammer")
                    .with_style(|s| s.size(26.0, 26.0))
                    .pick_ignore(),
            ),
        )
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(2.0))
                .child(
                    El::text("Crafting")
                        .class("title")
                        .text_style(|t| t.shadow()),
                )
                .child(
                    El::text(format!(
                        "Old Brannoc's workbench  ·  {known} of {} recipes known",
                        book.recipes.len()
                    ))
                    .text_style(|t| t.nowrap().shadow())
                    .with_style(|s| s.font_size(13.0).font_weight(600).color(p.info)),
                ),
        )
        .child(El::new().class("spacer"))
        .child(widgets::tabs(&labels, st.station.index(), "craft.station"))
        .child(widgets::close_button());

    let station = st.station;
    let mut list = El::new()
        .class("col")
        .id("craft_list")
        .with_style(|s| s.gap(8.0).width(pct(100.0)));
    for r in book.at(station) {
        list = list.child(recipe_row(
            &theme,
            &db,
            &inv,
            r,
            st.selected.as_deref() == Some(r.id.as_str()),
        ));
    }
    let left = El::new()
        .class("panel col")
        .id("craft_recipes")
        .with_style(|s| {
            s.width(400.0).gap(10.0).background(Fill::Linear {
                from: station.color().with_alpha(0.10),
                to: Color::TRANSPARENT,
                angle_deg: 90.0,
            })
        })
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(10.0))
                .child(
                    El::new()
                        .with_style(|s| {
                            s.size(10.0, 10.0)
                                .radius(Corners::all(5.0))
                                .bg(station.color())
                                .shadow(Shadow::glow(station.color().fade(0.6), 10.0))
                        })
                        .pick_ignore(),
                )
                .child(
                    El::text(station.label().to_uppercase())
                        .text_style(|t| t.bold().nowrap())
                        .with_style(|s| {
                            s.font_size(14.0)
                                .color(station.color().lerp(Color::WHITE, 0.3))
                        }),
                )
                .child(El::new().class("spacer"))
                .child(
                    El::text(format!("{} recipes", book.at(station).count()))
                        .text_style(|t| t.nowrap())
                        .with_style(|s| s.font_size(13.0).color(p.text_dim)),
                ),
        )
        .child(
            El::new()
                .class("scroll")
                .id("craft_scroll")
                .with_style(|s| s.grow(1.0).padding([2.0, 6.0, 2.0, 2.0]))
                .child(list),
        );

    El::new()
        .with_style(|s| {
            s.fill_parent()
                .column()
                .center()
                .gap(16.0)
                .pick(Pick::Ignore)
        })
        .child(top)
        .child(
            El::new()
                .class("row")
                .with_style(|s| {
                    s.gap(18.0)
                        .align_items(Align::Stretch)
                        .height(640.0)
                        .width(1290.0)
                })
                .child(left)
                .child(details(world, st)),
        )
        .child(hints())
}

fn view(world: &World, st: &CraftingScreen) -> Option<CraftView> {
    let theme = world.resource::<Theme>();
    let db = world.resource::<ItemDatabase>();
    let r = world
        .get_resource::<RecipeBook>()?
        .get(st.selected.as_deref()?)?;
    let def = db.get(&r.result)?;
    Some(CraftView {
        icon: def.icon.clone(),
        color: theme.rarity(def.rarity as usize),
        station: r.station.color(),
        known: r.known,
        progress: st
            .job
            .as_ref()
            .map(|j| (j.elapsed / j.total.max(1e-3)).min(1.0)),
        flash: st.flash,
        time: 0.0,
    })
}

fn fill(world: &mut World, root: Entity, st: &CraftingScreen) -> Result<(), String> {
    let content = build(world, st);
    el::spawn(world, Some(root), &content)?;
    if let (Some(v), Some(canvas)) = (
        view(world, st),
        screens::find_in(world, root, "craft_result"),
    ) {
        world.entity_mut(canvas).insert(v);
    }
    Ok(())
}

fn rebuild(world: &mut World, root: Entity) {
    let Some(st) = world.get::<CraftingScreen>(root).cloned() else {
        return;
    };
    let scroll = screens::find_in(world, root, "craft_scroll")
        .and_then(|e| world.get::<ScrollState>(e).copied());
    let time = screens::find_in(world, root, "craft_result")
        .and_then(|e| world.get::<CraftView>(e))
        .map_or(0.0, |v| v.time);
    let children: Vec<Entity> = world
        .get::<Children>(root)
        .map(|c| c.iter().collect())
        .unwrap_or_default();
    for c in children.iter().skip(1) {
        if let Ok(e) = world.get_entity_mut(*c) {
            e.despawn();
        }
    }
    if let Err(e) = fill(world, root, &st) {
        tracing::warn!("crafting: {e}");
    }
    if let (Some(s), Some(e)) = (scroll, screens::find_in(world, root, "craft_scroll")) {
        world.entity_mut(e).insert(s);
    }
    if let Some(e) = screens::find_in(world, root, "craft_result")
        && let Some(mut v) = world.get_mut::<CraftView>(e)
    {
        v.time = time;
    }
    let snap = snapshot(world, &st);
    if let Some(mut s) = world.get_mut::<CraftingScreen>(root) {
        s.snapshot = snap;
    }
}

/// Builder for `ui.show {"screen": "crafting"}`.
pub fn build_screen(world: &mut World, args: &Value) -> Result<Entity, String> {
    let owner = inventory::find_owner(world, args)?;
    let book = world
        .get_resource::<RecipeBook>()
        .cloned()
        .unwrap_or_default();
    if book.recipes.is_empty() {
        return Err("There are no recipes (fill them with craft.recipes).".into());
    }
    let inv = world.get::<Inventory>(owner).cloned().unwrap_or_default();
    let picked = args
        .get("select")
        .and_then(Value::as_str)
        .and_then(|id| book.get(id));
    let station = picked
        .map(|r| r.station)
        .or_else(|| {
            args.get("station")
                .and_then(Value::as_str)
                .and_then(Station::from_key)
        })
        .unwrap_or_default();
    let selected = picked
        .map(|r| r.id.clone())
        .or_else(|| first_pick(&book, station, &inv));
    let mut st = CraftingScreen {
        owner,
        station,
        selected,
        quantity: 1,
        job: None,
        flash: 0.0,
        snapshot: None,
    };
    let el = El::new()
        .class("screen")
        .child(El::new().with_style(|_| widgets::scrim(0.74)));
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
    fill(world, root, &st)?;
    st.snapshot = snapshot(world, &st);
    world.entity_mut(root).insert(st);
    Ok(root)
}

/// What a station opens on: the first recipe you can make, else the first known one.
fn first_pick(book: &RecipeBook, station: Station, inv: &Inventory) -> Option<String> {
    book.at(station)
        .find(|r| r.can_make(inv) > 0)
        .or_else(|| book.at(station).find(|r| r.known))
        .or_else(|| book.at(station).next())
        .map(|r| r.id.clone())
}

/// Input for open crafting screens, the craft timer, and refreshes when the bag changes.
pub fn crafting_system(world: &mut World, mut seen: Local<MessageCursor<UiEvent>>) {
    let events: Vec<UiEvent> = crate::input::read_events(world, &mut seen);
    let dt = world.resource::<Time>().real_delta_secs().min(0.1);
    let mut q = world.query::<(Entity, &CraftingScreen, &Screen)>();
    let open: Vec<(Entity, CraftingScreen, String)> = q
        .iter(world)
        .filter(|(_, _, s)| !s.closing)
        .map(|(e, st, s)| (e, st.clone(), s.key.clone()))
        .collect();
    for (root, mut st, key) in open {
        if world.get::<Inventory>(st.owner).is_none() {
            screens::close_screen(world, root, false);
            continue;
        }
        let book = world
            .get_resource::<RecipeBook>()
            .cloned()
            .unwrap_or_default();
        let before = (
            st.station,
            st.selected.clone(),
            st.quantity,
            st.job.is_some(),
        );
        let mut go = false;
        let mut station_delta = 0i32;
        for ev in &events {
            match ev {
                UiEvent::Action { action, entity, .. } => {
                    if entity.is_some_and(|e| !screens::is_descendant(world, e, root)) {
                        continue;
                    }
                    if let Some(id) = action.strip_prefix("craft.select:") {
                        if st.selected.as_deref() != Some(id) {
                            st.quantity = 1;
                        }
                        st.selected = Some(id.to_string());
                    } else if let Some(i) = action
                        .strip_prefix("craft.station:")
                        .and_then(|i| i.parse::<usize>().ok())
                        && let Some(s) = Station::ALL.get(i)
                    {
                        if *s != st.station {
                            st.station = *s;
                            st.selected = None;
                        }
                    } else if let Some(d) = action
                        .strip_prefix("craft.qty:")
                        .and_then(|d| d.parse::<i32>().ok())
                    {
                        st.quantity = (st.quantity as i32 + d).clamp(1, 99) as u32;
                    } else if action == "craft.qty_max" {
                        st.quantity = 99;
                    } else if action == "craft.go" {
                        go = true;
                    }
                }
                UiEvent::Tab { screen, delta } if *screen == key => station_delta += delta,
                _ => {}
            }
        }
        if station_delta != 0 && st.job.is_none() {
            let n = Station::ALL.len() as i32;
            let i = (st.station.index() as i32 + station_delta).rem_euclid(n) as usize;
            st.station = Station::ALL[i];
            st.selected = None;
        }
        let inv = world
            .get::<Inventory>(st.owner)
            .cloned()
            .unwrap_or_default();
        if st.selected.is_none() {
            st.selected = first_pick(&book, st.station, &inv);
            st.quantity = 1;
        }
        let on_top = screens::open_screens(world)
            .into_iter()
            .find(|(.., modal)| *modal)
            .is_some_and(|(_, e, ..)| e == root);
        if on_top {
            let keys = world.get_resource::<ButtonInput<KeyCode>>();
            let pad = world.get_resource::<ButtonInput<GamepadButton>>();
            if keys.is_some_and(|k| k.just_pressed(KeyCode::Space))
                || pad.is_some_and(|b| b.just_pressed(GamepadButton::West))
            {
                go = true;
            }
        }
        // Keep the amount inside what the bag allows.
        let recipe = st.selected.as_deref().and_then(|id| book.get(id)).cloned();
        let max = recipe.as_ref().map_or(0, |r| r.can_make(&inv));
        st.quantity = st.quantity.clamp(1, max.max(1));
        if go && st.job.is_none() {
            match &recipe {
                Some(r) if max > 0 => {
                    st.job = Some(CraftJob {
                        recipe: r.id.clone(),
                        qty: st.quantity,
                        elapsed: 0.0,
                        total: job_time(r, st.quantity),
                    });
                }
                Some(r) => {
                    let db = world.resource::<ItemDatabase>().clone();
                    let why = if r.known {
                        r.missing(&inv, &db).unwrap_or_default()
                    } else {
                        "You don't know that recipe yet.".into()
                    };
                    toast::notify(world, ToastKind::Bad, "", "", &why, 2.4);
                }
                None => {}
            }
        }
        // The timer, then the craft itself.
        let mut finished = false;
        if let Some(j) = &mut st.job {
            j.elapsed += dt;
            if j.elapsed >= j.total {
                let (id, qty) = (j.recipe.clone(), j.qty);
                st.job = None;
                match craft(world, st.owner, &id, qty) {
                    Ok(m) => {
                        let icon = book
                            .get(&id)
                            .and_then(|r| world.resource::<ItemDatabase>().get(&r.result))
                            .map_or(String::new(), |d| d.icon.clone());
                        toast::notify(world, ToastKind::Loot, &icon, "Crafted", &m, 2.6);
                        st.flash = 1.0;
                        st.quantity = 1;
                    }
                    Err(e) => toast::notify(world, ToastKind::Bad, "", "", &e, 2.4),
                }
                finished = true;
            }
        }
        st.flash = (st.flash - dt * 1.6).max(0.0);
        let changed = before
            != (
                st.station,
                st.selected.clone(),
                st.quantity,
                st.job.is_some(),
            );
        let stale = snapshot(world, &st) != st.snapshot;
        let progress_text = st.job.as_ref().map(|j| {
            format!(
                "Crafting…  {:.0}%",
                (j.elapsed / j.total.max(1e-3) * 100.0).min(100.0)
            )
        });
        if let Some(mut s) = world.get_mut::<CraftingScreen>(root) {
            *s = CraftingScreen {
                snapshot: s.snapshot.clone(),
                ..st.clone()
            };
        }
        if changed || stale || finished {
            rebuild(world, root);
        } else if let Some(text) = progress_text
            && let Some(btn) = screens::find_in(world, root, "craft_go")
        {
            // The percentage on the button, without a rebuild.
            let label: Option<Entity> = world
                .get::<Children>(btn)
                .and_then(|c| c.iter().find(|c| world.get::<Text>(*c).is_some()));
            if let Some(l) = label
                && let Some(mut t) = world.get_mut::<Text>(l)
                && t.text != text
            {
                t.text = text;
            }
        }
        // Progress, the burst and the slow turn on the showcase.
        let progress = st
            .job
            .as_ref()
            .map(|j| (j.elapsed / j.total.max(1e-3)).min(1.0));
        if let Some(canvas) = screens::find_in(world, root, "craft_result")
            && let Some(mut v) = world.get_mut::<CraftView>(canvas)
        {
            v.time += dt;
            if v.progress != progress {
                v.progress = progress;
            }
            if v.flash != st.flash {
                v.flash = st.flash;
            }
        }
    }
}

// -------------------------------------------------------------------------------- demo

/// The demo recipes: three stations, some made from what the demo player carries, some
/// short of an ingredient, and one still to be learned.
pub fn demo_recipes() -> RecipeBook {
    let r =
        |id: &str, result: &str, count: u32, station: Station, time: f32, ings: &[(&str, u32)]| {
            Recipe {
                id: id.into(),
                result: result.into(),
                count,
                station,
                ingredients: ings
                    .iter()
                    .map(|(i, n)| Ingredient {
                        item: i.to_string(),
                        count: *n,
                    })
                    .collect(),
                time,
                known: true,
                hint: String::new(),
            }
        };
    let unknown = |mut r: Recipe, hint: &str| {
        r.known = false;
        r.hint = hint.into();
        r
    };
    use Station::*;
    RecipeBook {
        recipes: vec![
            r(
                "r_potion_health",
                "potion_health",
                2,
                Alchemy,
                1.2,
                &[("herb", 3)],
            ),
            r(
                "r_potion_mana",
                "potion_mana",
                1,
                Alchemy,
                1.4,
                &[("herb", 2), ("mushroom_glow", 1)],
            ),
            r(
                "r_potion_stamina",
                "potion_stamina",
                1,
                Alchemy,
                1.2,
                &[("herb", 2), ("bread", 1)],
            ),
            r(
                "r_medkit",
                "medkit",
                1,
                Alchemy,
                1.8,
                &[("herb", 4), ("potion_health", 1)],
            ),
            r(
                "r_elixir",
                "elixir",
                1,
                Alchemy,
                3.0,
                &[
                    ("crystal_glow", 2),
                    ("mushroom_glow", 4),
                    ("gem_emerald", 1),
                ],
            ),
            unknown(
                r(
                    "r_mushroom",
                    "mushroom_glow",
                    3,
                    Alchemy,
                    2.0,
                    &[("herb", 6), ("crystal_glow", 1)],
                ),
                "Taught by the herbalist in the forest",
            ),
            r("r_helmet", "helmet", 1, Smithing, 2.2, &[("ore_iron", 5)]),
            r(
                "r_dagger",
                "dagger",
                1,
                Smithing,
                2.0,
                &[("ore_iron", 3), ("gem_sapphire", 1)],
            ),
            r(
                "r_sword",
                "sword",
                1,
                Smithing,
                2.4,
                &[("ore_iron", 6), ("wood_log", 1)],
            ),
            r(
                "r_shield_small",
                "shield_small",
                1,
                Smithing,
                2.0,
                &[("ore_iron", 4), ("wood_log", 2)],
            ),
            r(
                "r_ring",
                "ring",
                1,
                Smithing,
                2.6,
                &[("ore_gold", 2), ("gem_ruby", 1)],
            ),
            unknown(
                r(
                    "r_hammer",
                    "hammer",
                    1,
                    Smithing,
                    3.0,
                    &[("ore_iron", 10), ("ore_gold", 2), ("wood_log", 2)],
                ),
                "Find the smith's notes in the siege fortress",
            ),
            r(
                "r_lockpick",
                "lockpick",
                2,
                Tinkering,
                0.8,
                &[("ore_iron", 1)],
            ),
            r(
                "r_grenade",
                "grenade",
                2,
                Tinkering,
                1.6,
                &[("ore_iron", 2), ("fuel_cell", 1)],
            ),
            r(
                "r_torch",
                "torch",
                3,
                Tinkering,
                0.8,
                &[("wood_log", 1), ("herb", 1)],
            ),
            r(
                "r_compass",
                "compass",
                1,
                Tinkering,
                2.6,
                &[("circuit", 1), ("crystal_glow", 1), ("ore_gold", 1)],
            ),
            unknown(
                r(
                    "r_rifle",
                    "rifle",
                    1,
                    Tinkering,
                    4.0,
                    &[
                        ("circuit", 2),
                        ("fuel_cell", 2),
                        ("ore_iron", 10),
                        ("crystal_glow", 1),
                    ],
                ),
                "A schematic sold at the spaceport",
            ),
        ],
    }
}

/// Put the demo recipes in place (unless a game already did).
pub fn add_demo_recipes(world: &mut World) {
    if world
        .get_resource::<RecipeBook>()
        .is_none_or(|b| b.recipes.is_empty())
    {
        world.insert_resource(demo_recipes());
    }
}

/// Register the `craft.*` API.
pub fn register_api(app: &mut engine_core::prelude::App) {
    app.register_api(
        "craft.recipes",
        "Get the recipe book (no args) or replace it (pass {recipes}). A recipe: {id, result (item id), count (made per craft), station (alchemy, smithing, tinkering), ingredients [{item, count}], time (s), known, hint (where an unknown one is learned)}.",
        r#"{"recipes": [{"id": "r_tonic", "result": "potion_stamina", "station": "alchemy", "ingredients": [{"item": "herb", "count": 2}]}]}"#,
        |world, args| {
            if args.get("recipes").is_some() {
                let book: RecipeBook = serde_json::from_value(args)
                    .map_err(|e| format!("That isn't a recipe book: {e}."))?;
                let n = book.recipes.len();
                world.insert_resource(book);
                return Ok(json!({"ok": true, "recipes": n}));
            }
            let book = world
                .get_resource::<RecipeBook>()
                .cloned()
                .unwrap_or_default();
            serde_json::to_value(&book).map_err(|e| e.to_string())
        },
    );
    app.register_api(
        "craft.make",
        "Craft at once, as the bench would when its timer ends: {id, count (crafts, default 1), owner (default Player)}. Fails with what is missing.",
        r#"{"id": "r_potion_health", "count": 2}"#,
        |world, args| {
            let owner = inventory::find_owner(world, &args)?;
            let id = args
                .get("id")
                .and_then(Value::as_str)
                .ok_or("Pass the recipe's \"id\".")?
                .to_string();
            let qty = args.get("count").and_then(Value::as_u64).unwrap_or(1) as u32;
            let msg = craft(world, owner, &id, qty)?;
            Ok(json!({"ok": true, "message": msg}))
        },
    );
    app.register_api(
        "craft.learn",
        "Teach a recipe: {id}. Returns learned false when it was already known.",
        r#"{"id": "r_rifle"}"#,
        |world, args| {
            let id = args
                .get("id")
                .and_then(Value::as_str)
                .ok_or("Pass the recipe's \"id\".")?
                .to_string();
            if world
                .get_resource::<RecipeBook>()
                .is_none_or(|b| b.get(&id).is_none())
            {
                return Err(format!("There is no recipe \"{id}\"."));
            }
            Ok(json!({"ok": true, "learned": learn(world, &id)}))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::player_inventory;

    #[test]
    fn demo_recipes_mix_craftable_short_and_unknown() {
        let db = ItemDatabase::default();
        let inv = player_inventory(&db);
        let book = demo_recipes();
        for r in &book.recipes {
            assert!(
                db.get(&r.result).is_some(),
                "{} makes an unknown item",
                r.id
            );
            for i in &r.ingredients {
                assert!(db.get(&i.item).is_some(), "{} needs an unknown item", r.id);
            }
        }
        for s in Station::ALL {
            assert!(
                book.at(s).any(|r| r.can_make(&inv) > 0),
                "{s:?} has nothing to make"
            );
            assert!(book.at(s).any(|r| !r.known), "{s:?} has nothing to learn");
        }
        // 23 Silverleaf make seven health brews; the sword needs an Oak Log.
        assert_eq!(book.get("r_potion_health").unwrap().can_make(&inv), 7);
        assert_eq!(
            book.get("r_sword").unwrap().missing(&inv, &db).as_deref(),
            Some("Missing 1 Oak Log")
        );
        assert_eq!(book.get("r_rifle").unwrap().can_make(&inv), 0);
    }

    #[test]
    fn removing_takes_from_small_stacks_first() {
        let db = ItemDatabase::default();
        let mut inv = Inventory::with_capacity(4);
        inv.slots[0] = Some(ItemStack::new("herb", 40));
        inv.slots[2] = Some(ItemStack::new("herb", 3));
        remove_items(&mut inv, "herb", 5);
        assert_eq!(inv.count_of("herb"), 38);
        assert!(inv.slots[2].is_none());
        assert_eq!(inv.add(&db, "herb", 1), 0);
    }
}
