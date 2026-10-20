//! Save and load: the game's save slots as cards with a painted view of where each was made,
//! the chapter, place, level, play time and when it was saved. Load one (after a confirm),
//! save into a new slot or over an old one, or delete one.
//!
//! ```ignore
//! ui::show(world, "saves", &json!({"mode": "save"}))?;
//! ```
//!
//! The slots live in the [`SaveSlots`] resource, which the game fills and keeps in step
//! (`saves.list` reads or replaces it). The screen never touches files: it sends
//! `game.load:<id>`, `game.save:<id>` and `game.delete_save:<id>` for the game to act on,
//! and updates the slots it shows.

use bevy_ecs::message::MessageCursor;
use bevy_ecs::prelude::*;
use engine_core::prelude::*;
use engine_input::{ButtonInput, GamepadButton, KeyCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::color::Color;
use crate::data::UiData;
use crate::draw::{DrawCtx, Painters};
use crate::el::{self, El};
use crate::input::UiEvent;
use crate::map::PlanetKind;
use crate::node::*;
use crate::screens;
use crate::style::{Align, Corners, Fill, Pick, pct};
use crate::theme::Theme;
use crate::toast::{self, ToastKind};
use crate::widgets;

// -------------------------------------------------------------------------------- slots

/// How a save was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveKind {
    Auto,
    Quick,
    #[default]
    Manual,
}

impl SaveKind {
    pub fn label(self) -> &'static str {
        match self {
            SaveKind::Auto => "Autosave",
            SaveKind::Quick => "Quicksave",
            SaveKind::Manual => "Save",
        }
    }
}

/// One saved game, as the menu shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SaveSlot {
    pub id: String,
    pub kind: SaveKind,
    /// The player's own name for a manual save (may be empty).
    pub name: String,
    pub character: String,
    pub chapter: String,
    pub location: String,
    /// What the card paints: valley, forest, village_night, snow, desert, orbit.
    pub scene: String,
    /// Varies the painting (hills, trees, stars).
    pub seed: f32,
    pub level: u32,
    pub playtime_s: u64,
    /// When, as shown ("Today 14:32").
    pub saved_at: String,
    /// The tracked quest's step at the time.
    pub objective: String,
}

impl Default for SaveSlot {
    fn default() -> Self {
        Self {
            id: String::new(),
            kind: SaveKind::Manual,
            name: String::new(),
            character: String::new(),
            chapter: String::new(),
            location: String::new(),
            scene: "valley".into(),
            seed: 0.0,
            level: 1,
            playtime_s: 0,
            saved_at: String::new(),
            objective: String::new(),
        }
    }
}

impl SaveSlot {
    /// "Autosave", "Quicksave", or the save's own name ("Save 3" when it has none).
    pub fn title(&self, index: usize) -> String {
        match self.kind {
            SaveKind::Manual if !self.name.is_empty() => self.name.clone(),
            SaveKind::Manual => format!("Save {}", index + 1),
            k => k.label().into(),
        }
    }
}

/// Every save the game has, newest first.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SaveSlots {
    pub slots: Vec<SaveSlot>,
    /// Most manual saves (autosave and quicksave don't count).
    pub max_manual: usize,
}

impl Default for SaveSlots {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            max_manual: 8,
        }
    }
}

impl SaveSlots {
    pub fn get(&self, id: &str) -> Option<&SaveSlot> {
        self.slots.iter().find(|s| s.id == id)
    }

    fn manual(&self) -> usize {
        self.slots
            .iter()
            .filter(|s| s.kind == SaveKind::Manual)
            .count()
    }

    fn index_of(&self, id: &str) -> usize {
        self.slots
            .iter()
            .filter(|s| s.kind == SaveKind::Manual)
            .position(|s| s.id == id)
            .unwrap_or(0)
    }
}

/// "12 h 41 m" or "38 m".
pub fn playtime(secs: u64) -> String {
    let (h, m) = (secs / 3600, secs / 60 % 60);
    if h > 0 {
        format!("{h} h {m} m")
    } else {
        format!("{m} m")
    }
}

/// A slot for the game as it is now, from [`UiData`] (`player.*`, `quest.*`, `location.*`).
pub fn current_slot(world: &World, id: &str) -> SaveSlot {
    let data = world.get_resource::<UiData>();
    let s = |p: &str| {
        data.and_then(|d| d.get_str(p))
            .unwrap_or_default()
            .to_string()
    };
    let n = |p: &str| data.and_then(|d| d.get_f64(p)).unwrap_or(0.0);
    SaveSlot {
        id: id.into(),
        kind: SaveKind::Manual,
        name: String::new(),
        character: s("player.name"),
        chapter: s("quest.title"),
        scene: Some(s("location.scene"))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "valley".into()),
        location: s("location.name"),
        seed: (n("player.playtime_s") as f32 * 0.37).fract() * 10.0,
        level: n("player.level") as u32,
        playtime_s: n("player.playtime_s") as u64,
        saved_at: "Just now".into(),
        objective: s("quest.step"),
    }
}

fn new_id(slots: &SaveSlots) -> String {
    (1..)
        .map(|i| format!("save_{i}"))
        .find(|id| slots.get(id).is_none())
        .unwrap_or_default()
}

/// Save the game as it is now into slot `id` (a new manual slot when `None`). Sends
/// `game.save:<id>` and returns the slot's id.
pub fn save(world: &mut World, id: Option<&str>) -> Result<String, String> {
    let mut slots = world
        .get_resource::<SaveSlots>()
        .cloned()
        .unwrap_or_default();
    let id = match id {
        Some(id) => id.to_string(),
        None => {
            if slots.manual() >= slots.max_manual {
                return Err(format!(
                    "Every save slot is used ({}). Save over an old one.",
                    slots.max_manual
                ));
            }
            new_id(&slots)
        }
    };
    let mut slot = current_slot(world, &id);
    if let Some(i) = slots.slots.iter().position(|s| s.id == id) {
        let old = slots.slots.remove(i);
        slot.kind = old.kind;
        slot.name = old.name;
    }
    let title = slot.title(slots.manual());
    // Newest first, after the autosave and quicksave.
    let at = slots
        .slots
        .iter()
        .position(|s| s.kind == SaveKind::Manual)
        .unwrap_or(slots.slots.len());
    let at = if slot.kind == SaveKind::Manual { at } else { 0 };
    slots.slots.insert(at, slot);
    world.insert_resource(slots);
    crate::input::dispatch(world, &format!("game.save:{id}"));
    toast::notify(
        world,
        ToastKind::Good,
        "scroll",
        "Game saved",
        &format!("[b]{title}[/b]"),
        2.4,
    );
    Ok(id)
}

/// Remove slot `id`. Sends `game.delete_save:<id>`.
pub fn delete(world: &mut World, id: &str) -> bool {
    let Some(mut slots) = world.get_resource::<SaveSlots>().cloned() else {
        return false;
    };
    let before = slots.slots.len();
    slots.slots.retain(|s| s.id != id);
    if slots.slots.len() == before {
        return false;
    }
    world.insert_resource(slots);
    crate::input::dispatch(world, &format!("game.delete_save:{id}"));
    true
}

/// Ask the game to load slot `id` (sends `game.load:<id>`) and close the menus.
pub fn load(world: &mut World, id: &str) -> Result<(), String> {
    let slot = world
        .get_resource::<SaveSlots>()
        .and_then(|s| s.get(id))
        .cloned()
        .ok_or_else(|| format!("There is no save \"{id}\"."))?;
    crate::input::dispatch(world, &format!("game.load:{id}"));
    screens::hide_all(world, &["hud"], false);
    toast::notify(
        world,
        ToastKind::Info,
        "map_scroll",
        "Loading",
        &format!("[b]{}[/b]  ·  {}", slot.chapter, slot.location),
        2.4,
    );
    Ok(())
}

// -------------------------------------------------------------------------------- thumbnails

/// A painted view of where a save was made (on the node with `paint: "save_thumb"`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SaveThumb {
    pub scene: String,
    pub seed: f32,
}

struct Scene {
    sky: [u32; 3],
    sun: Option<(f32, f32, u32)>,
    stars: f32,
    layers: [(f32, f32, u32); 3],
    snow: bool,
    trees: u32,
    water: Option<u32>,
    lights: u32,
}

fn scene(name: &str) -> Scene {
    match name {
        "forest" => Scene {
            sky: [0x2d2a52, 0x8a5a7a, 0xf0a070],
            sun: Some((0.72, 0.62, 0xffb070)),
            stars: 0.0,
            layers: [
                (0.55, 0.16, 0x4a4868),
                (0.66, 0.12, 0x2a3040),
                (0.80, 0.08, 0x141a22),
            ],
            snow: false,
            trees: 0x0c1218,
            water: None,
            lights: 0,
        },
        "village_night" => Scene {
            sky: [0x070b1a, 0x14203a, 0x2a3a5a],
            sun: Some((0.22, 0.22, 0xdfe8ff)),
            stars: 1.0,
            layers: [
                (0.52, 0.18, 0x1c2640),
                (0.68, 0.10, 0x121a2c),
                (0.82, 0.06, 0x0a0f1a),
            ],
            snow: true,
            trees: 0x070b12,
            water: Some(0x1a2a48),
            lights: 7,
        },
        "snow" => Scene {
            sky: [0x5a86c0, 0xa8c8e8, 0xe8f0f8],
            sun: Some((0.30, 0.30, 0xfff6e0)),
            stars: 0.0,
            layers: [
                (0.40, 0.26, 0x9ab0cc),
                (0.62, 0.14, 0x6a7e98),
                (0.80, 0.07, 0xdfe8f2),
            ],
            snow: true,
            trees: 0x203040,
            water: None,
            lights: 0,
        },
        "desert" => Scene {
            sky: [0x6a3a30, 0xc06a40, 0xf0b070],
            sun: Some((0.66, 0.48, 0xfff0c0)),
            stars: 0.0,
            layers: [
                (0.58, 0.10, 0xa05a3a),
                (0.70, 0.07, 0x7a3e28),
                (0.84, 0.05, 0x4a2418),
            ],
            snow: false,
            trees: 0,
            water: None,
            lights: 0,
        },
        // The valley at dawn: snowy peaks, a lake and the village waking up.
        _ => Scene {
            sky: [0x2a4a7a, 0x7a9ac0, 0xf6c890],
            sun: Some((0.62, 0.50, 0xffe0a8)),
            stars: 0.0,
            layers: [
                (0.46, 0.22, 0x6a7ea0),
                (0.62, 0.12, 0x3a5048),
                (0.78, 0.06, 0x1e2c22),
            ],
            snow: true,
            trees: 0x101c14,
            water: Some(0x5a7a9a),
            lights: 3,
        },
    }
}

/// A ridge line: a few sines, sharpened into peaks. `x` 0-1, returns 0-1 (1 = the top).
fn ridge(x: f32, seed: f32, freq: f32) -> f32 {
    let a = 1.0 - (x * freq * 3.1 + seed).sin().abs();
    let b = (x * freq * 7.3 + seed * 1.7).sin() * 0.5 + 0.5;
    let c = (x * freq * 17.0 + seed * 3.3).sin() * 0.5 + 0.5;
    (a * 0.62 + b * 0.26 + c * 0.12).clamp(0.0, 1.0)
}

fn hash(i: u32, seed: f32) -> f32 {
    let v = ((i as f32 * 12.9898 + seed * 78.233).sin() * 43_758.547).fract();
    v.abs()
}

fn paint_thumb(world: &mut World, e: Entity, ctx: &mut DrawCtx) {
    let Some(t) = world.get::<SaveThumb>(e).cloned() else {
        return;
    };
    let r = ctx.rect;
    let px = ctx.px;
    let hex = Color::hex;
    ctx.with_clip(r, |ctx| {
        if t.scene == "orbit" {
            paint_orbit(ctx, r, t.seed);
            return;
        }
        let sc = scene(&t.scene);
        // Sky: two bands, zenith to horizon.
        let mid = r.y + r.h * 0.42;
        ctx.rect_fill(
            Rect::new(r.x, r.y, r.w, mid - r.y + 1.0),
            Fill::Linear {
                from: hex(sc.sky[0]),
                to: hex(sc.sky[1]),
                angle_deg: 90.0,
            },
            [0.0; 4],
            0.0,
            Color::TRANSPARENT,
        );
        ctx.rect_fill(
            Rect::new(r.x, mid, r.w, (r.y + r.h) - mid),
            Fill::Linear {
                from: hex(sc.sky[1]),
                to: hex(sc.sky[2]),
                angle_deg: 90.0,
            },
            [0.0; 4],
            0.0,
            Color::TRANSPARENT,
        );
        if sc.stars > 0.0 {
            ctx.starfield(
                Rect::new(r.x, r.y, r.w, r.h * 0.55),
                Vec2::new(t.seed * 31.0, 0.0),
                sc.stars,
                t.seed,
                0.0,
            );
        }
        if let Some((sx, sy, col)) = sc.sun {
            let c = Vec2::new(r.x + r.w * sx, r.y + r.h * sy);
            ctx.glow(c, r.w * 0.45, hex(col).with_alpha(0.35), 1.2);
            ctx.glow(c, r.w * 0.12, hex(col).with_alpha(0.7), 1.6);
            ctx.rect(
                Rect::new(c.x - r.w * 0.03, c.y - r.w * 0.03, r.w * 0.06, r.w * 0.06),
                hex(col).lerp(Color::WHITE, 0.6),
                r.w * 0.03,
                0.0,
                Color::TRANSPARENT,
            );
        }
        // Hills, far to near, each a run of columns under its ridge, with haze between.
        let step = (2.5 * px).max(1.0);
        let cols = (r.w / step).ceil() as u32 + 1;
        for (li, (base, amp, col)) in sc.layers.iter().enumerate() {
            let freq = [1.6, 2.4, 3.4][li];
            let seed = t.seed * (li as f32 + 1.3) + li as f32 * 2.1;
            let color = hex(*col);
            for i in 0..cols {
                let x = r.x + i as f32 * step;
                let u = (x - r.x) / r.w;
                let top = r.y + r.h * (base - amp * ridge(u, seed, freq));
                ctx.line(
                    Vec2::new(x, top),
                    Vec2::new(x, r.y + r.h),
                    step + 0.6,
                    color,
                    0.0,
                );
                if sc.snow && li == 0 {
                    let peak = ridge(u, seed, freq);
                    if peak > 0.62 {
                        let cap = r.h * amp * (peak - 0.62) * 0.9;
                        ctx.line(
                            Vec2::new(x, top),
                            Vec2::new(x, top + cap),
                            step + 0.6,
                            hex(0xeef4fb).with_alpha(0.92),
                            0.0,
                        );
                    }
                }
            }
            // Haze in front of each far layer.
            if li < 2 {
                let y = r.y + r.h * (base - amp * 0.2);
                ctx.rect_fill(
                    Rect::new(r.x, y, r.w, (r.y + r.h) - y),
                    Fill::Linear {
                        from: hex(sc.sky[2]).with_alpha(0.0),
                        to: hex(sc.sky[2]).with_alpha(0.22),
                        angle_deg: 90.0,
                    },
                    [0.0; 4],
                    0.0,
                    Color::TRANSPARENT,
                );
            }
            // The lake sits between the middle hills and the near shore.
            if li == 1
                && let Some(w) = sc.water
            {
                let y = r.y + r.h * 0.70;
                ctx.rect_fill(
                    Rect::new(r.x + r.w * 0.18, y, r.w * 0.64, r.h * 0.07),
                    Fill::Linear {
                        from: hex(sc.sky[2]).lerp(hex(w), 0.4),
                        to: hex(w),
                        angle_deg: 90.0,
                    },
                    [r.h * 0.03; 4],
                    0.0,
                    Color::TRANSPARENT,
                );
                for k in 0..5 {
                    let gx = r.x + r.w * (0.3 + hash(k, t.seed) * 0.4);
                    let gy = y + r.h * (0.012 + k as f32 * 0.011);
                    ctx.line(
                        Vec2::new(gx, gy),
                        Vec2::new(gx + r.w * 0.05, gy),
                        1.0 * px,
                        Color::WHITE.with_alpha(0.25),
                        0.0,
                    );
                }
            }
        }
        // Pines along the near ridge.
        if sc.trees != 0 {
            let (base, amp, _) = sc.layers[2];
            let seed = t.seed * 3.3 + 4.2;
            let n = 9 + (hash(1, t.seed) * 6.0) as u32;
            for k in 0..n {
                let u = (k as f32 + hash(k + 7, t.seed) * 0.8) / n as f32;
                let gy = r.y + r.h * (base - amp * ridge(u, seed, 3.4)) + 2.0 * px;
                let h = r.h * (0.12 + hash(k + 31, t.seed) * 0.10);
                let w = h * 0.42;
                let cx = r.x + r.w * u;
                let rows = (h / (1.5 * px)).max(4.0) as u32;
                for j in 0..rows {
                    let f = j as f32 / rows as f32;
                    // Wider toward the bottom, in three tiers.
                    let tier = (f * 3.0).fract();
                    let half = w * 0.5 * (0.25 + f * 0.75) * (0.55 + tier * 0.45);
                    let y = gy - h + f * h;
                    ctx.line(
                        Vec2::new(cx - half, y),
                        Vec2::new(cx + half, y),
                        1.8 * px,
                        hex(sc.trees),
                        0.0,
                    );
                }
            }
        }
        // Warm windows in the village.
        for k in 0..sc.lights {
            let c = Vec2::new(
                r.x + r.w * (0.36 + hash(k + 50, t.seed) * 0.3),
                r.y + r.h * (0.74 + hash(k + 70, t.seed) * 0.06),
            );
            ctx.glow(c, 7.0 * px, hex(0xffb050).with_alpha(0.75), 2.0);
            ctx.rect(
                Rect::new(c.x - px, c.y - px, 2.0 * px, 2.0 * px),
                hex(0xffe0a0),
                0.0,
                0.0,
                Color::TRANSPARENT,
            );
        }
        // A soft vignette, like a photo.
        ctx.rect_fill(
            r,
            Fill::Radial {
                inner: Color::hexa(0x00000000),
                outer: Color::hexa(0x00000070),
            },
            [0.0; 4],
            0.0,
            Color::TRANSPARENT,
        );
    });
}

fn paint_orbit(ctx: &mut DrawCtx, r: Rect, seed: f32) {
    ctx.rect(r, Color::hex(0x03050c), 0.0, 0.0, Color::TRANSPARENT);
    ctx.starfield(r, Vec2::new(seed * 40.0, seed * 13.0), 1.0, seed, 0.0);
    // The home planet low in the frame, lit from the upper left.
    let c = Vec2::new(r.x + r.w * 0.62, r.y + r.h * 1.05);
    let radius = r.w * 0.62;
    ctx.glow(
        Vec2::new(r.x + r.w * 0.08, r.y + r.h * 0.12),
        r.w * 0.35,
        Color::hex(0xfff0d0).with_alpha(0.3),
        1.3,
    );
    ctx.planet(
        c,
        radius,
        PlanetKind::Terran.look(seed),
        seed,
        0.25,
        -2.2,
        0.5,
        0.05,
    );
}

pub fn add_painters(p: &mut Painters) {
    p.add("save_thumb", paint_thumb);
}

// -------------------------------------------------------------------------------- screen

/// Loading saves or saving the game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveMode {
    Load,
    Save,
}

/// The selection meaning "a new save" in save mode.
const NEW: &str = "+new";

/// The open saves screen (on its root).
#[derive(Component, Debug, Clone)]
pub struct SavesScreen {
    pub mode: SaveMode,
    pub selected: Option<String>,
    pub snapshot: Option<SaveSlots>,
}

fn thumb(scene: &str, seed: f32, w: f32, h: f32) -> El {
    El::new()
        .paint("save_thumb")
        .attach("SaveThumb", &json!({"scene": scene, "seed": seed}))
        .with_style(|s| {
            s.size(w, h)
                .shrink(0.0)
                .radius(Corners::all(6.0))
                .border(1.0, Color::WHITE.with_alpha(0.14))
                .pick(Pick::Ignore)
        })
}

fn kind_color(theme: &Theme, kind: SaveKind) -> Color {
    let p = &theme.palette;
    match kind {
        SaveKind::Auto => p.info,
        SaveKind::Quick => p.good,
        SaveKind::Manual => p.accent,
    }
}

fn slot_card(theme: &Theme, slots: &SaveSlots, s: &SaveSlot, selected: bool) -> El {
    let p = &theme.palette;
    let kc = kind_color(theme, s.kind);
    let title = s.title(slots.index_of(&s.id));
    El::new()
        .class("card")
        .id(format!("save:{}", s.id))
        .on_click(format!("saves.select:{}", s.id))
        .selected(selected)
        .focusable(true)
        .with_style(|st| {
            st.row()
                .align_items(Align::Center)
                .gap(16.0)
                .padding([10.0, 16.0, 10.0, 10.0])
                .width(pct(100.0))
        })
        .child(thumb(&s.scene, s.seed, 176.0, 99.0))
        .child(
            El::new()
                .class("col")
                .with_style(|st| st.gap(4.0).grow(1.0).shrink(1.0))
                .child(
                    El::text(title.to_uppercase())
                        .text_style(|t| t.bold().nowrap())
                        .with_style(|st| {
                            st.font_size(12.0).color(if selected {
                                kc.lerp(Color::WHITE, 0.55)
                            } else {
                                kc
                            })
                        })
                        .pick_ignore(),
                )
                .child(
                    El::text(s.chapter.clone())
                        .text_style(|t| t.semibold().nowrap())
                        .with_style(|st| st.font_size(17.0).color(Color::WHITE))
                        .pick_ignore(),
                )
                .child(
                    El::text(format!("{}  ·  Level {}", s.location, s.level))
                        .text_style(|t| t.nowrap())
                        .with_style(|st| {
                            st.font_size(13.0).color(if selected {
                                Color::WHITE.with_alpha(0.85)
                            } else {
                                p.text_dim
                            })
                        })
                        .pick_ignore(),
                ),
        )
        .child(
            El::new()
                .class("col")
                .with_style(|st| st.gap(4.0).align_items(Align::End))
                .child(
                    El::text(s.saved_at.clone())
                        .text_style(|t| t.semibold().nowrap())
                        .with_style(|st| st.font_size(13.0))
                        .pick_ignore(),
                )
                .child(
                    El::text(playtime(s.playtime_s))
                        .text_style(|t| t.nowrap().tabular())
                        .with_style(|st| {
                            st.font_size(12.0).color(if selected {
                                Color::WHITE.with_alpha(0.8)
                            } else {
                                p.text_faint
                            })
                        })
                        .pick_ignore(),
                ),
        )
}

fn new_card(world: &World, slots: &SaveSlots, selected: bool) -> El {
    let theme = world.resource::<Theme>();
    let p = &theme.palette;
    let now = current_slot(world, "");
    let full = slots.manual() >= slots.max_manual;
    El::new()
        .class("card")
        .id("save:new")
        .on_click(format!("saves.select:{NEW}"))
        .selected(selected)
        .focusable(true)
        .with_style(|st| {
            st.row()
                .align_items(Align::Center)
                .gap(16.0)
                .padding([10.0, 16.0, 10.0, 10.0])
                .width(pct(100.0))
                .border(1.0, p.accent.with_alpha(0.45))
        })
        .child(
            El::new()
                .with_style(|st| {
                    st.size(176.0, 99.0)
                        .shrink(0.0)
                        .radius(Corners::all(6.0))
                        .border(1.5, p.accent.with_alpha(0.5))
                        .bg(p.accent.with_alpha(0.08))
                        .center()
                })
                .child(
                    El::icon("ui_plus")
                        .icon_tint(p.accent_light)
                        .with_style(|st| st.size(30.0, 30.0)),
                )
                .pick_ignore(),
        )
        .child(
            El::new()
                .class("col")
                .with_style(|st| st.gap(4.0).grow(1.0).shrink(1.0))
                .child(
                    El::text("NEW SAVE")
                        .text_style(|t| t.bold().nowrap())
                        .with_style(|st| {
                            st.font_size(12.0).color(if selected {
                                Color::WHITE
                            } else {
                                p.accent_light
                            })
                        })
                        .pick_ignore(),
                )
                .child(
                    El::text(now.chapter.clone())
                        .text_style(|t| t.semibold().nowrap())
                        .with_style(|st| st.font_size(17.0).color(Color::WHITE))
                        .pick_ignore(),
                )
                .child(
                    El::text(if full {
                        format!("All {} slots used", slots.max_manual)
                    } else {
                        format!(
                            "{} of {} slots free",
                            slots.max_manual - slots.manual(),
                            slots.max_manual
                        )
                    })
                    .text_style(|t| t.nowrap())
                    .with_style(|st| {
                        st.font_size(13.0).color(if selected {
                            Color::WHITE.with_alpha(0.85)
                        } else if full {
                            p.bad
                        } else {
                            p.text_dim
                        })
                    })
                    .pick_ignore(),
                ),
        )
}

fn detail_row(label: &str, value: String) -> El {
    widgets::stat_row(label, &value, None)
}

fn details(world: &World, st: &SavesScreen) -> El {
    let theme = world.resource::<Theme>().clone();
    let p = theme.palette.clone();
    let slots = world
        .get_resource::<SaveSlots>()
        .cloned()
        .unwrap_or_default();
    let mut panel = El::new()
        .class("panel col")
        .id("saves_details")
        .with_style(|s| s.width(520.0).gap(14.0));
    let is_new = st.selected.as_deref() == Some(NEW);
    let slot = if is_new {
        Some(current_slot(world, NEW))
    } else {
        st.selected.as_deref().and_then(|id| slots.get(id)).cloned()
    };
    let Some(s) = slot else {
        return panel.child(
            El::text(if slots.slots.is_empty() {
                "No saved games yet."
            } else {
                "Pick a save."
            })
            .class("dim"),
        );
    };
    let kc = kind_color(&theme, s.kind);
    let title = if is_new {
        "New save".to_string()
    } else {
        s.title(slots.index_of(&s.id))
    };
    panel = panel
        .child(thumb(&s.scene, s.seed, 484.0, 272.0))
        .child(
            El::new()
                .class("col")
                .with_style(|x| x.gap(4.0))
                .child(
                    El::text(title.to_uppercase())
                        .text_style(|t| t.bold().nowrap())
                        .with_style(|x| x.font_size(12.0).color(kc)),
                )
                .child(
                    El::text(s.chapter.clone())
                        .class("title")
                        .text_style(|t| t.shadow()),
                )
                .child(
                    El::text(s.objective.clone())
                        .class("body")
                        .with_style(|x| x.color(p.text_dim)),
                ),
        )
        .child(widgets::divider())
        .child(detail_row("Character", s.character.clone()))
        .child(detail_row("Location", s.location.clone()))
        .child(detail_row("Level", s.level.to_string()))
        .child(detail_row("Play time", playtime(s.playtime_s)))
        .child(detail_row(
            "Saved",
            if is_new {
                "Not yet".into()
            } else {
                s.saved_at.clone()
            },
        ))
        .child(El::new().class("spacer"));

    let full = slots.manual() >= slots.max_manual;
    let (label, enabled) = match (st.mode, is_new) {
        (SaveMode::Load, _) => ("Load".to_string(), true),
        (SaveMode::Save, true) => ("Save here".to_string(), !full),
        (SaveMode::Save, false) => ("Save over this".to_string(), true),
    };
    let main = El::button(label, "saves.go")
        .class("primary")
        .id("saves_go")
        .disabled(!enabled)
        .with_style(|x| x.grow(1.0).height(50.0));
    let mut row = El::new()
        .class("row")
        .with_style(|x| x.gap(10.0).align_items(Align::Center));
    if !is_new {
        row = row.child(
            El::new()
                .class("button ghost")
                .id("saves_delete")
                .on_click("saves.delete")
                .tooltip("", "Delete this save [color=#8a94a8](Del / Y)[/color]")
                .with_style(|x| x.height(50.0).padding([0.0, 18.0]))
                .child(
                    El::icon("ui_trash")
                        .icon_tint(p.bad.lerp(Color::WHITE, 0.2))
                        .with_style(|x| x.size(18.0, 18.0))
                        .pick_ignore(),
                ),
        );
    }
    panel.child(row.child(main))
}

fn hints(mode: SaveMode) -> El {
    El::new()
        .class("hud_panel row")
        .with_style(|s| {
            s.gap(22.0)
                .padding([10.0, 18.0])
                .radius(Corners::all(999.0))
        })
        .child(widgets::key_hint("LMB", "A", "Select"))
        .child(widgets::key_hint(
            "Enter",
            "X",
            if mode == SaveMode::Load {
                "Load"
            } else {
                "Save"
            },
        ))
        .child(widgets::key_hint("Del", "Y", "Delete"))
        .child(widgets::key_hint2(["Q", "E"], ["LB", "RB"], "Load / Save"))
        .child(widgets::key_hint("Esc", "B", "Close"))
}

fn build(world: &World, st: &SavesScreen) -> El {
    let theme = world.resource::<Theme>().clone();
    let p = theme.palette.clone();
    let slots = world
        .get_resource::<SaveSlots>()
        .cloned()
        .unwrap_or_default();
    let data = world.get_resource::<UiData>();
    let s = |k: &str| {
        data.and_then(|d| d.get_str(k))
            .unwrap_or_default()
            .to_string()
    };
    let level = data.and_then(|d| d.get_f64("player.level")).unwrap_or(1.0);
    let played = data
        .and_then(|d| d.get_f64("player.playtime_s"))
        .unwrap_or(0.0) as u64;
    let top = El::new()
        .class("row")
        .id("saves_top")
        .with_style(|x| x.gap(16.0).width(1290.0).padding([0.0, 4.0]))
        .child(
            El::new().class("header_icon").child(
                El::icon(if st.mode == SaveMode::Load {
                    "map_scroll"
                } else {
                    "scroll"
                })
                .with_style(|x| x.size(26.0, 26.0))
                .pick_ignore(),
            ),
        )
        .child(
            El::new()
                .class("col")
                .with_style(|x| x.gap(2.0))
                .child(
                    El::text(if st.mode == SaveMode::Load {
                        "Load Game"
                    } else {
                        "Save Game"
                    })
                    .class("title")
                    .text_style(|t| t.shadow()),
                )
                .child(
                    El::text(format!(
                        "{}  ·  Level {level}  ·  {} played",
                        s("player.name"),
                        playtime(played)
                    ))
                    .text_style(|t| t.nowrap().shadow())
                    .with_style(|x| x.font_size(13.0).font_weight(600).color(p.info)),
                ),
        )
        .child(El::new().class("spacer"))
        .child(widgets::tabs(
            &["Load", "Save"],
            (st.mode == SaveMode::Save) as usize,
            "saves.mode",
        ))
        .child(widgets::close_button());

    let mut list = El::new()
        .class("col")
        .id("saves_list")
        .with_style(|x| x.gap(8.0).width(pct(100.0)));
    if st.mode == SaveMode::Save {
        list = list.child(new_card(world, &slots, st.selected.as_deref() == Some(NEW)));
    }
    for slot in &slots.slots {
        // Autosaves can be loaded, not written.
        if st.mode == SaveMode::Save && slot.kind == SaveKind::Auto {
            continue;
        }
        list = list.child(slot_card(
            &theme,
            &slots,
            slot,
            st.selected.as_deref() == Some(slot.id.as_str()),
        ));
    }
    if slots.slots.is_empty() && st.mode == SaveMode::Load {
        list = list.child(El::text("No saved games yet.").class("dim"));
    }
    let left = El::new()
        .class("panel col")
        .id("saves_slots")
        .with_style(|x| x.grow(1.0).gap(10.0))
        .child(
            El::new()
                .class("row")
                .child(
                    El::text(if st.mode == SaveMode::Load {
                        "SAVED GAMES"
                    } else {
                        "SAVE INTO"
                    })
                    .class("label_caps"),
                )
                .child(El::new().class("spacer"))
                .child(
                    El::text(format!(
                        "{} saves  ·  {} of {} slots used",
                        slots.slots.len(),
                        slots.manual(),
                        slots.max_manual
                    ))
                    .text_style(|t| t.nowrap())
                    .with_style(|x| x.font_size(13.0).color(p.text_dim)),
                ),
        )
        .child(
            El::new()
                .class("scroll")
                .id("saves_scroll")
                .with_style(|x| x.grow(1.0).padding([2.0, 6.0, 2.0, 2.0]))
                .child(list),
        );

    El::new()
        .with_style(|x| {
            x.fill_parent()
                .column()
                .center()
                .gap(16.0)
                .pick(Pick::Ignore)
        })
        .child(top)
        .child(
            El::new()
                .class("row")
                .with_style(|x| {
                    x.gap(18.0)
                        .align_items(Align::Stretch)
                        .height(640.0)
                        .width(1290.0)
                })
                .child(left)
                .child(details(world, st)),
        )
        .child(hints(st.mode))
}

fn first_pick(slots: &SaveSlots, mode: SaveMode) -> Option<String> {
    match mode {
        SaveMode::Save => Some(NEW.into()),
        SaveMode::Load => slots.slots.first().map(|s| s.id.clone()),
    }
}

/// Builder for `ui.show {"screen": "saves"}`.
pub fn build_screen(world: &mut World, args: &Value) -> Result<Entity, String> {
    let slots = world
        .get_resource::<SaveSlots>()
        .cloned()
        .unwrap_or_default();
    let mode = if args.get("mode").and_then(Value::as_str) == Some("save") {
        SaveMode::Save
    } else {
        SaveMode::Load
    };
    let selected = args
        .get("select")
        .and_then(Value::as_str)
        .filter(|id| slots.get(id).is_some())
        .map(str::to_string)
        .or_else(|| first_pick(&slots, mode));
    let st = SavesScreen {
        mode,
        selected,
        snapshot: Some(slots),
    };
    let el = El::new()
        .class("screen")
        .child(El::new().with_style(|_| widgets::scrim(0.74)))
        .child(build(world, &st));
    let sp = screens::spawn_screen(
        world,
        Screen {
            modal: true,
            layer: 22,
            ..Default::default()
        },
        &el,
    )?;
    let root = sp.root.ok_or("empty")?;
    world.entity_mut(root).insert(st);
    Ok(root)
}

fn rebuild(world: &mut World, root: Entity) {
    let Some(st) = world.get::<SavesScreen>(root).cloned() else {
        return;
    };
    let scroll = screens::find_in(world, root, "saves_scroll")
        .and_then(|e| world.get::<ScrollState>(e).copied());
    let content = build(world, &st);
    let children: Vec<Entity> = world
        .get::<Children>(root)
        .map(|c| c.iter().collect())
        .unwrap_or_default();
    for c in children.iter().skip(1) {
        if let Ok(e) = world.get_entity_mut(*c) {
            e.despawn();
        }
    }
    let _ = el::spawn(world, Some(root), &content);
    if let (Some(s), Some(e)) = (scroll, screens::find_in(world, root, "saves_scroll")) {
        world.entity_mut(e).insert(s);
    }
    let snap = world.get_resource::<SaveSlots>().cloned();
    if let Some(mut s) = world.get_mut::<SavesScreen>(root) {
        s.snapshot = snap;
    }
}

fn confirm(world: &mut World, title: &str, text: &str, ok: &str, action: String, danger: bool) {
    let args =
        json!({"title": title, "text": text, "confirm": ok, "action": action, "danger": danger});
    if let Err(e) = screens::show(world, "confirm", &args) {
        toast::notify(world, ToastKind::Bad, "", "", &e, 2.4);
    }
}

/// Save, load and delete actions (also after their confirms) and the open screens' input.
pub fn saves_system(world: &mut World, mut seen: Local<MessageCursor<UiEvent>>) {
    let events: Vec<UiEvent> = crate::input::read_events(world, &mut seen);
    // Confirmed actions arrive from the confirm dialog, wherever it was opened.
    for ev in &events {
        let UiEvent::Action { action, .. } = ev else {
            continue;
        };
        let r = if let Some(id) = action.strip_prefix("saves.load_now:") {
            load(world, id)
        } else if let Some(id) = action.strip_prefix("saves.save_now:") {
            save(world, (id != NEW).then_some(id)).map(|_| ())
        } else if let Some(id) = action.strip_prefix("saves.delete_now:") {
            delete(world, id);
            Ok(())
        } else {
            continue;
        };
        if let Err(e) = r {
            toast::notify(world, ToastKind::Bad, "", "", &e, 2.4);
        }
    }
    let mut q = world.query::<(Entity, &SavesScreen, &Screen)>();
    let open: Vec<(Entity, SavesScreen, String)> = q
        .iter(world)
        .filter(|(_, _, s)| !s.closing)
        .map(|(e, st, s)| (e, st.clone(), s.key.clone()))
        .collect();
    for (root, mut st, key) in open {
        let before = (st.mode, st.selected.clone());
        let mut go = false;
        let mut del = false;
        for ev in &events {
            match ev {
                UiEvent::Action { action, entity, .. } => {
                    if entity.is_some_and(|e| !screens::is_descendant(world, e, root)) {
                        continue;
                    }
                    if let Some(id) = action.strip_prefix("saves.select:") {
                        st.selected = Some(id.to_string());
                    } else if let Some(m) = action.strip_prefix("saves.mode:") {
                        st.mode = if m == "1" {
                            SaveMode::Save
                        } else {
                            SaveMode::Load
                        };
                    } else if action == "saves.go" {
                        go = true;
                    } else if action == "saves.delete" {
                        del = true;
                    }
                }
                UiEvent::Tab { screen, .. } if *screen == key => {
                    st.mode = if st.mode == SaveMode::Load {
                        SaveMode::Save
                    } else {
                        SaveMode::Load
                    };
                }
                _ => {}
            }
        }
        let on_top = screens::open_screens(world)
            .into_iter()
            .find(|(.., modal)| *modal)
            .is_some_and(|(_, e, ..)| e == root);
        if on_top {
            let keys = world.get_resource::<ButtonInput<KeyCode>>();
            let pad = world.get_resource::<ButtonInput<GamepadButton>>();
            if keys.is_some_and(|k| k.just_pressed(KeyCode::Enter))
                || pad.is_some_and(|b| b.just_pressed(GamepadButton::West))
            {
                go = true;
            }
            if keys.is_some_and(|k| k.just_pressed(KeyCode::Delete))
                || pad.is_some_and(|b| b.just_pressed(GamepadButton::North))
            {
                del = true;
            }
        }
        let slots = world
            .get_resource::<SaveSlots>()
            .cloned()
            .unwrap_or_default();
        // A mode change or a deleted slot picks again.
        let valid = match st.selected.as_deref() {
            Some(NEW) => st.mode == SaveMode::Save,
            Some(id) => slots
                .get(id)
                .is_some_and(|s| st.mode == SaveMode::Load || s.kind != SaveKind::Auto),
            None => false,
        };
        if !valid || st.mode != before.0 {
            st.selected = first_pick(&slots, st.mode);
        }
        let sel = st.selected.clone();
        if go && let Some(id) = &sel {
            match st.mode {
                SaveMode::Load => {
                    if let Some(s) = slots.get(id) {
                        confirm(
                            world,
                            "Load this save?",
                            &format!(
                                "{}  ·  {}. Progress since your last save will be lost.",
                                s.chapter, s.location
                            ),
                            "Load",
                            format!("saves.load_now:{id}"),
                            false,
                        );
                    }
                }
                SaveMode::Save if id == NEW => {
                    if let Err(e) = save(world, None) {
                        toast::notify(world, ToastKind::Bad, "", "", &e, 2.4);
                    } else if let Some(first) = world
                        .get_resource::<SaveSlots>()
                        .and_then(|s| s.slots.iter().find(|s| s.kind == SaveKind::Manual))
                    {
                        st.selected = Some(first.id.clone());
                    }
                }
                SaveMode::Save => {
                    let title = slots
                        .get(id)
                        .map(|s| s.title(slots.index_of(id)))
                        .unwrap_or_default();
                    confirm(
                        world,
                        &format!("Save over {title}?"),
                        "The game saved there is replaced by the game as it is now.",
                        "Save over",
                        format!("saves.save_now:{id}"),
                        true,
                    );
                }
            }
        }
        if del && let Some(id) = sel.filter(|id| id != NEW) {
            let title = slots
                .get(&id)
                .map(|s| s.title(slots.index_of(&id)))
                .unwrap_or_default();
            confirm(
                world,
                &format!("Delete {title}?"),
                "This save is gone for good.",
                "Delete",
                format!("saves.delete_now:{id}"),
                true,
            );
        }
        if let Some(mut s) = world.get_mut::<SavesScreen>(root) {
            s.mode = st.mode;
            s.selected = st.selected.clone();
        }
        let stale = world.get_resource::<SaveSlots>().cloned() != st.snapshot;
        if stale || before != (st.mode, st.selected.clone()) {
            rebuild(world, root);
        }
    }
}

// -------------------------------------------------------------------------------- demo

/// Six saves along the demo's story, in different places and times of day.
pub fn demo_saves() -> SaveSlots {
    let s = |id: &str,
             kind: SaveKind,
             name: &str,
             chapter: &str,
             location: &str,
             scene: &str,
             seed: f32,
             level: u32,
             playtime_s: u64,
             saved_at: &str,
             objective: &str| SaveSlot {
        id: id.into(),
        kind,
        name: name.into(),
        character: "Mara Voss".into(),
        chapter: chapter.into(),
        location: location.into(),
        scene: scene.into(),
        seed,
        level,
        playtime_s,
        saved_at: saved_at.into(),
        objective: objective.into(),
    };
    use SaveKind::*;
    SaveSlots {
        slots: vec![
            s(
                "auto",
                Auto,
                "",
                "The Silent Relay",
                "Hollow Reach",
                "valley",
                2.3,
                14,
                45_660,
                "Today 14:32",
                "Reach the relay tower on Veyra's northern ridge",
            ),
            s(
                "quick",
                Quick,
                "",
                "The Silent Relay",
                "Brannoc's Market",
                "village_night",
                5.1,
                14,
                44_880,
                "Today 14:19",
                "Buy a climbing rope before the ridge",
            ),
            s(
                "save_3",
                Manual,
                "Before the ridge",
                "The Silent Relay",
                "Pinewatch Forest",
                "forest",
                7.7,
                13,
                41_220,
                "Yesterday 22:05",
                "Follow the old trail north through the pines",
            ),
            s(
                "save_2",
                Manual,
                "",
                "Salt and Iron",
                "Dune Sea Outpost",
                "desert",
                3.9,
                11,
                33_600,
                "Mon 19:48",
                "Deliver the fuel cells to the outpost",
            ),
            s(
                "save_4",
                Manual,
                "Summit",
                "Whiteout",
                "Frostspine Pass",
                "snow",
                9.2,
                9,
                26_400,
                "Sat 16:10",
                "Cross the pass before the storm",
            ),
            s(
                "save_1",
                Manual,
                "",
                "Landfall",
                "Orbit above Veyra",
                "orbit",
                1.4,
                3,
                6_120,
                "Oct 12",
                "Land at the Hollow Reach spaceport",
            ),
        ],
        max_manual: 8,
    }
}

/// Put the demo saves in place (unless a game already did).
pub fn add_demo_saves(world: &mut World) {
    if world
        .get_resource::<SaveSlots>()
        .is_none_or(|s| s.slots.is_empty())
    {
        world.insert_resource(demo_saves());
    }
}

/// Register the `saves.*` API.
pub fn register_api(app: &mut engine_core::prelude::App) {
    app.register_api(
        "saves.list",
        "Get the save slots (no args) or replace them (pass {slots, max_manual}). A slot: {id, kind (auto, quick, manual), name, character, chapter, location, scene (valley, forest, village_night, snow, desert, orbit), seed, level, playtime_s, saved_at (shown as is), objective}.",
        r#"{"slots": [{"id": "auto", "kind": "auto", "chapter": "Landfall", "location": "Hollow Reach", "scene": "valley", "level": 2, "playtime_s": 900, "saved_at": "Today 09:12"}]}"#,
        |world, args| {
            if args.get("slots").is_some() {
                let slots: SaveSlots = serde_json::from_value(args)
                    .map_err(|e| format!("Those aren't save slots: {e}."))?;
                let n = slots.slots.len();
                world.insert_resource(slots);
                return Ok(json!({"ok": true, "slots": n}));
            }
            let slots = world
                .get_resource::<SaveSlots>()
                .cloned()
                .unwrap_or_default();
            serde_json::to_value(&slots).map_err(|e| e.to_string())
        },
    );
    app.register_api(
        "saves.save",
        "Save as the menu would, without asking: {id} saves over that slot, no id makes a new one. Sends game.save:<id>.",
        "{}",
        |world, args| {
            let id = args.get("id").and_then(Value::as_str).map(str::to_string);
            let id = save(world, id.as_deref())?;
            Ok(json!({"ok": true, "id": id}))
        },
    );
    app.register_api(
        "saves.delete",
        "Delete a save without asking: {id}. Sends game.delete_save:<id>.",
        r#"{"id": "save_2"}"#,
        |world, args| {
            let id = args
                .get("id")
                .and_then(Value::as_str)
                .ok_or("Pass the save's \"id\".")?
                .to_string();
            Ok(json!({"ok": delete(world, &id)}))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_playtime_and_new_ids() {
        let slots = demo_saves();
        assert_eq!(slots.get("auto").unwrap().title(0), "Autosave");
        assert_eq!(slots.get("save_3").unwrap().title(0), "Before the ridge");
        let i = slots.index_of("save_2");
        assert_eq!(slots.get("save_2").unwrap().title(i), "Save 2");
        assert_eq!(playtime(45_660), "12 h 41 m");
        assert_eq!(playtime(2_280), "38 m");
        assert_eq!(new_id(&slots), "save_5");
    }

    #[test]
    fn ridges_stay_in_range() {
        for i in 0..200 {
            let v = ridge(i as f32 / 200.0, 3.7, 2.4);
            assert!((0.0..=1.0).contains(&v));
        }
    }
}
